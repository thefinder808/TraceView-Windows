//! Performance limits that fail the build if the engine slows down. They need a
//! large log and a release build, so they're ignored in ordinary test runs. Use
//! `scripts/check-perf.ps1`, which sets `TV_PERF_LOG` and runs them.
//!
//! Limits assume a warm disk cache and an NVMe drive, and scale with file size.

use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};
use tv_engine::query::{self, Query};
use tv_engine::{summary, CacheUse, Control, LogFile};

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

fn log_path() -> PathBuf {
    PathBuf::from(std::env::var_os("TV_PERF_LOG").expect("set TV_PERF_LOG to a large log"))
}

fn gibibytes() -> f64 {
    std::fs::metadata(log_path()).unwrap().len() as f64 / GIB
}

fn check(name: &str, elapsed: Duration, limit: f64) {
    println!("{name}: {:.3}s (limit {limit:.3}s)", elapsed.as_secs_f64());
    assert!(
        elapsed.as_secs_f64() <= limit,
        "{name} took {:.3}s, over its {limit:.3}s limit",
        elapsed.as_secs_f64()
    );
}

fn open_uncached() -> (LogFile, Duration) {
    let started = Instant::now();
    let (log, _) = LogFile::open_with_cache(&log_path(), None, &AtomicU64::new(0)).unwrap();
    (log, started.elapsed())
}

#[test]
#[ignore = "needs TV_PERF_LOG and a release build"]
fn first_open_stays_under_one_second_per_gibibyte() {
    let (_log, elapsed) = open_uncached();
    check("first open", elapsed, gibibytes());
}

#[test]
#[ignore = "needs TV_PERF_LOG and a release build"]
fn cached_reopen_stays_under_100_ms_per_gibibyte() {
    let dir = tempfile::tempdir().unwrap();
    let progress = AtomicU64::new(0);
    let path = log_path();
    drop(LogFile::open_with_cache(&path, Some(dir.path()), &progress).unwrap());

    let started = Instant::now();
    let (_log, stats) = LogFile::open_with_cache(&path, Some(dir.path()), &progress).unwrap();
    assert_eq!(stats.cache, CacheUse::Hit);
    check("cached reopen", started.elapsed(), 0.1 * gibibytes());
}

#[test]
#[ignore = "needs TV_PERF_LOG and a release build"]
fn index_stays_compact() {
    let (log, _) = open_uncached();
    let per_row = log.index_memory_bytes() as f64 / log.rows() as f64;
    println!("index: {:.1} bytes per row, {} rows", per_row, log.rows());
    assert!(per_row <= 10.0, "index uses {per_row:.1} bytes per row");
}

#[test]
#[ignore = "needs TV_PERF_LOG and a release build"]
fn search_stays_under_half_a_second_per_gibibyte() {
    let (log, _) = open_uncached();
    let budget = 0.5 * gibibytes();
    for (name, text, regex) in [
        ("literal search", "hash mismatch", false),
        ("regex search", r"error 0x[0-9a-fA-F]{8}", true),
        ("search with no matches", r"KB\d+: never appears", true),
    ] {
        let query = Query {
            text: Some(text.to_string()),
            regex,
            ..Query::default()
        };
        let started = Instant::now();
        query::run(&log, &query, &Control::default()).unwrap();
        check(name, started.elapsed(), budget);
    }
}

#[test]
#[ignore = "needs TV_PERF_LOG and a release build"]
fn index_only_filter_stays_under_100_ms_per_gibibyte() {
    let (log, _) = open_uncached();
    let query = Query {
        min_level: Some(tv_engine::Level::Error),
        ..Query::default()
    };
    let started = Instant::now();
    query::run(&log, &query, &Control::default()).unwrap();
    check("level filter", started.elapsed(), 0.1 * gibibytes());
}

#[test]
#[ignore = "needs TV_PERF_LOG and a release build"]
fn summary_stays_under_one_second_per_gibibyte() {
    let (log, _) = open_uncached();
    let started = Instant::now();
    summary::summarize(&log, 20, &Control::default()).unwrap();
    check("summary", started.elapsed(), gibibytes());
}

/// Reading a screenful of rows happens every frame, so it must fit well inside
/// a 60 Hz frame even at the far end of the file.
#[test]
#[ignore = "needs TV_PERF_LOG and a release build"]
fn reading_visible_rows_fits_in_a_frame() {
    let (log, _) = open_uncached();
    let mut buffer = Vec::new();
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut samples = Vec::with_capacity(2000);
    for _ in 0..2000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let first = (state % (log.rows() as u64 - 100)) as usize;
        let started = Instant::now();
        log.read_rows(first..first + 60, &mut buffer, |_, raw, _| {
            std::hint::black_box(raw);
        })
        .unwrap();
        samples.push(started.elapsed());
    }
    samples.sort();
    let p99 = samples[samples.len() * 99 / 100];
    check("60 visible rows, p99", p99, 0.002);
}

#[test]
#[ignore = "needs TV_PERF_LOG and a release build"]
fn checking_an_unchanged_live_log_is_cheap() {
    let (mut log, _) = open_uncached();
    let started = Instant::now();
    for _ in 0..100 {
        log.refresh(&AtomicU64::new(0)).unwrap();
    }
    check("100 refresh checks", started.elapsed(), 0.05);
}
