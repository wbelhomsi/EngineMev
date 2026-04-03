use solana_mev_bot::{config, executor, mempool, router, state};

use anyhow::Result;
use crossbeam_channel::bounded;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{info, warn, error};

use config::BotConfig;
use executor::{BundleBuilder, RelayDispatcher};
use executor::relays::{Relay, jito::JitoRelay, astralane::AstralaneRelay,
    nozomi::NozomiRelay, bloxroute::BloxrouteRelay, zeroslot::ZeroSlotRelay};
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

        // Bootstrap Sanctum LST indices from on-chain LstStateList
        if let Err(e) = bootstrap_lst_indices(&http_client, &config.rpc_url, &state_cache).await {
            warn!("Failed to bootstrap LST indices: {} — Sanctum routes will be disabled", e);
        }
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

    // Load searcher keypair
    let searcher_keypair = load_keypair(&config.searcher_keypair_path)?;
    let bundle_builder = Arc::new(BundleBuilder::new(searcher_keypair.insecure_clone(), state_cache.clone()));

    // Initialize per-relay modules — each owns its own tip accounts, rate limiting, and submission
    let relays: Vec<Arc<dyn Relay>> = vec![
        Arc::new(JitoRelay::new(&config)),
        Arc::new(AstralaneRelay::new(&config, shutdown_rx.clone())),
        Arc::new(NozomiRelay::new(&config)),
        Arc::new(BloxrouteRelay::new(&config)),
        Arc::new(ZeroSlotRelay::new(&config)),
    ];
    let relay_dispatcher = Arc::new(RelayDispatcher::new(relays, Arc::new(searcher_keypair)));
    relay_dispatcher.warmup().await;

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
        let relay_dispatcher = relay_dispatcher.clone();
        let bundle_builder = bundle_builder.clone();
        let config = config.clone();
        let state_cache = state_cache.clone();
        let blockhash_cache = blockhash_cache.clone();

        tokio::task::spawn_blocking(move || {
            info!("Router thread started");
            let mut opportunities_found: u64 = 0;
            let mut bundles_submitted: u64 = 0;
            let simulate_bundles = std::env::var("SIMULATE_BUNDLES").map(|v| v == "true").unwrap_or(false);
            let send_public = std::env::var("SEND_PUBLIC").map(|v| v == "true").unwrap_or(false);
            let mut public_sent = false;
            if send_public {
                warn!("SEND_PUBLIC=true — will send FIRST opportunity via public RPC (costs tx fee)");
            }
            if simulate_bundles {
                warn!("SIMULATE_BUNDLES=true — will simulateTransaction before each submission");
            }

            // Create a tokio runtime handle for async relay submission from sync context.
            // The relay fan-out is async (HTTP calls), but the router loop is sync.
            let rt = tokio::runtime::Handle::current();

            let mut recent_pools: std::collections::HashMap<solana_sdk::pubkey::Pubkey, u64> = std::collections::HashMap::new();

            // Arb dedup: track recent submissions by route signature (token path).
            // Key = sorted intermediate mints in the route. Value = (count, first_seen).
            // Allows up to MAX_SUBS_PER_ARB submissions per arb per DEDUP_WINDOW.
            const MAX_SUBS_PER_ARB: u32 = 5;
            const DEDUP_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);
            let mut recent_arbs: std::collections::HashMap<Vec<solana_sdk::pubkey::Pubkey>, (u32, std::time::Instant)>
                = std::collections::HashMap::new();

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

                // Filter: only keep routes that start/end with SOL (the token we hold)
                let sol = config::sol_mint();
                let total_before = routes.len();
                routes.retain(|r| r.base_mint == sol);
                if total_before > 0 && routes.is_empty() {
                    tracing::debug!("Filtered {} routes (none SOL-base)", total_before);
                } else if total_before > 0 {
                    tracing::debug!("{} routes found, {} SOL-base", total_before, routes.len());
                }

                // Deduplicate by sorting and taking best
                routes.sort_by(|a, b| b.estimated_profit.cmp(&a.estimated_profit));

                if routes.is_empty() {
                    continue;
                }

                // Simulate the best route
                let best_route = &routes[0];
                tracing::debug!("Best route: {} hops, est_profit={}, base_mint={}",
                    best_route.hop_count(), best_route.estimated_profit, best_route.base_mint);
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

                        // Arb dedup
                        let arb_key: Vec<solana_sdk::pubkey::Pubkey> = route.hops.iter()
                            .map(|h| h.output_mint)
                            .filter(|m| *m != route.base_mint)
                            .collect();
                        let now_dedup = std::time::Instant::now();
                        recent_arbs.retain(|_, (_, t)| now_dedup.duration_since(*t) < DEDUP_WINDOW);
                        let entry = recent_arbs.entry(arb_key).or_insert((0, now_dedup));
                        if entry.0 >= MAX_SUBS_PER_ARB {
                            tracing::trace!("Arb dedup: already submitted {} times, skipping", entry.0);
                            continue;
                        }
                        entry.0 += 1;

                        let blockhash = match blockhash_cache.get() {
                            Some(h) => h,
                            None => {
                                warn!("Stale or missing blockhash, skipping opportunity");
                                continue;
                            }
                        };

                        // Build base instructions (no tips — each relay adds its own).
                        // min_final_output protects the SWAP output only.
                        // The tip is a separate SOL transfer added by each relay,
                        // so the swap must return at least input + gross_profit.
                        let min_final_output = route.input_amount
                            + route.estimated_profit_lamports;
                        match bundle_builder.build_arb_instructions(&route, min_final_output) {
                            Ok(instructions) => {
                                // Optional: simulate before submission
                                if simulate_bundles {
                                    let http = http_client.clone();
                                    let rpc_url = config.rpc_url.clone();
                                    let ixs = instructions.clone();
                                    let signer_pub = bundle_builder.signer_pubkey();
                                    let bh = blockhash;
                                    rt.spawn(async move {
                                        // Build temp tx for simulation (no tip needed)
                                        let tx = solana_sdk::transaction::Transaction::new_with_payer(
                                            &ixs, Some(&signer_pub),
                                        );
                                        let bytes = bincode::serialize(&tx).unwrap_or_default();
                                        simulate_bundle_tx(&http, &rpc_url, &[bytes]).await;
                                    });
                                }
                                // One-shot public send for on-chain verification
                                if send_public && !public_sent {
                                    public_sent = true;
                                    let http = http_client.clone();
                                    let rpc = config.rpc_url.clone();
                                    let ixs = instructions.clone();
                                    let bh = blockhash;
                                    let signer_arc = relay_dispatcher.signer();
                                    warn!("SEND_PUBLIC: sending 1 tx via public RPC...");
                                    rt.spawn(async move {
                                        send_public_tx(&http, &rpc, &ixs, &signer_arc, bh).await;
                                    });
                                }

                                // Dispatch to all relays concurrently
                                relay_dispatcher.dispatch(
                                    &instructions, tip_lamports, blockhash, &rt,
                                );
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
        | router::pool::DexType::RaydiumClmm
        | router::pool::DexType::OrcaWhirlpool
        | router::pool::DexType::MeteoraDlmm
        | router::pool::DexType::MeteoraDammV2
        | router::pool::DexType::SanctumInfinity
        | router::pool::DexType::Phoenix
        | router::pool::DexType::Manifest
    ))
}

