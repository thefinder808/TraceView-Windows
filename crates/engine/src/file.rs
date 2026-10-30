//! An opened log: the memory map plus its row index.

use crate::cache::{self, Stamp};
use crate::index::{self, Format, Level, RowIndex};
use memmap2::Mmap;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::time::{Instant, UNIX_EPOCH};

/// Small files index so quickly that a cache entry isn't worth writing.
const CACHE_MIN_LEN: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct OpenStats {
    pub cache_hit: bool,
    pub seconds: f64,
}

pub struct LogFile {
    pub path: PathBuf,
    map: Option<Mmap>,
    pub index: RowIndex,
}

impl LogFile {
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
        let file_len = metadata.len();
        if file_len >= index::MAX_FILE_LEN {
            return Err(io::Error::other("files of 1 TiB or more aren't supported"));
        }

        // Windows refuses to map an empty file.
        let map = if file_len == 0 {
            None
        } else {
            // SAFETY: Windows blocks truncation while a view is mapped, so the
            // mapped range stays valid. Bytes may change, and all parsing
            // treats them as untrusted and is bounds-checked.
            Some(unsafe { Mmap::map(&file)? })
        };
        let bytes = map.as_deref().unwrap_or(&[]);
        if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
            return Err(io::Error::other("UTF-16 logs aren't supported yet"));
        }

        let stamp = Stamp {
            file_len,
            modified_nanos: metadata
                .modified()?
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos() as u64),
        };
        let cache_dir = cache_dir.filter(|_| file_len >= CACHE_MIN_LEN);
        let cached = cache_dir.and_then(|dir| cache::load(dir, &path, &stamp));
        let cache_hit = cached.is_some();
        let row_index = cached.unwrap_or_else(|| {
            let built = index::build(bytes, progress);
            if let Some(dir) = cache_dir {
                // A failed cache write only costs the next open a rebuild.
                let _ = cache::store(dir, &path, &stamp, &built);
            }
            built
        });
        if row_index.rows() > u32::MAX as usize {
            return Err(io::Error::other("logs of 4 billion rows or more aren't supported"));
        }

        let stats = OpenStats {
            cache_hit,
            seconds: started.elapsed().as_secs_f64(),
        };
        Ok((
            Self {
                path,
                map,
                index: row_index,
            },
            stats,
        ))
    }

    pub fn bytes(&self) -> &[u8] {
        self.map.as_deref().unwrap_or(&[])
    }

    pub fn rows(&self) -> usize {
        self.index.rows()
    }

    pub fn format(&self) -> Format {
        self.index.format
    }

    pub fn level(&self, row: usize) -> Level {
        self.index.level(row)
    }

    pub fn row_bytes(&self, row: usize) -> &[u8] {
        let bytes = self.bytes();
        let (start, end) = self.index.range(row, bytes.len());
        &bytes[start..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn large_log() -> Vec<u8> {
        let mut log = Vec::with_capacity(CACHE_MIN_LEN as usize + 64);
        while (log.len() as u64) < CACHE_MIN_LEN {
            log.extend_from_slice(b"2026-01-01 00:00:00 host app[1]: all fine\n");
        }
        log
    }

    #[test]
    fn opens_an_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.log");
        fs::write(&path, b"").unwrap();
        let (log, _) = LogFile::open_with_cache(&path, None, &AtomicU64::new(0)).unwrap();
        assert_eq!(log.rows(), 0);
    }

    #[test]
    fn rejects_utf16_instead_of_showing_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("utf16.log");
        fs::write(&path, [0xFF, 0xFE, b'a', 0]).unwrap();
        assert!(LogFile::open_with_cache(&path, None, &AtomicU64::new(0)).is_err());
    }

    #[test]
    fn second_open_hits_the_cache_and_a_changed_file_misses_it() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let path = dir.path().join("large.log");
        let mut log = large_log();
        fs::write(&path, &log).unwrap();
        let progress = AtomicU64::new(0);

        let (first, stats) = LogFile::open_with_cache(&path, Some(&cache_dir), &progress).unwrap();
        assert!(!stats.cache_hit);
        let rows = first.rows();
        drop(first);

        let (second, stats) = LogFile::open_with_cache(&path, Some(&cache_dir), &progress).unwrap();
        assert!(stats.cache_hit);
        assert_eq!(second.rows(), rows);
        assert_eq!(second.row_bytes(rows - 1), b"2026-01-01 00:00:00 host app[1]: all fine\n");
        drop(second);

        log.extend_from_slice(b"one more row\n");
        fs::write(&path, &log).unwrap();
        let (third, stats) = LogFile::open_with_cache(&path, Some(&cache_dir), &progress).unwrap();
        assert!(!stats.cache_hit);
        assert_eq!(third.rows(), rows + 1);
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
            fs::OpenOptions::new()
                .write(true)
                .open(&entry)
                .unwrap()
                .set_len(length - 1)
                .unwrap();
        }
        let (second, stats) = LogFile::open_with_cache(&path, Some(&cache_dir), &progress).unwrap();
        assert!(!stats.cache_hit);
        assert_eq!(second.rows(), rows);
    }
}
