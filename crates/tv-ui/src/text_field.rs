//! A single-line text field: typing, caret, selection, word jumps, clipboard
//! and undo. The caret doesn't blink, because blinking needs a timer, and the app
//! uses no CPU while idle.

use crate::canvas::{Align, Canvas, FontRole, TextStyle};
use crate::geometry::{Point, Rect};
use crate::input::{Event, Key, MouseButton};
use crate::theme::Theme;

const MAX_CHARS: usize = 2000;
const MAX_UNDO: usize = 100;
const PADDING: f32 = 6.0;
const STYLE: TextStyle = TextStyle::new(FontRole::Ui, Align::Left);

/// What a field needs its owner to do after handling input.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FieldResponse {
    pub redraw: bool,
    pub changed: bool,
    /// Enter was pressed. `true` in the tuple means Shift was held.
    pub submitted: Option<bool>,
    pub cancelled: bool,
    /// Text to put on the clipboard.
    pub copy: Option<String>,
    /// The owner should read the clipboard and call [`TextField::insert`].
    pub wants_paste: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TextField {
    text: String,
    /// Byte offsets, always on character boundaries.
    cursor: usize,
    anchor: usize,
    undo: Vec<(String, usize)>,
    scroll: f32,
    dragging: bool,
}

fn is_word_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

impl TextField {
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Replaces all the text, leaving the caret at the end.
    pub fn set_text(&mut self, text: &str) {
        self.remember();
        self.text = clean(text);
        self.cursor = self.text.len();
        self.anchor = self.cursor;
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.text.len();
    }

