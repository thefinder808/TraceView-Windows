//! A command palette, like Ctrl+Shift+P in VS Code: a text field over a list
//! that narrows as you type. Matching is fuzzy, so "opf" finds "Open File", and
//! the matched letters are drawn in the accent color.

use crate::canvas::{Align, Canvas, FontRole, TextStyle};
use crate::geometry::{Point, Rect};
use crate::input::{Event, Key, MouseButton};
use crate::list_view::elide;
use crate::text_field::TextField;
use crate::theme::Theme;
use std::ops::Range;

pub const ROW_HEIGHT: f32 = 24.0;
const WIDTH: f32 = 560.0;
/// The gap between the top of the area and the palette.
const OFFSET: f32 = 8.0;
/// The least space kept either side of the palette in a narrow window.
const MARGIN: f32 = 16.0;
const PADDING: f32 = 6.0;
const FIELD_HEIGHT: f32 = 26.0;
/// Where the text field starts its text, so the placeholder lines up with it.
const FIELD_TEXT_INSET: f32 = 6.0;
const MAX_ROWS: usize = 12;
const ROW_PADDING: f32 = 10.0;
const GAP: f32 = 16.0;
/// A detail with less room than this isn't drawn at all.
const MIN_DETAIL_WIDTH: f32 = 24.0;
const UI_LEFT: TextStyle = TextStyle::new(FontRole::Ui, Align::Left);
const UI_RIGHT: TextStyle = TextStyle::new(FontRole::Ui, Align::Right);

// Fuzzy match scoring. Every matched character scores, more at the start of a
// word or straight after the previous match. Gaps between matches cost a
// little, as do characters skipped before the first one.
const MATCH: i32 = 16;
const WORD_START: i32 = 16;
const CONSECUTIVE: i32 = 16;
const GAP_START: i32 = 3;
const GAP_EXTEND: i32 = 1;
const MAX_LEADING: usize = 10;
/// Past this many query-by-text pairs, the first match found is used rather
/// than searching for the best, so a pasted query can't stall typing.
const MAX_CELLS: usize = 50_000;

/// How a query matched some text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FuzzyMatch {
    /// Higher is better. Only comparable between matches of the same query.
    pub score: i32,
    /// The byte offset in the text of each matched character, in order.
    pub positions: Vec<usize>,
}

/// Matches `query` against `text` as a subsequence, ignoring case and any
/// spaces in the query. Returns `None` when some query character is missing.
/// It finds the best placement, where matches run together or start words,
/// as in "camelCase", "snake_case" or "two words". An empty query matches
/// anything, with a score of zero.
pub fn fuzzy_match(query: &str, text: &str) -> Option<FuzzyMatch> {
    let query: Vec<char> = query
        .chars()
        .filter(|character| !character.is_whitespace())
        .map(fold)
        .collect();
    if query.is_empty() {
        return Some(FuzzyMatch::default());
    }
    let (offsets, chars): (Vec<usize>, Vec<char>) = text.char_indices().unzip();
    let folded: Vec<char> = chars.iter().copied().map(fold).collect();
    let first = first_alignment(&query, &folded)?;
    let indexes = if query.len().saturating_mul(chars.len()) <= MAX_CELLS {
        best_alignment(&query, &folded, &chars)
    } else {
        first
    };
    Some(FuzzyMatch {
        score: score_positions(&chars, &indexes),
        positions: indexes.into_iter().map(|index| offsets[index]).collect(),
    })
}

fn fold(character: char) -> char {
    character.to_lowercase().next().unwrap_or(character)
}

/// True where a word begins: at the start, after a separator, or at a capital
/// after a lowercase letter.
fn is_word_start(chars: &[char], index: usize) -> bool {
    let current = chars[index];
    match index.checked_sub(1).map(|previous| chars[previous]) {
        None => true,
        Some(previous) => {
            (!previous.is_alphanumeric() && current.is_alphanumeric())
                || (previous.is_lowercase() && current.is_uppercase())
        }
    }
}

fn char_score(chars: &[char], index: usize) -> i32 {
    if is_word_start(chars, index) {
        MATCH + WORD_START
    } else {
        MATCH
    }
}

fn leading_penalty(index: usize) -> i32 {
    index.min(MAX_LEADING) as i32
}

/// The cost of skipping `skipped` characters between two matches.
fn gap_penalty(skipped: usize) -> i32 {
    GAP_START + GAP_EXTEND * (skipped.min(100_000) as i32 - 1)
}

