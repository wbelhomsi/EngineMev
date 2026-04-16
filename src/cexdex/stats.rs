//! Stats collector for CEX-DEX dry-run analysis.
//!
//! Accumulates every opportunity (detected AND rejected) and writes a JSON
//! file on shutdown so we can offline-analyze optimal sizing, skew thresholds,
//! pool performance, etc.

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Single opportunity record (one line per detection → simulation outcome).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpportunityRecord {
    pub ts_ms: u64,
    pub pool: String,
    pub dex: String,
    pub direction: String,
    pub input_amount: u64,
    pub input_mint: String,
    pub expected_output: u64,
    pub output_mint: String,
    pub cex_bid: f64,
    pub cex_ask: f64,
    pub cex_mid: f64,
    /// USD profit from detector (slippage-adjusted)
    pub detected_profit_usd: f64,
    /// USD profit from simulator, after tip + fee (None if simulator rejected)
    pub sim_net_profit_usd: Option<f64>,
    /// Tip in lamports (None if simulator rejected)
    pub sim_tip_lamports: Option<u64>,
    /// min_final_output enforced on-chain (None if simulator rejected)
    pub sim_min_final_output: Option<u64>,
    /// Why the simulator rejected this (None if Profitable)
    pub sim_reject_reason: Option<String>,
    /// Inventory ratio at the moment of detection (0.0 = all USDC, 1.0 = all SOL)
    pub inventory_ratio: f64,
    /// SOL balance at detection (available, after reservations)
    pub inv_sol_available: u64,
    pub inv_usdc_available: u64,
    /// Was it submitted (false if dry_run, inventory-capped post-detection, or build failed)
    pub submitted: bool,
}

/// Summary emitted once on shutdown.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RunSummary {
    pub start_ts_ms: u64,
    pub end_ts_ms: u64,
    pub duration_secs: u64,
    pub total_detections: u64,
    pub sim_profitable: u64,
    pub sim_rejected: u64,
    pub submitted: u64,
    pub by_direction: std::collections::HashMap<String, u64>,
    pub by_pool: std::collections::HashMap<String, u64>,
    pub by_reject_reason: std::collections::HashMap<String, u64>,
    /// Final inventory state
    pub final_inventory_ratio: f64,
    pub final_sol_lamports: u64,
    pub final_usdc_atoms: u64,
}

pub struct StatsCollector {
    inner: Mutex<StatsInner>,
}

struct StatsInner {
    start_ts_ms: u64,
    records: Vec<OpportunityRecord>,
}

impl StatsCollector {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(StatsInner {
                start_ts_ms: now_ms(),
                records: Vec::with_capacity(8192),
            }),
        }
    }

    pub fn record(&self, rec: OpportunityRecord) {
        if let Ok(mut g) = self.inner.lock() {
            g.records.push(rec);
        }
    }

    /// Compute summary + write both raw records and summary to disk.
    /// Writes two files:
    ///   - `<base>_records.jsonl` — newline-delimited per-opportunity records
    ///   - `<base>_summary.json` — single summary blob
    pub fn finalize_to_disk(
        &self,
        base_path: impl AsRef<Path>,
        final_ratio: f64,
        final_sol: u64,
        final_usdc: u64,
    ) -> anyhow::Result<RunSummary> {
        let guard = self.inner.lock().map_err(|e| anyhow::anyhow!("mutex poisoned: {}", e))?;
        let end_ts = now_ms();
        let mut summary = RunSummary {
            start_ts_ms: guard.start_ts_ms,
            end_ts_ms: end_ts,
            duration_secs: (end_ts - guard.start_ts_ms) / 1000,
            total_detections: guard.records.len() as u64,
            final_inventory_ratio: final_ratio,
            final_sol_lamports: final_sol,
            final_usdc_atoms: final_usdc,
            ..Default::default()
        };

        for r in &guard.records {
            if r.sim_net_profit_usd.is_some() {
                summary.sim_profitable += 1;
            } else {
                summary.sim_rejected += 1;
                if let Some(reason) = &r.sim_reject_reason {
                    *summary.by_reject_reason.entry(reason.clone()).or_insert(0) += 1;
                }
            }
            if r.submitted {
                summary.submitted += 1;
            }
            *summary.by_direction.entry(r.direction.clone()).or_insert(0) += 1;
            *summary.by_pool.entry(r.pool.clone()).or_insert(0) += 1;
        }

        let base = base_path.as_ref();
        let records_path = base.with_extension("records.jsonl");
        let summary_path = base.with_extension("summary.json");

        // Write records as newline-delimited JSON
        let mut records_txt = String::with_capacity(guard.records.len() * 512);
        for r in &guard.records {
            records_txt.push_str(&serde_json::to_string(r)?);
            records_txt.push('\n');
        }
        std::fs::write(&records_path, records_txt)?;

        // Write summary
        let summary_json = serde_json::to_string_pretty(&summary)?;
        std::fs::write(&summary_path, summary_json)?;

        Ok(summary)
    }
}

