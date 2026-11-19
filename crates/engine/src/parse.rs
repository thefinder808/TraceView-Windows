//! Splits one row's UTF-8 bytes into fields. These are hand-written rather than
//! regexes because indexing runs them on every row of very large files.
//!
//! Plain-text formats are tried from most specific to most general, in the same
//! order as the macOS app, plus the CBS and DISM format used on Windows.

use crate::custom::CustomFormat;
use crate::level::{self, Level};
use crate::time::{self, Stamp, YearHint, Zone};
use memchr::memmem;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const SCCM_START: &[u8] = b"<![LOG[";
const SCCM_END: &[u8] = b"]LOG]!>";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum Format {
    Plain = 0,
    Sccm = 1,
    /// A format defined by a pattern, which the parse context carries.
    Custom = 2,
}

impl Format {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Format::Plain),
            1 => Some(Format::Sccm),
            2 => Some(Format::Custom),
            _ => None,
        }
    }
}

/// Facts about the file that some formats need to read a row.
#[derive(Clone, Debug)]
pub struct ParseContext {
    pub year_hint: YearHint,
    /// The definition of a custom format, when the file has one.
    pub custom: Option<Arc<CustomFormat>>,
    /// What the log's first lines say about reading its rows.
    pub hint: Hint,
}

/// What a plain log's first lines say about how to read rows that don't say
/// it themselves. It's found from the start of the file when it opens, so
/// every row can still be read on its own, in any order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Hint {
    #[default]
    None,
    /// A Windows Installer log. Its rows give only a time of day, so they take
    /// the day logging started, and a time before that is the day after.
    Msi { started: Stamp },
    /// A PowerShell transcript, whose error records are marked as errors.
    Transcript,
    /// A `setupapi` device install log, whose `!!!` and `!` mark errors and
    /// warnings, rather than its words.
    SetupApi,
    /// A W3C extended log, as IIS and Windows Firewall write, whose `#Fields:`
    /// line says which space-separated column is which.
    W3c(W3cFields),
}

/// Where a W3C extended log's columns are, from its `#Fields:` line. Each is
/// a column's number, if the log has it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct W3cFields {
    pub date: Option<u8>,
    pub time: Option<u8>,
    /// IIS's `sc-status`, the HTTP status, which sets the level.
    pub status: Option<u8>,
    /// Windows Firewall's `action`, such as `DROP`, which sets the level and
    /// is the component.
    pub action: Option<u8>,
    /// IIS's `cs-uri-stem`, the page asked for, which is the component.
    pub page: Option<u8>,
    /// The client's address, `c-ip` or `src-ip`, which is the source.
    pub client: Option<u8>,
    /// Windows Firewall's `pid`, which is the thread.
    pub process: Option<u8>,
    /// W3C times are UTC unless `#Time Format: Local` says otherwise, as
    /// Windows Firewall's does.
    pub utc: bool,
    /// The zone UTC times are shown in, so they line up with other logs'
    /// local times: this PC's, except in tests.
    pub zone: Zone,
}

impl W3cFields {
    /// Reads the directives at the start of a log, or `None` if they don't
    /// name a date and a time column.
    fn from_directives(head: &[u8]) -> Option<Self> {
        let mut fields = Self {
            utc: true,
            ..Self::default()
        };
        for line in head
            .split(|byte| *byte == b'\n')
            .map(|line| trim_spaces(trim_line_ending(line)))
            .take_while(|line| line.first() == Some(&b'#'))
        {
            if let Some(names) = line.strip_prefix(b"#Fields:") {
                let names = names
                    .split(|byte| *byte == b' ')
                    .filter(|name| !name.is_empty());
                for (index, name) in names.take(usize::from(u8::MAX)).enumerate() {
                    let index = Some(index as u8);
                    match name {
                        b"date" => fields.date = index,
                        b"time" => fields.time = index,
                        b"sc-status" => fields.status = index,
                        b"action" => fields.action = index,
                        b"cs-uri-stem" => fields.page = index,
                        b"c-ip" | b"src-ip" => fields.client = index,
                        b"pid" => fields.process = index,
                        _ => {}
                    }
                }
            } else if let Some(format) = line.strip_prefix(b"#Time Format:") {
                fields.utc = !trim_spaces(format).eq_ignore_ascii_case(b"local");
            }
        }
        (fields.date.is_some() && fields.time.is_some()).then_some(fields)
    }
}

/// Finds a plain log's hint from its first lines, as UTF-8.
pub fn hint_for(head: &[u8]) -> Hint {
    let head = head.strip_prefix("\u{feff}".as_bytes()).unwrap_or(head);
    let first = head
        .split(|byte| *byte == b'\n')
        .map(|line| trim_spaces(trim_line_ending(line)))
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let msi_started = [
        &b"=== Verbose logging started: "[..],
        b"=== Logging started: ",
    ]
    .iter()
    .find_map(|prefix| first.strip_prefix(*prefix))
    .and_then(time::parse_msi_date);
    let w3c = [&b"#Software:"[..], b"#Version:", b"#Date:"]
        .iter()
        .any(|directive| first.starts_with(directive))
        .then(|| W3cFields::from_directives(head))
        .flatten();
    if let Some(started) = msi_started {
        Hint::Msi { started }
    } else if let Some(fields) = w3c {
        Hint::W3c(fields)
    } else if first == b"[Device Install Log]" {
        Hint::SetupApi
    } else if memmem::find(&head[..head.len().min(512)], b"PowerShell transcript start").is_some() {
        Hint::Transcript
    } else {
        Hint::None
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Fields<'a> {
    pub stamp: Option<Stamp>,
    pub level: Level,
    pub component: &'a [u8],
    pub thread: &'a [u8],
    pub source: &'a [u8],
    pub message: &'a [u8],
}

impl<'a> Fields<'a> {
    fn message_only(message: &'a [u8], level: Level) -> Self {
        Self {
            stamp: None,
            level,
            component: b"",
            thread: b"",
            source: b"",
            message,
        }
    }
}

pub fn fields<'a>(format: Format, row: &'a [u8], context: &ParseContext) -> Fields<'a> {
    let line = trim_line_ending(row);
    match format {
        Format::Plain => plain_fields(line, context),
        Format::Sccm => {
            sccm_fields(line).unwrap_or_else(|| Fields::message_only(line, level::detect(line)))
        }
        // A line the pattern doesn't match is shown whole, with its level
        // from its words, as plain text with no time would be.
        Format::Custom => context
            .custom
            .as_deref()
            .and_then(|custom| custom.fields(line, context))
            .unwrap_or_else(|| Fields::message_only(line, level::detect(line))),
    }
}

/// The offset from UTC that a row's time was written in, as minutes east of
/// UTC, when the row says. SCCM rows end their time with the bias Windows adds
/// to local time to get UTC, so `10:23:01.442+360` is six hours behind UTC.
/// Other rows that start with an ISO time can end it with `Z` or `+hh:mm`.
/// It's read only for rows being shown, as the index keeps no zones.
pub fn utc_offset(format: Format, row: &[u8]) -> Option<i32> {
    let line = trim_line_ending(row);
    match format {
        Format::Sccm => {
            let message = sccm_message(line)?;
            let attributes = &line[SCCM_START.len() + message.len() + SCCM_END.len()..];
            bias_offset(sccm_attribute(attributes, b"time=\""))
        }
        Format::Plain | Format::Custom => {
            let line = trim_spaces(line);
            if format == Format::Plain {
                if let Some(server) = server_parts(line) {
                    return bias_offset(server.time);
                }
            }
            match time::parse_iso(line) {
                Some(parsed) => parsed.zone,
                // ReportingEvents.log writes its offset as -0500.
                None if format == Format::Plain => reporting_events_start(line)?.1,
                None => None,
            }
        }
    }
}

/// For a W3C log written in UTC, the offset its row's time was shown in, as
/// minutes east of UTC, so a row says which zone its time is in.
pub fn w3c_offset(row: &[u8], columns: W3cFields) -> Option<i32> {
    let text = trim_spaces(trim_line_ending(row));
    let as_written = W3cFields {
        zone: Zone::Fixed(0),
        ..columns
    };
    let utc = w3c_fields(text, as_written).stamp?;
    Some(columns.zone.offset_at(utc))
}

/// The offset from UTC in an SCCM time such as `10:23:01.442+360`, whose bias
/// is what Windows adds to local time to get UTC.
fn bias_offset(time: &[u8]) -> Option<i32> {
    let sign_at = time.iter().rposition(|byte| matches!(byte, b'+' | b'-'))?;
    let bias = &time[sign_at + 1..];
    if bias.is_empty() || bias.len() > 4 || !bias.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let bias: i32 = std::str::from_utf8(bias).ok()?.parse().ok()?;
    if bias > 14 * 60 {
        return None;
    }
    Some(if time[sign_at] == b'+' { -bias } else { bias })
}

/// The text a filter searches. SCCM metadata, and the part of a site server
/// row after its `$$<`, is left out, so searching for `type` or `thread`
/// doesn't match every row.
pub fn searchable(format: Format, row: &[u8]) -> &[u8] {
    let line = trim_line_ending(row);
    match format {
        Format::Plain => server_parts(trim_spaces(line)).map_or(line, |server| server.message),
        Format::Custom => line,
        Format::Sccm => sccm_message(line).unwrap_or(line),
    }
}

/// Treats a file as SCCM when its start holds a complete entry.
pub fn detect_format(head: &[u8]) -> Format {
    let has_start = head.starts_with(SCCM_START) || memmem::find(head, b"\n<![LOG[").is_some();
    if has_start && memmem::find(head, SCCM_END).is_some() {
        Format::Sccm
    } else {
        Format::Plain
    }
}

fn trim_line_ending(row: &[u8]) -> &[u8] {
    let line = row.strip_suffix(b"\n").unwrap_or(row);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn is_space(byte: u8) -> bool {
    byte == b' ' || byte == b'\t'
}

fn skip_spaces(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !is_space(*byte))
        .unwrap_or(bytes.len());
    &bytes[start..]
}

fn trim_spaces(bytes: &[u8]) -> &[u8] {
    let bytes = skip_spaces(bytes);
    let end = bytes
        .iter()
        .rposition(|byte| !is_space(*byte))
        .map_or(0, |at| at + 1);
    &bytes[..end]
}

/// Splits off the first space-delimited word. Returns `None` if nothing follows
/// the word or no space separates them.
fn split_word(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let end = bytes.iter().position(|byte| is_space(*byte))?;
    if end == 0 {
        return None;
    }
    Some((&bytes[..end], skip_spaces(&bytes[end..])))
}

fn sccm_message(line: &[u8]) -> Option<&[u8]> {
    let body = line.strip_prefix(SCCM_START)?;
    Some(&body[..memmem::rfind(body, SCCM_END)?])
}

fn sccm_attribute<'a>(attributes: &'a [u8], name: &[u8]) -> &'a [u8] {
    let Some(found) = memmem::find(attributes, name) else {
        return b"";
    };
    let value = &attributes[found + name.len()..];
    memchr::memchr(b'"', value).map_or(b"", |end| &value[..end])
}

