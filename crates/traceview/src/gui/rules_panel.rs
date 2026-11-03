//! The highlight rules editor, opened by Tools > Highlight rules. It floats over
//! the window, as Error Lookup does, and takes all input until it closes.
//!
//! Every change applies at once, so rows light up while a pattern is typed. The
//! list ends with a "New rule" slot: typing there adds a rule, and a rule whose
//! pattern is cleared goes away once something else is selected.

use super::rules::{self, Style};
use super::settings::{Rule, MOST_RULES};
use tv_ui::chip_bar::{self, Chip, ChipBar};
use tv_ui::list_view::{ListItem, ListView};
use tv_ui::text_field::TextField;
use tv_ui::theme::RULE_COLOR_NAMES;
use tv_ui::{Align, Canvas, Event, FontRole, Key, MouseButton, Point, Rect, TextStyle, Theme};

const WIDTH: f32 = 640.0;
const HEIGHT: f32 = 420.0;
/// How far below the top of its area the panel sits.
const OFFSET: f32 = 24.0;
const PADDING: f32 = 14.0;
const FIELD_HEIGHT: f32 = 26.0;
const LINE_HEIGHT: f32 = 22.0;
const GAP: f32 = 6.0;
const LABEL: TextStyle = TextStyle::new(FontRole::Ui, Align::Left);
pub const TITLE: &str = "Highlight rules";
const PLACEHOLDER: &str = "Rows to highlight, as in the filter box: words, /regex/, level:error";
const HINT: &str = "Tab moves between parts     Ctrl+Up and Ctrl+Down reorder     Esc closes";
const ORDER_NOTE: &str = "The first rule that matches a row decides its color";
const NEW_RULE: &str = "New rule";

/// The buttons after the colors, in order.
pub const OPTIONS: [&str; 6] = [
    "Whole row",
    "Marker",
    "On",
    "Move up",
    "Move down",
    "Delete",
];
const WHOLE_ROW: usize = 0;
const MARKER: usize = 1;
const ON: usize = 2;
const MOVE_UP: usize = 3;
const MOVE_DOWN: usize = 4;
const DELETE: usize = 5;
/// The options that stay pressed, rather than acting once.
const TOGGLES: usize = 3;

/// The part of the panel the keyboard is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Field,
    Colors,
    Options,
    List,
}

impl Part {
    const ORDER: [Part; 4] = [Part::Field, Part::Colors, Part::Options, Part::List];

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

/// What the panel needs its owner to do after handling input.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RulesResponse {
    pub redraw: bool,
    /// The rules changed, so they should be applied and saved.
    pub changed: bool,
    pub closed: bool,
    /// Text to put on the clipboard.
    pub copy: Option<String>,
    /// The owner should read the clipboard and call [`RulesPanel::paste`].
    pub wants_paste: bool,
}

impl RulesResponse {
    fn redraw(changed: bool) -> Self {
        Self {
            redraw: true,
            changed,
            ..Self::default()
        }
    }
}

pub struct RulesPanel {
    pub field: TextField,
    /// The rules as edited, in order, including one whose pattern is being retyped.
    pub rules: Vec<Rule>,
    /// The rule being edited, or `rules.len()` for the "New rule" slot.
    selected: usize,
    /// The color, style and state a rule typed into the "New rule" slot gets.
    draft: Rule,
    pub focus: Part,
    pub list: ListView,
    pub colors: ChipBar,
    pub options: ChipBar,
    color_chips: Vec<Chip>,
    option_chips: Vec<Chip>,
    /// What's wrong with the selected rule's pattern, if anything.
    problem: Option<String>,
    rect: Rect,
    field_rect: Rect,
    colors_rect: Rect,
    options_rect: Rect,
    note_rect: Rect,
    hint_rect: Rect,
}

fn new_draft(existing: usize) -> Rule {
    Rule {
        color: rules::color_name(rules::color_for_new_rule(existing)),
        ..Rule::default()
    }
}

