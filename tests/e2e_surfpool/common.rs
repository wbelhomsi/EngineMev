use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    system_instruction,
};
use std::str::FromStr;

use solana_mev_bot::mempool::stream::{
    parse_meteora_damm_v2, parse_meteora_dlmm, parse_orca_whirlpool, parse_raydium_clmm,
    parse_raydium_cp,
};
use solana_mev_bot::executor::bundle::{
    build_damm_v2_swap_ix, build_meteora_dlmm_swap_ix, build_orca_whirlpool_swap_ix,
    build_raydium_clmm_swap_ix, build_raydium_cp_swap_ix,
};
use solana_mev_bot::router::pool::{DexType, PoolState};

use super::harness::SurfpoolHarness;

// ─── Well-known program IDs ─────────────────────────────────────────────────

fn spl_token_program() -> Pubkey {
    Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap()
}

fn ata_program() -> Pubkey {
    Pubkey::from_str("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL").unwrap()
}

fn compute_budget_program() -> Pubkey {
    Pubkey::from_str("ComputeBudget111111111111111111111111111111").unwrap()
}

pub fn wsol_mint() -> Pubkey {
    Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap()
}

// ─── Pool registry ──────────────────────────────────────────────────────────

/// A verified mainnet pool with known addresses and token mints.
#[derive(Debug, Clone)]
pub struct KnownPool {
    pub address: Pubkey,
    pub dex_type: DexType,
    pub token_a_mint: Pubkey,
    pub token_b_mint: Pubkey,
    pub data_size: usize,
}

