use anyhow::Result;
use crossbeam_channel::Sender;
use solana_sdk::pubkey::Pubkey;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{info, warn, error};

use crate::config::BotConfig;
use crate::mempool::decoder::SwapDecoder;
use crate::router::pool::DetectedSwap;
use crate::state::StateCache;

/// Streams pending transactions from Jito's gRPC mempool.
///
/// Architecture:
/// - Subscribes to `programSubscribe` on target DEX programs
/// - Receives transaction notifications before they land on-chain
/// - Decodes swap instructions and pushes DetectedSwap to the channel
/// - The route calculator consumes from the other end
///
/// This is the most latency-sensitive component. Every microsecond
/// between seeing a tx and forwarding it to the router matters.
pub struct MempoolStream {
    config: Arc<BotConfig>,
    state_cache: StateCache,
    decoder: SwapDecoder,
}

impl MempoolStream {
    pub fn new(config: Arc<BotConfig>, state_cache: StateCache) -> Self {
        Self {
            config,
            state_cache,
            decoder: SwapDecoder::new(),
        }
    }

    /// Start streaming mempool transactions.
    ///
    /// Sends decoded swaps to `tx_sender` for downstream processing.
    /// Respects `shutdown_rx` for graceful termination.
    pub async fn start(
        &self,
        tx_sender: Sender<DetectedSwap>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> Result<()> {
        let programs = self.config.monitored_programs();
        info!(
            "Starting mempool stream, monitoring {} programs",
            programs.len()
        );

        // In production, this connects to Jito's gRPC endpoint:
        // let channel = tonic::transport::Channel::from_shared(self.config.jito_block_engine_url.clone())?
        //     .connect()
        //     .await?;
        //
        // For now, we use the subscription pattern that jito-rs provides.
        // The actual gRPC subscription looks like:
        //
        // let mut client = SearcherServiceClient::new(channel);
        // let sub = client.subscribe_mempool(MempoolSubscription {
        //     program_v0_sub: Some(ProgramSubscriptionV0 {
        //         programs: programs.iter().map(|p| p.to_string()).collect(),
        //     }),
        //     ..Default::default()
        // }).await?;

        info!("Mempool stream: connecting to {}", self.config.jito_block_engine_url);

        // Main event loop — process incoming mempool transactions
        self.stream_loop(&programs, tx_sender, shutdown_rx).await
    }

    async fn stream_loop(
        &self,
        programs: &[Pubkey],
        tx_sender: Sender<DetectedSwap>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> Result<()> {
        // TODO: Replace with actual Jito gRPC stream
        // This is the skeleton that will be filled in with the real
        // SearcherServiceClient subscription.
        //
        // The pattern:
        // 1. Connect to Jito block engine gRPC
        // 2. Subscribe to mempool with program filters
        // 3. For each incoming tx notification:
        //    a. Decode the transaction
        //    b. Check if it contains a swap instruction for our target DEXes
        //    c. If yes, create a DetectedSwap and send it to the channel
        //    d. Update state cache with any account changes we can infer

        info!("Mempool stream loop started, waiting for transactions...");

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Mempool stream: shutdown signal received");
                        break;
                    }
                }
                // In production, this arm would be:
                // Some(notification) = stream.next() => { ... }
                //
                // For scaffold, we sleep to prevent busy-loop
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => {
                    // Placeholder: will be replaced with actual gRPC stream processing
                }
            }
        }

        info!("Mempool stream loop exited");
        Ok(())
    }

    /// Process a raw mempool transaction notification.
    /// This is called for each transaction that touches one of our monitored programs.
    ///
    /// Returns a DetectedSwap if the transaction contains a decodable swap.
    fn process_mempool_tx(
        &self,
        tx_data: &[u8],
        slot: u64,
    ) -> Option<DetectedSwap> {
        // Decode the transaction
        let swap = self.decoder.decode_swap(tx_data)?;

        // Enrich with slot info
        Some(DetectedSwap {
            observed_slot: slot,
            ..swap
        })
    }
}

/// Stats tracked for monitoring mempool stream health.
#[derive(Debug, Default)]
pub struct StreamStats {
    pub txs_received: u64,
    pub txs_decoded: u64,
    pub txs_relevant: u64,
    pub decode_errors: u64,
    pub channel_full_drops: u64,
}
