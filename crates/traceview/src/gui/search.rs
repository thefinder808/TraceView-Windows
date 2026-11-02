//! The filter and find boxes. Both take the same short syntax, and both run on
//! background threads that can be cancelled when the text changes.
//!
//! ```text
//! failed level:warning component:PolicyAgent from:2026-01-16 to:2026-01-17
//! /error 0x[0-9a-f]{8}/ thread:4128 "two words"
//! ```

use super::row_cache::SharedLog;
use std::ops::Range;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Instant;
use tv_engine::merge::MergedRow;
use tv_engine::query::{self, Matcher, Query};
use tv_engine::{Control, Level};
use tv_platform::Waker;

/// Splits text into words, keeping "quoted phrases" together. Each word says
/// whether it can be a key, which a quote before its colon rules out, so
/// "level:error" searches for that text.
fn words(text: &str) -> Vec<(String, bool)> {
    let mut words = Vec::new();
    let mut current = String::new();
    let (mut quoted, mut can_be_key) = (false, true);
    for character in text.chars() {
        match character {
            '"' => {
                quoted = !quoted;
                can_be_key &= current.contains(':');
            }
            ' ' | '\t' if !quoted => {
                if !current.is_empty() {
                    words.push((std::mem::take(&mut current), can_be_key));
                }
                can_be_key = true;
            }
            _ => current.push(character),
        }
    }
    if !current.is_empty() {
        words.push((current, can_be_key));
    }
    words
}

/// Reads the filter syntax into an engine query.
pub fn parse(text: &str) -> Result<Query, String> {
    let mut query = Query::default();
    let mut free_text = Vec::new();
    for (word, can_be_key) in words(text) {
        let Some((key, value)) = word.split_once(':').filter(|_| can_be_key) else {
            free_text.push(word);
            continue;
        };
        match key.to_ascii_lowercase().as_str() {
            "level" | "lvl" => {
                let level = Level::from_name(value.as_bytes()).ok_or_else(|| {
                    format!("Unknown level \"{value}\". Use debug, info, notice, warning, error or critical")
                })?;
                query.min_level = Some(level);
            }
            "component" | "comp" => query.component = Some(value.to_string()),
            "thread" => query.thread = Some(value.to_string()),
            "from" | "after" => query.from = Some(value.to_string()),
            "to" | "before" => query.to = Some(value.to_string()),
            // Anything else with a colon, like a URL or "error:", is just text.
            _ => free_text.push(word),
        }
    }
    let joined = free_text.join(" ");
    if joined.len() > 2 && joined.starts_with('/') && joined.ends_with('/') {
        query.text = Some(joined[1..joined.len() - 1].to_string());
        query.regex = true;
    } else if !joined.is_empty() {
        query.text = Some(joined);
    }
    // Checking now means a typo is reported at once, not after a scan starts.
    Matcher::new(&query).map_err(|error| error.to_string())?;
    Ok(query)
}

/// The rows a search found, and the rows it looked at.
struct Found {
    searched: Range<usize>,
    rows: Vec<u32>,
}

/// What a search runs over.
#[derive(Clone)]
pub enum Target {
    Log(SharedLog),
    /// A merged tab: its files, and their rows in time order. Matches are
    /// positions in that order.
    Merged(Vec<SharedLog>, Arc<Vec<MergedRow>>),
}

impl Target {
    fn rows(&self) -> usize {
        match self {
            Self::Log(log) => log.read().map_or(0, |log| log.rows()),
            Self::Merged(_, order) => order.len(),
        }
    }
}

fn unavailable<T>(_: T) -> String {
    "The log is unavailable".to_string()
}

/// Matches rows from `from` to the end. A merged target is always searched whole.
fn search(target: &Target, query: &Query, from: usize, control: &Control) -> Result<Found, String> {
    let matcher = Matcher::new(query).map_err(|error| error.to_string())?;
    match target {
        Target::Log(log) => {
            let log = log.read().map_err(unavailable)?;
            let searched = from.min(log.rows())..log.rows();
            let rows = query::matching_rows_in(&log, &matcher, searched.clone(), control)
                .map_err(|error| error.to_string())?;
            Ok(Found { searched, rows })
        }
        Target::Merged(logs, order) => {
            // Each file is searched on its own, then its matches are picked out of
            // the merged order with one bit per row.
            let mut matched = Vec::with_capacity(logs.len());
            for log in logs {
                let log = log.read().map_err(unavailable)?;
                let mut bits = vec![0u64; log.rows().div_ceil(64)];
                for row in query::matching_rows(&log, &matcher, control)
                    .map_err(|error| error.to_string())?
                {
                    bits[row as usize / 64] |= 1 << (row % 64);
                }
                matched.push(bits);
            }
            let is_match = |merged: &MergedRow| {
                matched
                    .get(usize::from(merged.source))
                    .and_then(|bits| bits.get(merged.row as usize / 64))
                    .is_some_and(|word| word & (1 << (merged.row % 64)) != 0)
            };
            let rows = (0..order.len())
                .filter(|position| is_match(&order[*position]))
                .map(|position| position as u32)
                .collect();
            Ok(Found {
                searched: 0..order.len(),
                rows,
            })
        }
    }
}

