//! Timestamps as the log wrote them. Time zones are read but not applied, so
//! values are wall-clock time and only compare correctly within one zone.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// Days from 1970-01-01 to 2000-01-01.
const EPOCH_DAYS: i64 = 10_957;
const MONTHS: [&[u8]; 12] = [
    b"jan", b"feb", b"mar", b"apr", b"may", b"jun", b"jul", b"aug", b"sep", b"oct", b"nov", b"dec",
];

/// A moment in wall-clock time, stored as seconds since 2000-01-01.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Stamp {
    pub seconds: u32,
    pub millis: u16,
}

/// Fills in the year for formats that leave it out, using the file's modified
/// date, which is more reliable than today's date for old rotated logs.
#[derive(Clone, Copy, Debug)]
pub struct YearHint {
    pub year: i32,
    pub month: u32,
}

impl YearHint {
    pub fn from_system_time(time: SystemTime) -> Self {
        let days = time
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| (elapsed.as_secs() / 86_400) as i64);
        let (year, month, _) = civil_from_days(days);
        Self { year, month }
    }

    /// A month later in the year than the file's date must be from the year before.
    fn year_for(self, month: u32) -> i32 {
        if month > self.month {
            self.year - 1
        } else {
            self.year
        }
    }
}

/// Howard Hinnant's days-from-civil algorithm: days since 1970-01-01.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month = i64::from(month);
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year as i32, month, day)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        _ => 28,
    }
}

impl Stamp {
    /// Returns `None` for impossible dates and anything before 2000.
    pub fn from_parts(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
        millis: u16,
    ) -> Option<Self> {
        if !(2000..=2130).contains(&year)
            || !(1..=12).contains(&month)
            || day == 0
            || day > days_in_month(year, month)
            || hour > 23
            || minute > 59
            || second > 60
        {
            return None;
        }
        let days = days_from_civil(year, month, day) - EPOCH_DAYS;
        let seconds =
            days * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second);
        Some(Self {
            seconds: u32::try_from(seconds).ok()?,
            millis,
        })
    }

    pub fn from_seconds(seconds: u32) -> Self {
        Self { seconds, millis: 0 }
    }

    /// The date, and the seconds into that day.
    fn civil(&self) -> ((i32, u32, u32), i64) {
        let total = i64::from(self.seconds);
        (civil_from_days(total / 86_400 + EPOCH_DAYS), total % 86_400)
    }

    /// The time to the second, as `2026-01-16 13:40:00`, for labels such as
    /// the histogram's. The time of day alone starts at character 11.
    pub fn label(&self) -> String {
        let ((year, month, day), in_day) = self.civil();
        format!(
            "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
            in_day / 3600,
            in_day / 60 % 60,
            in_day % 60,
        )
    }
}

impl fmt::Display for Stamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ((year, month, day), in_day) = self.civil();
        write!(
            formatter,
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}",
            in_day / 3600,
            in_day / 60 % 60,
            in_day % 60,
            self.millis,
        )
    }
}

fn digits(bytes: &[u8], at: usize, count: usize) -> Option<u32> {
    let slice = bytes.get(at..at + count)?;
    let mut value = 0u32;
    for byte in slice {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value * 10 + u32::from(byte - b'0');
    }
    Some(value)
}

/// Reads `.fff` digits at `at`. Returns milliseconds and the position after them.
fn fraction(bytes: &[u8], mut at: usize) -> (u16, usize) {
    if bytes.get(at) != Some(&b'.') {
        return (0, at);
    }
    let start = at + 1;
    at = start;
    let (mut millis, mut scale) = (0u16, 100u16);
    while let Some(byte) = bytes.get(at).filter(|byte| byte.is_ascii_digit()) {
        if at - start < 3 {
            millis += u16::from(byte - b'0') * scale;
            scale /= 10;
        }
        at += 1;
    }
    if at == start {
        return (0, start - 1);
    }
    (millis, at)
}

/// Reads `HH:MM:SS[.fff]` and returns its parts and the position after it.
fn clock(bytes: &[u8], at: usize) -> Option<(u32, u32, u32, u16, usize)> {
    let hour = digits(bytes, at, 2)?;
    if bytes.get(at + 2) != Some(&b':') {
        return None;
    }
    let minute = digits(bytes, at + 3, 2)?;
    if bytes.get(at + 5) != Some(&b':') {
        return None;
    }
    let second = digits(bytes, at + 6, 2)?;
    let (millis, end) = fraction(bytes, at + 8);
    Some((hour, minute, second, millis, end))
}

