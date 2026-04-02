use solana_mev_bot::{config, executor, mempool, router, state};

use anyhow::Result;
use crossbeam_channel::bounded;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{info, warn, error};

use config::BotConfig;
use executor::{BundleBuilder, MultiRelay};
use mempool::{GeyserStream, PoolStateChange};
use router::pool::DetectedSwap;
use router::{RouteCalculator, ProfitSimulator};
use router::simulator::SimulationResult;
use state::StateCache;

/// Channel capacity for pool state changes from Geyser.
/// Keep small — we want backpressure if the router can't keep up.
/// A backed-up channel means we're too slow and stale events are worthless anyway.
const STATE_CHANGE_CHANNEL_CAPACITY: usize = 256;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("solana_mev_bot=debug".parse()?)
                .add_directive("info".parse()?),
        )
        .with_target(true)
        .with_thread_ids(true)
        .json()
        .init();

    info!("=== Solana MEV Backrun Arbitrage Engine ===");
    info!("Halal-compliant: spot arb + JIT liquidity only");

    // Load config
    let config = Arc::new(BotConfig::from_env()?);

    if config.dry_run {
        warn!("DRY RUN MODE — opportunities will be logged but not submitted");
    }

    info!(
        "Config: tip_fraction={:.0}%, min_profit={} lamports, max_hops={}",
        config.tip_fraction * 100.0,
        config.min_profit_lamports,
        config.max_hops,
    );

    // Initialize shared state cache
    let state_cache = StateCache::new(config.pool_state_ttl);

    // Initialize HTTP client (shared for RPC calls)
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .pool_max_idle_per_host(4)
        .build()?;

    // Initialize blockhash cache and do first fetch
    let blockhash_cache = state::BlockhashCache::new();
    if let Err(e) = state::blockhash::fetch_and_update(&http_client, &config.rpc_url, &blockhash_cache).await {
        warn!("Initial blockhash fetch failed (will retry in background): {}", config::redact_url(&e.to_string()));
    } else {
        info!("Initial blockhash fetched");
    }

    // Bootstrap Sanctum virtual pools for LST arb
    if config.lst_arb_enabled {
        bootstrap_sanctum_pools(&state_cache);
        info!("LST arb enabled: {} Sanctum virtual pools bootstrapped", config::lst_mints().len());
    }

    // Shutdown signal
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Ctrl+C handler
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("Shutdown signal received");
        let _ = shutdown_tx_clone.send(true);
    });

    // Task: Blockhash refresh (async, 2s interval)
    let blockhash_handle = {
        let client = http_client.clone();
        let rpc_url = config.rpc_url.clone();
        let cache = blockhash_cache.clone();
        let shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            state::blockhash::run_blockhash_loop(client, rpc_url, cache, shutdown_rx).await;
        })
    };

    // Channel for pool state changes: Geyser stream → router
    let (change_tx, change_rx) = bounded(STATE_CHANGE_CHANNEL_CAPACITY);

    // Initialize components
    let geyser_stream = GeyserStream::new(config.clone(), state_cache.clone(), http_client.clone());
    let route_calculator = RouteCalculator::new(state_cache.clone(), config.max_hops);
    let profit_simulator = ProfitSimulator::new(
        state_cache.clone(),
        config.tip_fraction,
        config.min_profit_lamports,
    );
    let multi_relay = Arc::new(MultiRelay::new(config.clone()));

    // Load searcher keypair
    let searcher_keypair = load_keypair(&config.searcher_keypair_path)?;
    let bundle_builder = Arc::new(BundleBuilder::new(searcher_keypair, state_cache.clone()));

    // Warm up relay connections (pre-establish TCP+TLS+HTTP2)
    multi_relay.warmup().await;

    info!("All components initialized, starting pipeline...");

    // === Pipeline (post-mempool architecture) ===
    //
    // Geyser stream (async) → Channel → State update + Route calc (sync, CPU-bound)
    //   → Simulate → Bundle → Multi-relay fan-out (async)
    //
    // Old flow (dead): Jito mempool → decode pending swap → backrun same bundle
    // New flow: Geyser vault change → update reserves → detect price dislocation → arb next slot
    //
    // The router runs on a dedicated thread to avoid async overhead on
    // the hot path. Route calculation is pure CPU work — no I/O, no awaits.

    // Task 1: Geyser streaming with reconnect (async, I/O bound)
    let stream_handle = {
        let shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            let mut backoff = std::time::Duration::from_secs(1);
            const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(30);

            loop {
                match geyser_stream.start(change_tx.clone(), shutdown_rx.clone()).await {
                    Ok(()) => {
                        info!("Geyser stream ended cleanly");
                        backoff = std::time::Duration::from_secs(1); // reset on clean exit
                    }
                    Err(e) => {
                        error!("Geyser stream error: {}", config::redact_url(&e.to_string()));
                    }
                }

                if *shutdown_rx.borrow() {
                    info!("Geyser: shutdown requested, not reconnecting");
                    break;
                }

                warn!("Geyser disconnected, reconnecting in {:?}...", backoff);
                tokio::time::sleep(backoff).await;

                if *shutdown_rx.borrow() {
                    break;
                }

                info!("Geyser: attempting reconnect (backoff {:?})...", backoff);
                backoff = std::cmp::min(backoff * 2, MAX_BACKOFF);
            }
        })
    };

    // Task 2: Route calculation + simulation + submission
    // Runs as a blocking task on a dedicated thread
    let router_handle = {
        let shutdown_rx = shutdown_rx.clone();
        let multi_relay = multi_relay.clone();
        let bundle_builder = bundle_builder.clone();
        let config = config.clone();
        let state_cache = state_cache.clone();
        let blockhash_cache = blockhash_cache.clone();

        tokio::task::spawn_blocking(move || {
            info!("Router thread started");
            let mut opportunities_found: u64 = 0;
            let mut bundles_submitted: u64 = 0;

            // Create a tokio runtime handle for async relay submission from sync context.
            // The relay fan-out is async (HTTP calls), but the router loop is sync.
            let rt = tokio::runtime::Handle::current();

            let mut recent_pools: std::collections::HashMap<solana_sdk::pubkey::Pubkey, u64> = std::collections::HashMap::new();

            loop {
                // Check shutdown
                if *shutdown_rx.borrow() {
                    break;
                }

                // Receive pool state change from Geyser (timeout to check shutdown)
                let change: PoolStateChange = match change_rx
                    .recv_timeout(std::time::Duration::from_millis(100))
                {
                    Ok(c) => c,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                };

                // Dedup: skip if we already processed this pool in this slot
                if recent_pools.get(&change.pool_address) == Some(&change.slot) {
                    continue;
                }
                recent_pools.insert(change.pool_address, change.slot);

                // Evict old entries periodically
                if recent_pools.len() > 10_000 {
                    let current_slot = change.slot;
                    recent_pools.retain(|_, slot| current_slot.saturating_sub(*slot) < 10);
                }

                // Pool state was already updated by the Geyser stream.
                let pool_state = match state_cache.get_any(&change.pool_address) {
                    Some(s) => s,
                    None => continue,
                };

                let pool_address = change.pool_address;

                // Construct a DetectedSwap trigger from the state change.
                // We don't know the exact swap direction, so we set output_mint
                // to token_a — the route calculator will search both directions.
                let trigger = DetectedSwap {
                    signature: String::new(), // No tx sig in post-block model
                    dex_type: pool_state.dex_type,
                    pool_address,
                    input_mint: pool_state.token_a_mint,
                    output_mint: pool_state.token_b_mint,
                    amount: None,
                    observed_slot: change.slot,
                };

                // Also search with reversed direction for full coverage.
                let trigger_reverse = DetectedSwap {
                    signature: String::new(),
                    dex_type: pool_state.dex_type,
                    pool_address,
                    input_mint: pool_state.token_b_mint,
                    output_mint: pool_state.token_a_mint,
                    amount: None,
                    observed_slot: change.slot,
                };

                // Find profitable routes in both directions
                let mut routes = route_calculator.find_routes(&trigger);
                routes.extend(route_calculator.find_routes(&trigger_reverse));

                // Deduplicate by sorting and taking best
                routes.sort_by(|a, b| b.estimated_profit.cmp(&a.estimated_profit));

                if routes.is_empty() {
                    continue;
                }

                // Simulate the best route
                let best_route = &routes[0];
                let sim_result = profit_simulator.simulate(best_route);

                match sim_result {
                    SimulationResult::Profitable {
                        route,
                        net_profit_lamports,
                        tip_lamports,
                        final_profit_lamports,
                    } => {
                        opportunities_found += 1;
                        info!(
                            "OPPORTUNITY #{}: {} hops, gross={}, tip={}, net={} lamports, pool={}",
                            opportunities_found,
                            route.hop_count(),
                            net_profit_lamports,
                            tip_lamports,
                            final_profit_lamports,
                            pool_address,
                        );

                        if config.dry_run {
                            info!("DRY RUN — skipping bundle submission");
                            continue;
                        }

                        if !can_submit_route(&route) {
                            tracing::debug!("Route has unsupported DEX, skipping submission");
                            continue;
                        }

                        // Get recent blockhash from cache
                        let blockhash = match blockhash_cache.get() {
                            Some(h) => h,
                            None => {
                                warn!("Stale or missing blockhash, skipping opportunity");
                                continue;
                            }
                        };

                        match bundle_builder.build_arb_bundle(&route, tip_lamports, blockhash) {
                            Ok(bundle_txs) => {
                                let relay = multi_relay.clone();
                                let tip = tip_lamports;
                                // Fire-and-forget relay submission on async runtime.
                                // We don't wait — next opportunity is more valuable than
                                // tracking this bundle's fate.
                                rt.spawn(async move {
                                    let results = relay.submit_bundle(&bundle_txs, tip).await;
                                    let landed = results.iter().filter(|r| r.success).count();
                                    if landed > 0 {
                                        info!("Bundle landed on {}/{} relays", landed, results.len());
                                    }
                                });
                                bundles_submitted += 1;
                            }
                            Err(e) => {
                                error!("Bundle build failed: {}", e);
                            }
                        }
                    }
                    SimulationResult::Unprofitable { reason } => {
                        tracing::trace!("Route rejected: {}", reason);
                    }
                }
            }

            info!(
                "Router thread exiting. Opportunities: {}, Bundles: {}",
                opportunities_found, bundles_submitted
            );
        })
    };

    // Task 3: Periodic state cache maintenance
    let cache_handle = {
        let state_cache = state_cache.clone();
        let mut shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() { break; }
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                        state_cache.evict_stale();
                        info!("Cache: {} pools tracked", state_cache.len());
                    }
                }
            }
        })
    };

    // Wait for all tasks
    let _ = tokio::try_join!(stream_handle, cache_handle, blockhash_handle);
    let _ = router_handle.await;

    info!("Engine shutdown complete");
    Ok(())
}

