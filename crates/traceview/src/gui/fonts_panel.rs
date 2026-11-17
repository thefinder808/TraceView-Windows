//! The fonts editor, opened by View > Fonts and row spacing. It lists the log
//! font, the row spacing and the window font, and its buttons change whichever
//! is selected, with labels to match. Choosing a font hands over to a
//! searchable list of installed fonts, which comes back here when it closes.
//! Every change applies at once.

use super::fonts::{self, Spacing, Target, LOG_SIZES, WINDOW_SIZES};
use super::settings::FontSettings;
use tv_ui::chip_bar::{self, Chip, ChipBar};
use tv_ui::list_view::{ListItem, ListView};
use tv_ui::{Align, Canvas, Event, FontRole, Key, MouseButton, Rect, TextStyle, Theme};

const WIDTH: f32 = 520.0;
const HEIGHT: f32 = 230.0;
/// How far below the top of its area the panel sits.
const OFFSET: f32 = 24.0;
const PADDING: f32 = 14.0;
const LINE_HEIGHT: f32 = 22.0;
const GAP: f32 = 6.0;
const LABEL: TextStyle = TextStyle::new(FontRole::Ui, Align::Left);
pub const TITLE: &str = "Fonts and row spacing";
const NOTE: &str = "For larger everything, change Windows' display scale";
const FONT_HINT: &str =
    "Enter chooses a font     - and + resize     Ctrl+Plus and Ctrl+Minus work anywhere";
const ROWS_HINT: &str = "- and + change the space between rows";

/// What a button under the list does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Choose,
    Smaller,
    Larger,
    Reset,
}

/// The buttons for the selected item, in order. Row spacing has no font to
/// choose, and its smaller and larger are less and more space.
fn buttons_for(item: Item) -> &'static [(Press, &'static str)] {
    match item {
        Item::Rows => &[
            (Press::Smaller, "Less space"),
            (Press::Larger, "More space"),
            (Press::Reset, "Reset"),
        ],
        Item::LogFont | Item::WindowFont => &[
            (Press::Choose, "Choose font"),
            (Press::Smaller, "Smaller"),
            (Press::Larger, "Larger"),
            (Press::Reset, "Reset"),
        ],
    }
}

/// What the list's rows stand for, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    LogFont,
    Rows,
    WindowFont,
}

impl Item {
    const ALL: [Item; 3] = [Item::LogFont, Item::Rows, Item::WindowFont];

    fn label(self) -> &'static str {
        match self {
            Item::LogFont => Target::Log.label(),
            Item::Rows => "Row spacing",
            Item::WindowFont => Target::Window.label(),
        }
    }

    pub fn for_target(target: Target) -> Self {
        match target {
            Target::Log => Item::LogFont,
            Target::Window => Item::WindowFont,
        }
    }
}

/// The part of the panel the keyboard is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    List,
    Buttons,
}

/// What the panel needs its owner to do after handling input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FontsResponse {
    pub redraw: bool,
    /// The fonts changed, so they should be applied and saved.
    pub changed: bool,
    pub closed: bool,
    /// The owner should close the panel and show the fonts to choose from.
    pub choose: Option<Target>,
}

impl FontsResponse {
    fn redraw(changed: bool) -> Self {
        Self {
            redraw: true,
            changed,
            ..Self::default()
        }
    }
}

pub struct FontsPanel {
    pub fonts: FontSettings,
    pub focus: Part,
    pub list: ListView,
    pub buttons: ChipBar,
    button_chips: Vec<Chip>,
    /// Whether the log and window fonts are missing here, so the default draws.
    missing: (bool, bool),
    rect: Rect,
    buttons_rect: Rect,
    note_rect: Rect,
    hint_rect: Rect,
}

impl FontsPanel {
    pub fn new(fonts: FontSettings, selected: Item) -> Self {
        let mut panel = Self {
            fonts,
            focus: Part::List,
            list: ListView::default(),
            buttons: ChipBar::default(),
            button_chips: Vec::new(),
            missing: (false, false),
            rect: Rect::default(),
            buttons_rect: Rect::default(),
            note_rect: Rect::default(),
            hint_rect: Rect::default(),
        };
        panel.check_fonts();
        panel.fill_list();
        panel.select(selected);
        panel
    }

    /// Asks Windows whether the fonts are installed. Done when they change,
    /// rather than every frame.
    fn check_fonts(&mut self) {
        let installed = |name: &str| name.trim().is_empty() || tv_render::is_installed(name.trim());
        self.missing = (
            !installed(&self.fonts.log_font),
            !installed(&self.fonts.window_font),
        );
    }

