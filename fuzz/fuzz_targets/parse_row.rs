//! Log text is untrusted, so no row may ever make the parsers panic.

#![no_main]

use libfuzzer_sys::fuzz_target;
use std::sync::{Arc, OnceLock};
use tv_engine::parse::{self, Format, Hint, ParseContext, W3cFields};
use tv_engine::summary::normalize;
use tv_engine::time::{self, YearHint, Zone};
use tv_engine::CustomFormat;

const HINT: YearHint = YearHint { year: 2026, month: 6 };
const PLAIN: ParseContext = ParseContext {
    year_hint: HINT,
    custom: None,
    hint: Hint::None,
};
/// Plain logs whose first lines change how rows are read. The installer log
/// starts near the last time a stamp can hold, so the next day overflows,
/// and the W3C log names a column as late as a row can have.
const HINTED: [ParseContext; 4] = [
    ParseContext {
        year_hint: HINT,
        custom: None,
        hint: Hint::Msi {
            started: time::Stamp {
                seconds: u32::MAX - 60,
                millis: 0,
            },
        },
    },
    ParseContext {
        year_hint: HINT,
        custom: None,
        hint: Hint::Transcript,
    },
    ParseContext {
        year_hint: HINT,
        custom: None,
        hint: Hint::SetupApi,
    },
    ParseContext {
        year_hint: HINT,
        custom: None,
        hint: Hint::W3c(W3cFields {
            date: Some(0),
            time: Some(1),
            status: Some(4),
            action: Some(2),
            page: Some(3),
            client: Some(5),
            process: Some(47),
            utc: true,
            // Far enough behind UTC that a time early on 2000-01-01 can't be shown.
            zone: Zone::Fixed(-12 * 60),
        }),
    },
];
/// Names every part a custom format can, loosely, so most rows match and
/// each part's parsing is reached.
const PATTERN: &str = r"^(?P<time>\S+(?: \S+)?(?: [AP]M)?)\s+\[?(?P<level>\w*)\]?\s*(?P<component>[^:\s]*):?\s*(?P<thread>\d*)\s*(?P<source>\S*)\s*(?P<message>.*)$";

fn custom() -> &'static ParseContext {
    static CONTEXT: OnceLock<ParseContext> = OnceLock::new();
    CONTEXT.get_or_init(|| ParseContext {
        year_hint: HINT,
        custom: Some(Arc::new(CustomFormat::new("Fuzz", PATTERN, "", true).unwrap())),
        hint: Hint::None,
    })
}

fuzz_target!(|data: &[u8]| {
    for (format, context) in [
        (Format::Plain, &PLAIN),
        (Format::Sccm, &PLAIN),
        (Format::Event, &PLAIN),
        (Format::Custom, custom()),
        (Format::Plain, &HINTED[0]),
        (Format::Plain, &HINTED[1]),
        (Format::Plain, &HINTED[2]),
        (Format::Plain, &HINTED[3]),
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
    let _ = parse::hint_for(data);
    let _ = time::parse_msi_date(data);
    let _ = time::parse_compact(data);
    let _ = time::parse_time_of_day(data);
    let _ = time::parse_iso(data);
    let _ = time::parse_syslog(data, HINT);
    let _ = time::parse_any(data, HINT, false);
    let _ = time::parse_any(data, HINT, true);
    let (split_a, split_b) = data.split_at(data.len() / 2);
    let _ = time::parse_sccm(split_a, split_b);
    let _ = time::parse_group_policy(split_a, split_b);
    let _ = time::parse_netlogon(split_a, split_b, HINT);
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = time::parse_user_time(text);
    }
    let _ = tv_engine::encoding::detect(data);
    let _ = tv_engine::encoding::display(data);
});
