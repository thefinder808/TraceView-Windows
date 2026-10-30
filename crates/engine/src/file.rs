//! An opened log: its row index, row reading, and following a live file.

use crate::cache::{self, Stamp as CacheStamp};
use crate::encoding::{self, Encoding};
use crate::index::{self, Layout, RowArrays};
use crate::level::Level;
use crate::parse::{self, Fields, Format, ParseContext};
use crate::reader;
use crate::time::YearHint;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Small files index so quickly that a cache entry isn't worth writing.
const CACHE_MIN_LEN: u64 = 64 * 1024 * 1024;
const HEAD_LEN: usize = 64 * 1024;
/// The most contiguous row data read at once.
const READ_BLOCK: u64 = 4 * 1024 * 1024;
/// Rows longer than this are cut off when read, so one enormous row can't
/// exhaust memory. The rest of the row stays in the file.
pub const MAX_ROW_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheUse {
    /// No usable entry, so the whole file was indexed.
    Miss,
    /// An entry matched the file exactly.
    Hit,
    /// An entry matched the start of a log that had grown, so only new rows were indexed.
    Extended,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct OpenStats {
    pub cache: CacheUse,
    pub seconds: f64,
}

/// What changed on disk since the log was last read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    Unchanged,
    /// Rows were added. `new_rows` can be zero when only the last row grew.
    Appended {
        new_rows: usize,
    },
    /// The file was truncated or replaced, so everything was indexed again.
    Reset,
    /// The path no longer exists. Rows read so far remain available if the
    /// file is still held open.
    Removed,
}

pub struct LogFile {
    path: PathBuf,
    file: File,
    layout: Layout,
    context: ParseContext,
    rows: RowArrays,
    indexed_len: u64,
    modified: SystemTime,
    fingerprint: u64,
}

fn modified_nanos(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as u64)
}

fn detect_layout(file: &File, len: u64) -> io::Result<Layout> {
    let mut head = vec![0u8; HEAD_LEN.min(len as usize)];
    let filled = reader::read_at(file, 0, &mut head)?;
    head.truncate(filled);
    let (encoding, body_offset) = encoding::detect(&head);
    let body = head.get(body_offset as usize..).unwrap_or(&[]);
    let mut scratch = Vec::new();
    let format = parse::detect_format(encoding.to_utf8(body, &mut scratch));
    Ok(Layout {
        encoding,
        format,
        body_offset,
    })
}

fn check_size(len: u64) -> io::Result<()> {
    if len >= index::MAX_FILE_LEN {
        return Err(io::Error::other("files of 1 TiB or more aren't supported"));
    }
    Ok(())
}

fn check_rows(rows: &RowArrays) -> io::Result<()> {
    if rows.rows() > u32::MAX as usize {
        return Err(io::Error::other(
            "logs of 4 billion rows or more aren't supported",
        ));
    }
    Ok(())
}

impl LogFile {
    /// Opens a log using the default index cache.
    pub fn open(path: &Path, progress: &AtomicU64) -> io::Result<(Self, OpenStats)> {
        Self::open_with_cache(path, cache::default_dir().as_deref(), progress)
    }

