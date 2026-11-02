//! Live tail: notices when a log changes on disk and brings its index up to
//! date. Checking the file and reading its new rows happen on the watcher's own
//! thread, so the UI thread only puts the prepared refresh in place, which
//! takes microseconds.
//!
//! The watcher polls rather than waiting for change notifications. Windows
//! updates a file's directory entry lazily while a writer holds it open, and
//! notifications from network shares are unreliable. Each check opens the file
//! and reads its size, which takes tens of microseconds.

use super::row_cache::SharedLog;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Duration;
use tv_engine::{Change, RefreshPlan};
use tv_platform::Waker;

const INTERVAL: Duration = Duration::from_millis(250);

/// A change put in place, and how many rows the log had before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Applied {
    pub before: usize,
    pub change: Change,
}

pub struct Watcher {
    plans: Receiver<io::Result<RefreshPlan>>,
    pending: Option<RefreshPlan>,
    /// Set while a plan waits to be put in place, so no other is prepared.
    waiting: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: std::thread::Thread,
}

impl Watcher {
    pub fn start(log: SharedLog, waker: Waker) -> Self {
        let (sender, plans) = channel();
        let waiting = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_waiting, thread_paused, thread_stop) =
            (waiting.clone(), paused.clone(), stop.clone());
        let handle = std::thread::spawn(move || {
            let progress = AtomicU64::new(0);
            // Repeats of these say nothing new, so each is sent once.
            let (mut said_removed, mut said_error) = (false, false);
            loop {
                std::thread::park_timeout(INTERVAL);
                if thread_stop.load(Ordering::Acquire) {
                    return;
                }
                if thread_waiting.load(Ordering::Acquire) {
                    // The last plan isn't in place yet, perhaps because a search
                    // was reading the log, so remind the window.
                    waker.wake();
                    continue;
                }
                if thread_paused.load(Ordering::Relaxed) {
                    continue;
                }
                let Ok(log) = log.read() else { return };
                let prepared = log.prepare_refresh(&progress);
                drop(log);
                let message = match prepared {
                    Ok(None) => {
                        said_error = false;
                        continue;
                    }
                    Ok(Some(plan)) => {
                        said_error = false;
                        let removed = plan.change() == Change::Removed;
                        if removed && said_removed {
                            continue;
                        }
                        said_removed = removed;
                        thread_waiting.store(true, Ordering::Release);
                        Ok(plan)
                    }
                    // A writer may hold the file briefly, so errors are retried
                    // quietly after the first.
                    Err(_) if said_error => continue,
                    Err(error) => {
                        said_error = true;
                        Err(error)
                    }
                };
                if sender.send(message).is_err() {
                    return;
                }
                waker.wake();
            }
        });
        Self {
            plans,
            pending: None,
            waiting,
            paused,
            stop,
            thread: handle.thread().clone(),
        }
    }

    /// Puts a prepared refresh in place, if one is ready and nothing is reading
    /// the log. A search can hold the log for a second or more, so rather than
    /// wait, this tries again when the watcher next wakes the window.
    pub fn apply(&mut self, log: &SharedLog) -> Option<io::Result<Applied>> {
        if self.pending.is_none() {
            match self.plans.try_recv() {
                Ok(Ok(plan)) => self.pending = Some(plan),
                Ok(Err(error)) => return Some(Err(error)),
                Err(_) => return None,
            }
        }
        let Ok(mut log) = log.try_write() else {
            return None;
        };
        let plan = self.pending.take()?;
        let before = log.rows();
        let applied = log.apply_refresh(plan);
        drop(log);
        self.waiting.store(false, Ordering::Release);
        match applied {
            Ok(Some(change)) => Some(Ok(Applied { before, change })),
            // The log changed after the plan was made, and the watcher makes another.
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        }
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.unpark();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::RwLock;
    use std::time::Instant;
    use tv_engine::LogFile;

    fn open(dir: &tempfile::TempDir, text: &str) -> (std::path::PathBuf, SharedLog) {
        let path = dir.path().join("live.log");
        std::fs::write(&path, text).unwrap();
        let (log, _) = LogFile::open_with_cache(&path, None, &AtomicU64::new(0)).unwrap();
        (path, Arc::new(RwLock::new(log)))
    }

    fn wait_for(watcher: &mut Watcher, log: &SharedLog) -> Applied {
        let started = Instant::now();
        loop {
            if let Some(applied) = watcher.apply(log) {
                return applied.unwrap();
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "no change seen"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn append(path: &std::path::Path, text: &str) {
        let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn appended_rows_are_put_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let (path, log) = open(&dir, "one\ntwo\n");
        let mut watcher = Watcher::start(log.clone(), Waker::detached());
        append(&path, "three\nfour\n");
        let applied = wait_for(&mut watcher, &log);
        assert_eq!(
            applied,
            Applied {
                before: 2,
                change: Change::Appended { new_rows: 2 }
            }
        );
        assert_eq!(log.read().unwrap().rows(), 4);
    }

    #[test]
    fn a_busy_log_is_tried_again_rather_than_waited_for() {
        let dir = tempfile::tempdir().unwrap();
        let (path, log) = open(&dir, "one\n");
        let mut watcher = Watcher::start(log.clone(), Waker::detached());
        append(&path, "two\n");
        let reader = log.read().unwrap();
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(600) {
            // Never blocks, and never applies while the log is being read.
            assert!(watcher.apply(&log).is_none());
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(reader);
        assert_eq!(
            wait_for(&mut watcher, &log).change,
            Change::Appended { new_rows: 1 }
        );
    }

    #[test]
    fn removal_is_reported_once_and_a_new_file_is_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let (path, log) = open(&dir, "old one\nold two\n");
        let mut watcher = Watcher::start(log.clone(), Waker::detached());
        std::fs::rename(&path, dir.path().join("live.lo_")).unwrap();
        assert_eq!(wait_for(&mut watcher, &log).change, Change::Removed);
        std::thread::sleep(INTERVAL * 3);
        assert!(watcher.apply(&log).is_none(), "removal is said once");
        std::fs::write(&path, "new\n").unwrap();
        assert_eq!(wait_for(&mut watcher, &log).change, Change::Reset);
        assert_eq!(log.read().unwrap().rows(), 1);
    }

    #[test]
    fn pausing_stops_updates() {
        let dir = tempfile::tempdir().unwrap();
        let (path, log) = open(&dir, "one\n");
        let mut watcher = Watcher::start(log.clone(), Waker::detached());
        watcher.set_paused(true);
        append(&path, "two\n");
        std::thread::sleep(INTERVAL * 3);
        assert!(watcher.apply(&log).is_none());
        watcher.set_paused(false);
        assert_eq!(
            wait_for(&mut watcher, &log).change,
            Change::Appended { new_rows: 1 }
        );
    }
}
