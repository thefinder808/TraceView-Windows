//! Row index: one packed file offset and one severity byte per row.
//!
//! A row is a physical line in plain logs. In SCCM logs a row is a whole
//! `<![LOG[...]LOG]!>` entry, because messages can span several lines.

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use memchr::memmem;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

const CHUNK_SIZE: usize = 16 * 1024 * 1024;
const SCCM_START: &[u8] = b"<![LOG[";
const SCCM_END: &[u8] = b"]LOG]!>";
const SCCM_NEXT: &[u8] = b"\n<![LOG[";

/// Packed offsets address up to 1 TiB, which is checked when a file is opened.
pub const MAX_FILE_LEN: u64 = 1 << 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Format {
    Plain = 0,
    Sccm = 1,
}

impl Format {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Format::Plain),
            1 => Some(Format::Sccm),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum Level {
    Info = 0,
    Warning = 1,
    Error = 2,
    Critical = 3,
}

impl Level {
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => Level::Warning,
            2 => Level::Error,
            3 => Level::Critical,
            _ => Level::Info,
        }
    }
}

pub struct RowIndex {
    pub format: Format,
    pub(crate) offsets: Vec<[u8; 5]>,
    pub(crate) levels: Vec<u8>,
}

impl RowIndex {
    pub fn rows(&self) -> usize {
        self.offsets.len()
    }

    pub fn level(&self, row: usize) -> Level {
        Level::from_u8(self.levels[row])
    }

    pub fn memory_bytes(&self) -> usize {
        self.offsets.len() * 5 + self.levels.len()
    }

    /// Byte range of a row, clamped so a damaged cache can never read out of bounds.
    pub fn range(&self, row: usize, file_len: usize) -> (usize, usize) {
        let start = unpack(self.offsets[row]).min(file_len);
        let end = self
            .offsets
            .get(row + 1)
            .map_or(file_len, |next| unpack(*next).min(file_len));
        (start, end.max(start))
    }
}

fn pack(offset: usize) -> [u8; 5] {
    let bytes = (offset as u64).to_le_bytes();
    [bytes[0], bytes[1], bytes[2], bytes[3], bytes[4]]
}

fn unpack(packed: [u8; 5]) -> usize {
    u64::from_le_bytes([packed[0], packed[1], packed[2], packed[3], packed[4], 0, 0, 0]) as usize
}

/// Treats a file as SCCM only when its head contains a complete entry.
pub fn detect_format(bytes: &[u8]) -> Format {
    let head = &bytes[..bytes.len().min(64 * 1024)];
    let starts_entry = head.starts_with(SCCM_START)
        || head.starts_with(b"\xEF\xBB\xBF<![LOG[")
        || memmem::find(head, SCCM_NEXT).is_some();
    if starts_entry && memmem::find(head, SCCM_END).is_some() {
        Format::Sccm
    } else {
        Format::Plain
    }
}

pub fn build(bytes: &[u8], progress: &AtomicU64) -> RowIndex {
    build_with_chunk_size(bytes, CHUNK_SIZE, progress)
}

pub(crate) fn build_with_chunk_size(
    bytes: &[u8],
    chunk_size: usize,
    progress: &AtomicU64,
) -> RowIndex {
    let format = detect_format(bytes);
    let keywords = severity_keywords();
    let chunks: Vec<(Vec<[u8; 5]>, Vec<u8>)> = (0..bytes.len().div_ceil(chunk_size))
        .into_par_iter()
        .map(|chunk| {
            let start = chunk * chunk_size;
            let end = (start + chunk_size).min(bytes.len());
            let rows = match format {
                Format::Plain => scan_plain(bytes, start, end, &keywords),
                Format::Sccm => scan_sccm(bytes, start, end),
            };
            progress.fetch_add((end - start) as u64, Ordering::Relaxed);
            rows
        })
        .collect();

    let rows = chunks.iter().map(|(offsets, _)| offsets.len()).sum();
    let mut offsets = Vec::with_capacity(rows);
    let mut levels = Vec::with_capacity(rows);
    for (chunk_offsets, chunk_levels) in chunks {
        offsets.extend_from_slice(&chunk_offsets);
        levels.extend_from_slice(&chunk_levels);
    }
    RowIndex {
        format,
        offsets,
        levels,
    }
}

/// Indexes the rows that start inside `start..end`. A row can run past `end`,
/// and the chunk that holds its first byte owns it.
fn scan_plain(
    bytes: &[u8],
    start: usize,
    end: usize,
    keywords: &AhoCorasick,
) -> (Vec<[u8; 5]>, Vec<u8>) {
    let mut offsets = Vec::new();
    let mut levels = Vec::new();
    let mut pos = if start == 0 {
        0
    } else {
        match memchr::memchr(b'\n', &bytes[start - 1..]) {
            Some(found) => start + found,
            None => return (offsets, levels),
        }
    };
    while pos < end {
        let line_end = memchr::memchr(b'\n', &bytes[pos..]).map_or(bytes.len(), |found| pos + found);
        offsets.push(pack(pos));
        levels.push(plain_level(&bytes[pos..line_end], keywords) as u8);
        pos = line_end + 1;
    }
    (offsets, levels)
}

