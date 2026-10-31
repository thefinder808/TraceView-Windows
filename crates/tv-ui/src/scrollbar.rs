//! Scroll bar behavior for very long content. Positions are `f64` row numbers,
//! because `f32` can't address every row of a file with tens of millions of them.

use crate::geometry::{Point, Rect};

pub const WIDTH: f32 = 14.0;
const MIN_THUMB: f32 = 28.0;
const THUMB_INSET: f32 = 2.0;

/// The content's size and position, in rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollState {
    pub total: f64,
    pub visible: f64,
    pub top: f64,
}

impl ScrollState {
    pub fn max_top(&self) -> f64 {
        (self.total - self.visible).max(0.0)
    }
}

/// Where the mouse landed on the bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Press {
    /// On the thumb. Keeps the grab offset, so the thumb doesn't jump.
    Thumb { grab: f32 },
    /// Above the thumb, which pages up.
    Before,
    /// Below the thumb, which pages down.
    After,
}

fn thumb_height(track: &Rect, state: &ScrollState) -> f32 {
    let share = if state.total > 0.0 {
        (state.visible / state.total).min(1.0)
    } else {
        1.0
    };
    (track.height() * share as f32)
        .max(MIN_THUMB)
        .min(track.height())
}

/// The thumb's rectangle for the current position.
pub fn thumb_rect(track: &Rect, state: &ScrollState) -> Rect {
    let height = thumb_height(track, state);
    let travel = f64::from((track.height() - height).max(0.0));
    let max_top = state.max_top();
    let position = if max_top > 0.0 {
        (state.top / max_top).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let top = track.top + (position * travel) as f32;
    Rect::new(
        track.left + THUMB_INSET,
        top,
        track.right - THUMB_INSET,
        top + height,
    )
}

pub fn press_at(track: &Rect, state: &ScrollState, point: Point) -> Press {
    let thumb = thumb_rect(track, state);
    if point.y < thumb.top {
        Press::Before
    } else if point.y >= thumb.bottom {
        Press::After
    } else {
        Press::Thumb {
            grab: point.y - thumb.top,
        }
    }
}

/// The position for a thumb being dragged, kept to whole rows.
pub fn drag_position(track: &Rect, state: &ScrollState, mouse_y: f32, grab: f32) -> f64 {
    let height = thumb_height(track, state);
    let travel = f64::from((track.height() - height).max(0.0));
    if travel <= 0.0 {
        return 0.0;
    }
    let ratio = f64::from(mouse_y - grab - track.top) / travel;
    (ratio.clamp(0.0, 1.0) * state.max_top()).round()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRACK: Rect = Rect::new(986.0, 0.0, 1000.0, 800.0);

    fn state(total: f64, top: f64) -> ScrollState {
        ScrollState {
            total,
            visible: 40.0,
            top,
        }
    }

    #[test]
    fn huge_content_keeps_a_grabbable_thumb() {
        let thumb = thumb_rect(&TRACK, &state(50_000_000.0, 0.0));
        assert_eq!(thumb.height(), MIN_THUMB);
    }

    #[test]
    fn thumb_reaches_both_ends_of_the_track() {
        let content = state(50_000_000.0, 0.0);
        assert_eq!(thumb_rect(&TRACK, &content).top, TRACK.top);
        let at_end = state(50_000_000.0, content.max_top());
        assert_eq!(thumb_rect(&TRACK, &at_end).bottom, TRACK.bottom);
    }

    #[test]
    fn dragging_to_the_bottom_reaches_the_last_row_exactly() {
        let content = state(47_815_464.0, 0.0);
        let top = drag_position(&TRACK, &content, TRACK.bottom + 100.0, 0.0);
        assert_eq!(top, content.max_top());
        assert_eq!(top, 47_815_424.0);
        assert_eq!(drag_position(&TRACK, &content, -50.0, 0.0), 0.0);
    }

    #[test]
    fn grabbing_the_thumb_does_not_move_it() {
        let content = state(1_000_000.0, 500_000.0);
        let thumb = thumb_rect(&TRACK, &content);
        let mouse_y = thumb.top + 10.0;
        let Press::Thumb { grab } = press_at(&TRACK, &content, Point::new(990.0, mouse_y)) else {
            panic!("expected to press the thumb");
        };
        let top = drag_position(&TRACK, &content, mouse_y, grab);
        assert!((top - content.top).abs() < 100.0, "moved to {top}");
    }

    #[test]
    fn clicking_either_side_of_the_thumb_pages() {
        let content = state(1_000_000.0, 500_000.0);
        let thumb = thumb_rect(&TRACK, &content);
        assert_eq!(
            press_at(&TRACK, &content, Point::new(990.0, thumb.top - 1.0)),
            Press::Before
        );
        assert_eq!(
            press_at(&TRACK, &content, Point::new(990.0, thumb.bottom + 1.0)),
            Press::After
        );
    }

    #[test]
    fn content_that_fits_fills_the_track_and_cannot_move() {
        let content = state(10.0, 0.0);
        assert_eq!(thumb_rect(&TRACK, &content).height(), TRACK.height());
        assert_eq!(drag_position(&TRACK, &content, 400.0, 0.0), 0.0);
    }

    #[test]
    fn empty_content_is_safe() {
        let content = state(0.0, 0.0);
        let _ = thumb_rect(&TRACK, &content);
        assert_eq!(content.max_top(), 0.0);
    }
}
