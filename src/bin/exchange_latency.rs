//! Exchange latency measurement tool.
//! Hits server-time endpoints and computes half-RTT stats.
//!
//! Usage: cargo run --bin exchange_latency [--iterations 100] [--max-time 120]

use std::time::{Duration, Instant};
use anyhow::{Result, Context};
use serde::Deserialize;

#[derive(Debug)]
struct LatencyResult {
    exchange: &'static str,
    endpoint: &'static str,
    samples: Vec<f64>,
    clock_offsets: Vec<f64>,
    errors: u32,
}

#[derive(Deserialize)]
struct BinanceTime {
    #[serde(rename = "serverTime")]
    server_time: i64,
}

#[derive(Deserialize)]
struct BybitTimeResult {
    #[serde(rename = "timeNano")]
    time_nano: String,
}

#[derive(Deserialize)]
struct BybitTime {
    result: BybitTimeResult,
}

#[derive(Deserialize)]
struct OkxTimeEntry {
    ts: String,
}

#[derive(Deserialize)]
struct OkxTime {
    data: Vec<OkxTimeEntry>,
}

async fn measure_binance(client: &reqwest::Client, url: &str) -> Result<(f64, f64)> {
    let t0 = Instant::now();
    let local_ms_before = chrono::Utc::now().timestamp_millis();
    let resp: BinanceTime = client.get(url).send().await?.json().await?;
    let rtt_us = t0.elapsed().as_micros() as f64;
    let local_ms_after = chrono::Utc::now().timestamp_millis();
    let local_mid = (local_ms_before + local_ms_after) as f64 / 2.0;
    let offset_ms = resp.server_time as f64 - local_mid;
    Ok((rtt_us / 2.0, offset_ms))
}

async fn measure_bybit(client: &reqwest::Client, url: &str) -> Result<(f64, f64)> {
    let t0 = Instant::now();
    let local_ms_before = chrono::Utc::now().timestamp_millis();
    let resp: BybitTime = client.get(url).send().await?.json().await?;
    let rtt_us = t0.elapsed().as_micros() as f64;
    let local_ms_after = chrono::Utc::now().timestamp_millis();
    let local_mid = (local_ms_before + local_ms_after) as f64 / 2.0;
    let server_ms = resp.result.time_nano.parse::<f64>().unwrap_or(0.0) / 1_000_000.0;
    let offset_ms = server_ms - local_mid;
    Ok((rtt_us / 2.0, offset_ms))
}

async fn measure_hyperliquid(client: &reqwest::Client, url: &str) -> Result<(f64, f64)> {
    let t0 = Instant::now();
    // HL has no server time endpoint — just measure RTT to /info
    let _resp = client.post(url)
        .json(&serde_json::json!({"type": "meta"}))
        .send().await?
        .bytes().await?;
    let rtt_us = t0.elapsed().as_micros() as f64;
    Ok((rtt_us / 2.0, 0.0)) // no clock offset available
}

async fn measure_risex(client: &reqwest::Client, url: &str) -> Result<(f64, f64)> {
    let t0 = Instant::now();
    // Rise has no server time endpoint — just measure RTT to /client
    let _resp = client.get(url).send().await?.bytes().await?;
    let rtt_us = t0.elapsed().as_micros() as f64;
    Ok((rtt_us / 2.0, 0.0)) // no clock offset available
}

async fn measure_hotstuff(client: &reqwest::Client, url: &str) -> Result<(f64, f64)> {
    let t0 = Instant::now();
    // No server time endpoint — just measure RTT
    let _resp = client.get(url).send().await?.bytes().await?;
    let rtt_us = t0.elapsed().as_micros() as f64;
    Ok((rtt_us / 2.0, 0.0))
}

async fn measure_zeroone(client: &reqwest::Client, url: &str) -> Result<(f64, f64)> {
    let t0 = Instant::now();
    let local_ms_before = chrono::Utc::now().timestamp_millis();
    let resp = client.get(url).send().await?.text().await?;
    let rtt_us = t0.elapsed().as_micros() as f64;
    let local_ms_after = chrono::Utc::now().timestamp_millis();
    let local_mid = (local_ms_before + local_ms_after) as f64 / 2.0;
    // ZeroOne returns seconds
    let server_ms = resp.trim().parse::<f64>().unwrap_or(0.0) * 1000.0;
    let offset_ms = server_ms - local_mid;
    Ok((rtt_us / 2.0, offset_ms))
}

async fn measure_okx(client: &reqwest::Client, url: &str) -> Result<(f64, f64)> {
    let t0 = Instant::now();
    let local_ms_before = chrono::Utc::now().timestamp_millis();
    let resp: OkxTime = client.get(url).send().await?.json().await?;
    let rtt_us = t0.elapsed().as_micros() as f64;
    let local_ms_after = chrono::Utc::now().timestamp_millis();
    let local_mid = (local_ms_before + local_ms_after) as f64 / 2.0;
    let server_ms = resp.data.first()
        .and_then(|e| e.ts.parse::<f64>().ok())
        .unwrap_or(0.0);
    let offset_ms = server_ms - local_mid;
    Ok((rtt_us / 2.0, offset_ms))
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() { return 0.0; }
    let idx = ((p / 100.0) * (sorted.len() - 1) as f64) as usize;
    sorted[idx]
}

fn fmt_us(us: f64) -> String {
    if us < 1000.0 {
        format!("{:>7.0} µs", us)
    } else if us < 1_000_000.0 {
        format!("{:>7.2} ms", us / 1000.0)
    } else {
        format!("{:>7.2}  s", us / 1_000_000.0)
    }
}