    /// How an item reads in the list, after its label.
    pub fn describe(&self, item: Item) -> String {
        let shown = fonts::render_fonts(&self.fonts);
        let font = |name: &str, size: f32, missing: bool, default: &str| {
            if missing {
                format!("{name} isn't installed here, so {default} is used, {size}")
            } else {
                format!("{name}, {size}")
            }
        };
        let defaults = tv_render::Fonts::default();
        match item {
            Item::LogFont => font(
                &shown.grid_family,
                shown.grid_size,
                self.missing.0,
                &defaults.grid_family,
            ),
            Item::Rows => format!(
                "{} spacing",
                Spacing::from_name(&self.fonts.row_spacing).name()
            ),
            Item::WindowFont => font(
                &shown.ui_family,
                shown.ui_size,
                self.missing.1,
                &defaults.ui_family,
            ),
        }
    }

    fn fill_list(&mut self) {
        let selected = self.selected();
        self.list.items = Item::ALL
            .iter()
            .enumerate()
            .map(|(index, item)| ListItem::entry(item.label(), &self.describe(*item), index as u32))
            .collect();
        self.select(selected);
    }

    pub fn selected(&self) -> Item {
        self.list
            .selected_id()
            .and_then(|id| Item::ALL.get(id as usize).copied())
            .unwrap_or(Item::LogFont)
    }

    pub fn select(&mut self, item: Item) {
        let index = Item::ALL.iter().position(|each| *each == item).unwrap_or(0);
        self.list.select_id(Some(index as u32));
    }

    pub fn button_chips(&self) -> &[Chip] {
        &self.button_chips
    }

    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// What the button at `index` does for the selected item.
    fn button(&self, index: usize) -> Option<Press> {
        buttons_for(self.selected())
            .get(index)
            .map(|(press, _)| *press)
    }

    /// Whether the button at `index` does anything for the selected item.
    pub fn button_applies(&self, index: usize) -> bool {
        self.button(index).is_some_and(|press| self.applies(press))
    }

    fn applies(&self, press: Press) -> bool {
        let shown = fonts::render_fonts(&self.fonts);
        let spacing = Spacing::from_name(&self.fonts.row_spacing);
        match (press, self.selected()) {
            (Press::Choose, item) => item != Item::Rows,
            (Press::Smaller, Item::LogFont) => shown.grid_size > *LOG_SIZES.start(),
            (Press::Larger, Item::LogFont) => shown.grid_size < *LOG_SIZES.end(),
            (Press::Smaller, Item::WindowFont) => shown.ui_size > *WINDOW_SIZES.start(),
            (Press::Larger, Item::WindowFont) => shown.ui_size < *WINDOW_SIZES.end(),
            (Press::Smaller, Item::Rows) => spacing != Spacing::Compact,
            (Press::Larger, Item::Rows) => spacing != Spacing::Roomy,
            (Press::Reset, _) => self.fonts != FontSettings::default(),
        }
    }

    /// Presses the button at `index`.
    pub fn press(&mut self, index: usize) -> FontsResponse {
        match self.button(index) {
            Some(press) => self.act(press),
            None => FontsResponse::redraw(false),
        }
    }

    fn act(&mut self, press: Press) -> FontsResponse {
        if !self.applies(press) {
            return FontsResponse::redraw(false);
        }
        let shown = fonts::render_fonts(&self.fonts);
        let larger = press == Press::Larger;
        match (press, self.selected()) {
            (Press::Choose, Item::LogFont) => {
                return FontsResponse {
                    choose: Some(Target::Log),
                    ..FontsResponse::default()
                };
            }
            (Press::Choose, Item::WindowFont) => {
                return FontsResponse {
                    choose: Some(Target::Window),
                    ..FontsResponse::default()
                };
            }
            (Press::Smaller | Press::Larger, Item::LogFont) => {
                self.fonts.log_size = fonts::step_size(shown.grid_size, &LOG_SIZES, larger);
            }
            (Press::Smaller | Press::Larger, Item::WindowFont) => {
                self.fonts.window_size = fonts::step_size(shown.ui_size, &WINDOW_SIZES, larger);
            }
            (Press::Smaller | Press::Larger, Item::Rows) => {
                let spacing = Spacing::from_name(&self.fonts.row_spacing).step(larger);
                self.fonts.row_spacing = spacing.name().to_string();
            }
            (Press::Reset, _) => self.fonts = FontSettings::default(),
            (Press::Choose, Item::Rows) => return FontsResponse::redraw(false),
        }
        self.check_fonts();
        self.fill_list();
        FontsResponse::redraw(true)
    }

