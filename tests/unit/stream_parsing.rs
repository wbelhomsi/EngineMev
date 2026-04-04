use solana_sdk::pubkey::Pubkey;
use solana_mev_bot::router::pool::DexType;
use solana_mev_bot::mempool::stream::{
    parse_orca_whirlpool, parse_raydium_clmm, parse_meteora_dlmm,
    parse_meteora_damm_v2, parse_raydium_amm_v4, parse_raydium_cp,
    parse_phoenix_market, parse_manifest_market,
};

fn make_whirlpool_data(mint_a: &Pubkey, vault_a: &Pubkey, mint_b: &Pubkey, vault_b: &Pubkey, sqrt_price: u128, tick: i32, liquidity: u128) -> Vec<u8> {
    let mut data = vec![0u8; 653];
    data[49..65].copy_from_slice(&liquidity.to_le_bytes());
    data[65..81].copy_from_slice(&sqrt_price.to_le_bytes());
    data[81..85].copy_from_slice(&tick.to_le_bytes());
    data[101..133].copy_from_slice(mint_a.as_ref());
    data[133..165].copy_from_slice(vault_a.as_ref());
    data[181..213].copy_from_slice(mint_b.as_ref());
    data[213..245].copy_from_slice(vault_b.as_ref());
    data
}

#[test]
fn test_parse_orca_whirlpool() {
    let addr = Pubkey::new_unique();
    let mint_a = Pubkey::new_unique();
    let vault_a = Pubkey::new_unique();
    let mint_b = Pubkey::new_unique();
    let vault_b = Pubkey::new_unique();
    let data = make_whirlpool_data(&mint_a, &vault_a, &mint_b, &vault_b, 1_000_000u128, -50, 500_000u128);
    let result = parse_orca_whirlpool(&addr, &data, 100);
    assert!(result.is_some());
    let pool = result.unwrap();
    assert_eq!(pool.dex_type, DexType::OrcaWhirlpool);
    assert_eq!(pool.token_a_mint, mint_a);
    assert_eq!(pool.token_b_mint, mint_b);
    assert_eq!(pool.sqrt_price_x64, Some(1_000_000));
    assert_eq!(pool.current_tick, Some(-50));
    assert_eq!(pool.liquidity, Some(500_000));
}

#[test]
fn test_parse_orca_rejects_short_data() {
    assert!(parse_orca_whirlpool(&Pubkey::new_unique(), &[0u8; 100], 100).is_none());
}

fn make_raydium_clmm_data(mint_0: &Pubkey, mint_1: &Pubkey, vault_0: &Pubkey, vault_1: &Pubkey, sqrt_price: u128, tick: i32, liquidity: u128) -> Vec<u8> {
    let mut data = vec![0u8; 1560];
    data[73..105].copy_from_slice(mint_0.as_ref());
    data[105..137].copy_from_slice(mint_1.as_ref());
    data[137..169].copy_from_slice(vault_0.as_ref());
    data[169..201].copy_from_slice(vault_1.as_ref());
    data[237..253].copy_from_slice(&liquidity.to_le_bytes());
    data[253..269].copy_from_slice(&sqrt_price.to_le_bytes());
    data[269..273].copy_from_slice(&tick.to_le_bytes());
    data
}

#[test]
fn test_parse_raydium_clmm() {
    let addr = Pubkey::new_unique();
    let m0 = Pubkey::new_unique(); let m1 = Pubkey::new_unique();
    let v0 = Pubkey::new_unique(); let v1 = Pubkey::new_unique();
    let data = make_raydium_clmm_data(&m0, &m1, &v0, &v1, 2_000_000u128, 100, 800_000u128);
    let result = parse_raydium_clmm(&addr, &data, 100);
    assert!(result.is_some());
    let pool = result.unwrap();
    assert_eq!(pool.dex_type, DexType::RaydiumClmm);
    assert_eq!(pool.token_a_mint, m0);
    assert_eq!(pool.token_b_mint, m1);
    assert_eq!(pool.sqrt_price_x64, Some(2_000_000));
    assert_eq!(pool.current_tick, Some(100));
}