/// How a rule reads in the list, after its pattern.
fn describe(rule: &Rule) -> String {
    let mark = rules::mark_of(rule);
    let mut text = format!(
        "{}, {}",
        RULE_COLOR_NAMES[mark.color],
        match mark.style {
            Style::Row => "whole row",
            Style::Marker => "marker",
        }
    );
    if !rule.enabled {
        text.push_str(", off");
    }
    if rules::check(&rule.pattern).is_err() {
        text.push_str(", not valid");
    }
    text
}

impl RulesPanel {
    /// Opens the editor on `rules`, with the "New rule" slot selected so typing
    /// adds one.
    pub fn new(rules: Vec<Rule>) -> Self {
        let selected = rules.len();
        let mut panel = Self {
            field: TextField::default(),
            draft: new_draft(rules.len()),
            rules,
            selected,
            focus: Part::Field,
            list: ListView::default(),
            colors: ChipBar::default(),
            options: ChipBar::default(),
            color_chips: Vec::new(),
            option_chips: Vec::new(),
            problem: None,
            rect: Rect::default(),
            field_rect: Rect::default(),
            colors_rect: Rect::default(),
            options_rect: Rect::default(),
            note_rect: Rect::default(),
            hint_rect: Rect::default(),
        };
        panel.load();
        panel
    }

