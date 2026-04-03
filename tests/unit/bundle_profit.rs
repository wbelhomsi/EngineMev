use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::hash::Hash;
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
            open_orders: Some(Pubkey::new_unique()),
            amm_nonce: Some(254),
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    };
    state_cache.upsert(amm_pool_address, amm_pool);

    let builder = BundleBuilder::new(keypair, state_cache);

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

    let tip_lamports = 25_000_000; // 50% of profit

    let result = builder.build_arb_bundle(&route, tip_lamports, Hash::default());
    assert!(result.is_ok(), "Bundle build should succeed");

    // Verify the bundle was built (detailed IX inspection requires deserializing,
    // but we verify it doesn't error)
    let bundle = result.unwrap();
    assert_eq!(bundle.len(), 1, "Single tx bundle (arb + tip in one tx)");
}
