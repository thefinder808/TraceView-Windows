//! The widget toolkit's foundations: geometry, the drawing interface, theme
//! tokens, input events and shared widget behavior. It knows nothing about logs
//! or Windows, so it can be tested without a display.

#![forbid(unsafe_code)]

pub mod canvas;
pub mod geometry;
pub mod input;
pub mod recording;
pub mod scrollbar;
pub mod theme;

pub use canvas::{Align, Canvas, FontRole, TextStyle};
pub use geometry::{Color, Point, Rect};
pub use input::{Event, Key, Modifiers, MouseButton};
pub use theme::Theme;
