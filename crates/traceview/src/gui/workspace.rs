//! Everything in the window except the window itself: tabs, menus, keyboard
//! focus, layout, drawing and routing input. It draws on any `Canvas`, so tests
//! drive it without a display.

use super::columns::{self, Column, ColumnSpec, LogKind};
use super::columns_panel::ColumnsPanel;
use super::command_palette::{Action, CommandPalette, Mode};
use super::details::PANE_HEIGHT;
use super::fonts::{self, Spacing, Target as FontTarget, LOG_SIZES};
use super::fonts_panel::{FontsPanel, FontsResponse, Item as FontItem};
use super::formats_panel::{FormatsPanel, FormatsResponse, Part as FormatsPart, Sample};
use super::home::{Choice as HomeChoice, Home};
use super::keymap::{self, Keymap};
use super::layouts;
use super::log_view::{snap, Metrics, RowSource};
use super::lookup::{first_code, LookupPanel};
use super::overview;
use super::reload::{Reload, Reloader};
use super::row_cache::{cells_from_row, SharedLog};
use super::rules::RuleSet;
use super::rules_panel::{Part, RulesPanel};
use super::settings::{self, LayoutSetting, Preset, Settings, Store};
use super::shortcut_panel::{Choice, ShortcutPanel};
use super::sidebar::{self, Sidebar, Target};
use super::tab::{file_name, with_commas, Tab};
use super::themes::{self, FileTheme};
use super::themes_panel::{Part as ThemesPart, ThemesPanel, ThemesResponse};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use tv_engine::{CustomFormat, Format, Level};
use tv_platform::{Command, Commands, Cursor, Waker};
use tv_ui::chip_bar::{self, Chip, ChipBar};
use tv_ui::context_menu::ContextMenu;
use tv_ui::menu::{self, Menu, MenuBar, MenuItem};
use tv_ui::palette::PaletteItem;
use tv_ui::splitter::{Orientation, Splitter};
use tv_ui::tab_bar::{self, Tab as TabLabel, TabBar};
use tv_ui::text_field::FieldResponse;
use tv_ui::theme::ContrastColors;
use tv_ui::{
    Align, Canvas, Event, FontRole, Key, Modifiers, MouseButton, Point, Rect, TextStyle, Theme,
};

const TOOLBAR_HEIGHT: f32 = 34.0;
const FIELD_HEIGHT: f32 = 24.0;
const MARGIN: f32 = 8.0;
/// Each pane of a split view keeps at least this much width.
const MIN_PANE: f32 = 240.0;
const UI_LEFT: TextStyle = TextStyle::new(FontRole::Ui, Align::Left);
/// More than this many rows in one copy would make a clipboard entry too large
/// for most apps to accept anyway.
const MAX_COPY_ROWS: usize = 100_000;
/// How many of a log's first rows the formats editor tries a pattern on.
const SAMPLE_ROWS: usize = 20;
const FILTER_HINT: &str = "Filter (Ctrl+Shift+F): words, /regex/, level:warning, component:Name";
const FIND_HINT: &str = "Find (Ctrl+F, then F3)";

/// Menu command numbers.
pub mod command {
    pub const OPEN: u32 = 1;
    pub const CLOSE_TAB: u32 = 2;
    pub const EXIT: u32 = 3;
    pub const MERGE: u32 = 4;
    pub const EXPORT: u32 = 5;
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
    pub const HIGHLIGHTING: u32 = 24;
    pub const COLUMNS: u32 = 25;
    pub const SIDEBAR: u32 = 26;
    pub const CHIPS: u32 = 27;
    pub const HISTOGRAM: u32 = 28;
    pub const RULES: u32 = 29;
    /// The themes, in `Theme::ALL` order, from this number on.
    pub const THEMES: u32 = 40;
    pub const ERROR_LOOKUP: u32 = 30;
    pub const PAUSE: u32 = 31;
    pub const PALETTE: u32 = 35;
    pub const GO_TO_ROW: u32 = 36;
    pub const SAVE_PRESET: u32 = 37;
    pub const SPLIT: u32 = 38;
    pub const SYNC: u32 = 39;
    pub const BOOKMARK: u32 = 32;
    pub const NEXT_BOOKMARK: u32 = 33;
    pub const PREVIOUS_BOOKMARK: u32 = 34;
    pub const FONTS: u32 = 60;
    pub const LARGER_TEXT: u32 = 61;
    pub const SMALLER_TEXT: u32 = 62;
    pub const DEFAULT_TEXT: u32 = 63;
    pub const KEYS: u32 = 64;
    pub const NEXT_TAB: u32 = 65;
    pub const PREVIOUS_TAB: u32 = 66;
    pub const FORMATS: u32 = 67;
    pub const EDIT_THEME: u32 = 68;
    pub const SAVE_LAYOUT: u32 = 69;
    pub const USE_FOR_LOGS: u32 = 70;
    pub const STOP_FOR_LOGS: u32 = 71;
    /// Stands for the Windows contrast theme in the View menu. It does
    /// nothing, since Windows turns that on and off.
    pub const THEME_CONTRAST: u32 = 72;
    pub const ABOUT: u32 = 73;
    pub const OPEN_FOLDER: u32 = 74;
    /// Themes from files, in the order they load, from this number on.
    pub const FILE_THEMES: u32 = 100;
    /// Layouts, the built-in ones first, from this number on.
    pub const LAYOUTS: u32 = 200;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Grid,
    Filter,
    Find,
    Sidebar,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ThemeChoice {
    /// Classic in light mode and Dark in dark mode, following Windows.
    System,
    Fixed(&'static Theme),
}

/// The built-in theme a menu command picks, if it picks one.
fn builtin_theme_for(id: u32) -> Option<&'static Theme> {
    id.checked_sub(command::THEMES)
        .and_then(|index| Theme::ALL.get(index as usize).copied())
}

/// Problems as one message: the first, and how many more there are.
fn joined(problems: &[String]) -> Option<String> {
    match problems {
        [] => None,
        [only] => Some(only.clone()),
        [first, rest @ ..] => Some(format!("{first}, and {} more", rest.len())),
    }
}

/// `base`, or `base 2`, `base 3` and so on, whichever isn't taken yet.
fn free_theme_name(base: &str, taken: &[String]) -> String {
    (1..)
        .map(|number| match number {
            1 => base.to_string(),
            _ => format!("{base} {number}"),
        })
        .find(|name| !themes::name_taken(name, taken))
        .unwrap_or_else(|| base.to_string())
}

#[derive(Clone, Copy, Debug, Default)]
struct Layout {
    menu: Rect,
    tabs: Rect,
    filter: Rect,
    find: Rect,
    find_count: Rect,
    chips: Rect,
    histogram: Rect,
    grid: Rect,
    details: Rect,
    status: Rect,
    /// The focused pane, and in split view the other one.
    focused_area: Rect,
    other: Option<PaneLayout>,
}

/// Where one pane's parts go.
#[derive(Clone, Copy, Debug, Default)]
struct PaneLayout {
    area: Rect,
    filter: Rect,
    find: Rect,
    find_count: Rect,
    chips: Rect,
    histogram: Rect,
    grid: Rect,
}

/// Lays out a pane: the filter and find boxes, the chips and histogram, and
/// the grid in what's left.
fn pane_layout(
    mut area: Rect,
    ready: bool,
    chips: bool,
    histogram: bool,
    scale: f32,
) -> PaneLayout {
    let mut pane = PaneLayout {
        area,
        ..PaneLayout::default()
    };
    if ready {
        let toolbar = area.take_top(snap(TOOLBAR_HEIGHT, scale));
        let field_top = toolbar.top + (toolbar.height() - FIELD_HEIGHT) / 2.0;
        let width = toolbar.width();
        let filter_right =
            toolbar.left + MARGIN + (width * 0.5).max(160.0).min(width - 2.0 * MARGIN);
        pane.filter = Rect::new(
            toolbar.left + MARGIN,
            field_top,
            filter_right,
            field_top + FIELD_HEIGHT,
        );
        let find_right =
            (filter_right + MARGIN + (width * 0.28).max(120.0)).min(toolbar.right - MARGIN);
        pane.find = Rect::new(
            filter_right + MARGIN,
            field_top,
            find_right.max(filter_right + MARGIN),
            field_top + FIELD_HEIGHT,
        );
        pane.find_count = Rect::new(
            find_right + MARGIN,
            toolbar.top,
            toolbar.right - MARGIN,
            toolbar.bottom,
        );
        if chips {
            pane.chips = area.take_top(snap(chip_bar::BAR_HEIGHT, scale));
        }
        if histogram {
            pane.histogram =
                area.take_top(snap(overview::BAR_HEIGHT + overview::LABEL_HEIGHT, scale));
        }
    }
    pane.grid = area;
    pane
}

/// Draws a pane: its grid, boxes, chips and histogram. Returns the caret of
/// the box with focus, if one has it, for the IME. An unfocused pane passes
/// no focus.
fn draw_pane(
    canvas: &mut dyn Canvas,
    tab: &mut Tab,
    pane: &PaneLayout,
    theme: &Theme,
    focus: Option<Focus>,
    (chip_bar, chips): (&ChipBar, &[Chip]),
    hover: Option<usize>,
) -> Option<Point> {
    match tab.rows() {
        Some(rows) => tab.view.draw(canvas, &rows, theme),
        None => canvas.text(
            &tab.status(),
            Rect::new(
                pane.grid.left + 24.0,
                pane.grid.top + 20.0,
                pane.grid.right,
                pane.grid.top + 60.0,
            ),
            UI_LEFT,
            theme.secondary_text,
        ),
    }
    tab.document()?;
    let filter_caret = tab.filter_field.draw(
        canvas,
        pane.filter,
        theme,
        focus == Some(Focus::Filter),
        FILTER_HINT,
    );
    let find_caret = tab.find_field.draw(
        canvas,
        pane.find,
        theme,
        focus == Some(Focus::Find),
        FIND_HINT,
    );
    if let Some(summary) = tab.find_summary() {
        canvas.text(&summary, pane.find_count, UI_LEFT, theme.secondary_text);
    }
    if pane.chips.height() > 0.0 {
        chip_bar.draw(canvas, pane.chips, chips, theme, false);
    }
    if let (true, Some(histogram)) = (
        pane.histogram.height() > 0.0,
        &tab.overview.counts.histogram,
    ) {
        overview::draw_histogram(
            canvas,
            pane.histogram,
            histogram,
            &tab.overview.shadows,
            hover,
            theme,
        );
    }
    match focus {
        Some(Focus::Filter) => Some(filter_caret),
        Some(Focus::Find) => Some(find_caret),
        _ => None,
    }
}

/// The menu a right-clicked row opens, with Shift+F10 or the menu key too.
pub struct RowMenu {
    pub menu: ContextMenu,
    /// The row it's for, in the file or merged timeline.
    row: usize,
}

/// Two logs side by side, as in the Mac app.
pub struct Split {
    /// The tab in the pane without focus.
    pub other: usize,
    /// True when the focused pane is the right-hand one.
    pub focus_right: bool,
    /// Scrolling the focused pane lines the other up at the same time.
    pub sync: bool,
    splitter: Splitter,
    /// The tab the focused pane showed last frame, to notice a switch.
    focused_tab: usize,
    /// The focused tab and its top row when the panes were last lined up.
    synced: Option<(usize, usize)>,
}

/// An export to CSV running on another thread.
struct Exporting {
    result: Receiver<io::Result<()>>,
    done: Arc<AtomicUsize>,
    total: usize,
    name: String,
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
    pub show_sidebar: bool,
    pub sidebar: Sidebar,
    /// Where the mouse was last seen, since wheel events don't say.
    pointer: Point,
    /// The pointer shape last asked for.
    cursor: Cursor,
    pub show_chips: bool,
    pub show_histogram: bool,
    /// The start page, and the folder it lists, if one.
    pub home: Home,
    /// Shows the start page over open logs, for a folder opened while they are.
    home_over_tabs: bool,
    /// The tab Split view asked the Open dialog for a log to show beside.
    split_beside: Option<usize>,
    chip_bar: ChipBar,
    chips: Vec<Chip>,
    /// The level each chip filters to.
    chip_filters: Vec<Option<Level>>,
    histogram_hover: Option<usize>,
    /// Two logs side by side, when on.
    pub split: Option<Split>,
    other_chip_bar: ChipBar,
    other_chips: Vec<Chip>,
    /// The window's size at the last layout.
    size: (f32, f32),
    pub theme_choice: ThemeChoice,
    pub system_is_dark: bool,
    /// The Error Lookup panel, while it's open.
    pub lookup: Option<LookupPanel>,
    /// The command palette, while it's open.
    pub palette: Option<CommandPalette>,
    pub row_menu: Option<RowMenu>,
    /// The EXE this Windows user has registered to open `.log` files, if any.
    pub file_types: Option<PathBuf>,
    /// The Windows contrast theme, as a theme, while one is on.
    pub contrast: Option<&'static Theme>,
    /// The highlight rules editor, while it's open.
    pub rules_panel: Option<RulesPanel>,
    /// The columns editor, while it's open.
    pub columns_panel: Option<ColumnsPanel>,
    /// The fonts editor, while it's open.
    pub fonts_panel: Option<FontsPanel>,
    /// The log formats editor, while it's open.
    pub formats_panel: Option<FormatsPanel>,
    /// The theme editor, while it's open.
    pub themes_panel: Option<ThemesPanel>,
    /// Themes read from files, after the built-in ones.
    file_themes: Vec<FileTheme>,
    /// Where theme files go, beside the settings file. `None` when settings
    /// aren't saved, as in tests.
    themes_folder: Option<PathBuf>,
    /// What was wrong with theme files when they were last read, so a reload
    /// only says it again if it's new.
    theme_problems: Vec<String>,
    /// Notices settings and theme files edited elsewhere.
    reloader: Option<Reloader>,
    /// The highlight rules every tab's rows are marked with.
    rules: Arc<RuleSet>,
    /// Which command each shortcut runs.
    keymap: Keymap,
    /// The custom log formats that compile, which logs are opened with.
    formats: Vec<Arc<CustomFormat>>,
    /// The shortcut being changed, while it waits for keys.
    pub shortcut_panel: Option<ShortcutPanel>,
    /// What was typed in the list of shortcuts, to bring it back after one changes.
    keys_query: String,
    settings: Settings,
    store: Store,
    menu: MenuBar,
    tab_bar: TabBar,
    layout: Layout,
    waker: Waker,
    hwnd: isize,
    copying: Option<Receiver<io::Result<CopyResult>>>,
    exporting: Option<Exporting>,
    notice: Option<String>,
    scale: f32,
}

fn checked(mut item: MenuItem, is_checked: bool) -> MenuItem {
    if let MenuItem::Command { checked, .. } = &mut item {
        *checked = is_checked;
    }
    item
}

fn enabled(mut item: MenuItem, is_enabled: bool) -> MenuItem {
    if let MenuItem::Command { enabled, .. } = &mut item {
        *enabled = is_enabled;
    }
    item
}

/// What Help > About TraceView says.
fn about_text() -> String {
    format!(
        "TraceView {}\n\
         A fast log viewer for Windows, in the spirit of CMTrace.\n\n\
         Copyright © 2026 Nathaniel Graham\n\
         Released under the MIT License. To see the licenses of the open-source \
         parts it includes, run: traceview --licenses",
        env!("CARGO_PKG_VERSION")
    )
}

/// Builds tab-separated text of the columns on screen, so pasted rows line up
/// in a spreadsheet or editor. Each row is (its number in the tab, its file,
/// its row in that file).
fn copy_rows(
    logs: &[SharedLog],
    rows: &[(usize, usize, usize)],
    columns: &[Column],
) -> io::Result<String> {
    let (mut buffer, mut scratch) = (Vec::new(), Vec::new());
    let mut text = String::new();
    for (number, source, row) in rows {
        let log = logs[*source]
            .read()
            .map_err(|_| io::Error::other("the log is unavailable"))?;
        log.read_rows(*row..row + 1, &mut buffer, |row, raw, _| {
            let cells = cells_from_row(&log, raw, &mut scratch);
            text.push_str(&(number + 1).to_string());
            for column in columns {
                text.push('\t');
                text.push_str(match column {
                    Column::Level => super::log_view::level_name(log.level(row)),
                    column => column.text(&cells),
                });
            }
            text.push_str("\r\n");
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
            show_sidebar: false,
            sidebar: Sidebar::new(sidebar::DEFAULT_WIDTH, &waker),
            pointer: Point::default(),
            cursor: Cursor::Arrow,
            show_chips: true,
            show_histogram: true,
            home: Home::default(),
            home_over_tabs: false,
            split_beside: None,
            chip_bar: ChipBar::default(),
            chips: Vec::new(),
            chip_filters: Vec::new(),
            histogram_hover: None,
            split: None,
            other_chip_bar: ChipBar::default(),
            other_chips: Vec::new(),
            size: (0.0, 0.0),
            theme_choice: ThemeChoice::System,
            system_is_dark,
            lookup: None,
            palette: None,
            rules_panel: None,
            columns_panel: None,
            fonts_panel: None,
            formats_panel: None,
            themes_panel: None,
            file_themes: Vec::new(),
            themes_folder: None,
            theme_problems: Vec::new(),
            reloader: None,
            rules: Arc::default(),
            keymap: Keymap::new(&BTreeMap::new()),
            formats: Vec::new(),
            shortcut_panel: None,
            keys_query: String::new(),
            row_menu: None,
            file_types: None,
            contrast: None,
            settings: Settings::default(),
            store: Store::disabled(),
            menu: MenuBar::default(),
            tab_bar: TabBar::default(),
            layout: Layout::default(),
            waker,
            hwnd,
            copying: None,
            exporting: None,
            notice: None,
            scale: 1.0,
        }
    }

    /// Starts from saved settings, and saves changes back through `store`.
    /// `problem` says why the settings file couldn't be used, if it couldn't.
    pub fn use_settings(&mut self, settings: Settings, store: Store, problem: Option<String>) {
        self.themes_folder = store.folder().map(|folder| folder.join(themes::FOLDER));
        let (file_themes, theme_problems) = self
            .themes_folder
            .as_deref()
            .map(themes::load)
            .unwrap_or_default();
        self.file_themes = file_themes;
        self.take_settings(settings);
        // A settings problem matters more than a theme file's.
        self.notice = problem.or_else(|| joined(&theme_problems));
        self.theme_problems = theme_problems;
        self.reloader = match (store.path(), &self.themes_folder) {
            (Some(path), Some(folder)) => Some(Reloader::start(
                path.to_path_buf(),
                store.known(),
                folder.clone(),
                self.waker.clone(),
            )),
            _ => None,
        };
        self.store = store;
    }

    /// Puts settings in place, at launch or when the file was edited.
    fn take_settings(&mut self, settings: Settings) {
        self.theme_choice = self
            .theme_named(&settings.theme)
            .map_or(ThemeChoice::System, ThemeChoice::Fixed);
        self.show_details = settings.details_pane;
        self.show_sidebar = settings.sidebar;
        self.show_chips = settings.chips;
        self.show_histogram = settings.histogram;
        self.sidebar.set_width(settings.sidebar_width);
        self.settings = settings;
        self.keymap = Keymap::new(&self.settings.keys);
        self.formats = settings::compile_formats(&self.settings.formats);
        self.apply_rules();
        let paths: BTreeSet<PathBuf> = self
            .tabs
            .iter()
            .flat_map(|tab| tab.paths.iter().cloned())
            .collect();
        for path in paths {
            let rows = self.settings.bookmarks_for(&path);
            for tab in &mut self.tabs {
                tab.update_bookmarks(&path, &rows);
            }
        }
    }

    /// Puts in place what live reload found. An edited settings file wins
    /// over the window's own state, and an editor of settings that's open is
    /// closed, since what it shows is out of date.
    fn reloaded(&mut self, reload: Reload, commands: &mut Commands) {
        match reload {
            Reload::Settings(settings, text) => {
                let editing = self.rules_panel.is_some()
                    || self.columns_panel.is_some()
                    || self.fonts_panel.is_some()
                    || self.formats_panel.is_some()
                    || self.themes_panel.is_some()
                    || self.shortcut_panel.is_some()
                    || self
                        .palette
                        .as_ref()
                        .is_some_and(|palette| palette.mode() != Mode::Everything);
                if editing {
                    self.close_panels();
                    commands.push(Command::SetImeEnabled(self.focus != Focus::Grid));
                }
                self.take_settings(*settings);
                self.store.loaded(text);
                if !self.show_sidebar && self.focus == Focus::Sidebar {
                    self.set_focus(Focus::Grid, commands);
                }
                self.notice = Some(
                    if editing {
                        "Settings were edited in traceview.toml, so they're loaded and the editor closed"
                    } else {
                        "Settings reloaded from traceview.toml"
                    }
                    .to_string(),
                );
            }
            Reload::Mistake(error) => {
                self.notice = Some(format!(
                    "traceview.toml has a mistake ({error}), so it wasn't loaded. Nothing is saved until it's fixed"
                ));
            }
            Reload::Themes(loaded, problems) => {
                self.file_themes = loaded;
                // The theme editor decides the theme while it's open.
                if self.themes_panel.is_none() {
                    if let ThemeChoice::Fixed(theme) = self.theme_choice {
                        if let Some(fresh) = self.theme_named(theme.name) {
                            self.theme_choice = ThemeChoice::Fixed(fresh);
                        }
                    }
                }
                if problems != self.theme_problems {
                    if let Some(problem) = joined(&problems) {
                        self.notice = Some(problem);
                    }
                    self.theme_problems = problems;
                }
            }
        }
        commands.redraw();
        commands.accessibility_changed();
    }

    /// Compiles the highlight rules and marks every tab's rows with them.
    fn apply_rules(&mut self) {
        self.rules = Arc::new(if self.settings.highlighting {
            RuleSet::compile(&self.settings.rules)
        } else {
            RuleSet::default()
        });
        for tab in &mut self.tabs {
            tab.set_rules(&self.rules);
        }
    }

    /// The fonts to draw with, from settings.
    pub fn fonts(&self) -> tv_render::Fonts {
        fonts::render_fonts(&self.settings.fonts)
    }

    /// Makes log text one size larger or smaller, or back to the default.
    fn resize_log_text(&mut self, larger: Option<bool>) {
        let size = match larger {
            Some(larger) => fonts::step_size(self.fonts().grid_size, &LOG_SIZES, larger),
            None => tv_render::Fonts::default().grid_size,
        };
        self.settings.fonts.log_size = size;
        self.notice = Some(format!("Log text is {size} pixels"));
        self.save_settings();
    }

    /// The installed fonts, for the palette to list. The one in use comes first.
    fn font_entries(&self, target: FontTarget) -> Vec<(PaletteItem, Action)> {
        let fonts = self.fonts();
        let current = match target {
            FontTarget::Log => fonts.grid_family,
            FontTarget::Window => fonts.ui_family,
        };
        let mut families = tv_render::installed_fonts();
        families.sort_by_key(|family| !family.name.eq_ignore_ascii_case(&current));
        families
            .into_iter()
            .map(|family| {
                let in_use = family.name.eq_ignore_ascii_case(&current);
                let detail = match (in_use, family.fixed_width) {
                    (true, true) => "in use, fixed width",
                    (true, false) => "in use",
                    (false, true) => "fixed width",
                    (false, false) => "",
                };
                (
                    PaletteItem::new(&family.name, detail, "", 0),
                    Action::SetFont(target, family.name),
                )
            })
            .collect()
    }

    /// A kind of log's columns, from settings. A tab that isn't open yet has
    /// none, and is drawn as a text log would be.
    fn columns_for(&self, kind: Option<LogKind>) -> Vec<ColumnSpec> {
        let kind = kind.unwrap_or_else(|| LogKind::builtin(Format::Plain));
        let saved = self.settings.columns.get(&kind.key());
        columns::from_settings(saved.map(Vec::as_slice), &kind)
    }

    /// Saves a kind of log's columns. Every tab of that kind follows them.
    fn set_columns(&mut self, kind: &LogKind, specs: &[ColumnSpec]) {
        self.settings
            .columns
            .insert(kind.key(), columns::to_settings(specs));
        self.save_settings();
    }

    /// Applies and saves what the rules editor holds, after it changed them.
    pub fn rules_edited(&mut self) {
        if let Some(panel) = &self.rules_panel {
            self.settings.rules = panel.saved_rules();
            self.apply_rules();
            self.save_settings();
        }
    }

    #[cfg(test)]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    fn save_settings(&mut self) {
        self.settings.theme = match self.theme_choice {
            ThemeChoice::System => "System",
            ThemeChoice::Fixed(theme) => theme.name,
        }
        .to_string();
        self.settings.details_pane = self.show_details;
        self.settings.sidebar = self.show_sidebar;
        self.settings.chips = self.show_chips;
        self.settings.histogram = self.show_histogram;
        self.settings.sidebar_width = self.sidebar.chosen_width();
        self.store.save(&self.settings);
    }

    /// The built-in layouts, then the saved ones.
    fn layouts(&self) -> Vec<LayoutSetting> {
        layouts::built_in()
            .into_iter()
            .chain(self.settings.layouts.iter().cloned())
            .collect()
    }

    /// What the window shows now, as a layout called `name`.
    fn current_layout(&self, name: &str) -> LayoutSetting {
        LayoutSetting {
            name: name.to_string(),
            sidebar: self.show_sidebar,
            sidebar_width: Some(self.sidebar.chosen_width()),
            chips: self.show_chips,
            histogram: self.show_histogram,
            details_pane: self.show_details,
        }
    }

    /// The layout a menu command picks, if it picks one.
    fn layout_for(&self, id: u32) -> Option<LayoutSetting> {
        let index = id.checked_sub(command::LAYOUTS)?;
        self.layouts().into_iter().nth(index as usize)
    }

    /// Shows the panes a layout names, and remembers them.
    fn apply_layout(&mut self, layout: &LayoutSetting, commands: &mut Commands) {
        self.show_sidebar = layout.sidebar;
        if let Some(width) = layout.sidebar_width {
            self.sidebar.set_width(width);
        }
        self.show_chips = layout.chips;
        self.show_histogram = layout.histogram;
        self.show_details = layout.details_pane;
        if !self.show_sidebar && self.focus == Focus::Sidebar {
            self.set_focus(Focus::Grid, commands);
        }
        self.save_settings();
    }

    /// Saves the panes shown now under a name, replacing a saved layout that
    /// has it. The palette doesn't offer built-in names.
    fn save_layout(&mut self, name: &str) {
        let name = name.trim();
        if layouts::is_built_in(name) {
            return;
        }
        let layout = self.current_layout(name);
        let layouts = &mut self.settings.layouts;
        match layouts
            .iter()
            .position(|saved| saved.name.eq_ignore_ascii_case(name))
        {
            Some(index) => layouts[index] = layout,
            None if layouts.len() >= settings::MOST_LAYOUTS => {
                self.notice = Some(format!(
                    "There are already {} layouts, so delete one first",
                    settings::MOST_LAYOUTS
                ));
                return;
            }
            None => layouts.push(layout),
        }
        self.notice = Some(format!("Saved layout {name}"));
        self.save_settings();
    }

    /// A built-in theme or one from a file, by name in any case.
    fn theme_named(&self, name: &str) -> Option<&'static Theme> {
        Theme::by_name(name).or_else(|| {
            self.file_themes
                .iter()
                .map(|file| file.theme)
                .find(|theme| theme.name.eq_ignore_ascii_case(name))
        })
    }