/// Skips `Z`, `+hh`, `+hhmm` or `+hh:mm` at `at`.
fn skip_zone(bytes: &[u8], at: usize) -> usize {
    match bytes.get(at) {
        Some(b'Z') => at + 1,
        Some(b'+' | b'-') if digits(bytes, at + 1, 2).is_some() => {
            if bytes.get(at + 3) == Some(&b':') && digits(bytes, at + 4, 2).is_some() {
                at + 6
            } else if digits(bytes, at + 3, 2).is_some() {
                at + 5
            } else {
                at + 3
            }
        }
        _ => at,
    }
}

/// What followed the date in a parsed timestamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParsedIso {
    pub stamp: Stamp,
    /// Position just after the timestamp.
    pub end: usize,
    pub space_separated: bool,
}

/// Reads `YYYY-MM-DD[T ]HH:MM:SS[.fff][zone]` at the start of `bytes`. Slashes
/// are also accepted in the date, because `WindowsUpdate.log` writes them.
pub fn parse_iso(bytes: &[u8]) -> Option<ParsedIso> {
    let year = digits(bytes, 0, 4)?;
    if !matches!(bytes.get(4), Some(b'-' | b'/')) {
        return None;
    }
    let month = digits(bytes, 5, 2)?;
    if bytes.get(7) != bytes.get(4) {
        return None;
    }
    let day = digits(bytes, 8, 2)?;
    let space_separated = match bytes.get(10) {
        Some(b' ') => true,
        Some(b'T') => false,
        _ => return None,
    };
    let (hour, minute, second, millis, end) = clock(bytes, 11)?;
    let stamp = Stamp::from_parts(year as i32, month, day, hour, minute, second, millis)?;
    Some(ParsedIso {
        stamp,
        end: skip_zone(bytes, end),
        space_separated,
    })
}

/// Reads the separate SCCM date (`MM-DD-YYYY`) and time (`HH:MM:SS.fff+bias`) values.
pub fn parse_sccm(date: &[u8], time: &[u8]) -> Option<Stamp> {
    let mut parts = date.split(|byte| *byte == b'-');
    let mut next = || -> Option<u32> {
        let part = parts
            .next()
            .filter(|part| !part.is_empty() && part.len() <= 4)?;
        digits(part, 0, part.len())
    };
    let (month, day, year) = (next()?, next()?, next()?);
    let (hour, minute, second, millis, _) = clock(time, 0)?;
    Stamp::from_parts(year as i32, month, day, hour, minute, second, millis)
}

/// Reads classic syslog's `MMM dd HH:MM:SS`, which has no year. Returns the
/// stamp and the position after it.
pub fn parse_syslog(bytes: &[u8], hint: YearHint) -> Option<(Stamp, usize)> {
    let name = bytes.get(..3)?;
    let month = MONTHS
        .iter()
        .position(|known| known.eq_ignore_ascii_case(name))? as u32
        + 1;
    let mut at = 3;
    if !matches!(bytes.get(at), Some(b' ' | b'\t')) {
        return None;
    }
    while matches!(bytes.get(at), Some(b' ' | b'\t')) {
        at += 1;
    }
    let day_start = at;
    while bytes.get(at).is_some_and(u8::is_ascii_digit) {
        at += 1;
    }
    if !(1..=2).contains(&(at - day_start)) {
        return None;
    }
    let day = digits(bytes, day_start, at - day_start)?;
    if !matches!(bytes.get(at), Some(b' ' | b'\t')) {
        return None;
    }
    while matches!(bytes.get(at), Some(b' ' | b'\t')) {
        at += 1;
    }
    let (hour, minute, second, millis, end) = clock(bytes, at)?;
    let stamp = Stamp::from_parts(
        hint.year_for(month),
        month,
        day,
        hour,
        minute,
        second,
        millis,
    )?;
    Some((stamp, end))
}

/// Reads 1 to `most` digits at `at`, moving past them.
fn number(bytes: &[u8], at: &mut usize, most: usize) -> Option<(u32, usize)> {
    let start = *at;
    while *at - start < most && bytes.get(*at).is_some_and(u8::is_ascii_digit) {
        *at += 1;
    }
    let count = *at - start;
    if count == 0 {
        return None;
    }
    digits(bytes, start, count).map(|value| (value, count))
}

