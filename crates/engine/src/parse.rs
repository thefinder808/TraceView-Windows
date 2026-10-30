//! Splits one row's raw bytes into display fields. Runs only for rows being
//! shown or filtered, never during indexing.

use crate::index::Format;
use memchr::memmem;

const SCCM_START: &[u8] = b"<![LOG[";
const SCCM_END: &[u8] = b"]LOG]!>";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Fields<'a> {
    pub date: &'a [u8],
    pub time: &'a [u8],
    pub component: &'a [u8],
    pub message: &'a [u8],
}

pub fn fields(format: Format, row: &[u8]) -> Fields<'_> {
    match format {
        Format::Plain => plain_fields(row),
        Format::Sccm => sccm_fields(row).unwrap_or(Fields {
            message: row,
            ..Fields::default()
        }),
    }
}

/// The text a filter searches. SCCM metadata is left out so a search for
/// `type` doesn't match every row.
pub fn searchable(format: Format, row: &[u8]) -> &[u8] {
    match format {
        Format::Plain => trim_line_ending(row),
        Format::Sccm => sccm_message(row).unwrap_or(row),
    }
}

fn trim_line_ending(row: &[u8]) -> &[u8] {
    let line = row.strip_suffix(b"\n").unwrap_or(row);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn sccm_message(row: &[u8]) -> Option<&[u8]> {
    let start = row.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(row);
    let start = start.strip_prefix(SCCM_START)?;
    Some(&start[..memmem::rfind(start, SCCM_END)?])
}

fn sccm_fields(row: &[u8]) -> Option<Fields<'_>> {
    let message = sccm_message(row)?;
    let end = memmem::rfind(row, SCCM_END)?;
    let attributes = &row[end + SCCM_END.len()..];
    let time = attribute(attributes, b"time=\"").unwrap_or(b"");
    let time = &time[..memchr::memchr(b'+', time).unwrap_or(time.len())];
    Some(Fields {
        date: attribute(attributes, b"date=\"").unwrap_or(b""),
        time,
        component: attribute(attributes, b"component=\"").unwrap_or(b""),
        message,
    })
}

fn attribute<'a>(attributes: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let start = memmem::find(attributes, name)? + name.len();
    let value = &attributes[start..];
    Some(&value[..memchr::memchr(b'"', value)?])
}

/// Handles `YYYY-MM-DD HH:MM:SS[.mmm][tz] host process[pid]: message`. Anything
/// else is shown as a message with no other fields.
fn plain_fields(row: &[u8]) -> Fields<'_> {
    let line = trim_line_ending(row);
    let whole_line = Fields {
        message: line,
        ..Fields::default()
    };

    let dated = line.len() > 19 && line[4] == b'-' && line[7] == b'-' && line[10] == b' ';
    if !dated {
        return whole_line;
    }
    let after_date = &line[11..];
    let Some(time_end) = memchr::memchr(b' ', after_date) else {
        return whole_line;
    };
    let after_time = &after_date[time_end + 1..];
    let Some(host_end) = memchr::memchr(b' ', after_time) else {
        return whole_line;
    };
    let after_host = &after_time[host_end + 1..];
    let Some(colon) = memmem::find(after_host, b": ") else {
        return Fields {
            date: &line[..10],
            time: &after_date[..time_end],
            component: b"",
            message: after_time,
        };
    };
    let process = &after_host[..colon];
    Fields {
        date: &line[..10],
        time: &after_date[..time_end],
        component: &process[..memchr::memchr(b'[', process).unwrap_or(process.len())],
        message: &after_host[colon + 2..],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_sccm_fields() {
        let row = b"<![LOG[Failed ]LOG]!> text]LOG]!><time=\"10:23:02.334+000\" date=\"04-06-2026\" component=\"PolicyAgent\" context=\"\" type=\"3\" thread=\"5240\" file=\"a.cpp\">\r\n";
        let parsed = fields(Format::Sccm, row);
        assert_eq!(parsed.time, b"10:23:02.334");
        assert_eq!(parsed.date, b"04-06-2026");
        assert_eq!(parsed.component, b"PolicyAgent");
        assert_eq!(parsed.message, b"Failed ]LOG]!> text");
    }

    #[test]
    fn malformed_sccm_row_falls_back_to_a_message() {
        let row = b"not an entry";
        assert_eq!(fields(Format::Sccm, row).message, row);
    }

    #[test]
    fn splits_plain_fields() {
        let parsed = fields(
            Format::Plain,
            b"2026-04-23 14:25:00.000 api-01 http[4521]: INFO: GET /v1 -> 200\r\n",
        );
        assert_eq!(parsed.date, b"2026-04-23");
        assert_eq!(parsed.time, b"14:25:00.000");
        assert_eq!(parsed.component, b"http");
        assert_eq!(parsed.message, b"INFO: GET /v1 -> 200");
    }

    #[test]
    fn undated_plain_row_is_only_a_message() {
        let parsed = fields(Format::Plain, b"just text\n");
        assert_eq!(parsed.date, b"");
        assert_eq!(parsed.message, b"just text");
    }

    #[test]
    fn searchable_excludes_sccm_metadata() {
        let row = b"<![LOG[hello]LOG]!><time=\"1\" type=\"3\">";
        assert_eq!(searchable(Format::Sccm, row), b"hello");
    }
}