    /// The theme a menu command picks, if it picks one.
    fn theme_for(&self, id: u32) -> Option<&'static Theme> {
        builtin_theme_for(id).or_else(|| {
            let index = id.checked_sub(command::FILE_THEMES)? as usize;
            self.file_themes.get(index).map(|file| file.theme)
        })
    }

    pub fn theme(&self) -> &'static Theme {
        // A Windows contrast theme wins over any choice, while it's on.
        if let Some(contrast) = self.contrast {
            return contrast;
        }
        match self.theme_choice {
            ThemeChoice::System => Theme::for_system(self.system_is_dark),
            ThemeChoice::Fixed(theme) => theme,
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
        if let Some(exporting) = &self.exporting {
            let percent = exporting.done.load(Ordering::Relaxed) * 100 / exporting.total.max(1);
            if !status.is_empty() {
                status.push_str("   |   ");
            }
            status.push_str(&format!("Exporting {percent}%"));
        }
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
        let now = self.current_layout("");
        let paused = self.active_tab().is_some_and(Tab::is_paused);
        // Shortcuts come from the keymap, so menus always show the current ones.
        let item = |label: &str, id: u32| MenuItem::command(label, &self.keymap.label_for(id), id);
        vec![
            Menu {
                title: "File".to_string(),
                items: vec![
                    item("Open...", command::OPEN),
                    item("Open folder...", command::OPEN_FOLDER),
                    item("Merge open logs", command::MERGE),
                    item("Export rows as CSV...", command::EXPORT),
                    item("Close tab", command::CLOSE_TAB),
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
                    item("Find...", command::FIND),
                    item("Find next", command::FIND_NEXT),
                    item("Find previous", command::FIND_PREVIOUS),
                    item("Go to row...", command::GO_TO_ROW),
                    item("Next error", command::NEXT_ERROR),
                    item("Previous error", command::PREVIOUS_ERROR),
                    MenuItem::Separator,
                    item("Toggle bookmark", command::BOOKMARK),
                    item("Next bookmark", command::NEXT_BOOKMARK),
                    item("Previous bookmark", command::PREVIOUS_BOOKMARK),
                ],
            },
            Menu {
                title: "View".to_string(),
                items: vec![
                    item("Filter...", command::FILTER),
                    item("Clear filter", command::CLEAR_FILTER),
                    item("Save filter as preset...", command::SAVE_PRESET),
                    item("Command palette...", command::PALETTE),
                    MenuItem::Separator,
                    checked(item("Sidebar", command::SIDEBAR), self.show_sidebar),
                    enabled(
                        checked(item("Split view", command::SPLIT), self.split.is_some()),
                        !self.tabs.is_empty(),
                    ),
                    checked(item("Sync split panes", command::SYNC), self.split.as_ref().is_some_and(|split| split.sync)),
                    checked(item("Severity chips", command::CHIPS), self.show_chips),
                    checked(item("Histogram", command::HISTOGRAM), self.show_histogram),
                    checked(
                        item("Details pane", command::DETAILS),
                        self.show_details,
                    ),
                    item("Save layout as...", command::SAVE_LAYOUT),
                ]
                .into_iter()
                .chain(
                    self.layouts()
                        .into_iter()
                        .zip(command::LAYOUTS..)
                        .map(|(layout, id)| {
                            let shown = layouts::is_shown(&layout, &now);
                            checked(item(&format!("Layout: {}", layout.name), id), shown)
                        }),
                )
                .chain([
                    MenuItem::Separator,
                    checked(
                        item("Pause live updates", command::PAUSE),
                        paused,
                    ),
                    checked(
                        item("Use highlight rules", command::HIGHLIGHTING),
                        self.settings.highlighting,
                    ),
                    item("Columns...", command::COLUMNS),
                    item("Fonts...", command::FONTS),
                    // Edits wouldn't show while a contrast theme is drawn instead.
                    enabled(item("Edit theme...", command::EDIT_THEME), self.contrast.is_none()),
                    item("Larger log text", command::LARGER_TEXT),
                    item("Smaller log text", command::SMALLER_TEXT),
                    MenuItem::Separator,
                ])
                .chain(self.contrast.map(|_| {
                    let note = item("Theme: Windows contrast, while it's on", command::THEME_CONTRAST);
                    checked(enabled(note, false), true)
                }))
                .chain([checked(
                    item("Theme: follow Windows", command::THEME_SYSTEM),
                    theme == ThemeChoice::System,
                )])
                .chain(
                    Theme::ALL
                        .iter()
                        .copied()
                        .zip(command::THEMES..)
                        .chain(self.file_themes.iter().map(|file| file.theme).zip(command::FILE_THEMES..))
                        .map(|(choice, id)| {
                            let name = format!("Theme: {}", choice.name);
                            checked(item(&name, id), matches!(theme, ThemeChoice::Fixed(fixed) if fixed.name == choice.name))
                        }),
                )
                .collect(),
            },
            Menu {
                title: "Tools".to_string(),
                items: vec![
                    item("Error lookup...", command::ERROR_LOOKUP),
                    item("Highlight rules...", command::RULES),
                    item("Log formats...", command::FORMATS),
                    item("Keyboard shortcuts...", command::KEYS),
                    MenuItem::Separator,
                    item(
                        if self.registered_elsewhere() {
                            "Use this copy for .log files..."
                        } else {
                            "Use TraceView for .log files..."
                        },
                        command::USE_FOR_LOGS,
                    ),
                    enabled(
                        item("Stop using TraceView for .log files", command::STOP_FOR_LOGS),
                        self.file_types.is_some(),
                    ),
                ],
            },
            Menu {
                title: "Help".to_string(),
                items: vec![item("About TraceView", command::ABOUT)],
            },
        ]
    }

    /// Whether the start page shows: with no log open, or for a folder.
    pub fn home_shown(&self) -> bool {
        self.tabs.is_empty() || self.home_over_tabs
    }

    /// Lists a folder's logs on the start page, over any open logs.
    pub fn open_folder(&mut self, folder: PathBuf) {
        let absolute = std::path::absolute(&folder).unwrap_or(folder);
        self.settings.add_recent_folder(&absolute);
        self.home
            .show_folder(absolute, self.formats.clone(), &self.waker);
        self.home_over_tabs = !self.tabs.is_empty();
        self.focus = Focus::Grid;
        self.save_settings();
    }

    /// Does what was chosen on the start page.
    pub fn home_chose(&mut self, choice: HomeChoice, commands: &mut Commands) {
        match choice {
            HomeChoice::OpenLog => self.run_command(command::OPEN, commands),
            HomeChoice::OpenFolder => self.run_command(command::OPEN_FOLDER, commands),
            HomeChoice::Open(path) => self.open_paths(vec![path]),
            HomeChoice::Folder(path) => self.open_folder(path),
            HomeChoice::Back => {
                // Open logs come back first, then the start page.
                if self.home_over_tabs || self.tabs.is_empty() {
                    self.home.close_folder();
                }
                self.home_over_tabs = false;
            }
        }
        commands.redraw();
        commands.accessibility_changed();
    }

    fn home_event(&mut self, event: Event, commands: &mut Commands) {
        let response = self.home.handle(event);
        if let Some(choice) = response.chosen {
            self.home_chose(choice, commands);
        }
        if response.redraw {
            commands.redraw();
            commands.accessibility_changed();
        }
    }

    /// Opens files in new tabs, or switches to one that's already open. A
    /// folder lists its logs on the start page instead. No paths means the Open
    /// dialog was cancelled.
    pub fn open_paths(&mut self, paths: Vec<PathBuf>) {
        let beside = self.split_beside.take();
        if paths.is_empty() {
            return;
        }
        let mut first_log = None;
        for path in paths {
            if path.is_dir() {
                self.open_folder(path);
                continue;
            }
            self.home_over_tabs = false;
            let absolute = std::path::absolute(&path).unwrap_or(path);
            self.settings.add_recent(&absolute);
            if let Some(index) = self
                .tabs
                .iter()
                .position(|tab| tab.paths == [absolute.clone()])
            {
                self.active = index;
                first_log.get_or_insert(index);
                continue;
            }
            self.tabs
                .push(Tab::open(vec![absolute], self.formats.clone(), &self.waker));
            self.active = self.tabs.len() - 1;
            first_log.get_or_insert(self.active);
        }
        if let (Some(left), Some(right)) = (beside, first_log) {
            self.split_with(left, right);
        }
        self.focus = Focus::Grid;
        self.save_settings();
    }

    /// Shows two tabs side by side, with focus in `right`.
    fn split_with(&mut self, left: usize, right: usize) {
        if left == right || left >= self.tabs.len() {
            self.notice = Some(
                "That log is already open on its own, so choose another to see beside it"
                    .to_string(),
            );
            return;
        }
        self.active = right;
        self.split = Some(Split {
            other: left,
            focus_right: true,
            sync: false,
            splitter: Splitter::new(Orientation::Vertical, 0.0, MIN_PANE, MIN_PANE),
            focused_tab: right,
            synced: None,
        });
    }

    /// Exports the active tab's rows to a CSV file, in the background. The file
    /// appears only once it's complete.
    pub fn export_to(&mut self, path: PathBuf) {
        let Some(export) = self.active_tab().and_then(Tab::export) else {
            return;
        };
        let total = export.rows.len();
        let done = Arc::new(AtomicUsize::new(0));
        let finished = Arc::new(AtomicBool::new(false));
        let (sender, result) = channel();
        let (job_done, job_finished, waker, target) = (
            done.clone(),
            finished.clone(),
            self.waker.clone(),
            path.clone(),
        );
        std::thread::spawn(move || {
            let partial = target.with_extension("csv.part");
            let outcome = std::fs::File::create(&partial)
                .and_then(|file| export.write(file, &job_done))
                .and_then(|()| std::fs::rename(&partial, &target));
            if outcome.is_err() {
                let _ = std::fs::remove_file(&partial);
            }
            job_finished.store(true, Ordering::Release);
            let _ = sender.send(outcome);
            waker.wake();
        });
        // Keeps the progress moving in the status bar.
        let ticker_waker = self.waker.clone();
        std::thread::spawn(move || {
            while !finished.load(Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_millis(200));
                ticker_waker.wake();
            }
        });
        self.exporting = Some(Exporting {
            result,
            done,
            total,
            name: file_name(&path),
        });
        self.notice = None;
    }

    /// Clicking a chip filters to its level, and clicking it again, or All,
    /// shows every level.
    pub fn toggle_chip(&mut self, index: usize) {
        let Some(level) = self.chip_filters.get(index).copied() else {
            return;
        };
        let waker = self.waker.clone();
        if let Some(tab) = self.active_tab_mut() {
            let next = if tab.chip == level { None } else { level };
            tab.set_chip(next, &waker);
        }
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
                self.tabs
                    .push(Tab::open(paths, self.formats.clone(), &self.waker));
                self.active = self.tabs.len() - 1;
            }
        }
        self.focus = Focus::Grid;
    }

    fn close_tab(&mut self, index: usize) {
        if let Some(split) = &mut self.split {
            if split.other > index {
                split.other -= 1;
            } else if split.other == index {
                split.other = usize::MAX;
            }
            if split.focused_tab > index {
                split.focused_tab -= 1;
            }
        }
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
                    Focus::Grid | Focus::Sidebar => {}
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
        let columns: Vec<Column> = tab.view.shown_columns().collect();
        let waker = self.waker.clone();
        let (sender, result) = channel();
        std::thread::spawn(move || {
            let copied = copy_rows(&logs, &rows, &columns).map(|text| CopyResult {
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
            command::OPEN_FOLDER => commands.push(Command::OpenFolderDialog),
            command::CLOSE_TAB => self.close_tab(self.active),
            command::EXIT => commands.push(Command::Close),
            command::MERGE => self.merge_open_logs(),
            command::PALETTE => self.open_palette(Mode::Everything, "", commands),
            command::SPLIT => {
                if self.split.take().is_none() {
                    match self.tabs.len() {
                        0 => {
                            self.notice = Some(
                                "Open a log, then split the view to see another beside it"
                                    .to_string(),
                            )
                        }
                        // Asks for the log to show beside the only one open.
                        1 => {
                            self.split_beside = Some(self.active);
                            commands.push(Command::OpenFileDialog);
                        }
                        _ => {
                            self.split = Some(Split {
                                other: (self.active + 1) % self.tabs.len(),
                                focus_right: false,
                                sync: false,
                                splitter: Splitter::new(
                                    Orientation::Vertical,
                                    0.0,
                                    MIN_PANE,
                                    MIN_PANE,
                                ),
                                focused_tab: self.active,
                                synced: None,
                            });
                        }
                    }
                }
            }
            command::SYNC => match &mut self.split {
                Some(split) => {
                    split.sync = !split.sync;
                    split.synced = None;
                }
                None => self.notice = Some("Split the view first, with Ctrl+\\".to_string()),
            },
            command::GO_TO_ROW => self.open_palette(Mode::Everything, ":", commands),
            command::SAVE_LAYOUT => self.open_palette(Mode::SaveLayout, "", commands),
            command::USE_FOR_LOGS => commands.push(Command::SetFileTypes(true)),
            command::STOP_FOR_LOGS => commands.push(Command::SetFileTypes(false)),
            command::ABOUT => commands.push(Command::ShowMessage {
                title: "About TraceView".to_string(),
                text: about_text(),
            }),
            command::SAVE_PRESET => {
                let has_filter = self.active_tab().is_some_and(|tab| {
                    !tab.filter_field.text().trim().is_empty() || tab.chip.is_some()
                });
                if has_filter {
                    self.open_palette(Mode::SavePreset, "", commands);
                } else {
                    self.notice = Some(
                        "Set a filter or a severity chip first, then save it as a preset"
                            .to_string(),
                    );
                }
            }
            command::EXPORT => {
                if let Some(tab) = self.active_tab().filter(|tab| tab.document().is_some()) {
                    let name = match tab.paths.as_slice() {
                        [path] => path
                            .file_stem()
                            .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned()),
                        _ => "Merged logs".to_string(),
                    };
                    commands.push(Command::SaveFileDialog {
                        suggested_name: format!("{name}.csv"),
                    });
                }
            }
            command::BOOKMARK => self.toggle_bookmark(None),
            command::NEXT_BOOKMARK | command::PREVIOUS_BOOKMARK => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.step_bookmark(id == command::NEXT_BOOKMARK);
                }
            }
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
            command::CHIPS => self.show_chips = !self.show_chips,
            command::HISTOGRAM => self.show_histogram = !self.show_histogram,
            command::SIDEBAR => {
                self.show_sidebar = !self.show_sidebar;
                if !self.show_sidebar && self.focus == Focus::Sidebar {
                    self.set_focus(Focus::Grid, commands);
                }
            }
            command::PAUSE => {
                if let Some(tab) = self.active_tab_mut() {
                    let paused = !tab.is_paused();
                    tab.set_paused(paused);
                }
            }
            command::THEME_SYSTEM => self.theme_choice = ThemeChoice::System,
            id if self.theme_for(id).is_some() => {
                self.theme_choice = self
                    .theme_for(id)
                    .map_or(ThemeChoice::System, ThemeChoice::Fixed);
            }
            command::ERROR_LOOKUP => {
                // Start with the selected row's first code, as most lookups are for that.
                let code = self
                    .active_tab()
                    .and_then(|tab| tab.details.shown())
                    .and_then(|detail| first_code(&detail.message))
                    .unwrap_or_default();
                let mut panel = LookupPanel::new(&code, &waker);
                panel.arrange(self.layout.grid);
                self.close_panels();
                self.lookup = Some(panel);
                commands.push(Command::SetImeEnabled(true));
            }
            command::RULES => {
                // Editing rules that are switched off would show nothing.
                if !self.settings.highlighting {
                    self.settings.highlighting = true;
                    self.apply_rules();
                }
                self.close_panels();
                self.rules_panel = Some(RulesPanel::new(self.settings.rules.clone()));
                commands.push(Command::SetImeEnabled(true));
            }
            command::HIGHLIGHTING => {
                self.settings.highlighting = !self.settings.highlighting;
                self.apply_rules();
            }
            command::COLUMNS => {
                let kind = self
                    .active_tab()
                    .and_then(Tab::kind)
                    .unwrap_or_else(|| LogKind::builtin(Format::Plain));
                let specs = self.columns_for(Some(kind.clone()));
                self.close_panels();
                self.columns_panel = Some(ColumnsPanel::new(kind, specs));
                commands.push(Command::SetImeEnabled(false));
            }
            command::FONTS => self.open_fonts(FontItem::LogFont, commands),
            command::EDIT_THEME => self.open_theme_editor(commands),
            command::FORMATS => {
                let sample = self.read_sample();
                self.close_panels();
                self.formats_panel = Some(FormatsPanel::new(self.settings.formats.clone(), sample));
                commands.push(Command::SetImeEnabled(true));
            }
            command::KEYS => {
                self.keys_query.clear();
                self.open_palette(Mode::Keys, "", commands);
            }
            command::NEXT_TAB | command::PREVIOUS_TAB if !self.tabs.is_empty() => {
                let count = self.tabs.len();
                self.active = if id == command::PREVIOUS_TAB {
                    (self.active + count - 1) % count
                } else {
                    (self.active + 1) % count
                };
            }
            command::LARGER_TEXT => self.resize_log_text(Some(true)),
            command::SMALLER_TEXT => self.resize_log_text(Some(false)),
            command::DEFAULT_TEXT => self.resize_log_text(None),
            id if id >= command::LAYOUTS => {
                if let Some(layout) = self.layout_for(id) {
                    self.apply_layout(&layout, commands);
                }
            }
            _ => {}
        }
        if matches!(
            id,
            command::DETAILS
                | command::SIDEBAR
                | command::CHIPS
                | command::HISTOGRAM
                | command::THEME_SYSTEM
                | command::HIGHLIGHTING
                | command::RULES
        ) || self.theme_for(id).is_some()
        {
            self.save_settings();
        }
        commands.redraw();
        commands.accessibility_changed();
    }

    /// Handles shortcuts that work whatever has focus. Returns true if used.
    /// Runs the command a key press is a shortcut for. Returns true if used.
    fn shortcut(&mut self, key: Key, modifiers: Modifiers, commands: &mut Commands) -> bool {
        // Ctrl+1 to Ctrl+9 always pick a tab, so they aren't in the keymap.
        if let (Key::Char(digit @ '1'..='9'), true, false, false) =
            (key, modifiers.ctrl, modifiers.shift, modifiers.alt)
        {
            let index = digit as usize - '1' as usize;
            if index < self.tabs.len() {
                self.active = index;
                commands.redraw();
            }
            return true;
        }
        match self.keymap.command_for(key, modifiers) {
            Some(id) => {
                self.run_command(id, commands);
                true
            }
            None => false,
        }
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
                        Focus::Grid | Focus::Sidebar => false,
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
                    Focus::Grid | Focus::Sidebar => {}
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

    /// The columns editor takes all input while it's open, and every change
    /// applies at once.
    fn columns_event(&mut self, event: Event, commands: &mut Commands) {
        let Some(panel) = &mut self.columns_panel else {
            return;
        };
        let response = panel.handle(event);
        if response.changed {
            let (kind, specs) = (panel.kind.clone(), panel.columns.clone());
            self.set_columns(&kind, &specs);
        }
        if response.closed {
            self.columns_panel = None;
            commands.push(Command::SetImeEnabled(self.focus != Focus::Grid));
        }
        if response.redraw || response.changed || response.closed {
            commands.redraw();
            commands.accessibility_changed();
        }
    }

    /// The fonts editor takes all input while it's open, and every change
    /// applies at once. Choosing a font hands over to the palette's font list.
    fn fonts_event(&mut self, event: Event, commands: &mut Commands) {
        let Some(panel) = &mut self.fonts_panel else {
            return;
        };
        let response = panel.handle(event);
        self.fonts_responded(response, commands);
    }

    /// Acts on what the fonts editor asked for.
    pub fn fonts_responded(&mut self, response: FontsResponse, commands: &mut Commands) {
        if response.changed {
            if let Some(panel) = &self.fonts_panel {
                self.settings.fonts = panel.fonts.clone();
                self.save_settings();
            }
        }
        if let Some(target) = response.choose {
            self.open_palette(Mode::Font(target), "", commands);
        } else if response.closed {
            self.fonts_panel = None;
            commands.push(Command::SetImeEnabled(self.focus != Focus::Grid));
        }
        commands.redraw();
        commands.accessibility_changed();
    }

    /// Opens the theme editor on the theme in use. A built-in theme is edited
    /// as a copy, made with the first change.
    fn open_theme_editor(&mut self, commands: &mut Commands) {
        if self.contrast.is_some() {
            self.notice =
                Some("Themes can't be edited while a Windows contrast theme is on".to_string());
            return;
        }
        let theme = self.theme();
        let path = self
            .file_themes
            .iter()
            .find(|file| file.theme.name == theme.name)
            .map(|file| file.path.clone());
        let taken = themes::taken_names(
            &self.file_themes,
            self.themes_folder.as_deref(),
            path.as_deref(),
        );
        let name = match path {
            Some(_) => theme.name.to_string(),
            None => free_theme_name(&format!("{} copy", theme.name), &taken),
        };
        self.close_panels();
        self.themes_panel = Some(ThemesPanel::new(&name, *theme, path, taken));
        commands.push(Command::SetImeEnabled(true));
    }

    /// The theme editor takes all input while it's open.
    fn themes_event(&mut self, event: Event, canvas: &mut dyn Canvas, commands: &mut Commands) {
        let hwnd = self.hwnd;
        let Some(panel) = &mut self.themes_panel else {
            return;
        };
        let mut response = panel.handle(event, canvas);
        if response.wants_paste {
            if let Some(text) = tv_platform::clipboard_text(hwnd) {
                response.changed |= panel.paste(&text);
                response.redraw = true;
            }
        }
        if let Some(text) = response.copy.take() {
            commands.push(Command::SetClipboard(text));
        }
        self.themes_responded(&response, commands);
    }

    /// Acts on what the theme editor asked for. A changed theme is used at
    /// once and saved to its file, which a new name renames.
    pub fn themes_responded(&mut self, response: &ThemesResponse, commands: &mut Commands) {
        if response.changed {
            if let Some(panel) = &mut self.themes_panel {
                let kept = themes::keep(&panel.name, panel.theme);
                let old = panel.path.clone();
                // A file keeps its own name until the theme is renamed.
                let renamed = self
                    .file_themes
                    .iter()
                    .find(|file| Some(&file.path) == old.as_ref())
                    .is_none_or(|file| file.theme.name != panel.name);
                let target = match &old {
                    Some(old) if !renamed => Some(old.clone()),
                    _ => self
                        .themes_folder
                        .as_ref()
                        .map(|folder| folder.join(themes::file_name(&panel.name))),
                };
                if let (Some(old), Some(new)) = (&old, &target) {
                    if old != new && old.is_file() {
                        let _ = std::fs::rename(old, new);
                    }
                }
                let saved = target.as_deref().map(|path| themes::save(path, kept));
                panel.path = target.clone();
                self.notice = match saved {
                    Some(Ok(())) => None,
                    Some(Err(error)) => Some(format!("Couldn't save the theme: {error}")),
                    None => Some(
                        "The theme can't be saved, because there's no settings folder".to_string(),
                    ),
                };
                if let Some(path) = target {
                    let entry = self
                        .file_themes
                        .iter_mut()
                        .find(|file| Some(&file.path) == old.as_ref());
                    match entry {
                        Some(file) => *file = FileTheme { theme: kept, path },
                        None => self.file_themes.push(FileTheme { theme: kept, path }),
                    }
                }
                self.theme_choice = ThemeChoice::Fixed(kept);
                self.save_settings();
            }
        }
        if response.open_folder {
            let shown = self
                .themes_folder
                .as_deref()
                .is_some_and(tv_platform::show_folder);
            if !shown {
                self.notice = Some("The themes folder couldn't be shown".to_string());
            }
        }
        if response.closed {
            self.themes_panel = None;
            commands.push(Command::SetImeEnabled(self.focus != Focus::Grid));
        }
        commands.redraw();
        commands.accessibility_changed();
    }

    /// Reads the active log's first lines on another thread, for the formats
    /// editor to try patterns on.
    fn read_sample(&self) -> Option<Receiver<Sample>> {
        let source = self.active_tab()?.document()?.sources.first()?;
        let (log, path) = (source.log.clone(), source.path.clone());
        let (sender, receiver) = channel();
        let waker = self.waker.clone();
        std::thread::spawn(move || {
            let mut lines = Vec::new();
            if let Ok(log) = log.read() {
                let (mut buffer, mut scratch) = (Vec::new(), Vec::new());
                let _ = log.read_rows(0..SAMPLE_ROWS, &mut buffer, |_, raw, _| {
                    let text = log.text(raw, &mut scratch);
                    // An SCCM row can hold several lines, and only its first is tried.
                    let line = text.split(|byte| *byte == b'\n').next().unwrap_or(&[]);
                    let line = line.strip_suffix(b"\r").unwrap_or(line);
                    if !line.trim_ascii().is_empty() {
                        lines.push(tv_engine::encoding::display(line).into_owned());
                    }
                });
            }
            let _ = sender.send(Sample {
                file_name: file_name(&path),
                lines,
            });
            waker.wake();
        });
        Some(receiver)
    }

    /// The formats editor takes all input while it's open. Changes are saved
    /// at once, and used for logs opened from then on.
    fn formats_event(&mut self, event: Event, canvas: &mut dyn Canvas, commands: &mut Commands) {
        let hwnd = self.hwnd;
        let Some(panel) = &mut self.formats_panel else {
            return;
        };
        let mut response = panel.handle(event, canvas);
        if response.wants_paste {
            if let Some(text) = tv_platform::clipboard_text(hwnd) {
                response.changed |= panel.paste(&text);
                response.redraw = true;
            }
        }
        if let Some(text) = response.copy.take() {
            commands.push(Command::SetClipboard(text));
        }
        self.formats_responded(&response, commands);
    }

    /// Acts on what the formats editor asked for.
    pub fn formats_responded(&mut self, response: &FormatsResponse, commands: &mut Commands) {
        if response.changed {
            if let Some(panel) = &self.formats_panel {
                self.settings.formats = panel.saved_formats();
                self.formats = settings::compile_formats(&self.settings.formats);
                self.save_settings();
            }
        }
        if response.reopen {
            self.reopen_active_tab();
        }
        if response.closed {
            self.formats_panel = None;
            commands.push(Command::SetImeEnabled(self.focus != Focus::Grid));
        }
        commands.redraw();
        commands.accessibility_changed();
    }

    /// Reads the active tab's logs again, so a changed format takes effect.
    fn reopen_active_tab(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let paths = tab.paths.clone();
        self.tabs[self.active] = Tab::open(paths, self.formats.clone(), &self.waker);
        self.notice = Some("Reading the log again with the formats".to_string());
    }

    /// Applies a change a screen reader made in the columns editor.
    pub fn columns_edited(&mut self) {
        if let Some(panel) = &self.columns_panel {
            let (kind, specs) = (panel.kind.clone(), panel.columns.clone());
            self.set_columns(&kind, &specs);
        }
    }

    fn close_rules(&mut self, commands: &mut Commands) {
        self.rules_panel = None;
        commands.push(Command::SetImeEnabled(self.focus != Focus::Grid));
        commands.redraw();
        commands.accessibility_changed();
    }

    /// The rules editor takes all input while it's open, and every change to a
    /// rule applies at once.
    fn rules_event(&mut self, event: Event, canvas: &mut dyn Canvas, commands: &mut Commands) {
        let hwnd = self.hwnd;
        let Some(panel) = &mut self.rules_panel else {
            return;
        };
        let mut response = panel.handle(event, canvas);
        if response.wants_paste {
            if let Some(text) = tv_platform::clipboard_text(hwnd) {
                response.changed |= panel.paste(&text);
                response.redraw = true;
            }
        }
        if let Some(text) = response.copy.take() {
            commands.push(Command::SetClipboard(text));
        }
        if response.changed {
            self.rules_edited();
        }
        if response.closed {
            self.close_rules(commands);
        } else if response.redraw || response.changed {
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

    /// Offers an event to the sidebar. Returns true if the sidebar used it up.
    /// Every mouse move and release reaches it, so hover clears and a divider
    /// drag ends wherever the mouse goes.
    fn sidebar_event(&mut self, event: Event, commands: &mut Commands) -> bool {
        let consumed = match event {
            Event::MouseDown(point, ..) => self.sidebar.contains(point),
            Event::MouseMove(_) | Event::MouseUp(..) => self.sidebar.is_dragging(),
            Event::Wheel(_) => self.sidebar.contains(self.pointer),
            Event::KeyDown(Key::Escape, _) if self.focus == Focus::Sidebar => {
                self.set_focus(Focus::Grid, commands);
                return true;
            }
            Event::KeyDown(..) | Event::Char(_) => self.focus == Focus::Sidebar,
            Event::ZoomWheel(_) => false,
        };
        if !consumed && !matches!(event, Event::MouseMove(_) | Event::MouseUp(..)) {
            return false;
        }
        let response = self.sidebar.handle(event);
        self.update_cursor(commands);
        if let Event::MouseDown(point, MouseButton::Left, _) = event {
            if consumed && !self.sidebar.is_dragging() && self.sidebar.list.contains(point) {
                self.set_focus(Focus::Sidebar, commands);
            }
        }
        if response.resized.is_some() {
            self.save_settings();
        }
        if let Some(target) = response.chosen {
            self.choose(target, commands);
        }
        if response.redraw || consumed {
            commands.redraw();
            commands.accessibility_changed();
        }
        consumed
    }

    /// Shows the resize cursor over either divider, and over a column's edge.
    fn update_cursor(&mut self, commands: &mut Commands) {
        let resize = (self.show_sidebar && self.sidebar.shows_resize_cursor())
            || self
                .split
                .as_ref()
                .is_some_and(|split| split.splitter.shows_resize_cursor())
            || self
                .active_tab()
                .is_some_and(|tab| tab.view.shows_resize_cursor(self.pointer));
        let cursor = if resize {
            Cursor::ResizeColumns
        } else {
            Cursor::Arrow
        };
        if cursor != self.cursor {
            self.cursor = cursor;
            commands.push(Command::SetCursor(cursor));
        }
    }

    /// Offers an event to the split view. Dragging the divider resizes the
    /// panes, and the wheel scrolls the pane under the mouse. A click in the
    /// other pane moves focus there, and then goes on as a click in that pane.
    fn split_event(
        &mut self,
        event: Event,
        canvas: &mut dyn Canvas,
        commands: &mut Commands,
    ) -> bool {
        let Some(split) = &mut self.split else {
            return false;
        };
        let to_divider = match event {
            Event::MouseDown(point, ..) => split.splitter.contains(point),
            Event::MouseMove(_) | Event::MouseUp(..) => true,
            _ => false,
        };
        if to_divider {
            let dragging = split.splitter.is_dragging();
            let response = split.splitter.handle(event);
            if response.redraw {
                commands.redraw();
            }
            self.update_cursor(commands);
            if dragging || matches!(event, Event::MouseDown(..)) {
                return true;
            }
        }
        let Some(other) = self.layout.other else {
            return false;
        };
        let other_tab = self.split.as_ref().map_or(usize::MAX, |split| split.other);
        match event {
            Event::Wheel(_) if other.grid.contains(self.pointer) => {
                if let Some(tab) = self.tabs.get_mut(other_tab) {
                    tab.view.handle(event);
                    tab.following = tab.view.at_bottom();
                }
                commands.redraw();
                true
            }
            Event::MouseDown(point, ..) if other.area.contains(point) => {
                self.active = other_tab;
                let (size, scale) = (self.size, self.scale);
                self.arrange(canvas, size, scale);
                commands.redraw();
                commands.accessibility_changed();
                false
            }
            _ => false,
        }
    }

    /// Does what a sidebar entry stands for.
    pub fn choose(&mut self, target: Target, commands: &mut Commands) {
        match target {
            Target::Tab(index) if index < self.tabs.len() => self.active = index,
            Target::Bookmark { tab, row } if tab < self.tabs.len() => {
                self.active = tab;
                self.tabs[tab].select_file_row(row);
                self.set_focus(Focus::Grid, commands);
            }
            Target::Open(path) => self.open_paths(vec![path]),
            Target::Place(index) => self.sidebar.toggle_place(index, &self.waker),
            Target::Tab(_) | Target::Bookmark { .. } => {}
        }
        commands.redraw();
        commands.accessibility_changed();
    }

    /// What the palette lists: every enabled menu command with its shortcut,
    /// then open tabs, filter presets and recent logs.
    fn palette_entries(&self) -> Vec<(PaletteItem, Action)> {
        let mut entries = Vec::new();
        for menu in self.build_menus() {
            for item in menu.items {
                if let MenuItem::Command {
                    label,
                    shortcut,
                    command,
                    enabled: true,
                    ..
                } = item
                {
                    if command != command::PALETTE {
                        let label = label.trim_end_matches("...");
                        entries.push((
                            PaletteItem::new(label, &menu.title, &shortcut, 0),
                            Action::Command(command),
                        ));
                    }
                }
            }
        }
        for (index, tab) in self.tabs.iter().enumerate() {
            entries.push((
                PaletteItem::new(&tab.title(), "Open tab", "", 0),
                Action::Tab(index),
            ));
        }
        for (index, preset) in self.settings.presets.iter().enumerate() {
            let name = &preset.name;
            entries.push((
                PaletteItem::new(&format!("Filter preset: {name}"), &preset.filter, "", 0),
                Action::ApplyPreset(index),
            ));
            entries.push((
                PaletteItem::new(&format!("Delete filter preset: {name}"), "", "", 0),
                Action::DeletePreset(index),
            ));
        }
        for (index, layout) in self.settings.layouts.iter().enumerate() {
            entries.push((
                PaletteItem::new(&format!("Delete layout: {}", layout.name), "", "", 0),
                Action::DeleteLayout(index),
            ));
        }
        for recent in &self.settings.recent {
            let path = PathBuf::from(recent);
            let folder = path
                .parent()
                .map_or_else(String::new, |folder| folder.display().to_string());
            entries.push((
                PaletteItem::new(&file_name(&path), &folder, "", 0),
                Action::Open(path),
            ));
        }
        entries
    }

    /// Every command a shortcut can run, with its shortcut now, for the list
    /// of shortcuts. Commands outside the menus come after them.
    fn keys_entries(&self) -> Vec<(PaletteItem, Action)> {
        let mut entries = Vec::new();
        for menu in self.build_menus() {
            for item in menu.items {
                if let MenuItem::Command {
                    label,
                    shortcut,
                    command,
                    ..
                } = item
                {
                    if keymap::name_of(command).is_some() {
                        entries.push((
                            PaletteItem::new(
                                label.trim_end_matches("..."),
                                &menu.title,
                                &shortcut,
                                0,
                            ),
                            Action::Rebind(command),
                        ));
                    }
                }
            }
        }
        for (id, label, area) in [
            (command::NEXT_TAB, "Next tab", "Tabs"),
            (command::PREVIOUS_TAB, "Previous tab", "Tabs"),
            (command::DEFAULT_TEXT, "Default log text size", "View"),
        ] {
            entries.push((
                PaletteItem::new(label, area, &self.keymap.label_for(id), 0),
                Action::Rebind(id),
            ));
        }
        entries.push((
            PaletteItem::new("Reset every shortcut to its default", "", "", 0),
            Action::ResetKeys,
        ));
        entries
    }

    /// A command's name, as the list of shortcuts shows it.
    fn command_label(&self, id: u32) -> String {
        self.keys_entries()
            .into_iter()
            .find(|(_, action)| *action == Action::Rebind(id))
            .map_or_else(String::new, |(item, _)| item.label)
    }

    /// Opens the list of shortcuts again, as it was, after one changed.
    fn reopen_keys(&mut self, commands: &mut Commands) {
        let query = self.keys_query.clone();
        self.open_palette(Mode::Keys, &query, commands);
    }

    /// Does what was chosen for a command's shortcut, and says what changed.
    fn shortcut_chosen(&mut self, id: u32, choice: Choice, commands: &mut Commands) {
        let shortcut = match choice {
            Choice::Set(shortcut) => Some(shortcut),
            Choice::Remove => None,
            Choice::Default => keymap::default_of(id),
            Choice::Cancel => {
                self.reopen_keys(commands);
                return;
            }
        };
        let label = self.command_label(id);
        let lost = keymap::assign(&mut self.settings.keys, id, shortcut);
        self.keymap = Keymap::new(&self.settings.keys);
        let mut notice = match self.keymap.shortcut_for(id) {
            Some(shortcut) => format!("{label} is now {}", shortcut.label()),
            None => format!("{label} has no shortcut now"),
        };
        if let Some(other) = lost {
            notice.push_str(&format!(", which {} had", self.command_label(other)));
        }
        self.notice = Some(notice);
        self.save_settings();
        self.reopen_keys(commands);
    }

    /// The panel waiting for a shortcut takes all input.
    fn shortcut_event(&mut self, event: Event, commands: &mut Commands) {
        let Some(panel) = &mut self.shortcut_panel else {
            return;
        };
        let response = panel.handle(event);
        let id = panel.command;
        if let Some(choice) = response.choice {
            self.shortcut_panel = None;
            self.shortcut_chosen(id, choice, commands);
        }
        if response.redraw || response.choice.is_some() {
            commands.redraw();
            commands.accessibility_changed();
        }
    }

    /// Presses a button in the panel waiting for a shortcut, as a screen reader can.
    pub fn press_shortcut_button(&mut self, index: usize, commands: &mut Commands) {
        let Some(panel) = &self.shortcut_panel else {
            return;
        };
        if let Some(choice) = panel.press(index) {
            let id = panel.command;
            self.shortcut_panel = None;
            self.shortcut_chosen(id, choice, commands);
        }
    }

    fn open_palette(&mut self, mode: Mode, query: &str, commands: &mut Commands) {
        let rows = self.active_tab().map_or(0, Tab::total_rows);
        let entries = match mode {
            Mode::Font(target) => self.font_entries(target),
            Mode::Keys => self.keys_entries(),
            Mode::Everything | Mode::SavePreset | Mode::SaveLayout => self.palette_entries(),
        };
        let mut palette = CommandPalette::open(mode, entries, rows, query);
        palette.arrange(Rect::new(
            0.0,
            self.layout.menu.bottom,
            self.layout.status.right,
            self.layout.status.bottom,
        ));
        self.close_panels();
        self.palette = Some(palette);
        commands.push(Command::SetImeEnabled(true));
    }

    /// Closes whichever floating panel is open, since only one can be.
    /// Draws with a Windows contrast theme's colors while one is on. Each
    /// distinct set of colors is kept once, as file themes are.
    pub fn set_contrast(&mut self, colors: Option<ContrastColors>) {
        self.contrast =
            colors.map(|colors| themes::keep("Windows contrast", Theme::contrast(&colors)));
    }

    /// Whether `.log` files are registered to open with some other copy of
    /// TraceView, as when a portable one was moved.
    fn registered_elsewhere(&self) -> bool {
        let here = std::env::current_exe().ok();
        match (&self.file_types, here) {
            (Some(registered), Some(here)) => !registered
                .to_string_lossy()
                .eq_ignore_ascii_case(&here.to_string_lossy()),
            _ => false,
        }
    }

    /// Says how registering for `.log` files went.
    pub fn file_types_changed(&mut self, outcome: Result<Option<PathBuf>, String>) {
        let types = tv_platform::FILE_TYPES.join(" and ");
        self.notice = Some(match outcome {
            Ok(Some(path)) => {
                self.file_types = Some(path);
                format!("TraceView can open {types} files. To make it their default, pick it on the Default apps page that opened")
            }
            Ok(None) => {
                self.file_types = None;
                format!("TraceView no longer opens {types} files")
            }
            Err(error) => format!("Couldn't change what opens {types} files: {error}"),
        });
    }

    /// Bookmarks a row of the active tab, or removes its bookmark: the row
    /// given, in the file or merged timeline, or else the selected one. Every
    /// tab showing that file sees it.
    fn toggle_bookmark(&mut self, row: Option<usize>) {
        let toggled = self
            .active_tab_mut()
            .and_then(|tab| tab.toggle_bookmark(row));
        if let Some((path, rows)) = toggled {
            for tab in &mut self.tabs {
                tab.update_bookmarks(&path, &rows);
            }
            self.settings.set_bookmarks(&path, &rows);
            self.save_settings();
        }
    }

    /// Opens a row's menu at a point. It offers what applies to that row.
    fn open_row_menu(&mut self, view_row: usize, at: Point, from_keyboard: bool) {
        let Some(rows) = self.active_tab().and_then(Tab::rows) else {
            return;
        };
        let (row, bookmarked) = (rows.file_row(view_row), rows.bookmarked(view_row));
        drop(rows);
        let bookmark = if bookmarked {
            "Remove bookmark"
        } else {
            "Add bookmark"
        };
        let items = vec![
            MenuItem::command(
                bookmark,
                &self.keymap.label_for(command::BOOKMARK),
                command::BOOKMARK,
            ),
            MenuItem::command("Copy", "Ctrl+C", command::COPY),
        ];
        self.row_menu = Some(RowMenu {
            menu: ContextMenu::new(items, at, from_keyboard),
            row,
        });
    }

    /// The row menu takes all input while it's open, as any menu does.
    fn row_menu_event(&mut self, event: Event, commands: &mut Commands) {
        let Some(open) = &mut self.row_menu else {
            return;
        };
        let response = open.menu.handle(event);
        if response.closed {
            let row = open.row;
            self.row_menu = None;
            self.row_menu_chose(response.command, row, commands);
        }
        if response.redraw || response.closed {
            commands.redraw();
            commands.accessibility_changed();
        }
    }

    /// Chooses an item of the open row menu, as a screen reader can.
    pub fn press_row_menu_item(&mut self, item: usize, commands: &mut Commands) {
        let Some(open) = &mut self.row_menu else {
            return;
        };
        let response = open.menu.activate(item);
        if response.closed {
            let row = open.row;
            self.row_menu = None;
            self.row_menu_chose(response.command, row, commands);
            commands.redraw();
            commands.accessibility_changed();
        }
    }

    /// Does what was chosen from a row's menu, for its row.
    fn row_menu_chose(&mut self, command: Option<u32>, row: usize, commands: &mut Commands) {
        match command {
            Some(command::BOOKMARK) => self.toggle_bookmark(Some(row)),
            Some(id) => self.run_command(id, commands),
            None => {}
        }
    }

    fn close_panels(&mut self) {
        self.row_menu = None;
        self.lookup = None;
        self.palette = None;
        self.rules_panel = None;
        self.columns_panel = None;
        self.fonts_panel = None;
        self.formats_panel = None;
        self.themes_panel = None;
        self.shortcut_panel = None;
    }

    fn open_fonts(&mut self, selected: FontItem, commands: &mut Commands) {
        self.close_panels();
        self.fonts_panel = Some(FontsPanel::new(self.settings.fonts.clone(), selected));
        commands.push(Command::SetImeEnabled(false));
    }

    fn close_palette(&mut self, commands: &mut Commands) {
        self.palette = None;
        commands.push(Command::SetImeEnabled(self.focus != Focus::Grid));
    }

    /// The palette takes all input while it's open.
    fn palette_event(&mut self, event: Event, canvas: &mut dyn Canvas, commands: &mut Commands) {
        let hwnd = self.hwnd;
        let Some(palette) = &mut self.palette else {
            return;
        };
        let outcome = palette.handle(event, canvas);
        if let Some(text) = outcome.copy {
            commands.push(Command::SetClipboard(text));
        }
        if outcome.wants_paste {
            if let Some(text) = tv_platform::clipboard_text(hwnd) {
                palette.paste(&text);
            }
        }
        let mode = palette.mode();
        if mode == Mode::Keys {
            self.keys_query = palette.palette.query().to_string();
        }
        if outcome.cancelled || outcome.chosen.is_some() {
            self.close_palette(commands);
        }
        if let Some(action) = outcome.chosen {
            self.perform(action, commands);
        } else if let (true, Mode::Font(target)) = (outcome.cancelled, mode) {
            // Leaving the list of fonts goes back to the fonts editor.
            self.open_fonts(FontItem::for_target(target), commands);
        }
        commands.redraw();
        commands.accessibility_changed();
    }

    /// Chooses a palette match, as a screen reader can.
    pub fn choose_palette_match(&mut self, index: usize, commands: &mut Commands) {
        if let Some(action) = self
            .palette
            .as_ref()
            .and_then(|palette| palette.action_at(index))
        {
            self.close_palette(commands);
            self.perform(action, commands);
        }
    }

    /// Does what a palette entry stands for.
    pub fn perform(&mut self, action: Action, commands: &mut Commands) {
        let waker = self.waker.clone();
        match action {
            Action::Command(id) => self.run_command(id, commands),
            Action::Tab(index) if index < self.tabs.len() => self.active = index,
            Action::Open(path) => self.open_paths(vec![path]),
            Action::GoToRow(row) => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.select_file_row(row);
                }
                self.set_focus(Focus::Grid, commands);
            }
            Action::ApplyPreset(index) => {
                let preset = self.settings.presets.get(index).cloned();
                if let (Some(preset), Some(tab)) = (preset, self.active_tab_mut()) {
                    tab.filter_field.set_text(&preset.filter);
                    tab.chip = Level::from_name(preset.level.as_bytes());
                    tab.apply_filter(&waker);
                }
            }
            Action::DeletePreset(index) if index < self.settings.presets.len() => {
                let removed = self.settings.presets.remove(index);
                self.notice = Some(format!("Deleted filter preset {}", removed.name));
                self.save_settings();
            }
            Action::Rebind(id) => {
                let label = self.command_label(id);
                let panel = ShortcutPanel::new(
                    id,
                    &label,
                    self.keymap.shortcut_for(id),
                    keymap::default_of(id),
                );
                self.close_panels();
                self.shortcut_panel = Some(panel);
                commands.push(Command::SetImeEnabled(false));
            }
            Action::ResetKeys => {
                self.settings.keys.clear();
                self.keymap = Keymap::new(&self.settings.keys);
                self.notice = Some("Every shortcut is back to its default".to_string());
                self.save_settings();
                self.reopen_keys(commands);
            }
            Action::SetFont(target, name) => {
                match target {
                    FontTarget::Log => self.settings.fonts.log_font = name,
                    FontTarget::Window => self.settings.fonts.window_font = name,
                }
                self.save_settings();
                self.open_fonts(FontItem::for_target(target), commands);
            }
            Action::SavePreset(name) => {
                let Some(tab) = self.active_tab() else { return };
                let preset = Preset {
                    name: name.clone(),
                    filter: tab.filter_field.text().trim().to_string(),
                    level: tab
                        .chip
                        .map(|level| format!("{level:?}").to_lowercase())
                        .unwrap_or_default(),
                };
                match self
                    .settings
                    .presets
                    .iter_mut()
                    .find(|saved| saved.name.eq_ignore_ascii_case(&name))
                {
                    Some(saved) => *saved = preset,
                    None => self.settings.presets.push(preset),
                }
                self.notice = Some(format!("Saved filter preset {name}"));
                self.save_settings();
            }
            Action::DeleteLayout(index) if index < self.settings.layouts.len() => {
                let removed = self.settings.layouts.remove(index);
                self.notice = Some(format!("Deleted layout {}", removed.name));
                self.save_settings();
            }
            Action::SaveLayout(name) => self.save_layout(&name),
            Action::Tab(_)
            | Action::DeletePreset(_)
            | Action::DeleteLayout(_)
            | Action::Nothing => {}
        }
        commands.redraw();
        commands.accessibility_changed();
    }

    fn menu_wants(&self, event: &Event) -> bool {
        let open = self.menu.open_menu().is_some();
        match event {
            Event::KeyDown(key, modifiers) => {
                self.menu.is_active() || modifiers.alt || (*key == Key::F(10) && !modifiers.shift)
            }
            Event::MouseDown(point, ..) => open || self.menu.contains(*point),
            Event::MouseMove(_)
            | Event::MouseUp(..)
            | Event::Wheel(_)
            | Event::ZoomWheel(_)
            | Event::Char(_) => open,
        }
    }

    /// Routes input to the menu, a shortcut, the tab bar, a text field or the grid.
    pub fn event(&mut self, event: Event, canvas: &mut dyn Canvas, commands: &mut Commands) {
        if self.row_menu.is_some() {
            self.row_menu_event(event, commands);
            return;
        }
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
        if self.shortcut_panel.is_some() {
            self.shortcut_event(event, commands);
            return;
        }
        if self.rules_panel.is_some() {
            self.rules_event(event, canvas, commands);
            return;
        }
        if self.columns_panel.is_some() {
            self.columns_event(event, commands);
            return;
        }
        if self.fonts_panel.is_some() {
            self.fonts_event(event, commands);
            return;
        }
        if self.formats_panel.is_some() {
            self.formats_event(event, canvas, commands);
            return;
        }
        if self.themes_panel.is_some() {
            self.themes_event(event, canvas, commands);
            return;
        }
        if self.lookup.is_some() {
            self.lookup_event(event, canvas, commands);
            return;
        }
        if self.palette.is_some() {
            self.palette_event(event, canvas, commands);
            return;
        }
        // Ctrl and the wheel make log text larger or smaller, as in editors.
        if let Event::ZoomWheel(rows) = event {
            if rows != 0.0 {
                self.resize_log_text(Some(rows > 0.0));
                commands.redraw();
                commands.accessibility_changed();
            }
            return;
        }
        if let Event::KeyDown(key, modifiers) = event {
            if self.shortcut(key, modifiers, commands) {
                return;
            }
            if key == Key::Tab && !modifiers.ctrl && self.focus != Focus::Grid {
                let sidebar = if self.show_sidebar {
                    Focus::Sidebar
                } else {
                    Focus::Grid
                };
                let next = match (self.focus, modifiers.shift) {
                    (Focus::Filter, false) => Focus::Find,
                    (Focus::Find, false) => sidebar,
                    (Focus::Sidebar, false) => Focus::Grid,
                    (Focus::Grid, false) | (Focus::Find, true) => Focus::Filter,
                    (Focus::Filter, true) => Focus::Grid,
                    (Focus::Sidebar, true) => Focus::Find,
                    (Focus::Grid, true) => {
                        if self.show_sidebar {
                            Focus::Sidebar
                        } else {
                            Focus::Find
                        }
                    }
                };
                self.set_focus(next, commands);
                return;
            }
        }
        if let Event::MouseMove(point) = event {
            self.pointer = point;
        }
        if self.show_sidebar && self.sidebar_event(event, commands) {
            return;
        }
        if self.split.is_some() && self.split_event(event, canvas, commands) {
            return;
        }

        let layout = self.layout;
        // The start page takes everything below the tabs.
        if self.home_shown()
            && !matches!(event, Event::MouseDown(point, ..) if layout.tabs.contains(point))
        {
            self.home_event(event, commands);
            return;
        }
        if let Event::MouseDown(point, button, _) = event {
            if layout.tabs.contains(point) {
                let response = self.tab_bar.handle(event);
                if let Some(index) = response.close {
                    self.close_tab(index);
                } else if let Some(index) = response.select {
                    self.active = index;
                    self.home_over_tabs = false;
                }
                commands.redraw();
                commands.accessibility_changed();
                return;
            }
            if layout.chips.contains(point) {
                if let Some(index) = self.chip_bar.handle(event).toggled {
                    self.toggle_chip(index);
                }
                commands.redraw();
                commands.accessibility_changed();
                return;
            }
            if layout.histogram.contains(point) {
                let buckets = self
                    .active_tab()
                    .and_then(|tab| tab.overview.counts.histogram.as_ref())
                    .map_or(0, |histogram| histogram.buckets.len());
                if let (Some(bucket), Some(tab)) = (
                    overview::bucket_at(layout.histogram, buckets, point.x),
                    self.active_tab_mut(),
                ) {
                    tab.jump_to_bucket(bucket);
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
        if let Event::MouseMove(point) = event {
            let buckets = self
                .active_tab()
                .and_then(|tab| tab.overview.counts.histogram.as_ref())
                .map_or(0, |histogram| histogram.buckets.len());
            let hover = layout
                .histogram
                .contains(point)
                .then(|| overview::bucket_at(layout.histogram, buckets, point.x))
                .flatten();
            if hover != self.histogram_hover || self.chip_bar.handle(event).redraw {
                self.histogram_hover = hover;
                commands.redraw();
            }
            if self.tab_bar.handle(event).redraw {
                commands.redraw();
            }
            // Whatever has focus, a column's edge shows the resize pointer.
            self.update_cursor(commands);
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
                Focus::Grid | Focus::Sidebar => FieldResponse::default(),
            };
            self.handle_field(focus, response, commands);
            return;
        }
        let response = tab.view.handle(event);
        tab.following = tab.view.at_bottom();
        // A right-click opens the menu where it was, and the keyboard below the row.
        let row_menu = response.row_menu.map(|row| match event {
            Event::MouseDown(point, ..) => (row, point, false),
            _ => {
                let rect = tab.view.row_rect(row);
                (row, Point::new(rect.left + 40.0, rect.bottom), true)
            }
        });
        let changed_columns = response
            .columns_changed
            .then(|| (tab.kind(), tab.view.columns().to_vec()));
        if let Some((Some(kind), specs)) = changed_columns {
            self.set_columns(&kind, &specs);
        }
        if response.columns_menu {
            self.run_command(command::COLUMNS, commands);
        }
        if let Some((row, at, from_keyboard)) = row_menu {
            self.open_row_menu(row, at, from_keyboard);
        }
        if matches!(event, Event::MouseMove(_) | Event::MouseUp(..)) {
            self.update_cursor(commands);
        }
        if response.copy_selection {
            self.start_copy();
        }
        if response.redraw || response.columns_changed {
            commands.redraw();
            commands.accessibility_changed();
        }
    }

    /// Picks up finished background work from every tab.
    pub fn woken(&mut self, commands: &mut Commands) {
        let waker = self.waker.clone();
        let reloads = self
            .reloader
            .as_ref()
            .map(Reloader::take)
            .unwrap_or_default();
        for reload in reloads {
            self.reloaded(reload, commands);
        }
        for tab in &mut self.tabs {
            tab.poll(&waker, &self.settings, &self.rules);
        }
        if let Some(panel) = &mut self.lookup {
            panel.poll();
        }
        if let Some(panel) = &mut self.formats_panel {
            panel.poll();
        }
        self.sidebar.poll();
        self.home.poll(&waker);
        let exported =
            self.exporting
                .as_ref()
                .and_then(|exporting| match exporting.result.try_recv() {
                    Ok(outcome) => {
                        Some(outcome.map(|()| (exporting.total, exporting.name.clone())))
                    }
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => {
                        Some(Err(io::Error::other("the export stopped")))
                    }
                });
        if let Some(outcome) = exported {
            self.exporting = None;
            self.notice = Some(match outcome {
                Ok((rows, name)) => format!("Exported {} rows to {name}", with_commas(rows)),
                Err(error) => format!("Couldn't export: {error}"),
            });
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
        self.menu.arrange(menu_bar, size.1, canvas);
        if self.show_sidebar {
            self.sidebar
                .rebuild(&self.tabs, self.active, &self.settings, &self.waker);
            area = self.sidebar.arrange(area);
        }
        let labels: Vec<TabLabel> = self.tab_labels();
        let tabs = if self.tabs.is_empty() {
            Rect::default()
        } else {
            area.take_top(snap(tab_bar::BAR_HEIGHT, scale))
        };
        self.tab_bar.arrange(tabs, &labels, canvas);
        // The start page takes the place of the panes, so none are laid out.
        let home = self.home_shown();
        let ready = |tab: Option<&Tab>| !home && tab.is_some_and(|tab| tab.document().is_some());
        let show_histogram = self.show_histogram;
        let with_histogram = |tab: Option<&Tab>| {
            show_histogram && tab.is_some_and(|tab| tab.overview.counts.histogram.is_some())
        };
        // Bars grow to fit a larger font, and never shrink below the theme's height.
        let status_height = theme
            .status_height
            .max(canvas.line_height(FontRole::Ui) + 6.0);
        let status = area.take_bottom(snap(status_height, scale));
        let mut details = Rect::default();
        if self.show_details && ready(self.active_tab()) {
            details = area.take_bottom(snap(PANE_HEIGHT, scale));
        }

        self.check_split();
        let other_tab = self
            .split
            .as_ref()
            .and_then(|split| self.tabs.get(split.other));
        let (other_ready, other_histogram) = (ready(other_tab), with_histogram(other_tab));
        let (focused_area, other_area) = match &mut self.split {
            Some(split) if !home => {
                if split.splitter.position() <= 0.0 {
                    split.splitter.set_position(area.width() / 2.0);
                }
                split.splitter.arrange(area);
                let (left, right) = (split.splitter.first_rect(), split.splitter.second_rect());
                if split.focus_right {
                    (right, Some(left))
                } else {
                    (left, Some(right))
                }
            }
            _ => (area, None),
        };
        let focused = pane_layout(
            focused_area,
            ready(self.active_tab()),
            self.show_chips,
            with_histogram(self.active_tab()),
            scale,
        );
        let other = other_area
            .map(|area| pane_layout(area, other_ready, self.show_chips, other_histogram, scale));

        // Chips exist only while they show, so hidden ones can't be clicked
        // or heard by a screen reader.
        let (chip_list, filters) = match self.active_tab() {
            Some(tab) if focused.chips.height() > 0.0 => {
                overview::chips(&tab.overview.counts.levels, tab.chip, theme)
            }
            _ => (Vec::new(), Vec::new()),
        };
        self.chip_bar.arrange(focused.chips, &chip_list, canvas);
        self.chips = chip_list;
        self.chip_filters = filters;
        let other_chips = match (other, other_tab) {
            (Some(pane), Some(tab)) if pane.chips.height() > 0.0 => {
                overview::chips(&tab.overview.counts.levels, tab.chip, theme).0
            }
            _ => Vec::new(),
        };
        self.other_chip_bar.arrange(
            other.map_or_else(Rect::default, |pane| pane.chips),
            &other_chips,
            canvas,
        );
        self.other_chips = other_chips;

        self.size = size;
        self.layout = Layout {
            menu: menu_bar,
            tabs,
            filter: focused.filter,
            find: focused.find,
            find_count: focused.find_count,
            chips: focused.chips,
            histogram: focused.histogram,
            grid: focused.grid,
            details,
            status,
            focused_area,
            other,
        };
        if home {
            let shortcuts = [
                self.keymap.label_for(command::OPEN),
                self.keymap.label_for(command::OPEN_FOLDER),
            ];
            self.home
                .rebuild(&self.settings, self.sidebar.places(), theme, shortcuts);
            self.home.arrange(self.layout.grid, canvas);
        }
        let grid_line = canvas.line_height(FontRole::Grid);
        let spacing = Spacing::from_name(&self.settings.fonts.row_spacing);
        let metrics = Metrics {
            row_height: snap(grid_line + spacing.padding(), scale),
            char_width: canvas.char_width(FontRole::Grid),
            header_height: snap(theme.header_height.max(grid_line + 6.0), scale),
        };
        let grid = self.layout.grid;
        if let Some(panel) = &mut self.lookup {
            panel.arrange(grid);
        }
        if let Some(palette) = &mut self.palette {
            palette.arrange(Rect::new(0.0, self.layout.menu.bottom, size.0, size.1));
        }
        let below_menu = Rect::new(0.0, self.layout.menu.bottom, size.0, size.1);
        if let Some(panel) = &mut self.rules_panel {
            panel.arrange(below_menu, canvas, theme);
        }
        if let Some(panel) = &mut self.columns_panel {
            panel.arrange(below_menu, canvas, theme);
        }
        if let Some(panel) = &mut self.fonts_panel {
            panel.arrange(below_menu, canvas, theme);
        }
        if let Some(panel) = &mut self.shortcut_panel {
            panel.arrange(below_menu, canvas, theme);
        }
        if let Some(panel) = &mut self.formats_panel {
            panel.arrange(below_menu, canvas, theme);
        }
        if let Some(panel) = &mut self.themes_panel {
            panel.arrange(below_menu, canvas, theme);
        }
        if let Some(open) = &mut self.row_menu {
            open.menu
                .arrange(Rect::new(0.0, 0.0, size.0, size.1), canvas);
        }
        let other_index = self.split.as_ref().map(|split| split.other);
        for (index, pane) in [(Some(self.active), Some(focused)), (other_index, other)] {
            let (Some(index), Some(pane)) = (index, pane) else {
                continue;
            };
            let columns = self.columns_for(self.tabs.get(index).and_then(Tab::kind));
            if let Some(tab) = self.tabs.get_mut(index) {
                let count = tab.rows().map_or(0, |rows| rows.row_count());
                tab.view.arrange(pane.grid, count, metrics, &columns);
                if tab.following {
                    tab.view.scroll_to_end();
                }
            }
        }
        self.line_up();
    }

    /// Keeps the split view's tabs valid. Choosing the tab the other pane
    /// shows, by clicking that pane or its tab, moves focus to that pane.
    fn check_split(&mut self) {
        if self.tabs.len() < 2 {
            self.split = None;
        }
        let Some(split) = &mut self.split else { return };
        let count = self.tabs.len();
        if split.other >= count {
            split.other = (self.active + 1) % count;
        }
        if split.other == self.active {
            split.other = if split.focused_tab < count && split.focused_tab != self.active {
                split.focused_tab
            } else {
                (self.active + 1) % count
            };
            split.focus_right = !split.focus_right;
        }
        split.focused_tab = self.active;
    }

    /// With sync on, scrolls the other pane to the time at the top of the
    /// focused one, and follows new rows when it does.
    fn line_up(&mut self) {
        let Some(split) = &self.split else { return };
        let Some(tab) = self.tabs.get(self.active).filter(|_| split.sync) else {
            return;
        };
        let top = tab.view.top_row();
        if split.synced == Some((self.active, top)) {
            return;
        }
        let (other, following) = (split.other, tab.following);
        let Some(seconds) = tab.time_at(top) else {
            return;
        };
        if let Some(other) = self.tabs.get_mut(other) {
            other.scroll_to_time(seconds);
            other.following = following;
        }
        if let Some(split) = &mut self.split {
            split.synced = Some((self.active, top));
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
        // Some of what screen readers hear comes from layout, such as the chips'
        // counts and an editor's buttons, so each frame brings them up to date.
        // Nothing is built unless a screen reader is listening.
        commands.accessibility_changed();
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
        let home = self.home_shown();
        match self.tabs.get_mut(self.active) {
            _ if home => self.home.draw(canvas, theme, focus == Focus::Grid),
            None => {}
            Some(tab) => {
                tab.request_rows(&waker);
                let pane = PaneLayout {
                    area: layout.focused_area,
                    filter: layout.filter,
                    find: layout.find,
                    find_count: layout.find_count,
                    chips: layout.chips,
                    histogram: layout.histogram,
                    grid: layout.grid,
                };
                let chips = (&self.chip_bar, self.chips.as_slice());
                caret = draw_pane(
                    canvas,
                    tab,
                    &pane,
                    theme,
                    Some(focus),
                    chips,
                    self.histogram_hover,
                );
                if self.show_details && tab.document().is_some() {
                    tab.details.draw(canvas, layout.details, theme);
                }
            }
        }
        if let (Some(split), Some(other), false) = (&self.split, layout.other, home) {
            if let Some(tab) = self.tabs.get_mut(split.other) {
                tab.request_rows(&waker);
                let chips = (&self.other_chip_bar, self.other_chips.as_slice());
                draw_pane(canvas, tab, &other, theme, None, chips, None);
            }
            split.splitter.draw(canvas, theme, false);
            // The focused pane is marked, as in the Mac app.
            let focused = layout.focused_area;
            canvas.fill_rect(
                Rect::new(focused.left, focused.top, focused.right, focused.top + 2.0),
                theme.accent,
            );
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

        if self.show_sidebar {
            self.sidebar.draw(canvas, theme, focus == Focus::Sidebar);
        }
        if let Some(panel) = &mut self.lookup {
            caret = Some(panel.draw(canvas, theme));
        }
        if let Some(palette) = &mut self.palette {
            caret = Some(palette.draw(canvas, theme));
        }
        if let Some(panel) = &mut self.rules_panel {
            let field_caret = panel.draw(canvas, theme);
            if panel.focus == Part::Field {
                caret = Some(field_caret);
            }
        }
        if let Some(panel) = &self.columns_panel {
            panel.draw(canvas, theme);
        }
        if let Some(panel) = &self.fonts_panel {
            panel.draw(canvas, theme);
        }
        if let Some(panel) = &self.shortcut_panel {
            panel.draw(canvas, theme);
        }
        if let Some(panel) = &mut self.formats_panel {
            let field_caret = panel.draw(canvas, theme);
            if panel.focus == FormatsPart::Field {
                caret = Some(field_caret);
            }
        }
        if let Some(panel) = &mut self.themes_panel {
            let field_caret = panel.draw(canvas, theme);
            if panel.focus == ThemesPart::Field {
                caret = Some(field_caret);
            }
        }

        self.menu.draw_bar(canvas, layout.menu, theme);
        self.menu.draw_popup(canvas, theme);
        if let Some(open) = &self.row_menu {
            open.menu.draw(canvas, theme);
        }

        if let Some(caret) = caret {
            commands.push(Command::SetImePosition(Point::new(caret.x, caret.y)));
        }
    }

    /// The chips showing, with where each one is.
    pub fn chip_list(&self) -> impl Iterator<Item = (&Chip, Option<Rect>)> {
        self.chips
            .iter()
            .enumerate()
            .map(|(index, chip)| (chip, self.chip_bar.chip_rect(index)))
    }

    pub fn histogram_rect(&self) -> Rect {
        self.layout.histogram
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
    use tv_ui::Color;

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
    const SHIFT: Modifiers = Modifiers {
        ctrl: false,
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
        workspace.run_command(command::THEMES + 1, &mut commands);
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
        let plain: Vec<Column> = columns::shown(&columns::defaults(Format::Plain)).collect();
        assert_eq!(
            copy_rows(&logs, &[(0, 0, 0), (1, 0, 1)], &plain).unwrap(),
            "1\t2026-01-01 10:00:00.000\tError\tapp\tdisk full\r\n2\t\tInformation\t\tnext\r\n"
        );
        // Copies follow the columns on screen, in their order.
        let moved = [Column::Message, Column::Level];
        assert_eq!(
            copy_rows(&logs, &[(0, 0, 0)], &moved).unwrap(),
            "1\tdisk full\tError\r\n"
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
    fn with_no_tabs_the_start_page_shows() {
        let mut workspace = workspace();
        let mut canvas = RecordingCanvas::new();
        workspace.draw(&mut canvas, SIZE, 1.0, &mut Commands::default());
        let texts = canvas.texts();
        for text in ["TraceView", "Open log...", "Open folder...", "Ctrl+Shift+O"] {
            assert!(texts.contains(&text), "{text} in {texts:?}");
        }
        assert!(canvas.clips_balance());

        let mut commands = Commands::default();
        workspace.home_chose(HomeChoice::OpenFolder, &mut commands);
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::OpenFolderDialog)));
    }

    /// The start page's entry with this label, as a choice.
    fn home_entry(workspace: &Workspace, label: &str) -> Option<HomeChoice> {
        let index = workspace.home.list.items.iter().position(|item| {
            matches!(item, tv_ui::list_view::ListItem::Entry { label: shown, .. } if shown == label)
        })?;
        workspace.home.target_of(index)
    }

    #[test]
    fn a_folder_lists_its_logs_and_one_click_opens_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = log_file(&dir, "app.log", 20);
        log_file(&dir, "other.log", 5);
        let mut workspace = workspace();
        // Dropping a folder, or choosing one, comes here as a path.
        workspace.open_paths(vec![dir.path().to_path_buf()]);
        assert!(workspace.home_shown());
        settle(&mut workspace, |workspace| {
            workspace.home.subtitle().ends_with("2 logs, 2 with errors")
        });
        assert_eq!(workspace.settings().recent_folders.len(), 1);

        let mut commands = Commands::default();
        let choice = home_entry(&workspace, "app.log").unwrap();
        workspace.home_chose(choice, &mut commands);
        assert_eq!(workspace.tab_titles(), ["app.log"]);
        assert!(!workspace.home_shown());
        assert_eq!(workspace.active_tab().unwrap().paths, [path]);

        // With a log open, a folder shows over it, and Escape goes back to the log.
        workspace.open_paths(vec![dir.path().to_path_buf()]);
        assert!(workspace.home_shown());
        send(
            &mut workspace,
            Event::KeyDown(Key::Escape, Modifiers::default()),
        );
        assert!(!workspace.home_shown());
        assert_eq!(workspace.home.folder(), None);
    }

    #[test]
    fn closing_the_last_log_returns_to_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        log_file(&dir, "app.log", 20);
        let mut workspace = workspace();
        workspace.open_folder(dir.path().to_path_buf());
        settle(&mut workspace, |workspace| {
            home_entry(workspace, "app.log").is_some()
        });
        let choice = home_entry(&workspace, "app.log").unwrap();
        workspace.home_chose(choice, &mut Commands::default());
        workspace.run_command(command::CLOSE_TAB, &mut Commands::default());
        assert!(workspace.home_shown());
        assert!(workspace.home.folder().is_some());
        send(
            &mut workspace,
            Event::KeyDown(Key::Escape, Modifiers::default()),
        );
        assert_eq!(workspace.home.folder(), None);
    }

    #[test]
    fn the_histogram_shows_by_default() {
        let (_dir, mut workspace) = opened(100);
        settle(&mut workspace, |workspace| {
            workspace.histogram_rect().height() > 0.0
        });
        assert!(workspace.settings().histogram);
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
        let plain: Vec<Column> = columns::shown(&columns::defaults(Format::Plain)).collect();
        let copied = copy_rows(&logs, &rows, &plain).unwrap();
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
        first.run_command(command::THEMES + 1, &mut Commands::default());
        first.run_command(command::DETAILS, &mut Commands::default());
        first.run_command(command::SIDEBAR, &mut Commands::default());
        first.run_command(command::HISTOGRAM, &mut Commands::default());
        first.open_paths(vec![log.clone()]);
        drop(first);

        let mut restored = workspace();
        let (store, settings, problem) = Store::open(Some(file));
        restored.use_settings(settings, store, problem);
        assert_eq!(restored.theme().name, "Console");
        assert!(!restored.show_details);
        // The histogram starts on, so toggling it turned it off.
        assert!(restored.show_sidebar && !restored.show_histogram);
        assert_eq!(restored.settings().recent, [log.to_str().unwrap()]);
    }

    fn row_point(workspace: &Workspace, row: usize) -> Point {
        let rect = workspace.tabs[0].view.row_rect(row);
        Point::new(rect.left + 200.0, rect.top + rect.height() / 2.0)
    }

    /// Opens a row's menu by right-clicking it, and lays the menu out.
    fn right_click(workspace: &mut Workspace, row: usize) -> Vec<String> {
        let point = row_point(workspace, row);
        send(workspace, Event::MouseDown(point, MouseButton::Right, NONE));
        settle(workspace, |_| true);
        let open = workspace.row_menu.as_ref().expect("the row's menu opened");
        open.menu
            .items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Command { label, .. } => Some(label.clone()),
                MenuItem::Separator => None,
            })
            .collect()
    }

    fn choose(workspace: &mut Workspace, item: usize) {
        let rect = workspace
            .row_menu
            .as_ref()
            .unwrap()
            .menu
            .item_rect(item)
            .unwrap();
        let point = Point::new(rect.left + 10.0, rect.top + rect.height() / 2.0);
        send(workspace, Event::MouseDown(point, MouseButton::Left, NONE));
    }

    #[test]
    fn right_clicking_a_row_bookmarks_it_and_copies_the_selection() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(super::super::settings::FILE_NAME);
        let log = log_file(&dir, "app.log", 100);
        let mut workspace = workspace();
        let (store, settings, problem) = Store::open(Some(file));
        workspace.use_settings(settings, store, problem);
        workspace.open_paths(vec![log.clone()]);
        settle(&mut workspace, |workspace| {
            workspace.tabs[0].document().is_some()
        });
        // The log opens at its newest rows, so row 95 is on screen.
        assert_eq!(right_click(&mut workspace, 95), ["Add bookmark", "Copy"]);
        assert_eq!(
            workspace.tabs[0].cursor_file_row(),
            Some(95),
            "the row is selected"
        );
        choose(&mut workspace, 0);
        assert!(workspace.row_menu.is_none());
        assert!(workspace.tabs[0].rows().unwrap().bookmarked(95));
        assert_eq!(
            workspace.settings().bookmarks_for(&log),
            BTreeSet::from([95])
        );

        // Again, it offers to remove it, and Esc closes it.
        assert_eq!(right_click(&mut workspace, 95)[0], "Remove bookmark");
        send(&mut workspace, Event::KeyDown(Key::Escape, NONE));
        assert!(workspace.row_menu.is_none());
        assert!(
            workspace.tabs[0].rows().unwrap().bookmarked(95),
            "Esc chose nothing"
        );

        // Inside a selection, the selection stays, so Copy copies all of it.
        workspace.tabs[0].view.select(92, false);
        workspace.tabs[0].view.select(96, true);
        right_click(&mut workspace, 94);
        assert_eq!(workspace.tabs[0].view.selection(), Some(92..=96));
        choose(&mut workspace, 1);
        settle(&mut workspace, |workspace| {
            workspace.status().contains("Copied 5 rows")
        });

        // Shift+F10 opens it for the selected row, ready for Enter.
        send(&mut workspace, Event::KeyDown(Key::F(10), SHIFT));
        assert!(workspace.row_menu.is_some());
        assert!(!workspace.menu().is_active(), "it isn't the menu bar's F10");
        send(&mut workspace, Event::KeyDown(Key::Enter, NONE));
        assert!(workspace.tabs[0].rows().unwrap().bookmarked(96));
    }

    fn retype_theme_box(workspace: &mut Workspace, text: &str) {
        workspace.themes_panel.as_mut().unwrap().field.select_all();
        type_text(workspace, text);
    }

    #[test]
    fn editing_a_built_in_theme_saves_a_copy_that_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(super::super::settings::FILE_NAME);
        let folder = dir.path().join(themes::FOLDER);
        // A hand-edited file with a mistake isn't loaded, but isn't written over either.
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("Classic copy.toml"),
            "name = \"Classic copy\"\nsparkle = 1\n",
        )
        .unwrap();
        let mut first = workspace();
        let (store, settings, problem) = Store::open(Some(file.clone()));
        first.use_settings(settings, store, problem);
        assert!(first
            .status()
            .contains("Theme file Classic copy.toml wasn't used"));
        first.run_command(command::THEMES, &mut Commands::default());
        assert_eq!(first.theme().name, "Classic");
        first.run_command(command::EDIT_THEME, &mut Commands::default());
        let panel = first.themes_panel.as_ref().unwrap();
        assert_eq!(panel.name, "Classic copy 2");
        assert!(panel.path.is_none());

        send(&mut first, Event::KeyDown(Key::Down, NONE));
        retype_theme_box(&mut first, "#102030");
        assert_eq!(first.theme().name, "Classic copy 2");
        assert_eq!(first.theme().grid_background, Color::rgb(0x10_20_30));
        assert_ne!(
            Theme::CLASSIC.grid_background,
            Color::rgb(0x10_20_30),
            "built-ins never change"
        );
        assert!(folder.join("Classic copy 2.toml").is_file());

        // A new name renames the file, on Enter.
        first.themes_panel.as_mut().unwrap().choose_option(1);
        retype_theme_box(&mut first, "Paper");
        send(&mut first, Event::KeyDown(Key::Enter, NONE));
        assert!(folder.join("Paper.toml").is_file());
        assert!(!folder.join("Classic copy 2.toml").exists());
        assert!(folder.join("Classic copy.toml").is_file());
        send(&mut first, Event::KeyDown(Key::Escape, NONE));
        assert!(first.themes_panel.is_none());
        assert_eq!(first.file_themes.len(), 1);
        drop(first);

        let mut second = workspace();
        let (store, settings, problem) = Store::open(Some(file));
        second.use_settings(settings, store, problem);
        assert_eq!(second.theme().name, "Paper");
        assert_eq!(second.theme().grid_background, Color::rgb(0x10_20_30));
        // A file theme is edited in place.
        second.run_command(command::EDIT_THEME, &mut Commands::default());
        let panel = second.themes_panel.as_ref().unwrap();
        assert_eq!(panel.name, "Paper");
        assert_eq!(
            panel.path.as_deref(),
            Some(folder.join("Paper.toml").as_path())
        );
    }

    /// Gives the reloader time to notice anything, and the workspace time to
    /// take it in.
    fn idle(workspace: &mut Workspace) {
        for _ in 0..10 {
            settle(workspace, |_| true);
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn tools_item(workspace: &Workspace, id: u32) -> (String, bool) {
        workspace
            .build_menus()
            .iter()
            .flat_map(|menu| &menu.items)
            .find_map(|item| match item {
                MenuItem::Command {
                    label,
                    command,
                    enabled,
                    ..
                } if *command == id => Some((label.clone(), *enabled)),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn registering_for_log_files_is_asked_of_the_window_and_reported() {
        let mut workspace = workspace();
        let mut commands = Commands::default();
        workspace.run_command(command::USE_FOR_LOGS, &mut commands);
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::SetFileTypes(true))));
        assert!(
            !tools_item(&workspace, command::STOP_FOR_LOGS).1,
            "nothing to stop yet"
        );

        let here = std::env::current_exe().unwrap();
        workspace.file_types_changed(Ok(Some(here)));
        assert!(workspace
            .status()
            .contains("TraceView can open .log and .lo_ files"));
        assert_eq!(
            tools_item(&workspace, command::USE_FOR_LOGS).0,
            "Use TraceView for .log files..."
        );
        assert!(tools_item(&workspace, command::STOP_FOR_LOGS).1);

        // A portable copy that moved says it's this copy that would take over.
        workspace.file_types = Some(PathBuf::from(r"D:\Old\traceview.exe"));
        assert_eq!(
            tools_item(&workspace, command::USE_FOR_LOGS).0,
            "Use this copy for .log files..."
        );
        workspace.file_types_changed(Err("Access is denied".to_string()));
        assert!(workspace.status().contains("Couldn't change what opens"));
        workspace.file_types_changed(Ok(None));
        assert!(workspace.status().contains("no longer opens"));
        assert!(!tools_item(&workspace, command::STOP_FOR_LOGS).1);
    }

    #[test]
    fn about_shows_the_version_and_licenses() {
        let mut workspace = workspace();
        assert_eq!(
            tools_item(&workspace, command::ABOUT),
            ("About TraceView".to_string(), true)
        );
        let mut commands = Commands::default();
        workspace.run_command(command::ABOUT, &mut commands);
        let text = commands
            .iter()
            .find_map(|command| match command {
                Command::ShowMessage { title, text } if title == "About TraceView" => Some(text),
                _ => None,
            })
            .expect("About shows a message");
        assert!(text.starts_with(&format!("TraceView {}\n", env!("CARGO_PKG_VERSION"))));
        assert!(text.contains("MIT License"));
        assert!(text.contains("traceview --licenses"));
    }

    #[test]
    fn a_windows_contrast_theme_wins_while_it_is_on() {
        let night_sky = ContrastColors {
            background: Color::rgb(0x000000),
            text: Color::rgb(0xFFFFFF),
            link: Color::rgb(0x8080FF),
            disabled: Color::rgb(0xA6A6A6),
            selected: Color::rgb(0xD6B4FD),
            selected_text: Color::rgb(0x2B2B2B),
        };
        let mut workspace = workspace();
        workspace.theme_choice = ThemeChoice::Fixed(&Theme::NEON);
        workspace.set_contrast(Some(night_sky));
        assert!(workspace.theme().contrast);
        assert_eq!(workspace.theme().grid_background, Color::rgb(0x000000));
        assert!(
            !tools_item(&workspace, command::EDIT_THEME).1,
            "edits wouldn't show"
        );
        assert!(!tools_item(&workspace, command::THEME_CONTRAST).1);
        workspace.run_command(command::EDIT_THEME, &mut Commands::default());
        assert!(workspace.themes_panel.is_none());
        assert!(workspace.status().contains("can't be edited"));
        // The same colors again are the same theme, kept once.
        let first = workspace.theme();
        workspace.set_contrast(Some(night_sky));
        assert!(std::ptr::eq(first, workspace.theme()));

        workspace.set_contrast(None);
        assert_eq!(workspace.theme().name, "Neon", "the choice comes back");
        assert!(tools_item(&workspace, command::EDIT_THEME).1);
    }

    #[test]
    fn hidden_chips_cant_be_clicked_or_heard() {
        let (_dir, mut workspace) = opened(100);
        settle(&mut workspace, |workspace| {
            workspace.chip_list().count() > 0
        });
        workspace.run_command(command::CHIPS, &mut Commands::default());
        settle(&mut workspace, |_| true);
        assert_eq!(workspace.chip_list().count(), 0);
    }

    #[test]
    fn settings_edited_by_hand_load_while_running() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(super::super::settings::FILE_NAME);
        let log = log_file(&dir, "app.log", 100);
        let mut workspace = workspace();
        let (store, settings, problem) = Store::open(Some(file.clone()));
        workspace.use_settings(settings, store, problem);
        workspace.open_paths(vec![log.clone()]);
        settle(&mut workspace, |workspace| {
            workspace.tabs[0].document().is_some()
        });
        // A change made in the window is saved, and isn't read back as an edit.
        workspace.run_command(command::DETAILS, &mut Commands::default());
        settle(&mut workspace, |_| {
            std::fs::read_to_string(&file).is_ok_and(|text| text.contains("details_pane = false"))
        });
        idle(&mut workspace);
        assert!(!workspace.status().contains("reloaded"));

        // Opening the editor saves, so that's left to finish first, as it
        // would be before a person could edit the file.
        workspace.run_command(command::RULES, &mut Commands::default());
        idle(&mut workspace);
        let key = log.to_string_lossy().to_lowercase();
        let edited = format!(
            "theme = \"Neon\"\n\n[[rules]]\npattern = \"message 9\"\ncolor = \"blue\"\n\n[bookmarks]\n'{key}' = [3]\n"
        );
        std::fs::write(&file, &edited).unwrap();
        settle(&mut workspace, |workspace| workspace.theme().name == "Neon");
        assert!(workspace.rules_panel.is_none(), "the open editor closed");
        assert!(workspace.status().contains("the editor closed"));
        assert!(
            workspace.show_details,
            "left out of the file, so the default"
        );
        settle(&mut workspace, |workspace| is_marked(workspace, 95));
        assert!(workspace.tabs[0].rows().unwrap().bookmarked(3));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            edited,
            "nothing was saved over it"
        );

        // A mistake is pointed out, and nothing is saved over it.
        std::fs::write(&file, "theme = \n").unwrap();
        settle(&mut workspace, |workspace| {
            workspace.status().contains("has a mistake (line 1")
        });
        assert_eq!(workspace.theme().name, "Neon", "the settings in use stay");
        workspace.run_command(command::CHIPS, &mut Commands::default());
        idle(&mut workspace);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "theme = \n");
    }

    #[test]
    fn a_file_broken_at_launch_loads_once_fixed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(super::super::settings::FILE_NAME);
        let folder = dir.path().join(themes::FOLDER);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("Night.toml"),
            "name = \"Night\"\nbased_on = \"Dark\"\n",
        )
        .unwrap();
        std::fs::write(&file, "theme = \"Night\"\nsidebar = maybe\n").unwrap();
        let mut workspace = workspace();
        let (store, settings, problem) = Store::open(Some(file.clone()));
        workspace.use_settings(settings, store, problem);
        assert!(workspace.status().contains("Until then nothing is saved"));
        assert_eq!(workspace.file_themes.len(), 1, "theme files still load");
        std::fs::write(&file, "theme = \"Night\"\nsidebar = true\n").unwrap();
        settle(&mut workspace, |workspace| {
            workspace.theme().name == "Night"
        });
        assert!(workspace.show_sidebar);
        assert!(workspace.status().contains("Settings reloaded"));
        // Saving works again.
        workspace.run_command(command::CHIPS, &mut Commands::default());
        settle(&mut workspace, |_| {
            std::fs::read_to_string(&file).is_ok_and(|text| text.contains("chips = false"))
        });
    }

    #[test]
    fn theme_files_edited_by_hand_load_while_running() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(super::super::settings::FILE_NAME);
        let folder = dir.path().join(themes::FOLDER);
        let mut workspace = workspace();
        let (store, settings, problem) = Store::open(Some(file));
        workspace.use_settings(settings, store, problem);
        std::fs::create_dir_all(&folder).unwrap();
        let night = folder.join("Night.toml");
        std::fs::write(&night, "name = \"Night\"\nbased_on = \"Dark\"\n").unwrap();
        settle(&mut workspace, |workspace| workspace.file_themes.len() == 1);
        workspace.run_command(command::FILE_THEMES, &mut Commands::default());
        assert_eq!(workspace.theme().name, "Night");

        // Editing the theme in use recolors the window.
        std::fs::write(
            &night,
            "name = \"Night\"\nbased_on = \"Dark\"\n[colors]\ngrid_background = \"#102030\"\n",
        )
        .unwrap();
        settle(&mut workspace, |workspace| {
            workspace.theme().grid_background == Color::rgb(0x10_20_30)
        });

        // A broken one is pointed out.
        std::fs::write(folder.join("Broken.toml"), "name = 5\n").unwrap();
        settle(&mut workspace, |workspace| {
            workspace
                .status()
                .contains("Theme file Broken.toml wasn't used")
        });
        assert_eq!(workspace.file_themes.len(), 1);
    }

    fn is_marked(workspace: &Workspace, row: usize) -> bool {
        workspace.tabs[0]
            .rows()
            .and_then(|rows| rows.cells(row))
            .is_some_and(|cells| cells.mark.is_some())
    }

    #[test]
    fn highlight_rules_mark_rows_as_typed_and_are_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(super::super::settings::FILE_NAME);
        let log = log_file(&dir, "app.log", 100);
        let mut first = workspace();
        let (store, settings, problem) = Store::open(Some(file.clone()));
        first.use_settings(settings, store, problem);
        first.open_paths(vec![log.clone()]);
        settle(&mut first, |workspace| {
            workspace.tabs[0]
                .rows()
                .is_some_and(|rows| rows.cells(99).is_some())
        });
        send(&mut first, Event::KeyDown(Key::Char('H'), CTRL));
        assert!(first.rules_panel.is_some());
        // The log opens at its newest rows, so rows 90 to 99 are on screen.
        type_text(&mut first, "message 9");
        settle(&mut first, |workspace| is_marked(workspace, 95));
        assert!(!is_marked(&first, 85));
        assert_eq!(first.settings().rules.len(), 1);
        send(&mut first, Event::KeyDown(Key::Escape, NONE));
        assert!(first.rules_panel.is_none());

        // Turned off together, and on again.
        first.run_command(command::HIGHLIGHTING, &mut Commands::default());
        settle(&mut first, |workspace| !is_marked(workspace, 95));
        first.run_command(command::HIGHLIGHTING, &mut Commands::default());
        settle(&mut first, |workspace| is_marked(workspace, 95));
        drop(first);

        let mut second = workspace();
        let (store, settings, problem) = Store::open(Some(file));
        second.use_settings(settings, store, problem);
        second.open_paths(vec![log]);
        settle(&mut second, |workspace| {
            workspace
                .tabs
                .first()
                .is_some_and(|tab| tab.document().is_some())
                && is_marked(workspace, 99)
        });
    }

    fn shown(workspace: &Workspace) -> Vec<Column> {
        workspace.tabs[0].view.shown_columns().collect()
    }

    #[test]
    fn dragging_a_header_saves_the_columns_for_that_format() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(super::super::settings::FILE_NAME);
        let log = log_file(&dir, "app.log", 100);
        let mut first = workspace();
        let (store, settings, problem) = Store::open(Some(file.clone()));
        first.use_settings(settings, store, problem);
        first.open_paths(vec![log.clone()]);
        settle(&mut first, |workspace| {
            workspace.tabs[0].document().is_some()
        });
        let component = first.tabs[0].view.header_rect(Column::Component).unwrap();
        let grab = Point::new(component.left + 10.0, component.top + 5.0);
        let target = Point::new(component.left - 300.0, component.top + 5.0);
        send(&mut first, Event::MouseDown(grab, MouseButton::Left, NONE));
        send(&mut first, Event::MouseMove(target));
        send(&mut first, Event::MouseUp(target, MouseButton::Left));
        settle(&mut first, |_| true);
        let moved = [
            Column::Component,
            Column::Time,
            Column::Level,
            Column::Message,
        ];
        assert_eq!(shown(&first), moved);
        assert_eq!(first.settings().columns["plain"][0].name, "component");
        drop(first);

        let mut second = workspace();
        let (store, settings, problem) = Store::open(Some(file));
        second.use_settings(settings, store, problem);
        second.open_paths(vec![log]);
        settle(&mut second, |workspace| {
            workspace.tabs[0].document().is_some()
        });
        assert_eq!(shown(&second), moved);
    }

    #[test]
    fn right_clicking_the_header_opens_the_columns_editor() {
        let (_dir, mut workspace) = opened(10);
        let time = workspace.tabs[0].view.header_rect(Column::Time).unwrap();
        let point = Point::new(time.left + 5.0, time.top + 5.0);
        send(
            &mut workspace,
            Event::MouseDown(point, MouseButton::Right, NONE),
        );
        let panel = workspace.columns_panel.as_ref().unwrap();
        assert_eq!(panel.title(), "Columns for other logs");
        // Hide the component, the third column.
        send(&mut workspace, Event::KeyDown(Key::Down, NONE));
        send(&mut workspace, Event::KeyDown(Key::Down, NONE));
        send(&mut workspace, Event::Char(' '));
        settle(&mut workspace, |_| true);
        assert_eq!(
            shown(&workspace),
            [Column::Time, Column::Level, Column::Message]
        );
        send(&mut workspace, Event::KeyDown(Key::Escape, NONE));
        assert!(workspace.columns_panel.is_none());
    }

    fn menu_shortcut(workspace: &Workspace, id: u32) -> String {
        workspace
            .build_menus()
            .into_iter()
            .flat_map(|menu| menu.items)
            .find_map(|item| match item {
                MenuItem::Command {
                    shortcut, command, ..
                } if command == id => Some(shortcut),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn shortcuts_change_through_the_list_and_are_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(super::super::settings::FILE_NAME);
        let log = log_file(&dir, "app.log", 100);
        let mut first = workspace();
        let (store, settings, problem) = Store::open(Some(file.clone()));
        first.use_settings(settings, store, problem);
        first.open_paths(vec![log.clone()]);
        settle(&mut first, |workspace| {
            workspace.tabs[0].document().is_some()
        });

        first.run_command(command::KEYS, &mut Commands::default());
        type_text(&mut first, "next error");
        let palette = first.palette.as_ref().unwrap();
        let item = palette.palette.matched_item(0).unwrap();
        assert_eq!(
            (item.label.as_str(), item.shortcut.as_str()),
            ("Next error", "Ctrl+E")
        );
        send(&mut first, Event::KeyDown(Key::Enter, NONE));
        let panel = first.shortcut_panel.as_ref().unwrap();
        assert_eq!(panel.title(), "Shortcut for Next error");
        // Ctrl+F is Find's, so it moves, and isn't run while being chosen.
        send(&mut first, Event::KeyDown(Key::Char('F'), CTRL));
        assert_eq!(first.focus, Focus::Grid);
        assert!(first.shortcut_panel.is_none());
        assert!(
            first
                .status()
                .contains("Next error is now Ctrl+F, which Find had"),
            "{}",
            first.status()
        );
        let palette = first.palette.as_ref().unwrap();
        assert_eq!(palette.mode(), Mode::Keys);
        assert_eq!(
            palette.palette.query(),
            "next error",
            "the list comes back as it was"
        );
        send(&mut first, Event::KeyDown(Key::Escape, NONE));
        assert_eq!(menu_shortcut(&first, command::NEXT_ERROR), "Ctrl+F");
        assert_eq!(menu_shortcut(&first, command::FIND), "");
        send(&mut first, Event::KeyDown(Key::Char('F'), CTRL));
        assert_eq!(
            first.tabs[0].cursor_file_row(),
            Some(3),
            "Ctrl+F found the first error"
        );
        drop(first);

        let mut second = workspace();
        let (store, settings, problem) = Store::open(Some(file));
        second.use_settings(settings, store, problem);
        assert_eq!(menu_shortcut(&second, command::NEXT_ERROR), "Ctrl+F");
        second.run_command(command::KEYS, &mut Commands::default());
        type_text(&mut second, "reset every");
        send(&mut second, Event::KeyDown(Key::Enter, NONE));
        assert_eq!(menu_shortcut(&second, command::FIND), "Ctrl+F");
        assert!(second.settings().keys.is_empty());
    }

    #[test]
    fn every_frame_brings_screen_readers_up_to_date() {
        // Chip counts and an editor's buttons come from layout, so a frame drawn
        // after the last change must still reach screen readers.
        let (_dir, mut workspace) = opened(10);
        let mut commands = Commands::default();
        workspace.draw(&mut RecordingCanvas::new(), SIZE, 1.0, &mut commands);
        assert!(commands.tells_screen_readers());
    }

    #[test]
    fn a_format_typed_in_the_editor_reads_the_log_and_is_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(super::super::settings::FILE_NAME);
        let log = dir.path().join("service.log");
        let text: String = (0..30)
            .map(|row| {
                let level = if row % 10 == 3 { "ERROR" } else { "INFO" };
                format!("2026-01-31 10:22:{:02}|{level}|request {row}\n", row % 60)
            })
            .collect();
        std::fs::write(&log, text).unwrap();
        let mut first = workspace();
        let (store, settings, problem) = Store::open(Some(file.clone()));
        first.use_settings(settings, store, problem);
        first.open_paths(vec![log.clone()]);
        settle(&mut first, |workspace| {
            workspace.tabs[0].document().is_some()
        });
        assert_eq!(first.tabs[0].kind().unwrap().format, Format::Plain);

        first.run_command(command::FORMATS, &mut Commands::default());
        type_text(
            &mut first,
            r"^(?P<time>[^|]+)\|(?P<level>[^|]+)\|(?P<message>.*)$",
        );
        // The log's first lines are read in the background, and tried once they arrive.
        settle(&mut first, |workspace| {
            workspace.formats_panel.as_ref().is_some_and(|panel| {
                panel
                    .test()
                    .first()
                    .is_some_and(|line| line.starts_with("Matches"))
            })
        });
        let panel = first.formats_panel.as_ref().unwrap();
        assert_eq!(
            panel.test()[0],
            "Matches 20 of the first 20 lines of service.log"
        );
        assert_eq!(first.settings().formats.len(), 1, "saved as it's typed");

        // "Use for this file" names the log in the format and reads it again.
        let response = first.formats_panel.as_mut().unwrap().choose_option(5);
        first.formats_responded(&response, &mut Commands::default());
        settle(&mut first, |workspace| {
            workspace.tabs[0].document().is_some()
        });
        let kind = first.tabs[0].kind().unwrap();
        assert_eq!(
            (kind.format, kind.name.as_str()),
            (Format::Custom, "Format 1")
        );
        assert!(first.status().contains("Format 1"), "{}", first.status());
        assert_eq!(
            shown(&first),
            [Column::Time, Column::Level, Column::Message],
            "a custom format shows the parts it names"
        );
        assert_eq!(first.tabs[0].rows().unwrap().level(3), Level::Error);
        drop(first);

        let mut second = workspace();
        let (store, settings, problem) = Store::open(Some(file));
        second.use_settings(settings, store, problem);
        assert_eq!(second.settings().formats[0].files, "service.log");
        second.open_paths(vec![log]);
        settle(&mut second, |workspace| {
            workspace.tabs[0].document().is_some()
        });
        assert_eq!(second.tabs[0].kind().unwrap().format, Format::Custom);
    }

    #[test]
    fn a_shortcut_can_be_removed_or_given_back_its_default() {
        let (_dir, mut workspace) = opened(10);
        for (key, expected) in [(Key::Backspace, ""), (Key::Delete, "Ctrl+K")] {
            workspace.run_command(command::KEYS, &mut Commands::default());
            type_text(&mut workspace, "command palette");
            send(&mut workspace, Event::KeyDown(Key::Enter, NONE));
            send(&mut workspace, Event::KeyDown(key, NONE));
            assert_eq!(menu_shortcut(&workspace, command::PALETTE), expected);
            send(&mut workspace, Event::KeyDown(Key::Escape, NONE));
        }
    }

    #[test]
    fn ctrl_plus_minus_zero_and_the_wheel_size_log_text() {
        let (_dir, mut workspace) = opened(10);
        let size = |workspace: &Workspace| workspace.fonts().grid_size;
        send(&mut workspace, Event::KeyDown(Key::Char('='), CTRL));
        send(&mut workspace, Event::KeyDown(Key::Char('='), CTRL_SHIFT));
        assert_eq!(size(&workspace), 15.0);
        assert!(workspace.status().contains("Log text is 15 pixels"));
        send(&mut workspace, Event::KeyDown(Key::Char('-'), CTRL));
        send(&mut workspace, Event::ZoomWheel(-3.0));
        assert_eq!(size(&workspace), 13.0);
        send(&mut workspace, Event::ZoomWheel(3.0));
        send(&mut workspace, Event::KeyDown(Key::Char('0'), CTRL));
        assert_eq!(size(&workspace), 13.0);
        for _ in 0..40 {
            send(&mut workspace, Event::KeyDown(Key::Char('-'), CTRL));
        }
        assert_eq!(size(&workspace), *LOG_SIZES.start());
    }

    #[test]
    fn row_spacing_sets_the_row_height_from_the_font() {
        let (_dir, mut workspace) = opened(10);
        let height = |workspace: &Workspace| workspace.tabs[0].view.row_rect(0).height();
        // The test canvas's lines are 15 high.
        assert_eq!(height(&workspace), 18.0);
        workspace.run_command(command::FONTS, &mut Commands::default());
        send(&mut workspace, Event::KeyDown(Key::Down, NONE));
        send(&mut workspace, Event::Char('+'));
        settle(&mut workspace, |_| true);
        assert_eq!(workspace.settings().fonts.row_spacing, "roomy");
        assert_eq!(height(&workspace), 23.0);
    }

    #[test]
    fn choosing_a_font_goes_through_the_list_and_back() {
        let (_dir, mut workspace) = opened(10);
        workspace.run_command(command::FONTS, &mut Commands::default());
        send(&mut workspace, Event::KeyDown(Key::Enter, NONE));
        assert!(workspace.fonts_panel.is_none());
        let palette = workspace.palette.as_ref().unwrap();
        assert_eq!(palette.mode(), Mode::Font(FontTarget::Log));
        let first = palette.palette.matched_item(0).unwrap();
        assert_eq!(
            (first.label.as_str(), first.detail.as_str()),
            ("Consolas", "in use, fixed width")
        );
        // Leaving the list returns to the editor, with nothing changed.
        send(&mut workspace, Event::KeyDown(Key::Escape, NONE));
        assert!(workspace.fonts_panel.is_some());
        send(&mut workspace, Event::KeyDown(Key::Enter, NONE));
        type_text(&mut workspace, "Courier New");
        send(&mut workspace, Event::KeyDown(Key::Enter, NONE));
        assert_eq!(workspace.settings().fonts.log_font, "Courier New");
        assert_eq!(workspace.fonts().grid_family, "Courier New");
        assert!(workspace.fonts_panel.is_some(), "back in the editor");
    }

    #[test]
    fn the_rules_editor_turns_rules_back_on_and_takes_all_input() {
        let (_dir, mut workspace) = opened(10);
        workspace.run_command(command::HIGHLIGHTING, &mut Commands::default());
        assert!(!workspace.settings().highlighting);
        send(&mut workspace, Event::KeyDown(Key::Char('H'), CTRL));
        assert!(workspace.settings().highlighting);
        // Ctrl+F would focus the find box, but the editor has the keyboard.
        send(&mut workspace, Event::KeyDown(Key::Char('F'), CTRL));
        assert_eq!(workspace.focus, Focus::Grid);
        assert!(workspace.rules_panel.is_some());
        send(&mut workspace, Event::KeyDown(Key::Char('K'), CTRL));
        assert!(workspace.palette.is_none());
    }

    #[test]
    fn bookmarks_toggle_step_and_are_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(super::super::settings::FILE_NAME);
        let log = log_file(&dir, "app.log", 100);
        let mut first = workspace();
        let (store, settings, problem) = Store::open(Some(file.clone()));
        first.use_settings(settings, store, problem);
        first.open_paths(vec![log.clone()]);
        settle(&mut first, |workspace| {
            workspace.tabs[0].document().is_some()
        });
        for row in [40, 10] {
            first.tabs[0].select_file_row(row);
            send(&mut first, Event::KeyDown(Key::Char('D'), CTRL));
        }
        let rows = first.tabs[0].rows().unwrap();
        assert!(rows.bookmarked(10) && rows.bookmarked(40) && !rows.bookmarked(11));
        drop(rows);
        send(&mut first, Event::KeyDown(Key::F(2), NONE));
        assert_eq!(first.tabs[0].cursor_file_row(), Some(40));
        send(&mut first, Event::KeyDown(Key::F(2), NONE));
        assert_eq!(first.tabs[0].cursor_file_row(), Some(10), "F2 wraps around");
        send(&mut first, Event::KeyDown(Key::F(2), SHIFT));
        assert_eq!(first.tabs[0].cursor_file_row(), Some(40));
        // Ctrl+D again removes a bookmark.
        send(&mut first, Event::KeyDown(Key::Char('D'), CTRL));
        drop(first);

        let mut second = workspace();
        let (store, settings, problem) = Store::open(Some(file));
        second.use_settings(settings, store, problem);
        second.open_paths(vec![log]);
        settle(&mut second, |workspace| {
            workspace.tabs[0].document().is_some()
        });
        let rows = second.tabs[0].rows().unwrap();
        assert!(rows.bookmarked(10) && !rows.bookmarked(40));
    }

    #[test]
    fn a_merged_tab_shares_its_files_bookmarks() {
        let dir = tempfile::tempdir().unwrap();
        let mut workspace = merged(&dir);
        // Row 3 of the merged timeline is b.log's second row.
        workspace.tabs[2].select_file_row(3);
        send(&mut workspace, Event::KeyDown(Key::Char('D'), CTRL));
        assert!(
            workspace.tabs[1].rows().unwrap().bookmarked(1),
            "b.log's own tab shows it"
        );
        assert!(workspace.tabs[2].rows().unwrap().bookmarked(3));
        assert!(!workspace.tabs[0].rows().unwrap().bookmarked(1));
    }

    #[test]
    fn export_writes_the_filtered_rows_and_reports_it() {
        let (dir, mut workspace) = opened(100);
        let mut commands = Commands::default();
        workspace.run_command(command::EXPORT, &mut commands);
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::SaveFileDialog { suggested_name } if suggested_name == "app.csv"
        )));
        send(&mut workspace, Event::KeyDown(Key::Char('F'), CTRL_SHIFT));
        type_text(&mut workspace, "level:error");
        send(&mut workspace, Event::KeyDown(Key::Enter, NONE));
        settle(&mut workspace, |workspace| workspace.tabs[0].is_filtered());

        let path = dir.path().join("errors.csv");
        workspace.export_to(path.clone());
        settle(&mut workspace, |workspace| {
            workspace.status().contains("Exported")
        });
        assert!(workspace
            .status()
            .contains("Exported 10 rows to errors.csv"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 11);
        assert!(text
            .lines()
            .nth(1)
            .unwrap()
            .starts_with("4,2026-01-01 10:00:03.000,Error,"));
        assert!(!dir.path().join("errors.csv.part").exists());
    }

    fn sidebar_labels(workspace: &Workspace) -> Vec<String> {
        workspace
            .sidebar
            .list
            .items
            .iter()
            .map(|item| match item {
                tv_ui::list_view::ListItem::Heading(text) => text.clone(),
                tv_ui::list_view::ListItem::Entry { label, .. } => label.trim().to_string(),
            })
            .collect()
    }

    fn click_sidebar(workspace: &mut Workspace, label: &str) {
        let index = sidebar_labels(workspace)
            .iter()
            .position(|item| item == label)
            .unwrap();
        workspace.sidebar.list.reveal(index);
        workspace.draw(
            &mut RecordingCanvas::new(),
            SIZE,
            1.0,
            &mut Commands::default(),
        );
        let rect = workspace.sidebar.list.item_rect(index).unwrap();
        let point = Point::new(rect.left + 10.0, rect.top + rect.height() / 2.0);
        send(workspace, Event::MouseDown(point, MouseButton::Left, NONE));
        send(workspace, Event::MouseUp(point, MouseButton::Left));
    }

    #[test]
    fn ctrl_b_shows_open_nearby_and_recent_logs_in_the_sidebar() {
        let dir = tempfile::tempdir().unwrap();
        let first = log_file(&dir, "app.log", 10);
        log_file(&dir, "other.log", 5);
        let mut workspace = workspace();
        workspace.open_paths(vec![first]);
        settle(&mut workspace, |workspace| {
            workspace.tabs[0].document().is_some()
        });
        send(&mut workspace, Event::KeyDown(Key::Char('B'), CTRL));
        assert!(workspace.show_sidebar);
        settle(&mut workspace, |workspace| {
            sidebar_labels(workspace).contains(&"other.log".to_string())
        });
        let labels = sidebar_labels(&workspace);
        for heading in ["OPEN", "NEARBY", "RECENT"] {
            assert!(labels.contains(&heading.to_string()), "{labels:?}");
        }
        assert!(workspace.grid_rect().left >= sidebar::DEFAULT_WIDTH);

        click_sidebar(&mut workspace, "other.log");
        assert_eq!(workspace.tab_titles(), ["app.log", "other.log"]);
        assert_eq!(workspace.active, 1);
        assert_eq!(
            workspace.focus,
            Focus::Grid,
            "an opened log is ready to read"
        );

        // Choosing an open tab keeps the sidebar focused, and Escape leaves it.
        click_sidebar(&mut workspace, "app.log");
        assert_eq!((workspace.active, workspace.focus), (0, Focus::Sidebar));
        send(&mut workspace, Event::KeyDown(Key::Escape, NONE));
        assert_eq!(workspace.focus, Focus::Grid);
    }

    #[test]
    fn sidebar_bookmarks_jump_to_their_row() {
        let (_dir, mut workspace) = opened(100);
        workspace.tabs[0].select_file_row(41);
        send(&mut workspace, Event::KeyDown(Key::Char('D'), CTRL));
        workspace.tabs[0].select_file_row(0);
        send(&mut workspace, Event::KeyDown(Key::Char('B'), CTRL));
        settle(&mut workspace, |workspace| {
            sidebar_labels(workspace).contains(&"BOOKMARKS".to_string())
        });
        let bookmark = sidebar_labels(&workspace)
            .into_iter()
            .find(|label| label == "Row 42" || label.starts_with("42: "))
            .expect("the bookmark is listed by its row and message");
        click_sidebar(&mut workspace, &bookmark);
        assert_eq!(workspace.tabs[0].cursor_file_row(), Some(41));
        assert_eq!(workspace.focus, Focus::Grid);
    }

    #[test]
    fn dragging_the_sidebar_edge_resizes_it() {
        let (_dir, mut workspace) = opened(10);
        send(&mut workspace, Event::KeyDown(Key::Char('B'), CTRL));
        workspace.draw(
            &mut RecordingCanvas::new(),
            SIZE,
            1.0,
            &mut Commands::default(),
        );
        let edge = workspace.sidebar.width();
        let start = Point::new(edge + 1.0, 300.0);
        let end = Point::new(edge + 101.0, 300.0);
        let mut commands = Commands::default();
        workspace.event(
            Event::MouseMove(start),
            &mut RecordingCanvas::new(),
            &mut commands,
        );
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::SetCursor(Cursor::ResizeColumns))));
        send(
            &mut workspace,
            Event::MouseDown(start, MouseButton::Left, NONE),
        );
        send(&mut workspace, Event::MouseMove(end));
        send(&mut workspace, Event::MouseUp(end, MouseButton::Left));
        assert!(
            (workspace.sidebar.width() - (edge + 100.0)).abs() < 1.0,
            "{}",
            workspace.sidebar.width()
        );
        assert_eq!(
            workspace.tabs[0].view.cursor(),
            None,
            "the drag didn't reach the grid"
        );
    }

    fn chip_summary(workspace: &Workspace) -> Vec<(String, usize, bool)> {
        workspace
            .chip_list()
            .map(|(chip, _)| {
                (
                    chip.label.clone(),
                    chip.count.unwrap_or_default(),
                    chip.active,
                )
            })
            .collect()
    }

    fn click_chip(workspace: &mut Workspace, label: &str) {
        let rect = workspace
            .chip_list()
            .find(|(chip, _)| chip.label == label)
            .and_then(|(_, rect)| rect)
            .unwrap();
        let point = Point::new(rect.left + 4.0, rect.top + rect.height() / 2.0);
        send(workspace, Event::MouseDown(point, MouseButton::Left, NONE));
        send(workspace, Event::MouseUp(point, MouseButton::Left));
    }

    #[test]
    fn chips_count_each_level_and_filter_when_clicked() {
        let (_dir, mut workspace) = opened(100);
        settle(&mut workspace, |workspace| {
            workspace.chip_list().count() > 1
        });
        let all = |active| ("All".to_string(), 100, active);
        assert_eq!(
            chip_summary(&workspace),
            [
                all(true),
                ("Error".to_string(), 10, false),
                ("Warning".to_string(), 10, false),
                ("Info".to_string(), 80, false)
            ]
        );

        click_chip(&mut workspace, "Error");
        settle(&mut workspace, |workspace| workspace.tabs[0].is_filtered());
        assert_eq!(row_count(&workspace), 10);
        assert!(chip_summary(&workspace)[1].2, "the Error chip is on");
        // Clicking it again shows every level.
        click_chip(&mut workspace, "Error");
        settle(&mut workspace, |workspace| !workspace.tabs[0].is_filtered());

        // A chip and the filter box work together.
        click_chip(&mut workspace, "Error");
        send(&mut workspace, Event::KeyDown(Key::Char('F'), CTRL_SHIFT));
        type_text(&mut workspace, "message 1");
        send(&mut workspace, Event::KeyDown(Key::Enter, NONE));
        settle(&mut workspace, |workspace| row_count(workspace) == 1);
        click_chip(&mut workspace, "All");
        settle(&mut workspace, |workspace| row_count(workspace) == 11);
    }

    #[test]
    fn clicking_the_histogram_selects_the_worst_row_there() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hour.log");
        let text: String = (0..60)
            .map(|minute| {
                let level = if minute == 30 { "ERROR" } else { "INFO" };
                format!("2026-01-01 10:{minute:02}:00 host app[1]: {level}: minute {minute}\n")
            })
            .collect();
        std::fs::write(&path, text).unwrap();
        let mut workspace = workspace();
        workspace.show_histogram = true;
        workspace.open_paths(vec![path]);
        settle(&mut workspace, |workspace| {
            workspace.histogram_rect().height() > 0.0
        });
        let histogram = workspace.tabs[0].overview.counts.histogram.clone().unwrap();
        let bucket = histogram
            .buckets
            .iter()
            .position(|bucket| bucket.get(Level::Error) > 0)
            .unwrap();
        let area = workspace.histogram_rect();
        let width = area.width() / histogram.buckets.len() as f32;
        let point = Point::new(area.left + (bucket as f32 + 0.5) * width, area.top + 10.0);
        send(
            &mut workspace,
            Event::MouseDown(point, MouseButton::Left, NONE),
        );
        assert_eq!(workspace.tabs[0].cursor_file_row(), Some(30));
        assert!(!workspace.tabs[0].following);
    }

    #[test]
    fn ctrl_k_finds_and_runs_commands() {
        let (_dir, mut workspace) = opened(100);
        workspace.tabs[0].select_file_row(5);
        send(&mut workspace, Event::KeyDown(Key::Char('K'), CTRL));
        assert!(workspace.palette.is_some());
        type_text(&mut workspace, "toggle book");
        send(&mut workspace, Event::KeyDown(Key::Enter, NONE));
        assert!(workspace.palette.is_none());
        assert!(workspace.tabs[0].rows().unwrap().bookmarked(5));
    }

    #[test]
    fn ctrl_g_goes_to_a_row() {
        let (_dir, mut workspace) = opened(100);
        send(&mut workspace, Event::KeyDown(Key::Char('G'), CTRL));
        type_text(&mut workspace, "42");
        send(&mut workspace, Event::KeyDown(Key::Enter, NONE));
        assert_eq!(workspace.tabs[0].cursor_file_row(), Some(41));
        assert_eq!(workspace.focus, Focus::Grid);
    }

    #[test]
    fn filters_save_as_presets_and_come_back() {
        let (_dir, mut workspace) = opened(100);
        workspace.run_command(command::SAVE_PRESET, &mut Commands::default());
        assert!(workspace.palette.is_none());
        assert!(workspace.status().contains("Set a filter"));

        send(&mut workspace, Event::KeyDown(Key::Char('F'), CTRL_SHIFT));
        type_text(&mut workspace, "message 1");
        send(&mut workspace, Event::KeyDown(Key::Enter, NONE));
        workspace.tabs[0].set_chip(Some(Level::Error), &Waker::detached());
        settle(&mut workspace, |workspace| row_count(workspace) == 1);
        workspace.run_command(command::SAVE_PRESET, &mut Commands::default());
        type_text(&mut workspace, "Early errors");
        send(&mut workspace, Event::KeyDown(Key::Enter, NONE));
        let saved = &workspace.settings().presets[0];
        assert_eq!(
            (
                saved.name.as_str(),
                saved.filter.as_str(),
                saved.level.as_str()
            ),
            ("Early errors", "message 1", "error")
        );

        workspace.run_command(command::CLEAR_FILTER, &mut Commands::default());
        workspace.tabs[0].set_chip(None, &Waker::detached());
        settle(&mut workspace, |workspace| row_count(workspace) == 100);
        send(&mut workspace, Event::KeyDown(Key::Char('K'), CTRL));
        type_text(&mut workspace, "preset early");
        send(&mut workspace, Event::KeyDown(Key::Enter, NONE));
        settle(&mut workspace, |workspace| row_count(workspace) == 1);
        assert_eq!(workspace.tabs[0].chip, Some(Level::Error));
    }

    /// Sidebar, chips, histogram and details pane.
    fn panes(workspace: &Workspace) -> (bool, bool, bool, bool) {
        (
            workspace.show_sidebar,
            workspace.show_chips,
            workspace.show_histogram,
            workspace.show_details,
        )
    }

    fn layout_checked(workspace: &Workspace, name: &str) -> bool {
        let label = format!("Layout: {name}");
        workspace
            .build_menus()
            .iter()
            .flat_map(|menu| &menu.items)
            .any(|item| matches!(item, MenuItem::Command { label: shown, checked: true, .. } if *shown == label))
    }

    #[test]
    fn layouts_switch_panes_and_saved_ones_come_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(super::super::settings::FILE_NAME);
        let mut first = workspace();
        let (store, settings, problem) = Store::open(Some(file.clone()));
        first.use_settings(settings, store, problem);
        first.run_command(command::LAYOUTS + 1, &mut Commands::default());
        assert_eq!(panes(&first), (true, true, true, true));
        assert!(layout_checked(&first, "Everything"));
        first.run_command(command::LAYOUTS, &mut Commands::default());
        assert_eq!(panes(&first), (false, false, false, true));
        assert!(layout_checked(&first, "CMTrace"));
        assert!(!layout_checked(&first, "Everything"));

        // The sidebar, wider, and the histogram, saved as Wide.
        first.run_command(command::SIDEBAR, &mut Commands::default());
        first.sidebar.set_width(400.0);
        first.run_command(command::HISTOGRAM, &mut Commands::default());
        first.perform(
            Action::SaveLayout("cmtrace".to_string()),
            &mut Commands::default(),
        );
        assert!(
            first.settings().layouts.is_empty(),
            "built-in names aren't taken"
        );
        first.run_command(command::SAVE_LAYOUT, &mut Commands::default());
        type_text(&mut first, "Wide");
        send(&mut first, Event::KeyDown(Key::Enter, NONE));
        assert_eq!(first.settings().layouts.len(), 1);
        assert!(first.status().contains("Saved layout Wide"));
        assert!(layout_checked(&first, "Wide"));
        first.run_command(command::LAYOUTS, &mut Commands::default());
        assert!(!layout_checked(&first, "Wide"));
        drop(first);

        let mut second = workspace();
        let (store, settings, problem) = Store::open(Some(file));
        second.use_settings(settings, store, problem);
        assert_eq!(
            panes(&second),
            (false, false, false, true),
            "the panes in use come back"
        );
        assert_eq!(
            second.sidebar.chosen_width(),
            400.0,
            "a sidebar that was never drawn keeps its width"
        );
        // Saved layouts are in the palette, as every menu command is.
        send(&mut second, Event::KeyDown(Key::Char('K'), CTRL));
        type_text(&mut second, "layout: wide");
        send(&mut second, Event::KeyDown(Key::Enter, NONE));
        assert_eq!(panes(&second), (true, false, true, true));
        assert_eq!(second.sidebar.chosen_width(), 400.0);
        second.perform(Action::DeleteLayout(0), &mut Commands::default());
        assert!(second.settings().layouts.is_empty());
        assert!(second.status().contains("Deleted layout Wide"));
    }

    fn two_hour_logs() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let logs: Vec<PathBuf> = ["a", "b"]
            .iter()
            .map(|name| {
                let rows: Vec<(u32, String)> = (0..60)
                    .map(|minute| (minute, format!("{name}{minute}")))
                    .collect();
                let path = dir.path().join(format!("{name}.log"));
                let text: String = rows
                    .iter()
                    .map(|(minute, message)| {
                        format!("2026-01-01 10:{minute:02}:00 host app[1]: INFO: {message}\n")
                    })
                    .collect();
                std::fs::write(&path, text).unwrap();
                path
            })
            .collect();
        let mut workspace = workspace();
        workspace.open_paths(logs);
        settle(&mut workspace, |workspace| {
            workspace.tabs.iter().all(|tab| tab.document().is_some())
        });
        for tab in &mut workspace.tabs {
            tab.following = false;
        }
        (dir, workspace)
    }

    #[test]
    fn ctrl_backslash_splits_the_view_and_a_click_moves_focus() {
        let (_dir, mut workspace) = two_hour_logs();
        assert_eq!(workspace.active, 1);
        send(&mut workspace, Event::KeyDown(Key::Char('\\'), CTRL));
        settle(&mut workspace, |workspace| workspace.layout.other.is_some());
        let other = workspace.layout.other.unwrap();
        assert!(
            workspace.grid_rect().right <= other.grid.left,
            "the focused pane is on the left"
        );

        // Clicking a row in the right pane focuses it and selects the row.
        let row = workspace.tabs[0].view.row_rect(3);
        let point = Point::new(other.grid.left + 40.0, row.top + row.height() / 2.0);
        send(
            &mut workspace,
            Event::MouseDown(point, MouseButton::Left, NONE),
        );
        send(&mut workspace, Event::MouseUp(point, MouseButton::Left));
        assert_eq!(workspace.active, 0);
        assert!(workspace.split.as_ref().unwrap().focus_right);
        assert_eq!(workspace.tabs[0].cursor_file_row(), Some(3));

        send(&mut workspace, Event::KeyDown(Key::Char('W'), CTRL));
        assert!(workspace.tabs.len() == 1);
        workspace.draw(
            &mut RecordingCanvas::new(),
            SIZE,
            1.0,
            &mut Commands::default(),
        );
        assert!(workspace.split.is_none(), "one log can't be split");
    }

    #[test]
    fn synced_panes_line_up_by_time() {
        let (_dir, mut workspace) = two_hour_logs();
        send(&mut workspace, Event::KeyDown(Key::Char('\\'), CTRL));
        send(&mut workspace, Event::KeyDown(Key::Char('S'), CTRL_SHIFT));
        assert!(workspace.split.as_ref().unwrap().sync);
        workspace.tabs[1].view.scroll_to(30.0);
        workspace.draw(
            &mut RecordingCanvas::new(),
            SIZE,
            1.0,
            &mut Commands::default(),
        );
        assert_eq!(workspace.tabs[0].view.top_row(), 30);
        workspace.tabs[1].view.scroll_to(12.0);
        workspace.draw(
            &mut RecordingCanvas::new(),
            SIZE,
            1.0,
            &mut Commands::default(),
        );
        assert_eq!(workspace.tabs[0].view.top_row(), 12);
    }

    fn split_with_one_log(workspace: &mut Workspace) -> Commands {
        let mut commands = Commands::default();
        workspace.event(
            Event::KeyDown(Key::Char('\\'), CTRL),
            &mut RecordingCanvas::new(),
            &mut commands,
        );
        commands
    }

    #[test]
    fn splitting_one_log_asks_for_the_log_to_show_beside_it() {
        let (dir, mut workspace) = opened(10);
        let commands = split_with_one_log(&mut workspace);
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::OpenFileDialog)));
        assert!(
            workspace.split.is_none(),
            "nothing changes until one is chosen"
        );

        let second = log_file(&dir, "second.log", 10);
        workspace.open_paths(vec![second.clone()]);
        let split = workspace.split.as_ref().unwrap();
        assert_eq!((split.other, split.focus_right), (0, true));
        assert_eq!(workspace.active, 1);
        assert_eq!(workspace.active_tab().unwrap().paths, [second]);
    }

    #[test]
    fn cancelling_the_split_dialog_changes_nothing() {
        let (dir, mut workspace) = opened(10);
        split_with_one_log(&mut workspace);
        workspace.open_paths(Vec::new());
        assert!(workspace.split.is_none());
        // A log opened later opens in a tab of its own, as always.
        workspace.open_paths(vec![log_file(&dir, "later.log", 10)]);
        assert!(workspace.split.is_none());
        assert_eq!(workspace.tabs.len(), 2);
    }

    #[test]
    fn splitting_with_the_same_log_says_to_choose_another() {
        let (_dir, mut workspace) = opened(10);
        split_with_one_log(&mut workspace);
        let same = workspace.tabs[0].paths.clone();
        workspace.open_paths(same);
        assert!(workspace.split.is_none());
        assert!(workspace.status().contains("choose another"));
    }

    #[test]
    fn split_view_is_off_with_no_log_open() {
        let mut workspace = workspace();
        let split = workspace
            .build_menus()
            .iter()
            .flat_map(|menu| &menu.items)
            .find_map(|item| match item {
                MenuItem::Command {
                    command, enabled, ..
                } if *command == command::SPLIT => Some(*enabled),
                _ => None,
            });
        assert_eq!(split, Some(false));
        send(&mut workspace, Event::KeyDown(Key::Char('\\'), CTRL));
        assert!(workspace.status().contains("Open a log, then split"));
    }

    #[test]
    fn stepping_with_no_bookmarks_says_how_to_add_one() {
        let (_dir, mut workspace) = opened(10);
        send(&mut workspace, Event::KeyDown(Key::F(2), NONE));
        assert!(workspace.status().contains("Ctrl+D bookmarks a row"));
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
