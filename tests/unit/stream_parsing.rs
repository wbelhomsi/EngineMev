use solana_sdk::pubkey::Pubkey;
use solana_mev_bot::router::pool::DexType;
use solana_mev_bot::mempool::stream::{
    parse_orca_whirlpool, parse_raydium_clmm, parse_meteora_dlmm,
    parse_meteora_damm_v2, parse_raydium_amm_v4, parse_raydium_cp,
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
    assert!(parse_orca_whirlpool(&Pubkey::new_unique(), &vec![0u8; 100], 100).is_none());
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
    let data = make_dlmm_data(&mx, &my, &vx, &vy, 8388608, 10);
    let result = parse_meteora_dlmm(&addr, &data, 100);
    assert!(result.is_some());
    let pool = result.unwrap();
    assert_eq!(pool.dex_type, DexType::MeteoraDlmm);
    assert_eq!(pool.token_a_mint, mx);
    assert_eq!(pool.token_b_mint, my);
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
