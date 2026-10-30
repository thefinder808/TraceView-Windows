//! Compares two ways to scan a log with a cold Windows file cache: page faults
//! through a memory map, and plain positional reads. The read path is what
//! avoids blocking a writer's truncate (see mapped_file_behavior).
//!
//! Usage: cold_read <evict|map|read> <path>

use rayon::prelude::*;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::windows::fs::{FileExt, OpenOptionsExt};
use std::time::Instant;

const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;
const CHUNK: usize = 16 * 1024 * 1024;

/// Reads a block unbuffered. A non-cached open makes Windows drop the file's
/// cached pages when no other handle or mapping holds it.
fn evict(path: &str) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_NO_BUFFERING)
        .open(path)?;
    // Unbuffered reads need a sector-aligned buffer, and 4 KiB suits all drives here.
    let mut raw = vec![0u8; 8192];
    let aligned = raw.as_ptr().align_offset(4096);
    file.read_exact(&mut raw[aligned..aligned + 4096])?;
    Ok(())
}

/// Times the read a grid would make per frame on a random jump: about 60 rows.
fn read_latency(file: &File, len: usize) -> std::io::Result<()> {
    const BLOCK: usize = 16 * 1024;
    let mut buffer = vec![0u8; BLOCK];
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut samples = Vec::with_capacity(1000);
    for _ in 0..1000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let offset = state % (len - BLOCK) as u64;
        let started = Instant::now();
        file.seek_read(&mut buffer, offset)?;
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "16 KiB random read: p50 {:.3} ms, p99 {:.3} ms, max {:.3} ms",
        samples[500], samples[990], samples[999]
    );
    Ok(())
}

fn count_newlines(bytes: &[u8]) -> usize {
    memchr::memchr_iter(b'\n', bytes).count()
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, mode, path] = args.as_slice() else {
        eprintln!("usage: cold_read <evict|map|read> <path>");
        std::process::exit(2);
    };
    if mode == "evict" {
        return evict(path);
    }

    let file = File::open(path)?;
    let len = file.metadata()?.len() as usize;
    if mode == "latency" {
        return read_latency(&file, len);
    }
    let chunks = len.div_ceil(CHUNK);
    let started = Instant::now();
    let lines: usize = match mode.as_str() {
        "map" => {
            let map = unsafe { memmap2::Mmap::map(&file)? };
            map.par_chunks(CHUNK).map(count_newlines).sum()
        }
        "read" => (0..chunks)
            .into_par_iter()
            .map_init(
                || vec![0u8; CHUNK],
                |buffer, chunk| {
                    let offset = (chunk * CHUNK) as u64;
                    let want = CHUNK.min(len - chunk * CHUNK);
                    let mut filled = 0;
                    while filled < want {
                        let got = file
                            .seek_read(&mut buffer[filled..want], offset + filled as u64)
                            .expect("read failed");
                        if got == 0 {
                            break;
                        }
                        filled += got;
                    }
                    count_newlines(&buffer[..filled])
                },
            )
            .sum(),
        _ => {
            eprintln!("mode must be evict, map or read");
            std::process::exit(2);
        }
    };
    let seconds = started.elapsed().as_secs_f64();
    println!(
        "{mode}: {lines} lines in {seconds:.2}s ({:.2} GB/s)",
        len as f64 / 1e9 / seconds
    );
    Ok(())
}
