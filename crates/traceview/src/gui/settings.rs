//! Saved settings: theme, layout and named layouts, recent files, filter
//! presets and highlight rules, in one TOML file a person can also edit by hand. It lives in `%APPDATA%\TraceView`, or
//! beside the EXE when a `traceview.toml` is already there, which makes a copy
//! portable.
//!
//! The file is read at launch, before the first frame, so the window opens in
//! the right theme, and again whenever it's edited elsewhere (see `reload`).
//! Saving happens on a background thread, because `%APPDATA%` can be
//! redirected to a network share, and never writes over an edit it hasn't
//! seen.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

pub const FILE_NAME: &str = "traceview.toml";
/// Lists are capped so a hand-edited or shared file can't make menus unusable.
const MOST_RECENT: usize = 20;
const MOST_PRESETS: usize = 100;
const MOST_BOOKMARKED_LOGS: usize = 200;
const MOST_BOOKMARKS_PER_LOG: usize = 10_000;
pub const MOST_RULES: usize = 100;
const MOST_COLUMN_LAYOUTS: usize = 20;
const MOST_COLUMNS: usize = 20;
const MOST_KEYS: usize = 200;
pub const MOST_FORMATS: usize = 50;
pub const MOST_LAYOUTS: usize = 50;
const HEADER: &str = "# TraceView settings. A running TraceView loads changes saved here within a\n# second. If a line has a mistake, it says so and saves nothing until it's fixed.\n\n";

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
    /// Turns every highlight rule on or off at once.
    pub highlighting: bool,
    /// Highlight rules, first match first.
    pub rules: Vec<Rule>,
    /// The grid's columns for each log format, by format name, such as `sccm`.
    pub columns: BTreeMap<String, Vec<ColumnSetting>>,
    pub fonts: FontSettings,
    /// Shortcuts changed from the defaults, by command name, such as
    /// `find = "Ctrl+J"`. An empty one means no shortcut.
    pub keys: BTreeMap<String, String>,
    /// Log formats defined by a pattern, tried in order.
    pub formats: Vec<FormatSetting>,
    /// Arrangements of panes saved under a name, after the built-in ones.
    pub layouts: Vec<LayoutSetting>,
}

/// Which panes show, saved under a name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutSetting {
    pub name: String,
    pub sidebar: bool,
    /// In pixels at 100% display scale, or none to leave the width as it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidebar_width: Option<f32>,
    pub chips: bool,
    pub histogram: bool,
    pub details_pane: bool,
}

/// A log format defined by a pattern. Named groups say where each part of a
/// line is: `time`, `level`, `component`, `thread`, `source` and `message`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FormatSetting {
    pub name: String,
    pub pattern: String,
    /// File names it's for, separated by `;`, with `*` and `?` wildcards.
    /// Logs with other names still use it when most of their lines match.
    pub files: String,
    /// Reads dates such as `31/01/2026` day first.
    pub day_first: bool,
}

impl FormatSetting {
    pub fn compile(&self) -> Result<tv_engine::CustomFormat, String> {
        tv_engine::CustomFormat::new(&self.name, &self.pattern, &self.files, self.day_first)
    }
}

/// The formats in settings that can be used, ready to read logs with. One
/// with a mistake is left out until it's fixed.
pub fn compile_formats(formats: &[FormatSetting]) -> Vec<std::sync::Arc<tv_engine::CustomFormat>> {
    formats
        .iter()
        .filter_map(|format| format.compile().ok())
        .map(std::sync::Arc::new)
        .collect()
}

/// Fonts by the names Windows lists them under, with sizes in pixels at 100%
/// display scale.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontSettings {
    pub log_font: String,
    pub log_size: f32,
    /// `compact`, `normal` or `roomy`.
    pub row_spacing: String,
    pub window_font: String,
    pub window_size: f32,
}

impl Default for FontSettings {
    fn default() -> Self {
        Self {
            log_font: "Consolas".to_string(),
            log_size: 13.0,
            row_spacing: "normal".to_string(),
            window_font: "Segoe UI".to_string(),
            window_size: 12.0,
        }
    }
}

/// One grid column, as saved.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColumnSetting {
    /// Such as `time`, `thread` or `message`.
    pub name: String,
    /// In characters of the grid font.
    pub width: f32,
    pub shown: bool,
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
            highlighting: true,
            rules: Vec::new(),
            columns: BTreeMap::new(),
            fonts: FontSettings::default(),
            keys: BTreeMap::new(),
            formats: Vec::new(),
            layouts: Vec::new(),
        }
    }
}

/// A highlight rule. The first turned-on rule that matches a row decides how
/// it's drawn. Names are kept as text, so a file from a newer version with a
/// color this one doesn't know still loads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    /// Filter box text, such as `download failed level:error`.
    pub pattern: String,
    /// A color's name, such as `blue`.
    pub color: String,
    /// `row` colors the whole row, and `marker` puts a mark by its number.
    pub style: String,
    pub enabled: bool,
}

impl Default for Rule {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            color: "yellow".to_string(),
            style: "row".to_string(),
            enabled: true,
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
        self.rules.truncate(MOST_RULES);
        while self.columns.len() > MOST_COLUMN_LAYOUTS {
            self.columns.pop_last();
        }
        for columns in self.columns.values_mut() {
            columns.truncate(MOST_COLUMNS);
        }
        while self.keys.len() > MOST_KEYS {
            self.keys.pop_last();
        }
        self.formats.truncate(MOST_FORMATS);
        self.layouts.truncate(MOST_LAYOUTS);
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

/// What the file holds as far as TraceView knows: the text it last read or
/// wrote there. A file that doesn't match was changed elsewhere, so it's
/// loaded rather than written over. `None` until a good file has been read.
pub type Known = Arc<Mutex<Option<String>>>;

pub fn lock(known: &Known) -> MutexGuard<'_, Option<String>> {
    known.lock().unwrap_or_else(PoisonError::into_inner)
}

