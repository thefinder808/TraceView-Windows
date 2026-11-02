//! A menu bar drawn by the app, so it follows the theme. Native Win32 menus can't
//! follow dark themes. Works with the mouse, arrow keys, F10 and Alt+letter.

use crate::canvas::{Align, Canvas, FontRole, TextStyle};
use crate::geometry::{Point, Rect};
use crate::input::{Event, Key, MouseButton};
use crate::theme::Theme;

pub const BAR_HEIGHT: f32 = 24.0;
const ITEM_HEIGHT: f32 = 24.0;
const SEPARATOR_HEIGHT: f32 = 9.0;
const TITLE_PADDING: f32 = 10.0;
const ITEM_PADDING: f32 = 24.0;
const SHORTCUT_GAP: f32 = 32.0;
const UI_LEFT: TextStyle = TextStyle::new(FontRole::Ui, Align::Left);
const UI_RIGHT: TextStyle = TextStyle::new(FontRole::Ui, Align::Right);

#[derive(Clone, Debug, PartialEq)]
pub enum MenuItem {
    Command {
        label: String,
        shortcut: String,
        command: u32,
        enabled: bool,
        checked: bool,
    },
    Separator,
}

impl MenuItem {
    pub fn command(label: &str, shortcut: &str, command: u32) -> Self {
        MenuItem::Command {
            label: label.to_string(),
            shortcut: shortcut.to_string(),
            command,
            enabled: true,
            checked: false,
        }
    }

