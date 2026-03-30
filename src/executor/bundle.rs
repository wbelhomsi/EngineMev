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
use tracing::debug;

use crate::router::pool::{ArbRoute, DexType};

/// Jito tip accounts — bundles must include a tip transfer to one of these.
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

/// Builds Jito-compatible transaction bundles for backrun arbitrage.
///
/// A bundle contains:
/// 1. The target transaction (the swap we're backrunning)
/// 2. Our arbitrage transaction(s)
/// 3. A tip transaction to a Jito tip account
///
/// All three execute atomically — if any fails, none land on-chain.
pub struct BundleBuilder {
    searcher_keypair: Keypair,
}

impl BundleBuilder {
    pub fn new(searcher_keypair: Keypair) -> Self {
        Self { searcher_keypair }
    }

    /// Build a complete bundle for a profitable arbitrage route.
    ///
    /// `target_tx` - the raw serialized transaction we're backrunning
    /// `route` - the profitable arb route to execute
    /// `tip_lamports` - SOL tip to include for Jito validators
    /// `recent_blockhash` - current blockhash for transaction validity
    pub fn build_bundle(
        &self,
        target_tx: &[u8],
        route: &ArbRoute,
        tip_lamports: u64,
        recent_blockhash: Hash,
    ) -> Result<Vec<Vec<u8>>> {
        let mut bundle_txs: Vec<Vec<u8>> = Vec::with_capacity(3);

        // 1. Target transaction (the swap we're backrunning) — already serialized
        bundle_txs.push(target_tx.to_vec());

        // 2. Our arbitrage transaction
        let arb_tx = self.build_arb_transaction(route, recent_blockhash)?;
        bundle_txs.push(bincode::serialize(&arb_tx)?);

        // 3. Tip transaction
        let tip_tx = self.build_tip_transaction(tip_lamports, recent_blockhash)?;
        bundle_txs.push(bincode::serialize(&tip_tx)?);

        debug!(
            "Built bundle: {} txs, tip={} lamports, route={} hops",
            bundle_txs.len(),
            tip_lamports,
            route.hop_count(),
        );

        Ok(bundle_txs)
    }

    /// Build the arbitrage swap transaction.
    ///
    /// This constructs the actual swap instructions for each hop in the route.
    fn build_arb_transaction(
        &self,
        route: &ArbRoute,
        recent_blockhash: Hash,
    ) -> Result<Transaction> {
        let mut instructions = Vec::with_capacity(route.hop_count());

        for hop in &route.hops {
            let ix = self.build_swap_instruction(hop)?;
            instructions.push(ix);
        }

        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&self.searcher_keypair.pubkey()),
            &[&self.searcher_keypair],
            recent_blockhash,
        );

        Ok(tx)
    }

    /// Build a single swap instruction for one hop.
    ///
    /// Each DEX has its own instruction format. This dispatches to
    /// the appropriate builder based on DEX type.
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

    fn build_raydium_amm_swap(
        &self,
        hop: &crate::router::pool::RouteHop,
    ) -> Result<Instruction> {
        // Raydium AMM swap instruction (discriminator = 9)
        // Full account list must be populated from on-chain pool state.
        // Placeholder structure — the actual accounts come from pool state cache.
        let mut data = vec![9u8]; // swap discriminator
        data.extend_from_slice(&hop.estimated_output.to_le_bytes()); // amount_in
        data.extend_from_slice(&0u64.to_le_bytes()); // minimum_amount_out (set to 0 for arb — bundle atomicity protects us)

        Ok(Instruction {
            program_id: crate::config::programs::raydium_amm(),
            accounts: vec![
                // TODO: populate from cached pool state
                // These account metas are placeholders
                AccountMeta::new_readonly(hop.pool_address, false),
            ],
            data,
        })
    }

    fn build_raydium_clmm_swap(
        &self,
        hop: &crate::router::pool::RouteHop,
    ) -> Result<Instruction> {
        // Raydium CLMM uses Anchor-style instruction encoding
        // The exact accounts depend on the pool's tick arrays
        Ok(Instruction {
            program_id: crate::config::programs::raydium_clmm(),
            accounts: vec![
                AccountMeta::new_readonly(hop.pool_address, false),
            ],
            data: vec![], // TODO: Anchor-encoded swap instruction
        })
    }

    fn build_orca_whirlpool_swap(
        &self,
        hop: &crate::router::pool::RouteHop,
    ) -> Result<Instruction> {
        // Orca Whirlpool swap instruction (Anchor)
        // Requires: whirlpool, token_authority, token_owner_a/b, vault_a/b, tick_arrays, oracle
        Ok(Instruction {
            program_id: crate::config::programs::orca_whirlpool(),
            accounts: vec![
                AccountMeta::new_readonly(hop.pool_address, false),
            ],
            data: vec![], // TODO: Anchor-encoded swap instruction
        })
    }

    fn build_meteora_dlmm_swap(
        &self,
        hop: &crate::router::pool::RouteHop,
    ) -> Result<Instruction> {
        // Meteora DLMM swap instruction
        Ok(Instruction {
            program_id: crate::config::programs::meteora_dlmm(),
            accounts: vec![
                AccountMeta::new_readonly(hop.pool_address, false),
            ],
            data: vec![], // TODO: DLMM-specific swap encoding
        })
    }

    /// Build the tip transaction to a Jito tip account.
    fn build_tip_transaction(
        &self,
        tip_lamports: u64,
        recent_blockhash: Hash,
    ) -> Result<Transaction> {
        // Pick a tip account (rotate to distribute load)
        let tip_account_str = JITO_TIP_ACCOUNTS[0]; // TODO: rotate
        let tip_pubkey: Pubkey = tip_account_str.parse()?;

        let tip_ix = system_instruction::transfer(
            &self.searcher_keypair.pubkey(),
            &tip_pubkey,
            tip_lamports,
        );

        let tx = Transaction::new_signed_with_payer(
            &[tip_ix],
            Some(&self.searcher_keypair.pubkey()),
            &[&self.searcher_keypair],
            recent_blockhash,
        );

        Ok(tx)
    }
}
