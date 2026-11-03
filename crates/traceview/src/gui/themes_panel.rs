//! The theme editor, opened by View > Edit theme. It lists every color of
//! the theme in use, and the box takes a new one as `#RRGGBB`, which the whole
//! window shows at once. A built-in theme is never changed: the first change
//! makes a copy, saved as a file in the themes folder, which becomes the
//! theme in use. Contrast below what the built-in themes meet is pointed out.

use super::themes::{color_text, name_taken, parse_color};
use std::path::PathBuf;
use tv_ui::chip_bar::{self, Chip, ChipBar};
use tv_ui::list_view::{ListItem, ListView};
use tv_ui::text_field::TextField;
use tv_ui::theme::{token_label, COLOR_TOKENS, RULE_COLOR_NAMES};
use tv_ui::{
    Align, Canvas, Color, Event, FontRole, Key, MouseButton, Point, Rect, TextStyle, Theme,
};

const WIDTH: f32 = 620.0;
const HEIGHT: f32 = 460.0;
/// How far below the top of its area the panel sits.
const OFFSET: f32 = 24.0;
const PADDING: f32 = 14.0;
const FIELD_HEIGHT: f32 = 26.0;
const LINE_HEIGHT: f32 = 22.0;
const GAP: f32 = 6.0;
const LABEL: TextStyle = TextStyle::new(FontRole::Ui, Align::Left);
const HINT: &str = "Tab moves between parts     Up and Down choose a color     Esc closes";
const RULE_PARTS: [&str; 3] = ["row", "text", "marker"];

/// The buttons under the box, in order.
pub const OPTIONS: [&str; 4] = ["Color", "Name", "Dark", "Open folder"];
const COLOR: usize = 0;
const NAME: usize = 1;
const DARK: usize = 2;
const OPEN_FOLDER: usize = 3;

/// The part of the panel the keyboard is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Field,
    Options,
    List,
}

impl Part {
    const ORDER: [Part; 3] = [Part::Field, Part::Options, Part::List];