/// Scores matched character indexes, the same way [`best_alignment`] does.
fn score_positions(chars: &[char], indexes: &[usize]) -> i32 {
    let mut score = 0;
    for (number, &index) in indexes.iter().enumerate() {
        score += char_score(chars, index);
        match number.checked_sub(1).map(|previous| indexes[previous]) {
            None => score -= leading_penalty(index),
            Some(previous) if index == previous + 1 => score += CONSECUTIVE,
            Some(previous) => score -= gap_penalty(index - previous - 1),
        }
    }
    score
}

/// The earliest place each query character matches, if they all do.
fn first_alignment(query: &[char], folded: &[char]) -> Option<Vec<usize>> {
    let mut indexes = Vec::with_capacity(query.len());
    let mut next = 0;
    for &wanted in query {
        let found = next + folded[next..].iter().position(|&each| each == wanted)?;
        indexes.push(found);
        next = found + 1;
    }
    Some(indexes)
}

/// The highest-scoring place for each query character, by dynamic programming
/// over every query and text character pair. There must be at least one match.
fn best_alignment(query: &[char], folded: &[char], chars: &[char]) -> Vec<usize> {
    const NONE: i32 = i32::MIN / 2;
    let columns = folded.len();
    // The best score with query character `row` matched at text character
    // `column`, and the column the previous query character matched at.
    let mut best = vec![NONE; query.len() * columns];
    let mut from = vec![0usize; query.len() * columns];
    for (row, &wanted) in query.iter().enumerate() {
        // The best way to reach this column across a gap, already charged for it.
        let mut gap = (NONE, 0);
        for column in 0..columns {
            if row > 0 && column >= 2 {
                gap.0 -= GAP_EXTEND;
                let candidate = best[(row - 1) * columns + column - 2];
                if candidate > NONE && candidate - GAP_START >= gap.0 {
                    gap = (candidate - GAP_START, column - 2);
                }
            }
            if folded[column] != wanted {
                continue;
            }
            let cell = row * columns + column;
            let own = char_score(chars, column);
            if row == 0 {
                best[cell] = own - leading_penalty(column);
                continue;
            }
            if column == 0 {
                continue;
            }
            let mut reach = (NONE, 0);
            let previous = best[cell - columns - 1];
            if previous > NONE {
                reach = (previous + CONSECUTIVE, column - 1);
            }
            if gap.0 > reach.0 {
                reach = gap;
            }
            if reach.0 > NONE {
                best[cell] = reach.0 + own;
                from[cell] = reach.1;
            }
        }
    }
    let last_row = (query.len() - 1) * columns;
    let mut column = (0..columns)
        .max_by_key(|&column| (best[last_row + column], std::cmp::Reverse(column)))
        .unwrap_or(0);
    let mut indexes = vec![0; query.len()];
    for row in (0..query.len()).rev() {
        indexes[row] = column;
        column = from[row * columns + column];
    }
    indexes
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteItem {
    pub label: String,
    /// Dimmer text after the label, such as a file's folder. Empty for none.
    pub detail: String,
    /// A key shown on the right, such as "Ctrl+O". Empty for none.
    pub shortcut: String,
    /// The app's own identifier, which comes back when the item is chosen.
    pub id: u32,
}

impl PaletteItem {
    pub fn new(label: &str, detail: &str, shortcut: &str, id: u32) -> Self {
        Self {
            label: label.to_string(),
            detail: detail.to_string(),
            shortcut: shortcut.to_string(),
            id,
        }
    }
}

/// What a palette needs its owner to do after handling input.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaletteResponse {
    pub redraw: bool,
    /// An item was chosen with Enter or a click. Close the palette and run it.
    pub chosen: Option<u32>,
    /// Escape, or a click outside. Close the palette.
    pub cancelled: bool,
    /// Text to put on the clipboard.
    pub copy: Option<String>,
    /// The owner should read the clipboard and call [`Palette::paste`].
    pub wants_paste: bool,
}

impl PaletteResponse {
    const CANCELLED: PaletteResponse = PaletteResponse {
        redraw: true,
        chosen: None,
        cancelled: true,
        copy: None,
        wants_paste: false,
    };
}

/// An item that matches the query, and where.
#[derive(Clone, Debug)]
struct Found {
    item: usize,
    positions: Vec<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Layout {
    rect: Rect,
    field: Rect,
    list: Rect,
}

#[derive(Clone, Debug, Default)]
pub struct Palette {
    field: TextField,
    items: Vec<PaletteItem>,
    /// The items matching the query, best first.
    found: Vec<Found>,
    /// An index into `found`.
    selected: usize,
    /// The first row on screen. Fractional wheel movement builds up here.
    top: f64,
    area: Rect,
}

impl Palette {
    /// Opens with an empty query, listing every item in the order given.
    pub fn new(items: Vec<PaletteItem>) -> Self {
        let mut palette = Self {
            items,
            ..Self::default()
        };
        palette.refilter();
        palette
    }