    /// The rules worth keeping: every one with a pattern.
    pub fn saved_rules(&self) -> Vec<Rule> {
        self.rules
            .iter()
            .filter(|rule| !rule.pattern.trim().is_empty())
            .cloned()
            .collect()
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    fn is_new(&self) -> bool {
        self.selected >= self.rules.len()
    }

    /// The selected rule, or the draft for the "New rule" slot.
    pub fn current(&self) -> &Rule {
        self.rules.get(self.selected).unwrap_or(&self.draft)
    }

    fn current_mut(&mut self) -> &mut Rule {
        match self.rules.get_mut(self.selected) {
            Some(rule) => rule,
            None => &mut self.draft,
        }
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// Puts the selected rule's pattern in the field, with fresh undo, since
    /// undoing into another rule's pattern would be confusing.
    fn load(&mut self) {
        self.field = TextField::default();
        let pattern = self.current().pattern.clone();
        self.field.set_text(&pattern);
        self.check();
        self.list.select_id(Some(self.selected as u32));
        self.list.reveal(self.selected);
    }

    fn check(&mut self) {
        self.problem = rules::check(&self.current().pattern)
            .err()
            .map(|error| format!("Not valid yet: {error}"));
    }

    /// Selects a rule, or the "New rule" slot past the last one. Returns true
    /// if leaving the old rule removed it, because its pattern was empty.
    pub fn select(&mut self, index: usize) -> bool {
        let mut index = index.min(self.rules.len());
        let mut removed = false;
        if index != self.selected
            && self
                .rules
                .get(self.selected)
                .is_some_and(|rule| rule.pattern.trim().is_empty())
        {
            self.rules.remove(self.selected);
            if index > self.selected {
                index -= 1;
            }
            removed = true;
        }
        self.selected = index;
        if self.is_new() {
            self.draft = new_draft(self.rules.len());
        }
        self.load();
        removed
    }

    /// Takes the field's text as the selected rule's pattern. Typing into the
    /// "New rule" slot adds a rule.
    fn pattern_changed(&mut self) -> bool {
        let text = self.field.text().to_string();
        if self.is_new() {
            if text.trim().is_empty() || self.rules.len() >= MOST_RULES {
                return false;
            }
            self.rules.push(Rule {
                pattern: text,
                ..self.draft.clone()
            });
        } else {
            self.rules[self.selected].pattern = text;
        }
        self.check();
        self.list.select_id(Some(self.selected as u32));
        true
    }

    pub fn paste(&mut self, text: &str) -> bool {
        self.field.insert(text) && self.pattern_changed()
    }

    /// Gives the selected rule, or the next new one, a color. Returns true if
    /// a rule changed.
    pub fn choose_color(&mut self, index: usize) -> bool {
        if index >= RULE_COLOR_NAMES.len() {
            return false;
        }
        self.current_mut().color = rules::color_name(index);
        !self.is_new()
    }

    /// Presses one of the [`OPTIONS`]. Returns true if a rule changed.
    pub fn choose_option(&mut self, index: usize) -> bool {
        let is_new = self.is_new();
        match index {
            WHOLE_ROW | MARKER => {
                let style = if index == MARKER {
                    Style::Marker
                } else {
                    Style::Row
                };
                self.current_mut().style = style.name().to_string();
                !is_new
            }
            ON => {
                let rule = self.current_mut();
                rule.enabled = !rule.enabled;
                !is_new
            }
            MOVE_UP => self.move_selected(true),
            MOVE_DOWN => self.move_selected(false),
            DELETE if !is_new => {
                self.rules.remove(self.selected);
                if self.is_new() {
                    self.draft = new_draft(self.rules.len());
                }
                self.load();
                true
            }
            _ => false,
        }
    }

    /// Moves the selected rule up or down the order.
    fn move_selected(&mut self, up: bool) -> bool {
        let index = self.selected;
        let target = if up {
            index.checked_sub(1)
        } else {
            Some(index + 1)
        };
        match target.filter(|target| *target < self.rules.len() && index < self.rules.len()) {
            Some(target) => {
                self.rules.swap(index, target);
                self.selected = target;
                self.list.select_id(Some(target as u32));
                self.list.reveal(target);
                true
            }
            None => false,
        }
    }

    pub fn rect(&self) -> Rect {
        self.rect
    }

    pub fn field_rect(&self) -> Rect {
        self.field_rect
    }

    pub fn color_chips(&self) -> &[Chip] {
        &self.color_chips
    }

    pub fn option_chips(&self) -> &[Chip] {
        &self.option_chips
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
        self.field_rect = inner.take_top(FIELD_HEIGHT);
        inner.take_top(GAP);
        self.colors_rect = inner.take_top(chip_bar::BAR_HEIGHT);
        self.options_rect = inner.take_top(chip_bar::BAR_HEIGHT);
        inner.take_top(GAP);
        self.hint_rect = inner.take_bottom(LINE_HEIGHT);
        self.note_rect = inner.take_bottom(LINE_HEIGHT);
        self.rebuild(theme);
        self.list.arrange(inner);
        // The chip bars start a margin in, so they're nudged back to line up.
        let nudge = |rect: Rect| Rect::new(rect.left - 8.0, rect.top, rect.right, rect.bottom);
        self.colors_rect = nudge(self.colors_rect);
        self.options_rect = nudge(self.options_rect);
        self.colors
            .arrange(self.colors_rect, &self.color_chips, canvas);
        self.options
            .arrange(self.options_rect, &self.option_chips, canvas);
    }

    fn rebuild(&mut self, theme: &Theme) {
        let current = rules::mark_of(self.current());
        let enabled = self.current().enabled;
        self.color_chips = RULE_COLOR_NAMES
            .iter()
            .zip(theme.rule_colors)
            .enumerate()
            .map(|(index, (name, color))| Chip {
                label: (*name).to_string(),
                count: None,
                color: color.marker,
                active: current.color == index,
            })
            .collect();
        self.option_chips = OPTIONS
            .iter()
            .enumerate()
            .map(|(index, label)| Chip {
                label: (*label).to_string(),
                count: None,
                color: theme.primary_text,
                active: match index {
                    WHOLE_ROW => current.style == Style::Row,
                    MARKER => current.style == Style::Marker,
                    ON => enabled,
                    _ => false,
                },
            })
            .collect();
        let mut items: Vec<ListItem> = self
            .rules
            .iter()
            .enumerate()
            .map(|(index, rule)| {
                let label = if rule.pattern.trim().is_empty() {
                    "(empty)"
                } else {
                    rule.pattern.as_str()
                };
                let color = theme.rule_colors[rules::mark_of(rule).color].marker;
                ListItem::entry(label, &describe(rule), index as u32).with_swatch(color)
            })
            .collect();
        let detail = if self.rules.len() >= MOST_RULES {
            format!("{MOST_RULES} rules is the most")
        } else {
            "type a pattern to add one".to_string()
        };
        items.push(ListItem::entry(NEW_RULE, &detail, self.rules.len() as u32));
        self.list.items = items;
        self.list.select_id(Some(self.selected as u32));
    }

    /// Whether an option does anything for the current selection, for screen readers.
    pub fn option_applies(&self, index: usize) -> bool {
        match index {
            MOVE_UP => !self.is_new() && self.selected > 0,
            MOVE_DOWN => self.selected + 1 < self.rules.len(),
            DELETE => !self.is_new(),
            _ => true,
        }
    }

    /// True for options that stay pressed, rather than acting once.
    pub fn option_is_toggle(index: usize) -> bool {
        index < TOGGLES
    }

    pub fn handle(&mut self, event: Event, canvas: &mut dyn Canvas) -> RulesResponse {
        match event {
            Event::MouseDown(point, ..) if !self.rect.contains(point) => {
                return RulesResponse {
                    closed: true,
                    ..RulesResponse::default()
                };
            }
            Event::KeyDown(Key::Escape, _) => {
                return RulesResponse {
                    closed: true,
                    ..RulesResponse::default()
                };
            }
            Event::KeyDown(Key::Tab, modifiers) if !modifiers.ctrl && !modifiers.alt => {
                self.focus = self.focus.step(modifiers.shift);
                return RulesResponse::redraw(false);
            }
            Event::KeyDown(key @ (Key::Up | Key::Down), modifiers)
                if modifiers.ctrl && !modifiers.alt =>
            {
                let changed = self.move_selected(key == Key::Up);
                return RulesResponse::redraw(changed);
            }
            Event::MouseDown(point, MouseButton::Left, _) => {
                return self.press(point, event, canvas)
            }
            Event::MouseMove(_) | Event::MouseUp(..) => {
                let field = self.field.handle(event, canvas, self.field_rect);
                let redraw = field.redraw
                    | self.colors.handle(event).redraw
                    | self.options.handle(event).redraw
                    | self.list.handle(event).redraw;
                return RulesResponse {
                    redraw,
                    ..RulesResponse::default()
                };
            }
            Event::Wheel(_) => {
                let redraw = self.list.handle(event).redraw;
                return RulesResponse {
                    redraw,
                    ..RulesResponse::default()
                };
            }
            _ => {}
        }
        match self.focus {
            Part::Field => self.field_event(event, canvas),
            Part::Colors => {
                let response = self.colors.handle(event);
                let changed = response
                    .toggled
                    .is_some_and(|index| self.choose_color(index));
                RulesResponse {
                    redraw: response.redraw,
                    changed,
                    ..RulesResponse::default()
                }
            }
            Part::Options => {
                let response = self.options.handle(event);
                let changed = response
                    .toggled
                    .is_some_and(|index| self.choose_option(index));
                RulesResponse {
                    redraw: response.redraw,
                    changed,
                    ..RulesResponse::default()
                }
            }
            Part::List => self.list_event(event),
        }
    }

    fn press(&mut self, point: Point, event: Event, canvas: &mut dyn Canvas) -> RulesResponse {
        if self.field_rect.contains(point) {
            self.focus = Part::Field;
            let response = self.field.handle(event, canvas, self.field_rect);
            return RulesResponse::redraw(response.changed);
        }
        if self.colors.contains(point) {
            self.focus = Part::Colors;
            let changed = self
                .colors
                .handle(event)
                .toggled
                .is_some_and(|index| self.choose_color(index));
            return RulesResponse::redraw(changed);
        }
        if self.options.contains(point) {
            self.focus = Part::Options;
            let changed = self
                .options
                .handle(event)
                .toggled
                .is_some_and(|index| self.choose_option(index));
            return RulesResponse::redraw(changed);
        }
        if self.list.contains(point) {
            self.focus = Part::List;
            let response = self.list.handle(event);
            let changed = match response.activated {
                Some(id) if id as usize != self.selected => self.select(id as usize),
                _ => false,
            };
            return RulesResponse::redraw(changed);
        }
        RulesResponse::default()
    }

    fn field_event(&mut self, event: Event, canvas: &mut dyn Canvas) -> RulesResponse {
        match event {
            Event::KeyDown(key @ (Key::Up | Key::Down), modifiers) if !modifiers.alt => {
                let index = if key == Key::Up {
                    self.selected.saturating_sub(1)
                } else {
                    self.selected + 1
                };
                let changed = self.select(index);
                return RulesResponse::redraw(changed);
            }
            // Enter finishes a rule and moves on to adding the next.
            Event::KeyDown(Key::Enter, _) => {
                let changed = self.select(self.rules.len());
                return RulesResponse::redraw(changed);
            }
            _ => {}
        }
        let response = self.field.handle(event, canvas, self.field_rect);
        let changed = response.changed && self.pattern_changed();
        RulesResponse {
            redraw: response.redraw || response.changed,
            changed,
            closed: false,
            copy: response.copy,
            wants_paste: response.wants_paste,
        }
    }

    fn list_event(&mut self, event: Event) -> RulesResponse {
        match event {
            Event::Char(' ') => return RulesResponse::redraw(self.choose_option(ON)),
            Event::KeyDown(Key::Delete, _) => {
                return RulesResponse::redraw(self.choose_option(DELETE));
            }
            Event::KeyDown(Key::Enter, _) => {
                self.focus = Part::Field;
                return RulesResponse::redraw(false);
            }
            _ => {}
        }
        let response = self.list.handle(event);
        let changed = match self.list.selected_id() {
            Some(id) if id as usize != self.selected => self.select(id as usize),
            _ => false,
        };
        RulesResponse {
            redraw: response.redraw || changed,
            changed,
            ..RulesResponse::default()
        }
    }

    /// Draws the panel and returns where the field's caret is, for the IME.
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
        let line = |top: f32| {
            Rect::new(
                rect.left + PADDING,
                top,
                rect.right - PADDING,
                top + LINE_HEIGHT,
            )
        };
        canvas.text(
            TITLE,
            line(rect.top + PADDING - 4.0),
            LABEL,
            theme.primary_text,
        );
        let caret = self.field.draw(
            canvas,
            self.field_rect,
            theme,
            self.focus == Part::Field,
            PLACEHOLDER,
        );
        self.colors.draw(
            canvas,
            self.colors_rect,
            &self.color_chips,
            theme,
            self.focus == Part::Colors,
        );
        self.options.draw(
            canvas,
            self.options_rect,
            &self.option_chips,
            theme,
            self.focus == Part::Options,
        );
        self.list.draw(canvas, theme, self.focus == Part::List);
        let (note, color) = match &self.problem {
            Some(problem) => (problem.as_str(), theme.error_text),
            None => (ORDER_NOTE, theme.secondary_text),
        };
        canvas.text(note, self.note_rect, LABEL, color);
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
    const CTRL: Modifiers = Modifiers {
        ctrl: true,
        shift: false,
        alt: false,
    };
    const AREA: Rect = Rect::new(0.0, 0.0, 1000.0, 700.0);

    fn rule(pattern: &str) -> Rule {
        Rule {
            pattern: pattern.to_string(),
            ..Rule::default()
        }
    }

    fn panel(rules: Vec<Rule>) -> RulesPanel {
        let mut panel = RulesPanel::new(rules);
        panel.arrange(AREA, &mut RecordingCanvas::new(), &Theme::CLASSIC);
        panel
    }

    fn send(panel: &mut RulesPanel, event: Event) -> RulesResponse {
        let response = panel.handle(event, &mut RecordingCanvas::new());
        panel.arrange(AREA, &mut RecordingCanvas::new(), &Theme::CLASSIC);
        response
    }

    fn type_text(panel: &mut RulesPanel, text: &str) -> bool {
        let mut changed = false;
        for character in text.chars() {
            changed |= send(panel, Event::Char(character)).changed;
        }
        changed
    }

    fn key(panel: &mut RulesPanel, key: Key, modifiers: Modifiers) -> RulesResponse {
        send(panel, Event::KeyDown(key, modifiers))
    }

    fn patterns(panel: &RulesPanel) -> Vec<&str> {
        panel
            .rules
            .iter()
            .map(|rule| rule.pattern.as_str())
            .collect()
    }

    #[test]
    fn typing_in_the_new_slot_adds_a_rule_and_enter_starts_another() {
        let mut panel = panel(Vec::new());
        assert!(type_text(&mut panel, "download failed"));
        assert_eq!(patterns(&panel), ["download failed"]);
        assert_eq!(panel.rules[0].color, "yellow");
        key(&mut panel, Key::Enter, NONE);
        assert_eq!(panel.field.text(), "");
        type_text(&mut panel, "level:error");
        assert_eq!(patterns(&panel), ["download failed", "level:error"]);
        assert_eq!(
            panel.rules[1].color, "green",
            "each new rule gets another color"
        );
    }

    #[test]
    fn arrows_load_each_rule_and_editing_changes_it() {
        let mut panel = panel(vec![rule("first"), rule("second")]);
        assert_eq!(panel.selected(), 2);
        key(&mut panel, Key::Up, NONE);
        assert_eq!(panel.field.text(), "second");
        type_text(&mut panel, " try");
        assert_eq!(patterns(&panel), ["first", "second try"]);
        key(&mut panel, Key::Up, NONE);
        assert_eq!(panel.field.text(), "first");
    }

    #[test]
    fn a_rule_cleared_out_goes_when_left() {
        let mut panel = panel(vec![rule("ab"), rule("keep")]);
        key(&mut panel, Key::Home, NONE);
        key(&mut panel, Key::Up, NONE);
        key(&mut panel, Key::Up, NONE);
        assert_eq!(panel.field.text(), "ab");
        key(&mut panel, Key::Backspace, NONE);
        key(&mut panel, Key::Backspace, NONE);
        assert_eq!(patterns(&panel), ["", "keep"], "kept while it's retyped");
        assert_eq!(panel.saved_rules().len(), 1);
        assert!(key(&mut panel, Key::Down, NONE).changed);
        assert_eq!(patterns(&panel), ["keep"]);
        assert_eq!(panel.field.text(), "keep");
    }

    #[test]
    fn colors_styles_and_switches_apply_to_the_selected_rule() {
        let mut panel = panel(vec![rule("first")]);
        panel.select(0);
        assert!(panel.choose_color(4));
        assert!(panel.choose_option(MARKER));
        assert!(panel.choose_option(ON));
        assert_eq!(panel.rules[0].color, "blue");
        assert_eq!(panel.rules[0].style, "marker");
        assert!(!panel.rules[0].enabled);
        panel.arrange(AREA, &mut RecordingCanvas::new(), &Theme::CLASSIC);
        assert!(panel.color_chips()[4].active);
        assert!(panel.option_chips()[MARKER].active && !panel.option_chips()[ON].active);
    }

    #[test]
    fn choices_made_in_the_new_slot_go_to_the_next_rule() {
        let mut panel = panel(Vec::new());
        assert!(!panel.choose_color(0), "no rule changed yet");
        panel.choose_option(MARKER);
        type_text(&mut panel, "x");
        assert_eq!(panel.rules[0].color, "red");
        assert_eq!(panel.rules[0].style, "marker");
    }

    #[test]
    fn rules_move_and_delete() {
        let mut panel = panel(vec![rule("a"), rule("b"), rule("c")]);
        panel.select(2);
        assert!(key(&mut panel, Key::Up, CTRL).changed);
        assert_eq!(patterns(&panel), ["a", "c", "b"]);
        assert_eq!(panel.selected(), 1);
        assert!(panel.choose_option(MOVE_DOWN));
        assert_eq!(patterns(&panel), ["a", "b", "c"]);
        assert!(!panel.choose_option(MOVE_DOWN), "already last");
        panel.select(0);
        assert!(!panel.choose_option(MOVE_UP), "already first");
        assert!(panel.choose_option(DELETE));
        assert_eq!(panel.field.text(), panel.rules[0].pattern);
        panel.select(panel.rules.len());
        assert!(
            !panel.choose_option(DELETE),
            "nothing to delete in the new slot"
        );
    }

    #[test]
    fn tab_cycles_through_the_parts_and_the_list_has_its_own_keys() {
        let mut panel = panel(vec![rule("a"), rule("b")]);
        for expected in [Part::Colors, Part::Options, Part::List, Part::Field] {
            key(&mut panel, Key::Tab, NONE);
            assert_eq!(panel.focus, expected);
        }
        panel.focus = Part::List;
        key(&mut panel, Key::Up, NONE);
        assert_eq!(panel.selected(), 1);
        assert!(send(&mut panel, Event::Char(' ')).changed);
        assert!(!panel.rules[1].enabled);
        assert!(key(&mut panel, Key::Delete, NONE).changed);
        assert_eq!(patterns(&panel), ["a"]);
    }

    #[test]
    fn mistakes_are_explained_but_kept() {
        let mut panel = panel(Vec::new());
        type_text(&mut panel, "/0x8007[/");
        assert!(panel
            .problem()
            .is_some_and(|problem| problem.starts_with("Not valid yet")));
        assert_eq!(patterns(&panel), ["/0x8007[/"]);
        key(&mut panel, Key::Backspace, NONE);
        type_text(&mut panel, "0-9]/");
        assert_eq!(patterns(&panel), ["/0x8007[0-9]/"]);
        assert_eq!(panel.problem(), None);
    }

    #[test]
    fn clicking_parts_focuses_them_and_outside_closes() {
        let mut panel = panel(vec![rule("a")]);
        let chip = panel.colors.chip_rect(5).unwrap();
        let point = Point::new(chip.left + 3.0, chip.top + 3.0);
        assert!(send(&mut panel, Event::MouseDown(point, MouseButton::Left, NONE)).redraw);
        assert_eq!(panel.focus, Part::Colors);
        let row = panel.list.item_rect(0).unwrap();
        let point = Point::new(row.left + 30.0, row.top + 5.0);
        assert!(send(&mut panel, Event::MouseDown(point, MouseButton::Left, NONE)).redraw);
        assert_eq!((panel.focus, panel.selected()), (Part::List, 0));
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
    fn the_panel_fits_small_windows_and_balances_clips() {
        let mut panel = RulesPanel::new(vec![rule("a")]);
        panel.arrange(
            Rect::new(0.0, 0.0, 300.0, 200.0),
            &mut RecordingCanvas::new(),
            &Theme::DARK,
        );
        assert!(panel.rect().width() <= 300.0 && panel.rect().bottom <= 200.0);
        let mut canvas = RecordingCanvas::new();
        panel.draw(&mut canvas, &Theme::DARK);
        assert!(canvas.clips_balance());
        assert!(canvas.texts().contains(&TITLE));
    }

    #[test]
    fn there_is_a_limit() {
        let many = (0..MOST_RULES)
            .map(|number| rule(&number.to_string()))
            .collect();
        let mut panel = panel(many);
        assert!(!type_text(&mut panel, "one too many"));
        assert_eq!(panel.rules.len(), MOST_RULES);
    }
}
