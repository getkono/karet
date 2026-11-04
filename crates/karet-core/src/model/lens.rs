//! Code lenses: actionable annotations a producer draws on a row of their own
//! above the line they describe ("3 references", "Run test"), each of which
//! may run a command when clicked.

use super::CommandId;
use crate::coord::Range;

/// One code lens: a title shown above `range`, and the command a click runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CodeLens {
    /// The span the lens describes; it is drawn above the line `range`
    /// starts on.
    pub range: Range,
    /// The text shown, or `None` while the producer has deferred it: a lens
    /// without a title is not drawn until it has been resolved.
    pub title: Option<String>,
    /// The command a click runs, or `None` for a lens that only informs.
    pub command: Option<CommandId>,
    /// The producer's own encoding of the lens, opaque to everyone else.
    ///
    /// Handed back to the producer to resolve the lens or to run its
    /// command; `None` when the producer needs nothing back.
    pub payload: Option<String>,
}

impl CodeLens {
    /// Whether the lens has a title to show.
    #[must_use]
    pub fn is_resolved(&self) -> bool {
        self.title.is_some()
    }

    /// The line the lens is drawn above.
    #[must_use]
    pub fn line(&self) -> u32 {
        self.range.start.line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LineCol;

    fn lens(line: u32, title: Option<&str>) -> CodeLens {
        CodeLens {
            range: Range {
                start: LineCol::new(line, 4),
                end: LineCol::new(line, 9),
            },
            title: title.map(str::to_owned),
            command: None,
            payload: None,
        }
    }

    #[test]
    fn a_lens_is_resolved_once_it_has_a_title_and_sits_on_its_start_line() {
        assert!(!lens(3, None).is_resolved());
        assert!(lens(3, Some("Run")).is_resolved());
        assert_eq!(lens(3, None).line(), 3);
    }
}