fn make_dlmm_data(mint_x: &Pubkey, mint_y: &Pubkey, vault_x: &Pubkey, vault_y: &Pubkey, active_id: i32, bin_step: u16) -> Vec<u8> {
    let mut data = vec![0u8; 904];
    data[76..80].copy_from_slice(&active_id.to_le_bytes());
    data[80..82].copy_from_slice(&bin_step.to_le_bytes());
    data[88..120].copy_from_slice(mint_x.as_ref());
    data[120..152].copy_from_slice(mint_y.as_ref());
    data[152..184].copy_from_slice(vault_x.as_ref());
    data[184..216].copy_from_slice(vault_y.as_ref());
    data
}

#[test]
fn test_parse_meteora_dlmm() {
    let addr = Pubkey::new_unique();
    let mx = Pubkey::new_unique(); let my = Pubkey::new_unique();
    let vx = Pubkey::new_unique(); let vy = Pubkey::new_unique();
    // Use a realistic active_id (max valid is ~443636)
    let data = make_dlmm_data(&mx, &my, &vx, &vy, 5000, 10);
    let result = parse_meteora_dlmm(&addr, &data, 100);
    assert!(result.is_some());
    let pool = result.unwrap();
    assert_eq!(pool.dex_type, DexType::MeteoraDlmm);
    assert_eq!(pool.token_a_mint, mx);
    assert_eq!(pool.token_b_mint, my);
}

#[test]
fn test_parse_meteora_dlmm_rejects_garbage_active_id() {
    let addr = Pubkey::new_unique();
    let mx = Pubkey::new_unique(); let my = Pubkey::new_unique();
    let vx = Pubkey::new_unique(); let vy = Pubkey::new_unique();
    // active_id 8388608 is garbage (exceeds max ~443636)
    let data = make_dlmm_data(&mx, &my, &vx, &vy, 8388608, 10);
    let result = parse_meteora_dlmm(&addr, &data, 100);
    assert!(result.is_none(), "Should reject garbage active_id > 500_000");
}

fn make_damm_v2_data(mint_a: &Pubkey, mint_b: &Pubkey, vault_a: &Pubkey, vault_b: &Pubkey, reserve_a: u64, reserve_b: u64, collect_fee_mode: u8) -> Vec<u8> {
    let mut data = vec![0u8; 1112];
    data[0..8].copy_from_slice(&[241, 154, 109, 4, 17, 177, 109, 188]);
    data[168..200].copy_from_slice(mint_a.as_ref());
    data[200..232].copy_from_slice(mint_b.as_ref());
    data[232..264].copy_from_slice(vault_a.as_ref());
    data[264..296].copy_from_slice(vault_b.as_ref());
    data[484] = collect_fee_mode;
    data[680..688].copy_from_slice(&reserve_a.to_le_bytes());
    data[688..696].copy_from_slice(&reserve_b.to_le_bytes());
    data
}

#[test]
fn test_parse_damm_v2_compounding() {
    let addr = Pubkey::new_unique();
    let ma = Pubkey::new_unique(); let mb = Pubkey::new_unique();
    let va = Pubkey::new_unique(); let vb = Pubkey::new_unique();
    let data = make_damm_v2_data(&ma, &mb, &va, &vb, 5_000_000_000, 10_000_000_000, 4);
    let result = parse_meteora_damm_v2(&addr, &data, 100);
    assert!(result.is_some());
    let pool = result.unwrap();
    assert_eq!(pool.dex_type, DexType::MeteoraDammV2);
    assert_eq!(pool.token_a_reserve, 5_000_000_000);
    assert_eq!(pool.token_b_reserve, 10_000_000_000);
}

