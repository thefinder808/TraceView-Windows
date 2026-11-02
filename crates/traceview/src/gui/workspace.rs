//! Everything in the window except the window itself: tabs, menus, keyboard
//! focus, layout, drawing and routing input. It draws on any `Canvas`, so tests
//! drive it without a display.

use super::details::PANE_HEIGHT;
use super::log_view::{snap, Metrics, RowSource};
use super::lookup::{first_code, LookupPanel};
use super::settings::{Settings, Store};
use super::row_cache::{cells_from_row, SharedLog};
use super::tab::{with_commas, Tab};
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use tv_platform::{Command, Commands, Waker};
use tv_ui::menu::{self, Menu, MenuBar, MenuItem};
use tv_ui::tab_bar::{self, Tab as TabLabel, TabBar};
use tv_ui::text_field::FieldResponse;
use tv_ui::{
    Align, Canvas, Event, FontRole, Key, Modifiers, MouseButton, Point, Rect, TextStyle, Theme,
};

const TOOLBAR_HEIGHT: f32 = 34.0;
const FIELD_HEIGHT: f32 = 24.0;
const MARGIN: f32 = 8.0;
const UI_LEFT: TextStyle = TextStyle::new(FontRole::Ui, Align::Left);
/// More than this many rows in one copy would make a clipboard entry too large
/// for most apps to accept anyway.
const MAX_COPY_ROWS: usize = 100_000;
const WELCOME: &str = "Open a log with Ctrl+O, or drop one here";
const FILTER_HINT: &str = "Filter (Ctrl+Shift+F): words, /regex/, level:warning, component:Name";
const FIND_HINT: &str = "Find (Ctrl+F, then F3)";

/// Menu command numbers.
pub mod command {
    pub const OPEN: u32 = 1;
    pub const CLOSE_TAB: u32 = 2;
    pub const EXIT: u32 = 3;
    pub const MERGE: u32 = 4;
    pub const COPY: u32 = 10;
    pub const SELECT_ALL: u32 = 11;
    pub const FIND: u32 = 12;
    pub const FIND_NEXT: u32 = 13;
    pub const FIND_PREVIOUS: u32 = 14;
    pub const NEXT_ERROR: u32 = 15;
    pub const PREVIOUS_ERROR: u32 = 16;
    pub const FILTER: u32 = 20;
    pub const CLEAR_FILTER: u32 = 21;
    pub const DETAILS: u32 = 22;
    pub const THEME_SYSTEM: u32 = 23;
    pub const THEME_CLASSIC: u32 = 24;
    pub const THEME_CONSOLE: u32 = 25;
    pub const ERROR_LOOKUP: u32 = 30;
    pub const PAUSE: u32 = 31;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Grid,
    Filter,
    Find,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    System,
    Classic,
    Console,
}

#[derive(Clone, Copy, Debug, Default)]
struct Layout {
    menu: Rect,
    tabs: Rect,
    filter: Rect,
    find: Rect,
    find_count: Rect,
    grid: Rect,
    details: Rect,
    status: Rect,
}

struct CopyResult {
    text: String,
    rows: usize,
    cut_short: bool,
}

pub struct Workspace {
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub focus: Focus,
    pub show_details: bool,
    pub theme_choice: ThemeChoice,
    pub system_is_dark: bool,
    /// The Error Lookup panel, while it's open.
    pub lookup: Option<LookupPanel>,
    settings: Settings,
    store: Store,
    menu: MenuBar,
    tab_bar: TabBar,
    layout: Layout,
    waker: Waker,
    hwnd: isize,
    copying: Option<Receiver<io::Result<CopyResult>>>,
    notice: Option<String>,
    scale: f32,
}

fn checked(mut item: MenuItem, is_checked: bool) -> MenuItem {
    if let MenuItem::Command { checked, .. } = &mut item {
        *checked = is_checked;
    }
    item
}

/// Builds tab-separated text, so pasted rows line up in a spreadsheet or editor.
/// Each row is (its number in the tab, its file, its row in that file).
fn copy_rows(logs: &[SharedLog], rows: &[(usize, usize, usize)]) -> io::Result<String> {
    let (mut buffer, mut scratch) = (Vec::new(), Vec::new());
    let mut text = String::new();
    for (number, source, row) in rows {
        let log = logs[*source]
            .read()
            .map_err(|_| io::Error::other("the log is unavailable"))?;
        log.read_rows(*row..row + 1, &mut buffer, |row, raw, _| {
            let cells = cells_from_row(&log, raw, &mut scratch);
            text.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\r\n",
                number + 1,
                cells.time,
                super::log_view::level_name(log.level(row)),
                cells.component,
                cells.message
            ));
        })?;
    }
    Ok(text)
}

