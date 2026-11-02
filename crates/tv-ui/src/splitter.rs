//! A draggable divider between two panes, such as the sidebar and the grid.
//! The line draws 1 px wide, but the mouse can grab it a few pixels either side,
//! and with focus the arrow keys move it.

use crate::canvas::Canvas;
use crate::geometry::{Point, Rect};
use crate::input::{Event, Key, MouseButton};
use crate::theme::Theme;

/// The drawn line's thickness.
pub const LINE: f32 = 1.0;
/// How far either side of the line the mouse can grab it.
const REACH: f32 = 3.0;
/// How far one arrow key press moves the line.
const KEY_STEP: f32 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    /// A vertical line with panes to its left and right, which moves sideways.
    Vertical,
    /// A horizontal line with panes above and below, which moves up and down.
    Horizontal,
}

/// What a splitter needs its owner to do after handling input.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SplitterResponse {
    pub redraw: bool,
    /// The first pane's new size, when it changed.
    pub position: Option<f32>,
}

#[derive(Clone, Debug)]
pub struct Splitter {
    pub orientation: Orientation,
    /// The smallest the first pane, on the left or top, can be.
    pub min_first: f32,
    /// The smallest the second pane, on the right or bottom, can be.
    pub min_second: f32,
    /// The first pane's size as asked for. A window too small for it uses
    /// less, and gets it back when it grows again.
    wanted: f32,
    area: Rect,
    /// Where on the line the mouse grabbed it, while dragging.
    grab: Option<f32>,
    hovered: bool,
}

impl Splitter {
    /// `position` is the first pane's size: the left pane's width, or the top
    /// pane's height.
    pub fn new(orientation: Orientation, position: f32, min_first: f32, min_second: f32) -> Self {
        Self {
            orientation,
            min_first,
            min_second,
            wanted: position,
            area: Rect::default(),
            grab: None,
            hovered: false,
        }
    }

    /// Sets the area both panes and the line share. Call before drawing or
    /// handling input.
    pub fn arrange(&mut self, area: Rect) {
        self.area = area;
    }

    /// Where the area starts along the axis the line moves on, and its length.
    fn span(&self) -> (f32, f32) {
        match self.orientation {
            Orientation::Vertical => (self.area.left, self.area.width()),
            Orientation::Horizontal => (self.area.top, self.area.height()),
        }
    }

    fn along(&self, point: Point) -> f32 {
        match self.orientation {
            Orientation::Vertical => point.x,
            Orientation::Horizontal => point.y,
        }
    }

    /// Keeps both panes at their minimum sizes when there's room for both. When
    /// there isn't, the first pane keeps its minimum, and the line stays inside.
    fn clamp(&self, position: f32) -> f32 {
        let room = (self.span().1 - LINE).max(0.0);
        let most = (room - self.min_second).max(self.min_first);
        position.clamp(self.min_first, most).min(room)
    }

    /// The first pane's size: the left pane's width, or the top pane's height.
    pub fn position(&self) -> f32 {
        self.clamp(self.wanted)
    }

    /// Moves the line, for example to a size saved from the last session.
    pub fn set_position(&mut self, position: f32) {
        self.wanted = position;
    }

    /// The part of the area between two distances along the axis.
    fn band(&self, from: f32, to: f32) -> Rect {
        let (start, _) = self.span();
        let area = self.area;
        match self.orientation {
            Orientation::Vertical => Rect::new(start + from, area.top, start + to, area.bottom),
            Orientation::Horizontal => Rect::new(area.left, start + from, area.right, start + to),
        }
    }

    pub fn first_rect(&self) -> Rect {
        self.band(0.0, self.position())
    }

    pub fn line_rect(&self) -> Rect {
        let position = self.position();
        self.band(position, position + LINE)
    }

    pub fn second_rect(&self) -> Rect {
        let after = self.position() + LINE;
        self.band(after, self.span().1.max(after))
    }

    /// The area the mouse can grab, which is wider than the line. It overlaps
    /// both panes, so the app should offer mouse presses here to the splitter first.
    pub fn grip_rect(&self) -> Rect {
        let position = self.position();
        self.band(position - REACH, position + LINE + REACH)
    }

    pub fn contains(&self, point: Point) -> bool {
        self.grip_rect().contains(point)
    }

    pub fn is_dragging(&self) -> bool {
        self.grab.is_some()
    }

    /// True when the mouse should show a resize cursor: over the grip, or dragging.
    pub fn shows_resize_cursor(&self) -> bool {
        self.hovered || self.grab.is_some()
    }

    fn move_to(&mut self, position: f32) -> SplitterResponse {
        let before = self.position();
        self.wanted = self.clamp(position);
        let after = self.position();
        let moved = after != before;
        SplitterResponse {
            redraw: moved,
            position: moved.then_some(after),
        }
    }

