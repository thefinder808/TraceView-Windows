//! Reads rows on a background thread, so the UI thread never touches the disk.
//! When a row isn't ready, the grid draws a placeholder and moves on, which means
//! scrolling can't stall, even on a slow disk or network share.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
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
    let fields = log.fields(log.text(raw, scratch));
    RowCells {
        time: fields.stamp.map(display_time).unwrap_or_default(),
        component: encoding::display(fields.component).into_owned(),
        thread: encoding::display(fields.thread).into_owned(),
        message: encoding::display(first_line(fields.message)).into_owned(),
    }
}

/// One block of loaded rows.
type Block = Arc<[Arc<RowCells>]>;

#[derive(Default)]
struct Blocks {
    entries: HashMap<usize, (u64, Block)>,
    clock: u64,
}

impl Blocks {
    fn insert(&mut self, block: usize, rows: Block) {
        self.clock += 1;
        self.entries.insert(block, (self.clock, rows));
        if self.entries.len() > MAX_BLOCKS {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, (used, _))| *used)
                .map(|(block, _)| *block);
            if let Some(oldest) = oldest {
                self.entries.remove(&oldest);
            }
        }
    }
}

pub struct RowCache {
    blocks: Arc<Mutex<Blocks>>,
    requests: Sender<Range<usize>>,
    last_request: Option<Range<usize>>,
}

impl RowCache {
    /// Starts the loader. `wake` runs on the loader's thread whenever new rows
    /// are ready. The thread ends when the cache is dropped.
    pub fn new(log: Arc<LogFile>, wake: impl Fn() + Send + 'static) -> Self {
        let blocks = Arc::new(Mutex::new(Blocks::default()));
        let (requests, incoming) = channel();
        let shared = blocks.clone();
        std::thread::spawn(move || load_rows(&log, &incoming, &shared, wake));
        Self {
            blocks,
            requests,
            last_request: None,
        }
    }

    /// The row's cells, if they've been loaded.
    pub fn get(&self, row: usize) -> Option<Arc<RowCells>> {
        let mut blocks = self.blocks.lock().ok()?;
        blocks.clock += 1;
        let clock = blocks.clock;
        let entry = blocks.entries.get_mut(&(row / ROWS_PER_BLOCK))?;
        entry.0 = clock;
        entry.1.get(row % ROWS_PER_BLOCK).cloned()
    }

    /// Says which rows are on screen. The loader fetches them, then the rows
    /// around them, so ordinary scrolling usually finds the next rows ready.
    pub fn request(&mut self, rows: Range<usize>) {
        if self.last_request.as_ref() != Some(&rows) {
            self.last_request = Some(rows.clone());
            let _ = self.requests.send(rows);
        }
    }
}

fn block_range(rows: &Range<usize>) -> Range<usize> {
    rows.start / ROWS_PER_BLOCK..rows.end.div_ceil(ROWS_PER_BLOCK)
}

fn newest_request(
    incoming: &Receiver<Range<usize>>,
    mut current: Option<Range<usize>>,
) -> Result<Option<Range<usize>>, ()> {
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
    buffer: &mut Vec<u8>,
    scratch: &mut Vec<u8>,
) -> Option<Block> {
    let start = block * ROWS_PER_BLOCK;
    let end = (start + ROWS_PER_BLOCK).min(log.rows());
    let mut rows = Vec::with_capacity(end.saturating_sub(start));
    log.read_rows(start..end, buffer, |_, raw, _| {
        rows.push(Arc::new(cells_from_row(log, raw, scratch)));
    })
    .ok()?;
    Some(rows.into())
}

fn load_rows(
    log: &LogFile,
    incoming: &Receiver<Range<usize>>,
    blocks: &Mutex<Blocks>,
    wake: impl Fn(),
) {
    let (mut buffer, mut scratch) = (Vec::new(), Vec::new());
    while let Ok(first) = incoming.recv() {
        let mut request = Some(first);
        while let Some(rows) = request.take() {
            let visible = block_range(&rows);
            let span = visible.len();
            let ahead = visible.end..(visible.end + span).min(log.rows().div_ceil(ROWS_PER_BLOCK));
            let behind = visible.start.saturating_sub(span)..visible.start;
            let mut superseded = false;
            for block in visible.clone().chain(ahead).chain(behind) {
                let present = blocks
                    .lock()
                    .map(|blocks| blocks.entries.contains_key(&block))
                    .unwrap_or(true);
                if !present {
                    if let Some(loaded) = read_block(log, block, &mut buffer, &mut scratch) {
                        if let Ok(mut blocks) = blocks.lock() {
                            blocks.insert(block, loaded);
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
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    fn open(text: &str) -> (tempfile::TempDir, Arc<LogFile>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.log");
        std::fs::write(&path, text).unwrap();
        let (log, _) = LogFile::open_with_cache(&path, None, &AtomicU64::new(0)).unwrap();
        (dir, Arc::new(log))
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
        let mut cache = RowCache::new(Arc::new(log), || {});
        cache.request(0..1);
        assert_eq!(wait_for(&cache, 0).message, "caf\u{e9} opened");
    }

    #[test]
    fn the_cache_stays_within_its_limit() {
        let mut blocks = Blocks::default();
        for block in 0..MAX_BLOCKS + 50 {
            blocks.insert(block, Arc::from(Vec::new()));
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
        assert_eq!(cache.last_request, Some(0..2));
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
