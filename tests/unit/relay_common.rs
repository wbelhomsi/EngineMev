use solana_mev_bot::executor::relays::common;
use solana_mev_bot::executor::relays::RelayResult;
use std::time::Duration;

// ---------------------------------------------------------------------------
// RateLimiter
// ---------------------------------------------------------------------------

#[test]
fn test_rate_limiter_first_call_passes() {
    let rl = common::RateLimiter::new(Duration::from_secs(1));
    assert!(rl.check("test-relay").is_ok());
}

#[test]
fn test_rate_limiter_rapid_calls_blocked() {
    let rl = common::RateLimiter::new(Duration::from_secs(10));
    // First call succeeds and records the timestamp.
    assert!(rl.check("test-relay").is_ok());
    // Immediate second call must be rate-limited.
    let err = rl.check("test-relay").unwrap_err();
    assert!(!err.success);
    assert_eq!(err.error.as_deref(), Some("Rate limited"));
    assert_eq!(err.relay_name, "test-relay");
}

#[test]
fn test_rate_limiter_after_interval_passes() {
    let rl = common::RateLimiter::new(Duration::from_millis(50));
    assert!(rl.check("test-relay").is_ok());
    std::thread::sleep(Duration::from_millis(60));
    assert!(rl.check("test-relay").is_ok());
}

#[test]
fn test_rate_limiter_zero_interval_always_passes() {
    let rl = common::RateLimiter::new(Duration::ZERO);
    assert!(rl.check("test-relay").is_ok());
    assert!(rl.check("test-relay").is_ok());
    assert!(rl.check("test-relay").is_ok());
}

// ---------------------------------------------------------------------------
// interval_from_tps
// ---------------------------------------------------------------------------

#[test]
fn test_interval_from_tps_normal() {
    let d = common::interval_from_tps(5.0);
    // 1000/5 = 200ms + 10ms padding = 210ms
    assert_eq!(d.as_millis(), 210);
}

#[test]
fn test_interval_from_tps_one() {
    let d = common::interval_from_tps(1.0);
    // 1000/1 = 1000 + 10 = 1010ms
    assert_eq!(d.as_millis(), 1010);
}

#[test]
fn test_interval_from_zero_tps() {
    let d = common::interval_from_tps(0.0);
    assert_eq!(d.as_millis(), 1000);
}

#[test]
fn test_interval_from_negative_tps() {
    let d = common::interval_from_tps(-1.0);
    assert_eq!(d.as_millis(), 1000);
}

// ---------------------------------------------------------------------------
// parse_jsonrpc_response
// ---------------------------------------------------------------------------

#[test]
fn test_parse_success_response() {
    let body: serde_json::Value =
        serde_json::json!({"jsonrpc": "2.0", "result": "bundle-id-123", "id": 1});
    let r = common::parse_jsonrpc_response("jito", &body, 5000);
    assert!(r.success);
    assert_eq!(r.bundle_id.as_deref(), Some("bundle-id-123"));
    assert_eq!(r.relay_name, "jito");
    assert_eq!(r.latency_us, 5000);
    assert!(r.error.is_none());
}

#[test]
fn test_parse_error_response() {
    let body: serde_json::Value = serde_json::json!({
        "jsonrpc": "2.0",
        "error": {"code": -32000, "message": "bundle simulation failed"},
        "id": 1
    });
    let r = common::parse_jsonrpc_response("nozomi", &body, 1234);
    assert!(!r.success);
    assert!(r.bundle_id.is_none());
    assert_eq!(r.relay_name, "nozomi");
    assert_eq!(r.latency_us, 1234);
    let err = r.error.as_ref().unwrap();
    assert!(err.contains("bundle simulation failed"));
}

#[test]
fn test_parse_unexpected_format() {
    let body: serde_json::Value = serde_json::json!({});
    let r = common::parse_jsonrpc_response("bloxroute", &body, 0);
    assert!(!r.success);
    assert!(r.bundle_id.is_none());
    assert_eq!(
        r.error.as_deref(),
        Some("Unexpected response format")
    );
}

