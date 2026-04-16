//! CEX-DEX arbitrage binary (Model A, SOL/USDC).
//!
//! Run: `cargo run --release --bin cexdex`

use anyhow::Result;
use crossbeam_channel::bounded;
use solana_mev_bot::cexdex::detector::{Detector, DetectorConfig};
use solana_mev_bot::cexdex::geyser::{narrow_bot_config, start_geyser};
use solana_mev_bot::cexdex::simulator::{CexDexSimulator, CexDexSimulatorConfig, SimulationResult};
use solana_mev_bot::cexdex::{CexDexConfig, Inventory, PriceStore};
use solana_mev_bot::config::{BotConfig, RelayEndpoints};
use solana_mev_bot::executor::relays::{
    astralane::AstralaneRelay, jito::JitoRelay, Relay,
};
use solana_mev_bot::executor::{BundleBuilder, RelayDispatcher};
use solana_mev_bot::feed::binance::run_solusdc_loop;
use solana_mev_bot::metrics;
use solana_mev_bot::rpc_helpers;
use solana_mev_bot::state::{self, BlockhashCache, TipFloorCache};
use solana_sdk::signer::Signer;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .json()
        .init();

    info!("=== CEX-DEX Arbitrage Engine (Model A, SOL/USDC) ===");
    let config = CexDexConfig::from_env()?;
    info!(
        "Config: min_spread={}bps, min_profit=${:.2}, max_trade={} SOL, dry_run={}",
        config.min_spread_bps, config.min_profit_usd, config.max_trade_size_sol, config.dry_run,
    );
    info!("Monitoring {} pools", config.pools.len());
    if config.pools.is_empty() {
        anyhow::bail!("CEXDEX_POOLS must list at least one pool");
    }

    // Metrics (optional)
    metrics::init(config.metrics_port, None, "cexdex");

    // Shared state
    let store = PriceStore::new();
    let inventory = Inventory::new(
        config.hard_cap_ratio,
        config.preferred_low,
        config.preferred_high,
        config.skewed_profit_multiplier,
    );

    // HTTP client (shared)
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    // Load searcher wallet.
    // Note: `rpc_helpers::load_keypair` checks SEARCHER_PRIVATE_KEY (base58) first,
    // then falls back to the JSON file at the given path. The cexdex config
    // exposes `CEXDEX_SEARCHER_PRIVATE_KEY`, but the helper only reads
    // SEARCHER_PRIVATE_KEY. If the cexdex-specific env var is set, mirror it
    // into SEARCHER_PRIVATE_KEY for this process so the helper picks it up.
    if let Some(pk) = &config.searcher_private_key {
        if std::env::var("SEARCHER_PRIVATE_KEY").is_err() {
            std::env::set_var("SEARCHER_PRIVATE_KEY", pk);
        }
    }
    let searcher_keypair = rpc_helpers::load_keypair(&config.searcher_keypair_path)?;
    let searcher_pubkey = searcher_keypair.pubkey();
    info!("Searcher wallet: {}", searcher_pubkey);

    // Initial balance fetch
    let (sol_lamports, usdc_atoms) =
        fetch_initial_balances(&http_client, &config.rpc_url, &searcher_pubkey).await?;
    inventory.set_on_chain(sol_lamports, usdc_atoms);
    info!(
        "Initial balance: {} SOL, {} USDC",
        sol_lamports as f64 / 1e9,
        usdc_atoms as f64 / 1e6,
    );

    // Shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Ctrl+C handler
    let shutdown_tx_ctrlc = shutdown_tx.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("Shutdown signal received");
        let _ = shutdown_tx_ctrlc.send(true);
    });

    // Blockhash cache
    let blockhash_cache = BlockhashCache::new();
    if let Err(e) =
        state::blockhash::fetch_and_update(&http_client, &config.rpc_url, &blockhash_cache).await
    {
        warn!("Initial blockhash fetch failed: {}", e);
    }
    let _bh_handle = {
        let client = http_client.clone();
        let cache = blockhash_cache.clone();
        let rpc = config.rpc_url.clone();
        let rx = shutdown_rx.clone();
        tokio::spawn(async move {
            state::blockhash::run_blockhash_loop(client, rpc, cache, rx).await;
        })
    };

    // Tip floor cache (reuses main engine's Jito WS)
    let tip_floor_cache = TipFloorCache::new();
    let _tip_handle = {
        let client = http_client.clone();
        let cache = tip_floor_cache.clone();
        let rx = shutdown_rx.clone();
        tokio::spawn(async move {
            state::tip_floor::run_tip_floor_loop(client, cache, rx).await;
        })
    };

    // Start Binance WS
    let _binance_handle = {
        let store = store.clone();
        let rx = shutdown_rx.clone();
        tokio::spawn(async move { run_solusdc_loop(store, rx).await })
    };

    // Start narrow Geyser
    let bot_config_geyser = narrow_bot_config(
        config.geyser_grpc_url.clone(),
        config.geyser_auth_token.clone(),
        config.rpc_url.clone(),
        config.pool_state_ttl,
    );
    let (change_tx, change_rx) =
        bounded::<solana_mev_bot::mempool::PoolStateChange>(1024);
    let _geyser_handle = start_geyser(
        bot_config_geyser,
        store.clone(),
        http_client.clone(),
        change_tx,
        shutdown_rx.clone(),
    )
    .await?;

    // Build detector and simulator
    let detector_config = DetectorConfig {
        min_spread_bps: config.min_spread_bps,
        min_profit_usd: config.min_profit_usd,
        max_trade_size_sol: config.max_trade_size_sol,
        cex_staleness_ms: config.cex_staleness_ms,
        slippage_tolerance: config.slippage_tolerance,
    };
    let detector = Detector::new(
        store.clone(),
        inventory.clone(),
        config.pools.clone(),
        detector_config,
    );

    let sim_config = CexDexSimulatorConfig {
        min_profit_usd: config.min_profit_usd,
        slippage_tolerance: config.slippage_tolerance,
        tx_fee_lamports: 5_000,
        min_tip_lamports: 1_000,
        tip_fraction: 0.50,
    };
    let simulator = CexDexSimulator::new(store.clone(), sim_config);

    // BundleBuilder.
    // NOTE: `BundleBuilder::new` takes a `StateCache`, not a `PriceStore`.
    // `store.pools` is the underlying cache (shared with the Geyser writer).
    let bundle_builder = BundleBuilder::new(
        searcher_keypair.insecure_clone(),
        store.pools.clone(),
        config.arb_guard_program_id,
    );

    // Relays: build a BotConfig-shaped bridge so we can reuse the per-relay
    // constructors from the main engine. Only Jito + Astralane are wired for
    // the MVP — other relays can be added by extending `RelayEndpoints` below.
    let bot_config_relays = Arc::new(BotConfig {
        jito_block_engine_url: config.jito_block_engine_url.clone(),
        jito_auth_keypair_path: String::new(),
        geyser_grpc_url: String::new(),
        geyser_auth_token: String::new(),
        rpc_url: config.rpc_url.clone(),
        searcher_keypair_path: config.searcher_keypair_path.clone(),
        relay_endpoints: RelayEndpoints {
            jito: config.jito_block_engine_url.clone(),
            nozomi: None,
            bloxroute: None,
            astralane: config.astralane_relay_url.clone(),
            zeroslot: None,
        },
        tip_fraction: 0.50,
        min_profit_lamports: 0,
        min_tip_lamports: 1_000,
        max_hops: 1,
        pool_state_ttl: config.pool_state_ttl,
        slippage_tolerance: config.slippage_tolerance,
        dry_run: config.dry_run,
        lst_arb_enabled: false,
        lst_min_spread_bps: 0,
        arb_guard_program_id: config.arb_guard_program_id,
        metrics_port: config.metrics_port,
        otlp_endpoint: None,
        otlp_service_name: "cexdex".to_string(),
    });

    // ASTRALANE_API_KEY is read from env by AstralaneRelay::new directly.
    // Mirror the cexdex-specific key into the expected var if set.
    if let Some(key) = &config.astralane_api_key {
        if std::env::var("ASTRALANE_API_KEY").is_err() {
            std::env::set_var("ASTRALANE_API_KEY", key);
        }
    }

    let mut relays: Vec<Arc<dyn Relay>> = Vec::new();
    relays.push(Arc::new(JitoRelay::new(&bot_config_relays)));
    relays.push(Arc::new(AstralaneRelay::new(
        &bot_config_relays,
        shutdown_rx.clone(),
    )));

    // No ALTs for the MVP — single-leg CEX-DEX tx fits comfortably in a
    // legacy transaction.
    let alts: Vec<Arc<solana_message::AddressLookupTableAccount>> = Vec::new();

    let signer_arc = Arc::new(searcher_keypair.insecure_clone());
    let dispatcher = RelayDispatcher::new(relays, signer_arc, alts);
    dispatcher.warmup().await;

    info!("All components initialized, starting detector loop");

    run_detector_loop(
        detector,
        simulator,
        bundle_builder,
        dispatcher,
        blockhash_cache,
        tip_floor_cache,
        inventory.clone(),
        store.clone(),
        config,
        change_rx,
        shutdown_rx,
    )
    .await?;

    let _ = shutdown_tx.send(true);
    Ok(())
}

