//! Waits for a command's new shortcut, once it's chosen from the list of
//! shortcuts. The next keys pressed become the shortcut, unless the app keeps
//! them for something else, which it says. Backspace removes the shortcut,
//! Delete restores the default, and Esc leaves it as it was.

use super::keymap::Shortcut;
use tv_ui::chip_bar::{self, Chip, ChipBar};
use tv_ui::{Align, Canvas, Event, FontRole, Key, MouseButton, Rect, TextStyle, Theme};

const WIDTH: f32 = 480.0;
const HEIGHT: f32 = 158.0;
/// How far below the top of its area the panel sits.
const OFFSET: f32 = 24.0;
const PADDING: f32 = 14.0;
const LINE_HEIGHT: f32 = 22.0;
const LABEL: TextStyle = TextStyle::new(FontRole::Ui, Align::Left);
const HINT: &str = "Backspace removes it     Delete restores the default     Esc cancels";

/// The buttons, for the mouse and screen readers, in order.
pub const BUTTONS: [&str; 3] = ["Remove", "Default", "Cancel"];

/// What to do with the command's shortcut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Set(Shortcut),
    Remove,
    Default,
    Cancel,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShortcutResponse {
    pub redraw: bool,
    /// The panel is done, and this is what was chosen.
    pub choice: Option<Choice>,
}

impl ShortcutResponse {
    fn chose(choice: Choice) -> Self {
        Self {
            redraw: true,
            choice: Some(choice),
        }
    }
}

pub struct ShortcutPanel {
    pub command: u32,
    label: String,
    current: Option<Shortcut>,
    default: Option<Shortcut>,
    /// Why the last keys pressed can't be used.
    problem: Option<String>,
    pub buttons: ChipBar,
    button_chips: Vec<Chip>,
    rect: Rect,
    buttons_rect: Rect,
}

fn shown(shortcut: Option<Shortcut>) -> String {
    shortcut.map_or_else(|| "none".to_string(), |shortcut| shortcut.label())
}

impl ShortcutPanel {
    pub fn new(
        command: u32,
        label: &str,
        current: Option<Shortcut>,
        default: Option<Shortcut>,
    ) -> Self {
        Self {
            command,
            label: label.to_string(),
            current,
            default,
            problem: None,
            buttons: ChipBar::default(),
            button_chips: Vec::new(),
            rect: Rect::default(),
            buttons_rect: Rect::default(),
        }
    }

    pub fn title(&self) -> String {
        format!("Shortcut for {}", self.label)
    }

    fn ask(&self) -> String {
        format!(
            "Press the new keys. Now {}, and by default {}.",
            shown(self.current),
            shown(self.default)
        )
    }

    /// What the panel asks for, and why the last keys didn't work, if they didn't.
    pub fn prompt(&self) -> String {
        let mut prompt = self.ask();
        if let Some(problem) = &self.problem {
            prompt.push(' ');
            prompt.push_str(problem);
            prompt.push('.');
        }
        prompt
    }