/// Fetch the Sanctum LstStateList from on-chain and populate mint->index mapping.
/// Each entry is 80 bytes: padding(16) + mint(32) + calculator(32).
async fn bootstrap_lst_indices(
    client: &reqwest::Client,
    rpc_url: &str,
    state_cache: &state::StateCache,
) -> Result<()> {
    use base64::{engine::general_purpose, Engine as _};

    let s_controller = config::programs::sanctum_s_controller();
    let (lst_state_list_pda, _) = Pubkey::find_program_address(&[b"lst-state-list"], &s_controller);

    let payload = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "getAccountInfo",
        "params": [lst_state_list_pda.to_string(), {"encoding": "base64"}]
    });

    let resp = client.post(rpc_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(10))
        .send().await?
        .json::<serde_json::Value>().await?;

    let b64 = resp["result"]["value"]["data"][0]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("LstStateList account not found"))?;

    let data = general_purpose::STANDARD.decode(b64)?;
    info!("LstStateList: {} bytes", data.len());

    // Parse as array of 80-byte entries, skip 16-byte header
    let header_size = 16;
    if data.len() < header_size { return Ok(()); }
    let entry_data = &data[header_size..];
    let entry_size = 80;
    let count = entry_data.len() / entry_size;

    let mut found = 0;
    let supported_lsts: Vec<(Pubkey, &str)> = config::lst_mints();
    let sol = config::sol_mint();

    for i in 0..count {
        let offset = i * entry_size;
        if offset + entry_size > entry_data.len() { break; }

        // sol_value at bytes 8..16 within each entry (u64 LE)
        let sol_value = u64::from_le_bytes(
            entry_data[offset + 8..offset + 16].try_into().unwrap_or([0u8; 8])
        );
        // mint is at bytes 16..48 within each entry
        let mint_bytes: [u8; 32] = entry_data[offset + 16..offset + 48]
            .try_into().unwrap_or([0u8; 32]);
        let mint = Pubkey::new_from_array(mint_bytes);
        if mint == Pubkey::default() { continue; }
        state_cache.set_lst_index(mint, i as u32);
        found += 1;

        // For supported LSTs: update Sanctum virtual pool with real on-chain rate
        if sol_value > 0 {
            for (lst_mint, name) in &supported_lsts {
                if mint == *lst_mint {
                    // sol_value is in lamports per 10^9 LST atoms (rate = sol_value / 10^9)
                    let rate = sol_value as f64 / 1_000_000_000.0;
                    if rate > 0.5 && rate < 5.0 { // sanity check
                        // Update the virtual pool reserves to reflect real rate
                        let (virtual_pool_addr, _) = Pubkey::find_program_address(
                            &[b"sanctum-virtual", lst_mint.as_ref()],
                            &solana_sdk::system_program::id(),
                        );
                        let reserve_a: u64 = 1_000_000_000_000_000; // 1M SOL
                        let reserve_b = (reserve_a as f64 / rate) as u64;
                        if let Some(mut pool) = state_cache.get_any(&virtual_pool_addr) {
                            pool.token_a_reserve = reserve_a;
                            pool.token_b_reserve = reserve_b;
                            state_cache.upsert(virtual_pool_addr, pool);
                            info!("Updated Sanctum rate for {}: sol_value={}, rate={:.6}", name, sol_value, rate);
                        }
                    }
                }
            }
        }
    }

    info!("Bootstrapped {} LST indices from LstStateList", found);
    Ok(())
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