fn make_raydium_amm_data(base_vault: &Pubkey, quote_vault: &Pubkey, base_mint: &Pubkey, quote_mint: &Pubkey) -> Vec<u8> {
    let mut data = vec![0u8; 752];
    data[0..8].copy_from_slice(&6u64.to_le_bytes());
    // nonce at offset 8 (low byte of u64)
    data[8] = 253;
    // trade_fee_numerator at offset 144
    data[144..152].copy_from_slice(&25u64.to_le_bytes());
    // trade_fee_denominator at offset 152
    data[152..160].copy_from_slice(&10000u64.to_le_bytes());
    data[336..368].copy_from_slice(base_vault.as_ref());
    data[368..400].copy_from_slice(quote_vault.as_ref());
    data[400..432].copy_from_slice(base_mint.as_ref());
    data[432..464].copy_from_slice(quote_mint.as_ref());
    data
}

#[test]
fn test_parse_raydium_amm_v4() {
    let addr = Pubkey::new_unique();
    let bv = Pubkey::new_unique(); let qv = Pubkey::new_unique();
    let bm = Pubkey::new_unique(); let qm = Pubkey::new_unique();
    let data = make_raydium_amm_data(&bv, &qv, &bm, &qm);
    let result = parse_raydium_amm_v4(&addr, &data, 100);
    assert!(result.is_some());
    let (pool, vaults) = result.unwrap();
    assert_eq!(pool.dex_type, DexType::RaydiumAmm);
    assert_eq!(pool.token_a_mint, bm);
    assert_eq!(pool.token_b_mint, qm);
    assert_eq!(vaults, (bv, qv));
    assert_eq!(pool.fee_bps, 25, "Fee should be parsed from pool state");
    assert_eq!(pool.extra.amm_nonce, Some(253));
    // open_orders/market/market_program/target_orders are zeroed Pubkeys in test data
    assert!(pool.extra.open_orders.is_some());
    assert!(pool.extra.market.is_some());
    assert!(pool.extra.market_program.is_some());
    assert!(pool.extra.target_orders.is_some());
}

fn make_raydium_cp_data(vault_0: &Pubkey, vault_1: &Pubkey, mint_0: &Pubkey, mint_1: &Pubkey) -> Vec<u8> {
    let mut data = vec![0u8; 637];
    data[0..8].copy_from_slice(&[247, 237, 227, 245, 215, 195, 222, 70]);
    data[72..104].copy_from_slice(vault_0.as_ref());
    data[104..136].copy_from_slice(vault_1.as_ref());
    data[168..200].copy_from_slice(mint_0.as_ref());
    data[200..232].copy_from_slice(mint_1.as_ref());
    data
}

#[test]
fn test_phoenix_and_manifest_dex_types_exist() {
    assert_eq!(DexType::Phoenix.base_fee_bps(), 2);
    assert_eq!(DexType::Manifest.base_fee_bps(), 0);
}

#[test]
fn test_parse_raydium_cp() {
    let addr = Pubkey::new_unique();
    let v0 = Pubkey::new_unique(); let v1 = Pubkey::new_unique();
    let m0 = Pubkey::new_unique(); let m1 = Pubkey::new_unique();
    let data = make_raydium_cp_data(&v0, &v1, &m0, &m1);
    let result = parse_raydium_cp(&addr, &data, 100);
    assert!(result.is_some());
    let (pool, vaults) = result.unwrap();
    assert_eq!(pool.dex_type, DexType::RaydiumCp);
    assert_eq!(pool.token_a_mint, m0);
    assert_eq!(pool.token_b_mint, m1);
    assert_eq!(vaults, (v0, v1));
}

// ─── Phoenix market parser tests ─────────────────────────────────────────────

#[test]
fn test_parse_phoenix_market_too_short() {
    let data = vec![0u8; 623];
    assert!(parse_phoenix_market(&Pubkey::new_unique(), &data, 100).is_none());
}