/// Reads a timestamp in any shape a custom format's `time` part may hold:
/// ISO's `2026-01-31 10:22:01`, syslog's `Jan 31 10:22:01`, or a date with the
/// year last, as `01/31/2026 1:22:01 PM`. `day_first` reads that last kind as
/// `31/01/2026`. A two-digit year is taken to be after 2000.
pub fn parse_any(bytes: &[u8], hint: YearHint, day_first: bool) -> Option<Stamp> {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let bytes = &bytes[start..];
    if let Some(parsed) = parse_iso(bytes) {
        return Some(parsed.stamp);
    }
    if let Some((stamp, _)) = parse_syslog(bytes, hint) {
        return Some(stamp);
    }
    parse_year_last(bytes, day_first)
}

/// `MM/DD/YYYY HH:MM:SS[.fff][ AM|PM]`, with `/`, `-` or `.` between the date's
/// parts, and the hour and the date's first two parts in one or two digits.
fn parse_year_last(bytes: &[u8], day_first: bool) -> Option<Stamp> {
    let mut at = 0;
    let (first, _) = number(bytes, &mut at, 2)?;
    let separator = *bytes
        .get(at)
        .filter(|byte| matches!(byte, b'/' | b'-' | b'.'))?;
    at += 1;
    let (second, _) = number(bytes, &mut at, 2)?;
    if bytes.get(at) != Some(&separator) {
        return None;
    }
    at += 1;
    let year = match number(bytes, &mut at, 4)? {
        (year, 4) => year,
        (year, 2) => 2000 + year,
        _ => return None,
    };
    while matches!(bytes.get(at), Some(b' ' | b',' | b'T')) {
        at += 1;
    }
    let (mut hour, _) = number(bytes, &mut at, 2)?;
    if bytes.get(at) != Some(&b':') {
        return None;
    }
    let minute = digits(bytes, at + 1, 2)?;
    if bytes.get(at + 3) != Some(&b':') {
        return None;
    }
    let second_of_minute = digits(bytes, at + 4, 2)?;
    let (millis, mut end) = fraction(bytes, at + 6);
    while bytes.get(end) == Some(&b' ') {
        end += 1;
    }
    let half = bytes.get(end..end + 2).map(<[u8]>::to_ascii_uppercase);
    match half.as_deref() {
        Some(b"AM") if (1..=12).contains(&hour) => hour %= 12,
        Some(b"PM") if (1..=12).contains(&hour) => hour = hour % 12 + 12,
        _ => {}
    }
    let (month, day) = if day_first {
        (second, first)
    } else {
        (first, second)
    };
    Stamp::from_parts(
        year as i32,
        month,
        day,
        hour,
        minute,
        second_of_minute,
        millis,
    )
}

