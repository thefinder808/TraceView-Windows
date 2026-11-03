//! Row index: compact arrays with one file offset, severity and timestamp per
//! row. A row is a line in plain logs and a whole `<![LOG[...]LOG]!>` entry in
//! SCCM logs, because SCCM messages can span several lines.

use crate::encoding::Encoding;
use crate::level::Level;
use crate::parse::{self, Format, ParseContext};
use crate::reader;
use crate::time::Stamp;
use memchr::memmem::Finder;
use rayon::prelude::*;
use std::fs::File;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

const CHUNK_SIZE: usize = 16 * 1024 * 1024;
/// How far past its chunk a row is read to analyze it. Longer rows are still
/// indexed correctly, but their level and time come from this much of the row.
const LOOKAHEAD: usize = 1024 * 1024;

/// Packed offsets address up to 1 TiB, which is checked when a file is opened.
pub const MAX_FILE_LEN: u64 = 1 << 40;
/// Bytes of index memory per row: offset, level and time.
pub const BYTES_PER_ROW: usize = 5 + 1 + 4;
/// The least room left for new rows when arrays are copied to grow them, so a
/// small live log isn't copied on every refresh.
const MIN_ROOM_ROWS: usize = 4096;

/// How a file's bytes are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub encoding: Encoding,
    pub format: Format,
    /// Where text starts, after any byte order mark.
    pub body_offset: u64,
    /// Which custom format definition the rows were read with, or 0 for a
    /// built-in format, so an index is never reused after a pattern changes.
    pub format_key: u64,
}

#[derive(Debug, Default)]
pub struct RowArrays {
    pub(crate) offsets: Vec<[u8; 5]>,
    pub(crate) levels: Vec<u8>,
    /// Seconds since 2000-01-01 plus one, with zero meaning no timestamp.
    pub(crate) times: Vec<u32>,
}

impl RowArrays {
    pub fn rows(&self) -> usize {
        self.offsets.len()
    }

    pub fn level(&self, row: usize) -> Level {
        Level::from_u8(self.levels[row])
    }

    pub fn stamp_seconds(&self, row: usize) -> Option<u32> {
        self.times[row].checked_sub(1)
    }

    pub fn offset(&self, row: usize) -> u64 {
        unpack(self.offsets[row])
    }

    pub fn memory_bytes(&self) -> usize {
        self.rows() * BYTES_PER_ROW
    }

    fn append(&mut self, other: RowArrays) {
        self.offsets.extend_from_slice(&other.offsets);
        self.levels.extend_from_slice(&other.levels);
        self.times.extend_from_slice(&other.times);
    }

    pub(crate) fn truncate(&mut self, rows: usize) {
        self.offsets.truncate(rows);
        self.levels.truncate(rows);
        self.times.truncate(rows);
    }

    pub(crate) fn extend(&mut self, other: RowArrays) {
        self.append(other);
    }

    /// Whether the arrays can hold `rows` rows without reallocating.
    pub(crate) fn has_room_for(&self, rows: usize) -> bool {
        self.offsets.capacity() >= rows
            && self.levels.capacity() >= rows
            && self.times.capacity() >= rows
    }

    /// The first `keep` rows followed by `added`, in new arrays with room to
    /// grow by an eighth. Growing a 27-million-row index this way takes about
    /// 55 ms, which a refresh does before it needs exclusive access to the log,
    /// and a live log isn't copied again until it has grown by that much.
    pub(crate) fn copy_with_room(&self, keep: usize, added: RowArrays) -> RowArrays {
        let rows = keep + added.rows();
        let capacity = rows + (rows / 8).max(MIN_ROOM_ROWS);
        let mut copy = RowArrays {
            offsets: Vec::with_capacity(capacity),
            levels: Vec::with_capacity(capacity),
            times: Vec::with_capacity(capacity),
        };
        copy.offsets.extend_from_slice(&self.offsets[..keep]);
        copy.levels.extend_from_slice(&self.levels[..keep]);
        copy.times.extend_from_slice(&self.times[..keep]);
        copy.append(added);
        copy
    }

    fn push(&mut self, offset: u64, level: Level, stamp: Option<Stamp>) {
        self.offsets.push(pack(offset));
        self.levels.push(level as u8);
        self.times
            .push(stamp.map_or(0, |stamp| stamp.seconds.saturating_add(1)));
    }
}

