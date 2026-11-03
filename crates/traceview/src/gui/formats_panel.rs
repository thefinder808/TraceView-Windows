//! The log formats editor, opened by Tools > Log formats. It floats over the
//! window and takes all input until it closes, as the rules editor does.
//!
//! One box edits the selected format's name, pattern or file names, chosen
//! with the buttons under it. The list ends with a "New format" slot, and
//! typing there adds one. While a pattern is typed, it's tried on the open
//! log's first lines, so it can be got right without reopening anything.
//! Changes apply to logs opened afterwards. "Use for this file" adds the open
//! log's name to the format and reads it again with it.

use super::settings::{FormatSetting, MOST_FORMATS};
use std::sync::mpsc::Receiver;
use std::time::SystemTime;
use tv_engine::custom::PARTS;
use tv_engine::parse::ParseContext;
use tv_engine::time::YearHint;
use tv_engine::{encoding, CustomFormat};
use tv_ui::chip_bar::{self, Chip, ChipBar};
use tv_ui::list_view::{ListItem, ListView};
use tv_ui::text_field::TextField;
use tv_ui::{Align, Canvas, Event, FontRole, Key, MouseButton, Point, Rect, TextStyle, Theme};

const WIDTH: f32 = 680.0;
const HEIGHT: f32 = 440.0;
/// How far below the top of its area the panel sits.
const OFFSET: f32 = 24.0;
const PADDING: f32 = 14.0;
const FIELD_HEIGHT: f32 = 26.0;
const LINE_HEIGHT: f32 = 22.0;
const GAP: f32 = 6.0;
const LABEL: TextStyle = TextStyle::new(FontRole::Ui, Align::Left);
pub const TITLE: &str = "Log formats";
const HINT: &str = "Tab moves between parts     Name parts as (?P<time>...)     Esc closes";
const NEW_FORMAT: &str = "New format";

/// The buttons under the box, in order.
pub const OPTIONS: [&str; 6] = [
    "Name",
    "Pattern",
    "Files",
    "Day first",
    "Delete",
    "Use for this file",
];
const NAME: usize = 0;
const PATTERN: usize = 1;
const FILES: usize = 2;
const DAY_FIRST: usize = 3;
const DELETE: usize = 4;
const USE_HERE: usize = 5;

/// Which part of a format the box edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Property {
    Name,
    Pattern,
    Files,
}

impl Property {
    fn placeholder(self) -> &'static str {
        match self {
            Property::Name => "The format's name, as the status bar shows it",
            Property::Pattern => {
                r"A regex for one line, as ^(?P<time>\S+ \S+) (?P<level>\w+) (?P<message>.*)$"
            }
            Property::Files => {
                "File names it's for, as u_ex*.log; app?.txt, or empty to go by the lines"
            }
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Property::Name => "Format name",
            Property::Pattern => "Format pattern",
            Property::Files => "Format file names",
        }
    }
}

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

/// The open log's name and first lines, to try patterns on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sample {
    pub file_name: String,
    pub lines: Vec<String>,
}

/// What the panel needs its owner to do after handling input.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FormatsResponse {
    pub redraw: bool,
    /// The formats changed, so they should be saved and used.
    pub changed: bool,
    pub closed: bool,
    /// The open log should be read again, as the selected format now names it.
    pub reopen: bool,
    pub copy: Option<String>,
    pub wants_paste: bool,
}

impl FormatsResponse {
    fn redraw(changed: bool) -> Self {
        Self {
            redraw: true,
            changed,
            ..Self::default()
        }
    }
}

