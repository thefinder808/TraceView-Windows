//! Log text is untrusted, so no row may ever make the parsers panic.

#![no_main]

use libfuzzer_sys::fuzz_target;
use std::sync::{Arc, OnceLock};
use tv_engine::parse::{self, Format, ParseContext};
use tv_engine::summary::normalize;
use tv_engine::time::{self, YearHint};
use tv_engine::CustomFormat;

const HINT: YearHint = YearHint { year: 2026, month: 6 };
const PLAIN: ParseContext = ParseContext {
    year_hint: HINT,
    custom: None,
};
/// Names every part a custom format can, loosely, so most rows match and
/// each part's parsing is reached.
const PATTERN: &str = r"^(?P<time>\S+(?: \S+)?(?: [AP]M)?)\s+\[?(?P<level>\w*)\]?\s*(?P<component>[^:\s]*):?\s*(?P<thread>\d*)\s*(?P<source>\S*)\s*(?P<message>.*)$";

fn custom() -> &'static ParseContext {
    static CONTEXT: OnceLock<ParseContext> = OnceLock::new();
    CONTEXT.get_or_init(|| ParseContext {
        year_hint: HINT,
        custom: Some(Arc::new(CustomFormat::new("Fuzz", PATTERN, "", true).unwrap())),
    })
}

fuzz_target!(|data: &[u8]| {
    for (format, context) in [
        (Format::Plain, &PLAIN),
        (Format::Sccm, &PLAIN),
        (Format::Custom, custom()),
    ] {
        let fields = parse::fields(format, data, context);
        let _ = parse::searchable(format, data);
        let mut template = Vec::new();
        normalize(fields.message, &mut template);
        if let Some(stamp) = fields.stamp {
            let _ = stamp.to_string();
        }
    }
    let _ = parse::detect_format(data);
    let _ = time::parse_iso(data);
    let _ = time::parse_syslog(data, HINT);
    let _ = time::parse_any(data, HINT, false);
    let _ = time::parse_any(data, HINT, true);
    let (split_a, split_b) = data.split_at(data.len() / 2);
    let _ = time::parse_sccm(split_a, split_b);
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = time::parse_user_time(text);
    }
    let _ = tv_engine::encoding::detect(data);
    let _ = tv_engine::encoding::display(data);
});
