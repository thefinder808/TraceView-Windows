//! Interleaves several logs into one timeline, for a merged view like CMTrace's
//! "Merge selected files".
//!
//! The index keeps only whole seconds, which can't order rows from different
//! files within the same second, so merging reads every row for its
//! milliseconds. They're kept only while merging, at 2 bytes per row, so the
//! index stays at 10 bytes per row.
//!
//! Each file keeps its own row order. A row's place in the timeline is the
//! latest time its file has reached by that row, so a row with no time, such
//! as a continuation line, stays right after the row before it, and so does a
//! row whose time goes backwards. Rows before a file's first time take that
//! time, so a header stays with its file. A file with no times at all comes
//! first. Ties go to the earlier source, then the earlier row.

use crate::file::LogFile;
use crate::query::{Control, QueryError, ROWS_PER_TASK};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::binary_heap::{BinaryHeap, PeekMut};
use std::io;
use std::sync::atomic::Ordering;

/// Blocks of rows per merge task. Each task first finds where it starts in
/// every file, which costs up to a block per file, so parts are kept much
/// larger than that while still giving every core several to work on.
const BLOCKS_PER_PART: usize = 16;

/// One row of a merged timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergedRow {
    /// The log's position in the slice given to [`merge_order`].
    pub source: u16,
    /// The zero-based row in that log.
    pub row: u32,
}

/// One log's times to the millisecond, read for a merge.
struct Timeline<'a> {
    log: &'a LogFile,
    /// Each row's milliseconds, to go with the whole seconds the index holds.
    millis: Vec<u16>,
    block_rows: usize,
    /// The latest time the file has reached by the end of each block. It never
    /// decreases, so it can be searched.
    block_latest: Vec<u64>,
    /// The file's first time, which rows before it share. Zero if no row has one.
    first: u64,
}

/// A position in one file while merging.
#[derive(Clone, Copy)]
struct Cursor {
    row: usize,
    /// The latest time the file reached before `row`.
    latest: u64,
}

impl Timeline<'_> {
    /// A row's own time in milliseconds, if it has one.
    fn time(&self, row: usize) -> Option<u64> {
        self.log
            .stamp_seconds(row)
            .map(|seconds| u64::from(seconds) * 1000 + u64::from(self.millis[row]))
    }

    /// A row's place in the timeline, given the latest time its file reached
    /// before it.
    fn place(&self, row: usize, latest: u64) -> u64 {
        self.time(row).map_or(latest, |time| time.max(latest))
    }

    fn start(&self) -> Cursor {
        Cursor {
            row: 0,
            latest: self.first,
        }
    }

    fn end(&self) -> Cursor {
        Cursor {
            row: self.millis.len(),
            latest: 0,
        }
    }

    /// Finds the first row whose place is at or after `target`.
    fn seek(&self, target: u64) -> Cursor {
        let block = self.block_latest.partition_point(|latest| *latest < target);
        let mut latest = match block.checked_sub(1) {
            Some(previous) => self.block_latest[previous],
            None => self.first,
        };
        let start = block * self.block_rows;
        let end = (start + self.block_rows).min(self.millis.len());
        for row in start..end {
            let place = self.place(row, latest);
            if place >= target {
                return Cursor { row, latest };
            }
            latest = place;
        }
        Cursor { row: end, latest }
    }
}

/// Reads every row of `log` that the index gives a time, in parallel blocks,
/// to learn its milliseconds.
fn read_timeline<'a>(
    log: &'a LogFile,
    block_rows: usize,
    control: &Control,
) -> Result<Timeline<'a>, QueryError> {
    let mut millis = vec![0u16; log.rows()];
    // Each block's first time and latest time, if it has any.
    let extents: Vec<Option<(u64, u64)>> = millis
        .par_chunks_mut(block_rows)
        .enumerate()
        .map_init(
            || (Vec::new(), Vec::new()),
            |(buffer, scratch), (block, block_millis)| {
                if control.cancel.load(Ordering::Relaxed) {
                    return Err(QueryError::Cancelled);
                }
                let start = block * block_rows;
                let rows = start..start + block_millis.len();
                // Skip the read when the index says no row here has a time.
                if rows.clone().any(|row| log.stamp_seconds(row).is_some()) {
                    log.read_rows(rows.clone(), buffer, |row, raw, _| {
                        let Some(seconds) = log.stamp_seconds(row) else {
                            return;
                        };
                        let text = log.text(raw, scratch);
                        // The index decides each row's second, so merged order
                        // agrees with time filters. Milliseconds count only if
                        // the row still reads as that second, which it might
                        // not if the file was rewritten since it was indexed.
                        if let Some(stamp) = log.fields(text).stamp {
                            if stamp.seconds == seconds {
                                block_millis[row - start] = stamp.millis;
                            }
                        }
                    })?;
                }
                control
                    .rows_done
                    .fetch_add(block_millis.len(), Ordering::Relaxed);
                let mut times = rows.filter_map(|row| {
                    log.stamp_seconds(row).map(|seconds| {
                        u64::from(seconds) * 1000 + u64::from(block_millis[row - start])
                    })
                });
                Ok(times
                    .next()
                    .map(|first| (first, times.fold(first, u64::max))))
            },
        )
        .collect::<Result<_, QueryError>>()?;

    let first = extents
        .iter()
        .flatten()
        .next()
        .map_or(0, |(first, _)| *first);
    let mut latest = first;
    let block_latest = extents
        .iter()
        .map(|extent| {
            if let Some((_, block_latest)) = extent {
                latest = latest.max(*block_latest);
            }
            latest
        })
        .collect();
    Ok(Timeline {
        log,
        millis,
        block_rows,
        block_latest,
        first,
    })
}

