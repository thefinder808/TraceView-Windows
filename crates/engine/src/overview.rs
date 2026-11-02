//! Counts for the strip above the grid: rows of each severity for the chips,
//! and rows over time for the event histogram. They come from the index
//! alone, so no row text is read.
//!
//! Each count covers a whole log, or a sorted list of its rows such as a
//! filter's matches. Listed rows past the end of the log are ignored.
//!
//! Time works as it does in [`crate::merge`]. A row's place in time is the
//! latest time its log has reached by that row, so a row with no time, or one
//! whose time goes backwards, takes the time of the row before it, and rows
//! before the first time take that time. Places never decrease down a log, so
//! the rows in any stretch of time sit together in the grid. Times are whole
//! seconds since 2000-01-01, as the index keeps them, and
//! [`Stamp::label`](crate::time::Stamp::label) shows one.

use crate::file::LogFile;
use crate::query::{Control, QueryError, ROWS_PER_TASK};
use crate::summary::LevelCounts;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::ops::Range;
use std::sync::atomic::Ordering;

/// The most buckets a histogram has.
pub const MAX_BUCKETS: usize = 10_000;
/// A spike has more than this many times the rows of the median bucket that
/// has any, as in the Mac app.
const SPIKE_MULTIPLE: f64 = 3.0;
/// A spike has more rows than this, so a tiny log has none.
const SPIKE_FLOOR: f64 = 5.0;
/// The tallest spikes are kept, up to this many.
const MAX_REMEMBERED_SPIKES: usize = 100;

/// A stretch of time in whole seconds since 2000-01-01, including both ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeRange {
    pub first: u32,
    pub last: u32,
}

impl TimeRange {
    /// The range covering both, such as a merged tab's files.
    pub fn union(self, other: TimeRange) -> TimeRange {
        TimeRange {
            first: self.first.min(other.first),
            last: self.last.max(other.last),
        }
    }

    fn seconds(self) -> u64 {
        u64::from(self.last.saturating_sub(self.first)) + 1
    }
}

/// Rows over time for the event histogram: a range split into equal buckets,
/// with each row counted in the bucket holding its place.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Histogram {
    pub range: TimeRange,
    /// Earliest first.
    pub buckets: Vec<LevelCounts>,
}

impl Histogram {
    /// The first second of a bucket. A bucket runs up to the start of the
    /// next, so pass the bucket count for the end of the last one, which is a
    /// second after the range.
    pub fn bucket_start(&self, index: usize) -> u32 {
        let start = bucket_start(self.range, self.buckets.len(), index);
        u32::try_from(start).unwrap_or(u32::MAX)
    }

    /// The bucket holding a time, if the time is in range.
    pub fn bucket_of(&self, seconds: u32) -> Option<usize> {
        bucket_of(self.range, self.buckets.len(), seconds)
    }

    /// Adds another histogram over the same range and bucket count, such as
    /// another file's in a merged tab.
    ///
    /// # Panics
    ///
    /// If the range or bucket count differs.
    pub fn absorb(&mut self, other: &Histogram) {
        assert!(
            self.range == other.range && self.buckets.len() == other.buckets.len(),
            "only histograms with the same range and bucket count can be added"
        );
        for (bucket, counts) in self.buckets.iter_mut().zip(&other.buckets) {
            bucket.absorb(counts);
        }
    }
}

/// The bucket holding `seconds`, when `count` buckets split `range`.
fn bucket_of(range: TimeRange, count: usize, seconds: u32) -> Option<usize> {
    let offset = u64::from(seconds.checked_sub(range.first)?);
    let total = range.seconds();
    (offset < total && count > 0).then(|| (offset * count as u64 / total) as usize)
}

/// The first second that [`bucket_of`] puts in a bucket.
fn bucket_start(range: TimeRange, count: usize, index: usize) -> u64 {
    let offset = (index as u64)
        .saturating_mul(range.seconds())
        .div_ceil(count.max(1) as u64);
    u64::from(range.first) + offset
}

/// Spikes remembered across recounts, as the Mac app does, so the histogram
/// keeps showing how tall a spike was. When live tail adds rows, the range
/// grows and every bucket moves, which can split a spike between two buckets
/// and halve it. Keep one per histogram, and start a new one when a log is
/// replaced and read again from the start, or a tab shows different files.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpikeMemory {
    /// The middle second of each spike's bucket, and its rows.
    peaks: Vec<(u32, u64)>,
}