/// Reads a time typed by a person: a full timestamp or just `YYYY-MM-DD`.
pub fn parse_user_time(text: &str) -> Option<Stamp> {
    let bytes = text.trim().as_bytes();
    if let Some(parsed) = parse_iso(bytes) {
        return (parsed.end == bytes.len()).then_some(parsed.stamp);
    }
    if bytes.len() == 10 {
        let separators = bytes[4] == b'-' && bytes[7] == b'-';
        let year = digits(bytes, 0, 4)?;
        let month = digits(bytes, 5, 2)?;
        let day = digits(bytes, 8, 2)?;
        if separators {
            return Stamp::from_parts(year as i32, month, day, 0, 0, 0, 0);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_round_trip_through_display() {
        let stamp = Stamp::from_parts(2026, 3, 1, 13, 40, 14, 100).unwrap();
        assert_eq!(stamp.to_string(), "2026-03-01T13:40:14.100");
    }

    #[test]
    fn labels_show_the_time_to_the_second() {
        let stamp = Stamp::from_parts(2026, 1, 16, 13, 40, 0, 250).unwrap();
        assert_eq!(stamp.label(), "2026-01-16 13:40:00");
        assert_eq!(Stamp::from_seconds(0).label(), "2000-01-01 00:00:00");
        assert_eq!(&stamp.label()[11..], "13:40:00");
    }

    #[test]
    fn custom_format_times_take_every_common_shape() {
        let hint = YearHint {
            year: 2026,
            month: 6,
        };
        let read = |text: &str, day_first: bool| {
            parse_any(text.as_bytes(), hint, day_first).map(|stamp| stamp.to_string())
        };
        let expected = Some("2026-01-31T13:22:01.000".to_string());
        assert_eq!(read("2026-01-31 13:22:01", false), expected);
        assert_eq!(read(" 2026-01-31T13:22:01Z", false), expected);
        assert_eq!(read("Jan 31 13:22:01", false), expected);
        assert_eq!(read("01/31/2026 1:22:01 PM", false), expected);
        assert_eq!(read("1/31/26 13:22:01", false), expected);
        assert_eq!(read("31.01.2026 13:22:01", true), expected);
        assert_eq!(read("31-01-2026, 13:22:01", true), expected);
        assert_eq!(
            read("01/31/2026 12:05:00.250 am", false).as_deref(),
            Some("2026-01-31T00:05:00.250")
        );
        for wrong in [
            "31/01/2026 13:22:01",
            "01/31/202 13:22:01",
            "13:22:01",
            "soon",
            "",
        ] {
            assert_eq!(read(wrong, false), None, "{wrong}");
        }
    }

    #[test]
    fn leap_days_are_handled() {
        assert!(Stamp::from_parts(2024, 2, 29, 0, 0, 0, 0).is_some());
        assert!(Stamp::from_parts(2026, 2, 29, 0, 0, 0, 0).is_none());
        assert!(Stamp::from_parts(2100, 2, 29, 0, 0, 0, 0).is_none());
        assert_eq!(
            Stamp::from_parts(2024, 2, 29, 0, 0, 0, 0)
                .unwrap()
                .to_string(),
            "2024-02-29T00:00:00.000"
        );
    }

    #[test]
    fn rejects_impossible_values() {
        assert!(Stamp::from_parts(2026, 13, 1, 0, 0, 0, 0).is_none());
        assert!(Stamp::from_parts(2026, 4, 31, 0, 0, 0, 0).is_none());
        assert!(Stamp::from_parts(2026, 1, 1, 24, 0, 0, 0).is_none());
        assert!(Stamp::from_parts(1999, 1, 1, 0, 0, 0, 0).is_none());
    }

    #[test]
    fn parses_iso_variants() {
        for (text, expected) in [
            ("2026-04-23 14:25:00", "2026-04-23T14:25:00.000"),
            ("2026-04-23T14:25:00.812", "2026-04-23T14:25:00.812"),
            ("2026-04-23 14:25:00.8123456", "2026-04-23T14:25:00.812"),
            ("2026/04/23 14:25:00.5", "2026-04-23T14:25:00.500"),
            ("2026-04-23T14:25:00Z", "2026-04-23T14:25:00.000"),
            ("2026-04-23 14:25:00-05", "2026-04-23T14:25:00.000"),
            ("2026-04-23 14:25:00+05:30", "2026-04-23T14:25:00.000"),
            ("2026-04-23 14:25:00-0500", "2026-04-23T14:25:00.000"),
        ] {
            let parsed = parse_iso(text.as_bytes()).unwrap();
            assert_eq!(parsed.stamp.to_string(), expected, "{text}");
            assert_eq!(parsed.end, text.len(), "{text}");
        }
    }

    #[test]
    fn stops_at_the_end_of_the_timestamp() {
        let parsed = parse_iso(b"2026-04-23 14:25:00, Info").unwrap();
        assert_eq!(parsed.end, 19);
        assert!(parsed.space_separated);
    }

    #[test]
    fn mixed_date_separators_are_rejected() {
        assert!(parse_iso(b"2026-04/23 14:25:00").is_none());
    }

    #[test]
    fn parses_sccm_date_and_time() {
        let stamp = parse_sccm(b"04-06-2026", b"10:23:01.442+000").unwrap();
        assert_eq!(stamp.to_string(), "2026-04-06T10:23:01.442");
        assert!(parse_sccm(b"4-6-2026", b"10:23:01").is_some());
        assert!(parse_sccm(b"not-a-date", b"10:23:01").is_none());
    }

    #[test]
    fn syslog_year_comes_from_the_file_date() {
        let hint = YearHint {
            year: 2026,
            month: 2,
        };
        let (january, _) = parse_syslog(b"Jan 15 10:00:00 host", hint).unwrap();
        assert!(january.to_string().starts_with("2026-01-15"));
        let (december, _) = parse_syslog(b"Dec 31 23:59:59 host", hint).unwrap();
        assert!(december.to_string().starts_with("2025-12-31"));
    }

    #[test]
    fn parses_times_people_type() {
        assert_eq!(
            parse_user_time("2026-01-16").unwrap().to_string(),
            "2026-01-16T00:00:00.000"
        );
        assert!(parse_user_time("2026-01-16T13:40:00").is_some());
        assert!(parse_user_time("yesterday").is_none());
        assert!(parse_user_time("2026-01-16 13:40:00 junk").is_none());
    }

    #[test]
    fn file_dates_become_year_hints() {
        let time = UNIX_EPOCH + std::time::Duration::from_secs(1_771_200_000);
        let hint = YearHint::from_system_time(time);
        assert_eq!((hint.year, hint.month), (2026, 2));
    }
}