/// Merges one part of the timeline: each file's rows from `starts` up to
/// `ends`, which together fill `output` exactly.
fn merge_part(
    timelines: &[Timeline],
    starts: &[Cursor],
    ends: &[Cursor],
    output: &mut [MergedRow],
) {
    let mut cursors = starts.to_vec();
    // The next row of each file by place, then source, earliest first.
    let mut heads = BinaryHeap::with_capacity(timelines.len());
    for (source, timeline) in timelines.iter().enumerate() {
        let cursor = cursors[source];
        if cursor.row < ends[source].row {
            heads.push(Reverse((
                timeline.place(cursor.row, cursor.latest),
                source as u16,
            )));
        }
    }
    for slot in output {
        let Some(mut head) = heads.peek_mut() else {
            break;
        };
        let Reverse((place, source)) = *head;
        let index = usize::from(source);
        let cursor = &mut cursors[index];
        *slot = MergedRow {
            source,
            row: cursor.row as u32,
        };
        cursor.row += 1;
        cursor.latest = place;
        if cursor.row < ends[index].row {
            *head = Reverse((timelines[index].place(cursor.row, place), source));
        } else {
            PeekMut::pop(head);
        }
    }
}

/// Interleaves several logs into one timeline by timestamp, for a merged view
/// like CMTrace's "Merge selected files". Uses all cores and can be cancelled.
/// `control.rows_done` counts rows read, which is most of the work, so it
/// reaches the logs' total row count shortly before the merge finishes.
///
/// The result holds 8 bytes per row, and the merge briefly needs 2 more bytes
/// per row for milliseconds.
pub fn merge_order(logs: &[&LogFile], control: &Control) -> Result<Vec<MergedRow>, QueryError> {
    merge_with_sizes(logs, control, ROWS_PER_TASK, BLOCKS_PER_PART)
}