    fn step(self, backward: bool) -> Self {
        let index = Self::ORDER
            .iter()
            .position(|part| *part == self)
            .unwrap_or(0);
        let next = if backward {
            index + Self::ORDER.len() - 1
        } else {
            index + 1
        };
        Self::ORDER[next % Self::ORDER.len()]
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThemesResponse {
    pub redraw: bool,
    /// The theme changed, so it should be used and saved.
    pub changed: bool,
    pub closed: bool,
    /// The owner should show the themes folder.
    pub open_folder: bool,
    pub copy: Option<String>,
    pub wants_paste: bool,
}

impl ThemesResponse {
    fn redraw(changed: bool) -> Self {
        Self {
            redraw: true,
            changed,
            ..Self::default()
        }
    }
}

/// How many colors a theme has: its named ones, then three per rule color.
fn color_count() -> usize {
    COLOR_TOKENS.len() + RULE_COLOR_NAMES.len() * RULE_PARTS.len()
}

/// What a color in the list is called.
fn color_label(index: usize) -> String {
    match COLOR_TOKENS.get(index) {
        Some(token) => token_label(token),
        None => {
            let rule = index - COLOR_TOKENS.len();
            format!(
                "{} rule {}",
                RULE_COLOR_NAMES[rule / RULE_PARTS.len()],
                RULE_PARTS[rule % RULE_PARTS.len()]
            )
        }
    }
}

fn color_at(theme: &Theme, index: usize) -> Color {
    match COLOR_TOKENS.get(index) {
        Some(token) => theme.color(token).unwrap_or(theme.primary_text),
        None => {
            let rule = index - COLOR_TOKENS.len();
            let colors = theme.rule_colors[rule / RULE_PARTS.len()];
            match rule % RULE_PARTS.len() {
                0 => colors.row,
                1 => colors.text,
                _ => colors.marker,
            }
        }
    }
}

fn set_color_at(theme: &mut Theme, index: usize, color: Color) {
    match COLOR_TOKENS.get(index) {
        Some(token) => {
            theme.set_color(token, color);
        }
        None => {
            let rule = index - COLOR_TOKENS.len();
            let colors = &mut theme.rule_colors[rule / RULE_PARTS.len()];
            match rule % RULE_PARTS.len() {
                0 => colors.row = color,
                1 => colors.text = color,
                _ => colors.marker = color,
            }
        }
    }
}

pub struct ThemesPanel {
    pub field: TextField,
    /// The theme's name, which is also its file's.
    pub name: String,
    pub theme: Theme,
    /// Its file, or `None` while it's still a built-in theme that hasn't been copied.
    pub path: Option<PathBuf>,
    /// Names other themes and theme files have, which this one can't take.
    taken: Vec<String>,
    editing_name: bool,
    selected: usize,
    pub focus: Part,
    pub list: ListView,
    pub options: ChipBar,
    option_chips: Vec<Chip>,
    /// What's wrong with the box's text, if anything.
    problem: Option<String>,
    rect: Rect,
    field_rect: Rect,
    options_rect: Rect,
    note_rect: Rect,
    hint_rect: Rect,
}

impl ThemesPanel {
    /// Opens the editor on a theme. A built-in one comes with no file, and
    /// `name` is what its copy will be called.
    pub fn new(name: &str, theme: Theme, path: Option<PathBuf>, taken: Vec<String>) -> Self {
        let mut panel = Self {
            field: TextField::default(),
            name: name.to_string(),
            theme,
            path,
            taken,
            editing_name: false,
            selected: 0,
            focus: Part::Field,
            list: ListView::default(),
            options: ChipBar::default(),
            option_chips: Vec::new(),
            problem: None,
            rect: Rect::default(),
            field_rect: Rect::default(),
            options_rect: Rect::default(),
            note_rect: Rect::default(),
            hint_rect: Rect::default(),
        };
        panel.fill_list();
        panel.load();
        panel
    }

    pub fn title(&self) -> String {
        match &self.path {
            Some(_) => format!("Edit theme: {}", self.name),
            None => format!("Edit theme: a change makes a copy called {}", self.name),
        }
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn rect(&self) -> Rect {
        self.rect
    }

    pub fn field_rect(&self) -> Rect {
        self.field_rect
    }

    pub fn option_chips(&self) -> &[Chip] {
        &self.option_chips
    }

    /// What the box holds, for screen readers.
    pub fn field_label(&self) -> String {
        if self.editing_name {
            "Theme name".to_string()
        } else {
            color_label(self.selected)
        }
    }

    /// What to point out: a mistake in the box, or else the theme's weakest
    /// contrast, if any falls short.
    pub fn note(&self) -> Option<String> {
        if let Some(problem) = &self.problem {
            return Some(problem.clone());
        }
        let problems = self.theme.contrast_problems();
        let first = problems.first()?;
        Some(match problems.len() {
            1 => format!("Low contrast: {first}"),
            count => format!("Low contrast: {first}, and {} more", count - 1),
        })
    }

    fn fill_list(&mut self) {
        self.list.items = (0..color_count())
            .map(|index| {
                let color = color_at(&self.theme, index);
                ListItem::entry(&color_label(index), &color_text(color), index as u32)
                    .with_swatch(color)
            })
            .collect();
        self.list.select_id(Some(self.selected as u32));
    }

    /// Puts the selected color, or the name, in the box, with fresh undo.
    fn load(&mut self) {
        self.field = TextField::default();
        let text = if self.editing_name {
            self.name.clone()
        } else {
            color_text(color_at(&self.theme, self.selected))
        };
        self.field.set_text(&text);
        self.problem = None;
        self.list.select_id(Some(self.selected as u32));
        self.list.reveal(self.selected);
    }

    pub fn select(&mut self, index: usize) {
        self.selected = index.min(color_count() - 1);
        self.editing_name = false;
        self.load();
    }

    /// Why a name can't be used, if it can't.
    fn name_problem(&self, name: &str) -> Option<String> {
        if name.is_empty() {
            return Some("A theme needs a name".to_string());
        }
        name_taken(name, &self.taken).then(|| format!("Another theme is called {name}"))
    }

    /// Takes the box's text as a color once it's a whole one. A name is only
    /// checked while it's typed, and taken on Enter, since it renames the file.
    fn text_changed(&mut self) -> bool {
        let text = self.field.text().trim().to_string();
        if self.editing_name {
            self.problem = self.name_problem(&text);
            return false;
        }
        match parse_color(&text) {
            Some(color) => {
                self.problem = None;
                set_color_at(&mut self.theme, self.selected, color);
                self.fill_list();
                true
            }
            None => {
                self.problem = Some("Type a color as #RRGGBB".to_string());
                false
            }
        }
    }

    pub fn paste(&mut self, text: &str) -> bool {
        self.field.insert(text) && self.text_changed()
    }

    /// Presses one of the [`OPTIONS`].
    pub fn choose_option(&mut self, index: usize) -> ThemesResponse {
        match index {
            COLOR | NAME => {
                self.editing_name = index == NAME;
                self.load();
                ThemesResponse::redraw(false)
            }
            DARK => {
                // Dark themes get a dark title bar.
                self.theme.is_dark = !self.theme.is_dark;
                ThemesResponse::redraw(true)
            }
            OPEN_FOLDER => ThemesResponse {
                open_folder: true,
                ..ThemesResponse::default()
            },
            _ => ThemesResponse::default(),
        }
    }

    pub fn arrange(&mut self, area: Rect, canvas: &mut dyn Canvas, theme: &Theme) {
        let width = WIDTH.min(area.width() - 2.0 * PADDING).max(0.0);
        let left = area.left + (area.width() - width) / 2.0;
        let top = area.top + OFFSET;
        self.rect = Rect::new(
            left,
            top,
            left + width,
            (top + HEIGHT).min(area.bottom.max(top)),
        );
        let mut inner = Rect::new(
            self.rect.left + PADDING,
            self.rect.top + PADDING + LINE_HEIGHT,
            self.rect.right - PADDING,
            (self.rect.bottom - PADDING).max(self.rect.top + PADDING + LINE_HEIGHT),
        );
        self.field_rect = inner.take_top(FIELD_HEIGHT);
        inner.take_top(GAP);
        let options = inner.take_top(chip_bar::BAR_HEIGHT);
        inner.take_top(GAP);
        self.hint_rect = inner.take_bottom(LINE_HEIGHT);
        self.note_rect = inner.take_bottom(LINE_HEIGHT);
        self.list.arrange(inner);
        self.option_chips = OPTIONS
            .iter()
            .enumerate()
            .map(|(index, label)| Chip {
                label: (*label).to_string(),
                count: None,
                color: theme.primary_text,
                active: match index {
                    COLOR => !self.editing_name,
                    NAME => self.editing_name,
                    DARK => self.theme.is_dark,
                    _ => false,
                },
            })
            .collect();
        // The chip bar starts a margin in, so it's nudged back to line up.
        self.options_rect = Rect::new(
            options.left - 8.0,
            options.top,
            options.right,
            options.bottom,
        );
        self.options
            .arrange(self.options_rect, &self.option_chips, canvas);
    }

    pub fn handle(&mut self, event: Event, canvas: &mut dyn Canvas) -> ThemesResponse {
        match event {
            Event::MouseDown(point, ..) if !self.rect.contains(point) => {
                return ThemesResponse {
                    closed: true,
                    ..ThemesResponse::default()
                };
            }
            Event::KeyDown(Key::Escape, _) => {
                return ThemesResponse {
                    closed: true,
                    ..ThemesResponse::default()
                };
            }
            Event::KeyDown(Key::Tab, modifiers) if !modifiers.ctrl && !modifiers.alt => {
                self.focus = self.focus.step(modifiers.shift);
                return ThemesResponse::redraw(false);
            }
            Event::MouseDown(point, MouseButton::Left, _) => {
                return self.press(point, event, canvas)
            }
            Event::MouseMove(_) | Event::MouseUp(..) => {
                let redraw = self.field.handle(event, canvas, self.field_rect).redraw
                    | self.options.handle(event).redraw
                    | self.list.handle(event).redraw;
                return ThemesResponse {
                    redraw,
                    ..ThemesResponse::default()
                };
            }
            Event::Wheel(_) => {
                return ThemesResponse {
                    redraw: self.list.handle(event).redraw,
                    ..ThemesResponse::default()
                };
            }
            _ => {}
        }
        match self.focus {
            Part::Field => self.field_event(event, canvas),
            Part::Options => {
                let response = self.options.handle(event);
                match response.toggled {
                    Some(index) => self.choose_option(index),
                    None => ThemesResponse {
                        redraw: response.redraw,
                        ..ThemesResponse::default()
                    },
                }
            }
            Part::List => {
                let response = self.list.handle(event);
                if let Some(id) = self
                    .list
                    .selected_id()
                    .filter(|id| *id as usize != self.selected)
                {
                    self.select(id as usize);
                }
                ThemesResponse {
                    redraw: response.redraw,
                    ..ThemesResponse::default()
                }
            }
        }
    }

    fn press(&mut self, point: Point, event: Event, canvas: &mut dyn Canvas) -> ThemesResponse {
        if self.field_rect.contains(point) {
            self.focus = Part::Field;
            self.field.handle(event, canvas, self.field_rect);
            return ThemesResponse::redraw(false);
        }
        if self.options.contains(point) {
            self.focus = Part::Options;
            return match self.options.handle(event).toggled {
                Some(index) => self.choose_option(index),
                None => ThemesResponse::redraw(false),
            };
        }
        if self.list.contains(point) {
            self.focus = Part::List;
            if let Some(id) = self.list.handle(event).activated {
                self.select(id as usize);
            }
            return ThemesResponse::redraw(false);
        }
        ThemesResponse::default()
    }

    fn field_event(&mut self, event: Event, canvas: &mut dyn Canvas) -> ThemesResponse {
        if let Event::KeyDown(key @ (Key::Up | Key::Down), modifiers) = event {
            if !modifiers.alt && !modifiers.ctrl && !self.editing_name {
                let index = if key == Key::Up {
                    self.selected.saturating_sub(1)
                } else {
                    self.selected + 1
                };
                self.select(index);
                return ThemesResponse::redraw(false);
            }
        }
        let response = self.field.handle(event, canvas, self.field_rect);
        let mut changed = response.changed && self.text_changed();
        if response.submitted.is_some() && self.editing_name {
            let name = self.field.text().trim().to_string();
            self.problem = self.name_problem(&name);
            if self.problem.is_none() && name != self.name {
                self.name = name;
                changed = true;
            }
        }
        ThemesResponse {
            redraw: response.redraw || response.changed || response.submitted.is_some(),
            changed,
            copy: response.copy,
            wants_paste: response.wants_paste,
            ..ThemesResponse::default()
        }
    }

    /// Draws the panel in the theme being edited, so it previews too, and
    /// returns where the box's caret is, for the IME.
    pub fn draw(&mut self, canvas: &mut dyn Canvas, theme: &Theme) -> Point {
        let rect = self.rect;
        canvas.fill_rect(
            Rect::new(
                rect.left - 1.0,
                rect.top - 1.0,
                rect.right + 1.0,
                rect.bottom + 1.0,
            ),
            theme.border,
        );
        canvas.fill_rect(rect, theme.panel_background);
        canvas.push_clip(rect);
        let title = Rect::new(
            rect.left + PADDING,
            rect.top + PADDING - 4.0,
            rect.right - PADDING,
            rect.top + PADDING - 4.0 + LINE_HEIGHT,
        );
        canvas.text(&self.title(), title, LABEL, theme.primary_text);
        let placeholder = if self.editing_name {
            "The theme's name, which is also its file's"
        } else {
            "A color, as #RRGGBB"
        };
        let caret = self.field.draw(
            canvas,
            self.field_rect,
            theme,
            self.focus == Part::Field,
            placeholder,
        );
        self.options.draw(
            canvas,
            self.options_rect,
            &self.option_chips,
            theme,
            self.focus == Part::Options,
        );
        self.list.draw(canvas, theme, self.focus == Part::List);
        if let Some(note) = self.note() {
            canvas.text(&note, self.note_rect, LABEL, theme.error_text);
        }
        canvas.text(HINT, self.hint_rect, LABEL, theme.secondary_text);
        canvas.pop_clip();
        caret
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tv_ui::recording::RecordingCanvas;
    use tv_ui::Modifiers;

    const NONE: Modifiers = Modifiers {
        ctrl: false,
        shift: false,
        alt: false,
    };
    const AREA: Rect = Rect::new(0.0, 0.0, 1000.0, 700.0);

    fn panel() -> ThemesPanel {
        let mut panel = ThemesPanel::new(
            "Classic copy",
            Theme::CLASSIC,
            None,
            vec!["Mine".to_string()],
        );
        panel.arrange(AREA, &mut RecordingCanvas::new(), &Theme::CLASSIC);
        panel
    }

    fn send(panel: &mut ThemesPanel, event: Event) -> ThemesResponse {
        let response = panel.handle(event, &mut RecordingCanvas::new());
        panel.arrange(AREA, &mut RecordingCanvas::new(), &Theme::CLASSIC);
        response
    }

    fn retype(panel: &mut ThemesPanel, text: &str) -> bool {
        panel.field.select_all();
        let mut changed = false;
        for character in text.chars() {
            changed |= send(panel, Event::Char(character)).changed;
        }
        changed
    }

    #[test]
    fn every_color_is_listed_with_its_value() {
        let panel = panel();
        assert_eq!(panel.list.items.len(), 58);
        assert_eq!(panel.field.text(), "#FFFFFF", "window background first");
        assert_eq!(color_label(1), "Grid background");
        assert_eq!(color_label(COLOR_TOKENS.len()), "Red rule row");
        assert_eq!(color_label(57), "Purple rule marker");
        assert_eq!(
            panel.title(),
            "Edit theme: a change makes a copy called Classic copy"
        );
    }

    #[test]
    fn a_full_color_applies_and_partial_ones_wait() {
        let mut panel = panel();
        send(&mut panel, Event::KeyDown(Key::Down, NONE));
        assert_eq!(panel.selected(), 1);
        assert!(retype(&mut panel, "#102030"));
        assert_eq!(panel.theme.grid_background, Color::rgb(0x102030));
        retype(&mut panel, "#10");
        assert_eq!(panel.note().as_deref(), Some("Type a color as #RRGGBB"));
        assert_eq!(
            panel.theme.grid_background,
            Color::rgb(0x102030),
            "unchanged meanwhile"
        );
        panel.select(COLOR_TOKENS.len() + 4);
        assert!(retype(&mut panel, "ABCDEF"));
        assert_eq!(panel.theme.rule_colors[1].text, Color::rgb(0xABCDEF));
    }

    #[test]
    fn weak_contrast_is_pointed_out() {
        let mut panel = panel();
        panel.select(7);
        assert_eq!(color_label(7), "Primary text");
        retype(&mut panel, "#EEEEEE");
        let note = panel.note().unwrap();
        assert!(
            note.starts_with("Low contrast: Primary text: 1.2 to 1"),
            "{note}"
        );
        assert!(note.contains("more"));
    }

    #[test]
    fn names_must_be_free_and_dark_is_a_switch() {
        let mut panel = panel();
        panel.choose_option(NAME);
        assert!(panel.editing_name);
        assert_eq!(panel.field_label(), "Theme name");
        assert_eq!(panel.field.text(), "Classic copy");
        let enter = |panel: &mut ThemesPanel| send(panel, Event::KeyDown(Key::Enter, NONE)).changed;
        assert!(!retype(&mut panel, "mine"));
        assert_eq!(
            panel.note().as_deref(),
            Some("Another theme is called mine")
        );
        assert!(!enter(&mut panel), "a taken name isn't used");
        retype(&mut panel, "Neon");
        assert!(!enter(&mut panel), "nor a built-in theme's");
        assert!(!retype(&mut panel, "Paper"), "a name waits for Enter");
        assert_eq!(panel.name, "Classic copy");
        assert!(enter(&mut panel));
        assert_eq!(panel.name, "Paper");
        assert!(panel.choose_option(DARK).changed);
        assert!(panel.theme.is_dark);
        assert!(panel.choose_option(OPEN_FOLDER).open_folder);
    }

    #[test]
    fn it_fits_small_windows_and_closes() {
        let mut panel = panel();
        let mut canvas = RecordingCanvas::new();
        panel.arrange(Rect::new(0.0, 0.0, 300.0, 120.0), &mut canvas, &Theme::DARK);
        panel.draw(&mut canvas, &Theme::DARK);
        assert!(canvas.clips_balance());
        assert!(send(&mut panel, Event::KeyDown(Key::Escape, NONE)).closed);
    }
}
