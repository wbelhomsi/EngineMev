use solana_mev_bot::cexdex::PriceStore;
use solana_mev_bot::feed::PriceSnapshot;
use std::time::Instant;

fn mk_snapshot(bid: f64, ask: f64) -> PriceSnapshot {
    PriceSnapshot {
        best_bid_usd: bid,
        best_ask_usd: ask,
        received_at: Instant::now(),
    }
}

#[test]
fn test_empty_store_returns_none() {
    let store = PriceStore::new();
    assert!(store.get_cex("SOLUSDC").is_none());
}

#[test]
fn test_update_and_read() {
    let store = PriceStore::new();
    store.update_cex("SOLUSDC", mk_snapshot(185.20, 185.21));
    let snap = store.get_cex("SOLUSDC").unwrap();
    assert_eq!(snap.best_bid_usd, 185.20);
    assert_eq!(snap.best_ask_usd, 185.21);
}

#[test]
fn test_update_overwrites() {
    let store = PriceStore::new();
    store.update_cex("SOLUSDC", mk_snapshot(185.20, 185.21));
    store.update_cex("SOLUSDC", mk_snapshot(186.00, 186.05));
    let snap = store.get_cex("SOLUSDC").unwrap();
    assert_eq!(snap.best_bid_usd, 186.00);
}

#[test]
fn test_multiple_symbols_are_independent() {
    let store = PriceStore::new();
    store.update_cex("SOLUSDC", mk_snapshot(185.0, 185.1));
    store.update_cex("SOLUSDT", mk_snapshot(184.0, 184.1));

    let a = store.get_cex("SOLUSDC").unwrap();
    let b = store.get_cex("SOLUSDT").unwrap();
    assert_eq!(a.best_bid_usd, 185.0);
    assert_eq!(b.best_bid_usd, 184.0);
}

#[test]
fn test_staleness_check() {
    let store = PriceStore::new();
    let old_snap = PriceSnapshot {
        best_bid_usd: 185.0,
        best_ask_usd: 185.1,
        received_at: Instant::now() - std::time::Duration::from_secs(2),
    };
    store.update_cex("SOLUSDC", old_snap);

    assert!(store.is_stale("SOLUSDC", 500));
    assert!(!store.is_stale("SOLUSDC", 5000));

    // Missing symbol is always "stale"
    assert!(store.is_stale("MISSING", 500));
}