    /// Replaces the items, keeping the query.
    pub fn set_items(&mut self, items: Vec<PaletteItem>) {
        self.items = items;
        self.refilter();
    }

    pub fn query(&self) -> &str {
        self.field.text()
    }

    pub fn set_query(&mut self, text: &str) {
        self.field.set_text(text);
        self.refilter();
    }

    /// Inserts clipboard text at the caret. Returns whether anything changed.
    pub fn paste(&mut self, text: &str) -> bool {
        let changed = self.field.insert(text);
        if changed {
            self.refilter();
        }
        changed
    }

    fn refilter(&mut self) {
        let query = self.field.text();
        let mut found: Vec<(i32, Found)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(item, entry)| {
                fuzzy_match(query, &entry.label).map(|matched| {
                    (
                        matched.score,
                        Found {
                            item,
                            positions: matched.positions,
                        },
                    )
                })
            })
            .collect();
        if query.chars().any(|character| !character.is_whitespace()) {
            // Best first. A tie goes to the shorter label, then to the original order.
            let items = &self.items;
            found.sort_by(|(first_score, first), (second_score, second)| {
                second_score.cmp(first_score).then_with(|| {
                    items[first.item]
                        .label
                        .len()
                        .cmp(&items[second.item].label.len())
                })
            });
        }
        self.found = found.into_iter().map(|(_, found)| found).collect();
        self.selected = 0;
        self.top = 0.0;
    }

    /// Centers the palette near the top of `area`, usually the window's client
    /// area below the menu. Call before drawing or handling input.
    pub fn arrange(&mut self, area: Rect) {
        self.area = area;
        self.clamp_top();
    }

    fn list_top(&self) -> f32 {
        self.area.top + OFFSET + PADDING + FIELD_HEIGHT + PADDING
    }

    /// How many rows the area has room for, up to the most the palette shows.
    fn capacity(&self) -> usize {
        let room = self.area.bottom - self.list_top() - PADDING;
        ((room / ROW_HEIGHT).max(0.0) as usize).min(MAX_ROWS)
    }

    /// How many rows are on screen.
    fn rows(&self) -> usize {
        self.found.len().min(self.capacity())
    }

    fn layout(&self) -> Layout {
        let area = self.area;
        let width = WIDTH.min(area.width() - 2.0 * MARGIN).max(0.0);
        let left = area.left + (area.width() - width) / 2.0;
        let top = area.top + OFFSET;
        let field = Rect::new(
            left + PADDING,
            top + PADDING,
            left + width - PADDING,
            top + PADDING + FIELD_HEIGHT,
        );
        let list_top = self.list_top();
        let rows = self.rows();
        let list = Rect::new(
            left,
            list_top,
            left + width,
            list_top + rows as f32 * ROW_HEIGHT,
        );
        let bottom = if rows > 0 {
            list.bottom + PADDING
        } else {
            list.top
        };
        Layout {
            rect: Rect::new(left, top, left + width, bottom),
            field,
            list,
        }
    }

    /// The whole palette. A click outside it cancels.
    pub fn rect(&self) -> Rect {
        self.layout().rect
    }

    pub fn field_rect(&self) -> Rect {
        self.layout().field
    }

    /// How many items match the query.
    pub fn match_count(&self) -> usize {
        self.found.len()
    }

    /// The item at a place in the filtered list.
    pub fn matched_item(&self, index: usize) -> Option<&PaletteItem> {
        self.found.get(index).map(|found| &self.items[found.item])
    }

    /// The highlighted place in the filtered list, which Enter chooses.
    pub fn selected(&self) -> Option<usize> {
        (self.selected < self.found.len()).then_some(self.selected)
    }

    /// The places in the filtered list that are on screen.
    pub fn visible_matches(&self) -> Range<usize> {
        let first = (self.top as usize).min(self.found.len());
        first..(first + self.rows()).min(self.found.len())
    }

    /// The rectangle of a place in the filtered list, or `None` off screen.
    pub fn match_rect(&self, index: usize) -> Option<Rect> {
        if !self.visible_matches().contains(&index) {
            return None;
        }
        let list = self.layout().list;
        let top = list.top + (index - self.top as usize) as f32 * ROW_HEIGHT;
        Some(Rect::new(list.left, top, list.right, top + ROW_HEIGHT))
    }

    fn max_top(&self) -> f64 {
        (self.found.len() - self.rows()) as f64
    }

    fn clamp_top(&mut self) {
        self.top = self.top.clamp(0.0, self.max_top());
    }

    fn reveal(&mut self, index: usize) {
        let rows = self.rows().max(1);
        let first = self.top as usize;
        if index < first {
            self.top = index as f64;
        } else if index >= first + rows {
            self.top = (index + 1 - rows) as f64;
        }
        self.clamp_top();
    }

