//! Saved settings: theme, layout, recent files and filter presets, in one TOML
//! file a person can also edit by hand. It lives in `%APPDATA%\TraceView`, or
//! beside the EXE when a `traceview.toml` is already there, which makes a copy
//! portable.
//!
//! The file is read once at launch, before the first frame, so the window
//! opens in the right theme. Saving happens on a background thread, because
//! `%APPDATA%` can be redirected to a network share.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::JoinHandle;

pub const FILE_NAME: &str = "traceview.toml";
/// Lists are capped so a hand-edited or shared file can't make menus unusable.
const MOST_RECENT: usize = 20;
const MOST_PRESETS: usize = 100;
const MOST_BOOKMARKED_LOGS: usize = 200;
const MOST_BOOKMARKS_PER_LOG: usize = 10_000;
const HEADER: &str = "# TraceView settings. Edit them while TraceView is closed, or your changes may be overwritten.\n\n";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// "System", to follow Windows' light or dark setting, or a theme's name.
    pub theme: String,
    pub details_pane: bool,
    pub sidebar: bool,
    pub sidebar_width: f32,
    pub chips: bool,
    pub histogram: bool,
    /// Logs opened lately, newest first.
    pub recent: Vec<String>,
    pub presets: Vec<Preset>,
    /// Bookmarked rows, by log path in lower case. Rows count from 0.
    pub bookmarks: BTreeMap<String, Vec<u32>>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "System".to_string(),
            details_pane: true,
            sidebar: false,
            sidebar_width: super::sidebar::DEFAULT_WIDTH,
            chips: true,
            histogram: false,
            recent: Vec::new(),
            presets: Vec::new(),
            bookmarks: BTreeMap::new(),
        }
    }
}

/// A saved filter, recalled by name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preset {
    pub name: String,
    /// Filter box text, such as `failed level:warning`.
    pub filter: String,
    /// The severity chip that was on, such as `error`, or empty for none.
    pub level: String,
}

fn bookmark_key(log: &Path) -> String {
    log.to_string_lossy().to_lowercase()
}

/// Windows paths ignore case.
fn same_path(first: &str, second: &str) -> bool {
    first.eq_ignore_ascii_case(second) || first.to_lowercase() == second.to_lowercase()
}

impl Settings {
    /// Puts a log first in the recent list.
    pub fn add_recent(&mut self, path: &Path) {
        // A path that isn't valid Unicode can't be saved as text, so it isn't kept.
        let Some(path) = path.to_str() else { return };
        self.recent.retain(|recent| !same_path(recent, path));
        self.recent.insert(0, path.to_string());
        self.recent.truncate(MOST_RECENT);
    }

    pub fn bookmarks_for(&self, log: &Path) -> BTreeSet<u32> {
        self.bookmarks
            .get(&bookmark_key(log))
            .map_or_else(BTreeSet::new, |rows| rows.iter().copied().collect())
    }

    /// Saves a log's bookmarks, forgetting the log when it has none.
    pub fn set_bookmarks(&mut self, log: &Path, rows: &BTreeSet<u32>) {
        let key = bookmark_key(log);
        if rows.is_empty() {
            self.bookmarks.remove(&key);
        } else if self.bookmarks.contains_key(&key) || self.bookmarks.len() < MOST_BOOKMARKED_LOGS {
            self.bookmarks.insert(
                key,
                rows.iter().copied().take(MOST_BOOKMARKS_PER_LOG).collect(),
            );
        }
    }

    fn tidy(&mut self) {
        self.recent.truncate(MOST_RECENT);
        self.presets.truncate(MOST_PRESETS);
        while self.bookmarks.len() > MOST_BOOKMARKED_LOGS {
            self.bookmarks.pop_last();
        }
        for rows in self.bookmarks.values_mut() {
            rows.truncate(MOST_BOOKMARKS_PER_LOG);
        }
    }
}

/// Reads settings text. Missing entries take their defaults, and unknown ones
/// are ignored, so files from older and newer versions both load.
pub fn parse(text: &str) -> Result<Settings, String> {
    let mut settings: Settings = toml::from_str(text).map_err(|error| {
        let line = error.span().map_or(0, |span| {
            text[..span.start.min(text.len())].lines().count().max(1)
        });
        format!("line {line}: {}", error.message().trim())
    })?;
    settings.tidy();
    Ok(settings)
}

/// Where settings live: beside the EXE if a settings file is there, which
/// makes that copy portable, otherwise `%APPDATA%\TraceView`.
pub fn location() -> Option<PathBuf> {
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|folder| folder.join(FILE_NAME)));
    if let Some(path) = beside_exe.filter(|path| path.is_file()) {
        return Some(path);
    }
    let app_data = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(app_data).join("TraceView").join(FILE_NAME))
}

/// Saves settings in the background, newest first.
pub struct Store {
    writes: Option<Sender<String>>,
    writer: Option<JoinHandle<()>>,
}