fn pack(offset: u64) -> [u8; 5] {
    let bytes = offset.to_le_bytes();
    [bytes[0], bytes[1], bytes[2], bytes[3], bytes[4]]
}

fn unpack(packed: [u8; 5]) -> u64 {
    u64::from_le_bytes([
        packed[0], packed[1], packed[2], packed[3], packed[4], 0, 0, 0,
    ])
}

/// Everything a chunk needs in order to find and analyze rows.
struct Scanner<'a> {
    file: &'a File,
    len: u64,
    layout: Layout,
    context: &'a ParseContext,
    from: u64,
    chunk_size: usize,
    lookahead: usize,
    separator: Finder<'static>,
    newline_len: usize,
}

impl Scanner<'_> {
    /// Finds the next separator at a code-unit boundary. `base` is the file
    /// offset of `buffer[0]`, and the search starts at `start` in the buffer.
    fn find_separator(&self, buffer: &[u8], base: u64, start: usize) -> Option<usize> {
        let unit = self.layout.encoding.unit() as u64;
        let mut search = start;
        loop {
            let found = search + self.separator.find(buffer.get(search..)?)?;
            let offset = base + found as u64;
            if offset
                .saturating_sub(self.layout.body_offset)
                .is_multiple_of(unit)
            {
                return Some(found);
            }
            search = found + 1;
        }
    }

    fn scan_chunk(
        &self,
        chunk: usize,
        buffer: &mut Vec<u8>,
        scratch: &mut Vec<u8>,
    ) -> io::Result<RowArrays> {
        let unit = self.layout.encoding.unit() as u64;
        let start = self.from + (chunk * self.chunk_size) as u64;
        let end = (start + self.chunk_size as u64).min(self.len);
        // Start one unit early, so a separator ending right before this chunk
        // is seen and the row starting exactly here is claimed.
        let read_start = if chunk == 0 { start } else { start - unit };
        let read_end = (end + self.lookahead as u64).min(self.len);
        buffer.resize((read_end - read_start) as usize, 0);
        let filled = reader::read_at(self.file, read_start, buffer)?;
        buffer.truncate(filled);

        let mut rows = RowArrays::default();
        let mut position = if chunk == 0 {
            0
        } else {
            match self.find_separator(buffer, read_start, 0) {
                Some(found) => found + self.newline_len,
                None => return Ok(rows),
            }
        };
        let end_in_buffer = (end - read_start) as usize;
        let at_file_end = read_start + filled as u64 >= self.len;
        while position < end_in_buffer && position < buffer.len() {
            let next = self.find_separator(buffer, read_start, position);
            let row_end = next.map_or(buffer.len(), |found| found + self.newline_len);
            let raw = &buffer[position..row_end];
            let text = self.layout.encoding.to_utf8(raw, scratch);
            let fields = parse::fields(self.layout.format, text, self.context);
            rows.push(read_start + position as u64, fields.level, fields.stamp);
            if next.is_none() {
                debug_assert!(at_file_end || row_end >= end_in_buffer);
                break;
            }
            position = row_end;
        }
        Ok(rows)
    }
}

/// Reads and analyzes rows from `from` to `len` in parallel. `from` must be the
/// start of a row.
pub(crate) fn scan(
    file: &File,
    len: u64,
    layout: Layout,
    context: &ParseContext,
    from: u64,
    progress: &AtomicU64,
) -> io::Result<RowArrays> {
    scan_with_sizes(
        file, len, layout, context, from, progress, CHUNK_SIZE, LOOKAHEAD,
    )
}

