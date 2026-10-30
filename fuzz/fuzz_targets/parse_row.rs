//! Log text is untrusted, so no row may ever make the parsers panic.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tv_engine::parse::{self, Format, ParseContext};
use tv_engine::summary::normalize;
use tv_engine::time::{self, YearHint};

const CONTEXT: ParseContext = ParseContext {
    year_hint: YearHint { year: 2026, month: 6 },
};

fuzz_target!(|data: &[u8]| {
    for format in [Format::Plain, Format::Sccm] {
        let fields = parse::fields(format, data, &CONTEXT);
        let _ = parse::searchable(format, data);
        let mut template = Vec::new();
        normalize(fields.message, &mut template);
        if let Some(stamp) = fields.stamp {
            let _ = stamp.to_string();
        }
    }
    let _ = parse::detect_format(data);
    let _ = time::parse_iso(data);
    let _ = time::parse_syslog(data, CONTEXT.year_hint);
    let (split_a, split_b) = data.split_at(data.len() / 2);
    let _ = time::parse_sccm(split_a, split_b);
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = time::parse_user_time(text);
    }
    let _ = tv_engine::encoding::detect(data);
    let _ = tv_engine::encoding::display(data);
});
