//! Narrow Geyser subscription for the cexdex binary.
//!
//! Wraps the shared `GeyserStream` with a cexdex-specific `BotConfig` and
//! ties it to the `PriceStore`'s pool cache.  The "narrow" part is semantic:
//! the underlying stream subscribes by DEX program owner (same as the main
//! engine), but the pool state lands in an independent `StateCache` owned by
//! the cexdex binary, so the two pipelines don't share mutable state.

use std::sync::Arc;

use anyhow::Result;
use crossbeam_channel::Sender;
use solana_sdk::pubkey::Pubkey;
use tokio::sync::watch;
use tracing::info;

use crate::config::{BotConfig, RelayEndpoints};
use crate::mempool::{GeyserStream, PoolStateChange, SubscriptionMode};
use crate::feed::PriceStore;

/// Build a `BotConfig` suitable for the cexdex Geyser subscription.
///
/// Only the Geyser and RPC fields are populated — relay endpoints, tip
/// parameters, and arb-guard fields are left at zero/empty/None because the
/// cexdex binary does not submit bundles through the main relay stack.
pub fn narrow_bot_config(
    geyser_grpc_url: String,
    geyser_auth_token: String,
    rpc_url: String,
    pool_state_ttl: std::time::Duration,
) -> BotConfig {
    BotConfig {
        geyser_grpc_url,
        geyser_auth_token,
        rpc_url,

        // Relay / keypair fields — unused by cexdex binary
        jito_block_engine_url: String::new(),
        searcher_keypair_path: String::new(),
        relay_endpoints: RelayEndpoints {
            jito: String::new(),
            nozomi: None,
            bloxroute: None,
            astralane: None,
            zeroslot: None,
        },

        // Economics — not used for cexdex opportunity detection
        tip_fraction: 0.5,
        min_profit_lamports: 0,
        min_tip_lamports: 0,
        max_hops: 2,
        pool_state_ttl,
        slippage_tolerance: 0.25,

        // Feature flags
        dry_run: true,
        lst_arb_enabled: false,
        lst_min_spread_bps: 0,

        // Optional infrastructure
        arb_guard_program_id: None,
        metrics_port: None,
        otlp_endpoint: None,
        otlp_service_name: "cexdex".to_string(),
    }
}

/// Spawn a `GeyserStream` task backed by the `PriceStore`'s pool cache.
///
/// Pool state updates received from Geyser are written into `store.pools`
/// (the cexdex `StateCache`) and a `PoolStateChange` signal is sent on
/// `change_tx` so the detector loop knows which pool to re-evaluate.
///
/// `monitored_pools` narrows the LaserStream subscription to exactly those
/// account pubkeys — cexdex does not need the wide DEX-program-owner
/// subscription that the main engine uses for lazy pool discovery.
///
/// `nonce_pool` adds the managed nonce accounts to the subscription list so
/// Geyser delivers real-time hash updates; `searcher_pubkey` is used as a
/// sanity-check authority on each parsed nonce update.
///
/// Returns a `JoinHandle` for the spawned task.  The task exits when
/// `shutdown_rx` fires or the stream encounters a fatal error.
pub async fn start_geyser(
    config: BotConfig,
    store: PriceStore,
    http_client: reqwest::Client,
    monitored_pools: Vec<Pubkey>,
    nonce_pool: crate::cexdex::NoncePool,
    searcher_pubkey: solana_sdk::pubkey::Pubkey,
    change_tx: Sender<PoolStateChange>,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<tokio::task::JoinHandle<()>> {
    // Combined subscription list: pools + nonces.
    let mut subscription: Vec<Pubkey> = monitored_pools.clone();
    subscription.extend(nonce_pool.pubkeys());

    let pool_count = store.pools.len();
    info!(
        "Starting narrow Geyser (cexdex): {} pools in cache, {} pool accounts + {} nonce accounts = {} monitored total",
        pool_count,
        monitored_pools.len(),
        nonce_pool.len(),
        subscription.len(),
    );

    let stream = GeyserStream::new(Arc::new(config), store.pools.clone(), http_client)
        .with_subscription_mode(SubscriptionMode::SpecificAccounts(subscription))
        .with_nonce_pool(nonce_pool, searcher_pubkey);

    let handle = tokio::spawn(async move {
        if let Err(e) = stream.start(change_tx, shutdown_rx).await {
            tracing::error!("cexdex Geyser stream exited: {e}");
        }
    });

    Ok(handle)
}
