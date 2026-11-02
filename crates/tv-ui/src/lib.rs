//! The widget toolkit's foundations: geometry, the drawing interface, theme
//! tokens, input events and shared widget behavior. It knows nothing about logs
//! or Windows, so it can be tested without a display.

#![forbid(unsafe_code)]

pub mod canvas;
pub mod chip_bar;
pub mod geometry;
pub mod input;
pub mod list_view;
pub mod menu;
pub mod palette;
pub mod recording;
pub mod scrollbar;
pub mod splitter;
pub mod tab_bar;
pub mod text_field;
pub mod theme;

pub use canvas::{Align, Canvas, FontRole, TextStyle};
pub use geometry::{Color, Point, Rect};
pub use input::{Event, Key, Modifiers, MouseButton};
pub use theme::Theme;