    pub fn open_with_cache(
        path: &Path,
        cache_dir: Option<&Path>,
        progress: &AtomicU64,
    ) -> io::Result<(Self, OpenStats)> {
        let started = Instant::now();
        let path = std::path::absolute(path)?;
        let file = File::open(&path)?;
        let metadata = file.metadata()?;
        let len = metadata.len();
        check_size(len)?;
        let modified = metadata.modified()?;
        let context = ParseContext {
            year_hint: YearHint::from_system_time(modified),
        };
        let layout = detect_layout(&file, len)?;
        let use_cache = cache_dir.filter(|_| len >= CACHE_MIN_LEN);
        let cached = use_cache
            .and_then(|dir| cache::load(dir, &path))
            .filter(|entry| entry.layout == layout);

        let (rows, cache_use) = match cached {
            Some(entry) if is_exact_match(&entry.stamp, len, modified) => {
                (entry.rows, CacheUse::Hit)
            }
            Some(mut entry)
                if len > entry.stamp.indexed_len
                    && reader::fingerprint(&file, entry.stamp.indexed_len)?
                        == entry.stamp.fingerprint =>
            {
                let from = drop_last_row(&mut entry.rows, layout.body_offset);
                entry
                    .rows
                    .extend(index::scan(&file, len, layout, context, from, progress)?);
                (entry.rows, CacheUse::Extended)
            }
            _ => (
                index::scan(&file, len, layout, context, layout.body_offset, progress)?,
                CacheUse::Miss,
            ),
        };
        check_rows(&rows)?;

        let fingerprint = reader::fingerprint(&file, len)?;
        if cache_use != CacheUse::Hit {
            if let Some(dir) = use_cache {
                let stamp = CacheStamp {
                    indexed_len: len,
                    modified_nanos: modified_nanos(modified),
                    fingerprint,
                };
                // A failed cache write only costs the next open a rebuild.
                let _ = cache::store(dir, &path, &stamp, &layout, &rows);
            }
        }
        let stats = OpenStats {
            cache: cache_use,
            seconds: started.elapsed().as_secs_f64(),
        };
        Ok((
            Self {
                path,
                file,
                layout,
                context,
                rows,
                indexed_len: len,
                modified,
                fingerprint,
            },
            stats,
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn rows(&self) -> usize {
        self.rows.rows()
    }

    pub fn format(&self) -> Format {
        self.layout.format
    }

    pub fn encoding(&self) -> Encoding {
        self.layout.encoding
    }

    /// The bytes of the file covered by the index.
    pub fn indexed_len(&self) -> u64 {
        self.indexed_len
    }

    pub fn index_memory_bytes(&self) -> usize {
        self.rows.memory_bytes()
    }

    pub fn level(&self, row: usize) -> Level {
        self.rows.level(row)
    }

    pub fn stamp_seconds(&self, row: usize) -> Option<u32> {
        self.rows.stamp_seconds(row)
    }

    pub fn offset(&self, row: usize) -> u64 {
        self.rows.offset(row)
    }

    /// A row's byte range, kept within the file so a damaged cache entry can
    /// only produce wrong rows, never an out-of-range read.
    fn range(&self, row: usize) -> (u64, u64) {
        let start = self.rows.offset(row).min(self.indexed_len);
        let end = if row + 1 < self.rows() {
            self.rows.offset(row + 1).min(self.indexed_len)
        } else {
            self.indexed_len
        };
        (start, end.max(start))
    }

    /// Reads rows in order, calling `visit` with each row's number, its raw
    /// bytes, and whether they were cut off at [`MAX_ROW_BYTES`]. Nearby rows
    /// share one read. Use [`Self::text`] to turn raw bytes into UTF-8.
    pub fn read_rows(
        &self,
        rows: Range<usize>,
        buffer: &mut Vec<u8>,
        mut visit: impl FnMut(usize, &[u8], bool),
    ) -> io::Result<()> {
        let rows = rows.start..rows.end.min(self.rows());
        let mut row = rows.start;
        while row < rows.end {
            let (block_start, first_end) = self.range(row);
            if first_end - block_start >= MAX_ROW_BYTES {
                buffer.resize(MAX_ROW_BYTES as usize, 0);
                let filled = reader::read_at(&self.file, block_start, buffer)?;
                visit(
                    row,
                    &buffer[..filled],
                    first_end - block_start > MAX_ROW_BYTES,
                );
                row += 1;
                continue;
            }

            let mut last = row;
            let mut block_end = first_end;
            while last + 1 < rows.end {
                let (_, next_end) = self.range(last + 1);
                let next_len = next_end - self.range(last + 1).0;
                if next_end - block_start > READ_BLOCK || next_len >= MAX_ROW_BYTES {
                    break;
                }
                last += 1;
                block_end = next_end;
            }

            buffer.resize((block_end - block_start) as usize, 0);
            let filled = reader::read_at(&self.file, block_start, buffer)?;
            for current in row..=last {
                let (start, end) = self.range(current);
                let start = ((start - block_start) as usize).min(filled);
                let end = ((end - block_start) as usize).clamp(start, filled);
                visit(current, &buffer[start..end], false);
            }
            row = last + 1;
        }
        Ok(())
    }

    /// Converts a row's raw bytes to UTF-8, using `scratch` only if it must.
    pub fn text<'a>(&self, raw: &'a [u8], scratch: &'a mut Vec<u8>) -> &'a [u8] {
        self.layout.encoding.to_utf8(raw, scratch)
    }

    /// Splits a row's UTF-8 text into fields.
    pub fn fields<'a>(&self, text: &'a [u8]) -> Fields<'a> {
        parse::fields(self.layout.format, text, &self.context)
    }

    /// Checks the file for changes and updates the index. Only new data is
    /// read. Holding a log open never stops its writer from truncating,
    /// renaming or deleting it.
    pub fn refresh(&mut self, progress: &AtomicU64) -> io::Result<Change> {
        let metadata = match fs::metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Change::Removed),
            Err(error) => return Err(error),
        };
        let len = metadata.len();
        let modified = metadata.modified()?;
        if len == self.indexed_len && modified == self.modified {
            return Ok(Change::Unchanged);
        }
        check_size(len)?;