#[test]
fn test_parse_phoenix_market_extracts_mints() {
    let base_mint = Pubkey::new_unique();
    let quote_mint = Pubkey::new_unique();
    let base_vault = Pubkey::new_unique();
    let quote_vault = Pubkey::new_unique();

    let mut data = vec![0u8; 700];
    data[48..80].copy_from_slice(base_mint.as_ref());
    data[80..112].copy_from_slice(base_vault.as_ref());
    data[136..144].copy_from_slice(&1u64.to_le_bytes()); // base_lot_size
    data[152..184].copy_from_slice(quote_mint.as_ref());
    data[184..216].copy_from_slice(quote_vault.as_ref());
    data[240..248].copy_from_slice(&1u64.to_le_bytes()); // quote_lot_size

    let result = parse_phoenix_market(&Pubkey::new_unique(), &data, 100);
    assert!(result.is_some());
    let pool = result.unwrap();
    assert_eq!(pool.dex_type, DexType::Phoenix);
    assert_eq!(pool.token_a_mint, base_mint);
    assert_eq!(pool.token_b_mint, quote_mint);
    assert_eq!(pool.extra.vault_a, Some(base_vault));
    assert_eq!(pool.extra.vault_b, Some(quote_vault));
    // Data is only 700 bytes — not enough for FIFOMarket taker_fee_bps at 624+288=912
    // so fee falls back to default
    assert_eq!(pool.fee_bps, DexType::Phoenix.base_fee_bps());
    // No orderbook data → empty trees → None pricing
    assert_eq!(pool.best_bid_price, None);
    assert_eq!(pool.best_ask_price, None);
}

#[test]
fn test_parse_phoenix_rejects_zero_lot_size() {
    let mut data = vec![0u8; 700];
    data[48..80].copy_from_slice(Pubkey::new_unique().as_ref());
    data[152..184].copy_from_slice(Pubkey::new_unique().as_ref());
    // base_lot_size = 0 (default) → should reject
    assert!(parse_phoenix_market(&Pubkey::new_unique(), &data, 100).is_none());
}

// ─── Manifest market parser tests ────────────────────────────────────────────

#[test]
fn test_parse_manifest_market_too_short() {
    let data = vec![0u8; 255];
    assert!(parse_manifest_market(&Pubkey::new_unique(), &data, 100).is_none());
}

#[test]
fn test_parse_manifest_market_extracts_mints() {
    let base_mint = Pubkey::new_unique();
    let quote_mint = Pubkey::new_unique();
    let base_vault = Pubkey::new_unique();
    let quote_vault = Pubkey::new_unique();

    let mut data = vec![0u8; 300];
    data[16..48].copy_from_slice(base_mint.as_ref());
    data[48..80].copy_from_slice(quote_mint.as_ref());
    data[80..112].copy_from_slice(base_vault.as_ref());
    data[112..144].copy_from_slice(quote_vault.as_ref());

    let result = parse_manifest_market(&Pubkey::new_unique(), &data, 100);
    assert!(result.is_some());
    let pool = result.unwrap();
    assert_eq!(pool.dex_type, DexType::Manifest);
    assert_eq!(pool.token_a_mint, base_mint);
    assert_eq!(pool.token_b_mint, quote_mint);
    assert_eq!(pool.extra.vault_a, Some(base_vault));
    assert_eq!(pool.extra.vault_b, Some(quote_vault));
    assert_eq!(pool.fee_bps, 0);
}

#[test]
fn test_parse_manifest_rejects_zero_mints() {
    let data = vec![0u8; 300];
    assert!(parse_manifest_market(&Pubkey::new_unique(), &data, 100).is_none());
}

// ─── Phoenix top-of-book extraction tests ───────────────────────────────────

