//! Process measurements for the benchmark report.

use std::sync::Mutex;
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

static MARKS: Mutex<Vec<(&'static str, f64)>> = Mutex::new(Vec::new());

/// Records a named point on the startup timeline.
pub fn mark(name: &'static str) {
    if let Ok(mut marks) = MARKS.lock() {
        marks.push((name, ms_since_process_start()));
    }
}

pub fn marks() -> Vec<(&'static str, f64)> {
    MARKS.lock().map(|marks| marks.clone()).unwrap_or_default()
}

fn ticks(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

/// Milliseconds since the OS created this process, including time before `main`.
pub fn ms_since_process_start() -> f64 {
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user, mut now) = (zero, zero, zero, zero, zero);
    // SAFETY: the pseudo-handle is always valid and each pointer is a live local.
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        );
        GetSystemTimeAsFileTime(&mut now);
    }
    ticks(now).saturating_sub(ticks(created)) as f64 / 10_000.0
}

/// Returns `(working set, private bytes)`. The working set includes mapped log
/// pages that Windows can drop at any time. Private bytes are the app's own memory.
pub fn memory_bytes() -> (usize, usize) {
    // SAFETY: the struct is plain data and `cb` tells Windows its size.
    unsafe {
        let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb);
        (counters.WorkingSetSize, counters.PagefileUsage)
    }
}
