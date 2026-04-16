#!/usr/bin/env python3
"""Analyze a CEX-DEX dry-run for optimal parameters.

Usage:
    python3 scripts/analyze_cexdex_run.py /tmp/cexdex-analysis

Reads <base>.records.jsonl and <base>.summary.json, produces a report
recommending:
- Optimal `CEXDEX_MIN_SPREAD_BPS`
- Optimal `CEXDEX_MAX_TRADE_SIZE_SOL`
- Optimal `CEXDEX_HARD_CAP_RATIO` and skew thresholds
- Per-pool profitability ranking
- Per-direction stats
"""

import json
import sys
from collections import Counter, defaultdict
from pathlib import Path


def load_records(base: Path):
    records_path = base.with_suffix(".records.jsonl")
    summary_path = base.with_suffix(".summary.json")
    records = []
    if records_path.exists():
        with records_path.open() as f:
            for line in f:
                line = line.strip()
                if line:
                    records.append(json.loads(line))
    summary = {}
    if summary_path.exists():
        with summary_path.open() as f:
            summary = json.load(f)
    return records, summary


def quantile(values, q):
    if not values:
        return 0.0
    s = sorted(values)
    idx = min(int(len(s) * q), len(s) - 1)
    return s[idx]


def analyze(base: Path):
    records, summary = load_records(base)
    if not records:
        print("No records found. Did the run produce data?")
        return

    print("=" * 72)
    print(f"CEX-DEX Run Analysis — base={base}")
    print("=" * 72)
    print()

    # Overall stats
    n = len(records)
    profitable = [r for r in records if r.get("sim_net_profit_usd") is not None]
    rejected = [r for r in records if r.get("sim_net_profit_usd") is None]

    print("## Overall")
    print(f"  Duration: {summary.get('duration_secs', 0)}s ({summary.get('duration_secs', 0) / 60:.1f} min)")
    print(f"  Total detections:  {n}")
    print(f"  Simulator profitable: {len(profitable)} ({100 * len(profitable) / n:.1f}%)")
    print(f"  Simulator rejected:   {len(rejected)} ({100 * len(rejected) / n:.1f}%)")
    print(f"  Detections per minute: {n / max(1, summary.get('duration_secs', 1) / 60):.1f}")
    print()

    # Reject reasons
    print("## Rejection reasons")
    reason_counts = Counter(r.get("sim_reject_reason") for r in rejected if r.get("sim_reject_reason"))
    # Aggregate by reason prefix (reason text often has values; strip them)
    aggregated = Counter()
    for reason, count in reason_counts.items():
        # Normalize: take first 40 chars or up to first ':'
        key = reason.split(":")[0][:50] if reason else "unknown"
        aggregated[key] += count
    for reason, count in aggregated.most_common(10):
        print(f"  {count:>5}× {reason}")
    print()

    # Direction breakdown
    print("## By direction")
    by_dir = defaultdict(lambda: {"total": 0, "profitable": 0})
    for r in records:
        d = r.get("direction", "?")
        by_dir[d]["total"] += 1
        if r.get("sim_net_profit_usd") is not None:
            by_dir[d]["profitable"] += 1
    for d, s in by_dir.items():
        pct = 100 * s["profitable"] / max(1, s["total"])
        print(f"  {d}: {s['total']} total, {s['profitable']} profitable ({pct:.1f}%)")
    print()

    # Per-pool performance
    print("## Per-pool detection rate")
    by_pool = defaultdict(lambda: {"total": 0, "profitable": 0, "net_profits": []})
    for r in records:
        p = r.get("pool", "?")[:12]
        dex = r.get("dex", "?")
        key = f"{dex}:{p}"
        by_pool[key]["total"] += 1
        net = r.get("sim_net_profit_usd")
        if net is not None:
            by_pool[key]["profitable"] += 1
            by_pool[key]["net_profits"].append(net)
    for key, s in sorted(by_pool.items(), key=lambda kv: -kv[1]["profitable"]):
        profs = s["net_profits"]
        avg = sum(profs) / len(profs) if profs else 0.0
        tot = sum(profs) if profs else 0.0
        print(f"  {key}: {s['total']:>5} detected, {s['profitable']:>4} profitable | avg=${avg:.4f} | total=${tot:.2f}")
    print()

    if not profitable:
        print("No profitable simulations. Check CEXDEX_MIN_SPREAD_BPS/MIN_PROFIT_USD and pool liquidity.")
        return

    # Profit distribution
    print("## Profit distribution (profitable only)")
    net_profits = [r["sim_net_profit_usd"] for r in profitable]
    for q, label in [(0.1, "p10"), (0.5, "p50"), (0.9, "p90"), (0.99, "p99")]:
        print(f"  {label}: ${quantile(net_profits, q):.4f}")
    print(f"  max: ${max(net_profits):.4f}")
    print(f"  total: ${sum(net_profits):.2f}")
    print()

    # Trade size distribution
    print("## Trade size distribution (input, profitable only)")
    # Need to convert atoms/lamports to sol based on direction
    sizes_usd = []
    for r in profitable:
        if r["direction"] == "buy_on_dex":
            # input is USDC atoms (6 decimals)
            sizes_usd.append(r["input_amount"] / 1e6)
        else:
            # input is SOL lamports → convert via cex mid
            sizes_usd.append(r["input_amount"] / 1e9 * r["cex_mid"])
    for q, label in [(0.1, "p10"), (0.5, "p50"), (0.9, "p90")]:
        print(f"  {label}: ${quantile(sizes_usd, q):.2f}")
    print(f"  max: ${max(sizes_usd):.2f}")
    print()

    # Spread distribution (what bps of edge did we actually see?)
    print("## Spread distribution (all records, detected spread in bps)")
    spreads = []
    for r in records:
        cex_mid = r.get("cex_mid", 0)
        if cex_mid <= 0:
            continue
        # Back out the DEX spot from input/output and cex price
        if r["direction"] == "buy_on_dex":
            # sold USDC, got SOL. dex_price = usdc_in / sol_out
            sol_out = r["expected_output"] / 1e9
            usdc_in = r["input_amount"] / 1e6
            if sol_out > 0:
                dex_price = usdc_in / sol_out
                spread_bps = abs(dex_price - cex_mid) / cex_mid * 10_000
                spreads.append(spread_bps)
        else:
            sol_in = r["input_amount"] / 1e9
            usdc_out = r["expected_output"] / 1e6
            if sol_in > 0:
                dex_price = usdc_out / sol_in
                spread_bps = abs(dex_price - cex_mid) / cex_mid * 10_000
                spreads.append(spread_bps)
    if spreads:
        for q, label in [(0.1, "p10"), (0.5, "p50"), (0.9, "p90"), (0.99, "p99")]:
            print(f"  {label}: {quantile(spreads, q):.1f} bps")
        print(f"  max: {max(spreads):.1f} bps")
    print()

    # Tip distribution
    print("## Tip distribution (profitable only)")
    tips = [r.get("sim_tip_lamports") for r in profitable if r.get("sim_tip_lamports")]
    if tips:
        for q, label in [(0.5, "p50"), (0.9, "p90")]:
            print(f"  {label}: {quantile(tips, q):,} lamports")
        print(f"  max: {max(tips):,} lamports")
    print()

    # Inventory analysis
    print("## Inventory ratio at detection (all records)")
    ratios = [r["inventory_ratio"] for r in records]
    for q, label in [(0.1, "p10"), (0.5, "p50"), (0.9, "p90")]:
        print(f"  {label}: {quantile(ratios, q):.3f}")
    print()

    # Recommendations
    print("=" * 72)
    print("## RECOMMENDATIONS")
    print("=" * 72)

    if spreads:
        # Find the bps threshold that keeps 80% of profitable opportunities
        prof_spreads = []
        for r in profitable:
            cex_mid = r.get("cex_mid", 0)
            if cex_mid <= 0:
                continue
            if r["direction"] == "buy_on_dex":
                sol_out = r["expected_output"] / 1e9
                usdc_in = r["input_amount"] / 1e6
                if sol_out > 0:
                    prof_spreads.append(abs(usdc_in / sol_out - cex_mid) / cex_mid * 10_000)
            else:
                sol_in = r["input_amount"] / 1e9
                usdc_out = r["expected_output"] / 1e6
                if sol_in > 0:
                    prof_spreads.append(abs(usdc_out / sol_in - cex_mid) / cex_mid * 10_000)
        if prof_spreads:
            p20 = quantile(sorted(prof_spreads), 0.20)
            print(f"CEXDEX_MIN_SPREAD_BPS: {int(p20)} (keeps 80% of profitable opps)")

    if sizes_usd:
        p90 = quantile(sizes_usd, 0.90)
        # Max trade size in SOL at current prices (use median cex_mid as proxy)
        cex_mids = [r.get("cex_mid", 0) for r in profitable if r.get("cex_mid", 0) > 0]
        if cex_mids:
            med_price = sorted(cex_mids)[len(cex_mids) // 2]
            p90_sol = p90 / med_price
            print(f"CEXDEX_MAX_TRADE_SIZE_SOL: {p90_sol:.2f} (covers 90% of sized opps, at ${med_price:.2f}/SOL)")

    if net_profits:
        p10 = quantile(net_profits, 0.10)
        print(f"CEXDEX_MIN_PROFIT_USD: {p10:.3f} (keeps 90% of profitable sims)")

    # Skew / inventory
    if ratios:
        # If most detections happened near one ratio, loosen the preferred band there
        med_ratio = quantile(ratios, 0.5)
        print(f"Inventory ratio (median): {med_ratio:.3f} — "
              + ("start 50/50 (add USDC)" if med_ratio > 0.8 else
                 "start 50/50 (add SOL)" if med_ratio < 0.2 else
                 "already reasonably balanced"))

    # Best pools
    if by_pool:
        ranked = sorted(by_pool.items(), key=lambda kv: -sum(kv[1]["net_profits"] or [0]))
        print()
        print("Top pools by total net profit:")
        for key, s in ranked[:3]:
            tot = sum(s["net_profits"] or [0])
            print(f"  {key}: ${tot:.2f} total")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print("Usage: analyze_cexdex_run.py <base_path>")
        print("Example: analyze_cexdex_run.py /tmp/cexdex-analysis")
        sys.exit(1)
    analyze(Path(sys.argv[1]))