#[allow(clippy::too_many_arguments)]
fn scan_with_sizes(
    file: &File,
    len: u64,
    layout: Layout,
    context: &ParseContext,
    from: u64,
    progress: &AtomicU64,
    chunk_size: usize,
    lookahead: usize,
) -> io::Result<RowArrays> {
    let encoding = layout.encoding;
    let separator = match layout.format {
        Format::Plain | Format::Custom => encoding.encode_ascii(b"\n"),
        Format::Sccm => encoding.encode_ascii(b"\n<![LOG["),
    };
    let scanner = Scanner {
        file,
        len,
        layout,
        context,
        from,
        chunk_size,
        lookahead,
        separator: Finder::new(&separator).into_owned(),
        newline_len: encoding.unit(),
    };
    let chunks = (len.saturating_sub(from) as usize).div_ceil(chunk_size);
    let parts: Vec<RowArrays> = (0..chunks)
        .into_par_iter()
        .map_init(
            || (Vec::new(), Vec::new()),
            |(buffer, scratch), chunk| {
                let rows = scanner.scan_chunk(chunk, buffer, scratch)?;
                progress.fetch_add(chunk_size as u64, Ordering::Relaxed);
                Ok(rows)
            },
        )
        .collect::<io::Result<_>>()?;

    let total = parts.iter().map(RowArrays::rows).sum();
    let mut rows = RowArrays {
        offsets: Vec::with_capacity(total),
        levels: Vec::with_capacity(total),
        times: Vec::with_capacity(total),
    };
    for part in parts {
        rows.append(part);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::YearHint;
    use proptest::prelude::*;
    use std::io::Write;

    const CONTEXT: ParseContext = ParseContext {
        year_hint: YearHint {
            year: 2026,
            month: 6,
        },
        custom: None,
    };

    /// Returns each row's bytes after scanning with the given chunk size.
    fn scan_bytes(
        bytes: &[u8],
        encoding: Encoding,
        format: Format,
        body_offset: u64,
        chunk_size: usize,
        lookahead: usize,
    ) -> (RowArrays, Vec<Vec<u8>>) {
        let mut temp = tempfile::NamedTempFile::new().unwrap();
        temp.write_all(bytes).unwrap();
        let file = temp.reopen().unwrap();
        let layout = Layout {
            encoding,
            format,
            body_offset,
            format_key: 0,
        };
        let rows = scan_with_sizes(
            &file,
            bytes.len() as u64,
            layout,
            &CONTEXT,
            body_offset,
            &AtomicU64::new(0),
            chunk_size,
            lookahead,
        )
        .unwrap();
        let contents = (0..rows.rows())
            .map(|row| {
                let start = rows.offset(row) as usize;
                let end = if row + 1 < rows.rows() {
                    rows.offset(row + 1) as usize
                } else {
                    bytes.len()
                };
                bytes[start..end].to_vec()
            })
            .collect();
        (rows, contents)
    }

    fn plain_rows(bytes: &[u8], chunk_size: usize) -> Vec<Vec<u8>> {
        scan_bytes(bytes, Encoding::Utf8, Format::Plain, 0, chunk_size, 8).1
    }

    #[test]
    fn chunk_size_never_changes_plain_rows() {
        let log = b"one\r\n\nthree error\nfour";
        let expected = plain_rows(log, 1024);
        assert_eq!(expected.len(), 4);
        assert_eq!(expected[1], b"\n");
        for chunk_size in 1..log.len() {
            assert_eq!(
                plain_rows(log, chunk_size),
                expected,
                "chunk size {chunk_size}"
            );
        }
    }

    #[test]
    fn trailing_newline_does_not_add_a_row() {
        assert_eq!(plain_rows(b"one\ntwo\n", 3).len(), 2);
    }

    #[test]
    fn empty_input_has_no_rows() {
        assert_eq!(plain_rows(b"", 3).len(), 0);
    }

    #[test]
    fn rows_longer_than_the_lookahead_stay_whole() {
        let mut log = vec![b'x'; 5000];
        log.extend_from_slice(b"\nshort\n");
        for chunk_size in [16, 100, 1000, 4096, 5000, 5001, 5002] {
            let rows = scan_bytes(&log, Encoding::Utf8, Format::Plain, 0, chunk_size, 32).1;
            assert_eq!(rows.len(), 2, "chunk size {chunk_size}");
            assert_eq!(rows[0].len(), 5001, "chunk size {chunk_size}");
            assert_eq!(rows[1], b"short\n", "chunk size {chunk_size}");
        }
    }

    #[test]
    fn sccm_multiline_entry_is_one_row() {
        let log = b"<![LOG[first\r\nsecond]LOG]!><time=\"1\" type=\"3\" thread=\"1\">\r\n<![LOG[next]LOG]!><time=\"2\" type=\"2\" thread=\"1\">\r\n";
        let scan = |chunk_size| scan_bytes(log, Encoding::Utf8, Format::Sccm, 0, chunk_size, 1024);
        let (rows, expected) = scan(1024);
        assert_eq!(expected.len(), 2);
        assert_eq!(rows.level(0), Level::Error);
        assert_eq!(rows.level(1), Level::Warning);
        for chunk_size in 1..log.len() {
            let (rows, contents) = scan(chunk_size);
            assert_eq!(contents, expected, "chunk size {chunk_size}");
            assert_eq!(rows.level(0), Level::Error, "chunk size {chunk_size}");
        }
    }

    #[test]
    fn timestamps_are_stored_per_row() {
        let (rows, _) = scan_bytes(
            b"2026-01-01 10:00:00 hello\nno time here\n",
            Encoding::Utf8,
            Format::Plain,
            0,
            1024,
            1024,
        );
        assert!(rows.stamp_seconds(0).is_some());
        assert_eq!(rows.stamp_seconds(1), None);
    }

    #[test]
    fn utf16_rows_split_only_on_real_newlines() {
        // U+0A00 and U+010A contain the byte 0x0A at the wrong alignment.
        let text = "\u{0A00}first \u{010A} error\nsecond\n";
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        for chunk_size in [2, 4, 8, 16, 1024] {
            let (rows, contents) = scan_bytes(
                &bytes,
                Encoding::Utf16Le,
                Format::Plain,
                2,
                chunk_size,
                1024,
            );
            assert_eq!(contents.len(), 2, "chunk size {chunk_size}");
            assert_eq!(rows.level(0), Level::Error, "chunk size {chunk_size}");
            assert_eq!(rows.level(1), Level::Info, "chunk size {chunk_size}");
        }
    }

    #[test]
    fn big_endian_utf16_is_scanned_too() {
        let mut bytes = vec![0xFE, 0xFF];
        bytes.extend("one\ntwo\n".encode_utf16().flat_map(u16::to_be_bytes));
        let (_, contents) = scan_bytes(&bytes, Encoding::Utf16Be, Format::Plain, 2, 4, 64);
        assert_eq!(contents.len(), 2);
    }

    #[test]
    fn offsets_round_trip_at_the_size_limit() {
        assert_eq!(unpack(pack(MAX_FILE_LEN - 1)), MAX_FILE_LEN - 1);
    }

    proptest! {
        /// Any bytes split into rows must rejoin into exactly the original
        /// bytes, and chunking must never change where rows fall.
        #[test]
        fn rows_cover_any_input_exactly(
            bytes in proptest::collection::vec(any::<u8>(), 0..600),
            chunk_size in 1usize..64,
        ) {
            let rows = plain_rows(&bytes, chunk_size);
            prop_assert_eq!(rows.concat(), bytes.clone());
            prop_assert_eq!(rows, plain_rows(&bytes, 4096));
        }

        #[test]
        fn line_heavy_input_matches_a_simple_split(
            bytes in proptest::collection::vec(prop_oneof![Just(b'\n'), Just(b'\r'), Just(b'a')], 0..300),
            chunk_size in 1usize..32,
        ) {
            let expected: Vec<Vec<u8>> = bytes
                .split_inclusive(|byte| *byte == b'\n')
                .map(<[u8]>::to_vec)
                .collect();
            prop_assert_eq!(plain_rows(&bytes, chunk_size), expected);
        }

        #[test]
        fn sccm_scanning_covers_any_input_exactly(
            pieces in proptest::collection::vec(
                prop_oneof![
                    Just(&b"\n<![LOG["[..]),
                    Just(&b"]LOG]!>"[..]),
                    Just(&b"<time=\"1\" type=\"3\">"[..]),
                    Just(&b"text\r\n"[..]),
                ],
                0..40,
            ),
            chunk_size in 1usize..64,
        ) {
            let bytes = pieces.concat();
            let scan = |size| scan_bytes(&bytes, Encoding::Utf8, Format::Sccm, 0, size, 4096).1;
            let rows = scan(chunk_size);
            prop_assert_eq!(rows.concat(), bytes.clone());
            prop_assert_eq!(rows, scan(4096));
        }

        #[test]
        fn utf16_scanning_covers_any_input_exactly(
            units in proptest::collection::vec(
                prop_oneof![Just(0x000Au16), Just(0x0A00), Just(0x010A), Just(0x0061)],
                0..200,
            ),
            chunk_units in 1usize..32,
        ) {
            let bytes: Vec<u8> = units.iter().flat_map(|unit| unit.to_le_bytes()).collect();
            let scan = |size| scan_bytes(&bytes, Encoding::Utf16Le, Format::Plain, 0, size, 4096).1;
            let rows = scan(chunk_units * 2);
            prop_assert_eq!(rows.concat(), bytes.clone());
            prop_assert_eq!(rows.len(), units
                .split_inclusive(|unit| *unit == 0x000A)
                .count());
        }
    }
}
