//! A tab strip with close buttons and a live-tail dot.

use crate::canvas::{Align, Canvas, FontRole, TextStyle};
use crate::geometry::{Point, Rect};
use crate::input::{Event, MouseButton};
use crate::list_view::elide;
use crate::theme::Theme;

pub const BAR_HEIGHT: f32 = 28.0;
const PADDING: f32 = 12.0;
const CLOSE_SIZE: f32 = 16.0;
const DOT_SIZE: f32 = 6.0;
const MIN_WIDTH: f32 = 90.0;
const MAX_WIDTH: f32 = 260.0;
const UI_LEFT: TextStyle = TextStyle::new(FontRole::Ui, Align::Left);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tab {
    pub title: String,
    /// Drawn after the title, such as the machine a log is on: " (SRV01)".
    /// When the tab is too narrow for both, the title is shortened and this
    /// stays whole.
    pub suffix: String,
    /// Shows a dot, for a log being followed live.
    pub live: bool,
}

impl Tab {
    fn text(&self) -> String {
        format!("{}{}", self.title, self.suffix)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TabResponse {
    pub redraw: bool,
    pub select: Option<usize>,
    pub close: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct TabBar {
    tab_rects: Vec<Rect>,
    close_rects: Vec<Rect>,
    hovered_close: Option<usize>,
}

impl TabBar {
    pub fn arrange(&mut self, bar: Rect, tabs: &[Tab], canvas: &mut dyn Canvas) {
        self.tab_rects.clear();
        self.close_rects.clear();
        let mut widths: Vec<f32> = tabs
            .iter()
            .map(|tab| {
                let text = canvas.text_width(&tab.text(), FontRole::Ui);
                (text + PADDING * 3.0 + CLOSE_SIZE + DOT_SIZE).clamp(MIN_WIDTH, MAX_WIDTH)
            })
            .collect();
        shrink_to_fit(&mut widths, bar.width());
        let mut left = bar.left;
        for width in widths {
            let rect = Rect::new(left, bar.top, left + width, bar.bottom);
            let middle = (rect.top + rect.bottom) / 2.0;
            self.close_rects.push(Rect::new(
                rect.right - PADDING / 2.0 - CLOSE_SIZE,
                middle - CLOSE_SIZE / 2.0,
                rect.right - PADDING / 2.0,
                middle + CLOSE_SIZE / 2.0,
            ));
            self.tab_rects.push(rect);
            left += width;
        }
    }

    pub fn tab_rect(&self, index: usize) -> Option<Rect> {
        self.tab_rects.get(index).copied()
    }

    pub fn close_rect(&self, index: usize) -> Option<Rect> {
        self.close_rects.get(index).copied()
    }

    pub fn contains(&self, point: Point) -> bool {
        self.tab_rects.iter().any(|rect| rect.contains(point))
    }

    pub fn handle(&mut self, event: Event) -> TabResponse {
        match event {
            Event::MouseDown(point, MouseButton::Left, _) => {
                if let Some(index) = self
                    .close_rects
                    .iter()
                    .position(|rect| rect.contains(point))
                {
                    return TabResponse {
                        redraw: true,
                        close: Some(index),
                        ..TabResponse::default()
                    };
                }
                match self.tab_rects.iter().position(|rect| rect.contains(point)) {
                    Some(index) => TabResponse {
                        redraw: true,
                        select: Some(index),
                        ..TabResponse::default()
                    },
                    None => TabResponse::default(),
                }
            }
            // Middle-click closes a tab, as in browsers.
            Event::MouseDown(point, MouseButton::Middle, _) => {
                match self.tab_rects.iter().position(|rect| rect.contains(point)) {
                    Some(index) => TabResponse {
                        redraw: true,
                        close: Some(index),
                        ..TabResponse::default()
                    },
                    None => TabResponse::default(),
                }
            }
            Event::MouseMove(point) => {
                let hovered = self
                    .close_rects
                    .iter()
                    .position(|rect| rect.contains(point));
                if hovered != self.hovered_close {
                    self.hovered_close = hovered;
                    return TabResponse {
                        redraw: true,
                        ..TabResponse::default()
                    };
                }
                TabResponse::default()
            }
            _ => TabResponse::default(),
        }
    }

    pub fn draw(
        &self,
        canvas: &mut dyn Canvas,
        bar: Rect,
        tabs: &[Tab],
        active: usize,
        theme: &Theme,
    ) {
        canvas.fill_rect(bar, theme.panel_background);
        canvas.fill_rect(
            Rect::new(bar.left, bar.bottom - 1.0, bar.right, bar.bottom),
            theme.border,
        );
        for (index, (tab, rect)) in tabs.iter().zip(&self.tab_rects).enumerate() {
            let is_active = index == active;
            if is_active {
                canvas.fill_rect(*rect, theme.grid_background);
                canvas.fill_rect(
                    Rect::new(rect.left, rect.top, rect.right, rect.top + 2.0),
                    theme.accent,
                );
            }
            canvas.fill_rect(
                Rect::new(
                    rect.right - 1.0,
                    rect.top + 6.0,
                    rect.right,
                    rect.bottom - 6.0,
                ),
                theme.border,
            );
            let close = self.close_rects[index];
            let mut text_left = rect.left + PADDING;
            if tab.live {
                let middle = (rect.top + rect.bottom) / 2.0;
                canvas.fill_rect(
                    Rect::new(
                        text_left,
                        middle - DOT_SIZE / 2.0,
                        text_left + DOT_SIZE,
                        middle + DOT_SIZE / 2.0,
                    ),
                    theme.accent,
                );
                text_left += DOT_SIZE + 6.0;
            }
            let text_color = if is_active {
                theme.primary_text
            } else {
                theme.secondary_text
            };
            let label = Rect::new(text_left, rect.top, close.left - 4.0, rect.bottom);
            let title = fitted(canvas, tab, label.width());
            canvas.push_clip(label);
            canvas.text(&title, label, UI_LEFT, text_color);
            canvas.pop_clip();
            let close_color = if self.hovered_close == Some(index) {
                canvas.fill_rect(close, theme.hover_background);
                theme.on_highlight(theme.secondary_text)
            } else {
                theme.secondary_text
            };
            canvas.text(
                "\u{00D7}",
                Rect::new(close.left + 4.0, close.top, close.right, close.bottom),
                UI_LEFT,
                close_color,
            );
        }
    }
}

/// Narrows the widest tabs until every tab fits in `available`, as browsers
/// do, so none runs off the window's edge. Narrower tabs keep their width, and
/// none goes below the usual minimum, so a great many tabs can still overflow.
fn shrink_to_fit(widths: &mut [f32], available: f32) {
    if widths.iter().sum::<f32>() <= available {
        return;
    }
    // The widest a tab may be: the room left once the narrower tabs have
    // theirs, shared among the rest.
    let mut sorted = widths.to_vec();
    sorted.sort_by(f32::total_cmp);
    let (mut room, mut most) = (available, MIN_WIDTH);
    for (index, width) in sorted.iter().enumerate() {
        let share = room / (sorted.len() - index) as f32;
        if *width > share {
            most = share;
            break;
        }
        room -= width;
    }
    // Whole pixels keep the tabs' edges sharp.
    let most = most.floor().max(MIN_WIDTH);
    for width in widths {
        *width = width.min(most);
    }
}

/// The tab's title and suffix, shortened to fit `width` with an ellipsis. The
/// title gives way first, so the suffix stays whole while there's room for it.
fn fitted(canvas: &mut dyn Canvas, tab: &Tab, width: f32) -> String {
    let whole = tab.text();
    if tab.suffix.is_empty() || canvas.text_width(&whole, FontRole::Ui) <= width {
        return elide(canvas, &whole, width, FontRole::Ui).into_owned();
    }
    let room = width - canvas.text_width(&tab.suffix, FontRole::Ui);
    let title = elide(canvas, &tab.title, room, FontRole::Ui);
    if title.is_empty() {
        return elide(canvas, &whole, width, FontRole::Ui).into_owned();
    }
    format!("{title}{}", tab.suffix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Modifiers;
    use crate::recording::RecordingCanvas;

    const NONE: Modifiers = Modifiers {
        ctrl: false,
        shift: false,
        alt: false,
    };
    const BAR: Rect = Rect::new(0.0, 30.0, 1000.0, 30.0 + BAR_HEIGHT);

    fn tabs() -> Vec<Tab> {
        vec![
            Tab {
                title: "PolicyAgent.log".to_string(),
                live: true,
                ..Tab::default()
            },
            Tab {
                title: "a".to_string(),
                ..Tab::default()
            },
            Tab {
                title: "x".repeat(300),
                ..Tab::default()
            },
        ]
    }

    fn arranged() -> TabBar {
        let mut bar = TabBar::default();
        bar.arrange(BAR, &tabs(), &mut RecordingCanvas::new());
        bar
    }

    fn center(rect: Rect) -> Point {
        Point::new(
            (rect.left + rect.right) / 2.0,
            (rect.top + rect.bottom) / 2.0,
        )
    }

    #[test]
    fn widths_stay_within_limits() {
        let bar = arranged();
        assert_eq!(bar.tab_rect(1).unwrap().width(), MIN_WIDTH);
        assert_eq!(bar.tab_rect(2).unwrap().width(), MAX_WIDTH);
    }

    #[test]
    fn clicking_selects_and_the_close_button_closes() {
        let mut bar = arranged();
        let left_part = Point::new(bar.tab_rect(1).unwrap().left + 5.0, BAR.top + 10.0);
        assert_eq!(
            bar.handle(Event::MouseDown(left_part, MouseButton::Left, NONE))
                .select,
            Some(1)
        );
        let close = center(bar.close_rect(1).unwrap());
        let response = bar.handle(Event::MouseDown(close, MouseButton::Left, NONE));
        assert_eq!(response.close, Some(1));
        assert_eq!(response.select, None);
    }

    #[test]
    fn middle_click_closes() {
        let mut bar = arranged();
        let point = center(bar.tab_rect(0).unwrap());
        assert_eq!(
            bar.handle(Event::MouseDown(point, MouseButton::Middle, NONE))
                .close,
            Some(0)
        );
    }

    #[test]
    fn clicks_past_the_last_tab_do_nothing() {
        let mut bar = arranged();
        let response = bar.handle(Event::MouseDown(
            Point::new(990.0, BAR.top + 5.0),
            MouseButton::Left,
            NONE,
        ));
        assert_eq!(response, TabResponse::default());
    }

    #[test]
    fn titles_are_drawn_clipped() {
        let bar = arranged();
        let mut canvas = RecordingCanvas::new();
        bar.draw(&mut canvas, BAR, &tabs(), 0, &Theme::CLASSIC);
        assert!(canvas.texts().contains(&"PolicyAgent.log"));
        assert!(canvas.clips_balance());
    }

    #[test]
    fn a_title_too_long_for_its_tab_ends_in_an_ellipsis() {
        let bar = arranged();
        let mut canvas = RecordingCanvas::new();
        bar.draw(&mut canvas, BAR, &tabs(), 0, &Theme::CLASSIC);
        let long = canvas
            .texts()
            .into_iter()
            .find(|text| text.starts_with('x'))
            .unwrap()
            .to_string();
        assert!(long.ends_with('\u{2026}'), "{long}");
        assert!(long.len() < 300);
    }

    fn machine_tab(title: &str) -> Tab {
        Tab {
            title: title.to_string(),
            suffix: " (NYCLAP100234)".to_string(),
            live: true,
        }
    }

    #[test]
    fn a_shortened_title_keeps_its_machine_whole() {
        let tabs = [
            machine_tab("WUAHandler.log"),
            machine_tab("IntuneManagementExtension.log"),
        ];
        let mut bar = TabBar::default();
        let mut canvas = RecordingCanvas::new();
        bar.arrange(BAR, &tabs, &mut canvas);
        bar.draw(&mut canvas, BAR, &tabs, 0, &Theme::CLASSIC);
        let texts = canvas.texts();
        assert!(
            texts.contains(&"WUAHandler.log (NYCLAP100234)"),
            "{texts:?}"
        );
        let long = texts
            .iter()
            .find(|text| text.starts_with("Intune"))
            .unwrap();
        assert!(long.ends_with("\u{2026} (NYCLAP100234)"), "{long}");

        // With no room for the machine and some of the title, the end goes.
        let mut canvas = RecordingCanvas::new();
        let tab = machine_tab("WUAHandler.log");
        assert!(fitted(&mut canvas, &tab, 70.0).ends_with('\u{2026}'));
        assert_eq!(fitted(&mut canvas, &tab, 1000.0), tab.text());
    }

    #[test]
    fn tabs_too_wide_for_the_bar_shrink_the_widest_first() {
        let tabs: Vec<Tab> = ["a", "PolicyAgent.log", "IntuneManagementExtension.log"]
            .iter()
            .map(|title| machine_tab(title))
            .collect();
        // At their own widths they'd need 170 + 260 + 260 pixels.
        let narrow = Rect::new(0.0, 30.0, 600.0, 30.0 + BAR_HEIGHT);
        let mut bar = TabBar::default();
        bar.arrange(narrow, &tabs, &mut RecordingCanvas::new());
        let widths: Vec<f32> = (0..3).map(|i| bar.tab_rect(i).unwrap().width()).collect();
        // The first keeps its width, and the two wider share what's left.
        assert_eq!(widths, [170.0, 215.0, 215.0]);
        assert_eq!(bar.tab_rect(2).unwrap().right, 600.0);

        // No tab goes below the minimum, even if they then overflow.
        let mut widths = vec![200.0; 10];
        shrink_to_fit(&mut widths, 300.0);
        assert!(widths.iter().all(|width| *width == MIN_WIDTH));
    }
}
