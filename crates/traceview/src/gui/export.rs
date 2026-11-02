//! Exports rows as CSV for spreadsheets and tickets, on a background thread.
//! Log text is untrusted, so a cell a spreadsheet would run as a formula is
//! made plain text first.

use super::log_view::level_name;
use super::row_cache::SharedLog;
use std::io::{self, BufWriter, Write};
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tv_engine::encoding;
use tv_engine::merge::MergedRow;

/// Excel can't hold more than this in one cell.
const MOST_CELL_CHARS: usize = 32_000;
/// Rows that follow each other in a file are read this many at a time.
const MOST_RUN_ROWS: usize = 256;

/// Rows a tab shows, ready to export on another thread.
pub struct Export {
    pub logs: Vec<SharedLog>,
    /// File names, for a merged tab's File column.
    pub names: Option<Vec<String>>,
    pub order: Option<Arc<Vec<MergedRow>>>,
    /// A filter's rows, or `None` for every row.
    pub filter: Option<Arc<Vec<u32>>>,
    /// Which of the shown rows to export.
    pub rows: Range<usize>,
}

/// Adds a CSV field, quoted when it needs to be. Text a spreadsheet would run
/// as a formula gets a leading apostrophe, so it stays plain text.
fn cell(value: &str, output: &mut String) {
    let value = match value.char_indices().nth(MOST_CELL_CHARS) {
        Some((end, _)) => &value[..end],
        None => value,
    };
    let formula = value.starts_with(['=', '+', '-', '@', '\t', '\r']);
    let quoted = formula || value.contains([',', '"', '\n', '\r']);
    if quoted {
        output.push('"');
    }
    if formula {
        output.push('\'');
    }
    for character in value.chars() {
        if character == '"' {
            output.push('"');
        }
        output.push(character);
    }
    if quoted {
        output.push('"');
    }
}

impl Export {
    /// A shown row's number in the tab, and its file and row there.
    fn locate(&self, shown: usize) -> Option<(usize, usize, usize)> {
        let row = match &self.filter {
            Some(filter) => *filter.get(shown)? as usize,
            None => shown,
        };
        match &self.order {
            None => Some((row, 0, row)),
            Some(order) => order
                .get(row)
                .map(|merged| (row, usize::from(merged.source), merged.row as usize)),
        }
    }

    /// Writes the rows as CSV. `done` counts the rows written, for progress.
    pub fn write(&self, output: impl Write, done: &AtomicUsize) -> io::Result<()> {
        let mut output = BufWriter::with_capacity(1 << 20, output);
        // A byte order mark tells Excel the file is UTF-8.
        output.write_all("\u{FEFF}".as_bytes())?;
        let mut text = String::from(match self.names {
            Some(_) => "Row,File,Time,Level,Component,Thread,Message\r\n",
            None => "Row,Time,Level,Component,Thread,Message\r\n",
        });
        let (mut buffer, mut scratch) = (Vec::new(), Vec::new());
        let mut shown = self.rows.start;
        while shown < self.rows.end {
            let Some((number, source, first)) = self.locate(shown) else { break };
            // Rows that follow each other in one file are read together.
            let mut numbers = vec![number];
            while shown + numbers.len() < self.rows.end && numbers.len() < MOST_RUN_ROWS {
                match self.locate(shown + numbers.len()) {
                    Some((next, next_source, next_row)) if next_source == source && next_row == first + numbers.len() => {
                        numbers.push(next);
                    }
                    _ => break,
                }
            }
            let log = self.logs[source]
                .read()
                .map_err(|_| io::Error::other("the log is unavailable"))?;
            let mut next_number = numbers.iter();
            log.read_rows(first..first + numbers.len(), &mut buffer, |row, raw, _| {
                let fields = log.fields(log.text(raw, &mut scratch));
                let number = next_number.next().copied().unwrap_or(row);
                text.push_str(&(number + 1).to_string());
                if let Some(names) = &self.names {
                    text.push(',');
                    cell(&names[source], &mut text);
                }
                text.push(',');
                if let Some(stamp) = fields.stamp {
                    text.push_str(&stamp.to_string().replace('T', " "));
                }
                text.push(',');
                text.push_str(level_name(log.level(row)));
                for value in [fields.component, fields.thread, fields.message] {
                    text.push(',');
                    cell(&encoding::display(value), &mut text);
                }
                text.push_str("\r\n");
            })?;
            drop(log);
            output.write_all(text.as_bytes())?;
            text.clear();
            shown += numbers.len();
            done.store(shown - self.rows.start, Ordering::Relaxed);
        }
        output.write_all(text.as_bytes())?;
        output.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::sync::RwLock;
    use tv_engine::LogFile;

    fn open(dir: &tempfile::TempDir, name: &str, text: &str) -> SharedLog {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        let (log, _) = LogFile::open_with_cache(&path, None, &AtomicU64::new(0)).unwrap();
        Arc::new(RwLock::new(log))
    }

    fn csv(export: &Export) -> String {
        let mut bytes = Vec::new();
        export.write(&mut bytes, &AtomicUsize::new(0)).unwrap();
        String::from_utf8(bytes).unwrap()
    }

    fn whole(log: SharedLog, rows: Range<usize>) -> Export {
        Export {
            logs: vec![log],
            names: None,
            order: None,
            filter: None,
            rows,
        }
    }

    #[test]
    fn rows_become_csv_that_excel_reads_as_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let log = open(&dir, "a.log", "2026-01-01 10:00:00 host app[1]: ERROR: disk full, again\nplain \"quoted\" café\n");
        assert_eq!(
            csv(&whole(log, 0..2)),
            "\u{FEFF}Row,Time,Level,Component,Thread,Message\r\n\
             1,2026-01-01 10:00:00.000,Error,app,,\"disk full, again\"\r\n\
             2,,Information,,,\"plain \"\"quoted\"\" café\"\r\n"
        );
    }