fn sccm_fields(line: &[u8]) -> Option<Fields<'_>> {
    let message = sccm_message(line)?;
    let attributes = &line[SCCM_START.len() + message.len() + SCCM_END.len()..];
    let level = match sccm_attribute(attributes, b"type=\"") {
        b"2" => Level::Warning,
        b"3" => Level::Error,
        _ => Level::Info,
    };
    Some(Fields {
        stamp: time::parse_sccm(
            sccm_attribute(attributes, b"date=\""),
            sccm_attribute(attributes, b"time=\""),
        ),
        level,
        component: sccm_attribute(attributes, b"component=\""),
        thread: sccm_attribute(attributes, b"thread=\""),
        source: sccm_attribute(attributes, b"file=\""),
        message,
    })
}

/// The parts of a ConfigMgr site server row, as CMTrace reads them:
/// `message  $$<SMS_EXECUTIVE><11-18-2026 09:30:12.123+300><thread=1234 (0x4D2)>`.
struct ServerParts<'a> {
    message: &'a [u8],
    component: &'a [u8],
    date: &'a [u8],
    time: &'a [u8],
    thread: &'a [u8],
}

const SERVER_START: &[u8] = b"$$<";

/// Splits a site server row. Only rows ending as one does are searched for
/// its `$$<`, which keeps this quick for every other plain row.
fn server_parts(text: &[u8]) -> Option<ServerParts<'_>> {
    if !text.ends_with(b")>") {
        return None;
    }
    let start = memmem::rfind(text, SERVER_START)?;
    let trailer = text[start + SERVER_START.len()..].strip_suffix(b">")?;
    let mut parts = trailer.split(|byte| *byte == b'>');
    let component = parts.next()?;
    let when = parts.next()?.strip_prefix(b"<")?;
    let thread = parts.next()?.strip_prefix(b"<thread=")?;
    if parts.next().is_some() {
        return None;
    }
    let space = memchr::memchr(b' ', when)?;
    let thread = &thread[..memchr::memchr(b' ', thread).unwrap_or(thread.len())];
    if thread.is_empty() || !thread.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(ServerParts {
        message: trim_spaces(&text[..start]),
        component,
        date: &when[..space],
        time: &when[space + 1..],
        thread,
    })
}

/// A Windows Installer time of day, on the day logging started, or the day
/// after if it's earlier than the start.
fn msi_stamp(time_of_day: &[u8], started: Stamp) -> Option<Stamp> {
    let (seconds, millis) = time::parse_time_of_day(time_of_day)?;
    let midnight = started.seconds - started.seconds % 86_400;
    let next_day = if seconds < started.seconds % 86_400 {
        86_400
    } else {
        0
    };
    Some(Stamp {
        seconds: midnight.checked_add(seconds + next_day)?,
        millis,
    })
}

/// `Return value 3` is how Windows Installer says an action failed, and 2 that
/// it was cancelled. Anything else goes by its words, except that `Verbose
/// logging started` and the `Debug` policy, in every such log, aren't debug.
fn msi_level(message: &[u8]) -> Level {
    if memmem::find(message, b"Return value 3").is_some() {
        Level::Error
    } else if memmem::find(message, b"Return value 2").is_some() {
        Level::Warning
    } else {
        match level::detect(message) {
            Level::Debug => Level::Info,
            found => found,
        }
    }
}

/// A Windows Installer log's rows: `MSI (s) (A4:B8) [15:42:16:698]: message`,
/// whose context and process and thread are the component and thread, and
/// `Action start 15:42:20: InstallInitialize.`, besides the `=== Logging
/// started: 11/18/2026  15:42:16 ===` lines, which give the whole date. Other
/// rows, such as properties, have no time.
fn msi_fields(text: &[u8], started: Stamp) -> Option<Fields<'_>> {
    if let Some(rest) = text.strip_prefix(b"=== ") {
        let colon = memchr::memchr(b':', rest)?;
        let stamp = time::parse_msi_date(skip_spaces(&rest[colon + 1..]))?;
        return Some(Fields {
            stamp: Some(stamp),
            ..Fields::message_only(text, msi_level(text))
        });
    }
    if let Some(rest) = text.strip_prefix(b"MSI (") {
        let close = memchr::memchr(b')', rest)?;
        let component = &text[..b"MSI (".len() + close + 1];
        let rest = skip_spaces(&rest[close + 1..]).strip_prefix(b"(")?;
        let close = memchr::memchr(b')', rest)?;
        let thread = &rest[..close];
        let rest = skip_spaces(&rest[close + 1..]).strip_prefix(b"[")?;
        let close = memchr::memchr(b']', rest)?;
        let stamp = msi_stamp(&rest[..close], started)?;
        let message = skip_spaces(rest[close + 1..].strip_prefix(b":")?);
        return Some(Fields {
            stamp: Some(stamp),
            component,
            thread,
            ..Fields::message_only(message, msi_level(message))
        });
    }
    let rest = text
        .strip_prefix(b"Action start ")
        .or_else(|| text.strip_prefix(b"Action ended "))?;
    let stamp = msi_stamp(rest, started)?;
    Some(Fields {
        stamp: Some(stamp),
        ..Fields::message_only(text, msi_level(text))
    })
}

/// PowerShell's approved verbs, from `Get-Verb`, in byte order. A cmdlet's
/// name starts with one, which tells an error record's first line from
/// other output.
const POWERSHELL_VERBS: [&[u8]; 98] = [
    b"Add",
    b"Approve",
    b"Assert",
    b"Backup",
    b"Block",
    b"Checkpoint",
    b"Clear",
    b"Close",
    b"Compare",
    b"Complete",
    b"Compress",
    b"Confirm",
    b"Connect",
    b"Convert",
    b"ConvertFrom",
    b"ConvertTo",
    b"Copy",
    b"Debug",
    b"Deny",
    b"Disable",
    b"Disconnect",
    b"Dismount",
    b"Edit",
    b"Enable",
    b"Enter",
    b"Exit",
    b"Expand",
    b"Export",
    b"Find",
    b"Format",
    b"Get",
    b"Grant",
    b"Group",
    b"Hide",
    b"Import",
    b"Initialize",
    b"Install",
    b"Invoke",
    b"Join",
    b"Limit",
    b"Lock",
    b"Measure",
    b"Merge",
    b"Mount",
    b"Move",
    b"New",
    b"Open",
    b"Optimize",
    b"Out",
    b"Ping",
    b"Pop",
    b"Protect",
    b"Publish",
    b"Push",
    b"Read",
    b"Receive",
    b"Redo",
    b"Register",
    b"Remove",
    b"Rename",
    b"Repair",
    b"Request",
    b"Reset",
    b"Resize",
    b"Resolve",
    b"Restart",
    b"Restore",
    b"Resume",
    b"Revoke",
    b"Save",
    b"Search",
    b"Select",
    b"Send",
    b"Set",
    b"Show",
    b"Skip",
    b"Split",
    b"Start",
    b"Step",
    b"Stop",
    b"Submit",
    b"Suspend",
    b"Switch",
    b"Sync",
    b"Test",
    b"Trace",
    b"Unblock",
    b"Undo",
    b"Uninstall",
    b"Unlock",
    b"Unprotect",
    b"Unpublish",
    b"Unregister",
    b"Update",
    b"Use",
    b"Wait",
    b"Watch",
    b"Write",
];

/// The first line of an error record: `Get-Item : Cannot find path…` in
/// Windows PowerShell, or `Get-Item: C:\script.ps1:6` in PowerShell 7.
fn is_error_record_start(text: &[u8]) -> bool {
    let Some(colon) = memchr::memchr(b':', &text[..text.len().min(64)]) else {
        return false;
    };
    let name = trim_spaces(&text[..colon]);
    let Some(dash) = memchr::memchr(b'-', name) else {
        return false;
    };
    let noun = &name[dash + 1..];
    POWERSHELL_VERBS.binary_search(&&name[..dash]).is_ok()
        && noun.first().is_some_and(u8::is_ascii_uppercase)
        && noun.iter().all(u8::is_ascii_alphanumeric)
        && text.get(colon + 1) == Some(&b' ')
}

/// A PowerShell transcript's rows. Only the times in its headers, as in
/// `Start time: 20261118154047` or `Command start time:`, are rows' times.
/// Error records are errors: their first line, `PS>TerminatingError(…)`, and
/// Windows PowerShell's `+ CategoryInfo` and `+ FullyQualifiedErrorId`.
/// `WARNING:` and the rest read as in any other log.
fn transcript_fields(text: &[u8]) -> Option<Fields<'_>> {
    let timed = [&b"Start time: "[..], b"End time: ", b"Command start time: "]
        .iter()
        .find_map(|prefix| text.strip_prefix(*prefix))
        .and_then(time::parse_compact);
    if let Some(stamp) = timed {
        return Some(Fields {
            stamp: Some(stamp),
            ..Fields::message_only(text, Level::Info)
        });
    }
    let error = text.starts_with(b"PS>TerminatingError(")
        || text.starts_with(b"+ CategoryInfo ")
        || text.starts_with(b"+ FullyQualifiedErrorId ")
        || is_error_record_start(text);
    error.then(|| Fields::message_only(text, Level::Error))
}

/// A `setupapi` device install log's rows. `!!!` marks an error and `!` a
/// warning, and other rows are information whatever their words, so
/// `Error Control = 1` isn't an error. `>>>  Section start` and `<<<  Section
/// end` give the time, and `[Exit status: FAILURE(…)]` is an error. The short
/// name before a colon, as in `dvi:`, is the component.
fn setupapi_fields(text: &[u8]) -> Fields<'_> {
    let (marked, rest) = if let Some(rest) = text.strip_prefix(b"!!!") {
        (Level::Error, skip_spaces(rest))
    } else if let Some(rest) = text.strip_prefix(b"!") {
        (Level::Warning, skip_spaces(rest))
    } else {
        (Level::Info, text)
    };
    if let Some(section) = rest
        .strip_prefix(b">>>")
        .or_else(|| rest.strip_prefix(b"<<<"))
    {
        let section = skip_spaces(section);
        let stamp = section
            .strip_prefix(b"Section start ")
            .or_else(|| section.strip_prefix(b"Section end "))
            .and_then(time::parse_iso)
            .map(|parsed| parsed.stamp);
        let level = if section.starts_with(b"[Exit status: FAILURE") {
            Level::Error
        } else {
            marked
        };
        return Fields {
            stamp,
            ..Fields::message_only(text, level)
        };
    }
    let category = memchr::memchr(b':', &rest[..rest.len().min(5)])
        .filter(|colon| *colon >= 2 && rest[..*colon].iter().all(u8::is_ascii_alphabetic));
    match category {
        Some(colon) => Fields {
            component: &rest[..colon],
            ..Fields::message_only(skip_spaces(&rest[colon + 1..]), marked)
        },
        None => Fields::message_only(rest, marked),
    }
}

/// The most columns a W3C row is split into. IIS has 21 at most.
const W3C_COLUMNS: usize = 48;

