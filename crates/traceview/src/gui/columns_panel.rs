//! The columns editor, opened by View > Columns or by right-clicking the grid's
//! header. It shows, hides, moves and resizes the columns of one log format,
//! which the mouse can also do by dragging the header. Every change applies at
//! once.

use super::columns::{self, Column, ColumnSpec, MAX_WIDTH, MIN_WIDTH};
use tv_engine::Format;
use tv_ui::chip_bar::{self, Chip, ChipBar};
use tv_ui::list_view::{ListItem, ListView};
use tv_ui::{Align, Canvas, Event, FontRole, Key, MouseButton, Rect, TextStyle, Theme};

const WIDTH: f32 = 520.0;
const HEIGHT: f32 = 330.0;
/// How far below the top of its area the panel sits.
const OFFSET: f32 = 24.0;
const PADDING: f32 = 14.0;
const LINE_HEIGHT: f32 = 22.0;
const GAP: f32 = 6.0;
/// Characters added or taken away by Wider and Narrower.
const WIDTH_STEP: f32 = 2.0;
const LABEL: TextStyle = TextStyle::new(FontRole::Ui, Align::Left);
const NOTE: &str = "The row number always comes first, and the last column fills the window";
const HINT: &str =
    "Space shows or hides     Ctrl+Up and Ctrl+Down move     - and + resize     Esc closes";

/// The buttons under the list, in order.
pub const BUTTONS: [&str; 6] = [
    "Shown",
    "Move up",
    "Move down",
    "Narrower",
    "Wider",
    "Reset",
];
const SHOWN: usize = 0;
const MOVE_UP: usize = 1;
const MOVE_DOWN: usize = 2;
const NARROWER: usize = 3;
const WIDER: usize = 4;
const RESET: usize = 5;

/// The part of the panel the keyboard is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    List,
    Buttons,
}

/// What the panel needs its owner to do after handling input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ColumnsResponse {
    pub redraw: bool,
    /// The columns changed, so they should be applied and saved.
    pub changed: bool,
    pub closed: bool,
}

impl ColumnsResponse {
    fn redraw(changed: bool) -> Self {
        Self {
            redraw: true,
            changed,
            closed: false,
        }
    }
}

/// What a format's logs are called in the panel's title.
fn logs_called(format: Format) -> &'static str {
    match format {
        Format::Sccm => "SCCM logs",
        Format::Plain => "other logs",
    }
}

pub struct ColumnsPanel {
    pub format: Format,
    /// Every column in order, as edited.
    pub columns: Vec<ColumnSpec>,
    pub focus: Part,
    pub list: ListView,
    pub buttons: ChipBar,
    button_chips: Vec<Chip>,
    rect: Rect,
    buttons_rect: Rect,
    note_rect: Rect,
    hint_rect: Rect,
}

/// How a column reads in the list, after its name.
pub fn describe(spec: &ColumnSpec) -> String {
    let width = format!("{} characters wide", spec.width.round());
    match (spec.column, spec.shown) {
        (Column::Message, _) => format!("always shown, {width}"),
        (_, true) => format!("shown, {width}"),
        (_, false) => "hidden".to_string(),
    }
}

impl ColumnsPanel {
    pub fn new(format: Format, columns: Vec<ColumnSpec>) -> Self {
        let mut panel = Self {
            format,
            columns,
            focus: Part::List,
            list: ListView::default(),
            buttons: ChipBar::default(),
            button_chips: Vec::new(),
            rect: Rect::default(),
            buttons_rect: Rect::default(),
            note_rect: Rect::default(),
            hint_rect: Rect::default(),
        };
        // Filled now, so keys pressed before the first frame still work.
        panel.fill_list();
        panel
    }

    fn fill_list(&mut self) {
        let selected = self.selected();
        self.list.items = self
            .columns
            .iter()
            .enumerate()
            .map(|(index, spec)| {
                ListItem::entry(spec.column.label(), &describe(spec), index as u32)
            })
            .collect();
        self.list.select_id(Some(selected as u32));
    }

    pub fn title(&self) -> String {
        format!("Columns for {}", logs_called(self.format))
    }

    /// The selected column's place in the list.
    pub fn selected(&self) -> usize {
        self.list
            .selected_id()
            .map_or(0, |id| id as usize)
            .min(self.columns.len().saturating_sub(1))
    }

    pub fn select(&mut self, index: usize) {
        self.list.select_id(Some(index as u32));
        self.list.reveal(index);
    }