/// Simulate a bundle's first transaction via RPC simulateTransaction.
/// Logs the result (success/failure + program logs) for debugging.
async fn simulate_bundle_tx(
    client: &reqwest::Client,
    rpc_url: &str,
    bundle_txs: &[Vec<u8>],
) {
    use base64::{engine::general_purpose, Engine as _};

    if bundle_txs.is_empty() {
        return;
    }

    let tx_b64 = general_purpose::STANDARD.encode(&bundle_txs[0]);

    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "simulateTransaction",
        "params": [
            tx_b64,
            {
                "encoding": "base64",
                "replaceRecentBlockhash": true,
                "sigVerify": false,
                "commitment": "processed"
            }
        ]
    });

    match client
        .post(rpc_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) => {
            match resp.json::<serde_json::Value>().await {
                Ok(json) => {
                    let result = &json["result"]["value"];
                    let err = &result["err"];
                    let logs = result["logs"]
                        .as_array()
                        .map(|a| a.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join("\n  "))
                        .unwrap_or_default();

                    if err.is_null() {
                        info!("SIM SUCCESS | logs:\n  {}", logs);
                    } else {
                        warn!("SIM FAILED | err={} | logs:\n  {}", err, logs);
                    }
                }
                Err(e) => warn!("Simulation response parse error: {}", e),
            }
        }
        Err(e) => warn!("Simulation request failed: {}", crate::config::redact_url(&e.to_string())),
    }
}

/// Send ONE transaction via public RPC (sendTransaction) for on-chain verification.
/// This bypasses Jito bundles entirely — goes through normal tx processing.
/// Costs: tx fee (~5000 lamports) + priority fee. minimum_amount_out protects against loss.
async fn send_public_tx(
    client: &reqwest::Client,
    rpc_url: &str,
    base_instructions: &[solana_sdk::instruction::Instruction],
    signer: &solana_sdk::signature::Keypair,
    recent_blockhash: solana_sdk::hash::Hash,
) {
    use base64::{engine::general_purpose, Engine as _};
    use solana_sdk::{signer::Signer, transaction::Transaction};

    // Build and sign (no tip needed for public send)
    let tx = Transaction::new_signed_with_payer(
        base_instructions,
        Some(&signer.pubkey()),
        &[signer],
        recent_blockhash,
    );

    let tx_bytes = match bincode::serialize(&tx) {
        Ok(b) => b,
        Err(e) => { warn!("SEND_PUBLIC: serialize error: {}", e); return; }
    };

    if tx_bytes.len() > 1232 {
        warn!("SEND_PUBLIC: tx too large ({} bytes), skipping", tx_bytes.len());
        return;
    }

    let tx_b64 = general_purpose::STANDARD.encode(&tx_bytes);
    info!("SEND_PUBLIC: sending tx ({} bytes) to public RPC...", tx_bytes.len());

    let payload = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "sendTransaction",
        "params": [
            tx_b64,
            {
                "encoding": "base64",
                "skipPreflight": true,
                "preflightCommitment": "processed",
                "maxRetries": 3
            }
        ]
    });

    match client.post(rpc_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(10))
        .send().await
    {
        Ok(resp) => {
            match resp.json::<serde_json::Value>().await {
                Ok(json) => {
                    if let Some(sig) = json["result"].as_str() {
                        warn!("SEND_PUBLIC SUCCESS: tx signature = {}", sig);
                        warn!("Check: https://solscan.io/tx/{}", sig);
                    } else if let Some(err) = json.get("error") {
                        warn!("SEND_PUBLIC FAILED: {}", err);
                    } else {
                        warn!("SEND_PUBLIC: unexpected response: {}", json);
                    }
                }
                Err(e) => warn!("SEND_PUBLIC: response parse error: {}", e),
            }
        }
        Err(e) => warn!("SEND_PUBLIC: request failed: {}", crate::config::redact_url(&e.to_string())),
    }
}
