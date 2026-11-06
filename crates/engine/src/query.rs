//! Searching a log. Query and result types serialize to JSON, so the GUI, CLI
//! and a future MCP server all share them. Results are always paged and capped.

use crate::encoding;
use crate::file::{LogFile, MAX_ROW_BYTES};
use crate::level::Level;
use crate::parse;
use crate::time::{self, Stamp};
use rayon::prelude::*;
use regex::bytes::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub const DEFAULT_LIMIT: usize = 100;
pub const MAX_LIMIT: usize = 1000;
pub const MAX_MESSAGE_CHARS: usize = 4096;
pub(crate) const ROWS_PER_TASK: usize = 64 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Query {
    /// Text to find. Literal unless `regex` is set.
    pub text: Option<String>,
    pub regex: bool,
    pub case_sensitive: bool,
    pub min_level: Option<Level>,
    /// Only rows at exactly this level, as a severity chip filters.
    pub level: Option<Level>,
    /// An exact component name, in any case.
    pub component: Option<String>,
    /// An exact thread, in any case. SCCM logs write thread IDs, while some
    /// other formats write names.
    pub thread: Option<String>,
    /// Inclusive start, as `YYYY-MM-DD` or a full timestamp.
    pub from: Option<String>,
    /// Inclusive end. A bare date means the end of that day.
    pub to: Option<String>,
    pub offset: usize,
    /// Rows per page. Defaults to [`DEFAULT_LIMIT`] and is capped at [`MAX_LIMIT`].
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowRecord {
    /// One-based row number. An SCCM entry spanning several lines is one row,
    /// so this can differ from a text editor's line number.
    pub row: u64,
    /// The row's byte offset in the file.
    pub offset: u64,
    /// Wall-clock time as the log wrote it, with no time zone applied.
    pub timestamp: Option<String>,
    pub level: Level,
    pub component: String,
    pub thread: String,
    pub source: String,
    pub message: String,
    /// True if the row or message was too long and was cut off.
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryResult {
    pub total_rows: u64,
    pub total_matches: u64,
    pub offset: usize,
    pub limit: usize,
    pub rows: Vec<RowRecord>,
    /// The offset of the next page, if there is one.
    pub next_offset: Option<usize>,
}

#[derive(Debug)]
pub enum QueryError {
    InvalidPattern(String),
    InvalidTime(String),
    Io(io::Error),
    Cancelled,
}

impl QueryError {
    /// A stable code for callers to act on.
    pub fn code(&self) -> &'static str {
        match self {
            QueryError::InvalidPattern(_) => "invalid_pattern",
            QueryError::InvalidTime(_) => "invalid_time",
            QueryError::Io(_) => "io_error",
            QueryError::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for QueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryError::InvalidPattern(detail) => write!(formatter, "invalid pattern: {detail}"),
            QueryError::InvalidTime(text) => {
                write!(
                    formatter,
                    "couldn't read time \"{text}\", expected YYYY-MM-DD or YYYY-MM-DDTHH:MM:SS"
                )
            }
            QueryError::Io(error) => write!(formatter, "read failed: {error}"),
            QueryError::Cancelled => formatter.write_str("cancelled"),
        }
    }
}

impl std::error::Error for QueryError {}

impl From<io::Error> for QueryError {
    fn from(error: io::Error) -> Self {
        QueryError::Io(error)
    }
}

/// Lets another thread watch progress and cancel a long scan.
#[derive(Debug, Default)]
pub struct Control {
    pub cancel: AtomicBool,
    pub rows_done: AtomicUsize,
}

/// A compiled query, ready to test rows.
pub struct Matcher {
    pattern: Option<Regex>,
    min_level: Option<Level>,
    level: Option<Level>,
    component: Option<Vec<u8>>,
    thread: Option<Vec<u8>>,
    from: Option<u32>,
    to: Option<u32>,
}

/// Passes when there's no filter, or when the field equals it in any case.
fn field_matches(wanted: Option<&[u8]>, found: &[u8]) -> bool {
    wanted.is_none_or(|wanted| found.eq_ignore_ascii_case(wanted))
}

fn parse_bound(text: &str, end_of_day: bool) -> Result<u32, QueryError> {
    let stamp: Stamp =
        time::parse_user_time(text).ok_or_else(|| QueryError::InvalidTime(text.to_string()))?;
    let date_only = text.trim().len() == 10;
    Ok(if end_of_day && date_only {
        stamp.seconds + 86_399
    } else {
        stamp.seconds
    })
}

impl Matcher {
    pub fn new(query: &Query) -> Result<Self, QueryError> {
        let pattern = match query.text.as_deref().filter(|text| !text.is_empty()) {
            Some(text) => {
                let source = if query.regex {
                    text.to_string()
                } else {
                    regex::escape(text)
                };
                Some(
                    RegexBuilder::new(&source)
                        .case_insensitive(!query.case_sensitive)
                        .build()
                        .map_err(|error| QueryError::InvalidPattern(error.to_string()))?,
                )
            }
            None => None,
        };
        Ok(Self {
            pattern,
            min_level: query.min_level,
            level: query.level,
            component: query
                .component
                .as_ref()
                .map(|name| name.as_bytes().to_vec()),
            thread: query
                .thread
                .as_ref()
                .map(|thread| thread.as_bytes().to_vec()),
            from: query
                .from
                .as_deref()
                .map(|text| parse_bound(text, false))
                .transpose()?,
            to: query
                .to
                .as_deref()
                .map(|text| parse_bound(text, true))
                .transpose()?,
        })
    }

    fn needs_row_text(&self) -> bool {
        self.pattern.is_some() || self.needs_fields()
    }

    fn needs_fields(&self) -> bool {
        self.component.is_some() || self.thread.is_some()
    }

    /// Checks everything answerable from the index, without reading the file.
    pub(crate) fn passes_index(&self, log: &LogFile, row: usize) -> bool {
        if self
            .min_level
            .is_some_and(|minimum| log.level(row) < minimum)
        {
            return false;
        }
        if self.level.is_some_and(|level| log.level(row) != level) {
            return false;
        }
        if self.from.is_none() && self.to.is_none() {
            return true;
        }
        log.stamp_seconds(row).is_some_and(|seconds| {
            self.from.is_none_or(|from| seconds >= from) && self.to.is_none_or(|to| seconds <= to)
        })
    }

    fn passes_text(&self, log: &LogFile, text: &[u8]) -> bool {
        // Split the row into fields once, and only if a field filter needs them.
        if self.needs_fields() {
            let fields = log.fields(text);
            if !field_matches(self.component.as_deref(), fields.component)
                || !field_matches(self.thread.as_deref(), fields.thread)
            {
                return false;
            }
        }
        self.pattern
            .as_ref()
            .is_none_or(|pattern| pattern.is_match(parse::searchable(log.format(), text)))
    }

    /// Tests one row, given its decoded text from [`LogFile::text`]. This is
    /// for rows already read, such as the ones on screen.
    pub fn matches(&self, log: &LogFile, row: usize, text: &[u8]) -> bool {
        self.passes_index(log, row) && self.passes_text(log, text)
    }
}

/// Finds every matching row, in file order, using all cores.
pub fn matching_rows(
    log: &LogFile,
    matcher: &Matcher,
    control: &Control,
) -> Result<Vec<u32>, QueryError> {
    matching_rows_in(log, matcher, 0..log.rows(), control)
}

/// Finds the matching rows within `rows`, in file order, using all cores. Live
/// tail uses this to filter only the rows a refresh added. The range is
/// clamped to the rows the log has.
pub fn matching_rows_in(
    log: &LogFile,
    matcher: &Matcher,
    rows: Range<usize>,
    control: &Control,
) -> Result<Vec<u32>, QueryError> {
    let end = rows.end.min(log.rows());
    let rows = rows.start.min(end)..end;
    let parts: Vec<Vec<u32>> = (0..rows.len().div_ceil(ROWS_PER_TASK))
        .into_par_iter()
        .map_init(
            || (Vec::new(), Vec::new()),
            |(buffer, scratch), task| {
                if control.cancel.load(Ordering::Relaxed) {
                    return Err(QueryError::Cancelled);
                }
                let start = rows.start + task * ROWS_PER_TASK;
                let end = (start + ROWS_PER_TASK).min(rows.end);
                let mut matched = Vec::new();
                if matcher.needs_row_text() {
                    // Skip the read when the index already rules out the whole task.
                    if (start..end).any(|row| matcher.passes_index(log, row)) {
                        log.read_rows(start..end, buffer, |row, raw, _| {
                            if matcher.passes_index(log, row) {
                                let text = log.text(raw, scratch);
                                if matcher.passes_text(log, text) {
                                    matched.push(row as u32);
                                }
                            }
                        })?;
                    }
                } else {
                    matched.extend(
                        (start..end)
                            .filter(|row| matcher.passes_index(log, *row))
                            .map(|row| row as u32),
                    );
                }
                control.rows_done.fetch_add(end - start, Ordering::Relaxed);
                Ok(matched)
            },
        )
        .collect::<Result<_, QueryError>>()?;
    Ok(parts.concat())
}

fn display_text(bytes: &[u8]) -> String {
    encoding::display(bytes).into_owned()
}

/// Reads one row into its output form.
pub fn read_record(
    log: &LogFile,
    row: usize,
    buffer: &mut Vec<u8>,
    scratch: &mut Vec<u8>,
) -> io::Result<RowRecord> {
    let mut record = None;
    log.read_rows(row..row + 1, buffer, |_, raw, cut| {
        let text = log.text(raw, scratch);
        let fields = log.fields(text);
        let message = display_text(fields.message);
        let char_count = message.chars().count();
        let message_cut = char_count > MAX_MESSAGE_CHARS;
        let message = if message_cut {
            message.chars().take(MAX_MESSAGE_CHARS).collect()
        } else {
            message
        };
        record = Some(RowRecord {
            row: row as u64 + 1,
            offset: log.offset(row),
            timestamp: fields.stamp.map(|stamp| stamp.to_string()),
            level: log.level(row),
            component: display_text(fields.component),
            thread: display_text(fields.thread),
            source: display_text(fields.source),
            message,
            truncated: cut || message_cut,
        });
    })?;
    record.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "row is out of range"))
}

