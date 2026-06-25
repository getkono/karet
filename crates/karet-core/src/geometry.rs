//! Terminal-cell geometry: points, sizes, rectangles, and offset math.
//!
//! All coordinates are measured in terminal cells (`u16`), independent of any
//! particular rendering backend (e.g. `ratatui::layout::Rect`).

/// A point in terminal-cell space (column `x`, row `y`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Point {
    /// Zero-based column.
    pub x: u16,
    /// Zero-based row.
    pub y: u16,
}

/// A size in terminal cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Size {
    /// Width in cells.
    pub width: u16,
    /// Height in cells.
    pub height: u16,
}

/// A signed scroll/translation delta in cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Offset {
    /// Horizontal delta.
    pub dx: i32,
    /// Vertical delta.
    pub dy: i32,
}

/// An axis-aligned rectangle in terminal-cell space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Rect {
    /// Left edge (column).
    pub x: u16,
    /// Top edge (row).
    pub y: u16,
    /// Width in cells.
    pub width: u16,
    /// Height in cells.
    pub height: u16,
}

impl Rect {
    /// Create a rectangle at `(x, y)` with the given `width` and `height`.
    #[must_use]
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The total cell area (`width * height`), widened to avoid overflow.
    #[must_use]
    pub const fn area(self) -> u32 {
        self.width as u32 * self.height as u32
    }

    /// The exclusive right edge (`x + width`), saturating at `u16::MAX`.
    #[must_use]
    pub const fn right(self) -> u16 {
        self.x.saturating_add(self.width)
    }

    /// The exclusive bottom edge (`y + height`), saturating at `u16::MAX`.
    #[must_use]
    pub const fn bottom(self) -> u16 {
        self.y.saturating_add(self.height)
    }

    /// Whether `p` lies inside the rectangle (half-open on the right/bottom edges).
    #[must_use]
    pub fn contains(self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    /// The overlapping rectangle of `self` and `other` (zero-sized if disjoint).
    #[must_use]
    pub fn intersection(self, other: Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        Rect {
            x,
            y,
            width: right.saturating_sub(x),
            height: bottom.saturating_sub(y),
        }
    }

    /// Clamp this rectangle so it lies entirely within `bounds`.
    #[must_use]
    pub fn clamp(self, bounds: Rect) -> Rect {
        self.intersection(bounds)
    }
}

/// Clamp a value into the inclusive range `[min, max]`.
///
/// A foundational building block for laying out and constraining terminal UI
/// geometry (cursor positions, viewport sizes, scroll offsets).
#[must_use]
pub fn clamp(value: u16, min: u16, max: u16) -> u16 {
    value.max(min).min(max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_within_range() {
        assert_eq!(clamp(5, 0, 10), 5);
        assert_eq!(clamp(15, 0, 10), 10);
        assert_eq!(clamp(0, 3, 10), 3);
    }

    #[test]
    fn rect_area_and_contains() {
        let r = Rect::new(1, 1, 4, 3);
        assert_eq!(r.area(), 12);
        assert_eq!(r.right(), 5);
        assert_eq!(r.bottom(), 4);
        assert!(r.contains(Point { x: 1, y: 1 }));
        assert!(r.contains(Point { x: 4, y: 3 }));
        assert!(!r.contains(Point { x: 5, y: 3 }));
        assert!(!r.contains(Point { x: 0, y: 0 }));
    }

    #[test]
    fn rect_intersection() {
        let a = Rect::new(0, 0, 4, 4);
        let b = Rect::new(2, 2, 4, 4);
        assert_eq!(a.intersection(b), Rect::new(2, 2, 2, 2));
        let disjoint = Rect::new(10, 10, 2, 2);
        assert_eq!(a.intersection(disjoint).area(), 0);
    }
}