fn print_header() {
    println!("  {:<12} {:<40} {:>5} {:>4}  {:>10} {:>10} {:>10} {:>10} {:>10}  {:>10}",
        "Exchange", "Endpoint", "N", "Err", "Min", "p25", "p50", "p75", "p99", "Clk Offset");
    println!("  {:-<12} {:-<40} {:->5} {:->4}  {:->10} {:->10} {:->10} {:->10} {:->10}  {:->10}",
        "", "", "", "", "", "", "", "", "", "");
}

fn print_row(r: &LatencyResult) {
    let mut sorted = r.samples.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = sorted.len();

    let mut offsets = r.clock_offsets.clone();
    offsets.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median_offset = percentile(&offsets, 50.0);

    println!("  {:<12} {:<40} {:>5} {:>4}  {:>10} {:>10} {:>10} {:>10} {:>10}  {:>+8.1} ms",
        r.exchange,
        r.endpoint,
        n,
        r.errors,
        fmt_us(sorted[0]),
        fmt_us(percentile(&sorted, 25.0)),
        fmt_us(percentile(&sorted, 50.0)),
        fmt_us(percentile(&sorted, 75.0)),
        fmt_us(percentile(&sorted, 99.0)),
        median_offset,
    );
}

fn print_footer() {
    println!();
    println!("  Half-RTT = one-way floor estimate. Min is the tightest bound.");
    println!("  Clock offset = server_time - local_time (positive = server ahead).");
    println!();
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut iterations: usize = 100;
    let mut max_secs: f64 = 120.0;
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len() {
        if args[i] == "--iterations" || args[i] == "-n" {
            iterations = args.get(i + 1)
                .context("--iterations requires a number")?
                .parse()
                .context("invalid iteration count")?;
        }
        if args[i] == "--max-time" || args[i] == "-t" {
            max_secs = args.get(i + 1)
                .context("--max-time requires seconds")?
                .parse()
                .context("invalid max-time value")?;
        }
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .pool_max_idle_per_host(1)
        .build()?;

    // (exchange, url, parser, rate_limit_per_sec)
    // Rate limits (conservative):
    // Binance: 2400 req/min = 40/s → use 30/s
    // Bybit: 120 req/s → use 30/s
    // OKX: 20 req/2s = 10/s → use 8/s
    // Others: no known limits → use 30/s
    let endpoints: Vec<(&str, &str, &str, f64)> = vec![
        ("Binance", "https://fapi.binance.com/fapi/v1/time", "binance", 30.0),
        ("Binance-spot", "https://api.binance.com/api/v3/time", "binance", 30.0),
        ("Bybit", "https://api.bybit.com/v5/market/time", "bybit", 30.0),
        ("OKX", "https://www.okx.com/api/v5/public/time", "okx", 8.0),
        ("Hyperliquid", "https://api.hyperliquid.xyz/info", "hyperliquid", 30.0),
        ("RiseX", "https://api.rise.trade/client", "risex", 30.0),
        ("HotStuff", "https://api.hotstuff.trade/", "hotstuff", 30.0),
        ("ZeroOne", "https://zo-mainnet.n1.xyz/timestamp", "zeroone", 30.0),
    ];

    // Compute per-endpoint interval from time budget: spread iterations evenly across max_secs
    // Each endpoint gets its own interval = max_secs / iterations, clamped to not exceed rate limit

    println!("\n  Exchange Latency Benchmark ({iterations} iterations per endpoint)\n");
    print_header();

    let mut handles = Vec::new();

    let time_interval = max_secs / iterations as f64;
    let deadline = Duration::from_secs_f64(max_secs);
    for (exchange, url, parser, rps) in endpoints {
        let client = client.clone();
        let n = iterations;
        let interval = Duration::from_secs_f64(time_interval.max(1.0 / rps));
        handles.push(tokio::spawn(async move {
            // Warmup
            for _ in 0..3 {
                let _ = match parser {
                    "binance" => measure_binance(&client, &url).await,
                    "bybit" => measure_bybit(&client, &url).await,
                    "okx" => measure_okx(&client, &url).await,
                    "hyperliquid" => measure_hyperliquid(&client, &url).await,
                    "risex" => measure_risex(&client, &url).await,
                    "hotstuff" => measure_hotstuff(&client, &url).await,
                    "zeroone" => measure_zeroone(&client, &url).await,
                    _ => unreachable!(),
                };
            }

            let mut samples = Vec::with_capacity(n);
            let mut offsets = Vec::with_capacity(n);
            let mut errors = 0u32;
            let start = Instant::now();

            for _ in 0..n {
                if start.elapsed() >= deadline {
                    break;
                }

                let result = match parser {
                    "binance" => measure_binance(&client, &url).await,
                    "bybit" => measure_bybit(&client, &url).await,
                    "okx" => measure_okx(&client, &url).await,
                    "hyperliquid" => measure_hyperliquid(&client, &url).await,
                    "risex" => measure_risex(&client, &url).await,
                    "hotstuff" => measure_hotstuff(&client, &url).await,
                    "zeroone" => measure_zeroone(&client, &url).await,
                    _ => unreachable!(),
                };

                match result {
                    Ok((half_rtt, offset)) => {
                        samples.push(half_rtt);
                        offsets.push(offset);
                    }
                    Err(_) => { errors += 1; }
                }

                tokio::time::sleep(interval).await;
            }

            LatencyResult {
                exchange,
                endpoint: url,
                samples,
                clock_offsets: offsets,
                errors,
            }
        }));
    }

    for h in handles {
        let result = h.await?;
        print_row(&result);
    }

    print_footer();

    Ok(())
}
