use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use solana_system_interface::instruction as system_instruction;
use std::str::FromStr;

use solana_mev_bot::mempool::stream::parse_orca_whirlpool;
use solana_mev_bot::router::pool::DexType;

use super::common::{
    build_execute_arb_ix_e2e, compute_budget_program, create_ata_idempotent_ix, derive_ata,
    memo_program, orca_whirlpool_program, pool_for_dex, spl_token_program, wsol_mint,
};
use super::harness::SurfpoolHarness;

const ARB_GUARD_SO: &str = "programs/arb-guard/target/sbpf-solana-solana/release/arb_guard.so";
const ARB_GUARD_PROGRAM_ID: &str = "CbjPG5TEEhZGXsA8prmJPfvgH51rudYgcubRUtCCGyUw";

/// Test 1: Single-hop Orca swap through CPI executor.
#[test]
fn test_execute_arb_single_hop_orca() {
    let guard_program = Pubkey::from_str(ARB_GUARD_PROGRAM_ID).unwrap();
    let harness = SurfpoolHarness::start_with_program(ARB_GUARD_SO, ARB_GUARD_PROGRAM_ID);
    let signer = SurfpoolHarness::test_keypair();
    let signer_pubkey = signer.pubkey();

    let pool_info = pool_for_dex(DexType::OrcaWhirlpool).expect("No Orca pool");
    let pool_data = harness
        .get_account_data(&pool_info.address)
        .expect("Failed to fetch Orca pool data");
    let pool_state =
        parse_orca_whirlpool(&pool_info.address, &pool_data, 0).expect("Failed to parse Orca pool");

    let wsol = wsol_mint();
    let a_to_b = pool_state.token_a_mint == wsol;
    let output_mint = if a_to_b {
        pool_state.token_b_mint
    } else {
        pool_state.token_a_mint
    };

    let amount_lamports: u64 = 1_000_000; // 0.001 SOL

    let mut instructions = Vec::new();

    // Compute budget
    let mut cu_limit = vec![2u8];
    cu_limit.extend_from_slice(&400_000u32.to_le_bytes());
    instructions.push(Instruction {
        program_id: compute_budget_program(),
        accounts: vec![],
        data: cu_limit,
    });
    let mut cu_price = vec![3u8];
    cu_price.extend_from_slice(&1_000u64.to_le_bytes());
    instructions.push(Instruction {
        program_id: compute_budget_program(),
        accounts: vec![],
        data: cu_price,
    });

    // ATA creates
    let wsol_ata = derive_ata(&signer_pubkey, &wsol);
    instructions.push(create_ata_idempotent_ix(
        &signer_pubkey,
        &wsol_ata,
        &wsol,
        &spl_token_program(),
    ));
    let output_ata = derive_ata(&signer_pubkey, &output_mint);
    instructions.push(create_ata_idempotent_ix(
        &signer_pubkey,
        &output_ata,
        &output_mint,
        &spl_token_program(),
    ));

    // wSOL wrap
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

    // execute_arb: single hop
    let execute_arb_ix = build_execute_arb_ix_e2e(
        &guard_program,
        &signer_pubkey,
        &wsol,
        amount_lamports,
        0, // min_amount_out = 0 (just test CPI works)
        &[(pool_state, a_to_b, output_mint)],
    );
    instructions.push(execute_arb_ix);

    let result = harness.send_tx(&instructions, &signer);
    println!("[arb-guard-cpi] Signature: {}", result.signature);
    for log in &result.logs {
        println!("[arb-guard-cpi] {}", log);
    }
    assert!(
        result.success,
        "execute_arb single-hop should succeed: {:?}",
        result.error
    );

    let output_balance = harness.get_token_balance(&signer_pubkey, &output_mint);
    println!("[arb-guard-cpi] Output token balance: {}", output_balance);
    assert!(output_balance > 0, "Should have received output tokens");
}

/// Test 2: min_amount_out enforcement -- should revert.
#[test]
fn test_execute_arb_min_output_revert() {
    let guard_program = Pubkey::from_str(ARB_GUARD_PROGRAM_ID).unwrap();
    let harness = SurfpoolHarness::start_with_program(ARB_GUARD_SO, ARB_GUARD_PROGRAM_ID);
    let signer = SurfpoolHarness::test_keypair();
    let signer_pubkey = signer.pubkey();

    let pool_info = pool_for_dex(DexType::OrcaWhirlpool).expect("No Orca pool");
    let pool_data = harness
        .get_account_data(&pool_info.address)
        .expect("Failed to fetch Orca pool data");
    let pool_state =
        parse_orca_whirlpool(&pool_info.address, &pool_data, 0).expect("Failed to parse Orca pool");

    let wsol = wsol_mint();
    let a_to_b = pool_state.token_a_mint == wsol;
    let output_mint = if a_to_b {
        pool_state.token_b_mint
    } else {
        pool_state.token_a_mint
    };

    let amount_lamports: u64 = 1_000_000;

    let mut instructions = Vec::new();

    let mut cu_limit = vec![2u8];
    cu_limit.extend_from_slice(&400_000u32.to_le_bytes());
    instructions.push(Instruction {
        program_id: compute_budget_program(),
        accounts: vec![],
        data: cu_limit,
    });
    let mut cu_price = vec![3u8];
    cu_price.extend_from_slice(&1_000u64.to_le_bytes());
    instructions.push(Instruction {
        program_id: compute_budget_program(),
        accounts: vec![],
        data: cu_price,
    });

    let wsol_ata = derive_ata(&signer_pubkey, &wsol);
    instructions.push(create_ata_idempotent_ix(
        &signer_pubkey,
        &wsol_ata,
        &wsol,
        &spl_token_program(),
    ));
    let output_ata = derive_ata(&signer_pubkey, &output_mint);
    instructions.push(create_ata_idempotent_ix(
        &signer_pubkey,
        &output_ata,
        &output_mint,
        &spl_token_program(),
    ));
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

    let execute_arb_ix = build_execute_arb_ix_e2e(
        &guard_program,
        &signer_pubkey,
        &wsol,
        amount_lamports,
        u64::MAX, // impossibly high
        &[(pool_state, a_to_b, output_mint)],
    );
    instructions.push(execute_arb_ix);

    let result = harness.send_tx(&instructions, &signer);
    println!("[arb-guard-revert] Signature: {}", result.signature);
    for log in &result.logs {
        println!("[arb-guard-revert] {}", log);
    }
    assert!(
        !result.success,
        "execute_arb should REVERT when min_amount_out is impossibly high"
    );
}

