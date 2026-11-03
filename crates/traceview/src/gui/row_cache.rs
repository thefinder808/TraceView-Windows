//! Reads rows on a background thread, so the UI thread never touches the disk.
//! When a row isn't ready, the grid draws a placeholder and moves on, which means
//! scrolling can't stall, even on a slow disk or network share.
//!
//! Highlight rules are checked here too, as each row is read.

use super::rules::{Mark, RuleSet};
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, RwLock};
use tv_engine::time::Stamp;
use tv_engine::{encoding, LogFile};

const ROWS_PER_BLOCK: usize = 128;
const MAX_BLOCKS: usize = 512;
/// Only this much of a message is kept for display, because no screen shows more.
const MAX_DISPLAY_BYTES: usize = 2048;

/// A row's fields, ready to draw.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowCells {
    pub time: String,
    pub component: String,
    pub thread: String,
    pub message: String,
    /// How the first highlight rule that matches the row shows on it.
    pub mark: Option<Mark>,
}

fn first_line(bytes: &[u8]) -> &[u8] {
    let capped = &bytes[..bytes.len().min(MAX_DISPLAY_BYTES)];
    let end = capped
        .iter()
        .position(|byte| matches!(byte, b'\n' | b'\r'))
        .unwrap_or(capped.len());
    &capped[..end]
}

fn display_time(stamp: Stamp) -> String {
    stamp.to_string().replace('T', " ")
}

pub fn cells_from_row(log: &LogFile, raw: &[u8], scratch: &mut Vec<u8>) -> RowCells {
    cells_from_text(log, log.text(raw, scratch))
}

/// A row's cells from its text, as [`LogFile::text`] gives it.
fn cells_from_text(log: &LogFile, text: &[u8]) -> RowCells {
    let fields = log.fields(text);
    RowCells {
        time: fields.stamp.map(display_time).unwrap_or_default(),
        component: encoding::display(fields.component).into_owned(),
        thread: encoding::display(fields.thread).into_owned(),
        message: encoding::display(first_line(fields.message)).into_owned(),
        mark: None,
    }
}

/// A log shared between the UI and background threads. Readers never block
/// each other, and live tail takes the write lock only briefly to add rows.
pub type SharedLog = Arc<RwLock<LogFile>>;

/// One block of loaded rows.
type Block = Arc<[Arc<RowCells>]>;

struct Entry {
    /// When the block was last used, by the cache's clock.
    used: u64,
    /// The rules version its marks came from.
    rules: u64,
    rows: Block,
}

#[derive(Default)]
struct Blocks {
    entries: HashMap<usize, Entry>,
    clock: u64,
    /// Goes up with each invalidation, so a block read before one is thrown
    /// away rather than stored.
    generation: u64,
    rules: Arc<RuleSet>,
    /// Goes up when the rules change. Blocks marked by older rules still
    /// draw until they're read again, so rows never blank out meanwhile.
    rules_version: u64,
}

impl Blocks {
    fn insert(&mut self, block: usize, rules: u64, rows: Block) {
        self.clock += 1;
        let used = self.clock;
        self.entries.insert(block, Entry { used, rules, rows });
        if self.entries.len() > MAX_BLOCKS {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(block, _)| *block);
            if let Some(oldest) = oldest {
                self.entries.remove(&oldest);
            }
        }
    }
}

pub struct RowCache {
    blocks: Arc<Mutex<Blocks>>,
    requests: Sender<Vec<usize>>,
    last_request: Vec<usize>,
}

impl RowCache {
    /// Starts the loader. `wake` runs on the loader's thread whenever new rows
    /// are ready. The thread ends when the cache is dropped.
    pub fn new(log: SharedLog, wake: impl Fn() + Send + 'static) -> Self {
        let blocks = Arc::new(Mutex::new(Blocks::default()));
        let (requests, incoming) = channel();
        let shared = blocks.clone();
        std::thread::spawn(move || load_rows(&log, &incoming, &shared, wake));
        Self {
            blocks,
            requests,
            last_request: Vec::new(),
        }
    }

    /// The row's cells, if they've been loaded.
    pub fn get(&self, row: usize) -> Option<Arc<RowCells>> {
        let mut blocks = self.blocks.lock().ok()?;
        blocks.clock += 1;
        let clock = blocks.clock;
        let entry = blocks.entries.get_mut(&(row / ROWS_PER_BLOCK))?;
        entry.used = clock;
        entry.rows.get(row % ROWS_PER_BLOCK).cloned()
    }

    /// Marks rows with new highlight rules. Rows on screen are read again
    /// with them, and keep their old marks until then.
    pub fn set_rules(&mut self, rules: Arc<RuleSet>) {
        if let Ok(mut blocks) = self.blocks.lock() {
            blocks.rules = rules;
            blocks.rules_version += 1;
        }
        self.last_request.clear();
    }

    /// Forgets rows from `row` onward, because live tail changed them. Pass 0
    /// after the file was replaced.
    pub fn invalidate_from(&mut self, row: usize) {
        if let Ok(mut blocks) = self.blocks.lock() {
            let first = row / ROWS_PER_BLOCK;
            blocks.generation += 1;
            blocks.entries.retain(|block, _| *block < first);
        }
        self.last_request.clear();
    }