impl SpikeMemory {
    /// Remembers the spikes in a histogram that's about to be replaced. A
    /// spike is a bucket with more than 5 rows and more than three times the
    /// median of the buckets that have any. The 100 tallest are kept.
    pub fn remember(&mut self, histogram: &Histogram) {
        let mut totals: Vec<u64> = histogram
            .buckets
            .iter()
            .map(LevelCounts::total)
            .filter(|total| *total > 0)
            .collect();
        if totals.is_empty() {
            return;
        }
        totals.sort_unstable();
        let middle = totals.len() / 2;
        let median = if totals.len().is_multiple_of(2) {
            (totals[middle - 1] as f64 + totals[middle] as f64) / 2.0
        } else {
            totals[middle] as f64
        };
        let threshold = (median * SPIKE_MULTIPLE).max(SPIKE_FLOOR);
        for (index, bucket) in histogram.buckets.iter().enumerate() {
            let rows = bucket.total();
            if rows as f64 > threshold {
                let start = histogram.bucket_start(index);
                let end = histogram.bucket_start(index + 1);
                self.peaks.push((start + (end - start) / 2, rows));
            }
        }
        if self.peaks.len() > MAX_REMEMBERED_SPIKES {
            self.peaks.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            self.peaks.truncate(MAX_REMEMBERED_SPIKES);
        }
    }

    /// For each bucket, the tallest remembered spike in it, where that's
    /// taller than the bucket is now, or zero. The GUI draws these as faint
    /// shadows behind the bars, and scales the bars to fit them.
    pub fn shadows(&self, histogram: &Histogram) -> Vec<u64> {
        let mut shadows = vec![0; histogram.buckets.len()];
        for (seconds, rows) in &self.peaks {
            if let Some(index) = histogram.bucket_of(*seconds) {
                shadows[index] = shadows[index].max(*rows);
            }
        }
        for (shadow, bucket) in shadows.iter_mut().zip(&histogram.buckets) {
            if *shadow <= bucket.total() {
                *shadow = 0;
            }
        }
        shadows
    }
}

fn task_span(rows: usize, task_rows: usize, task: usize) -> Range<usize> {
    let start = (task * task_rows).min(rows);
    start..(start + task_rows).min(rows)
}

/// The latest time reached once `row` is read.
fn latest_with(log: &LogFile, row: usize, latest: u32) -> u32 {
    log.stamp_seconds(row)
        .map_or(latest, |seconds| seconds.max(latest))
}

/// The latest time a log has reached at the start of each task, so a row's
/// place can be found by reading only the rows before it in its task.
struct Places {
    task_rows: usize,
    /// The latest time before each task's first row, then the latest time in
    /// the whole log. It never decreases.
    reached: Vec<u32>,
}

impl Places {
    /// Reads every row's time in parallel tasks. Returns `None` if no row has
    /// one. `control.rows_done` counts the rows read.
    fn find(
        log: &LogFile,
        task_rows: usize,
        control: &Control,
    ) -> Result<Option<Places>, QueryError> {
        let rows = log.rows();
        // Each task's first time and latest time, if it has any.
        let extents: Vec<Option<(u32, u32)>> = (0..rows.div_ceil(task_rows))
            .into_par_iter()
            .map(|task| {
                if control.cancel.load(Ordering::Relaxed) {
                    return Err(QueryError::Cancelled);
                }
                let span = task_span(rows, task_rows, task);
                control.rows_done.fetch_add(span.len(), Ordering::Relaxed);
                let mut times = span.filter_map(|row| log.stamp_seconds(row));
                Ok(times
                    .next()
                    .map(|first| (first, times.fold(first, u32::max))))
            })
            .collect::<Result<_, QueryError>>()?;

        let Some(first) = extents.iter().flatten().next().map(|(first, _)| *first) else {
            return Ok(None);
        };
        let mut latest = first;
        let mut reached = Vec::with_capacity(extents.len() + 1);
        reached.push(first);
        for extent in &extents {
            if let Some((_, task_latest)) = extent {
                latest = latest.max(*task_latest);
            }
            reached.push(latest);
        }
        Ok(Some(Places { task_rows, reached }))
    }

    /// A row's place.
    fn of(&self, log: &LogFile, row: usize) -> u32 {
        let task = row / self.task_rows;
        (task * self.task_rows..=row).fold(self.reached[task], |latest, row| {
            latest_with(log, row, latest)
        })
    }

    /// The first row placed at or after `seconds`, or the row count if none is.
    fn seek(&self, log: &LogFile, seconds: u32) -> usize {
        if self.reached[0] >= seconds {
            return 0;
        }
        let task = self.reached[1..].partition_point(|latest| *latest < seconds);
        let mut latest = self.reached[task];
        for row in task_span(log.rows(), self.task_rows, task) {
            latest = latest_with(log, row, latest);
            if latest >= seconds {
                return row;
            }
        }
        log.rows()
    }
}

/// One task's share of the rows being counted: a span of the log's rows, and
/// which of them are listed, or `None` for all of them.
struct Part<'a> {
    task: usize,
    span: Range<usize>,
    listed: Option<&'a [u32]>,
}

