//! An animated one-cell progress spinner, resolved per [`IconStyle`].
//!
//! Long-running work marks itself with a single animating cell — a slow save on a
//! tab, a nested-repository read in the explorer, an agent session thinking. The
//! frame cycle is chosen per tier so the animation degrades on terminals without
//! Braille coverage, and every frame in every tier is one printable display column,
//! so a reserved status slot never shifts the layout mid-animation.
//!
//! The spinner owns neither a clock nor a reveal threshold: the caller decides
//! *whether* the work has run long enough to deserve a spinner (the application's
//! delayed-reveal policy) and passes the elapsed time in. That keeps this a pure
//! function of `(elapsed, style)` and composes with any such policy instead of
//! duplicating one.

use std::time::Duration;

use karet_filetype::IconStyle;

/// Braille dot cycle for the Nerd Font tier: a dot orbiting the 2×4 dot matrix.
const NERD_FRAMES: [char; 10] = [
    '\u{280b}', // ⠋
    '\u{2819}', // ⠙
    '\u{2839}', // ⠹
    '\u{2838}', // ⠸
    '\u{283c}', // ⠼
    '\u{2834}', // ⠴
    '\u{2826}', // ⠦
    '\u{2827}', // ⠧
    '\u{2807}', // ⠇
    '\u{280f}', // ⠏
];

/// Quadrant cycle for the Unicode tier: one dot rotating through the cell's
/// corners. Block Elements are far more widely covered by terminal fonts than
/// Braille Patterns, so the animation survives a terminal without a Nerd Font.
const UNICODE_FRAMES: [char; 4] = [
    '\u{2596}', // ▖ lower left
    '\u{2598}', // ▘ upper left
    '\u{259d}', // ▝ upper right
    '\u{2597}', // ▗ lower right
];

/// The classic bar cycle for the most portable tier: printable, single-width, and
/// unambiguous beside a label (no digits, which would read as part of a count).
const ASCII_FRAMES: [char; 4] = ['|', '/', '-', '\\'];

/// A one-cell animated spinner whose frames follow an [`IconStyle`] tier.
///
/// Build one for the active style and ask it for the frame at an elapsed time:
///
/// ```
/// use std::time::Duration;
///
/// use karet_widgets::IconStyle;
/// use karet_widgets::Spinner;
///
/// let spinner = Spinner::new(IconStyle::Ascii);
/// assert_eq!(spinner.frame(Duration::ZERO), '|');
/// assert_eq!(spinner.frame(Spinner::FRAME_INTERVAL), '/');
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Spinner {
    /// The tier whose frame cycle this spinner animates.
    style: IconStyle,
}

impl Spinner {
    /// How long each frame is held. A caller animating a spinner should schedule
    /// its repaint wake at this cadence rather than picking its own.
    pub const FRAME_INTERVAL: Duration = Duration::from_millis(100);

    /// A spinner drawing the cycle for `style`.
    #[must_use]
    pub const fn new(style: IconStyle) -> Self {
        Self { style }
    }