    #[test]
    fn formulas_in_log_text_stay_plain_text() {
        let mut output = String::new();
        for value in ["=HYPERLINK(\"http://x\")", "+1", "-2+3", "@SUM(A1)", "safe"] {
            cell(value, &mut output);
            output.push('|');
        }
        assert_eq!(output, "\"'=HYPERLINK(\"\"http://x\"\")\"|\"'+1\"|\"'-2+3\"|\"'@SUM(A1)\"|safe|");
    }

    #[test]
    fn huge_cells_are_cut_to_what_excel_holds() {
        let mut output = String::new();
        cell(&"x".repeat(100_000), &mut output);
        assert_eq!(output.len(), MOST_CELL_CHARS);
    }

    #[test]
    fn a_filter_exports_only_its_rows_and_keeps_their_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let text: String = (0..1000).map(|row| format!("line {row}\n")).collect();
        let log = open(&dir, "a.log", &text);
        let export = Export {
            filter: Some(Arc::new(vec![3, 4, 5, 900])),
            ..whole(log, 1..4)
        };
        let done = AtomicUsize::new(0);
        let mut bytes = Vec::new();
        export.write(&mut bytes, &done).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let lines: Vec<&str> = text.lines().skip(1).collect();
        assert_eq!(lines, ["5,,Information,,,line 4", "6,,Information,,,line 5", "901,,Information,,,line 900"]);
        assert_eq!(done.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn merged_rows_name_their_file() {
        let dir = tempfile::tempdir().unwrap();
        let first = open(&dir, "a.log", "a0\na1\n");
        let second = open(&dir, "b.log", "b0\n");
        let order = vec![
            MergedRow { source: 0, row: 0 },
            MergedRow { source: 1, row: 0 },
            MergedRow { source: 0, row: 1 },
        ];
        let export = Export {
            logs: vec![first, second],
            names: Some(vec!["a.log".to_string(), "b.log".to_string()]),
            order: Some(Arc::new(order)),
            filter: None,
            rows: 0..3,
        };
        let text = csv(&export);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "\u{FEFF}Row,File,Time,Level,Component,Thread,Message");
        assert_eq!(&lines[1..], ["1,a.log,,Information,,,a0", "2,b.log,,Information,,,b0", "3,a.log,,Information,,,a1"]);
    }

    #[test]
    fn many_rows_are_read_in_runs_and_all_arrive() {
        let dir = tempfile::tempdir().unwrap();
        let text: String = (0..5000).map(|row| format!("line {row}\n")).collect();
        let log = open(&dir, "a.log", &text);
        let output = csv(&whole(log, 0..5000));
        assert_eq!(output.lines().count(), 5001);
        assert!(output.ends_with("5000,,Information,,,line 4999\r\n"));
    }
}