impl Part<'_> {
    fn each_row(&self, visit: impl FnMut(usize)) {
        match self.listed {
            None => self.span.clone().for_each(visit),
            // The check only matters if the list isn't sorted, and keeps every
            // row read within the log.
            Some(listed) => listed
                .iter()
                .map(|row| *row as usize)
                .filter(|row| self.span.contains(row))
                .for_each(visit),
        }
    }

    /// Calls `visit` with each row and its place, given the latest time
    /// reached before the span.
    fn each_place(&self, log: &LogFile, mut latest: u32, mut visit: impl FnMut(usize, u32)) {
        if self.listed.is_none() {
            // Counting every row is the common case, and a plain loop for it
            // measured about a fifth faster.
            for row in self.span.clone() {
                latest = latest_with(log, row, latest);
                visit(row, latest);
            }
            return;
        }
        let mut next = self.span.start;
        self.each_row(|row| {
            while next <= row {
                latest = latest_with(log, next, latest);
                next += 1;
            }
            visit(row, latest);
        });
    }
}

/// Splits the rows to count into tasks of the log's rows, so they're counted
/// in parallel and each task knows where its rows start in time.
fn parts(rows: usize, listed: Option<&[u32]>, task_rows: usize) -> Vec<Part<'_>> {
    (0..rows.div_ceil(task_rows))
        .map(|task| {
            let span = task_span(rows, task_rows, task);
            let listed = listed.map(|listed| {
                let start = listed.partition_point(|row| (*row as usize) < span.start);
                let end = listed.partition_point(|row| (*row as usize) < span.end);
                &listed[start..end.max(start)]
            });
            Part { task, span, listed }
        })
        .collect()
}

/// Counts the rows of each severity in a log, or in the listed rows, using
/// all cores.
pub fn level_counts(
    log: &LogFile,
    rows: Option<&[u32]>,
    control: &Control,
) -> Result<LevelCounts, QueryError> {
    level_counts_with(log, rows, ROWS_PER_TASK, control)
}

fn level_counts_with(
    log: &LogFile,
    rows: Option<&[u32]>,
    task_rows: usize,
    control: &Control,
) -> Result<LevelCounts, QueryError> {
    parts(log.rows(), rows, task_rows)
        .par_iter()
        .map(|part| {
            if control.cancel.load(Ordering::Relaxed) {
                return Err(QueryError::Cancelled);
            }
            let mut counts = LevelCounts::default();
            part.each_row(|row| counts.add(log.level(row)));
            control
                .rows_done
                .fetch_add(part.span.len(), Ordering::Relaxed);
            Ok(counts)
        })
        .try_reduce(LevelCounts::default, |mut total, counts| {
            total.absorb(&counts);
            Ok(total)
        })
}

/// Counts the rows of each severity in `rows`, on this thread, which suits
/// the few rows a live refresh adds. The range is clamped to the rows the log
/// has. A refresh reads the last row again, as it may still have been
/// growing, so its level can change: keep counts up to the last row and add
/// the last row separately.
pub fn level_counts_in(log: &LogFile, rows: Range<usize>) -> LevelCounts {
    let end = rows.end.min(log.rows());
    let mut counts = LevelCounts::default();
    for row in rows.start.min(end)..end {
        counts.add(log.level(row));
    }
    counts
}

/// The time a log, or the listed rows, cover: from the first row's place to
/// the last row's. `None` if no row has a time, or no row is listed.
pub fn time_range(
    log: &LogFile,
    rows: Option<&[u32]>,
    control: &Control,
) -> Result<Option<TimeRange>, QueryError> {
    time_range_with(log, rows, ROWS_PER_TASK, control)
}

fn time_range_with(
    log: &LogFile,
    rows: Option<&[u32]>,
    task_rows: usize,
    control: &Control,
) -> Result<Option<TimeRange>, QueryError> {
    let Some(places) = Places::find(log, task_rows, control)? else {
        return Ok(None);
    };
    let (first, last) = match rows {
        // A log with times has rows.
        None => (0, log.rows() - 1),
        Some(rows) => {
            let in_log = |row: &&u32| (**row as usize) < log.rows();
            match (rows.iter().find(in_log), rows.iter().rev().find(in_log)) {
                (Some(first), Some(last)) => (*first as usize, *last as usize),
                _ => return Ok(None),
            }
        }
    };
    Ok(Some(TimeRange {
        first: places.of(log, first),
        last: places.of(log, last),
    }))
}

/// Counts rows over time, using all cores: `range` split into `buckets` equal
/// buckets, with each row of the log, or each listed row, counted in the
/// bucket holding its place. Rows placed outside the range aren't counted.
///
/// A bucket is at least a second, as the index keeps whole seconds, so a short
/// range has fewer buckets than asked for, and there are at most
/// [`MAX_BUCKETS`]. The count depends only on the range and `buckets`, so a
/// merged tab can count each file over one shared range and add them with
/// [`Histogram::absorb`].
pub fn histogram(
    log: &LogFile,
    rows: Option<&[u32]>,
    range: TimeRange,
    buckets: usize,
    control: &Control,
) -> Result<Histogram, QueryError> {
    histogram_with(log, rows, range, buckets, ROWS_PER_TASK, control)
}

