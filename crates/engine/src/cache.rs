//! On-disk index cache. Entries are keyed by path and rejected when the file's
//! size or modified time changes.

use crate::index::{Format, RowIndex};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"TVIDX01\0";

pub struct Stamp {
    pub file_len: u64,
    pub modified_nanos: u64,
}

pub fn default_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(base).join("TraceView").join("indexes"))
}

fn entry_path(dir: &Path, log_path: &Path) -> PathBuf {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    log_path.hash(&mut hasher);
    dir.join(format!("{:016x}.idx", hasher.finish()))
}

pub fn load(dir: &Path, log_path: &Path, stamp: &Stamp) -> Option<RowIndex> {
    let mut file = fs::File::open(entry_path(dir, log_path)).ok()?;
    let mut header = [0u8; 40];
    file.read_exact(&mut header).ok()?;
    if &header[..8] != MAGIC {
        return None;
    }
    let read_u64 = |at: usize| u64::from_le_bytes(header[at..at + 8].try_into().unwrap());
    if read_u64(8) != stamp.file_len || read_u64(16) != stamp.modified_nanos {
        return None;
    }
    let format = Format::from_u8(header[24])?;
    let rows = usize::try_from(read_u64(25)).ok()?;
    let path_len = u32::from_le_bytes(header[33..37].try_into().unwrap()) as usize;

    let expected_len = 40 + path_len as u64 + rows as u64 * 6;
    if file.metadata().ok()?.len() != expected_len {
        return None;
    }
    let mut stored_path = vec![0u8; path_len];
    file.read_exact(&mut stored_path).ok()?;
    if stored_path != log_path.to_string_lossy().as_bytes() {
        return None;
    }

    let mut offsets = vec![[0u8; 5]; rows];
    file.read_exact(bytemuck::cast_slice_mut(&mut offsets)).ok()?;
    let mut levels = vec![0u8; rows];
    file.read_exact(&mut levels).ok()?;
    Some(RowIndex {
        format,
        offsets,
        levels,
    })
}

/// Writes to a temp file and renames it, so a crash can't leave a partial entry.
pub fn store(dir: &Path, log_path: &Path, stamp: &Stamp, index: &RowIndex) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let target = entry_path(dir, log_path);
    let temp = target.with_extension(format!("tmp{}", std::process::id()));
    let path_text = log_path.to_string_lossy();
    let path_bytes = path_text.as_bytes();

    let mut header = [0u8; 40];
    header[..8].copy_from_slice(MAGIC);
    header[8..16].copy_from_slice(&stamp.file_len.to_le_bytes());
    header[16..24].copy_from_slice(&stamp.modified_nanos.to_le_bytes());
    header[24] = index.format as u8;
    header[25..33].copy_from_slice(&(index.rows() as u64).to_le_bytes());
    header[33..37].copy_from_slice(&(path_bytes.len() as u32).to_le_bytes());

    let mut file = fs::File::create(&temp)?;
    file.write_all(&header)?;
    file.write_all(path_bytes)?;
    file.write_all(bytemuck::cast_slice(&index.offsets))?;
    file.write_all(&index.levels)?;
    drop(file);
    fs::rename(&temp, &target)
}
