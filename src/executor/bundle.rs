use anyhow::Result;
use borsh::BorshSerialize;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
};
use solana_system_interface::instruction as system_instruction;
use tracing::debug;

use crate::addresses;
use crate::executor::swaps::{
    build_damm_v2_swap_ix, build_manifest_swap_ix, build_meteora_dlmm_swap_ix,
    build_orca_whirlpool_swap_ix, build_phoenix_swap_ix, build_pumpswap_swap_ix,
    build_raydium_amm_swap_ix, build_raydium_clmm_swap_ix, build_raydium_cp_swap_ix,
    build_sanctum_swap_ix, derive_ata, derive_ata_with_program,
};
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
    /// When set, execute_arb_v2 CPI wraps all swap hops atomically.
    arb_guard_program_id: Option<Pubkey>,
}

/// Client-side mirror of on-chain ArbV2Params for Borsh serialization.
#[derive(BorshSerialize)]
struct ArbV2Params {
    min_amount_out: u64,
    hops: Vec<HopV2Params>,
}

/// Client-side mirror of on-chain HopV2Params for Borsh serialization.
#[derive(BorshSerialize)]
struct HopV2Params {
    program_id_index: u8,
    accounts_start: u8,
    accounts_len: u8,
    output_token_index: u8,
    amount_in_offset: u8,
    ix_data: Vec<u8>,
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

