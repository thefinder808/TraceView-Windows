//! Formats a person defines with a regex, for logs TraceView doesn't know.
//! Named groups say where each part of a row is: `time`, `level`,
//! `component`, `thread`, `source` and `message`. Each line is a row.
//!
//! Patterns run in the `regex` crate's linear-time engine, as searches do, so
//! a shared definition can't hang a scan or run code.

use crate::level::{self, Level};
use crate::parse::{Fields, ParseContext};
use crate::reader::fnv1a;
use crate::time;
use regex::bytes::{Regex, RegexBuilder};
use std::sync::Arc;

/// The parts a pattern can name, in the order `CustomFormat::parts` holds them.
pub const PARTS: [&str; 6] = ["time", "level", "component", "thread", "source", "message"];
const TIME: usize = 0;
const LEVEL: usize = 1;
const COMPONENT: usize = 2;
const THREAD: usize = 3;
const SOURCE: usize = 4;
const MESSAGE: usize = 5;
/// Keeps a pattern small, so a hand-edited file can't make one that uses a
/// lot of memory.
const SIZE_LIMIT: usize = 1 << 20;
/// How many of a log's first lines are checked when choosing a format by
/// what's in it, and how many of them must match.
const SAMPLE_LINES: usize = 20;
const MATCH_PERCENT: usize = 80;

#[derive(Debug)]
pub struct CustomFormat {
    name: String,
    pattern: Regex,
    /// File names it's for, as lowercase wildcard patterns such as `u_ex*.log`.
    files: Vec<String>,
    day_first: bool,
    /// Changes whenever parsing would, so a cached index made with an older
    /// pattern is never reused.
    key: u64,
    /// The capture group of each of [`PARTS`], if the pattern names it.
    parts: [Option<usize>; 6],
}

impl CustomFormat {
    /// `files` is a list of names separated by `;`, where `*` stands for any
    /// characters and `?` for one. It can be empty.
    pub fn new(name: &str, pattern: &str, files: &str, day_first: bool) -> Result<Self, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("A format needs a name".to_string());
        }
        let regex = RegexBuilder::new(pattern)
            .size_limit(SIZE_LIMIT)
            .build()
            .map_err(|error| error.to_string())?;
        let mut parts = [None; 6];
        for (slot, part) in parts.iter_mut().zip(PARTS) {
            *slot = regex.capture_names().position(|name| name == Some(part));
        }
        if parts.iter().all(Option::is_none) {
            return Err(
                "Name at least one part, as in (?P<time>...) or (?P<message>...)".to_string(),
            );
        }
        let files = files
            .split(';')
            .map(|file| file.trim().to_lowercase())
            .filter(|file| !file.is_empty())
            .collect();
        let key = fnv1a(
            fnv1a(0xcbf2_9ce4_8422_2325, pattern.as_bytes()),
            &[u8::from(day_first)],
        );
        Ok(Self {
            name: name.to_string(),
            pattern: regex,
            files,
            day_first,
            // Zero means a built-in format.
            key: key.max(1),
            parts,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn key(&self) -> u64 {
        self.key
    }

    /// Whether the pattern names a part, such as `thread`.
    pub fn has_part(&self, part: &str) -> bool {
        PARTS
            .iter()
            .position(|known| *known == part)
            .is_some_and(|index| self.parts[index].is_some())
    }

    /// Whether a file name, without its folder, is one this format is for.
    pub fn matches_file(&self, file_name: &str) -> bool {
        let name = file_name.to_lowercase();
        self.files
            .iter()
            .any(|pattern| wildcard(pattern.as_bytes(), name.as_bytes()))
    }

    pub fn matches(&self, line: &[u8]) -> bool {
        self.pattern.is_match(line)
    }

    /// A line's parts, or `None` if the pattern doesn't match it. Without a
    /// `message` part the whole line is the message, and without a level the
    /// message's words decide it, as for any log.
    pub fn fields<'a>(&self, line: &'a [u8], context: &ParseContext) -> Option<Fields<'a>> {
        let captures = self.pattern.captures(line)?;
        let part = |index: usize| {
            self.parts[index]
                .and_then(|group| captures.get(group))
                .map(|found| found.as_bytes())
        };
        let message = part(MESSAGE).unwrap_or(line);
        let level = part(LEVEL)
            .and_then(|word| Level::from_name(word.trim_ascii()))
            .unwrap_or_else(|| level::detect(message));
        Some(Fields {
            stamp: part(TIME)
                .and_then(|text| time::parse_any(text, context.year_hint, self.day_first)),
            level,
            component: part(COMPONENT).unwrap_or(b""),
            thread: part(THREAD).unwrap_or(b""),
            source: part(SOURCE).unwrap_or(b""),
            message,
        })
    }
}

/// Matches `*` and `?` wildcards against a whole name.
fn wildcard(pattern: &[u8], name: &[u8]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some((b'*', rest)) => (0..=name.len()).any(|skip| wildcard(rest, &name[skip..])),
        Some((b'?', rest)) => !name.is_empty() && wildcard(rest, &name[1..]),
        Some((first, rest)) => name.first() == Some(first) && wildcard(rest, &name[1..]),
    }
}

