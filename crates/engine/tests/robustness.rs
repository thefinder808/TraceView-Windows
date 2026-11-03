//! Log files are untrusted input. Whatever bytes a file holds, the engine must
//! not panic, must account for every byte, and must keep its counts consistent.
//!
//! This runs the same checks as the coverage-guided fuzz targets in `fuzz/`, on
//! every platform and in every test run. Set `PROPTEST_CASES` for a longer run.

use proptest::prelude::*;
use std::sync::atomic::AtomicU64;
use tv_engine::parse::{self, Format, ParseContext};
use tv_engine::query::{self, Query};
use tv_engine::summary::{self, normalize};
use tv_engine::time::YearHint;
use tv_engine::{overview, Control, Level, LevelCounts, LogFile};

const CONTEXT: ParseContext = ParseContext {
    year_hint: YearHint {
        year: 2026,
        month: 6,
    },
    custom: None,
};

/// Pieces of real log syntax, so generated input reaches deep into the parsers
/// rather than failing the first check.
const SYNTAX: &[&str] = &[
    "\n",
    "\r\n",
    "<![LOG[",
    "]LOG]!>",
    "<time=\"10:23:01.442+000\" ",
    "date=\"04-06-2026\" ",
    "component=\"App\" ",
    "type=\"3\" ",
    "thread=\"1\" ",
    "file=\"a.cpp\">",
    "2026-04-23 14:25:00",
    "2026-04-23T14:25:00.123Z",
    "2026/04/23 14:25:00.1234567",
    "Apr 23 14:25:00",
    "9999-99-99 99:99:99",
    ", Info   CBS   ",
    "time=\"",
    " level=warn ",
    "msg=\"",
    "host ",
    "proc[42]",
    "proc[x]",
    "<Warning>",
    ": ",
    ":",
    "[",
    "]",
    "<",
    ">",
    "\"",
    "ERROR",
    "warning",
    "Information",
    "failed",
    "0x80004005",
    "6F9619FF-8B86-D011-B42D-00C04FC964FF",
    "6F9619FF-",
    "12345",
    " ",
    "\t",
    "\u{e9}",
    "\u{FFFD}",
];

fn syntax_soup() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(
        prop_oneof![
            4 => proptest::sample::select(SYNTAX).prop_map(|piece| piece.as_bytes().to_vec()),
            1 => proptest::collection::vec(any::<u8>(), 0..8),
        ],
        0..60,
    )
    .prop_map(|pieces| pieces.concat())
}

fn any_log() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        3 => syntax_soup(),
        1 => proptest::collection::vec(any::<u8>(), 0..800),
        1 => syntax_soup().prop_map(|text| to_utf16(&text, true)),
        1 => syntax_soup().prop_map(|text| to_utf16(&text, false)),
    ]
}

fn to_utf16(text: &[u8], with_bom: bool) -> Vec<u8> {
    let mut bytes = if with_bom {
        vec![0xFF, 0xFE]
    } else {
        Vec::new()
    };
    bytes.extend(
        String::from_utf8_lossy(text)
            .encode_utf16()
            .flat_map(u16::to_le_bytes),
    );
    bytes
}