/// A search running on another thread.
struct SearchJob {
    control: Arc<Control>,
    result: Receiver<Result<Found, String>>,
    total_rows: usize,
}

impl SearchJob {
    /// Starts matching the rows from `from` to the end. `wake` runs when the
    /// result is ready.
    fn start(target: Target, query: Query, from: usize, wake: impl Fn() + Send + 'static) -> Self {
        let control = Arc::new(Control::default());
        let (sender, result) = channel();
        let total_rows = target.rows().saturating_sub(from);
        let job_control = control.clone();
        std::thread::spawn(move || {
            let _ = sender.send(search(&target, &query, from, &job_control));
            wake();
        });
        Self {
            control,
            result,
            total_rows,
        }
    }

    /// The finished result, if it's ready.
    fn poll(&self) -> Option<Result<Found, String>> {
        match self.result.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("The search stopped".to_string())),
        }
    }

    fn percent_done(&self) -> usize {
        (self.control.rows_done.load(Ordering::Relaxed) * 100 / self.total_rows.max(1)).min(100)
    }
}

impl Drop for SearchJob {
    /// A replaced or abandoned search stops using the CPU.
    fn drop(&mut self) {
        self.control.cancel.store(true, Ordering::Relaxed);
    }
}

/// The rows matching a filter or find, kept current as live tail changes the
/// log: after the first search, only the rows that changed are searched.
pub struct Matches {
    target: Target,
    query: Query,
    /// Rows in file or merged order. `None` until the first search finishes.
    rows: Option<Arc<Vec<u32>>>,
    job: Option<SearchJob>,
    /// Rows that changed while a search ran, to search once it finishes.
    stale_from: Option<usize>,
    started: Instant,
}

fn job_waker(waker: &Waker) -> impl Fn() + Send + 'static {
    let waker = waker.clone();
    move || waker.wake()
}

impl Matches {
    pub fn start(target: Target, query: Query, waker: &Waker) -> Self {
        let job = SearchJob::start(target.clone(), query.clone(), 0, job_waker(waker));
        Self {
            target,
            query,
            rows: None,
            job: Some(job),
            stale_from: None,
            started: Instant::now(),
        }
    }

    pub fn rows(&self) -> Option<&[u32]> {
        self.rows.as_deref().map(Vec::as_slice)
    }

    /// How far the first search has got.
    pub fn percent_done(&self) -> usize {
        self.job.as_ref().map_or(100, SearchJob::percent_done)
    }

