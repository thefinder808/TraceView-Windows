//! A row of toggle chips, such as `[All 1.8k] [Err 186] [Warn 267]`. The bar
//! only reports which chip was toggled. What that means, including what "All"
//! does to the others, is up to the app.

use crate::canvas::{Align, Canvas, FontRole, TextStyle};
use crate::geometry::{Color, Point, Rect};
use crate::input::{Event, Key, MouseButton};
use crate::theme::Theme;

pub const BAR_HEIGHT: f32 = 30.0;
const CHIP_HEIGHT: f32 = 22.0;
/// The space before the first chip.
const MARGIN: f32 = 8.0;
const SPACING: f32 = 6.0;
const PADDING: f32 = 8.0;
const COUNT_GAP: f32 = 5.0;
/// How far outside a chip its focus outline is drawn.
const FOCUS_OFFSET: f32 = 2.0;
const UI_LEFT: TextStyle = TextStyle::new(FontRole::Ui, Align::Left);
const UI_RIGHT: TextStyle = TextStyle::new(FontRole::Ui, Align::Right);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chip {
    pub label: String,
    pub count: usize,
    /// The text color while the chip is off, so an error chip can be red.
    pub color: Color,
    pub active: bool,
}

/// What a chip bar needs its owner to do after handling input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChipResponse {
    pub redraw: bool,
    /// The chip that was clicked, or pressed with Enter or Space.
    pub toggled: Option<usize>,
}

