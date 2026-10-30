//! Severity levels and the keyword fallback for logs that don't state a level.
//! The keyword rules match the macOS app, so both apps classify a log alike.

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// Only this much of a message is scanned for keywords.
const MAX_SCAN: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum Level {
    Debug = 0,
    Info = 1,
    Notice = 2,
    Warning = 3,
    Error = 4,
    Critical = 5,
}

impl Level {
    pub const ALL: [Level; 6] = [
        Level::Debug,
        Level::Info,
        Level::Notice,
        Level::Warning,
        Level::Error,
        Level::Critical,
    ];

    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Level::Debug,
            2 => Level::Notice,
            3 => Level::Warning,
            4 => Level::Error,
            5 => Level::Critical,
            _ => Level::Info,
        }
    }

    /// Recognizes a level written out in a log, in any case.
    pub fn from_name(name: &[u8]) -> Option<Self> {
        const NAMES: [(&[u8], Level); 10] = [
            (b"debug", Level::Debug),
            (b"info", Level::Info),
            (b"notice", Level::Notice),
            (b"warn", Level::Warning),
            (b"warning", Level::Warning),
            (b"error", Level::Error),
            (b"err", Level::Error),
            (b"critical", Level::Critical),
            (b"fatal", Level::Critical),
            (b"crit", Level::Critical),
        ];
        NAMES
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(name))
            .map(|(_, level)| *level)
    }
}

const KEYWORDS: [(&str, Level); 22] = [
    ("error", Level::Error),
    ("failed", Level::Error),
    ("failure", Level::Error),
    ("fatal", Level::Error),
    ("crash", Level::Error),
    ("crashed", Level::Error),
    ("panic", Level::Error),
    ("abort", Level::Error),
    ("aborted", Level::Error),
    ("exception", Level::Error),
    ("critical", Level::Error),
    ("severe", Level::Error),
    ("warn", Level::Warning),
    ("warning", Level::Warning),
    ("caution", Level::Warning),
    ("deprecated", Level::Warning),
    ("retrying", Level::Warning),
    ("timeout", Level::Warning),
    ("timed out", Level::Warning),
    ("debug", Level::Debug),
    ("trace", Level::Debug),
    ("verbose", Level::Debug),
];

fn keywords() -> &'static AhoCorasick {
    static KEYWORD_MATCHER: OnceLock<AhoCorasick> = OnceLock::new();
    KEYWORD_MATCHER.get_or_init(|| {
        AhoCorasickBuilder::new()
            .ascii_case_insensitive(true)
            .match_kind(MatchKind::LeftmostLongest)
            .build(KEYWORDS.iter().map(|(word, _)| *word))
            .expect("static keywords always compile")
    })
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

/// Picks a level from whole-word keywords. Errors beat warnings, which beat debug.
pub fn detect(text: &[u8]) -> Level {
    let text = &text[..text.len().min(MAX_SCAN)];
    let mut found: Option<Level> = None;
    for hit in keywords().find_iter(text) {
        let starts_word = hit.start() == 0 || !is_word_byte(text[hit.start() - 1]);
        let ends_word = hit.end() == text.len() || !is_word_byte(text[hit.end()]);
        if !(starts_word && ends_word) {
            continue;
        }
        let level = KEYWORDS[hit.pattern().as_usize()].1;
        if level == Level::Error {
            return Level::Error;
        }
        // Warning outranks debug, even though debug's number is lower than info's.
        found = Some(found.map_or(level, |current| current.max(level)));
    }
    found.unwrap_or(Level::Info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_whole_words_only() {
        assert_eq!(detect(b"an error occurred"), Level::Error);
        assert_eq!(detect(b"0 errors found"), Level::Info);
        assert_eq!(detect(b"terror"), Level::Info);
        assert_eq!(detect(b"tracert finished"), Level::Info);
    }

    #[test]
    fn matching_ignores_case() {
        assert_eq!(detect(b"FAILED to start"), Level::Error);
        assert_eq!(detect(b"Timed Out waiting"), Level::Warning);
    }

    #[test]
    fn higher_severity_wins_wherever_it_appears() {
        assert_eq!(detect(b"debug: warning then error"), Level::Error);
        assert_eq!(detect(b"trace warning"), Level::Warning);
        assert_eq!(detect(b"warning trace"), Level::Warning);
    }

    #[test]
    fn longer_forms_count_as_words() {
        assert_eq!(detect(b"process crashed"), Level::Error);
        assert_eq!(detect(b"job aborted"), Level::Error);
    }

    #[test]
    fn plain_text_is_info() {
        assert_eq!(detect(b"all fine"), Level::Info);
        assert_eq!(detect(b""), Level::Info);
    }

    #[test]
    fn level_numbers_round_trip() {
        for level in Level::ALL {
            assert_eq!(Level::from_u8(level as u8), level);
        }
    }

    #[test]
    fn level_names_ignore_case() {
        assert_eq!(Level::from_name(b"WARN"), Some(Level::Warning));
        assert_eq!(Level::from_name(b"Fatal"), Some(Level::Critical));
        assert_eq!(Level::from_name(b"Information"), None);
    }
}
