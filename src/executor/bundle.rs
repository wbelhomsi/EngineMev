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
use std::sync::LazyLock;
use tracing::debug;

use crate::router::pool::{ArbRoute, DexType};

// ─── Static Pubkeys (parsed once, reused everywhere) ───────────────────────

static SPL_TOKEN_PROGRAM: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap()
});
static TOKEN_2022_PROGRAM: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb").unwrap()
});
static ATA_PROGRAM: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL").unwrap()
});
static COMPUTE_BUDGET_PROGRAM: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("ComputeBudget111111111111111111111111111111").unwrap()
});
static MEMO_PROGRAM: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr").unwrap()
});
static WSOL_MINT: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap()
});
static WSOL_CALCULATOR: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("wsoGmxQLSvwWpuaidCApxN5kEowLe2HLQLJhCQnj4bE").unwrap()
});

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

/// Default Astralane tip in lamports (0.0001 SOL).
/// Override via ASTRALANE_TIP_LAMPORTS env var.
pub const DEFAULT_ASTRALANE_TIP_LAMPORTS: u64 = 100_000;

/// Returns the configured Astralane tip amount, or the default.
pub fn astralane_tip_lamports() -> u64 {
    std::env::var("ASTRALANE_TIP_LAMPORTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_ASTRALANE_TIP_LAMPORTS)
}

/// Returns the total extra relay tips (beyond Jito) that will be added to bundles.
/// Currently this is only Astralane when configured.
pub fn relay_extra_tips() -> u64 {
    if std::env::var("ASTRALANE_RELAY_URL").is_ok() {
        astralane_tip_lamports()
    } else {
        0
    }
}

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
    /// `tip_lamports` - total tip budget (Jito + all relay extras like Astralane)
    /// `recent_blockhash` - current blockhash for transaction validity
    pub fn build_arb_bundle(
        &self,
        route: &ArbRoute,
        tip_lamports: u64,
        recent_blockhash: Hash,
    ) -> Result<Vec<Vec<u8>>> {
        let mut bundle_txs: Vec<Vec<u8>> = Vec::with_capacity(2);

        // Determine Astralane tip from total budget
        let astralane_tip = if std::env::var("ASTRALANE_RELAY_URL").is_ok() {
            astralane_tip_lamports()
        } else {
            0
        };
        // Jito gets the remainder of the total tip budget
        let jito_tip = tip_lamports.saturating_sub(astralane_tip);

        // Safety: reject if total tips exceed estimated profit
        if tip_lamports >= route.estimated_profit_lamports {
            anyhow::bail!(
                "Total tips ({}) >= estimated profit ({}), would lose money",
                tip_lamports,
                route.estimated_profit_lamports
            );
        }

        // Calculate minimum output for profit enforcement on final hop
        // Must account for ALL tips (Jito + Astralane)
        let min_final_output = route.input_amount + route.estimated_profit_lamports.saturating_sub(tip_lamports);

        let arb_tx = self.build_arb_transaction_with_tip(route, jito_tip, astralane_tip, min_final_output, recent_blockhash)?;
        bundle_txs.push(bincode::serialize(&arb_tx)?);

        // Log the base64 transaction for simulation debugging
        {
            use base64::{engine::general_purpose, Engine as _};
            let tx_b64 = general_purpose::STANDARD.encode(&bundle_txs[0]);
            debug!(
                "Built bundle: {} txs, total_tip={} (jito={}, astralane={}), min_out={}, route={} hops, tx_b64={}",
                bundle_txs.len(),
                tip_lamports,
                jito_tip,
                astralane_tip,
                min_final_output,
                route.hop_count(),
                tx_b64,
            );
        }

        Ok(bundle_txs)
    }

    /// Build arb transaction with tip as last instruction.
    /// `min_final_output` is set on the final hop to guarantee profit on-chain.
    /// `jito_tip` and `astralane_tip` are the split tip amounts (already derived from total budget).
    fn build_arb_transaction_with_tip(
        &self,
        route: &ArbRoute,
        jito_tip: u64,
        astralane_tip: u64,
        min_final_output: u64,
        recent_blockhash: Hash,
    ) -> Result<Transaction> {
        let mut instructions = Vec::with_capacity(route.hop_count() * 3 + 4);

        // Compute budget: set unit limit and priority fee for Jito auction placement
        let compute_budget_program = *COMPUTE_BUDGET_PROGRAM;
        // SetComputeUnitLimit: instruction index 2, data = [2, limit_u32_le]
        let mut cu_limit_data = vec![2u8];
        cu_limit_data.extend_from_slice(&400_000u32.to_le_bytes());
        instructions.push(Instruction {
            program_id: compute_budget_program,
            accounts: vec![],
            data: cu_limit_data,
        });
        // SetComputeUnitPrice: instruction index 3, data = [3, price_u64_le] (micro-lamports)
        let mut cu_price_data = vec![3u8];
        cu_price_data.extend_from_slice(&1_000u64.to_le_bytes());
        instructions.push(Instruction {
            program_id: compute_budget_program,
            accounts: vec![],
            data: cu_price_data,
        });

        let signer_pubkey = self.searcher_keypair.pubkey();
        let ata_program = *ATA_PROGRAM;
        let token_program = *SPL_TOKEN_PROGRAM;

        // Collect unique mints and resolve their token program from cache.
        // The Geyser stream fetches mint owners (SPL Token vs Token-2022) asynchronously
        // and caches them in StateCache.mint_programs. This is the authoritative source.
        let wsol = *WSOL_MINT;
        let mut ata_mints: Vec<(Pubkey, Pubkey)> = Vec::new();
        for hop in &route.hops {
            for mint in [hop.input_mint, hop.output_mint] {
                if !ata_mints.iter().any(|(m, _)| *m == mint) {
                    let prog = if mint == wsol {
                        token_program // wSOL is always SPL Token
                    } else {
                        match self.state_cache.get_mint_program(&mint) {
                            Some(p) => p,
                            None => {
                                // Mint program not cached — default to SPL Token.
                                // The Geyser stream gates notifications on cached mints,
                                // so this should rarely be hit.
                                debug!("Mint program not cached for {}, defaulting SPL Token", mint);
                                token_program
                            }
                        }
                    };
                    ata_mints.push((mint, prog));
                }
            }
        }

        // Create ATAs idempotently (no-op if they already exist)
        for (mint, mint_token_program) in &ata_mints {
            debug!("ATA create: mint={}, program={}", mint, mint_token_program);
            let ata = derive_ata_with_program(&signer_pubkey, mint, mint_token_program);
            let create_ata_ix = Instruction {
                program_id: ata_program,
                accounts: vec![
                    AccountMeta::new(signer_pubkey, true),
                    AccountMeta::new(ata, false),
                    AccountMeta::new_readonly(signer_pubkey, false),
                    AccountMeta::new_readonly(*mint, false),
                    AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
                    AccountMeta::new_readonly(*mint_token_program, false),
                ],
                data: vec![1], // 1 = CreateIdempotent
            };
            instructions.push(create_ata_ix);
        }

        // Swap instructions — intermediate hops get min_out=0, final hop gets profit floor.
        // Track amount_in per hop: first hop uses route.input_amount,
        // subsequent hops use the previous hop's estimated_output.
        let last_idx = route.hops.len() - 1;
        for (i, hop) in route.hops.iter().enumerate() {
            let min_out = if i == last_idx { min_final_output } else { 0 };
            let amount_in = if i == 0 {
                route.input_amount
            } else {
                route.hops[i - 1].estimated_output
            };
            let ix = self.build_swap_instruction_with_min_out(hop, amount_in, min_out)?;
            instructions.push(ix);
        }

        // Tip instructions — each relay needs its own tip
        // Jito tip (rotated across 8 accounts)
        let tip_ix = self.build_tip_instruction(jito_tip)?;
        instructions.push(tip_ix);

        // Astralane tip — deducted from total tip budget, not added on top
        if astralane_tip > 0 {
            const ASTRALANE_TIP_ACCOUNTS: &[&str] = &[
                "astrazznxsGUhWShqgNtAdfrzP2G83DzcWVJDxwV9bF",
                "astra4uejePWneqNaJKuFFA8oonqCE1sqF6b45kDMZm",
                "astra9xWY93QyfG6yM8zwsKsRodscjQ2uU2HKNL5prk",
                "astraRVUuTHjpwEVvNBeQEgwYx9w9CFyfxjYoobCZhL",
                "astraEJ2fEj8Xmy6KLG7B3VfbKfsHXhHrNdCQx7iGJK",
                "astraubkDw81n4LuutzSQ8uzHCv4BhPVhfvTcYv8SKC",
                "astraZW5GLFefxNPAatceHhYjfA1ciq9gvfEg2S47xk",
                "astrawVNP4xDBKT7rAdxrLYiTSTdqtUr63fSMduivXK",
                "AstrA1ejL4UeXC2SBP4cpeEmtcFPZVLxx3XGKXyCW6to",
                "AsTra79FET4aCKWspPqeSFvjJNyp96SvAnrmyAxqg5b7",
                "AstrABAu8CBTyuPXpV4eSCJ5fePEPnxN8NqBaPKQ9fHR",
                "AsTRADtvb6tTmrsqULQ9Wji9PigDMjhfEMza6zkynEvV",
                "AsTRAEoyMofR3vUPpf9k68Gsfb6ymTZttEtsAbv8Bk4d",
                "AStrAJv2RN2hKCHxwUMtqmSxgdcNZbihCwc1mCSnG83W",
                "Astran35aiQUF57XZsmkWMtNCtXGLzs8upfiqXxth2bz",
                "AStRAnpi6kFrKypragExgeRoJ1QnKH7pbSjLAKQVWUum",
                "ASTRaoF93eYt73TYvwtsv6fMWHWbGmMUZfVZPo3CRU9C",
            ];
            let idx = self.tip_account_index.load(std::sync::atomic::Ordering::Relaxed)
                % ASTRALANE_TIP_ACCOUNTS.len();
            let astralane_tip_account: Pubkey = ASTRALANE_TIP_ACCOUNTS[idx].parse().unwrap();
            instructions.push(system_instruction::transfer(
                &self.searcher_keypair.pubkey(),
                &astralane_tip_account,
                astralane_tip,
            ));
        }

        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&self.searcher_keypair.pubkey()),
            &[&self.searcher_keypair],
            recent_blockhash,
        );

        Ok(tx)
    }

    /// Build a single swap instruction for one hop with correct amount_in and minimum_amount_out.
    fn build_swap_instruction_with_min_out(
        &self,
        hop: &crate::router::pool::RouteHop,
        amount_in: u64,
        minimum_amount_out: u64,
    ) -> Result<Instruction> {
        match hop.dex_type {
            DexType::RaydiumAmm => {
                let pool = self.state_cache.get_any(&hop.pool_address)
                    .ok_or_else(|| anyhow::anyhow!("Pool not found for Raydium AMM: {}", hop.pool_address))?;
                build_raydium_amm_swap_ix(
                    &self.searcher_keypair.pubkey(), &pool, hop.input_mint,
                    amount_in, minimum_amount_out,
                ).ok_or_else(|| anyhow::anyhow!("Missing pool data for Raydium AMM v4"))
            }
            DexType::RaydiumClmm => {
                let pool = self.state_cache.get_any(&hop.pool_address)
                    .ok_or_else(|| anyhow::anyhow!("Pool not found for CLMM: {}", hop.pool_address))?;
                build_raydium_clmm_swap_ix(
                    &self.searcher_keypair.pubkey(), &pool, hop.input_mint,
                    amount_in, minimum_amount_out,
                ).ok_or_else(|| anyhow::anyhow!("Missing pool data for Raydium CLMM"))
            }
            DexType::RaydiumCp => {
                let pool = self.state_cache.get_any(&hop.pool_address)
                    .ok_or_else(|| anyhow::anyhow!("Pool not found for Raydium CP: {}", hop.pool_address))?;
                build_raydium_cp_swap_ix(
                    &self.searcher_keypair.pubkey(), &pool, hop.input_mint,
                    amount_in, minimum_amount_out,
                ).ok_or_else(|| anyhow::anyhow!("Missing pool data for Raydium CP"))
            }
            DexType::OrcaWhirlpool => {
                let pool = self.state_cache.get_any(&hop.pool_address)
                    .ok_or_else(|| anyhow::anyhow!("Pool not found for Orca: {}", hop.pool_address))?;
                build_orca_whirlpool_swap_ix(
                    &self.searcher_keypair.pubkey(), &pool, hop.input_mint,
                    amount_in, minimum_amount_out,
                ).ok_or_else(|| anyhow::anyhow!("Missing pool data for Orca Whirlpool"))
            }
            DexType::MeteoraDlmm => {
                let pool = self.state_cache.get_any(&hop.pool_address)
                    .ok_or_else(|| anyhow::anyhow!("Pool not found for DLMM: {}", hop.pool_address))?;
                build_meteora_dlmm_swap_ix(
                    &self.searcher_keypair.pubkey(), &pool, hop.input_mint,
                    amount_in, minimum_amount_out,
                ).ok_or_else(|| anyhow::anyhow!("Missing pool data for Meteora DLMM"))
            }
            DexType::MeteoraDammV2 => {
                let pool = self.state_cache.get_any(&hop.pool_address)
                    .ok_or_else(|| anyhow::anyhow!("Pool not found for DAMM v2: {}", hop.pool_address))?;
                build_damm_v2_swap_ix(
                    &self.searcher_keypair.pubkey(), &pool, hop.input_mint,
                    amount_in, minimum_amount_out,
                ).ok_or_else(|| anyhow::anyhow!("Missing pool data for DAMM v2"))
            }
            DexType::SanctumInfinity => {
                build_sanctum_swap_ix(
                    &self.searcher_keypair.pubkey(),
                    &hop.input_mint,
                    &hop.output_mint,
                    amount_in,
                    minimum_amount_out,
                ).ok_or_else(|| anyhow::anyhow!("Failed to build Sanctum swap IX"))
            }
            DexType::Phoenix => {
                let pool = self.state_cache.get_any(&hop.pool_address)
                    .ok_or_else(|| anyhow::anyhow!("Pool not found for Phoenix: {}", hop.pool_address))?;
                build_phoenix_swap_ix(
                    &self.searcher_keypair.pubkey(), &pool, hop.input_mint,
                    amount_in, minimum_amount_out,
                ).ok_or_else(|| anyhow::anyhow!("Missing pool data for Phoenix"))
            }
            DexType::Manifest => {
                let pool = self.state_cache.get_any(&hop.pool_address)
                    .ok_or_else(|| anyhow::anyhow!("Pool not found for Manifest: {}", hop.pool_address))?;
                build_manifest_swap_ix(
                    &self.searcher_keypair.pubkey(), &pool, hop.input_mint,
                    amount_in, minimum_amount_out,
                ).ok_or_else(|| anyhow::anyhow!("Missing pool data for Manifest"))
            }
        }
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

/// Derive an Associated Token Account address (SPL Token only).
/// ATA = PDA([wallet, TOKEN_PROGRAM_ID, mint], ATA_PROGRAM_ID)
fn derive_ata(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
    derive_ata_with_program(wallet, mint, &SPL_TOKEN_PROGRAM)
}

fn derive_ata_with_program(wallet: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    let seeds = &[
        wallet.as_ref(),
        token_program.as_ref(),
        mint.as_ref(),
    ];
    let (ata, _) = Pubkey::find_program_address(seeds, &ATA_PROGRAM);
    ata
}

/// Build a Raydium AMM v4 swap instruction with 9 accounts.
pub fn build_raydium_amm_swap_ix(
    signer: &Pubkey,
    pool: &crate::router::pool::PoolState,
    input_mint: Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Option<Instruction> {
    let extra = &pool.extra;
    let vault_a = extra.vault_a?;
    let vault_b = extra.vault_b?;
    let open_orders = extra.open_orders?;
    let nonce = extra.amm_nonce?;

    let amm_program = crate::config::programs::raydium_amm();
    let amm_authority = Pubkey::create_program_address(
        &[&[nonce]],
        &amm_program,
    ).ok()?;

    let a_to_b = input_mint == pool.token_a_mint;
    let output_mint = if a_to_b { pool.token_b_mint } else { pool.token_a_mint };
    let user_source_ata = derive_ata(signer, &input_mint);
    let user_dest_ata = derive_ata(signer, &output_mint);

    let mut data = Vec::with_capacity(17);
    data.push(9u8);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());

    let accounts = vec![
        AccountMeta::new_readonly(*SPL_TOKEN_PROGRAM, false),
        AccountMeta::new(pool.address, false),
        AccountMeta::new_readonly(amm_authority, false),
        AccountMeta::new(open_orders, false),
        AccountMeta::new(vault_a, false),
        AccountMeta::new(vault_b, false),
        AccountMeta::new(user_source_ata, false),
        AccountMeta::new(user_dest_ata, false),
        AccountMeta::new_readonly(*signer, true),
    ];

    Some(Instruction { program_id: amm_program, accounts, data })
}

/// Anchor discriminator for Sanctum S Controller `swap_exact_in`
const SANCTUM_SWAP_EXACT_IN_DISCRIMINATOR: [u8; 8] = [0x68, 0x68, 0x83, 0x56, 0xa1, 0xbd, 0xb4, 0xd8];

/// Build a Sanctum Infinity SwapExactIn instruction.
pub fn build_sanctum_swap_ix(
    signer: &Pubkey,
    input_mint: &Pubkey,
    output_mint: &Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Option<Instruction> {
    let accounts = sanctum_swap_accounts(signer, input_mint, output_mint);
    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&SANCTUM_SWAP_EXACT_IN_DISCRIMINATOR);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());
    Some(Instruction {
        program_id: crate::config::programs::sanctum_s_controller(),
        accounts,
        data,
    })
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

    let cp_program = crate::config::programs::raydium_cp();
    let (authority, _) = Pubkey::find_program_address(&[], &cp_program);
    let (observation, _) = Pubkey::find_program_address(
        &[b"observation", pool.address.as_ref()], &cp_program,
    );

    let a_to_b = input_mint == pool.token_a_mint;
    let (input_vault, output_vault) = if a_to_b { (vault_a, vault_b) } else { (vault_b, vault_a) };
    let (input_token_prog, output_token_prog) = if a_to_b { (token_prog_a, token_prog_b) } else { (token_prog_b, token_prog_a) };
    let output_mint = if a_to_b { pool.token_b_mint } else { pool.token_a_mint };

    // Use derive_ata_with_program — Raydium CP supports Token-2022 per side
    let user_input_ata = derive_ata_with_program(signer, &input_mint, &input_token_prog);
    let user_output_ata = derive_ata_with_program(signer, &output_mint, &output_token_prog);

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

    let damm_program = crate::config::programs::meteora_damm_v2();
    let (pool_authority, _) = Pubkey::find_program_address(&[], &damm_program);
    let (event_authority, _) = Pubkey::find_program_address(&[b"__event_authority"], &damm_program);

    let a_to_b = input_mint == pool.token_a_mint;
    let (input_vault, output_vault) = if a_to_b { (vault_a, vault_b) } else { (vault_b, vault_a) };
    let output_mint = if a_to_b { pool.token_b_mint } else { pool.token_a_mint };

    let user_input_ata = derive_ata(signer, &input_mint);
    let user_output_ata = derive_ata(signer, &output_mint);

    let token_program = *SPL_TOKEN_PROGRAM;

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

/// Floor division that rounds toward negative infinity (needed for tick array computation).
fn floor_div(dividend: i32, divisor: i32) -> i32 {
    if dividend % divisor == 0 || dividend.signum() == divisor.signum() {
        dividend / divisor
    } else {
        dividend / divisor - 1
    }
}

/// Build an Orca Whirlpool swap_v2 instruction with full account layout.
///
/// Program: whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc
/// Discriminator: [0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62] (swap_v2)
/// Accounts: 15 (token_program_a, token_program_b, memo_program, token_authority, whirlpool,
///           token_mint_a, token_mint_b, ata_a, vault_a, ata_b, vault_b,
///           tick_array_0, tick_array_1, tick_array_2, oracle)
pub fn build_orca_whirlpool_swap_ix(
    signer: &Pubkey,
    pool: &crate::router::pool::PoolState,
    input_mint: Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Option<Instruction> {
    let extra = &pool.extra;
    let vault_a = extra.vault_a?;
    let vault_b = extra.vault_b?;
    let tick_spacing = extra.tick_spacing?;

    let whirlpool_program = crate::config::programs::orca_whirlpool();
    let token_program = *SPL_TOKEN_PROGRAM;
    let memo_program = *MEMO_PROGRAM;

    let a_to_b = input_mint == pool.token_a_mint;
    let tick_current = pool.current_tick.unwrap_or(0);

    // Oracle PDA
    let (oracle, _) = Pubkey::find_program_address(
        &[b"oracle", pool.address.as_ref()], &whirlpool_program,
    );

    // Tick array PDAs (3 arrays, string-encoded start index)
    let ticks_in_array: i32 = 88 * tick_spacing as i32;
    let start_base = floor_div(tick_current, ticks_in_array) * ticks_in_array;

    let offsets: [i32; 3] = if a_to_b {
        [0, -1, -2]
    } else if tick_current + tick_spacing as i32 >= start_base + ticks_in_array {
        [1, 2, 3]
    } else {
        [0, 1, 2]
    };

    let tick_arrays: Vec<Pubkey> = offsets.iter().map(|&o| {
        let start = start_base + o * ticks_in_array;
        Pubkey::find_program_address(
            &[b"tick_array", pool.address.as_ref(), start.to_string().as_bytes()],
            &whirlpool_program,
        ).0
    }).collect();

    // sqrt_price_limit
    let sqrt_price_limit: u128 = if a_to_b { 4295048016u128 } else { 79226673515401279992447579055u128 };

    // User token accounts
    let user_ata_a = derive_ata(signer, &pool.token_a_mint);
    let user_ata_b = derive_ata(signer, &pool.token_b_mint);

    // Discriminator: swap_v2 [0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62]
    let mut data = Vec::with_capacity(43);
    data.extend_from_slice(&[0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62]);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());
    data.extend_from_slice(&sqrt_price_limit.to_le_bytes());
    data.push(1u8); // is_exact_in = true
    data.push(if a_to_b { 1u8 } else { 0u8 }); // a_to_b
    data.push(0u8); // remaining_accounts_info = None

    // SwapV2 account layout (15 accounts):
    let accounts = vec![
        AccountMeta::new_readonly(token_program, false),   // 0: token_program_a (SPL Token)
        AccountMeta::new_readonly(token_program, false),   // 1: token_program_b (SPL Token — Whirlpool doesn't support Token-2022)
        AccountMeta::new_readonly(memo_program, false),    // 2: memo_program
        AccountMeta::new(*signer, true),                   // 3: token_authority (signer)
        AccountMeta::new(pool.address, false),             // 4: whirlpool
        AccountMeta::new_readonly(pool.token_a_mint, false), // 5: token_mint_a
        AccountMeta::new_readonly(pool.token_b_mint, false), // 6: token_mint_b
        AccountMeta::new(user_ata_a, false),               // 7: token_owner_account_a
        AccountMeta::new(vault_a, false),                  // 8: token_vault_a
        AccountMeta::new(user_ata_b, false),               // 9: token_owner_account_b
        AccountMeta::new(vault_b, false),                  // 10: token_vault_b
        AccountMeta::new(tick_arrays[0], false),           // 11: tick_array_0
        AccountMeta::new(tick_arrays[1], false),           // 12: tick_array_1
        AccountMeta::new(tick_arrays[2], false),           // 13: tick_array_2
        AccountMeta::new(oracle, false),                   // 14: oracle
    ];

    Some(Instruction { program_id: whirlpool_program, accounts, data })
}

/// Build a Raydium CLMM swap_v2 instruction with full account layout.
///
/// Program: CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK
/// Discriminator: [0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62] (swap_v2)
/// Accounts: 17 (payer, amm_config, pool_state, input_ata, output_ata, input_vault, output_vault,
///           observation_state, token_program, token_2022, memo, input_mint, output_mint,
///           bitmap_extension, tick_array_0, tick_array_1, tick_array_2)
pub fn build_raydium_clmm_swap_ix(
    signer: &Pubkey,
    pool: &crate::router::pool::PoolState,
    input_mint: Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Option<Instruction> {
    let extra = &pool.extra;
    let vault_a = extra.vault_a?;
    let vault_b = extra.vault_b?;
    let tick_spacing = extra.tick_spacing?;
    let amm_config = extra.config?;
    let observation_state = extra.observation?;

    let clmm_program = crate::config::programs::raydium_clmm();
    let token_program = *SPL_TOKEN_PROGRAM;
    let token_2022_program = *TOKEN_2022_PROGRAM;
    let memo_program = *MEMO_PROGRAM;

    let a_to_b = input_mint == pool.token_a_mint;
    let tick_current = pool.current_tick.unwrap_or(0);

    let (input_vault, output_vault) = if a_to_b { (vault_a, vault_b) } else { (vault_b, vault_a) };
    let output_mint = if a_to_b { pool.token_b_mint } else { pool.token_a_mint };

    let user_input_ata = derive_ata(signer, &input_mint);
    let user_output_ata = derive_ata(signer, &output_mint);

    // Bitmap extension PDA
    let (bitmap_extension, _) = Pubkey::find_program_address(
        &[b"pool_tick_array_bitmap_extension", pool.address.as_ref()],
        &clmm_program,
    );

    // Tick array PDAs (3 arrays, i32 big-endian encoded)
    let ticks_in_array: i32 = 60 * tick_spacing as i32;
    let start_base = floor_div(tick_current, ticks_in_array) * ticks_in_array;

    let tick_offsets: [i32; 3] = if a_to_b {
        [0, -1, -2]
    } else {
        [0, 1, 2]
    };

    let tick_arrays: Vec<Pubkey> = tick_offsets.iter().map(|&o| {
        let start = start_base + o * ticks_in_array;
        Pubkey::find_program_address(
            &[b"tick_array", pool.address.as_ref(), &start.to_be_bytes()],
            &clmm_program,
        ).0
    }).collect();

    // sqrt_price_limit
    let sqrt_price_limit: u128 = if a_to_b { 4295048016u128 } else { 79226673521066979257578248091u128 };

    // Discriminator: swap_v2
    let mut data = Vec::with_capacity(41);
    data.extend_from_slice(&[0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62]);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());
    data.extend_from_slice(&sqrt_price_limit.to_le_bytes());
    data.push(1u8); // is_exact_in = true

    let accounts = vec![
        AccountMeta::new(*signer, true),                       // 0: payer
        AccountMeta::new_readonly(amm_config, false),          // 1: amm_config
        AccountMeta::new(pool.address, false),                 // 2: pool_state
        AccountMeta::new(user_input_ata, false),               // 3: input_token_account
        AccountMeta::new(user_output_ata, false),              // 4: output_token_account
        AccountMeta::new(input_vault, false),                  // 5: input_vault
        AccountMeta::new(output_vault, false),                 // 6: output_vault
        AccountMeta::new(observation_state, false),            // 7: observation_state
        AccountMeta::new_readonly(token_program, false),       // 8: token_program
        AccountMeta::new_readonly(token_2022_program, false),  // 9: token_program_2022
        AccountMeta::new_readonly(memo_program, false),        // 10: memo_program
        AccountMeta::new_readonly(input_mint, false),          // 11: input_vault_mint
        AccountMeta::new_readonly(output_mint, false),         // 12: output_vault_mint
        // Remaining accounts:
        AccountMeta::new(bitmap_extension, false),             // 13: bitmap extension
        AccountMeta::new(tick_arrays[0], false),               // 14: tick_array_0
        AccountMeta::new(tick_arrays[1], false),               // 15: tick_array_1
        AccountMeta::new(tick_arrays[2], false),               // 16: tick_array_2
    ];

    Some(Instruction { program_id: clmm_program, accounts, data })
}

/// Build a Meteora DLMM swap2 instruction with full account layout.
///
/// Program: LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo
/// Discriminator: [0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88] (swap2)
/// Accounts: 15 fixed + N bin arrays as remaining accounts
pub fn build_meteora_dlmm_swap_ix(
    signer: &Pubkey,
    pool: &crate::router::pool::PoolState,
    input_mint: Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Option<Instruction> {
    let extra = &pool.extra;
    let vault_a = extra.vault_a?;  // reserve_x
    let vault_b = extra.vault_b?;  // reserve_y

    let dlmm_program = crate::config::programs::meteora_dlmm();
    let token_program = *SPL_TOKEN_PROGRAM;
    let memo_program = *MEMO_PROGRAM;

    let a_to_b = input_mint == pool.token_a_mint; // X -> Y
    let active_id = pool.current_tick.unwrap_or(0);

    // Determine token programs for correct ATA derivation
    let prog_a = extra.token_program_a.unwrap_or(token_program);
    let prog_b = extra.token_program_b.unwrap_or(token_program);
    let (input_prog, output_prog) = if a_to_b { (prog_a, prog_b) } else { (prog_b, prog_a) };
    let output_mint = if a_to_b { pool.token_b_mint } else { pool.token_a_mint };

    let user_input_ata = derive_ata_with_program(signer, &input_mint, &input_prog);
    let user_output_ata = derive_ata_with_program(signer, &output_mint, &output_prog);

    // Oracle PDA
    let (oracle, _) = Pubkey::find_program_address(
        &[b"oracle", pool.address.as_ref()], &dlmm_program,
    );

    // Event authority PDA
    let (event_authority, _) = Pubkey::find_program_address(
        &[b"__event_authority"], &dlmm_program,
    );

    // Bitmap extension: only needed when active_id is near the edge of internal bitmap range
    // (±512 bin array indices). Most pools don't need it. If the PDA doesn't exist on-chain,
    // just don't include it — the swap can't traverse beyond the internal bitmap but that's fine
    // for single-bin arbs. To properly support it, we'd need to check on-chain existence at
    // pool discovery time and store in PoolExtra.

    // Bin array PDAs: compute the current bin array index and get a few in the swap direction
    let bin_array_index = if active_id >= 0 {
        active_id / 70
    } else if active_id % 70 == 0 {
        active_id / 70
    } else {
        active_id / 70 - 1
    };

    // Get 3 bin arrays in swap direction
    let bin_offsets: [i32; 3] = if a_to_b {
        [0, -1, -2] // X->Y, price goes down, bins decrease
    } else {
        [0, 1, 2]   // Y->X, price goes up, bins increase
    };

    let bin_arrays: Vec<Pubkey> = bin_offsets.iter().map(|&o| {
        let idx = (bin_array_index + o) as i64;
        Pubkey::find_program_address(
            &[b"bin_array", pool.address.as_ref(), &idx.to_le_bytes()],
            &dlmm_program,
        ).0
    }).collect();

    // Discriminator: swap2
    let mut data = Vec::with_capacity(28);
    data.extend_from_slice(&[0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88]);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());
    // remaining_accounts_info: empty Vec (Borsh: 4 bytes of 0)
    data.extend_from_slice(&0u32.to_le_bytes());

    // Bitmap extension — Option<UncheckedAccount> in the DLMM program.
    // If the pool doesn't have one, pass the DLMM program ID itself.
    // Anchor interprets the executing program ID as None for Option accounts.
    let bitmap_extension = extra.bitmap_extension.unwrap_or(dlmm_program);

    let mut accounts = vec![
        AccountMeta::new(pool.address, false),              // 0: lb_pair
        AccountMeta::new(bitmap_extension, false),          // 1: bin_array_bitmap_extension
        AccountMeta::new(vault_a, false),                   // 2: reserve_x
        AccountMeta::new(vault_b, false),                   // 3: reserve_y
        AccountMeta::new(user_input_ata, false),            // 4: user_token_in
        AccountMeta::new(user_output_ata, false),           // 5: user_token_out
        AccountMeta::new_readonly(pool.token_a_mint, false),// 6: token_x_mint
        AccountMeta::new_readonly(pool.token_b_mint, false),// 7: token_y_mint
        AccountMeta::new(oracle, false),                    // 8: oracle
        AccountMeta::new(dlmm_program, false),               // 9: host_fee_in (None — pass program ID for Option, must be writable per IDL)
        AccountMeta::new(*signer, true),                    // 10: user (signer)
        AccountMeta::new_readonly(prog_a, false),             // 11: token_x_program
        AccountMeta::new_readonly(prog_b, false),            // 12: token_y_program
        AccountMeta::new_readonly(memo_program, false),       // 13: memo_program
        AccountMeta::new_readonly(event_authority, false),   // 14: event_authority
        AccountMeta::new_readonly(dlmm_program, false),      // 15: program
    ];

    // Append bin arrays as remaining accounts
    for ba in &bin_arrays {
        accounts.push(AccountMeta::new(*ba, false));
    }

    Some(Instruction { program_id: dlmm_program, accounts, data })
}

/// Build a Phoenix swap instruction (ImmediateOrCancel order).
///
/// Program: PhoeNiXZ8ByJGLkxNfZRnkUfjvmuYqLR89jjFHGqdXY
/// Discriminant: 0x00 (Swap)
/// Accounts: 9 (phoenix_program, log_authority, market, trader, base_ata, quote_ata,
///           base_vault, quote_vault, token_program)
pub fn build_phoenix_swap_ix(
    signer: &Pubkey,
    pool: &crate::router::pool::PoolState,
    input_mint: Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Option<Instruction> {
    let vault_a = pool.extra.vault_a?; // base vault
    let vault_b = pool.extra.vault_b?; // quote vault

    let phoenix_program = crate::config::programs::phoenix_v1();
    let token_program = *SPL_TOKEN_PROGRAM;

    let (log_authority, _) = Pubkey::find_program_address(&[b"log"], &phoenix_program);

    // token_a_mint = base, token_b_mint = quote
    let a_to_b = input_mint == pool.token_a_mint; // selling base = Ask side
    let side: u8 = if a_to_b { 0x01 } else { 0x00 }; // Ask=0x01, Bid=0x00

    let base_ata = derive_ata(signer, &pool.token_a_mint);
    let quote_ata = derive_ata(signer, &pool.token_b_mint);

    // Instruction data: discriminant + ImmediateOrCancel OrderPacket
    let mut data = Vec::with_capacity(44);
    data.push(0x00u8);             // instruction discriminant: Swap
    data.push(0x01u8);             // OrderPacket discriminant: ImmediateOrCancel
    data.push(side);               // side: 0x00=Bid, 0x01=Ask
    data.extend_from_slice(&0u64.to_le_bytes());              // price_in_ticks (0 = market)
    data.extend_from_slice(&amount_in.to_le_bytes());         // num_base_lots
    data.extend_from_slice(&0u64.to_le_bytes());              // num_quote_lots
    data.extend_from_slice(&minimum_amount_out.to_le_bytes()); // min_base_lots_to_fill
    data.extend_from_slice(&0u64.to_le_bytes());              // min_quote_lots_to_fill
    data.push(0x00u8);             // self_trade_behavior: Abort
    data.push(0x00u8);             // match_limit: None
    data.extend_from_slice(&0u128.to_le_bytes());             // client_order_id
    data.push(0x00u8);             // use_only_deposited_funds: false

    let accounts = vec![
        AccountMeta::new_readonly(phoenix_program, false), // 0: Phoenix program
        AccountMeta::new_readonly(log_authority, false),   // 1: Log authority PDA
        AccountMeta::new(pool.address, false),             // 2: Market (writable)
        AccountMeta::new(*signer, true),                   // 3: Trader/signer
        AccountMeta::new(base_ata, false),                 // 4: Trader base token account
        AccountMeta::new(quote_ata, false),                // 5: Trader quote token account
        AccountMeta::new(vault_a, false),                  // 6: Base vault
        AccountMeta::new(vault_b, false),                  // 7: Quote vault
        AccountMeta::new_readonly(token_program, false),   // 8: Token Program
    ];

    Some(Instruction { program_id: phoenix_program, accounts, data })
}

/// Build a Manifest swap instruction.
///
/// Program: MNFSTqtC93rEfYHB6hF82sKdZpUDFWkViLByLd1k1Ms
/// Discriminant: 4 (Swap)
/// Accounts: 8 (payer, market, system_program, base_ata, quote_ata, base_vault, quote_vault,
///           token_program)
pub fn build_manifest_swap_ix(
    signer: &Pubkey,
    pool: &crate::router::pool::PoolState,
    input_mint: Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Option<Instruction> {
    let vault_a = pool.extra.vault_a?; // base vault
    let vault_b = pool.extra.vault_b?; // quote vault

    let manifest_program = crate::config::programs::manifest();
    let token_program = *SPL_TOKEN_PROGRAM;
    let system_program = solana_sdk::system_program::id();

    // token_a_mint = base, token_b_mint = quote
    let is_base_in: u8 = if input_mint == pool.token_a_mint { 1 } else { 0 };

    let base_ata = derive_ata(signer, &pool.token_a_mint);
    let quote_ata = derive_ata(signer, &pool.token_b_mint);

    let mut data = Vec::with_capacity(19);
    data.push(4u8);                                           // Swap discriminant
    data.extend_from_slice(&amount_in.to_le_bytes());         // in_atoms
    data.extend_from_slice(&minimum_amount_out.to_le_bytes()); // out_atoms
    data.push(is_base_in);                                    // is_base_in
    data.push(1u8);                                           // is_exact_in = true

    let accounts = vec![
        AccountMeta::new(*signer, true),                   // 0: Payer/signer
        AccountMeta::new(pool.address, false),             // 1: Market
        AccountMeta::new_readonly(system_program, false),  // 2: System program
        AccountMeta::new(base_ata, false),                 // 3: Trader base token account
        AccountMeta::new(quote_ata, false),                // 4: Trader quote token account
        AccountMeta::new(vault_a, false),                  // 5: Base vault
        AccountMeta::new(vault_b, false),                  // 6: Quote vault
        AccountMeta::new_readonly(token_program, false),   // 7: Token program
    ];

    Some(Instruction { program_id: manifest_program, accounts, data })
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
        .unwrap_or(*WSOL_CALCULATOR);
    let dest_calc = crate::config::sanctum_sol_value_calculator(output_mint)
        .unwrap_or(*WSOL_CALCULATOR);

    let token_program = *SPL_TOKEN_PROGRAM;
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
