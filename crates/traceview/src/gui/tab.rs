//! One tab and everything about how it's being viewed: scroll position,
//! selection, filter, find matches, the details pane and live tail.
//!
//! A tab shows one log, or several merged into one timeline. Either way the
//! grid sees a single list of rows. A merged tab keeps the rows of every file in
//! time order, and each of its rows stands for a row in one of the files.

use super::columns::LogKind;
use super::details::{DetailTarget, Details};
use super::export::Export;
use super::log_view::{LogView, RowSource};
use super::overview::Overview;
use super::row_cache::{RowCache, RowCells, SharedLog};
use super::rules::RuleSet;
use super::search::{self, Matches, Target};
use super::settings::Settings;
use super::watch::{Applied, Watcher};
use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Arc, RwLock, RwLockReadGuard};
use tv_engine::merge::{self, MergedRow};
use tv_engine::overview as counting;
use tv_engine::{CacheUse, Change, Control, CustomFormat, Level, LogFile, OpenStats};
use tv_platform::Waker;
use tv_ui::text_field::TextField;

/// One file in a tab. A merged tab has several.
pub struct Source {
    pub path: PathBuf,
    pub log: SharedLog,
    pub stats: OpenStats,
    pub cache: RowCache,
    /// Bookmarked rows of this file.
    pub bookmarks: BTreeSet<u32>,
    watcher: Watcher,
}

/// Merging again after live tail changed a file of a merged tab.
struct MergeJob {
    control: Arc<Control>,
    result: Receiver<Result<Vec<MergedRow>, String>>,
}

impl Drop for MergeJob {
    fn drop(&mut self) {
        self.control.cancel.store(true, Ordering::Relaxed);
    }
}

fn merge_logs(logs: &[&LogFile], control: &Control) -> Result<Vec<MergedRow>, String> {
    merge::merge_order(logs, control).map_err(|error| error.to_string())
}

pub struct Document {
    pub sources: Vec<Source>,
    /// A merged tab's rows, from every file in time order. `None` for one file.
    order: Option<Arc<Vec<MergedRow>>>,
    merging: Option<MergeJob>,
    /// A file changed while a merge ran, so merge again once it finishes.
    merge_stale: bool,
}

impl Document {
    pub fn is_merged(&self) -> bool {
        self.order.is_some()
    }

    /// The file and row a tab row stands for.
    pub fn locate(&self, row: usize) -> Option<(usize, usize)> {
        match &self.order {
            None => Some((0, row)),
            Some(order) => order
                .get(row)
                .map(|merged| (usize::from(merged.source), merged.row as usize)),
        }
    }

    /// What filters and finds search.
    pub fn target(&self) -> Target {
        match &self.order {
            None => Target::Log(self.sources[0].log.clone()),
            Some(order) => Target::Merged(
                self.sources
                    .iter()
                    .map(|source| source.log.clone())
                    .collect(),
                order.clone(),
            ),
        }
    }

    /// Asks each file's cache for the tab rows on screen.
    pub fn request(&mut self, rows: Vec<usize>) {
        if self.order.is_none() {
            self.sources[0].cache.request(rows);
            return;
        }
        let mut wanted = vec![Vec::new(); self.sources.len()];
        for row in rows {
            if let Some((source, row)) = self.locate(row) {
                wanted[source].push(row);
            }
        }
        for (source, rows) in self.sources.iter_mut().zip(wanted) {
            source.cache.request(rows);
        }
    }

    fn start_merge(&mut self, waker: &Waker) {
        if self.merging.is_some() {
            self.merge_stale = true;
            return;
        }
        let logs: Vec<SharedLog> = self
            .sources
            .iter()
            .map(|source| source.log.clone())
            .collect();
        let control = Arc::new(Control::default());
        let (sender, result) = channel();
        let (job_control, waker) = (control.clone(), waker.clone());
        std::thread::spawn(move || {
            let guards: Result<Vec<_>, _> = logs.iter().map(|log| log.read()).collect();
            let outcome = match &guards {
                Ok(guards) => merge_logs(
                    &guards.iter().map(|guard| &**guard).collect::<Vec<_>>(),
                    &job_control,
                ),
                Err(_) => Err("A log is unavailable".to_string()),
            };
            drop(guards);
            let _ = sender.send(outcome);
            waker.wake();
        });
        self.merging = Some(MergeJob { control, result });
    }