    /// Lays the panel out near the top of `area` and rebuilds what it shows.
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
        self.hint_rect = inner.take_bottom(LINE_HEIGHT);
        self.note_rect = inner.take_bottom(LINE_HEIGHT);
        let buttons = inner.take_bottom(chip_bar::BAR_HEIGHT);
        inner.take_bottom(GAP);
        self.fill_list();
        self.list.arrange(inner);
        self.button_chips = buttons_for(self.selected())
            .iter()
            .map(|(_, label)| Chip {
                label: (*label).to_string(),
                count: None,
                color: theme.primary_text,
                active: false,
            })
            .collect();
        // The chip bar starts a margin in, so it's nudged back to line up.
        self.buttons_rect = Rect::new(
            buttons.left - 8.0,
            buttons.top,
            buttons.right,
            buttons.bottom,
        );
        self.buttons
            .arrange(self.buttons_rect, &self.button_chips, canvas);
    }

    pub fn handle(&mut self, event: Event) -> FontsResponse {
        let closed = FontsResponse {
            closed: true,
            ..FontsResponse::default()
        };
        match event {
            Event::MouseDown(point, ..) if !self.rect.contains(point) => return closed,
            Event::KeyDown(Key::Escape, _) => return closed,
            Event::KeyDown(Key::Tab, modifiers) if !modifiers.ctrl && !modifiers.alt => {
                self.focus = match self.focus {
                    Part::List => Part::Buttons,
                    Part::Buttons => Part::List,
                };
                return FontsResponse::redraw(false);
            }
            Event::MouseDown(point, MouseButton::Left, _) if self.buttons.contains(point) => {
                self.focus = Part::Buttons;
                return match self.buttons.handle(event).toggled {
                    Some(index) => self.press(index),
                    None => FontsResponse::redraw(false),
                };
            }
            Event::MouseDown(point, MouseButton::Left, _) if self.list.contains(point) => {
                self.focus = Part::List;
                self.list.handle(event);
                return FontsResponse::redraw(false);
            }
            Event::MouseMove(_) | Event::MouseUp(..) => {
                let redraw = self.list.handle(event).redraw | self.buttons.handle(event).redraw;
                return FontsResponse {
                    redraw,
                    ..FontsResponse::default()
                };
            }
            _ => {}
        }
        match self.focus {
            Part::Buttons => {
                let response = self.buttons.handle(event);
                match response.toggled {
                    Some(index) => self.press(index),
                    None => FontsResponse {
                        redraw: response.redraw,
                        ..FontsResponse::default()
                    },
                }
            }
            Part::List => match event {
                Event::KeyDown(Key::Enter, _) => self.act(Press::Choose),
                Event::Char('-') => self.act(Press::Smaller),
                Event::Char('+' | '=') => self.act(Press::Larger),
                _ => FontsResponse {
                    redraw: self.list.handle(event).redraw,
                    ..FontsResponse::default()
                },
            },
        }
    }

    pub fn draw(&self, canvas: &mut dyn Canvas, theme: &Theme) {
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
        self.list.draw(canvas, theme, self.focus == Part::List);
        self.buttons.draw(
            canvas,
            self.buttons_rect,
            &self.button_chips,
            theme,
            self.focus == Part::Buttons,
        );
        canvas.text(NOTE, self.note_rect, LABEL, theme.secondary_text);
        let hint = match self.selected() {
            Item::Rows => ROWS_HINT,
            Item::LogFont | Item::WindowFont => FONT_HINT,
        };
        canvas.text(hint, self.hint_rect, LABEL, theme.secondary_text);
        canvas.pop_clip();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tv_ui::recording::RecordingCanvas;
    use tv_ui::{Modifiers, Point};

    const NONE: Modifiers = Modifiers {
        ctrl: false,
        shift: false,
        alt: false,
    };
    const AREA: Rect = Rect::new(0.0, 0.0, 1000.0, 700.0);

    fn panel(selected: Item) -> FontsPanel {
        let mut panel = FontsPanel::new(FontSettings::default(), selected);
        panel.arrange(AREA, &mut RecordingCanvas::new(), &Theme::CLASSIC);
        panel
    }

    fn send(panel: &mut FontsPanel, event: Event) -> FontsResponse {
        let response = panel.handle(event);
        panel.arrange(AREA, &mut RecordingCanvas::new(), &Theme::CLASSIC);
        response
    }

    #[test]
    fn it_lists_the_fonts_and_spacing() {
        let panel = panel(Item::LogFont);
        assert_eq!(panel.describe(Item::LogFont), "Consolas, 13");
        assert_eq!(panel.describe(Item::Rows), "normal spacing");
        assert_eq!(panel.describe(Item::WindowFont), "Segoe UI, 12");
        assert!(!panel.applies(Press::Reset), "nothing to reset yet");
    }

    #[test]
    fn row_spacing_has_buttons_and_a_hint_of_its_own() {
        let labels = |panel: &FontsPanel| -> Vec<String> {
            panel
                .button_chips()
                .iter()
                .map(|chip| chip.label.clone())
                .collect()
        };
        let mut panel = panel(Item::LogFont);
        assert_eq!(
            labels(&panel),
            ["Choose font", "Smaller", "Larger", "Reset"]
        );
        send(&mut panel, Event::KeyDown(Key::Down, NONE));
        assert_eq!(panel.selected(), Item::Rows);
        assert_eq!(labels(&panel), ["Less space", "More space", "Reset"]);
        let mut canvas = RecordingCanvas::new();
        panel.draw(&mut canvas, &Theme::CLASSIC);
        assert!(canvas.texts().contains(&ROWS_HINT));
        assert!(!canvas.texts().contains(&FONT_HINT));

        // The second button means more space here, not a larger font.
        assert!(panel.press(1).changed);
        assert_eq!(panel.fonts.row_spacing, "roomy");
        assert_eq!(panel.fonts.log_size, FontSettings::default().log_size);
        assert!(!panel.button_applies(1), "already roomy");
        assert!(!panel.button_applies(3), "there's no fourth button");
    }

    #[test]
    fn plus_and_minus_change_whatever_is_selected() {
        let mut panel = panel(Item::LogFont);
        assert!(send(&mut panel, Event::Char('+')).changed);
        assert_eq!(panel.fonts.log_size, 14.0);
        send(&mut panel, Event::KeyDown(Key::Down, NONE));
        assert_eq!(panel.selected(), Item::Rows);
        assert!(send(&mut panel, Event::Char('-')).changed);
        assert_eq!(panel.fonts.row_spacing, "compact");
        assert!(
            !send(&mut panel, Event::Char('-')).changed,
            "already compact"
        );
        send(&mut panel, Event::KeyDown(Key::Down, NONE));
        for _ in 0..10 {
            send(&mut panel, Event::Char('+'));
        }
        assert_eq!(panel.fonts.window_size, *WINDOW_SIZES.end());
        assert!(panel.act(Press::Reset).changed);
        assert_eq!(panel.fonts, FontSettings::default());
    }

    #[test]
    fn enter_asks_for_a_font_but_not_for_spacing() {
        let mut panel = panel(Item::WindowFont);
        assert_eq!(
            send(&mut panel, Event::KeyDown(Key::Enter, NONE)).choose,
            Some(Target::Window)
        );
        panel.select(Item::Rows);
        assert_eq!(
            send(&mut panel, Event::KeyDown(Key::Enter, NONE)).choose,
            None
        );
    }

    #[test]
    fn a_font_missing_here_says_what_draws_instead() {
        let fonts = FontSettings {
            log_font: "No Such Font Anywhere".to_string(),
            ..FontSettings::default()
        };
        let panel = FontsPanel::new(fonts, Item::LogFont);
        assert_eq!(
            panel.describe(Item::LogFont),
            "No Such Font Anywhere isn't installed here, so Consolas is used, 13"
        );
    }

    #[test]
    fn buttons_work_by_mouse_and_clicking_outside_closes() {
        let mut panel = panel(Item::LogFont);
        // Choose font, Smaller, then Larger.
        let larger = panel.buttons.chip_rect(2).unwrap();
        let point = Point::new(larger.left + 3.0, larger.top + 3.0);
        assert!(send(&mut panel, Event::MouseDown(point, MouseButton::Left, NONE)).changed);
        assert_eq!(panel.focus, Part::Buttons);
        assert_eq!(panel.fonts.log_size, 14.0);
        let outside = Point::new(2.0, 690.0);
        assert!(
            send(
                &mut panel,
                Event::MouseDown(outside, MouseButton::Left, NONE)
            )
            .closed
        );
        let mut canvas = RecordingCanvas::new();
        panel.arrange(Rect::new(0.0, 0.0, 300.0, 100.0), &mut canvas, &Theme::DARK);
        panel.draw(&mut canvas, &Theme::DARK);
        assert!(canvas.clips_balance());
    }
}