        // Reopen by path, because rotation leaves the old handle on the old file.
        let file = File::open(&self.path)?;
        let grew_in_place = len >= self.indexed_len
            && reader::fingerprint(&file, self.indexed_len)? == self.fingerprint;
        let rows_before = self.rows();
        let change = if grew_in_place {
            let from = drop_last_row(&mut self.rows, self.layout.body_offset);
            let added = index::scan(&file, len, self.layout, self.context, from, progress)?;
            self.rows.extend(added);
            Change::Appended {
                new_rows: self.rows().saturating_sub(rows_before),
            }
        } else {
            self.layout = detect_layout(&file, len)?;
            self.context = ParseContext {
                year_hint: YearHint::from_system_time(modified),
            };
            self.rows = index::scan(
                &file,
                len,
                self.layout,
                self.context,
                self.layout.body_offset,
                progress,
            )?;
            Change::Reset
        };
        check_rows(&self.rows)?;
        self.fingerprint = reader::fingerprint(&file, len)?;
        self.file = file;
        self.indexed_len = len;
        self.modified = modified;
        Ok(change)
    }
}

fn is_exact_match(stamp: &CacheStamp, len: u64, modified: SystemTime) -> bool {
    stamp.indexed_len == len && stamp.modified_nanos == modified_nanos(modified)
}