    fn handle_key(&mut self, key: Key) -> SplitterResponse {
        let position = self.position();
        let target = match (self.orientation, key) {
            (Orientation::Vertical, Key::Left) | (Orientation::Horizontal, Key::Up) => {
                position - KEY_STEP
            }
            (Orientation::Vertical, Key::Right) | (Orientation::Horizontal, Key::Down) => {
                position + KEY_STEP
            }
            (_, Key::Home) => 0.0,
            (_, Key::End) => f32::MAX,
            _ => return SplitterResponse::default(),
        };
        self.move_to(target)
    }

    /// Handles input. While dragging, send it every mouse move and the mouse
    /// release. Send keys only while the splitter has focus.
    pub fn handle(&mut self, event: Event) -> SplitterResponse {
        match event {
            Event::MouseDown(point, MouseButton::Left, _) if self.contains(point) => {
                self.grab = Some(self.along(point) - self.span().0 - self.position());
                SplitterResponse {
                    redraw: true,
                    position: None,
                }
            }
            Event::MouseMove(point) => match self.grab {
                Some(grab) => self.move_to((self.along(point) - self.span().0 - grab).round()),
                None => {
                    let hovered = self.contains(point);
                    let changed = hovered != self.hovered;
                    self.hovered = hovered;
                    SplitterResponse {
                        redraw: changed,
                        position: None,
                    }
                }
            },
            Event::MouseUp(_, MouseButton::Left) if self.grab.is_some() => {
                self.grab = None;
                SplitterResponse {
                    redraw: true,
                    position: None,
                }
            }
            Event::KeyDown(key, modifiers) if !modifiers.alt => self.handle_key(key),
            _ => SplitterResponse::default(),
        }
    }