pub struct FormatsPanel {
    pub field: TextField,
    /// The formats as edited, in order, including one whose pattern is being retyped.
    pub formats: Vec<FormatSetting>,
    /// The format being edited, or `formats.len()` for the "New format" slot.
    selected: usize,
    property: Property,
    pub focus: Part,
    pub list: ListView,
    pub options: ChipBar,
    option_chips: Vec<Chip>,
    sample: Option<Sample>,
    /// The sample on its way from a background read.
    pending: Option<Receiver<Sample>>,
    /// What's wrong with the selected format, if anything.
    problem: Option<String>,
    /// How the selected format does on the sample: a count, then a row's parts.
    test: Vec<String>,
    rect: Rect,
    field_rect: Rect,
    options_rect: Rect,
    test_rect: Rect,
    hint_rect: Rect,
}

fn describe(format: &FormatSetting) -> String {
    let mut text = if format.files.trim().is_empty() {
        "chosen by its lines".to_string()
    } else {
        format!("for {}", format.files.trim())
    };
    if format.compile().is_err() {
        text.push_str(", not valid");
    }
    text
}

impl FormatsPanel {
    /// Opens the editor on `formats`, with the "New format" slot selected.
    /// The sample arrives later through `pending`, if there's an open log.
    pub fn new(formats: Vec<FormatSetting>, pending: Option<Receiver<Sample>>) -> Self {
        let selected = formats.len();
        let mut panel = Self {
            field: TextField::default(),
            formats,
            selected,
            property: Property::Pattern,
            focus: Part::Field,
            list: ListView::default(),
            options: ChipBar::default(),
            option_chips: Vec::new(),
            sample: None,
            pending,
            problem: None,
            test: Vec::new(),
            rect: Rect::default(),
            field_rect: Rect::default(),
            options_rect: Rect::default(),
            test_rect: Rect::default(),
            hint_rect: Rect::default(),
        };
        panel.load();
        panel
    }

    /// Picks up the sample once it's read. Returns true if it arrived.
    pub fn poll(&mut self) -> bool {
        let Some(sample) = self
            .pending
            .as_ref()
            .and_then(|pending| pending.try_recv().ok())
        else {
            return false;
        };
        self.pending = None;
        self.sample = Some(sample);
        self.check();
        true
    }

    #[cfg(test)]
    pub fn set_sample(&mut self, sample: Sample) {
        self.sample = Some(sample);
        self.check();
    }