/// A W3C extended log's rows, from IIS or Windows Firewall, read by the
/// columns its `#Fields:` line names. The date and time give the time, and
/// the rest of the row is the message. IIS's page is the component, and its
/// status the level: 5xx is an error, and 4xx a warning, except 401, the
/// challenge every Windows-authenticated request gets first. Windows
/// Firewall's action is the component, `DROP` a warning, and its process ID
/// the thread. The client's address is the source, and `-` is empty. Other
/// rows aren't judged by their words, as pages and paths would mislead.
fn w3c_fields(text: &[u8], columns: W3cFields) -> Fields<'_> {
    // A UTC time is shown in the log's zone, which is this PC's.
    let shown = |stamp: Option<Stamp>| {
        if columns.utc {
            stamp.and_then(|stamp| columns.zone.from_utc(stamp))
        } else {
            stamp
        }
    };
    if let Some(directive) = text.strip_prefix(b"#") {
        let stamp = directive
            .strip_prefix(b"Date:")
            .and_then(|date| time::parse_iso(skip_spaces(date)))
            .map(|parsed| parsed.stamp);
        return Fields {
            stamp: shown(stamp),
            ..Fields::message_only(text, Level::Info)
        };
    }
    let mut parts: [&[u8]; W3C_COLUMNS] = [b""; W3C_COLUMNS];
    let mut count = 0;
    for part in text.split(|byte| *byte == b' ').take(W3C_COLUMNS) {
        parts[count] = part;
        count += 1;
    }
    let column = |index: Option<u8>| {
        let part = parts[..count].get(usize::from(index?))?;
        Some(if *part == b"-" { &b""[..] } else { *part })
    };
    let stamp = match (column(columns.date), column(columns.time)) {
        (Some(date), Some(time)) if date.len() + time.len() < 63 => {
            let mut joined = [0u8; 64];
            joined[..date.len()].copy_from_slice(date);
            joined[date.len()] = b' ';
            joined[date.len() + 1..date.len() + 1 + time.len()].copy_from_slice(time);
            time::parse_iso(&joined[..date.len() + 1 + time.len()]).map(|parsed| parsed.stamp)
        }
        _ => None,
    };
    let status = column(columns.status)
        .and_then(|status| std::str::from_utf8(status).ok()?.parse::<u16>().ok());
    let action = column(columns.action).unwrap_or_default();
    let level = match status {
        Some(500..=599) => Level::Error,
        Some(400..=499) if status != Some(401) => Level::Warning,
        _ if action == b"DROP" || action == b"INFO-EVENTS-LOST" => Level::Warning,
        _ => Level::Info,
    };
    // When the date and time come first, as they do in both, the message is
    // what follows them.
    let message = match (columns.date, columns.time) {
        (Some(0), Some(1)) | (Some(1), Some(0)) if count > 2 => {
            &text[parts[0].len() + parts[1].len() + 2..]
        }
        _ => text,
    };
    Fields {
        stamp: shown(stamp),
        level,
        component: column(columns.page)
            .or(column(columns.action))
            .unwrap_or_default(),
        thread: column(columns.process).unwrap_or_default(),
        source: column(columns.client).unwrap_or_default(),
        message,
    }
}

/// A site server row has no level, so its words decide, as in CMTrace.
fn server_fields(text: &[u8]) -> Option<Fields<'_>> {
    let server = server_parts(text)?;
    Some(Fields {
        stamp: time::parse_sccm(server.date, server.time),
        component: server.component,
        thread: server.thread,
        ..Fields::message_only(server.message, level::detect(server.message))
    })
}

fn plain_fields<'a>(line: &'a [u8], context: &ParseContext) -> Fields<'a> {
    let text = trim_spaces(line);
    let Some(first) = text.first() else {
        return Fields::message_only(text, Level::Info);
    };
    if let Some(fields) = server_fields(text) {
        return fields;
    }
    let hinted = match context.hint {
        Hint::None => None,
        Hint::Msi { started } => msi_fields(text, started),
        Hint::Transcript => transcript_fields(text),
        Hint::SetupApi => Some(setupapi_fields(text)),
        Hint::W3c(columns) => Some(w3c_fields(text, columns)),
    };
    if let Some(fields) = hinted {
        return fields;
    }
    let parsed = if first.is_ascii_digit() {
        dated_fields(text)
            .or_else(|| netlogon_fields(text, context))
            .or_else(|| uls_fields(text))
    } else if *first == b'{' {
        reporting_events_fields(text)
    } else if *first == b'[' {
        bracketed_fields(text)
    } else if first.is_ascii_alphabetic() {
        group_policy_fields(text)
            .or_else(|| syslog_fields(text, context))
            .or_else(|| logfmt_fields(text))
    } else {
        None
    };
    parsed.unwrap_or_else(|| {
        let (level, message, component) = level_and_component(text);
        Fields {
            component,
            ..Fields::message_only(message, level)
        }
    })
}

/// A process name, any level tag, and the message after them.
type ProcessPrefix<'a> = (&'a [u8], Option<&'a [u8]>, &'a [u8]);

/// Splits `name[pid]<tag>:` from the front of `text`, where `name` runs to the
/// first `:` or `[`. Returns the name, any tag, and what follows the colon.
fn process_prefix(text: &[u8]) -> Option<ProcessPrefix<'_>> {
    let name_end = text
        .iter()
        .position(|byte| matches!(byte, b':' | b'[' | b'<'))?;
    let name = trim_spaces(&text[..name_end]);
    let mut rest = &text[name_end..];
    if rest.first() == Some(&b'[') {
        let close = memchr::memchr(b']', rest)?;
        let pid = &rest[1..close];
        if pid.is_empty() || !pid.iter().all(u8::is_ascii_digit) {
            return None;
        }
        rest = &rest[close + 1..];
    }
    let mut tag = None;
    let after_spaces = skip_spaces(rest);
    if after_spaces.first() == Some(&b'<') {
        let close = memchr::memchr(b'>', after_spaces)?;
        tag = Some(&after_spaces[1..close]);
        rest = &after_spaces[close + 1..];
    }
    Some((name, tag, skip_spaces(rest.strip_prefix(b":")?)))
}

/// `Apr 23 14:25:00 host process[pid] <Level>: message`
fn syslog_fields<'a>(text: &'a [u8], context: &ParseContext) -> Option<Fields<'a>> {
    let (stamp, end) = time::parse_syslog(text, context.year_hint)?;
    let (_host, rest) = split_word(skip_spaces(&text[end..]))?;
    let (name, tag, raw_message) = process_prefix(rest)?;
    if name.is_empty() || name.iter().any(|byte| is_space(*byte)) {
        return None;
    }
    let (level, message) = match tag.and_then(Level::from_name) {
        Some(level) => (level, raw_message),
        None => {
            let (level, message, _) = level_and_component(raw_message);
            (level, message)
        }
    };
    Some(Fields {
        stamp: Some(stamp),
        component: name,
        ..Fields::message_only(message, level)
    })
}

/// Group Policy's debug logs, such as `gpsvc.log`:
/// `GPSVC(188c.25d8) 2026-01-30 11:13:17:216 ProcessGPOs(Machine): message`.
/// The name before the brackets is the component, and the process and thread
/// IDs inside them the thread. Windows 11 added the date in late 2025. Older
/// rows give only a time of day, which can't be placed on a timeline, so it's
/// left at the start of their message.
fn group_policy_fields(text: &[u8]) -> Option<Fields<'_>> {
    let open = memchr::memchr(b'(', &text[..text.len().min(17)])?;
    let name = &text[..open];
    if name.is_empty() || !name.iter().all(u8::is_ascii_alphabetic) {
        return None;
    }
    let close = open + memchr::memchr(b')', &text[open..])?;
    let ids = &text[open + 1..close];
    let (process, thread) = ids.split_at(memchr::memchr(b'.', ids)?);
    let is_id = |id: &[u8]| (1..=8).contains(&id.len()) && id.iter().all(u8::is_ascii_hexdigit);
    if !is_id(process) || !is_id(&thread[1..]) {
        return None;
    }
    let rest = text[close + 1..].strip_prefix(b" ")?;
    let (first, after) = split_word(rest).unwrap_or((rest, &b""[..]));
    let (stamp, message) = if first.contains(&b'-') {
        let (time, message) = split_word(after).unwrap_or((after, &b""[..]));
        (Some(time::parse_group_policy(first, time)?), message)
    } else {
        time::parse_time_of_day(first)?;
        (None, rest)
    };
    let message = trim_spaces(message);
    // The whole log is verbose output, so rows such as `Verbose output to
    // eventlog requested` aren't debug.
    let level = match level::detect(message) {
        Level::Debug => Level::Info,
        found => found,
    };
    Some(Fields {
        stamp,
        component: name,
        thread: ids,
        ..Fields::message_only(message, level)
    })
}

/// Netlogon's debug log, `netlogon.log`:
/// `01/31 13:56:53 [LOGON] [2632] CONTOSO: SamLogon: ... Returns 0xC000006A`.
/// The category is the component, and the number some versions write after
/// it the thread. `[CRITICAL]` rows are errors, and a request that returns an
/// NTSTATUS error, `0xC…`, such as a failed logon, is a warning.
fn netlogon_fields<'a>(text: &'a [u8], context: &ParseContext) -> Option<Fields<'a>> {
    let (date, rest) = split_word(text)?;
    let (time, rest) = split_word(rest).unwrap_or((rest, &b""[..]));
    let stamp = time::parse_netlogon(date, time, context.year_hint)?;
    let bracketed = |text: &'a [u8], wanted: fn(&u8) -> bool| -> Option<(&'a [u8], &'a [u8])> {
        let inside = text.strip_prefix(b"[")?;
        let close = memchr::memchr(b']', &inside[..inside.len().min(24)])?;
        let word = &inside[..close];
        (!word.is_empty() && word.iter().all(wanted))
            .then(|| (word, skip_spaces(&inside[close + 1..])))
    };
    let is_category = |byte: &u8| byte.is_ascii_uppercase() || *byte == b'_';
    let (component, rest) = bracketed(rest, is_category).unwrap_or((&b""[..], rest));
    let (thread, message) = bracketed(rest, u8::is_ascii_digit).unwrap_or((&b""[..], rest));
    let message = trim_spaces(message);
    let level = if component == b"CRITICAL" {
        Level::Error
    } else if memmem::find(message, b"Returns 0xC").is_some() {
        Level::Warning
    } else {
        level::detect(message)
    };
    Some(Fields {
        stamp: Some(stamp),
        component,
        thread,
        ..Fields::message_only(message, level)
    })
}

/// The event GUID and time that start a row of Windows Update's
/// `ReportingEvents.log`, and the tab-separated fields after them.
fn reporting_events_start(text: &[u8]) -> Option<(Stamp, Option<i32>, &[u8])> {
    let tab = memchr::memchr(b'\t', text)?;
    let guid = &text[..tab];
    if guid.len() != 38 || guid.first() != Some(&b'{') || guid.last() != Some(&b'}') {
        return None;
    }
    let rest = &text[tab + 1..];
    let time_end = memchr::memchr(b'\t', rest)?;
    let (stamp, zone) = time::parse_reporting_events(&rest[..time_end])?;
    Some((stamp, zone, &rest[time_end + 1..]))
}