    /// Picks up a finished merge, starting another if a file changed meanwhile.
    fn poll_merge(&mut self, waker: &Waker) -> Option<Result<(), String>> {
        let outcome = match self.merging.as_ref()?.result.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("The merge stopped".to_string()),
        };
        self.merging = None;
        if std::mem::take(&mut self.merge_stale) {
            self.start_merge(waker);
        }
        Some(outcome.map(|order| self.order = Some(Arc::new(order))))
    }
}

struct Opened {
    logs: Vec<(LogFile, OpenStats)>,
    order: Option<Vec<MergedRow>>,
}

pub struct Opening {
    size: u64,
    bytes_done: Arc<AtomicU64>,
    result: Receiver<io::Result<Opened>>,
}

pub enum Content {
    Opening(Opening),
    Ready(Document),
    Failed(String),
}

/// Rows as the grid sees them: in view order, which skips rows a filter hides.
/// Holds read locks on the logs while drawing.
pub struct TabRows<'a> {
    logs: Vec<RwLockReadGuard<'a, LogFile>>,
    document: &'a Document,
    filter: Option<&'a [u32]>,
    find: Option<&'a [u32]>,
}

impl RowSource for TabRows<'_> {
    fn row_count(&self) -> usize {
        match (self.filter, &self.document.order) {
            (Some(filter), _) => filter.len(),
            (None, Some(order)) => order.len(),
            (None, None) => self.logs[0].rows(),
        }
    }

    fn level(&self, row: usize) -> Level {
        // Rows always exist, but a crash would cost more than this check.
        match self.document.locate(self.file_row(row)) {
            Some((source, row)) if row < self.logs[source].rows() => self.logs[source].level(row),
            _ => Level::Info,
        }
    }

    fn cells(&self, row: usize) -> Option<Arc<RowCells>> {
        let (source, row) = self.document.locate(self.file_row(row))?;
        self.document.sources[source].cache.get(row)
    }

    /// The row in the file, or in the merged timeline.
    fn file_row(&self, row: usize) -> usize {
        self.filter.map_or(row, |filter| {
            filter.get(row).map_or(row, |row| *row as usize)
        })
    }

    fn highlighted(&self, row: usize) -> bool {
        let file_row = self.file_row(row) as u32;
        self.find
            .is_some_and(|matches| matches.binary_search(&file_row).is_ok())
    }

    fn bookmarked(&self, row: usize) -> bool {
        self.document
            .locate(self.file_row(row))
            .is_some_and(|(source, row)| {
                self.document.sources[source]
                    .bookmarks
                    .contains(&(row as u32))
            })
    }
}

pub fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn open_logs(
    paths: &[PathBuf],
    formats: &[Arc<CustomFormat>],
    progress: &AtomicU64,
) -> io::Result<Opened> {
    let mut logs = Vec::with_capacity(paths.len());
    for path in paths {
        logs.push(LogFile::open_with_formats(path, formats, progress)?);
    }
    let order = if logs.len() > 1 {
        let refs: Vec<&LogFile> = logs.iter().map(|(log, _)| log).collect();
        Some(merge_logs(&refs, &Control::default()).map_err(io::Error::other)?)
    } else {
        None
    };
    Ok(Opened { logs, order })
}

pub struct Tab {
    /// One file, or the files merged in this tab.
    pub paths: Vec<PathBuf>,
    pub content: Content,
    pub view: LogView,
    pub filter_field: TextField,
    pub find_field: TextField,
    filter: Option<Matches>,
    /// A new filter still running its first search. The old one stays on
    /// screen until it finishes.
    next_filter: Option<Matches>,
    find: Option<Matches>,
    /// The find text the current matches came from.
    find_searched: String,
    /// Move to the next match as soon as a search finishes.
    step_when_found: Option<bool>,
    pub details: Details,
    pub message: Option<String>,
    /// The severity chips' counts and the event histogram.
    pub overview: Overview,
    /// The level the active severity chip filters to.
    pub chip: Option<Level>,
    /// Keep the newest rows in view as they arrive. True while the view is at
    /// the bottom, which is where a log opens.
    pub following: bool,
}

