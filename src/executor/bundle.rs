use anyhow::Result;
use solana_sdk::{
    hash::Hash,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    system_instruction,
    transaction::Transaction,
};
use tracing::{debug, info};

use crate::router::pool::{ArbRoute, DexType};

/// Jito tip accounts — bundles must include a SOL transfer to one of these.
/// These are fetched via getTipAccounts RPC but hardcoded as fallback.
const JITO_TIP_ACCOUNTS: &[&str] = &[
    "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
    "HFqU5x63VTqvQss8hp11i4bPKELzFLDELBGnNYpzHCDf",
    "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
    "ADaUMid9yfUytqMBgopwjb2DTLSLzzWw1pa8U5j7cUi2",
    "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
    "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
    "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
    "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
];

/// Jito tip floor API endpoints for dynamic tip pricing.
const JITO_TIP_FLOOR_REST: &str = "https://bundles-api-rest.jito.wtf/api/v1/bundles/tip_floor";

/// Builds Jito-compatible transaction bundles for backrun arbitrage.
///
/// Post-mempool architecture (2024+):
/// We no longer backrun a pending tx in the same bundle.
/// Instead, we observe a state change via Geyser and submit a standalone
/// arb bundle for the next slot. The bundle contains:
/// 1. Our arbitrage transaction(s)
/// 2. A tip transaction to a Jito tip account
///
/// Tip strategy:
/// - Query tip floor API for current minimum
/// - Bid tip_fraction * estimated_profit, floored at the Jito minimum (1000 lamports)
/// - Auctions happen every 200ms
pub struct BundleBuilder {
    searcher_keypair: Keypair,
    /// Index into tip accounts, rotated per bundle
    tip_account_index: std::sync::atomic::AtomicUsize,
}

