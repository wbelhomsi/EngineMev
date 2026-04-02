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
use std::str::FromStr;
use tracing::debug;

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
    state_cache: crate::state::StateCache,
}

impl BundleBuilder {
    pub fn new(searcher_keypair: Keypair, state_cache: crate::state::StateCache) -> Self {
        Self {
            searcher_keypair,
            tip_account_index: std::sync::atomic::AtomicUsize::new(0),
            state_cache,
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

        // Calculate minimum output for profit enforcement on final hop
        let min_final_output = route.input_amount + route.estimated_profit_lamports.saturating_sub(tip_lamports);

        let arb_tx = self.build_arb_transaction_with_tip(route, tip_lamports, min_final_output, recent_blockhash)?;
        bundle_txs.push(bincode::serialize(&arb_tx)?);

        debug!(
            "Built bundle: {} txs, tip={} lamports, min_out={}, route={} hops",
            bundle_txs.len(),
            tip_lamports,
            min_final_output,
            route.hop_count(),
        );

        Ok(bundle_txs)
    }

    /// Build arb transaction with tip as last instruction.
    /// `min_final_output` is set on the final hop to guarantee profit on-chain.
    fn build_arb_transaction_with_tip(
        &self,
        route: &ArbRoute,
        tip_lamports: u64,
        min_final_output: u64,
        recent_blockhash: Hash,
    ) -> Result<Transaction> {
        let mut instructions = Vec::with_capacity(route.hop_count() + 1);

        // Swap instructions — intermediate hops get min_out=0, final hop gets profit floor
        let last_idx = route.hops.len() - 1;
        for (i, hop) in route.hops.iter().enumerate() {
            let min_out = if i == last_idx { min_final_output } else { 0 };
            let ix = self.build_swap_instruction_with_min_out(hop, min_out)?;
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

    /// Build a single swap instruction for one hop with minimum_amount_out.
    fn build_swap_instruction_with_min_out(
        &self,
        hop: &crate::router::pool::RouteHop,
        minimum_amount_out: u64,
    ) -> Result<Instruction> {
        match hop.dex_type {
            DexType::RaydiumAmm => self.build_raydium_amm_swap(hop, minimum_amount_out),
            DexType::RaydiumClmm => self.build_raydium_clmm_swap(hop),
            DexType::RaydiumCp => {
                let pool = self.state_cache.get_any(&hop.pool_address)
                    .ok_or_else(|| anyhow::anyhow!("Pool not found for Raydium CP: {}", hop.pool_address))?;
                build_raydium_cp_swap_ix(
                    &self.searcher_keypair.pubkey(), &pool, hop.input_mint,
                    hop.estimated_output, minimum_amount_out,
                ).ok_or_else(|| anyhow::anyhow!("Missing pool data for Raydium CP"))
            }
            DexType::OrcaWhirlpool => self.build_orca_whirlpool_swap(hop),
            DexType::MeteoraDlmm => self.build_meteora_dlmm_swap(hop),
            DexType::MeteoraDammV2 => {
                let pool = self.state_cache.get_any(&hop.pool_address)
                    .ok_or_else(|| anyhow::anyhow!("Pool not found for DAMM v2: {}", hop.pool_address))?;
                build_damm_v2_swap_ix(
                    &self.searcher_keypair.pubkey(), &pool, hop.input_mint,
                    hop.estimated_output, minimum_amount_out,
                ).ok_or_else(|| anyhow::anyhow!("Missing pool data for DAMM v2"))
            }
            DexType::SanctumInfinity => self.build_sanctum_swap(hop, minimum_amount_out),
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
        minimum_amount_out: u64,
    ) -> Result<Instruction> {
        // Swap V2 instruction data: discriminator + amount_in + minimum_amount_out
        let mut data = vec![9u8]; // swap discriminator (same for V1/V2)
        data.extend_from_slice(&hop.estimated_output.to_le_bytes());
        data.extend_from_slice(&minimum_amount_out.to_le_bytes());

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

    fn build_sanctum_swap(
        &self,
        hop: &crate::router::pool::RouteHop,
        minimum_amount_out: u64,
    ) -> Result<Instruction> {
        let accounts = sanctum_swap_accounts(
            &self.searcher_keypair.pubkey(),
            &hop.input_mint,
            &hop.output_mint,
        );

        // SwapExactIn instruction data: discriminator (8 bytes) + amount (u64) + min_out (u64)
        let mut data = Vec::with_capacity(24);
        // Anchor discriminator for "swap_exact_in": sha256("global:swap_exact_in")[..8]
        data.extend_from_slice(&[0x0a, 0xd3, 0xc8, 0x1a, 0x3e, 0x4d, 0x2b, 0x1c]);
        data.extend_from_slice(&hop.estimated_output.to_le_bytes()); // amount_in
        data.extend_from_slice(&minimum_amount_out.to_le_bytes()); // profit enforcement on final hop

        Ok(Instruction {
            program_id: crate::config::programs::sanctum_s_controller(),
            accounts,
            data,
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

/// Derive an Associated Token Account address.
/// ATA = PDA([wallet, TOKEN_PROGRAM_ID, mint], ATA_PROGRAM_ID)
fn derive_ata(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
    let token_program = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
    let ata_program = Pubkey::from_str("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL").unwrap();
    let seeds = &[
        wallet.as_ref(),
        token_program.as_ref(),
        mint.as_ref(),
    ];
    let (ata, _) = Pubkey::find_program_address(seeds, &ata_program);
    ata
}

/// Build a Raydium CP-Swap instruction with the full 13-account layout.
///
/// Program: CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C
/// Discriminator: [0x8f, 0xbe, 0x5a, 0xda, 0xc4, 0x1e, 0x33, 0xde] (swap_base_in)
pub fn build_raydium_cp_swap_ix(
    signer: &Pubkey,
    pool: &crate::router::pool::PoolState,
    input_mint: Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Option<Instruction> {
    let extra = &pool.extra;
    let vault_a = extra.vault_a?;
    let vault_b = extra.vault_b?;
    let amm_config = extra.config?;
    let token_prog_a = extra.token_program_a?;
    let token_prog_b = extra.token_program_b?;

    let cp_program = Pubkey::from_str("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C").unwrap();
    let (authority, _) = Pubkey::find_program_address(&[], &cp_program);
    let (observation, _) = Pubkey::find_program_address(
        &[b"observation", pool.address.as_ref()], &cp_program,
    );

    let a_to_b = input_mint == pool.token_a_mint;
    let (input_vault, output_vault) = if a_to_b { (vault_a, vault_b) } else { (vault_b, vault_a) };
    let (input_token_prog, output_token_prog) = if a_to_b { (token_prog_a, token_prog_b) } else { (token_prog_b, token_prog_a) };
    let output_mint = if a_to_b { pool.token_b_mint } else { pool.token_a_mint };

    let user_input_ata = derive_ata(signer, &input_mint);
    let user_output_ata = derive_ata(signer, &output_mint);

    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&[0x8f, 0xbe, 0x5a, 0xda, 0xc4, 0x1e, 0x33, 0xde]);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());

    let accounts = vec![
        AccountMeta::new(*signer, true),
        AccountMeta::new_readonly(authority, false),
        AccountMeta::new_readonly(amm_config, false),
        AccountMeta::new(pool.address, false),
        AccountMeta::new(user_input_ata, false),
        AccountMeta::new(user_output_ata, false),
        AccountMeta::new(input_vault, false),
        AccountMeta::new(output_vault, false),
        AccountMeta::new_readonly(input_token_prog, false),
        AccountMeta::new_readonly(output_token_prog, false),
        AccountMeta::new_readonly(input_mint, false),
        AccountMeta::new_readonly(output_mint, false),
        AccountMeta::new(observation, false),
    ];

    Some(Instruction { program_id: cp_program, accounts, data })
}

/// Build a Meteora DAMM v2 swap instruction with the full 12-account layout.
///
/// Program: cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG
/// Discriminator: [0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88] (swap)
pub fn build_damm_v2_swap_ix(
    signer: &Pubkey,
    pool: &crate::router::pool::PoolState,
    input_mint: Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Option<Instruction> {
    let extra = &pool.extra;
    let vault_a = extra.vault_a?;
    let vault_b = extra.vault_b?;

    let damm_program = Pubkey::from_str("cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG").unwrap();
    let (pool_authority, _) = Pubkey::find_program_address(&[], &damm_program);
    let (event_authority, _) = Pubkey::find_program_address(&[b"__event_authority"], &damm_program);

    let a_to_b = input_mint == pool.token_a_mint;
    let (input_vault, output_vault) = if a_to_b { (vault_a, vault_b) } else { (vault_b, vault_a) };
    let output_mint = if a_to_b { pool.token_b_mint } else { pool.token_a_mint };

    let user_input_ata = derive_ata(signer, &input_mint);
    let user_output_ata = derive_ata(signer, &output_mint);

    let token_program = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();

    let mut data = Vec::with_capacity(25);
    data.extend_from_slice(&[0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88]);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());
    data.push(0u8); // swap_mode = 0 (ExactIn)

    let accounts = vec![
        AccountMeta::new(pool.address, false),
        AccountMeta::new_readonly(pool_authority, false),
        AccountMeta::new(input_vault, false),
        AccountMeta::new(output_vault, false),
        AccountMeta::new(user_input_ata, false),
        AccountMeta::new(user_output_ata, false),
        AccountMeta::new_readonly(input_mint, false),
        AccountMeta::new_readonly(output_mint, false),
        AccountMeta::new_readonly(token_program, false),
        AccountMeta::new_readonly(event_authority, false),
        AccountMeta::new_readonly(damm_program, false),
        AccountMeta::new(*signer, true),
    ];

    Some(Instruction { program_id: damm_program, accounts, data })
}

/// Build the account list for a Sanctum Infinity SwapExactIn instruction.
pub fn sanctum_swap_accounts(
    signer: &Pubkey,
    input_mint: &Pubkey,
    output_mint: &Pubkey,
) -> Vec<AccountMeta> {
    let s_controller = crate::config::programs::sanctum_s_controller();
    let pricing_program = crate::config::programs::sanctum_flat_fee_pricing();

    // PDAs
    let (pool_state_pda, _) = Pubkey::find_program_address(&[b"state"], &s_controller);
    let (lst_state_list_pda, _) = Pubkey::find_program_address(&[b"lst-state-list"], &s_controller);

    // Reserve ATAs (owned by Pool State PDA)
    let source_reserve_ata = derive_ata(&pool_state_pda, input_mint);
    let dest_reserve_ata = derive_ata(&pool_state_pda, output_mint);

    // User ATAs
    let user_source_ata = derive_ata(signer, input_mint);
    let user_dest_ata = derive_ata(signer, output_mint);

    // SOL Value Calculators
    let source_calc = crate::config::sanctum_sol_value_calculator(input_mint)
        .unwrap_or_else(|| {
            // For SOL (wSOL), use the wSOL calculator
            Pubkey::from_str("wsoGmxQLSvwWpuaidCApxN5kEowLe2HLQLJhCQnj4bE").unwrap()
        });
    let dest_calc = crate::config::sanctum_sol_value_calculator(output_mint)
        .unwrap_or_else(|| {
            Pubkey::from_str("wsoGmxQLSvwWpuaidCApxN5kEowLe2HLQLJhCQnj4bE").unwrap()
        });

    let token_program = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
    let system_program = solana_sdk::system_program::id();

    vec![
        AccountMeta::new_readonly(*signer, true),           // 1. Payer/signer
        AccountMeta::new(pool_state_pda, false),             // 2. Pool State PDA
        AccountMeta::new_readonly(lst_state_list_pda, false),// 3. LST State List PDA
        AccountMeta::new(source_reserve_ata, false),         // 4. Source reserve ATA
        AccountMeta::new(dest_reserve_ata, false),           // 5. Dest reserve ATA
        AccountMeta::new_readonly(pricing_program, false),   // 6. Pricing program
        AccountMeta::new_readonly(source_calc, false),       // 7. Source SOL Value Calc
        AccountMeta::new_readonly(dest_calc, false),         // 8. Dest SOL Value Calc
        AccountMeta::new(user_source_ata, false),            // 9. User source ATA
        AccountMeta::new(user_dest_ata, false),              // 10. User dest ATA
        AccountMeta::new_readonly(token_program, false),     // 11. Token Program
        AccountMeta::new_readonly(system_program, false),    // 12. System Program
    ]
}
