//! Treats arbitrary bytes as a whole log file and runs the full pipeline: every
//! encoding and format path, row reading, search and summary.

#![no_main]

use libfuzzer_sys::fuzz_target;
use std::sync::atomic::AtomicU64;
use tv_engine::query::{self, Query};
use tv_engine::{summary, Control, Level, LogFile};

fuzz_target!(|data: &[u8]| {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fuzz.log");
    std::fs::write(&path, data).unwrap();
    let (log, _) = LogFile::open_with_cache(&path, None, &AtomicU64::new(0)).unwrap();

    // Rows must cover the file's text exactly, in order, with no gaps or overlap.
    let body_start = if log.rows() > 0 { log.offset(0) as usize } else { data.len() };
    let mut joined = Vec::new();
    log.read_rows(0..log.rows(), &mut Vec::new(), |_, raw, _| {
        joined.extend_from_slice(raw);
    })
    .unwrap();
    assert_eq!(joined.as_slice(), &data[body_start.min(data.len())..]);

    let control = Control::default();
    let all = query::run(&log, &Query::default(), &control).unwrap();
    assert_eq!(all.total_matches, log.rows() as u64);
    let search = Query {
        text: Some("error".to_string()),
        min_level: Some(Level::Warning),
        ..Query::default()
    };
    let _ = query::run(&log, &search, &control).unwrap();
    let summary = summary::summarize(&log, 5, &control).unwrap();
    let counted = summary.levels.debug
        + summary.levels.info
        + summary.levels.notice
        + summary.levels.warning
        + summary.levels.error
        + summary.levels.critical;
    assert_eq!(counted, log.rows() as u64);
});
