//! Inlay hints: inferred types, parameter names and other annotations a
//! producer draws between characters, with the label parts a renderer can
//! hover and follow.

use super::Location;
use super::Markup;
use super::VirtualText;
use crate::coord::LineCol;
use crate::token::ThemeRole;

/// The kind of an [`InlayHint`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum InlayHintKind {
    /// An inferred type annotation.
    Type,
    /// A parameter name.
    Parameter,
    /// A hint the producer gave no kind: chained-call types, closure
    /// captures, lifetime elisions, and whatever else a server annotates.
    Other,
    /// Forward-compatibility fallback: an unrecognized value from a newer peer.
    #[cfg_attr(feature = "serde", serde(other))]
    Unknown,
}

impl InlayHintKind {
    /// The theme role a hint of this kind is painted in.
    #[must_use]
    pub fn role(self) -> ThemeRole {
        match self {
            Self::Type => ThemeRole::InlayHintType,
            Self::Parameter => ThemeRole::InlayHintParameter,
            Self::Other | Self::Unknown => ThemeRole::InlayHint,
        }
    }
}

/// One separately hoverable, followable piece of an [`InlayHint`]'s label —
/// e.g. the `Vec` in `: Vec<String>` that leads to `Vec`'s definition.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InlayHintLabelPart {
    /// The text of this part.
    pub value: String,
    /// What hovering this part shows, when it says more than the hint does.
    #[cfg_attr(feature = "serde", serde(default))]
    pub tooltip: Option<Markup>,
    /// Where following this part (Ctrl+click) goes.
    #[cfg_attr(feature = "serde", serde(default))]
    pub location: Option<Location>,
}

/// An inline hint rendered between characters (inferred types, parameter names).
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InlayHint {
    /// Where the hint is anchored.
    pub position: LineCol,
    /// The whole label as one string: the parts' values joined, when it has
    /// [`parts`](Self::parts).
    pub label: String,
    /// The hint kind.
    pub kind: InlayHintKind,
    /// Render padding before the hint.
    pub padding_left: bool,
    /// Render padding after the hint.
    pub padding_right: bool,
    /// The label as the producer split it, each part with its own tooltip and
    /// location; empty when the label is one plain string.
    #[cfg_attr(feature = "serde", serde(default))]
    pub parts: Vec<InlayHintLabelPart>,
    /// What hovering the hint shows.
    #[cfg_attr(feature = "serde", serde(default))]
    pub tooltip: Option<Markup>,
    /// The producer's own encoding of the hint, opaque to everyone else.
    ///
    /// Handed back to the producer to fill in what it deferred (a tooltip, a
    /// part's location); `None` when there is nothing left to fill in.
    #[cfg_attr(feature = "serde", serde(default))]
    pub payload: Option<String>,
}

impl InlayHint {
    /// A plain hint: `label` at `position`, unpadded, with no parts, tooltip
    /// or payload.
    #[must_use]
    pub fn new(position: LineCol, label: impl Into<String>, kind: InlayHintKind) -> Self {
        Self {
            position,
            label: label.into(),
            kind,
            padding_left: false,
            padding_right: false,
            parts: Vec::new(),
            tooltip: None,
            payload: None,
        }
    }

    /// The hint as one [`VirtualText`] per label part, in order, all anchored
    /// at its position in its kind's color; a hint without parts is one text.
    ///
    /// Separate texts at one column paint side by side in supply order, so
    /// the hint reads exactly as one, while a renderer can tell which part a
    /// pointer is over by which text it hit. The padding goes on the outer
    /// edges only: before the first text and after the last.
    #[must_use]
    pub fn virtual_texts(&self) -> Vec<VirtualText> {
        let values: Vec<&str> = if self.parts.is_empty() {
            vec![self.label.as_str()]
        } else {
            self.parts.iter().map(|part| part.value.as_str()).collect()
        };
        let last = values.len().saturating_sub(1);
        values
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                let mut text = String::with_capacity(value.len().saturating_add(2));
                if index == 0 && self.padding_left {
                    text.push(' ');
                }
                text.push_str(value);
                if index == last && self.padding_right {
                    text.push(' ');
                }
                VirtualText::inline(
                    self.position.line,
                    self.position.col,
                    text,
                    self.kind.role(),
                )
            })
            .collect()
    }

    /// What hovering part `part` shows: that part's own tooltip, else the
    /// hint's. `part` indexes [`parts`](Self::parts); any index of a hint
    /// without parts asks for the hint's own.
    #[must_use]
    pub fn tooltip_for(&self, part: usize) -> Option<&Markup> {
        self.parts
            .get(part)
            .and_then(|part| part.tooltip.as_ref())
            .or(self.tooltip.as_ref())
    }

    /// Where following part `part` goes, if anywhere.
    #[must_use]
    pub fn location_for(&self, part: usize) -> Option<&Location> {
        self.parts.get(part)?.location.as_ref()
    }

    /// Adopt what the producer filled in for this hint: its tooltip, and each
    /// part's tooltip and location, where `resolved` has them. The position,
    /// kind and text stay this hint's own — they may have been carried across
    /// edits since the producer last saw it — and the payload is spent, so the
    /// hint is not resolved again.
    pub fn adopt_resolved(&mut self, resolved: Self) {
        if resolved.tooltip.is_some() {
            self.tooltip = resolved.tooltip;
        }
        if resolved.parts.len() == self.parts.len() {
            for (own, filled) in self.parts.iter_mut().zip(resolved.parts) {
                if filled.tooltip.is_some() {
                    own.tooltip = filled.tooltip;
                }
                if filled.location.is_some() {
                    own.location = filled.location;
                }
            }
        }
        self.payload = None;
    }
}