    #[cfg(test)]
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    pub fn button_chips(&self) -> &[Chip] {
        &self.button_chips
    }

    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// Presses one of the [`BUTTONS`].
    pub fn press(&self, index: usize) -> Option<Choice> {
        match index {
            0 => Some(Choice::Remove),
            1 => Some(Choice::Default),
            2 => Some(Choice::Cancel),
            _ => None,
        }
    }

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
        let top = self.rect.top + PADDING + 3.0 * LINE_HEIGHT;
        self.buttons_rect = Rect::new(
            self.rect.left + PADDING - 8.0,
            top,
            self.rect.right - PADDING,
            top + chip_bar::BAR_HEIGHT,
        );
        self.button_chips = BUTTONS
            .iter()
            .map(|label| Chip {
                label: (*label).to_string(),
                count: None,
                color: theme.primary_text,
                active: false,
            })
            .collect();
        self.buttons
            .arrange(self.buttons_rect, &self.button_chips, canvas);
    }

    pub fn handle(&mut self, event: Event) -> ShortcutResponse {
        match event {
            Event::MouseDown(point, ..) if !self.rect.contains(point) => {
                ShortcutResponse::chose(Choice::Cancel)
            }
            Event::MouseDown(_, MouseButton::Left, _) => {
                match self
                    .buttons
                    .handle(event)
                    .toggled
                    .and_then(|index| self.press(index))
                {
                    Some(choice) => ShortcutResponse::chose(choice),
                    None => ShortcutResponse::default(),
                }
            }
            Event::MouseMove(_) => ShortcutResponse {
                redraw: self.buttons.handle(event).redraw,
                choice: None,
            },
            Event::KeyDown(key, modifiers) => {
                let plain = !modifiers.ctrl && !modifiers.shift && !modifiers.alt;
                match key {
                    Key::Escape => return ShortcutResponse::chose(Choice::Cancel),
                    Key::Backspace if plain => return ShortcutResponse::chose(Choice::Remove),
                    Key::Delete if plain => return ShortcutResponse::chose(Choice::Default),
                    _ => {}
                }
                let checked = Shortcut::from_press(key, modifiers)
                    .ok_or_else(|| "That key can't be part of a shortcut".to_string())
                    .and_then(|shortcut| shortcut.check().map(|()| shortcut));
                match checked {
                    Ok(shortcut) => ShortcutResponse::chose(Choice::Set(shortcut)),
                    Err(problem) => {
                        self.problem = Some(problem);
                        ShortcutResponse {
                            redraw: true,
                            choice: None,
                        }
                    }
                }
            }
            _ => ShortcutResponse::default(),
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
        let line = |index: f32| {
            let top = rect.top + PADDING - 4.0 + index * LINE_HEIGHT;
            Rect::new(
                rect.left + PADDING,
                top,
                rect.right - PADDING,
                top + LINE_HEIGHT,
            )
        };
        canvas.text(&self.title(), line(0.0), LABEL, theme.primary_text);
        canvas.text(&self.ask(), line(1.0), LABEL, theme.primary_text);
        if let Some(problem) = &self.problem {
            canvas.text(problem, line(2.0), LABEL, theme.error_text);
        }
        self.buttons
            .draw(canvas, self.buttons_rect, &self.button_chips, theme, false);
        canvas.text(HINT, line(4.6), LABEL, theme.secondary_text);
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

    fn panel() -> ShortcutPanel {
        let current = Shortcut::parse("Ctrl+F").unwrap();
        let mut panel = ShortcutPanel::new(12, "Find", current, current);
        panel.arrange(
            Rect::new(0.0, 0.0, 1000.0, 600.0),
            &mut RecordingCanvas::new(),
            &Theme::CLASSIC,
        );
        panel
    }

    fn key(panel: &mut ShortcutPanel, key: Key, modifiers: Modifiers) -> Option<Choice> {
        panel.handle(Event::KeyDown(key, modifiers)).choice
    }

    #[test]
    fn the_keys_pressed_become_the_shortcut() {
        let mut panel = panel();
        assert_eq!(panel.title(), "Shortcut for Find");
        assert_eq!(
            panel.prompt(),
            "Press the new keys. Now Ctrl+F, and by default Ctrl+F."
        );
        assert_eq!(
            key(&mut panel, Key::Char('J'), CTRL),
            Some(Choice::Set(Shortcut::parse("Ctrl+J").unwrap().unwrap()))
        );
        assert!(matches!(
            key(&mut panel, Key::F(7), NONE),
            Some(Choice::Set(_))
        ));
    }

    #[test]
    fn keys_the_app_keeps_say_why_and_wait_for_others() {
        let mut panel = panel();
        assert_eq!(key(&mut panel, Key::Char('C'), CTRL), None);
        assert_eq!(panel.problem(), Some("Ctrl+C is kept for editing"));
        assert_eq!(key(&mut panel, Key::Char('J'), NONE), None);
        assert!(panel.problem().unwrap().contains("typing"));
        assert_eq!(key(&mut panel, Key::Up, CTRL), None);
        assert_eq!(
            panel.problem(),
            Some("That key can't be part of a shortcut")
        );
        assert!(panel
            .prompt()
            .ends_with("That key can't be part of a shortcut."));
    }

    #[test]
    fn backspace_delete_escape_and_the_buttons_choose_the_rest() {
        let mut panel = panel();
        assert_eq!(key(&mut panel, Key::Backspace, NONE), Some(Choice::Remove));
        assert_eq!(key(&mut panel, Key::Delete, NONE), Some(Choice::Default));
        assert_eq!(key(&mut panel, Key::Escape, NONE), Some(Choice::Cancel));
        let default = panel.buttons.chip_rect(1).unwrap();
        let point = Point::new(default.left + 3.0, default.top + 3.0);
        assert_eq!(
            panel
                .handle(Event::MouseDown(point, MouseButton::Left, NONE))
                .choice,
            Some(Choice::Default)
        );
        let outside = Point::new(2.0, 590.0);
        assert_eq!(
            panel
                .handle(Event::MouseDown(outside, MouseButton::Left, NONE))
                .choice,
            Some(Choice::Cancel)
        );
        let mut canvas = RecordingCanvas::new();
        panel.draw(&mut canvas, &Theme::CLASSIC);
        assert!(canvas.clips_balance());
    }
}
