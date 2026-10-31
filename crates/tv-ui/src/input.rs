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
    /// A letter or digit key, uppercase, regardless of keyboard state.
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
}