/// Check if all hops in a route use DEXes with real swap IX builders.
fn can_submit_route(route: &router::pool::ArbRoute) -> bool {
    route.hops.iter().all(|hop| matches!(
        hop.dex_type,
        router::pool::DexType::RaydiumCp
        | router::pool::DexType::MeteoraDammV2
        | router::pool::DexType::OrcaWhirlpool
        | router::pool::DexType::RaydiumClmm
        | router::pool::DexType::MeteoraDlmm
    ))
}

/// Create Sanctum virtual pools for each supported LST.
///
/// Each LST gets a virtual pool modeling the Sanctum Infinity oracle rate.
/// Reserves are synthetic — large values that produce the correct exchange rate
/// under constant-product math with negligible price impact.
///
/// Initial rates are hardcoded approximations. In production, these should be
/// fetched from on-chain stake pool state at startup (total_lamports / pool_token_supply).
/// The Geyser stream will keep them updated as Sanctum reserve ATAs change.
fn bootstrap_sanctum_pools(state_cache: &state::StateCache) {
    use router::pool::{DexType, PoolExtra, PoolState};

    let sol = config::sol_mint();
    const SYNTHETIC_RESERVE_BASE: u64 = 1_000_000_000_000_000; // 1M SOL in lamports

    // Approximate current exchange rates (SOL per LST).
    // These get corrected as soon as the first Geyser update arrives.
    let lst_rates: Vec<(solana_sdk::pubkey::Pubkey, &str, f64)> = config::lst_mints()
        .into_iter()
        .map(|(mint, name)| {
            let rate = match name {
                "jitoSOL" => 1.082,
                "mSOL" => 1.075,
                "bSOL" => 1.060,
                _ => 1.050, // conservative default
            };
            (mint, name, rate)
        })
        .collect();

    for (lst_mint, name, rate) in &lst_rates {
        // Deterministic virtual pool address: PDA([b"sanctum-virtual", lst_mint], system_program)
        let (virtual_pool_addr, _) = solana_sdk::pubkey::Pubkey::find_program_address(
            &[b"sanctum-virtual", lst_mint.as_ref()],
            &solana_sdk::system_program::id(),
        );

        let reserve_a = SYNTHETIC_RESERVE_BASE;
        let reserve_b = (SYNTHETIC_RESERVE_BASE as f64 / rate) as u64;

        let pool = PoolState {
            address: virtual_pool_addr,
            dex_type: DexType::SanctumInfinity,
            token_a_mint: sol,
            token_b_mint: *lst_mint,
            token_a_reserve: reserve_a,
            token_b_reserve: reserve_b,
            fee_bps: 3, // Sanctum typical fee
            current_tick: None,
            sqrt_price_x64: None,
            liquidity: None,
            last_slot: 0,
            extra: PoolExtra::default(),
            best_bid_price: None,
            best_ask_price: None,
        };

        state_cache.upsert(virtual_pool_addr, pool);
        info!("Bootstrapped Sanctum virtual pool for {}: rate={}, addr={}", name, rate, virtual_pool_addr);
    }
}

