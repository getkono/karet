//! The wire model for the Seam view: what crosses the backend seam, and why in this shape.
//!
//! The index itself never crosses. It owns a parser pool, an edge store, and a fuzzy
//! matcher, none of which a presentation layer should hold — and it is the kind of
//! structure a future client-server split could not send at all. What crosses is a
//! flattened, serde-ready projection of it.
//!
//! The whole node list is sent at once rather than fetched level by level. That looks
//! profligate until you consider what the alternative costs: a round trip per column
//! keystroke, which is exactly the latency a navigator like this cannot afford. A large
//! crate flattens to a few hundred kilobytes, and in exchange every navigation, lens
//! toggle, and reroot is answered locally and instantly.
//!
//! Node identity travels as its path string. It is the citation unit an agent hands back
//! to a human, the key view state is restored against, and stable across every edit that
//! does not rename or reparent — so it is the right thing to key on across a boundary
//! that a numeric handle could not survive.

use std::path::PathBuf;

use karet_core::Range;
use karet_diff::TokenSpan;

/// One node, flattened for the presentation layer.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SeamNodeView {
    /// The node's identity, as its semantic path.
    pub id: String,
    /// The display name.
    pub name: String,
    /// The universal kind, by name.
    pub kind: String,
    /// A short signature or descriptor.
    pub detail: Option<String>,
    /// The file the node lives in.
    pub file: PathBuf,
    /// The node's full extent.
    pub range: Range,
    /// The range to reveal when navigating here.
    pub selection: Range,
    /// The parent's identity, absent at a root.
    pub parent: Option<String>,
    /// The children's identities, in source order.
    pub children: Vec<String>,
    /// The seam properties this node carries.
    pub facets: Vec<SeamFacetView>,
    /// Per-lens subtree counts, in lens order.
    pub rollups: [u32; 5],
    /// Effective visibility, by name.
    pub visibility: Option<String>,
    /// Whether the active configuration includes it: active, inactive, or indeterminate.
    pub membership: String,
    /// Whether the node came from a subtree that failed to parse cleanly.
    pub provisional: bool,
}

/// One facet, flattened.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SeamFacetView {
    /// Which lens it belongs to, by name.
    pub lens: String,
    /// The subtype within that lens.
    pub subtype: String,
    /// Supporting text.
    pub detail: Option<String>,
    /// Sub-item occurrences inside the node.
    pub sites: Vec<Range>,
    /// Reach through re-exports, on `api` facets that have it.
    pub effective: Option<String>,
}

/// One edge leaving or arriving at a node.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SeamEdgeView {
    /// The relation, by name.
    pub kind: String,
    /// Whether the edge points away from the node or at it.
    pub outgoing: bool,
    /// The other end's identity, when it resolves inside the package.
    pub target: Option<String>,
    /// What to show for the other end when it does not.
    pub display: Option<String>,
    /// `resolved`, `external`, or `unresolved`.
    pub state: String,
    /// Whether an unresolved endpoint could still become resolved.
    pub resolvable: bool,
    /// Where the relation is written.
    pub site: Option<Range>,
}

/// The source behind one seam node: the lines that define it, and the lines around it.
///
/// The detail pane exists so a reader can tell whether pressing Enter is worth it, and a
/// name with nothing around it does not answer that — an attribute, a doc comment, or the
/// item it sits beside usually does.
///
/// Described in absolute file coordinates plus an index range, so the view never
/// re-derives what the worker already knew. Deliberately *larger* than any pane: how many
/// rows to show is the renderer's budget, not the protocol's.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SeamPreview {
    /// The file the lines were read from.
    pub file: PathBuf,
    /// The 0-based line, in that file, of `lines[0]`.
    pub first_line: u32,
    /// The lines themselves, newline stripped, in file order: the context before the
    /// node, the node's own extent, then the context after.
    pub lines: Vec<String>,
    /// The first index into [`Self::lines`] belonging to the node itself.
    ///
    /// Fewer than [`context`](Self::context) lines before it means the node sits near the
    /// top of its file. That is reported rather than padded, so the *view* can decide to
    /// reserve the missing rows instead of being handed blank lines it cannot tell from
    /// real ones.
    pub body_start: usize,
    /// One past the last index into [`Self::lines`] belonging to the node.
    pub body_end: usize,
    /// How many of the node's own lines were dropped to stay inside the fetch cap.
    ///
    /// Zero means the node's whole extent is present.
    pub dropped: u32,
    /// How many lines of context were asked for on each side.
    ///
    /// Stated rather than assumed, so a view can tell "the file ended" from "only three
    /// were ever fetched".
    pub context: u32,
    /// Per entry of [`Self::lines`], the syntax token runs as byte offsets within it.
    ///
    /// Empty when the file has no grammar, would not parse, or is too large to highlight
    /// — the view then paints unstyled text rather than nothing.
    pub tokens: Vec<Vec<TokenSpan>>,
}

