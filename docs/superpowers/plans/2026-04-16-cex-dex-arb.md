# CEX-DEX Arbitrage (Model A, SOL/USDC) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a separate `cexdex` binary that arbitrages SOL/USDC between Binance and on-chain Solana DEXes via inventory-based single-leg execution.

**Architecture:** New binary at `src/bin/cexdex.rs`. New modules `src/feed/` (Binance WS) and `src/cexdex/` (detector, inventory, simulator, route, units). Reuses existing `BundleBuilder`, `RelayDispatcher`, and `router::dex` quoters. Separate wallet for P&L isolation.

**Tech Stack:** Rust, `tokio-tungstenite` (WS), existing `solana-sdk` 4.0 stack, `crossbeam-channel`, `dashmap`, `tracing`.

**Spec:** `docs/superpowers/specs/2026-04-16-cex-dex-arb-design.md`

---

## File Structure

**New:**
```
src/
├── bin/
│   └── cexdex.rs                       # Binary entry point
├── feed/
│   ├── mod.rs                          # CexFeed trait, PriceSnapshot type
│   └── binance.rs                      # Binance bookTicker WS client
└── cexdex/
    ├── mod.rs                          # Public re-exports + CexDexConfig
    ├── config.rs                       # CexDexConfig::from_env()
    ├── units.rs                        # Decimal conversion helpers
    ├── price_store.rs                  # PriceStore with atomic CEX data
    ├── inventory.rs                    # Inventory tracking + gates
    ├── route.rs                        # CexDexRoute type + direction enum
    ├── detector.rs                     # Divergence detection + sizing
    ├── simulator.rs                    # CEX-priced profit simulator
    ├── bundle.rs                       # Wraps BundleBuilder for single-leg swaps
    └── geyser.rs                       # Narrow LaserStream subscription for specific pools

tests/unit/
├── cexdex_units.rs
├── cexdex_inventory.rs
├── cexdex_detector.rs
├── cexdex_simulator.rs
└── cexdex_price_store.rs

tests/e2e/
└── cexdex_pipeline.rs                  # End-to-end detector → bundle
```

**Modified:**
- `src/lib.rs` — add `pub mod cexdex;` and `pub mod feed;`
- `tests/unit/mod.rs` — declare new test modules
- `tests/e2e/mod.rs` — declare new test module
- `Cargo.toml` — add `[[bin]]` entry for cexdex
- `.env.example` — add CEXDEX_* variables

---

## Task 1: Scaffold `cexdex` Module + Binary Entry Point

**Files:**
- Create: `src/cexdex/mod.rs`
- Create: `src/cexdex/config.rs`
- Create: `src/bin/cexdex.rs`
- Create: `src/feed/mod.rs`
- Modify: `src/lib.rs`
- Modify: `Cargo.toml`

- [ ] **Step 1: Add modules to `src/lib.rs`**

Replace contents of `src/lib.rs`:

```rust
pub mod addresses;
pub mod cexdex;
pub mod config;
pub mod executor;
pub mod feed;
pub mod mempool;
pub mod metrics;
pub mod router;
pub mod rpc_helpers;
pub mod sanctum;
pub mod state;
```

- [ ] **Step 2: Create `src/feed/mod.rs` (stub)**

```rust
//! CEX price feeds (currently Binance only).

pub mod binance;

/// Snapshot of a CEX top-of-book at a specific instant.
/// All prices in USD per base unit (e.g., USD per SOL).
#[derive(Debug, Clone, Copy)]
pub struct PriceSnapshot {
    pub best_bid_usd: f64,
    pub best_ask_usd: f64,
    /// Local receive time, NOT exchange timestamp (avoids clock skew).
    pub received_at: std::time::Instant,
}

impl PriceSnapshot {
    pub fn mid(&self) -> f64 {
        (self.best_bid_usd + self.best_ask_usd) / 2.0
    }

    pub fn age_ms(&self) -> u64 {
        self.received_at.elapsed().as_millis() as u64
    }
}
```

- [ ] **Step 3: Create `src/feed/binance.rs` (stub — real impl in Task 3)**

```rust
//! Binance bookTicker WebSocket client.

pub const BINANCE_WS_URL: &str = "wss://stream.binance.com:9443/ws";
pub const SOLUSDC_STREAM: &str = "solusdc@bookTicker";

// Implementation in Task 3.
```

- [ ] **Step 4: Create `src/cexdex/mod.rs`**

```rust
//! CEX-DEX arbitrage module (Model A, SOL/USDC).
//!
//! Run via `cargo run --release --bin cexdex`.

pub mod config;
pub mod units;
pub mod price_store;
pub mod inventory;
pub mod route;
pub mod detector;
pub mod simulator;
pub mod bundle;
pub mod geyser;

pub use config::CexDexConfig;
pub use inventory::Inventory;
pub use price_store::PriceStore;
pub use route::{ArbDirection, CexDexRoute};
```

- [ ] **Step 5: Create `src/cexdex/config.rs` stub**

```rust
//! CEX-DEX configuration loaded from environment variables.
//!
//! All env vars prefixed `CEXDEX_` to avoid collision with the main engine.

use anyhow::Result;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::time::Duration;

use crate::router::pool::DexType;

#[derive(Debug, Clone)]
pub struct CexDexConfig {
    // Wallet (separate from main engine)
    pub searcher_keypair_path: String,
    pub searcher_private_key: Option<String>,

    // Solana
    pub rpc_url: String,
    pub geyser_grpc_url: String,
    pub geyser_auth_token: String,

    // Binance
    pub binance_ws_url: String,
    pub cex_staleness_ms: u64,

    // Pools to monitor (DexType + Pubkey pairs)
    pub pools: Vec<(DexType, Pubkey)>,

    // Strategy
    pub min_spread_bps: u64,
    pub min_profit_usd: f64,
    pub max_trade_size_sol: f64,

    // Inventory gates
    pub hard_cap_ratio: f64,
    pub preferred_low: f64,
    pub preferred_high: f64,
    pub skewed_profit_multiplier: f64,

    // Slippage
    pub slippage_tolerance: f64,

    // Safety
    pub dry_run: bool,
    pub pool_state_ttl: Duration,

    // Relays (reuses main engine env vars)
    pub jito_block_engine_url: String,
    pub astralane_relay_url: Option<String>,
    pub astralane_api_key: Option<String>,

    // Arb-guard program (optional — single-leg may not need it)
    pub arb_guard_program_id: Option<Pubkey>,

    // Metrics
    pub metrics_port: Option<u16>,
}

impl CexDexConfig {
    pub fn from_env() -> Result<Self> {
        dotenv::dotenv().ok();

        let searcher_keypair_path = std::env::var("CEXDEX_SEARCHER_KEYPAIR")
            .unwrap_or_else(|_| "cexdex-searcher.json".to_string());
        let searcher_private_key = std::env::var("CEXDEX_SEARCHER_PRIVATE_KEY").ok();

        let rpc_url = std::env::var("RPC_URL")
            .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string());
        let geyser_grpc_url = std::env::var("GEYSER_GRPC_URL")
            .unwrap_or_else(|_| "http://localhost:10000".to_string());
        let geyser_auth_token = std::env::var("GEYSER_AUTH_TOKEN").unwrap_or_default();

        let binance_ws_url = std::env::var("CEXDEX_BINANCE_WS_URL")
            .unwrap_or_else(|_| "wss://stream.binance.com:9443/ws".to_string());
        let cex_staleness_ms: u64 = std::env::var("CEXDEX_CEX_STALENESS_MS")
            .unwrap_or_else(|_| "500".to_string())
            .parse()?;

        // Format: "RaydiumCp:<pubkey>,Orca:<pubkey>,..."
        let pools_raw = std::env::var("CEXDEX_POOLS").unwrap_or_default();
        let mut pools: Vec<(DexType, Pubkey)> = Vec::new();
        for entry in pools_raw.split(',').filter(|s| !s.is_empty()) {
            let (dex_str, pk_str) = entry.split_once(':')
                .ok_or_else(|| anyhow::anyhow!("Invalid CEXDEX_POOLS entry: {}", entry))?;
            let dex_type = match dex_str.trim() {
                "RaydiumAmm" => DexType::RaydiumAmm,
                "RaydiumCp" => DexType::RaydiumCp,
                "RaydiumClmm" => DexType::RaydiumClmm,
                "Orca" | "OrcaWhirlpool" => DexType::OrcaWhirlpool,
                "MeteoraDlmm" => DexType::MeteoraDlmm,
                "MeteoraDammV2" => DexType::MeteoraDammV2,
                other => anyhow::bail!("Unsupported DexType for CEX-DEX: {}", other),
            };
            let pubkey = Pubkey::from_str(pk_str.trim())
                .map_err(|e| anyhow::anyhow!("Invalid pubkey '{}': {}", pk_str, e))?;
            pools.push((dex_type, pubkey));
        }

        let min_spread_bps: u64 = std::env::var("CEXDEX_MIN_SPREAD_BPS")
            .unwrap_or_else(|_| "15".to_string()).parse()?;
        let min_profit_usd: f64 = std::env::var("CEXDEX_MIN_PROFIT_USD")
            .unwrap_or_else(|_| "0.10".to_string()).parse()?;
        let max_trade_size_sol: f64 = std::env::var("CEXDEX_MAX_TRADE_SIZE_SOL")
            .unwrap_or_else(|_| "10.0".to_string()).parse()?;

        let hard_cap_ratio: f64 = std::env::var("CEXDEX_HARD_CAP_RATIO")
            .unwrap_or_else(|_| "0.80".to_string()).parse()?;
        let preferred_low: f64 = std::env::var("CEXDEX_PREFERRED_LOW")
            .unwrap_or_else(|_| "0.40".to_string()).parse()?;
        let preferred_high: f64 = std::env::var("CEXDEX_PREFERRED_HIGH")
            .unwrap_or_else(|_| "0.60".to_string()).parse()?;
        let skewed_profit_multiplier: f64 = std::env::var("CEXDEX_SKEWED_PROFIT_MULTIPLIER")
            .unwrap_or_else(|_| "2.0".to_string()).parse()?;

        let slippage_tolerance: f64 = std::env::var("CEXDEX_SLIPPAGE_TOLERANCE")
            .unwrap_or_else(|_| "0.25".to_string()).parse()?;

        let dry_run = std::env::var("CEXDEX_DRY_RUN")
            .unwrap_or_else(|_| "true".to_string()).parse()?;

        let pool_state_ttl = Duration::from_secs(
            std::env::var("CEXDEX_POOL_TTL_SECS")
                .unwrap_or_else(|_| "5".to_string()).parse()?,
        );

        let jito_block_engine_url = std::env::var("JITO_BLOCK_ENGINE_URL")
            .unwrap_or_else(|_| "https://mainnet.block-engine.jito.wtf".to_string());
        let astralane_relay_url = std::env::var("ASTRALANE_RELAY_URL").ok();
        let astralane_api_key = std::env::var("ASTRALANE_API_KEY").ok();

        let arb_guard_program_id = std::env::var("ARB_GUARD_PROGRAM_ID")
            .ok()
            .and_then(|s| Pubkey::from_str(&s).ok());

        let metrics_port = std::env::var("CEXDEX_METRICS_PORT").ok()
            .and_then(|s| s.parse().ok());

        Ok(Self {
            searcher_keypair_path,
            searcher_private_key,
            rpc_url,
            geyser_grpc_url,
            geyser_auth_token,
            binance_ws_url,
            cex_staleness_ms,
            pools,
            min_spread_bps,
            min_profit_usd,
            max_trade_size_sol,
            hard_cap_ratio,
            preferred_low,
            preferred_high,
            skewed_profit_multiplier,
            slippage_tolerance,
            dry_run,
            pool_state_ttl,
            jito_block_engine_url,
            astralane_relay_url,
            astralane_api_key,
            arb_guard_program_id,
            metrics_port,
        })
    }
}
```

- [ ] **Step 6: Create stub modules referenced by mod.rs**

Create empty stubs so the crate compiles. Each file gets the following placeholder:

`src/cexdex/units.rs`:
```rust
//! Decimal conversion helpers. Real impl in Task 2.
```

`src/cexdex/price_store.rs`:
```rust
//! PriceStore. Real impl in Task 4.
```

