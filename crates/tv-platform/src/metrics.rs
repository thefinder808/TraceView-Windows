//! Process measurements for performance checks.

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

fn ticks(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

/// Milliseconds since Windows created this process, including time before `main`.
pub fn ms_since_process_start() -> f64 {
    let (mut created, mut exited, mut kernel, mut user) = Default::default();
    // SAFETY: the pseudo-handle is always valid, and each pointer is a live local.
    let now = unsafe {
        let _ = GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        );
        GetSystemTimeAsFileTime()
    };
    ticks(now).saturating_sub(ticks(created)) as f64 / 10_000.0
}

/// This process's own memory in bytes, excluding pages shared with the file cache.
pub fn private_bytes() -> usize {
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    // SAFETY: the struct is plain data, and `cb` tells Windows its size.
    let _ = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    counters.PagefileUsage
}

/// CPU time this process has used, in seconds.
pub fn cpu_seconds() -> f64 {
    let (mut created, mut exited, mut kernel, mut user) = Default::default();
    // SAFETY: the pseudo-handle is always valid, and each pointer is a live local.
    let _ = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    };
    (ticks(kernel) + ticks(user)) as f64 / 10_000_000.0
}
