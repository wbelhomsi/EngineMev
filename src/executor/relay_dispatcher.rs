use std::sync::Arc;
use solana_sdk::{
    address_lookup_table::AddressLookupTableAccount,
    hash::Hash, instruction::Instruction, signature::Keypair,
};
use tracing::{info, warn};

use super::relays::Relay;

/// Dispatches bundles to all configured relays concurrently.
///
/// Each relay independently appends its own tip, signs its own transaction,
/// and submits via HTTP. No relay waits for any other relay.
pub struct RelayDispatcher {
    relays: Vec<Arc<dyn Relay>>,
    signer: Arc<Keypair>,
    alt: Option<Arc<AddressLookupTableAccount>>,
}

impl RelayDispatcher {
    pub fn new(
        relays: Vec<Arc<dyn Relay>>,
        signer: Arc<Keypair>,
        alt: Option<Arc<AddressLookupTableAccount>>,
    ) -> Self {
        Self { relays, signer, alt }
    }

    pub fn signer(&self) -> Arc<Keypair> {
        self.signer.clone()
    }

    /// Fire all configured relays concurrently. No relay waits for another.
    /// Each relay task logs its own result. Returns immediately.
    pub fn dispatch(
        &self,
        base_instructions: &[Instruction],
        tip_lamports: u64,
        recent_blockhash: Hash,
        rt: &tokio::runtime::Handle,
    ) {
        for relay in &self.relays {
            if !relay.is_configured() {
                continue;
            }
            let relay = relay.clone();
            let ixs = base_instructions.to_vec();
            let signer = self.signer.clone();
            let tip = tip_lamports;
            let bh = recent_blockhash;
            let alt = self.alt.clone();
            rt.spawn(async move {
                let result = relay.submit(&ixs, tip, &signer, bh, alt.as_deref()).await;
                if result.success {
                    info!(
                        "Bundle accepted by {}: id={:?} latency={}us",
                        result.relay_name, result.bundle_id, result.latency_us
                    );
                } else if let Some(ref err) = result.error {
                    warn!(
                        "Bundle REJECTED by {}: {} (latency={}us)",
                        result.relay_name, err, result.latency_us
                    );
                }
            });
        }
    }

    /// Warm up connections — log which relays are configured.
    pub async fn warmup(&self) {
        for relay in &self.relays {
            if relay.is_configured() {
                info!("Relay configured: {}", relay.name());
            }
        }
    }
}