impl SeamPreview {
    /// The token runs for `row`, empty when there are none and when `row` is out of range.
    ///
    /// Total by construction: a renderer walking its own row budget must never be able to
    /// index past the table.
    #[must_use]
    pub fn tokens_for(&self, row: usize) -> &[TokenSpan] {
        self.tokens.get(row).map_or(&[], Vec::as_slice)
    }

    /// Whether `row` is one of the node's own lines rather than surrounding context.
    #[must_use]
    pub fn is_body(&self, row: usize) -> bool {
        row >= self.body_start && row < self.body_end
    }

    /// The 0-based file line `lines[row]` came from.
    #[must_use]
    pub fn line_number(&self, row: usize) -> u32 {
        self.first_line
            .saturating_add(u32::try_from(row).unwrap_or(u32::MAX))
    }
}

/// What an indexed package amounts to, for the header and the empty states.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SeamSummary {
    /// What the index is *of*: a single package's name, or the name of the directory an
    /// index spanning several was built from.
    ///
    /// No member's name describes a workspace, so naming one would be worse than naming
    /// none — see [`Self::packages`] for how many it holds.
    pub package: String,
    /// How many package roots the index holds.
    pub packages: usize,
    /// How many nodes the index holds.
    pub nodes: usize,
    /// How many files it spans.
    pub files: usize,
    /// The active configuration's name, always shown in the header.
    pub configuration: String,
    /// Every configuration the package can be read under.
    pub available_configurations: Vec<String>,
    /// Whether the `variation` lens can claim completeness under the active configuration.
    ///
    /// False when no manifest was readable, in which case the feature set is a guess and
    /// the header has to say so rather than implying a complete answer.
    pub variation_complete: bool,
    /// How many files were scanned before indexing was cut short, if it was.
    ///
    /// A truncated index that says nothing reads as a complete one.
    pub truncated_after: Option<usize>,
    /// Modules whose text could not be found, with the paths that were tried.
    pub unresolved_modules: Vec<(String, Vec<PathBuf>)>,
}

impl SeamSummary {
    /// Whether the index is complete: nothing truncated, nothing unresolved.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.truncated_after.is_none() && self.unresolved_modules.is_empty()
    }
}

/// A parse failure in a query, positioned so a caret can sit under it.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SeamQueryError {
    /// What went wrong.
    pub message: String,
    /// The byte offset the offending text starts at.
    pub start: usize,
    /// One past its last byte.
    pub end: usize,
    /// The nearest valid names.
    pub suggestions: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview() -> SeamPreview {
        SeamPreview {
            file: PathBuf::from("src/lib.rs"),
            first_line: 8,
            lines: (0..7).map(|n| format!("line {n}")).collect(),
            body_start: 3,
            body_end: 5,
            dropped: 0,
            context: 3,
            tokens: Vec::new(),
        }
    }

    #[test]
    fn tokens_for_a_row_past_the_table_are_empty_rather_than_a_panic() {
        // A renderer walks its own row budget; the table may be shorter, or absent.
        let preview = preview();
        assert!(preview.tokens_for(0).is_empty());
        assert!(preview.tokens_for(999).is_empty());
    }

    #[test]
    fn context_rows_are_not_body_rows() {
        let preview = preview();
        let body: Vec<bool> = (0..preview.lines.len())
            .map(|r| preview.is_body(r))
            .collect();
        assert_eq!(body, [false, false, false, true, true, false, false]);
    }

    #[test]
    fn a_preview_numbers_its_rows_from_the_file_it_came_from() {
        let preview = preview();
        assert_eq!(preview.line_number(0), 8);
        assert_eq!(preview.line_number(preview.body_start), 11);
    }

    #[test]
    fn a_summary_reports_completeness_only_when_nothing_was_lost() {
        let complete = SeamSummary {
            package: "demo".to_owned(),
            nodes: 10,
            ..SeamSummary::default()
        };
        assert!(complete.is_complete());

        let truncated = SeamSummary {
            truncated_after: Some(20_000),
            ..complete.clone()
        };
        assert!(!truncated.is_complete());

        let unresolved = SeamSummary {
            unresolved_modules: vec![("demo::absent".to_owned(), Vec::new())],
            ..complete
        };
        assert!(!unresolved.is_complete());
    }
}
