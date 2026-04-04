mod harness;
mod common;
mod dex_swaps;
mod pipeline;
mod arb_guard_cpi;

#[test]
fn test_surfpool_starts() {
    let harness = harness::SurfpoolHarness::start();
    assert!(harness.is_ready(), "Surfpool should be running");
}