fn merge_with_sizes(
    logs: &[&LogFile],
    control: &Control,
    block_rows: usize,
    blocks_per_part: usize,
) -> Result<Vec<MergedRow>, QueryError> {
    if logs.len() > usize::from(u16::MAX) + 1 {
        return Err(QueryError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "can't merge more than 65,536 logs",
        )));
    }
    let timelines: Vec<Timeline> = logs
        .par_iter()
        .map(|log| read_timeline(log, block_rows, control))
        .collect::<Result<_, QueryError>>()?;

    // Split the timeline into parts that merge independently, at times taken
    // from the ends of blocks so parts are of similar size. Places never
    // decrease within a file, so a boundary splits each file at one row, and
    // rows with the same place always land in the same part.
    let mut samples: Vec<u64> = timelines
        .iter()
        .flat_map(|timeline| timeline.block_latest.iter().copied())
        .collect();
    samples.sort_unstable();
    let mut boundaries: Vec<u64> = samples
        .into_iter()
        .skip(blocks_per_part)
        .step_by(blocks_per_part)
        .collect();
    boundaries.dedup();
    let mut edges: Vec<Vec<Cursor>> = Vec::with_capacity(boundaries.len() + 2);
    edges.push(timelines.iter().map(Timeline::start).collect());
    edges.par_extend(boundaries.par_iter().map(|boundary| {
        timelines
            .iter()
            .map(|timeline| timeline.seek(*boundary))
            .collect()
    }));
    edges.push(timelines.iter().map(Timeline::end).collect());

    // Each part writes its own stretch of the result.
    let total = timelines.iter().map(|timeline| timeline.millis.len()).sum();
    let mut merged = vec![MergedRow { source: 0, row: 0 }; total];
    let mut outputs = Vec::with_capacity(edges.len() - 1);
    let mut rest = merged.as_mut_slice();
    for pair in edges.windows(2) {
        let rows = pair[0]
            .iter()
            .zip(&pair[1])
            .map(|(start, end)| end.row - start.row)
            .sum();
        let (output, after) = std::mem::take(&mut rest).split_at_mut(rows);
        outputs.push(output);
        rest = after;
    }
    edges
        .par_windows(2)
        .zip(outputs)
        .try_for_each(|(pair, output)| {
            if control.cancel.load(Ordering::Relaxed) {
                return Err(QueryError::Cancelled);
            }
            merge_part(&timelines, &pair[0], &pair[1], output);
            Ok(())
        })?;
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::sync::atomic::AtomicU64;

    fn open(dir: &tempfile::TempDir, name: &str, text: &str) -> LogFile {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        LogFile::open_with_cache(&path, None, &AtomicU64::new(0))
            .unwrap()
            .0
    }

    /// Opens one log per text, named by position.
    fn open_all(texts: &[&str]) -> (tempfile::TempDir, Vec<LogFile>) {
        let dir = tempfile::tempdir().unwrap();
        let logs = texts
            .iter()
            .enumerate()
            .map(|(number, text)| open(&dir, &format!("{number}.log"), text))
            .collect();
        (dir, logs)
    }

    /// The merged order as (source, row) pairs, for readable expectations.
    fn merged(logs: &[LogFile]) -> Vec<(u16, u32)> {
        let logs: Vec<&LogFile> = logs.iter().collect();
        merge_order(&logs, &Control::default())
            .unwrap()
            .iter()
            .map(|merged| (merged.source, merged.row))
            .collect()
    }

    /// The rules written as simply as possible: give every row its place, then
    /// sort by place, source and row. Times come straight from parsing here,
    /// not from the index.
    fn simple_merge(logs: &[&LogFile]) -> Vec<MergedRow> {
        let mut placed = Vec::new();
        for (source, log) in logs.iter().enumerate() {
            let mut times = Vec::new();
            let mut scratch = Vec::new();
            log.read_rows(0..log.rows(), &mut Vec::new(), |_, raw, _| {
                let text = log.text(raw, &mut scratch);
                times.push(
                    log.fields(text)
                        .stamp
                        .map(|stamp| u64::from(stamp.seconds) * 1000 + u64::from(stamp.millis)),
                );
            })
            .unwrap();
            let mut latest = times.iter().flatten().next().copied().unwrap_or(0);
            for (row, time) in times.iter().enumerate() {
                latest = latest.max(time.unwrap_or(0));
                placed.push((latest, source as u16, row as u32));
            }
        }
        placed.sort_unstable();
        placed
            .into_iter()
            .map(|(_, source, row)| MergedRow { source, row })
            .collect()
    }

    #[test]
    fn orders_rows_within_one_second_by_milliseconds() {
        let (_dir, logs) = open_all(&[
            "2026-01-01 10:00:00.500 a\n2026-01-01 10:00:01.000 a\n",
            "2026-01-01 10:00:00.250 b\n2026-01-01 10:00:00.750 b\n",
        ]);
        assert_eq!(merged(&logs), vec![(1, 0), (0, 0), (1, 1), (0, 1)]);
    }

    #[test]
    fn merges_sccm_and_plain_logs_by_milliseconds() {
        let entry = |time: &str| {
            format!("<![LOG[entry]LOG]!><time=\"{time}+000\" date=\"01-01-2026\" component=\"App\" context=\"\" type=\"1\" thread=\"1\" file=\"\">\r\n")
        };
        let sccm = [entry("10:00:00.100"), entry("10:00:00.900")].concat();
        let (_dir, logs) = open_all(&[&sccm, "2026-01-01 10:00:00.500 plain\n"]);
        assert_eq!(merged(&logs), vec![(0, 0), (1, 0), (0, 1)]);
    }

    #[test]
    fn rows_without_a_time_stay_after_the_row_before_them() {
        let (_dir, logs) = open_all(&[
            "2026-01-01 10:00:00 first\n  continued\n  and more\n2026-01-01 10:00:02 last\n",
            "2026-01-01 10:00:01 other\n",
        ]);
        assert_eq!(merged(&logs), vec![(0, 0), (0, 1), (0, 2), (1, 0), (0, 3)]);
    }

    #[test]
    fn rows_before_the_first_time_stay_with_their_file() {
        let (_dir, logs) = open_all(&[
            "2026-01-01 10:00:00 a\n2026-01-01 10:00:05 a\n",
            "=== header ===\n2026-01-01 10:00:03 b\n",
        ]);
        assert_eq!(merged(&logs), vec![(0, 0), (1, 0), (1, 1), (0, 1)]);
    }

    #[test]
    fn ties_go_to_the_earlier_source_then_row() {
        let (_dir, logs) = open_all(&[
            "2026-01-01 10:00:00.000 b\n",
            "2026-01-01 10:00:00.000 a\n2026-01-01 10:00:00.000 a\n",
        ]);
        assert_eq!(merged(&logs), vec![(0, 0), (1, 0), (1, 1)]);
        let swapped = [&logs[1], &logs[0]];
        let order = merge_order(&swapped, &Control::default()).unwrap();
        let pairs: Vec<(u16, u32)> = order.iter().map(|row| (row.source, row.row)).collect();
        assert_eq!(pairs, vec![(0, 0), (0, 1), (1, 0)]);
    }

    #[test]
    fn each_file_keeps_its_order_when_its_time_goes_backwards() {
        let (_dir, logs) = open_all(&[
            "2026-01-01 10:00:05 a\n2026-01-01 10:00:01 clock went back\n2026-01-01 10:00:06 a\n",
            "2026-01-01 10:00:02 b\n2026-01-01 10:00:07 b\n",
        ]);
        assert_eq!(merged(&logs), vec![(1, 0), (0, 0), (0, 1), (0, 2), (1, 1)]);
    }

    #[test]
    fn empty_logs_merge_cleanly() {
        assert!(merge_order(&[], &Control::default()).unwrap().is_empty());
        let (_dir, logs) = open_all(&["", "2026-01-01 10:00:00 b\nmore\n", ""]);
        assert!(merged(&logs[..1]).is_empty());
        assert_eq!(merged(&logs), vec![(1, 0), (1, 1)]);
    }

    #[test]
    fn logs_with_no_times_keep_their_order_and_come_first() {
        let (_dir, logs) = open_all(&["x\ny\n", "2026-01-01 10:00:00 timed\n", "p\nq\n"]);
        assert_eq!(merged(&logs), vec![(0, 0), (0, 1), (2, 0), (2, 1), (1, 0)]);
    }

    #[test]
    fn one_log_keeps_its_own_order() {
        let (_dir, logs) = open_all(&[
            "header\n2026-01-01 10:00:05 a\n2026-01-01 10:00:01 b\nno time\n2026-01-01 10:00:03 c\n",
        ]);
        assert_eq!(
            merged(&logs),
            (0..5).map(|row| (0, row)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn merging_a_log_with_itself_alternates_its_rows() {
        let text: String = (0..300)
            .map(|n| format!("2026-01-01 10:00:{:02}.{:03} row\n", n / 10, n % 10 * 100))
            .collect();
        let (_dir, logs) = open_all(&[&text]);
        let order = merge_with_sizes(&[&logs[0], &logs[0]], &Control::default(), 7, 2).unwrap();
        let expected: Vec<MergedRow> = (0..300)
            .flat_map(|row| [0, 1].map(|source| MergedRow { source, row }))
            .collect();
        assert_eq!(order, expected);
    }

    #[test]
    fn cancelling_stops_the_merge() {
        let (_dir, logs) = open_all(&["2026-01-01 10:00:00 a\n"]);
        let control = Control::default();
        control.cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            merge_order(&[&logs[0]], &control).unwrap_err().code(),
            "cancelled"
        );
    }

    /// A row with a time at one of a few seconds and milliseconds, so ties
    /// and times going backwards are common, or with no time at all.
    fn any_row() -> impl Strategy<Value = Option<(u32, u16)>> {
        proptest::option::of((0u32..4, prop_oneof![Just(0u16), Just(250), Just(500)]))
    }

    fn log_text(rows: &[Option<(u32, u16)>]) -> String {
        rows.iter()
            .map(|row| match row {
                Some((second, millis)) => format!("2026-01-01 10:00:0{second}.{millis:03} row\n"),
                None => "no time\n".to_string(),
            })
            .collect()
    }

    proptest! {
        /// However the work is split into blocks and parts, the merge must
        /// match the rules applied in the simplest way.
        #[test]
        fn any_split_matches_a_simple_merge(
            files in proptest::collection::vec(proptest::collection::vec(any_row(), 0..40), 1..4),
            block_rows in 1usize..8,
            blocks_per_part in 1usize..4,
        ) {
            let texts: Vec<String> = files.iter().map(|rows| log_text(rows)).collect();
            let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
            let (_dir, logs) = open_all(&texts);
            let logs: Vec<&LogFile> = logs.iter().collect();
            let order = merge_with_sizes(&logs, &Control::default(), block_rows, blocks_per_part).unwrap();
            prop_assert_eq!(order, simple_merge(&logs));
        }
    }
}
