//! Splits one row's UTF-8 bytes into fields. These are hand-written rather than
//! regexes because indexing runs them on every row of very large files.
//!
//! Plain-text formats are tried from most specific to most general, in the same
//! order as the macOS app, plus the CBS and DISM format used on Windows.

use crate::custom::CustomFormat;
use crate::level::{self, Level};
use crate::time::{self, Stamp, YearHint};
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
            let time = sccm_attribute(attributes, b"time=\"");
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
        Format::Plain | Format::Custom => time::parse_iso(trim_spaces(line))?.zone,
    }
}

/// The text a filter searches. SCCM metadata is left out, so searching for
/// `type` doesn't match every row.
pub fn searchable(format: Format, row: &[u8]) -> &[u8] {
    let line = trim_line_ending(row);
    match format {
        Format::Plain | Format::Custom => line,
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

fn plain_fields<'a>(line: &'a [u8], context: &ParseContext) -> Fields<'a> {
    let text = trim_spaces(line);
    let Some(first) = text.first() else {
        return Fields::message_only(text, Level::Info);
    };
    let parsed = if first.is_ascii_digit() {
        dated_fields(text)
    } else if *first == b'[' {
        bracketed_fields(text)
    } else if first.is_ascii_alphabetic() {
        syslog_fields(text, context).or_else(|| logfmt_fields(text))
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

/// Rows that begin with `[`: `[timestamp tag] [tag] message`,
/// `[prefix] timestamp message`, or `[LEVEL] message`.
fn bracketed_fields(text: &[u8]) -> Option<Fields<'_>> {
    let close = memchr::memchr(b']', text)?;
    let inside = &text[1..close];
    let after = skip_spaces(&text[close + 1..]);

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
}
