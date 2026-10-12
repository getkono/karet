//! Application wiring for Expand and Shrink Selection (Shift+Alt+Right/Left).
//!
//! Expand asks the document's language server for its selection ranges at
//! every caret ([`SessionCommand::SelectionRanges`]) and grows each selection
//! along the server's chain. Where the server has nothing that encloses a
//! selection — no server runs, it does not offer selection ranges, or it found
//! none there — that caret grows along the tree-sitter parse of the document
//! instead, injected languages included, so a selection in a Markdown code
//! fence grows through the fence's own language first. Either way the ranges
//! go to `karet_editor`, which picks each caret's step and keeps the history
//! Shrink walks back, so Shrink is the same whichever source grew it.

use karet_core::CursorState;
use karet_core::Span;
use karet_treesitter::LayeredTree;

use super::*;

/// An Expand Selection waiting on the language server's selection ranges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingExpand {
    /// The request the answer must carry.
    id: RequestId,
    /// The document asked about.
    doc: DocumentId,
    /// The tab's buffer version when asked: an answer to older text is stale.
    version: u64,
    /// The selections when asked: an answer for selections the user has
    /// since changed is stale.
    cursors: CursorState,
    /// Presses so far, each one step once the answer lands. A press while
    /// the answer is on its way asks nothing new — the same selections would
    /// get the same answer — but still counts.
    steps: usize,
}

impl App {
    /// Grow every selection in the active code tab to the next enclosing
    /// syntax node; a bare caret on a word takes the word first. A tab backed
    /// by a session document asks the language server first and grows when it
    /// answers; any other grows at once along tree-sitter, and a file with no
    /// grammar still gets the word step.
    pub(super) fn expand_selection(&mut self) {
        let Some((doc, version, cursors)) = self.expand_target() else {
            self.grow_selection(&[]);
            return;
        };
        if let Some(pending) = self.pending_expand.as_mut()
            && (pending.doc, pending.version, &pending.cursors) == (doc, version, &cursors)
        {
            pending.steps += 1;
            return;
        }
        let Some(backend) = self.backend.clone() else {
            self.grow_selection(&[]);
            return;
        };
        let id = backend.next_id();
        // A selection's start lies inside every range that encloses it; its
        // end may sit just past the node it closes.
        let positions = cursors
            .selections
            .iter()
            .map(|selection| selection.range().start)
            .collect();
        if backend
            .send(id, SessionCommand::SelectionRanges { doc, positions })
            .is_err()
        {
            self.grow_selection(&[]);
            return;
        }
        self.pending_expand = Some(PendingExpand {
            id,
            doc,
            version,
            cursors,
            steps: 1,
        });
    }

    /// Handle the [`SessionEvent::SelectionRanges`] answering a pending
    /// Expand Selection: grow by the presses made since the request, along
    /// the server's chains. Dropped when it answers another request, or when
    /// the active tab, its text or its selections have changed since.
    pub(super) fn on_selection_ranges(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        chains: Vec<Vec<Range>>,
    ) {
        let Some(pending) = self
            .pending_expand
            .take_if(|pending| Some(pending.id) == id && pending.doc == doc)
        else {
            return;
        };
        if self.expand_target() != Some((pending.doc, pending.version, pending.cursors)) {
            return;
        }
        let server: Vec<Range> = chains.into_iter().flatten().collect();
        for _ in 0..pending.steps {
            if !self.grow_selection(&server) {
                break;
            }
        }
    }

    /// The active tab's session document, buffer version and selections:
    /// what an Expand Selection request is about.
    fn expand_target(&self) -> Option<(DocumentId, u64, CursorState)> {
        let Tab {
            kind:
                TabKind::Code {
                    doc: Some(doc),
                    buffer,
                    ..
                },
            editor,
            ..
        } = self.tabs.get(self.active)?
        else {
            return None;
        };
        Some((*doc, buffer.version(), editor.cursors().clone()))
    }

    /// Grow every selection in the active code tab one step: along the
    /// `server` ranges that enclose it, or where none does, along the
    /// tree-sitter parse, made only if some caret needs it. `false` when no
    /// selection grew.
    fn grow_selection(&mut self, server: &[Range]) -> bool {
        let Some(Tab {
            kind: TabKind::Code { path, buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        else {
            return false;
        };
        let mut tree: Option<Option<LayeredTree>> = None;
        editor.expand_selection(buffer, |range| {
            let enclosing: Vec<Range> = server
                .iter()
                .copied()
                .filter(|r| r.start <= range.start && range.end <= r.end && *r != range)
                .collect();
            if !enclosing.is_empty() {
                return enclosing;
            }
            tree.get_or_insert_with(|| parse(path, buffer))
                .as_ref()
                .map(|tree| enclosing_ranges(tree, buffer, range))
                .unwrap_or_default()
        })
    }

    /// Restore the selections the last Expand Selection grew from, in the active
    /// code tab.
    pub(super) fn shrink_selection(&mut self) {
        if let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            editor.shrink_selection(buffer);
        }
    }
}

/// The layered tree-sitter parse of `buffer`, when `path` names a language
/// with a grammar.
fn parse(path: &Path, buffer: &TextBuffer) -> Option<LayeredTree> {
    let lang = karet_treesitter::language_id_from_path(path)?;
    karet_treesitter::LayeredParser::new()
        .parse(lang, &buffer.text())
        .ok()
}

/// The ranges of every node in `tree` that encloses `range`, smallest first.
fn enclosing_ranges(tree: &LayeredTree, buffer: &TextBuffer, range: Range) -> Vec<Range> {
    let (Ok(start), Ok(end)) = (
        buffer.line_col_to_byte(range.start),
        buffer.line_col_to_byte(range.end),
    ) else {
        return Vec::new();
    };
    tree.enclosing_spans(Span { start, end })
        .into_iter()
        .map(|span| Range {
            start: buffer.byte_to_line_col(span.start),
            end: buffer.byte_to_line_col(span.end),
        })
        .collect()
}