impl ChipResponse {
    fn toggled(index: usize) -> Self {
        ChipResponse {
            redraw: true,
            toggled: Some(index),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ChipBar {
    chip_rects: Vec<Rect>,
    hovered: Option<usize>,
    /// The chip the arrow keys have reached, while the bar has focus.
    focus: usize,
}

/// Formats a count to fit on a chip: 1,234 exactly, then 12.3k, 123k, 4.5M and
/// so on, rounded to the nearest.
pub fn compact_count(count: usize) -> String {
    if count < 10_000 {
        return with_commas(count);
    }
    let count = count as u128;
    for (unit, suffix) in [(1_000u128, "k"), (1_000_000, "M"), (1_000_000_000, "B")] {
        let tenths = (count * 10 + unit / 2) / unit;
        if tenths < 1_000 {
            return if tenths.is_multiple_of(10) {
                format!("{}{suffix}", tenths / 10)
            } else {
                format!("{}.{}{suffix}", tenths / 10, tenths % 10)
            };
        }
        let whole = (count + unit / 2) / unit;
        if whole < 1_000 {
            return format!("{whole}{suffix}");
        }
    }
    format!("{}B", (count + 500_000_000) / 1_000_000_000)
}

fn with_commas(value: usize) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// A rectangle grown by `by` on every side, or shrunk when it's negative.
fn grow(rect: Rect, by: f32) -> Rect {
    Rect::new(
        rect.left - by,
        rect.top - by,
        rect.right + by,
        rect.bottom + by,
    )
}

/// Draws a 1 px frame just inside `rect`.
fn outline(canvas: &mut dyn Canvas, rect: Rect, color: Color) {
    let (left, top, right, bottom) = (rect.left, rect.top, rect.right, rect.bottom);
    canvas.fill_rect(Rect::new(left, top, right, top + 1.0), color);
    canvas.fill_rect(Rect::new(left, bottom - 1.0, right, bottom), color);
    canvas.fill_rect(Rect::new(left, top + 1.0, left + 1.0, bottom - 1.0), color);
    canvas.fill_rect(
        Rect::new(right - 1.0, top + 1.0, right, bottom - 1.0),
        color,
    );
}

impl ChipBar {
    /// Sizes each chip to its label and count, left to right.
    pub fn arrange(&mut self, bar: Rect, chips: &[Chip], canvas: &mut dyn Canvas) {
        self.chip_rects.clear();
        let middle = (bar.top + bar.bottom) / 2.0;
        let (top, bottom) = (middle - CHIP_HEIGHT / 2.0, middle + CHIP_HEIGHT / 2.0);
        let mut left = bar.left + MARGIN;
        for chip in chips {
            let label = canvas.text_width(&chip.label, FontRole::Ui);
            let count = canvas.text_width(&compact_count(chip.count), FontRole::Ui);
            let width = PADDING * 2.0 + label + COUNT_GAP + count;
            self.chip_rects
                .push(Rect::new(left, top, left + width, bottom));
            left += width + SPACING;
        }
        self.focus = self.focus.min(chips.len().saturating_sub(1));
        self.hovered = self.hovered.filter(|&index| index < chips.len());
    }

    pub fn chip_rect(&self, index: usize) -> Option<Rect> {
        self.chip_rects.get(index).copied()
    }

    /// The chip keyboard focus is on, for the app to put screen reader focus on.
    pub fn focus(&self) -> Option<usize> {
        (self.focus < self.chip_rects.len()).then_some(self.focus)
    }

    pub fn contains(&self, point: Point) -> bool {
        self.chip_rects.iter().any(|rect| rect.contains(point))
    }

    fn chip_at(&self, point: Point) -> Option<usize> {
        self.chip_rects.iter().position(|rect| rect.contains(point))
    }

    fn handle_key(&mut self, key: Key) -> ChipResponse {
        let Some(last) = self.chip_rects.len().checked_sub(1) else {
            return ChipResponse::default();
        };
        let focus = match key {
            Key::Left => self.focus.saturating_sub(1),
            Key::Right => (self.focus + 1).min(last),
            Key::Home => 0,
            Key::End => last,
            Key::Enter => return ChipResponse::toggled(self.focus),
            _ => return ChipResponse::default(),
        };
        let moved = focus != self.focus;
        self.focus = focus;
        ChipResponse {
            redraw: moved,
            toggled: None,
        }
    }

    /// Handles input. Send keys and typed characters only while the bar has
    /// focus: arrows move between chips, and Enter or Space toggles one.
    pub fn handle(&mut self, event: Event) -> ChipResponse {
        match event {
            Event::MouseDown(point, MouseButton::Left, _) => match self.chip_at(point) {
                Some(index) => {
                    self.focus = index;
                    ChipResponse::toggled(index)
                }
                None => ChipResponse::default(),
            },
            Event::MouseMove(point) => {
                let hovered = self.chip_at(point);
                let changed = hovered != self.hovered;
                self.hovered = hovered;
                ChipResponse {
                    redraw: changed,
                    toggled: None,
                }
            }
            Event::KeyDown(key, modifiers) if !modifiers.alt && !modifiers.ctrl => {
                self.handle_key(key)
            }
            Event::Char(' ') if !self.chip_rects.is_empty() => ChipResponse::toggled(self.focus),
            _ => ChipResponse::default(),
        }
    }

    /// Draws the bar. Active chips use the chip-active colors, and inactive ones
    /// their own text color on the chip background.
    pub fn draw(
        &self,
        canvas: &mut dyn Canvas,
        bar: Rect,
        chips: &[Chip],
        theme: &Theme,
        focused: bool,
    ) {
        canvas.fill_rect(bar, theme.panel_background);
        canvas.push_clip(bar);
        for (index, (chip, rect)) in chips.iter().zip(&self.chip_rects).enumerate() {
            let text_color = if chip.active {
                canvas.fill_rect(*rect, theme.chip_active_background);
                theme.chip_active_text
            } else {
                canvas.fill_rect(*rect, theme.border);
                let background = if self.hovered == Some(index) {
                    theme.hover_background
                } else {
                    theme.chip_background
                };
                canvas.fill_rect(grow(*rect, -1.0), background);
                chip.color
            };
            let text = Rect::new(
                rect.left + PADDING,
                rect.top,
                rect.right - PADDING,
                rect.bottom,
            );
            canvas.text(&chip.label, text, UI_LEFT, text_color);
            canvas.text(&compact_count(chip.count), text, UI_RIGHT, text_color);
            if focused && index == self.focus {
                outline(canvas, grow(*rect, FOCUS_OFFSET), theme.accent);
            }
        }
        canvas.pop_clip();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Modifiers;
    use crate::recording::{DrawOp, RecordingCanvas};

    const NONE: Modifiers = Modifiers {
        ctrl: false,
        shift: false,
        alt: false,
    };
    const BAR: Rect = Rect::new(0.0, 50.0, 800.0, 50.0 + BAR_HEIGHT);
    const RED: Color = Color::rgb(0xD13438);

    fn chips() -> Vec<Chip> {
        let chip = |label: &str, count: usize, color: Color, active: bool| Chip {
            label: label.to_string(),
            count,
            color,
            active,
        };
        let text = Theme::CLASSIC.primary_text;
        vec![
            chip("All", 1_769, text, true),
            chip("Err", 186, RED, false),
            chip("Warn", 267, text, false),
            chip("Info", 1_234_567, text, false),
        ]
    }

    fn arranged() -> ChipBar {
        let mut bar = ChipBar::default();
        bar.arrange(BAR, &chips(), &mut RecordingCanvas::new());
        bar
    }

    fn center(rect: Rect) -> Point {
        Point::new(
            (rect.left + rect.right) / 2.0,
            (rect.top + rect.bottom) / 2.0,
        )
    }

    fn key(bar: &mut ChipBar, key: Key) -> ChipResponse {
        bar.handle(Event::KeyDown(key, NONE))
    }

    #[test]
    fn counts_are_compact() {
        for (count, text) in [
            (0, "0"),
            (999, "999"),
            (1_234, "1,234"),
            (9_999, "9,999"),
            (10_000, "10k"),
            (12_345, "12.3k"),
            (99_949, "99.9k"),
            (99_950, "100k"),
            (123_456, "123k"),
            (999_499, "999k"),
            (999_500, "1M"),
            (4_500_000, "4.5M"),
            (26_904_325, "26.9M"),
            (1_234_567_890, "1.2B"),
        ] {
            assert_eq!(compact_count(count), text, "for {count}");
        }
        assert!(compact_count(usize::MAX).ends_with('B'));
    }

    #[test]
    fn chips_fit_their_text_from_left_to_right() {
        let bar = arranged();
        // "All" and "1,769" are 3 and 5 characters, at 7 each.
        let all = bar.chip_rect(0).unwrap();
        assert_eq!(all.left, MARGIN);
        assert_eq!(all.width(), PADDING * 2.0 + 21.0 + COUNT_GAP + 35.0);
        assert_eq!(all.height(), CHIP_HEIGHT);
        let err = bar.chip_rect(1).unwrap();
        assert_eq!(err.left, all.right + SPACING);
        assert_eq!(err.top, all.top);
        assert_eq!(bar.chip_rect(4), None);
    }

    #[test]
    fn clicking_reports_the_chip_toggled() {
        let mut bar = arranged();
        let err = center(bar.chip_rect(1).unwrap());
        let response = bar.handle(Event::MouseDown(err, MouseButton::Left, NONE));
        assert_eq!(response, ChipResponse::toggled(1));
        assert_eq!(bar.focus(), Some(1));
        let gap = Point::new(bar.chip_rect(1).unwrap().right + 1.0, err.y);
        assert_eq!(
            bar.handle(Event::MouseDown(gap, MouseButton::Left, NONE)),
            ChipResponse::default()
        );
        assert_eq!(
            bar.handle(Event::MouseDown(err, MouseButton::Right, NONE)),
            ChipResponse::default()
        );
    }

    #[test]
    fn hovering_redraws_only_on_change() {
        let mut bar = arranged();
        let warn = center(bar.chip_rect(2).unwrap());
        assert!(bar.handle(Event::MouseMove(warn)).redraw);
        assert!(!bar.handle(Event::MouseMove(warn)).redraw);
        assert!(bar.handle(Event::MouseMove(Point::new(790.0, 60.0))).redraw);
    }

    #[test]
    fn keys_move_focus_and_toggle() {
        let mut bar = arranged();
        assert_eq!(bar.focus(), Some(0));
        assert!(!key(&mut bar, Key::Left).redraw, "already at the start");
        assert!(key(&mut bar, Key::Right).redraw);
        assert_eq!(key(&mut bar, Key::Enter).toggled, Some(1));
        assert_eq!(bar.handle(Event::Char(' ')).toggled, Some(1));
        key(&mut bar, Key::End);
        assert_eq!(bar.focus(), Some(3));
        assert!(!key(&mut bar, Key::Right).redraw, "already at the end");
        key(&mut bar, Key::Home);
        assert_eq!(bar.focus(), Some(0));
        assert_eq!(key(&mut bar, Key::Down), ChipResponse::default());
        let ctrl = Modifiers { ctrl: true, ..NONE };
        assert_eq!(
            bar.handle(Event::KeyDown(Key::Right, ctrl)),
            ChipResponse::default()
        );
    }

    #[test]
    fn active_and_inactive_chips_use_their_colors() {
        let bar = arranged();
        let theme = Theme::CLASSIC;
        let mut canvas = RecordingCanvas::new();
        bar.draw(&mut canvas, BAR, &chips(), &theme, false);
        assert!(canvas.clips_balance());
        let all = bar.chip_rect(0).unwrap();
        assert!(canvas
            .ops
            .contains(&DrawOp::Fill(all, theme.chip_active_background)));
        let err = bar.chip_rect(1).unwrap();
        assert!(canvas
            .ops
            .contains(&DrawOp::Fill(grow(err, -1.0), theme.chip_background)));
        let text_color = |wanted: &str| {
            canvas.ops.iter().find_map(|op| match op {
                DrawOp::Text(text, _, _, color) if text == wanted => Some(*color),
                _ => None,
            })
        };
        assert_eq!(text_color("All"), Some(theme.chip_active_text));
        assert_eq!(text_color("1,769"), Some(theme.chip_active_text));
        assert_eq!(text_color("Err"), Some(RED));
        assert_eq!(text_color("186"), Some(RED));
        assert_eq!(text_color("1.2M"), Some(theme.primary_text));
    }

    #[test]
    fn focus_draws_an_outline_only_with_focus() {
        let mut bar = arranged();
        key(&mut bar, Key::Right);
        // Console's active chips aren't the accent color, so only the outline is.
        let theme = Theme::CONSOLE;
        let accent_fills = |focused: bool| {
            let mut canvas = RecordingCanvas::new();
            bar.draw(&mut canvas, BAR, &chips(), &theme, focused);
            canvas
                .ops
                .iter()
                .filter(|op| matches!(op, DrawOp::Fill(_, color) if *color == theme.accent))
                .count()
        };
        assert_eq!(accent_fills(true), 4);
        assert_eq!(accent_fills(false), 0);
    }

    #[test]
    fn an_empty_bar_is_safe() {
        let mut bar = ChipBar::default();
        bar.arrange(BAR, &[], &mut RecordingCanvas::new());
        assert_eq!(bar.focus(), None);
        assert_eq!(key(&mut bar, Key::Enter), ChipResponse::default());
        assert_eq!(key(&mut bar, Key::Right), ChipResponse::default());
        assert_eq!(bar.handle(Event::Char(' ')), ChipResponse::default());
        let mut canvas = RecordingCanvas::new();
        bar.draw(&mut canvas, BAR, &[], &Theme::CLASSIC, true);
        assert!(canvas.clips_balance());
        assert!(canvas.texts().is_empty());
    }
}
