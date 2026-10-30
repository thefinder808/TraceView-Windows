//! Text encodings found in Windows logs. Everything past this module works on
//! UTF-8 bytes, and row offsets always refer to bytes in the file.

use encoding_rs::{UTF_16BE, UTF_16LE, WINDOWS_1252};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum Encoding {
    Utf8 = 0,
    Utf16Le = 1,
    Utf16Be = 2,
}

impl Encoding {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Encoding::Utf8),
            1 => Some(Encoding::Utf16Le),
            2 => Some(Encoding::Utf16Be),
            _ => None,
        }
    }

    /// Bytes per code unit, which row boundaries must align to.
    pub fn unit(self) -> usize {
        match self {
            Encoding::Utf8 => 1,
            Encoding::Utf16Le | Encoding::Utf16Be => 2,
        }
    }

    /// Encodes ASCII text, for searching raw file bytes.
    pub fn encode_ascii(self, text: &[u8]) -> Vec<u8> {
        match self {
            Encoding::Utf8 => text.to_vec(),
            Encoding::Utf16Le => text.iter().flat_map(|byte| [*byte, 0]).collect(),
            Encoding::Utf16Be => text.iter().flat_map(|byte| [0, *byte]).collect(),
        }
    }

    /// Converts a row's raw bytes to UTF-8, using `scratch` only when it must.
    pub fn to_utf8<'a>(self, raw: &'a [u8], scratch: &'a mut Vec<u8>) -> &'a [u8] {
        let decoder = match self {
            Encoding::Utf8 => return raw,
            Encoding::Utf16Le => UTF_16LE,
            Encoding::Utf16Be => UTF_16BE,
        };
        let (decoded, _) = decoder.decode_without_bom_handling(raw);
        scratch.clear();
        scratch.extend_from_slice(decoded.as_bytes());
        scratch
    }
}

/// Finds the encoding and where the text starts. BOMs decide when present.
/// Without one, UTF-16 shows up as a NUL in nearly every other byte.
pub fn detect(head: &[u8]) -> (Encoding, u64) {
    if head.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return (Encoding::Utf8, 3);
    }
    if head.starts_with(&[0xFF, 0xFE]) {
        return (Encoding::Utf16Le, 2);
    }
    if head.starts_with(&[0xFE, 0xFF]) {
        return (Encoding::Utf16Be, 2);
    }
    let sample = &head[..head.len().min(256) & !1];
    if sample.len() >= 8 {
        let pairs = sample.len() / 2;
        let high_nuls = sample
            .iter()
            .skip(1)
            .step_by(2)
            .filter(|byte| **byte == 0)
            .count();
        let low_nuls = sample.iter().step_by(2).filter(|byte| **byte == 0).count();
        if high_nuls * 10 >= pairs * 9 {
            return (Encoding::Utf16Le, 0);
        }
        if low_nuls * 10 >= pairs * 9 {
            return (Encoding::Utf16Be, 0);
        }
    }
    (Encoding::Utf8, 0)
}

/// Makes field text displayable. Bytes that aren't valid UTF-8 are read as
/// Windows-1252, the usual encoding for older Windows logs.
pub fn display(bytes: &[u8]) -> Cow<'_, str> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Cow::Borrowed(text),
        Err(_) => WINDOWS_1252.decode_without_bom_handling(bytes).0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boms_decide_the_encoding() {
        assert_eq!(detect(&[0xEF, 0xBB, 0xBF, b'a']), (Encoding::Utf8, 3));
        assert_eq!(detect(&[0xFF, 0xFE, b'a', 0]), (Encoding::Utf16Le, 2));
        assert_eq!(detect(&[0xFE, 0xFF, 0, b'a']), (Encoding::Utf16Be, 2));
    }

    #[test]
    fn utf16_without_a_bom_is_recognized() {
        let little: Vec<u8> = b"hello world".iter().flat_map(|byte| [*byte, 0]).collect();
        let big: Vec<u8> = b"hello world".iter().flat_map(|byte| [0, *byte]).collect();
        assert_eq!(detect(&little), (Encoding::Utf16Le, 0));
        assert_eq!(detect(&big), (Encoding::Utf16Be, 0));
    }

    #[test]
    fn ordinary_text_is_utf8() {
        assert_eq!(detect(b"plain text log line"), (Encoding::Utf8, 0));
        assert_eq!(detect(b""), (Encoding::Utf8, 0));
    }

    #[test]
    fn decodes_utf16_rows() {
        let raw: Vec<u8> = "caf\u{e9}\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let mut scratch = Vec::new();
        assert_eq!(
            Encoding::Utf16Le.to_utf8(&raw, &mut scratch),
            "caf\u{e9}\n".as_bytes()
        );
    }

    #[test]
    fn utf8_rows_are_not_copied() {
        let raw = b"text";
        let mut scratch = Vec::new();
        let result = Encoding::Utf8.to_utf8(raw, &mut scratch);
        assert_eq!(result.as_ptr(), raw.as_ptr());
    }

    #[test]
    fn invalid_utf8_reads_as_windows_1252() {
        assert_eq!(display(b"caf\xE9"), "caf\u{e9}");
        assert_eq!(display("caf\u{e9}".as_bytes()), "caf\u{e9}");
    }
}