/// Test 3: Unsupported DEX type should revert.
#[test]
fn test_execute_arb_unsupported_dex_revert() {
    let guard_program = Pubkey::from_str(ARB_GUARD_PROGRAM_ID).unwrap();
    let harness = SurfpoolHarness::start_with_program(ARB_GUARD_SO, ARB_GUARD_PROGRAM_ID);
    let signer = SurfpoolHarness::test_keypair();
    let signer_pubkey = signer.pubkey();

    let pool_info = pool_for_dex(DexType::OrcaWhirlpool).expect("No Orca pool");
    let pool_data = harness
        .get_account_data(&pool_info.address)
        .expect("Failed to fetch Orca pool data");
    let pool_state =
        parse_orca_whirlpool(&pool_info.address, &pool_data, 0).expect("Failed to parse Orca pool");

    let wsol = wsol_mint();
    let a_to_b = pool_state.token_a_mint == wsol;
    let output_mint = if a_to_b {
        pool_state.token_b_mint
    } else {
        pool_state.token_a_mint
    };
    let amount_lamports: u64 = 1_000_000;

    // Build IX manually with dex_type=1 (unsupported)
    let token_program = spl_token_program();
    let memo = memo_program();
    let orca_program = orca_whirlpool_program();
    let wsol_ata = derive_ata(&signer_pubkey, &wsol);
    let output_ata = derive_ata(&signer_pubkey, &output_mint);

    let accounts = vec![
        AccountMeta::new(signer_pubkey, true),
        AccountMeta::new_readonly(token_program, false),
        AccountMeta::new_readonly(memo, false),
        AccountMeta::new(wsol_ata, false),
        AccountMeta::new_readonly(wsol, false),
        AccountMeta::new_readonly(orca_program, false),
        // 9 dummy hop accounts
        AccountMeta::new(pool_info.address, false),
        AccountMeta::new(Pubkey::new_unique(), false),
        AccountMeta::new(Pubkey::new_unique(), false),
        AccountMeta::new(Pubkey::new_unique(), false),
        AccountMeta::new(Pubkey::new_unique(), false),
        AccountMeta::new(Pubkey::new_unique(), false),
        AccountMeta::new(Pubkey::new_unique(), false),
        AccountMeta::new(output_ata, false),
        AccountMeta::new_readonly(output_mint, false),
    ];

    // Build discriminator manually
    use solana_sdk::hash::Hasher;
    let mut hasher = Hasher::default();
    hasher.hash(b"global:execute_arb");
    let hash = hasher.result();
    let mut data = hash.as_ref()[..8].to_vec();
    data.extend_from_slice(&amount_lamports.to_le_bytes());
    data.extend_from_slice(&0u64.to_le_bytes()); // min_amount_out = 0
    data.extend_from_slice(&1u32.to_le_bytes()); // 1 hop
    data.push(1u8); // dex_type = 1 (UNSUPPORTED)
    data.push(1u8); // a_to_b = true

    let ix = Instruction {
        program_id: guard_program,
        accounts,
        data,
    };

    let mut instructions = Vec::new();
    let mut cu_limit = vec![2u8];
    cu_limit.extend_from_slice(&400_000u32.to_le_bytes());
    instructions.push(Instruction {
        program_id: compute_budget_program(),
        accounts: vec![],
        data: cu_limit,
    });
    instructions.push(create_ata_idempotent_ix(
        &signer_pubkey,
        &wsol_ata,
        &wsol,
        &token_program,
    ));
    instructions.push(system_instruction::transfer(
        &signer_pubkey,
        &wsol_ata,
        amount_lamports,
    ));
    instructions.push(Instruction {
        program_id: token_program,
        accounts: vec![AccountMeta::new(wsol_ata, false)],
        data: vec![17], // SyncNative
    });
    instructions.push(ix);

    let result = harness.send_tx(&instructions, &signer);
    println!("[arb-guard-unsupported] Signature: {}", result.signature);
    for log in &result.logs {
        println!("[arb-guard-unsupported] {}", log);
    }
    assert!(
        !result.success,
        "execute_arb should REVERT for unsupported dex_type"
    );
}
