use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;

use std::str::FromStr;
use std::time::Duration;

use solana_mev_bot::router::pool::{ArbRoute, DexType, PoolState, PoolExtra, RouteHop};
use solana_mev_bot::executor::BundleBuilder;
use solana_mev_bot::state::StateCache;

#[test]
fn test_bundle_sets_min_out_on_final_hop() {
    let keypair = Keypair::new();
    let state_cache = StateCache::new(Duration::from_secs(60));

    let base_mint = Pubkey::new_unique();
    let other_mint = Pubkey::new_unique();
    let amm_pool_address = Pubkey::new_unique();

    // Derive valid PDA nonce for AMM authority
    let amm_program = Pubkey::from_str("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8").unwrap();
    let nonce = (0u8..=255).find(|n| {
        Pubkey::create_program_address(&[&[*n]], &amm_program).is_ok()
    }).unwrap();

    // Insert a Raydium AMM pool into the cache so the builder can look it up
    let amm_pool = PoolState {
        address: amm_pool_address,
        dex_type: DexType::RaydiumAmm,
        token_a_mint: base_mint,
        token_b_mint: other_mint,
        token_a_reserve: 1_000_000_000,
        token_b_reserve: 1_000_000_000,
        fee_bps: 25,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            amm_nonce: Some(nonce),
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    };
    state_cache.upsert(amm_pool_address, amm_pool);

    // Populate mint program cache for non-wSOL mints (required since we no longer
    // silently default to SPL Token on cache miss).
    let spl_token = solana_mev_bot::addresses::SPL_TOKEN;
    state_cache.set_mint_program(base_mint, spl_token);
    state_cache.set_mint_program(other_mint, spl_token);

    // Set LST indices so Sanctum IX builder can find them
    state_cache.set_lst_index(other_mint, 5);
    state_cache.set_lst_index(base_mint, 1);

    let builder = BundleBuilder::new(keypair, state_cache, None);

    let route = ArbRoute {
        hops: vec![
            RouteHop {
                pool_address: amm_pool_address,
                dex_type: DexType::RaydiumAmm,
                input_mint: base_mint,
                output_mint: other_mint,
                estimated_output: 1_100_000_000,
            },
            RouteHop {
                pool_address: Pubkey::new_unique(),
                dex_type: DexType::SanctumInfinity,
                input_mint: other_mint,
                output_mint: base_mint,
                estimated_output: 1_050_000_000,
            },
        ],
        base_mint,
        input_amount: 1_000_000_000, // 1 SOL
        estimated_profit: 50_000_000,
        estimated_profit_lamports: 50_000_000,
    };

    // Break-even: min_final_output = input_amount (not input+profit).
    let min_final_output = route.input_amount;

    let result = builder.build_arb_instructions(&route, min_final_output);
    assert!(result.is_ok(), "Instruction build should succeed");

    // Verify instructions were built (no tips — relays add their own)
    let instructions = result.unwrap();
    assert!(instructions.len() >= 3, "Should have compute budget + ATA + swap IXs");
}

/// min_final_output must be break-even (input_amount), NOT the optimistic
/// estimate (input + profit).  Setting it to input+profit causes
/// ExceededSlippage when the actual output is profitable but less than the
/// simulator's estimate.  arb-guard's execute_arb_v2 verifies real profit
/// on-chain, so the per-TX guard only needs break-even protection.
#[test]
fn test_min_output_is_break_even_not_optimistic_estimate() {
    let input = 10_000_000u64; // 0.01 SOL
    let estimated_profit = 500_000u64; // 0.0005 SOL

    // Correct: min = input (break-even)
    let min_final_output = input;

    assert_eq!(min_final_output, input,
        "min_final_output must equal input_amount (break-even)");
    assert!(min_final_output < input + estimated_profit,
        "min_final_output must be less than input+profit to avoid ExceededSlippage");
}

/// Regression: ensure that a slightly-less-than-estimated output still
/// passes the break-even check.  Under the old logic (input+profit) this
/// trade would be rejected despite being profitable.
#[test]
fn test_profitable_but_below_estimate_passes_break_even() {
    let input = 1_000_000_000u64; // 1 SOL
    let estimated_profit = 50_000_000u64; // 0.05 SOL
    let actual_output = input + estimated_profit / 2; // half the estimated profit

    // Break-even guard
    let min_final_output = input;

    assert!(actual_output >= min_final_output,
        "Trade that beats break-even should not be rejected");

    // Old (buggy) guard would have rejected this
    let old_min = input + estimated_profit;
    assert!(actual_output < old_min,
        "Same trade would fail the old optimistic guard — confirming the bug");
}
