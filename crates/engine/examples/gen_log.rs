//! Writes a deterministic test log, so timings compare across runs and machines.
//!
//! Usage: gen_log <plain|sccm> <gigabytes> <output path>
//! Severity mix matches the Mac generator: about 95% info, 4% warning, 1% error.

use std::fs::File;
use std::io::{BufWriter, Write};

const COMPONENTS: [&str; 8] = [
    "CcmExec",
    "PolicyAgent",
    "ContentTransferManager",
    "ScanAgent",
    "StateMessage",
    "WUAHandler",
    "UpdatesDeployment",
    "AppEnforce",
];
const INFO: [&str; 7] = [
    "Successfully connected to management point mp01.contoso.com",
    "Sending state message body to the management point",
    "Software update scan completed successfully, 3 updates applicable",
    "Client certificate validation succeeded for the current site",
    "Evaluating deployment policy assignments for the local machine",
    "Cache invalidated and rebuilt from the content library",
    "Configuration loaded from the local policy store",
];
const WARN: [&str; 3] = [
    "Retrying policy download in 15 minutes after a transient failure",
    "Deprecated client setting found in the local policy store",
    "Throttling enabled because the distribution point is busy",
];
const ERROR: [&str; 3] = [
    "Failed to download policy from http://mp01.contoso.com/SMS_MP - error 0x80004005",
    "Content download failed for update KB5034441: hash mismatch",
    "Unable to acquire the content lock - error 0x80070020",
];

/// Small deterministic generator, so no dependency decides the file contents.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, limit: u64) -> u64 {
        self.next() % limit
    }
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, format, gigabytes, output] = args.as_slice() else {
        eprintln!("usage: gen_log <plain|sccm> <gigabytes> <output path>");
        std::process::exit(2);
    };
    let sccm = match format.as_str() {
        "sccm" => true,
        "plain" => false,
        _ => {
            eprintln!("format must be plain or sccm");
            std::process::exit(2);
        }
    };
    let target = (gigabytes
        .parse::<f64>()
        .expect("gigabytes must be a number")
        * 1024.0
        * 1024.0
        * 1024.0) as u64;

    let mut out = BufWriter::with_capacity(4 * 1024 * 1024, File::create(output)?);
    let mut random = Random(0x9E37_79B9_7F4A_7C15);
    let mut line = Vec::with_capacity(512);
    let (mut written, mut rows) = (0u64, 0u64);

    while written < target {
        let second = rows / 20;
        let (hour, minute, sec) = (second / 3600 % 24, second / 60 % 60, second % 60);
        let day = second / 86_400 % 28 + 1;
        let month = second / (86_400 * 28) % 12 + 1;
        let millis = rows % 20 * 50;
        let component = COMPONENTS[random.below(8) as usize];
        let thread = 1000 + random.below(9000);

        let roll = random.below(100);
        let (message, severity) = if roll < 95 {
            (INFO[random.below(7) as usize], 1)
        } else if roll < 99 {
            (WARN[random.below(3) as usize], 2)
        } else {
            (ERROR[random.below(3) as usize], 3)
        };

        line.clear();
        if sccm {
            // Real SCCM logs have occasional multi-line messages.
            let extra = if rows % 1000 == 999 {
                "\r\ncontinuation line"
            } else {
                ""
            };
            write!(
                line,
                "<![LOG[{message}{extra}]LOG]!><time=\"{hour:02}:{minute:02}:{sec:02}.{millis:03}+000\" date=\"{month:02}-{day:02}-2026\" component=\"{component}\" context=\"\" type=\"{severity}\" thread=\"{thread}\" file=\"{}.cpp\">\r\n",
                component.to_lowercase()
            )?;
        } else {
            writeln!(
                line,
                "2026-{month:02}-{day:02} {hour:02}:{minute:02}:{sec:02}.{millis:03}-05 host01 {component}[{thread}]: {message}"
            )?;
        }
        out.write_all(&line)?;
        written += line.len() as u64;
        rows += 1;
    }
    out.flush()?;
    println!("wrote {output}: {rows} rows, {written} bytes");
    Ok(())
}
