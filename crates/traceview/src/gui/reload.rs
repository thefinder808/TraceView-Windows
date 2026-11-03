//! Live reload: notices when `traceview.toml` or a theme file is changed by
//! hand, or by a copy tool, and reads it again. Checking and reading happen on
//! the reloader's own thread, so the window only puts the result in place.
//!
//! It polls each second rather than waiting for change notifications, as live
//! tail does, because notifications from network shares are unreliable and
//! `%APPDATA%` can be one. A check reads only sizes and times. The settings
//! file is read only when those change, and is then compared with what
//! TraceView last read or wrote, so its own saves never come back as reloads.

use super::settings::{lock, parse, Known, Settings};
use super::themes::{self, FileTheme};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tv_platform::Waker;

#[cfg(not(test))]
const INTERVAL: Duration = Duration::from_secs(1);
/// Tests change files and wait, so they check far more often.
#[cfg(test)]
const INTERVAL: Duration = Duration::from_millis(20);

/// Something that changed on disk.
pub enum Reload {
    /// The settings file was edited, and this is what it holds now, with the
    /// text it was read from.
    Settings(Box<Settings>, String),
    /// The settings file was edited but has a mistake, said here.
    Mistake(String),
    /// Theme files changed: the ones that load, and what's wrong with the rest.
    Themes(Vec<FileTheme>, Vec<String>),
}

/// A file's size and modified time, or `None` if it isn't there.
type Stamp = Option<(u64, SystemTime)>;

fn stamp(path: &Path) -> Stamp {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()?))
}

/// Every theme file in a folder, by name, with its stamp.
fn folder_stamp(folder: &Path) -> Vec<(PathBuf, Stamp)> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut files: Vec<(PathBuf, Stamp)> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
        })
        .map(|path| {
            let stamp = stamp(&path);
            (path, stamp)
        })
        .collect();
    files.sort();
    files
}

/// Reads the settings file again, if it holds something other than what
/// TraceView knows. The comparison happens while the writer is held off, so
/// a save of its own that's under way can't be mistaken for an edit.
/// `mistaken` is whether the last reading had a mistake, so a file put back
/// as it was is loaded too, ending the pause on saving.
fn read_settings(path: &Path, known: &Known, mistaken: bool) -> Option<Reload> {
    let known = lock(known);
    // A file that's gone is left alone, and the next save writes it again.
    let text = std::fs::read_to_string(path).ok()?;
    if !mistaken && known.as_deref() == Some(text.as_str()) {
        return None;
    }
    Some(match parse(&text) {
        Ok(settings) => Reload::Settings(Box::new(settings), text),
        Err(error) => Reload::Mistake(error),
    })
}

pub struct Reloader {
    reloads: Receiver<Reload>,
    stop: Arc<AtomicBool>,
    thread: std::thread::Thread,
}

impl Reloader {
    /// Watches the settings file and the themes folder. `known` is what the
    /// settings store last read or wrote.
    pub fn start(settings: PathBuf, known: Known, themes: PathBuf, waker: Waker) -> Self {
        let (sender, reloads) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        // A store that knows no text for a file that's there found a mistake
        // in it at launch, and already said so.
        let mistake_at_launch = lock(&known).is_none() && settings.is_file();
        let handle = std::thread::spawn(move || {
            // Nothing counts as seen, so the first check compares what's there
            // with what launch read, in case it changed in between.
            let (mut settings_seen, mut themes_seen): (Stamp, _) = (None, Vec::new());
            // The last mistake said, so it isn't said twice.
            let mut said: Option<String> = None;
            let mut first = true;
            loop {
                std::thread::park_timeout(INTERVAL);
                if thread_stop.load(Ordering::Acquire) {
                    return;
                }
                let mut changed = Vec::new();
                let settings_now = stamp(&settings);
                if settings_now != settings_seen {
                    settings_seen = settings_now;
                    let mistaken = said.is_some() || (first && mistake_at_launch);
                    match read_settings(&settings, &known, mistaken) {
                        Some(Reload::Mistake(error)) => {
                            let repeat =
                                said.as_ref() == Some(&error) || (first && mistake_at_launch);
                            if !repeat {
                                changed.push(Reload::Mistake(error.clone()));
                            }
                            said = Some(error);
                        }
                        Some(reload) => {
                            said = None;
                            changed.push(reload);
                        }
                        None => {}
                    }
                }
                first = false;
                let themes_now = folder_stamp(&themes);
                if themes_now != themes_seen {
                    themes_seen = themes_now;
                    let (loaded, problems) = themes::load(&themes);
                    changed.push(Reload::Themes(loaded, problems));
                }
                if changed.is_empty() {
                    continue;
                }
                for reload in changed {
                    if sender.send(reload).is_err() {
                        return;
                    }
                }
                waker.wake();
            }
        });
        Self {
            reloads,
            stop,
            thread: handle.thread().clone(),
        }
    }

