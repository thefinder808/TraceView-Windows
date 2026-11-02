//! Error Lookup reads text from logs and from people, so any input at all must
//! be handled without panicking, and every result must be well formed. Set
//! `PROPTEST_CASES` for a longer run.

use proptest::prelude::*;
use tv_errors::{find_codes, lookup, Decoded};

/// Pieces of code-like text, so generated input reaches the parsers' edges
/// rather than failing the first check.
const SYNTAX: &[&str] = &[
    "0x",
    "0X",
    "8007",
    "0005",
    "C000",
    "D000",
    "2EE2",
    "FFFFFFFF",
    "00000000",
    "-",
    "+",
    "2147024891",
    "4294967295",
    "4294967296",
    "12002",
    "5",
    "0",
    "error",
    "Error",
    "error code",
    "exitcode",
    "hr",
    "HRESULT",
    " ",
    "=",
    ":",
    ".",
    ",",
    "/",
    "(",
    ")",
    "KB",
    "%1",
    "%",
    "é",
    "日本",
    "\u{0}",
    "\n",
    "\r\n",
    "\t",
    "ff",
    "g",
];

fn code_like_text() -> impl Strategy<Value = String> {
    proptest::collection::vec(proptest::sample::select(SYNTAX), 0..24)
        .prop_map(|pieces| pieces.concat())
}

fn check_found_codes(message: &str) {
    let found = find_codes(message);
    let mut previous_end = 0;
    for code in &found {
        assert!(code.range.start >= previous_end, "overlap in {found:?}");
        assert_eq!(message.get(code.range.clone()), Some(code.text.as_str()));
        previous_end = code.range.end;
    }
}

fn check_decoded(results: &[Decoded]) {
    for decoded in results {
        assert_eq!(decoded.hex, format!("0x{:08X}", decoded.code));
        assert_eq!(decoded.signed, decoded.code as i32);
        assert!(!decoded.message.is_empty());
        assert_eq!(decoded.message.trim(), decoded.message);
        assert!(!decoded.message.contains("  "));
        assert!(!decoded.source.is_empty());
    }
}

proptest! {
    #[test]
    fn any_text_is_handled(text in any::<String>()) {
        check_found_codes(&text);
        check_decoded(&lookup(&text));
    }

    #[test]
    fn code_like_text_is_handled(text in code_like_text()) {
        check_found_codes(&text);
        check_decoded(&lookup(&text));
    }

    #[test]
    fn any_value_is_handled(value in any::<u32>()) {
        for text in [format!("0x{value:08X}"), value.to_string(), (value as i32).to_string()] {
            let results = lookup(&text);
            check_decoded(&results);
            check_found_codes(&format!("error {text}"));
        }
    }
}
