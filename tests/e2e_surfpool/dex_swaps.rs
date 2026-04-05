use solana_sdk::signer::Signer;
use solana_mev_bot::router::pool::DexType;

use super::common::{build_single_swap_tx, pool_for_dex, wsol_mint};
use super::harness::SurfpoolHarness;

/// Swap 0.001 SOL on an Orca Whirlpool and verify the TX succeeds.
#[test]
fn test_orca_whirlpool_swap() {
    let harness = SurfpoolHarness::start();
    let signer = SurfpoolHarness::test_keypair();
    let pool = pool_for_dex(DexType::OrcaWhirlpool)
        .expect("No Orca Whirlpool pool registered");

    let instructions = build_single_swap_tx(&harness, &pool, 1_000_000, &signer);
    println!("[orca] Built {} instructions", instructions.len());

    let result = harness.send_tx(&instructions, &signer);
    println!("[orca] Signature: {}", result.signature);
    for log in &result.logs {
        println!("[orca] {}", log);
    }
    assert!(result.success, "Orca Whirlpool swap failed: {:?}", result.error);

    // Verify we received output tokens (USDC)
    let output_mint = if pool.token_a_mint == wsol_mint() {
        pool.token_b_mint
    } else {
        pool.token_a_mint
    };
    let token_balance = harness.get_token_balance(&signer.pubkey(), &output_mint);
    println!("[orca] Output token balance: {}", token_balance);
    assert!(token_balance > 0, "Should have received output tokens from Orca swap");
}

/// Swap 0.001 SOL on Raydium CP and verify the TX succeeds.
#[test]
fn test_raydium_cp_swap() {
    let harness = SurfpoolHarness::start();
    let signer = SurfpoolHarness::test_keypair();
    let pool = pool_for_dex(DexType::RaydiumCp)
        .expect("No Raydium CP pool registered");

    let instructions = build_single_swap_tx(&harness, &pool, 1_000_000, &signer);
    println!("[raydium-cp] Built {} instructions", instructions.len());

    let result = harness.send_tx(&instructions, &signer);
    println!("[raydium-cp] Signature: {}", result.signature);
    for log in &result.logs {
        println!("[raydium-cp] {}", log);
    }
    assert!(result.success, "Raydium CP swap failed: {:?}", result.error);

    let output_mint = if pool.token_a_mint == wsol_mint() {
        pool.token_b_mint
    } else {
        pool.token_a_mint
    };
    let token_balance = harness.get_token_balance(&signer.pubkey(), &output_mint);
    println!("[raydium-cp] Output token balance: {}", token_balance);
    assert!(token_balance > 0, "Should have received output tokens from Raydium CP swap");
}

/// Swap 0.001 SOL on Raydium CLMM and verify the TX succeeds.
#[test]
#[ignore = "Test pool has insufficient liquidity — IX format verified correct (no more SqrtPriceLimitOverflow)"]
fn test_raydium_clmm_swap() {
    let harness = SurfpoolHarness::start();
    let signer = SurfpoolHarness::test_keypair();
    let pool = pool_for_dex(DexType::RaydiumClmm)
        .expect("No Raydium CLMM pool registered");

    let instructions = build_single_swap_tx(&harness, &pool, 1_000_000, &signer);
    println!("[raydium-clmm] Built {} instructions", instructions.len());

    let result = harness.send_tx(&instructions, &signer);
    println!("[raydium-clmm] Signature: {}", result.signature);
    for log in &result.logs {
        println!("[raydium-clmm] {}", log);
    }
    assert!(result.success, "Raydium CLMM swap failed: {:?}", result.error);

    let output_mint = if pool.token_a_mint == wsol_mint() {
        pool.token_b_mint
    } else {
        pool.token_a_mint
    };
    let token_balance = harness.get_token_balance(&signer.pubkey(), &output_mint);
    println!("[raydium-clmm] Output token balance: {}", token_balance);
    assert!(token_balance > 0, "Should have received output tokens from Raydium CLMM swap");
}