    /// What changed since the last call, oldest first.
    pub fn take(&self) -> Vec<Reload> {
        self.reloads.try_iter().collect()
    }
}

impl Drop for Reloader {
    /// Doesn't wait, since the thread may be reading a slow share.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.unpark();
    }
}

#[cfg(test)]
mod tests {
    use super::super::settings::{Store, FILE_NAME};
    use super::*;
    use std::time::Instant;

    /// Waits for the reloader to report something.
    fn next(reloader: &Reloader) -> Reload {
        let started = Instant::now();
        loop {
            if let Some(reload) = reloader.take().into_iter().next() {
                return reload;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "nothing changed"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn quiet(reloader: &Reloader) -> bool {
        std::thread::sleep(INTERVAL * 5);
        reloader.take().is_empty()
    }

    fn watched(dir: &tempfile::TempDir) -> (PathBuf, Store, Reloader) {
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, "theme = \"Classic\"\n").unwrap();
        let (store, _, problem) = Store::open(Some(path.clone()));
        assert_eq!(problem, None);
        let reloader = Reloader::start(
            path.clone(),
            store.known(),
            dir.path().join(themes::FOLDER),
            Waker::detached(),
        );
        (path, store, reloader)
    }

    #[test]
    fn an_edit_is_read_and_a_save_of_its_own_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let (path, store, reloader) = watched(&dir);
        store.save(&Settings {
            theme: "Dark".to_string(),
            ..Settings::default()
        });
        assert!(quiet(&reloader), "its own save isn't a change");
        assert!(std::fs::read_to_string(&path).unwrap().contains("Dark"));

        std::fs::write(&path, "theme = \"Neon\"\n").unwrap();
        match next(&reloader) {
            Reload::Settings(settings, text) => {
                assert_eq!(settings.theme, "Neon");
                assert_eq!(text, "theme = \"Neon\"\n");
            }
            _ => panic!("expected settings"),
        }
    }

    #[test]
    fn an_edit_not_yet_loaded_is_never_saved_over() {
        let dir = tempfile::tempdir().unwrap();
        let (path, store, _reloader) = watched(&dir);
        std::fs::write(&path, "theme = \"Neon\"\n").unwrap();
        store.save(&Settings::default());
        drop(store);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "theme = \"Neon\"\n"
        );
    }

    #[test]
    fn a_mistake_pauses_saving_until_the_file_is_fixed() {
        let dir = tempfile::tempdir().unwrap();
        let (path, store, reloader) = watched(&dir);
        std::fs::write(&path, "theme = \n").unwrap();
        assert!(matches!(next(&reloader), Reload::Mistake(error) if error.contains("line 1")));
        store.save(&Settings::default());
        std::fs::write(&path, "theme = \"Light\"\n").unwrap();
        let Reload::Settings(settings, text) = next(&reloader) else {
            panic!("expected settings");
        };
        assert_eq!(settings.theme, "Light");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "theme = \"Light\"\n",
            "the save during the mistake was dropped"
        );
        // Once the window has taken it in, saving works again.
        store.loaded(text);
        store.save(&Settings::default());
        drop(store);
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("details_pane = true"));
    }

    #[test]
    fn a_file_put_back_after_a_mistake_is_loaded_again() {
        let dir = tempfile::tempdir().unwrap();
        let (path, _store, reloader) = watched(&dir);
        std::fs::write(&path, "theme = \n").unwrap();
        assert!(matches!(next(&reloader), Reload::Mistake(_)));
        std::fs::write(&path, "theme = \"Classic\"\n").unwrap();
        assert!(matches!(next(&reloader), Reload::Settings(..)));
    }

    #[test]
    fn theme_files_are_read_again_when_one_changes() {
        let dir = tempfile::tempdir().unwrap();
        let (_path, _store, reloader) = watched(&dir);
        let folder = dir.path().join(themes::FOLDER);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("Night.toml"),
            "name = \"Night\"\nbased_on = \"Dark\"\n",
        )
        .unwrap();
        let Reload::Themes(loaded, problems) = next(&reloader) else {
            panic!("expected themes");
        };
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].theme.name, "Night");
        assert!(problems.is_empty());
    }
}