#[test]
fn test_parse_phoenix_market_with_orderbook() {
    let base_mint = Pubkey::new_unique();
    let quote_mint = Pubkey::new_unique();

    // Layout sizes:
    // Header: 624 bytes
    // FIFOMarket padding+fields: 304 bytes (up to bids tree start)
    // Bids tree: 32 (header) + bids_size*64 (nodes)
    // Asks tree: 32 (header) + asks_size*64 (nodes)
    let bids_size: u64 = 1;
    let asks_size: u64 = 1;
    // bids tree at 928, size = 32 + 64 = 96 → asks at 1024, size = 96
    // Total: 1024 + 96 = 1120, plus some padding
    let mut data = vec![0u8; 1200];

    // MarketHeader fields
    data[16..24].copy_from_slice(&bids_size.to_le_bytes());
    data[24..32].copy_from_slice(&asks_size.to_le_bytes());
    data[48..80].copy_from_slice(base_mint.as_ref());
    data[80..112].copy_from_slice(Pubkey::new_unique().as_ref()); // base_vault
    data[136..144].copy_from_slice(&1u64.to_le_bytes()); // base_lot_size = 1
    data[152..184].copy_from_slice(quote_mint.as_ref());
    data[184..216].copy_from_slice(Pubkey::new_unique().as_ref()); // quote_vault
    data[240..248].copy_from_slice(&1u64.to_le_bytes()); // quote_lot_size = 1
    data[248..256].copy_from_slice(&100u64.to_le_bytes()); // tick_size = 100

    // FIFOMarket taker_fee_bps at 624+280 = 904
    data[904..912].copy_from_slice(&3u64.to_le_bytes()); // 3 bps taker fee

    // Bids tree at offset 928:
    // root = 1 (first node, 1-based)
    data[928..932].copy_from_slice(&1u32.to_le_bytes());
    // Node 0 at offset 928 + 32 = 960:
    // left=0 (SENTINEL), right=0 (leaf)
    data[960..964].copy_from_slice(&0u32.to_le_bytes()); // left
    data[964..968].copy_from_slice(&0u32.to_le_bytes()); // right
    // price_in_ticks at +16 = 976
    data[976..984].copy_from_slice(&50u64.to_le_bytes());
    // num_base_lots at +40 = 1000
    data[1000..1008].copy_from_slice(&200u64.to_le_bytes());

    // Asks tree at offset 928 + 32 + 1*64 = 1024:
    data[1024..1028].copy_from_slice(&1u32.to_le_bytes()); // root = 1
    // Node 0 at 1024 + 32 = 1056:
    data[1056..1060].copy_from_slice(&0u32.to_le_bytes()); // left
    data[1060..1064].copy_from_slice(&0u32.to_le_bytes()); // right
    // price_in_ticks at +16 = 1072
    data[1072..1080].copy_from_slice(&55u64.to_le_bytes());
    // num_base_lots at +40 = 1096
    data[1096..1104].copy_from_slice(&150u64.to_le_bytes());

    let result = parse_phoenix_market(&Pubkey::new_unique(), &data, 100);
    assert!(result.is_some());
    let pool = result.unwrap();
    assert_eq!(pool.dex_type, DexType::Phoenix);
    // best_bid = 50 ticks * 100 tick_size = 5000
    assert_eq!(pool.best_bid_price, Some(5000));
    // best_ask = 55 ticks * 100 tick_size = 5500
    assert_eq!(pool.best_ask_price, Some(5500));
    // bid depth = 200 lots * 1 base_lot_size = 200
    assert_eq!(pool.token_a_reserve, 200);
    // ask depth = 150 lots * 1 base_lot_size = 150
    assert_eq!(pool.token_b_reserve, 150);
    // taker fee read from data
    assert_eq!(pool.fee_bps, 3);
}

#[test]
fn test_parse_phoenix_empty_trees() {
    let base_mint = Pubkey::new_unique();
    let quote_mint = Pubkey::new_unique();
    let bids_size: u64 = 1;
    let asks_size: u64 = 1;
    let mut data = vec![0u8; 1200];

    data[16..24].copy_from_slice(&bids_size.to_le_bytes());
    data[24..32].copy_from_slice(&asks_size.to_le_bytes());
    data[48..80].copy_from_slice(base_mint.as_ref());
    data[80..112].copy_from_slice(Pubkey::new_unique().as_ref());
    data[136..144].copy_from_slice(&1u64.to_le_bytes());
    data[152..184].copy_from_slice(quote_mint.as_ref());
    data[184..216].copy_from_slice(Pubkey::new_unique().as_ref());
    data[240..248].copy_from_slice(&1u64.to_le_bytes());
    data[248..256].copy_from_slice(&100u64.to_le_bytes());

    // Trees have root=0 (SENTINEL) → empty
    // (data is zeroed, so root is already 0)

    let result = parse_phoenix_market(&Pubkey::new_unique(), &data, 100);
    assert!(result.is_some());
    let pool = result.unwrap();
    assert_eq!(pool.best_bid_price, None);
    assert_eq!(pool.best_ask_price, None);
    assert_eq!(pool.token_a_reserve, 0);
    assert_eq!(pool.token_b_reserve, 0);
}