/// Runs the whole pipeline over one file and checks everything that must hold.
fn check_log(bytes: &[u8]) -> Result<(), TestCaseError> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input.log");
    std::fs::write(&path, bytes).unwrap();
    let (mut log, _) = LogFile::open_with_cache(&path, None, &AtomicU64::new(0)).unwrap();

    // Rows must cover the file's text exactly, with no gaps or overlaps.
    let body = if log.rows() > 0 {
        log.offset(0) as usize
    } else {
        bytes.len()
    };
    let mut joined = Vec::new();
    let mut scratch = Vec::new();
    let mut previous = None;
    log.read_rows(0..log.rows(), &mut Vec::new(), |row, raw, cut| {
        assert!(!cut);
        assert_eq!(previous.map_or(0, |last| last + 1), row);
        previous = Some(row);
        joined.extend_from_slice(raw);
        let text = log.text(raw, &mut scratch);
        let _ = log.fields(text);
    })
    .unwrap();
    prop_assert_eq!(joined.as_slice(), &bytes[body..]);

    // Offsets must strictly increase.
    for row in 1..log.rows() {
        prop_assert!(log.offset(row) > log.offset(row - 1));
    }

    // With no filters, everything matches.
    let control = Control::default();
    let all = query::run(
        &log,
        &Query {
            limit: Some(1000),
            ..Query::default()
        },
        &control,
    )
    .unwrap();
    prop_assert_eq!(all.total_matches, log.rows() as u64);
    prop_assert_eq!(all.rows.len(), log.rows().min(1000));
    for record in &all.rows {
        let row = record.row as usize - 1;
        prop_assert_eq!(record.level, log.level(row));
    }

    // A level filter must agree with the index.
    let errors = query::run(
        &log,
        &Query {
            min_level: Some(Level::Error),
            ..Query::default()
        },
        &control,
    )
    .unwrap();
    let expected = (0..log.rows())
        .filter(|row| log.level(*row) >= Level::Error)
        .count();
    prop_assert_eq!(errors.total_matches, expected as u64);

    // Text searches and summaries must complete, and counts must add up.
    let search = Query {
        text: Some(r"\d+|fail".to_string()),
        regex: true,
        ..Query::default()
    };
    let found = query::run(&log, &search, &control).unwrap();
    prop_assert!(found.total_matches <= log.rows() as u64);

    let result = summary::summarize(&log, 5, &control).unwrap();
    let levels = &result.levels;
    let total = levels.debug
        + levels.info
        + levels.notice
        + levels.warning
        + levels.error
        + levels.critical;
    prop_assert_eq!(total, log.rows() as u64);

    // The chips must agree with the summary, and the histogram over the log's
    // whole range must count every row once.
    prop_assert_eq!(
        &overview::level_counts(&log, None, &control).unwrap(),
        levels
    );
    if let Some(range) = overview::time_range(&log, None, &control).unwrap() {
        let histogram = overview::histogram(&log, None, range, 100, &control).unwrap();
        let placed: u64 = histogram.buckets.iter().map(LevelCounts::total).sum();
        prop_assert_eq!(placed, log.rows() as u64);
        let jump = |seconds| overview::first_row_at(&log, None, seconds, &control).unwrap();
        prop_assert_eq!(jump(range.first), 0);
        prop_assert_eq!(jump(range.last.saturating_add(1)), log.rows());
    }

    // Refreshing an untouched file must change nothing.
    let rows_before = log.rows();
    log.refresh(&AtomicU64::new(0)).unwrap();
    prop_assert_eq!(log.rows(), rows_before);
    Ok(())
}

proptest! {
    #[test]
    fn parsing_any_row_never_panics(row in any_log()) {
        for format in [Format::Plain, Format::Sccm] {
            let fields = parse::fields(format, &row, &CONTEXT);
            let _ = parse::searchable(format, &row);
            let mut template = Vec::new();
            normalize(fields.message, &mut template);
            if let Some(stamp) = fields.stamp {
                let _ = stamp.to_string();
            }
        }
    }

    #[test]
    fn any_file_survives_the_whole_pipeline(bytes in any_log()) {
        check_log(&bytes)?;
    }
}

#[test]
fn a_file_of_only_a_bom_has_no_rows() {
    for bytes in [&[0xEF, 0xBB, 0xBF][..], &[0xFF, 0xFE], &[0xFE, 0xFF]] {
        check_log(bytes).unwrap();
    }
}

#[test]
fn a_utf16_file_cut_mid_character_is_handled() {
    check_log(&[0xFF, 0xFE, b'a', 0, b'\n', 0, b'b']).unwrap();
}

#[test]
fn nul_filled_files_are_handled() {
    check_log(&vec![0u8; 4096]).unwrap();
}