    /// The formats worth keeping: every one with a pattern.
    pub fn saved_formats(&self) -> Vec<FormatSetting> {
        self.formats
            .iter()
            .filter(|format| !format.pattern.trim().is_empty())
            .cloned()
            .collect()
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn property(&self) -> Property {
        self.property
    }

    fn is_new(&self) -> bool {
        self.selected >= self.formats.len()
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// How the selected format does on the open log's first lines.
    pub fn test(&self) -> &[String] {
        &self.test
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

    fn value(&self) -> String {
        let Some(format) = self.formats.get(self.selected) else {
            return String::new();
        };
        match self.property {
            Property::Name => format.name.clone(),
            Property::Pattern => format.pattern.clone(),
            Property::Files => format.files.clone(),
        }
    }

    /// Puts the selected format's chosen part in the box, with fresh undo.
    fn load(&mut self) {
        self.field = TextField::default();
        let value = self.value();
        self.field.set_text(&value);
        self.check();
        self.list.select_id(Some(self.selected as u32));
        self.list.reveal(self.selected);
    }

    /// Tries the selected format, and on the sample if there is one.
    fn check(&mut self) {
        self.test.clear();
        self.problem = None;
        let Some(setting) = self.formats.get(self.selected) else {
            return;
        };
        if setting.pattern.trim().is_empty() {
            return;
        }
        let format = match setting.compile() {
            Ok(format) => format,
            Err(error) => {
                self.problem = Some(format!("Not valid yet: {error}"));
                return;
            }
        };
        let Some(sample) = &self.sample else {
            self.test
                .push("Open a log to try the pattern on its lines".to_string());
            return;
        };
        let matched: Vec<&String> = sample
            .lines
            .iter()
            .filter(|line| format.matches(line.as_bytes()))
            .collect();
        self.test.push(format!(
            "Matches {} of the first {} lines of {}",
            matched.len(),
            sample.lines.len(),
            sample.file_name
        ));
        if let Some(line) = matched.first() {
            self.test.push(parts_of(&format, line));
        }
    }

    /// Selects a format, or the "New format" slot past the last one. Returns
    /// true if leaving the old format removed it, because its pattern was empty.
    pub fn select(&mut self, index: usize) -> bool {
        let mut index = index.min(self.formats.len());
        let mut removed = false;
        if index != self.selected
            && self
                .formats
                .get(self.selected)
                .is_some_and(|format| format.pattern.trim().is_empty())
        {
            self.formats.remove(self.selected);
            if index > self.selected {
                index -= 1;
            }
            removed = true;
        }
        self.selected = index;
        if self.is_new() {
            self.property = Property::Pattern;
        }
        self.load();
        removed
    }

    fn set_property(&mut self, property: Property) {
        self.property = property;
        self.load();
    }

    /// Takes the box's text as the selected format's chosen part. Typing into
    /// the "New format" slot adds a format.
    fn text_changed(&mut self) -> bool {
        let text = self.field.text().to_string();
        if self.is_new() {
            if text.trim().is_empty() || self.formats.len() >= MOST_FORMATS {
                return false;
            }
            self.formats.push(FormatSetting {
                name: format!("Format {}", self.formats.len() + 1),
                ..FormatSetting::default()
            });
        }
        let format = &mut self.formats[self.selected];
        match self.property {
            Property::Name => format.name = text,
            Property::Pattern => format.pattern = text,
            Property::Files => format.files = text,
        }
        self.check();
        self.list.select_id(Some(self.selected as u32));
        true
    }

    pub fn paste(&mut self, text: &str) -> bool {
        self.field.insert(text) && self.text_changed()
    }

    /// Whether an option does anything now, for screen readers.
    pub fn option_applies(&self, index: usize) -> bool {
        match index {
            DAY_FIRST | DELETE => !self.is_new(),
            USE_HERE => {
                !self.is_new()
                    && self
                        .sample
                        .as_ref()
                        .is_some_and(|sample| !sample.file_name.is_empty())
            }
            _ => true,
        }
    }

    /// Presses one of the [`OPTIONS`].
    pub fn choose_option(&mut self, index: usize) -> FormatsResponse {
        if !self.option_applies(index) {
            return FormatsResponse::redraw(false);
        }
        match index {
            NAME => self.set_property(Property::Name),
            PATTERN => self.set_property(Property::Pattern),
            FILES => self.set_property(Property::Files),
            DAY_FIRST => {
                let format = &mut self.formats[self.selected];
                format.day_first = !format.day_first;
                self.check();
                return FormatsResponse::redraw(true);
            }
            DELETE => {
                self.formats.remove(self.selected);
                if self.is_new() {
                    self.property = Property::Pattern;
                }
                self.load();
                return FormatsResponse::redraw(true);
            }
            USE_HERE => {
                let name = self
                    .sample
                    .as_ref()
                    .map(|sample| sample.file_name.clone())
                    .unwrap_or_default();
                let format = &mut self.formats[self.selected];
                let already = format
                    .compile()
                    .is_ok_and(|compiled| compiled.matches_file(&name));
                if !already {
                    format.files = if format.files.trim().is_empty() {
                        name
                    } else {
                        format!("{}; {name}", format.files.trim())
                    };
                }
                self.load();
                return FormatsResponse {
                    redraw: true,
                    changed: !already,
                    reopen: true,
                    ..FormatsResponse::default()
                };
            }
            _ => {}
        }
        FormatsResponse::redraw(false)
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
        self.test_rect = inner.take_bottom(2.0 * LINE_HEIGHT);
        self.rebuild(theme);
        self.list.arrange(inner);
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

    fn rebuild(&mut self, theme: &Theme) {
        let day_first = self
            .formats
            .get(self.selected)
            .is_some_and(|format| format.day_first);
        self.option_chips = OPTIONS
            .iter()
            .enumerate()
            .map(|(index, label)| Chip {
                label: (*label).to_string(),
                count: None,
                color: theme.primary_text,
                active: match index {
                    NAME => self.property == Property::Name,
                    PATTERN => self.property == Property::Pattern,
                    FILES => self.property == Property::Files,
                    DAY_FIRST => day_first,
                    _ => false,
                },
            })
            .collect();
        let mut items: Vec<ListItem> = self
            .formats
            .iter()
            .enumerate()
            .map(|(index, format)| ListItem::entry(&format.name, &describe(format), index as u32))
            .collect();
        let detail = if self.formats.len() >= MOST_FORMATS {
            format!("{MOST_FORMATS} formats is the most")
        } else {
            "type a pattern to add one".to_string()
        };
        items.push(ListItem::entry(
            NEW_FORMAT,
            &detail,
            self.formats.len() as u32,
        ));
        self.list.items = items;
        self.list.select_id(Some(self.selected as u32));
    }

    pub fn handle(&mut self, event: Event, canvas: &mut dyn Canvas) -> FormatsResponse {
        let closed = FormatsResponse {
            closed: true,
            ..FormatsResponse::default()
        };
        match event {
            Event::MouseDown(point, ..) if !self.rect.contains(point) => return closed,
            Event::KeyDown(Key::Escape, _) => return closed,
            Event::KeyDown(Key::Tab, modifiers) if !modifiers.ctrl && !modifiers.alt => {
                self.focus = self.focus.step(modifiers.shift);
                return FormatsResponse::redraw(false);
            }
            Event::MouseDown(point, MouseButton::Left, _) => {
                return self.press(point, event, canvas)
            }
            Event::MouseMove(_) | Event::MouseUp(..) => {
                let redraw = self.field.handle(event, canvas, self.field_rect).redraw
                    | self.options.handle(event).redraw
                    | self.list.handle(event).redraw;
                return FormatsResponse {
                    redraw,
                    ..FormatsResponse::default()
                };
            }
            Event::Wheel(_) => {
                return FormatsResponse {
                    redraw: self.list.handle(event).redraw,
                    ..FormatsResponse::default()
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
                    None => FormatsResponse {
                        redraw: response.redraw,
                        ..FormatsResponse::default()
                    },
                }
            }
            Part::List => {
                let response = self.list.handle(event);
                let changed = match self.list.selected_id() {
                    Some(id) if id as usize != self.selected => self.select(id as usize),
                    _ => false,
                };
                FormatsResponse {
                    redraw: response.redraw || changed,
                    changed,
                    ..FormatsResponse::default()
                }
            }
        }
    }

    fn press(&mut self, point: Point, event: Event, canvas: &mut dyn Canvas) -> FormatsResponse {
        if self.field_rect.contains(point) {
            self.focus = Part::Field;
            self.field.handle(event, canvas, self.field_rect);
            return FormatsResponse::redraw(false);
        }
        if self.options.contains(point) {
            self.focus = Part::Options;
            return match self.options.handle(event).toggled {
                Some(index) => self.choose_option(index),
                None => FormatsResponse::redraw(false),
            };
        }
        if self.list.contains(point) {
            self.focus = Part::List;
            let changed = match self.list.handle(event).activated {
                Some(id) if id as usize != self.selected => self.select(id as usize),
                _ => false,
            };
            return FormatsResponse::redraw(changed);
        }
        FormatsResponse::default()
    }

    fn field_event(&mut self, event: Event, canvas: &mut dyn Canvas) -> FormatsResponse {
        if let Event::KeyDown(key @ (Key::Up | Key::Down), modifiers) = event {
            if !modifiers.alt && !modifiers.ctrl {
                let index = if key == Key::Up {
                    self.selected.saturating_sub(1)
                } else {
                    self.selected + 1
                };
                return FormatsResponse::redraw(self.select(index));
            }
        }
        let response = self.field.handle(event, canvas, self.field_rect);
        let changed = response.changed && self.text_changed();
        FormatsResponse {
            redraw: response.redraw || response.changed,
            changed,
            copy: response.copy,
            wants_paste: response.wants_paste,
            ..FormatsResponse::default()
        }
    }

    /// Draws the panel and returns where the box's caret is, for the IME.
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
        canvas.text(TITLE, title, LABEL, theme.primary_text);
        let caret = self.field.draw(
            canvas,
            self.field_rect,
            theme,
            self.focus == Part::Field,
            self.property.placeholder(),
        );
        self.options.draw(
            canvas,
            self.options_rect,
            &self.option_chips,
            theme,
            self.focus == Part::Options,
        );
        self.list.draw(canvas, theme, self.focus == Part::List);
        let line = |index: f32| {
            Rect::new(
                self.test_rect.left,
                self.test_rect.top + index * LINE_HEIGHT,
                self.test_rect.right,
                self.test_rect.top + (index + 1.0) * LINE_HEIGHT,
            )
        };
        match &self.problem {
            Some(problem) => canvas.text(problem, line(0.0), LABEL, theme.error_text),
            None => {
                for (index, text) in self.test.iter().enumerate() {
                    canvas.text(text, line(index as f32), LABEL, theme.secondary_text);
                }
            }
        }
        canvas.text(HINT, self.hint_rect, LABEL, theme.secondary_text);
        canvas.pop_clip();
        caret
    }
}

/// The parts a format finds in a line, as `time 10:22:01, level Warning, message slow`.
fn parts_of(format: &CustomFormat, line: &str) -> String {
    let context = ParseContext {
        year_hint: YearHint::from_system_time(SystemTime::now()),
        custom: None,
    };
    let Some(fields) = format.fields(line.as_bytes(), &context) else {
        return String::new();
    };
    let shown = |bytes: &[u8]| encoding::display(bytes).into_owned();
    let mut parts = Vec::new();
    for part in PARTS {
        if !format.has_part(part) && !matches!(part, "level" | "message") {
            continue;
        }
        let value = match part {
            "time" => fields.stamp.map_or_else(
                || "not read".to_string(),
                |stamp| stamp.to_string().replace('T', " "),
            ),
            "level" => format!("{:?}", fields.level),
            "component" => shown(fields.component),
            "thread" => shown(fields.thread),
            "source" => shown(fields.source),
            _ => shown(fields.message),
        };
        parts.push(format!("{part} {value}"));
    }
    format!("First match: {}", parts.join(", "))
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
    const PIPES: &str = r"^(?P<time>[^|]+)\|(?P<level>[^|]+)\|(?P<message>.*)$";

    fn sample() -> Sample {
        Sample {
            file_name: "service.log".to_string(),
            lines: vec![
                "2026-01-31 10:22:01|WARN|slow".to_string(),
                "2026-01-31 10:22:02|INFO|fine".to_string(),
                "something else".to_string(),
            ],
        }
    }

    fn panel(formats: Vec<FormatSetting>) -> FormatsPanel {
        let mut panel = FormatsPanel::new(formats, None);
        panel.set_sample(sample());
        panel.arrange(AREA, &mut RecordingCanvas::new(), &Theme::CLASSIC);
        panel
    }

    fn send(panel: &mut FormatsPanel, event: Event) -> FormatsResponse {
        let response = panel.handle(event, &mut RecordingCanvas::new());
        panel.arrange(AREA, &mut RecordingCanvas::new(), &Theme::CLASSIC);
        response
    }

    fn type_text(panel: &mut FormatsPanel, text: &str) -> bool {
        let mut changed = false;
        for character in text.chars() {
            changed |= send(panel, Event::Char(character)).changed;
        }
        changed
    }

    fn pipes() -> FormatSetting {
        FormatSetting {
            name: "Pipes".to_string(),
            pattern: PIPES.to_string(),
            ..FormatSetting::default()
        }
    }

    #[test]
    fn typing_a_pattern_adds_a_format_and_tries_it_on_the_log() {
        let mut panel = panel(Vec::new());
        assert_eq!(panel.property(), Property::Pattern);
        assert!(type_text(&mut panel, PIPES));
        assert_eq!(panel.formats[0].name, "Format 1");
        assert_eq!(panel.formats[0].pattern, PIPES);
        assert_eq!(
            panel.test()[0],
            "Matches 2 of the first 3 lines of service.log"
        );
        assert_eq!(
            panel.test()[1],
            "First match: time 2026-01-31 10:22:01.000, level Warning, message slow"
        );
    }

    #[test]
    fn the_buttons_pick_what_the_box_edits() {
        let mut panel = panel(vec![pipes()]);
        panel.select(0);
        assert_eq!(panel.field.text(), PIPES);
        panel.choose_option(NAME);
        assert_eq!(panel.field.text(), "Pipes");
        send(&mut panel, Event::KeyDown(Key::Backspace, NONE));
        type_text(&mut panel, " line");
        assert_eq!(panel.formats[0].name, "Pipe line");
        panel.choose_option(FILES);
        type_text(&mut panel, "svc*.log");
        assert_eq!(panel.formats[0].files, "svc*.log");
        assert!(panel.choose_option(DAY_FIRST).changed);
        assert!(panel.formats[0].day_first);
    }

    #[test]
    fn mistakes_are_explained_and_empty_formats_go_when_left() {
        let mut panel = panel(Vec::new());
        type_text(&mut panel, "(?P<time>");
        assert!(panel
            .problem()
            .is_some_and(|problem| problem.starts_with("Not valid yet")));
        send(&mut panel, Event::KeyDown(Key::Home, NONE));
        for _ in 0..9 {
            send(&mut panel, Event::KeyDown(Key::Delete, NONE));
        }
        assert_eq!(panel.formats.len(), 1, "kept while it's retyped");
        assert!(panel.saved_formats().is_empty());
        send(&mut panel, Event::KeyDown(Key::Down, NONE));
        assert!(panel.formats.is_empty());
    }

    #[test]
    fn use_for_this_file_adds_its_name_once_and_reads_it_again() {
        let mut panel = panel(vec![pipes()]);
        panel.select(0);
        let response = panel.choose_option(USE_HERE);
        assert!(response.reopen && response.changed);
        assert_eq!(panel.formats[0].files, "service.log");
        let again = panel.choose_option(USE_HERE);
        assert!(again.reopen && !again.changed, "already named");
        assert_eq!(panel.formats[0].files, "service.log");
        assert!(panel.choose_option(DELETE).changed);
        assert!(panel.formats.is_empty());
        assert!(!panel.option_applies(USE_HERE), "nothing selected to use");
    }

    #[test]
    fn without_a_log_it_says_how_to_try_a_pattern() {
        let mut panel = FormatsPanel::new(vec![pipes()], None);
        panel.select(0);
        assert_eq!(panel.test(), ["Open a log to try the pattern on its lines"]);
        assert!(!panel.option_applies(USE_HERE));
    }

    #[test]
    fn it_fits_small_windows_and_tab_cycles() {
        let mut panel = panel(vec![pipes()]);
        for expected in [Part::Options, Part::List, Part::Field] {
            send(&mut panel, Event::KeyDown(Key::Tab, NONE));
            assert_eq!(panel.focus, expected);
        }
        let mut canvas = RecordingCanvas::new();
        panel.arrange(Rect::new(0.0, 0.0, 300.0, 120.0), &mut canvas, &Theme::DARK);
        panel.draw(&mut canvas, &Theme::DARK);
        assert!(canvas.clips_balance());
        assert!(send(&mut panel, Event::KeyDown(Key::Escape, NONE)).closed);
    }
}