fn histogram_with(
    log: &LogFile,
    rows: Option<&[u32]>,
    range: TimeRange,
    buckets: usize,
    task_rows: usize,
    control: &Control,
) -> Result<Histogram, QueryError> {
    let range = TimeRange {
        first: range.first.min(range.last),
        last: range.first.max(range.last),
    };
    let count = (buckets.clamp(1, MAX_BUCKETS) as u64).min(range.seconds()) as usize;
    let empty = vec![LevelCounts::default(); count];
    let Some(places) = Places::find(log, task_rows, control)? else {
        return Ok(Histogram {
            range,
            buckets: empty,
        });
    };
    let buckets = parts(log.rows(), rows, task_rows)
        .par_iter()
        .try_fold(
            || empty.clone(),
            |mut buckets, part| {
                if control.cancel.load(Ordering::Relaxed) {
                    return Err(QueryError::Cancelled);
                }
                // Rows seldom leave the bucket of the row before them, so the
                // current bucket's seconds are kept to skip most divisions.
                let (mut index, mut seconds) = (0, 0..0);
                part.each_place(log, places.reached[part.task], |row, place| {
                    if !seconds.contains(&u64::from(place)) {
                        let Some(found) = bucket_of(range, count, place) else {
                            return;
                        };
                        index = found;
                        seconds = bucket_start(range, count, index)
                            ..bucket_start(range, count, index + 1);
                    }
                    buckets[index].add(log.level(row));
                });
                Ok(buckets)
            },
        )
        .try_reduce(
            || empty.clone(),
            |mut total, buckets| {
                for (bucket, counts) in total.iter_mut().zip(&buckets) {
                    bucket.absorb(counts);
                }
                Ok(total)
            },
        )?;
    Ok(Histogram { range, buckets })
}

/// Finds the first row placed at or after `seconds`, for jumping to a time
/// such as a histogram bucket's start. Returns its position in the listed
/// rows, or its row number when `rows` is `None`. If every row is placed
/// earlier, returns the number of rows.
///
/// The result is also how many rows are placed earlier, so in a merged tab,
/// adding it up across the files gives the position in the merged timeline.
/// A log with no times counts as entirely earlier, as merging puts it first.
pub fn first_row_at(
    log: &LogFile,
    rows: Option<&[u32]>,
    seconds: u32,
    control: &Control,
) -> Result<usize, QueryError> {
    first_row_at_with(log, rows, seconds, ROWS_PER_TASK, control)
}

