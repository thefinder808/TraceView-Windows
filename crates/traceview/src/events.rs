//! Windows event logs. A saved `.evtx` file, or one of this PC's logs named
//! as `eventlog:System`, is written to a temporary text file, one row per
//! event in the form the engine reads as `Format::Event`, which is then
//! opened like any log. So filters, find, merging, summaries and the command
//! line all work on events as they do on other logs.
//!
//! The copy is temporary, so logs such as Security don't stay behind in the
//! user's profile: the command line deletes it as soon as it's open, and
//! Windows removes it once it's closed, and the viewer deletes it when its
//! tab closes.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};
use tv_engine::time::{offset_text, Stamp};
use tv_engine::{LogFile, OpenStats};
use tv_platform::events::{self as platform, Event, EventSource};

/// Names one of this PC's event logs where a path would go, as in
/// `eventlog:System`.
pub const PREFIX: &str = "eventlog:";
/// Days a temporary copy left by a crash is kept before it's cleared away.
const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
/// FILETIME counts from 1601, and Unix time from 1970.
const UNIX_EPOCH_SECONDS: i64 = 11_644_473_600;
/// Seconds from 1970 to 2000, where the engine's times count from.
const SINCE_2000: i64 = 946_684_800;

/// The event log a path names, if it's one: a `.evtx` file, or one of this
/// PC's logs, as `eventlog:System`.
pub fn source(path: &Path) -> Option<EventSource<'_>> {
    let text = path.to_str()?;
    if let Some(name) = text
        .get(..PREFIX.len())
        .filter(|prefix| prefix.eq_ignore_ascii_case(PREFIX))
        .map(|_| &text[PREFIX.len()..])
    {
        return (!name.trim().is_empty()).then_some(EventSource::Log(name.trim()));
    }
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("evtx"))
        .then_some(EventSource::File(path))
}

/// A temporary text copy of an event log, deleted when dropped.
pub struct Converted {
    pub path: PathBuf,
    /// The newest event written, so later ones can be added.
    pub last_record: u64,
    pub events: usize,
}

impl Drop for Converted {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn folder() -> PathBuf {
    std::env::temp_dir().join("TraceView")
}

/// Clears copies a crash left behind, a day after they were written.
fn clear_stale(folder: &Path) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| now.duration_since(modified).unwrap_or_default() > STALE_AFTER);
        if name.starts_with("events-") && name.ends_with(".log") && old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// A new, empty file for a copy.