impl BundleBuilder {
    pub fn new(searcher_keypair: Keypair) -> Self {
        Self {
            searcher_keypair,
            tip_account_index: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Build a standalone arb bundle (no target tx — we're post-block, not same-block).
    ///
    /// `route` - the profitable arb route to execute
    /// `tip_lamports` - SOL tip (should be >= tip floor from API)
    /// `recent_blockhash` - current blockhash for transaction validity
    pub fn build_arb_bundle(
        &self,
        route: &ArbRoute,
        tip_lamports: u64,
        recent_blockhash: Hash,
    ) -> Result<Vec<Vec<u8>>> {
        let mut bundle_txs: Vec<Vec<u8>> = Vec::with_capacity(2);

        // 1. Arb transaction with tip included as last instruction
        let arb_tx = self.build_arb_transaction_with_tip(route, tip_lamports, recent_blockhash)?;
        bundle_txs.push(bincode::serialize(&arb_tx)?);

        debug!(
            "Built bundle: {} txs, tip={} lamports, route={} hops",
            bundle_txs.len(),
            tip_lamports,
            route.hop_count(),
        );

        Ok(bundle_txs)
    }

    /// Build arb transaction with tip as last instruction (saves a tx in the bundle).
    fn build_arb_transaction_with_tip(
        &self,
        route: &ArbRoute,
        tip_lamports: u64,
        recent_blockhash: Hash,
    ) -> Result<Transaction> {
        let mut instructions = Vec::with_capacity(route.hop_count() + 1);

        // Swap instructions
        for hop in &route.hops {
            let ix = self.build_swap_instruction(hop)?;
            instructions.push(ix);
        }

        // Tip instruction as last ix in the same tx
        let tip_ix = self.build_tip_instruction(tip_lamports)?;
        instructions.push(tip_ix);

        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&self.searcher_keypair.pubkey()),
            &[&self.searcher_keypair],
            recent_blockhash,
        );

        Ok(tx)
    }

    /// Build a single swap instruction for one hop.
    fn build_swap_instruction(
        &self,
        hop: &crate::router::pool::RouteHop,
    ) -> Result<Instruction> {
        match hop.dex_type {
            DexType::RaydiumAmm => self.build_raydium_amm_swap(hop),
            DexType::RaydiumClmm => self.build_raydium_clmm_swap(hop),
            DexType::OrcaWhirlpool => self.build_orca_whirlpool_swap(hop),
            DexType::MeteoraDlmm => self.build_meteora_dlmm_swap(hop),
        }
    }

    /// Raydium AMM v4 swap.
    ///
    /// Supports both V1 (18 accounts, discriminator 9) and V2 (8 accounts).
    /// V2 deployed Sept 2025 — removes OpenBook market accounts.
    /// Both V1 and V2 remain functional. We use V2 when possible (fewer accounts = smaller tx).
    fn build_raydium_amm_swap(
        &self,
        hop: &crate::router::pool::RouteHop,
    ) -> Result<Instruction> {
        // Swap V2 instruction data: discriminator + amount_in + minimum_amount_out
        // minimum_amount_out = 0 for arb — bundle atomicity protects us
        let mut data = vec![9u8]; // swap discriminator (same for V1/V2)
        data.extend_from_slice(&hop.estimated_output.to_le_bytes());
        data.extend_from_slice(&0u64.to_le_bytes());

        // TODO: populate full account list from cached pool state
        // V2 accounts (8): token_program, amm_id, amm_authority, amm_open_orders,
        //   pool_coin_vault, pool_pc_vault, user_source, user_dest
        Ok(Instruction {
            program_id: crate::config::programs::raydium_amm(),
            accounts: vec![
                AccountMeta::new_readonly(hop.pool_address, false),
            ],
            data,
        })
    }

    fn build_raydium_clmm_swap(
        &self,
        hop: &crate::router::pool::RouteHop,
    ) -> Result<Instruction> {
        // TODO: Anchor-encoded swap with tick array accounts
        Ok(Instruction {
            program_id: crate::config::programs::raydium_clmm(),
            accounts: vec![
                AccountMeta::new_readonly(hop.pool_address, false),
            ],
            data: vec![],
        })
    }

    fn build_orca_whirlpool_swap(
        &self,
        hop: &crate::router::pool::RouteHop,
    ) -> Result<Instruction> {
        // TODO: Anchor-encoded swap with tick array + oracle accounts
        Ok(Instruction {
            program_id: crate::config::programs::orca_whirlpool(),
            accounts: vec![
                AccountMeta::new_readonly(hop.pool_address, false),
            ],
            data: vec![],
        })
    }

    fn build_meteora_dlmm_swap(
        &self,
        hop: &crate::router::pool::RouteHop,
    ) -> Result<Instruction> {
        // TODO: DLMM bin-step swap encoding
        Ok(Instruction {
            program_id: crate::config::programs::meteora_dlmm(),
            accounts: vec![
                AccountMeta::new_readonly(hop.pool_address, false),
            ],
            data: vec![],
        })
    }

    /// Build tip instruction to a Jito tip account (rotated per bundle).
    fn build_tip_instruction(&self, tip_lamports: u64) -> Result<Instruction> {
        let idx = self.tip_account_index.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            % JITO_TIP_ACCOUNTS.len();
        let tip_pubkey: Pubkey = JITO_TIP_ACCOUNTS[idx].parse()?;

        Ok(system_instruction::transfer(
            &self.searcher_keypair.pubkey(),
            &tip_pubkey,
            tip_lamports,
        ))
    }

    /// Fetch the current Jito tip floor from the REST API.
    /// Returns the minimum tip in lamports needed for bundle inclusion.
    pub async fn fetch_tip_floor() -> Result<u64> {
        let client = reqwest::Client::new();
        let resp = client
            .get(JITO_TIP_FLOOR_REST)
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        // Parse tip floor from response
        // Response format: [{"time":"...","landed_tips_25th_percentile":...,"landed_tips_50th_percentile":...}]
        let tip_floor = resp
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|entry| entry.get("landed_tips_50th_percentile"))
            .and_then(|v| v.as_f64())
            .map(|sol| (sol * 1_000_000_000.0) as u64) // SOL to lamports
            .unwrap_or(1_000); // Fallback: 1000 lamports minimum

        debug!("Current Jito tip floor: {} lamports", tip_floor);
        Ok(tip_floor)
    }
}
