//! The drawing interface. Everything above it is unaware of how pixels get made,
//! which keeps the choice of renderer reversible.

use crate::geometry::{Color, Rect};

/// Which of the theme's fonts to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontRole {
    /// Interface text: tabs, labels, the status bar.
    Ui,
    /// Fixed-width log text.
    Grid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Align {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextStyle {
    pub font: FontRole,
    pub align: Align,
}

impl TextStyle {
    pub const fn new(font: FontRole, align: Align) -> Self {
        Self { font, align }
    }
}

pub trait Canvas {
    fn clear(&mut self, color: Color);
    fn fill_rect(&mut self, rect: Rect, color: Color);
    /// Draws one line of text, vertically centered and clipped to `rect`.
    fn text(&mut self, text: &str, rect: Rect, style: TextStyle, color: Color);
    fn push_clip(&mut self, rect: Rect);
    fn pop_clip(&mut self);
    /// The width of one character in a fixed-width font, for sizing columns.
    fn char_width(&mut self, font: FontRole) -> f32;
}
