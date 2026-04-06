use solana_sdk::pubkey::Pubkey;
use solana_mev_bot::router::pool::{ArbRoute, DexType, RouteHop};

/// Mirrors the can_submit_route() logic from main.rs.
/// We test this independently since can_submit_route is private.
fn can_submit_route(route: &ArbRoute) -> bool {
    route.hops.iter().all(|hop| matches!(
        hop.dex_type,
        DexType::RaydiumAmm
        | DexType::RaydiumCp
        | DexType::RaydiumClmm
        | DexType::OrcaWhirlpool
        | DexType::MeteoraDlmm
        | DexType::MeteoraDammV2
        | DexType::SanctumInfinity
        | DexType::Phoenix
        | DexType::Manifest
        | DexType::PumpSwap
    ))
}

fn make_hop(dex_type: DexType) -> RouteHop {
    RouteHop {
        pool_address: Pubkey::new_unique(),
        dex_type,
        input_mint: Pubkey::new_unique(),
        output_mint: Pubkey::new_unique(),
        estimated_output: 1000,
    }
}

fn make_route(hops: Vec<RouteHop>) -> ArbRoute {
    ArbRoute {
        hops,
        base_mint: Pubkey::new_unique(),
        input_amount: 1_000_000,
        estimated_profit: 1000,
        estimated_profit_lamports: 1000,
    }
}

#[test]
fn test_phoenix_route_accepted() {
    let route = make_route(vec![make_hop(DexType::Phoenix), make_hop(DexType::OrcaWhirlpool)]);
    assert!(can_submit_route(&route));
}

#[test]
fn test_manifest_route_accepted() {
    let route = make_route(vec![make_hop(DexType::Manifest), make_hop(DexType::RaydiumCp)]);
    assert!(can_submit_route(&route));
}

#[test]
fn test_sanctum_route_accepted() {
    let route = make_route(vec![make_hop(DexType::OrcaWhirlpool), make_hop(DexType::SanctumInfinity)]);
    assert!(can_submit_route(&route));
}

#[test]
fn test_all_submittable_types_accepted() {
    for dex in [
        DexType::RaydiumAmm, DexType::RaydiumCp, DexType::RaydiumClmm,
        DexType::OrcaWhirlpool, DexType::MeteoraDlmm, DexType::MeteoraDammV2,
        DexType::SanctumInfinity, DexType::Phoenix, DexType::Manifest,
        DexType::PumpSwap,
    ] {
        let route = make_route(vec![make_hop(dex)]);
        assert!(can_submit_route(&route), "Expected {:?} to be accepted", dex);
    }
}

#[test]
fn test_raydium_amm_accepted() {
    let route = make_route(vec![make_hop(DexType::RaydiumAmm), make_hop(DexType::OrcaWhirlpool)]);
    assert!(can_submit_route(&route));
}

#[test]
fn test_mixed_raydium_amm_phoenix_accepted() {
    let route = make_route(vec![make_hop(DexType::Phoenix), make_hop(DexType::RaydiumAmm)]);
    assert!(can_submit_route(&route));
}

#[test]
fn test_pumpswap_route_accepted() {
    let route = make_route(vec![make_hop(DexType::PumpSwap), make_hop(DexType::OrcaWhirlpool)]);
    assert!(can_submit_route(&route));
}
