mod harness;
// mod common;  // uncomment when common.rs exists
// mod dex_swaps;
// mod pipeline;

#[test]
fn test_surfpool_starts() {
    let harness = harness::SurfpoolHarness::start();
    assert!(harness.is_ready(), "Surfpool should be running");
}