#[test]
fn test_parse_result_non_string() {
    // "result" is a number, not a string — should fall through to error branch.
    let body: serde_json::Value = serde_json::json!({"result": 42});
    let r = common::parse_jsonrpc_response("test", &body, 0);
    assert!(!r.success);
    assert_eq!(
        r.error.as_deref(),
        Some("Unexpected response format")
    );
}

// ---------------------------------------------------------------------------
// fail / fail_with_latency
// ---------------------------------------------------------------------------

#[test]
fn test_fail_creates_failed_result() {
    let r = common::fail("test-relay", "some error".to_string());
    assert!(!r.success);
    assert_eq!(r.relay_name, "test-relay");
    assert_eq!(r.error.as_deref(), Some("some error"));
    assert!(r.bundle_id.is_none());
    assert_eq!(r.latency_us, 0);
}

#[test]
fn test_fail_with_latency() {
    let r = common::fail_with_latency("astralane", "timeout".to_string(), 9999);
    assert!(!r.success);
    assert_eq!(r.relay_name, "astralane");
    assert_eq!(r.error.as_deref(), Some("timeout"));
    assert_eq!(r.latency_us, 9999);
    assert!(r.bundle_id.is_none());
}

// ---------------------------------------------------------------------------
// record_relay_metrics (must not panic; counters are no-op without recorder)
// ---------------------------------------------------------------------------

#[test]
fn test_record_relay_metrics_success() {
    let r = RelayResult {
        relay_name: "jito".to_string(),
        success: true,
        bundle_id: Some("abc".to_string()),
        error: None,
        latency_us: 5000,
    };
    // Must not panic.
    common::record_relay_metrics(&r);
}

#[test]
fn test_record_relay_metrics_rate_limited() {
    let r = RelayResult {
        relay_name: "nozomi".to_string(),
        success: false,
        bundle_id: None,
        error: Some("Rate limited".to_string()),
        latency_us: 0,
    };
    common::record_relay_metrics(&r);
}

#[test]
fn test_record_relay_metrics_rejected() {
    let r = RelayResult {
        relay_name: "bloxroute".to_string(),
        success: false,
        bundle_id: None,
        error: Some("bundle simulation failed: some on-chain error".to_string()),
        latency_us: 3000,
    };
    common::record_relay_metrics(&r);
}

#[test]
fn test_record_relay_metrics_tx_too_large() {
    let r = RelayResult {
        relay_name: "astralane".to_string(),
        success: false,
        bundle_id: None,
        error: Some("Tx too large: 1300 bytes (limit 1232)".to_string()),
        latency_us: 100,
    };
    common::record_relay_metrics(&r);
}

#[test]
fn test_record_relay_metrics_network_error() {
    let r = RelayResult {
        relay_name: "zeroslot".to_string(),
        success: false,
        bundle_id: None,
        error: Some("Request failed: timeout".to_string()),
        latency_us: 5000,
    };
    common::record_relay_metrics(&r);
}

#[test]
fn test_record_relay_metrics_stale_blockhash() {
    let r = RelayResult {
        relay_name: "jito".to_string(),
        success: false,
        bundle_id: None,
        error: Some("blockhash not found".to_string()),
        latency_us: 200,
    };
    common::record_relay_metrics(&r);
}

#[test]
fn test_record_relay_metrics_build_error() {
    let r = RelayResult {
        relay_name: "jito".to_string(),
        success: false,
        bundle_id: None,
        error: Some("V0 sign error: invalid keypair".to_string()),
        latency_us: 50,
    };
    common::record_relay_metrics(&r);
}

// ---------------------------------------------------------------------------
// tps_from_env
// ---------------------------------------------------------------------------

#[test]
fn test_tps_from_env_uses_default_when_unset() {
    // Use a variable name very unlikely to be set.
    let tps = common::tps_from_env("__RELAY_TEST_TPS_NONEXISTENT__", 42.0);
    assert!((tps - 42.0).abs() < f64::EPSILON);
}
