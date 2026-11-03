//! A scrolling list with section headings, for the sidebar's "OPEN", "NEARBY",
//! "WINDOWS" and "RECENT" sections. Every row is the same height and only the
//! rows on screen are drawn, so a list of thousands of entries stays cheap.

use crate::canvas::{Align, Canvas, FontRole, TextStyle};
use crate::geometry::{Color, Point, Rect};
use crate::input::{Event, Key, MouseButton};
use crate::scrollbar::{self, Press, ScrollState};
use crate::theme::Theme;
use std::borrow::Cow;
use std::ops::Range;

pub const ROW_HEIGHT: f32 = 22.0;
const HEADING_INDENT: f32 = 10.0;
const ENTRY_INDENT: f32 = 18.0;
const RIGHT_PADDING: f32 = 8.0;
const DETAIL_GAP: f32 = 8.0;
const SWATCH_SIZE: f32 = 12.0;
/// A detail with less room than this isn't drawn at all.
const MIN_DETAIL_WIDTH: f32 = 24.0;
const ELLIPSIS: &str = "\u{2026}";
const UI_LEFT: TextStyle = TextStyle::new(FontRole::Ui, Align::Left);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListItem {
    /// A section title, such as "RECENT". It can't be selected.
    Heading(String),
    Entry {
        label: String,
        /// Dimmer text after the label, such as a folder. Empty for none.
        detail: String,
        /// The app's own identifier, which comes back when the entry is activated.
        id: u32,
        /// A square of color before the label, such as a highlight rule's.
        swatch: Option<Color>,
    },
}

impl ListItem {
    pub fn heading(title: &str) -> Self {
        ListItem::Heading(title.to_string())
    }

    pub fn entry(label: &str, detail: &str, id: u32) -> Self {
        ListItem::Entry {
            label: label.to_string(),
            detail: detail.to_string(),
            id,
            swatch: None,
        }
    }

    /// The same entry with a square of color before its label.
    pub fn with_swatch(mut self, color: Color) -> Self {
        if let ListItem::Entry { swatch, .. } = &mut self {
            *swatch = Some(color);
        }
        self
    }

    /// The entry's identifier, or `None` for a heading.
    pub fn id(&self) -> Option<u32> {
        match self {
            ListItem::Entry { id, .. } => Some(*id),
            ListItem::Heading(_) => None,
        }
    }
}

/// What a list needs its owner to do after handling input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListResponse {
    pub redraw: bool,
    /// An entry was clicked, or Enter was pressed on it.
    pub activated: Option<u32>,
}

impl ListResponse {
    const REDRAW: ListResponse = ListResponse {
        redraw: true,
        activated: None,
    };
}

#[derive(Clone, Debug, Default)]
pub struct ListView {
    pub items: Vec<ListItem>,
    /// Indexes into `items`, always of entries.
    selected: Option<usize>,
    hovered: Option<usize>,
    /// The first row on screen. Fractional wheel movement builds up here.
    top: f64,
    area: Rect,
    body: Rect,
    /// The scroll bar's track, which is empty when everything fits.
    track: Rect,
    /// Where the thumb was grabbed, while it's being dragged.
    grab: Option<f32>,
    /// The last mouse position, to keep hover right when rows scroll under it.
    pointer: Option<Point>,
}

/// Shortens `text` to fit `width`, ending it with an ellipsis. Text that fits
/// comes back unchanged, and if even the ellipsis doesn't fit, it's empty.
pub fn elide<'a>(
    canvas: &mut dyn Canvas,
    text: &'a str,
    width: f32,
    font: FontRole,
) -> Cow<'a, str> {
    fit(canvas, text, width, font).0
}