// ─── Manifest top-of-book extraction tests ──────────────────────────────────

#[test]
fn test_parse_manifest_market_with_orderbook() {
    let base_mint = Pubkey::new_unique();
    let quote_mint = Pubkey::new_unique();

    // Header (256) + dynamic data for two nodes (each 80 bytes)
    let mut data = vec![0u8; 500];
    data[16..48].copy_from_slice(base_mint.as_ref());
    data[48..80].copy_from_slice(quote_mint.as_ref());
    data[80..112].copy_from_slice(Pubkey::new_unique().as_ref()); // base_vault
    data[112..144].copy_from_slice(Pubkey::new_unique().as_ref()); // quote_vault

    // bids_best_index = 0 (byte offset 0 in dynamic section → absolute 256)
    data[160..164].copy_from_slice(&0u32.to_le_bytes());
    // asks_best_index = 80 (second node, byte offset 80 → absolute 336)
    data[168..172].copy_from_slice(&80u32.to_le_bytes());

    // Best bid node at absolute offset 256:
    // price (u128 D18) at +16 = offset 272
    let bid_price: u128 = 150_000_000_000_000_000_000; // 150 in D18
    data[272..288].copy_from_slice(&bid_price.to_le_bytes());
    // num_base_atoms at +32 = offset 288
    data[288..296].copy_from_slice(&1000u64.to_le_bytes());

    // Best ask node at absolute offset 336:
    // price at +16 = 352
    let ask_price: u128 = 160_000_000_000_000_000_000; // 160 in D18
    data[352..368].copy_from_slice(&ask_price.to_le_bytes());
    // num_base_atoms at +32 = 368
    data[368..376].copy_from_slice(&500u64.to_le_bytes());

    let result = parse_manifest_market(&Pubkey::new_unique(), &data, 100);
    assert!(result.is_some());
    let pool = result.unwrap();
    assert_eq!(pool.dex_type, DexType::Manifest);
    assert_eq!(pool.best_bid_price, Some(bid_price));
    assert_eq!(pool.best_ask_price, Some(ask_price));
    assert_eq!(pool.token_a_reserve, 1000);
    assert_eq!(pool.token_b_reserve, 500);
}

#[test]
fn test_parse_manifest_empty_book() {
    let base_mint = Pubkey::new_unique();
    let quote_mint = Pubkey::new_unique();
    let mut data = vec![0u8; 300];
    data[16..48].copy_from_slice(base_mint.as_ref());
    data[48..80].copy_from_slice(quote_mint.as_ref());
    data[80..112].copy_from_slice(Pubkey::new_unique().as_ref());
    data[112..144].copy_from_slice(Pubkey::new_unique().as_ref());

    // Set both indices to u32::MAX (sentinel for empty)
    data[160..164].copy_from_slice(&u32::MAX.to_le_bytes());
    data[168..172].copy_from_slice(&u32::MAX.to_le_bytes());

    let result = parse_manifest_market(&Pubkey::new_unique(), &data, 100);
    assert!(result.is_some());
    let pool = result.unwrap();
    assert_eq!(pool.best_bid_price, None);
    assert_eq!(pool.best_ask_price, None);
    assert_eq!(pool.token_a_reserve, 0);
    assert_eq!(pool.token_b_reserve, 0);
}
