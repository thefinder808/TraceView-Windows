//! Finds likely error codes in a log message, so they can be marked and looked up.
//!
//! Three shapes count as codes:
//! - Eight-digit hex starting 0x8 or 0xC, as in "(0x80070005)". That's where
//!   failure HRESULTs and NTSTATUS errors live.
//! - Any hex or decimal value after a word that says it's a code: `HRESULT`,
//!   `hr`, `error`, `error code`, `errorcode` or `exit code`, as in `hr=0x1`,
//!   `error 5` or `error code 2147942405`.
//! - A ten-digit negative decimal, as in "-2147024891", which is how failure
//!   HRESULTs print when read as signed.
//!
//! Plain numbers aren't codes, so timestamps, KB numbers, ports and counts don't
//! match. A value must also end cleanly: one followed by a letter, or by a
//! separator and a digit as in "2026-11-02" or "14:25", is left alone.

use crate::parse::parse_values;
use crate::FoundCode;
use regex::Regex;
use std::sync::LazyLock;

/// A word saying a code follows, its separator, and the value. `hr` needs `=`
/// or `:`, checked afterwards, so "1 hr 5 min" isn't read as a code.
static AFTER_KEYWORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(hresult|hr|error(?:[ _]?code)?|exit[ _]?code)(\s*[:=]\s*|\s+)(0x[0-9a-f]{1,8}|-?[0-9]{1,10})",
    )
    .expect("the keyword pattern is valid")
});

/// Eight-digit hex in the failure ranges, as a whole word.
static FAILURE_HEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b0[xX][89cC][0-9a-fA-F]{7}\b").expect("the hex pattern is valid")
});

/// A ten-digit negative decimal. What comes before it is checked afterwards.
static NEGATIVE_DECIMAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-[0-9]{10}\b").expect("the decimal pattern is valid"));

/// Finds likely error codes in a message: 0x-prefixed 8-digit hex values in the
/// 0x8/0xC ranges, `HRESULT = 0x...`, `hr=0x...`, `error 5`, `error = 5`,
/// `error code 2147942405`, negative 10-digit decimals. Avoids false positives
/// like timestamps, KB numbers (KB5034441), port numbers and plain counts.
///
/// Results are in order and never overlap. Each one's text reads as at least one
/// 32-bit value, though Windows may not have a message for it.
pub fn find_codes(message: &str) -> Vec<FoundCode> {
    let mut found: Vec<FoundCode> = Vec::new();
    for captures in AFTER_KEYWORD.captures_iter(message) {
        let (Some(keyword), Some(separator), Some(value)) =
            (captures.get(1), captures.get(2), captures.get(3))
        else {
            continue;
        };
        let says_equals = separator.as_str().contains([':', '=']);
        let is_hex = value
            .as_str()
            .get(..2)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("0x"));
        if keyword.as_str().eq_ignore_ascii_case("hr") && !says_equals && !is_hex {
            continue;
        }
        push_if_code(message, value.range(), &mut found);
    }
    for value in FAILURE_HEX.find_iter(message) {
        push_if_code(message, value.range(), &mut found);
    }
    for value in NEGATIVE_DECIMAL.find_iter(message) {
        if starts_cleanly(message, value.start()) {
            push_if_code(message, value.range(), &mut found);
        }
    }
    found.sort_by_key(|code| (code.range.start, std::cmp::Reverse(code.range.end)));
    let mut kept: Vec<FoundCode> = Vec::with_capacity(found.len());
    for code in found {
        if kept
            .last()
            .is_none_or(|last| code.range.start >= last.range.end)
        {
            kept.push(code);
        }
    }
    kept
}

fn push_if_code(message: &str, range: std::ops::Range<usize>, found: &mut Vec<FoundCode>) {
    let Some(text) = message.get(range.clone()) else {
        return;
    };
    if ends_cleanly(message, range.end) && !parse_values(text).is_empty() {
        found.push(FoundCode {
            range,
            text: text.to_string(),
        });
    }
}

/// Whether a value starting here stands alone, rather than continuing a word,
/// number or date such as "2026-1234567890".
fn starts_cleanly(message: &str, start: usize) -> bool {
    let before = message
        .get(..start)
        .and_then(|text| text.chars().next_back());
    !before.is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | '/'))
}

/// Whether a value ending here stands alone. A letter or digit straight after
/// means it's part of something longer, and so does a separator followed by a
/// digit, as in a date, time, version or thousands separator.
fn ends_cleanly(message: &str, end: usize) -> bool {
    let mut after = message.get(end..).unwrap_or_default().chars();
    match after.next() {
        None => true,
        Some(c) if c.is_alphanumeric() || c == '_' => false,
        Some('.' | ',' | ':' | '-' | '/') => !after.next().is_some_and(|c| c.is_ascii_digit()),
        Some(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(message: &str) -> Vec<String> {
        find_codes(message)
            .into_iter()
            .map(|code| {
                assert_eq!(&message[code.range.clone()], code.text);
                code.text
            })
            .collect()
    }

    #[test]
    fn finds_codes_in_their_usual_shapes() {
        let cases: &[(&str, &[&str])] = &[
            ("Failed to download (0x80070005)", &["0x80070005"]),
            ("Error: 0xC0000005.", &["0xC0000005"]),
            ("HRESULT = 0x80004005", &["0x80004005"]),
            ("CreateFile failed, hr=0x8007000e", &["0x8007000e"]),
            ("hr: 0x1 returned", &["0x1"]),
            ("failed with error 5", &["5"]),
            ("error = 5", &["5"]),
            ("Error code 2147942405", &["2147942405"]),
            ("errorCode=0x87D00324", &["0x87D00324"]),
            ("Result -2147024891 from call", &["-2147024891"]),
            ("process terminated with exitcode: 1603", &["1603"]),
            ("Exit code 0", &["0"]),
            ("error 5, retrying", &["5"]),
            ("hr=0x80070005: Access denied", &["0x80070005"]),
            (
                "Error 0x87D00324 (-2016410844)",
                &["0x87D00324", "-2016410844"],
            ),
            ("é0x80070005 and ün 0xC0000022", &["0xC0000022"]),
        ];
        for (message, expected) in cases {
            assert_eq!(texts(message), *expected, "{message:?}");
        }
    }

    #[test]
    fn plain_numbers_are_not_codes() {
        for message in [
            "2026-11-02 14:25:00.123 copied 15 files",
            "Installing KB5034441",
            "Connecting to server.contoso.com:443",
            "listening on port 8080",
            "Elapsed 1 hr 5 min",
            "error 2026-11-02",
            "error 14:25",
            "error 5.1 of the spec",
            "error 1,234 rows",
            "3 errors 5 warnings",
            "Error5 is a test name",
            "flags 0x00000005",
            "address 0x80070005ABCD",
            "pointer 0x00007FF6A1B2C3D4",
            "{6F9619FF-8B86-D011-B42D-00C04FC964FF}",
            "epoch 1698765432",
            "offset -12345",
            "id 2026-1234567890",
            "version 10.0.26100.1",
            "abc0x80070005",
            "error 99999999999",
            "error 9999999999",
        ] {
            assert_eq!(texts(message), Vec::<String>::new(), "{message:?}");
        }
    }

    #[test]
    fn ranges_are_byte_offsets() {
        let message = "日本 error 5";
        let found = find_codes(message);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].range, 13..14);
    }

    #[test]
    fn keyword_and_bare_hex_matches_are_not_duplicated() {
        let found = find_codes("error 0x80070005");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "0x80070005");
    }
}
