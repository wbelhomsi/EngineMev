mod config;
mod executor;
mod mempool;
mod router;
mod state;

use anyhow::Result;
use crossbeam_channel::bounded;
use solana_sdk::signature::Keypair;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{info, warn, error};

use config::BotConfig;
use executor::{BundleBuilder, MultiRelay};
use mempool::MempoolStream;
use router::{RouteCalculator, ProfitSimulator};
use router::simulator::SimulationResult;
use state::StateCache;

/// Channel capacity for detected swaps.
/// Keep small — we want backpressure if the router can't keep up.
/// A backed-up channel means we're too slow and opportunities are stale anyway.
const SWAP_CHANNEL_CAPACITY: usize = 256;

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

    // Shutdown signal
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Ctrl+C handler
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("Shutdown signal received");
        let _ = shutdown_tx_clone.send(true);
    });

    // Channel for detected swaps: mempool stream → router
    let (swap_tx, swap_rx) = bounded(SWAP_CHANNEL_CAPACITY);

    // Initialize components
    let mempool_stream = MempoolStream::new(config.clone(), state_cache.clone());
    let route_calculator = RouteCalculator::new(state_cache.clone(), config.max_hops);
    let profit_simulator = ProfitSimulator::new(
        state_cache.clone(),
        config.tip_fraction,
        config.min_profit_lamports,
    );
    let multi_relay = Arc::new(MultiRelay::new(config.clone()));

    // Load searcher keypair
    let searcher_keypair = load_keypair(&config.searcher_keypair_path)?;
    let bundle_builder = BundleBuilder::new(searcher_keypair);

    info!("All components initialized, starting pipeline...");

    // === Pipeline ===
    //
    // Stream (async) → Channel → Router (sync, CPU-bound) → Simulator → Bundle → Relay (async)
    //
    // The router runs on a dedicated thread to avoid async overhead on
    // the hot path. Route calculation is pure CPU work — no I/O, no awaits.

    // Task 1: Mempool streaming (async, I/O bound)
    let stream_handle = {
        let shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            if let Err(e) = mempool_stream.start(swap_tx, shutdown_rx).await {
                error!("Mempool stream error: {}", e);
            }
        })
    };

    // Task 2: Route calculation + simulation + submission
    // Runs as a blocking task on a dedicated thread
    let router_handle = {
        let shutdown_rx = shutdown_rx.clone();
        let multi_relay = multi_relay.clone();
        let config = config.clone();

        tokio::task::spawn_blocking(move || {
            info!("Router thread started");
            let mut opportunities_found: u64 = 0;
            let mut bundles_submitted: u64 = 0;

            loop {
                // Check shutdown
                if *shutdown_rx.borrow() {
                    break;
                }

                // Receive detected swap (timeout to check shutdown periodically)
                let swap = match swap_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(swap) => swap,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                };

                // Find profitable routes
                let routes = route_calculator.find_routes(&swap);

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
                            "OPPORTUNITY #{}: {} hops, gross={}, tip={}, net={} lamports",
                            opportunities_found,
                            route.hop_count(),
                            net_profit_lamports,
                            tip_lamports,
                            final_profit_lamports,
                        );

                        if config.dry_run {
                            info!("DRY RUN — skipping bundle submission");
                            continue;
                        }

                        // Build and submit bundle
                        // TODO: get recent blockhash from RPC
                        // TODO: get target transaction bytes
                        // For now, log the opportunity
                        bundles_submitted += 1;
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
    let _ = tokio::try_join!(stream_handle, cache_handle);
    let _ = router_handle.await;

    info!("Engine shutdown complete");
    Ok(())
}

/// Load a Solana keypair from a JSON file.
fn load_keypair(path: &str) -> Result<Keypair> {
    // In production, load from file:
    // let data = std::fs::read_to_string(path)?;
    // let bytes: Vec<u8> = serde_json::from_str(&data)?;
    // Ok(Keypair::from_bytes(&bytes)?)

    // For development, generate a throwaway keypair
    warn!("Using generated keypair — replace with real keypair for production");
    Ok(Keypair::new())
}