impl Tab {
    /// Opens one log, or merges several, reading each with a custom format if
    /// one suits it.
    pub fn open(paths: Vec<PathBuf>, formats: Vec<Arc<CustomFormat>>, waker: &Waker) -> Self {
        let size = paths
            .iter()
            .map(|path| std::fs::metadata(path).map_or(0, |metadata| metadata.len()))
            .sum();
        let bytes_done = Arc::new(AtomicU64::new(0));
        let (sender, result) = channel();
        let (open_paths, progress, open_waker) = (paths.clone(), bytes_done.clone(), waker.clone());
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let open_finished = finished.clone();
        std::thread::spawn(move || {
            let opened = open_logs(&open_paths, &formats, &progress);
            open_finished.store(true, Ordering::Release);
            let _ = sender.send(opened);
            open_waker.wake();
        });
        // Keeps the progress display moving during a long first open.
        let ticker_waker = waker.clone();
        std::thread::spawn(move || {
            while !finished.load(Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_millis(100));
                ticker_waker.wake();
            }
        });
        Self {
            paths,
            content: Content::Opening(Opening {
                size,
                bytes_done,
                result,
            }),
            view: LogView::default(),
            filter_field: TextField::default(),
            find_field: TextField::default(),
            filter: None,
            next_filter: None,
            find: None,
            find_searched: String::new(),
            step_when_found: None,
            details: Details::default(),
            message: None,
            overview: Overview::default(),
            chip: None,
            following: true,
        }
    }

    pub fn title(&self) -> String {
        match self.paths.as_slice() {
            [path] => file_name(path),
            [first, second] => format!("{} + {}", file_name(first), file_name(second)),
            [first, rest @ ..] => format!("{} + {} more", file_name(first), rest.len()),
            [] => String::new(),
        }
    }

    pub fn document(&self) -> Option<&Document> {
        match &self.content {
            Content::Ready(document) => Some(document),
            _ => None,
        }
    }

    pub fn document_mut(&mut self) -> Option<&mut Document> {
        match &mut self.content {
            Content::Ready(document) => Some(document),
            _ => None,
        }
    }

    pub fn rows(&self) -> Option<TabRows<'_>> {
        let document = self.document()?;
        Some(TabRows {
            logs: document
                .sources
                .iter()
                .map(|source| source.log.read().ok())
                .collect::<Option<_>>()?,
            document,
            filter: self.filter_rows(),
            find: self.find.as_ref().and_then(Matches::rows),
        })
    }

    /// The view and its rows together, for code that scrolls while reading rows.
    #[cfg_attr(not(feature = "bench"), allow(dead_code))]
    pub fn split(&mut self) -> (&mut LogView, Option<TabRows<'_>>) {
        let rows = match &self.content {
            Content::Ready(document) => document
                .sources
                .iter()
                .map(|source| source.log.read().ok())
                .collect::<Option<_>>()
                .map(|logs| TabRows {
                    logs,
                    document,
                    filter: self.filter.as_ref().and_then(Matches::rows),
                    find: self.find.as_ref().and_then(Matches::rows),
                }),
            _ => None,
        };
        (&mut self.view, rows)
    }

    /// Every row of the file or merged timeline, whatever the filter shows.
    pub fn total_rows(&self) -> usize {
        match self.document() {
            Some(Document {
                order: Some(order), ..
            }) => order.len(),
            Some(document) => document.sources[0].log.read().map_or(0, |log| log.rows()),
            None => 0,
        }
    }

    pub fn filter_rows(&self) -> Option<&[u32]> {
        self.filter.as_ref().and_then(Matches::rows)
    }

    #[cfg(test)]
    pub fn is_filtered(&self) -> bool {
        self.filter_rows().is_some()
    }

    /// The row under the selection's cursor, in the file or merged timeline.
    pub fn cursor_file_row(&self) -> Option<usize> {
        let rows = self.rows()?;
        self.view.cursor().map(|row| rows.file_row(row))
    }

    /// Selects a row of the file or merged timeline, showing the nearest
    /// visible row if a filter hides it.
    pub fn select_file_row(&mut self, file_row: usize) {
        let position = search::view_position(self.filter_rows(), file_row);
        self.view.select(position, false);
        self.following = self.view.at_bottom();
    }

    /// Asks for the selected row's details and the rows on screen, which load
    /// in the background.
    pub fn request_rows(&mut self, waker: &Waker) {
        let selected = self.cursor_file_row();
        let visible: Vec<usize> = match self.rows() {
            Some(rows) => self
                .view
                .visible_rows()
                .map(|row| rows.file_row(row))
                .collect(),
            None => Vec::new(),
        };
        let Some(document) = self.document_mut() else {
            return;
        };
        let target = selected
            .and_then(|row| document.locate(row))
            .map(|(source, row)| DetailTarget {
                log: document.sources[source].log.clone(),
                source,
                row,
                file: document
                    .is_merged()
                    .then(|| file_name(&document.sources[source].path)),
            });
        document.request(visible);
        let wake = waker.clone();
        self.details.request(target, move || wake.wake());
    }

    pub fn is_paused(&self) -> bool {
        self.document().is_some_and(|document| {
            document
                .sources
                .iter()
                .all(|source| source.watcher.is_paused())
        })
    }

    /// Pauses or resumes live tail.
    pub fn set_paused(&mut self, paused: bool) {
        if let Some(document) = self.document() {
            for source in &document.sources {
                source.watcher.set_paused(paused);
            }
        }
    }

    /// The kind of log the tab's first file is, which decides its columns.
    pub fn kind(&self) -> Option<LogKind> {
        let source = self.document()?.sources.first()?;
        source.log.read().ok().map(|log| LogKind::of(&log))
    }

    /// Marks rows with new highlight rules.
    pub fn set_rules(&mut self, rules: &Arc<RuleSet>) {
        if let Some(document) = self.document_mut() {
            for source in &mut document.sources {
                source.cache.set_rules(rules.clone());
            }
        }
    }

    /// Picks up finished background work. Returns true if anything changed.
    pub fn poll(&mut self, waker: &Waker, settings: &Settings, rules: &Arc<RuleSet>) -> bool {
        let mut changed = self.details.poll_changed();
        if let Content::Opening(opening) = &self.content {
            match opening.result.try_recv() {
                Ok(Ok(opened)) => {
                    let sources = opened
                        .logs
                        .into_iter()
                        .zip(&self.paths)
                        .map(|((log, stats), path)| {
                            let log: SharedLog = Arc::new(RwLock::new(log));
                            let wake = waker.clone();
                            let mut cache = RowCache::new(log.clone(), move || wake.wake());
                            cache.set_rules(rules.clone());
                            Source {
                                path: path.clone(),
                                cache,
                                watcher: Watcher::start(log.clone(), waker.clone()),
                                bookmarks: settings.bookmarks_for(path),
                                log,
                                stats,
                            }
                        })
                        .collect();
                    self.content = Content::Ready(Document {
                        sources,
                        order: opened.order.map(Arc::new),
                        merging: None,
                        merge_stale: false,
                    });
                    changed = true;
                }
                Ok(Err(error)) => {
                    self.content =
                        Content::Failed(format!("Couldn't open {}: {error}", self.title()));
                    changed = true;
                }
                Err(TryRecvError::Disconnected) => {
                    self.content = Content::Failed(format!("Couldn't open {}", self.title()));
                    changed = true;
                }
                Err(TryRecvError::Empty) => changed = true,
            }
        }
        let Some(document) = self.document_mut() else {
            return changed;
        };

        let applied: Vec<(usize, io::Result<Applied>)> = document
            .sources
            .iter_mut()
            .enumerate()
            .filter_map(|(index, source)| {
                source
                    .watcher
                    .apply(&source.log)
                    .map(|result| (index, result))
            })
            .collect();
        for (source, result) in applied {
            match result {
                Ok(applied) => self.log_changed(source, applied, waker),
                Err(error) => self.message = Some(format!("Couldn't read new rows: {error}")),
            }
            changed = true;
        }
        changed |= self.poll_merge(waker);
        let logs: Option<Vec<SharedLog>> = self.document().map(|document| {
            document
                .sources
                .iter()
                .map(|source| source.log.clone())
                .collect()
        });
        if let Some(logs) = logs {
            self.overview.update(&logs, waker);
        }
        changed |= self.overview.poll();

        if let Some(next) = &mut self.next_filter {
            match next.poll(waker) {
                Ok(false) => {}
                Ok(true) => {
                    let seconds = next.seconds();
                    let keep = self.cursor_file_row();
                    self.filter = self.next_filter.take();
                    let shown = self.filter_rows().map_or(0, <[u32]>::len);
                    self.view.reset(shown);
                    if let Some(row) = keep {
                        self.select_file_row(row);
                    }
                    self.message = Some(format!(
                        "Filter shows {} rows ({seconds:.2} s)",
                        with_commas(shown)
                    ));
                    changed = true;
                }
                Err(error) => {
                    self.next_filter = None;
                    self.message = Some(error);
                    changed = true;
                }
            }
        }
        if let Some(filter) = &mut self.filter {
            match filter.poll(waker) {
                Ok(updated) => changed |= updated,
                Err(error) => {
                    self.message = Some(error);
                    changed = true;
                }
            }
        }
        if let Some(find) = &mut self.find {
            match find.poll(waker) {
                Ok(false) => {}
                Ok(true) => {
                    if let Some(forward) = self.step_when_found.take() {
                        self.step_find(forward);
                    }
                    changed = true;
                }
                Err(error) => {
                    self.find = None;
                    self.message = Some(error);
                    changed = true;
                }
            }
        }
        changed
    }

    /// Brings everything up to date after live tail changed one of the files.
    fn log_changed(&mut self, source: usize, applied: Applied, waker: &Waker) {
        let first = match applied.change {
            Change::Unchanged => return,
            // The last row may have grown, so it's read again too.
            Change::Appended { .. } => applied.before.saturating_sub(1),
            Change::Reset => 0,
            Change::Removed => {
                self.message =
                    Some("A file was deleted or renamed. Rows read so far stay here".to_string());
                return;
            }
        };
        self.overview.changed(applied.change == Change::Reset);
        let Some(document) = self.document_mut() else {
            return;
        };
        document.sources[source].cache.invalidate_from(first);
        if document.is_merged() {
            // New rows can land anywhere in a merged timeline, so it's merged again.
            document.start_merge(waker);
            return;
        }
        if self.cursor_file_row().is_some_and(|row| row >= first) {
            self.details.refresh();
        }
        if applied.change == Change::Reset {
            let Some(target) = self.document().map(Document::target) else {
                return;
            };
            for matches in [&mut self.filter, &mut self.next_filter, &mut self.find]
                .into_iter()
                .flatten()
            {
                matches.restart(target.clone(), true, waker);
            }
            let count = self.rows().map_or(0, |rows| rows.row_count());
            self.view.reset(count);
            self.details.refresh();
            self.message = Some("The file was replaced, so it was read again".to_string());
        } else {
            for matches in [&mut self.filter, &mut self.next_filter, &mut self.find]
                .into_iter()
                .flatten()
            {
                matches.changed_from(first, waker);
            }
        }
    }

    /// Picks up a merged tab's new timeline, keeping the selected row selected.
    fn poll_merge(&mut self, waker: &Waker) -> bool {
        // Found before the timeline changes, as the row's position will.
        let kept = self
            .cursor_file_row()
            .and_then(|row| self.document().and_then(|document| document.locate(row)));
        match self
            .document_mut()
            .and_then(|document| document.poll_merge(waker))
        {
            None => return false,
            Some(Err(error)) => {
                self.message = Some(error);
                return true;
            }
            Some(Ok(())) => {}
        }
        let Some(document) = self.document() else {
            return true;
        };
        let target = document.target();
        let position = kept.and_then(|(source, row)| {
            let order = document.order.as_ref()?;
            order.iter().position(|merged| {
                usize::from(merged.source) == source && merged.row as usize == row
            })
        });
        for matches in [&mut self.filter, &mut self.next_filter, &mut self.find]
            .into_iter()
            .flatten()
        {
            matches.restart(target.clone(), false, waker);
        }
        let count = self.rows().map_or(0, |rows| rows.row_count());
        self.view.set_row_count(count);
        if let Some(position) = position {
            let position = search::view_position(self.filter_rows(), position);
            self.view.select(position, false);
        }
        self.details.refresh();
        true
    }

    /// Filters to rows matching the filter box. An empty box shows every row.
    pub fn apply_filter(&mut self, waker: &Waker) {
        let text = self.filter_field.text().trim().to_string();
        self.next_filter = None;
        if text.is_empty() && self.chip.is_none() {
            let keep = self.cursor_file_row();
            self.filter = None;
            let count = self.rows().map_or(0, |rows| rows.row_count());
            self.view.reset(count);
            if let Some(row) = keep {
                self.select_file_row(row);
            }
            self.message = Some("Filter cleared".to_string());
            return;
        }
        let Some(target) = self.document().map(Document::target) else {
            return;
        };
        match search::parse(&text) {
            Ok(mut query) => {
                query.level = self.chip;
                self.next_filter = Some(Matches::start(target, query, waker));
                self.message = None;
            }
            Err(error) => self.message = Some(error),
        }
    }

    /// Filters to one level, as clicking a severity chip does, or back to every
    /// level. The filter box still applies.
    pub fn set_chip(&mut self, level: Option<Level>, waker: &Waker) {
        self.chip = level;
        self.apply_filter(waker);
    }

    /// Where the first row at or after a time is, in the file or merged
    /// timeline. A merged position is the sum of each file's.
    fn position_at(&self, seconds: u32) -> usize {
        let Some(document) = self.document() else {
            return 0;
        };
        document
            .sources
            .iter()
            .filter_map(|source| {
                let log = source.log.read().ok()?;
                counting::first_row_at(&log, None, seconds, &Control::default()).ok()
            })
            .sum()
    }

    /// The time of a view row, or of the nearest timed row above it.
    pub fn time_at(&self, row: usize) -> Option<u32> {
        let rows = self.rows()?;
        let document = self.document()?;
        (0..=row).rev().take(1000).find_map(|row| {
            let (source, row) = document.locate(rows.file_row(row))?;
            rows.logs[source].stamp_seconds(row)
        })
    }

    /// Scrolls so the first row at or after a time is at the top.
    pub fn scroll_to_time(&mut self, seconds: u32) {
        let position = search::view_position(self.filter_rows(), self.position_at(seconds));
        self.view.scroll_to(position as f64);
    }

    /// Selects the most severe row in a histogram bucket: its first error, or
    /// else its first warning, or else its first row. Following stops, so the
    /// row stays in view.
    pub fn jump_to_bucket(&mut self, bucket: usize) {
        let Some(histogram) = &self.overview.counts.histogram else {
            return;
        };
        let (start, end) = (
            histogram.bucket_start(bucket),
            histogram.bucket_start(bucket + 1),
        );
        let (first, last) = (self.position_at(start), self.position_at(end));
        let Some(rows) = self.rows() else { return };
        let count = rows.row_count();
        let filter = self.filter_rows();
        let (from, to) = (
            search::view_position(filter, first),
            search::view_position(filter, last).min(count),
        );
        if from >= count {
            return;
        }
        let worst = |level: Level| (from..to).find(|row| rows.level(*row) >= level);
        let row = worst(Level::Error)
            .or_else(|| worst(Level::Warning))
            .unwrap_or(from);
        drop(rows);
        self.view.select(row, false);
        self.following = false;
    }

    /// Moves to the next or previous match of the find box. If the text changed,
    /// it searches first and moves when the search finishes.
    pub fn find_next(&mut self, forward: bool, waker: &Waker) {
        let text = self.find_field.text().trim().to_string();
        if text.is_empty() {
            self.clear_find();
            return;
        }
        if text != self.find_searched || self.find.is_none() {
            let Some(target) = self.document().map(Document::target) else {
                return;
            };
            match search::parse(&text) {
                Ok(query) => {
                    self.find = Some(Matches::start(target, query, waker));
                    self.find_searched = text;
                    self.step_when_found = Some(forward);
                    self.message = None;
                }
                Err(error) => self.message = Some(error),
            }
            return;
        }
        if self.find.as_ref().is_some_and(|find| find.rows().is_none()) {
            // Still searching, so move once it's done.
            self.step_when_found = Some(forward);
            return;
        }
        self.step_find(forward);
    }

    fn step_find(&mut self, forward: bool) {
        let Some(matches) = self.find.as_ref().and_then(Matches::rows) else {
            return;
        };
        match search::step_match(matches, self.filter_rows(), self.cursor_file_row(), forward) {
            Some(row) => self.select_file_row(row),
            None => self.message = Some("No matches".to_string()),
        }
    }

    /// Clears find highlighting.
    pub fn clear_find(&mut self) {
        self.find = None;
        self.find_searched.clear();
        self.step_when_found = None;
    }

    /// Where the selection sits among the find matches, for "3 of 120".
    pub fn find_summary(&self) -> Option<String> {
        let find = self.find.as_ref()?;
        let Some(matches) = find.rows() else {
            return Some(format!("Searching {}%", find.percent_done()));
        };
        if matches.is_empty() {
            return Some("No matches".to_string());
        }
        let total = with_commas(matches.len());
        match self
            .cursor_file_row()
            .and_then(|row| matches.binary_search(&(row as u32)).ok())
        {
            Some(index) => Some(format!("{} of {total}", with_commas(index + 1))),
            None => Some(format!("{total} matches")),
        }
    }

    /// The rows to export: the selection if it's more than one row, otherwise
    /// every row shown.
    pub fn export(&self) -> Option<Export> {
        let document = self.document()?;
        let count = self.rows()?.row_count();
        let rows = match self.view.selection() {
            Some(range) if range.end() > range.start() => *range.start()..*range.end() + 1,
            _ => 0..count,
        };
        Some(Export {
            logs: document
                .sources
                .iter()
                .map(|source| source.log.clone())
                .collect(),
            names: document.is_merged().then(|| {
                document
                    .sources
                    .iter()
                    .map(|source| file_name(&source.path))
                    .collect()
            }),
            order: document.order.clone(),
            filter: self.filter.as_ref().and_then(Matches::shared_rows),
            rows,
        })
    }

    /// Bookmarks the row under the cursor, or removes its bookmark. Returns
    /// that row's file and all its bookmarks, to save.
    /// Bookmarks a row of the file or merged timeline, or removes its
    /// bookmark: the one given, or else the selected one.
    pub fn toggle_bookmark(&mut self, row: Option<usize>) -> Option<(PathBuf, BTreeSet<u32>)> {
        let row = row.or_else(|| self.cursor_file_row())?;
        let document = self.document_mut()?;
        let (source, row) = document.locate(row)?;
        let source = &mut document.sources[source];
        if !source.bookmarks.remove(&(row as u32)) {
            source.bookmarks.insert(row as u32);
        }
        Some((source.path.clone(), source.bookmarks.clone()))
    }

    /// Uses a file's bookmarks as changed in another tab.
    pub fn update_bookmarks(&mut self, path: &Path, rows: &BTreeSet<u32>) {
        if let Some(document) = self.document_mut() {
            for source in document
                .sources
                .iter_mut()
                .filter(|source| source.path == path)
            {
                source.bookmarks.clone_from(rows);
            }
        }
    }

    /// Selects the next or previous bookmarked row the filter shows.
    pub fn step_bookmark(&mut self, forward: bool) {
        let Some(document) = self.document() else {
            return;
        };
        let marked: Vec<u32> = match &document.order {
            None => document.sources[0].bookmarks.iter().copied().collect(),
            Some(order) => (0..order.len())
                .filter(|position| {
                    let merged = order[*position];
                    document.sources[usize::from(merged.source)]
                        .bookmarks
                        .contains(&merged.row)
                })
                .map(|position| position as u32)
                .collect(),
        };
        match search::step_match(&marked, self.filter_rows(), self.cursor_file_row(), forward) {
            Some(row) => self.select_file_row(row),
            None if marked.is_empty() => {
                self.message = Some("No bookmarks yet. Ctrl+D bookmarks a row".to_string())
            }
            None => self.message = Some("The filter hides every bookmark".to_string()),
        }
    }

    /// Selects the next or previous row at error level or above.
    pub fn step_error(&mut self, forward: bool) {
        let Some(rows) = self.rows() else { return };
        let count = rows.row_count();
        let start = self.view.cursor();
        let found = if forward {
            let from = start.map_or(0, |row| row + 1);
            (from..count).find(|row| rows.level(*row) >= Level::Error)
        } else {
            let to = start.unwrap_or(count);
            (0..to).rev().find(|row| rows.level(*row) >= Level::Error)
        };
        drop(rows);
        match found {
            Some(row) => {
                self.view.select(row, false);
                self.following = self.view.at_bottom();
            }
            None => {
                self.message = Some(
                    if forward {
                        "No more errors below"
                    } else {
                        "No more errors above"
                    }
                    .to_string(),
                )
            }
        }
    }

    /// Status text for this tab.
    pub fn status(&self) -> String {
        let document = match &self.content {
            Content::Opening(opening) => {
                let done = opening.bytes_done.load(Ordering::Relaxed).min(opening.size);
                return format!(
                    "Indexing {}: {}%",
                    self.title(),
                    done * 100 / opening.size.max(1)
                );
            }
            Content::Failed(message) => return message.clone(),
            Content::Ready(document) => document,
        };
        let Some(rows) = self.rows() else {
            return String::new();
        };
        let total = match &document.order {
            Some(order) => order.len(),
            None => rows.logs[0].rows(),
        };
        let mut parts = vec![format!("{} rows", with_commas(total))];
        if let Some(filter) = self.filter_rows() {
            parts.push(format!("{} shown", with_commas(filter.len())));
        }
        if let Some(next) = &self.next_filter {
            parts.push(format!("Filtering {}%", next.percent_done()));
        }
        if let Some(range) = self.view.selection() {
            let count = range.end() - range.start() + 1;
            if count > 1 {
                parts.push(format!("{} selected", with_commas(count)));
            } else {
                parts.push(format!(
                    "Row {} selected",
                    with_commas(rows.file_row(*range.start()) + 1)
                ));
            }
        }
        if document.is_merged() {
            let size: u64 = rows.logs.iter().map(|log| log.indexed_len()).sum();
            parts.push(format!("{} logs merged", document.sources.len()));
            parts.push(super::app::readable_size(size));
        } else {
            let (log, stats) = (&rows.logs[0], &document.sources[0].stats);
            parts.push(super::app::encoding_name(log.encoding()).to_string());
            parts.push(super::app::readable_size(log.indexed_len()));
            parts.push(log.format_name().to_string());
            parts.push(format!(
                "Opened in {:.2} s{}",
                stats.seconds,
                match stats.cache {
                    CacheUse::Hit => " from cache",
                    CacheUse::Extended => " from cache, plus new rows",
                    CacheUse::Miss => "",
                }
            ));
        }
        if self.is_paused() {
            parts.push("Paused".to_string());
        } else if self.following {
            parts.push("Following".to_string());
        }
        if let Some(message) = &self.message {
            parts.push(message.clone());
        }
        parts.join("   |   ")
    }
}

/// Groups digits: 26904325 becomes 26,904,325.
pub fn with_commas(value: usize) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_get_thousands_separators() {
        assert_eq!(with_commas(0), "0");
        assert_eq!(with_commas(999), "999");
        assert_eq!(with_commas(1000), "1,000");
        assert_eq!(with_commas(26_904_325), "26,904,325");
    }

    #[test]
    fn merged_titles_name_the_files() {
        let tab = |paths: &[&str]| Tab {
            paths: paths.iter().map(PathBuf::from).collect(),
            ..Tab::open(Vec::new(), Vec::new(), &Waker::detached())
        };
        assert_eq!(tab(&["C:\\logs\\a.log"]).title(), "a.log");
        assert_eq!(tab(&["a.log", "b.log"]).title(), "a.log + b.log");
        assert_eq!(tab(&["a.log", "b.log", "c.log"]).title(), "a.log + 2 more");
    }
}
