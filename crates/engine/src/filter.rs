//! Parallel regex filter over raw bytes. Rows are never decoded to UTF-16 or
//! allocated, which keeps multi-gigabyte scans fast.

use crate::file::LogFile;
use crate::parse;
use rayon::prelude::*;
use regex::bytes::Regex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const ROWS_PER_TASK: usize = 64 * 1024;

/// Builds a matcher from filter box text. `literal` treats the text as plain,
/// case-insensitive text instead of a pattern.
pub fn compile(text: &str, literal: bool) -> Result<Regex, regex::Error> {
    if literal {
        Regex::new(&format!("(?i){}", regex::escape(text)))
    } else {
        Regex::new(text)
    }
}

/// Returns the matching row numbers in file order, or `None` when cancelled.
pub fn run(
    log: &LogFile,
    matcher: &Regex,
    rows_done: &AtomicUsize,
    cancel: &AtomicBool,
) -> Option<Vec<u32>> {
    let format = log.format();
    let rows = log.rows();
    let parts: Option<Vec<Vec<u32>>> = (0..rows.div_ceil(ROWS_PER_TASK))
        .into_par_iter()
        .map(|task| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let start = task * ROWS_PER_TASK;
            let end = (start + ROWS_PER_TASK).min(rows);
            let mut matched = Vec::new();
            for row in start..end {
                if matcher.is_match(parse::searchable(format, log.row_bytes(row))) {
                    matched.push(row as u32);
                }
            }
            rows_done.fetch_add(end - start, Ordering::Relaxed);
            Some(matched)
        })
        .collect();
    parts.map(|parts| parts.concat())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    fn open(text: &str) -> (tempfile::TempDir, LogFile) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.log");
        std::fs::write(&path, text).unwrap();
        let (log, _) = LogFile::open_with_cache(&path, None, &AtomicU64::new(0)).unwrap();
        (dir, log)
    }

    #[test]
    fn keeps_file_order_across_parallel_tasks() {
        let text: String = (0..200_000).map(|n| format!("row {n}\n")).collect();
        let (_dir, log) = open(&text);
        let matcher = compile(r"row \d*7$", false).unwrap();
        let matched = run(&log, &matcher, &AtomicUsize::new(0), &AtomicBool::new(false)).unwrap();
        assert_eq!(matched.len(), 20_000);
        assert!(matched.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn literal_mode_escapes_pattern_characters() {
        let (_dir, log) = open("a.c\nabc\nA.C\n");
        let matcher = compile("a.c", true).unwrap();
        let matched = run(&log, &matcher, &AtomicUsize::new(0), &AtomicBool::new(false)).unwrap();
        assert_eq!(matched, vec![0, 2]);
    }

    #[test]
    fn cancelling_returns_nothing() {
        let (_dir, log) = open("a\nb\n");
        let matcher = compile("a", true).unwrap();
        assert!(run(&log, &matcher, &AtomicUsize::new(0), &AtomicBool::new(true)).is_none());
    }

    #[test]
    fn invalid_pattern_is_an_error_not_a_panic() {
        assert!(compile("(", false).is_err());
    }
}
