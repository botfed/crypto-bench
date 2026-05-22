#!/usr/bin/env python3
"""Exchange latency measurement tool.
Hits server-time endpoints and computes half-RTT stats.

Usage: python3 exchange_latency.py [--iterations 100] [--max-time 120]

Zero external dependencies — stdlib only.
"""

import argparse
import json
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed


ENDPOINTS = [
    # (exchange, url, parser, rate_limit_per_sec)
    ("Binance",      "https://fapi.binance.com/fapi/v1/time",  "binance",     30.0),
    ("Binance-spot", "https://api.binance.com/api/v3/time",    "binance",     30.0),
    ("Bybit",        "https://api.bybit.com/v5/market/time",   "bybit",       30.0),
    ("OKX",          "https://www.okx.com/api/v5/public/time", "okx",          8.0),
    ("Hyperliquid",  "https://api.hyperliquid.xyz/info",       "hyperliquid", 30.0),
    ("RiseX",        "https://api.rise.trade/v1/system/config", "risex",       30.0),
    ("HotStuff",     "https://api.hotstuff.trade/",            "hotstuff",    30.0),
    ("ZeroOne",      "https://zo-mainnet.n1.xyz/timestamp",    "zeroone",     30.0),
]

TIMEOUT = 1  # seconds


def now_ms():
    return time.time() * 1000.0


def measure_binance(url):
    t0 = time.monotonic()
    local_before = now_ms()
    with urllib.request.urlopen(url, timeout=TIMEOUT) as r:
        data = json.load(r)
    rtt_us = (time.monotonic() - t0) * 1e6
    local_after = now_ms()
    local_mid = (local_before + local_after) / 2.0
    offset_ms = data["serverTime"] - local_mid
    return rtt_us / 2.0, offset_ms


def measure_bybit(url):
    t0 = time.monotonic()
    local_before = now_ms()
    with urllib.request.urlopen(url, timeout=TIMEOUT) as r:
        data = json.load(r)
    rtt_us = (time.monotonic() - t0) * 1e6
    local_after = now_ms()
    local_mid = (local_before + local_after) / 2.0
    server_ms = float(data["result"]["timeNano"]) / 1e6
    offset_ms = server_ms - local_mid
    return rtt_us / 2.0, offset_ms


def measure_okx(url):
    t0 = time.monotonic()
    local_before = now_ms()
    with urllib.request.urlopen(url, timeout=TIMEOUT) as r:
        data = json.load(r)
    rtt_us = (time.monotonic() - t0) * 1e6
    local_after = now_ms()
    local_mid = (local_before + local_after) / 2.0
    server_ms = float(data["data"][0]["ts"])
    offset_ms = server_ms - local_mid
    return rtt_us / 2.0, offset_ms


def measure_hyperliquid(url):
    body = json.dumps({"type": "meta"}).encode()
    req = urllib.request.Request(url, data=body, headers={"Content-Type": "application/json"})
    t0 = time.monotonic()
    with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
        r.read()
    rtt_us = (time.monotonic() - t0) * 1e6
    return rtt_us / 2.0, 0.0


def measure_risex(url):
    t0 = time.monotonic()
    with urllib.request.urlopen(url, timeout=TIMEOUT) as r:
        r.read()
    rtt_us = (time.monotonic() - t0) * 1e6
    return rtt_us / 2.0, 0.0


def measure_hotstuff(url):
    t0 = time.monotonic()
    with urllib.request.urlopen(url, timeout=TIMEOUT) as r:
        r.read()
    rtt_us = (time.monotonic() - t0) * 1e6
    return rtt_us / 2.0, 0.0


def measure_zeroone(url):
    t0 = time.monotonic()
    local_before = now_ms()
    with urllib.request.urlopen(url, timeout=TIMEOUT) as r:
        text = r.read().decode().strip()
    rtt_us = (time.monotonic() - t0) * 1e6
    local_after = now_ms()
    local_mid = (local_before + local_after) / 2.0
    server_ms = float(text) * 1000.0
    offset_ms = server_ms - local_mid
    return rtt_us / 2.0, offset_ms


PARSERS = {
    "binance":     measure_binance,
    "bybit":       measure_bybit,
    "okx":         measure_okx,
    "hyperliquid": measure_hyperliquid,
    "risex":       measure_risex,
    "hotstuff":    measure_hotstuff,
    "zeroone":     measure_zeroone,
}


def percentile(sorted_vals, p):
    if not sorted_vals:
        return 0.0
    idx = int((p / 100.0) * (len(sorted_vals) - 1))
    return sorted_vals[idx]


def fmt_us(us):
    if us < 1000:
        return f"{us:>7.0f} µs"
    elif us < 1_000_000:
        return f"{us / 1000:>7.2f} ms"
    else:
        return f"{us / 1_000_000:>7.2f}  s"


def run_endpoint(exchange, url, parser_name, rps, iterations, max_secs):
    measure = PARSERS[parser_name]
    interval = max(max_secs / iterations, 1.0 / rps)

    # Warmup
    for _ in range(3):
        try:
            measure(url)
        except Exception:
            pass

    samples = []
    offsets = []
    errors = 0
    start = time.monotonic()
    deadline = max_secs

    for _ in range(iterations):
        if time.monotonic() - start >= deadline:
            break
        try:
            half_rtt, offset = measure(url)
            samples.append(half_rtt)
            offsets.append(offset)
        except Exception:
            errors += 1
        time.sleep(interval)

    return {
        "exchange": exchange,
        "endpoint": url,
        "samples": samples,
        "offsets": offsets,
        "errors": errors,
    }


def print_header():
    print(f"  {'Exchange':<12} {'Endpoint':<40} {'N':>5} {'Err':>4}  {'Min':>10} {'p25':>10} {'p50':>10} {'p75':>10} {'p99':>10}  {'Clk Offset':>10}")
    print(f"  {'-'*12} {'-'*40} {'-'*5} {'-'*4}  {'-'*10} {'-'*10} {'-'*10} {'-'*10} {'-'*10}  {'-'*10}")


def print_row(r):
    s = sorted(r["samples"])
    o = sorted(r["offsets"])
    if not s:
        return
    median_offset = percentile(o, 50.0)
    print(f"  {r['exchange']:<12} {r['endpoint']:<40} {len(s):>5} {r['errors']:>4}  {fmt_us(s[0]):>10} {fmt_us(percentile(s, 25)):>10} {fmt_us(percentile(s, 50)):>10} {fmt_us(percentile(s, 75)):>10} {fmt_us(percentile(s, 99)):>10}  {median_offset:>+8.1f} ms")


def main():
    ap = argparse.ArgumentParser(description="Exchange latency benchmark")
    ap.add_argument("-n", "--iterations", type=int, default=100)
    ap.add_argument("-t", "--max-time", type=float, default=120.0)
    args = ap.parse_args()

    print(f"\n  Exchange Latency Benchmark ({args.iterations} iterations per endpoint)\n")
    print_header()

    futures = {}
    with ThreadPoolExecutor(max_workers=len(ENDPOINTS)) as pool:
        for exchange, url, parser, rps in ENDPOINTS:
            f = pool.submit(run_endpoint, exchange, url, parser, rps, args.iterations, args.max_time)
            futures[f] = exchange

        # Print results in submission order
        for f in futures:
            print_row(f.result())

    print()
    print("  Half-RTT = one-way floor estimate. Min is the tightest bound.")
    print("  Clock offset = server_time - local_time (positive = server ahead).")
    print()


if __name__ == "__main__":
    main()
