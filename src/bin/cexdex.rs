//! CEX-DEX arbitrage binary (Model A, SOL/USDC).
//!
//! Run: `cargo run --release --bin cexdex`

use anyhow::Result;
use crossbeam_channel::bounded;
use solana_mev_bot::cexdex::detector::{Detector, DetectorConfig};
use solana_mev_bot::cexdex::geyser::{narrow_bot_config, start_geyser};
use solana_mev_bot::cexdex::simulator::{CexDexSimulator, CexDexSimulatorConfig, SimulationResult};
use solana_mev_bot::cexdex::stats::{now_ms, OpportunityRecord, StatsCollector};
use solana_mev_bot::cexdex::{CexDexConfig, Inventory, PriceStore};
use solana_mev_bot::config::{BotConfig, RelayEndpoints};
use solana_mev_bot::executor::relays::{
    jito::JitoRelay, Relay,
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
    // `rpc_helpers::load_keypair` reads SEARCHER_PRIVATE_KEY (base58) first, then
    // falls back to the JSON file. If CEXDEX_SEARCHER_PRIVATE_KEY is set, it must
    // override SEARCHER_PRIVATE_KEY for this process — the cexdex binary is
    // documented to use a separate wallet for clean P&L isolation.
    if let Some(pk) = &config.searcher_private_key {
        std::env::set_var("SEARCHER_PRIVATE_KEY", pk);
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

    // Optional auto-shutdown after N seconds (useful for analysis runs).
    // Set CEXDEX_RUN_SECS=3600 to auto-terminate after 1 hour.
    if let Ok(secs) = std::env::var("CEXDEX_RUN_SECS").and_then(|s| s.parse::<u64>().map_err(|_| std::env::VarError::NotPresent)) {
        info!("Auto-shutdown scheduled after {}s", secs);
        let shutdown_tx_timer = shutdown_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
            info!("Auto-shutdown timer elapsed ({}s) — requesting shutdown", secs);
            let _ = shutdown_tx_timer.send(true);
        });
    }

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
    let monitored_pool_pubkeys: Vec<solana_sdk::pubkey::Pubkey> =
        config.pools.iter().map(|(_, pk)| *pk).collect();
    let nonce_pool = solana_mev_bot::cexdex::NoncePool::new(config.nonce_accounts.clone());
    let _geyser_handle = start_geyser(
        bot_config_geyser,
        store.clone(),
        http_client.clone(),
        monitored_pool_pubkeys,
        nonce_pool.clone(),
        searcher_pubkey,
        change_tx,
        shutdown_rx.clone(),
    )
    .await?;

    // Build detector and simulator
    let detector_config = DetectorConfig {
        min_spread_bps: config.min_spread_bps,
        min_profit_usd: config.min_profit_usd,
        max_trade_size_sol: config.max_trade_size_sol,
        max_position_fraction: config.max_position_fraction,
        cex_staleness_ms: config.cex_staleness_ms,
        slippage_tolerance: config.slippage_tolerance,
        dedup_window_ms: config.dedup_window_ms,
        global_submit_cooldown_ms: config.global_submit_cooldown_ms,
    };
    let detector = Detector::new(
        store.clone(),
        inventory.clone(),
        config.pools.clone(),
        detector_config,
    );

    let max_tip_fraction = config
        .tip_fractions
        .values()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);
    let sim_config = CexDexSimulatorConfig {
        min_profit_usd: config.min_profit_usd,
        slippage_tolerance: config.slippage_tolerance,
        tx_fee_lamports: 5_000,
        min_tip_lamports: 1_000,
        max_tip_fraction,
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
        tip_fraction: config.tip_fraction,
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

    // Single-relay only (Jito) for CEX-DEX arb until the nonce-based
    // non-equivocation fix is implemented.
    //
    // Why: fanning out to multiple relays (Jito + Astralane) produces DIFFERENT
    // signed txs per relay because each relay requires its OWN tip account
    // (Jito won't accept Astralane tips and vice versa). Different instructions
    // → different tx signatures → Solana's signature-uniqueness invariant
    // doesn't prevent both from landing. For DEX↔DEX arb this is safe because
    // the second landing's arb-guard check fails due to pool state changes; for
    // CEX↔DEX single-leg there's no such natural safeguard, and a double-fill
    // is a real risk. Seen in prod on 2026-04-17 (slot 413825986).
    //
    // TODO: implement nonce-based non-equivocation (durable nonce account or
    // dedicated on-chain marker) to re-enable relay fan-out safely.
    let relays: Vec<Arc<dyn Relay>> = vec![
        Arc::new(JitoRelay::new(&bot_config_relays)),
    ];

    // No ALTs for the MVP — single-leg CEX-DEX tx fits comfortably in a
    // legacy transaction.
    let alts: Vec<Arc<solana_message::AddressLookupTableAccount>> = Vec::new();

    let signer_arc = Arc::new(searcher_keypair.insecure_clone());
    let dispatcher = RelayDispatcher::new(relays, signer_arc, alts);
    dispatcher.warmup().await;

    info!("All components initialized, starting detector loop");

    // On-chain balance refresher: polls the wallet every 30s and updates the
    // inventory tracker. Without this, inventory is frozen at startup values
    // because commit() is only called by a bundle confirmation tracker (not
    // yet implemented — MVP uses optimistic release). This keeps the ratio
    // and MTM gauges truthful even between confirmations.
    let _balance_handle = {
        let inv = inventory.clone();
        let client = http_client.clone();
        let rpc = config.rpc_url.clone();
        let wallet = searcher_pubkey;
        let mut rx = shutdown_rx.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
            tick.tick().await; // immediate fire is redundant — initial balance already set
            loop {
                tokio::select! {
                    _ = rx.changed() => { if *rx.borrow() { break; } }
                    _ = tick.tick() => {
                        match fetch_initial_balances(&client, &rpc, &wallet).await {
                            Ok((sol, usdc)) => inv.set_on_chain(sol, usdc),
                            Err(e) => tracing::warn!("balance refresh failed: {}", e),
                        }
                    }
                }
            }
        })
    };

    // P&L gauge updater: samples inventory every second and pushes
    // realized/unrealized/MTM to Prometheus.
    let _pnl_handle = {
        let inv = inventory.clone();
        let store = store.clone();
        let mut rx = shutdown_rx.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
            tick.tick().await; // skip the immediate-first tick
            loop {
                tokio::select! {
                    _ = rx.changed() => {
                        if *rx.borrow() { break; }
                    }
                    _ = tick.tick() => {
                        if let Some(snap) = store.get_cex("SOLUSDC") {
                            let price = (snap.best_bid_usd + snap.best_ask_usd) / 2.0;
                            if price > 0.0 {
                                inv.set_sol_price_usd(price);
                                inv.capture_initial_value_usd_if_unset();
                                solana_mev_bot::metrics::counters::set_cexdex_sol_price_usd(price);
                            }
                        }
                        solana_mev_bot::metrics::counters::set_cexdex_realized_pnl_usd(inv.realized_pnl_usd());
                        solana_mev_bot::metrics::counters::set_cexdex_unrealized_pnl_usd(inv.unrealized_pnl_usd());
                        solana_mev_bot::metrics::counters::set_cexdex_inventory_value_usd(inv.current_value_usd());
                        solana_mev_bot::metrics::counters::set_cexdex_initial_inventory_value_usd(inv.initial_value_usd());
                        solana_mev_bot::metrics::counters::set_cexdex_inventory_ratio(inv.ratio());
                    }
                }
            }
        })
    };

    // Stats collector — emits JSON on shutdown for post-run analysis.
    let stats = Arc::new(StatsCollector::new());
    let stats_path = std::env::var("CEXDEX_STATS_PATH")
        .unwrap_or_else(|_| format!("/tmp/cexdex-run-{}", now_ms()));

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
        stats.clone(),
        http_client.clone(),
        searcher_pubkey,
    )
    .await?;

    // Snapshot final inventory for summary (from in-memory tracker).
    let final_ratio = inventory.ratio();
    let sol_final = inventory.sol_lamports_available();
    let usdc_final = inventory.usdc_atoms_available();

    match stats.finalize_to_disk(&stats_path, final_ratio, sol_final, usdc_final) {
        Ok(summary) => {
            info!(
                "=== RUN SUMMARY === duration={}s detections={} profitable={} rejected={} submitted={} | wrote {}.{{records.jsonl,summary.json}}",
                summary.duration_secs,
                summary.total_detections,
                summary.sim_profitable,
                summary.sim_rejected,
                summary.submitted,
                stats_path,
            );
        }
        Err(e) => warn!("Failed to write stats: {}", e),
    }

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
    stats: Arc<StatsCollector>,
    http_client: reqwest::Client,
    searcher_pubkey: solana_sdk::pubkey::Pubkey,
) -> Result<()> {
    let rt = tokio::runtime::Handle::current();
    let mut opportunities: u64 = 0;

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() { break; }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
        }

        while change_rx.try_recv().is_ok() {}

        if let Some(snap) = store.get_cex("SOLUSDC") {
            inventory.set_sol_price_usd(snap.mid());
        }

        let route = match detector.check_all() {
            Some(r) => r,
            None => continue,
        };

        // Snapshot the inventory + CEX state at detection time for stats.
        let inv_ratio_snap = inventory.ratio();
        let inv_sol_snap = inventory.sol_lamports_available();
        let inv_usdc_snap = inventory.usdc_atoms_available();
        let cex_snap = store.get_cex("SOLUSDC");
        let (cex_bid, cex_ask) = cex_snap
            .map(|s| (s.best_bid_usd, s.best_ask_usd))
            .unwrap_or((0.0, 0.0));
        let detected_profit = route.expected_profit_usd;
        let detected_direction = route.direction.label().to_string();
        let detected_pool = route.pool_address.to_string();
        let detected_dex = format!("{:?}", route.dex_type);
        let detected_input = route.input_amount;
        let detected_input_mint = route.input_mint.to_string();
        let detected_output = route.expected_output;
        let detected_output_mint = route.output_mint.to_string();

        // Every detector hit before sim — "opportunity" means the divergence
        // survived all detector gates (inventory, spread, min profit pre-sim).
        solana_mev_bot::metrics::counters::inc_cexdex_opportunities();

        let sim_result = simulator.simulate(&route);
        let (route, tip_lamports, min_final_output, net_profit_usd, will_submit) = match sim_result {
            SimulationResult::Profitable {
                route,
                adjusted_profit_sol,
                adjusted_profit_usd: _,  // unused until Task 9 wires it for logging
                net_profit_usd_worst_case,
                min_final_output,
            } => {
                // Task 9 will compute tip_lamports per-relay from `adjusted_profit_sol`.
                // For now stub to 0 — the dispatch path downstream must not rely on this
                // value. It gets overwritten when per-relay loop lands.
                let _ = adjusted_profit_sol; // silence unused var until Task 9 consumes it
                let tip_lamports = 0u64;
                (route, tip_lamports, min_final_output, net_profit_usd_worst_case, !config.dry_run)
            }
            SimulationResult::Unprofitable { reason } => {
                // Bucket the reject by leading keyword so the counter stays low-cardinality.
                let bucket = if reason.starts_with("below threshold") {
                    "below_min_profit"
                } else if reason.starts_with("non-positive") {
                    "non_positive_net"
                } else if reason.starts_with("not profitable") {
                    "no_gross_profit"
                } else if reason.starts_with("zero output") {
                    "zero_output"
                } else if reason.starts_with("invalid CEX") {
                    "invalid_cex_price"
                } else if reason.contains("not found") {
                    "pool_not_cached"
                } else {
                    "other"
                };
                solana_mev_bot::metrics::counters::inc_cexdex_sim_rejected(bucket);
                tracing::debug!("sim unprofitable: {}", reason);
                stats.record(OpportunityRecord {
                    ts_ms: now_ms(),
                    pool: detected_pool,
                    dex: detected_dex,
                    direction: detected_direction,
                    input_amount: detected_input,
                    input_mint: detected_input_mint,
                    expected_output: detected_output,
                    output_mint: detected_output_mint,
                    cex_bid,
                    cex_ask,
                    cex_mid: (cex_bid + cex_ask) / 2.0,
                    detected_profit_usd: detected_profit,
                    sim_net_profit_usd: None,
                    sim_tip_lamports: None,
                    sim_min_final_output: None,
                    sim_reject_reason: Some(reason),
                    inventory_ratio: inv_ratio_snap,
                    inv_sol_available: inv_sol_snap,
                    inv_usdc_available: inv_usdc_snap,
                    submitted: false,
                });
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

        // Record the profitable opportunity (submitted=false if dry_run).
        stats.record(OpportunityRecord {
            ts_ms: now_ms(),
            pool: route.pool_address.to_string(),
            dex: format!("{:?}", route.dex_type),
            direction: route.direction.label().to_string(),
            input_amount: route.input_amount,
            input_mint: route.input_mint.to_string(),
            expected_output: route.expected_output,
            output_mint: route.output_mint.to_string(),
            cex_bid,
            cex_ask,
            cex_mid: (cex_bid + cex_ask) / 2.0,
            detected_profit_usd: detected_profit,
            sim_net_profit_usd: Some(net_profit_usd),
            sim_tip_lamports: Some(tip_lamports),
            sim_min_final_output: Some(min_final_output),
            sim_reject_reason: None,
            inventory_ratio: inv_ratio_snap,
            inv_sol_available: inv_sol_snap,
            inv_usdc_available: inv_usdc_snap,
            submitted: will_submit,
        });

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

        // Belt-and-suspenders: re-verify net profit is strictly positive right
        // before dispatch. The simulator already gates on this, but checking at
        // the submission boundary gives us a clean audit trail: nothing flows to
        // the relay fan-out unless profit > 0 after tip + fee is accounted for.
        //
        // CU fees (~200_000 * 5_000 microlamports ≈ 1_000 lamports) are negligible
        // compared to the 5_000 lamport base tx fee already in sim_config, so we
        // don't double-count them here.
        let tip_sol = tip_lamports as f64 / 1e9;
        let sol_price = (cex_bid + cex_ask) / 2.0;
        let tip_usd_check = tip_sol * sol_price;
        if net_profit_usd <= 0.0 || net_profit_usd <= tip_usd_check * 0.01 {
            warn!(
                "ABORT submit: net_profit=${:.6} would not cover tip=${:.6} with margin — simulator bug?",
                net_profit_usd, tip_usd_check,
            );
            continue;
        }

        info!(
            "SUBMIT: net=${:.4}, tip={} lamports (${:.4}), margin=${:.4}",
            net_profit_usd, tip_lamports, tip_usd_check, net_profit_usd,
        );

        // Reserve inventory so concurrent detections don't double-spend.
        inventory.reserve(route.direction, route.input_amount, route.expected_output);

        // Submit via multi-relay fan-out.
        let relay_rx = dispatcher.dispatch(&instructions, tip_lamports, blockhash, &rt, None);

        // Every bundle we hand to the relay — NOT a land count. Gap vs the
        // confirmed counter shows how many were rate-limited / auction-lost /
        // tx-failed. Attempted profit is the "money left on the table" if
        // nothing actually lands.
        solana_mev_bot::metrics::counters::inc_cexdex_bundles_attempted();
        solana_mev_bot::metrics::counters::add_cexdex_attempted_profit_usd(net_profit_usd);

        // Mark this (pool, direction) + global as just-dispatched. Gates the
        // detector from firing again on the same opportunity until the
        // cooldowns expire — prevents the Geyser-tick burst from producing
        // multiple back-to-back submissions against the same pool.
        detector.mark_dispatched(route.pool_address, route.direction);

        // Spawn a confirmation tracker that credits realized P&L ONLY when the
        // bundle is confirmed landed on-chain. Previous behavior credited at
        // dispatch time, which was optimistic and produced false positives
        // whenever bundles were rate-limited or lost the Jito auction.
        {
            let inv_cb = inventory.clone();
            let net = net_profit_usd;
            let on_landed: solana_mev_bot::executor::confirmation::OnLandedCallback =
                Box::new(move || {
                    inv_cb.add_realized_pnl_usd(net);
                    solana_mev_bot::metrics::counters::inc_cexdex_bundles_confirmed();
                });
            let confirm_jito = format!(
                "{}/api/v1/bundles",
                config.jito_block_engine_url.trim_end_matches('/')
            );
            let profit_lamports = (net_profit_usd / (cex_bid + cex_ask) * 2.0 * 1e9) as u64;
            solana_mev_bot::executor::spawn_confirmation_tracker(
                http_client.clone(),
                confirm_jito,
                profit_lamports,
                tip_lamports,
                relay_rx,
                config.rpc_url.clone(),
                route.pool_address.to_string(),
                route.observed_slot,
                Some(on_landed),
            );
        }

        // Fast on-chain balance refresh ~3s after dispatch. Gives the Grafana
        // `inventory_ratio` + MTM gauges a timely update when a bundle lands
        // without waiting for the 30s periodic poll. No-op if the bundle
        // didn't land — we just re-read the unchanged balance.
        {
            let inv = inventory.clone();
            let client = http_client.clone();
            let rpc = config.rpc_url.clone();
            let wallet = searcher_pubkey;
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                if let Ok((sol, usdc)) = fetch_initial_balances(&client, &rpc, &wallet).await {
                    inv.set_on_chain(sol, usdc);
                }
            });
        }

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
