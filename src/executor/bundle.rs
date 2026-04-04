use anyhow::Result;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    system_instruction,
};
use tracing::debug;

use crate::addresses;
use crate::router::pool::{ArbRoute, DexType};

/// Builds base arb instructions for relay submission.
///
/// Post-mempool architecture (2024+): observe state change via Geyser,
/// build arb instructions, hand off to per-relay modules which add their
/// own tips, sign, and submit independently.
pub struct BundleBuilder {
    searcher_keypair: Keypair,
    state_cache: crate::state::StateCache,
    /// Optional arb-guard program ID for on-chain profit verification.
    /// When set, start_check is prepended and profit_check is appended to arb TXs.
    arb_guard_program_id: Option<Pubkey>,
}

/// Client-side mirror of on-chain HopParams for serialization.
struct HopParamsClient {
    dex_type: u8,
    a_to_b: bool,
}

impl BundleBuilder {
    pub fn new(
        searcher_keypair: Keypair,
        state_cache: crate::state::StateCache,
        arb_guard_program_id: Option<Pubkey>,
    ) -> Self {
        Self { searcher_keypair, state_cache, arb_guard_program_id }
    }

    pub fn signer_pubkey(&self) -> Pubkey {
        self.searcher_keypair.pubkey()
    }

    /// Build base arb instructions: compute budget + ATA creates + swaps.
    /// Does NOT include tips or signing — each relay adds its own tip and signs.
    pub fn build_arb_instructions(
        &self,
        route: &ArbRoute,
        min_final_output: u64,
    ) -> Result<Vec<Instruction>> {
        let mut instructions = Vec::with_capacity(route.hop_count() * 3 + 6);
        let wsol = addresses::WSOL;

        // If arb-guard CPI executor is available and ALL hops are Orca, use execute_arb
        if self.arb_guard_program_id.is_some()
            && route.hops.iter().all(|h| h.dex_type == DexType::OrcaWhirlpool)
        {
            let mut instructions = Vec::with_capacity(8);
            let compute_budget_program = addresses::COMPUTE_BUDGET;
            let signer_pubkey = self.searcher_keypair.pubkey();

            // Compute budget
            let mut cu_limit_data = vec![2u8];
            cu_limit_data.extend_from_slice(&400_000u32.to_le_bytes());
            instructions.push(Instruction { program_id: compute_budget_program, accounts: vec![], data: cu_limit_data });
            let mut cu_price_data = vec![3u8];
            cu_price_data.extend_from_slice(&1_000u64.to_le_bytes());
            instructions.push(Instruction { program_id: compute_budget_program, accounts: vec![], data: cu_price_data });

            // ATA creates (reuse existing logic pattern)
            let ata_program = addresses::ATA_PROGRAM;
            let token_program = addresses::SPL_TOKEN;
            let mut ata_mints: Vec<(Pubkey, Pubkey)> = Vec::new();
            for hop in &route.hops {
                for mint in [hop.input_mint, hop.output_mint] {
                    if !ata_mints.iter().any(|(m, _)| *m == mint) {
                        let prog = if mint == wsol { token_program } else {
                            self.state_cache.get_mint_program(&mint).unwrap_or(token_program)
                        };
                        ata_mints.push((mint, prog));
                    }
                }
            }
            for (mint, mint_token_program) in &ata_mints {
                let ata = derive_ata_with_program(&signer_pubkey, mint, mint_token_program);
                instructions.push(Instruction {
                    program_id: ata_program,
                    accounts: vec![
                        AccountMeta::new(signer_pubkey, true),
                        AccountMeta::new(ata, false),
                        AccountMeta::new_readonly(signer_pubkey, false),
                        AccountMeta::new_readonly(*mint, false),
                        AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
                        AccountMeta::new_readonly(*mint_token_program, false),
                    ],
                    data: vec![1],
                });
            }

            // wSOL wrap (if first hop input is wSOL)
            if !route.hops.is_empty() && route.hops[0].input_mint == wsol {
                let wsol_ata = derive_ata(&signer_pubkey, &wsol);
                instructions.push(system_instruction::transfer(&signer_pubkey, &wsol_ata, route.input_amount));
                instructions.push(Instruction {
                    program_id: addresses::SPL_TOKEN,
                    accounts: vec![AccountMeta::new(wsol_ata, false)],
                    data: vec![17],
                });
            }

            // The single execute_arb IX
            instructions.push(self.build_execute_arb_ix(route, min_final_output)?);

            // wSOL unwrap (if last hop output is wSOL)
            if !route.hops.is_empty() && route.hops.last().unwrap().output_mint == wsol {
                let wsol_ata = derive_ata(&signer_pubkey, &wsol);
                instructions.push(Instruction {
                    program_id: addresses::SPL_TOKEN,
                    accounts: vec![
                        AccountMeta::new(wsol_ata, false),
                        AccountMeta::new(signer_pubkey, false),
                        AccountMeta::new_readonly(signer_pubkey, true),
                    ],
                    data: vec![9],
                });
            }

            debug!("Built {} arb instructions (CPI executor) for {} hops", instructions.len(), route.hop_count());
            return Ok(instructions);
        }

        // Optional: arb-guard start_check (records pre-swap wSOL ATA balance)
        if let Some(ref guard_program) = self.arb_guard_program_id {
            let wsol_ata = derive_ata(&self.searcher_keypair.pubkey(), &wsol);
            instructions.push(build_guard_start_check_ix(guard_program, &self.searcher_keypair.pubkey(), &wsol_ata));
        }

        // Compute budget: set unit limit and priority fee for Jito auction placement
        let compute_budget_program = addresses::COMPUTE_BUDGET;
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
        let ata_program = addresses::ATA_PROGRAM;
        let token_program = addresses::SPL_TOKEN;

        // Collect unique mints and resolve their token program from RPC cache.
        // get_mint_program() is the authoritative source (fetched via getAccountInfo).
        // Pool state flags (extra.token_program_a/b) can be stale or wrong for Token-2022.
        // IMPORTANT: swap IX builders must also use this same resolution.
        let mut ata_mints: Vec<(Pubkey, Pubkey)> = Vec::new();
        for hop in &route.hops {
            for mint in [hop.input_mint, hop.output_mint] {
                if !ata_mints.iter().any(|(m, _)| *m == mint) {
                    let prog = if mint == wsol {
                        token_program
                    } else {
                        self.state_cache.get_mint_program(&mint).unwrap_or(token_program)
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

        // If the first hop's input is wSOL, wrap native SOL into the wSOL ATA.
        // We hold native SOL but DEX swaps need wSOL (SPL Token).
        if !route.hops.is_empty() && route.hops[0].input_mint == wsol {
            let wsol_ata = derive_ata(&signer_pubkey, &wsol);
            // Transfer native SOL to wSOL ATA
            instructions.push(system_instruction::transfer(
                &signer_pubkey,
                &wsol_ata,
                route.input_amount,
            ));
            // SyncNative: tell the SPL Token program to update the wSOL balance
            instructions.push(Instruction {
                program_id: addresses::SPL_TOKEN,
                accounts: vec![AccountMeta::new(wsol_ata, false)],
                data: vec![17], // 17 = SyncNative instruction
            });
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

        // Optional: arb-guard profit_check BEFORE CloseAccount so the wSOL ATA
        // balance is still readable. Reverts the entire TX if no profit detected.
        if let Some(ref guard_program) = self.arb_guard_program_id {
            let wsol_ata = derive_ata(&signer_pubkey, &wsol);
            let min_profit: u64 = std::env::var("MIN_ON_CHAIN_PROFIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            instructions.push(build_guard_profit_check_ix(
                guard_program,
                &signer_pubkey,
                &wsol_ata,
                min_profit,
            ));
        }

        // If the last hop outputs wSOL, close the ATA to unwrap back to native SOL.
        // This recovers the arb profit + rent as native SOL in our wallet.
        if !route.hops.is_empty() && route.hops[route.hops.len() - 1].output_mint == wsol {
            let wsol_ata = derive_ata(&signer_pubkey, &wsol);
            // CloseAccount: transfers remaining wSOL balance to signer as native SOL
            instructions.push(Instruction {
                program_id: addresses::SPL_TOKEN,
                accounts: vec![
                    AccountMeta::new(wsol_ata, false),       // account to close
                    AccountMeta::new(signer_pubkey, false),   // destination for SOL
                    AccountMeta::new_readonly(signer_pubkey, true), // authority
                ],
                data: vec![9], // 9 = CloseAccount instruction
            });
        }

        debug!("Built {} arb instructions for {} hops", instructions.len(), route.hop_count());
        Ok(instructions)
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
                // Use RPC-cached mint programs (authoritative) instead of pool.extra flags
                let prog_a = self.state_cache.get_mint_program(&pool.token_a_mint);
                let prog_b = self.state_cache.get_mint_program(&pool.token_b_mint);
                build_meteora_dlmm_swap_ix(
                    &self.searcher_keypair.pubkey(), &pool, hop.input_mint,
                    amount_in, minimum_amount_out, prog_a, prog_b,
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
                let src_idx = self.state_cache.get_lst_index(&hop.input_mint)
                    .ok_or_else(|| anyhow::anyhow!("LST index not found for {}", hop.input_mint))?;
                let dst_idx = self.state_cache.get_lst_index(&hop.output_mint)
                    .ok_or_else(|| anyhow::anyhow!("LST index not found for {}", hop.output_mint))?;
                build_sanctum_swap_ix(
                    &self.searcher_keypair.pubkey(),
                    &hop.input_mint,
                    &hop.output_mint,
                    amount_in,
                    minimum_amount_out,
                    src_idx,
                    dst_idx,
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

    /// Build a single `execute_arb` CPI instruction that atomically swaps through
    /// all hops via the arb-guard on-chain program.  Currently supports Orca
    /// Whirlpool hops only; returns an error for mixed-DEX or non-Orca routes.
    pub fn build_execute_arb_ix(
        &self,
        route: &ArbRoute,
        min_amount_out: u64,
    ) -> Result<Instruction> {
        let guard_program = self.arb_guard_program_id
            .ok_or_else(|| anyhow::anyhow!("arb_guard_program_id not set"))?;

        // Currently only Orca Whirlpool is wired up for CPI
        if !route.hops.iter().all(|h| h.dex_type == DexType::OrcaWhirlpool) {
            anyhow::bail!("execute_arb CPI only supports OrcaWhirlpool hops");
        }

        let signer_pubkey = self.searcher_keypair.pubkey();
        let token_program = addresses::SPL_TOKEN;
        let memo_program = addresses::MEMO;
        let orca_program = addresses::ORCA_WHIRLPOOL;

        let first_hop = route.hops.first()
            .ok_or_else(|| anyhow::anyhow!("Route has no hops"))?;

        let input_token_account = derive_ata(&signer_pubkey, &first_hop.input_mint);

        // Fixed accounts [0..6]
        let mut accounts = vec![
            AccountMeta::new(signer_pubkey, true),                   // [0] signer
            AccountMeta::new_readonly(token_program, false),         // [1] token_program
            AccountMeta::new_readonly(memo_program, false),          // [2] memo_program
            AccountMeta::new(input_token_account, false),            // [3] input_token_account
            AccountMeta::new_readonly(first_hop.input_mint, false),  // [4] input_mint
            AccountMeta::new_readonly(orca_program, false),          // [5] orca_program
        ];

        let mut hop_params: Vec<HopParamsClient> = Vec::with_capacity(route.hops.len());

        for hop in &route.hops {
            let pool = self.state_cache.get_any(&hop.pool_address)
                .ok_or_else(|| anyhow::anyhow!("Pool not found: {}", hop.pool_address))?;
            let extra = &pool.extra;
            let vault_a = extra.vault_a
                .ok_or_else(|| anyhow::anyhow!("Missing vault_a for pool {}", hop.pool_address))?;
            let vault_b = extra.vault_b
                .ok_or_else(|| anyhow::anyhow!("Missing vault_b for pool {}", hop.pool_address))?;
            let tick_spacing = extra.tick_spacing
                .ok_or_else(|| anyhow::anyhow!("Missing tick_spacing for pool {}", hop.pool_address))?;

            let a_to_b = hop.input_mint == pool.token_a_mint;

            // Oracle PDA
            let (oracle, _) = Pubkey::find_program_address(
                &[b"oracle", pool.address.as_ref()],
                &orca_program,
            );

            // Tick array PDAs (same logic as build_orca_whirlpool_swap_ix)
            let tick_current = pool.current_tick.unwrap_or(0);
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
                    &orca_program,
                ).0
            }).collect();

            // Output ATA for this hop
            let output_ata = derive_ata(&signer_pubkey, &hop.output_mint);

            // Per-hop accounts (9 each)
            accounts.push(AccountMeta::new(pool.address, false));       // whirlpool
            accounts.push(AccountMeta::new(vault_a, false));            // vault_a
            accounts.push(AccountMeta::new(vault_b, false));            // vault_b
            accounts.push(AccountMeta::new(tick_arrays[0], false));     // tick_array_0
            accounts.push(AccountMeta::new(tick_arrays[1], false));     // tick_array_1
            accounts.push(AccountMeta::new(tick_arrays[2], false));     // tick_array_2
            accounts.push(AccountMeta::new(oracle, false));             // oracle
            accounts.push(AccountMeta::new(output_ata, false));         // output_ata
            accounts.push(AccountMeta::new_readonly(hop.output_mint, false)); // output_mint

            hop_params.push(HopParamsClient { dex_type: 0, a_to_b });
        }

        // Serialize instruction data
        let disc = anchor_discriminator("execute_arb");
        let mut data = Vec::with_capacity(8 + 8 + 8 + 4 + hop_params.len() * 2);
        data.extend_from_slice(&disc);
        data.extend_from_slice(&route.input_amount.to_le_bytes());
        data.extend_from_slice(&min_amount_out.to_le_bytes());
        data.extend_from_slice(&(hop_params.len() as u32).to_le_bytes()); // Borsh Vec prefix
        for hp in &hop_params {
            data.push(hp.dex_type);
            data.push(if hp.a_to_b { 1u8 } else { 0u8 });
        }

        Ok(Instruction {
            program_id: guard_program,
            accounts,
            data,
        })
    }

}

/// Derive an Associated Token Account address (SPL Token only).
/// ATA = PDA([wallet, TOKEN_PROGRAM_ID, mint], ATA_PROGRAM_ID)
fn derive_ata(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
    derive_ata_with_program(wallet, mint, &addresses::SPL_TOKEN)
}

fn derive_ata_with_program(wallet: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    let seeds = &[
        wallet.as_ref(),
        token_program.as_ref(),
        mint.as_ref(),
    ];
    let (ata, _) = Pubkey::find_program_address(seeds, &addresses::ATA_PROGRAM);
    ata
}

// ─── Arb-Guard IX builders ───────────────────────────────────────────────────

/// Compute Anchor instruction discriminator: sha256("global:<name>")[..8]
fn anchor_discriminator(name: &str) -> [u8; 8] {
    let mut hasher = solana_sdk::hash::Hasher::default();
    hasher.hash(format!("global:{}", name).as_bytes());
    let hash = hasher.result();
    let mut disc = [0u8; 8];
    disc.copy_from_slice(&hash.as_ref()[..8]);
    disc
}

/// Derive the guard state PDA: seeds=[b"guard", authority]
fn derive_guard_pda(program_id: &Pubkey, authority: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"guard", authority.as_ref()],
        program_id,
    ).0
}

/// Build start_check IX for arb-guard program.
/// Records the wSOL ATA balance before swaps begin.
fn build_guard_start_check_ix(
    program_id: &Pubkey,
    authority: &Pubkey,
    token_account: &Pubkey,
) -> Instruction {
    let disc = anchor_discriminator("start_check");
    let guard_state = derive_guard_pda(program_id, authority);

    Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(*authority, true),                         // authority (signer, mut for init_if_needed)
            AccountMeta::new(guard_state, false),                       // guard_state PDA (mut)
            AccountMeta::new_readonly(*token_account, false),           // token_account
            AccountMeta::new_readonly(solana_sdk::system_program::id(), false), // system_program
        ],
        data: disc.to_vec(),
    }
}

