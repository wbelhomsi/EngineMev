//! Tests for adaptive tip floor integration.

use std::time::{Duration, Instant};

#[test]
fn test_tip_floor_cache_callable() {
    let cache = solana_mev_bot::state::TipFloorCache::new();
    // Before any update, returns None
    assert!(cache.get_floor_lamports().is_none());
    assert!(cache.get_competitive_floor_lamports().is_none());
}

#[test]
fn test_tip_floor_overrides_static_minimum() {
    // Simulate: static min_tip = 50_000, dynamic floor = 100_000
    // The effective tip should be max(static, dynamic) = 100_000
    let cache = solana_mev_bot::state::TipFloorCache::new();
    cache.update(solana_mev_bot::state::tip_floor::TipFloorInfo {
        p50_lamports: 80_000,
        p75_lamports: 200_000,
        ema_p50_lamports: 100_000,
        fetched_at: Instant::now(),
    });

    let static_min: u64 = 50_000;
    let dynamic_floor = cache.get_floor_lamports().unwrap_or(0);
    let effective = static_min.max(dynamic_floor);
    assert_eq!(effective, 100_000);
}

#[test]
fn test_tip_floor_falls_back_to_static_when_stale() {
    let cache = solana_mev_bot::state::TipFloorCache::new();
    cache.update(solana_mev_bot::state::tip_floor::TipFloorInfo {
        p50_lamports: 80_000,
        p75_lamports: 200_000,
        ema_p50_lamports: 100_000,
        fetched_at: Instant::now() - Duration::from_secs(60), // stale
    });

    let static_min: u64 = 50_000;
    let dynamic_floor = cache.get_floor_lamports().unwrap_or(0);
    let effective = static_min.max(dynamic_floor);
    // Dynamic is stale → falls back to 0, so effective = static
    assert_eq!(effective, 50_000);
}

#[test]
fn test_tip_fraction_with_dynamic_floor() {
    // Simulate the tip calculation from the simulator:
    // gross_profit = 1_000_000, tip_fraction = 0.50, dynamic floor = 600_000
    // fraction_tip = 500_000, but floor = 600_000, so tip = 600_000
    let gross_profit: u64 = 1_000_000;
    let tip_fraction: f64 = 0.50;
    let static_min: u64 = 50_000;
    let dynamic_floor: u64 = 600_000;

    let fraction_tip = (gross_profit as f64 * tip_fraction) as u64;
    let effective_min = static_min.max(dynamic_floor);
    let tip = fraction_tip.max(effective_min);

    assert_eq!(fraction_tip, 500_000);
    assert_eq!(tip, 600_000); // Dynamic floor wins
}