/// Elides like [`elide`], and also returns how wide the result draws.
fn fit<'a>(
    canvas: &mut dyn Canvas,
    text: &'a str,
    width: f32,
    font: FontRole,
) -> (Cow<'a, str>, f32) {
    let full = canvas.text_width(text, font);
    if full <= width || text.is_empty() {
        return (Cow::Borrowed(text), full);
    }
    let ellipsis = canvas.text_width(ELLIPSIS, font);
    if ellipsis > width {
        return (Cow::Borrowed(""), 0.0);
    }
    let count = text.chars().count();
    let prefix = |chars: usize| {
        text.char_indices()
            .nth(chars)
            .map_or(text, |(index, _)| &text[..index])
    };
    // Start from a proportional guess and step to the exact fit, which takes a
    // few measurements rather than one per character.
    let mut keep = ((count as f32 * (width - ellipsis) / full) as usize).min(count - 1);
    let mut used = canvas.text_width(prefix(keep), font);
    while keep > 0 && used + ellipsis > width {
        keep -= 1;
        used = canvas.text_width(prefix(keep), font);
    }
    while keep + 1 < count {
        let next = canvas.text_width(prefix(keep + 1), font);
        if next + ellipsis > width {
            break;
        }
        keep += 1;
        used = next;
    }
    let kept = prefix(keep).trim_end();
    (Cow::Owned(format!("{kept}{ELLIPSIS}")), used + ellipsis)
}

impl ListView {
    pub fn new(items: Vec<ListItem>) -> Self {
        Self {
            items,
            ..Self::default()
        }
    }

    /// Lays out the rows and the scroll bar, which only shows when the items
    /// don't fit. Call before drawing or handling input, and after changing `items`.
    pub fn arrange(&mut self, area: Rect) {
        self.area = area;
        self.body = area;
        self.track = Rect::default();
        if self.items.len() > self.page_rows() {
            self.track = self.body.take_right(scrollbar::WIDTH);
        }
        // The items may have changed since the last frame.
        self.selected = self.selected.filter(|&index| self.is_entry(index));
        self.hovered = self.hovered.filter(|&index| self.is_entry(index));
        self.clamp_top();
    }

    /// How many whole rows fit, which is what a page moves by.
    pub fn page_rows(&self) -> usize {
        ((self.body.height() / ROW_HEIGHT) as usize).max(1)
    }

    pub fn top_item(&self) -> usize {
        self.top as usize
    }

