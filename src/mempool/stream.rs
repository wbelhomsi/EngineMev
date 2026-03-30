use anyhow::Result;
use crossbeam_channel::Sender;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{info, warn, error, debug};
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof,
    CommitmentLevel, SubscribeRequest, SubscribeRequestFilterAccounts,
};

use crate::config::BotConfig;
use crate::state::StateCache;

/// Event emitted when a pool's on-chain state changes.
///
/// This replaces the old DetectedSwap from the dead Jito mempool API.
/// We no longer see pending transactions — we see the *result* of swaps
/// via account state changes pushed by Yellowstone Geyser.
#[derive(Debug, Clone)]
pub struct PoolStateChange {
    /// Token vault account that changed
    pub vault_address: Pubkey,
    /// New vault balance (token amount)
    pub new_balance: u64,
    /// Slot this change was observed in
    pub slot: u64,
}

/// Streams pool state changes via Yellowstone gRPC Geyser plugin.
///
/// Architecture (post Jito mempool shutdown, March 2024):
///
/// OLD (dead): Jito subscribe_mempool → see pending tx → backrun in same bundle
/// NEW (current): Geyser account stream → see vault balance change → submit arb next slot
///
/// Yellowstone gRPC streams account updates directly from validator memory
/// at sub-50ms latency vs 100-300ms for standard WebSocket.
///
/// Flow:
/// 1. Subscribe to token vault accounts owned by target DEX programs
/// 2. When vault balances change (someone swapped), emit PoolStateChange
/// 3. Downstream router detects price dislocation across DEXes
/// 4. Bundle submitted for next slot via multi-relay fan-out
pub struct GeyserStream {
    config: Arc<BotConfig>,
    state_cache: StateCache,
}

impl GeyserStream {
    pub fn new(config: Arc<BotConfig>, state_cache: StateCache) -> Self {
        Self {
            config,
            state_cache,
        }
    }

    /// Start streaming pool state changes via Yellowstone gRPC.
    pub async fn start(
        &self,
        tx_sender: Sender<PoolStateChange>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> Result<()> {
        let programs = self.config.monitored_programs();
        info!(
            "Starting Geyser stream, monitoring {} DEX programs",
            programs.len()
        );

        // Connect to Yellowstone gRPC endpoint
        let mut client = GeyserGrpcClient::build_from_shared(
            self.config.geyser_grpc_url.clone(),
        )?
        .x_token(if self.config.geyser_auth_token.is_empty() {
            None
        } else {
            Some(self.config.geyser_auth_token.clone())
        })?
        .connect()
        .await?;

        info!("Connected to Geyser gRPC at {}", self.config.geyser_grpc_url);

        // Build subscription: watch all accounts owned by target DEX programs.
        // When a swap happens, the pool's token vault accounts get updated.
        let mut accounts_filter: HashMap<String, SubscribeRequestFilterAccounts> = HashMap::new();

        for (i, program_id) in programs.iter().enumerate() {
            accounts_filter.insert(
                format!("dex_{}", i),
                SubscribeRequestFilterAccounts {
                    account: vec![],
                    owner: vec![program_id.to_string()],
                    filters: vec![],
                    nonempty_txn_signature: None,
                },
            );
        }

        let subscribe_request = SubscribeRequest {
            accounts: accounts_filter,
            commitment: Some(CommitmentLevel::Processed as i32),
            ..Default::default()
        };

        let (_, mut stream) = client.subscribe_with_request(Some(subscribe_request)).await?;

        info!("Geyser subscription active, waiting for account updates...");

        // Main event loop
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Geyser stream: shutdown signal received");
                        break;
                    }
                }
                msg = Self::next_message(&mut stream) => {
                    match msg {
                        Some(update) => {
                            self.process_update(update, &tx_sender);
                        }
                        None => {
                            warn!("Geyser stream ended, needs reconnect");
                            break; // Caller should implement reconnect loop
                        }
                    }
                }
            }
        }

        info!("Geyser stream loop exited");
        Ok(())
    }

    /// Extract next message from the gRPC stream.
    async fn next_message(
        stream: &mut tonic::Streaming<yellowstone_grpc_proto::prelude::SubscribeUpdate>,
    ) -> Option<yellowstone_grpc_proto::prelude::SubscribeUpdate> {
        use tokio_stream::StreamExt;
        stream.next().await?.ok()
    }

    /// Process a Geyser account update.
    ///
    /// For SPL Token accounts (token vaults), the balance sits at bytes 64..72.
    /// When this changes, a swap just happened on the parent pool.
    fn process_update(
        &self,
        update: yellowstone_grpc_proto::prelude::SubscribeUpdate,
        tx_sender: &Sender<PoolStateChange>,
    ) {
        let Some(update_oneof) = update.update_oneof else {
            return;
        };

        match update_oneof {
            UpdateOneof::Account(account_update) => {
                let Some(account_info) = account_update.account else {
                    return;
                };

                let slot = account_update.slot;

                // Parse account pubkey (32 bytes)
                let pubkey_bytes: [u8; 32] = match account_info.pubkey.try_into() {
                    Ok(b) => b,
                    Err(_) => return,
                };
                let account_pubkey = Pubkey::new_from_array(pubkey_bytes);

                // SPL Token account layout: balance (amount) is at offset 64, 8 bytes LE
                let data = &account_info.data;
                if data.len() >= 72 {
                    let balance = u64::from_le_bytes(
                        data[64..72].try_into().unwrap_or_default()
                    );

                    let event = PoolStateChange {
                        vault_address: account_pubkey,
                        new_balance: balance,
                        slot,
                    };

                    // Non-blocking send. If channel is full, drop — stale events are worthless.
                    if let Err(e) = tx_sender.try_send(event) {
                        debug!("Channel full, dropping state change: {}", e);
                    }
                }
            }
            _ => {} // Ignore slot/block/tx-level updates
        }
    }
}

/// Stats for monitoring Geyser stream health.
#[derive(Debug, Default)]
pub struct StreamStats {
    pub account_updates_received: u64,
    pub vault_changes_detected: u64,
    pub channel_full_drops: u64,
    pub reconnects: u64,
}
