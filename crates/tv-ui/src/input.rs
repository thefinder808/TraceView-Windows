//! Input events, independent of how the platform delivers them.

use crate::geometry::Point;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Escape,
    Enter,
    Tab,
    Backspace,
    Delete,
    F(u8),
    /// The menu key, which opens a context menu.
    Menu,
    /// A letter or digit key, uppercase, regardless of keyboard state. The
    /// plus and minus keys, on the main keyboard or the number pad, are `=`
    /// and `-`.
    Char(char),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    KeyDown(Key, Modifiers),
    MouseDown(Point, MouseButton, Modifiers),
    MouseUp(Point, MouseButton),
    MouseMove(Point),
    /// Wheel movement in rows, positive when scrolling toward the file's start.
    Wheel(f32),
    /// The wheel turned with Ctrl held, which zooms. Positive is away from the
    /// user, which makes text larger.
    ZoomWheel(f32),
    /// A typed character, after keyboard layout and IME processing.
    Char(char),
}