enum Write {
    Save(String),
    /// The window took in this text from the file, so saving over it is safe.
    Loaded(Option<String>),
}

/// Saves settings in the background, newest first, and never over an edit
/// made elsewhere that hasn't been loaded yet.
pub struct Store {
    writes: Option<Sender<Write>>,
    writer: Option<JoinHandle<()>>,
    /// The settings file, so files that go with it, such as themes, sit beside it.
    path: Option<PathBuf>,
    known: Known,
}

impl Store {
    /// A store that never writes, for tests, and for a settings file that
    /// can't be read at all.
    pub fn disabled() -> Self {
        Self {
            writes: None,
            writer: None,
            path: None,
            known: Known::default(),
        }
    }

    fn writing_to(path: PathBuf, text: Option<String>) -> Self {
        let (writes, receiver) = channel();
        let known: Known = Arc::new(Mutex::new(text));
        let (target, shared) = (path.clone(), known.clone());
        let writer = std::thread::spawn(move || write_settings(&target, &receiver, &shared));
        Self {
            writes: Some(writes),
            writer: Some(writer),
            path: Some(path),
            known,
        }
    }

    /// The folder the settings file is in, which files that go with it share.
    /// `None` for a store that doesn't write.
    pub fn folder(&self) -> Option<PathBuf> {
        self.path
            .as_ref()
            .and_then(|path| path.parent().map(Path::to_path_buf))
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// What the file holds as far as this store knows, for live reload to
    /// tell its own saves from edits made elsewhere.
    pub fn known(&self) -> Known {
        self.known.clone()
    }

    /// Reads the settings file. A missing file gives the defaults. A file with
    /// a mistake also gives the defaults, plus a message saying what's wrong,
    /// and nothing is saved over it until it's fixed and loaded.
    pub fn open(path: Option<PathBuf>) -> (Self, Settings, Option<String>) {
        let Some(path) = path else {
            return (Self::disabled(), Settings::default(), None);
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => match parse(&text) {
                Ok(settings) => (Self::writing_to(path, Some(text)), settings, None),
                Err(error) => {
                    let message = format!(
                        "Settings weren't loaded ({error}). Fix {}, and they'll load. Until then nothing is saved",
                        path.display()
                    );
                    (
                        Self::writing_to(path, None),
                        Settings::default(),
                        Some(message),
                    )
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                (Self::writing_to(path, None), Settings::default(), None)
            }
            Err(error) => {
                let message = format!("Settings weren't loaded: {error}");
                (Self::disabled(), Settings::default(), Some(message))
            }
        }
    }

    pub fn save(&self, settings: &Settings) {
        if let (Some(writes), Ok(text)) = (&self.writes, toml::to_string_pretty(settings)) {
            let _ = writes.send(Write::Save(format!("{HEADER}{text}")));
        }
    }

    /// Says the window took in this text from the file, so later saves may
    /// replace it. It goes through the writer, after any save sent before it,
    /// so the window never waits for the file.
    pub fn loaded(&self, text: String) {
        if let Some(writes) = &self.writes {
            let _ = writes.send(Write::Loaded(Some(text)));
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

fn write_settings(path: &Path, writes: &Receiver<Write>, known: &Known) {
    while let Ok(first) = writes.recv() {
        let batch: Vec<Write> = std::iter::once(first).chain(writes.try_iter()).collect();
        // Only the newest save matters, but loads before it come first.
        let newest = batch
            .iter()
            .rposition(|write| matches!(write, Write::Save(_)));
        for (index, write) in batch.into_iter().enumerate() {
            match write {
                Write::Loaded(text) => *lock(known) = text,
                Write::Save(text) if Some(index) == newest => {
                    save_unless_changed(path, &text, known)
                }
                Write::Save(_) => {}
            }
        }
    }
}

/// Writes the settings, unless the file holds something other than what was
/// last read or written, which means it was edited elsewhere. That edit wins:
/// live reload loads it, and this save is dropped. The new text is written
/// beside the file first, so the check comes just before the swap. An edit
/// saved in the instant between them would still be replaced.
fn save_unless_changed(path: &Path, text: &str, known: &Known) {
    let mut known = lock(known);
    let temporary = path.with_extension("toml.tmp");
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&temporary, text));
    if written.is_err() {
        return;
    }
    let free = match std::fs::read_to_string(path) {
        Ok(current) => known.as_deref() == Some(current.as_str()),
        Err(error) => error.kind() == io::ErrorKind::NotFound,
    };
    if free && std::fs::rename(&temporary, path).is_ok() {
        *known = Some(text.to_string());
    } else {
        let _ = std::fs::remove_file(&temporary);
    }
}

/// Writes beside the file, then swaps it in, so a crash mid-write never leaves
/// half a file.
pub(super) fn write_atomically(path: &Path, text: &str) -> io::Result<()> {
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
        settings.rules.push(Rule {
            pattern: "/0x8007[0-9a-f]{4}/ level:warning".to_string(),
            color: "blue".to_string(),
            style: "marker".to_string(),
            enabled: false,
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