    /// The full frame cycle for this spinner's tier. Never empty; frame counts
    /// differ between tiers.
    #[must_use]
    pub const fn frames(self) -> &'static [char] {
        match self.style {
            IconStyle::NerdFont => &NERD_FRAMES,
            IconStyle::Unicode => &UNICODE_FRAMES,
            IconStyle::Ascii => &ASCII_FRAMES,
        }
    }

    /// The frame to paint for work that has been running for `elapsed`.
    ///
    /// The phase advances once per [`Spinner::FRAME_INTERVAL`] and wraps at the end
    /// of the tier's cycle, so tiers with different frame counts stay in step with
    /// the same wall clock.
    #[must_use]
    pub fn frame(self, elapsed: Duration) -> char {
        let frames = self.frames();
        let cycle = u128::try_from(frames.len()).unwrap_or(1).max(1);
        let step = Self::FRAME_INTERVAL.as_millis().max(1);
        let index = (elapsed.as_millis() / step) % cycle;
        // The modulo keeps `index` inside a non-empty cycle; the fallback merely
        // keeps the reserved slot filled instead of panicking if that ever changed.
        frames
            .get(usize::try_from(index).unwrap_or(0))
            .copied()
            .unwrap_or(' ')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STYLES: [IconStyle; 3] = [IconStyle::NerdFont, IconStyle::Unicode, IconStyle::Ascii];

    #[test]
    fn the_frame_advances_once_per_interval() {
        for style in STYLES {
            let spinner = Spinner::new(style);
            let frames = spinner.frames();
            for (step, expected) in frames.iter().enumerate() {
                let elapsed = Spinner::FRAME_INTERVAL * u32::try_from(step).unwrap_or(0);
                assert_eq!(spinner.frame(elapsed), *expected, "{style:?} step {step}");
            }
        }
    }

    #[test]
    fn a_frame_is_held_for_the_whole_interval() {
        // Phase changes exactly at the boundary, never inside it.
        let spinner = Spinner::new(IconStyle::NerdFont);
        let first = spinner.frame(Duration::ZERO);
        assert_eq!(
            spinner.frame(Spinner::FRAME_INTERVAL - Duration::from_millis(1)),
            first
        );
        assert_ne!(spinner.frame(Spinner::FRAME_INTERVAL), first);
    }

    #[test]
    fn the_cycle_wraps_at_the_end() {
        for style in STYLES {
            let spinner = Spinner::new(style);
            let cycle = u32::try_from(spinner.frames().len()).unwrap_or(1);
            let period = Spinner::FRAME_INTERVAL * cycle;
            assert_eq!(spinner.frame(period), spinner.frame(Duration::ZERO));
            assert_eq!(
                spinner.frame(period + Spinner::FRAME_INTERVAL),
                spinner.frame(Spinner::FRAME_INTERVAL),
                "{style:?}",
            );
        }
    }

    #[test]
    fn a_long_elapsed_time_stays_in_the_cycle() {
        // Hours of pending work must not overflow or fall out of the frame list.
        for style in STYLES {
            let spinner = Spinner::new(style);
            let frame = spinner.frame(Duration::from_secs(60 * 60 * 24));
            assert!(spinner.frames().contains(&frame), "{style:?}");
        }
    }

    #[test]
    fn every_tier_is_a_non_empty_cycle_of_distinct_single_cell_frames() {
        for style in STYLES {
            let frames = Spinner::new(style).frames();
            assert!(!frames.is_empty(), "{style:?}");
            for frame in frames {
                assert!(
                    !frame.is_control(),
                    "{style:?} frame {frame:?} is a control"
                );
                assert!(!frame.is_whitespace(), "{style:?} frame {frame:?} is blank");
                assert_eq!(
                    unicode_width::UnicodeWidthChar::width(*frame),
                    Some(1),
                    "{style:?} frame {frame:?} is not one cell",
                );
            }
            let mut seen = frames.to_vec();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), frames.len(), "duplicate frame in {style:?}");
        }
    }

    #[test]
    fn the_nerd_tier_keeps_the_ten_braille_frames() {
        // The tab save mark migrated onto this widget; its animation must not change.
        assert_eq!(
            Spinner::new(IconStyle::NerdFont).frames(),
            &[
                '\u{280b}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283c}', '\u{2834}', '\u{2826}',
                '\u{2827}', '\u{2807}', '\u{280f}',
            ],
        );
    }

    #[test]
    fn the_ascii_tier_avoids_ambiguous_characters() {
        for frame in Spinner::new(IconStyle::Ascii).frames() {
            assert!(frame.is_ascii_graphic(), "{frame:?} is not printable ASCII");
            assert!(
                !frame.is_ascii_digit(),
                "{frame:?} reads as part of a count"
            );
            assert!(
                !frame.is_ascii_alphanumeric(),
                "{frame:?} reads as part of a label",
            );
        }
    }
}