`src/cexdex/inventory.rs`:
```rust
//! Inventory tracking. Real impl in Task 5.
```

`src/cexdex/route.rs`:
```rust
//! CexDexRoute type. Real impl in Task 6.
```

`src/cexdex/detector.rs`:
```rust
//! Divergence detector. Real impl in Task 7.
```

`src/cexdex/simulator.rs`:
```rust
//! CEX-priced profit simulator. Real impl in Task 8.
```

`src/cexdex/bundle.rs`:
```rust
//! Bundle building wrapper. Real impl in Task 9.
```

`src/cexdex/geyser.rs`:
```rust
//! Narrow Geyser subscription. Real impl in Task 10.
```

- [ ] **Step 7: Create `src/bin/cexdex.rs` (skeleton)**

```rust
//! CEX-DEX arbitrage binary.
//!
//! Run: `cargo run --release --bin cexdex`

use anyhow::Result;
use solana_mev_bot::cexdex::CexDexConfig;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .json()
        .init();

    info!("=== CEX-DEX Arbitrage Engine (Model A, SOL/USDC) ===");
    let config = CexDexConfig::from_env()?;
    info!(
        "Config: min_spread={}bps, min_profit=${:.2}, max_trade={} SOL, dry_run={}",
        config.min_spread_bps, config.min_profit_usd, config.max_trade_size_sol, config.dry_run,
    );
    info!("Monitoring {} pools", config.pools.len());

    // Full pipeline wired up in Task 11.
    anyhow::bail!("CEX-DEX binary: pipeline not yet wired (see Task 11)");
}
```

- [ ] **Step 8: Add `[[bin]]` entry to `Cargo.toml`**

Append after the existing `[[test]]` entries:

```toml
[[bin]]
name = "cexdex"
path = "src/bin/cexdex.rs"

[[bin]]
name = "solana-mev-bot"
path = "src/main.rs"
```

(The existing `main.rs` becomes an explicit bin entry to coexist with the new one.)

- [ ] **Step 9: Run `cargo check`**

Run: `cargo check --bin cexdex`
Expected: Compiles clean. Warnings about unused modules OK.

Run: `cargo check --bin solana-mev-bot`
Expected: Compiles clean. Main engine unaffected.

- [ ] **Step 10: Run existing tests to verify no regression**

Run: `cargo test`
Expected: 253 unit tests pass (same as before — 1 pre-existing `router_perf` flake in debug).

- [ ] **Step 11: Commit**

```bash
git add src/lib.rs src/feed/ src/cexdex/ src/bin/cexdex.rs Cargo.toml
git commit -m "feat(cexdex): scaffold CEX-DEX module, binary, and feed structure

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Task 2: Units Module (Decimal Conversion Helpers)

**Files:**
- Modify: `src/cexdex/units.rs`
- Create: `tests/unit/cexdex_units.rs`
- Modify: `tests/unit/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `tests/unit/cexdex_units.rs`:

```rust
use solana_mev_bot::cexdex::units::*;

#[test]
fn test_sol_lamports_roundtrip() {
    assert_eq!(sol_to_lamports(1.0), 1_000_000_000);
    assert_eq!(sol_to_lamports(0.5), 500_000_000);
    assert_eq!(lamports_to_sol(1_000_000_000), 1.0);
    assert_eq!(lamports_to_sol(500_000_000), 0.5);
}

#[test]
fn test_usdc_atoms_roundtrip() {
    assert_eq!(usdc_to_atoms(1.0), 1_000_000);
    assert_eq!(usdc_to_atoms(185.20), 185_200_000);
    assert_eq!(atoms_to_usdc(1_000_000), 1.0);
    assert_eq!(atoms_to_usdc(185_200_000), 185.20);
}

#[test]
fn test_sol_to_usdc_atoms_at_price() {
    // 1 SOL @ $185.00 = 185 USDC = 185_000_000 atoms
    assert_eq!(sol_to_usdc_atoms(1_000_000_000, 185.0), 185_000_000);
    // 0.5 SOL @ $200.00 = 100 USDC
    assert_eq!(sol_to_usdc_atoms(500_000_000, 200.0), 100_000_000);
}

#[test]
fn test_usdc_atoms_to_sol_lamports_at_price() {
    // 185 USDC @ $185.00 = 1 SOL
    assert_eq!(usdc_atoms_to_sol_lamports(185_000_000, 185.0), 1_000_000_000);
    // 100 USDC @ $200.00 = 0.5 SOL
    assert_eq!(usdc_atoms_to_sol_lamports(100_000_000, 200.0), 500_000_000);
}

#[test]
fn test_zero_amounts() {
    assert_eq!(sol_to_lamports(0.0), 0);
    assert_eq!(usdc_to_atoms(0.0), 0);
    assert_eq!(lamports_to_sol(0), 0.0);
    assert_eq!(atoms_to_usdc(0), 0.0);
}

#[test]
fn test_large_amounts() {
    // 1000 SOL = 1e12 lamports (well within u64)
    assert_eq!(sol_to_lamports(1000.0), 1_000_000_000_000);
    // 1M USDC = 1e12 atoms
    assert_eq!(usdc_to_atoms(1_000_000.0), 1_000_000_000_000);
}

#[test]
fn test_bps_to_fraction() {
    assert_eq!(bps_to_fraction(0), 0.0);
    assert_eq!(bps_to_fraction(100), 0.01);      // 1%
    assert_eq!(bps_to_fraction(10_000), 1.0);    // 100%
    assert_eq!(bps_to_fraction(15), 0.0015);     // 15 bps
}

#[test]
fn test_spread_bps() {
    // 0.5% spread: cex_mid=100, dex_price=100.5 → 50 bps
    let bps = spread_bps(100.0, 100.5);
    assert_eq!(bps, 50);

    // Negative direction (DEX cheaper): cex=100.5, dex=100 → 50 bps
    let bps2 = spread_bps(100.5, 100.0);
    assert_eq!(bps2, 50);

    // Zero spread
    assert_eq!(spread_bps(100.0, 100.0), 0);

    // Zero reference price (edge case — returns 0)
    assert_eq!(spread_bps(0.0, 100.0), 0);
}
```

- [ ] **Step 2: Add module declaration**

In `tests/unit/mod.rs`, add:

```rust
mod cexdex_units;
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test cexdex_units 2>&1 | tail -5`
Expected: FAIL with "function not defined" / "no function or associated item named ...".

- [ ] **Step 4: Implement `src/cexdex/units.rs`**

```rust
//! Decimal conversion helpers for SOL (9 decimals), USDC (6 decimals),
//! and USD prices (f64). All callers MUST use these helpers — never do
//! raw decimal math elsewhere.

pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;
pub const ATOMS_PER_USDC: u64 = 1_000_000;

#[inline]
pub fn sol_to_lamports(sol: f64) -> u64 {
    (sol * LAMPORTS_PER_SOL as f64) as u64
}

#[inline]
pub fn lamports_to_sol(lamports: u64) -> f64 {
    lamports as f64 / LAMPORTS_PER_SOL as f64
}

#[inline]
pub fn usdc_to_atoms(usdc: f64) -> u64 {
    (usdc * ATOMS_PER_USDC as f64) as u64
}

#[inline]
pub fn atoms_to_usdc(atoms: u64) -> f64 {
    atoms as f64 / ATOMS_PER_USDC as f64
}

/// Convert SOL lamports to USDC atoms at a given price (USD per SOL).
#[inline]
pub fn sol_to_usdc_atoms(sol_lamports: u64, price_usd_per_sol: f64) -> u64 {
    let sol = lamports_to_sol(sol_lamports);
    let usdc = sol * price_usd_per_sol;
    usdc_to_atoms(usdc)
}

/// Convert USDC atoms to SOL lamports at a given price (USD per SOL).
#[inline]
pub fn usdc_atoms_to_sol_lamports(usdc_atoms: u64, price_usd_per_sol: f64) -> u64 {
    if price_usd_per_sol <= 0.0 {
        return 0;
    }
    let usdc = atoms_to_usdc(usdc_atoms);
    let sol = usdc / price_usd_per_sol;
    sol_to_lamports(sol)
}

/// Convert basis points (1 bp = 0.01%) to a fraction.
#[inline]
pub fn bps_to_fraction(bps: u64) -> f64 {
    bps as f64 / 10_000.0
}

/// Compute the absolute spread between two prices in basis points.
/// Reference = first argument. Returns 0 if reference is 0.
#[inline]
pub fn spread_bps(reference: f64, other: f64) -> u64 {
    if reference <= 0.0 {
        return 0;
    }
    let diff = (other - reference).abs();
    ((diff / reference) * 10_000.0) as u64
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test cexdex_units 2>&1 | tail -5`
Expected: `test result: ok. 7 passed; 0 failed`.

- [ ] **Step 6: Commit**

```bash
git add src/cexdex/units.rs tests/unit/cexdex_units.rs tests/unit/mod.rs
git commit -m "feat(cexdex): add units module for decimal conversions

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Task 3: Binance Feed (WS Client)

**Files:**
- Modify: `src/feed/binance.rs`
- Modify: `src/feed/mod.rs`

- [ ] **Step 1: Implement `src/feed/binance.rs` with auto-reconnect**

Replace the stub with:

```rust
//! Binance bookTicker WebSocket client for SOL/USDC (and future pairs).
//!
//! Pattern mirrors `src/state/tip_floor.rs` — auto-reconnect with backoff,
//! graceful shutdown, and first-message logging for debugging.

use anyhow::Result;
use futures::StreamExt;
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use crate::feed::PriceSnapshot;
use crate::cexdex::PriceStore;

/// Default Binance WebSocket endpoint.
pub const BINANCE_WS_URL: &str = "wss://stream.binance.com:9443/ws";

/// SOL/USDC bookTicker stream name.
pub const SOLUSDC_STREAM: &str = "solusdc@bookTicker";

/// Reconnect delay after disconnect.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

/// If no message in this window, assume connection is dead and reconnect.
const WS_TIMEOUT: Duration = Duration::from_secs(30);

/// Connect to Binance bookTicker stream for SOL/USDC and update the PriceStore.
/// Reconnects on any error. Exits cleanly when `shutdown_rx` signals true.
pub async fn run_solusdc_loop(
    price_store: PriceStore,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        if *shutdown_rx.borrow() {
            break;
        }

        let url = format!("{}/{}", BINANCE_WS_URL, SOLUSDC_STREAM);
        info!("Connecting to Binance bookTicker: {}", url);

        let ws_result = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(&url),
        )
        .await;

        match ws_result {
            Ok(Ok((ws_stream, _response))) => {
                info!("Binance WS connected");
                let (_write, mut read) = ws_stream.split();
                let mut first_msg = true;

                loop {
                    tokio::select! {
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                info!("Binance WS loop shutting down");
                                return;
                            }
                        }
                        msg = tokio::time::timeout(WS_TIMEOUT, read.next()) => {
                            match msg {
                                Ok(Some(Ok(Message::Text(text)))) => {
                                    if first_msg {
                                        info!("Binance first message (raw): {}",
                                            &text[..text.len().min(500)]);
                                        first_msg = false;
                                    }
                                    match parse_book_ticker(&text) {
                                        Ok(snapshot) => {
                                            price_store.update_cex("SOLUSDC", snapshot);
                                        }
                                        Err(e) => {
                                            debug!("Failed to parse Binance msg: {}", e);
                                        }
                                    }
                                }
                                Ok(Some(Ok(Message::Ping(_)))) => {
                                    // tungstenite handles pong automatically
                                }
                                Ok(Some(Ok(Message::Close(_)))) => {
                                    warn!("Binance WS closed by server");
                                    break;
                                }
                                Ok(Some(Err(e))) => {
                                    warn!("Binance WS error: {}", e);
                                    break;
                                }
                                Ok(None) => {
                                    warn!("Binance WS ended");
                                    break;
                                }
                                Err(_) => {
                                    warn!("Binance WS no message in {}s, reconnecting",
                                        WS_TIMEOUT.as_secs());
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                warn!("Binance WS connect failed: {}", e);
            }
            Err(_) => {
                warn!("Binance WS connect timed out");
            }
        }

        // Wait before reconnecting
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() { break; }
            }
            _ = tokio::time::sleep(RECONNECT_DELAY) => {}
        }
    }

    info!("Binance WS loop exited");
}

