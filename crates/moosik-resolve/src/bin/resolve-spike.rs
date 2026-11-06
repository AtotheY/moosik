//! M1 gate: prove a video id can be turned into a real, full-speed Opus stream.
//!
//! A successful resolve is not sufficient evidence that the resolver works. If the `n`
//! parameter is deciphered incorrectly YouTube still serves the stream, throttled to a
//! trickle. So this probes actual throughput and fails loudly if it is slow.
//!
//!     cargo run -p moosik-resolve --bin resolve-spike -- <video_id> [more ids...]

use std::time::Instant;

use anyhow::{Context, Result, bail};
use moosik_resolve::{
    ChainResolver, Codec, StreamCache, StreamResolver, VideoId, YtDlpResolver, probe_throughput,
};

/// Below this the `n` parameter is almost certainly wrong: a wrong one throttles the stream to
/// about real time (~20 KB/s for Opus at 160 kbps). The guest path's ranged reads measure
/// ~0.66 MiB/s from a cold client per range (2026-09-23) — some thirty times real time, and not
/// throttled — so the bar sits well below that and well above a trickle.
const MIN_THROUGHPUT_BYTES_PER_SEC: f64 = 250_000.0;
const PROBE_BYTES: u64 = 2 * 1024 * 1024;

#[tokio::main]
async fn main() -> Result<()> {
    // `filter::Targets`, not `EnvFilter`: the latter's dynamic matching pulls in the regex
    // crates, and this parses the same `info,moosik_resolve=debug` syntax without them.
    {
        use tracing_subscriber::{filter::Targets, fmt, prelude::*};
        let filter = std::env::var("RUST_LOG")
            .ok()
            .and_then(|s| s.parse::<Targets>().ok())
            .unwrap_or_else(|| "info,moosik_resolve=debug".parse::<Targets>().unwrap());
        tracing_subscriber::registry()
            .with(fmt::layer())
            .with(filter)
            .init();
    }

    let ids: Vec<String> = std::env::args().skip(1).collect();
    if ids.is_empty() {
        bail!("usage: resolve-spike <video_id> [video_id...]");
    }

    let ytdlp = YtDlpResolver::new();
    let ytdlp_available = ytdlp.is_available().await;
    if !ytdlp_available {
        eprintln!("note: yt-dlp not found on PATH; the fallback path is unavailable\n");
    }

    // The app's own order: the guest request, then yt-dlp (and with `--features native-resolver`
    // the native path between them, which is how to re-test whether it has come back).
    let guest = moosik_resolve::GuestResolver::new(reqwest::Client::new());
    let resolver = StreamCache::new(ChainResolver::default_order(guest, ytdlp));

    let mut failures = 0usize;

    for raw in &ids {
        let id =
            VideoId::new(raw.as_str()).with_context(|| format!("{raw:?} is not a video id"))?;

        println!("=== {id} ===");

        let started = Instant::now();
        let stream = match resolver.resolve(&id).await {
            Ok(s) => s,
            Err(e) => {
                println!("  RESOLVE FAILED: {e:#}\n");
                failures += 1;
                continue;
            }
        };
        let resolve_time = started.elapsed();

        println!("  resolver     {}", stream.resolved_by);
        println!("  itag         {}", stream.itag);
        println!("  codec        {}", stream.codec);
        println!("  bitrate      {} kbps", stream.bitrate_kbps);
        println!(
            "  size         {}",
            stream
                .content_length
                .map(|b| format!("{:.1} MiB", b as f64 / 1024.0 / 1024.0))
                .unwrap_or_else(|| "unknown".into())
        );
        println!(
            "  duration     {}",
            stream
                .duration
                .map(|d| format!("{}:{:02}", d.as_secs() / 60, d.as_secs() % 60))
                .unwrap_or_else(|| "unknown".into())
        );
        println!(
            "  loudness     {}",
            stream
                .loudness_db
                .map(|d| format!("{d:.1} dB"))
                .unwrap_or_else(|| "not reported".into())
        );
        println!(
            "  expires      {}",
            stream
                .expires_at
                .and_then(|e| e.duration_since(std::time::SystemTime::now()).ok())
                .map(|d| format!("in {:.1} h", d.as_secs_f64() / 3600.0))
                .unwrap_or_else(|| "unknown".into())
        );
        println!(
            "  resolve time {:.0} ms",
            resolve_time.as_secs_f64() * 1000.0
        );
        if std::env::var_os("SPIKE_PRINT_URL").is_some() {
            println!("  url          {}", stream.url);
        }

        // Gate 1: Opus. AAC resolves fine but means we silently lost audio quality.
        if stream.codec != Codec::Opus {
            println!("  ** WARNING: got {} rather than opus **", stream.codec);
        }

        // Gate 2: throughput.
        match probe_throughput(&stream, PROBE_BYTES).await {
            Ok((bytes, elapsed)) => {
                let bps = bytes as f64 / elapsed.as_secs_f64();
                println!(
                    "  throughput   {:.2} MiB/s ({:.1} MiB in {:.2}s)",
                    bps / 1024.0 / 1024.0,
                    bytes as f64 / 1024.0 / 1024.0,
                    elapsed.as_secs_f64()
                );
                if bps < MIN_THROUGHPUT_BYTES_PER_SEC {
                    println!("  ** FAIL: throttled — the `n` parameter is likely wrong **");
                    failures += 1;
                } else {
                    println!("  PASS");
                }
            }
            Err(e) => {
                println!("  ** PROBE FAILED: {e:#} **");
                failures += 1;
            }
        }
        println!();
    }

    // Second resolve of the first id should be a cache hit, i.e. effectively free.
    if let Some(first) = ids.first()
        && let Ok(id) = VideoId::new(first.as_str())
    {
        let started = Instant::now();
        if resolver.resolve(&id).await.is_ok() {
            println!(
                "cache hit re-resolve: {:.2} ms",
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
    }

    if failures > 0 {
        bail!("{failures} of {} checks failed", ids.len());
    }
    println!("all {} id(s) passed", ids.len());
    Ok(())
}
