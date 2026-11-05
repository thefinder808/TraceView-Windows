//! Summarizes a log: severity counts, repeated message patterns, and minutes
//! when errors spiked. It answers "what's going on in this file?" without
//! requiring anyone to read the whole thing.

use crate::encoding::{self, Encoding};
use crate::file::LogFile;
use crate::level::Level;
use crate::parse::Format;
use crate::query::{self, Control, QueryError, ROWS_PER_TASK};
use crate::time::Stamp;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::sync::atomic::Ordering;

pub const DEFAULT_TOP: usize = 20;
pub const MAX_TOP: usize = 200;
const MAX_TEMPLATE_BYTES: usize = 200;
/// An example keeps the pattern's real values, such as an exit code, so it's
/// capped tighter than a query's messages.
const MAX_EXAMPLE_CHARS: usize = 500;
/// Limits on distinct patterns tracked, so a log of unique messages can't
/// exhaust memory. Rows past the limit are counted in `other_rows`.
const MAX_PATTERNS_PER_TASK: usize = 20_000;
const MAX_PATTERNS: usize = 200_000;
const MAX_SPIKES: usize = 10;
const MIN_SPIKE_ERRORS: u64 = 5;
const MIN_MINUTES_FOR_SPIKES: usize = 10;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LevelCounts {
    pub debug: u64,
    pub info: u64,
    pub notice: u64,
    pub warning: u64,
    pub error: u64,
    pub critical: u64,
}

impl LevelCounts {
    /// Counts one more row of `level`.
    pub fn add(&mut self, level: Level) {
        match level {
            Level::Debug => self.debug += 1,
            Level::Info => self.info += 1,
            Level::Notice => self.notice += 1,
            Level::Warning => self.warning += 1,
            Level::Error => self.error += 1,
            Level::Critical => self.critical += 1,
        }
    }

    /// Adds another set of counts, such as another part of a log or another file.
    pub fn absorb(&mut self, other: &LevelCounts) {
        self.debug += other.debug;
        self.info += other.info;
        self.notice += other.notice;
        self.warning += other.warning;
        self.error += other.error;
        self.critical += other.critical;
    }

    /// Rows of one level.
    pub fn get(&self, level: Level) -> u64 {
        match level {
            Level::Debug => self.debug,
            Level::Info => self.info,
            Level::Notice => self.notice,
            Level::Warning => self.warning,
            Level::Error => self.error,
            Level::Critical => self.critical,
        }
    }

