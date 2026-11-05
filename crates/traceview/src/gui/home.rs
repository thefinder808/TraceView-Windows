//! The start page. With no log open, it offers buttons to open a log or a
//! folder, then recent folders and logs, then this PC's usual log folders with
//! their key logs, each one click away.
//!
//! Opening a folder, such as one of logs copied from another PC, lists its
//! logs here instead, newest first. Each is marked with how many errors and
//! warnings it has, counted in the background, so the logs worth reading
//! stand out before any is opened.

use super::app::readable_size;
use super::places::{FolderJob, LogEntry, Place};
use super::settings::Settings;
use super::tab::file_name;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use tv_engine::{CustomFormat, Level, LogFile};
use tv_platform::Waker;
use tv_ui::list_view::{elide, ListItem, ListView};
use tv_ui::{
    Align, Canvas, Color, Event, FontRole, Key, MouseButton, Point, Rect, TextStyle, Theme,
};

/// The page keeps to this width, so its lines stay easy to follow on a wide screen.
const MAX_WIDTH: f32 = 760.0;
const PADDING: f32 = 24.0;
const BUTTON_HEIGHT: f32 = 28.0;
const BUTTON_PADDING: f32 = 14.0;
const BUTTON_GAP: f32 = 8.0;
const SHORTCUT_GAP: f32 = 10.0;
/// The list draws headings this far in, so it starts this far left of the
/// title for the two to line up.
const LIST_INDENT: f32 = 10.0;
/// Recent folders and logs listed at most. Settings keep more, for the palette.
const MOST_RECENT: usize = 10;
/// Logs larger than this aren't counted, so a huge one can't keep the disk busy.
const MOST_COUNTED_BYTES: u64 = 256 * 1024 * 1024;
const UI_LEFT: TextStyle = TextStyle::new(FontRole::Ui, Align::Left);
const KEY_LOG_INDENT: &str = "    ";
const INTRODUCTION: &str = "Open a log, or drop logs or a folder anywhere in this window.";