/// Runs a query and returns one page of results.
pub fn run(log: &LogFile, query: &Query, control: &Control) -> Result<QueryResult, QueryError> {
    let matcher = Matcher::new(query)?;
    let matched = matching_rows(log, &matcher, control)?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let start = query.offset.min(matched.len());
    let end = start.saturating_add(limit).min(matched.len());

    let (mut buffer, mut scratch) = (Vec::with_capacity(MAX_ROW_BYTES as usize / 16), Vec::new());
    let rows = matched[start..end]
        .iter()
        .map(|row| read_record(log, *row as usize, &mut buffer, &mut scratch))
        .collect::<io::Result<Vec<_>>>()?;
    Ok(QueryResult {
        total_rows: log.rows() as u64,
        total_matches: matched.len() as u64,
        offset: start,
        limit,
        rows,
        next_offset: (end < matched.len()).then_some(end),
    })
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

    fn matched(log: &LogFile, query: Query) -> Vec<u64> {
        run(log, &query, &Control::default())
            .unwrap()
            .rows
            .iter()
            .map(|row| row.row)
            .collect()
    }

    fn text_query(text: &str) -> Query {
        Query {
            text: Some(text.to_string()),
            ..Query::default()
        }
    }

    #[test]
    fn keeps_file_order_across_parallel_tasks() {
        let text: String = (0..200_000).map(|n| format!("row {n}\n")).collect();
        let (_dir, log) = open(&text);
        let matcher = Matcher::new(&Query {
            text: Some(r"row \d*7$".to_string()),
            regex: true,
            ..Query::default()
        })
        .unwrap();
        let rows = matching_rows(&log, &matcher, &Control::default()).unwrap();
        assert_eq!(rows.len(), 20_000);
        assert!(rows.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn one_row_matches_as_a_scan_would() {
        let (_dir, log) = open(
            "2026-01-01 10:00:00 host alpha[1]: ERROR: disk full\n2026-01-01 10:00:01 host beta[2]: disk fine\n",
        );
        let matcher = Matcher::new(&Query {
            text: Some("disk".to_string()),
            min_level: Some(Level::Error),
            ..Query::default()
        })
        .unwrap();
        let scanned = matching_rows(&log, &matcher, &Control::default()).unwrap();
        let mut one_at_a_time = Vec::new();
        let (mut buffer, mut scratch) = (Vec::new(), Vec::new());
        log.read_rows(0..log.rows(), &mut buffer, |row, raw, _| {
            if matcher.matches(&log, row, log.text(raw, &mut scratch)) {
                one_at_a_time.push(row as u32);
            }
        })
        .unwrap();
        assert_eq!(one_at_a_time, scanned);
        assert_eq!(scanned, vec![0]);
    }

    #[test]
    fn literal_text_treats_pattern_characters_as_text() {
        let (_dir, log) = open("a.c\nabc\nA.C\n");
        assert_eq!(matched(&log, text_query("a.c")), vec![1, 3]);
    }

    #[test]
    fn case_sensitivity_is_respected() {
        let (_dir, log) = open("Hello\nhello\n");
        assert_eq!(matched(&log, text_query("hello")), vec![1, 2]);
        let query = Query {
            case_sensitive: true,
            ..text_query("hello")
        };
        assert_eq!(matched(&log, query), vec![2]);
    }

    #[test]
    fn dollar_matches_the_end_of_a_row_despite_crlf() {
        let (_dir, log) = open("first end\r\nsecond\r\n");
        let query = Query {
            regex: true,
            ..text_query("end$")
        };
        assert_eq!(matched(&log, query), vec![1]);
    }

    #[test]
    fn sccm_searches_only_the_message() {
        let (_dir, log) = open(
            "<![LOG[hello]LOG]!><time=\"10:00:00.000+000\" date=\"01-01-2026\" component=\"App\" context=\"\" type=\"1\" thread=\"7\" file=\"a.cpp\">\r\n",
        );
        assert_eq!(matched(&log, text_query("hello")), vec![1]);
        assert!(matched(&log, text_query("thread")).is_empty());
    }

    #[test]
    fn filters_by_minimum_level() {
        let (_dir, log) = open("fine\nretrying soon\nit failed\n");
        let query = Query {
            min_level: Some(Level::Warning),
            ..Query::default()
        };
        assert_eq!(matched(&log, query), vec![2, 3]);
    }

    #[test]
    fn filters_by_component_in_any_case() {
        let (_dir, log) =
            open("2026-01-01 10:00:00 host alpha[1]: one\n2026-01-01 10:00:01 host beta[2]: two\n");
        let query = Query {
            component: Some("ALPHA".to_string()),
            ..Query::default()
        };
        assert_eq!(matched(&log, query), vec![1]);
    }

    #[test]
    fn filters_by_thread_in_any_case() {
        let entry = |thread: &str| {
            format!("<![LOG[hello]LOG]!><time=\"10:00:00.000+000\" date=\"01-01-2026\" component=\"App\" context=\"\" type=\"1\" thread=\"{thread}\" file=\"a.cpp\">\r\n")
        };
        let (_dir, log) = open(&[entry("12"), entry("Main"), entry("123")].concat());
        let thread = |name: &str| Query {
            thread: Some(name.to_string()),
            ..Query::default()
        };
        assert_eq!(matched(&log, thread("12")), vec![1]);
        assert_eq!(matched(&log, thread("MAIN")), vec![2]);
        let both = Query {
            component: Some("app".to_string()),
            ..thread("123")
        };
        assert_eq!(matched(&log, both), vec![3]);
        let wrong_component = Query {
            component: Some("other".to_string()),
            ..thread("123")
        };
        assert!(matched(&log, wrong_component).is_empty());
    }

    #[test]
    fn matching_within_a_range_keeps_order_and_clamps() {
        let text: String = (0..200_000).map(|n| format!("row {n}\n")).collect();
        let (_dir, log) = open(&text);
        let matcher = Matcher::new(&Query {
            text: Some(r"row \d*7$".to_string()),
            regex: true,
            ..Query::default()
        })
        .unwrap();
        let control = Control::default();
        let everything = matching_rows(&log, &matcher, &control).unwrap();
        let range = 70_001..150_000;
        let expected: Vec<u32> = everything
            .iter()
            .copied()
            .filter(|row| range.contains(&(*row as usize)))
            .collect();
        assert_eq!(
            matching_rows_in(&log, &matcher, range, &control).unwrap(),
            expected
        );
        assert_eq!(
            matching_rows_in(&log, &matcher, 199_990..1_000_000, &control).unwrap(),
            vec![199_997]
        );
        assert!(matching_rows_in(&log, &matcher, 500_000..600_000, &control)
            .unwrap()
            .is_empty());
        let backwards = Range {
            start: 100,
            end: 50,
        };
        assert!(matching_rows_in(&log, &matcher, backwards, &control)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn filters_by_time_and_a_bare_end_date_covers_that_day() {
        let (_dir, log) =
            open("2026-01-01 10:00:00 a\n2026-01-02 23:30:00 b\n2026-01-03 00:00:00 c\nno time\n");
        let query = Query {
            from: Some("2026-01-02".to_string()),
            to: Some("2026-01-02".to_string()),
            ..Query::default()
        };
        assert_eq!(matched(&log, query), vec![2]);
        let query = Query {
            from: Some("2026-01-03T00:00:00".to_string()),
            ..Query::default()
        };
        assert_eq!(matched(&log, query), vec![3]);
    }

    #[test]
    fn combines_every_filter() {
        let (_dir, log) = open(
            "2026-01-01 10:00:00 host a[1]: ERROR: disk one\n2026-01-01 10:00:01 host a[1]: INFO: disk two\n2026-01-01 10:00:02 host b[1]: ERROR: disk three\n",
        );
        let query = Query {
            text: Some("disk".to_string()),
            min_level: Some(Level::Error),
            component: Some("a".to_string()),
            ..Query::default()
        };
        assert_eq!(matched(&log, query), vec![1]);
    }

    #[test]
    fn pages_through_results_without_gaps() {
        let text: String = (0..25).map(|n| format!("row {n}\n")).collect();
        let (_dir, log) = open(&text);
        let mut seen = Vec::new();
        let mut query = Query {
            limit: Some(10),
            ..Query::default()
        };
        loop {
            let page = run(&log, &query, &Control::default()).unwrap();
            assert_eq!(page.total_matches, 25);
            seen.extend(page.rows.iter().map(|row| row.row));
            match page.next_offset {
                Some(next) => query.offset = next,
                None => break,
            }
        }
        assert_eq!(seen, (1..=25).collect::<Vec<u64>>());
    }

    #[test]
    fn limit_is_capped_and_offset_past_the_end_is_empty() {
        let text: String = (0..1500).map(|n| format!("row {n}\n")).collect();
        let (_dir, log) = open(&text);
        let page = run(
            &log,
            &Query {
                limit: Some(1_000_000),
                ..Query::default()
            },
            &Control::default(),
        )
        .unwrap();
        assert_eq!(page.rows.len(), MAX_LIMIT);
        assert_eq!(page.next_offset, Some(MAX_LIMIT));

        let past_end = run(
            &log,
            &Query {
                offset: 999_999,
                ..Query::default()
            },
            &Control::default(),
        )
        .unwrap();
        assert!(past_end.rows.is_empty());
        assert_eq!(past_end.next_offset, None);
    }

    #[test]
    fn long_messages_are_cut_and_flagged() {
        let text = format!("{}\nshort\n", "x".repeat(MAX_MESSAGE_CHARS + 10));
        let (_dir, log) = open(&text);
        let page = run(&log, &Query::default(), &Control::default()).unwrap();
        assert_eq!(page.rows[0].message.chars().count(), MAX_MESSAGE_CHARS);
        assert!(page.rows[0].truncated);
        assert!(!page.rows[1].truncated);
    }

    #[test]
    fn windows_1252_text_is_displayed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.log");
        std::fs::write(&path, b"caf\xE9 opened\n").unwrap();
        let (log, _) = LogFile::open_with_cache(&path, None, &AtomicU64::new(0)).unwrap();
        let page = run(&log, &Query::default(), &Control::default()).unwrap();
        assert_eq!(page.rows[0].message, "caf\u{e9} opened");
    }

    #[test]
    fn utf16_logs_can_be_searched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.log");
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(
            "alpha\nbeta caf\u{e9}\n"
                .encode_utf16()
                .flat_map(u16::to_le_bytes),
        );
        std::fs::write(&path, bytes).unwrap();
        let (log, _) = LogFile::open_with_cache(&path, None, &AtomicU64::new(0)).unwrap();
        assert_eq!(matched(&log, text_query("caf\u{e9}")), vec![2]);
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        let (_dir, log) = open("a\n");
        let bad_pattern = Query {
            regex: true,
            ..text_query("(")
        };
        assert_eq!(
            run(&log, &bad_pattern, &Control::default())
                .unwrap_err()
                .code(),
            "invalid_pattern"
        );
        let bad_time = Query {
            from: Some("yesterday".to_string()),
            ..Query::default()
        };
        assert_eq!(
            run(&log, &bad_time, &Control::default())
                .unwrap_err()
                .code(),
            "invalid_time"
        );
    }

    #[test]
    fn cancelling_stops_the_scan() {
        let (_dir, log) = open("a\nb\n");
        let control = Control::default();
        control.cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            run(&log, &Query::default(), &control).unwrap_err().code(),
            "cancelled"
        );
    }

    #[test]
    fn queries_round_trip_through_json_and_reject_unknown_fields() {
        let query = Query {
            text: Some("x".to_string()),
            min_level: Some(Level::Warning),
            thread: Some("5240".to_string()),
            ..Query::default()
        };
        let json = serde_json::to_string(&query).unwrap();
        assert!(json.contains(r#""thread":"5240""#));
        assert_eq!(serde_json::from_str::<Query>(&json).unwrap(), query);
        assert!(serde_json::from_str::<Query>(r#"{"unknown": 1}"#).is_err());
        assert_eq!(
            serde_json::from_str::<Query>("{}").unwrap(),
            Query::default()
        );
    }
}