impl From<&InlayHint> for VirtualText {
    /// The hint's whole label at its position, with its padding as one space
    /// either side, in its kind's color. [`InlayHint::virtual_texts`] keeps
    /// the parts apart instead.
    fn from(hint: &InlayHint) -> Self {
        let mut text = String::with_capacity(hint.label.len().saturating_add(2));
        if hint.padding_left {
            text.push(' ');
        }
        text.push_str(&hint.label);
        if hint.padding_right {
            text.push(' ');
        }
        Self::inline(
            hint.position.line,
            hint.position.col,
            text,
            hint.kind.role(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::coord::Range;
    use crate::model::MarkupKind;
    use crate::model::VirtualTextPlacement;

    fn markup(value: &str) -> Markup {
        Markup {
            kind: MarkupKind::PlainText,
            value: value.to_owned(),
        }
    }

    fn part(value: &str) -> InlayHintLabelPart {
        InlayHintLabelPart {
            value: value.to_owned(),
            ..InlayHintLabelPart::default()
        }
    }

    fn location(path: &str, line: u32) -> Location {
        Location {
            path: PathBuf::from(path),
            range: Range {
                start: LineCol::new(line, 0),
                end: LineCol::new(line, 3),
            },
        }
    }

    #[test]
    fn an_inlay_hint_becomes_inline_virtual_text_with_its_padding_as_spaces() {
        let hint = InlayHint {
            padding_left: true,
            ..InlayHint::new(LineCol::new(2, 9), "i32", InlayHintKind::Type)
        };
        assert_eq!(
            VirtualText::from(&hint),
            VirtualText {
                line: 2,
                placement: VirtualTextPlacement::Inline { col: 9 },
                text: " i32".to_owned(),
                role: ThemeRole::InlayHintType,
            }
        );
        let both = InlayHint {
            padding_right: true,
            ..hint
        };
        assert_eq!(VirtualText::from(&both).text, " i32 ");
    }

    #[test]
    fn each_kind_paints_in_its_own_role() {
        assert_eq!(InlayHintKind::Type.role(), ThemeRole::InlayHintType);
        assert_eq!(
            InlayHintKind::Parameter.role(),
            ThemeRole::InlayHintParameter
        );
        assert_eq!(InlayHintKind::Other.role(), ThemeRole::InlayHint);
        assert_eq!(InlayHintKind::Unknown.role(), ThemeRole::InlayHint);
    }

    #[test]
    fn a_hint_without_parts_is_one_text() {
        let hint = InlayHint::new(LineCol::new(0, 4), "n:", InlayHintKind::Parameter);
        let texts = hint.virtual_texts();
        assert_eq!(texts.len(), 1);
        assert_eq!(texts.first().map(|t| t.text.as_str()), Some("n:"));
        assert_eq!(
            texts.first().map(|t| t.role),
            Some(ThemeRole::InlayHintParameter)
        );
    }

    /// Parts become one text each at the same column, so they paint as the
    /// whole label; padding goes only on the outer edges.
    #[test]
    fn parts_become_one_text_each_with_padding_only_on_the_outside() {
        let hint = InlayHint {
            padding_left: true,
            padding_right: true,
            parts: vec![part(": "), part("Vec"), part("<String>")],
            ..InlayHint::new(LineCol::new(1, 7), ": Vec<String>", InlayHintKind::Type)
        };
        let texts = hint.virtual_texts();
        let painted: Vec<&str> = texts.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(painted, [" : ", "Vec", "<String> "]);
        assert!(
            texts
                .iter()
                .all(|t| t.line == 1 && t.placement == VirtualTextPlacement::Inline { col: 7 })
        );
        assert_eq!(painted.concat(), format!(" {} ", hint.label));
    }

    #[test]
    fn a_part_tooltip_wins_over_the_hint_tooltip() {
        let hint = InlayHint {
            tooltip: Some(markup("whole")),
            parts: vec![
                InlayHintLabelPart {
                    tooltip: Some(markup("own")),
                    ..part("a")
                },
                part("b"),
            ],
            ..InlayHint::new(LineCol::new(0, 0), "ab", InlayHintKind::Type)
        };
        assert_eq!(hint.tooltip_for(0), Some(&markup("own")));
        assert_eq!(hint.tooltip_for(1), Some(&markup("whole")));
        assert_eq!(hint.tooltip_for(9), Some(&markup("whole")));
        let bare = InlayHint::new(LineCol::new(0, 0), "x", InlayHintKind::Other);
        assert_eq!(bare.tooltip_for(0), None);
    }

    #[test]
    fn only_a_part_with_a_location_can_be_followed() {
        let hint = InlayHint {
            parts: vec![
                part(": "),
                InlayHintLabelPart {
                    location: Some(location("vec.rs", 40)),
                    ..part("Vec")
                },
            ],
            ..InlayHint::new(LineCol::new(0, 0), ": Vec", InlayHintKind::Type)
        };
        assert_eq!(hint.location_for(0), None);
        assert_eq!(hint.location_for(1), Some(&location("vec.rs", 40)));
        assert_eq!(hint.location_for(2), None);
    }

    /// Resolving fills in what was deferred, keeps what the client holds
    /// (its position may have moved with edits), and spends the payload.
    #[test]
    fn adopting_a_resolved_hint_fills_in_tooltips_and_locations_only() {
        let mut held = InlayHint {
            parts: vec![part(": "), part("Vec")],
            payload: Some("{}".to_owned()),
            ..InlayHint::new(LineCol::new(5, 2), ": Vec", InlayHintKind::Type)
        };
        let resolved = InlayHint {
            tooltip: Some(markup("a vector")),
            parts: vec![
                part(": "),
                InlayHintLabelPart {
                    tooltip: Some(markup("std::vec::Vec")),
                    location: Some(location("vec.rs", 40)),
                    ..part("Vec")
                },
            ],
            payload: Some("{\"data\":1}".to_owned()),
            ..InlayHint::new(LineCol::new(3, 9), "ignored", InlayHintKind::Other)
        };
        held.adopt_resolved(resolved);
        assert_eq!(held.position, LineCol::new(5, 2));
        assert_eq!(held.label, ": Vec");
        assert_eq!(held.kind, InlayHintKind::Type);
        assert_eq!(held.tooltip, Some(markup("a vector")));
        assert_eq!(held.tooltip_for(1), Some(&markup("std::vec::Vec")));
        assert_eq!(held.location_for(1), Some(&location("vec.rs", 40)));
        assert_eq!(held.payload, None);
    }

    /// A resolve answer whose parts no longer line up with the held label
    /// cannot say which part is which, so only the hint's own tooltip is taken.
    #[test]
    fn a_resolved_hint_with_other_parts_leaves_the_parts_alone() {
        let mut held = InlayHint {
            parts: vec![part("a"), part("b")],
            ..InlayHint::new(LineCol::new(0, 0), "ab", InlayHintKind::Type)
        };
        let resolved = InlayHint {
            tooltip: Some(markup("t")),
            parts: vec![InlayHintLabelPart {
                location: Some(location("x.rs", 1)),
                ..part("ab")
            }],
            ..InlayHint::new(LineCol::new(0, 0), "ab", InlayHintKind::Type)
        };
        held.adopt_resolved(resolved);
        assert_eq!(held.tooltip, Some(markup("t")));
        assert_eq!(held.location_for(0), None);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn a_hint_round_trips_and_older_wire_forms_still_read() -> Result<(), serde_json::Error> {
        let hint = InlayHint {
            tooltip: Some(markup("t")),
            parts: vec![InlayHintLabelPart {
                location: Some(location("a.rs", 2)),
                ..part("x")
            }],
            payload: Some("p".to_owned()),
            ..InlayHint::new(LineCol::new(1, 2), "x", InlayHintKind::Other)
        };
        let json = serde_json::to_string(&hint)?;
        assert_eq!(serde_json::from_str::<InlayHint>(&json)?, hint);
        // A peer that predates parts, tooltips and payloads.
        let old = r#"{"position":{"line":1,"col":2},"label":"x","kind":"Type",
            "padding_left":false,"padding_right":false}"#;
        let read: InlayHint = serde_json::from_str(old)?;
        assert!(read.parts.is_empty() && read.tooltip.is_none() && read.payload.is_none());
        // A kind from a newer peer degrades rather than failing the payload.
        let newer = old.replace("\"Type\"", "\"Lifetime\"");
        assert_eq!(
            serde_json::from_str::<InlayHint>(&newer)?.kind,
            InlayHintKind::Unknown
        );
        Ok(())
    }
}
