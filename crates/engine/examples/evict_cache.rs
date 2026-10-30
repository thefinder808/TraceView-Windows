//! Drops a file from the Windows file cache, for measuring cold-disk speed.
//! A non-cached open makes Windows discard the file's cached pages, provided no
//! other process has the file open.
//!
//! Usage: evict_cache <path>...

#[cfg(windows)]
fn main() -> std::io::Result<()> {
    use std::io::Read;
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;
    for path in std::env::args().skip(1) {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_NO_BUFFERING)
            .open(path)?;
        // Unbuffered reads need a sector-aligned buffer.
        let mut raw = vec![0u8; 8192];
        let aligned = raw.as_ptr().align_offset(4096);
        let _ = file.read(&mut raw[aligned..aligned + 4096])?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("evict_cache only works on Windows");
}