impl Workspace {
    pub fn new(waker: Waker, hwnd: isize, system_is_dark: bool) -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
            focus: Focus::Grid,
            show_details: true,
            theme_choice: ThemeChoice::System,
            system_is_dark,
            lookup: None,
            settings: Settings::default(),
            store: Store::disabled(),
            menu: MenuBar::default(),
            tab_bar: TabBar::default(),
            layout: Layout::default(),
            waker,
            hwnd,
            copying: None,
            notice: None,
            scale: 1.0,
        }
    }

    /// Starts from saved settings, and saves changes back through `store`.
    /// `problem` says why the settings file couldn't be used, if it couldn't.
    pub fn use_settings(&mut self, settings: Settings, store: Store, problem: Option<String>) {
        self.theme_choice = match settings.theme.as_str() {
            "Classic" => ThemeChoice::Classic,
            "Console" => ThemeChoice::Console,
            _ => ThemeChoice::System,
        };
        self.show_details = settings.details_pane;
        self.settings = settings;
        self.store = store;
        self.notice = problem;
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    fn save_settings(&mut self) {
        self.settings.theme = match self.theme_choice {
            ThemeChoice::System => "System",
            ThemeChoice::Classic => "Classic",
            ThemeChoice::Console => "Console",
        }
        .to_string();
        self.settings.details_pane = self.show_details;
        self.store.save(&self.settings);
    }

    pub fn theme(&self) -> &'static Theme {
        match self.theme_choice {
            ThemeChoice::System => Theme::for_system(self.system_is_dark),
            ThemeChoice::Classic => &Theme::CLASSIC,
            ThemeChoice::Console => &Theme::CONSOLE,
        }
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    pub fn title(&self) -> String {
        self.active_tab().map_or_else(
            || "TraceView".to_string(),
            |tab| format!("TraceView - {}", tab.title()),
        )
    }

    pub fn status(&self) -> String {
        let mut status = self.active_tab().map(Tab::status).unwrap_or_default();
        if let Some(notice) = &self.notice {
            if !status.is_empty() {
                status.push_str("   |   ");
            }
            status.push_str(notice);
        }
        status
    }

    pub fn menu(&self) -> &MenuBar {
        &self.menu
    }

    pub fn close_menu(&mut self) {
        self.menu.close();
    }

    fn build_menus(&self) -> Vec<Menu> {
        let theme = self.theme_choice;
        let paused = self.active_tab().is_some_and(Tab::is_paused);
        vec![
            Menu {
                title: "File".to_string(),
                items: vec![
                    MenuItem::command("Open...", "Ctrl+O", command::OPEN),
                    MenuItem::command("Merge open logs", "Ctrl+Shift+M", command::MERGE),
                    MenuItem::command("Close tab", "Ctrl+W", command::CLOSE_TAB),
                    MenuItem::Separator,
                    MenuItem::command("Exit", "Alt+F4", command::EXIT),
                ],
            },
            Menu {
                title: "Edit".to_string(),
                items: vec![
                    MenuItem::command("Copy rows", "Ctrl+C", command::COPY),
                    MenuItem::command("Select all", "Ctrl+A", command::SELECT_ALL),
                    MenuItem::Separator,
                    MenuItem::command("Find...", "Ctrl+F", command::FIND),
                    MenuItem::command("Find next", "F3", command::FIND_NEXT),
                    MenuItem::command("Find previous", "Shift+F3", command::FIND_PREVIOUS),
                    MenuItem::command("Next error", "Ctrl+E", command::NEXT_ERROR),
                    MenuItem::command("Previous error", "Ctrl+Shift+E", command::PREVIOUS_ERROR),
                ],
            },
            Menu {
                title: "View".to_string(),
                items: vec![
                    MenuItem::command("Filter...", "Ctrl+Shift+F", command::FILTER),
                    MenuItem::command("Clear filter", "", command::CLEAR_FILTER),
                    MenuItem::Separator,
                    checked(
                        MenuItem::command("Details pane", "", command::DETAILS),
                        self.show_details,
                    ),
                    checked(
                        MenuItem::command("Pause live updates", "Ctrl+U", command::PAUSE),
                        paused,
                    ),
                    MenuItem::Separator,
                    checked(
                        MenuItem::command("Theme: follow Windows", "", command::THEME_SYSTEM),
                        theme == ThemeChoice::System,
                    ),
                    checked(
                        MenuItem::command("Theme: Classic", "", command::THEME_CLASSIC),
                        theme == ThemeChoice::Classic,
                    ),
                    checked(
                        MenuItem::command("Theme: Console", "", command::THEME_CONSOLE),
                        theme == ThemeChoice::Console,
                    ),
                ],
            },
            Menu {
                title: "Tools".to_string(),
                items: vec![MenuItem::command(
                    "Error lookup...",
                    "Ctrl+L",
                    command::ERROR_LOOKUP,
                )],
            },
        ]
    }

    /// Opens files in new tabs, or switches to one that's already open.
    pub fn open_paths(&mut self, paths: Vec<PathBuf>) {
        for path in paths {
            let absolute = std::path::absolute(&path).unwrap_or(path);
            self.settings.add_recent(&absolute);
            if let Some(index) = self
                .tabs
                .iter()
                .position(|tab| tab.paths == [absolute.clone()])
            {
                self.active = index;
                continue;
            }
            self.tabs.push(Tab::open(vec![absolute], &self.waker));
            self.active = self.tabs.len() - 1;
        }
        self.focus = Focus::Grid;
        self.save_settings();
    }

    /// Opens a tab with every open log merged into one timeline.
    fn merge_open_logs(&mut self) {
        let paths: Vec<PathBuf> = self
            .tabs
            .iter()
            .filter(|tab| tab.paths.len() == 1 && tab.document().is_some())
            .map(|tab| tab.paths[0].clone())
            .collect();
        if paths.len() < 2 {
            self.notice = Some("Open two or more logs to merge them".to_string());
            return;
        }
        match self.tabs.iter().position(|tab| tab.paths == paths) {
            Some(index) => self.active = index,
            None => {
                self.tabs.push(Tab::open(paths, &self.waker));
                self.active = self.tabs.len() - 1;
            }
        }
        self.focus = Focus::Grid;
    }

    fn close_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.tabs.remove(index);
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len().saturating_sub(1);
            } else if self.active > index {
                self.active -= 1;
            }
        }
        self.focus = Focus::Grid;
    }

    fn set_focus(&mut self, focus: Focus, commands: &mut Commands) {
        if self.focus != focus {
            self.focus = focus;
            commands.push(Command::SetImeEnabled(focus != Focus::Grid));
        }
        if focus != Focus::Grid {
            if let Some(tab) = self.active_tab_mut() {
                match focus {
                    Focus::Filter => tab.filter_field.select_all(),
                    Focus::Find => tab.find_field.select_all(),
                    Focus::Grid => {}
                }
            }
        }
        commands.redraw();
    }

    fn start_copy(&mut self) {
        let Some(tab) = self.active_tab() else { return };
        let (Some(document), Some(selection)) = (tab.document(), tab.view.selection()) else {
            return;
        };
        let total = selection.end() - selection.start() + 1;
        let count = total.min(MAX_COPY_ROWS);
        let start = *selection.start();
        let numbers: Vec<usize> = match tab.filter_rows() {
            Some(filter) => filter[start..start + count]
                .iter()
                .map(|row| *row as usize)
                .collect(),
            None => (start..start + count).collect(),
        };
        let rows: Vec<(usize, usize, usize)> = numbers
            .into_iter()
            .filter_map(|number| {
                document
                    .locate(number)
                    .map(|(source, row)| (number, source, row))
            })
            .collect();
        let logs: Vec<SharedLog> = document
            .sources
            .iter()
            .map(|source| source.log.clone())
            .collect();
        let waker = self.waker.clone();
        let (sender, result) = channel();
        std::thread::spawn(move || {
            let copied = copy_rows(&logs, &rows).map(|text| CopyResult {
                text,
                rows: count,
                cut_short: total > count,
            });
            let _ = sender.send(copied);
            waker.wake();
        });
        self.copying = Some(result);
        self.notice = Some("Copying...".to_string());
    }

    pub fn run_command(&mut self, id: u32, commands: &mut Commands) {
        let waker = self.waker.clone();
        match id {
            command::OPEN => commands.push(Command::OpenFileDialog),
            command::CLOSE_TAB => self.close_tab(self.active),
            command::EXIT => commands.push(Command::Close),
            command::MERGE => self.merge_open_logs(),
            command::COPY => self.start_copy(),
            command::SELECT_ALL => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.view.handle(Event::KeyDown(
                        Key::Char('A'),
                        Modifiers {
                            ctrl: true,
                            ..Modifiers::default()
                        },
                    ));
                }
            }
            command::FIND => self.set_focus(Focus::Find, commands),
            command::FIND_NEXT | command::FIND_PREVIOUS => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.find_next(id == command::FIND_NEXT, &waker);
                }
            }
            command::NEXT_ERROR | command::PREVIOUS_ERROR => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.step_error(id == command::NEXT_ERROR);
                }
            }
            command::FILTER => self.set_focus(Focus::Filter, commands),
            command::CLEAR_FILTER => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.filter_field.set_text("");
                    tab.apply_filter(&waker);
                }
            }
            command::DETAILS => self.show_details = !self.show_details,
            command::PAUSE => {
                if let Some(tab) = self.active_tab_mut() {
                    let paused = !tab.is_paused();
                    tab.set_paused(paused);
                }
            }
            command::THEME_SYSTEM => self.theme_choice = ThemeChoice::System,
            command::THEME_CLASSIC => self.theme_choice = ThemeChoice::Classic,
            command::THEME_CONSOLE => self.theme_choice = ThemeChoice::Console,
            command::ERROR_LOOKUP => {
                // Start with the selected row's first code, as most lookups are for that.
                let code = self
                    .active_tab()
                    .and_then(|tab| tab.details.shown())
                    .and_then(|detail| first_code(&detail.message))
                    .unwrap_or_default();
                let mut panel = LookupPanel::new(&code, &waker);
                panel.arrange(self.layout.grid);
                self.lookup = Some(panel);
                commands.push(Command::SetImeEnabled(true));
            }
            _ => {}
        }
        if matches!(id, command::DETAILS | command::THEME_SYSTEM | command::THEME_CLASSIC | command::THEME_CONSOLE) {
            self.save_settings();
        }
        commands.redraw();
        commands.accessibility_changed();
    }

    /// Handles shortcuts that work whatever has focus. Returns true if used.
    fn shortcut(&mut self, key: Key, modifiers: Modifiers, commands: &mut Commands) -> bool {
        let Modifiers { ctrl, shift, alt } = modifiers;
        if alt {
            return false;
        }
        let id = match (key, ctrl, shift) {
            (Key::Char('O'), true, false) => command::OPEN,
            (Key::Char('W'), true, false) => command::CLOSE_TAB,
            (Key::Char('F'), true, false) => command::FIND,
            (Key::Char('F'), true, true) => command::FILTER,
            (Key::Char('L'), true, false) => command::ERROR_LOOKUP,
            (Key::Char('U'), true, false) => command::PAUSE,
            (Key::Char('M'), true, true) => command::MERGE,
            (Key::F(3), false, false) => command::FIND_NEXT,
            (Key::F(3), false, true) => command::FIND_PREVIOUS,
            (Key::Char('E'), true, false) => command::NEXT_ERROR,
            (Key::Char('E'), true, true) => command::PREVIOUS_ERROR,
            (Key::Tab, true, _) => {
                if !self.tabs.is_empty() {
                    let count = self.tabs.len();
                    self.active = if shift {
                        (self.active + count - 1) % count
                    } else {
                        (self.active + 1) % count
                    };
                    commands.redraw();
                }
                return true;
            }
            (Key::Char(digit @ '1'..='9'), true, false) => {
                let index = digit as usize - '1' as usize;
                if index < self.tabs.len() {
                    self.active = index;
                    commands.redraw();
                }
                return true;
            }
            _ => return false,
        };
        self.run_command(id, commands);
        true
    }

    fn handle_field(&mut self, focus: Focus, response: FieldResponse, commands: &mut Commands) {
        let waker = self.waker.clone();
        if let Some(text) = response.copy {
            commands.push(Command::SetClipboard(text));
        }
        if response.wants_paste {
            if let Some(text) = tv_platform::clipboard_text(self.hwnd) {
                if let Some(tab) = self.active_tab_mut() {
                    match focus {
                        Focus::Filter => tab.filter_field.insert(&text),
                        Focus::Find => tab.find_field.insert(&text),
                        Focus::Grid => false,
                    };
                }
            }
        }
        if response.cancelled {
            self.set_focus(Focus::Grid, commands);
        }
        if let Some(shift) = response.submitted {
            if let Some(tab) = self.active_tab_mut() {
                match focus {
                    Focus::Filter => tab.apply_filter(&waker),
                    Focus::Find => tab.find_next(!shift, &waker),
                    Focus::Grid => {}
                }
            }
        }
        if response.changed
            && focus == Focus::Find
            && self
                .active_tab()
                .is_some_and(|tab| tab.find_field.text().is_empty())
        {
            if let Some(tab) = self.active_tab_mut() {
                tab.clear_find();
            }
        }
        if response.redraw
            || response.changed
            || response.submitted.is_some()
            || response.wants_paste
        {
            commands.redraw();
            commands.accessibility_changed();
        }
    }

    fn close_lookup(&mut self, commands: &mut Commands) {
        self.lookup = None;
        commands.push(Command::SetImeEnabled(self.focus != Focus::Grid));
        commands.redraw();
        commands.accessibility_changed();
    }

    /// The lookup panel takes all input while it's open. A click outside it closes it.
    fn lookup_event(&mut self, event: Event, canvas: &mut dyn Canvas, commands: &mut Commands) {
        let (waker, hwnd) = (self.waker.clone(), self.hwnd);
        let Some(panel) = &mut self.lookup else {
            return;
        };
        match event {
            Event::MouseDown(point, ..) if !panel.rect().contains(point) => {
                self.close_lookup(commands);
                return;
            }
            Event::KeyDown(
                Key::Char('C'),
                Modifiers {
                    ctrl: true,
                    shift: true,
                    alt: false,
                },
            ) => {
                if !panel.result().is_empty() {
                    commands.push(Command::SetClipboard(panel.result().replace('\n', "\r\n")));
                }
                return;
            }
            _ => {}
        }
        let field_rect = panel.field_rect();
        let response = panel.field.handle(event, canvas, field_rect);
        if let Some(text) = response.copy {
            commands.push(Command::SetClipboard(text));
        }
        if response.wants_paste {
            if let Some(text) = tv_platform::clipboard_text(hwnd) {
                panel.field.insert(&text);
            }
        }
        if response.submitted.is_some() {
            panel.look_up(&waker);
        }
        if response.cancelled {
            self.close_lookup(commands);
            return;
        }
        if response.redraw
            || response.changed
            || response.submitted.is_some()
            || response.wants_paste
        {
            commands.redraw();
            commands.accessibility_changed();
        }
    }

    fn menu_wants(&self, event: &Event) -> bool {
        let open = self.menu.open_menu().is_some();
        match event {
            Event::KeyDown(key, modifiers) => {
                self.menu.is_active() || modifiers.alt || *key == Key::F(10)
            }
            Event::MouseDown(point, ..) => open || self.menu.contains(*point),
            Event::MouseMove(_) | Event::MouseUp(..) | Event::Wheel(_) | Event::Char(_) => open,
        }
    }

    /// Routes input to the menu, a shortcut, the tab bar, a text field or the grid.
    pub fn event(&mut self, event: Event, canvas: &mut dyn Canvas, commands: &mut Commands) {
        if self.menu_wants(&event) {
            let response = self.menu.handle(event);
            if response.redraw {
                commands.redraw();
                commands.accessibility_changed();
            }
            if let Some(id) = response.command {
                self.run_command(id, commands);
            }
            if response.consumed {
                return;
            }
        }
        if self.lookup.is_some() {
            self.lookup_event(event, canvas, commands);
            return;
        }
        if let Event::KeyDown(key, modifiers) = event {
            if self.shortcut(key, modifiers, commands) {
                return;
            }
            if key == Key::Tab && !modifiers.ctrl && self.focus != Focus::Grid {
                let next = match (self.focus, modifiers.shift) {
                    (Focus::Filter, false) | (Focus::Grid, true) => Focus::Find,
                    (Focus::Find, false) => Focus::Grid,
                    (Focus::Find, true) | (Focus::Grid, false) => Focus::Filter,
                    (Focus::Filter, true) => Focus::Grid,
                };
                self.set_focus(next, commands);
                return;
            }
        }

        let layout = self.layout;
        if let Event::MouseDown(point, button, _) = event {
            if layout.tabs.contains(point) {
                let response = self.tab_bar.handle(event);
                if let Some(index) = response.close {
                    self.close_tab(index);
                } else if let Some(index) = response.select {
                    self.active = index;
                }
                commands.redraw();
                commands.accessibility_changed();
                return;
            }
            let target = if layout.filter.contains(point) {
                Some(Focus::Filter)
            } else if layout.find.contains(point) {
                Some(Focus::Find)
            } else if layout.grid.contains(point) {
                Some(Focus::Grid)
            } else {
                None
            };
            match target {
                Some(focus) if button == MouseButton::Left => {
                    if self.focus != focus {
                        self.focus = focus;
                        commands.push(Command::SetImeEnabled(focus != Focus::Grid));
                    }
                }
                Some(_) => {}
                None => return,
            }
        }
        if let Event::MouseMove(_) = event {
            if self.tab_bar.handle(event).redraw {
                commands.redraw();
            }
        }

        let focus = self.focus;
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let field_event = match event {
            Event::Wheel(_) => false,
            Event::MouseDown(point, ..) => !layout.grid.contains(point),
            _ => focus != Focus::Grid,
        };
        if field_event {
            let response = match focus {
                Focus::Filter => tab.filter_field.handle(event, canvas, layout.filter),
                Focus::Find => tab.find_field.handle(event, canvas, layout.find),
                Focus::Grid => FieldResponse::default(),
            };
            self.handle_field(focus, response, commands);
            return;
        }
        let response = tab.view.handle(event);
        tab.following = tab.view.at_bottom();
        if response.copy_selection {
            self.start_copy();
        }
        if response.redraw {
            commands.redraw();
            commands.accessibility_changed();
        }
    }

    /// Picks up finished background work from every tab.
    pub fn woken(&mut self, commands: &mut Commands) {
        let waker = self.waker.clone();
        for tab in &mut self.tabs {
            tab.poll(&waker);
        }
        if let Some(panel) = &mut self.lookup {
            panel.poll();
        }
        if let Some(copying) = &self.copying {
            match copying.try_recv() {
                Ok(Ok(copied)) => {
                    commands.push(Command::SetClipboard(copied.text));
                    let rows = with_commas(copied.rows);
                    self.notice = Some(if copied.cut_short {
                        format!("Copied the first {rows} rows")
                    } else {
                        format!("Copied {rows} rows")
                    });
                    self.copying = None;
                }
                Ok(Err(error)) => {
                    self.notice = Some(format!("Couldn't copy: {error}"));
                    self.copying = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.copying = None,
            }
        }
        commands.redraw();
        commands.accessibility_changed();
    }

    fn arrange(&mut self, canvas: &mut dyn Canvas, size: (f32, f32), scale: f32) {
        self.scale = scale;
        let theme = self.theme();
        let mut area = Rect::new(0.0, 0.0, size.0, size.1);
        let menu_bar = area.take_top(snap(menu::BAR_HEIGHT, scale));
        self.menu.menus = self.build_menus();
        self.menu.arrange(menu_bar, canvas);
        let labels: Vec<TabLabel> = self.tab_labels();
        let tabs = if self.tabs.is_empty() {
            Rect::default()
        } else {
            area.take_top(snap(tab_bar::BAR_HEIGHT, scale))
        };
        self.tab_bar.arrange(tabs, &labels, canvas);
        let ready = self
            .active_tab()
            .is_some_and(|tab| tab.document().is_some());
        let status = area.take_bottom(snap(theme.status_height, scale));
        let (mut filter, mut find, mut find_count, mut details) = Default::default();
        if ready {
            let toolbar = area.take_top(snap(TOOLBAR_HEIGHT, scale));
            let field_top = toolbar.top + (toolbar.height() - FIELD_HEIGHT) / 2.0;
            let width = toolbar.width();
            let filter_right = toolbar.left + MARGIN + (width * 0.5).max(240.0);
            filter = Rect::new(
                toolbar.left + MARGIN,
                field_top,
                filter_right,
                field_top + FIELD_HEIGHT,
            );
            let find_right =
                (filter_right + MARGIN + (width * 0.28).max(180.0)).min(toolbar.right - MARGIN);
            find = Rect::new(
                filter_right + MARGIN,
                field_top,
                find_right,
                field_top + FIELD_HEIGHT,
            );
            find_count = Rect::new(
                find_right + MARGIN,
                toolbar.top,
                toolbar.right - MARGIN,
                toolbar.bottom,
            );
            if self.show_details {
                details = area.take_bottom(snap(PANE_HEIGHT, scale));
            }
        }
        self.layout = Layout {
            menu: menu_bar,
            tabs,
            filter,
            find,
            find_count,
            grid: area,
            details,
            status,
        };
        let row_height = snap(theme.row_height, scale);
        let metrics = Metrics {
            row_height,
            char_width: canvas.char_width(FontRole::Grid),
        };
        let grid = self.layout.grid;
        if let Some(panel) = &mut self.lookup {
            panel.arrange(grid);
        }
        if let Some(tab) = self.tabs.get_mut(self.active) {
            let count = tab.rows().map_or(0, |rows| rows.row_count());
            tab.view.arrange(grid, count, metrics, theme, scale);
            if tab.following {
                tab.view.scroll_to_end();
            }
        }
    }

    fn tab_labels(&self) -> Vec<TabLabel> {
        self.tabs
            .iter()
            .map(|tab| TabLabel {
                title: tab.title(),
                live: tab.following && !tab.is_paused() && tab.document().is_some(),
            })
            .collect()
    }

    /// Lays out and draws the whole window.
    pub fn draw(
        &mut self,
        canvas: &mut dyn Canvas,
        size: (f32, f32),
        scale: f32,
        commands: &mut Commands,
    ) {
        self.arrange(canvas, size, scale);
        let theme = self.theme();
        let layout = self.layout;
        let focus = self.focus;
        let waker = self.waker.clone();
        canvas.clear(theme.window_background);

        let labels = self.tab_labels();
        if !labels.is_empty() {
            self.tab_bar
                .draw(canvas, layout.tabs, &labels, self.active, theme);
        }

        let mut caret = None;
        match self.tabs.get_mut(self.active) {
            None => canvas.text(
                WELCOME,
                Rect::new(
                    layout.grid.left + 24.0,
                    layout.grid.top + 20.0,
                    layout.grid.right,
                    layout.grid.top + 60.0,
                ),
                UI_LEFT,
                theme.secondary_text,
            ),
            Some(tab) => {
                tab.request_rows(&waker);
                match tab.rows() {
                    Some(rows) => tab.view.draw(canvas, &rows, theme),
                    None => canvas.text(
                        &tab.status(),
                        Rect::new(
                            layout.grid.left + 24.0,
                            layout.grid.top + 20.0,
                            layout.grid.right,
                            layout.grid.top + 60.0,
                        ),
                        UI_LEFT,
                        theme.secondary_text,
                    ),
                }
                if tab.document().is_some() {
                    let filter_caret = tab.filter_field.draw(
                        canvas,
                        layout.filter,
                        theme,
                        focus == Focus::Filter,
                        FILTER_HINT,
                    );
                    let find_caret = tab.find_field.draw(
                        canvas,
                        layout.find,
                        theme,
                        focus == Focus::Find,
                        FIND_HINT,
                    );
                    caret = match focus {
                        Focus::Filter => Some(filter_caret),
                        Focus::Find => Some(find_caret),
                        Focus::Grid => None,
                    };
                    if let Some(summary) = tab.find_summary() {
                        canvas.text(&summary, layout.find_count, UI_LEFT, theme.secondary_text);
                    }
                    if self.show_details {
                        tab.details.draw(canvas, layout.details, theme);
                    }
                }
            }
        }

        let status = self.status();
        canvas.fill_rect(layout.status, theme.status_background);
        canvas.fill_rect(
            Rect::new(
                layout.status.left,
                layout.status.top,
                layout.status.right,
                layout.status.top + 1.0,
            ),
            theme.border,
        );
        canvas.text(
            &status,
            Rect::new(
                layout.status.left + MARGIN,
                layout.status.top,
                layout.status.right - MARGIN,
                layout.status.bottom,
            ),
            UI_LEFT,
            theme.secondary_text,
        );

        if let Some(panel) = &mut self.lookup {
            caret = Some(panel.draw(canvas, theme));
        }

        self.menu.draw_bar(canvas, layout.menu, theme);
        self.menu.draw_popup(canvas, theme);

        if let Some(caret) = caret {
            commands.push(Command::SetImePosition(Point::new(caret.x, caret.y)));
        }
    }

    #[cfg(test)]
    pub fn grid_rect(&self) -> Rect {
        self.layout.grid
    }

    pub fn filter_rect(&self) -> Rect {
        self.layout.filter
    }

    pub fn find_rect(&self) -> Rect {
        self.layout.find
    }

    pub fn tab_titles(&self) -> Vec<String> {
        self.tabs.iter().map(Tab::title).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use tv_ui::recording::RecordingCanvas;

    const SIZE: (f32, f32) = (1200.0, 700.0);
    const NONE: Modifiers = Modifiers {
        ctrl: false,
        shift: false,
        alt: false,
    };
    const CTRL: Modifiers = Modifiers {
        ctrl: true,
        shift: false,
        alt: false,
    };
    const CTRL_SHIFT: Modifiers = Modifiers {
        ctrl: true,
        shift: true,
        alt: false,
    };

    fn log_file(dir: &tempfile::TempDir, name: &str, rows: usize) -> PathBuf {
        let path = dir.path().join(name);
        let text: String = (0..rows)
            .map(|row| {
                let level = match row % 10 {
                    3 => "ERROR",
                    7 => "WARN",
                    _ => "INFO",
                };
                format!(
                    "2026-01-01 10:00:{:02} host app[1]: {level}: message {row}\n",
                    row % 60
                )
            })
            .collect();
        std::fs::write(&path, text).unwrap();
        path
    }

    fn workspace() -> Workspace {
        Workspace::new(Waker::detached(), 0, false)
    }

    /// Runs the loop a window would: poll work, then draw, until `done` holds.
    fn settle(workspace: &mut Workspace, done: impl Fn(&Workspace) -> bool) {
        let started = Instant::now();
        loop {
            let mut commands = Commands::default();
            workspace.woken(&mut commands);
            workspace.draw(&mut RecordingCanvas::new(), SIZE, 1.0, &mut commands);
            if done(workspace) {
                return;
            }
            assert!(started.elapsed() < Duration::from_secs(10), "never settled");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn send(workspace: &mut Workspace, event: Event) {
        workspace.event(event, &mut RecordingCanvas::new(), &mut Commands::default());
    }

    fn type_text(workspace: &mut Workspace, text: &str) {
        for character in text.chars() {
            send(workspace, Event::Char(character));
        }
    }

    fn opened(rows: usize) -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let path = log_file(&dir, "app.log", rows);
        let mut workspace = workspace();
        workspace.open_paths(vec![path]);
        settle(&mut workspace, |workspace| {
            workspace
                .active_tab()
                .is_some_and(|tab| tab.document().is_some())
        });
        (dir, workspace)
    }

    #[test]
    fn opening_a_file_makes_a_ready_tab() {
        let (_dir, workspace) = opened(100);
        assert_eq!(workspace.tab_titles(), vec!["app.log".to_string()]);
        assert!(workspace.status().contains("100 rows"));
    }

    #[test]
    fn opening_the_same_file_twice_reuses_its_tab() {
        let (dir, mut workspace) = opened(10);
        workspace.open_paths(vec![dir.path().join("app.log")]);
        assert_eq!(workspace.tabs.len(), 1);
    }

    #[test]
    fn ctrl_tab_and_ctrl_digits_switch_tabs_and_ctrl_w_closes() {
        let (dir, mut workspace) = opened(10);
        let second = log_file(&dir, "second.log", 5);
        workspace.open_paths(vec![second]);
        assert_eq!(workspace.active, 1);
        send(&mut workspace, Event::KeyDown(Key::Char('1'), CTRL));
        assert_eq!(workspace.active, 0);
        send(&mut workspace, Event::KeyDown(Key::Tab, CTRL));
        assert_eq!(workspace.active, 1);
        send(&mut workspace, Event::KeyDown(Key::Char('W'), CTRL));
        assert_eq!(workspace.tabs.len(), 1);
        assert_eq!(workspace.active, 0);
    }

    #[test]
    fn filter_box_filters_and_keeps_the_selected_row() {
        let (_dir, mut workspace) = opened(100);
        workspace.tabs[0].select_file_row(13);
        send(&mut workspace, Event::KeyDown(Key::Char('F'), CTRL_SHIFT));
        assert_eq!(workspace.focus, Focus::Filter);
        type_text(&mut workspace, "level:error");
        send(&mut workspace, Event::KeyDown(Key::Enter, NONE));
        settle(&mut workspace, |workspace| workspace.tabs[0].is_filtered());
        let tab = &workspace.tabs[0];
        assert_eq!(tab.rows().unwrap().row_count(), 10);
        assert_eq!(
            tab.cursor_file_row(),
            Some(13),
            "the selected error row stays selected"
        );
        assert!(workspace.status().contains("10 shown"));
    }

    #[test]
    fn a_bad_filter_says_why_and_changes_nothing() {
        let (_dir, mut workspace) = opened(20);
        send(&mut workspace, Event::KeyDown(Key::Char('F'), CTRL_SHIFT));
        type_text(&mut workspace, "level:loud");
        send(&mut workspace, Event::KeyDown(Key::Enter, NONE));
        assert!(!workspace.tabs[0].is_filtered());
        assert!(workspace.status().contains("Unknown level"));
    }

    #[test]
    fn find_highlights_and_f3_steps_through_matches() {
        let (_dir, mut workspace) = opened(100);
        send(&mut workspace, Event::KeyDown(Key::Char('F'), CTRL));
        type_text(&mut workspace, "message 4");
        send(&mut workspace, Event::KeyDown(Key::Enter, NONE));
        settle(&mut workspace, |workspace| {
            workspace.tabs[0].cursor_file_row().is_some()
        });
        assert_eq!(workspace.tabs[0].cursor_file_row(), Some(4));
        send(&mut workspace, Event::KeyDown(Key::F(3), NONE));
        assert_eq!(workspace.tabs[0].cursor_file_row(), Some(40));
        assert_eq!(workspace.tabs[0].find_summary().as_deref(), Some("2 of 11"));
        let rows = workspace.tabs[0].rows().unwrap();
        assert!(rows.highlighted(4) && rows.highlighted(49) && !rows.highlighted(5));
    }

    #[test]
    fn ctrl_e_jumps_between_errors() {
        let (_dir, mut workspace) = opened(100);
        send(&mut workspace, Event::KeyDown(Key::Char('E'), CTRL));
        assert_eq!(workspace.tabs[0].cursor_file_row(), Some(3));
        send(&mut workspace, Event::KeyDown(Key::Char('E'), CTRL));
        assert_eq!(workspace.tabs[0].cursor_file_row(), Some(13));
    }

    #[test]
    fn escape_returns_focus_to_the_grid() {
        let (_dir, mut workspace) = opened(10);
        send(&mut workspace, Event::KeyDown(Key::Char('F'), CTRL));
        send(&mut workspace, Event::KeyDown(Key::Escape, NONE));
        assert_eq!(workspace.focus, Focus::Grid);
    }

    #[test]
    fn typing_in_the_grid_does_not_reach_the_fields() {
        let (_dir, mut workspace) = opened(10);
        type_text(&mut workspace, "abc");
        assert_eq!(workspace.tabs[0].filter_field.text(), "");
    }

    #[test]
    fn alt_letter_opens_menus_and_commands_run() {
        let (_dir, mut workspace) = opened(10);
        send(
            &mut workspace,
            Event::KeyDown(Key::Char('V'), Modifiers { alt: true, ..NONE }),
        );
        assert_eq!(workspace.menu().open_menu(), Some(2));
        send(&mut workspace, Event::KeyDown(Key::Escape, NONE));
        send(&mut workspace, Event::KeyDown(Key::Escape, NONE));
        let mut commands = Commands::default();
        workspace.run_command(command::DETAILS, &mut commands);
        assert!(!workspace.show_details);
        workspace.run_command(command::THEME_CONSOLE, &mut commands);
        assert_eq!(workspace.theme().name, "Console");
    }

    #[test]
    fn clicking_a_field_focuses_it() {
        let (_dir, mut workspace) = opened(10);
        let filter = workspace.filter_rect();
        send(
            &mut workspace,
            Event::MouseDown(
                Point::new(filter.left + 5.0, filter.top + 5.0),
                MouseButton::Left,
                NONE,
            ),
        );
        assert_eq!(workspace.focus, Focus::Filter);
        let grid = workspace.grid_rect();
        send(
            &mut workspace,
            Event::MouseDown(
                Point::new(grid.left + 50.0, grid.top + 40.0),
                MouseButton::Left,
                NONE,
            ),
        );
        assert_eq!(workspace.focus, Focus::Grid);
    }

    #[test]
    fn copies_rows_as_tab_separated_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.log");
        std::fs::write(
            &path,
            "2026-01-01 10:00:00 host app[1]: ERROR: disk full\nnext\n",
        )
        .unwrap();
        let (log, _) =
            tv_engine::LogFile::open_with_cache(&path, None, &std::sync::atomic::AtomicU64::new(0))
                .unwrap();
        let logs = [std::sync::Arc::new(std::sync::RwLock::new(log))];
        assert_eq!(
            copy_rows(&logs, &[(0, 0, 0), (1, 0, 1)]).unwrap(),
            "1\t2026-01-01 10:00:00.000\tError\tapp\tdisk full\r\n2\t\tInformation\t\tnext\r\n"
        );
    }

    #[test]
    fn a_missing_file_shows_an_error_in_its_tab() {
        let mut workspace = workspace();
        workspace.open_paths(vec![PathBuf::from("Z:\\definitely\\missing.log")]);
        settle(&mut workspace, |workspace| {
            workspace.status().contains("Couldn't open")
        });
    }

    #[test]
    fn with_no_tabs_the_welcome_text_shows() {
        let mut workspace = workspace();
        let mut canvas = RecordingCanvas::new();
        workspace.draw(&mut canvas, SIZE, 1.0, &mut Commands::default());
        assert!(canvas.texts().contains(&WELCOME));
        assert!(canvas.clips_balance());
    }

    #[test]
    fn ctrl_l_looks_up_the_selected_rows_code_and_escape_closes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("setup.log");
        std::fs::write(
            &path,
            "2026-01-01 10:00:00 host setup[1]: ERROR: install failed, error 0x80070005\n",
        )
        .unwrap();
        let mut workspace = workspace();
        workspace.open_paths(vec![path]);
        settle(&mut workspace, |workspace| {
            workspace
                .active_tab()
                .is_some_and(|tab| tab.document().is_some())
        });
        workspace.tabs[0].select_file_row(0);
        settle(&mut workspace, |workspace| {
            workspace.tabs[0].details.shown().is_some()
        });
        assert!(workspace.tabs[0].details.shown().unwrap().codes[0].starts_with("0x80070005: "));

        send(&mut workspace, Event::KeyDown(Key::Char('L'), CTRL));
        settle(&mut workspace, |workspace| {
            workspace
                .lookup
                .as_ref()
                .is_some_and(|panel| !panel.result().is_empty())
        });
        let panel = workspace.lookup.as_ref().unwrap();
        assert_eq!(panel.field.text(), "0x80070005");
        assert!(panel.result().starts_with("HRESULT 0x80070005"));

        // Typing goes to the panel, not the grid or the other boxes.
        send(&mut workspace, Event::Char('5'));
        send(&mut workspace, Event::KeyDown(Key::Enter, NONE));
        settle(&mut workspace, |workspace| {
            workspace
                .lookup
                .as_ref()
                .is_some_and(|panel| panel.result().starts_with("Win32 5"))
        });
        assert_eq!(workspace.focus, Focus::Grid);
        send(&mut workspace, Event::KeyDown(Key::Escape, NONE));
        assert!(workspace.lookup.is_none());
    }

    fn append(dir: &tempfile::TempDir, from: usize, to: usize) {
        use std::io::Write;
        let lines: String = (from..to)
            .map(|row| {
                let level = if row % 10 == 3 { "ERROR" } else { "INFO" };
                format!(
                    "2026-01-01 10:01:{:02} host app[1]: {level}: message {row}\n",
                    row % 60
                )
            })
            .collect();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.path().join("app.log"))
            .unwrap();
        file.write_all(lines.as_bytes()).unwrap();
    }

    fn row_count(workspace: &Workspace) -> usize {
        workspace.tabs[0].rows().map_or(0, |rows| rows.row_count())
    }

    #[test]
    fn a_log_opens_at_the_end_and_follows_new_rows() {
        let (dir, mut workspace) = opened(100);
        assert!(workspace.tabs[0].view.visible_rows().any(|row| row == 99));
        append(&dir, 100, 150);
        settle(&mut workspace, |workspace| {
            workspace.tabs[0]
                .document()
                .is_some_and(|document| document.sources[0].log.read().unwrap().rows() == 150)
        });
        let tab = &workspace.tabs[0];
        assert!(
            tab.view.visible_rows().any(|row| row == 149),
            "the newest row is in view"
        );
        assert!(workspace.status().contains("Following"));
    }

    #[test]
    fn scrolling_up_stops_following() {
        let (dir, mut workspace) = opened(100);
        send(&mut workspace, Event::KeyDown(Key::Home, CTRL));
        assert!(!workspace.tabs[0].following);
        append(&dir, 100, 150);
        settle(&mut workspace, |workspace| row_count(workspace) == 150);
        assert_eq!(workspace.tabs[0].view.top_row(), 0);
        send(&mut workspace, Event::KeyDown(Key::End, CTRL));
        assert!(
            workspace.tabs[0].following,
            "going to the end follows again"
        );
    }

    #[test]
    fn new_rows_reach_the_filter_and_find() {
        let (dir, mut workspace) = opened(100);
        send(&mut workspace, Event::KeyDown(Key::Char('F'), CTRL_SHIFT));
        type_text(&mut workspace, "level:error");
        send(&mut workspace, Event::KeyDown(Key::Enter, NONE));
        settle(&mut workspace, |workspace| workspace.tabs[0].is_filtered());
        send(&mut workspace, Event::KeyDown(Key::Char('F'), CTRL));
        type_text(&mut workspace, "message 1");
        send(&mut workspace, Event::KeyDown(Key::Enter, NONE));
        settle(&mut workspace, |workspace| {
            workspace.tabs[0]
                .find_summary()
                .is_some_and(|summary| !summary.starts_with("Searching"))
        });
        assert_eq!(row_count(&workspace), 10);

        append(&dir, 100, 120);
        settle(&mut workspace, |workspace| row_count(workspace) == 12);
        assert_eq!(workspace.tabs[0].filter_rows().unwrap()[10..], [103, 113]);
        // "message 1" matches 1, 10-19 and 100-119.
        settle(&mut workspace, |workspace| {
            workspace.tabs[0]
                .find_summary()
                .is_some_and(|summary| summary == "31 matches" || summary.ends_with(" of 31"))
        });
    }

    #[test]
    fn ctrl_u_pauses_live_updates() {
        let (dir, mut workspace) = opened(100);
        send(&mut workspace, Event::KeyDown(Key::Char('U'), CTRL));
        assert!(workspace.status().contains("Paused"));
        append(&dir, 100, 110);
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(800) {
            settle(&mut workspace, |_| true);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(row_count(&workspace), 100);
        send(&mut workspace, Event::KeyDown(Key::Char('U'), CTRL));
        settle(&mut workspace, |workspace| row_count(workspace) == 110);
    }

    fn timed_log(dir: &tempfile::TempDir, name: &str, rows: &[(u32, &str)]) -> PathBuf {
        let path = dir.path().join(name);
        let text: String = rows
            .iter()
            .map(|(second, message)| {
                format!("2026-01-01 10:00:{second:02} host app[1]: INFO: {message}\n")
            })
            .collect();
        std::fs::write(&path, text).unwrap();
        path
    }

    fn messages(workspace: &Workspace) -> Vec<String> {
        let Some(rows) = workspace.active_tab().and_then(Tab::rows) else {
            return Vec::new();
        };
        (0..rows.row_count())
            .map(|row| {
                rows.cells(row)
                    .map(|cells| cells.message.clone())
                    .unwrap_or_default()
            })
            .collect()
    }

    fn merged(dir: &tempfile::TempDir) -> Workspace {
        let first = timed_log(dir, "a.log", &[(0, "a0"), (2, "a1"), (4, "a2")]);
        let second = timed_log(dir, "b.log", &[(1, "b0"), (3, "b1")]);
        let mut workspace = workspace();
        workspace.open_paths(vec![first, second]);
        settle(&mut workspace, |workspace| {
            workspace.tabs.iter().all(|tab| tab.document().is_some())
        });
        send(&mut workspace, Event::KeyDown(Key::Char('M'), CTRL_SHIFT));
        assert_eq!(workspace.active, 2);
        settle(&mut workspace, |workspace| {
            messages(workspace)
                .iter()
                .filter(|message| !message.is_empty())
                .count()
                == 5
        });
        workspace
    }

    #[test]
    fn merging_interleaves_open_logs_by_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut workspace = merged(&dir);
        assert_eq!(workspace.tab_titles()[2], "a.log + b.log");
        assert_eq!(messages(&workspace), ["a0", "b0", "a1", "b1", "a2"]);
        assert!(workspace.status().contains("2 logs merged"));

        // Details name the file a row came from.
        workspace.tabs[2].select_file_row(3);
        settle(&mut workspace, |workspace| {
            workspace.tabs[2].details.shown().is_some()
        });
        let detail = workspace.tabs[2].details.shown().unwrap();
        assert_eq!(
            (detail.file.as_deref(), detail.row, detail.message.as_str()),
            (Some("b.log"), 1, "b1")
        );

        // Merging the same logs again goes back to their tab.
        send(&mut workspace, Event::KeyDown(Key::Char('1'), CTRL));
        send(&mut workspace, Event::KeyDown(Key::Char('M'), CTRL_SHIFT));
        assert_eq!((workspace.active, workspace.tabs.len()), (2, 3));
    }

    #[test]
    fn a_merged_tab_filters_and_copies_across_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut workspace = merged(&dir);
        send(&mut workspace, Event::KeyDown(Key::Char('F'), CTRL_SHIFT));
        type_text(&mut workspace, "/1$/");
        send(&mut workspace, Event::KeyDown(Key::Enter, NONE));
        settle(&mut workspace, |workspace| workspace.tabs[2].is_filtered());
        assert_eq!(workspace.tabs[2].filter_rows(), Some(&[2u32, 3][..]));

        let document = workspace.tabs[2].document().unwrap();
        let logs: Vec<SharedLog> = document
            .sources
            .iter()
            .map(|source| source.log.clone())
            .collect();
        let rows: Vec<_> = [2, 3]
            .iter()
            .map(|row| {
                (
                    *row,
                    document.locate(*row).unwrap().0,
                    document.locate(*row).unwrap().1,
                )
            })
            .collect();
        let copied = copy_rows(&logs, &rows).unwrap();
        assert!(
            copied.starts_with("3\t2026-01-01 10:00:02.000\tInformation\tapp\ta1\r\n4\t"),
            "{copied}"
        );
    }

    #[test]
    fn a_merged_tab_takes_new_rows_from_any_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut workspace = merged(&dir);
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.path().join("b.log"))
            .unwrap();
        file.write_all(b"2026-01-01 10:00:05 host app[1]: INFO: b2\n")
            .unwrap();
        settle(&mut workspace, |workspace| {
            messages(workspace)
                .last()
                .is_some_and(|message| message == "b2")
        });
        assert_eq!(messages(&workspace), ["a0", "b0", "a1", "b1", "a2", "b2"]);
    }

    #[test]
    fn merging_needs_two_open_logs() {
        let (_dir, mut workspace) = opened(10);
        send(&mut workspace, Event::KeyDown(Key::Char('M'), CTRL_SHIFT));
        assert_eq!(workspace.tabs.len(), 1);
        assert!(workspace.status().contains("Open two or more logs"));
    }

    #[test]
    fn choices_are_saved_and_restored() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(super::super::settings::FILE_NAME);
        let log = log_file(&dir, "app.log", 5);
        let mut first = workspace();
        let (store, settings, problem) = Store::open(Some(file.clone()));
        first.use_settings(settings, store, problem);
        first.run_command(command::THEME_CONSOLE, &mut Commands::default());
        first.run_command(command::DETAILS, &mut Commands::default());
        first.open_paths(vec![log.clone()]);
        drop(first);

        let mut restored = workspace();
        let (store, settings, problem) = Store::open(Some(file));
        restored.use_settings(settings, store, problem);
        assert_eq!(restored.theme().name, "Console");
        assert!(!restored.show_details);
        assert_eq!(restored.settings().recent, [log.to_str().unwrap()]);
    }

    #[test]
    fn clicking_outside_the_lookup_panel_closes_it() {
        let mut workspace = workspace();
        workspace.draw(
            &mut RecordingCanvas::new(),
            SIZE,
            1.0,
            &mut Commands::default(),
        );
        workspace.run_command(command::ERROR_LOOKUP, &mut Commands::default());
        let inside = workspace.lookup.as_ref().unwrap().field_rect();
        send(
            &mut workspace,
            Event::MouseDown(
                Point::new(inside.left + 5.0, inside.top + 5.0),
                MouseButton::Left,
                NONE,
            ),
        );
        assert!(workspace.lookup.is_some());
        send(
            &mut workspace,
            Event::MouseDown(Point::new(2.0, SIZE.1 - 2.0), MouseButton::Left, NONE),
        );
        assert!(workspace.lookup.is_none());
    }
}