async fn fetch_initial_balances(
    client: &reqwest::Client,
    rpc_url: &str,
    wallet: &solana_sdk::pubkey::Pubkey,
) -> Result<(u64, u64)> {
    // getBalance for SOL
    let payload = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "getBalance",
        "params": [wallet.to_string()],
    });
    let resp = client
        .post(rpc_url)
        .json(&payload)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;
    let sol_lamports = resp["result"]["value"].as_u64().unwrap_or(0);

    // getTokenAccountsByOwner for USDC
    let usdc_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    let payload = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "getTokenAccountsByOwner",
        "params": [
            wallet.to_string(),
            { "mint": usdc_mint },
            { "encoding": "jsonParsed" }
        ],
    });
    let resp = client
        .post(rpc_url)
        .json(&payload)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;
    let usdc_atoms = resp["result"]["value"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|acc| acc["account"]["data"]["parsed"]["info"]["tokenAmount"]["amount"].as_str())
        .and_then(|s| u64::from_str(s).ok())
        .unwrap_or(0);

    Ok((sol_lamports, usdc_atoms))
}

#[allow(clippy::too_many_arguments)]
async fn run_detector_loop(
    detector: Detector,
    simulator: CexDexSimulator,
    bundle_builder: BundleBuilder,
    dispatcher: RelayDispatcher,
    blockhash_cache: BlockhashCache,
    _tip_floor_cache: TipFloorCache,
    inventory: Inventory,
    store: PriceStore,
    config: CexDexConfig,
    change_rx: crossbeam_channel::Receiver<solana_mev_bot::mempool::PoolStateChange>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let rt = tokio::runtime::Handle::current();
    let mut opportunities: u64 = 0;

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() { break; }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                // periodic detection tick — also catches CEX-side updates
            }
        }

        // Drain Geyser change signals (non-blocking).
        // The detector reads pool state directly from the shared cache;
        // these signals are informational for now.
        while change_rx.try_recv().is_ok() {}

        // Refresh the inventory's SOL price from the latest CEX snapshot so
        // the ratio gate uses current pricing.
        if let Some(snap) = store.get_cex("SOLUSDC") {
            inventory.set_sol_price_usd(snap.mid());
        }

        let route = match detector.check_all() {
            Some(r) => r,
            None => continue,
        };

        let sim_result = simulator.simulate(&route);
        let (route, tip_lamports, min_final_output, net_profit_usd) = match sim_result {
            SimulationResult::Profitable {
                route,
                tip_lamports,
                min_final_output,
                net_profit_usd,
            } => (route, tip_lamports, min_final_output, net_profit_usd),
            SimulationResult::Unprofitable { reason } => {
                tracing::debug!("sim unprofitable: {}", reason);
                continue;
            }
        };

        opportunities += 1;
        info!(
            "OPPORTUNITY #{}: {} on {:?} pool={} input={} expected_output={} tip={} net=${:.4}",
            opportunities,
            route.direction.label(),
            route.dex_type,
            route.pool_address,
            route.input_amount,
            route.expected_output,
            tip_lamports,
            net_profit_usd,
        );

        if config.dry_run {
            info!("DRY_RUN — not submitting");
            continue;
        }

        // Build instructions
        let instructions = match solana_mev_bot::cexdex::bundle::build_instructions_for_cex_dex(
            &bundle_builder,
            &route,
            min_final_output,
        ) {
            Ok(ixs) => ixs,
            Err(e) => {
                warn!("bundle build failed: {}", e);
                continue;
            }
        };

        let blockhash = match blockhash_cache.get() {
            Some(h) => h,
            None => {
                warn!("no blockhash, skipping");
                continue;
            }
        };

        // Reserve inventory so concurrent detections don't double-spend.
        inventory.reserve(route.direction, route.input_amount, route.expected_output);

        // Submit via multi-relay fan-out.
        let _relay_rx = dispatcher.dispatch(&instructions, tip_lamports, blockhash, &rt);

        // MVP: optimistic release after a fixed delay. A proper confirmation
        // tracker (polling getBundleStatuses / getSignatureStatuses) is a
        // follow-up — see Task 12 / the roadmap for CEX-DEX arb.
        let inv = inventory.clone();
        let dir = route.direction;
        let input = route.input_amount;
        let output = route.expected_output;
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            inv.release(dir, input, output);
        });
    }

    Ok(())
}