    pub fn selection(&self) -> std::ops::Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }

    fn selected_text(&self) -> &str {
        &self.text[self.selection()]
    }

    fn remember(&mut self) {
        if self.undo.last().map(|(text, _)| text) != Some(&self.text) {
            self.undo.push((self.text.clone(), self.cursor));
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
        }
    }

    fn delete_selection(&mut self) -> bool {
        let range = self.selection();
        if range.is_empty() {
            return false;
        }
        self.text.replace_range(range.clone(), "");
        self.cursor = range.start;
        self.anchor = range.start;
        true
    }

    /// Inserts text at the caret, replacing any selection. Line breaks become
    /// spaces, because the field holds one line. Returns whether anything changed.
    pub fn insert(&mut self, text: &str) -> bool {
        let cleaned = clean(text);
        let selected_chars = self.selected_text().chars().count();
        let room = (MAX_CHARS + selected_chars).saturating_sub(self.text.chars().count());
        let allowed: String = cleaned.chars().take(room).collect();
        if allowed.is_empty() && selected_chars == 0 {
            return false;
        }
        self.remember();
        self.delete_selection();
        self.text.insert_str(self.cursor, &allowed);
        self.cursor += allowed.len();
        self.anchor = self.cursor;
        true
    }

    fn previous_boundary(&self, from: usize) -> usize {
        self.text[..from]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next_boundary(&self, from: usize) -> usize {
        self.text[from..]
            .chars()
            .next()
            .map_or(from, |character| from + character.len_utf8())
    }

    /// Skips back over spaces, then over one word.
    fn previous_word(&self, from: usize) -> usize {
        let mut position = from;
        while position > 0 && !self.char_before(position).is_some_and(is_word_char) {
            position = self.previous_boundary(position);
        }
        while position > 0 && self.char_before(position).is_some_and(is_word_char) {
            position = self.previous_boundary(position);
        }
        position
    }

    /// Skips over one word, then over spaces, the way Windows text boxes do.
    fn next_word(&self, from: usize) -> usize {
        let mut position = from;
        while position < self.text.len() && self.char_at(position).is_some_and(is_word_char) {
            position = self.next_boundary(position);
        }
        while position < self.text.len() && !self.char_at(position).is_some_and(is_word_char) {
            position = self.next_boundary(position);
        }
        position
    }

    fn char_before(&self, position: usize) -> Option<char> {
        self.text[..position].chars().next_back()
    }

    fn char_at(&self, position: usize) -> Option<char> {
        self.text[position..].chars().next()
    }

    fn move_to(&mut self, position: usize, extend: bool) {
        self.cursor = position;
        if !extend {
            self.anchor = position;
        }
    }

    fn handle_key(&mut self, key: Key, ctrl: bool, shift: bool) -> FieldResponse {
        let mut response = FieldResponse {
            redraw: true,
            ..FieldResponse::default()
        };
        let collapsed = self.selection().is_empty();
        match key {
            Key::Left if !shift && !collapsed => {
                let start = self.selection().start;
                self.move_to(start, false);
            }
            Key::Right if !shift && !collapsed => {
                let end = self.selection().end;
                self.move_to(end, false);
            }
            Key::Left => {
                let target = if ctrl {
                    self.previous_word(self.cursor)
                } else {
                    self.previous_boundary(self.cursor)
                };
                self.move_to(target, shift);
            }
            Key::Right => {
                let target = if ctrl {
                    self.next_word(self.cursor)
                } else {
                    self.next_boundary(self.cursor)
                };
                self.move_to(target, shift);
            }
            Key::Home => self.move_to(0, shift),
            Key::End => self.move_to(self.text.len(), shift),
            Key::Backspace => {
                self.remember();
                if !self.delete_selection() && self.cursor > 0 {
                    let start = if ctrl {
                        self.previous_word(self.cursor)
                    } else {
                        self.previous_boundary(self.cursor)
                    };
                    self.text.replace_range(start..self.cursor, "");
                    self.move_to(start, false);
                }
                response.changed = true;
            }
            Key::Delete => {
                self.remember();
                if !self.delete_selection() && self.cursor < self.text.len() {
                    let end = if ctrl {
                        self.next_word(self.cursor)
                    } else {
                        self.next_boundary(self.cursor)
                    };
                    self.text.replace_range(self.cursor..end, "");
                }
                response.changed = true;
            }
            Key::Enter => response.submitted = Some(shift),
            Key::Escape => response.cancelled = true,
            Key::Char('A') if ctrl => self.select_all(),
            Key::Char('C') if ctrl => {
                if !collapsed {
                    response.copy = Some(self.selected_text().to_string());
                }
            }
            Key::Char('X') if ctrl => {
                if !collapsed {
                    response.copy = Some(self.selected_text().to_string());
                    self.remember();
                    self.delete_selection();
                    response.changed = true;
                }
            }
            Key::Char('V') if ctrl => response.wants_paste = true,
            Key::Char('Z') if ctrl => {
                if let Some((text, cursor)) = self.undo.pop() {
                    self.text = text;
                    self.cursor = cursor.min(self.text.len());
                    self.anchor = self.cursor;
                    response.changed = true;
                }
            }
            _ => return FieldResponse::default(),
        }
        response
    }

    /// Places the caret at the character boundary closest to `x`.
    fn position_at(&self, x: f32, canvas: &mut dyn Canvas, rect: Rect) -> usize {
        let target = x - rect.left - PADDING + self.scroll;
        let mut best = (0usize, f32::MAX);
        let mut boundaries: Vec<usize> = self.text.char_indices().map(|(index, _)| index).collect();
        boundaries.push(self.text.len());
        for boundary in boundaries {
            let distance = (canvas.text_width(&self.text[..boundary], FontRole::Ui) - target).abs();
            if distance < best.1 {
                best = (boundary, distance);
            }
        }
        best.0
    }

    /// Handles input. Mouse events need the canvas to measure text and the
    /// field's rectangle to know where it is.
    pub fn handle(&mut self, event: Event, canvas: &mut dyn Canvas, rect: Rect) -> FieldResponse {
        match event {
            Event::Char(character) if !character.is_control() => {
                let mut buffer = [0u8; 4];
                let changed = self.insert(character.encode_utf8(&mut buffer));
                FieldResponse {
                    redraw: changed,
                    changed,
                    ..FieldResponse::default()
                }
            }
            Event::KeyDown(key, modifiers) if !modifiers.alt => {
                self.handle_key(key, modifiers.ctrl, modifiers.shift)
            }
            Event::MouseDown(point, MouseButton::Left, modifiers) if rect.contains(point) => {
                let position = self.position_at(point.x, canvas, rect);
                self.move_to(position, modifiers.shift);
                self.dragging = true;
                FieldResponse {
                    redraw: true,
                    ..FieldResponse::default()
                }
            }
            Event::MouseMove(point) if self.dragging => {
                let position = self.position_at(point.x, canvas, rect);
                self.move_to(position, true);
                FieldResponse {
                    redraw: true,
                    ..FieldResponse::default()
                }
            }
            Event::MouseUp(..) => {
                self.dragging = false;
                FieldResponse::default()
            }
            _ => FieldResponse::default(),
        }
    }

    /// Draws the field and returns where the caret is, for placing an IME's
    /// composition window there.
    pub fn draw(
        &mut self,
        canvas: &mut dyn Canvas,
        rect: Rect,
        theme: &Theme,
        focused: bool,
        placeholder: &str,
    ) -> Point {
        canvas.fill_rect(rect, if focused { theme.accent } else { theme.border });
        let inner = Rect::new(
            rect.left + 1.0,
            rect.top + 1.0,
            rect.right - 1.0,
            rect.bottom - 1.0,
        );
        canvas.fill_rect(inner, theme.input_background);
        let content = Rect::new(
            inner.left + PADDING - 1.0,
            inner.top,
            inner.right - PADDING,
            inner.bottom,
        );

        // Keep the caret in view by scrolling the text sideways.
        let caret_offset = canvas.text_width(&self.text[..self.cursor], FontRole::Ui);
        if caret_offset - self.scroll > content.width() {
            self.scroll = caret_offset - content.width();
        } else if caret_offset < self.scroll {
            self.scroll = caret_offset;
        }
        let origin = content.left - self.scroll;

        canvas.push_clip(content);
        if self.text.is_empty() {
            if !focused {
                canvas.text(placeholder, content, STYLE, theme.placeholder_text);
            }
        } else {
            let range = self.selection();
            if focused && !range.is_empty() {
                let start = canvas.text_width(&self.text[..range.start], FontRole::Ui);
                let end = canvas.text_width(&self.text[..range.end], FontRole::Ui);
                canvas.fill_rect(
                    Rect::new(
                        origin + start,
                        inner.top + 3.0,
                        origin + end,
                        inner.bottom - 3.0,
                    ),
                    theme.text_selection,
                );
            }
            canvas.text(
                &self.text,
                Rect::new(
                    origin,
                    content.top,
                    content.right.max(origin + caret_offset + 1000.0),
                    content.bottom,
                ),
                STYLE,
                theme.primary_text,
            );
        }
        let caret_x = (origin + caret_offset).round();
        if focused {
            canvas.fill_rect(
                Rect::new(caret_x, inner.top + 3.0, caret_x + 1.0, inner.bottom - 3.0),
                theme.primary_text,
            );
        }
        canvas.pop_clip();
        Point::new(caret_x, inner.top + 3.0)
    }
}