fn scan_sccm(bytes: &[u8], start: usize, end: usize) -> (Vec<[u8; 5]>, Vec<u8>) {
    let next_entry = memmem::Finder::new(SCCM_NEXT);
    let mut offsets = Vec::new();
    let mut levels = Vec::new();
    let mut pos = if start == 0 {
        0
    } else {
        match next_entry.find(&bytes[start - 1..]) {
            Some(found) => start + found,
            None => return (offsets, levels),
        }
    };
    while pos < end {
        let entry_end = next_entry
            .find(&bytes[pos..])
            .map_or(bytes.len(), |found| pos + found + 1);
        offsets.push(pack(pos));
        levels.push(sccm_level(&bytes[pos..entry_end]) as u8);
        pos = entry_end;
    }
    (offsets, levels)
}

fn severity_keywords() -> AhoCorasick {
    AhoCorasickBuilder::new()
        .ascii_case_insensitive(true)
        .match_kind(MatchKind::LeftmostFirst)
        .build(["critical", "fatal", "error", "fail", "warn"])
        .expect("static severity keywords always compile")
}

fn plain_level(line: &[u8], keywords: &AhoCorasick) -> Level {
    keywords
        .find_iter(line)
        .map(|found| match found.pattern().as_usize() {
            0 | 1 => Level::Critical,
            2 | 3 => Level::Error,
            _ => Level::Warning,
        })
        .max()
        .unwrap_or(Level::Info)
}

/// The attribute sits after the message, so searching from the tail avoids
/// matching `type="3"` text inside a message.
fn sccm_level(entry: &[u8]) -> Level {
    let tail = &entry[entry.len().saturating_sub(512)..];
    let digit = memmem::rfind(tail, b" type=\"").and_then(|found| tail.get(found + 7));
    match digit {
        Some(b'2') => Level::Warning,
        Some(b'3') => Level::Error,
        _ => Level::Info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(bytes: &[u8], chunk_size: usize) -> Vec<Vec<u8>> {
        let index = build_with_chunk_size(bytes, chunk_size, &AtomicU64::new(0));
        (0..index.rows())
            .map(|row| {
                let (start, end) = index.range(row, bytes.len());
                bytes[start..end].to_vec()
            })
            .collect()
    }

    #[test]
    fn chunk_size_never_changes_plain_rows() {
        let log = b"one\r\n\nthree error\nfour";
        let expected = rows(log, 1024);
        assert_eq!(expected.len(), 4);
        assert_eq!(expected[1], b"\n");
        for chunk_size in 1..log.len() {
            assert_eq!(rows(log, chunk_size), expected, "chunk size {chunk_size}");
        }
    }

    #[test]
    fn trailing_newline_does_not_add_a_row() {
        assert_eq!(rows(b"one\ntwo\n", 3).len(), 2);
    }

    #[test]
    fn empty_input_has_no_rows() {
        assert_eq!(rows(b"", 3).len(), 0);
    }

    #[test]
    fn sccm_multiline_entry_is_one_row() {
        let log = b"<![LOG[first\r\nsecond]LOG]!><time=\"1\" type=\"3\" thread=\"1\">\r\n<![LOG[next]LOG]!><time=\"2\" type=\"2\" thread=\"1\">\r\n";
        let expected = rows(log, 1024);
        assert_eq!(expected.len(), 2);
        assert!(expected[0].windows(6).any(|window| window == b"second"));
        for chunk_size in 1..log.len() {
            assert_eq!(rows(log, chunk_size), expected, "chunk size {chunk_size}");
        }
        let index = build_with_chunk_size(log, 7, &AtomicU64::new(0));
        assert_eq!(index.level(0), Level::Error);
        assert_eq!(index.level(1), Level::Warning);
    }

    #[test]
    fn sccm_level_ignores_type_text_inside_the_message() {
        let entry = b"<![LOG[saw type=\"3\" here]LOG]!><time=\"1\" type=\"1\" thread=\"1\">";
        assert_eq!(sccm_level(entry), Level::Info);
    }

    #[test]
    fn plain_level_keeps_the_highest_severity() {
        let keywords = severity_keywords();
        assert_eq!(plain_level(b"warn then FATAL", &keywords), Level::Critical);
        assert_eq!(plain_level(b"retry warning", &keywords), Level::Warning);
        assert_eq!(plain_level(b"all fine", &keywords), Level::Info);
    }

    #[test]
    fn range_clamps_offsets_past_the_end() {
        let index = RowIndex {
            format: Format::Plain,
            offsets: vec![pack(100), pack(200)],
            levels: vec![0, 0],
        };
        assert_eq!(index.range(0, 10), (10, 10));
    }

    #[test]
    fn offsets_round_trip_at_the_size_limit() {
        let largest = (MAX_FILE_LEN - 1) as usize;
        assert_eq!(unpack(pack(largest)), largest);
    }
}