/// Windows Update's `ReportingEvents.log`: tab-separated fields, from the
/// event's GUID and time, as in
/// `{GUID}  2026-11-02 15:37:25:598-0500  1  148 [AGENT_DETECTION_FAILED]  101
/// {update}  0  80240438  <<PROCESS>>: wa_3rd_party_host_32.exe  Failure
/// Software Synchronization  Windows Update Client failed to detect ...  {id}`.
/// The operation is the component, the program that asked the thread, the
/// event's name the source, and a Failure is an error. Every field can still
/// be searched, as searches read the whole row.
fn reporting_events_fields(text: &[u8]) -> Option<Fields<'_>> {
    let (stamp, _, rest) = reporting_events_start(text)?;
    let mut parts = rest.split(|byte| *byte == b'\t');
    let _revision = parts.next()?;
    let event = parts.next()?;
    let _category = parts.next()?;
    let _update = parts.next()?;
    let _update_revision = parts.next()?;
    let _code = parts.next()?;
    // Later fields can be empty, and the row's trailing tabs are trimmed off
    // before it gets here, so missing ones are empty.
    let mut next = || parts.next().unwrap_or(b"");
    let caller = next();
    let result = trim_spaces(next());
    let operation = next();
    let message = trim_spaces(next());
    // Some events have no text, so their event, such as
    // `170 [AGENT_DOWNLOAD_PROGRESS]`, stands in.
    let message = if message.is_empty() {
        trim_spaces(event)
    } else {
        message
    };
    // `148 [AGENT_DETECTION_FAILED]` is named by its brackets.
    let name = memchr::memchr(b'[', event)
        .and_then(|open| {
            let inside = &event[open + 1..];
            memchr::memchr(b']', inside).map(|close| &inside[..close])
        })
        .unwrap_or(event);
    let caller = caller
        .strip_prefix(b"<<PROCESS>>:")
        .map_or(caller, skip_spaces);
    let level = if result.eq_ignore_ascii_case(b"failure") {
        Level::Error
    } else {
        Level::Info
    };
    Some(Fields {
        stamp: Some(stamp),
        level,
        component: trim_spaces(operation),
        thread: trim_spaces(caller),
        source: trim_spaces(name),
        message,
    })
}

/// Rows that begin with `YYYY-MM-DD`.
fn dated_fields(text: &[u8]) -> Option<Fields<'_>> {
    let parsed = time::parse_iso(text)?;
    let after = &text[parsed.end..];

    // CBS and DISM: `2026-04-23 14:25:00, Info   CBS    message`
    if let Some(rest) = after.strip_prefix(b",") {
        return Some(windows_servicing_fields(parsed.stamp, skip_spaces(rest)));
    }
    if !after.first().copied().is_some_and(is_space) {
        return None;
    }
    let rest = skip_spaces(after);

    // `Get-WindowsUpdateLog` writes its dates with slashes.
    if text.get(4) == Some(&b'/') {
        if let Some(fields) = windows_update_fields(parsed.stamp, rest) {
            return Some(fields);
        }
    }
    if let Some(fields) = tagged_fields(parsed.stamp, rest) {
        return Some(fields);
    }
    if parsed.space_separated {
        if let Some(fields) = dated_syslog_fields(parsed.stamp, rest) {
            return Some(fields);
        }
    }
    if let Some(fields) = dated_component_fields(parsed.stamp, rest) {
        return Some(fields);
    }
    let (level, message, component) = level_and_component(rest);
    Some(Fields {
        stamp: Some(parsed.stamp),
        component,
        ..Fields::message_only(message, level)
    })
}

/// The `WindowsUpdate.log` that `Get-WindowsUpdateLog` writes:
/// `2026/11/11 14:24:21.1458910 47996 51540 Misc   message`, with the
/// process and thread IDs and the component after the time. The thread ID is
/// the thread, and the process ID the source.
fn windows_update_fields(stamp: Stamp, rest: &[u8]) -> Option<Fields<'_>> {
    let is_number = |word: &[u8]| word.iter().all(u8::is_ascii_digit);
    let (process, rest) = split_word(rest)?;
    let (thread, rest) = split_word(rest)?;
    let (component, message) = split_word(rest)?;
    if !is_number(process)
        || !is_number(thread)
        || !component.first().is_some_and(u8::is_ascii_alphabetic)
    {
        return None;
    }
    let message = trim_spaces(message);
    Some(Fields {
        stamp: Some(stamp),
        component,
        thread,
        source: process,
        ..Fields::message_only(message, level::detect(message))
    })
}

/// winget's `2026-11-11 15:40:09.381 <I> [CORE] message`: a one-letter
/// level in angle brackets, then the component in square ones.
fn tagged_fields(stamp: Stamp, rest: &[u8]) -> Option<Fields<'_>> {
    let (tag, after) = split_word(rest)?;
    let level = match tag {
        b"<V>" => Level::Debug,
        b"<I>" => Level::Info,
        b"<W>" => Level::Warning,
        b"<E>" => Level::Error,
        b"<C>" => Level::Critical,
        _ => return None,
    };
    let (component, message) = match after.strip_prefix(b"[") {
        Some(inside) => match memchr::memchr(b']', &inside[..inside.len().min(32)]) {
            Some(close) => (&inside[..close], skip_spaces(&inside[close + 1..])),
            None => (&b""[..], after),
        },
        None => (&b""[..], after),
    };
    Some(Fields {
        stamp: Some(stamp),
        component,
        ..Fields::message_only(message, level)
    })
}

/// CBS writes real failures at `Info`, as in `Failed to open package. [HRESULT =
/// 0x800f0805]`, so a stated level there can be raised by what the message says.
fn servicing_problem_level(message: &[u8]) -> Level {
    if memmem::find(message, b"HRESULT = 0x8").is_some() {
        return Level::Error;
    }
    match level::detect(message) {
        found @ (Level::Warning | Level::Error) => found,
        _ => Level::Debug,
    }
}

fn windows_servicing_fields(stamp: Stamp, rest: &[u8]) -> Fields<'_> {
    if let Some((level_word, after_level)) = split_word(rest) {
        let known = if level_word.eq_ignore_ascii_case(b"perf") {
            Some(Level::Debug)
        } else {
            Level::from_name(level_word)
        };
        if let (Some(stated), Some((component, message))) = (known, split_word(after_level)) {
            let message = trim_spaces(message);
            return Fields {
                stamp: Some(stamp),
                component,
                ..Fields::message_only(message, stated.max(servicing_problem_level(message)))
            };
        }
    }
    let (level, message, component) = level_and_component(rest);
    Fields {
        stamp: Some(stamp),
        component,
        ..Fields::message_only(message, level)
    }
}

/// `host process[pid]: message` after a space-separated timestamp.
fn dated_syslog_fields(stamp: Stamp, rest: &[u8]) -> Option<Fields<'_>> {
    let (host, after_host) = split_word(rest)?;
    let host_is_valid = host.first().is_some_and(u8::is_ascii_alphanumeric)
        && host
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    // A level in the host position means this isn't syslog at all, so
    // `2026-01-01 10:00:00 INFO Starting: x` keeps its level.
    if !host_is_valid || Level::from_name(host).is_some() {
        return None;
    }
    let (name, _tag, raw_message) = process_prefix(after_host)?;
    if name.is_empty() {
        return None;
    }
    let (level, message, _) = level_and_component(raw_message);
    Some(Fields {
        stamp: Some(stamp),
        component: name,
        ..Fields::message_only(message, level)
    })
}

/// `component[pid]: message` directly after a timestamp.
fn dated_component_fields(stamp: Stamp, rest: &[u8]) -> Option<Fields<'_>> {
    let name_end = rest
        .iter()
        .position(|byte| matches!(byte, b':' | b'[') || is_space(*byte))?;
    if name_end == 0 || Level::from_name(&rest[..name_end]).is_some() {
        return None;
    }
    let (name, _tag, raw_message) = process_prefix(rest)?;
    if name.len() != name_end {
        return None;
    }
    let (level, message, _) = level_and_component(raw_message);
    Some(Fields {
        stamp: Some(stamp),
        component: name,
        ..Fields::message_only(message, level)
    })
}

/// The ULS format of Office's Click-to-Run logs, such as
/// `%TEMP%\<PC>-20261111-1324.log`, and SharePoint's:
/// `11/11/2026 13:24:09.703⇥OFFICEC2 (0x8cf8)⇥0x6a00⇥⇥Activity⇥bjtcw⇥Medium⇥msg`.
/// Its tab-separated fields are the time, process, thread, area, category,
/// event ID, level and message. The category is the component, and the
/// process the source.
fn uls_fields(text: &[u8]) -> Option<Fields<'_>> {
    let mut parts = text.split(|byte| *byte == b'\t');
    let time = parts.next()?;
    let process = parts.next()?;
    let thread = parts.next()?;
    let _area = parts.next()?;
    let category = parts.next()?;
    let _event = parts.next()?;
    let level = match trim_spaces(parts.next()?) {
        b"Critical" => Level::Critical,
        b"Unexpected" => Level::Error,
        // Above Medium, but in Click-to-Run logs mostly notes, such as
        // `Deactivated event detected.`
        b"Monitorable" | b"High" => Level::Notice,
        b"Medium" => Level::Info,
        b"Verbose" | b"VerboseEx" => Level::Debug,
        _ => return None,
    };
    Some(Fields {
        stamp: time::parse_month_first(time),
        level,
        component: trim_spaces(category),
        thread: trim_spaces(thread),
        source: trim_spaces(process),
        message: trim_spaces(parts.next().unwrap_or_default()),
    })
}

/// True for a process and thread pair such as `0A1C:0B2C`.
fn is_hex_pair(text: &[u8]) -> bool {
    let Some(colon) = memchr::memchr(b':', text) else {
        return false;
    };
    let hex = |part: &[u8]| !part.is_empty() && part.iter().all(u8::is_ascii_hexdigit);
    hex(&text[..colon]) && hex(&text[colon + 1..])
}

/// WiX Burn's bootstrapper logs, which the Visual C++ and .NET installers and
/// the Visual Studio installer write: `[0A1C:0B2C][2026-11-18T12:31:32]i001:
/// msg`. The process and thread are the thread, and the letter of Burn's
/// message code says the level: `i` information, `w` a warning and `e` an
/// error. Without one, the words decide.
fn burn_fields<'a>(inside: &'a [u8], after: &'a [u8]) -> Option<Fields<'a>> {
    let rest = after.strip_prefix(b"[")?;
    let close = memchr::memchr(b']', rest)?;
    let parsed = time::parse_iso(&rest[..close]).filter(|parsed| parsed.end == close)?;
    let message = skip_spaces(&rest[close + 1..]);
    let coded = message.get(4) == Some(&b':') && message[1..4].iter().all(u8::is_ascii_digit);
    let level = match message.first() {
        Some(b'i') if coded => Level::Info,
        Some(b'w') if coded => Level::Warning,
        Some(b'e') if coded => Level::Error,
        _ => level::detect(message),
    };
    let (thread, component) = if is_hex_pair(inside) {
        (inside, &b""[..])
    } else {
        (&b""[..], inside)
    };
    Some(Fields {
        stamp: Some(parsed.stamp),
        thread,
        component,
        ..Fields::message_only(message, level)
    })
}