    /// The items on screen, including a partly visible last one.
    pub fn visible_items(&self) -> Range<usize> {
        let first = self.top_item().min(self.items.len());
        let count = (self.body.height() / ROW_HEIGHT).ceil() as usize;
        first..(first + count).min(self.items.len())
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn selected_id(&self) -> Option<u32> {
        self.selected
            .and_then(|index| self.items.get(index))
            .and_then(ListItem::id)
    }

    /// Selects the first entry with this identifier, or clears the selection.
    /// It doesn't scroll, so the app can call it every frame.
    pub fn select_id(&mut self, id: Option<u32>) {
        self.selected = id.and_then(|id| self.items.iter().position(|item| item.id() == Some(id)));
    }

    pub fn hovered(&self) -> Option<usize> {
        self.hovered
    }

    /// Where the rows are, not counting the scroll bar.
    pub fn body_rect(&self) -> Rect {
        self.body
    }

    pub fn contains(&self, point: Point) -> bool {
        self.area.contains(point)
    }

    /// The rectangle of an item on screen, or `None` when it's scrolled away.
    pub fn item_rect(&self, index: usize) -> Option<Rect> {
        if !self.visible_items().contains(&index) {
            return None;
        }
        let top = self.body.top + (index - self.top_item()) as f32 * ROW_HEIGHT;
        Some(Rect::new(
            self.body.left,
            top,
            self.body.right,
            top + ROW_HEIGHT,
        ))
    }

    fn is_entry(&self, index: usize) -> bool {
        matches!(self.items.get(index), Some(ListItem::Entry { .. }))
    }

    fn max_top(&self) -> f64 {
        self.items.len().saturating_sub(self.page_rows()) as f64
    }

    fn clamp_top(&mut self) {
        self.top = self.top.clamp(0.0, self.max_top());
    }

    fn scroll_state(&self) -> ScrollState {
        ScrollState {
            total: self.items.len() as f64,
            visible: self.page_rows() as f64,
            top: self.top,
        }
    }

    /// Scrolls just far enough to bring an item on screen, along with the
    /// heading right above it when scrolling up.
    pub fn reveal(&mut self, index: usize) {
        let page = self.page_rows();
        let top = self.top_item();
        let heading_above =
            index > 0 && matches!(self.items.get(index - 1), Some(ListItem::Heading(_)));
        let first = if heading_above && page > 1 {
            index - 1
        } else {
            index
        };
        if first < top {
            self.top = first as f64;
        } else if index >= top + page {
            self.top = (index + 1 - page) as f64;
        }
        self.clamp_top();
    }

    /// The nearest entry at `index` or beyond it going one way, or failing
    /// that, going the other way.
    fn entry_near(&self, index: usize, forward: bool) -> Option<usize> {
        let last = self.items.len().checked_sub(1)?;
        let index = index.min(last);
        let after = || (index..=last).find(|&item| self.is_entry(item));
        let before = || (0..=index).rev().find(|&item| self.is_entry(item));
        if forward {
            after().or_else(before)
        } else {
            before().or_else(after)
        }
    }

    fn item_at(&self, point: Point) -> Option<usize> {
        if !self.body.contains(point) {
            return None;
        }
        let index = self.top_item() + ((point.y - self.body.top) / ROW_HEIGHT) as usize;
        (index < self.items.len()).then_some(index)
    }

    fn entry_at(&self, point: Point) -> Option<usize> {
        self.item_at(point).filter(|&index| self.is_entry(index))
    }

    fn move_to(&mut self, target: Option<usize>) -> ListResponse {
        let Some(index) = target else {
            return ListResponse::default();
        };
        let before = (self.selected, self.top);
        self.selected = Some(index);
        self.reveal(index);
        ListResponse {
            redraw: (self.selected, self.top) != before,
            activated: None,
        }
    }

    fn handle_key(&mut self, key: Key) -> ListResponse {
        let page = self.page_rows();
        let target = match (key, self.selected) {
            (Key::Enter, Some(index)) => {
                return ListResponse {
                    redraw: false,
                    activated: self.items.get(index).and_then(ListItem::id),
                };
            }
            (Key::Home, _) => self.entry_near(0, true),
            (Key::End, _) => self.entry_near(usize::MAX, false),
            // With nothing selected, any move starts at the top of the screen.
            (Key::Up | Key::Down | Key::PageUp | Key::PageDown, None) => {
                self.entry_near(self.top_item(), true)
            }
            (Key::Up, Some(index)) => self.entry_near(index.saturating_sub(1), false),
            (Key::Down, Some(index)) => self.entry_near(index + 1, true),
            (Key::PageUp, Some(index)) => self.entry_near(index.saturating_sub(page), false),
            (Key::PageDown, Some(index)) => self.entry_near(index + page, true),
            _ => return ListResponse::default(),
        };
        self.move_to(target)
    }

    fn handle_press(&mut self, point: Point) -> ListResponse {
        if self.track.contains(point) {
            let state = self.scroll_state();
            let page = self.page_rows() as f64;
            match scrollbar::press_at(&self.track, &state, point) {
                Press::Thumb { grab } => self.grab = Some(grab),
                Press::Before => self.top -= page,
                Press::After => self.top += page,
            }
            self.clamp_top();
            return ListResponse::REDRAW;
        }
        match self.entry_at(point) {
            Some(index) => {
                self.selected = Some(index);
                ListResponse {
                    redraw: true,
                    activated: self.items[index].id(),
                }
            }
            None => ListResponse::default(),
        }
    }

    fn handle_move(&mut self, point: Point) -> ListResponse {
        self.pointer = Some(point);
        if let Some(grab) = self.grab {
            let state = self.scroll_state();
            self.top = scrollbar::drag_position(&self.track, &state, point.y, grab);
            self.clamp_top();
            return ListResponse::default();
        }
        let hovered = self.entry_at(point);
        if hovered == self.hovered {
            return ListResponse::default();
        }
        self.hovered = hovered;
        ListResponse::REDRAW
    }

    /// Handles input. The app sends keys while the list has focus, and the
    /// wheel while the mouse is over it, since wheel events have no position.
    /// Send every mouse move, so hover clears when the mouse leaves.
    pub fn handle(&mut self, event: Event) -> ListResponse {
        let top = self.top;
        let mut response = match event {
            Event::KeyDown(_, modifiers) if modifiers.alt => ListResponse::default(),
            Event::KeyDown(key, _) => self.handle_key(key),
            Event::MouseDown(point, MouseButton::Left, _) => self.handle_press(point),
            Event::MouseUp(_, MouseButton::Left) => {
                self.grab = None;
                ListResponse::default()
            }
            Event::MouseMove(point) => self.handle_move(point),
            Event::Wheel(rows) => {
                self.top -= f64::from(rows);
                self.clamp_top();
                ListResponse::default()
            }
            _ => ListResponse::default(),
        };
        if self.top != top {
            // The rows moved under a still mouse.
            self.hovered = self.pointer.and_then(|point| self.entry_at(point));
            response.redraw = true;
        }
        response
    }

    /// Draws the list. Without focus, the selected entry is drawn in a quieter
    /// color, as Windows does.
    pub fn draw(&self, canvas: &mut dyn Canvas, theme: &Theme, focused: bool) {
        canvas.fill_rect(self.area, theme.sidebar_background);
        canvas.push_clip(self.body);
        for index in self.visible_items() {
            let Some(rect) = self.item_rect(index) else {
                continue;
            };
            match &self.items[index] {
                ListItem::Heading(title) => {
                    let text = Rect::new(
                        rect.left + HEADING_INDENT,
                        rect.top,
                        rect.right - RIGHT_PADDING,
                        rect.bottom,
                    );
                    let (title, _) = fit(canvas, title, text.width(), FontRole::Ui);
                    canvas.text(&title, text, UI_LEFT, theme.section_text);
                }
                ListItem::Entry { .. } => self.draw_entry(canvas, theme, focused, index, rect),
            }
        }
        canvas.pop_clip();
        if self.track.width() > 0.0 {
            canvas.fill_rect(self.track, theme.scrollbar_track);
            let thumb = scrollbar::thumb_rect(&self.track, &self.scroll_state());
            canvas.fill_rect(thumb, theme.scrollbar_thumb);
        }
    }

    fn draw_entry(
        &self,
        canvas: &mut dyn Canvas,
        theme: &Theme,
        focused: bool,
        index: usize,
        rect: Rect,
    ) {
        let ListItem::Entry {
            label,
            detail,
            swatch,
            ..
        } = &self.items[index]
        else {
            return;
        };
        let selected = self.selected == Some(index);
        let (background, label_color, detail_color) = if selected && focused {
            (
                Some(theme.selected_row),
                theme.selected_text,
                theme.selected_text,
            )
        } else if selected || self.hovered == Some(index) {
            (
                Some(theme.hover_background),
                theme.on_highlight(theme.primary_text),
                theme.on_highlight(theme.secondary_text),
            )
        } else {
            (None, theme.primary_text, theme.secondary_text)
        };
        if let Some(color) = background {
            canvas.fill_rect(rect, color);
        }
        let mut text = Rect::new(
            rect.left + ENTRY_INDENT,
            rect.top,
            rect.right - RIGHT_PADDING,
            rect.bottom,
        );
        if let Some(color) = swatch {
            let middle = (rect.top + rect.bottom) / 2.0;
            let half = SWATCH_SIZE / 2.0;
            let outline = Rect::new(
                text.left,
                middle - half,
                text.left + SWATCH_SIZE,
                middle + half,
            );
            // Outlined, so a color close to the list's own still shows.
            canvas.fill_rect(outline, detail_color);
            canvas.fill_rect(
                Rect::new(
                    outline.left + 1.0,
                    outline.top + 1.0,
                    outline.right - 1.0,
                    outline.bottom - 1.0,
                ),
                *color,
            );
            text.left += SWATCH_SIZE + DETAIL_GAP;
        }
        let (label, label_width) = fit(canvas, label, text.width(), FontRole::Ui);
        canvas.text(&label, text, UI_LEFT, label_color);
        let detail_left = text.left + label_width + DETAIL_GAP;
        if !detail.is_empty() && text.right - detail_left >= MIN_DETAIL_WIDTH {
            let room = Rect::new(detail_left, rect.top, text.right, rect.bottom);
            let (detail, _) = fit(canvas, detail, room.width(), FontRole::Ui);
            canvas.text(&detail, room, UI_LEFT, detail_color);
        }
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
    /// Tall enough for every sample item.
    const AREA: Rect = Rect::new(0.0, 0.0, 200.0, ROW_HEIGHT * 10.0);
    /// Four rows tall, so the sample items scroll.
    const SHORT: Rect = Rect::new(0.0, 0.0, 200.0, ROW_HEIGHT * 4.0);

    fn sample() -> Vec<ListItem> {
        vec![
            ListItem::heading("OPEN"),
            ListItem::entry("PolicyAgent.log", "", 1),
            ListItem::entry("ccmexec.log", "", 2),
            ListItem::heading("NEARBY"),
            ListItem::heading("WINDOWS"),
            ListItem::entry("CCM", "C:\\Windows\\CCM\\Logs", 3),
            ListItem::entry("CBS", "C:\\Windows\\Logs\\CBS", 4),
            ListItem::heading("RECENT"),
            ListItem::entry("setupact.log", "C:\\Windows\\Panther", 5),
        ]
    }

    fn list(area: Rect) -> ListView {
        let mut list = ListView::new(sample());
        list.arrange(area);
        list
    }

    fn key(list: &mut ListView, key: Key) -> ListResponse {
        list.handle(Event::KeyDown(key, NONE))
    }

    fn click(list: &mut ListView, point: Point) -> ListResponse {
        list.handle(Event::MouseDown(point, MouseButton::Left, NONE))
    }

    fn center(rect: Rect) -> Point {
        Point::new(
            (rect.left + rect.right) / 2.0,
            (rect.top + rect.bottom) / 2.0,
        )
    }

    fn drawn(list: &ListView, focused: bool) -> RecordingCanvas {
        let mut canvas = RecordingCanvas::new();
        list.draw(&mut canvas, &Theme::CLASSIC, focused);
        assert!(canvas.clips_balance());
        canvas
    }

    #[test]
    fn rows_stack_and_only_overflow_gets_a_scroll_bar() {
        let tall = list(AREA);
        assert_eq!(
            tall.item_rect(0),
            Some(Rect::new(0.0, 0.0, 200.0, ROW_HEIGHT))
        );
        assert_eq!(
            tall.item_rect(8),
            Some(Rect::new(0.0, ROW_HEIGHT * 8.0, 200.0, ROW_HEIGHT * 9.0))
        );
        assert_eq!(tall.body_rect().right, 200.0);
        let short = list(SHORT);
        assert_eq!(short.body_rect().right, 200.0 - scrollbar::WIDTH);
        assert_eq!(short.visible_items(), 0..4);
        assert_eq!(short.item_rect(4), None);
    }

    #[test]
    fn arrows_skip_headings() {
        let mut list = list(AREA);
        key(&mut list, Key::Down);
        assert_eq!(list.selected(), Some(1), "starts at the first entry");
        key(&mut list, Key::Down);
        assert_eq!(list.selected(), Some(2));
        assert!(key(&mut list, Key::Down).redraw);
        assert_eq!(list.selected(), Some(5), "skips two headings");
        key(&mut list, Key::Up);
        assert_eq!(list.selected(), Some(2));
        key(&mut list, Key::End);
        assert_eq!(list.selected(), Some(8));
        assert!(!key(&mut list, Key::Down).redraw, "stays on the last entry");
        assert_eq!(list.selected(), Some(8));
        key(&mut list, Key::Home);
        assert_eq!(list.selected(), Some(1));
        assert!(!key(&mut list, Key::Up).redraw, "stays on the first entry");
        assert_eq!(list.selected(), Some(1));
    }

    #[test]
    fn page_keys_land_on_entries() {
        let mut list = list(SHORT);
        assert_eq!(list.page_rows(), 4);
        key(&mut list, Key::Home);
        key(&mut list, Key::PageDown);
        assert_eq!(list.selected(), Some(5));
        key(&mut list, Key::PageDown);
        assert_eq!(list.selected(), Some(8), "stops at the last entry");
        key(&mut list, Key::PageUp);
        assert_eq!(list.selected(), Some(2), "lands past the headings");
    }

    #[test]
    fn keys_scroll_the_selection_and_its_heading_into_view() {
        let mut list = list(SHORT);
        key(&mut list, Key::End);
        assert_eq!(list.visible_items(), 5..9);
        key(&mut list, Key::Up);
        assert_eq!(list.selected(), Some(6));
        assert_eq!(list.top_item(), 5, "already on screen");
        key(&mut list, Key::Up);
        assert_eq!(list.selected(), Some(5));
        assert_eq!(list.top_item(), 4, "shows the WINDOWS heading too");
        key(&mut list, Key::Home);
        assert_eq!(list.top_item(), 0);
    }

    #[test]
    fn enter_activates_the_selected_entry() {
        let mut list = list(AREA);
        assert_eq!(key(&mut list, Key::Enter), ListResponse::default());
        key(&mut list, Key::Down);
        assert_eq!(key(&mut list, Key::Enter).activated, Some(1));
    }

    #[test]
    fn keys_with_alt_and_unused_keys_pass_through() {
        let mut list = list(AREA);
        let alt = Modifiers { alt: true, ..NONE };
        assert_eq!(
            list.handle(Event::KeyDown(Key::Down, alt)),
            ListResponse::default()
        );
        assert_eq!(key(&mut list, Key::F(3)), ListResponse::default());
        assert_eq!(list.selected(), None);
    }

    #[test]
    fn clicking_an_entry_selects_and_activates_it() {
        let mut list = list(AREA);
        let entry = center(list.item_rect(2).unwrap());
        let response = click(&mut list, entry);
        assert_eq!(response.activated, Some(2));
        assert!(response.redraw);
        assert_eq!(list.selected(), Some(2));
        assert_eq!(list.selected_id(), Some(2));
        let heading = center(list.item_rect(0).unwrap());
        assert_eq!(click(&mut list, heading), ListResponse::default());
        assert_eq!(list.selected(), Some(2), "a heading can't be selected");
        let below = Point::new(100.0, ROW_HEIGHT * 9.5);
        assert_eq!(click(&mut list, below), ListResponse::default());
        let right_click =
            Event::MouseDown(center(list.item_rect(5).unwrap()), MouseButton::Right, NONE);
        assert_eq!(list.handle(right_click), ListResponse::default());
    }

    #[test]
    fn hover_follows_the_mouse_and_skips_headings() {
        let mut list = list(AREA);
        let entry = center(list.item_rect(1).unwrap());
        assert!(list.handle(Event::MouseMove(entry)).redraw);
        assert_eq!(list.hovered(), Some(1));
        let nearby = Point::new(entry.x + 5.0, entry.y);
        assert!(!list.handle(Event::MouseMove(nearby)).redraw);
        let heading = center(list.item_rect(0).unwrap());
        assert!(list.handle(Event::MouseMove(heading)).redraw);
        assert_eq!(list.hovered(), None);
        list.handle(Event::MouseMove(entry));
        list.handle(Event::MouseMove(Point::new(500.0, 500.0)));
        assert_eq!(list.hovered(), None, "leaving the list clears hover");
    }

    #[test]
    fn wheel_scrolls_within_limits() {
        let mut list = list(SHORT);
        assert!(list.handle(Event::Wheel(-3.0)).redraw);
        assert_eq!(list.top_item(), 3);
        list.handle(Event::Wheel(-10.0));
        assert_eq!(list.top_item(), 5, "stops with the last item at the bottom");
        list.handle(Event::Wheel(100.0));
        assert_eq!(list.top_item(), 0);
        assert!(!list.handle(Event::Wheel(1.0)).redraw, "already at the top");

        let mut tall = self::list(AREA);
        assert!(!tall.handle(Event::Wheel(-3.0)).redraw, "nothing to scroll");
        assert_eq!(tall.top_item(), 0);
    }

    #[test]
    fn hover_follows_rows_that_scroll_under_the_mouse() {
        let mut list = list(SHORT);
        list.handle(Event::MouseMove(Point::new(50.0, ROW_HEIGHT * 1.5)));
        assert_eq!(list.hovered(), Some(1));
        list.handle(Event::Wheel(-1.0));
        assert_eq!(list.hovered(), Some(2));
        list.handle(Event::Wheel(-1.0));
        assert_eq!(list.hovered(), None, "a heading is under the mouse now");
    }

    #[test]
    fn the_scroll_bar_pages_and_drags() {
        let mut list = list(SHORT);
        let below_thumb = Point::new(list.track.left + 5.0, list.track.bottom - 1.0);
        assert!(click(&mut list, below_thumb).redraw);
        assert_eq!(list.top_item(), 4);

        let thumb = scrollbar::thumb_rect(&list.track, &list.scroll_state());
        click(&mut list, center(thumb));
        assert!(
            list.handle(Event::MouseMove(Point::new(190.0, 1000.0)))
                .redraw
        );
        assert_eq!(list.top_item(), 5);
        list.handle(Event::MouseMove(Point::new(190.0, -1000.0)));
        assert_eq!(list.top_item(), 0);
        list.handle(Event::MouseUp(Point::new(190.0, 0.0), MouseButton::Left));
        list.handle(Event::MouseMove(Point::new(190.0, 1000.0)));
        assert_eq!(list.top_item(), 0, "the drag ended");
        assert_eq!(list.selected(), None, "the scroll bar doesn't select");
    }

    #[test]
    fn huge_lists_draw_only_what_is_visible() {
        let items = (0..100_000u32)
            .map(|index| {
                if index % 100 == 0 {
                    ListItem::heading(&format!("SECTION {index}"))
                } else {
                    ListItem::entry(&format!("file{index}.log"), "", index)
                }
            })
            .collect();
        let mut list = ListView::new(items);
        list.arrange(SHORT);
        key(&mut list, Key::End);
        assert_eq!(list.selected_id(), Some(99_999));
        assert_eq!(list.visible_items(), 99_996..100_000);
        let canvas = drawn(&list, true);
        assert_eq!(
            canvas.texts(),
            vec![
                "file99996.log",
                "file99997.log",
                "file99998.log",
                "file99999.log"
            ]
        );
        key(&mut list, Key::PageUp);
        assert_eq!(list.selected(), Some(99_995));
    }

    #[test]
    fn an_empty_list_is_safe() {
        let mut list = ListView::new(Vec::new());
        list.arrange(AREA);
        for each in [
            Key::Up,
            Key::Down,
            Key::PageUp,
            Key::PageDown,
            Key::Home,
            Key::End,
            Key::Enter,
        ] {
            assert_eq!(key(&mut list, each), ListResponse::default());
        }
        assert_eq!(click(&mut list, center(AREA)), ListResponse::default());
        assert!(!list.handle(Event::Wheel(-3.0)).redraw);
        assert!(!list.handle(Event::MouseMove(center(AREA))).redraw);
        assert!(drawn(&list, true).texts().is_empty());
    }

    #[test]
    fn long_text_is_elided_to_fit() {
        let mut list = ListView::new(vec![ListItem::entry(&"x".repeat(100), "C:\\Windows", 1)]);
        list.arrange(Rect::new(0.0, 0.0, 100.0, ROW_HEIGHT * 2.0));
        let canvas = drawn(&list, true);
        // 74 wide leaves room for nine characters and the ellipsis, and none
        // for the detail.
        assert_eq!(canvas.texts(), vec!["xxxxxxxxx\u{2026}"]);
    }

    #[test]
    fn details_follow_the_label_in_a_dimmer_color() {
        let list = list(AREA);
        let canvas = drawn(&list, true);
        let detail = canvas.ops.iter().find_map(|op| match op {
            DrawOp::Text(text, rect, _, color) if text == "C:\\Windows\\CCM\\Logs" => {
                Some((*rect, *color))
            }
            _ => None,
        });
        let (rect, color) = detail.expect("the detail is drawn");
        assert_eq!(rect.left, ENTRY_INDENT + 3.0 * 7.0 + DETAIL_GAP);
        assert_eq!(color, Theme::CLASSIC.secondary_text);
        let headings: Vec<_> = canvas
            .ops
            .iter()
            .filter_map(|op| match op {
                DrawOp::Text(text, _, _, color) if *color == Theme::CLASSIC.section_text => {
                    Some(text.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(headings, vec!["OPEN", "NEARBY", "WINDOWS", "RECENT"]);
    }

    #[test]
    fn the_selection_is_quieter_without_focus() {
        let mut list = list(AREA);
        key(&mut list, Key::Down);
        let row = list.item_rect(1).unwrap();
        let theme = Theme::CLASSIC;
        let focused = drawn(&list, true);
        assert!(focused.ops.contains(&DrawOp::Fill(row, theme.selected_row)));
        let unfocused = drawn(&list, false);
        assert!(unfocused
            .ops
            .contains(&DrawOp::Fill(row, theme.hover_background)));
        assert!(!unfocused
            .ops
            .contains(&DrawOp::Fill(row, theme.selected_row)));
    }

    #[test]
    fn changing_items_drops_a_stale_selection() {
        let mut list = list(AREA);
        key(&mut list, Key::End);
        list.items.truncate(3);
        list.arrange(AREA);
        assert_eq!(list.selected(), None);
        list.select_id(Some(2));
        assert_eq!(list.selected(), Some(2));
        assert_eq!(list.selected_id(), Some(2));
        list.select_id(Some(99));
        assert_eq!(list.selected(), None);
    }

    #[test]
    fn elide_keeps_text_that_fits() {
        let mut canvas = RecordingCanvas::new();
        let text = elide(&mut canvas, "abc", 21.0, FontRole::Ui);
        assert!(matches!(text, Cow::Borrowed("abc")));
    }

    #[test]
    fn elide_shortens_with_an_ellipsis() {
        let mut canvas = RecordingCanvas::new();
        // Each character is 7 wide, so six and the ellipsis make 49.
        assert_eq!(
            elide(&mut canvas, "abcdefghij", 50.0, FontRole::Ui),
            "abcdef\u{2026}"
        );
        assert_eq!(
            elide(&mut canvas, "abc   defghij", 50.0, FontRole::Ui),
            "abc\u{2026}",
            "no spaces before the ellipsis"
        );
        assert_eq!(
            elide(
                &mut canvas,
                "\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}",
                30.0,
                FontRole::Ui
            ),
            "\u{e9}\u{e9}\u{e9}\u{2026}",
            "cuts between whole characters"
        );
        assert_eq!(elide(&mut canvas, "abcdef", 8.0, FontRole::Ui), "\u{2026}");
        assert_eq!(elide(&mut canvas, "abcdef", 5.0, FontRole::Ui), "");
    }
}
