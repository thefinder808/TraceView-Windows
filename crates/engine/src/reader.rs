//! File access. The engine uses positional reads rather than memory maps,
//! because a mapped file can't be truncated by the app writing it, and because
//! reads were about 40% faster on a cold disk.

use std::fs::File;
use std::io;

/// Windows reports reading past the end of a file as this error on some handles.
#[cfg(windows)]
const ERROR_HANDLE_EOF: i32 = 38;

#[cfg(windows)]
fn read_once(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    match file.seek_read(buffer, offset) {
        Err(error) if error.raw_os_error() == Some(ERROR_HANDLE_EOF) => Ok(0),
        result => result,
    }
}

/// The product is Windows-only, but this lets coverage-guided fuzzing run on
/// Linux, where libFuzzer works.
#[cfg(unix)]
fn read_once(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(buffer, offset)
}

/// Fills as much of `buffer` as the file allows and returns the bytes read,
/// which is short at the end of the file or if the file shrank.
pub(crate) fn read_at(file: &File, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match read_once(file, &mut buffer[filled..], offset + filled as u64) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

/// Where a log's bytes come from: its file, or all of it, already read. A
/// small log is read whole when it's opened, so a slow network share is asked
/// once rather than for each part.
#[derive(Clone, Copy)]
pub(crate) enum Source<'a> {
    File(&'a File),
    Bytes(&'a [u8]),
}

impl Source<'_> {
    /// As [`read_at`], from whichever the source is.
    pub(crate) fn read_at(self, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Source::File(file) => read_at(file, offset, buffer),
            Source::Bytes(bytes) => {
                let start = usize::try_from(offset).map_or(bytes.len(), |at| at.min(bytes.len()));
                let count = buffer.len().min(bytes.len() - start);
                buffer[..count].copy_from_slice(&bytes[start..start + count]);
                Ok(count)
            }
        }
    }
}

/// A hash of a file's first 8 KiB and last 4 KiB up to `len`. It tells a log
/// that grew from one that was replaced, without reading the whole file. Live
/// tail reads it on every refresh, so it's kept small for slow network shares.
pub(crate) fn fingerprint(source: Source<'_>, len: u64) -> io::Result<u64> {
    const HEAD: u64 = 8 * 1024;
    const TAIL: u64 = 4 * 1024;
    let mut buffer = vec![0u8; HEAD.min(len) as usize];
    let filled = source.read_at(0, &mut buffer)?;
    let mut hash = fnv1a(0xcbf2_9ce4_8422_2325, &buffer[..filled]);
    let tail_len = TAIL.min(len);
    buffer.resize(tail_len as usize, 0);
    let filled = source.read_at(len - tail_len, &mut buffer)?;
    hash = fnv1a(hash, &buffer[..filled]);
    Ok(fnv1a(hash, &len.to_le_bytes()))
}

/// FNV-1a. This detects changes and names cache files, and isn't cryptographic.
pub(crate) fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}