    pub fn button_chips(&self) -> &[Chip] {
        &self.button_chips
    }

    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// Whether a button does anything for the selected column.
    pub fn button_applies(&self, index: usize) -> bool {
        let selected = self.selected();
        let Some(spec) = self.columns.get(selected) else {
            return false;
        };
        match index {
            SHOWN => spec.column != Column::Message,
            MOVE_UP => selected > 0,
            MOVE_DOWN => selected + 1 < self.columns.len(),
            NARROWER => spec.width > MIN_WIDTH,
            WIDER => spec.width < MAX_WIDTH,
            _ => true,
        }
    }

    /// Presses one of the [`BUTTONS`]. Returns true if the columns changed.
    pub fn press(&mut self, index: usize) -> bool {
        if !self.button_applies(index) {
            return false;
        }
        let selected = self.selected();
        match index {
            SHOWN => {
                let spec = &mut self.columns[selected];
                spec.shown = !spec.shown;
            }
            MOVE_UP | MOVE_DOWN => {
                let target = if index == MOVE_UP {
                    selected - 1
                } else {
                    selected + 1
                };
                self.columns.swap(selected, target);
                self.select(target);
            }
            NARROWER | WIDER => {
                let step = if index == WIDER {
                    WIDTH_STEP
                } else {
                    -WIDTH_STEP
                };
                let spec = &mut self.columns[selected];
                spec.width = (spec.width.round() + step).clamp(MIN_WIDTH, MAX_WIDTH);
            }
            RESET => {
                let before = self.columns.clone();
                self.columns = columns::defaults(self.format);
                return self.columns != before;
            }
            _ => return false,
        }
        true
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
        let shown = self
            .columns
            .get(self.selected())
            .is_some_and(|spec| spec.shown);
        self.button_chips = BUTTONS
            .iter()
            .enumerate()
            .map(|(index, label)| Chip {
                label: (*label).to_string(),
                count: None,
                color: theme.primary_text,
                active: index == SHOWN && shown,
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

    pub fn handle(&mut self, event: Event) -> ColumnsResponse {
        let closed = ColumnsResponse {
            closed: true,
            ..ColumnsResponse::default()
        };
        match event {
            Event::MouseDown(point, ..) if !self.rect.contains(point) => return closed,
            Event::KeyDown(Key::Escape, _) => return closed,
            Event::KeyDown(Key::Tab, modifiers) if !modifiers.ctrl && !modifiers.alt => {
                self.focus = match self.focus {
                    Part::List => Part::Buttons,
                    Part::Buttons => Part::List,
                };
                return ColumnsResponse::redraw(false);
            }
            Event::KeyDown(key @ (Key::Up | Key::Down), modifiers) if modifiers.ctrl => {
                let button = if key == Key::Up { MOVE_UP } else { MOVE_DOWN };
                return ColumnsResponse::redraw(self.press(button));
            }
            Event::MouseDown(point, MouseButton::Left, _) if self.buttons.contains(point) => {
                self.focus = Part::Buttons;
                let changed = self
                    .buttons
                    .handle(event)
                    .toggled
                    .is_some_and(|index| self.press(index));
                return ColumnsResponse::redraw(changed);
            }
            Event::MouseDown(point, MouseButton::Left, _) if self.list.contains(point) => {
                self.focus = Part::List;
                self.list.handle(event);
                return ColumnsResponse::redraw(false);
            }
            Event::MouseMove(_) | Event::MouseUp(..) | Event::Wheel(_) => {
                let redraw = self.list.handle(event).redraw | self.buttons.handle(event).redraw;
                return ColumnsResponse {
                    redraw,
                    ..ColumnsResponse::default()
                };
            }
            _ => {}
        }
        match self.focus {
            Part::Buttons => {
                let response = self.buttons.handle(event);
                let changed = response.toggled.is_some_and(|index| self.press(index));
                ColumnsResponse {
                    redraw: response.redraw || changed,
                    changed,
                    closed: false,
                }
            }
            Part::List => match event {
                Event::Char(' ') => ColumnsResponse::redraw(self.press(SHOWN)),
                Event::Char('-') => ColumnsResponse::redraw(self.press(NARROWER)),
                Event::Char('+' | '=') => ColumnsResponse::redraw(self.press(WIDER)),
                _ => ColumnsResponse {
                    redraw: self.list.handle(event).redraw,
                    ..ColumnsResponse::default()
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
        canvas.text(&self.title(), title, LABEL, theme.primary_text);
        self.list.draw(canvas, theme, self.focus == Part::List);
        self.buttons.draw(
            canvas,
            self.buttons_rect,
            &self.button_chips,
            theme,
            self.focus == Part::Buttons,
        );
        canvas.text(NOTE, self.note_rect, LABEL, theme.secondary_text);
        canvas.text(HINT, self.hint_rect, LABEL, theme.secondary_text);
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
    const CTRL: Modifiers = Modifiers {
        ctrl: true,
        shift: false,
        alt: false,
    };
    const AREA: Rect = Rect::new(0.0, 0.0, 1000.0, 700.0);

    fn panel() -> ColumnsPanel {
        let mut panel = ColumnsPanel::new(Format::Sccm, columns::defaults(Format::Sccm));
        panel.arrange(AREA, &mut RecordingCanvas::new(), &Theme::CLASSIC);
        panel
    }

    fn send(panel: &mut ColumnsPanel, event: Event) -> ColumnsResponse {
        let response = panel.handle(event);
        panel.arrange(AREA, &mut RecordingCanvas::new(), &Theme::CLASSIC);
        response
    }

    fn key(panel: &mut ColumnsPanel, key: Key, modifiers: Modifiers) -> ColumnsResponse {
        send(panel, Event::KeyDown(key, modifiers))
    }

    fn order(panel: &ColumnsPanel) -> Vec<Column> {
        panel.columns.iter().map(|spec| spec.column).collect()
    }

    #[test]
    fn space_shows_and_hides_but_the_message_stays() {
        let mut panel = panel();
        key(&mut panel, Key::Down, NONE);
        assert!(send(&mut panel, Event::Char(' ')).changed);
        assert!(!panel.columns[1].shown, "Level is hidden");
        assert!(send(&mut panel, Event::Char(' ')).changed);
        key(&mut panel, Key::End, NONE);
        assert_eq!(panel.columns[panel.selected()].column, Column::Message);
        assert!(!send(&mut panel, Event::Char(' ')).changed);
        assert!(panel.columns[5].shown);
    }

    #[test]
    fn columns_move_and_resize_from_the_keyboard() {
        let mut panel = panel();
        assert!(key(&mut panel, Key::Down, CTRL).changed);
        assert_eq!(order(&panel)[..2], [Column::Level, Column::Time]);
        assert_eq!(panel.selected(), 1, "the selection moves with the column");
        assert!(key(&mut panel, Key::Up, CTRL).changed);
        assert_eq!(panel.selected(), 0);
        assert!(!key(&mut panel, Key::Up, CTRL).changed, "already first");
        assert!(send(&mut panel, Event::Char('+')).changed);
        assert_eq!(panel.columns[0].width, 25.0);
        send(&mut panel, Event::Char('-'));
        send(&mut panel, Event::Char('-'));
        assert_eq!(panel.columns[0].width, 21.0);
    }

    #[test]
    fn reset_brings_back_the_defaults() {
        let mut panel = panel();
        panel.press(SHOWN);
        panel.press(WIDER);
        assert!(panel.press(RESET));
        assert_eq!(panel.columns, columns::defaults(Format::Sccm));
        assert!(!panel.press(RESET), "nothing left to reset");
    }

    #[test]
    fn buttons_work_by_mouse_and_tab_reaches_them() {
        let mut panel = panel();
        key(&mut panel, Key::Tab, NONE);
        assert_eq!(panel.focus, Part::Buttons);
        key(&mut panel, Key::Tab, NONE);
        assert_eq!(panel.focus, Part::List);
        let wider = panel.buttons.chip_rect(WIDER).unwrap();
        let point = Point::new(wider.left + 3.0, wider.top + 3.0);
        assert!(send(&mut panel, Event::MouseDown(point, MouseButton::Left, NONE)).changed);
        assert_eq!(panel.focus, Part::Buttons);
        assert_eq!(panel.columns[0].width, 25.0);
        let row = panel.list.item_rect(3).unwrap();
        let point = Point::new(row.left + 20.0, row.top + 4.0);
        send(&mut panel, Event::MouseDown(point, MouseButton::Left, NONE));
        assert_eq!((panel.focus, panel.selected()), (Part::List, 3));
        let outside = Point::new(2.0, 690.0);
        assert!(
            send(
                &mut panel,
                Event::MouseDown(outside, MouseButton::Left, NONE)
            )
            .closed
        );
        assert!(key(&mut panel, Key::Escape, NONE).closed);
    }

    #[test]
    fn it_reads_well_and_fits_small_windows() {
        let mut panel = panel();
        assert_eq!(panel.title(), "Columns for SCCM logs");
        assert_eq!(describe(&panel.columns[0]), "shown, 23 characters wide");
        assert_eq!(describe(&panel.columns[4]), "hidden");
        assert_eq!(
            describe(&panel.columns[5]),
            "always shown, 80 characters wide"
        );
        panel.arrange(
            Rect::new(0.0, 0.0, 300.0, 120.0),
            &mut RecordingCanvas::new(),
            &Theme::NEON,
        );
        assert!(panel.rect().width() <= 300.0 && panel.rect().bottom <= 120.0);
        let mut canvas = RecordingCanvas::new();
        panel.draw(&mut canvas, &Theme::NEON);
        assert!(canvas.clips_balance());
    }
}
