//! A canvas that records what was drawn, so widgets can be tested without a
//! display or a graphics device.

use crate::canvas::{Canvas, FontRole, TextStyle};
use crate::geometry::{Color, Rect};

#[derive(Clone, Debug, PartialEq)]
pub enum DrawOp {
    Clear(Color),
    Fill(Rect, Color),
    Text(String, Rect, TextStyle, Color),
    PushClip(Rect),
    PopClip,
}

#[derive(Default)]
pub struct RecordingCanvas {
    pub ops: Vec<DrawOp>,
}

impl RecordingCanvas {
    pub fn new() -> Self {
        Self::default()
    }

    /// All text drawn, in order.
    pub fn texts(&self) -> Vec<&str> {
        self.ops
            .iter()
            .filter_map(|op| match op {
                DrawOp::Text(text, ..) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// True if every clip was closed, which the renderer requires.
    pub fn clips_balance(&self) -> bool {
        let mut depth = 0i32;
        for op in &self.ops {
            match op {
                DrawOp::PushClip(_) => depth += 1,
                DrawOp::PopClip => depth -= 1,
                _ => {}
            }
            if depth < 0 {
                return false;
            }
        }
        depth == 0
    }
}

impl Canvas for RecordingCanvas {
    fn clear(&mut self, color: Color) {
        self.ops.push(DrawOp::Clear(color));
    }

    fn fill_rect(&mut self, rect: Rect, color: Color) {
        self.ops.push(DrawOp::Fill(rect, color));
    }

    fn text(&mut self, text: &str, rect: Rect, style: TextStyle, color: Color) {
        self.ops
            .push(DrawOp::Text(text.to_string(), rect, style, color));
    }

    fn push_clip(&mut self, rect: Rect) {
        self.ops.push(DrawOp::PushClip(rect));
    }

    fn pop_clip(&mut self) {
        self.ops.push(DrawOp::PopClip);
    }

    fn char_width(&mut self, _font: FontRole) -> f32 {
        7.0
    }

    /// 15, which with normal spacing gives the usual 18-pixel row.
    fn line_height(&mut self, _font: FontRole) -> f32 {
        15.0
    }

    /// Every character is 7 wide here, which keeps test arithmetic simple.
    fn text_width(&mut self, text: &str, _font: FontRole) -> f32 {
        text.chars().count() as f32 * 7.0
    }

    fn text_block(&mut self, text: &str, rect: Rect, font: FontRole, color: Color) {
        self.ops.push(DrawOp::Text(
            text.to_string(),
            rect,
            TextStyle::new(font, crate::canvas::Align::Left),
            color,
        ));
    }
}