    /// Rows of every level.
    pub fn total(&self) -> u64 {
        Level::ALL.iter().map(|level| self.get(*level)).sum()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pattern {
    /// The message with variable parts replaced by `<n>`, `<hex>` or `<guid>`.
    pub template: String,
    /// The pattern's first row as written, with its real numbers and codes.
    /// Capped at 500 characters.
    pub example: String,
    pub count: u64,
    /// The highest severity seen with this pattern.
    pub level: Level,
    pub first_row: u64,
    pub last_row: u64,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spike {
    /// The start of the minute.
    pub minute: String,
    pub errors: u64,
    pub rows: u64,
    /// The average errors per minute across the log, for comparison.
    pub typical_errors: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub format: Format,
    pub encoding: Encoding,
    pub total_rows: u64,
    pub rows_with_timestamps: u64,
    pub levels: LevelCounts,
    pub first_timestamp: Option<String>,
    pub last_timestamp: Option<String>,
    pub distinct_patterns: u64,
    /// Rows not grouped because the pattern limit was reached.
    pub other_rows: u64,
    /// The most frequent patterns overall.
    pub top_patterns: Vec<Pattern>,
    /// Warning and higher patterns, most severe first.
    pub problem_patterns: Vec<Pattern>,
    /// Minutes with unusually many errors, busiest first.
    pub error_spikes: Vec<Spike>,
}

#[derive(Clone, Copy)]
struct Tally {
    count: u64,
    level: Level,
    first_row: u32,
    last_row: u32,
}

impl Tally {
    fn absorb(&mut self, other: Tally) {
        self.count += other.count;
        self.level = self.level.max(other.level);
        self.first_row = self.first_row.min(other.first_row);
        self.last_row = self.last_row.max(other.last_row);
    }
}

#[derive(Default)]
struct PatternCounts {
    tallies: HashMap<Vec<u8>, Tally>,
    other_rows: u64,
}

impl PatternCounts {
    fn merge(mut self, other: PatternCounts) -> PatternCounts {
        self.other_rows += other.other_rows;
        for (template, tally) in other.tallies {
            if let Some(existing) = self.tallies.get_mut(&template) {
                existing.absorb(tally);
            } else if self.tallies.len() < MAX_PATTERNS {
                self.tallies.insert(template, tally);
            } else {
                self.other_rows += tally.count;
            }
        }
        self
    }
}

fn hex_run(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .take_while(|byte| byte.is_ascii_hexdigit())
        .count()
}

fn is_guid(bytes: &[u8]) -> bool {
    const LENGTHS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut at = 0;
    for (part, length) in LENGTHS.iter().enumerate() {
        if hex_run(&bytes[at.min(bytes.len())..]) < *length {
            return false;
        }
        at += length;
        if part < 4 {
            if bytes.get(at) != Some(&b'-') {
                return false;
            }
            at += 1;
        }
    }
    !bytes.get(at).is_some_and(u8::is_ascii_alphanumeric)
}

/// Replaces the variable parts of a message, so that messages differing only
/// by numbers, codes or IDs group together. Only the first line is used.
pub fn normalize(message: &[u8], output: &mut Vec<u8>) {
    output.clear();
    let line_end = message
        .iter()
        .position(|byte| matches!(byte, b'\n' | b'\r'))
        .unwrap_or(message.len());
    let message = &message[..line_end];
    let mut at = 0;
    while at < message.len() && output.len() < MAX_TEMPLATE_BYTES {
        let rest = &message[at..];
        let starts_word = at == 0 || !message[at - 1].is_ascii_alphanumeric();
        let byte = rest[0];
        if starts_word && byte.is_ascii_hexdigit() && is_guid(rest) {
            output.extend_from_slice(b"<guid>");
            at += 36;
        } else if starts_word
            && rest.len() > 2
            && byte == b'0'
            && matches!(rest[1], b'x' | b'X')
            && rest[2].is_ascii_hexdigit()
        {
            output.extend_from_slice(b"<hex>");
            at += 2 + hex_run(&rest[2..]);
        } else if starts_word
            && hex_run(rest) >= 8
            && !rest
                .get(hex_run(rest))
                .is_some_and(u8::is_ascii_alphanumeric)
            && rest[..hex_run(rest)].iter().any(u8::is_ascii_alphabetic)
        {
            output.extend_from_slice(b"<hex>");
            at += hex_run(rest);
        } else if byte.is_ascii_digit() {
            output.extend_from_slice(b"<n>");
            at += rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
        } else {
            output.push(byte);
            at += 1;
        }
    }
}

fn stamp_text(log: &LogFile, row: u32) -> Option<String> {
    log.stamp_seconds(row as usize)
        .map(|seconds| Stamp::from_seconds(seconds).to_string())
}

fn to_pattern(
    log: &LogFile,
    template: &[u8],
    tally: &Tally,
    buffers: &mut (Vec<u8>, Vec<u8>),
) -> io::Result<Pattern> {
    let first = query::read_record(
        log,
        tally.first_row as usize,
        &mut buffers.0,
        &mut buffers.1,
    )?;
    let example = if first.message.chars().count() > MAX_EXAMPLE_CHARS {
        first.message.chars().take(MAX_EXAMPLE_CHARS).collect()
    } else {
        first.message
    };
    Ok(Pattern {
        template: encoding::display(template).into_owned(),
        example,
        count: tally.count,
        level: tally.level,
        first_row: u64::from(tally.first_row) + 1,
        last_row: u64::from(tally.last_row) + 1,
        first_seen: stamp_text(log, tally.first_row),
        last_seen: stamp_text(log, tally.last_row),
    })
}

/// Finds minutes well above the log's usual error rate.
fn find_spikes(minutes: &HashMap<u32, (u64, u64)>) -> Vec<Spike> {
    if minutes.len() < MIN_MINUTES_FOR_SPIKES {
        return Vec::new();
    }
    let count = minutes.len() as f64;
    let mean = minutes
        .values()
        .map(|(_, errors)| *errors as f64)
        .sum::<f64>()
        / count;
    let variance = minutes
        .values()
        .map(|(_, errors)| (*errors as f64 - mean).powi(2))
        .sum::<f64>()
        / count;
    // Counts are whole numbers, so the margin only guards against rounding. It
    // means perfectly steady errors, with no deviation at all, are never a spike.
    let threshold = mean + 3.0 * variance.sqrt() + 0.5;
    let mut spikes: Vec<(u32, u64, u64)> = minutes
        .iter()
        .filter(|(_, (_, errors))| *errors as f64 > threshold && *errors >= MIN_SPIKE_ERRORS)
        .map(|(minute, (rows, errors))| (*minute, *errors, *rows))
        .collect();
    spikes.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    spikes.truncate(MAX_SPIKES);
    spikes
        .into_iter()
        .map(|(minute, errors, rows)| Spike {
            minute: Stamp::from_seconds(minute * 60).to_string(),
            errors,
            rows,
            typical_errors: (mean * 100.0).round() / 100.0,
        })
        .collect()
}

/// Summarizes the whole log using all cores. `top` caps each pattern list.
pub fn summarize(log: &LogFile, top: usize, control: &Control) -> Result<Summary, QueryError> {
    let top = top.clamp(1, MAX_TOP);
    let rows = log.rows();

    let mut levels = LevelCounts::default();
    let mut rows_with_timestamps = 0;
    let (mut earliest, mut latest) = (u32::MAX, 0);
    let mut minutes: HashMap<u32, (u64, u64)> = HashMap::new();
    let mut current_minute = (u32::MAX, 0u64, 0u64);
    for row in 0..rows {
        let level = log.level(row);
        levels.add(level);
        if let Some(seconds) = log.stamp_seconds(row) {
            rows_with_timestamps += 1;
            earliest = earliest.min(seconds);
            latest = latest.max(seconds);
            let minute = seconds / 60;
            // Logs are mostly in order, so one running bucket avoids most lookups.
            if minute != current_minute.0 {
                if current_minute.0 != u32::MAX {
                    let entry = minutes.entry(current_minute.0).or_default();
                    entry.0 += current_minute.1;
                    entry.1 += current_minute.2;
                }
                current_minute = (minute, 0, 0);
            }
            current_minute.1 += 1;
            current_minute.2 += u64::from(level >= Level::Error);
        }
    }
    if current_minute.0 != u32::MAX {
        let entry = minutes.entry(current_minute.0).or_default();
        entry.0 += current_minute.1;
        entry.1 += current_minute.2;
    }

    let counts: PatternCounts = (0..rows.div_ceil(ROWS_PER_TASK))
        .into_par_iter()
        .map_init(
            || (Vec::new(), Vec::new(), Vec::new()),
            |(buffer, scratch, template), task| -> Result<PatternCounts, QueryError> {
                if control.cancel.load(Ordering::Relaxed) {
                    return Err(QueryError::Cancelled);
                }
                let start = task * ROWS_PER_TASK;
                let end = (start + ROWS_PER_TASK).min(rows);
                let mut counts = PatternCounts::default();
                log.read_rows(start..end, buffer, |row, raw, _| {
                    let text = log.text(raw, scratch);
                    normalize(log.fields(text).message, template);
                    let level = log.level(row);
                    let row = row as u32;
                    if let Some(tally) = counts.tallies.get_mut(template.as_slice()) {
                        tally.absorb(Tally {
                            count: 1,
                            level,
                            first_row: row,
                            last_row: row,
                        });
                    } else if counts.tallies.len() < MAX_PATTERNS_PER_TASK {
                        counts.tallies.insert(
                            template.clone(),
                            Tally {
                                count: 1,
                                level,
                                first_row: row,
                                last_row: row,
                            },
                        );
                    } else {
                        counts.other_rows += 1;
                    }
                })?;
                control.rows_done.fetch_add(end - start, Ordering::Relaxed);
                Ok(counts)
            },
        )
        .try_reduce(PatternCounts::default, |left, right| Ok(left.merge(right)))?;

    let mut by_count: Vec<(&Vec<u8>, &Tally)> = counts.tallies.iter().collect();
    by_count.sort_by(|a, b| {
        b.1.count
            .cmp(&a.1.count)
            .then(a.1.first_row.cmp(&b.1.first_row))
    });
    let mut buffers = (Vec::new(), Vec::new());
    let top_patterns = by_count
        .iter()
        .take(top)
        .map(|(template, tally)| to_pattern(log, template, tally, &mut buffers))
        .collect::<io::Result<_>>()?;

    let mut by_severity: Vec<&(&Vec<u8>, &Tally)> = by_count
        .iter()
        .filter(|(_, tally)| tally.level >= Level::Warning)
        .collect();
    by_severity.sort_by(|a, b| {
        b.1.level
            .cmp(&a.1.level)
            .then(b.1.count.cmp(&a.1.count))
            .then(a.1.first_row.cmp(&b.1.first_row))
    });
    let problem_patterns = by_severity
        .iter()
        .take(top)
        .map(|(template, tally)| to_pattern(log, template, tally, &mut buffers))
        .collect::<io::Result<_>>()?;

    let has_times = rows_with_timestamps > 0;
    Ok(Summary {
        format: log.format(),
        encoding: log.encoding(),
        total_rows: rows as u64,
        rows_with_timestamps,
        levels,
        first_timestamp: has_times.then(|| Stamp::from_seconds(earliest).to_string()),
        last_timestamp: has_times.then(|| Stamp::from_seconds(latest).to_string()),
        distinct_patterns: counts.tallies.len() as u64,
        other_rows: counts.other_rows,
        top_patterns,
        problem_patterns,
        error_spikes: find_spikes(&minutes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;
    use std::sync::atomic::AtomicU64;

    fn open(text: &str) -> (tempfile::TempDir, LogFile) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.log");
        std::fs::write(&path, text).unwrap();
        let (log, _) = LogFile::open_with_cache(&path, None, &AtomicU64::new(0)).unwrap();
        (dir, log)
    }

    fn normalized(text: &str) -> String {
        let mut output = Vec::new();
        normalize(text.as_bytes(), &mut output);
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn normalize_replaces_variable_parts() {
        assert_eq!(normalized("retry 3 of 10"), "retry <n> of <n>");
        assert_eq!(
            normalized("error 0x80004005 returned"),
            "error <hex> returned"
        );
        assert_eq!(normalized("update KB5034441 failed"), "update KB<n> failed");
        assert_eq!(
            normalized("id {6F9619FF-8B86-D011-B42D-00C04FC964FF} seen"),
            "id {<guid>} seen"
        );
        assert_eq!(normalized("hash deadbeef12 ok"), "hash <hex> ok");
    }

    #[test]
    fn normalize_leaves_ordinary_words_alone() {
        assert_eq!(normalized("added feed cafe"), "added feed cafe");
        assert_eq!(normalized("no numbers here"), "no numbers here");
    }

    #[test]
    fn normalize_uses_only_the_first_line() {
        assert_eq!(normalized("first\r\nsecond"), "first");
    }

    #[test]
    fn normalize_caps_template_length() {
        assert!(normalized(&"x".repeat(5000)).len() <= MAX_TEMPLATE_BYTES);
    }

    #[test]
    fn normalize_handles_truncated_tokens() {
        for text in ["0x", "0", "deadbeef", "6F9619FF-8B86", "6F9619FF-", ""] {
            let _ = normalized(text);
        }
    }

    #[test]
    fn groups_rows_into_patterns_with_their_extent() {
        let (_dir, log) = open(
            "2026-01-01 10:00:00 host a[1]: retry 1 failed\n2026-01-01 10:00:01 host a[1]: all good\n2026-01-01 10:00:02 host a[1]: retry 2 failed\n",
        );
        let summary = summarize(&log, 10, &Control::default()).unwrap();
        assert_eq!(summary.total_rows, 3);
        assert_eq!(summary.levels.error, 2);
        assert_eq!(summary.levels.info, 1);
        assert_eq!(summary.distinct_patterns, 2);

        let top = &summary.top_patterns[0];
        assert_eq!(top.template, "retry <n> failed");
        assert_eq!(top.example, "retry 1 failed");
        assert_eq!(top.count, 2);
        assert_eq!((top.first_row, top.last_row), (1, 3));
        assert_eq!(top.first_seen.as_deref(), Some("2026-01-01T10:00:00.000"));
        assert_eq!(top.last_seen.as_deref(), Some("2026-01-01T10:00:02.000"));
        assert_eq!(summary.problem_patterns.len(), 1);
        assert_eq!(
            summary.first_timestamp.as_deref(),
            Some("2026-01-01T10:00:00.000")
        );
    }

    #[test]
    fn problem_patterns_list_the_most_severe_first() {
        let (_dir, log) = open("retrying now\nretrying now\nretrying now\nit failed\n");
        let summary = summarize(&log, 10, &Control::default()).unwrap();
        assert_eq!(summary.problem_patterns[0].template, "it failed");
        assert_eq!(summary.problem_patterns[1].template, "retrying now");
        assert_eq!(summary.top_patterns[0].template, "retrying now");
    }

    #[test]
    fn a_long_example_is_cut_short() {
        let (_dir, log) = open(&format!(
            "{}
",
            "x".repeat(2000)
        ));
        let summary = summarize(&log, 10, &Control::default()).unwrap();
        assert_eq!(summary.top_patterns[0].example.len(), MAX_EXAMPLE_CHARS);
    }

    #[test]
    fn the_top_option_caps_the_lists() {
        let text: String = (0..50)
            .map(|n| format!("kind{}x message\n", (b'a' + n % 26) as char))
            .collect();
        let (_dir, log) = open(&text);
        let summary = summarize(&log, 3, &Control::default()).unwrap();
        assert_eq!(summary.top_patterns.len(), 3);
        assert!(summary.distinct_patterns > 3);
    }

    #[test]
    fn finds_an_error_spike_in_one_minute() {
        let mut text = String::new();
        for minute in 0..30 {
            writeln!(text, "2026-01-01 10:{minute:02}:00 host a[1]: all good").unwrap();
        }
        for second in 0..40 {
            writeln!(text, "2026-01-01 10:15:{second:02} host a[1]: disk failed").unwrap();
        }
        let (_dir, log) = open(&text);
        let summary = summarize(&log, 10, &Control::default()).unwrap();
        assert_eq!(summary.error_spikes.len(), 1);
        let spike = &summary.error_spikes[0];
        assert_eq!(spike.minute, "2026-01-01T10:15:00.000");
        assert_eq!(spike.errors, 40);
    }

    #[test]
    fn steady_errors_are_not_a_spike() {
        let mut text = String::new();
        for minute in 0..30 {
            for second in 0..6 {
                writeln!(
                    text,
                    "2026-01-01 10:{minute:02}:{second:02} host a[1]: disk failed"
                )
                .unwrap();
            }
        }
        let (_dir, log) = open(&text);
        assert!(summarize(&log, 10, &Control::default())
            .unwrap()
            .error_spikes
            .is_empty());
    }

    #[test]
    fn an_empty_log_summarizes_cleanly() {
        let (_dir, log) = open("");
        let summary = summarize(&log, 10, &Control::default()).unwrap();
        assert_eq!(summary.total_rows, 0);
        assert_eq!(summary.first_timestamp, None);
        assert!(summary.top_patterns.is_empty());
    }

    #[test]
    fn cancelling_stops_the_summary() {
        let (_dir, log) = open("a\nb\n");
        let control = Control::default();
        control.cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            summarize(&log, 10, &control).unwrap_err().code(),
            "cancelled"
        );
    }

    #[test]
    fn summaries_serialize_to_json() {
        let (_dir, log) = open("it failed\n");
        let summary = summarize(&log, 10, &Control::default()).unwrap();
        let json = serde_json::to_string(&summary).unwrap();
        assert_eq!(serde_json::from_str::<Summary>(&json).unwrap(), summary);
    }
}