    fn match_at(&self, point: Point) -> Option<usize> {
        let list = self.layout().list;
        if !list.contains(point) {
            return None;
        }
        let index = self.top as usize + ((point.y - list.top) / ROW_HEIGHT) as usize;
        (index < self.found.len()).then_some(index)
    }

    fn chosen(&self, index: usize) -> PaletteResponse {
        PaletteResponse {
            redraw: true,
            chosen: self.matched_item(index).map(|item| item.id),
            ..PaletteResponse::default()
        }
    }

    /// The keys the palette keeps for itself. The rest edit the query.
    fn handle_key(&mut self, key: Key) -> Option<PaletteResponse> {
        let last = self.found.len().checked_sub(1);
        let page = self.rows().max(1);
        let target = match (key, last) {
            (Key::Escape, _) => return Some(PaletteResponse::CANCELLED),
            (Key::Enter, _) => {
                return Some(match self.selected() {
                    Some(index) => self.chosen(index),
                    None => PaletteResponse::default(),
                });
            }
            (Key::Up | Key::Down | Key::PageUp | Key::PageDown, None) => {
                return Some(PaletteResponse::default());
            }
            (Key::Up, Some(last)) => self.selected.checked_sub(1).unwrap_or(last),
            (Key::Down, Some(last)) if self.selected >= last => 0,
            (Key::Down, Some(_)) => self.selected + 1,
            (Key::PageUp, Some(_)) => self.selected.saturating_sub(page),
            (Key::PageDown, Some(last)) => (self.selected + page).min(last),
            _ => return None,
        };
        let before = (self.selected, self.top);
        self.selected = target;
        self.reveal(target);
        Some(PaletteResponse {
            redraw: (self.selected, self.top) != before,
            ..PaletteResponse::default()
        })
    }

    /// Handles input. The palette takes all input while it's open. The canvas
    /// measures text for clicks in the field.
    pub fn handle(&mut self, event: Event, canvas: &mut dyn Canvas) -> PaletteResponse {
        let layout = self.layout();
        match event {
            Event::KeyDown(key, modifiers) if !modifiers.alt => {
                if let Some(response) = self.handle_key(key) {
                    return response;
                }
            }
            Event::MouseDown(point, ..) if !layout.rect.contains(point) => {
                return PaletteResponse::CANCELLED;
            }
            Event::MouseDown(point, MouseButton::Left, _) => {
                if let Some(index) = self.match_at(point) {
                    return self.chosen(index);
                }
            }
            Event::Wheel(rows) => {
                let before = self.top;
                self.top -= f64::from(rows);
                self.clamp_top();
                return PaletteResponse {
                    redraw: self.top != before,
                    ..PaletteResponse::default()
                };
            }
            _ => {}
        }
        let field = self.field.handle(event, canvas, layout.field);
        if field.changed {
            self.refilter();
        }
        let mut response = PaletteResponse {
            redraw: field.redraw || field.changed,
            copy: field.copy,
            wants_paste: field.wants_paste,
            ..PaletteResponse::default()
        };
        // Hovering moves the highlight, as in a menu, unless the mouse is
        // selecting text in the field.
        if let Event::MouseMove(point) = event {
            if let Some(index) = self.match_at(point).filter(|_| !field.redraw) {
                if index != self.selected {
                    self.selected = index;
                    response.redraw = true;
                }
            }
        }
        response
    }

    /// Draws the palette. Call last, so it covers everything beneath it.
    /// `placeholder` shows while the query is empty. Returns where the caret
    /// is, for placing an IME's composition window there.
    pub fn draw(&mut self, canvas: &mut dyn Canvas, theme: &Theme, placeholder: &str) -> Point {
        let layout = self.layout();
        let rect = layout.rect;
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
        let caret = self.field.draw(canvas, layout.field, theme, true, "");
        if self.field.text().is_empty() {
            let field = layout.field;
            let hint = Rect::new(
                field.left + FIELD_TEXT_INSET,
                field.top,
                field.right - FIELD_TEXT_INSET,
                field.bottom,
            );
            canvas.text(placeholder, hint, UI_LEFT, theme.placeholder_text);
        }
        canvas.push_clip(layout.list);
        for index in self.visible_matches() {
            if let Some(row) = self.match_rect(index) {
                self.draw_row(canvas, theme, index, row);
            }
        }
        canvas.pop_clip();
        caret
    }