/// The first format meant for this file name.
pub(crate) fn for_file_name<'a>(
    formats: &'a [Arc<CustomFormat>],
    file_name: &str,
) -> Option<&'a Arc<CustomFormat>> {
    formats.iter().find(|format| format.matches_file(file_name))
}

/// The first format whose pattern matches most of a log's first lines.
/// `complete` says whether `head` holds the whole file, since otherwise its
/// last line may be cut short.
pub(crate) fn for_contents<'a>(
    formats: &'a [Arc<CustomFormat>],
    head: &[u8],
    complete: bool,
) -> Option<&'a Arc<CustomFormat>> {
    if formats.is_empty() {
        return None;
    }
    let mut lines: Vec<&[u8]> = head.split(|byte| *byte == b'\n').collect();
    if !complete {
        lines.pop();
    }
    let lines: Vec<&[u8]> = lines
        .into_iter()
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .filter(|line| !line.trim_ascii().is_empty())
        .take(SAMPLE_LINES)
        .collect();
    if lines.is_empty() {
        return None;
    }
    formats.iter().find(|format| {
        let matched = lines.iter().filter(|line| format.matches(line)).count();
        matched * 100 >= lines.len() * MATCH_PERCENT
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::YearHint;

    const CONTEXT: ParseContext = ParseContext {
        year_hint: YearHint {
            year: 2026,
            month: 6,
        },
        custom: None,
    };
    const IIS: &str = r"^(?P<time>\S+ \S+) (?P<component>\S+) (?P<level>\w+) (?P<message>.*)$";

    fn iis() -> CustomFormat {
        CustomFormat::new("IIS", IIS, "u_ex*.log; iis?.txt", false).unwrap()
    }

    #[test]
    fn named_parts_become_fields() {
        let line = b"2026-01-31 10:22:01 W3SVC1 WARN slow request to /api";
        let fields = iis().fields(line, &CONTEXT).unwrap();
        assert_eq!(
            fields.stamp.map(|stamp| stamp.to_string()).as_deref(),
            Some("2026-01-31T10:22:01.000")
        );
        assert_eq!(fields.component, b"W3SVC1");
        assert_eq!(fields.level, Level::Warning);
        assert_eq!(fields.message, b"slow request to /api");
        assert!(iis().fields(b"#Fields: date time", &CONTEXT).is_none());
    }

    #[test]
    fn without_a_message_or_level_part_the_line_decides() {
        let format =
            CustomFormat::new("Stamped", r"^(?P<time>\d{2}/\d{2}/\d{4} \S+)", "", false).unwrap();
        let line = b"01/31/2026 10:22:01 the download failed";
        let fields = format.fields(line, &CONTEXT).unwrap();
        assert_eq!(fields.message, &line[..]);
        assert_eq!(fields.level, Level::Error);
        assert!(fields.stamp.is_some());
        assert!(format.has_part("time") && !format.has_part("thread"));
    }

    #[test]
    fn mistakes_are_reported() {
        assert!(CustomFormat::new("", IIS, "", false).is_err());
        assert!(CustomFormat::new("Broken", "(?P<time>", "", false).is_err());
        let unnamed = CustomFormat::new("Unnamed", r"^(\S+) (.*)$", "", false).unwrap_err();
        assert!(unnamed.starts_with("Name at least one part"), "{unnamed}");
        assert!(CustomFormat::new("Huge", "\\w{1000}{1000}", "", false).is_err());
    }

    #[test]
    fn file_names_match_wildcards_in_any_case() {
        let format = iis();
        assert!(format.matches_file("U_EX260131.LOG"));
        assert!(format.matches_file("iis1.txt"));
        assert!(!format.matches_file("iis12.txt"));
        assert!(!format.matches_file("u_ex260131.log.old"));
        let none = CustomFormat::new("None", IIS, "  ;  ", false).unwrap();
        assert!(!none.matches_file("anything.log"));
    }

    #[test]
    fn a_format_is_chosen_when_most_lines_match() {
        let formats = vec![Arc::new(iis())];
        let mostly: String = (0..10)
            .map(|row| {
                if row == 3 {
                    "a stray line\n".to_string()
                } else {
                    format!("2026-01-31 10:22:0{row} W3SVC1 INFO request {row}\n")
                }
            })
            .collect();
        assert!(for_contents(&formats, mostly.as_bytes(), true).is_some());
        let half = "2026-01-31 10:22:01 W3SVC1 INFO a\nnot it\nnot it either\n";
        assert!(for_contents(&formats, half.as_bytes(), true).is_none());
        // A cut-off last line doesn't count against it.
        let cut = "2026-01-31 10:22:01 W3SVC1 INFO a\n2026-01-31 10:22:02 W3SVC1 INFO b\n2026-01-3";
        assert!(for_contents(&formats, cut.as_bytes(), false).is_some());
        assert!(for_contents(&formats, b"", true).is_none());
        assert!(for_contents(&[], mostly.as_bytes(), true).is_none());
    }

    #[test]
    fn the_key_changes_with_the_pattern_but_not_the_name() {
        let first = iis();
        let renamed = CustomFormat::new("Web", IIS, "", false).unwrap();
        let day_first = CustomFormat::new("IIS", IIS, "", true).unwrap();
        assert_eq!(first.key(), renamed.key());
        assert_ne!(first.key(), day_first.key());
        assert_ne!(first.key(), 0);
    }
}