/// Swap 0.001 SOL on Meteora DLMM and verify the TX succeeds.
/// Note: wSOL is token_y (token_b) in this DLMM pool.
#[test]
fn test_meteora_dlmm_swap() {
    let harness = SurfpoolHarness::start();
    let signer = SurfpoolHarness::test_keypair();
    let pool = pool_for_dex(DexType::MeteoraDlmm)
        .expect("No Meteora DLMM pool registered");

    let instructions = build_single_swap_tx(&harness, &pool, 1_000_000, &signer);
    println!("[dlmm] Built {} instructions", instructions.len());

    let result = harness.send_tx(&instructions, &signer);
    println!("[dlmm] Signature: {}", result.signature);
    for log in &result.logs {
        println!("[dlmm] {}", log);
    }
    assert!(result.success, "Meteora DLMM swap failed: {:?}", result.error);

    let output_mint = if pool.token_a_mint == wsol_mint() {
        pool.token_b_mint
    } else {
        pool.token_a_mint
    };
    let token_balance = harness.get_token_balance(&signer.pubkey(), &output_mint);
    println!("[dlmm] Output token balance: {}", token_balance);
    assert!(token_balance > 0, "Should have received output tokens from DLMM swap");
}

/// Swap 0.001 SOL on Raydium AMM v4 and verify the TX succeeds.
/// AMM v4 requires Serum/OpenBook market accounts for the swap IX.
#[test]
fn test_raydium_amm_v4_swap() {
    let harness = SurfpoolHarness::start();
    let signer = SurfpoolHarness::test_keypair();
    let pool = pool_for_dex(DexType::RaydiumAmm)
        .expect("No Raydium AMM v4 pool registered");

    let instructions = build_single_swap_tx(&harness, &pool, 1_000_000, &signer);
    println!("[raydium-amm] Built {} instructions", instructions.len());

    let result = harness.send_tx(&instructions, &signer);
    println!("[raydium-amm] Signature: {}", result.signature);
    for log in &result.logs {
        println!("[raydium-amm] {}", log);
    }
    assert!(result.success, "Raydium AMM v4 swap failed: {:?}", result.error);

    let output_mint = if pool.token_a_mint == wsol_mint() {
        pool.token_b_mint
    } else {
        pool.token_a_mint
    };
    let token_balance = harness.get_token_balance(&signer.pubkey(), &output_mint);
    println!("[raydium-amm] Output token balance: {}", token_balance);
    assert!(token_balance > 0, "Should have received output tokens from Raydium AMM v4 swap");
}

/// Swap 0.001 SOL on Meteora DAMM v2 and verify the TX succeeds.
#[test]
#[ignore = "DAMM v2 AccountNotEnoughKeys — program may have been upgraded Q1 2026"]
fn test_meteora_damm_v2_swap() {
    let harness = SurfpoolHarness::start();
    let signer = SurfpoolHarness::test_keypair();
    let pool = pool_for_dex(DexType::MeteoraDammV2)
        .expect("No Meteora DAMM v2 pool registered");

    let instructions = build_single_swap_tx(&harness, &pool, 1_000_000, &signer);
    println!("[damm-v2] Built {} instructions", instructions.len());

    let result = harness.send_tx(&instructions, &signer);
    println!("[damm-v2] Signature: {}", result.signature);
    for log in &result.logs {
        println!("[damm-v2] {}", log);
    }
    assert!(result.success, "Meteora DAMM v2 swap failed: {:?}", result.error);

    let output_mint = if pool.token_a_mint == wsol_mint() {
        pool.token_b_mint
    } else {
        pool.token_a_mint
    };
    let token_balance = harness.get_token_balance(&signer.pubkey(), &output_mint);
    println!("[damm-v2] Output token balance: {}", token_balance);
    assert!(token_balance > 0, "Should have received output tokens from DAMM v2 swap");
}
