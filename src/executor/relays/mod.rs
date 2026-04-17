pub mod common;
pub mod jito;
pub mod astralane;
pub mod nozomi;
pub mod bloxroute;
pub mod zeroslot;

use solana_message::AddressLookupTableAccount;
use solana_sdk::{
    hash::Hash,
    instruction::Instruction,
    signature::Keypair,
};

/// Result from a relay submission attempt.
#[derive(Debug)]
pub struct RelayResult {
    pub relay_name: String,
    pub bundle_id: Option<String>,
    pub success: bool,
    pub latency_us: u64,
    pub error: Option<String>,
}

/// Every relay implements this trait. Each relay independently:
/// - Checks its own rate limit
/// - Appends its own tip instruction
/// - Signs the transaction
/// - Serializes and sends via HTTP
/// No relay waits for any other relay.
#[async_trait::async_trait]
pub trait Relay: Send + Sync {
    fn name(&self) -> &str;
    fn is_configured(&self) -> bool;
    async fn submit(
        &self,
        base_instructions: &[Instruction],
        tip_lamports: u64,
        signer: &Keypair,
        recent_blockhash: Hash,
        alts: &[&AddressLookupTableAccount],
        nonce: Option<crate::cexdex::NonceInfo>,
    ) -> RelayResult;
}
