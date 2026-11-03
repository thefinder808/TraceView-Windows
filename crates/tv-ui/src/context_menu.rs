//! A menu that opens where it's asked for, as when a row is right-clicked,
//! and closes once an item is chosen or a click lands anywhere else. It looks
//! and works like the menu bar's menus: arrows move, Enter chooses, Esc closes.

use crate::canvas::Canvas;
use crate::geometry::{Point, Rect};
use crate::input::{Event, Key};
use crate::menu::{draw_items, popup_width, step, MenuItem};
use crate::theme::Theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContextResponse {
    pub redraw: bool,
    /// The menu is done, with a command chosen or not.
    pub closed: bool,
    pub command: Option<u32>,
}

impl ContextResponse {
    const CLOSED: ContextResponse = ContextResponse {
        redraw: true,
        closed: true,
        command: None,
    };
}

#[derive(Clone, Debug)]
pub struct ContextMenu {
    pub items: Vec<MenuItem>,
    /// Where it was asked for, which is its top left corner unless that
    /// wouldn't fit.
    at: Point,
    highlighted: Option<usize>,
    popup: Rect,
    item_rects: Vec<Rect>,
}

impl ContextMenu {
    /// Opens at a point. Opened from the keyboard, its first item starts
    /// highlighted, so arrows and Enter work at once.
    pub fn new(items: Vec<MenuItem>, at: Point, from_keyboard: bool) -> Self {
        let highlighted = if from_keyboard {
            step(&items, None, true)
        } else {
            None
        };
        Self {
            items,
            at,
            highlighted,
            popup: Rect::default(),
            item_rects: Vec::new(),
        }
    }

    /// Lays it out inside `area`, opening left of or above the point instead
    /// when it wouldn't fit to the right or below.
    pub fn arrange(&mut self, area: Rect, canvas: &mut dyn Canvas) {
        let width = popup_width(&self.items, canvas);
        let height: f32 = self.items.iter().map(MenuItem::height).sum();
        let left = if self.at.x + width > area.right {
            (self.at.x - width).max(area.left)
        } else {
            self.at.x
        };
        let top = if self.at.y + height > area.bottom {
            (self.at.y - height).max(area.top)
        } else {
            self.at.y
        };
        self.popup = Rect::new(left, top, left + width, top + height);
        self.item_rects.clear();
        let mut y = top;
        for item in &self.items {
            let bottom = y + item.height();
            self.item_rects
                .push(Rect::new(left, y, left + width, bottom));
            y = bottom;
        }
    }

    pub fn rect(&self) -> Rect {
        self.popup
    }

    pub fn item_rect(&self, item: usize) -> Option<Rect> {
        self.item_rects.get(item).copied()
    }

    pub fn highlighted_item(&self) -> Option<usize> {
        self.highlighted
    }

    fn item_at(&self, point: Point) -> Option<usize> {
        self.item_rects.iter().position(|rect| rect.contains(point))
    }

    /// Chooses an item, if it's a command that's enabled.
    pub fn activate(&mut self, item: usize) -> ContextResponse {
        match self.items.get(item) {
            Some(MenuItem::Command {
                command,
                enabled: true,
                ..
            }) => ContextResponse {
                command: Some(*command),
                ..ContextResponse::CLOSED
            },
            _ => ContextResponse::default(),
        }
    }

    /// Takes every event while it's open, as a menu does.
    pub fn handle(&mut self, event: Event) -> ContextResponse {
        match event {
            Event::KeyDown(Key::Up | Key::Down, modifiers) if !modifiers.alt => {
                let forward = matches!(event, Event::KeyDown(Key::Down, _));
                self.highlighted =
                    step(&self.items, self.highlighted, forward).or(self.highlighted);
                ContextResponse {
                    redraw: true,
                    ..ContextResponse::default()
                }
            }
            Event::KeyDown(Key::Enter, _) => match self.highlighted {
                Some(item) => self.activate(item),
                None => ContextResponse::default(),
            },
            Event::KeyDown(Key::Escape | Key::F(10) | Key::Menu, _) => ContextResponse::CLOSED,
            Event::KeyDown(_, modifiers) if modifiers.alt => ContextResponse::CLOSED,
            Event::MouseMove(point) => {
                let hovered = self
                    .item_at(point)
                    .filter(|item| self.items[*item].is_enabled_command());
                let redraw = hovered.is_some() && hovered != self.highlighted;
                if hovered.is_some() {
                    self.highlighted = hovered;
                }
                ContextResponse {
                    redraw,
                    ..ContextResponse::default()
                }
            }
            Event::MouseDown(point, ..) => match self.item_at(point) {
                Some(item) => self.activate(item),
                None if self.popup.contains(point) => ContextResponse::default(),
                None => ContextResponse::CLOSED,
            },
            _ => ContextResponse::default(),
        }
    }

