use solana_mev_bot::{config, executor, mempool, router, rpc_helpers, sanctum, state};

use anyhow::Result;
use crossbeam_channel::bounded;
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
    // Load config first (needed for metrics setup)
    let config = Arc::new(BotConfig::from_env()?);

    // Initialize Prometheus metrics recorder
    solana_mev_bot::metrics::init(
        config.metrics_port,
        config.otlp_endpoint.as_deref(),
        &config.otlp_service_name,
    );

    // Build tracing layers
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let env_filter = tracing_subscriber::EnvFilter::from_default_env()
        .add_directive("solana_mev_bot=debug".parse()?)
        .add_directive("info".parse()?);

    let json_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_target(true)
        .with_thread_ids(true);

    // Build optional OTLP tracing layer
    let otel_guard = config.otlp_endpoint.as_deref().and_then(|endpoint| {
        solana_mev_bot::metrics::tracing_layer::build_layer(endpoint, &config.otlp_service_name)
    });

    let _otel_provider = if let Some((otel_layer, provider)) = otel_guard {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(json_layer)
            .with(otel_layer)
            .init();
        Some(provider)
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(json_layer)
            .init();
        None
    };

    info!("=== Solana MEV Backrun Arbitrage Engine ===");
    info!("Halal-compliant: spot arb + JIT liquidity only");

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

    // Pre-populate mint program cache for known mints (all SPL Token).
    // Without this, Sanctum virtual pools and early routes fail with
    // "Token program unknown" because the async mint fetch hasn't completed.
    {
        use std::str::FromStr;
        let spl_token = solana_mev_bot::addresses::SPL_TOKEN;
        state_cache.set_mint_program(solana_mev_bot::addresses::WSOL, spl_token);
        for (mint, _name) in config::lst_mints() {
            state_cache.set_mint_program(mint, spl_token);
        }
        state_cache.set_mint_program(
            solana_sdk::pubkey::Pubkey::from_str("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap(), spl_token);
        state_cache.set_mint_program(
            solana_sdk::pubkey::Pubkey::from_str("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB").unwrap(), spl_token);
        info!("Pre-populated mint program cache for {} known mints", 2 + config::lst_mints().len() + 1);
    }

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
        sanctum::bootstrap_pools(&state_cache);
        info!("LST arb enabled: {} Sanctum virtual pools bootstrapped", config::lst_mints().len());

        // Bootstrap Sanctum LST indices from on-chain LstStateList
        if let Err(e) = sanctum::bootstrap_lst_indices(&http_client, &config.rpc_url, &state_cache).await {
            warn!("Failed to bootstrap LST indices: {} — Sanctum routes will be disabled", e);
        }

        // Fetch real-time LST rates from on-chain stake pool accounts
        if let Err(e) = sanctum::fetch_lst_rates(&http_client, &config.rpc_url, &state_cache).await {
            warn!("Failed to fetch LST rates: {} — using fallback rates", e);
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

    // Initialize dynamic tip floor cache (Jito WebSocket stream, REST fallback)
    let tip_floor_cache = state::TipFloorCache::new();
    if let Err(e) = state::tip_floor::fetch_and_update(&http_client, &tip_floor_cache).await {
        warn!("Initial tip floor REST fetch failed (WS stream will take over): {}", e);
    } else if let Some(floor) = tip_floor_cache.get_floor_lamports() {
        info!("Tip floor fetched: {} lamports (ema p50)", floor);
    }

    // Task: Tip floor stream (WebSocket with REST fallback)
    let _tip_floor_handle = {
        let client = http_client.clone();
        let cache = tip_floor_cache.clone();
        let shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            state::tip_floor::run_tip_floor_loop(client, cache, shutdown_rx).await;
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
        config.min_tip_lamports,
    ).with_tip_floor(tip_floor_cache.clone());

    // Load searcher keypair
    let searcher_keypair = rpc_helpers::load_keypair(&config.searcher_keypair_path)?;
    let bundle_builder = Arc::new(BundleBuilder::new(
        searcher_keypair.insecure_clone(),
        state_cache.clone(),
        config.arb_guard_program_id,
    ));

    // Load Address Lookup Tables (enables V0 versioned transactions)
    let mut alts: Vec<Arc<solana_message::AddressLookupTableAccount>> = Vec::new();

    if let Ok(alt_addr_str) = std::env::var("ALT_ADDRESS") {
        match rpc_helpers::load_alt(&http_client, &config.rpc_url, &alt_addr_str).await {
            Ok(alt) => {
                info!("Loaded ALT {} with {} addresses", alt_addr_str, alt.addresses.len());
                alts.push(Arc::new(alt));
            }
            Err(e) => {
                warn!("Failed to load ALT: {} — using legacy transactions", e);
            }
        }
    } else {
        info!("No ALT_ADDRESS configured — using legacy transactions");
    }

    // Load additional public ALTs for better V0 compression
    match rpc_helpers::load_alt(&http_client, &config.rpc_url, "2kkWwe1YoqdrhyVsfcgVJ5sqhxxunX4RieGs7GuV1shf").await {
        Ok(alt) => {
            info!("Loaded external ALT with {} addresses", alt.addresses.len());
            alts.push(Arc::new(alt));
        }
        Err(e) => {
            warn!("Failed to load external ALT: {} — continuing without it", e);
        }
    }

    // Initialize per-relay modules — each owns its own tip accounts, rate limiting, and submission
    let relays: Vec<Arc<dyn Relay>> = vec![
        Arc::new(JitoRelay::new(&config)),
        Arc::new(AstralaneRelay::new(&config, shutdown_rx.clone())),
        Arc::new(NozomiRelay::new(&config)),
        Arc::new(BloxrouteRelay::new(&config)),
        Arc::new(ZeroSlotRelay::new(&config)),
    ];
    let alts_for_public = alts.clone(); // Keep a ref for SEND_PUBLIC
    let relay_dispatcher = Arc::new(RelayDispatcher::new(relays, Arc::new(searcher_keypair), alts));
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

    // Task 1: Geyser streaming (LaserStream handles reconnection internally)
    let stream_handle = {
        let shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            if let Err(e) = geyser_stream.start(change_tx.clone(), shutdown_rx.clone()).await {
                error!("Geyser stream fatal error: {}", config::redact_url(&e.to_string()));
            }
            info!("Geyser stream task exited");
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
            let skip_simulator = std::env::var("SKIP_SIMULATOR").map(|v| v == "true").unwrap_or(false);
            let send_public = std::env::var("SEND_PUBLIC").map(|v| v == "true").unwrap_or(false);
            let mut public_sent = false;
            if skip_simulator {
                warn!("SKIP_SIMULATOR=true — bypassing profit simulation, relying on minimum_amount_out");
            }
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

            let sol_mint_for_filter = config::sol_mint();
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

                let pipeline_start = std::time::Instant::now();

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

                // Sample channel backpressure
                solana_mev_bot::metrics::counters::set_channel_backpressure(change_rx.len());

                // Pool state was already updated by the Geyser stream.
                let pool_state = match state_cache.get(&change.pool_address) {
                    Some(s) => s,
                    None => continue,
                };

                // Skip dust pools — too small for meaningful arb.
                // Check the SOL-side reserve specifically (we arb in SOL).
                // The old `&&` check let pools like 0.84 SOL + 200K USDT through
                // because the USDT side was numerically large in raw units.
                const MIN_SOL_RESERVE: u64 = 10_000_000_000; // 10 SOL
                let sol_reserve = if pool_state.token_a_mint == sol_mint_for_filter {
                    pool_state.token_a_reserve
                } else if pool_state.token_b_mint == sol_mint_for_filter {
                    pool_state.token_b_reserve
                } else {
                    // Non-SOL pair — use the smaller reserve as a proxy
                    std::cmp::min(pool_state.token_a_reserve, pool_state.token_b_reserve)
                };
                if sol_reserve < MIN_SOL_RESERVE {
                    continue;
                }

                let pool_address = change.pool_address;

                // Construct a DetectedSwap trigger from the state change.
                // We don't know the exact swap direction, so we set output_mint
                // to token_a — the route calculator will search both directions.
                let trigger = DetectedSwap {
                    dex_type: pool_state.dex_type,
                    pool_address,
                    input_mint: pool_state.token_a_mint,
                    output_mint: pool_state.token_b_mint,
                    amount: None,
                    observed_slot: change.slot,
                };

                // Also search with reversed direction for full coverage.
                let trigger_reverse = DetectedSwap {
                    dex_type: pool_state.dex_type,
                    pool_address,
                    input_mint: pool_state.token_b_mint,
                    output_mint: pool_state.token_a_mint,
                    amount: None,
                    observed_slot: change.slot,
                };

                // Find profitable routes in both directions
                let route_start = std::time::Instant::now();
                let mut routes = route_calculator.find_routes(&trigger);
                routes.extend(route_calculator.find_routes(&trigger_reverse));
                solana_mev_bot::metrics::counters::record_route_calc_duration_us(
                    route_start.elapsed().as_micros() as u64);

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

                // Cap routes to avoid spending time on low-value candidates.
                // The sort ensures we keep the best ones.
                const MAX_ROUTES_PER_EVENT: usize = 10;
                routes.truncate(MAX_ROUTES_PER_EVENT);

                if routes.is_empty() {
                    continue;
                }

                solana_mev_bot::metrics::counters::inc_routes_found(routes[0].hop_count());

                // Simulate (or skip) the best route
                let best_route = &routes[0];
                tracing::debug!("Best route: {} hops, est_profit={}, base_mint={}",
                    best_route.hop_count(), best_route.estimated_profit, best_route.base_mint);

                // When SKIP_SIMULATOR=true, bypass re-simulation for speed.
                // The on-chain minimum_amount_out provides the safety net.
                let sim_start = std::time::Instant::now();
                let sim_result = if skip_simulator && best_route.estimated_profit > 0 {
                    // Sanity cap: reject routes with >10 SOL estimated profit (approximation artifact)
                    let max_profit_lamports = 10_000_000_000u64; // 10 SOL
                    if best_route.estimated_profit_lamports > max_profit_lamports {
                        warn!("SKIP_SIMULATOR: sanity cap — estimated profit {} > 10 SOL, skipping",
                              best_route.estimated_profit_lamports);
                        SimulationResult::Unprofitable {
                            reason: format!("sanity cap: estimated profit {} > 1 SOL",
                                            best_route.estimated_profit_lamports),
                        }
                    } else {
                        let fraction_tip = (best_route.estimated_profit_lamports as f64 * config.tip_fraction) as u64;
                        let dynamic_floor = tip_floor_cache.get_floor_lamports().unwrap_or(0);
                        let tip = fraction_tip.max(config.min_tip_lamports.max(dynamic_floor));
                        // Safety: tip must be less than profit
                        if tip >= best_route.estimated_profit_lamports {
                            warn!("SKIP_SIMULATOR: tip {} >= profit {}, skipping",
                                  tip, best_route.estimated_profit_lamports);
                            SimulationResult::Unprofitable {
                                reason: format!("tip {} >= profit {}",
                                                tip, best_route.estimated_profit_lamports),
                            }
                        } else {
                            let net = best_route.estimated_profit_lamports.saturating_sub(tip);
                            if net < config.min_profit_lamports {
                                SimulationResult::Unprofitable {
                                    reason: format!("net profit {} < min {}",
                                                    net, config.min_profit_lamports),
                                }
                            } else {
                                SimulationResult::Profitable {
                                    route: best_route.clone(),
                                    net_profit_lamports: best_route.estimated_profit_lamports,
                                    tip_lamports: tip,
                                    final_profit_lamports: net,
                                }
                            }
                        }
                    }
                } else {
                    profit_simulator.simulate(best_route)
                };
                solana_mev_bot::metrics::counters::record_simulation_duration_us(
                    sim_start.elapsed().as_micros() as u64);

                match sim_result {
                    SimulationResult::Profitable {
                        route,
                        net_profit_lamports,
                        tip_lamports,
                        final_profit_lamports,
                    } => {
                        opportunities_found += 1;
                        solana_mev_bot::metrics::counters::inc_opportunities(
                            &format!("{:?}", route.hops[0].dex_type));
                        solana_mev_bot::metrics::counters::add_estimated_profit_lamports(final_profit_lamports);
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
                            solana_mev_bot::metrics::counters::inc_bundles_skipped("dry_run");
                            continue;
                        }

                        if !router::can_submit_route(&route) {
                            tracing::debug!("Route has unsupported DEX, skipping submission");
                            solana_mev_bot::metrics::counters::inc_bundles_skipped("unsupported_dex");
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
                            solana_mev_bot::metrics::counters::inc_bundles_skipped("dedup");
                            continue;
                        }
                        entry.0 += 1;

                        let blockhash = match blockhash_cache.get() {
                            Some(h) => h,
                            None => {
                                warn!("Stale or missing blockhash, skipping opportunity");
                                solana_mev_bot::metrics::counters::inc_bundles_skipped("stale_blockhash");
                                continue;
                            }
                        };

                        // Build base instructions (no tips — each relay adds its own).
                        // min_final_output = input_amount (break-even protection).
                        // arb-guard's execute_arb_v2 verifies actual profit on-chain.
                        // Using input (not input+profit) avoids ExceededSlippage when
                        // the actual output is profitable but below the optimistic estimate.
                        let min_final_output = route.input_amount;
                        let build_start = std::time::Instant::now();
                        match bundle_builder.build_arb_instructions(&route, min_final_output) {
                            Ok(instructions) => {
                                solana_mev_bot::metrics::counters::record_bundle_build_duration_us(
                                    build_start.elapsed().as_micros() as u64);
                                // Optional: simulate before submission
                                if simulate_bundles {
                                    let http = http_client.clone();
                                    let rpc_url = config.rpc_url.clone();
                                    let ixs = instructions.clone();
                                    let signer_pub = bundle_builder.signer_pubkey();
                                    let _bh = blockhash;
                                    rt.spawn(async move {
                                        // Build temp tx for simulation (no tip needed)
                                        let tx = solana_sdk::transaction::Transaction::new_with_payer(
                                            &ixs, Some(&signer_pub),
                                        );
                                        let bytes = bincode::serialize(&tx).unwrap_or_default();
                                        rpc_helpers::simulate_bundle_tx(&http, &rpc_url, &[bytes]).await;
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
                                    let public_alts = alts_for_public.clone();
                                    warn!("SEND_PUBLIC: sending 1 tx via public RPC...");
                                    rt.spawn(async move {
                                        let alt_refs: Vec<&solana_message::AddressLookupTableAccount> = public_alts.iter().map(|a| a.as_ref()).collect();
                                        rpc_helpers::send_public_tx(&http, &rpc, &ixs, &signer_arc, bh, &alt_refs).await;
                                    });
                                }

                                // Dispatch to all relays concurrently
                                let relay_rx = relay_dispatcher.dispatch(
                                    &instructions, tip_lamports, blockhash, &rt,
                                );
                                bundles_submitted += 1;
                                solana_mev_bot::metrics::counters::inc_bundles_submitted();
                                // Record estimated (pre-confirmation) metrics
                                solana_mev_bot::metrics::counters::add_estimated_tips_lamports(tip_lamports);
                                solana_mev_bot::metrics::counters::record_pipeline_duration_us(
                                    pipeline_start.elapsed().as_micros() as u64);

                                // Spawn async confirmation tracker (non-blocking).
                                // Polls getBundleStatuses to determine if the bundle
                                // actually landed. Only confirmed bundles increment
                                // the confirmed profit/tip metrics.
                                if !config.dry_run {
                                    let confirm_jito = format!(
                                        "{}/api/v1/bundles",
                                        config.relay_endpoints.jito.trim_end_matches('/')
                                    );
                                    executor::spawn_confirmation_tracker(
                                        http_client.clone(),
                                        confirm_jito,
                                        final_profit_lamports,
                                        tip_lamports,
                                        relay_rx,
                                    );
                                }
                            }
                            Err(e) => {
                                error!("Bundle build failed: {}", e);
                                solana_mev_bot::metrics::counters::inc_bundle_build_errors();
                            }
                        }
                    }
                    SimulationResult::Unprofitable { reason } => {
                        // Categorize rejection for metrics
                        let reason_label = if reason.contains("sanity cap") {
                            "sanity_cap"
                        } else if reason.contains("tip") && reason.contains("profit") {
                            "tip_exceeds_profit"
                        } else if reason.contains("net profit") && reason.contains("< min") {
                            "below_min_profit"
                        } else if reason.contains("stale") {
                            "stale_state"
                        } else {
                            "unprofitable"
                        };
                        solana_mev_bot::metrics::counters::inc_simulation_rejected(reason_label);
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
                        solana_mev_bot::metrics::counters::set_cache_pools_tracked(state_cache.len());
                        info!("Cache: {} pools tracked", state_cache.len());
                    }
                }
            }
        })
    };

    // Wait for all tasks
    let _ = tokio::try_join!(stream_handle, cache_handle, blockhash_handle);
    let _ = router_handle.await;

    // Flush any pending OTLP spans
    drop(_otel_provider);

    info!("Engine shutdown complete");
    Ok(())
}