/// What choosing something on the page does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Shows the Open dialog.
    OpenLog,
    /// Shows the dialog for choosing a folder.
    OpenFolder,
    Open(PathBuf),
    Folder(PathBuf),
    /// Leaves the folder, or the page when it's shown over open logs.
    Back,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Errors and critical rows.
    pub errors: u64,
    pub warnings: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Count {
    Waiting,
    Counted(Counts),
    /// Too large to count, or it couldn't be read.
    NotCounted,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HomeResponse {
    pub redraw: bool,
    pub chosen: Option<Choice>,
}

pub struct Button {
    pub label: &'static str,
    pub shortcut: String,
    pub choice: Choice,
    pub rect: Rect,
}

/// Counts each log's errors and warnings on another thread, one log at a
/// time, and stops when dropped.
struct CountJob {
    results: Receiver<(usize, Option<Counts>)>,
    stop: Arc<AtomicBool>,
}

impl CountJob {
    fn start(logs: &[LogEntry], formats: Vec<Arc<CustomFormat>>, waker: &Waker) -> Self {
        let (sender, results) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let logs: Vec<(PathBuf, u64)> = logs
            .iter()
            .map(|log| (log.path.clone(), log.size))
            .collect();
        let (waker, stopped) = (waker.clone(), stop.clone());
        std::thread::spawn(move || {
            for (index, (path, size)) in logs.into_iter().enumerate() {
                if stopped.load(Ordering::Relaxed) {
                    return;
                }
                let counts = (size <= MOST_COUNTED_BYTES)
                    .then(|| count(&path, &formats))
                    .flatten();
                if sender.send((index, counts)).is_err() {
                    return;
                }
                waker.wake();
            }
        });
        Self { results, stop }
    }
}

impl Drop for CountJob {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Reads a log's errors and warnings from its index, as its severity chips do.
fn count(path: &Path, formats: &[Arc<CustomFormat>]) -> Option<Counts> {
    let (log, _) = LogFile::open_with_formats(path, formats, &AtomicU64::new(0)).ok()?;
    let mut counts = Counts::default();
    for row in 0..log.rows() {
        match log.level(row) {
            Level::Error | Level::Critical => counts.errors += 1,
            Level::Warning => counts.warnings += 1,
            _ => {}
        }
    }
    Some(counts)
}

fn plural(count: u64, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

fn counts_text(count: Count) -> Option<String> {
    match count {
        Count::Waiting => Some("counting".to_string()),
        Count::NotCounted => None,
        Count::Counted(Counts {
            errors: 0,
            warnings: 0,
        }) => Some("no errors or warnings".to_string()),
        Count::Counted(Counts {
            errors: 0,
            warnings,
        }) => Some(plural(warnings, "warning", "warnings")),
        Count::Counted(Counts {
            errors,
            warnings: 0,
        }) => Some(plural(errors, "error", "errors")),
        Count::Counted(Counts { errors, warnings }) => Some(format!(
            "{}, {}",
            plural(errors, "error", "errors"),
            plural(warnings, "warning", "warnings")
        )),
    }
}

fn folder_name(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => path.display().to_string(),
    }
}

fn parent_label(path: &Path) -> String {
    path.parent()
        .map_or_else(String::new, |folder| folder.display().to_string())
}

struct Folder {
    path: PathBuf,
    listing: Option<FolderJob>,
    logs: Vec<LogEntry>,
    counts: Vec<Count>,
    counting: Option<CountJob>,
    formats: Vec<Arc<CustomFormat>>,
    problem: Option<String>,
}

impl Folder {
    fn summary(&self) -> String {
        let path = self.path.display();
        if self.listing.is_some() {
            return format!("{path} · listing its logs");
        }
        if let Some(problem) = &self.problem {
            return format!("{path} · couldn't list it: {problem}");
        }
        if self.logs.is_empty() {
            return format!("{path} · no .log, .lo_ or .txt files here");
        }
        let logs = plural(self.logs.len() as u64, "log", "logs");
        let waiting = self
            .counts
            .iter()
            .filter(|count| **count == Count::Waiting)
            .count();
        if waiting > 0 {
            let done = self.logs.len() - waiting;
            return format!("{path} · {logs}, counting errors in {done} of them so far");
        }
        let with_errors = self
            .counts
            .iter()
            .filter(|count| matches!(count, Count::Counted(counts) if counts.errors > 0))
            .count();
        format!("{path} · {logs}, {with_errors} with errors")
    }
}

#[derive(Default)]
pub struct Home {
    pub list: ListView,
    targets: Vec<Choice>,
    buttons: Vec<Button>,
    folder: Option<Folder>,
    area: Rect,
    title: Rect,
    subtitle: Rect,
    hovered_button: Option<usize>,
}

impl Home {
    /// The folder shown, if one is.
    pub fn folder(&self) -> Option<&Path> {
        self.folder.as_ref().map(|folder| folder.path.as_path())
    }

    /// Lists a folder's logs and starts counting their errors, reading them
    /// with `formats` as tabs would.
    pub fn show_folder(&mut self, path: PathBuf, formats: Vec<Arc<CustomFormat>>, waker: &Waker) {
        self.folder = Some(Folder {
            listing: Some(FolderJob::start(path.clone(), waker)),
            path,
            logs: Vec::new(),
            counts: Vec::new(),
            counting: None,
            formats,
            problem: None,
        });
        self.list = ListView::default();
    }

    /// Goes back to the start page, which stops any counting.
    pub fn close_folder(&mut self) {
        if self.folder.take().is_some() {
            self.list = ListView::default();
        }
    }

    /// Picks up the folder's listing and counts. Returns true if anything changed.
    pub fn poll(&mut self, waker: &Waker) -> bool {
        let Some(folder) = &mut self.folder else {
            return false;
        };
        let mut changed = false;
        if let Some(outcome) = folder.listing.as_ref().and_then(FolderJob::poll) {
            folder.listing = None;
            match outcome {
                Ok(logs) => {
                    folder.counts = vec![Count::Waiting; logs.len()];
                    folder.counting = Some(CountJob::start(&logs, folder.formats.clone(), waker));
                    folder.logs = logs;
                }
                Err(error) => folder.problem = Some(error.to_string()),
            }
            changed = true;
        }
        if let Some(job) = &folder.counting {
            loop {
                match job.results.try_recv() {
                    Ok((index, counts)) => {
                        if let Some(count) = folder.counts.get_mut(index) {
                            *count = counts.map_or(Count::NotCounted, Count::Counted);
                        }
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        folder.counting = None;
                        for count in &mut folder.counts {
                            if *count == Count::Waiting {
                                *count = Count::NotCounted;
                            }
                        }
                        changed = true;
                        break;
                    }
                }
            }
        }
        changed
    }

    pub fn title(&self) -> String {
        self.folder.as_ref().map_or_else(
            || "TraceView".to_string(),
            |folder| folder_name(&folder.path),
        )
    }

    pub fn subtitle(&self) -> String {
        self.folder
            .as_ref()
            .map_or_else(|| INTRODUCTION.to_string(), Folder::summary)
    }

    pub fn buttons(&self) -> &[Button] {
        &self.buttons
    }

    /// What choosing a list item does, for screen readers that choose it.
    pub fn target_of(&self, item: usize) -> Option<Choice> {
        let id = self.list.items.get(item)?.id()?;
        self.targets.get(id as usize).cloned()
    }

    /// Lists what's current. Cheap enough to run every frame. `shortcuts` are
    /// those for opening a log and a folder, to show on the buttons.
    pub fn rebuild(
        &mut self,
        settings: &Settings,
        places: &[Place],
        theme: &Theme,
        shortcuts: [String; 2],
    ) {
        let [open_log, open_folder] = shortcuts;
        let first = match self.folder {
            Some(_) => ("Back", "Esc".to_string(), Choice::Back),
            None => ("Open log...", open_log, Choice::OpenLog),
        };
        let wanted = [first, ("Open folder...", open_folder, Choice::OpenFolder)];
        self.buttons = wanted
            .into_iter()
            .zip(
                self.buttons
                    .iter()
                    .map(|button| button.rect)
                    .chain(std::iter::repeat(Rect::default())),
            )
            .map(|((label, shortcut, choice), rect)| Button {
                label,
                shortcut,
                choice,
                rect,
            })
            .collect();

        let mut items = Vec::new();
        let mut targets = Vec::new();
        match &self.folder {
            Some(folder) => {
                for (log, count) in folder.logs.iter().zip(&folder.counts) {
                    let size = readable_size(log.size);
                    let detail = match counts_text(*count) {
                        Some(counts) => format!("{counts} · {size}"),
                        None => size,
                    };
                    // Marked as the histogram is: red for errors, yellow for
                    // warnings, and gray for neither.
                    let swatch: Color = match count {
                        Count::Counted(counts) if counts.errors > 0 => theme.histogram_error,
                        Count::Counted(counts) if counts.warnings > 0 => theme.histogram_warning,
                        Count::Counted(_) => theme.histogram_info,
                        Count::Waiting | Count::NotCounted => theme.sidebar_background,
                    };
                    let entry =
                        ListItem::entry(&file_name(&log.path), &detail, targets.len() as u32);
                    items.push(entry.with_swatch(swatch));
                    targets.push(Choice::Open(log.path.clone()));
                }
            }
            None => {
                let mut push = |items: &mut Vec<ListItem>, label: &str, detail: &str, choice| {
                    items.push(ListItem::entry(label, detail, targets.len() as u32));
                    targets.push(choice);
                };
                if !settings.recent_folders.is_empty() {
                    items.push(ListItem::heading("RECENT FOLDERS"));
                    for recent in settings.recent_folders.iter().take(MOST_RECENT) {
                        let path = PathBuf::from(recent);
                        let (label, detail) = (folder_name(&path), parent_label(&path));
                        push(&mut items, &label, &detail, Choice::Folder(path));
                    }
                }
                if !settings.recent.is_empty() {
                    items.push(ListItem::heading("RECENT LOGS"));
                    for recent in settings.recent.iter().take(MOST_RECENT) {
                        let path = PathBuf::from(recent);
                        let (label, detail) = (file_name(&path), parent_label(&path));
                        push(&mut items, &label, &detail, Choice::Open(path));
                    }
                }
                if !places.is_empty() {
                    items.push(ListItem::heading("ON THIS PC"));
                    for place in places {
                        let folder = place.folder.display().to_string();
                        push(
                            &mut items,
                            place.name,
                            &folder,
                            Choice::Folder(place.folder.clone()),
                        );
                        for log in &place.key_logs {
                            let label = format!("{KEY_LOG_INDENT}{}", file_name(log));
                            push(&mut items, &label, "", Choice::Open(log.clone()));
                        }
                    }
                }
            }
        }
        self.list.items = items;
        self.targets = targets;
    }

    /// Lays out the title, buttons and list in `area`.
    pub fn arrange(&mut self, area: Rect, canvas: &mut dyn Canvas) {
        self.area = area;
        let width = (area.width() - 2.0 * PADDING).clamp(0.0, MAX_WIDTH);
        let left = area.left + ((area.width() - width) / 2.0).max(0.0);
        let line = canvas.line_height(FontRole::Ui) + 6.0;
        let top = area.top + PADDING;
        self.title = Rect::new(left, top, left + width, top + line);
        self.subtitle = Rect::new(
            left,
            self.title.bottom,
            left + width,
            self.title.bottom + line,
        );
        let button_top = self.subtitle.bottom + 12.0;
        let mut x = left;
        for button in &mut self.buttons {
            let mut text = canvas.text_width(button.label, FontRole::Ui);
            if !button.shortcut.is_empty() {
                text += SHORTCUT_GAP + canvas.text_width(&button.shortcut, FontRole::Ui);
            }
            button.rect = Rect::new(
                x,
                button_top,
                x + text + 2.0 * BUTTON_PADDING,
                button_top + BUTTON_HEIGHT,
            );
            x = button.rect.right + BUTTON_GAP;
        }
        let list_top = button_top + BUTTON_HEIGHT + 16.0;
        self.list.arrange(Rect::new(
            left - LIST_INDENT,
            list_top.min(area.bottom),
            left + width,
            (area.bottom - 8.0).max(list_top.min(area.bottom)),
        ));
    }

    fn button_at(&self, point: Point) -> Option<usize> {
        self.buttons
            .iter()
            .position(|button| button.rect.contains(point))
    }

    /// Handles input meant for the page. Escape goes back.
    pub fn handle(&mut self, event: Event) -> HomeResponse {
        match event {
            Event::KeyDown(Key::Escape, _) => {
                return HomeResponse {
                    redraw: true,
                    chosen: Some(Choice::Back),
                };
            }
            Event::MouseDown(point, MouseButton::Left, _) => {
                if let Some(index) = self.button_at(point) {
                    return HomeResponse {
                        redraw: true,
                        chosen: Some(self.buttons[index].choice.clone()),
                    };
                }
            }
            Event::MouseMove(point) => {
                let hovered = self.button_at(point);
                if hovered != self.hovered_button {
                    self.hovered_button = hovered;
                    let listed = self.list.handle(event);
                    return HomeResponse {
                        redraw: true,
                        chosen: listed
                            .activated
                            .and_then(|id| self.targets.get(id as usize).cloned()),
                    };
                }
            }
            _ => {}
        }
        let listed = self.list.handle(event);
        HomeResponse {
            redraw: listed.redraw,
            chosen: listed
                .activated
                .and_then(|id| self.targets.get(id as usize).cloned()),
        }
    }

    pub fn draw(&self, canvas: &mut dyn Canvas, theme: &Theme, focused: bool) {
        canvas.fill_rect(self.area, theme.sidebar_background);
        let title = self.title();
        let title = elide(canvas, &title, self.title.width(), FontRole::Ui);
        canvas.text(&title, self.title, UI_LEFT, theme.primary_text);
        let subtitle = self.subtitle();
        let subtitle = elide(canvas, &subtitle, self.subtitle.width(), FontRole::Ui);
        canvas.text(&subtitle, self.subtitle, UI_LEFT, theme.secondary_text);
        for (index, button) in self.buttons.iter().enumerate() {
            self.draw_button(
                canvas,
                theme,
                button,
                index == 0,
                self.hovered_button == Some(index),
            );
        }
        self.list.draw(canvas, theme, focused);
    }

    /// The first button is filled, as the page's main action, and the others
    /// are outlined.
    fn draw_button(
        &self,
        canvas: &mut dyn Canvas,
        theme: &Theme,
        button: &Button,
        main: bool,
        hovered: bool,
    ) {
        let rect = button.rect;
        let (fill, text, shortcut) = if main {
            (
                theme.chip_active_background,
                theme.chip_active_text,
                theme.chip_active_text,
            )
        } else {
            canvas.fill_rect(rect, theme.border);
            if hovered {
                (
                    theme.hover_background,
                    theme.on_highlight(theme.primary_text),
                    theme.on_highlight(theme.secondary_text),
                )
            } else {
                (
                    theme.sidebar_background,
                    theme.primary_text,
                    theme.secondary_text,
                )
            }
        };
        let inner = if main {
            rect
        } else {
            Rect::new(
                rect.left + 1.0,
                rect.top + 1.0,
                rect.right - 1.0,
                rect.bottom - 1.0,
            )
        };
        canvas.fill_rect(inner, fill);
        let label = Rect::new(
            rect.left + BUTTON_PADDING,
            rect.top,
            rect.right,
            rect.bottom,
        );
        canvas.text(button.label, label, UI_LEFT, text);
        if !button.shortcut.is_empty() {
            let left = label.left + canvas.text_width(button.label, FontRole::Ui) + SHORTCUT_GAP;
            let room = Rect::new(left, rect.top, rect.right, rect.bottom);
            canvas.text(&button.shortcut, room, UI_LEFT, shortcut);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use tv_ui::recording::RecordingCanvas;
    use tv_ui::Modifiers;

    const AREA: Rect = Rect::new(0.0, 30.0, 1000.0, 700.0);

    fn shortcuts() -> [String; 2] {
        ["Ctrl+O".to_string(), "Ctrl+Shift+O".to_string()]
    }

    fn laid_out(home: &mut Home, settings: &Settings, places: &[Place]) {
        home.rebuild(settings, places, &Theme::DARK, shortcuts());
        home.arrange(AREA, &mut RecordingCanvas::new());
    }

    fn labels(home: &Home) -> Vec<String> {
        home.list
            .items
            .iter()
            .map(|item| match item {
                ListItem::Heading(title) => title.clone(),
                ListItem::Entry { label, .. } => label.clone(),
            })
            .collect()
    }

    fn choice_of(home: &Home, label: &str) -> Option<Choice> {
        let index = labels(home).iter().position(|item| item == label)?;
        home.target_of(index)
    }

    fn click(home: &mut Home, point: Point) -> HomeResponse {
        home.handle(Event::MouseDown(
            point,
            MouseButton::Left,
            Modifiers::default(),
        ))
    }

    fn center(rect: Rect) -> Point {
        Point::new(
            (rect.left + rect.right) / 2.0,
            (rect.top + rect.bottom) / 2.0,
        )
    }

    #[test]
    fn the_start_page_lists_recent_folders_logs_and_this_pcs_folders() {
        let mut settings = Settings::default();
        settings.add_recent(Path::new(r"C:\Logs\WS-1\AppEnforce.log"));
        settings.add_recent_folder(Path::new(r"C:\Logs\WS-1"));
        let places = [Place {
            name: "ConfigMgr client",
            folder: PathBuf::from(r"C:\Windows\CCM\Logs"),
            key_logs: vec![PathBuf::from(r"C:\Windows\CCM\Logs\CAS.log")],
        }];
        let mut home = Home::default();
        laid_out(&mut home, &settings, &places);
        assert_eq!(
            labels(&home),
            [
                "RECENT FOLDERS",
                "WS-1",
                "RECENT LOGS",
                "AppEnforce.log",
                "ON THIS PC",
                "ConfigMgr client",
                "    CAS.log",
            ]
        );
        assert_eq!(
            choice_of(&home, "WS-1"),
            Some(Choice::Folder(PathBuf::from(r"C:\Logs\WS-1")))
        );
        assert_eq!(
            choice_of(&home, "    CAS.log"),
            Some(Choice::Open(PathBuf::from(r"C:\Windows\CCM\Logs\CAS.log")))
        );
        assert_eq!(home.title(), "TraceView");
    }

    #[test]
    fn the_buttons_open_a_log_or_a_folder() {
        let mut home = Home::default();
        laid_out(&mut home, &Settings::default(), &[]);
        let buttons: Vec<_> = home.buttons().iter().map(|button| button.rect).collect();
        assert_eq!(
            click(&mut home, center(buttons[0])).chosen,
            Some(Choice::OpenLog)
        );
        assert_eq!(
            click(&mut home, center(buttons[1])).chosen,
            Some(Choice::OpenFolder)
        );
        assert!(buttons[0].right < buttons[1].left);
        let escape = home.handle(Event::KeyDown(Key::Escape, Modifiers::default()));
        assert_eq!(escape.chosen, Some(Choice::Back));
    }

    #[test]
    fn a_folder_lists_its_logs_with_their_errors_and_warnings() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("bad.log"),
            "2026-01-01 10:00:00 host a[1]: ERROR: one\n2026-01-01 10:00:01 host a[1]: WARN: two\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("fine.log"),
            "2026-01-01 10:00:00 host a[1]: ok\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("notes.md"), "not a log").unwrap();

        let mut home = Home::default();
        let waker = Waker::detached();
        home.show_folder(dir.path().to_path_buf(), Vec::new(), &waker);
        let started = Instant::now();
        while !home.subtitle().contains("with errors") {
            home.poll(&waker);
            assert!(started.elapsed() < Duration::from_secs(10), "never counted");
            std::thread::sleep(Duration::from_millis(5));
        }
        laid_out(&mut home, &Settings::default(), &[]);
        assert!(
            home.subtitle().ends_with("2 logs, 1 with errors"),
            "{}",
            home.subtitle()
        );
        let mut names = labels(&home);
        names.sort();
        assert_eq!(names, ["bad.log", "fine.log"]);
        let details: Vec<_> = home
            .list
            .items
            .iter()
            .filter_map(|item| match item {
                ListItem::Entry {
                    label,
                    detail,
                    swatch,
                    ..
                } => Some((label.as_str(), detail.as_str(), *swatch)),
                ListItem::Heading(_) => None,
            })
            .collect();
        let bad = details
            .iter()
            .find(|(label, ..)| *label == "bad.log")
            .unwrap();
        assert!(bad.1.starts_with("1 error, 1 warning · "), "{}", bad.1);
        assert_eq!(bad.2, Some(Theme::DARK.histogram_error));
        let fine = details
            .iter()
            .find(|(label, ..)| *label == "fine.log")
            .unwrap();
        assert!(fine.1.starts_with("no errors or warnings · "));
        assert_eq!(
            choice_of(&home, "bad.log"),
            Some(Choice::Open(dir.path().join("bad.log")))
        );
        assert_eq!(home.buttons()[0].choice, Choice::Back);
        assert_eq!(home.title(), folder_name(dir.path()));

        home.close_folder();
        assert_eq!(home.folder(), None);
        assert_eq!(home.title(), "TraceView");
    }

    #[test]
    fn a_missing_folder_says_so() {
        let mut home = Home::default();
        let waker = Waker::detached();
        home.show_folder(PathBuf::from(r"Z:\does\not\exist"), Vec::new(), &waker);
        let started = Instant::now();
        while !home.subtitle().contains("couldn't list it") {
            home.poll(&waker);
            assert!(started.elapsed() < Duration::from_secs(5), "never listed");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn counts_read_as_short_phrases() {
        let counted = |errors, warnings| counts_text(Count::Counted(Counts { errors, warnings }));
        assert_eq!(counted(0, 0).as_deref(), Some("no errors or warnings"));
        assert_eq!(counted(3, 0).as_deref(), Some("3 errors"));
        assert_eq!(counted(0, 1).as_deref(), Some("1 warning"));
        assert_eq!(counted(14, 3).as_deref(), Some("14 errors, 3 warnings"));
        assert_eq!(counts_text(Count::NotCounted), None);
    }
}