/// Rows that begin with `[`: `[timestamp tag] [tag] message`,
/// `[prefix] timestamp message`, `[prefix][timestamp]message`, or
/// `[LEVEL] message`.
fn bracketed_fields(text: &[u8]) -> Option<Fields<'_>> {
    let close = memchr::memchr(b']', text)?;
    let inside = &text[1..close];
    let after = skip_spaces(&text[close + 1..]);
    if let Some(fields) = burn_fields(inside, after) {
        return Some(fields);
    }

    if let Some(parsed) = time::parse_iso(inside) {
        let inner_tag = trim_spaces(&inside[parsed.end..]);
        let (separate_tag, message) = if inner_tag.is_empty() && after.first() == Some(&b'[') {
            let tag_close = memchr::memchr(b']', after)?;
            (
                trim_spaces(&after[1..tag_close]),
                skip_spaces(&after[tag_close + 1..]),
            )
        } else {
            (&b""[..], after)
        };
        let tag = if inner_tag.is_empty() {
            separate_tag
        } else {
            inner_tag
        };
        let (stated, component) = interpret_tag(tag);
        return Some(Fields {
            stamp: Some(parsed.stamp),
            component,
            ..Fields::message_only(message, stated.unwrap_or_else(|| level::detect(message)))
        });
    }

    if let Some(parsed) = time::parse_iso(after) {
        let (level, message, _) = level_and_component(skip_spaces(&after[parsed.end..]));
        return Some(Fields {
            stamp: Some(parsed.stamp),
            component: inside,
            ..Fields::message_only(message, level)
        });
    }
    None
}

/// A tag may be a level, a level and component, or just a component.
fn interpret_tag(tag: &[u8]) -> (Option<Level>, &[u8]) {
    if tag.is_empty() {
        return (None, b"");
    }
    let (first, rest) = match tag.iter().position(|byte| is_space(*byte)) {
        Some(end) => (&tag[..end], skip_spaces(&tag[end..])),
        None => (tag, &b""[..]),
    };
    match Level::from_name(first) {
        Some(level) => (Some(level), rest),
        None => (None, tag),
    }
}

/// `time="..." level=info msg="..."`. Escapes in the message are left as written.
fn logfmt_fields(text: &[u8]) -> Option<Fields<'_>> {
    let after_time = text.strip_prefix(b"time=\"")?;
    let time_end = memchr::memchr(b'"', after_time)?;
    let stamp = time::parse_iso(&after_time[..time_end]).map(|parsed| parsed.stamp);
    let rest = skip_spaces(&after_time[time_end + 1..]);
    let (level_word, after_level) = split_word(rest.strip_prefix(b"level=")?)?;
    let message = after_level.strip_prefix(b"msg=")?;
    let message = match message.strip_prefix(b"\"") {
        Some(quoted) => quoted.strip_suffix(b"\"").unwrap_or(quoted),
        None => message,
    };
    Some(Fields {
        stamp,
        ..Fields::message_only(message, Level::from_name(level_word).unwrap_or(Level::Info))
    })
}

/// Reads a stated level from the front of a message: `[ERROR]`, `WARN:`,
/// `INFO component: text`. Falls back to keywords when there isn't one.
///
/// The level must be a whole word, so `Information loaded` stays a message.
/// A mixed-case word with no delimiter is treated as part of the sentence, so
/// `Error connecting to x` keeps all its words.
fn level_and_component(text: &[u8]) -> (Level, &[u8], &[u8]) {
    let keyword_result = |text| (level::detect(text), text, &b""[..]);
    let bracketed = text.first() == Some(&b'[');
    let body = if bracketed { &text[1..] } else { text };
    let word_end = body
        .iter()
        .position(|byte| !byte.is_ascii_alphabetic())
        .unwrap_or(body.len());
    let word = &body[..word_end];
    let Some(level) = Level::from_name(word) else {
        return keyword_result(text);
    };
    let mut rest = &body[word_end..];
    let closed = bracketed && rest.first() == Some(&b']');
    if closed {
        rest = &rest[1..];
    } else if bracketed {
        return keyword_result(text);
    }
    rest = skip_spaces(rest);
    let colon = rest.first() == Some(&b':');
    if colon {
        rest = skip_spaces(&rest[1..]);
    }
    let separated = closed || colon || word.iter().all(u8::is_ascii_uppercase);
    let word_boundary = body
        .get(word_end)
        .is_none_or(|byte| !byte.is_ascii_alphanumeric());
    if !word_boundary || !separated {
        return (level::detect(text), text, b"");
    }
    let (component, message) = component_prefix(rest);
    (level, message, component)
}

