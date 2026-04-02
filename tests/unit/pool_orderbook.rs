use solana_sdk::pubkey::Pubkey;
use solana_mev_bot::router::pool::{DexType, PoolState, PoolExtra};

fn make_orderbook_pool(
    dex_type: DexType,
    best_bid_price: u128,
    best_ask_price: u128,
    bid_depth: u64,
    ask_depth: u64,
    fee_bps: u64,
) -> PoolState {
    PoolState {
        address: Pubkey::new_unique(),
        dex_type,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: bid_depth,
        token_b_reserve: ask_depth,
        fee_bps,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
        best_bid_price: Some(best_bid_price),
        best_ask_price: Some(best_ask_price),
    }
}

#[test]
fn test_orderbook_output_a_to_b_sell_into_bids() {
    let pool = make_orderbook_pool(DexType::Manifest, 150, 160, 1000, 1000, 0);
    let output = pool.get_output_amount(100, true);
    assert_eq!(output, Some(15000));
}

#[test]
fn test_orderbook_output_b_to_a_buy_from_asks() {
    let pool = make_orderbook_pool(DexType::Manifest, 150, 160, 1000, 1000, 0);
    let output = pool.get_output_amount(1600, false);
    assert_eq!(output, Some(10));
}

#[test]
fn test_orderbook_output_with_phoenix_taker_fee() {
    let pool = make_orderbook_pool(DexType::Phoenix, 200, 210, 5000, 5000, 2);
    let output = pool.get_output_amount(1000, true);
    assert_eq!(output, Some(199800));
}

#[test]
fn test_orderbook_output_capped_by_depth() {
    let pool = make_orderbook_pool(DexType::Manifest, 150, 160, 50, 1000, 0);
    let output = pool.get_output_amount(100, true);
    assert_eq!(output, Some(7500));
}

#[test]
fn test_orderbook_output_zero_input() {
    let pool = make_orderbook_pool(DexType::Manifest, 150, 160, 1000, 1000, 0);
    assert_eq!(pool.get_output_amount(0, true), Some(0));
}

#[test]
fn test_orderbook_no_bid_ask_falls_through_to_none() {
    let mut pool = make_orderbook_pool(DexType::Phoenix, 0, 0, 0, 0, 2);
    pool.best_bid_price = None;
    pool.best_ask_price = None;
    assert_eq!(pool.get_output_amount(100, true), None);
}