/// Load the searcher keypair.
/// Tries SEARCHER_PRIVATE_KEY env var (base58) first, then falls back to JSON file.
fn load_keypair(path: &str) -> Result<Keypair> {
    // Try base58 private key from env var first
    if let Ok(pk_b58) = std::env::var("SEARCHER_PRIVATE_KEY") {
        let bytes = bs58::decode(pk_b58.trim())
            .into_vec()
            .map_err(|e| anyhow::anyhow!("Invalid base58 SEARCHER_PRIVATE_KEY: {}", e))?;
        let keypair = Keypair::from_bytes(&bytes)
            .map_err(|e| anyhow::anyhow!("Invalid keypair bytes: {}", e))?;
        info!("Loaded searcher keypair from SEARCHER_PRIVATE_KEY: {}", keypair.pubkey());
        return Ok(keypair);
    }

    // Fall back to JSON file
    let data = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Failed to read keypair file {}: {}", path, e))?;
    let bytes: Vec<u8> = serde_json::from_str(&data)
        .map_err(|e| anyhow::anyhow!("Invalid keypair JSON in {}: {}", path, e))?;
    let keypair = Keypair::from_bytes(&bytes)
        .map_err(|e| anyhow::anyhow!("Invalid keypair bytes in {}: {}", path, e))?;
    info!("Loaded searcher keypair from {}: {}", path, keypair.pubkey());
    Ok(keypair)
}