/// Splits `name: ` from the front of a message. Requiring a space after the
/// colon leaves URLs alone, and requiring a letter first leaves paths alone.
fn component_prefix(text: &[u8]) -> (&[u8], &[u8]) {
    let name_is_valid = |name: &[u8]| {
        !name.is_empty()
            && name.len() <= 31
            && name[0].is_ascii_alphabetic()
            && name
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    };
    if let Some(colon) = memchr::memchr(b':', &text[..text.len().min(32)]) {
        let name = &text[..colon];
        let after = &text[colon + 1..];
        if name_is_valid(name) && after.first().copied().is_some_and(is_space) {
            return (name, skip_spaces(after));
        }
    }
    (b"", text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTEXT: ParseContext = ParseContext {
        year_hint: YearHint {
            year: 2026,
            month: 6,
        },
        custom: None,
        hint: Hint::None,
    };

    fn plain(row: &str) -> Fields<'_> {
        fields(Format::Plain, row.as_bytes(), &CONTEXT)
    }

    fn stamp_text(parsed: &Fields<'_>) -> String {
        parsed
            .stamp
            .map_or(String::new(), |stamp| stamp.to_string())
    }

    #[test]
    fn splits_sccm_fields() {
        let row = b"<![LOG[Failed ]LOG]!> text]LOG]!><time=\"10:23:02.334+000\" date=\"04-06-2026\" component=\"PolicyAgent\" context=\"\" type=\"3\" thread=\"5240\" file=\"a.cpp\">\r\n";
        let parsed = fields(Format::Sccm, row, &CONTEXT);
        assert_eq!(stamp_text(&parsed), "2026-04-06T10:23:02.334");
        assert_eq!(parsed.level, Level::Error);
        assert_eq!(parsed.component, b"PolicyAgent");
        assert_eq!(parsed.thread, b"5240");
        assert_eq!(parsed.source, b"a.cpp");
        assert_eq!(parsed.message, b"Failed ]LOG]!> text");
    }

    #[test]
    fn splits_windows_update_reporting_events() {
        let failed = "{485A0372-C771-4077-8F8D-3618D2F69926}\t2026-11-02 15:37:16:808-0500\t1\t148 [AGENT_DETECTION_FAILED]\t101\t{8B24B027-1DEE-BABB-9A95-3517DFB9C552}\t0\t80240438\t<<PROCESS>>: wa_3rd_party_host_32.exe\tFailure\tSoftware Synchronization\tWindows Update Client failed to detect with error 0x80240438.\tINxXTpgwaU+ocK1f.1.2.0.3.0\r\n";
        let parsed = plain(failed);
        assert_eq!(stamp_text(&parsed), "2026-11-02T15:37:16.808");
        assert_eq!(parsed.level, Level::Error);
        assert_eq!(parsed.component, b"Software Synchronization");
        assert_eq!(parsed.thread, b"wa_3rd_party_host_32.exe");
        assert_eq!(parsed.source, b"AGENT_DETECTION_FAILED");
        assert_eq!(
            parsed.message,
            b"Windows Update Client failed to detect with error 0x80240438."
        );
        assert_eq!(utc_offset(Format::Plain, failed.as_bytes()), Some(-300));

        let found = plain("{14F2D8C9-B388-4400-9692-5CE782CD2491}\t2026-11-04 20:21:45:274+0130\t1\t147 [AGENT_DETECTION_FINISHED]\t101\t{855E8A7C-ECB4-4CA3-B045-1DFA50104289}\t0\t0\tMoUpdateOrchestrator\tSuccess\tSoftware Synchronization\tWindows Update Client successfully detected 0 updates.\tZ3IZV.4.3");
        assert_eq!(found.level, Level::Info);
        assert_eq!(found.thread, b"MoUpdateOrchestrator");

        // Trailing empty fields are trimmed off with the row's spaces, and an
        // event with no text is named by its event instead.
        let empty = "{FEFE1FD8-B102-4C41-A1CE-1EA570F91541}\t2026-11-05 11:44:13:175-0600\t1\t170 [AGENT_DOWNLOAD_PROGRESS]\t101\t{EBC09DA3-2221-48F2-95B8-6F88183A60D1}\t1\t0\t(null)\tUnknown\t\t\t\r\n";
        let parsed = plain(empty);
        assert_eq!(stamp_text(&parsed), "2026-11-05T11:44:13.175");
        assert_eq!(parsed.level, Level::Info);
        assert_eq!(parsed.component, b"");
        assert_eq!(parsed.message, b"170 [AGENT_DOWNLOAD_PROGRESS]");
        assert_eq!(utc_offset(Format::Plain, empty.as_bytes()), Some(-360));

        // Anything else starting with a brace reads as it did.
        for other in [
            "{not a guid}\t2026-11-05 11:44:13:175-0600\t1\t2\t3\t4\t5\t6",
            "{FEFE1FD8-B102-4C41-A1CE-1EA570F91541}\t2026-11-05 11:44:13.175\t1\t2\t3\t4\t5\t6",
            "{FEFE1FD8-B102-4C41-A1CE-1EA570F91541}\t2026-11-05 11:44:13:175-0600\t1\t2",
            "{\"json\": true}",
        ] {
            let parsed = plain(other);
            assert_eq!(parsed.stamp, None, "{other}");
            assert_eq!(parsed.message, other.as_bytes(), "{other}");
        }
    }

    #[test]
    fn splits_configmgr_site_server_rows() {
        let row = "SMS_EXECUTIVE started as thread ID 5112 (0x13F8).~  $$<SMS_EXECUTIVE><11-18-2026 09:30:12.123+300><thread=5112 (0x13F8)>\r\n";
        let parsed = plain(row);
        assert_eq!(stamp_text(&parsed), "2026-11-18T09:30:12.123");
        assert_eq!(parsed.component, b"SMS_EXECUTIVE");
        assert_eq!(parsed.thread, b"5112");
        assert_eq!(
            parsed.message,
            b"SMS_EXECUTIVE started as thread ID 5112 (0x13F8).~"
        );
        assert_eq!(parsed.level, Level::Info);
        assert_eq!(utc_offset(Format::Plain, row.as_bytes()), Some(-300));
        assert_eq!(
            searchable(Format::Plain, row.as_bytes()),
            b"SMS_EXECUTIVE started as thread ID 5112 (0x13F8).~"
        );

        // With no level, the words decide, as in CMTrace.
        let failed = plain("Failed to connect to the SQL Server.  $$<SMS_DATABASE_NOTIFICATION_MONITOR><11-18-2026 09:31:00.000-060><thread=4532 (0x11B4)>");
        assert_eq!(failed.level, Level::Error);
        assert_eq!(failed.component, b"SMS_DATABASE_NOTIFICATION_MONITOR");

        // Anything else ending the same way reads as it did.
        for other in [
            "nothing to see (here)>",
            "msg $$<Comp><11-18-2026 09:30:12.123+300><thread=x (0x1)>",
            "msg $$<Comp><11-18-2026 09:30:12.123+300>",
        ] {
            let parsed = plain(other);
            assert_eq!(parsed.stamp, None, "{other}");
            assert_eq!(parsed.message, other.as_bytes(), "{other}");
            assert_eq!(
                searchable(Format::Plain, other.as_bytes()),
                other.as_bytes()
            );
        }
    }

    fn hinted(hint: Hint, row: &str) -> Fields<'_> {
        let context = ParseContext { hint, ..CONTEXT };
        fields(Format::Plain, row.as_bytes(), &context)
    }

    #[test]
    fn windows_installer_rows_take_the_day_logging_started() {
        let head = "=== Verbose logging started: 11/18/2026  15:42:16  Build type: SHIP UNICODE 5.00.10011.00  Calling process: C:\\Windows\\system32\\msiexec.exe ===\r\nMSI (c) (50:E0) [15:42:16:698]: Resetting cached policy values\r\n";
        let hint = hint_for(head.as_bytes());
        let Hint::Msi { started } = hint else {
            panic!("not an installer log: {hint:?}");
        };
        assert_eq!(started.to_string(), "2026-11-18T15:42:16.000");
        assert_eq!(
            hinted(hint, head.lines().next().unwrap()).level,
            Level::Info
        );
        assert_eq!(
            hinted(
                hint,
                "MSI (c) (50:E0) [15:42:16:698]: Machine policy value 'Debug' is 0"
            )
            .level,
            Level::Info
        );

        let row = hinted(
            hint,
            "MSI (c) (50:E0) [15:42:16:698]: Resetting cached policy values\r\n",
        );
        assert_eq!(stamp_text(&row), "2026-11-18T15:42:16.698");
        assert_eq!(row.component, b"MSI (c)");
        assert_eq!(row.thread, b"50:E0");
        assert_eq!(row.message, b"Resetting cached policy values");
        assert_eq!(row.level, Level::Info);

        let failed = hinted(
            hint,
            "Action ended 15:42:21: InstallFinalize. Return value 3.",
        );
        assert_eq!(stamp_text(&failed), "2026-11-18T15:42:21.000");
        assert_eq!(failed.level, Level::Error);
        assert_eq!(
            failed.message,
            b"Action ended 15:42:21: InstallFinalize. Return value 3."
        );
        assert_eq!(
            hinted(hint, "Action ended 15:42:22: Foo. Return value 2.").level,
            Level::Warning
        );
        // A time before logging started is from after midnight.
        let late = hinted(hint, "Action start 00:05:00: InstallValidate.");
        assert_eq!(stamp_text(&late), "2026-11-19T00:05:00.000");
        let custom = hinted(
            hint,
            "MSI (s) (A4!B8) [15:42:20:123]: Product: Foo -- Installation failed.",
        );
        assert_eq!(custom.level, Level::Error);
        assert_eq!(custom.thread, b"A4!B8");
        let stopped = hinted(hint, "=== Logging stopped: 11/18/2026  15:42:30 ===");
        assert_eq!(stamp_text(&stopped), "2026-11-18T15:42:30.000");

        let property = hinted(hint, "Property(S): ProductName = Foo");
        assert_eq!(property.stamp, None);
        assert_eq!(property.message, b"Property(S): ProductName = Foo");

        // Dates in a day-first order are read too.
        for head in [
            "=== Logging started: 18.11.2026  15:42:16 ===",
            "=== Logging started: 18/11/2026  15:42:16 ===",
        ] {
            assert_eq!(hint_for(head.as_bytes()), hint, "{head}");
        }
        // Without its first line, a row reads as any other.
        assert_eq!(
            plain("MSI (c) (50:E0) [15:42:16:698]: Resetting").stamp,
            None
        );
    }

    #[test]
    fn transcript_error_records_are_errors() {
        let windows = "\u{feff}**********************\r\nWindows PowerShell transcript start\r\nStart time: 20261118154047\r\n";
        let core =
            "**********************\nPowerShell transcript start\nStart time: 20261118154049\n";
        assert_eq!(hint_for(windows.as_bytes()), Hint::Transcript);
        assert_eq!(hint_for(core.as_bytes()), Hint::Transcript);
        let row = |text| hinted(Hint::Transcript, text);

        let started = row("Start time: 20261118154047");
        assert_eq!(stamp_text(&started), "2026-11-18T15:40:47.000");
        assert_eq!(
            stamp_text(&row("Command start time: 20261118154050")),
            "2026-11-18T15:40:50.000"
        );
        for error in [
            "Get-Item : Cannot find path 'C:\\does-not-exist' because it does not exist.",
            "Get-Item: C:\\Users\\me\\make.ps1:6",
            "    + CategoryInfo          : ObjectNotFound: (C:\\does-not-exist:String) [Get-Item], ItemNotFoundException",
            "    + FullyQualifiedErrorId : PathNotFound,Microsoft.PowerShell.Commands.GetItemCommand",
            "PS>TerminatingError(Get-Item): \"Cannot find path\"",
            "ConvertFrom-Json : Invalid JSON primitive: x.",
        ] {
            assert_eq!(row(error).level, Level::Error, "{error}");
        }
        for other in [
            "Username: CONTOSO\\admin",
            "Host Application: C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
            "Last-Modified : Tue, 18 Nov 2026",
            "Get-thing : lowercase nouns aren't cmdlets",
            "PS C:\\> Get-Item C:\\does-not-exist",
            "done",
        ] {
            assert_eq!(row(other).level, Level::Info, "{other}");
        }
        assert_eq!(row("WARNING: careful now").level, Level::Warning);
        assert!(
            POWERSHELL_VERBS.windows(2).all(|pair| pair[0] < pair[1]),
            "the verbs are searched, so they stay in byte order"
        );
        // In any other log, those lines go by their words as before.
        assert_eq!(plain("Get-Item : Cannot find path").level, Level::Info);
    }

    #[test]
    fn setupapi_markers_set_the_level() {
        let head = "[Device Install Log]\r\n     OS Version = 10.0.26300\r\n";
        assert_eq!(hint_for(head.as_bytes()), Hint::SetupApi);
        let row = |text| hinted(Hint::SetupApi, text);

        let start = row(">>>  Section start 2026/10/14 09:59:16.183");
        assert_eq!(stamp_text(&start), "2026-10-14T09:59:16.183");
        assert_eq!(start.level, Level::Info);
        assert_eq!(
            stamp_text(&row("<<<  Section end 2026/10/14 09:59:16.734")),
            "2026-10-14T09:59:16.734"
        );
        assert_eq!(
            row("<<<  [Exit status: FAILURE(0xe0000219)]").level,
            Level::Error
        );
        assert_eq!(row("<<<  [Exit status: SUCCESS]").level, Level::Info);

        let error = row("!!!  set: Failed to remove INF oem33.inf, Err = 0x3");
        assert_eq!(error.level, Level::Error);
        assert_eq!(error.component, b"set");
        assert_eq!(error.message, b"Failed to remove INF oem33.inf, Err = 0x3");
        let warning = row("!    dvi: Query-and-Remove failed: 0x05: CR_INVALID_DEVNODE.");
        assert_eq!(warning.level, Level::Warning);
        assert_eq!(warning.component, b"dvi");
        // Words don't decide, so this setting isn't an error.
        let setting = row("     inf:                          Error Control = 1");
        assert_eq!(setting.level, Level::Info);
        assert_eq!(setting.component, b"inf");
        assert_eq!(setting.message, b"Error Control = 1");
        assert_eq!(row("     OS Version = 10.0.26300").component, b"");

        assert_eq!(hint_for(b"2026-01-01 10:00:00 plain text\n"), Hint::None);
        assert_eq!(hint_for(b""), Hint::None);
    }

    const IIS_HEAD: &str = "#Software: Microsoft Internet Information Services 10.0\r\n#Version: 1.0\r\n#Date: 2026-11-18 00:00:01\r\n#Fields: date time s-ip cs-method cs-uri-stem cs-uri-query s-port cs-username c-ip cs(User-Agent) cs(Referer) sc-status sc-substatus sc-win32-status time-taken\r\n";
    const FIREWALL_HEAD: &str = "#Version: 1.5\r\n#Software: Microsoft Windows Firewall\r\n#Time Format: Local\r\n#Fields: date time action protocol src-ip dst-ip src-port dst-port size tcpflags tcpsyn tcpack tcpwin icmptype icmpcode info path pid\r\n";

    #[test]
    fn iis_rows_are_read_by_their_fields_line() {
        let hint = hint_for(IIS_HEAD.as_bytes());
        let Hint::W3c(columns) = hint else {
            panic!("not a W3C log: {hint:?}");
        };
        assert!(columns.utc);
        let row = |status: &str| {
            let text = format!("2026-11-18 14:05:09 10.0.0.5 GET /SMS_MP/.sms_aut MPLIST 80 - 10.0.0.9 SMS+CCM+5.0 - {status} 0 0 15\r\n");
            let context = ParseContext { hint, ..CONTEXT };
            let parsed = fields(Format::Plain, text.as_bytes(), &context);
            (
                parsed.stamp.map(|stamp| stamp.to_string()),
                parsed.level,
                display(parsed.component),
                display(parsed.source),
                display(parsed.message),
            )
        };
        let (stamp, level, component, source, message) = row("200");
        assert_eq!(stamp.as_deref(), Some("2026-11-18T14:05:09.000"));
        assert_eq!(level, Level::Info);
        assert_eq!(component, "/SMS_MP/.sms_aut");
        assert_eq!(source, "10.0.0.9");
        assert_eq!(
            message,
            "10.0.0.5 GET /SMS_MP/.sms_aut MPLIST 80 - 10.0.0.9 SMS+CCM+5.0 - 200 0 0 15"
        );
        assert_eq!(row("500").1, Level::Error);
        assert_eq!(row("503").1, Level::Error);
        assert_eq!(row("404").1, Level::Warning);
        assert_eq!(row("403").1, Level::Warning);
        // Every Windows-authenticated request is challenged first.
        assert_eq!(row("401").1, Level::Info);
        assert_eq!(row("304").1, Level::Info);

        let directive = hinted(hint, "#Date: 2026-11-18 00:00:01");
        assert_eq!(stamp_text(&directive), "2026-11-18T00:00:01.000");
        // A cut-off row, or a directive, reads without panicking or guessing.
        assert_eq!(hinted(hint, "2026-11-18").stamp, None);
        assert_eq!(hinted(hint, "#Fields: date time").level, Level::Info);
        assert_eq!(hinted(hint, "").level, Level::Info);
    }

    fn display(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// A W3C log's hint, shown six hours behind UTC, as in US Central time.
    fn in_central_time(head: &str) -> (Hint, W3cFields) {
        let Hint::W3c(columns) = hint_for(head.as_bytes()) else {
            panic!("not a W3C log");
        };
        let columns = W3cFields {
            zone: Zone::Fixed(-360),
            ..columns
        };
        (Hint::W3c(columns), columns)
    }

    #[test]
    fn iis_times_are_shown_in_the_logs_zone() {
        let (hint, columns) = in_central_time(IIS_HEAD);
        let row = "2026-11-18 04:05:09 10.0.0.5 GET /SMS_MP/.sms_aut MPLIST 80 - 10.0.0.9 SMS+CCM+5.0 - 200 0 0 15";
        // Early in the morning in UTC is the evening before.
        assert_eq!(stamp_text(&hinted(hint, row)), "2026-11-17T22:05:09.000");
        assert_eq!(w3c_offset(row.as_bytes(), columns), Some(-360));
        let directive = hinted(hint, "#Date: 2026-11-18 00:00:01");
        assert_eq!(stamp_text(&directive), "2026-11-17T18:00:01.000");
        assert_eq!(w3c_offset(b"#Fields: date time", columns), None);

        // Windows Firewall writes local times, which stay as written.
        let (hint, _) = in_central_time(FIREWALL_HEAD);
        let dropped = hinted(hint, "2026-11-18 09:30:12 DROP TCP 10.0.0.9 10.0.0.5 51234 445 52 S 123456 0 64240 - - - RECEIVE 4");
        assert_eq!(stamp_text(&dropped), "2026-11-18T09:30:12.000");

        // With no zone set, as in tests, UTC stays UTC.
        assert_eq!(Zone::Local.offset_at(Stamp::from_seconds(0)), 0);
    }

    #[test]
    fn firewall_drops_are_warnings() {
        let hint = hint_for(FIREWALL_HEAD.as_bytes());
        let Hint::W3c(columns) = hint else {
            panic!("not a W3C log: {hint:?}");
        };
        assert!(!columns.utc, "it says its times are local");
        let dropped = hinted(hint, "2026-11-18 09:30:12 DROP TCP 10.0.0.9 10.0.0.5 51234 445 52 S 123456 0 64240 - - - RECEIVE 4");
        assert_eq!(stamp_text(&dropped), "2026-11-18T09:30:12.000");
        assert_eq!(dropped.level, Level::Warning);
        assert_eq!(dropped.component, b"DROP");
        assert_eq!(dropped.source, b"10.0.0.9");
        assert_eq!(dropped.thread, b"4");
        let allowed = hinted(
            hint,
            "2026-11-18 09:30:13 ALLOW UDP 10.0.0.5 10.0.0.1 55000 53 0 - - - - - - - SEND 2888",
        );
        assert_eq!(allowed.level, Level::Info);
        assert_eq!(allowed.component, b"ALLOW");

        // Other logs starting with # aren't W3C logs.
        assert_eq!(
            hint_for(b"# a comment\n2026-01-01 10:00:00 x\n"),
            Hint::None
        );
        assert_eq!(
            hint_for(b"#Software: x\n#Version: 1.0\n"),
            Hint::None,
            "no #Fields"
        );
    }

    #[test]
    fn splits_office_uls_rows() {
        let row = "11/11/2026 13:24:09.703\tOFFICEC2 (0x8cf8)\t0x6a00\t\tActivity\tbjtcw\tMedium\tActivityEnded {\"Name\": \"Office.Telemetry\"}\t\r\n";
        let parsed = plain(row);
        assert_eq!(stamp_text(&parsed), "2026-11-11T13:24:09.703");
        assert_eq!(parsed.level, Level::Info);
        assert_eq!(parsed.component, b"Activity");
        assert_eq!(parsed.thread, b"0x6a00");
        assert_eq!(parsed.source, b"OFFICEC2 (0x8cf8)");
        assert_eq!(
            parsed.message,
            b"ActivityEnded {\"Name\": \"Office.Telemetry\"}"
        );
        let level = |word: &str| {
            let row = format!("11/18/2026 09:30:12.34\tw3wp.exe (0x1234)\t0x5678\tSharePoint Foundation\tGeneral\t8nca\t{word}\tmsg\t");
            fields(Format::Plain, row.as_bytes(), &CONTEXT).level
        };
        assert_eq!(level("Unexpected"), Level::Error);
        assert_eq!(level("Monitorable"), Level::Notice);
        assert_eq!(level("High"), Level::Notice);
        assert_eq!(level("Verbose"), Level::Debug);
        assert_eq!(level("Critical"), Level::Critical);
        // A row with tabs but no ULS level reads as before.
        let other = plain("11/18/2026 09:30:12\ta\tb\tc\td\te\tLoud\tmsg");
        assert_eq!(other.stamp, None);
        assert_eq!(other.thread, b"");
    }

    #[test]
    fn splits_burn_installer_rows() {
        let parsed =
            plain("[0A1C:0B2C][2026-11-18T12:31:32]i001: Burn v3.11.2.4516, Windows v10.0");
        assert_eq!(stamp_text(&parsed), "2026-11-18T12:31:32.000");
        assert_eq!(parsed.thread, b"0A1C:0B2C");
        assert_eq!(parsed.level, Level::Info);
        assert_eq!(parsed.message, b"i001: Burn v3.11.2.4516, Windows v10.0");
        assert_eq!(
            plain("[0A1C:0B2C][2026-11-18T12:31:33]e000: Error 0x80070642: Failed to install MSI package.").level,
            Level::Error
        );
        assert_eq!(
            plain("[0A1C:0B2C][2026-11-18T12:31:33]w343: Prompt for source of package").level,
            Level::Warning
        );
        // The Visual Studio installer has no codes, so its words decide.
        let studio = plain("[959c:0018][2026-11-18T12:31:32] Adding orphaned package 'system.diagnostics' to the plan for removal.");
        assert_eq!(stamp_text(&studio), "2026-11-18T12:31:32.000");
        assert_eq!(studio.thread, b"959c:0018");
        assert_eq!(studio.level, Level::Info);
        let tagged = plain("[setup][2026-11-18T12:31:32] warning: low disk");
        assert_eq!(tagged.component, b"setup");
        assert_eq!(tagged.level, Level::Warning);
        assert_eq!(plain("[0A1C:0B2C][not a time] x").stamp, None);
    }

    #[test]
    fn splits_get_windows_update_log_rows() {
        let parsed = plain("2026/11/11 14:24:21.1461160 47996 51540 Misc            Load undocked module: UusId:wu.handler.wuauengcore|hr:0x0\r\n");
        assert_eq!(stamp_text(&parsed), "2026-11-11T14:24:21.146");
        assert_eq!(parsed.component, b"Misc");
        assert_eq!(parsed.thread, b"51540");
        assert_eq!(parsed.source, b"47996");
        assert_eq!(
            parsed.message,
            b"Load undocked module: UusId:wu.handler.wuauengcore|hr:0x0"
        );
        let failed = plain("2026/11/18 12:12:59.1234567 1234  5678  Agent           *FAILED* [80240438] wuauengcore.dll");
        assert_eq!(failed.level, Level::Error);
        assert_eq!(failed.component, b"Agent");
        assert_eq!(failed.thread, b"5678");

        // A slashed date that isn't followed by two numbers reads as before.
        let other = plain("2026/11/18 12:12:59 WARN: something");
        assert_eq!(other.level, Level::Warning);
        assert_eq!(other.thread, b"");
    }

    #[test]
    fn splits_winget_rows() {
        let parsed = plain(
            "2026-11-11 15:40:09.381 <I> [CORE] WinGet, version [1.29.380], activity [{43015E72}]",
        );
        assert_eq!(stamp_text(&parsed), "2026-11-11T15:40:09.381");
        assert_eq!(parsed.level, Level::Info);
        assert_eq!(parsed.component, b"CORE");
        assert_eq!(
            parsed.message,
            b"WinGet, version [1.29.380], activity [{43015E72}]"
        );
        assert_eq!(
            plain("2026-11-11 15:40:09.381 <E> [REPO] Failed to open source").level,
            Level::Error
        );
        let untagged = plain("2026-11-11 15:40:09.381 <W> no component here");
        assert_eq!(untagged.level, Level::Warning);
        assert_eq!(untagged.component, b"");
        assert_eq!(untagged.message, b"no component here");
        assert_eq!(
            plain("2026-11-11 15:40:09.381 <X> [CORE] not a level").component,
            b""
        );
    }

    #[test]
    fn rows_say_their_offset_from_utc() {
        let sccm = |time: &str| {
            let row = format!("<![LOG[hi]LOG]!><time=\"{time}\" date=\"11-01-2026\" type=\"1\">");
            utc_offset(Format::Sccm, row.as_bytes())
        };
        // SCCM writes the bias added to local time to get UTC.
        assert_eq!(sccm("01:30:00.000+360"), Some(-360));
        assert_eq!(sccm("01:30:00.000-060"), Some(60));
        assert_eq!(sccm("01:30:00.000+000"), Some(0));
        assert_eq!(sccm("01:30:00.000"), None);
        assert_eq!(sccm("01:30:00.000+99999"), None);
        assert_eq!(
            utc_offset(Format::Plain, b"2026-11-01T01:30:00-05:00 host a[1]: hi"),
            Some(-300)
        );
        assert_eq!(
            utc_offset(Format::Plain, b"2026-11-01 01:30:00, Info CBS hi"),
            None
        );
        assert_eq!(utc_offset(Format::Plain, b"no time at all"), None);
    }

    #[test]
    fn sccm_type_decides_the_level_not_message_keywords() {
        let row = b"<![LOG[error text type=\"3\"]LOG]!><time=\"1\" type=\"1\" thread=\"1\">";
        assert_eq!(fields(Format::Sccm, row, &CONTEXT).level, Level::Info);
        let row = b"<![LOG[all fine]LOG]!><time=\"1\" type=\"2\">";
        assert_eq!(fields(Format::Sccm, row, &CONTEXT).level, Level::Warning);
    }

    #[test]
    fn sccm_multiline_messages_are_kept_whole() {
        let row = b"<![LOG[first\r\nsecond]LOG]!><time=\"1\" type=\"1\">\r\n";
        assert_eq!(
            fields(Format::Sccm, row, &CONTEXT).message,
            b"first\r\nsecond"
        );
    }

    #[test]
    fn malformed_sccm_row_falls_back_to_keywords() {
        let parsed = fields(Format::Sccm, b"an error, not an entry", &CONTEXT);
        assert_eq!(parsed.message, b"an error, not an entry");
        assert_eq!(parsed.level, Level::Error);
    }

    #[test]
    fn searchable_excludes_sccm_metadata() {
        let row = b"<![LOG[hello]LOG]!><time=\"1\" type=\"3\">";
        assert_eq!(searchable(Format::Sccm, row), b"hello");
        assert_eq!(searchable(Format::Plain, b"line\r\n"), b"line");
    }

    #[test]
    fn detects_sccm_only_with_a_complete_entry() {
        assert_eq!(detect_format(b"<![LOG[a]LOG]!><time=\"1\">"), Format::Sccm);
        assert_eq!(
            detect_format(b"junk\n<![LOG[a]LOG]!><time=\"1\">"),
            Format::Sccm
        );
        assert_eq!(detect_format(b"<![LOG[ cut off"), Format::Plain);
        assert_eq!(detect_format(b"an ordinary log"), Format::Plain);
    }

    #[test]
    fn reads_classic_syslog() {
        let parsed = plain("Apr 23 14:25:00 myhost sshd[42]: ERROR: login refused\n");
        assert_eq!(stamp_text(&parsed), "2026-04-23T14:25:00.000");
        assert_eq!(parsed.component, b"sshd");
        assert_eq!(parsed.level, Level::Error);
        assert_eq!(parsed.message, b"login refused");
    }

    #[test]
    fn syslog_level_tag_wins() {
        let parsed = plain("Apr 23 14:25:00 host app[1] <Warning>: something odd");
        assert_eq!(parsed.level, Level::Warning);
        assert_eq!(parsed.message, b"something odd");
    }

    #[test]
    fn reads_dated_syslog_with_multiword_process() {
        let parsed = plain("2026-03-08 13:46:47-07 localhost Installer Progress[57]: copying");
        assert_eq!(parsed.component, b"Installer Progress");
        assert_eq!(parsed.message, b"copying");
        assert_eq!(stamp_text(&parsed), "2026-03-08T13:46:47.000");
    }

    #[test]
    fn level_in_the_host_position_stays_a_level() {
        let parsed = plain("2026-01-01 10:00:00 ERROR Starting service: foo");
        assert_eq!(parsed.level, Level::Error);
        assert_eq!(parsed.component, b"");
        assert_eq!(parsed.message, b"Starting service: foo");
    }

    #[test]
    fn reads_cbs_and_dism_rows() {
        let parsed = plain(
            "2026-04-23 14:25:00, Info                  CBS    Starting TrustedInstaller\r\n",
        );
        assert_eq!(parsed.level, Level::Info);
        assert_eq!(parsed.component, b"CBS");
        assert_eq!(parsed.message, b"Starting TrustedInstaller");
        let parsed =
            plain("2026-04-23 14:25:00, Warning               DISM   DISM Provider: PID=1");
        assert_eq!(parsed.level, Level::Warning);
        assert_eq!(parsed.component, b"DISM");
    }

    #[test]
    fn cbs_failures_written_at_info_are_raised_to_errors() {
        let failed = plain("2026-04-23 14:25:00, Info  CBS  Failed to open package.");
        assert_eq!(failed.level, Level::Error);
        let hresult =
            plain("2026-04-23 14:25:00, Info  CBS  Could not open. [HRESULT = 0x800f0805]");
        assert_eq!(hresult.level, Level::Error);
        let success = plain("2026-04-23 14:25:00, Info  CBS  Opened. [HRESULT = 0x00000000]");
        assert_eq!(success.level, Level::Info);
    }

    #[test]
    fn cbs_stated_levels_are_never_lowered() {
        let parsed = plain("2026-04-23 14:25:00, Error  CSI  all fine here");
        assert_eq!(parsed.level, Level::Error);
        let perf = plain("2026-04-23 14:25:00, Perf  CBS  timing data");
        assert_eq!(perf.level, Level::Debug);
    }

    #[test]
    fn reads_bracketed_timestamps() {
        let split = plain("[2026-04-19 19:54:24.115] [error] App failed");
        assert_eq!(split.level, Level::Error);
        assert_eq!(split.message, b"App failed");
        assert_eq!(stamp_text(&split), "2026-04-19T19:54:24.115");

        let combined = plain("[2026-04-03 11:52:48 INFO chrome-native-host] starting");
        assert_eq!(combined.level, Level::Info);
        assert_eq!(combined.component, b"chrome-native-host");

        let component_only = plain("[2026-04-03 11:52:48] [worker] crashed");
        assert_eq!(component_only.component, b"worker");
        assert_eq!(component_only.level, Level::Error);
    }

    #[test]
    fn reads_prefix_tagged_rows() {
        let parsed = plain("[VM] 2026-03-28 15:17:57 [info] startVM called");
        assert_eq!(parsed.component, b"VM");
        assert_eq!(parsed.level, Level::Info);
        assert_eq!(parsed.message, b"startVM called");
    }

    #[test]
    fn reads_logfmt() {
        let parsed = plain("time=\"2026-04-20T15:37:24-05:00\" level=warning msg=\"disk low\"");
        assert_eq!(parsed.level, Level::Warning);
        assert_eq!(parsed.message, b"disk low");
        assert_eq!(stamp_text(&parsed), "2026-04-20T15:37:24.000");
    }

    #[test]
    fn reads_component_after_timestamp() {
        let parsed = plain("2026-04-23T14:32:01.812 kernel[0]: NOTICE: en0: link down");
        assert_eq!(parsed.component, b"kernel");
        assert_eq!(parsed.level, Level::Notice);
    }

    #[test]
    fn reads_plain_timestamp_with_level() {
        let parsed = plain("2026-04-23T14:32:01 [WARN] retry later");
        assert_eq!(parsed.level, Level::Warning);
        assert_eq!(parsed.message, b"retry later");
    }

    #[test]
    fn reads_windows_update_style_dates() {
        let parsed = plain("2026/04/23 14:25:00.1234567 1234 5678 Agent  Installing");
        assert_eq!(stamp_text(&parsed), "2026-04-23T14:25:00.123");
    }

    #[test]
    fn levels_must_be_whole_words() {
        let parsed = plain("Information loaded");
        assert_eq!(parsed.level, Level::Info);
        assert_eq!(parsed.message, b"Information loaded");
        let parsed = plain("Debugging is underway");
        assert_eq!(parsed.message, b"Debugging is underway");
        let parsed = plain("Errors: none");
        assert_eq!(parsed.message, b"Errors: none");
    }

    #[test]
    fn sentences_keep_their_first_word() {
        let parsed = plain("Error connecting to server");
        assert_eq!(parsed.level, Level::Error);
        assert_eq!(parsed.message, b"Error connecting to server");
    }

    #[test]
    fn stated_levels_are_removed_from_messages() {
        assert_eq!(plain("ERROR disk full").message, b"disk full");
        assert_eq!(plain("Warning: disk low").message, b"disk low");
        assert_eq!(plain("[INFO] started").message, b"started");
    }

    #[test]
    fn urls_are_not_components() {
        let parsed = plain("INFO http://example.com/x loaded");
        assert_eq!(parsed.component, b"");
        assert_eq!(parsed.message, b"http://example.com/x loaded");
        let parsed = plain("INFO worker: started");
        assert_eq!(parsed.component, b"worker");
    }

    #[test]
    fn bare_text_uses_keywords() {
        let parsed = plain("the operation failed\n");
        assert_eq!(parsed.level, Level::Error);
        assert_eq!(parsed.stamp, None);
        assert_eq!(parsed.message, b"the operation failed");
    }

    #[test]
    fn empty_and_odd_rows_are_safe() {
        for row in [
            "", "\n", "\r\n", "   ", "[", "]", "[[", "2026-", "Apr", "time=\"", "<",
        ] {
            let _ = plain(row);
            let _ = fields(Format::Sccm, row.as_bytes(), &CONTEXT);
        }
    }

    #[test]
    fn group_policy_rows_have_their_time_service_and_ids() {
        let parsed =
            plain("GPSVC(188c.25d8) 2026-01-30 11:13:17:216 ProcessGPOs(Machine): Verbose output to eventlog requested.");
        assert_eq!(stamp_text(&parsed), "2026-01-30T11:13:17.216");
        assert_eq!(parsed.component, b"GPSVC");
        assert_eq!(parsed.thread, b"188c.25d8");
        assert_eq!(
            parsed.message,
            b"ProcessGPOs(Machine): Verbose output to eventlog requested."
        );
        assert_eq!(parsed.level, Level::Info);

        // The month and day can be one digit, and other Group Policy logs
        // write the same way.
        let parsed = plain("GPSVC(4b4.5dc) 2026-1-30 07:43:27:860 ProcessGPO(Machine):");
        assert_eq!(stamp_text(&parsed), "2026-01-30T07:43:27.860");
        let parsed = plain("GPMC(5ac.8f0) 2026-01-30 11:13:47:878 Opening the GPO failed");
        assert_eq!(parsed.component, b"GPMC");
        assert_eq!(parsed.level, Level::Error);

        // Before the date was added, a row's time of day stays in its message.
        let parsed = plain("GPSVC(1a4.230) 08:41:26:451 ProcessGPOs(User): Processing ended.");
        assert_eq!(parsed.stamp, None);
        assert_eq!(parsed.component, b"GPSVC");
        assert_eq!(parsed.thread, b"1a4.230");
        assert_eq!(
            parsed.message,
            b"08:41:26:451 ProcessGPOs(User): Processing ended."
        );

        // Other text with brackets isn't taken for it.
        for row in [
            "Function(arg) returned 5",
            "GPSVC(xyz.1) 2026-01-30 11:13:17:216 a",
            "GPSVC(1a4.230) later today",
            "GPSVC(1a4.230)2026-01-30 11:13:17:216 a",
        ] {
            assert_ne!(plain(row).component, b"GPSVC", "{row}");
        }
    }

    #[test]
    fn netlogon_rows_have_their_time_category_and_thread() {
        let parsed = plain(
            "01/31 13:56:53 [SESSION] CONTOSO: NlSetStatusClientSession: Set connection status to 0",
        );
        // The year is the file's, as for syslog.
        assert_eq!(stamp_text(&parsed), "2026-01-31T13:56:53.000");
        assert_eq!(parsed.component, b"SESSION");
        assert_eq!(parsed.thread, b"");
        assert_eq!(
            parsed.message,
            b"CONTOSO: NlSetStatusClientSession: Set connection status to 0"
        );
        assert_eq!(parsed.level, Level::Info);

        // Later versions write a thread, and a failed request is a warning.
        let parsed = plain(
            r"11/18 08:44:11 [LOGON] [2632] CONTOSO: SamLogon: Transitive Network logon of CONTOSO\pat from WKS01 Returns 0xC000006A",
        );
        assert_eq!(stamp_text(&parsed), "2025-11-18T08:44:11.000");
        assert_eq!(parsed.component, b"LOGON");
        assert_eq!(parsed.thread, b"2632");
        assert_eq!(parsed.level, Level::Warning);
        assert_eq!(
            plain("11/18 08:44:11 [LOGON] [2632] CONTOSO: SamLogon: Returns 0x0").level,
            Level::Info
        );

        let parsed = plain("01/31 13:56:53 [CRITICAL] NlDiscoverDc: Cannot find DC.");
        assert_eq!(parsed.level, Level::Error);
        let parsed = plain("01/31 13:56:53 NO_CLIENT_SITE: WKS01 10.0.0.5");
        assert_eq!(parsed.component, b"");
        assert_eq!(parsed.message, b"NO_CLIENT_SITE: WKS01 10.0.0.5");
        assert!(parsed.stamp.is_some());

        // A date that can't be one isn't taken for it.
        assert_eq!(plain("13/31 13:56:53 [LOGON] text").stamp, None);
        assert_eq!(plain("01/31 13:56 [LOGON] text").component, b"");
    }
}