impl Store {
    /// A store that never writes, for tests, and for a settings file that
    /// failed to load, so it isn't overwritten before it's fixed.
    pub fn disabled() -> Self {
        Self {
            writes: None,
            writer: None,
        }
    }

    fn writing_to(path: PathBuf) -> Self {
        let (writes, receiver) = channel();
        let writer = std::thread::spawn(move || write_settings(&path, &receiver));
        Self {
            writes: Some(writes),
            writer: Some(writer),
        }
    }

    /// Reads the settings file. A missing file gives the defaults. A file with
    /// a mistake also gives the defaults, plus a message saying what's wrong.
    pub fn open(path: Option<PathBuf>) -> (Self, Settings, Option<String>) {
        let Some(path) = path else {
            return (Self::disabled(), Settings::default(), None);
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => match parse(&text) {
                Ok(settings) => (Self::writing_to(path), settings, None),
                Err(error) => {
                    let message = format!(
                        "Settings weren't loaded ({error}). Fix {} and restart",
                        path.display()
                    );
                    (Self::disabled(), Settings::default(), Some(message))
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                (Self::writing_to(path), Settings::default(), None)
            }
            Err(error) => {
                let message = format!("Settings weren't loaded: {error}");
                (Self::disabled(), Settings::default(), Some(message))
            }
        }
    }

    pub fn save(&self, settings: &Settings) {
        if let (Some(writes), Ok(text)) = (&self.writes, toml::to_string_pretty(settings)) {
            let _ = writes.send(format!("{HEADER}{text}"));
        }
    }
}

impl Drop for Store {
    /// Waits for the last save, so closing the window never loses a change.
    fn drop(&mut self) {
        self.writes = None;
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

fn write_settings(path: &Path, saves: &Receiver<String>) {
    while let Ok(mut text) = saves.recv() {
        // Only the newest settings matter.
        while let Ok(newer) = saves.try_recv() {
            text = newer;
        }
        let _ = write_atomically(path, &text);
    }
}

/// Writes beside the file, then swaps it in, so a crash mid-write never leaves
/// half a file.
fn write_atomically(path: &Path, text: &str) -> io::Result<()> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_a_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings").join(FILE_NAME);
        let mut settings = Settings {
            theme: "Console".to_string(),
            details_pane: false,
            ..Settings::default()
        };
        settings.add_recent(Path::new("C:\\Windows\\CCM\\Logs\\PolicyAgent.log"));
        settings.presets.push(Preset {
            name: "Download errors".to_string(),
            filter: "download".to_string(),
            level: "error".to_string(),
        });
        let (store, _, problem) = Store::open(Some(path.clone()));
        assert_eq!(problem, None);
        store.save(&settings);
        drop(store);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# TraceView settings."));
        let (_, loaded, problem) = Store::open(Some(path));
        assert_eq!((loaded, problem), (settings, None));
    }

    #[test]
    fn missing_and_unknown_entries_are_fine() {
        let settings = parse("theme = \"Classic\"\nsomething_newer = 3\n").unwrap();
        assert_eq!(settings.theme, "Classic");
        assert!(settings.details_pane);
        assert_eq!(parse("").unwrap(), Settings::default());
    }

    #[test]
    fn a_mistake_gives_defaults_and_leaves_the_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let broken = "theme = \"Console\"\ndetails_pane = maybe\n";
        std::fs::write(&path, broken).unwrap();
        let (store, settings, problem) = Store::open(Some(path.clone()));
        assert_eq!(settings, Settings::default());
        let problem = problem.unwrap();
        assert!(problem.contains("line 2"), "{problem}");
        store.save(&Settings::default());
        drop(store);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
    }

    #[test]
    fn recent_logs_are_newest_first_without_repeats() {
        let mut settings = Settings::default();
        for name in ["a.log", "b.log", "A.LOG"] {
            settings.add_recent(Path::new(name));
        }
        assert_eq!(settings.recent, ["A.LOG", "b.log"]);
        for number in 0..50 {
            settings.add_recent(Path::new(&format!("{number}.log")));
        }
        assert_eq!(settings.recent.len(), MOST_RECENT);
        assert_eq!(settings.recent[0], "49.log");
    }

    #[test]
    fn huge_lists_are_cut_to_size() {
        let text: String = (0..500)
            .map(|number| format!("[[presets]]\nname = \"{number}\"\nfilter = \"x\"\n"))
            .collect();
        assert_eq!(parse(&text).unwrap().presets.len(), MOST_PRESETS);
    }

    #[test]
    fn many_quick_saves_end_with_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let (store, mut settings, _) = Store::open(Some(path.clone()));
        for number in 0..100 {
            settings.theme = format!("Theme {number}");
            store.save(&settings);
        }
        drop(store);
        assert_eq!(
            parse(&std::fs::read_to_string(&path).unwrap())
                .unwrap()
                .theme,
            "Theme 99"
        );
    }

    #[test]
    fn a_disabled_store_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        Store::disabled().save(&Settings::default());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
