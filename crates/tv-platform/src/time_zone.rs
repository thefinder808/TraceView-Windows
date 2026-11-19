//! This PC's time zone, which the engine uses to show logs written in UTC,
//! such as IIS's, in local time. The engine has no `unsafe` code, so the app
//! hands it these two functions at startup.

use std::sync::OnceLock;
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Time::{
    FileTimeToSystemTime, GetDynamicTimeZoneInformation, SystemTimeToFileTime,
    SystemTimeToTzSpecificLocalTimeEx, DYNAMIC_TIME_ZONE_INFORMATION, TIME_ZONE_ID_INVALID,
};

/// 100-nanosecond intervals from 1601-01-01, where `FILETIME` counts from, to
/// 1970-01-01.
const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;
const TICKS_PER_SECOND: i64 = 10_000_000;

/// The zone, read once. Later changes to it apply from the next launch.
fn zone() -> Option<&'static DYNAMIC_TIME_ZONE_INFORMATION> {
    static ZONE: OnceLock<Option<DYNAMIC_TIME_ZONE_INFORMATION>> = OnceLock::new();
    ZONE.get_or_init(|| {
        let mut zone = DYNAMIC_TIME_ZONE_INFORMATION::default();
        // SAFETY: the pointer is to a live local of the type the call fills.
        let id = unsafe { GetDynamicTimeZoneInformation(&mut zone) };
        (id != TIME_ZONE_ID_INVALID).then_some(zone)
    })
    .as_ref()
}

fn to_file_time(ticks: i64) -> FILETIME {
    FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    }
}

fn from_file_time(time: FILETIME) -> i64 {
    ((i64::from(time.dwHighDateTime)) << 32) | i64::from(time.dwLowDateTime)
}

/// The offset from UTC, in minutes east, at a moment given in seconds since
/// 1970 in UTC. Windows' rules for the zone in that year decide it, so
/// daylight saving time is right for past years too. 0 if Windows can't say.
pub fn offset_at(unix_seconds: i64) -> i32 {
    let Some(zone) = zone() else {
        return 0;
    };
    let Some(utc_ticks) = unix_seconds
        .checked_mul(TICKS_PER_SECOND)
        .and_then(|ticks| ticks.checked_add(UNIX_EPOCH_TICKS))
        .filter(|ticks| *ticks >= 0)
    else {
        return 0;
    };
    let (mut utc, mut local, mut local_time) = (
        SYSTEMTIME::default(),
        SYSTEMTIME::default(),
        FILETIME::default(),
    );
    // SAFETY: every pointer is to a live local, and `zone` lives for the
    // whole process.
    let converted = unsafe {
        FileTimeToSystemTime(&to_file_time(utc_ticks), &mut utc).is_ok()
            && SystemTimeToTzSpecificLocalTimeEx(Some(zone), &utc, &mut local).is_ok()
            && SystemTimeToFileTime(&local, &mut local_time).is_ok()
    };
    if !converted {
        return 0;
    }
    let minutes = (from_file_time(local_time) - utc_ticks) / TICKS_PER_SECOND / 60;
    minutes.clamp(-14 * 60, 14 * 60) as i32
}

/// Changes when the zone does, so the engine can rebuild indexes cached in
/// another zone. It's the zone's name and standard offset.
pub fn key() -> u64 {
    let Some(zone) = zone() else {
        return 0;
    };
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let name = zone.TimeZoneKeyName.iter().take_while(|unit| **unit != 0);
    for byte in name
        .flat_map(|unit| unit.to_le_bytes())
        .chain(zone.Bias.to_le_bytes())
    {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_are_whole_minutes_within_a_day() {
        // 2026-01-15 and 2026-07-15 at noon UTC, either side of most zones'
        // daylight saving time.
        for unix in [1_768_478_400, 1_784_116_800] {
            let offset = offset_at(unix);
            assert!((-14 * 60..=14 * 60).contains(&offset), "{offset}");
        }
        // Daylight saving time moves the clock by an hour at most.
        assert!((offset_at(1_768_478_400) - offset_at(1_784_116_800)).abs() <= 60);
        assert_eq!(offset_at(-1_000_000_000_000), 0);
        assert_eq!(key(), key());
    }
}