    pub fn is_enabled_command(&self) -> bool {
        matches!(self, MenuItem::Command { enabled: true, .. })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub title: String,
    pub items: Vec<MenuItem>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuResponse {
    pub redraw: bool,
    /// The input was meant for the menu, so nothing else should act on it.
    pub consumed: bool,
    pub command: Option<u32>,
}

impl MenuResponse {
    const CONSUMED: MenuResponse = MenuResponse {
        redraw: true,
        consumed: true,
        command: None,
    };
}

#[derive(Clone, Debug, Default)]
pub struct MenuBar {
    pub menus: Vec<Menu>,
    open: Option<usize>,
    highlighted: Option<usize>,
    /// True when the bar has keyboard focus with no menu open, after F10.
    focused: bool,
    /// Which title is highlighted while the bar has focus but no menu is open.
    title_focus: usize,
    title_rects: Vec<Rect>,
    popup: Rect,
    item_rects: Vec<Rect>,
}

impl MenuBar {
    pub fn new(menus: Vec<Menu>) -> Self {
        Self {
            menus,
            ..Self::default()
        }
    }

    pub fn is_active(&self) -> bool {
        self.open.is_some() || self.focused
    }

    pub fn open_menu(&self) -> Option<usize> {
        self.open
    }

    pub fn highlighted_item(&self) -> Option<usize> {
        self.highlighted
    }

    pub fn close(&mut self) {
        self.open = None;
        self.highlighted = None;
        self.focused = false;
    }

    fn open(&mut self, menu: usize) {
        self.open = Some(menu);
        self.focused = false;
        self.highlighted = self.menus[menu]
            .items
            .iter()
            .position(MenuItem::is_enabled_command);
    }

    /// Lays out titles and the open popup. Call before drawing or handling input.
    pub fn arrange(&mut self, bar: Rect, canvas: &mut dyn Canvas) {
        self.title_rects.clear();
        let mut left = bar.left;
        for menu in &self.menus {
            let width = canvas.text_width(&menu.title, FontRole::Ui) + TITLE_PADDING * 2.0;
            self.title_rects
                .push(Rect::new(left, bar.top, left + width, bar.bottom));
            left += width;
        }
        self.item_rects.clear();
        let Some(open) = self.open else {
            self.popup = Rect::default();
            return;
        };
        let items = &self.menus[open].items;
        let mut label_width: f32 = 0.0;
        let mut shortcut_width: f32 = 0.0;
        for item in items {
            if let MenuItem::Command {
                label, shortcut, ..
            } = item
            {
                label_width = label_width.max(canvas.text_width(label, FontRole::Ui));
                shortcut_width = shortcut_width.max(canvas.text_width(shortcut, FontRole::Ui));
            }
        }
        let width = ITEM_PADDING * 2.0 + label_width + SHORTCUT_GAP + shortcut_width;
        let left = self.title_rects[open].left;
        let mut top = bar.bottom;
        for item in items {
            let height = match item {
                MenuItem::Command { .. } => ITEM_HEIGHT,
                MenuItem::Separator => SEPARATOR_HEIGHT,
            };
            self.item_rects
                .push(Rect::new(left, top, left + width, top + height));
            top += height;
        }
        self.popup = Rect::new(left, bar.bottom, left + width, top);
    }

    fn step_item(&mut self, forward: bool) {
        let Some(open) = self.open else { return };
        let items = &self.menus[open].items;
        if items.is_empty() {
            return;
        }
        let count = items.len();
        let mut index = self
            .highlighted
            .unwrap_or(if forward { count - 1 } else { 0 });
        for _ in 0..count {
            index = if forward {
                (index + 1) % count
            } else {
                (index + count - 1) % count
            };
            if items[index].is_enabled_command() {
                self.highlighted = Some(index);
                return;
            }
        }
    }

    fn activate(&mut self, item: usize) -> MenuResponse {
        let Some(open) = self.open else {
            return MenuResponse::default();
        };
        match &self.menus[open].items[item] {
            MenuItem::Command {
                command,
                enabled: true,
                ..
            } => {
                let command = *command;
                self.close();
                MenuResponse {
                    redraw: true,
                    consumed: true,
                    command: Some(command),
                }
            }
            _ => MenuResponse::CONSUMED,
        }
    }

    fn handle_key(&mut self, key: Key, alt: bool) -> MenuResponse {
        let count = self.menus.len();
        if count == 0 {
            return MenuResponse::default();
        }
        if alt {
            if let Key::Char(letter) = key {
                let found = self.menus.iter().position(|menu| {
                    menu.title
                        .chars()
                        .next()
                        .is_some_and(|first| first.eq_ignore_ascii_case(&letter))
                });
                if let Some(menu) = found {
                    self.open(menu);
                    return MenuResponse::CONSUMED;
                }
            }
            return MenuResponse::default();
        }
        if key == Key::F(10) {
            if self.is_active() {
                self.close();
            } else {
                self.focused = true;
                self.highlighted = None;
                self.open = None;
                self.title_focus = 0;
            }
            return MenuResponse::CONSUMED;
        }
        if !self.is_active() {
            return MenuResponse::default();
        }
        let current = self.open.unwrap_or(self.title_focus);
        match key {
            Key::Escape => {
                if self.open.is_some() {
                    self.open = None;
                    self.highlighted = None;
                    self.focused = true;
                    self.title_focus = current;
                } else {
                    self.close();
                }
            }
            Key::Left => {
                let menu = (current + count - 1) % count;
                if self.open.is_some() {
                    self.open(menu)
                } else {
                    self.title_focus = menu
                }
            }
            Key::Right => {
                let menu = (current + 1) % count;
                if self.open.is_some() {
                    self.open(menu)
                } else {
                    self.title_focus = menu
                }
            }
            Key::Down | Key::Enter if self.open.is_none() => self.open(current),
            Key::Down => self.step_item(true),
            Key::Up => self.step_item(false),
            Key::Enter => {
                if let Some(item) = self.highlighted {
                    return self.activate(item);
                }
            }
            _ => {}
        }
        MenuResponse::CONSUMED
    }

    pub fn handle(&mut self, event: Event) -> MenuResponse {
        match event {
            Event::KeyDown(key, modifiers) => self.handle_key(key, modifiers.alt),
            Event::MouseDown(point, MouseButton::Left, _) => {
                if let Some(menu) = self
                    .title_rects
                    .iter()
                    .position(|rect| rect.contains(point))
                {
                    if self.open == Some(menu) {
                        self.close();
                    } else {
                        self.open(menu);
                    }
                    return MenuResponse::CONSUMED;
                }
                if self.open.is_some() {
                    if let Some(item) = self.item_rects.iter().position(|rect| rect.contains(point))
                    {
                        return self.activate(item);
                    }
                    // A click outside an open menu only closes it.
                    self.close();
                    return MenuResponse::CONSUMED;
                }
                MenuResponse::default()
            }
            Event::MouseMove(point) => {
                if self.open.is_none() {
                    return MenuResponse::default();
                }
                if let Some(menu) = self
                    .title_rects
                    .iter()
                    .position(|rect| rect.contains(point))
                {
                    if self.open != Some(menu) {
                        self.open(menu);
                        return MenuResponse::CONSUMED;
                    }
                }
                let hovered = self.item_rects.iter().position(|rect| rect.contains(point));
                if let Some(item) = hovered {
                    let enabled = self
                        .open
                        .is_some_and(|open| self.menus[open].items[item].is_enabled_command());
                    if enabled && self.highlighted != Some(item) {
                        self.highlighted = Some(item);
                        return MenuResponse::CONSUMED;
                    }
                }
                MenuResponse {
                    consumed: true,
                    ..MenuResponse::default()
                }
            }
            Event::MouseUp(..) | Event::Wheel(_) | Event::Char(_) if self.open.is_some() => {
                MenuResponse {
                    consumed: true,
                    ..MenuResponse::default()
                }
            }
            _ => MenuResponse::default(),
        }
    }

    pub fn draw_bar(&self, canvas: &mut dyn Canvas, bar: Rect, theme: &Theme) {
        canvas.fill_rect(bar, theme.panel_background);
        for (index, (menu, rect)) in self.menus.iter().zip(&self.title_rects).enumerate() {
            let active = self.open == Some(index) || (self.focused && self.title_focus == index);
            if active {
                canvas.fill_rect(*rect, theme.hover_background);
            }
            let label = Rect::new(rect.left + TITLE_PADDING, rect.top, rect.right, rect.bottom);
            canvas.text(&menu.title, label, UI_LEFT, theme.primary_text);
        }
    }

    /// Draws the open menu. Call last, so it covers everything beneath it.
    pub fn draw_popup(&self, canvas: &mut dyn Canvas, theme: &Theme) {
        let Some(open) = self.open else { return };
        let popup = self.popup;
        canvas.fill_rect(
            Rect::new(
                popup.left - 1.0,
                popup.top,
                popup.right + 1.0,
                popup.bottom + 1.0,
            ),
            theme.border,
        );
        canvas.fill_rect(popup, theme.panel_background);
        for (index, (item, rect)) in self.menus[open]
            .items
            .iter()
            .zip(&self.item_rects)
            .enumerate()
        {
            match item {
                MenuItem::Separator => {
                    let middle = (rect.top + rect.bottom) / 2.0;
                    canvas.fill_rect(
                        Rect::new(rect.left + 8.0, middle, rect.right - 8.0, middle + 1.0),
                        theme.border,
                    );
                }
                MenuItem::Command {
                    label,
                    shortcut,
                    enabled,
                    checked,
                    ..
                } => {
                    if self.highlighted == Some(index) {
                        canvas.fill_rect(*rect, theme.hover_background);
                    }
                    let color = if *enabled {
                        theme.primary_text
                    } else {
                        theme.placeholder_text
                    };
                    if *checked {
                        canvas.text(
                            "\u{2713}",
                            Rect::new(
                                rect.left + 6.0,
                                rect.top,
                                rect.left + ITEM_PADDING,
                                rect.bottom,
                            ),
                            UI_LEFT,
                            color,
                        );
                    }
                    let inner = Rect::new(
                        rect.left + ITEM_PADDING,
                        rect.top,
                        rect.right - ITEM_PADDING,
                        rect.bottom,
                    );
                    canvas.text(label, inner, UI_LEFT, color);
                    canvas.text(shortcut, inner, UI_RIGHT, theme.secondary_text);
                }
            }
        }
    }

    pub fn title_rect(&self, menu: usize) -> Option<Rect> {
        self.title_rects.get(menu).copied()
    }

    pub fn item_rect(&self, item: usize) -> Option<Rect> {
        self.item_rects.get(item).copied()
    }

    pub fn contains(&self, point: Point) -> bool {
        self.title_rects.iter().any(|rect| rect.contains(point))
            || (self.open.is_some() && self.popup.contains(point))
    }
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
    const ALT: Modifiers = Modifiers {
        ctrl: false,
        shift: false,
        alt: true,
    };
    const BAR: Rect = Rect::new(0.0, 0.0, 800.0, BAR_HEIGHT);

    fn bar() -> MenuBar {
        let mut disabled = MenuItem::command("Paste", "Ctrl+V", 12);
        if let MenuItem::Command { enabled, .. } = &mut disabled {
            *enabled = false;
        }
        let mut bar = MenuBar::new(vec![
            Menu {
                title: "File".to_string(),
                items: vec![
                    MenuItem::command("Open...", "Ctrl+O", 1),
                    MenuItem::Separator,
                    MenuItem::command("Exit", "", 2),
                ],
            },
            Menu {
                title: "Edit".to_string(),
                items: vec![MenuItem::command("Copy", "Ctrl+C", 11), disabled],
            },
        ]);
        bar.arrange(BAR, &mut RecordingCanvas::new());
        bar
    }

    fn key(bar: &mut MenuBar, key: Key, modifiers: Modifiers) -> MenuResponse {
        let response = bar.handle(Event::KeyDown(key, modifiers));
        bar.arrange(BAR, &mut RecordingCanvas::new());
        response
    }

    fn click(bar: &mut MenuBar, point: Point) -> MenuResponse {
        let response = bar.handle(Event::MouseDown(point, MouseButton::Left, NONE));
        bar.arrange(BAR, &mut RecordingCanvas::new());
        response
    }

    fn center(rect: Rect) -> Point {
        Point::new(
            (rect.left + rect.right) / 2.0,
            (rect.top + rect.bottom) / 2.0,
        )
    }

    #[test]
    fn clicking_a_title_opens_and_an_item_runs_its_command() {
        let mut bar = bar();
        {
            let point = center(bar.title_rect(0).unwrap());
            click(&mut bar, point)
        };
        assert_eq!(bar.open_menu(), Some(0));
        let response = {
            let point = center(bar.item_rect(2).unwrap());
            click(&mut bar, point)
        };
        assert_eq!(response.command, Some(2));
        assert_eq!(bar.open_menu(), None);
    }

    #[test]
    fn clicking_outside_closes_without_a_command() {
        let mut bar = bar();
        {
            let point = center(bar.title_rect(0).unwrap());
            click(&mut bar, point)
        };
        let response = click(&mut bar, Point::new(700.0, 500.0));
        assert!(response.consumed);
        assert_eq!(response.command, None);
        assert_eq!(bar.open_menu(), None);
    }

    #[test]
    fn separators_and_disabled_items_do_nothing() {
        let mut bar = bar();
        {
            let point = center(bar.title_rect(0).unwrap());
            click(&mut bar, point)
        };
        assert_eq!(
            {
                let point = center(bar.item_rect(1).unwrap());
                click(&mut bar, point)
            }
            .command,
            None
        );
        {
            let point = center(bar.title_rect(1).unwrap());
            click(&mut bar, point)
        };
        assert_eq!(
            {
                let point = center(bar.item_rect(1).unwrap());
                click(&mut bar, point)
            }
            .command,
            None
        );
    }

    #[test]
    fn alt_letter_opens_and_arrows_navigate() {
        let mut bar = bar();
        key(&mut bar, Key::Char('F'), ALT);
        assert_eq!(bar.open_menu(), Some(0));
        assert_eq!(bar.highlighted_item(), Some(0));
        key(&mut bar, Key::Down, NONE);
        assert_eq!(bar.highlighted_item(), Some(2), "skips the separator");
        key(&mut bar, Key::Down, NONE);
        assert_eq!(bar.highlighted_item(), Some(0), "wraps around");
        key(&mut bar, Key::Right, NONE);
        assert_eq!(bar.open_menu(), Some(1));
        key(&mut bar, Key::Down, NONE);
        assert_eq!(bar.highlighted_item(), Some(0), "skips the disabled item");
        assert_eq!(key(&mut bar, Key::Enter, NONE).command, Some(11));
    }

    #[test]
    fn f10_focuses_the_bar_and_escape_backs_out() {
        let mut bar = bar();
        key(&mut bar, Key::F(10), NONE);
        assert!(bar.is_active());
        assert_eq!(bar.open_menu(), None);
        key(&mut bar, Key::Down, NONE);
        assert_eq!(bar.open_menu(), Some(0));
        key(&mut bar, Key::Escape, NONE);
        assert_eq!(bar.open_menu(), None);
        assert!(bar.is_active());
        key(&mut bar, Key::Escape, NONE);
        assert!(!bar.is_active());
    }

    #[test]
    fn keys_pass_through_when_the_bar_is_idle() {
        let mut bar = bar();
        assert!(!key(&mut bar, Key::Down, NONE).consumed);
        assert!(!key(&mut bar, Key::Char('Q'), ALT).consumed);
    }

    #[test]
    fn hovering_another_title_switches_menus() {
        let mut bar = bar();
        {
            let point = center(bar.title_rect(0).unwrap());
            click(&mut bar, point)
        };
        {
            let point = center(bar.title_rect(1).unwrap());
            bar.handle(Event::MouseMove(point))
        };
        assert_eq!(bar.open_menu(), Some(1));
    }

    #[test]
    fn the_popup_draws_its_items() {
        let mut bar = bar();
        {
            let point = center(bar.title_rect(0).unwrap());
            click(&mut bar, point)
        };
        let mut canvas = RecordingCanvas::new();
        bar.draw_bar(&mut canvas, BAR, &Theme::CLASSIC);
        bar.draw_popup(&mut canvas, &Theme::CLASSIC);
        let texts = canvas.texts();
        assert!(texts.contains(&"Open..."));
        assert!(texts.contains(&"Ctrl+O"));
        assert!(canvas.clips_balance());
    }
}
