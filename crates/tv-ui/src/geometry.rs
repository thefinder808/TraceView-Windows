//! Positions and sizes in device-independent pixels (1/96 of an inch).

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Rect {
    pub const fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub fn width(&self) -> f32 {
        (self.right - self.left).max(0.0)
    }

    pub fn height(&self) -> f32 {
        (self.bottom - self.top).max(0.0)
    }

    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.left && point.x < self.right && point.y >= self.top && point.y < self.bottom
    }

    /// Splits off a strip from the top and returns it, shrinking this rectangle.
    pub fn take_top(&mut self, height: f32) -> Rect {
        let split = (self.top + height).min(self.bottom);
        let strip = Rect::new(self.left, self.top, self.right, split);
        self.top = split;
        strip
    }

    /// Splits off a strip from the bottom and returns it, shrinking this rectangle.
    pub fn take_bottom(&mut self, height: f32) -> Rect {
        let split = (self.bottom - height).max(self.top);
        let strip = Rect::new(self.left, split, self.right, self.bottom);
        self.bottom = split;
        strip
    }

    /// Splits off a strip from the right and returns it, shrinking this rectangle.
    pub fn take_right(&mut self, width: f32) -> Rect {
        let split = (self.right - width).max(self.left);
        let strip = Rect::new(split, self.top, self.right, self.bottom);
        self.right = split;
        strip
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(value: u32) -> Self {
        Self {
            r: (value >> 16) as u8,
            g: (value >> 8) as u8,
            b: value as u8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taking_strips_shrinks_the_rectangle() {
        let mut area = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert_eq!(area.take_top(10.0), Rect::new(0.0, 0.0, 100.0, 10.0));
        assert_eq!(area.take_bottom(20.0), Rect::new(0.0, 80.0, 100.0, 100.0));
        assert_eq!(area.take_right(5.0), Rect::new(95.0, 10.0, 100.0, 80.0));
        assert_eq!(area, Rect::new(0.0, 10.0, 95.0, 80.0));
    }

    #[test]
    fn taking_more_than_exists_stays_inside() {
        let mut area = Rect::new(0.0, 0.0, 100.0, 10.0);
        let strip = area.take_top(50.0);
        assert_eq!(strip.height(), 10.0);
        assert_eq!(area.height(), 0.0);
        assert_eq!(area.take_bottom(5.0).height(), 0.0);
    }

    #[test]
    fn right_and_bottom_edges_are_outside() {
        let area = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(area.contains(Point::new(0.0, 0.0)));
        assert!(!area.contains(Point::new(10.0, 5.0)));
        assert!(!area.contains(Point::new(5.0, 10.0)));
    }

    #[test]
    fn colors_unpack_from_hex() {
        assert_eq!(
            Color::rgb(0x12_34_56),
            Color {
                r: 0x12,
                g: 0x34,
                b: 0x56
            }
        );
    }
}