    /// Says which rows are on screen, by their numbers in the file. They can be
    /// scattered when a filter is on, so only their blocks are loaded. When they
    /// sit together, the loader also fetches the rows around them, so ordinary
    /// scrolling usually finds the next rows ready.
    pub fn request(&mut self, rows: impl IntoIterator<Item = usize>) {
        let mut blocks: Vec<usize> = rows.into_iter().map(|row| row / ROWS_PER_BLOCK).collect();
        blocks.dedup();
        if blocks != self.last_request {
            self.last_request.clone_from(&blocks);
            let _ = self.requests.send(blocks);
        }
    }
}

fn newest_request(
    incoming: &Receiver<Vec<usize>>,
    mut current: Option<Vec<usize>>,
) -> Result<Option<Vec<usize>>, ()> {
    loop {
        match incoming.try_recv() {
            Ok(newer) => current = Some(newer),
            Err(TryRecvError::Empty) => return Ok(current),
            Err(TryRecvError::Disconnected) => return Err(()),
        }
    }
}

fn read_block(
    log: &LogFile,
    block: usize,
    rules: &RuleSet,
    buffer: &mut Vec<u8>,
    scratch: &mut Vec<u8>,
) -> Option<Block> {
    let start = block * ROWS_PER_BLOCK;
    let end = (start + ROWS_PER_BLOCK).min(log.rows());
    let mut rows = Vec::with_capacity(end.saturating_sub(start));
    log.read_rows(start..end, buffer, |row, raw, _| {
        let text = log.text(raw, scratch);
        let mut cells = cells_from_text(log, text);
        cells.mark = rules.mark(log, row, text);
        rows.push(Arc::new(cells));
    })
    .ok()?;
    Some(rows.into())
}