impl Default for StatsCollector {
    fn default() -> Self {
        Self::new()
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn pubkey_to_str(p: &Pubkey) -> String {
    p.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn mk_record(direction: &str, net: Option<f64>, reason: Option<&str>) -> OpportunityRecord {
        OpportunityRecord {
            ts_ms: now_ms(),
            pool: "POOL1".to_string(),
            dex: "RaydiumCp".to_string(),
            direction: direction.to_string(),
            input_amount: 1_000_000,
            input_mint: "USDC".to_string(),
            expected_output: 5_000,
            output_mint: "SOL".to_string(),
            cex_bid: 185.0,
            cex_ask: 185.02,
            cex_mid: 185.01,
            detected_profit_usd: 0.5,
            sim_net_profit_usd: net,
            sim_tip_lamports: net.map(|_| 1_000),
            sim_min_final_output: net.map(|_| 5_000),
            sim_reject_reason: reason.map(|s| s.to_string()),
            inventory_ratio: 0.5,
            inv_sol_available: 1_000_000_000,
            inv_usdc_available: 1_000_000_000,
            submitted: net.is_some(),
        }
    }

    #[test]
    fn test_record_and_summarize() {
        let c = StatsCollector::new();
        c.record(mk_record("buy_on_dex", Some(0.3), None));
        c.record(mk_record("buy_on_dex", None, Some("stale")));
        c.record(mk_record("sell_on_dex", Some(0.1), None));

        let dir = tempdir().unwrap();
        let base = dir.path().join("run");
        let summary = c.finalize_to_disk(&base, 0.5, 1_000_000_000, 1_000_000_000).unwrap();

        assert_eq!(summary.total_detections, 3);
        assert_eq!(summary.sim_profitable, 2);
        assert_eq!(summary.sim_rejected, 1);
        assert_eq!(summary.submitted, 2);
        assert_eq!(summary.by_direction.get("buy_on_dex").copied().unwrap_or(0), 2);
        assert_eq!(summary.by_direction.get("sell_on_dex").copied().unwrap_or(0), 1);
        assert_eq!(summary.by_reject_reason.get("stale").copied().unwrap_or(0), 1);

        // Verify files written
        assert!(base.with_extension("records.jsonl").exists());
        assert!(base.with_extension("summary.json").exists());

        // Records file has 3 lines
        let records_txt = std::fs::read_to_string(base.with_extension("records.jsonl")).unwrap();
        assert_eq!(records_txt.lines().count(), 3);
    }

    #[test]
    fn test_empty_collector_produces_empty_summary() {
        let c = StatsCollector::new();
        let dir = tempdir().unwrap();
        let base = dir.path().join("empty");
        let summary = c.finalize_to_disk(&base, 0.5, 0, 0).unwrap();
        assert_eq!(summary.total_detections, 0);
        assert_eq!(summary.sim_profitable, 0);
        assert_eq!(summary.sim_rejected, 0);
    }
}