/// Build profit_check IX for arb-guard program.
/// Verifies balance increased by at least min_profit, then unlocks the guard.
fn build_guard_profit_check_ix(
    program_id: &Pubkey,
    authority: &Pubkey,
    token_account: &Pubkey,
    min_profit: u64,
) -> Instruction {
    let disc = anchor_discriminator("profit_check");
    let guard_state = derive_guard_pda(program_id, authority);

    let mut data = disc.to_vec();
    data.extend_from_slice(&min_profit.to_le_bytes()); // Borsh-serialized u64

    Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new_readonly(*authority, true),   // authority (signer)
            AccountMeta::new(guard_state, false),          // guard_state PDA (mut — updates locked flag)
            AccountMeta::new_readonly(*token_account, false), // token_account
        ],
        data,
    }
}

// ─── DEX swap IX builders ────────────────────────────────────────────────────

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
    let target_orders = extra.target_orders?;
    let market_id = extra.market?;
    let market_program = extra.market_program?;
    let nonce = extra.amm_nonce?;

    // Serum accounts — return None if not yet fetched
    let serum_bids = extra.serum_bids?;
    let serum_asks = extra.serum_asks?;
    let serum_event_queue = extra.serum_event_queue?;
    let serum_coin_vault = extra.serum_coin_vault?;
    let serum_pc_vault = extra.serum_pc_vault?;
    let serum_vault_signer_nonce = extra.serum_vault_signer_nonce?;

    let amm_program = addresses::RAYDIUM_AMM;
    let amm_authority = Pubkey::create_program_address(
        &[&[nonce]],
        &amm_program,
    ).ok()?;

    // Serum vault signer PDA: seeds=[market_id, nonce_le_bytes], program=serum_program
    let serum_vault_signer = Pubkey::create_program_address(
        &[market_id.as_ref(), &serum_vault_signer_nonce.to_le_bytes()],
        &market_program,
    ).ok()?;

    let a_to_b = input_mint == pool.token_a_mint;
    let output_mint = if a_to_b { pool.token_b_mint } else { pool.token_a_mint };
    let user_source_ata = derive_ata(signer, &input_mint);
    let user_dest_ata = derive_ata(signer, &output_mint);

    let mut data = Vec::with_capacity(17);
    data.push(9u8); // swap_base_in instruction discriminator
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());

    let accounts = vec![
        AccountMeta::new_readonly(addresses::SPL_TOKEN, false),   // [0]
        AccountMeta::new(pool.address, false),                     // [1] amm_id
        AccountMeta::new_readonly(amm_authority, false),           // [2] amm_authority
        AccountMeta::new(open_orders, false),                      // [3] amm_open_orders
        AccountMeta::new(target_orders, false),                    // [4] amm_target_orders
        AccountMeta::new(vault_a, false),                          // [5] pool_coin_token_account
        AccountMeta::new(vault_b, false),                          // [6] pool_pc_token_account
        AccountMeta::new_readonly(market_program, false),          // [7] serum_program_id
        AccountMeta::new(market_id, false),                        // [8] serum_market
        AccountMeta::new(serum_bids, false),                       // [9] serum_bids
        AccountMeta::new(serum_asks, false),                       // [10] serum_asks
        AccountMeta::new(serum_event_queue, false),                // [11] serum_event_queue
        AccountMeta::new(serum_coin_vault, false),                 // [12] serum_coin_vault_account
        AccountMeta::new(serum_pc_vault, false),                   // [13] serum_pc_vault_account
        AccountMeta::new_readonly(serum_vault_signer, false),      // [14] serum_vault_signer
        AccountMeta::new(user_source_ata, false),                  // [15] user_source_token_account
        AccountMeta::new(user_dest_ata, false),                    // [16] user_destination_token_account
        AccountMeta::new_readonly(*signer, true),                  // [17] user_source_owner
    ];

    Some(Instruction { program_id: amm_program, accounts, data })
}

