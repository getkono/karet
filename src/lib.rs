//! karet — primitives for TUI dev tools.

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
}
