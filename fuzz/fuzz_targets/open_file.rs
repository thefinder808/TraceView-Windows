//! Treats arbitrary bytes as a whole log file and runs the full pipeline: every
//! encoding and format path, row reading, search and summary. Each file is
//! opened twice: as TraceView picks a format by itself, and with a custom
//! format named for it, so the pattern-driven indexer is covered too.

#![no_main]

use libfuzzer_sys::fuzz_target;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, OnceLock};
use tv_engine::query::{self, Query};
use tv_engine::{summary, Control, CustomFormat, Level, LogFile};

const PATTERN: &str = r"^(?P<time>\S+(?: \S+)?)\s+\[?(?P<level>\w*)\]?\s*(?P<component>[^:\s]*):?\s*(?P<message>.*)$";

fn custom() -> Arc<CustomFormat> {
    static FORMAT: OnceLock<Arc<CustomFormat>> = OnceLock::new();
    FORMAT
        .get_or_init(|| Arc::new(CustomFormat::new("Fuzz", PATTERN, "fuzz.log", false).unwrap()))
        .clone()
}

fuzz_target!(|data: &[u8]| {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fuzz.log");
    std::fs::write(&path, data).unwrap();
    for formats in [Vec::new(), vec![custom()]] {
        let (log, _) = LogFile::open_with(&path, None, &formats, &AtomicU64::new(0)).unwrap();

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
    }
});
