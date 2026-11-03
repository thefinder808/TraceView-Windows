//! On-disk index cache, so reopening a large log is nearly instant. Entries hold
//! only offsets, severities and times, plus the log's path, and never log text.

use crate::encoding::Encoding;
use crate::index::{Layout, RowArrays};
use crate::parse::Format;
use crate::reader::fnv1a;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Version 3 added the custom format key, so older entries are rebuilt once.
const MAGIC: &[u8; 8] = b"TVIDX03\0";
const HEADER_LEN: usize = 8 + 8 + 8 + 8 + 8 + 8 + 1 + 1 + 4 + 8;
const BYTES_PER_ROW: u64 = 5 + 1 + 4;
/// The cache's size limit. The oldest entries go first when it's exceeded.
const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The file state an entry was built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub indexed_len: u64,
    pub modified_nanos: u64,
    pub fingerprint: u64,
}

pub(crate) struct Entry {
    pub stamp: Stamp,
    pub layout: Layout,
    pub rows: RowArrays,
}

pub fn default_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(base).join("TraceView").join("indexes"))
}

fn path_key(log_path: &Path) -> String {
    log_path.to_string_lossy().to_lowercase()
}

fn entry_path(dir: &Path, log_path: &Path) -> PathBuf {
    let hash = fnv1a(0xcbf2_9ce4_8422_2325, path_key(log_path).as_bytes());
    dir.join(format!("{hash:016x}.idx"))
}

/// Loads an entry if one exists and is intact. The caller decides whether it
/// still matches the file. Any damage reads as no entry, which causes a rebuild.
pub(crate) fn load(dir: &Path, log_path: &Path) -> Option<Entry> {
    let mut file = fs::File::open(entry_path(dir, log_path)).ok()?;
    let mut header = [0u8; HEADER_LEN];
    file.read_exact(&mut header).ok()?;
    if &header[..8] != MAGIC {
        return None;
    }
    let read_u64 = |at: usize| u64::from_le_bytes(header[at..at + 8].try_into().unwrap());
    let stamp = Stamp {
        indexed_len: read_u64(8),
        modified_nanos: read_u64(16),
        fingerprint: read_u64(24),
    };
    let body_offset = read_u64(32);
    let rows = read_u64(40);
    let format = Format::from_u8(header[48])?;
    let encoding = Encoding::from_u8(header[49])?;
    let path_len = u32::from_le_bytes(header[50..54].try_into().unwrap());
    let format_key = read_u64(54);

    let expected_len = (HEADER_LEN as u64)
        .checked_add(u64::from(path_len))?
        .checked_add(rows.checked_mul(BYTES_PER_ROW)?)?;
    if file.metadata().ok()?.len() != expected_len {
        return None;
    }
    let mut stored_path = vec![0u8; path_len as usize];
    file.read_exact(&mut stored_path).ok()?;
    if stored_path != path_key(log_path).as_bytes() {
        return None;
    }

    let rows = usize::try_from(rows).ok()?;
    let mut offsets = vec![[0u8; 5]; rows];
    file.read_exact(bytemuck::cast_slice_mut(&mut offsets))
        .ok()?;
    let mut levels = vec![0u8; rows];
    file.read_exact(&mut levels).ok()?;
    let mut times = vec![0u32; rows];
    file.read_exact(bytemuck::cast_slice_mut(&mut times)).ok()?;
    Some(Entry {
        stamp,
        layout: Layout {
            encoding,
            format,
            body_offset,
            format_key,
        },
        rows: RowArrays {
            offsets,
            levels,
            times,
        },
    })
}

/// Writes to a temp file and renames it, so a crash can't leave a partial entry.
pub(crate) fn store(
    dir: &Path,
    log_path: &Path,
    stamp: &Stamp,
    layout: &Layout,
    rows: &RowArrays,
) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let target = entry_path(dir, log_path);
    let temp = target.with_extension(format!("tmp{}", std::process::id()));
    let key = path_key(log_path);

    let mut header = [0u8; HEADER_LEN];
    header[..8].copy_from_slice(MAGIC);
    header[8..16].copy_from_slice(&stamp.indexed_len.to_le_bytes());
    header[16..24].copy_from_slice(&stamp.modified_nanos.to_le_bytes());
    header[24..32].copy_from_slice(&stamp.fingerprint.to_le_bytes());
    header[32..40].copy_from_slice(&layout.body_offset.to_le_bytes());
    header[40..48].copy_from_slice(&(rows.rows() as u64).to_le_bytes());
    header[48] = layout.format as u8;
    header[49] = layout.encoding as u8;
    header[50..54].copy_from_slice(&(key.len() as u32).to_le_bytes());
    header[54..62].copy_from_slice(&layout.format_key.to_le_bytes());

    let mut file = fs::File::create(&temp)?;
    file.write_all(&header)?;
    file.write_all(key.as_bytes())?;
    file.write_all(bytemuck::cast_slice(&rows.offsets))?;
    file.write_all(&rows.levels)?;
    file.write_all(bytemuck::cast_slice(&rows.times))?;
    drop(file);
    fs::rename(&temp, &target)?;
    evict_oldest(dir, &target, MAX_TOTAL_BYTES)
}

/// Deletes the oldest entries until the cache fits, always keeping `keep`.
fn evict_oldest(dir: &Path, keep: &Path, limit: u64) -> io::Result<()> {
    let mut entries = Vec::new();
    let mut total = 0;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_file() {
            total += metadata.len();
            entries.push((metadata.modified()?, metadata.len(), entry.path()));
        }
    }
    entries.sort_by_key(|(modified, _, _)| *modified);
    for (_, size, path) in entries {
        if total <= limit {
            break;
        }
        if path != keep {
            fs::remove_file(&path)?;
            total -= size;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eviction_removes_oldest_and_keeps_the_new_entry() {
        let dir = tempfile::tempdir().unwrap();
        let names = ["old.idx", "newer.idx", "keep.idx"];
        for name in names {
            fs::write(dir.path().join(name), vec![0u8; 100]).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let keep = dir.path().join("keep.idx");
        evict_oldest(dir.path(), &keep, 200).unwrap();
        assert!(!dir.path().join("old.idx").exists());
        assert!(dir.path().join("newer.idx").exists());
        assert!(keep.exists());
    }

    #[test]
    fn paths_differing_only_by_case_share_an_entry() {
        let dir = Path::new("cache");
        assert_eq!(
            entry_path(dir, Path::new("C:\\Logs\\App.log")),
            entry_path(dir, Path::new("c:\\logs\\app.LOG"))
        );
    }
}