/// Removes the last row, which may still be growing, and returns where to
/// resume scanning from.
fn drop_last_row(rows: &mut RowArrays, body_offset: u64) -> u64 {
    match rows.rows().checked_sub(1) {
        Some(last) => {
            let from = rows.offset(last);
            rows.truncate(last);
            from
        }
        None => body_offset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::Write;

    const LINE: &[u8] = b"2026-01-01 00:00:00 host app[1]: all fine\n";

    fn large_log() -> Vec<u8> {
        let mut log = Vec::with_capacity(CACHE_MIN_LEN as usize + 64);
        while (log.len() as u64) < CACHE_MIN_LEN {
            log.extend_from_slice(LINE);
        }
        log
    }

    fn open_uncached(path: &Path) -> LogFile {
        LogFile::open_with_cache(path, None, &AtomicU64::new(0))
            .unwrap()
            .0
    }

    fn all_rows(log: &LogFile) -> Vec<Vec<u8>> {
        let mut rows = Vec::new();
        log.read_rows(0..log.rows(), &mut Vec::new(), |_, raw, _| {
            rows.push(raw.to_vec())
        })
        .unwrap();
        rows
    }

    fn append(path: &Path, bytes: &[u8]) {
        let mut file = OpenOptions::new().append(true).open(path).unwrap();
        file.write_all(bytes).unwrap();
    }

    /// Windows only updates a file's modified time so precisely, so tests
    /// that change a file wait long enough for the change to be visible.
    fn let_clock_tick() {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    #[test]
    fn opens_an_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.log");
        fs::write(&path, b"").unwrap();
        assert_eq!(open_uncached(&path).rows(), 0);
    }

    #[test]
    fn reads_utf16_logs_with_correct_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("utf16.log");
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(
            "2026-01-01 10:00:00 host app[1]: ERROR: caf\u{e9} failed\nsecond\n"
                .encode_utf16()
                .flat_map(u16::to_le_bytes),
        );
        fs::write(&path, bytes).unwrap();
        let log = open_uncached(&path);
        assert_eq!(log.rows(), 2);
        assert_eq!(log.encoding(), Encoding::Utf16Le);
        assert_eq!(log.level(0), Level::Error);
        let rows = all_rows(&log);
        let mut scratch = Vec::new();
        let text = log.text(&rows[0], &mut scratch);
        let fields = log.fields(text);
        assert_eq!(fields.component, b"app");
        assert_eq!(fields.message, "caf\u{e9} failed".as_bytes());
    }

    #[test]
    fn reads_utf16_sccm_logs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sccm16.log");
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(
            "<![LOG[one]LOG]!><time=\"10:00:00.000+000\" date=\"04-06-2026\" component=\"A\" context=\"\" type=\"3\" thread=\"1\" file=\"\">\r\n<![LOG[two]LOG]!><time=\"10:00:01.000+000\" date=\"04-06-2026\" component=\"B\" context=\"\" type=\"1\" thread=\"1\" file=\"\">\r\n"
                .encode_utf16()
                .flat_map(u16::to_le_bytes),
        );
        fs::write(&path, bytes).unwrap();
        let log = open_uncached(&path);
        assert_eq!(log.format(), Format::Sccm);
        assert_eq!(log.rows(), 2);
        assert_eq!(log.level(0), Level::Error);
    }

    #[test]
    fn utf8_bom_is_not_part_of_the_first_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bom.log");
        fs::write(
            &path,
            b"\xEF\xBB\xBF<![LOG[hello]LOG]!><time=\"1\" type=\"2\">\r\n",
        )
        .unwrap();
        let log = open_uncached(&path);
        assert_eq!(log.format(), Format::Sccm);
        assert_eq!(log.level(0), Level::Warning);
        assert_eq!(log.offset(0), 3);
    }

    #[test]
    fn second_open_hits_the_cache_and_a_replaced_file_misses_it() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let path = dir.path().join("large.log");
        let log_bytes = large_log();
        fs::write(&path, &log_bytes).unwrap();
        let progress = AtomicU64::new(0);

        let (first, stats) = LogFile::open_with_cache(&path, Some(&cache_dir), &progress).unwrap();
        assert_eq!(stats.cache, CacheUse::Miss);
        let rows = first.rows();
        drop(first);

        let (second, stats) = LogFile::open_with_cache(&path, Some(&cache_dir), &progress).unwrap();
        assert_eq!(stats.cache, CacheUse::Hit);
        assert_eq!(second.rows(), rows);
        drop(second);

        let_clock_tick();
        let mut replaced = log_bytes.clone();
        replaced[0] = b'X';
        fs::write(&path, &replaced).unwrap();
        let (third, stats) = LogFile::open_with_cache(&path, Some(&cache_dir), &progress).unwrap();
        assert_eq!(stats.cache, CacheUse::Miss);
        assert_eq!(third.rows(), rows);
    }

    #[test]
    fn a_grown_log_only_indexes_new_rows() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let path = dir.path().join("large.log");
        fs::write(&path, large_log()).unwrap();
        let progress = AtomicU64::new(0);
        let (first, _) = LogFile::open_with_cache(&path, Some(&cache_dir), &progress).unwrap();
        let rows = first.rows();
        drop(first);

        let_clock_tick();
        append(&path, b"one more row\nand another\n");
        let (second, stats) = LogFile::open_with_cache(&path, Some(&cache_dir), &progress).unwrap();
        assert_eq!(stats.cache, CacheUse::Extended);
        assert_eq!(second.rows(), rows + 2);
        let mut last = Vec::new();
        second
            .read_rows(rows + 1..rows + 2, &mut Vec::new(), |_, raw, _| {
                last = raw.to_vec()
            })
            .unwrap();
        assert_eq!(last, b"and another\n");
    }

    #[test]
    fn damaged_cache_entry_is_rebuilt() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let path = dir.path().join("large.log");
        fs::write(&path, large_log()).unwrap();
        let progress = AtomicU64::new(0);
        let (first, _) = LogFile::open_with_cache(&path, Some(&cache_dir), &progress).unwrap();
        let rows = first.rows();
        drop(first);

        for entry in fs::read_dir(&cache_dir).unwrap() {
            let entry = entry.unwrap().path();
            let length = fs::metadata(&entry).unwrap().len();
            OpenOptions::new()
                .write(true)
                .open(&entry)
                .unwrap()
                .set_len(length - 1)
                .unwrap();
        }
        let (second, stats) = LogFile::open_with_cache(&path, Some(&cache_dir), &progress).unwrap();
        assert_eq!(stats.cache, CacheUse::Miss);
        assert_eq!(second.rows(), rows);
    }

    #[test]
    fn nearby_rows_and_long_rows_read_correctly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixed.log");
        let mut bytes = b"short\n".to_vec();
        bytes.extend(vec![b'x'; MAX_ROW_BYTES as usize + 500]);
        bytes.extend_from_slice(b"\nafter\n");
        fs::write(&path, &bytes).unwrap();
        let log = open_uncached(&path);

        let mut seen = Vec::new();
        log.read_rows(0..3, &mut Vec::new(), |row, raw, cut| {
            seen.push((row, raw.len(), cut))
        })
        .unwrap();
        assert_eq!(
            seen,
            vec![
                (0, 6, false),
                (1, MAX_ROW_BYTES as usize, true),
                (2, 6, false)
            ]
        );
    }

    #[test]
    fn reads_clamp_to_the_real_row_count() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.log");
        fs::write(&path, b"one\ntwo\n").unwrap();
        let log = open_uncached(&path);
        let mut count = 0;
        log.read_rows(1..100, &mut Vec::new(), |_, _, _| count += 1)
            .unwrap();
        assert_eq!(count, 1);
        log.read_rows(50..100, &mut Vec::new(), |_, _, _| count += 1)
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn refresh_picks_up_appended_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.log");
        fs::write(&path, b"one\ntwo\n").unwrap();
        let mut log = open_uncached(&path);
        let progress = AtomicU64::new(0);
        assert_eq!(log.refresh(&progress).unwrap(), Change::Unchanged);

        let_clock_tick();
        append(&path, b"three error\nfour\n");
        assert_eq!(
            log.refresh(&progress).unwrap(),
            Change::Appended { new_rows: 2 }
        );
        assert_eq!(log.rows(), 4);
        assert_eq!(log.level(2), Level::Error);
        assert_eq!(all_rows(&log)[3], b"four\n");
    }

    #[test]
    fn refresh_finishes_a_partly_written_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.log");
        fs::write(&path, b"one\npart").unwrap();
        let mut log = open_uncached(&path);
        let progress = AtomicU64::new(0);
        assert_eq!(log.rows(), 2);

        let_clock_tick();
        append(&path, b"ial error\n");
        assert_eq!(
            log.refresh(&progress).unwrap(),
            Change::Appended { new_rows: 0 }
        );
        assert_eq!(log.rows(), 2);
        assert_eq!(all_rows(&log)[1], b"partial error\n");
        assert_eq!(log.level(1), Level::Error);
    }

    #[test]
    fn refresh_finishes_a_partly_written_sccm_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.log");
        let head = b"<![LOG[one]LOG]!><time=\"1\" type=\"1\">\r\n<![LOG[two";
        fs::write(&path, head).unwrap();
        let mut log = open_uncached(&path);
        assert_eq!(log.format(), Format::Sccm);
        assert_eq!(log.rows(), 2);

        let_clock_tick();
        append(&path, b" more]LOG]!><time=\"2\" type=\"3\">\r\n");
        let progress = AtomicU64::new(0);
        assert_eq!(
            log.refresh(&progress).unwrap(),
            Change::Appended { new_rows: 0 }
        );
        assert_eq!(log.level(1), Level::Error);
    }

    #[test]
    fn refresh_resets_after_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.log");
        fs::write(&path, b"one\ntwo\nthree\n").unwrap();
        let mut log = open_uncached(&path);

        let_clock_tick();
        fs::write(&path, b"new\n").unwrap();
        let progress = AtomicU64::new(0);
        assert_eq!(log.refresh(&progress).unwrap(), Change::Reset);
        assert_eq!(all_rows(&log), vec![b"new\n".to_vec()]);
    }

    #[test]
    fn refresh_resets_after_rotation_to_a_larger_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.log");
        fs::write(&path, b"old one\nold two\n").unwrap();
        let mut log = open_uncached(&path);

        let_clock_tick();
        fs::rename(&path, dir.path().join("live.log.1")).unwrap();
        fs::write(&path, b"brand new contents\nthat are longer\nthan before\n").unwrap();
        let progress = AtomicU64::new(0);
        assert_eq!(log.refresh(&progress).unwrap(), Change::Reset);
        assert_eq!(log.rows(), 3);
        assert_eq!(all_rows(&log)[0], b"brand new contents\n");
    }

    #[test]
    fn refresh_reports_a_removed_file_and_keeps_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.log");
        fs::write(&path, b"one\ntwo\n").unwrap();
        let mut log = open_uncached(&path);
        fs::remove_file(&path).unwrap();
        let progress = AtomicU64::new(0);
        assert_eq!(log.refresh(&progress).unwrap(), Change::Removed);
        assert_eq!(all_rows(&log).len(), 2);
    }

    /// The reason the engine doesn't memory-map: holding a log open must never
    /// stop the application writing it.
    #[test]
    fn an_open_log_never_blocks_its_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.log");
        fs::write(&path, vec![b'x'; 64 * 1024]).unwrap();
        let log = open_uncached(&path);

        append(&path, b"appended\n");
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(10)
            .unwrap();
        fs::write(&path, b"overwritten\n").unwrap();
        let rotated = dir.path().join("live.log.1");
        fs::rename(&path, &rotated).unwrap();
        fs::remove_file(&rotated).unwrap();
        drop(log);
    }
}