/// Return all verified mainnet pools for testing.
pub fn known_pools() -> Vec<KnownPool> {
    vec![
        // Orca Whirlpool: SOL/USDC (653 bytes)
        KnownPool {
            address: Pubkey::from_str("HJPjoWUrhoZzkNfRpHuieeFk9WcZWjwy6PBjZ81ngndJ").unwrap(),
            dex_type: DexType::OrcaWhirlpool,
            token_a_mint: Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap(),
            token_b_mint: Pubkey::from_str("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap(),
            data_size: 653,
        },
        // Raydium CP: SOL/HrNsut7D... (637 bytes)
        KnownPool {
            address: Pubkey::from_str("HxzVq7QyztLVzq671ZqCe6UdbF9undvmMi8kWbjpWKEP").unwrap(),
            dex_type: DexType::RaydiumCp,
            token_a_mint: Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap(),
            token_b_mint: Pubkey::from_str("HrNsut7DMXWDYSzHL4M4d5UHWky4f5tSENNmZ8Vhsurg").unwrap(),
            data_size: 637,
        },
        // Raydium CLMM: SOL/2yrvNxT6... (1544 bytes)
        KnownPool {
            address: Pubkey::from_str("EyH84WGeShUdkpmRVcpdk9LeLimAzULLbjkGanRkYqLA").unwrap(),
            dex_type: DexType::RaydiumClmm,
            token_a_mint: Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap(),
            token_b_mint: Pubkey::from_str("2yrvNxT6UgBPNyPCgFUbas2FR6NeCjYLQ2oHjZKHM7yN").unwrap(),
            data_size: 1544,
        },
        // Meteora DLMM: CGEDT9Q.../SOL (904 bytes) — wSOL is token_y (token_b)
        KnownPool {
            address: Pubkey::from_str("CyxH2W4gU2gX3GGsVWpbf3ExKPRxKSdno38RB7QTPpng").unwrap(),
            dex_type: DexType::MeteoraDlmm,
            token_a_mint: Pubkey::from_str("CGEDT9QZDvvH5GmVkWJH2BXiMJqMJySC9ihWyr7Spump").unwrap(),
            token_b_mint: Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap(),
            data_size: 904,
        },
        // Meteora DAMM v2: 6S9FeWWj.../SOL (1112 bytes) — wSOL is token_b
        KnownPool {
            address: Pubkey::from_str("8vqz18RQFnUQyZpMYkw1KpZUMZLVjRJJNYGcV3xzRyQK").unwrap(),
            dex_type: DexType::MeteoraDammV2,
            token_a_mint: Pubkey::from_str("6S9FeWWj4XcR7bVqQRaBs8Eh9e76pHSCPgAqWGUgkLTg").unwrap(),
            token_b_mint: Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap(),
            data_size: 1112,
        },
    ]
}

/// Find a known pool for a given DEX type. Returns None if not registered.
pub fn pool_for_dex(dex_type: DexType) -> Option<KnownPool> {
    known_pools().into_iter().find(|p| p.dex_type == dex_type)
}

// ─── ATA derivation (mirrors bundle.rs logic) ──────────────────────────────

fn derive_ata(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
    derive_ata_with_program(wallet, mint, &spl_token_program())
}

fn derive_ata_with_program(wallet: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    let seeds = &[
        wallet.as_ref(),
        token_program.as_ref(),
        mint.as_ref(),
    ];
    let (ata, _) = Pubkey::find_program_address(seeds, &ata_program());
    ata
}

// ─── Swap TX builder ────────────────────────────────────────────────────────

/// Build a complete set of instructions for a single swap on a DEX.
///
/// The instruction sequence is:
/// 1. SetComputeUnitLimit (400K CU)
/// 2. SetComputeUnitPrice (1000 micro-lamports)
/// 3. CreateIdempotent ATA for input token (if not wSOL, wSOL ATA always created)
/// 4. CreateIdempotent ATA for output token
/// 5. Transfer SOL to wSOL ATA + SyncNative (wrap native SOL)
/// 6. Swap instruction (DEX-specific)
/// 7. CloseAccount on wSOL ATA (unwrap back to native SOL)
///
/// `amount_lamports` is the amount of SOL (in lamports) to swap.
/// The input side is always wSOL. The output side is the other token in the pool.
pub fn build_single_swap_tx(
    harness: &SurfpoolHarness,
    pool: &KnownPool,
    amount_lamports: u64,
    signer: &Keypair,
) -> Vec<Instruction> {
    let signer_pubkey = signer.pubkey();
    let wsol = wsol_mint();

    // Fetch pool account data from Surfpool
    let pool_data = harness
        .get_account_data(&pool.address)
        .unwrap_or_else(|| panic!("Failed to fetch pool account data for {}", pool.address));
    println!(
        "[common] Fetched pool {} data: {} bytes (expected {})",
        pool.address,
        pool_data.len(),
        pool.data_size
    );

    // Parse pool state using the appropriate DEX parser
    let pool_state = parse_pool(&pool.address, &pool_data, pool.dex_type);

    // Determine swap direction: we always swap wSOL in, other token out
    let (input_mint, output_mint) = if pool_state.token_a_mint == wsol {
        (pool_state.token_a_mint, pool_state.token_b_mint)
    } else if pool_state.token_b_mint == wsol {
        (pool_state.token_b_mint, pool_state.token_a_mint)
    } else {
        panic!(
            "Pool {} does not contain wSOL. token_a={}, token_b={}",
            pool.address, pool_state.token_a_mint, pool_state.token_b_mint
        );
    };

    // Resolve token programs for each mint
    let input_token_program = spl_token_program(); // wSOL is always SPL Token
    let output_token_program = resolve_token_program(&pool_state, &output_mint);

    let mut instructions = Vec::with_capacity(10);

    // 1. Compute budget: SetComputeUnitLimit
    let mut cu_limit_data = vec![2u8];
    cu_limit_data.extend_from_slice(&400_000u32.to_le_bytes());
    instructions.push(Instruction {
        program_id: compute_budget_program(),
        accounts: vec![],
        data: cu_limit_data,
    });

    // 2. Compute budget: SetComputeUnitPrice
    let mut cu_price_data = vec![3u8];
    cu_price_data.extend_from_slice(&1_000u64.to_le_bytes());
    instructions.push(Instruction {
        program_id: compute_budget_program(),
        accounts: vec![],
        data: cu_price_data,
    });

    // 3. Create ATA for wSOL (input)
    let wsol_ata = derive_ata(&signer_pubkey, &wsol);
    instructions.push(create_ata_idempotent_ix(
        &signer_pubkey,
        &wsol_ata,
        &wsol,
        &input_token_program,
    ));

    // 4. Create ATA for output token
    let output_ata = derive_ata_with_program(&signer_pubkey, &output_mint, &output_token_program);
    instructions.push(create_ata_idempotent_ix(
        &signer_pubkey,
        &output_ata,
        &output_mint,
        &output_token_program,
    ));

    // 5. Wrap native SOL: transfer + SyncNative
    instructions.push(system_instruction::transfer(
        &signer_pubkey,
        &wsol_ata,
        amount_lamports,
    ));
    instructions.push(Instruction {
        program_id: spl_token_program(),
        accounts: vec![AccountMeta::new(wsol_ata, false)],
        data: vec![17], // SyncNative
    });

    // 6. Build DEX-specific swap instruction
    let swap_ix = build_swap_ix(
        &signer_pubkey,
        &pool_state,
        input_mint,
        amount_lamports,
        0, // minimum_amount_out = 0 for tests
        harness,
    );
    instructions.push(swap_ix);

    // 7. Close wSOL ATA (unwrap back to native SOL)
    instructions.push(Instruction {
        program_id: spl_token_program(),
        accounts: vec![
            AccountMeta::new(wsol_ata, false),
            AccountMeta::new(signer_pubkey, false),
            AccountMeta::new_readonly(signer_pubkey, true),
        ],
        data: vec![9], // CloseAccount
    });

    instructions
}

/// Parse pool account data using the correct DEX parser.
fn parse_pool(pool_address: &Pubkey, data: &[u8], dex_type: DexType) -> PoolState {
    match dex_type {
        DexType::OrcaWhirlpool => {
            parse_orca_whirlpool(pool_address, data, 0)
                .unwrap_or_else(|| panic!("Failed to parse Orca Whirlpool pool {}", pool_address))
        }
        DexType::RaydiumClmm => {
            parse_raydium_clmm(pool_address, data, 0)
                .unwrap_or_else(|| panic!("Failed to parse Raydium CLMM pool {}", pool_address))
        }
        DexType::RaydiumCp => {
            // Raydium CP returns (PoolState, (vault0, vault1)) — reserves are 0 until vault fetch
            let (mut pool, _vaults) = parse_raydium_cp(pool_address, data, 0)
                .unwrap_or_else(|| panic!("Failed to parse Raydium CP pool {}", pool_address));
            // Set placeholder reserves — we only care about IX format in tests, not amounts
            pool.token_a_reserve = 1_000_000_000;
            pool.token_b_reserve = 1_000_000_000;
            pool
        }
        DexType::MeteoraDlmm => {
            parse_meteora_dlmm(pool_address, data, 0)
                .unwrap_or_else(|| panic!("Failed to parse DLMM pool {}", pool_address))
        }
        DexType::MeteoraDammV2 => {
            parse_meteora_damm_v2(pool_address, data, 0)
                .unwrap_or_else(|| panic!("Failed to parse DAMM v2 pool {}", pool_address))
        }
        other => panic!("Unsupported DEX type for e2e tests: {:?}", other),
    }
}

/// Build a swap instruction for the given pool and direction.
fn build_swap_ix(
    signer: &Pubkey,
    pool: &PoolState,
    input_mint: Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
    _harness: &SurfpoolHarness,
) -> Instruction {
    match pool.dex_type {
        DexType::OrcaWhirlpool => {
            build_orca_whirlpool_swap_ix(signer, pool, input_mint, amount_in, minimum_amount_out)
                .expect("Failed to build Orca Whirlpool swap IX")
        }
        DexType::RaydiumClmm => {
            build_raydium_clmm_swap_ix(signer, pool, input_mint, amount_in, minimum_amount_out)
                .expect("Failed to build Raydium CLMM swap IX")
        }
        DexType::RaydiumCp => {
            build_raydium_cp_swap_ix(signer, pool, input_mint, amount_in, minimum_amount_out)
                .expect("Failed to build Raydium CP swap IX")
        }
        DexType::MeteoraDlmm => {
            // Pass None for mint programs — let the builder use pool.extra flags
            build_meteora_dlmm_swap_ix(
                signer, pool, input_mint, amount_in, minimum_amount_out, None, None,
            )
            .expect("Failed to build DLMM swap IX")
        }
        DexType::MeteoraDammV2 => {
            build_damm_v2_swap_ix(signer, pool, input_mint, amount_in, minimum_amount_out)
                .expect("Failed to build DAMM v2 swap IX")
        }
        other => panic!("Unsupported DEX type for swap IX building: {:?}", other),
    }
}

/// Resolve the token program for a non-wSOL mint from PoolExtra flags.
fn resolve_token_program(pool: &PoolState, mint: &Pubkey) -> Pubkey {
    let extra = &pool.extra;
    if *mint == pool.token_a_mint {
        extra.token_program_a.unwrap_or_else(spl_token_program)
    } else if *mint == pool.token_b_mint {
        extra.token_program_b.unwrap_or_else(spl_token_program)
    } else {
        spl_token_program()
    }
}

/// Build a CreateIdempotent ATA instruction.
fn create_ata_idempotent_ix(
    payer: &Pubkey,
    ata: &Pubkey,
    mint: &Pubkey,
    token_program: &Pubkey,
) -> Instruction {
    Instruction {
        program_id: ata_program(),
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(*ata, false),
            AccountMeta::new_readonly(*payer, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
            AccountMeta::new_readonly(*token_program, false),
        ],
        data: vec![1], // 1 = CreateIdempotent
    }
}