/// Shank discriminant for Sanctum S Controller SwapExactIn (NOT Anchor).
const SANCTUM_SWAP_EXACT_IN_DISCM: u8 = 0x01;

/// Build a Sanctum Infinity SwapExactIn instruction (Shank format).
///
/// Data: 27 bytes = discm(1) + src_calc_accs(1) + dst_calc_accs(1)
///       + src_lst_index(4) + dst_lst_index(4) + min_amount_out(8) + amount(8)
/// Accounts: 12 fixed + variable remaining (calculator groups + pricing)
pub fn build_sanctum_swap_ix(
    signer: &Pubkey,
    input_mint: &Pubkey,
    output_mint: &Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
    src_lst_index: u32,
    dst_lst_index: u32,
) -> Option<Instruction> {
    let (src_calc_program, src_calc_suffix, src_calc_accs) =
        crate::config::sanctum_calculator_accounts(input_mint);
    let (dst_calc_program, dst_calc_suffix, dst_calc_accs) =
        crate::config::sanctum_calculator_accounts(output_mint);

    // 12 fixed accounts
    let mut accounts = sanctum_swap_accounts_v2(signer, input_mint, output_mint);

    // Group A: Source calculator remaining accounts
    accounts.push(AccountMeta::new_readonly(src_calc_program, false));
    for acc in &src_calc_suffix {
        accounts.push(AccountMeta::new_readonly(*acc, false));
    }

    // Group B: Destination calculator remaining accounts
    accounts.push(AccountMeta::new_readonly(dst_calc_program, false));
    for acc in &dst_calc_suffix {
        accounts.push(AccountMeta::new_readonly(*acc, false));
    }

    // Group C: Pricing program + state
    accounts.push(AccountMeta::new_readonly(addresses::SANCTUM_PRICING, false));
    accounts.push(AccountMeta::new_readonly(crate::config::sanctum_pricing_state(), false));

    // 27-byte Shank instruction data
    let mut data = Vec::with_capacity(27);
    data.push(SANCTUM_SWAP_EXACT_IN_DISCM);
    data.push(src_calc_accs);
    data.push(dst_calc_accs);
    data.extend_from_slice(&src_lst_index.to_le_bytes());
    data.extend_from_slice(&dst_lst_index.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());
    data.extend_from_slice(&amount_in.to_le_bytes());

    Some(Instruction {
        program_id: addresses::SANCTUM_S_CONTROLLER,
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

    let cp_program = addresses::RAYDIUM_CP;
    let (authority, _) = Pubkey::find_program_address(
        &[b"vault_and_lp_mint_auth_seed"], &cp_program,
    );
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

    let damm_program = addresses::METEORA_DAMM_V2;
    let (pool_authority, _) = Pubkey::find_program_address(&[], &damm_program);
    let (event_authority, _) = Pubkey::find_program_address(&[b"__event_authority"], &damm_program);

    let a_to_b = input_mint == pool.token_a_mint;
    let (input_vault, output_vault) = if a_to_b { (vault_a, vault_b) } else { (vault_b, vault_a) };
    let output_mint = if a_to_b { pool.token_b_mint } else { pool.token_a_mint };

    let token_program_a = extra.token_program_a.unwrap_or(addresses::SPL_TOKEN);
    let token_program_b = extra.token_program_b.unwrap_or(addresses::SPL_TOKEN);

    let input_token_program = if a_to_b { token_program_a } else { token_program_b };
    let output_token_program = if a_to_b { token_program_b } else { token_program_a };

    let user_input_ata = derive_ata_with_program(signer, &input_mint, &input_token_program);
    let user_output_ata = derive_ata_with_program(signer, &output_mint, &output_token_program);

    // DAMM v2 swap2 has a single token_program account (account 8).
    // Use the input side's token program — the on-chain program handles both sides.
    let token_program = input_token_program;

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

    let whirlpool_program = addresses::ORCA_WHIRLPOOL;
    let token_program_a = extra.token_program_a.unwrap_or(addresses::SPL_TOKEN);
    let token_program_b = extra.token_program_b.unwrap_or(addresses::SPL_TOKEN);
    let memo_program = addresses::MEMO;

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
    let user_ata_a = derive_ata_with_program(signer, &pool.token_a_mint, &token_program_a);
    let user_ata_b = derive_ata_with_program(signer, &pool.token_b_mint, &token_program_b);

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
        AccountMeta::new_readonly(token_program_a, false),   // 0: token_program_a
        AccountMeta::new_readonly(token_program_b, false),   // 1: token_program_b
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

    let clmm_program = addresses::RAYDIUM_CLMM;
    let token_program = addresses::SPL_TOKEN;
    let token_2022_program = addresses::TOKEN_2022;
    let memo_program = addresses::MEMO;

    let a_to_b = input_mint == pool.token_a_mint;
    let tick_current = pool.current_tick.unwrap_or(0);

    let (input_vault, output_vault) = if a_to_b { (vault_a, vault_b) } else { (vault_b, vault_a) };
    let output_mint = if a_to_b { pool.token_b_mint } else { pool.token_a_mint };

    let input_token_program = if input_mint == pool.token_a_mint {
        extra.token_program_a.unwrap_or(addresses::SPL_TOKEN)
    } else {
        extra.token_program_b.unwrap_or(addresses::SPL_TOKEN)
    };
    let output_token_program = if output_mint == pool.token_a_mint {
        extra.token_program_a.unwrap_or(addresses::SPL_TOKEN)
    } else {
        extra.token_program_b.unwrap_or(addresses::SPL_TOKEN)
    };
    let user_input_ata = derive_ata_with_program(signer, &input_mint, &input_token_program);
    let user_output_ata = derive_ata_with_program(signer, &output_mint, &output_token_program);

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

    // Pass 0 — on-chain program substitutes correct MIN+1/MAX-1 and determines
    // direction from input vault mint. Eliminates wrong-constant failures.
    // Ref: raydium-clmm/programs/amm/src/instructions/swap_v2.rs lines 153-158
    let sqrt_price_limit: u128 = 0u128;

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
    // Token programs from authoritative RPC source (not pool.extra flags)
    mint_a_program: Option<Pubkey>,
    mint_b_program: Option<Pubkey>,
) -> Option<Instruction> {
    let extra = &pool.extra;
    let vault_a = extra.vault_a?;  // reserve_x
    let vault_b = extra.vault_b?;  // reserve_y

    let dlmm_program = addresses::METEORA_DLMM;
    let token_program = addresses::SPL_TOKEN;
    let memo_program = addresses::MEMO;

    let a_to_b = input_mint == pool.token_a_mint; // X -> Y
    let active_id = pool.current_tick.unwrap_or(0);

    // Use authoritative token programs (from RPC cache), falling back to pool.extra
    let prog_a = mint_a_program
        .or(extra.token_program_a)
        .unwrap_or(token_program);
    let prog_b = mint_b_program
        .or(extra.token_program_b)
        .unwrap_or(token_program);
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
    let bin_array_index = if active_id >= 0 || active_id % 70 == 0 {
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

    // Bitmap extension — use cached value if available (confirmed on-chain).
    // If not cached, pass the DLMM program ID as Anchor's "None" marker.
    // Pools needing the bitmap but not having it will fail, but that's expected
    // (they can't be swapped without it). The bitmap_checked cache in stream.rs
    // tracks which pools have been checked.
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

    let phoenix_program = addresses::PHOENIX_V1;
    let token_program = addresses::SPL_TOKEN;

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

    let manifest_program = addresses::MANIFEST;
    let token_program = addresses::SPL_TOKEN;
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

/// Build the 12 fixed accounts for Sanctum SwapExactIn (Shank format).
fn sanctum_swap_accounts_v2(
    signer: &Pubkey,
    input_mint: &Pubkey,
    output_mint: &Pubkey,
) -> Vec<AccountMeta> {
    let s_controller = addresses::SANCTUM_S_CONTROLLER;
    let token_program = addresses::SPL_TOKEN;

    // PDAs
    let (pool_state_pda, _) = Pubkey::find_program_address(&[b"state"], &s_controller);
    let (lst_state_list_pda, _) = Pubkey::find_program_address(&[b"lst-state-list"], &s_controller);
    let (protocol_fee_pda, _) = Pubkey::find_program_address(&[b"protocol-fee"], &s_controller);

    // ATAs
    let user_src_ata = derive_ata(signer, input_mint);
    let user_dst_ata = derive_ata(signer, output_mint);
    let protocol_fee_accumulator = derive_ata(&protocol_fee_pda, output_mint);
    let src_pool_reserves = derive_ata(&pool_state_pda, input_mint);
    let dst_pool_reserves = derive_ata(&pool_state_pda, output_mint);

    vec![
        AccountMeta::new_readonly(*signer, true),              // 0: signer
        AccountMeta::new_readonly(*input_mint, false),         // 1: src_lst_mint
        AccountMeta::new_readonly(*output_mint, false),        // 2: dst_lst_mint
        AccountMeta::new(user_src_ata, false),                 // 3: src_lst_acc (writable)
        AccountMeta::new(user_dst_ata, false),                 // 4: dst_lst_acc (writable)
        AccountMeta::new(protocol_fee_accumulator, false),     // 5: protocol_fee_accumulator (writable)
        AccountMeta::new_readonly(token_program, false),       // 6: src_lst_token_program
        AccountMeta::new_readonly(token_program, false),       // 7: dst_lst_token_program
        AccountMeta::new(pool_state_pda, false),               // 8: pool_state (writable)
        AccountMeta::new(lst_state_list_pda, false),           // 9: lst_state_list (writable)
        AccountMeta::new(src_pool_reserves, false),            // 10: src_pool_reserves (writable)
        AccountMeta::new(dst_pool_reserves, false),            // 11: dst_pool_reserves (writable)
    ]
}