    /// Draws the line, widened in the accent color while it's hovered, dragged
    /// or focused. Draw it after both panes, since the highlight overlaps them.
    pub fn draw(&self, canvas: &mut dyn Canvas, theme: &Theme, focused: bool) {
        if focused || self.hovered || self.grab.is_some() {
            let position = self.position();
            canvas.fill_rect(
                self.band(position - 1.0, position + LINE + 1.0),
                theme.accent,
            );
        } else {
            canvas.fill_rect(self.line_rect(), theme.border);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Modifiers;
    use crate::recording::{DrawOp, RecordingCanvas};

    const NONE: Modifiers = Modifiers {
        ctrl: false,
        shift: false,
        alt: false,
    };
    const AREA: Rect = Rect::new(0.0, 20.0, 1000.0, 620.0);

    fn sidebar() -> Splitter {
        let mut splitter = Splitter::new(Orientation::Vertical, 240.0, 120.0, 300.0);
        splitter.arrange(AREA);
        splitter
    }

    fn press(splitter: &mut Splitter, x: f32) -> SplitterResponse {
        splitter.handle(Event::MouseDown(
            Point::new(x, 300.0),
            MouseButton::Left,
            NONE,
        ))
    }

    fn drag_to(splitter: &mut Splitter, x: f32) -> SplitterResponse {
        splitter.handle(Event::MouseMove(Point::new(x, 300.0)))
    }

    fn key(splitter: &mut Splitter, key: Key) -> SplitterResponse {
        splitter.handle(Event::KeyDown(key, NONE))
    }

    #[test]
    fn panes_sit_either_side_of_a_thin_line() {
        let splitter = sidebar();
        assert_eq!(splitter.first_rect(), Rect::new(0.0, 20.0, 240.0, 620.0));
        assert_eq!(splitter.line_rect(), Rect::new(240.0, 20.0, 241.0, 620.0));
        assert_eq!(
            splitter.second_rect(),
            Rect::new(241.0, 20.0, 1000.0, 620.0)
        );
        let grip = splitter.grip_rect();
        assert_eq!(grip.width(), LINE + 2.0 * REACH);
        assert!(grip.left < 240.0 && grip.right > 241.0);
    }

    #[test]
    fn horizontal_splits_stack_their_panes() {
        let mut splitter = Splitter::new(Orientation::Horizontal, 200.0, 50.0, 50.0);
        splitter.arrange(Rect::new(0.0, 0.0, 800.0, 600.0));
        assert_eq!(splitter.first_rect(), Rect::new(0.0, 0.0, 800.0, 200.0));
        assert_eq!(splitter.line_rect(), Rect::new(0.0, 200.0, 800.0, 201.0));
        assert_eq!(splitter.second_rect(), Rect::new(0.0, 201.0, 800.0, 600.0));
        splitter.handle(Event::MouseDown(
            Point::new(400.0, 202.0),
            MouseButton::Left,
            NONE,
        ));
        let response = splitter.handle(Event::MouseMove(Point::new(10.0, 302.0)));
        assert_eq!(response.position, Some(300.0));
    }

    #[test]
    fn dragging_moves_the_line_without_a_jump() {
        let mut splitter = sidebar();
        assert!(press(&mut splitter, 242.0).redraw);
        assert!(splitter.is_dragging());
        assert_eq!(drag_to(&mut splitter, 242.0), SplitterResponse::default());
        let response = drag_to(&mut splitter, 302.0);
        assert_eq!(response.position, Some(300.0));
        assert!(response.redraw);
        assert_eq!(splitter.position(), 300.0);
        assert!(
            splitter
                .handle(Event::MouseUp(Point::new(302.0, 300.0), MouseButton::Left))
                .redraw
        );
        assert!(!splitter.is_dragging());
        assert_eq!(drag_to(&mut splitter, 500.0).position, None);
        assert_eq!(splitter.position(), 300.0);
    }

    #[test]
    fn dragging_stops_at_each_minimum() {
        let mut splitter = sidebar();
        press(&mut splitter, 240.0);
        assert_eq!(drag_to(&mut splitter, -50.0).position, Some(120.0));
        assert_eq!(
            drag_to(&mut splitter, 2000.0).position,
            Some(1000.0 - LINE - 300.0)
        );
    }

    #[test]
    fn a_window_that_shrinks_and_grows_gets_its_size_back() {
        let mut splitter = sidebar();
        splitter.arrange(Rect::new(0.0, 20.0, 500.0, 620.0));
        assert_eq!(splitter.position(), 500.0 - LINE - 300.0);
        splitter.arrange(AREA);
        assert_eq!(splitter.position(), 240.0);
    }

    #[test]
    fn a_window_too_small_for_both_keeps_the_line_inside() {
        let mut splitter = sidebar();
        splitter.arrange(Rect::new(0.0, 20.0, 100.0, 620.0));
        assert_eq!(splitter.position(), 100.0 - LINE);
        assert_eq!(splitter.second_rect().width(), 0.0);
        splitter.arrange(Rect::default());
        assert_eq!(splitter.position(), 0.0);
    }

    #[test]
    fn presses_away_from_the_grip_are_ignored() {
        let mut splitter = sidebar();
        assert_eq!(press(&mut splitter, 500.0), SplitterResponse::default());
        assert!(!splitter.is_dragging());
        let right = Event::MouseDown(Point::new(240.0, 300.0), MouseButton::Right, NONE);
        assert_eq!(splitter.handle(right), SplitterResponse::default());
        let release = Event::MouseUp(Point::new(240.0, 300.0), MouseButton::Left);
        assert_eq!(splitter.handle(release), SplitterResponse::default());
    }

    #[test]
    fn hovering_the_grip_asks_for_the_resize_cursor() {
        let mut splitter = sidebar();
        assert!(drag_to(&mut splitter, 238.0).redraw);
        assert!(splitter.shows_resize_cursor());
        assert!(!drag_to(&mut splitter, 243.0).redraw);
        assert!(drag_to(&mut splitter, 600.0).redraw);
        assert!(!splitter.shows_resize_cursor());
    }

    #[test]
    fn keys_move_it_along_its_own_axis() {
        let mut splitter = sidebar();
        assert_eq!(key(&mut splitter, Key::Right).position, Some(250.0));
        assert_eq!(key(&mut splitter, Key::Left).position, Some(240.0));
        assert_eq!(key(&mut splitter, Key::Up), SplitterResponse::default());
        assert_eq!(key(&mut splitter, Key::Home).position, Some(120.0));
        assert_eq!(key(&mut splitter, Key::Left), SplitterResponse::default());
        assert_eq!(key(&mut splitter, Key::End).position, Some(699.0));
        let alt = Modifiers { alt: true, ..NONE };
        assert_eq!(
            splitter.handle(Event::KeyDown(Key::Left, alt)),
            SplitterResponse::default()
        );

        let mut stacked = Splitter::new(Orientation::Horizontal, 200.0, 50.0, 50.0);
        stacked.arrange(Rect::new(0.0, 0.0, 800.0, 600.0));
        assert_eq!(key(&mut stacked, Key::Down).position, Some(210.0));
        assert_eq!(key(&mut stacked, Key::Left), SplitterResponse::default());
    }

    #[test]
    fn draws_a_line_or_a_highlight() {
        let splitter = sidebar();
        let theme = Theme::CLASSIC;
        let mut canvas = RecordingCanvas::new();
        splitter.draw(&mut canvas, &theme, false);
        assert_eq!(
            canvas.ops,
            vec![DrawOp::Fill(splitter.line_rect(), theme.border)]
        );
        let mut canvas = RecordingCanvas::new();
        splitter.draw(&mut canvas, &theme, true);
        assert_eq!(
            canvas.ops,
            vec![DrawOp::Fill(
                Rect::new(239.0, 20.0, 242.0, 620.0),
                theme.accent
            )]
        );
    }
}