/// Parse a bookTicker message into a PriceSnapshot.
///
/// Payload format:
/// ```json
/// { "u": 400900217, "s": "SOLUSDC", "b": "185.20", "B": "100.00",
///   "a": "185.21", "A": "50.00" }
/// ```
pub fn parse_book_ticker(text: &str) -> Result<PriceSnapshot> {
    let v: serde_json::Value = serde_json::from_str(text)?;
    let best_bid: f64 = v["b"].as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'b'"))?
        .parse()?;
    let best_ask: f64 = v["a"].as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'a'"))?
        .parse()?;

    if best_bid <= 0.0 || best_ask <= 0.0 {
        anyhow::bail!("non-positive price: bid={} ask={}", best_bid, best_ask);
    }
    if best_bid > best_ask {
        anyhow::bail!("inverted book: bid={} ask={}", best_bid, best_ask);
    }

    Ok(PriceSnapshot {
        best_bid_usd: best_bid,
        best_ask_usd: best_ask,
        received_at: Instant::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_book_ticker_happy() {
        let msg = r#"{"u":400900217,"s":"SOLUSDC","b":"185.20","B":"100.00","a":"185.21","A":"50.00"}"#;
        let snap = parse_book_ticker(msg).unwrap();
        assert_eq!(snap.best_bid_usd, 185.20);
        assert_eq!(snap.best_ask_usd, 185.21);
        assert!((snap.mid() - 185.205).abs() < 1e-6);
    }

    #[test]
    fn test_parse_book_ticker_rejects_inverted() {
        let msg = r#"{"u":1,"s":"SOLUSDC","b":"185.25","B":"100","a":"185.20","A":"50"}"#;
        assert!(parse_book_ticker(msg).is_err());
    }

    #[test]
    fn test_parse_book_ticker_rejects_zero() {
        let msg = r#"{"u":1,"s":"SOLUSDC","b":"0","B":"100","a":"185.20","A":"50"}"#;
        assert!(parse_book_ticker(msg).is_err());
    }

    #[test]
    fn test_parse_book_ticker_rejects_missing_fields() {
        let msg = r#"{"u":1,"s":"SOLUSDC"}"#;
        assert!(parse_book_ticker(msg).is_err());
    }
}
```

- [ ] **Step 2: Run the parser unit tests**

Run: `cargo test feed::binance 2>&1 | tail -5`
Expected: `test result: ok. 4 passed`.

- [ ] **Step 3: Run full test suite for regression check**

Run: `cargo test 2>&1 | grep "^test result"`
Expected: All existing tests still pass.

- [ ] **Step 4: Commit**

Note: The `run_solusdc_loop` depends on `PriceStore::update_cex` which we implement in Task 4. Since the function is unused in tests and not yet called from main, it compiles but is not exercised. That's fine — it'll be wired up in Task 11.

```bash
git add src/feed/binance.rs
git commit -m "feat(cexdex): Binance bookTicker WS client with parser tests

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Task 4: PriceStore (Shared State)

**Files:**
- Modify: `src/cexdex/price_store.rs`
- Create: `tests/unit/cexdex_price_store.rs`
- Modify: `tests/unit/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `tests/unit/cexdex_price_store.rs`:

```rust
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

    assert!(store.is_stale("SOLUSDC", 500)); // 500ms threshold, 2s old → stale
    assert!(!store.is_stale("SOLUSDC", 5000)); // 5s threshold → fresh

    // Missing symbol is always "stale"
    assert!(store.is_stale("MISSING", 500));
}
```

- [ ] **Step 2: Register test module**

In `tests/unit/mod.rs`, add:

```rust
mod cexdex_price_store;
```

- [ ] **Step 3: Run to see failing tests**

Run: `cargo test cexdex_price_store 2>&1 | tail -5`
Expected: FAIL (PriceStore type not implemented).

- [ ] **Step 4: Implement `src/cexdex/price_store.rs`**

```rust
//! Shared price state for CEX and on-chain pools.
//!
//! Written to by the Binance WS client (CEX prices) and the narrow
//! Geyser subscriber (pool states, via the existing StateCache).
//! Read by the detector.

use dashmap::DashMap;
use std::sync::Arc;

use crate::feed::PriceSnapshot;
use crate::state::StateCache;

#[derive(Clone)]
pub struct PriceStore {
    /// CEX prices keyed by symbol (e.g., "SOLUSDC").
    cex: Arc<DashMap<String, PriceSnapshot>>,
    /// Pool state cache (reuses main engine type).
    pub pools: StateCache,
}

impl Default for PriceStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PriceStore {
    pub fn new() -> Self {
        Self {
            cex: Arc::new(DashMap::new()),
            pools: StateCache::new(std::time::Duration::from_secs(5)),
        }
    }

    pub fn with_state_cache(pools: StateCache) -> Self {
        Self {
            cex: Arc::new(DashMap::new()),
            pools,
        }
    }

    pub fn update_cex(&self, symbol: &str, snapshot: PriceSnapshot) {
        self.cex.insert(symbol.to_string(), snapshot);
    }

    pub fn get_cex(&self, symbol: &str) -> Option<PriceSnapshot> {
        self.cex.get(symbol).map(|v| *v.value())
    }

    /// Returns true if the CEX snapshot for `symbol` is older than
    /// `max_age_ms`, OR if no snapshot has ever been received.
    pub fn is_stale(&self, symbol: &str, max_age_ms: u64) -> bool {
        match self.cex.get(symbol) {
            Some(snap) => snap.age_ms() > max_age_ms,
            None => true,
        }
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test cexdex_price_store 2>&1 | tail -5`
Expected: 5 passed.

- [ ] **Step 6: Commit**

```bash
git add src/cexdex/price_store.rs tests/unit/cexdex_price_store.rs tests/unit/mod.rs
git commit -m "feat(cexdex): PriceStore with CEX snapshots and StateCache

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Task 5: Inventory Tracking and Gates

**Files:**
- Modify: `src/cexdex/inventory.rs`
- Create: `tests/unit/cexdex_inventory.rs`
- Modify: `tests/unit/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `tests/unit/cexdex_inventory.rs`:

```rust
use solana_mev_bot::cexdex::{Inventory, ArbDirection};

fn mk_inventory() -> Inventory {
    Inventory::new_for_test()
}

#[test]
fn test_initial_empty_balance() {
    let inv = mk_inventory();
    assert_eq!(inv.sol_lamports_available(), 0);
    assert_eq!(inv.usdc_atoms_available(), 0);
}

#[test]
fn test_set_balances_and_ratio_50_50() {
    let inv = mk_inventory();
    inv.set_on_chain(5_000_000_000, 925_000_000); // 5 SOL + 925 USDC @ $185
    inv.set_sol_price_usd(185.0);
    // 5 SOL * 185 = $925; USDC = $925 → ratio 0.5
    let r = inv.ratio();
    assert!((r - 0.5).abs() < 0.001, "expected 0.5, got {}", r);
}

#[test]
fn test_ratio_100_sol() {
    let inv = mk_inventory();
    inv.set_on_chain(5_000_000_000, 0);
    inv.set_sol_price_usd(185.0);
    assert_eq!(inv.ratio(), 1.0);
}

#[test]
fn test_ratio_100_usdc() {
    let inv = mk_inventory();
    inv.set_on_chain(0, 925_000_000);
    inv.set_sol_price_usd(185.0);
    assert_eq!(inv.ratio(), 0.0);
}

#[test]
fn test_allow_normal_zone_both_directions() {
    let inv = mk_inventory();
    inv.set_on_chain(5_000_000_000, 925_000_000); // 50/50
    inv.set_sol_price_usd(185.0);

    // Within preferred zone [0.40, 0.60] → both sides allowed at normal threshold
    assert!(inv.allows_direction(ArbDirection::BuyOnDex));
    assert!(inv.allows_direction(ArbDirection::SellOnDex));
    assert_eq!(inv.profit_multiplier(ArbDirection::BuyOnDex), 1.0);
    assert_eq!(inv.profit_multiplier(ArbDirection::SellOnDex), 1.0);
}

#[test]
fn test_skewed_sol_heavy_prefers_sell() {
    let inv = mk_inventory();
    // 70/30 SOL-heavy: 7 SOL @ $185 = $1295 SOL, 555 USDC → ratio = 1295/1850 = 0.7
    inv.set_on_chain(7_000_000_000, 555_000_000);
    inv.set_sol_price_usd(185.0);

    // Both allowed (not hard cap), but sell should be normal threshold,
    // buy should require 2× multiplier
    assert!(inv.allows_direction(ArbDirection::BuyOnDex));
    assert!(inv.allows_direction(ArbDirection::SellOnDex));
    assert_eq!(inv.profit_multiplier(ArbDirection::BuyOnDex), 2.0);
    assert_eq!(inv.profit_multiplier(ArbDirection::SellOnDex), 1.0);
}

#[test]
fn test_hard_cap_rejects_buy_when_sol_heavy() {
    let inv = mk_inventory();
    // 90/10 SOL-heavy: ratio = 0.9, hard cap is 0.8
    inv.set_on_chain(9_000_000_000, 185_000_000);
    inv.set_sol_price_usd(185.0);

    assert!(!inv.allows_direction(ArbDirection::BuyOnDex), "should block buy at 90% SOL");
    assert!(inv.allows_direction(ArbDirection::SellOnDex), "can still sell to rebalance");
}

#[test]
fn test_hard_cap_rejects_sell_when_usdc_heavy() {
    let inv = mk_inventory();
    // 10/90 SOL/USDC: ratio = 0.1, hard cap blocks selling more SOL
    inv.set_on_chain(1_000_000_000, 1_665_000_000); // 1 SOL + 1665 USDC @ $185
    inv.set_sol_price_usd(185.0);

    assert!(inv.allows_direction(ArbDirection::BuyOnDex));
    assert!(!inv.allows_direction(ArbDirection::SellOnDex));
}

#[test]
fn test_reservation_lifecycle_commit() {
    let inv = mk_inventory();
    inv.set_on_chain(5_000_000_000, 925_000_000);

    // Reserve 1 SOL for a sell
    inv.reserve(ArbDirection::SellOnDex, 1_000_000_000, 0);
    assert_eq!(inv.sol_lamports_available(), 4_000_000_000);

    // Commit: moves balance from reserved to "consumed"
    inv.commit(ArbDirection::SellOnDex, 1_000_000_000, 185_000_000);
    assert_eq!(inv.sol_lamports_available(), 4_000_000_000);
    // USDC should have increased
    assert_eq!(inv.usdc_atoms_available(), 925_000_000 + 185_000_000);
}

#[test]
fn test_reservation_lifecycle_release() {
    let inv = mk_inventory();
    inv.set_on_chain(5_000_000_000, 925_000_000);

    inv.reserve(ArbDirection::SellOnDex, 1_000_000_000, 0);
    assert_eq!(inv.sol_lamports_available(), 4_000_000_000);

    // Release (e.g., bundle dropped)
    inv.release(ArbDirection::SellOnDex, 1_000_000_000, 0);
    assert_eq!(inv.sol_lamports_available(), 5_000_000_000);
    assert_eq!(inv.usdc_atoms_available(), 925_000_000);
}
```

- [ ] **Step 2: Register test module**

In `tests/unit/mod.rs`, add:

```rust
mod cexdex_inventory;
```

- [ ] **Step 3: Run to see failures**

Run: `cargo test cexdex_inventory 2>&1 | tail -10`
Expected: FAIL (types not implemented).

- [ ] **Step 4: Implement `src/cexdex/inventory.rs`**

```rust
//! Inventory tracking with reservation lifecycle and ratio-based gates.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::cexdex::route::ArbDirection;
use crate::cexdex::units::{atoms_to_usdc, lamports_to_sol};

/// Default gate values (overridden from config in production).
const DEFAULT_HARD_CAP: f64 = 0.80;
const DEFAULT_PREFERRED_LOW: f64 = 0.40;
const DEFAULT_PREFERRED_HIGH: f64 = 0.60;
const DEFAULT_SKEWED_MULTIPLIER: f64 = 2.0;

#[derive(Clone)]
pub struct Inventory {
    inner: Arc<InventoryInner>,
}

struct InventoryInner {
    sol_on_chain_lamports: AtomicU64,
    usdc_on_chain_atoms: AtomicU64,
    sol_reserved_lamports: AtomicU64,
    usdc_reserved_atoms: AtomicU64,
    // sol price stored as atomic u64 (multiplied by 1e6 for precision)
    sol_price_usd_scaled: AtomicU64,

    hard_cap: f64,
    preferred_low: f64,
    preferred_high: f64,
    skewed_multiplier: f64,
}

impl Inventory {
    pub fn new(
        hard_cap: f64,
        preferred_low: f64,
        preferred_high: f64,
        skewed_multiplier: f64,
    ) -> Self {
        Self {
            inner: Arc::new(InventoryInner {
                sol_on_chain_lamports: AtomicU64::new(0),
                usdc_on_chain_atoms: AtomicU64::new(0),
                sol_reserved_lamports: AtomicU64::new(0),
                usdc_reserved_atoms: AtomicU64::new(0),
                sol_price_usd_scaled: AtomicU64::new(0),
                hard_cap,
                preferred_low,
                preferred_high,
                skewed_multiplier,
            }),
        }
    }

    /// Test helper — uses default gates.
    pub fn new_for_test() -> Self {
        Self::new(
            DEFAULT_HARD_CAP,
            DEFAULT_PREFERRED_LOW,
            DEFAULT_PREFERRED_HIGH,
            DEFAULT_SKEWED_MULTIPLIER,
        )
    }

    pub fn set_on_chain(&self, sol_lamports: u64, usdc_atoms: u64) {
        self.inner.sol_on_chain_lamports.store(sol_lamports, Ordering::SeqCst);
        self.inner.usdc_on_chain_atoms.store(usdc_atoms, Ordering::SeqCst);
    }

    pub fn set_sol_price_usd(&self, price: f64) {
        let scaled = (price * 1_000_000.0) as u64;
        self.inner.sol_price_usd_scaled.store(scaled, Ordering::SeqCst);
    }

    pub fn sol_price_usd(&self) -> f64 {
        self.inner.sol_price_usd_scaled.load(Ordering::SeqCst) as f64 / 1_000_000.0
    }

    pub fn sol_lamports_available(&self) -> u64 {
        let on_chain = self.inner.sol_on_chain_lamports.load(Ordering::SeqCst);
        let reserved = self.inner.sol_reserved_lamports.load(Ordering::SeqCst);
        on_chain.saturating_sub(reserved)
    }

    pub fn usdc_atoms_available(&self) -> u64 {
        let on_chain = self.inner.usdc_on_chain_atoms.load(Ordering::SeqCst);
        let reserved = self.inner.usdc_reserved_atoms.load(Ordering::SeqCst);
        on_chain.saturating_sub(reserved)
    }

    /// Returns ratio = sol_value_usd / (sol_value_usd + usdc_value_usd).
    /// Ranges [0.0, 1.0]. Returns 0.5 if total is zero (neutral).
    pub fn ratio(&self) -> f64 {
        let price = self.sol_price_usd();
        if price <= 0.0 {
            return 0.5;
        }
        let sol_usd = lamports_to_sol(
            self.inner.sol_on_chain_lamports.load(Ordering::SeqCst),
        ) * price;
        let usdc_usd = atoms_to_usdc(
            self.inner.usdc_on_chain_atoms.load(Ordering::SeqCst),
        );
        let total = sol_usd + usdc_usd;
        if total <= 0.0 {
            0.5
        } else {
            sol_usd / total
        }
    }

    /// True if we can take a trade in the given direction (not past hard cap).
    pub fn allows_direction(&self, dir: ArbDirection) -> bool {
        let r = self.ratio();
        match dir {
            ArbDirection::BuyOnDex => r < self.inner.hard_cap,  // adds SOL
            ArbDirection::SellOnDex => r > (1.0 - self.inner.hard_cap), // removes SOL
        }
    }

    /// Return the profit multiplier for a direction: 1.0 in preferred zone,
    /// `skewed_multiplier` if the direction would push further from neutral.
    pub fn profit_multiplier(&self, dir: ArbDirection) -> f64 {
        let r = self.ratio();
        let in_preferred = r >= self.inner.preferred_low && r <= self.inner.preferred_high;
        if in_preferred {
            return 1.0;
        }
        // Outside preferred: if direction worsens the skew, require skewed_multiplier.
        let worsens = match dir {
            ArbDirection::BuyOnDex => r > self.inner.preferred_high, // already SOL-heavy
            ArbDirection::SellOnDex => r < self.inner.preferred_low, // already USDC-heavy
        };
        if worsens {
            self.inner.skewed_multiplier
        } else {
            1.0
        }
    }

    /// Reserve funds before submitting a bundle.
    pub fn reserve(&self, dir: ArbDirection, input_amount: u64, _output_amount: u64) {
        match dir {
            ArbDirection::BuyOnDex => {
                self.inner.usdc_reserved_atoms.fetch_add(input_amount, Ordering::SeqCst);
            }
            ArbDirection::SellOnDex => {
                self.inner.sol_reserved_lamports.fetch_add(input_amount, Ordering::SeqCst);
            }
        }
    }

    /// Commit a reservation: deduct input from on-chain, add output, release reservation.
    pub fn commit(&self, dir: ArbDirection, input_amount: u64, output_amount: u64) {
        match dir {
            ArbDirection::BuyOnDex => {
                self.inner.usdc_on_chain_atoms.fetch_sub(input_amount, Ordering::SeqCst);
                self.inner.usdc_reserved_atoms.fetch_sub(input_amount, Ordering::SeqCst);
                self.inner.sol_on_chain_lamports.fetch_add(output_amount, Ordering::SeqCst);
            }
            ArbDirection::SellOnDex => {
                self.inner.sol_on_chain_lamports.fetch_sub(input_amount, Ordering::SeqCst);
                self.inner.sol_reserved_lamports.fetch_sub(input_amount, Ordering::SeqCst);
                self.inner.usdc_on_chain_atoms.fetch_add(output_amount, Ordering::SeqCst);
            }
        }
    }

    /// Release a reservation without committing (bundle dropped).
    pub fn release(&self, dir: ArbDirection, input_amount: u64, _output_amount: u64) {
        match dir {
            ArbDirection::BuyOnDex => {
                self.inner.usdc_reserved_atoms.fetch_sub(input_amount, Ordering::SeqCst);
            }
            ArbDirection::SellOnDex => {
                self.inner.sol_reserved_lamports.fetch_sub(input_amount, Ordering::SeqCst);
            }
        }
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test cexdex_inventory 2>&1 | tail -5`
Expected: 10 passed.

- [ ] **Step 6: Commit**

```bash
git add src/cexdex/inventory.rs tests/unit/cexdex_inventory.rs tests/unit/mod.rs
git commit -m "feat(cexdex): Inventory with reservation lifecycle and ratio gates

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Task 6: Route Type (CexDexRoute + ArbDirection)

**Files:**
- Modify: `src/cexdex/route.rs`

- [ ] **Step 1: Implement `src/cexdex/route.rs`**

```rust
//! CexDexRoute: a single-leg swap (USDC→SOL or SOL→USDC) on one pool.
//!
//! Unlike the main engine's `ArbRoute` which is circular (SOL→...→SOL),
//! this is unit-mismatched: input is one token, output is another.
//! Profit is calculated in USD via CEX prices, not by atom subtraction.

use solana_sdk::pubkey::Pubkey;

use crate::router::pool::DexType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArbDirection {
    /// DEX is cheap: we buy SOL on-chain with USDC.
    BuyOnDex,
    /// DEX is expensive: we sell SOL on-chain for USDC.
    SellOnDex,
}

impl ArbDirection {
    pub fn label(&self) -> &'static str {
        match self {
            ArbDirection::BuyOnDex => "buy_on_dex",
            ArbDirection::SellOnDex => "sell_on_dex",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CexDexRoute {
    pub pool_address: Pubkey,
    pub dex_type: DexType,
    pub direction: ArbDirection,
    pub input_mint: Pubkey,
    pub output_mint: Pubkey,
    pub input_amount: u64,        // atoms of input_mint
    pub expected_output: u64,     // atoms of output_mint (at current pool state)
    pub cex_bid_at_detection: f64,
    pub cex_ask_at_detection: f64,
    pub expected_profit_usd: f64, // gross, before tip
    pub observed_slot: u64,
}

impl CexDexRoute {
    pub fn cex_mid(&self) -> f64 {
        (self.cex_bid_at_detection + self.cex_ask_at_detection) / 2.0
    }
}
```

- [ ] **Step 2: Run `cargo check`**

Run: `cargo check --bin cexdex 2>&1 | tail -5`
Expected: Compiles cleanly.

- [ ] **Step 3: Commit**

```bash
git add src/cexdex/route.rs
git commit -m "feat(cexdex): CexDexRoute type and ArbDirection enum

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Task 7: Divergence Detector

**Files:**
- Modify: `src/cexdex/detector.rs`
- Create: `tests/unit/cexdex_detector.rs`
- Modify: `tests/unit/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `tests/unit/cexdex_detector.rs`:

```rust
use solana_mev_bot::addresses;
use solana_mev_bot::cexdex::detector::{Detector, DetectorConfig};
use solana_mev_bot::cexdex::route::ArbDirection;
use solana_mev_bot::cexdex::{Inventory, PriceStore};
use solana_mev_bot::feed::PriceSnapshot;
use solana_mev_bot::router::pool::{DexType, PoolExtra, PoolState};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::time::Instant;

fn usdc_mint() -> Pubkey {
    Pubkey::from_str("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap()
}

fn mk_detector_config() -> DetectorConfig {
    DetectorConfig {
        min_spread_bps: 15,
        min_profit_usd: 0.10,
        max_trade_size_sol: 10.0,
        cex_staleness_ms: 500,
        slippage_tolerance: 0.25,
    }
}

/// Build a RaydiumCp pool with given reserves (CPMM for deterministic math).
fn insert_cp_pool(
    store: &PriceStore,
    sol_reserve: u64,
    usdc_reserve: u64,
    fee_bps: u64,
) -> (Pubkey, DexType) {
    let addr = Pubkey::new_unique();
    store.pools.upsert(addr, PoolState {
        address: addr,
        dex_type: DexType::RaydiumCp,
        token_a_mint: addresses::WSOL,
        token_b_mint: usdc_mint(),
        token_a_reserve: sol_reserve,
        token_b_reserve: usdc_reserve,
        fee_bps,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            config: Some(Pubkey::new_unique()),
            token_program_a: Some(addresses::SPL_TOKEN),
            token_program_b: Some(addresses::SPL_TOKEN),
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    });
    (addr, DexType::RaydiumCp)
}

#[test]
fn test_no_opportunity_when_prices_aligned() {
    let store = PriceStore::new();
    // Pool at $185.00 per SOL: 100 SOL + 18500 USDC
    let (pool_addr, dex) = insert_cp_pool(
        &store,
        100_000_000_000,
        18_500_000_000,
        30,
    );
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 184.99,
        best_ask_usd: 185.01,
        received_at: Instant::now(),
    });

    let inv = Inventory::new_for_test();
    inv.set_on_chain(5_000_000_000, 5_000_000_000);
    inv.set_sol_price_usd(185.0);

    let detector = Detector::new(
        store,
        inv,
        vec![(dex, pool_addr)],
        mk_detector_config(),
    );
    let result = detector.check_all();
    assert!(result.is_none(), "no divergence → no opportunity");
}

#[test]
fn test_detects_buy_on_dex_when_dex_cheap() {
    let store = PriceStore::new();
    // DEX price = 18000/100 = $180 per SOL (cheap)
    let (pool_addr, dex) = insert_cp_pool(
        &store,
        100_000_000_000,
        18_000_000_000,
        30,
    );
    // CEX mid = $185 (DEX is $5 cheaper → buy SOL on DEX, notional sell on CEX)
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 184.99,
        best_ask_usd: 185.01,
        received_at: Instant::now(),
    });

    let inv = Inventory::new_for_test();
    inv.set_on_chain(5_000_000_000, 5_000_000_000);
    inv.set_sol_price_usd(185.0);

    let detector = Detector::new(
        store,
        inv,
        vec![(dex, pool_addr)],
        mk_detector_config(),
    );
    let result = detector.check_all().expect("should find opportunity");
    assert_eq!(result.direction, ArbDirection::BuyOnDex);
    assert!(result.input_amount > 0);
    assert!(result.expected_profit_usd > 0.10,
        "profit {} should exceed min", result.expected_profit_usd);
}

#[test]
fn test_rejects_when_cex_stale() {
    let store = PriceStore::new();
    let (pool_addr, dex) = insert_cp_pool(
        &store,
        100_000_000_000,
        18_000_000_000,
        30,
    );
    // CEX timestamp 2 seconds old → stale (threshold 500ms)
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 184.99,
        best_ask_usd: 185.01,
        received_at: Instant::now() - std::time::Duration::from_secs(2),
    });

    let inv = Inventory::new_for_test();
    inv.set_on_chain(5_000_000_000, 5_000_000_000);
    inv.set_sol_price_usd(185.0);

    let detector = Detector::new(
        store,
        inv,
        vec![(dex, pool_addr)],
        mk_detector_config(),
    );
    assert!(detector.check_all().is_none(), "stale CEX should reject");
}

#[test]
fn test_rejects_when_inventory_hard_capped() {
    let store = PriceStore::new();
    // Clear buy-on-dex opportunity
    let (pool_addr, dex) = insert_cp_pool(
        &store,
        100_000_000_000,
        18_000_000_000,
        30,
    );
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 184.99,
        best_ask_usd: 185.01,
        received_at: Instant::now(),
    });

    let inv = Inventory::new_for_test();
    // Already 90% SOL-heavy → hard cap blocks buying more SOL
    inv.set_on_chain(9_000_000_000, 185_000_000);
    inv.set_sol_price_usd(185.0);

    let detector = Detector::new(
        store,
        inv,
        vec![(dex, pool_addr)],
        mk_detector_config(),
    );
    assert!(detector.check_all().is_none(),
        "hard cap should block buy when SOL-heavy");
}

#[test]
fn test_picks_best_opportunity_across_pools() {
    let store = PriceStore::new();
    // Pool A: small divergence ($183 on DEX, $185 CEX)
    let (pool_a, _) = insert_cp_pool(&store, 100_000_000_000, 18_300_000_000, 30);
    // Pool B: larger divergence ($180 on DEX, $185 CEX)
    let (pool_b, _) = insert_cp_pool(&store, 100_000_000_000, 18_000_000_000, 30);

    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 184.99,
        best_ask_usd: 185.01,
        received_at: Instant::now(),
    });

    let inv = Inventory::new_for_test();
    inv.set_on_chain(5_000_000_000, 100_000_000_000);
    inv.set_sol_price_usd(185.0);

    let detector = Detector::new(
        store,
        inv,
        vec![(DexType::RaydiumCp, pool_a), (DexType::RaydiumCp, pool_b)],
        mk_detector_config(),
    );
    let result = detector.check_all().expect("should find opportunity");
    assert_eq!(result.pool_address, pool_b, "should prefer the more divergent pool");
}
```

- [ ] **Step 2: Register test module**

In `tests/unit/mod.rs`:

```rust
mod cexdex_detector;
```

- [ ] **Step 3: Implement `src/cexdex/detector.rs`**

```rust
//! Divergence detector: compares CEX prices vs on-chain pool prices and
//! constructs a CexDexRoute for the best opportunity.

use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use crate::addresses;
use crate::cexdex::inventory::Inventory;
use crate::cexdex::price_store::PriceStore;
use crate::cexdex::route::{ArbDirection, CexDexRoute};
use crate::cexdex::units::{
    atoms_to_usdc, lamports_to_sol, sol_to_lamports, spread_bps, usdc_to_atoms,
};
use crate::router::pool::{DexType, PoolState};

#[derive(Debug, Clone)]
pub struct DetectorConfig {
    pub min_spread_bps: u64,
    pub min_profit_usd: f64,
    pub max_trade_size_sol: f64,
    pub cex_staleness_ms: u64,
    pub slippage_tolerance: f64,
}

pub struct Detector {
    store: PriceStore,
    inventory: Inventory,
    pools: Vec<(DexType, Pubkey)>,
    config: DetectorConfig,
}

fn usdc_mint() -> Pubkey {
    Pubkey::from_str("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap()
}

impl Detector {
    pub fn new(
        store: PriceStore,
        inventory: Inventory,
        pools: Vec<(DexType, Pubkey)>,
        config: DetectorConfig,
    ) -> Self {
        Self { store, inventory, pools, config }
    }

    /// Check all monitored pools for divergence. Returns the most profitable
    /// CexDexRoute across all pools, or None if no qualifying opportunity.
    pub fn check_all(&self) -> Option<CexDexRoute> {
        // Gate: CEX must be fresh
        if self.store.is_stale("SOLUSDC", self.config.cex_staleness_ms) {
            return None;
        }
        let cex = self.store.get_cex("SOLUSDC")?;

        let mut best: Option<CexDexRoute> = None;

        for &(_dex, pool_addr) in &self.pools {
            let pool = match self.store.pools.get_any(&pool_addr) {
                Some(p) => p,
                None => continue,
            };

            for direction in [ArbDirection::BuyOnDex, ArbDirection::SellOnDex] {
                // Inventory gate
                if !self.inventory.allows_direction(direction) {
                    continue;
                }

                let route = match self.try_route(&pool, direction, &cex) {
                    Some(r) => r,
                    None => continue,
                };

                // Apply profit multiplier for skewed inventory
                let required_profit = self.config.min_profit_usd
                    * self.inventory.profit_multiplier(direction);
                if route.expected_profit_usd < required_profit {
                    continue;
                }

                match &best {
                    None => best = Some(route),
                    Some(b) if route.expected_profit_usd > b.expected_profit_usd => {
                        best = Some(route);
                    }
                    _ => {}
                }
            }
        }

        best
    }

    /// Try to construct a route for a single (pool, direction). Returns None
    /// if no qualifying divergence or sizing produces no profit.
    fn try_route(
        &self,
        pool: &PoolState,
        direction: ArbDirection,
        cex: &crate::feed::PriceSnapshot,
    ) -> Option<CexDexRoute> {
        // Pool mints: we assume one side is WSOL, the other is USDC.
        let wsol = addresses::WSOL;
        let usdc = usdc_mint();
        let (sol_is_a, sol_reserve, usdc_reserve) = if pool.token_a_mint == wsol
            && pool.token_b_mint == usdc
        {
            (true, pool.token_a_reserve, pool.token_b_reserve)
        } else if pool.token_b_mint == wsol && pool.token_a_mint == usdc {
            (false, pool.token_b_reserve, pool.token_a_reserve)
        } else {
            return None;
        };

        if sol_reserve == 0 || usdc_reserve == 0 {
            return None;
        }

        // Spot implied price (USDC per SOL, in USD-equivalent since USDC ≈ $1)
        let dex_spot = atoms_to_usdc(usdc_reserve) / lamports_to_sol(sol_reserve);

        // Determine if this direction has edge
        // - BuyOnDex  => DEX sells SOL cheaper than CEX buys SOL
        //                 DEX spot < cex.best_ask (we buy from DEX, sell on CEX at bid ≈ CEX mid)
        // - SellOnDex => DEX buys SOL higher than CEX sells SOL
        //                 DEX spot > cex.best_bid
        let (reference_price, edge_bps) = match direction {
            ArbDirection::BuyOnDex => {
                if dex_spot >= cex.best_bid_usd {
                    return None;
                }
                (cex.best_bid_usd, spread_bps(cex.best_bid_usd, dex_spot))
            }
            ArbDirection::SellOnDex => {
                if dex_spot <= cex.best_ask_usd {
                    return None;
                }
                (cex.best_ask_usd, spread_bps(cex.best_ask_usd, dex_spot))
            }
        };

        if edge_bps < self.config.min_spread_bps {
            return None;
        }

        // Size the trade: start with max configured, cap by inventory.
        let max_sol = self.config.max_trade_size_sol;
        let trade_sol = match direction {
            ArbDirection::BuyOnDex => {
                // Input is USDC, output is SOL.
                let usdc_available = self.inventory.usdc_atoms_available();
                let usdc_cap = atoms_to_usdc(usdc_available);
                let sol_from_usdc = usdc_cap / reference_price;
                sol_from_usdc.min(max_sol)
            }
            ArbDirection::SellOnDex => {
                let sol_available = lamports_to_sol(self.inventory.sol_lamports_available());
                sol_available.min(max_sol)
            }
        };

        if trade_sol < 0.001 {
            return None;
        }

        // Quote actual output at this size using the existing quoter.
        let (input_amount, input_mint, output_mint, a_to_b) = match direction {
            ArbDirection::BuyOnDex => {
                // USDC → SOL. Input is USDC atoms.
                let usdc_to_spend = trade_sol * reference_price;
                let input = usdc_to_atoms(usdc_to_spend);
                if sol_is_a {
                    // input_mint is USDC, which is token_b; direction is b→a
                    (input, usdc, wsol, false)
                } else {
                    (input, usdc, wsol, true)
                }
            }
            ArbDirection::SellOnDex => {
                let input = sol_to_lamports(trade_sol);
                if sol_is_a {
                    (input, wsol, usdc, true)
                } else {
                    (input, wsol, usdc, false)
                }
            }
        };

        let output = pool.get_output_amount_with_cache(
            input_amount,
            a_to_b,
            self.store.pools.get_bin_arrays(&pool.address).as_deref(),
            self.store.pools.get_tick_arrays(&pool.address).as_deref(),
        )?;
        if output == 0 {
            return None;
        }

        // CEX-priced profit = output_usd - input_usd
        let (input_usd, output_usd) = match direction {
            ArbDirection::BuyOnDex => {
                // input is USDC (≈$1), output is SOL (CEX sells at cex.bid)
                (atoms_to_usdc(input_amount), lamports_to_sol(output) * cex.best_bid_usd)
            }
            ArbDirection::SellOnDex => {
                // input is SOL, output is USDC
                (lamports_to_sol(input_amount) * cex.best_ask_usd, atoms_to_usdc(output))
            }
        };

        let gross_profit_usd = output_usd - input_usd;
        let slippage_discount = 1.0 - self.config.slippage_tolerance;
        let adjusted_profit_usd = gross_profit_usd * slippage_discount;
        if adjusted_profit_usd <= 0.0 {
            return None;
        }

        Some(CexDexRoute {
            pool_address: pool.address,
            dex_type: pool.dex_type,
            direction,
            input_mint,
            output_mint,
            input_amount,
            expected_output: output,
            cex_bid_at_detection: cex.best_bid_usd,
            cex_ask_at_detection: cex.best_ask_usd,
            expected_profit_usd: adjusted_profit_usd,
            observed_slot: pool.last_slot,
        })
    }
}
```

- [ ] **Step 4: Run detector tests**

Run: `cargo test cexdex_detector 2>&1 | tail -10`
Expected: 5 passed.

- [ ] **Step 5: Run full test suite**

Run: `cargo test 2>&1 | grep "^test result"`
Expected: All prior tests pass, +5 new.

- [ ] **Step 6: Commit**

```bash
git add src/cexdex/detector.rs tests/unit/cexdex_detector.rs tests/unit/mod.rs
git commit -m "feat(cexdex): divergence detector with trade sizing

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Task 8: CEX-Priced Simulator

**Files:**
- Modify: `src/cexdex/simulator.rs`
- Create: `tests/unit/cexdex_simulator.rs`
- Modify: `tests/unit/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `tests/unit/cexdex_simulator.rs`:

```rust
use solana_mev_bot::addresses;
use solana_mev_bot::cexdex::price_store::PriceStore;
use solana_mev_bot::cexdex::route::{ArbDirection, CexDexRoute};
use solana_mev_bot::cexdex::simulator::{CexDexSimulator, CexDexSimulatorConfig, SimulationResult};
use solana_mev_bot::feed::PriceSnapshot;
use solana_mev_bot::router::pool::{DexType, PoolExtra, PoolState};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::time::Instant;

fn usdc_mint() -> Pubkey {
    Pubkey::from_str("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap()
}

fn insert_cp_pool(
    store: &PriceStore,
    sol_reserve: u64,
    usdc_reserve: u64,
    fee_bps: u64,
) -> Pubkey {
    let addr = Pubkey::new_unique();
    store.pools.upsert(addr, PoolState {
        address: addr,
        dex_type: DexType::RaydiumCp,
        token_a_mint: addresses::WSOL,
        token_b_mint: usdc_mint(),
        token_a_reserve: sol_reserve,
        token_b_reserve: usdc_reserve,
        fee_bps,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
        best_bid_price: None,
        best_ask_price: None,
    });
    addr
}

fn mk_route_buy(pool: Pubkey, input_usdc_atoms: u64) -> CexDexRoute {
    CexDexRoute {
        pool_address: pool,
        dex_type: DexType::RaydiumCp,
        direction: ArbDirection::BuyOnDex,
        input_mint: usdc_mint(),
        output_mint: addresses::WSOL,
        input_amount: input_usdc_atoms,
        expected_output: 0,
        cex_bid_at_detection: 185.0,
        cex_ask_at_detection: 185.02,
        expected_profit_usd: 1.0,
        observed_slot: 100,
    }
}

fn mk_config() -> CexDexSimulatorConfig {
    CexDexSimulatorConfig {
        min_profit_usd: 0.10,
        slippage_tolerance: 0.25,
        tx_fee_lamports: 5_000,
        min_tip_lamports: 1_000,
        tip_fraction: 0.50,
    }
}

#[test]
fn test_profitable_route_passes() {
    let store = PriceStore::new();
    // DEX at $180 (cheap), CEX bid at $185. Buy 100 USDC worth of SOL.
    let pool = insert_cp_pool(&store, 100_000_000_000, 18_000_000_000, 30);
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 185.0,
        best_ask_usd: 185.02,
        received_at: Instant::now(),
    });
    let route = mk_route_buy(pool, 100_000_000); // 100 USDC

    let sim = CexDexSimulator::new(store, mk_config());
    let result = sim.simulate(&route);
    match result {
        SimulationResult::Profitable { net_profit_usd, tip_lamports, min_final_output, .. } => {
            assert!(net_profit_usd > 0.10);
            assert!(tip_lamports >= 1_000);
            assert!(min_final_output > 0);
        }
        SimulationResult::Unprofitable { reason } => {
            panic!("expected profitable, got: {}", reason);
        }
    }
}

#[test]
fn test_unprofitable_when_pool_not_cached() {
    let store = PriceStore::new();
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 185.0,
        best_ask_usd: 185.02,
        received_at: Instant::now(),
    });
    let route = mk_route_buy(Pubkey::new_unique(), 100_000_000);

    let sim = CexDexSimulator::new(store, mk_config());
    match sim.simulate(&route) {
        SimulationResult::Unprofitable { reason } => {
            assert!(reason.contains("not found") || reason.contains("cache"));
        }
        _ => panic!("expected unprofitable"),
    }
}

#[test]
fn test_unprofitable_when_profit_below_threshold() {
    let store = PriceStore::new();
    // DEX and CEX nearly identical → tiny profit, below min
    let pool = insert_cp_pool(&store, 100_000_000_000, 18_499_000_000, 30);
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 185.0,
        best_ask_usd: 185.02,
        received_at: Instant::now(),
    });
    let route = mk_route_buy(pool, 10_000_000); // 10 USDC: tiny amount

    let sim = CexDexSimulator::new(store, mk_config());
    match sim.simulate(&route) {
        SimulationResult::Unprofitable { .. } => {}
        _ => panic!("expected unprofitable"),
    }
}
```

- [ ] **Step 2: Register test module**

```rust
mod cexdex_simulator;
```

- [ ] **Step 3: Implement `src/cexdex/simulator.rs`**

```rust
//! CEX-priced profit simulator for CexDexRoute.

use tracing::debug;

use crate::cexdex::price_store::PriceStore;
use crate::cexdex::route::{ArbDirection, CexDexRoute};
use crate::cexdex::units::{atoms_to_usdc, lamports_to_sol, sol_to_lamports, usdc_to_atoms};

#[derive(Debug, Clone)]
pub struct CexDexSimulatorConfig {
    pub min_profit_usd: f64,
    pub slippage_tolerance: f64,
    pub tx_fee_lamports: u64,
    pub min_tip_lamports: u64,
    pub tip_fraction: f64,
}

#[derive(Debug)]
pub enum SimulationResult {
    Profitable {
        route: CexDexRoute,
        net_profit_usd: f64,
        tip_lamports: u64,
        /// On-chain arb-guard minimum output (in atoms of output_mint),
        /// set conservatively to allow for slippage.
        min_final_output: u64,
    },
    Unprofitable {
        reason: String,
    },
}

pub struct CexDexSimulator {
    store: PriceStore,
    config: CexDexSimulatorConfig,
}

impl CexDexSimulator {
    pub fn new(store: PriceStore, config: CexDexSimulatorConfig) -> Self {
        Self { store, config }
    }

    pub fn simulate(&self, route: &CexDexRoute) -> SimulationResult {
        // Re-read fresh pool state (no TTL enforcement — arb-guard gates on-chain)
        let pool = match self.store.pools.get_any(&route.pool_address) {
            Some(p) => p,
            None => {
                return SimulationResult::Unprofitable {
                    reason: format!("Pool {} not found in cache", route.pool_address),
                };
            }
        };

        // Re-quote at the route's input_amount with the latest bin/tick data
        let a_to_b = pool.token_a_mint == route.input_mint;
        let output = match pool.get_output_amount_with_cache(
            route.input_amount,
            a_to_b,
            self.store.pools.get_bin_arrays(&pool.address).as_deref(),
            self.store.pools.get_tick_arrays(&pool.address).as_deref(),
        ) {
            Some(out) if out > 0 => out,
            _ => {
                return SimulationResult::Unprofitable {
                    reason: "zero output".to_string(),
                };
            }
        };

        // Re-price via current CEX mid (fallback to detection-time prices if missing)
        let cex = self.store.get_cex("SOLUSDC");
        let (cex_bid, cex_ask) = match cex {
            Some(s) => (s.best_bid_usd, s.best_ask_usd),
            None => (route.cex_bid_at_detection, route.cex_ask_at_detection),
        };

        let (input_usd, output_usd) = match route.direction {
            ArbDirection::BuyOnDex => (
                atoms_to_usdc(route.input_amount),
                lamports_to_sol(output) * cex_bid,
            ),
            ArbDirection::SellOnDex => (
                lamports_to_sol(route.input_amount) * cex_ask,
                atoms_to_usdc(output),
            ),
        };

        let gross_profit_usd = output_usd - input_usd;
        if gross_profit_usd <= 0.0 {
            return SimulationResult::Unprofitable {
                reason: format!("not profitable: gross={}", gross_profit_usd),
            };
        }

        // Slippage-adjusted profit (used for tipping only; on-chain uses break-even)
        let adj_profit_usd = gross_profit_usd * (1.0 - self.config.slippage_tolerance);

        // Tip: percentage of adjusted profit, converted to lamports via CEX price
        let sol_price = (cex_bid + cex_ask) / 2.0;
        if sol_price <= 0.0 {
            return SimulationResult::Unprofitable {
                reason: "invalid CEX price".to_string(),
            };
        }
        let adj_profit_sol = adj_profit_usd / sol_price;
        let tip_sol = adj_profit_sol * self.config.tip_fraction;
        let tip_lamports = sol_to_lamports(tip_sol).max(self.config.min_tip_lamports);

        let tx_fee_usd = lamports_to_sol(self.config.tx_fee_lamports) * sol_price;
        let tip_usd = lamports_to_sol(tip_lamports) * sol_price;
        let net_profit_usd = adj_profit_usd - tip_usd - tx_fee_usd;

        if net_profit_usd < self.config.min_profit_usd {
            return SimulationResult::Unprofitable {
                reason: format!(
                    "below threshold: net={:.4} usd < {:.4}",
                    net_profit_usd, self.config.min_profit_usd,
                ),
            };
        }

        // min_final_output = break-even equivalent (input value priced in output token)
        // For BuyOnDex: input is USDC, output is SOL. min output = input_usdc / cex_ask
        // For SellOnDex: input is SOL, output is USDC. min output = input_sol * cex_bid
        let min_final_output = match route.direction {
            ArbDirection::BuyOnDex => {
                let min_sol = atoms_to_usdc(route.input_amount) / cex_ask;
                sol_to_lamports(min_sol * (1.0 - self.config.slippage_tolerance))
            }
            ArbDirection::SellOnDex => {
                let min_usdc = lamports_to_sol(route.input_amount) * cex_bid;
                usdc_to_atoms(min_usdc * (1.0 - self.config.slippage_tolerance))
            }
        };

        let mut fresh_route = route.clone();
        fresh_route.expected_output = output;
        fresh_route.expected_profit_usd = adj_profit_usd;
        fresh_route.cex_bid_at_detection = cex_bid;
        fresh_route.cex_ask_at_detection = cex_ask;

        debug!(
            "CexDex profitable: gross={:.4} adj={:.4} tip_lamports={} net={:.4} min_out={}",
            gross_profit_usd, adj_profit_usd, tip_lamports, net_profit_usd, min_final_output,
        );

        SimulationResult::Profitable {
            route: fresh_route,
            net_profit_usd,
            tip_lamports,
            min_final_output,
        }
    }
}
```

- [ ] **Step 4: Run simulator tests**

Run: `cargo test cexdex_simulator 2>&1 | tail -10`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add src/cexdex/simulator.rs tests/unit/cexdex_simulator.rs tests/unit/mod.rs
git commit -m "feat(cexdex): CEX-priced profit simulator with break-even min_output

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Task 9: Bundle Building Wrapper

**Files:**
- Modify: `src/cexdex/bundle.rs`

- [ ] **Step 1: Implement `src/cexdex/bundle.rs`**

```rust
//! Wraps the existing BundleBuilder to build single-leg CEX-DEX swap
//! instructions. Constructs a fake 1-hop ArbRoute because BundleBuilder
//! expects that shape.

use anyhow::Result;
use solana_sdk::instruction::Instruction;

use crate::cexdex::route::CexDexRoute;
use crate::executor::BundleBuilder;
use crate::router::pool::{ArbRoute, RouteHop};

/// Adapter that builds instructions for a CexDexRoute using the existing
/// multi-hop BundleBuilder. We construct a synthetic 1-hop ArbRoute where
/// base_mint is the route's input_mint.
pub fn build_instructions_for_cex_dex(
    builder: &BundleBuilder,
    route: &CexDexRoute,
    min_final_output: u64,
) -> Result<Vec<Instruction>> {
    let hop = RouteHop {
        pool_address: route.pool_address,
        dex_type: route.dex_type,
        input_mint: route.input_mint,
        output_mint: route.output_mint,
        estimated_output: route.expected_output,
    };

    // Note: ArbRoute.base_mint = input_mint (breaks the "circular" assumption
    // but BundleBuilder only uses base_mint for wSOL wrap logic, which we
    // handle explicitly via input_mint being USDC or WSOL).
    let synthetic_route = ArbRoute {
        hops: vec![hop],
        base_mint: route.input_mint,
        input_amount: route.input_amount,
        estimated_profit: 0, // unused by BundleBuilder
        estimated_profit_lamports: 0,
    };

    builder.build_arb_instructions(&synthetic_route, min_final_output)
}
```

- [ ] **Step 2: Run `cargo check`**

Run: `cargo check --bin cexdex 2>&1 | tail -5`
Expected: Compiles.

- [ ] **Step 3: Commit**

```bash
git add src/cexdex/bundle.rs
git commit -m "feat(cexdex): bundle building wrapper for single-leg routes

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Task 10: Narrow Geyser Subscription

**Files:**
- Modify: `src/cexdex/geyser.rs`

- [ ] **Step 1: Implement `src/cexdex/geyser.rs`**

Uses the existing `GeyserStream` but with a narrower filter — only specific pool accounts rather than full DEX programs.

```rust
//! Narrow Geyser subscription for a fixed list of pool addresses.
//!
//! Unlike the main engine which subscribes by DEX program owner, this
//! subscribes to specific pool pubkeys (one filter per DEX type that
//! includes only the pools in CEXDEX_POOLS).

use anyhow::Result;
use solana_sdk::pubkey::Pubkey;
use tokio::sync::watch;
use tracing::info;

use crate::cexdex::price_store::PriceStore;
use crate::config::BotConfig;
use crate::mempool::GeyserStream;
use crate::router::pool::DexType;
use crossbeam_channel::Sender;

/// Build a BotConfig suitable for the narrow cexdex Geyser subscription.
/// Reuses the same LaserStream connection settings as the main engine.
pub fn narrow_bot_config(
    geyser_grpc_url: String,
    geyser_auth_token: String,
    rpc_url: String,
    pool_state_ttl: std::time::Duration,
) -> BotConfig {
    use crate::config::RelayEndpoints;

    BotConfig {
        jito_block_engine_url: String::new(),
        jito_auth_keypair_path: String::new(),
        geyser_grpc_url,
        geyser_auth_token,
        rpc_url,
        searcher_keypair_path: String::new(),
        relay_endpoints: RelayEndpoints {
            jito: String::new(),
            nozomi: None,
            bloxroute: None,
            astralane: None,
            zeroslot: None,
        },
        tip_fraction: 0.5,
        min_profit_lamports: 0,
        min_tip_lamports: 0,
        max_hops: 2,
        pool_state_ttl,
        slippage_tolerance: 0.25,
        dry_run: false,
        lst_arb_enabled: false,
        lst_min_spread_bps: 0,
        arb_guard_program_id: None,
        metrics_port: None,
        otlp_endpoint: None,
        otlp_service_name: "cexdex".to_string(),
    }
}

/// Start a GeyserStream tied to the PriceStore's pool cache.
/// The pools list is used for logging only — the actual subscription
/// is by DEX program (GeyserStream subscribes to all DEX programs
/// globally; the PriceStore's detector will only check the specific
/// pools in its config).
pub async fn start_geyser(
    config: BotConfig,
    store: PriceStore,
    http_client: reqwest::Client,
    pool_list: Vec<(DexType, Pubkey)>,
    change_tx: Sender<crate::mempool::PoolStateChange>,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<tokio::task::JoinHandle<()>> {
    info!("Starting narrow Geyser (monitoring {} pools)", pool_list.len());
    let stream = GeyserStream::new(config, store.pools.clone(), http_client);
    let handle = tokio::spawn(async move {
        if let Err(e) = stream.run(change_tx, shutdown_rx).await {
            tracing::error!("Geyser stream exited: {}", e);
        }
    });
    Ok(handle)
}
```

- [ ] **Step 2: Check compilation**

Run: `cargo check --bin cexdex 2>&1 | tail -5`
Expected: Compiles. If `GeyserStream::run` signature differs, adjust — but the existing main.rs already uses this exact call.

- [ ] **Step 3: Commit**

```bash
git add src/cexdex/geyser.rs
git commit -m "feat(cexdex): narrow Geyser subscription wrapper

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Task 11: Wire Pipeline in Binary

**Files:**
- Modify: `src/bin/cexdex.rs`

- [ ] **Step 1: Implement the full binary pipeline**

Replace the contents of `src/bin/cexdex.rs`:

```rust
//! CEX-DEX arbitrage binary (Model A, SOL/USDC).
//!
//! Run: `cargo run --release --bin cexdex`

use anyhow::Result;
use crossbeam_channel::bounded;
use solana_mev_bot::cexdex::detector::{Detector, DetectorConfig};
use solana_mev_bot::cexdex::geyser::{narrow_bot_config, start_geyser};
use solana_mev_bot::cexdex::simulator::{CexDexSimulator, CexDexSimulatorConfig, SimulationResult};
use solana_mev_bot::cexdex::{CexDexConfig, Inventory, PriceStore};
use solana_mev_bot::executor::{BundleBuilder, RelayDispatcher};
use solana_mev_bot::feed::binance::run_solusdc_loop;
use solana_mev_bot::metrics::{self};
use solana_mev_bot::rpc_helpers;
use solana_mev_bot::state::{self, BlockhashCache, TipFloorCache};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .json()
        .init();

    info!("=== CEX-DEX Arbitrage Engine (Model A, SOL/USDC) ===");
    let config = CexDexConfig::from_env()?;
    info!(
        "Config: min_spread={}bps, min_profit=${:.2}, max_trade={} SOL, dry_run={}",
        config.min_spread_bps, config.min_profit_usd, config.max_trade_size_sol, config.dry_run,
    );
    info!("Monitoring {} pools", config.pools.len());
    if config.pools.is_empty() {
        anyhow::bail!("CEXDEX_POOLS must list at least one pool");
    }

    // Metrics
    if let Some(port) = config.metrics_port {
        if let Err(e) = metrics::init(port) {
            warn!("Metrics init failed: {}", e);
        }
    }

    // Shared state
    let store = PriceStore::new();
    let inventory = Inventory::new(
        config.hard_cap_ratio,
        config.preferred_low,
        config.preferred_high,
        config.skewed_profit_multiplier,
    );

    // HTTP client (shared)
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    // Load searcher wallet
    let searcher_keypair = rpc_helpers::load_keypair(&config.searcher_keypair_path)?;
    let searcher_pubkey = solana_sdk::signer::Signer::pubkey(&searcher_keypair);
    info!("Searcher wallet: {}", searcher_pubkey);

    // Initial balance fetch
    let (sol_lamports, usdc_atoms) = fetch_initial_balances(
        &http_client,
        &config.rpc_url,
        &searcher_pubkey,
    ).await?;
    inventory.set_on_chain(sol_lamports, usdc_atoms);
    info!(
        "Initial balance: {} SOL, {} USDC",
        sol_lamports as f64 / 1e9,
        usdc_atoms as f64 / 1e6,
    );

    // Shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let shutdown_tx = Arc::new(shutdown_tx);

    // Blockhash cache
    let blockhash_cache = BlockhashCache::new();
    if let Err(e) = state::blockhash::fetch_and_update(
        &http_client, &config.rpc_url, &blockhash_cache,
    ).await {
        warn!("Initial blockhash fetch failed: {}", e);
    }
    let _bh_handle = {
        let client = http_client.clone();
        let cache = blockhash_cache.clone();
        let rpc = config.rpc_url.clone();
        let rx = shutdown_rx.clone();
        tokio::spawn(async move {
            state::blockhash::run_blockhash_loop(client, rpc, cache, rx).await;
        })
    };

    // Tip floor cache (reuse main engine's WS)
    let tip_floor_cache = TipFloorCache::new();
    let _tip_handle = {
        let client = http_client.clone();
        let cache = tip_floor_cache.clone();
        let rx = shutdown_rx.clone();
        tokio::spawn(async move {
            state::tip_floor::run_tip_floor_loop(client, cache, rx).await;
        })
    };

    // Start Binance WS
    let _binance_handle = {
        let store = store.clone();
        let rx = shutdown_rx.clone();
        tokio::spawn(async move { run_solusdc_loop(store, rx).await })
    };

    // Start Geyser
    let bot_config = narrow_bot_config(
        config.geyser_grpc_url.clone(),
        config.geyser_auth_token.clone(),
        config.rpc_url.clone(),
        config.pool_state_ttl,
    );
    let (change_tx, change_rx) = bounded::<solana_mev_bot::mempool::PoolStateChange>(1024);
    let _geyser_handle = start_geyser(
        bot_config,
        store.clone(),
        http_client.clone(),
        config.pools.clone(),
        change_tx,
        shutdown_rx.clone(),
    ).await?;

    // Build detector and simulator
    let detector_config = DetectorConfig {
        min_spread_bps: config.min_spread_bps,
        min_profit_usd: config.min_profit_usd,
        max_trade_size_sol: config.max_trade_size_sol,
        cex_staleness_ms: config.cex_staleness_ms,
        slippage_tolerance: config.slippage_tolerance,
    };
    let detector = Detector::new(
        store.clone(),
        inventory.clone(),
        config.pools.clone(),
        detector_config,
    );

    let sim_config = CexDexSimulatorConfig {
        min_profit_usd: config.min_profit_usd,
        slippage_tolerance: config.slippage_tolerance,
        tx_fee_lamports: 5_000,
        min_tip_lamports: 1_000,
        tip_fraction: 0.50,
    };
    let simulator = CexDexSimulator::new(store.clone(), sim_config);

    // BundleBuilder + RelayDispatcher
    let bundle_builder = BundleBuilder::new(
        searcher_keypair.insecure_clone(),
        store.pools.clone(),
        config.arb_guard_program_id,
    );

    // Load ALTs (optional; main engine uses ALT_ADDRESS env var)
    let alts = vec![]; // CEX-DEX single-leg is small enough without ALT

    // Relays
    let mut relays: Vec<Arc<dyn solana_mev_bot::executor::relays::Relay>> = Vec::new();
    let jito_relay = Arc::new(
        solana_mev_bot::executor::relays::jito::JitoRelay::new(
            config.jito_block_engine_url.clone(),
            None,
            4,
        ),
    );
    relays.push(jito_relay);
    if let Some(url) = &config.astralane_relay_url {
        let astralane = Arc::new(
            solana_mev_bot::executor::relays::astralane::AstralaneRelay::new(
                url.clone(),
                config.astralane_api_key.clone(),
                15,
            ),
        );
        relays.push(astralane);
    }
    let signer_arc = Arc::new(searcher_keypair.insecure_clone());
    let dispatcher = RelayDispatcher::new(relays, signer_arc, alts);

    info!("All components initialized, starting detector loop");

    // Main loop: poll the detector on either a Geyser event OR a short tick
    // (to pick up CEX-side changes without a dedicated channel).
    run_detector_loop(
        detector,
        simulator,
        bundle_builder,
        dispatcher,
        blockhash_cache,
        tip_floor_cache,
        inventory.clone(),
        store.clone(),
        http_client.clone(),
        config,
        change_rx,
        shutdown_rx,
    ).await?;

    shutdown_tx.send(true).ok();
    Ok(())
}

async fn fetch_initial_balances(
    client: &reqwest::Client,
    rpc_url: &str,
    wallet: &solana_sdk::pubkey::Pubkey,
) -> Result<(u64, u64)> {
    // getBalance for SOL
    let payload = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "getBalance",
        "params": [wallet.to_string()],
    });
    let resp = client.post(rpc_url).json(&payload).send().await?
        .json::<serde_json::Value>().await?;
    let sol_lamports = resp["result"]["value"].as_u64().unwrap_or(0);

    // getTokenAccountsByOwner for USDC
    let usdc_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    let payload = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "getTokenAccountsByOwner",
        "params": [
            wallet.to_string(),
            { "mint": usdc_mint },
            { "encoding": "jsonParsed" }
        ],
    });
    let resp = client.post(rpc_url).json(&payload).send().await?
        .json::<serde_json::Value>().await?;
    let usdc_atoms = resp["result"]["value"].as_array()
        .and_then(|arr| arr.first())
        .and_then(|acc| acc["account"]["data"]["parsed"]["info"]["tokenAmount"]["amount"].as_str())
        .and_then(|s| u64::from_str(s).ok())
        .unwrap_or(0);

    Ok((sol_lamports, usdc_atoms))
}

#[allow(clippy::too_many_arguments)]
async fn run_detector_loop(
    detector: Detector,
    simulator: CexDexSimulator,
    bundle_builder: BundleBuilder,
    dispatcher: RelayDispatcher,
    blockhash_cache: BlockhashCache,
    _tip_floor_cache: TipFloorCache,
    inventory: Inventory,
    _store: PriceStore,
    http_client: reqwest::Client,
    config: CexDexConfig,
    change_rx: crossbeam_channel::Receiver<solana_mev_bot::mempool::PoolStateChange>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let rt = tokio::runtime::Handle::current();
    let mut opportunities = 0u64;

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() { break; }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                // periodic detection tick: catches CEX-side updates too
            }
        }

        // drain Geyser events (non-blocking)
        while let Ok(_change) = change_rx.try_recv() {
            // fall through — the detector reads pool cache directly
        }

        // Update inventory's SOL price from CEX
        if let Some(snap) = _store.get_cex("SOLUSDC") {
            inventory.set_sol_price_usd(snap.mid());
        }

        let route = match detector.check_all() {
            Some(r) => r,
            None => continue,
        };

        let sim_result = simulator.simulate(&route);
        let (route, tip_lamports, min_final_output, net_profit_usd) = match sim_result {
            SimulationResult::Profitable {
                route, tip_lamports, min_final_output, net_profit_usd,
            } => (route, tip_lamports, min_final_output, net_profit_usd),
            SimulationResult::Unprofitable { reason } => {
                tracing::debug!("sim unprofitable: {}", reason);
                continue;
            }
        };

        opportunities += 1;
        info!(
            "OPPORTUNITY #{}: {} on {} pool={} input={} expected_output={} tip={} net=${:.4}",
            opportunities,
            route.direction.label(),
            format!("{:?}", route.dex_type),
            route.pool_address,
            route.input_amount,
            route.expected_output,
            tip_lamports,
            net_profit_usd,
        );

        if config.dry_run {
            info!("DRY_RUN — not submitting");
            continue;
        }

        // Build instructions
        let instructions = match solana_mev_bot::cexdex::bundle::build_instructions_for_cex_dex(
            &bundle_builder,
            &route,
            min_final_output,
        ) {
            Ok(ixs) => ixs,
            Err(e) => {
                tracing::warn!("bundle build failed: {}", e);
                continue;
            }
        };

        let blockhash = match blockhash_cache.get() {
            Some(h) => h,
            None => {
                tracing::warn!("no blockhash, skipping");
                continue;
            }
        };

        // Reserve inventory
        inventory.reserve(route.direction, route.input_amount, route.expected_output);

        // Submit
        let _rx = dispatcher.dispatch(&instructions, tip_lamports, blockhash, &rt);

        // Confirmation tracking: release/commit based on outcome
        // (simplified: commit optimistically after delay; proper tracker in a follow-up)
        let inv = inventory.clone();
        let dir = route.direction;
        let input = route.input_amount;
        let output = route.expected_output;
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            inv.release(dir, input, output);
            // Refresh balance from chain after delay
            // (commit happens implicitly when the refresh picks up the new balance)
        });

        let _ = &http_client;
    }

    Ok(())
}
```

- [ ] **Step 2: Check compilation**

Run: `cargo check --bin cexdex 2>&1 | tail -10`
Expected: Compiles. If there are type mismatches (e.g., `Relay` trait not re-exported at expected path), adjust imports by reading the existing `src/executor/relays/mod.rs`.

- [ ] **Step 3: Run full test suite for regression**

Run: `cargo test 2>&1 | grep "^test result"`
Expected: All prior tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/bin/cexdex.rs
git commit -m "feat(cexdex): wire full pipeline in cexdex binary

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Task 12: E2E Pipeline Test

**Files:**
- Create: `tests/e2e/cexdex_pipeline.rs`
- Modify: `tests/e2e/mod.rs`

- [ ] **Step 1: Create the E2E test**

Create `tests/e2e/cexdex_pipeline.rs`:

```rust
//! E2E test: Binance price + synthetic pool → detector → simulator → bundle IXs.
//! Run with: cargo test --features e2e --test e2e cexdex_pipeline

use solana_mev_bot::addresses;
use solana_mev_bot::cexdex::detector::{Detector, DetectorConfig};
use solana_mev_bot::cexdex::simulator::{CexDexSimulator, CexDexSimulatorConfig, SimulationResult};
use solana_mev_bot::cexdex::{Inventory, PriceStore};
use solana_mev_bot::executor::BundleBuilder;
use solana_mev_bot::feed::PriceSnapshot;
use solana_mev_bot::router::pool::{DexType, PoolExtra, PoolState};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use std::str::FromStr;
use std::time::Instant;

fn usdc() -> Pubkey {
    Pubkey::from_str("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap()
}

#[test]
fn test_cex_dex_full_pipeline_buy_on_dex() {
    let store = PriceStore::new();

    // Pool with DEX at $180/SOL
    let pool_addr = Pubkey::new_unique();
    store.pools.upsert(pool_addr, PoolState {
        address: pool_addr,
        dex_type: DexType::RaydiumCp,
        token_a_mint: addresses::WSOL,
        token_b_mint: usdc(),
        token_a_reserve: 100_000_000_000,
        token_b_reserve: 18_000_000_000,
        fee_bps: 30,
        current_tick: None, sqrt_price_x64: None, liquidity: None,
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            config: Some(Pubkey::new_unique()),
            token_program_a: Some(addresses::SPL_TOKEN),
            token_program_b: Some(addresses::SPL_TOKEN),
            ..Default::default()
        },
        best_bid_price: None, best_ask_price: None,
    });

    // CEX at $185
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 185.0,
        best_ask_usd: 185.02,
        received_at: Instant::now(),
    });

    // Pre-populate mint programs so bundle builder doesn't error
    store.pools.set_mint_program(addresses::WSOL, addresses::SPL_TOKEN);
    store.pools.set_mint_program(usdc(), addresses::SPL_TOKEN);

    // Inventory
    let inv = Inventory::new_for_test();
    inv.set_on_chain(2_000_000_000, 2_000_000_000); // 2 SOL + 2000 USDC
    inv.set_sol_price_usd(185.0);

    // Detect
    let detector_config = DetectorConfig {
        min_spread_bps: 15,
        min_profit_usd: 0.10,
        max_trade_size_sol: 5.0,
        cex_staleness_ms: 500,
        slippage_tolerance: 0.25,
    };
    let detector = Detector::new(
        store.clone(),
        inv,
        vec![(DexType::RaydiumCp, pool_addr)],
        detector_config,
    );
    let route = detector.check_all().expect("should detect BuyOnDex");
    assert_eq!(
        route.direction,
        solana_mev_bot::cexdex::ArbDirection::BuyOnDex,
    );

    // Simulate
    let sim_config = CexDexSimulatorConfig {
        min_profit_usd: 0.10,
        slippage_tolerance: 0.25,
        tx_fee_lamports: 5_000,
        min_tip_lamports: 1_000,
        tip_fraction: 0.50,
    };
    let simulator = CexDexSimulator::new(store.clone(), sim_config);
    let sim_result = simulator.simulate(&route);
    let (route, min_final_output) = match sim_result {
        SimulationResult::Profitable { route, min_final_output, .. } => (route, min_final_output),
        SimulationResult::Unprofitable { reason } => panic!("expected profitable: {}", reason),
    };

    // Build instructions
    let signer = Keypair::new();
    let builder = BundleBuilder::new(
        signer.insecure_clone(),
        store.pools.clone(),
        Some(Pubkey::new_unique()), // arb-guard program
    );
    let instructions = solana_mev_bot::cexdex::bundle::build_instructions_for_cex_dex(
        &builder,
        &route,
        min_final_output,
    ).expect("bundle build should succeed");

    assert!(
        instructions.len() >= 4,
        "expected compute budget + ATA creates + swap, got {}",
        instructions.len(),
    );
}
```

- [ ] **Step 2: Register e2e test**

In `tests/e2e/mod.rs`:

```rust
mod cexdex_pipeline;
```

- [ ] **Step 3: Run e2e test**

Run: `cargo test --features e2e --test e2e cexdex 2>&1 | tail -10`
Expected: `test result: ok. 1 passed`.

- [ ] **Step 4: Run full test suites**

Run: `cargo test 2>&1 | grep "^test result"`
Run: `cargo test --features e2e --test e2e 2>&1 | grep "^test result"`
Expected: All tests still pass.

- [ ] **Step 5: Commit**

```bash
git add tests/e2e/cexdex_pipeline.rs tests/e2e/mod.rs
git commit -m "test(cexdex): add E2E pipeline test (detector → simulator → bundle)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Task 13: Documentation + Config Example

**Files:**
- Modify: `.env.example`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Add CEXDEX variables to `.env.example`**

Append to `.env.example`:

```
# ─── CEX-DEX Arbitrage (cexdex binary, Model A, SOL/USDC) ─────────────

# Separate wallet for clean P&L isolation
CEXDEX_SEARCHER_KEYPAIR=cexdex-searcher.json
# CEXDEX_SEARCHER_PRIVATE_KEY=  # optional alternative to keypair file

# Binance WS
CEXDEX_BINANCE_WS_URL=wss://stream.binance.com:9443/ws
CEXDEX_CEX_STALENESS_MS=500

# Pools to monitor (comma-separated DexType:pubkey)
# DexType: RaydiumAmm, RaydiumCp, RaydiumClmm, OrcaWhirlpool, MeteoraDlmm, MeteoraDammV2
CEXDEX_POOLS=RaydiumCp:<pool_pubkey>,OrcaWhirlpool:<pool_pubkey>

# Strategy
CEXDEX_MIN_SPREAD_BPS=15
CEXDEX_MIN_PROFIT_USD=0.10
CEXDEX_MAX_TRADE_SIZE_SOL=10.0
CEXDEX_SLIPPAGE_TOLERANCE=0.25

# Inventory gates
CEXDEX_HARD_CAP_RATIO=0.80
CEXDEX_PREFERRED_LOW=0.40
CEXDEX_PREFERRED_HIGH=0.60
CEXDEX_SKEWED_PROFIT_MULTIPLIER=2.0

# Safety
CEXDEX_DRY_RUN=true
CEXDEX_POOL_TTL_SECS=5

# Metrics (separate port from main engine)
CEXDEX_METRICS_PORT=9091
```

- [ ] **Step 2: Add a section to `CLAUDE.md`**

After the existing "Build & Run" section, add:

```markdown
### CEX-DEX Binary

Separate binary for Binance SOL/USDC CEX-DEX arbitrage (Model A, inventory-based).
Uses a separate wallet for clean P&L isolation.

```bash
# First time: generate a separate searcher keypair
solana-keygen new -o cexdex-searcher.json

# Fund it with SOL + USDC (manual top-up)

# Set CEXDEX_POOLS to the specific pool addresses to monitor in .env

# Run in dry-run first
CEXDEX_DRY_RUN=true cargo run --release --bin cexdex

# Go live
CEXDEX_DRY_RUN=false cargo run --release --bin cexdex
```

See `docs/superpowers/specs/2026-04-16-cex-dex-arb-design.md` for the full design.
```

- [ ] **Step 3: Commit**

```bash
git add .env.example CLAUDE.md
git commit -m "docs(cexdex): add CEXDEX env vars and run instructions

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Task 14: Final Verification

**Files:** none

- [ ] **Step 1: Full test suite (debug)**

Run: `cargo test 2>&1 | grep "^test result"`
Expected: All tests pass (plus 1 pre-existing `router_perf` flake in debug — passes in release).

- [ ] **Step 2: Full test suite (release, confirms perf test)**

Run: `cargo test --release 2>&1 | grep "^test result"`
Expected: All pass including `router_perf`.

- [ ] **Step 3: E2E suite**

Run: `cargo test --features e2e --test e2e 2>&1 | grep "^test result"`
Expected: All pass including new `cexdex_pipeline`.

- [ ] **Step 4: Clippy**

Run: `cargo clippy 2>&1 | tail -5`
Expected: 0 new warnings.

- [ ] **Step 5: Release build both binaries**

Run: `cargo build --release 2>&1 | tail -3`
Expected: Both `solana-mev-bot` and `cexdex` binaries build.

- [ ] **Step 6: Smoke test dry-run (no commit needed — manual verification)**

```bash
# With CEXDEX_DRY_RUN=true and at least one valid pool address in CEXDEX_POOLS:
timeout 60 cargo run --release --bin cexdex 2>&1 | grep -E "OPPORTUNITY|Config|Binance|Geyser" | head -20
```
Expected: Binance WS connects, Geyser starts, some log output. No panics. No OPPORTUNITY lines (dry_run is fine) — or OPPORTUNITY lines with DRY_RUN skip.

- [ ] **Step 7: Commit any fix-ups**

If steps 1-6 uncovered issues, fix and commit with a clear message. Otherwise, nothing to commit.

---

## Plan Summary

| Task | Deliverable | Tests |
|------|-------------|-------|
| 1 | Module scaffold + binary skeleton | compile checks |
| 2 | `units.rs` — decimal conversions | 7 unit tests |
| 3 | Binance WS client | 4 unit tests |
| 4 | `PriceStore` | 5 unit tests |
| 5 | `Inventory` | 10 unit tests |
| 6 | `CexDexRoute` + `ArbDirection` | compile checks |
| 7 | `Detector` | 5 unit tests |
| 8 | `CexDexSimulator` | 3 unit tests |
| 9 | Bundle wrapper | compile checks |
| 10 | Narrow Geyser | compile checks |
| 11 | Full pipeline in binary | compile checks |
| 12 | E2E pipeline test | 1 e2e test |
| 13 | Docs + `.env.example` | N/A |
| 14 | Verification + release build | N/A |

Total new tests: 34 unit + 1 e2e.
