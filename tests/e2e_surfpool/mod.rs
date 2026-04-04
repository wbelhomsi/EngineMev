mod harness;
mod common;
mod dex_swaps;
mod pipeline;

#[test]
fn test_surfpool_starts() {
    let harness = harness::SurfpoolHarness::start();
    assert!(harness.is_ready(), "Surfpool should be running");
}