    /// Draws it. Call after everything it covers.
    pub fn draw(&self, canvas: &mut dyn Canvas, theme: &Theme) {
        let popup = self.popup;
        // The shared drawing leaves the top open, for the menu bar's popups.
        canvas.fill_rect(
            Rect::new(
                popup.left - 1.0,
                popup.top - 1.0,
                popup.right + 1.0,
                popup.top,
            ),
            theme.border,
        );
        draw_items(
            canvas,
            popup,
            &self.items,
            &self.item_rects,
            self.highlighted,
            theme,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Modifiers, MouseButton};
    use crate::recording::RecordingCanvas;

    const NONE: Modifiers = Modifiers {
        ctrl: false,
        shift: false,
        alt: false,
    };
    const AREA: Rect = Rect::new(0.0, 0.0, 800.0, 600.0);

    fn menu(at: Point, from_keyboard: bool) -> ContextMenu {
        let mut disabled = MenuItem::command("Paste", "", 3);
        if let MenuItem::Command { enabled, .. } = &mut disabled {
            *enabled = false;
        }
        let mut menu = ContextMenu::new(
            vec![
                MenuItem::command("Add bookmark", "Ctrl+D", 1),
                disabled,
                MenuItem::Separator,
                MenuItem::command("Copy", "Ctrl+C", 2),
            ],
            at,
            from_keyboard,
        );
        menu.arrange(AREA, &mut RecordingCanvas::new());
        menu
    }

    fn center(rect: Rect) -> Point {
        Point::new(
            (rect.left + rect.right) / 2.0,
            (rect.top + rect.bottom) / 2.0,
        )
    }

    #[test]
    fn clicking_an_item_chooses_it_and_clicking_elsewhere_closes() {
        let mut menu = menu(Point::new(100.0, 100.0), false);
        assert_eq!(menu.rect().left, 100.0);
        assert_eq!(menu.highlighted_item(), None);
        let copy = center(menu.item_rect(3).unwrap());
        let chosen = menu.handle(Event::MouseDown(copy, MouseButton::Left, NONE));
        assert_eq!(chosen.command, Some(2));
        assert!(chosen.closed);
        let mut menu = self::menu(Point::new(100.0, 100.0), false);
        let disabled = center(menu.item_rect(1).unwrap());
        assert!(
            !menu
                .handle(Event::MouseDown(disabled, MouseButton::Left, NONE))
                .closed
        );
        let away = menu.handle(Event::MouseDown(
            Point::new(700.0, 500.0),
            MouseButton::Right,
            NONE,
        ));
        assert_eq!((away.closed, away.command), (true, None));
    }

    #[test]
    fn keys_move_past_what_cant_be_chosen() {
        let mut menu = menu(Point::new(100.0, 100.0), true);
        assert_eq!(
            menu.highlighted_item(),
            Some(0),
            "from the keyboard, the first item"
        );
        menu.handle(Event::KeyDown(Key::Down, NONE));
        assert_eq!(
            menu.highlighted_item(),
            Some(3),
            "past the disabled item and separator"
        );
        menu.handle(Event::KeyDown(Key::Down, NONE));
        assert_eq!(menu.highlighted_item(), Some(0), "around to the top");
        assert_eq!(
            menu.handle(Event::KeyDown(Key::Enter, NONE)).command,
            Some(1)
        );
        assert!(menu.handle(Event::KeyDown(Key::Escape, NONE)).closed);
    }

    #[test]
    fn it_opens_left_and_above_near_the_edges() {
        let menu = menu(Point::new(790.0, 590.0), false);
        let rect = menu.rect();
        assert!(rect.right <= 790.0 && rect.bottom <= 590.0);
        assert!(rect.left >= 0.0 && rect.top >= 0.0);
        let mut canvas = RecordingCanvas::new();
        menu.draw(&mut canvas, &Theme::CLASSIC);
        assert!(canvas.texts().contains(&"Add bookmark"));
        assert!(canvas.clips_balance());
    }
}
