#!/usr/bin/env python3
"""Analyze cex_arb_probe JSONL output.

Builds time-aligned (100ms bucket) top-of-book from Binance + Bybit per
symbol, finds cross-exchange gaps, measures their persistence and
executable depth.

A "gap" is defined as:
    max(bid_A - ask_B, bid_B - ask_A) / mid  >  threshold_bps

Executable size at the gap is min(bid_qty_side, ask_qty_side) — how
many base-token units you could trade in one shot before the gap closes
just from you hitting it.

Run:
    python3 scripts/analyze_cex_arb_probe.py /tmp/cex_arb_probe.jsonl \\
        --gap-bps 7 --bucket-ms 100 --fee-bps 7
"""

import argparse
import json
import statistics
import sys
from collections import defaultdict


def load(path):
    ticks_by_symbol = defaultdict(list)
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                r = json.loads(line)
            except json.JSONDecodeError:
                continue
            ticks_by_symbol[r['symbol']].append(r)
    return ticks_by_symbol


def align_to_buckets(ticks, bucket_ms, freshness_ms=500):
    """Return list of (bucket_start_ms, {venue: (bid, bid_qty, ask, ask_qty)}).

    A venue is ONLY included in a bucket if its latest tick is at most
    `freshness_ms` older than the bucket end. Carried-over stale state
    would otherwise count as "persistent gap" when one venue's feed
    just stopped updating, producing phantom events.
    """
    ticks.sort(key=lambda r: r['ts_ms'])
    if not ticks:
        return []

    t0 = ticks[0]['ts_ms']
    latest = {}  # venue -> (bid, bid_qty, ask, ask_qty, ts)
    buckets = []
    cur_bucket = (t0 // bucket_ms) * bucket_ms

    def emit_bucket(bucket_start):
        bucket_end = bucket_start + bucket_ms
        state = {}
        for v, (bid, bq, ask, aq, ts) in latest.items():
            if bucket_end - ts <= freshness_ms:
                state[v] = (bid, bq, ask, aq)
        buckets.append((bucket_start, state))

    for r in ticks:
        b = (r['ts_ms'] // bucket_ms) * bucket_ms
        while cur_bucket < b:
            emit_bucket(cur_bucket)
            cur_bucket += bucket_ms
        latest[r['venue']] = (r['bid'], r['bid_qty'], r['ask'], r['ask_qty'], r['ts_ms'])

    emit_bucket(cur_bucket)
    return buckets


def find_gaps(buckets, gap_bps):
    """Identify buckets where *any* cross-venue gap > gap_bps exists.

    Generalized to N>=2 venues: for each bucket, enumerate every
    ordered pair (buy_venue, sell_venue), compute
    `sell.bid - buy.ask`, pick the max across all pairs. Reports
    which pair won.

    Returns list of (bucket_start_ms, direction_key, edge_bps,
    exec_size_base, mid). direction_key is "buy_<v1>_sell_<v2>".
    """
    out = []
    for bucket_start, state in buckets:
        venues = [v for v, q in state.items() if len(q) >= 4 and q[0] > 0 and q[2] > 0]
        if len(venues) < 2:
            continue
        # For consistent "mid" across venues: average over all fresh venues.
        all_prices = []
        for v in venues:
            b, _, a, _ = state[v]
            all_prices.extend([b, a])
        mid = sum(all_prices) / len(all_prices)

        best = None  # (edge, edge_bps, direction, exec_size)
        for buy_v in venues:
            buy_ask = state[buy_v][2]
            buy_ask_qty = state[buy_v][3]
            for sell_v in venues:
                if sell_v == buy_v:
                    continue
                sell_bid = state[sell_v][0]
                sell_bid_qty = state[sell_v][1]
                edge = sell_bid - buy_ask
                if edge <= 0:
                    continue
                bps = edge / mid * 10000
                if best is None or bps > best[1]:
                    best = (
                        edge,
                        bps,
                        f'buy_{buy_v}_sell_{sell_v}',
                        min(buy_ask_qty, sell_bid_qty),
                    )

        if best is None or best[1] <= gap_bps:
            continue
        out.append((bucket_start, best[2], best[1], best[3], mid))
    return out


def gap_events(gaps, bucket_ms):
    """Group consecutive gap buckets into events. Returns list of events.

    An event = consecutive buckets (same direction) where the gap stays
    above threshold. Returns (start_ms, end_ms, duration_ms, peak_bps,
    mean_exec_size, direction, mean_mid).
    """
    if not gaps:
        return []
    events = []
    cur_start = gaps[0][0]
    cur_end = gaps[0][0]
    cur_dir = gaps[0][1]
    cur_bps = [gaps[0][2]]
    cur_size = [gaps[0][3]]
    cur_mid = [gaps[0][4]]

    for i in range(1, len(gaps)):
        b, d, bps, sz, mid = gaps[i]
        if d == cur_dir and b == cur_end + bucket_ms:
            cur_end = b
            cur_bps.append(bps)
            cur_size.append(sz)
            cur_mid.append(mid)
        else:
            events.append((cur_start, cur_end, cur_end - cur_start + bucket_ms,
                           max(cur_bps), statistics.mean(cur_size),
                           cur_dir, statistics.mean(cur_mid)))
            cur_start = b
            cur_end = b
            cur_dir = d
            cur_bps = [bps]
            cur_size = [sz]
            cur_mid = [mid]
    events.append((cur_start, cur_end, cur_end - cur_start + bucket_ms,
                   max(cur_bps), statistics.mean(cur_size),
                   cur_dir, statistics.mean(cur_mid)))
    return events


def p(values, q):
    if not values:
        return 0
    s = sorted(values)
    k = int(q * (len(s) - 1))
    return s[k]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('jsonl')
    ap.add_argument('--gap-bps', type=float, default=7.0,
                    help='min gap in bps to count as an opportunity (default 7)')
    ap.add_argument('--fee-bps', type=float, default=7.0,
                    help='round-trip fee assumption in bps (default 7)')
    ap.add_argument('--bucket-ms', type=int, default=100,
                    help='time alignment bucket size (default 100ms)')
    ap.add_argument('--min-dwell-ms', type=int, default=500,
                    help='min event duration to consider "executable" (default 500)')
    ap.add_argument('--freshness-ms', type=int, default=500,
                    help='venue quote considered stale after this (default 500)')
    args = ap.parse_args()

    by_sym = load(args.jsonl)
    if not by_sym:
        print(f'no data in {args.jsonl}', file=sys.stderr)
        return 1

    total_span_ms = 0
    header = f'{"symbol":10s} {"bx_tk":>6s} {"by_tk":>6s} {"gap_bkts":>8s} {"events":>7s} {"exec_ev":>7s} {"p50_dur":>7s} {"p95_dur":>7s} {"max_bps":>7s} {"net_bps":>7s} {"size_med":>10s} {"theor_pnl_$":>12s}'
    print(header)
    print('-' * len(header))

    totals = {'events': 0, 'exec_events': 0, 'pnl': 0.0}

    for sym in sorted(by_sym):
        ticks = by_sym[sym]
        bx = sum(1 for r in ticks if r['venue'] == 'binance')
        by = sum(1 for r in ticks if r['venue'] == 'bybit')
        if bx == 0 or by == 0:
            continue
        buckets = align_to_buckets(ticks, args.bucket_ms, args.freshness_ms)
        if not buckets:
            continue
        span = buckets[-1][0] - buckets[0][0]
        total_span_ms = max(total_span_ms, span)
        gaps = find_gaps(buckets, args.gap_bps)
        events = gap_events(gaps, args.bucket_ms)
        if not events:
            print(f'{sym:10s} {bx:>6d} {by:>6d} {0:>8d} {0:>7d} {0:>7d} {"-":>7s} {"-":>7s} {"-":>7s} {"-":>7s} {"-":>10s} {"-":>12s}')
            continue
        durations = [e[2] for e in events]
        bps_vals = [e[3] for e in events]
        sizes = [e[4] for e in events]
        mids = [e[6] for e in events]

        executable = [e for e in events if e[2] >= args.min_dwell_ms]
        # Naive PnL: for each executable event, profit_per_trade =
        # (net_edge_bps / 10000) * mean_size * mean_mid
        # (one trade per event; in practice you'd get fills as the book
        # refills. Treat this as a LOWER BOUND.)
        net_bps = max(0.0, max(bps_vals) - args.fee_bps)
        total_pnl = 0.0
        for (start, end, dur, peak_bps, sz, direction, mid) in executable:
            net = max(0.0, peak_bps - args.fee_bps)
            total_pnl += (net / 10000.0) * sz * mid

        totals['events'] += len(events)
        totals['exec_events'] += len(executable)
        totals['pnl'] += total_pnl

        print(
            f'{sym:10s} {bx:>6d} {by:>6d} {len(gaps):>8d} {len(events):>7d} {len(executable):>7d} '
            f'{p(durations, 0.5):>7d} {p(durations, 0.95):>7d} '
            f'{max(bps_vals):>7.1f} {net_bps:>7.1f} {statistics.median(sizes):>10.2f} {total_pnl:>12.2f}'
        )

    print('-' * len(header))
    print(f'window: {total_span_ms / 1000:.0f}s   '
          f'total events: {totals["events"]}   '
          f'executable (>= {args.min_dwell_ms}ms): {totals["exec_events"]}   '
          f'theoretical PnL: ${totals["pnl"]:.2f}')
    if total_span_ms > 0:
        hours = total_span_ms / 1000 / 3600
        print(f'annualized theoretical PnL (capital-agnostic lower bound): ${totals["pnl"] / hours * 24 * 365:.0f}/yr')
    return 0


if __name__ == '__main__':
    sys.exit(main())