fn load_rows(
    log: &RwLock<LogFile>,
    incoming: &Receiver<Vec<usize>>,
    blocks: &Mutex<Blocks>,
    wake: impl Fn(),
) {
    let (mut buffer, mut scratch) = (Vec::new(), Vec::new());
    while let Ok(first) = incoming.recv() {
        let mut request = Some(first);
        while let Some(visible) = request.take() {
            let (Some(&first), Some(&last)) = (visible.first(), visible.last()) else {
                continue;
            };
            let contiguous = last + 1 - first == visible.len();
            let span = if contiguous { visible.len() } else { 0 };
            let total_blocks = log
                .read()
                .map_or(0, |log| log.rows().div_ceil(ROWS_PER_BLOCK));
            let ahead = (last + 1)..(last + 1 + span).min(total_blocks);
            let behind = first.saturating_sub(span)..first;
            let mut superseded = false;
            for block in visible.iter().copied().chain(ahead).chain(behind) {
                let present = blocks
                    .lock()
                    .map(|blocks| {
                        blocks
                            .entries
                            .get(&block)
                            .is_some_and(|entry| entry.rules == blocks.rules_version)
                    })
                    .unwrap_or(true);
                if !present {
                    // The generation is read under the log's lock, so a refresh can't slip
                    // between it and the read.
                    let loaded = log.read().ok().and_then(|log| {
                        let (generation, rules, version) = blocks.lock().map_or_else(
                            |_| (0, Arc::default(), 0),
                            |blocks| {
                                (
                                    blocks.generation,
                                    blocks.rules.clone(),
                                    blocks.rules_version,
                                )
                            },
                        );
                        read_block(&log, block, &rules, &mut buffer, &mut scratch)
                            .map(|rows| (generation, version, rows))
                    });
                    if let Some((generation, version, loaded)) = loaded {
                        if let Ok(mut blocks) = blocks.lock() {
                            if blocks.generation == generation {
                                blocks.insert(block, version, loaded);
                            }
                        }
                        if visible.contains(&block) {
                            wake();
                        }
                    }
                }
                // Scrolling may have moved on, so don't finish loading stale rows.
                match newest_request(incoming, None) {
                    Ok(Some(newer)) => {
                        request = Some(newer);
                        superseded = true;
                        break;
                    }
                    Ok(None) => {}
                    Err(()) => return,
                }
            }
            if superseded {
                continue;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::settings::Rule;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    fn open(text: &str) -> (tempfile::TempDir, SharedLog) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.log");
        std::fs::write(&path, text).unwrap();
        let (log, _) = LogFile::open_with_cache(&path, None, &AtomicU64::new(0)).unwrap();
        (dir, Arc::new(RwLock::new(log)))
    }

    fn wait_for(cache: &RowCache, row: usize) -> Arc<RowCells> {
        let started = Instant::now();
        loop {
            if let Some(cells) = cache.get(row) {
                return cells;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "row {row} never loaded"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn requested_rows_load_and_wake_the_ui() {
        let (_dir, log) = open("2026-01-01 10:00:00 host app[1]: hello\nsecond\n");
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = wakes.clone();
        let mut cache = RowCache::new(log, move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        assert!(cache.get(0).is_none());
        cache.request(0..2);
        let cells = wait_for(&cache, 0);
        assert_eq!(cells.time, "2026-01-01 10:00:00.000");
        assert_eq!(cells.component, "app");
        assert_eq!(cells.message, "hello");
        assert_eq!(wait_for(&cache, 1).message, "second");
        assert!(wakes.load(Ordering::SeqCst) >= 1);
    }

    #[test]
    fn rows_past_the_screen_load_ahead_of_scrolling() {
        let text: String = (0..1000).map(|n| format!("row {n}\n")).collect();
        let (_dir, log) = open(&text);
        let mut cache = RowCache::new(log, || {});
        cache.request(0..50);
        assert_eq!(
            wait_for(&cache, ROWS_PER_BLOCK).message,
            format!("row {ROWS_PER_BLOCK}")
        );
    }

    #[test]
    fn multi_line_messages_show_their_first_line() {
        let (_dir, log) = open(
            "<![LOG[first\r\nsecond]LOG]!><time=\"10:23:01.442+000\" date=\"04-06-2026\" component=\"App\" context=\"\" type=\"1\" thread=\"7\" file=\"a.cpp\">\r\n",
        );
        let mut cache = RowCache::new(log, || {});
        cache.request(0..1);
        let cells = wait_for(&cache, 0);
        assert_eq!(cells.message, "first");
        assert_eq!(cells.thread, "7");
    }

    #[test]
    fn very_long_rows_are_capped_for_display() {
        let (_dir, log) = open(&format!("{}\n", "x".repeat(50_000)));
        let mut cache = RowCache::new(log, || {});
        cache.request(0..1);
        assert_eq!(wait_for(&cache, 0).message.len(), MAX_DISPLAY_BYTES);
    }

    #[test]
    fn invalid_utf8_still_displays() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.log");
        std::fs::write(&path, b"caf\xE9 opened\n").unwrap();
        let (log, _) = LogFile::open_with_cache(&path, None, &AtomicU64::new(0)).unwrap();
        let mut cache = RowCache::new(Arc::new(RwLock::new(log)), || {});
        cache.request(0..1);
        assert_eq!(wait_for(&cache, 0).message, "caf\u{e9} opened");
    }

    #[test]
    fn the_cache_stays_within_its_limit() {
        let mut blocks = Blocks::default();
        for block in 0..MAX_BLOCKS + 50 {
            blocks.insert(block, 0, Arc::from(Vec::new()));
        }
        assert_eq!(blocks.entries.len(), MAX_BLOCKS);
        assert!(!blocks.entries.contains_key(&0));
        assert!(blocks.entries.contains_key(&(MAX_BLOCKS + 49)));
    }

    #[test]
    fn repeated_requests_are_ignored() {
        let (_dir, log) = open("a\nb\n");
        let mut cache = RowCache::new(log, || {});
        cache.request(0..2);
        cache.request(0..2);
        assert_eq!(cache.last_request, vec![0]);
    }

    #[test]
    fn scattered_rows_load_only_their_blocks() {
        let text: String = (0..2000)
            .map(|n| {
                format!(
                    "row {n}
"
                )
            })
            .collect();
        let (_dir, log) = open(&text);
        let mut cache = RowCache::new(log, || {});
        cache.request([5, 1500]);
        assert_eq!(wait_for(&cache, 1500).message, "row 1500");
        assert_eq!(wait_for(&cache, 5).message, "row 5");
    }

    #[test]
    fn invalidated_rows_are_read_again() {
        let (_dir, log) = open(
            "a
b
",
        );
        let mut cache = RowCache::new(log.clone(), || {});
        cache.request(0..2);
        assert_eq!(wait_for(&cache, 1).message, "b");
        cache.invalidate_from(0);
        assert!(cache.get(1).is_none());
        cache.request(0..2);
        assert_eq!(wait_for(&cache, 1).message, "b");
    }

    #[test]
    fn new_rules_mark_rows_without_blanking_them() {
        let (_dir, log) = open("first\nsecond\n");
        let mut cache = RowCache::new(log, || {});
        cache.request(0..2);
        assert_eq!(wait_for(&cache, 1).mark, None);
        let rule = Rule {
            pattern: "second".to_string(),
            ..Rule::default()
        };
        cache.set_rules(Arc::new(RuleSet::compile(&[rule])));
        assert!(cache.get(1).is_some(), "rows stay until they're read again");
        cache.request(0..2);
        let started = Instant::now();
        while cache.get(1).and_then(|cells| cells.mark).is_none() {
            assert!(started.elapsed() < Duration::from_secs(5), "never marked");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(cache.get(0).unwrap().mark, None);
    }

    #[test]
    fn requests_past_the_end_are_safe() {
        let (_dir, log) = open("a\nb\n");
        let mut cache = RowCache::new(log, || {});
        cache.request(1_000_000..1_000_060);
        cache.request(0..2);
        assert_eq!(wait_for(&cache, 1).message, "b");
    }
}