/// Keeps text to one line, with no control characters.
fn clean(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character == '\n' || character == '\r' || character == '\t' {
                ' '
            } else {
                character
            }
        })
        .filter(|character| !character.is_control())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Modifiers;
    use crate::recording::RecordingCanvas;

    const RECT: Rect = Rect::new(0.0, 0.0, 300.0, 24.0);
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
    const SHIFT: Modifiers = Modifiers {
        ctrl: false,
        shift: true,
        alt: false,
    };
    const CTRL_SHIFT: Modifiers = Modifiers {
        ctrl: true,
        shift: true,
        alt: false,
    };

    fn send(field: &mut TextField, event: Event) -> FieldResponse {
        field.handle(event, &mut RecordingCanvas::new(), RECT)
    }

    fn typed(text: &str) -> TextField {
        let mut field = TextField::default();
        for character in text.chars() {
            send(&mut field, Event::Char(character));
        }
        field
    }

    fn key(field: &mut TextField, key: Key, modifiers: Modifiers) -> FieldResponse {
        send(field, Event::KeyDown(key, modifiers))
    }

    #[test]
    fn typing_inserts_at_the_caret() {
        let mut field = typed("hllo");
        key(&mut field, Key::Home, NONE);
        key(&mut field, Key::Right, NONE);
        send(&mut field, Event::Char('e'));
        assert_eq!(field.text(), "hello");
    }

    #[test]
    fn control_characters_are_ignored() {
        let mut field = typed("ab");
        let response = send(&mut field, Event::Char('\u{1}'));
        assert!(!response.changed);
        assert_eq!(field.text(), "ab");
    }

    #[test]
    fn characters_outside_the_basic_plane_are_whole() {
        let mut field = typed("a\u{1F600}b");
        key(&mut field, Key::End, NONE);
        key(&mut field, Key::Left, NONE);
        key(&mut field, Key::Backspace, NONE);
        assert_eq!(field.text(), "ab");
    }

    #[test]
    fn backspace_and_delete_respect_words_with_ctrl() {
        let mut field = typed("error code here");
        key(&mut field, Key::Backspace, CTRL);
        assert_eq!(field.text(), "error code ");
        key(&mut field, Key::Home, NONE);
        key(&mut field, Key::Delete, CTRL);
        assert_eq!(
            field.text(),
            "code ",
            "Ctrl+Delete takes the word and its space"
        );
    }

    #[test]
    fn ctrl_arrows_jump_by_word_like_windows() {
        let mut field = typed("one two three");
        key(&mut field, Key::Home, NONE);
        key(&mut field, Key::Right, CTRL);
        assert_eq!(field.cursor, 4);
        key(&mut field, Key::Right, CTRL);
        assert_eq!(field.cursor, 8);
        key(&mut field, Key::Left, CTRL);
        assert_eq!(field.cursor, 4);
    }

    #[test]
    fn shift_selects_and_typing_replaces_the_selection() {
        let mut field = typed("hello world");
        key(&mut field, Key::Left, CTRL_SHIFT);
        assert_eq!(field.selected_text(), "world");
        send(&mut field, Event::Char('X'));
        assert_eq!(field.text(), "hello X");
    }

    #[test]
    fn arrows_collapse_a_selection_to_its_edge() {
        let mut field = typed("hello");
        field.select_all();
        key(&mut field, Key::Left, NONE);
        assert_eq!((field.cursor, field.anchor), (0, 0));
        field.select_all();
        key(&mut field, Key::Right, NONE);
        assert_eq!((field.cursor, field.anchor), (5, 5));
    }

    #[test]
    fn copy_cut_and_paste() {
        let mut field = typed("abc");
        assert_eq!(key(&mut field, Key::Char('C'), CTRL).copy, None);
        field.select_all();
        assert_eq!(
            key(&mut field, Key::Char('C'), CTRL).copy.as_deref(),
            Some("abc")
        );
        let cut = key(&mut field, Key::Char('X'), CTRL);
        assert_eq!(cut.copy.as_deref(), Some("abc"));
        assert_eq!(field.text(), "");
        assert!(key(&mut field, Key::Char('V'), CTRL).wants_paste);
        field.insert("pasted\r\ntext");
        assert_eq!(field.text(), "pasted  text");
    }

    #[test]
    fn undo_restores_earlier_text() {
        let mut field = typed("ab");
        key(&mut field, Key::Backspace, NONE);
        assert_eq!(field.text(), "a");
        key(&mut field, Key::Char('Z'), CTRL);
        assert_eq!(field.text(), "ab");
    }

    #[test]
    fn enter_submits_and_escape_cancels() {
        let mut field = typed("x");
        assert_eq!(key(&mut field, Key::Enter, NONE).submitted, Some(false));
        assert_eq!(key(&mut field, Key::Enter, SHIFT).submitted, Some(true));
        assert!(key(&mut field, Key::Escape, NONE).cancelled);
    }

    #[test]
    fn length_is_capped() {
        let mut field = TextField::default();
        field.insert(&"x".repeat(MAX_CHARS + 50));
        assert_eq!(field.text().chars().count(), MAX_CHARS);
        assert!(!field.insert("more"));
    }

    #[test]
    fn clicking_places_the_caret_between_characters() {
        let mut field = typed("abcdef");
        // The recording canvas makes each character 7 wide.
        send(
            &mut field,
            Event::MouseDown(
                Point::new(PADDING + 7.0 * 2.0 + 1.0, 10.0),
                MouseButton::Left,
                NONE,
            ),
        );
        assert_eq!(field.cursor, 2);
    }

    #[test]
    fn keys_the_field_does_not_use_pass_through() {
        let mut field = typed("x");
        assert_eq!(key(&mut field, Key::F(3), NONE), FieldResponse::default());
        assert_eq!(key(&mut field, Key::Down, NONE), FieldResponse::default());
    }

    #[test]
    fn drawing_keeps_the_caret_in_view() {
        let mut field = typed(&"x".repeat(200));
        let mut canvas = RecordingCanvas::new();
        let caret = field.draw(&mut canvas, RECT, &Theme::CLASSIC, true, "");
        assert!(caret.x <= RECT.right, "caret at {}", caret.x);
        assert!(canvas.clips_balance());
    }

    #[test]
    fn placeholder_shows_only_when_empty_and_unfocused() {
        let mut field = TextField::default();
        let mut canvas = RecordingCanvas::new();
        field.draw(&mut canvas, RECT, &Theme::CLASSIC, false, "Filter rows");
        assert!(canvas.texts().contains(&"Filter rows"));
        let mut canvas = RecordingCanvas::new();
        field.draw(&mut canvas, RECT, &Theme::CLASSIC, true, "Filter rows");
        assert!(!canvas.texts().contains(&"Filter rows"));
    }
}