        // Try arb-guard CPI executor first. If the resulting tx would be too large
        // (many remaining_accounts), fall through to the non-CPI path with separate swap IXs.
        if self.arb_guard_program_id.is_some() {
            let mut instructions = Vec::with_capacity(8);
            let compute_budget_program = addresses::COMPUTE_BUDGET;
            let signer_pubkey = self.searcher_keypair.pubkey();

            // Compute budget
            let mut cu_limit_data = vec![2u8];
            cu_limit_data.extend_from_slice(&400_000u32.to_le_bytes());
            instructions.push(Instruction { program_id: compute_budget_program, accounts: vec![], data: cu_limit_data });

            // RequestHeapFrame: expand heap from 32KB to 256KB for complex CPI chains
            let mut heap_data = vec![1u8]; // instruction type 1
            heap_data.extend_from_slice(&(256 * 1024u32).to_le_bytes());
            instructions.push(Instruction { program_id: compute_budget_program, accounts: vec![], data: heap_data });
            // SetComputeUnitPrice omitted — tip determines bundle priority, not CU price

            // ATA creates (reuse existing logic pattern)
            let ata_program = addresses::ATA_PROGRAM;
            let token_program = addresses::SPL_TOKEN;
            let mut ata_mints: Vec<(Pubkey, Pubkey)> = Vec::new();
            for hop in &route.hops {
                for mint in [hop.input_mint, hop.output_mint] {
                    if !ata_mints.iter().any(|(m, _)| *m == mint) {
                        let prog = if mint == wsol {
                            token_program
                        } else {
                            match self.state_cache.get_mint_program(&mint) {
                                Some(p) => p,
                                None => {
                                    // Mint program not cached — check pool extra for hints.
                                    // If a hop involves this mint, the pool's parser may have
                                    // stored the token program in PoolExtra.
                                    let pool_prog = route.hops.iter()
                                        .find_map(|h| {
                                            let pool = self.state_cache.get_any(&h.pool_address)?;
                                            if pool.token_a_mint == mint {
                                                pool.extra.token_program_a
                                            } else if pool.token_b_mint == mint {
                                                pool.extra.token_program_b
                                            } else {
                                                None
                                            }
                                        });
                                    match pool_prog {
                                        Some(p) => {
                                            // Cache it for next time
                                            self.state_cache.set_mint_program(mint, p);
                                            p
                                        }
                                        None => {
                                            return Err(anyhow::anyhow!(
                                                "Mint program unknown for {} — cannot build ATA (would use wrong token program)",
                                                mint
                                            ));
                                        }
                                    }
                                }
                            }
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
                        AccountMeta::new_readonly(solana_system_interface::program::id(), false),
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

            // The single execute_arb_v2 IX (works with all DEX types)
            instructions.push(self.build_execute_arb_v2_ix(route, min_final_output)?);

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

            debug!("Built {} arb instructions (CPI executor v2) for {} hops", instructions.len(), route.hop_count());
            return Ok(instructions);
        }

        // No-guard path: raw swap IXs without arb-guard wrapping.

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

        // RequestHeapFrame: expand heap from 32KB to 256KB for complex CPI chains
        let mut heap_data = vec![1u8]; // instruction type 1
        heap_data.extend_from_slice(&(256 * 1024u32).to_le_bytes());
        instructions.push(Instruction {
            program_id: compute_budget_program,
            accounts: vec![],
            data: heap_data,
        });
        // Note: SetComputeUnitPrice omitted — for bundle submission, priority is
        // determined by the Jito tip, not the compute unit price. Omitting saves
        // ~20 bytes per tx, keeping routes under the 1232-byte limit with ALT.

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
                    AccountMeta::new_readonly(solana_system_interface::program::id(), false),
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

        debug!(
            "Built {} arb instructions for {} hops (no guard, raw swaps)",
            instructions.len(), route.hop_count()
        );
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
                    .ok_or_else(|| anyhow::anyhow!("LST index not found for src_mint {}", hop.input_mint))?;
                let dst_idx = self.state_cache.get_lst_index(&hop.output_mint)
                    .ok_or_else(|| anyhow::anyhow!("LST index not found for dst_mint {}", hop.output_mint))?;
                tracing::debug!(
                    "Sanctum swap: src_mint={} idx={}, dst_mint={} idx={}",
                    hop.input_mint, src_idx, hop.output_mint, dst_idx
                );
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
            DexType::PumpSwap => {
                let pool = self.state_cache.get_any(&hop.pool_address)
                    .ok_or_else(|| anyhow::anyhow!("Pool not found for PumpSwap: {}", hop.pool_address))?;
                build_pumpswap_swap_ix(
                    &self.searcher_keypair.pubkey(), &pool, hop.input_mint,
                    amount_in, minimum_amount_out,
                ).ok_or_else(|| anyhow::anyhow!("Missing pool data for PumpSwap"))
            }
        }
    }

    /// Build a single `execute_arb_v2` CPI instruction that works with ALL DEXes.
    /// Decomposes per-hop swap IXs into remaining_accounts + HopV2Params.
    pub fn build_execute_arb_v2_ix(
        &self,
        route: &ArbRoute,
        min_amount_out: u64,
    ) -> Result<Instruction> {
        let guard_program = self.arb_guard_program_id
            .ok_or_else(|| anyhow::anyhow!("arb_guard_program_id not set"))?;

        let signer_pubkey = self.searcher_keypair.pubkey();

        // Build per-hop swap IXs using existing builders
        let mut hop_ixs = Vec::new();
        let last_idx = route.hops.len() - 1;
        for (i, hop) in route.hops.iter().enumerate() {
            let min_out = if i == last_idx { min_amount_out } else { 0 };
            let amount_in = if i == 0 {
                route.input_amount
            } else {
                route.hops[i - 1].estimated_output
            };
            let ix = self.build_swap_instruction_with_min_out(hop, amount_in, min_out)?;
            hop_ixs.push(ix);
        }

        // Flatten all accounts into remaining_accounts WITHOUT dedup.
        // Each hop gets a contiguous slice. The on-chain program passes
        // only the hop's slice to invoke(), preserving DEX account ordering.
        // V0 message with ALT handles dedup at the transaction level.
        let mut remaining_accounts: Vec<AccountMeta> = vec![
            AccountMeta::new(signer_pubkey, true), // signer always first
        ];
        let mut hop_params = Vec::new();

        for (i, ix) in hop_ixs.iter().enumerate() {
            // Add the DEX program
            let program_id_index = remaining_accounts.len() as u8;
            remaining_accounts.push(AccountMeta::new_readonly(ix.program_id, false));

            // Add hop accounts (contiguous slice)
            let accounts_start = remaining_accounts.len() as u8;
            for meta in &ix.accounts {
                remaining_accounts.push(meta.clone());
            }
            let accounts_len = ix.accounts.len() as u8;

            // Find output token account index
            let output_mint = route.hops[i].output_mint;
            let output_token_program = if output_mint == addresses::WSOL {
                addresses::SPL_TOKEN
            } else {
                self.state_cache.get_mint_program(&output_mint).unwrap_or(addresses::SPL_TOKEN)
            };
            let output_ata = derive_ata_with_program(&signer_pubkey, &output_mint, &output_token_program);
            let output_token_index = match remaining_accounts.iter()
                .position(|a| a.pubkey == output_ata)
            {
                Some(idx) => idx as u8,
                None => {
                    return Err(anyhow::anyhow!(
                        "Output ATA {} (mint={}, prog={}) not found in remaining_accounts for hop {}",
                        output_ata, output_mint, output_token_program, i
                    ));
                }
            };

            // amount_in byte offset in instruction data:
            // Raydium AMM V4: 1-byte discriminator, then amount_in at offset 1
            // Anchor DEXes (Orca, CLMM, DLMM, DAMM v2, Raydium CP, Sanctum): 8-byte discriminator, then amount_in at offset 8
            // Phoenix/Manifest: 8-byte discriminator, then amount_in at offset 8
            let amount_in_offset: u8 = match route.hops[i].dex_type {
                DexType::RaydiumAmm => 1,
                _ => 8,
            };

            hop_params.push(HopV2Params {
                program_id_index,
                accounts_start,
                accounts_len,
                output_token_index,
                amount_in_offset,
                ix_data: ix.data.clone(),
            });
        }

        // Serialize ArbV2Params using Borsh (Anchor format).
        // Discriminator: sha256("global:execute_arb_v2")[..8] = [141, 60, 173, 81, 122, 89, 6, 39]
        let params = ArbV2Params {
            min_amount_out,
            hops: hop_params,
        };

        let discriminator: [u8; 8] = [141, 60, 173, 81, 122, 89, 6, 39];
        let mut data = discriminator.to_vec();
        borsh::to_writer(&mut data, &params)
            .map_err(|e| anyhow::anyhow!("Failed to serialize ArbV2Params: {}", e))?;

        Ok(Instruction {
            program_id: guard_program,
            accounts: remaining_accounts,
            data,
        })
    }

}

// ─── Tx size estimation ──────────────────────────────────────────────────────

/// Count unique accounts (program IDs + account metas) across instructions.
/// Used to estimate V0 transaction size — each account not in the ALT costs
/// 32 bytes as a static key.
pub fn estimate_unique_accounts(instructions: &[Instruction]) -> usize {
    let mut seen = std::collections::HashSet::new();
    for ix in instructions {
        seen.insert(ix.program_id);
        for meta in &ix.accounts {
            seen.insert(meta.pubkey);
        }
    }
    seen.len()
}