    fn draw_row(&self, canvas: &mut dyn Canvas, theme: &Theme, index: usize, row: Rect) {
        let found = &self.found[index];
        let item = &self.items[found.item];
        if index == self.selected {
            canvas.fill_rect(row, theme.hover_background);
        }
        let mut right = row.right - ROW_PADDING;
        if !item.shortcut.is_empty() {
            let width = canvas.text_width(&item.shortcut, FontRole::Ui);
            canvas.text(
                &item.shortcut,
                Rect::new(right - width, row.top, right, row.bottom),
                UI_RIGHT,
                theme.secondary_text,
            );
            right -= width + GAP;
        }
        let label = Rect::new(row.left + ROW_PADDING, row.top, right, row.bottom);
        draw_highlighted(canvas, &item.label, &found.positions, label, theme);
        if !item.detail.is_empty() {
            let left = label.left + canvas.text_width(&item.label, FontRole::Ui) + GAP / 2.0;
            if right - left >= MIN_DETAIL_WIDTH {
                let detail = elide(canvas, &item.detail, right - left, FontRole::Ui);
                canvas.text(
                    &detail,
                    Rect::new(left, row.top, right, row.bottom),
                    UI_LEFT,
                    theme.secondary_text,
                );
            }
        }
    }
}

/// Draws a label clipped to `rect`, with the characters at the byte offsets in
/// `positions` in the accent color. Each run of one color is drawn where the
/// whole label would put it.
fn draw_highlighted(
    canvas: &mut dyn Canvas,
    label: &str,
    positions: &[usize],
    rect: Rect,
    theme: &Theme,
) {
    let mut runs: Vec<(Range<usize>, bool)> = Vec::new();
    for (offset, character) in label.char_indices() {
        let matched = positions.binary_search(&offset).is_ok();
        let end = offset + character.len_utf8();
        match runs.last_mut() {
            Some((range, kind)) if *kind == matched => range.end = end,
            _ => runs.push((offset..end, matched)),
        }
    }
    for (range, matched) in runs {
        let left = rect.left + canvas.text_width(&label[..range.start], FontRole::Ui);
        if left >= rect.right {
            break;
        }
        let color = if matched {
            theme.accent
        } else {
            theme.primary_text
        };
        canvas.text(
            &label[range],
            Rect::new(left, rect.top, rect.right, rect.bottom),
            UI_LEFT,
            color,
        );
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
    const CTRL: Modifiers = Modifiers {
        ctrl: true,
        shift: false,
        alt: false,
    };
    const AREA: Rect = Rect::new(0.0, 0.0, 800.0, 600.0);

    // The fuzzy matcher.

    fn positions(query: &str, text: &str) -> Vec<usize> {
        fuzzy_match(query, text).expect("matches").positions
    }

    fn score(query: &str, text: &str) -> i32 {
        fuzzy_match(query, text).expect("matches").score
    }

    /// Texts in the order the query ranks them.
    fn ranked<'a>(query: &str, texts: &[&'a str]) -> Vec<&'a str> {
        let mut scored: Vec<(i32, &str)> = texts
            .iter()
            .filter_map(|text| fuzzy_match(query, text).map(|found| (found.score, *text)))
            .collect();
        scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        scored.into_iter().map(|(_, text)| text).collect()
    }

    #[test]
    fn an_empty_query_matches_anything() {
        assert_eq!(fuzzy_match("", "Open File"), Some(FuzzyMatch::default()));
        assert_eq!(fuzzy_match("  ", ""), Some(FuzzyMatch::default()));
    }

    #[test]
    fn every_query_character_must_appear_in_order() {
        assert_eq!(fuzzy_match("xyz", "Open File"), None);
        assert_eq!(fuzzy_match("fo", "Open File"), None, "order matters");
        assert_eq!(fuzzy_match("open files", "Open File"), None);
        assert_eq!(fuzzy_match("a", ""), None);
        assert!(fuzzy_match("ofe", "Open File").is_some());
    }

    #[test]
    fn case_and_query_spaces_are_ignored() {
        assert_eq!(positions("OPEN", "open"), vec![0, 1, 2, 3]);
        assert_eq!(
            positions("open file", "OpenFile"),
            positions("openfile", "OpenFile")
        );
    }

    #[test]
    fn positions_are_byte_offsets() {
        // The "\u{ef}" takes two bytes, so "v" starts at byte 4.
        assert_eq!(positions("vl", "na\u{ef}ve log"), vec![4, 7]);
        assert_eq!(positions("\u{c9}", "caf\u{e9}"), vec![3]);
    }

    #[test]
    fn the_best_placement_wins_over_the_first() {
        // The first "l" is in "Toggle", but the match belongs on "Log".
        assert_eq!(positions("log", "Toggle Live Log"), vec![12, 13, 14]);
        assert_eq!(
            positions("gs", "getSettings"),
            vec![0, 3],
            "a camelCase hump"
        );
        assert_eq!(
            positions("fb", "foo_bar_baz"),
            vec![0, 4],
            "after an underscore"
        );
    }

    #[test]
    fn the_best_placement_never_scores_below_the_first() {
        for (query, text) in [
            ("log", "Toggle Live Log"),
            ("of", "Toggle Profiler"),
            ("abc", "a_b_c abc"),
            ("ccm", "C:\\Windows\\CCM\\Logs"),
            ("ee", "eee e"),
        ] {
            let chars: Vec<char> = text.chars().collect();
            let folded: Vec<char> = chars.iter().copied().map(fold).collect();
            let query: Vec<char> = query.chars().map(fold).collect();
            let first = first_alignment(&query, &folded).unwrap();
            let best = best_alignment(&query, &folded, &chars);
            assert!(
                score_positions(&chars, &best) >= score_positions(&chars, &first),
                "{text}"
            );
        }
    }

    #[test]
    fn word_starts_beat_the_middle_of_words() {
        assert_eq!(
            ranked("of", &["Toggle Profiler", "Proof", "Open File"]),
            vec!["Open File", "Proof", "Toggle Profiler"]
        );
        assert_eq!(positions("of", "Open File"), vec![0, 5]);
    }

    #[test]
    fn consecutive_matches_beat_scattered_ones() {
        assert_eq!(
            ranked("fil", &["Profile", "Find In Logs", "File"]),
            vec!["File", "Find In Logs", "Profile"]
        );
        assert!(score("log", "Logs") > score("log", "Lots of glue"));
    }

    #[test]
    fn earlier_matches_beat_later_ones() {
        assert!(score("log", "Log Settings") > score("log", "Open Log"));
        // The penalty for a late start is capped, so word starts still count
        // deep inside a long path.
        assert!(
            score("pol", "C:\\Windows\\CCM\\Logs\\PolicyAgent.log")
                > score("pol", "C:\\Temp\\apolitical.log")
        );
    }

    #[test]
    fn very_long_text_falls_back_to_the_first_match() {
        let query = "a".repeat(300);
        let text = format!("{}{}", "b".repeat(1000), "a".repeat(300));
        assert!(query.len() * text.len() > MAX_CELLS);
        let found = fuzzy_match(&query, &text).unwrap();
        assert_eq!(found.positions.len(), 300);
        assert_eq!(found.positions[0], 1000);
        assert_eq!(fuzzy_match(&"a".repeat(400), &text), None);
    }

    // The palette.

    fn items() -> Vec<PaletteItem> {
        vec![
            PaletteItem::new("Open File", "", "Ctrl+O", 1),
            PaletteItem::new("Toggle Profiler", "", "", 2),
            PaletteItem::new("Proof", "", "", 3),
            PaletteItem::new("Find In Logs", "", "Ctrl+Shift+F", 4),
            PaletteItem::new("File Properties", "", "", 5),
            PaletteItem::new("PolicyAgent.log", "C:\\Windows\\CCM\\Logs", "", 6),
        ]
    }

    fn opened(items: Vec<PaletteItem>) -> Palette {
        let mut palette = Palette::new(items);
        palette.arrange(AREA);
        palette
    }

    fn send(palette: &mut Palette, event: Event) -> PaletteResponse {
        let response = palette.handle(event, &mut RecordingCanvas::new());
        palette.arrange(AREA);
        response
    }

    fn key(palette: &mut Palette, key: Key) -> PaletteResponse {
        send(palette, Event::KeyDown(key, NONE))
    }

    fn type_text(palette: &mut Palette, text: &str) {
        for character in text.chars() {
            send(palette, Event::Char(character));
        }
    }

    fn labels(palette: &Palette) -> Vec<&str> {
        (0..palette.match_count())
            .map(|index| palette.matched_item(index).unwrap().label.as_str())
            .collect()
    }

    fn center(rect: Rect) -> Point {
        Point::new(
            (rect.left + rect.right) / 2.0,
            (rect.top + rect.bottom) / 2.0,
        )
    }

    fn drawn(palette: &mut Palette) -> RecordingCanvas {
        let mut canvas = RecordingCanvas::new();
        palette.draw(&mut canvas, &Theme::CLASSIC, "Type a command");
        assert!(canvas.clips_balance());
        canvas
    }

    fn generated(count: u32) -> Vec<PaletteItem> {
        (0..count)
            .map(|index| PaletteItem::new(&format!("Command {index}"), "", "", index))
            .collect()
    }

    #[test]
    fn everything_shows_in_order_before_typing() {
        let palette = opened(items());
        assert_eq!(
            labels(&palette)[..3],
            ["Open File", "Toggle Profiler", "Proof"]
        );
        assert_eq!(palette.selected(), Some(0));
        assert_eq!(palette.visible_matches(), 0..6);
        let rect = palette.rect();
        assert_eq!(rect.width(), WIDTH);
        assert_eq!((rect.left + rect.right) / 2.0, 400.0, "centered");
        assert!(palette.field_rect().bottom <= palette.match_rect(0).unwrap().top);
    }

    #[test]
    fn typing_filters_and_ranks_the_list() {
        let mut palette = opened(items());
        key(&mut palette, Key::Down);
        type_text(&mut palette, "of");
        assert_eq!(palette.query(), "of");
        assert_eq!(labels(&palette)[0], "Open File");
        assert!(!labels(&palette).contains(&"Find In Logs"));
        assert_eq!(palette.selected(), Some(0), "typing resets the highlight");
    }

    #[test]
    fn nothing_matching_empties_the_list() {
        let mut palette = opened(items());
        type_text(&mut palette, "zzz");
        assert_eq!(palette.match_count(), 0);
        assert_eq!(palette.selected(), None);
        assert_eq!(key(&mut palette, Key::Enter), PaletteResponse::default());
        assert_eq!(key(&mut palette, Key::Down), PaletteResponse::default());
        assert_eq!(palette.rect().bottom, palette.field_rect().bottom + PADDING);
        drawn(&mut palette);
    }

    #[test]
    fn arrows_wrap_and_page_keys_stop_at_the_ends() {
        let mut palette = opened(items());
        key(&mut palette, Key::Down);
        assert_eq!(palette.selected(), Some(1));
        key(&mut palette, Key::Up);
        assert_eq!(palette.selected(), Some(0));
        assert!(key(&mut palette, Key::Up).redraw);
        assert_eq!(palette.selected(), Some(5), "wraps to the end");
        key(&mut palette, Key::Down);
        assert_eq!(palette.selected(), Some(0), "wraps to the start");
        key(&mut palette, Key::PageDown);
        assert_eq!(palette.selected(), Some(5));
        assert!(!key(&mut palette, Key::PageDown).redraw);
        key(&mut palette, Key::PageUp);
        assert_eq!(palette.selected(), Some(0));
    }

    #[test]
    fn enter_chooses_and_escape_cancels() {
        let mut palette = opened(items());
        key(&mut palette, Key::Down);
        let response = key(&mut palette, Key::Enter);
        assert_eq!(response.chosen, Some(2));
        assert!(!response.cancelled);
        let response = key(&mut palette, Key::Escape);
        assert!(response.cancelled);
        assert_eq!(response.chosen, None);
    }

    #[test]
    fn clicking_a_row_chooses_it() {
        let mut palette = opened(items());
        let row = center(palette.match_rect(3).unwrap());
        let response = send(&mut palette, Event::MouseDown(row, MouseButton::Left, NONE));
        assert_eq!(response.chosen, Some(4));
        let right_click = Event::MouseDown(row, MouseButton::Right, NONE);
        assert_eq!(send(&mut palette, right_click).chosen, None);
    }

    #[test]
    fn clicking_outside_cancels_but_the_field_does_not() {
        let mut palette = opened(items());
        let outside = Event::MouseDown(Point::new(5.0, 590.0), MouseButton::Left, NONE);
        assert!(send(&mut palette, outside).cancelled);
        let field = Event::MouseDown(center(palette.field_rect()), MouseButton::Left, NONE);
        let response = send(&mut palette, field);
        assert!(!response.cancelled);
        assert_eq!(response.chosen, None);
    }

    #[test]
    fn hovering_a_row_highlights_it() {
        let mut palette = opened(items());
        let row = center(palette.match_rect(2).unwrap());
        assert!(send(&mut palette, Event::MouseMove(row)).redraw);
        assert_eq!(palette.selected(), Some(2));
        assert!(!send(&mut palette, Event::MouseMove(row)).redraw);
    }

    #[test]
    fn long_lists_scroll_with_the_highlight_and_the_wheel() {
        let mut palette = opened(generated(1000));
        assert_eq!(palette.visible_matches(), 0..MAX_ROWS);
        assert!(send(&mut palette, Event::Wheel(-3.0)).redraw);
        assert_eq!(palette.visible_matches(), 3..MAX_ROWS + 3);
        send(&mut palette, Event::Wheel(10_000.0));
        assert_eq!(palette.visible_matches(), 0..MAX_ROWS);
        assert!(!send(&mut palette, Event::Wheel(1.0)).redraw);
        key(&mut palette, Key::Up);
        assert_eq!(palette.selected(), Some(999));
        assert_eq!(palette.visible_matches(), 1000 - MAX_ROWS..1000);
        key(&mut palette, Key::Down);
        assert_eq!(palette.visible_matches(), 0..MAX_ROWS);
        key(&mut palette, Key::PageDown);
        assert_eq!(palette.selected(), Some(MAX_ROWS));
        assert_eq!(palette.visible_matches(), 1..MAX_ROWS + 1);
        assert_eq!(palette.match_rect(0), None);
    }

    #[test]
    fn huge_lists_draw_only_the_visible_rows() {
        let mut palette = opened(generated(10_000));
        let canvas = drawn(&mut palette);
        // The placeholder, then one text per row.
        assert_eq!(canvas.texts().len(), 1 + MAX_ROWS);
        type_text(&mut palette, "9999");
        assert_eq!(labels(&palette)[0], "Command 9999");
    }

    #[test]
    fn matched_letters_are_drawn_in_the_accent_color() {
        let mut palette = opened(items());
        type_text(&mut palette, "of");
        let canvas = drawn(&mut palette);
        let accent = Theme::CLASSIC.accent;
        let row = palette.match_rect(0).unwrap();
        let first_row: Vec<(&str, bool)> = canvas
            .ops
            .iter()
            .filter_map(|op| match op {
                DrawOp::Text(text, rect, _, color) if rect.top == row.top => {
                    Some((text.as_str(), *color == accent))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            first_row,
            vec![
                ("Ctrl+O", false),
                ("O", true),
                ("pen ", false),
                ("F", true),
                ("ile", false)
            ]
        );
        // Each run sits where it would in the whole label.
        let file = canvas.ops.iter().find_map(|op| match op {
            DrawOp::Text(text, rect, ..) if text == "F" => Some(rect.left),
            _ => None,
        });
        assert_eq!(file, Some(row.left + ROW_PADDING + 5.0 * 7.0));
    }

    #[test]
    fn the_placeholder_shows_until_typing() {
        let mut palette = opened(items());
        assert!(drawn(&mut palette).texts().contains(&"Type a command"));
        type_text(&mut palette, "x");
        assert!(!drawn(&mut palette).texts().contains(&"Type a command"));
    }

    #[test]
    fn shortcuts_and_details_are_drawn() {
        let mut palette = opened(items());
        let canvas = drawn(&mut palette);
        let texts = canvas.texts();
        assert!(texts.contains(&"Ctrl+O"));
        assert!(texts.contains(&"Ctrl+Shift+F"));
        assert!(texts.contains(&"C:\\Windows\\CCM\\Logs"));
    }

    #[test]
    fn the_clipboard_goes_through_the_owner() {
        let mut palette = opened(items());
        type_text(&mut palette, "abc");
        send(&mut palette, Event::KeyDown(Key::Char('A'), CTRL));
        let copied = send(&mut palette, Event::KeyDown(Key::Char('C'), CTRL));
        assert_eq!(copied.copy.as_deref(), Some("abc"));
        assert!(send(&mut palette, Event::KeyDown(Key::Char('V'), CTRL)).wants_paste);
        assert!(palette.paste("open"));
        assert_eq!(palette.query(), "open");
        assert_eq!(labels(&palette), vec!["Open File"]);
    }

    #[test]
    fn new_items_keep_the_query() {
        let mut palette = opened(items());
        palette.set_query("proof");
        assert_eq!(labels(&palette), vec!["Proof"]);
        palette.set_items(generated(3));
        assert_eq!(palette.match_count(), 0);
        palette.set_query("");
        assert_eq!(palette.match_count(), 3);
    }

    #[test]
    fn an_empty_palette_is_safe() {
        let mut palette = opened(Vec::new());
        for each in [Key::Up, Key::Down, Key::PageUp, Key::PageDown, Key::Enter] {
            assert_eq!(key(&mut palette, each), PaletteResponse::default());
        }
        assert!(!send(&mut palette, Event::Wheel(-3.0)).redraw);
        assert!(key(&mut palette, Key::Escape).cancelled);
        drawn(&mut palette);
    }

    #[test]
    fn a_tiny_window_is_safe() {
        let mut palette = Palette::new(items());
        palette.arrange(Rect::new(0.0, 0.0, 20.0, 30.0));
        assert_eq!(palette.visible_matches(), 0..0);
        let mut canvas = RecordingCanvas::new();
        for event in [
            Event::KeyDown(Key::Down, NONE),
            Event::KeyDown(Key::PageDown, NONE),
            Event::Wheel(-3.0),
            Event::MouseMove(Point::new(10.0, 10.0)),
        ] {
            palette.handle(event, &mut canvas);
        }
        assert_eq!(palette.visible_matches().len(), 0);
        palette.draw(&mut canvas, &Theme::CLASSIC, "Type a command");
        assert!(canvas.clips_balance());
    }
}