    /// Time since the search started.
    pub fn seconds(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// Rows from `row` on changed, so search them again, now or once the
    /// running search finishes.
    pub fn changed_from(&mut self, row: usize, waker: &Waker) {
        if self.job.is_some() {
            self.stale_from = Some(self.stale_from.map_or(row, |stale| stale.min(row)));
        } else {
            self.job = Some(SearchJob::start(
                self.target.clone(),
                self.query.clone(),
                row,
                job_waker(waker),
            ));
        }
    }

    /// Searches everything again, in `target` from now on. `clear` drops the old
    /// matches at once, for when they may name rows that no longer exist.
    /// Otherwise they stay on screen until the new ones are ready.
    pub fn restart(&mut self, target: Target, clear: bool, waker: &Waker) {
        if clear && self.rows.is_some() {
            self.rows = Some(Arc::default());
        }
        self.target = target;
        self.stale_from = None;
        self.job = Some(SearchJob::start(
            self.target.clone(),
            self.query.clone(),
            0,
            job_waker(waker),
        ));
    }

    /// Picks up a finished search. Returns true if the matches changed.
    pub fn poll(&mut self, waker: &Waker) -> Result<bool, String> {
        let Some(outcome) = self.job.as_ref().and_then(SearchJob::poll) else {
            return Ok(false);
        };
        self.job = None;
        let found = outcome?;
        // Nothing else holds the list between frames, so this changes it in place.
        let rows = Arc::make_mut(self.rows.get_or_insert_with(Arc::default));
        let keep = rows.partition_point(|row| (*row as usize) < found.searched.start);
        rows.truncate(keep);
        rows.extend(found.rows);
        if let Some(stale) = self.stale_from.take() {
            self.changed_from(stale, waker);
        }
        Ok(true)
    }
}

/// The view position of a file row, or of the nearest row after it if a filter
/// hides it. `rows` is the filter's sorted list of file rows.
pub fn view_position(rows: Option<&[u32]>, file_row: usize) -> usize {
    match rows {
        Some(rows) => rows.partition_point(|row| (*row as usize) < file_row),
        None => file_row,
    }
}

/// The next or previous match after `from` (a file row), wrapping around, and
/// keeping to rows the filter shows. Returns a file row.
pub fn step_match(
    matches: &[u32],
    filter: Option<&[u32]>,
    from: Option<usize>,
    forward: bool,
) -> Option<usize> {
    let visible = |row: u32| filter.is_none_or(|filter| filter.binary_search(&row).is_ok());
    let candidates: Box<dyn Iterator<Item = &u32>> = match (from, forward) {
        (None, true) => Box::new(matches.iter()),
        (None, false) => Box::new(matches.iter().rev()),
        (Some(from), true) => {
            let split = matches.partition_point(|row| (*row as usize) <= from);
            Box::new(matches[split..].iter().chain(matches[..split].iter()))
        }
        (Some(from), false) => {
            let split = matches.partition_point(|row| (*row as usize) < from);
            Box::new(
                matches[..split]
                    .iter()
                    .rev()
                    .chain(matches[split..].iter().rev()),
            )
        }
    };
    candidates
        .copied()
        .find(|row| visible(*row))
        .map(|row| row as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_words_become_literal_text() {
        let query = parse("hash mismatch").unwrap();
        assert_eq!(query.text.as_deref(), Some("hash mismatch"));
        assert!(!query.regex);
    }

    #[test]
    fn keys_set_filters_and_the_rest_is_text() {
        let query = parse(
            "failed level:warning component:PolicyAgent thread:4128 from:2026-01-16 to:2026-01-17",
        )
        .unwrap();
        assert_eq!(query.text.as_deref(), Some("failed"));
        assert_eq!(query.min_level, Some(Level::Warning));
        assert_eq!(query.component.as_deref(), Some("PolicyAgent"));
        assert_eq!(query.thread.as_deref(), Some("4128"));
        assert_eq!(query.from.as_deref(), Some("2026-01-16"));
        assert_eq!(query.to.as_deref(), Some("2026-01-17"));
    }

    #[test]
    fn slashes_make_a_regex() {
        let query = parse("/error 0x[0-9a-f]{8}/").unwrap();
        assert!(query.regex);
        assert_eq!(query.text.as_deref(), Some("error 0x[0-9a-f]{8}"));
        assert!(!parse("/").unwrap().regex);
    }

    #[test]
    fn quotes_keep_phrases_together() {
        let query = parse("\"level:error\" component:\"Policy Agent\"").unwrap();
        assert_eq!(query.text.as_deref(), Some("level:error"));
        assert_eq!(query.component.as_deref(), Some("Policy Agent"));
    }

    #[test]
    fn unknown_keys_and_urls_stay_text() {
        let query = parse("http://mp01.contoso.com error:").unwrap();
        assert_eq!(
            query.text.as_deref(),
            Some("http://mp01.contoso.com error:")
        );
        assert_eq!(query.min_level, None);
    }

    #[test]
    fn mistakes_are_reported_plainly() {
        assert!(parse("level:loud").unwrap_err().contains("Unknown level"));
        assert!(parse("/(/").is_err());
        assert!(parse("from:yesterday").is_err());
    }

    #[test]
    fn empty_text_matches_everything() {
        assert_eq!(parse("   ").unwrap(), Query::default());
    }

    #[test]
    fn view_positions_account_for_hidden_rows() {
        let filter = [2u32, 5, 9];
        assert_eq!(view_position(Some(&filter), 5), 1);
        assert_eq!(view_position(Some(&filter), 6), 2);
        assert_eq!(view_position(Some(&filter), 100), 3);
        assert_eq!(view_position(None, 42), 42);
    }

    #[test]
    fn stepping_wraps_around_both_ways() {
        let matches = [3u32, 10, 20];
        assert_eq!(step_match(&matches, None, Some(10), true), Some(20));
        assert_eq!(step_match(&matches, None, Some(20), true), Some(3));
        assert_eq!(step_match(&matches, None, Some(10), false), Some(3));
        assert_eq!(step_match(&matches, None, Some(3), false), Some(20));
        assert_eq!(step_match(&matches, None, None, true), Some(3));
        assert_eq!(step_match(&matches, None, Some(5), true), Some(10));
        assert_eq!(step_match(&[], None, Some(5), true), None);
    }

    #[test]
    fn stepping_skips_matches_the_filter_hides() {
        let matches = [3u32, 10, 20];
        let filter = [3u32, 20];
        assert_eq!(step_match(&matches, Some(&filter), Some(3), true), Some(20));
        assert_eq!(step_match(&matches, Some(&[7]), Some(3), true), None);
    }
}