fn first_row_at_with(
    log: &LogFile,
    rows: Option<&[u32]>,
    seconds: u32,
    task_rows: usize,
    control: &Control,
) -> Result<usize, QueryError> {
    let row = match Places::find(log, task_rows, control)? {
        Some(places) => places.seek(log, seconds),
        None => log.rows(),
    };
    Ok(match rows {
        Some(rows) => rows.partition_point(|listed| (*listed as usize) < row),
        None => row,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::Level;
    use crate::merge;
    use crate::time::Stamp;
    use proptest::prelude::*;
    use std::sync::atomic::AtomicU64;

    fn open(dir: &tempfile::TempDir, name: &str, text: &str) -> LogFile {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        LogFile::open_with_cache(&path, None, &AtomicU64::new(0))
            .unwrap()
            .0
    }

    fn open_one(text: &str) -> (tempfile::TempDir, LogFile) {
        let dir = tempfile::tempdir().unwrap();
        let log = open(&dir, "a.log", text);
        (dir, log)
    }

    /// Index seconds for a moment that many seconds after 10:00 on the day
    /// the test logs use.
    fn at(seconds: u32) -> u32 {
        Stamp::from_parts(2026, 1, 1, 10, 0, 0, 0).unwrap().seconds + seconds
    }

    /// A row at that many seconds after 10:00.
    fn line(seconds: u32, level: &str) -> String {
        let (minute, second) = (seconds / 60, seconds % 60);
        format!("2026-01-01 10:{minute:02}:{second:02} [{level}] row\n")
    }

    fn totals(histogram: &Histogram) -> Vec<u64> {
        histogram.buckets.iter().map(LevelCounts::total).collect()
    }

    /// A histogram with the given rows per bucket, all of them info.
    fn made(range: TimeRange, totals: &[u64]) -> Histogram {
        Histogram {
            range,
            buckets: totals
                .iter()
                .map(|info| LevelCounts {
                    info: *info,
                    ..LevelCounts::default()
                })
                .collect(),
        }
    }

    #[test]
    fn counts_rows_of_each_level() {
        let levels = [
            "ERROR", "WARN", "INFO", "DEBUG", "CRITICAL", "NOTICE", "ERROR",
        ];
        let text: String = (0..).zip(levels).map(|(n, level)| line(n, level)).collect();
        let (_dir, log) = open_one(&text);
        let counts = level_counts(&log, None, &Control::default()).unwrap();
        let expected = LevelCounts {
            debug: 1,
            info: 1,
            notice: 1,
            warning: 1,
            error: 2,
            critical: 1,
        };
        assert_eq!(counts, expected);
        assert_eq!(counts.total(), 7);
        assert_eq!(counts.get(Level::Critical), 1);
    }

    #[test]
    fn counts_listed_rows_and_row_ranges() {
        let levels = [
            "ERROR", "WARN", "INFO", "DEBUG", "CRITICAL", "NOTICE", "ERROR",
        ];
        let text: String = (0..).zip(levels).map(|(n, level)| line(n, level)).collect();
        let (_dir, log) = open_one(&text);
        let control = Control::default();
        let listed = level_counts(&log, Some(&[0, 4, 6]), &control).unwrap();
        assert_eq!((listed.error, listed.critical, listed.total()), (2, 1, 3));
        let past_the_end = level_counts(&log, Some(&[6, 7, 1000]), &control).unwrap();
        assert_eq!((past_the_end.error, past_the_end.total()), (1, 1));
        assert_eq!(level_counts(&log, Some(&[]), &control).unwrap().total(), 0);

        // Live tail adds the counts of new rows to what it had.
        let mut counts = level_counts_in(&log, 0..3);
        assert_eq!((counts.error, counts.warning, counts.info), (1, 1, 1));
        counts.absorb(&level_counts_in(&log, 3..1000));
        assert_eq!(counts, level_counts(&log, None, &control).unwrap());
        let backwards = Range { start: 5, end: 2 };
        assert_eq!(level_counts_in(&log, backwards).total(), 0);
    }

    #[test]
    fn the_range_runs_from_the_first_place_to_the_latest() {
        let (_dir, log) = open_one(&format!(
            "header\n{}{}no time\n{}",
            line(5, "INFO"),
            line(1, "INFO"),
            line(9, "INFO")
        ));
        let control = Control::default();
        let range = |rows: Option<&[u32]>| time_range(&log, rows, &control).unwrap();
        let span = |first, last| {
            Some(TimeRange {
                first: at(first),
                last: at(last),
            })
        };
        assert_eq!(range(None), span(5, 9));
        // The header takes the first time, and the rows after one whose time
        // went back keep the latest time.
        assert_eq!(range(Some(&[0])), span(5, 5));
        assert_eq!(range(Some(&[2, 3])), span(5, 5));
        assert_eq!(range(Some(&[1, 4])), span(5, 9));
        assert_eq!(range(Some(&[2, 99])), span(5, 5));
        assert_eq!(range(Some(&[])), None);
        assert_eq!(range(Some(&[99])), None);

        let (_dir, untimed) = open_one("no\ntimes\n");
        assert_eq!(time_range(&untimed, None, &control).unwrap(), None);
        let (_dir, empty) = open_one("");
        assert_eq!(time_range(&empty, None, &control).unwrap(), None);
    }

    #[test]
    fn rows_without_a_time_count_with_the_row_before_them() {
        let (_dir, log) = open_one(&format!(
            "header\n{}  continued: it failed\n  more\n{}",
            line(0, "INFO"),
            line(9, "INFO")
        ));
        let control = Control::default();
        let range = time_range(&log, None, &control).unwrap().unwrap();
        let histogram = histogram(&log, None, range, 10, &control).unwrap();
        assert_eq!(totals(&histogram), [4, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(histogram.buckets[0].error, 1);
    }

    #[test]
    fn rows_whose_time_goes_backwards_keep_the_latest_time() {
        let (_dir, log) = open_one(&[line(5, "INFO"), line(1, "ERROR"), line(6, "INFO")].concat());
        let control = Control::default();
        let range = TimeRange {
            first: at(0),
            last: at(9),
        };
        let histogram = histogram(&log, None, range, 10, &control).unwrap();
        assert_eq!(totals(&histogram), [0, 0, 0, 0, 0, 2, 1, 0, 0, 0]);
        assert_eq!(histogram.buckets[5].error, 1);
        let listed = super::histogram(&log, Some(&[1]), range, 10, &control).unwrap();
        assert_eq!(listed.buckets[5].error, 1);
        assert_eq!(
            listed.buckets.iter().map(LevelCounts::total).sum::<u64>(),
            1
        );
    }

    #[test]
    fn the_first_and_last_seconds_land_in_buckets() {
        let text: String = (0..100).map(|n| line(n, "INFO")).collect();
        let (_dir, log) = open_one(&text);
        let control = Control::default();
        let range = time_range(&log, None, &control).unwrap().unwrap();
        assert_eq!((range.first, range.last), (at(0), at(99)));

        // 100 seconds don't split evenly into 7 buckets.
        let histogram = histogram(&log, None, range, 7, &control).unwrap();
        assert_eq!(histogram.buckets.len(), 7);
        assert_eq!(histogram.bucket_of(range.first), Some(0));
        assert_eq!(histogram.bucket_of(range.last), Some(6));
        assert_eq!(histogram.bucket_of(range.first - 1), None);
        assert_eq!(histogram.bucket_of(range.last + 1), None);
        assert_eq!(histogram.bucket_start(0), range.first);
        assert_eq!(histogram.bucket_start(7), range.last + 1);
        for index in 0..7 {
            let (start, end) = (
                histogram.bucket_start(index),
                histogram.bucket_start(index + 1),
            );
            assert!(end - start >= 14, "bucket {index}");
            assert_eq!(histogram.bucket_of(start), Some(index));
            assert_eq!(histogram.bucket_of(end - 1), Some(index));
            // One row a second, so each bucket holds one row per second it covers.
            assert_eq!(histogram.buckets[index].total(), u64::from(end - start));
        }

        // Buckets are never shorter than a second.
        let fine = super::histogram(&log, None, range, 1000, &control).unwrap();
        assert_eq!(totals(&fine), vec![1; 100]);
        let capped = super::histogram(&log, None, range, usize::MAX, &control).unwrap();
        assert_eq!(capped.buckets.len(), 100);
        let none_asked = super::histogram(&log, None, range, 0, &control).unwrap();
        assert_eq!(totals(&none_asked), [100]);
    }

    #[test]
    fn a_single_instant_fills_one_bucket() {
        let (_dir, log) = open_one(&format!("{}{}more\n", line(3, "INFO"), line(3, "ERROR")));
        let control = Control::default();
        let range = time_range(&log, None, &control).unwrap().unwrap();
        assert_eq!(range.first, range.last);
        let histogram = histogram(&log, None, range, 1000, &control).unwrap();
        assert_eq!(totals(&histogram), [3]);
        assert_eq!(histogram.bucket_of(at(3)), Some(0));
        assert_eq!(histogram.bucket_start(1), at(4));
    }

    #[test]
    fn a_range_given_backwards_is_turned_around() {
        let (_dir, log) = open_one(&[line(0, "INFO"), line(9, "INFO")].concat());
        let backwards = TimeRange {
            first: at(9),
            last: at(0),
        };
        let histogram = histogram(&log, None, backwards, 10, &Control::default()).unwrap();
        assert_eq!(histogram.range.first, at(0));
        assert_eq!(totals(&histogram), [1, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    }

    #[test]
    fn histograms_of_two_logs_add_up_over_a_shared_range() {
        let dir = tempfile::tempdir().unwrap();
        let first = open(&dir, "a.log", &[line(0, "ERROR"), line(4, "INFO")].concat());
        let second = open(&dir, "b.log", &[line(2, "WARN"), line(9, "INFO")].concat());
        let untimed = open(&dir, "c.log", "no times\n");
        let control = Control::default();
        let range = time_range(&first, None, &control)
            .unwrap()
            .unwrap()
            .union(time_range(&second, None, &control).unwrap().unwrap());
        assert_eq!((range.first, range.last), (at(0), at(9)));

        let mut merged = histogram(&first, None, range, 5, &control).unwrap();
        merged.absorb(&histogram(&second, None, range, 5, &control).unwrap());
        merged.absorb(&histogram(&untimed, None, range, 5, &control).unwrap());
        assert_eq!(totals(&merged), [1, 1, 1, 0, 1]);
        assert_eq!((merged.buckets[0].error, merged.buckets[1].warning), (1, 1));
    }

    #[test]
    #[should_panic(expected = "same range and bucket count")]
    fn histograms_of_different_shapes_cannot_be_added() {
        let range = TimeRange { first: 0, last: 99 };
        made(range, &[1, 2]).absorb(&made(range, &[1, 2, 3]));
    }

    #[test]
    fn jumps_to_the_first_row_at_or_after_a_time() {
        // Places by row: 2, 2, 2, 5, 5, 8.
        let (_dir, log) = open_one(&format!(
            "header\n{}  continued\n{}{}{}",
            line(2, "INFO"),
            line(5, "INFO"),
            line(3, "INFO"),
            line(8, "INFO")
        ));
        let control = Control::default();
        let jump = |rows: Option<&[u32]>, seconds| {
            first_row_at(&log, rows, at(seconds), &control).unwrap()
        };
        let whole: Vec<usize> = (0..10).map(|seconds| jump(None, seconds)).collect();
        assert_eq!(whole, [0, 0, 0, 3, 3, 3, 5, 5, 5, 6]);
        let listed: &[u32] = &[2, 4, 5];
        let positions: Vec<usize> = (0..10).map(|seconds| jump(Some(listed), seconds)).collect();
        assert_eq!(positions, [0, 0, 0, 1, 1, 1, 2, 2, 2, 3]);
        assert_eq!(jump(Some(&[]), 5), 0);

        let (_dir, untimed) = open_one("no\ntimes\n");
        assert_eq!(first_row_at(&untimed, None, at(0), &control).unwrap(), 2);
        assert_eq!(
            first_row_at(&untimed, Some(&[1]), at(0), &control).unwrap(),
            1
        );
    }

    /// Each row's place, worked out one row at a time.
    fn simple_places(log: &LogFile) -> Vec<Option<u32>> {
        let mut latest = (0..log.rows()).find_map(|row| log.stamp_seconds(row));
        (0..log.rows())
            .map(|row| {
                latest = latest.map(|latest| match log.stamp_seconds(row) {
                    Some(seconds) if seconds > latest => seconds,
                    _ => latest,
                });
                latest
            })
            .collect()
    }

    #[test]
    fn jumps_in_a_merged_tab_add_up_across_its_files() {
        let dir = tempfile::tempdir().unwrap();
        let logs = [
            open(
                &dir,
                "a.log",
                &format!(
                    "header\n{}  more\n{}{}",
                    line(2, "INFO"),
                    line(6, "INFO"),
                    line(4, "INFO")
                ),
            ),
            open(
                &dir,
                "b.log",
                &[line(1, "INFO"), line(5, "INFO"), line(9, "INFO")].concat(),
            ),
            open(&dir, "c.log", "no times\nat all\n"),
        ];
        let control = Control::default();
        let sources: Vec<&LogFile> = logs.iter().collect();
        let order = merge::merge_order(&sources, &control).unwrap();
        let places: Vec<Vec<Option<u32>>> = logs.iter().map(simple_places).collect();
        let place =
            |merged: &merge::MergedRow| places[usize::from(merged.source)][merged.row as usize];
        for seconds in 0..12 {
            let position: usize = logs
                .iter()
                .map(|log| first_row_at(log, None, at(seconds), &control).unwrap())
                .sum();
            assert!(
                order[..position]
                    .iter()
                    .all(|merged| place(merged).is_none_or(|place| place < at(seconds))),
                "{seconds}"
            );
            assert!(
                order[position..]
                    .iter()
                    .all(|merged| place(merged).is_some_and(|place| place >= at(seconds))),
                "{seconds}"
            );
        }
    }

    #[test]
    fn remembered_spikes_show_where_a_recount_shrank_them() {
        let range = TimeRange { first: 0, last: 99 };
        let before = made(range, &[1, 1, 1, 1, 40, 1, 1, 1, 1, 1]);
        let mut memory = SpikeMemory::default();
        memory.remember(&before);

        // The range doubled, and the spike, centered on second 45, was split.
        let wider = TimeRange {
            first: 0,
            last: 199,
        };
        let split = made(wider, &[2, 2, 25, 17, 2, 2, 2, 2, 2, 2]);
        assert_eq!(memory.shadows(&split), [0, 0, 40, 0, 0, 0, 0, 0, 0, 0]);
        // Nothing shows where the bucket is already as tall.
        let whole = made(wider, &[2, 2, 42, 2, 2, 2, 2, 2, 2, 2]);
        assert_eq!(memory.shadows(&whole), vec![0; 10]);
    }

    #[test]
    fn only_clear_spikes_are_remembered() {
        let range = TimeRange { first: 0, last: 9 };
        let mut memory = SpikeMemory::default();
        // The median is 1, so a spike needs more than 5 rows.
        memory.remember(&made(range, &[1, 1, 1, 5, 1, 0, 0, 0, 0, 0]));
        // The median is 3, halfway between 2 and 4, so a spike needs more than 9.
        memory.remember(&made(range, &[2, 9, 4, 2, 0, 0, 0, 0, 0, 0]));
        memory.remember(&made(range, &[]));
        assert!(memory.peaks.is_empty());
        memory.remember(&made(range, &[2, 10, 4, 2, 0, 0, 0, 0, 0, 0]));
        assert_eq!(memory.peaks, [(1, 10)]);
    }

    #[test]
    fn only_the_tallest_spikes_are_remembered() {
        let range = TimeRange {
            first: 0,
            last: 299,
        };
        let mut memory = SpikeMemory::default();
        for round in 0..30 {
            let mut totals = vec![1; 300];
            for (bucket, total) in totals.iter_mut().enumerate().skip(round).step_by(30) {
                *total = 10 + bucket as u64;
            }
            memory.remember(&made(range, &totals));
        }
        assert_eq!(memory.peaks.len(), MAX_REMEMBERED_SPIKES);
        assert_eq!(memory.peaks[0], (299, 309));
        assert!(memory.peaks.iter().all(|(_, rows)| *rows >= 210));
    }

    #[test]
    fn cancelling_stops_every_count() {
        let (_dir, log) = open_one(&line(0, "INFO"));
        let control = Control::default();
        control.cancel.store(true, Ordering::Relaxed);
        let range = TimeRange {
            first: at(0),
            last: at(0),
        };
        let codes = [
            level_counts(&log, None, &control).map(|_| ()),
            time_range(&log, None, &control).map(|_| ()),
            histogram(&log, None, range, 10, &control).map(|_| ()),
            first_row_at(&log, None, at(0), &control).map(|_| ()),
        ]
        .map(|result| result.unwrap_err().code());
        assert_eq!(codes, ["cancelled"; 4]);
    }

    #[test]
    fn progress_reaches_the_row_count() {
        let text: String = (0..50).map(|n| line(n, "INFO")).collect();
        let (_dir, log) = open_one(&text);
        let control = Control::default();
        let range = time_range_with(&log, None, 7, &control).unwrap().unwrap();
        assert_eq!(control.rows_done.load(Ordering::Relaxed), 50);
        histogram_with(&log, Some(&[3]), range, 10, 7, &control).unwrap();
        assert_eq!(control.rows_done.load(Ordering::Relaxed), 100);
    }

    #[test]
    fn results_serialize_to_json() {
        let (_dir, log) = open_one(&[line(0, "ERROR"), line(5, "INFO")].concat());
        let control = Control::default();
        let range = time_range(&log, None, &control).unwrap().unwrap();
        let histogram = histogram(&log, None, range, 3, &control).unwrap();
        let json = serde_json::to_string(&histogram).unwrap();
        assert_eq!(serde_json::from_str::<Histogram>(&json).unwrap(), histogram);
        let mut memory = SpikeMemory::default();
        memory.remember(&made(range, &[1, 1, 1, 30]));
        let json = serde_json::to_string(&memory).unwrap();
        assert_eq!(serde_json::from_str::<SpikeMemory>(&json).unwrap(), memory);
    }

    const LEVELS: [&str; 6] = ["DEBUG", "INFO", "NOTICE", "WARN", "ERROR", "CRITICAL"];

    /// A row at one of a few seconds, so ties and times going backwards are
    /// common, or with no time at all.
    fn any_row() -> impl Strategy<Value = (Option<u32>, usize)> {
        (proptest::option::of(0u32..6), 0..LEVELS.len())
    }

    fn log_text(rows: &[(Option<u32>, usize)]) -> String {
        rows.iter()
            .map(|(time, level)| match time {
                Some(seconds) => line(*seconds, LEVELS[*level]),
                None => format!("[{}] no time\n", LEVELS[*level]),
            })
            .collect()
    }

    proptest! {
        /// However the rows are split into tasks, every count must match the
        /// same count done one row at a time.
        #[test]
        fn any_split_matches_a_simple_count(
            rows in proptest::collection::vec(any_row(), 0..40),
            listed in proptest::option::of(proptest::collection::vec(any::<bool>(), 40)),
            task_rows in 1usize..8,
            buckets in 1usize..12,
            ends in (0u32..8, 0u32..8),
            jump in 0u32..8,
        ) {
            let (_dir, log) = open_one(&log_text(&rows));
            let control = Control::default();
            let places = simple_places(&log);
            let listed: Option<Vec<u32>> = listed.map(|keep| {
                (0..log.rows() as u32).filter(|row| keep[*row as usize]).collect()
            });
            let chosen: Vec<usize> = match &listed {
                Some(listed) => listed.iter().map(|row| *row as usize).collect(),
                None => (0..log.rows()).collect(),
            };
            let listed = listed.as_deref();

            let mut counts = LevelCounts::default();
            for row in &chosen {
                counts.add(log.level(*row));
            }
            prop_assert_eq!(level_counts_with(&log, listed, task_rows, &control).unwrap(), counts);

            let chosen_places: Vec<u32> = chosen.iter().filter_map(|row| places[*row]).collect();
            let range = chosen_places.iter().min().map(|first| TimeRange {
                first: *first,
                last: *chosen_places.iter().max().unwrap(),
            });
            prop_assert_eq!(time_range_with(&log, listed, task_rows, &control).unwrap(), range);

            let range = TimeRange { first: at(ends.0), last: at(ends.1) };
            let histogram = histogram_with(&log, listed, range, buckets, task_rows, &control).unwrap();
            let seconds = u64::from(ends.0.abs_diff(ends.1)) + 1;
            prop_assert_eq!(histogram.buckets.len() as u64, (buckets as u64).min(seconds));
            let mut expected = vec![LevelCounts::default(); histogram.buckets.len()];
            for row in &chosen {
                if let Some(index) = places[*row].and_then(|place| histogram.bucket_of(place)) {
                    expected[index].add(log.level(*row));
                }
            }
            prop_assert_eq!(histogram.buckets, expected);

            let first = chosen
                .iter()
                .position(|row| places[*row].is_some_and(|place| place >= at(jump)))
                .unwrap_or(chosen.len());
            prop_assert_eq!(first_row_at_with(&log, listed, at(jump), task_rows, &control).unwrap(), first);
        }
    }
}