fn new_file() -> io::Result<(PathBuf, File)> {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let folder = folder();
    fs::create_dir_all(&folder)?;
    clear_stale(&folder);
    loop {
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = folder.join(format!("events-{}-{number}.log", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

/// Writes a source's events to a new temporary copy. `progress` counts the
/// events written.
pub fn convert(source: EventSource<'_>, progress: &AtomicU64) -> Result<Converted, String> {
    let (path, file) = new_file().map_err(|error| format!("couldn't make its copy: {error}"))?;
    let mut converted = Converted {
        path,
        last_record: 0,
        events: 0,
    };
    let written = write_after(source, &mut converted, BufWriter::new(file), progress)?;
    converted.events = written;
    Ok(converted)
}

fn write_after(
    source: EventSource<'_>,
    converted: &mut Converted,
    mut out: impl Write,
    progress: &AtomicU64,
) -> Result<usize, String> {
    let mut failed = None;
    let mut last = converted.last_record;
    let mut previous_time = String::new();
    let count = platform::read_events(source, converted.last_record, |event| {
        let text = row(&event, &mut previous_time);
        if let Err(error) = out.write_all(text.as_bytes()) {
            failed = Some(error);
            return false;
        }
        last = last.max(event.record);
        progress.fetch_add(1, Ordering::Relaxed);
        true
    })?;
    if let Some(error) = failed.or_else(|| out.flush().err()) {
        return Err(format!("couldn't write its copy: {error}"));
    }
    converted.last_record = last;
    Ok(count)
}

/// Event Viewer's name for a level.
fn level_word(level: u8) -> &'static str {
    match level {
        1 => "Critical",
        2 => "Error",
        3 => "Warning",
        5 => "Verbose",
        _ => "Information",
    }
}

/// A one-line field: nothing in it may end the field or the row.
fn field(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\t' | '\r' | '\n' | '\u{1E}' => ' ',
            c => c,
        })
        .filter(|c| *c != '\0')
        .collect::<String>()
        .trim()
        .to_string()
}

/// A message, which may take several lines, but never one that starts a row.
fn message_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .chars()
        .filter_map(|c| match c {
            '\r' => Some('\n'),
            '\u{1E}' => Some('\u{241E}'),
            '\0' => None,
            c => Some(c),
        })
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// An event's time in this PC's time zone, with its offset, as in
/// `2026-11-20 09:51:57.123-06:00`, or none if it can't be shown.
fn time_text(time: u64) -> Option<String> {
    let unix = i64::try_from(time / 10_000_000).ok()? - UNIX_EPOCH_SECONDS;
    let millis = ((time / 10_000) % 1000) as u16;
    let offset = tv_platform::time_zone::offset_at(unix);
    let local = unix + i64::from(offset) * 60;
    let stamp = Stamp {
        seconds: u32::try_from(local - SINCE_2000).ok()?,
        millis,
    };
    Some(format!(
        "{}{}",
        stamp.to_string().replace('T', " "),
        offset_text(offset)
    ))
}

/// An event's row: its time, level, provider, event ID, computer and
/// message. An event with no time takes the one before it, so it stays in
/// place.
fn row(event: &Event, previous_time: &mut String) -> String {
    if let Some(time) = time_text(event.time) {
        *previous_time = time;
    } else if previous_time.is_empty() {
        *previous_time = "2000-01-01 00:00:00.000+00:00".to_string();
    }
    format!(
        "\u{1E}{}\t{}\t{}\t{}\t{}\t{}\n",
        previous_time,
        level_word(event.level),
        field(&event.provider),
        event.id,
        field(&event.computer),
        message_text(&event.message)
    )
}

/// Opens an event log as a log, for the command line, shown as `shown`.
/// Its copy is deleted at once, which Windows finishes when the log is
/// closed, as the engine lets files it has open be deleted.
pub fn open(source: EventSource<'_>, shown: &Path) -> Result<(LogFile, OpenStats), String> {
    let converted = convert(source, &AtomicU64::new(0))?;
    let (mut log, stats) = LogFile::open_with(&converted.path, None, &[], &AtomicU64::new(0))
        .map_err(|error| format!("couldn't open its copy: {error}"))?;
    drop(converted);
    log.show_as(shown.to_path_buf());
    Ok((log, stats))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tv_engine::{parse, Format, Level};

    #[test]
    fn evtx_files_and_named_logs_are_event_logs() {
        assert!(matches!(
            source(Path::new(r"C:\Logs\System.evtx")),
            Some(EventSource::File(_))
        ));
        assert!(matches!(
            source(Path::new(r"\\SRV01\c$\Logs\App.EVTX")),
            Some(EventSource::File(_))
        ));
        assert!(matches!(
            source(Path::new("eventlog:System")),
            Some(EventSource::Log("System"))
        ));
        assert!(matches!(
            source(Path::new(
                "EventLog:Microsoft-Windows-TaskScheduler/Operational"
            )),
            Some(EventSource::Log(
                "Microsoft-Windows-TaskScheduler/Operational"
            ))
        ));
        assert!(source(Path::new("eventlog:")).is_none());
        assert!(source(Path::new(r"C:\Logs\CBS.log")).is_none());
    }

    fn event(message: &str) -> Event {
        Event {
            record: 7,
            // 2026-11-20 15:51:57.123 UTC.
            time: (1_795_189_917 + UNIX_EPOCH_SECONDS as u64) * 10_000_000 + 1_230_000,
            level: 2,
            provider: "Service\tControl\nManager".to_string(),
            id: 7000,
            computer: "SRV01".to_string(),
            message: message.to_string(),
        }
    }

    #[test]
    fn a_crafted_event_cant_make_rows_of_its_own() {
        let forged =
            "Fine.\r\n\u{1E}2026-01-01 00:00:00.000+00:00\tCritical\tFake\t1\tPC\tForged row\r\n";
        let text = row(&event(forged), &mut String::new());
        assert_eq!(text.matches('\u{1E}').count(), 1, "{text:?}");
        assert!(text.ends_with('\n') && !text.ends_with("\n\n"));
        let parsed = parse::fields(Format::Event, text.as_bytes(), &context());
        assert_eq!(parsed.level, Level::Error);
        assert_eq!(parsed.component, b"Service Control Manager");
        assert_eq!(parsed.thread, b"7000");
        assert_eq!(parsed.source, b"SRV01");
        assert!(String::from_utf8_lossy(parsed.message).starts_with("Fine.\n\u{241E}2026"));
    }

    fn context() -> tv_engine::parse::ParseContext {
        tv_engine::parse::ParseContext {
            year_hint: tv_engine::time::YearHint {
                year: 2026,
                month: 11,
            },
            custom: None,
            hint: tv_engine::parse::Hint::None,
        }
    }

    #[test]
    fn rows_have_a_local_time_and_its_offset() {
        let text = row(&event("Message."), &mut String::new());
        let time = text[1..].split('\t').next().unwrap();
        let parsed = tv_engine::time::parse_iso(time.as_bytes()).unwrap();
        assert!(parsed.zone.is_some(), "{time}");
        assert!(time.contains(".123"), "{time}");
        // An event with no time keeps the one before it.
        let mut previous = String::new();
        let first = row(&event("a"), &mut previous);
        let mut timeless = event("b");
        timeless.time = 0;
        let second = row(&timeless, &mut previous);
        assert_eq!(
            first[1..].split('\t').next(),
            second[1..].split('\t').next()
        );
    }

    /// This process's copies in the temporary folder.
    fn copies() -> usize {
        let mine = format!("events-{}-", std::process::id());
        fs::read_dir(folder()).map_or(0, |entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().starts_with(&mine))
                .count()
        })
    }

    #[test]
    fn this_pcs_system_log_opens_as_a_log_and_its_copy_goes() {
        let folder_before = copies();
        let (log, _) = open(EventSource::Log("System"), Path::new("eventlog:System")).unwrap();
        assert_eq!(log.format(), Format::Event);
        assert!(log.rows() > 0);
        assert_eq!(log.path(), Path::new("eventlog:System"));
        assert_eq!(copies(), folder_before, "the copy is deleted once open");
        let mut scratch = Vec::new();
        let mut buffer = Vec::new();
        let mut checked = 0;
        log.read_rows(0..log.rows().min(50), &mut buffer, |_, raw, _| {
            let fields = log.fields(log.text(raw, &mut scratch));
            assert!(fields.stamp.is_some());
            assert!(!fields.component.is_empty());
            checked += 1;
        })
        .unwrap();
        assert!(checked > 0);
    }
}
