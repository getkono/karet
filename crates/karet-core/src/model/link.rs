//! Document links and document colours: what a producer says about spans of a
//! document's text beyond its syntax -- that a span leads somewhere (an
//! `#include`, a `$ref`, a package name), or that it spells a colour (a CSS
//! literal, a Tailwind class).

use super::Decoration;
use super::DecorationKind;
use crate::coord::LineCol;
use crate::coord::Range;

/// One span of a document that leads somewhere when followed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DocumentLink {
    /// The span that is the link.
    pub range: Range,
    /// Where it leads, as an absolute URI (`file:`, `https:`, ...), or `None`
    /// while the producer has deferred it: such a link is resolved when it is
    /// followed.
    pub target: Option<String>,
    /// What the producer says following it does, if anything.
    pub tooltip: Option<String>,
    /// The producer's own encoding of the link, opaque to everyone else.
    ///
    /// Handed back to the producer to resolve the link; `None` when the
    /// producer needs nothing back.
    pub payload: Option<String>,
}

impl DocumentLink {
    /// Whether the link knows where it leads.
    #[must_use]
    pub fn is_resolved(&self) -> bool {
        self.target.is_some()
    }

    /// Whether `position` is on the link.
    #[must_use]
    pub fn contains(&self, position: LineCol) -> bool {
        self.range.contains(position)
    }
}

/// One span of a document that spells a colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DocumentColor {
    /// The span that spells the colour.
    pub range: Range,
    /// The colour, as straight (un-premultiplied) 8-bit RGBA.
    pub rgba: [u8; 4],
}

impl DocumentColor {
    /// The colour as a swatch over its span.
    #[must_use]
    pub fn swatch(&self) -> Decoration {
        Decoration::new(
            self.range,
            DecorationKind::ColorSwatch { rgba: self.rgba },
            None,
        )
    }

    /// Whether this colour's span and `other` share a character.
    #[must_use]
    pub fn overlaps(&self, other: Range) -> bool {
        self.range.start < other.end && other.start < self.range.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(line: u32, start: u32, end: u32) -> Range {
        Range {
            start: LineCol::new(line, start),
            end: LineCol::new(line, end),
        }
    }

    #[test]
    fn a_link_is_resolved_once_it_has_a_target_and_holds_its_half_open_span() {
        let mut link = DocumentLink {
            range: range(2, 4, 9),
            ..DocumentLink::default()
        };
        assert!(!link.is_resolved());
        link.target = Some("https://example.com".to_owned());
        assert!(link.is_resolved());
        assert!(link.contains(LineCol::new(2, 4)));
        assert!(link.contains(LineCol::new(2, 8)));
        assert!(!link.contains(LineCol::new(2, 9)), "the end is exclusive");
        assert!(!link.contains(LineCol::new(1, 5)));
    }

    #[test]
    fn a_colour_draws_as_a_swatch_over_its_span() {
        let colour = DocumentColor {
            range: range(0, 7, 14),
            rgba: [255, 0, 0, 255],
        };
        let swatch = colour.swatch();
        assert_eq!(swatch.range, colour.range);
        assert_eq!(
            swatch.kind,
            DecorationKind::ColorSwatch {
                rgba: [255, 0, 0, 255]
            }
        );
        assert_eq!(swatch.role, None);
    }

    #[test]
    fn colours_overlap_only_spans_sharing_a_character() {
        let colour = DocumentColor {
            range: range(1, 4, 8),
            rgba: [0; 4],
        };
        assert!(colour.overlaps(range(1, 7, 12)));
        assert!(colour.overlaps(range(1, 0, 5)));
        assert!(colour.overlaps(range(1, 5, 6)));
        assert!(!colour.overlaps(range(1, 8, 12)), "touching is not sharing");
        assert!(!colour.overlaps(range(1, 0, 4)));
        assert!(!colour.overlaps(range(2, 4, 8)));
    }
}
