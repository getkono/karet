//! Application wiring for Expand and Shrink Selection (Shift+Alt+Right/Left).
//!
//! Expand asks the document's language server for its selection ranges at
//! every caret ([`LspCommand::SelectionRanges`]) and grows each selection
//! along the server's chain. Where the server has nothing that encloses a
//! selection — no server runs, it does not offer selection ranges, or it found
//! none there — that caret grows along the tree-sitter parse of the document
//! instead, injected languages included, so a selection in a Markdown code
//! fence grows through the fence's own language first. Either way the ranges
//! go to `karet_editor`, which picks each caret's step and keeps the history
//! Shrink walks back, so Shrink is the same whichever source grew it.

use std::cell::RefCell;
use std::sync::Arc;

use karet_core::CursorState;
use karet_core::Range;
use karet_core::Span;
use karet_session::DocumentId;
use karet_session::LspCommand;
use karet_session::RequestId;
use karet_session::ViewId;
use karet_text::TextBuffer;
use karet_treesitter::LanguageId;
use karet_treesitter::LayeredTree;

use crate::app::App;
use crate::tab::Tab;
use crate::tab::TabKind;

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
            .send(id, LspCommand::SelectionRanges { doc, positions }.into())
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
            kind: TabKind::Code {
                path, doc, buffer, ..
            },
            editor,
            view,
            ..
        }) = self.tabs.get_mut(self.active)
        else {
            return false;
        };
        let trees = &self.layered_trees;
        let parse = || {
            let lang = karet_treesitter::language_id_from_path(path)?;
            trees.get((*view, *doc), lang, buffer)
        };
        let mut tree: Option<Option<Arc<ParsedText>>> = None;
        editor.expand_selection(buffer, |range| {
            let enclosing: Vec<Range> = server
                .iter()
                .copied()
                .filter(|r| r.start <= range.start && range.end <= r.end && *r != range)
                .collect();
            if !enclosing.is_empty() {
                return enclosing;
            }
            tree.get_or_insert_with(parse)
                .as_ref()
                .map(|parsed| enclosing_ranges(&parsed.tree, buffer, range))
                .unwrap_or_default()
        })
    }

    /// The layered parse of the active code tab, shared with every other
    /// reader of the same document version through [`LayeredTreeCache`].
    /// `None` when the tab is not code, its path names no grammar, or the
    /// parse fails.
    pub(super) fn layered_tree(&self) -> Option<Arc<ParsedText>> {
        let Some(Tab {
            kind: TabKind::Code {
                path, doc, buffer, ..
            },
            view,
            ..
        }) = self.tabs.get(self.active)
        else {
            return None;
        };
        let lang = karet_treesitter::language_id_from_path(path)?;
        self.layered_trees.get((*view, *doc), lang, buffer)
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

/// One layered tree-sitter parse of a buffer, with the text it was parsed
/// from (the byte offsets every query over the tree reads against).
pub(crate) struct ParsedText {
    /// The parse, injected languages included.
    pub(crate) tree: LayeredTree,
    /// The buffer's text at the parsed version.
    pub(crate) text: String,
}

/// What a [`LayeredTreeCache`] entry was parsed from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TreeKey {
    view: ViewId,
    doc: Option<DocumentId>,
    version: u64,
    lang: LanguageId,
}

/// The most recent layered parse a UI-thread reader asked for, keyed by
/// document, buffer version and root language, so the readers of one keystroke
/// or command (comment toggling, Enter and paste indentation, Expand Selection,
/// inline macros) share a parse instead of each parsing the whole document.
///
/// The key alone does not prove the text is the same: a split view's local
/// edit runs ahead of the snapshot that reunites the views, and a tab waiting
/// for its text holds an empty placeholder at the version its first snapshot
/// then arrives with. So the view is part of the key, and a hit is used only
/// when the buffer's text equals the text parsed — a copy and a comparison,
/// far cheaper than the parse they save, and never a tree for other text.
///
/// One entry: every reader works on the active tab, and another view replaces
/// it.
#[derive(Default)]
pub(crate) struct LayeredTreeCache {
    slot: RefCell<Option<(TreeKey, Arc<ParsedText>)>>,
}

impl LayeredTreeCache {
    /// The entry the cache holds, without parsing: what the last reader left.
    #[cfg(test)]
    pub(crate) fn held(&self) -> Option<Arc<ParsedText>> {
        self.slot
            .borrow()
            .as_ref()
            .map(|(_, parsed)| Arc::clone(parsed))
    }

    /// The layered parse of `buffer` (the text of `doc` in `view`) as `lang`,
    /// reused when the last one was of the same view, document, buffer version,
    /// language and text. `None` when the grammar fails to parse.
    pub(crate) fn get(
        &self,
        (view, doc): (ViewId, Option<DocumentId>),
        lang: LanguageId,
        buffer: &TextBuffer,
    ) -> Option<Arc<ParsedText>> {
        let key = TreeKey {
            view,
            doc,
            version: buffer.version(),
            lang,
        };
        let text = buffer.text();
        if let Some((cached, parsed)) = self.slot.borrow().as_ref()
            && *cached == key
            && parsed.text == text
        {
            return Some(Arc::clone(parsed));
        }
        let tree = karet_treesitter::LayeredParser::new()
            .parse(lang, &text)
            .ok()?;
        let parsed = Arc::new(ParsedText { tree, text });
        *self.slot.borrow_mut() = Some((key, Arc::clone(&parsed)));
        Some(parsed)
    }
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use karet_core::Change;
    use karet_core::LineCol;
    use karet_core::TextEdit;

    use super::*;

    /// The Rust grammar, which the app always compiles in.
    fn rust() -> LanguageId {
        karet_treesitter::language_id_from_path(Path::new("lib.rs"))
            .unwrap_or_else(|| unreachable!("the app builds every grammar"))
    }

    /// Readers of one view, document, version and text share one parse.
    #[test]
    fn the_same_version_of_a_document_is_parsed_once() {
        let lang = rust();
        let cache = LayeredTreeCache::default();
        let buffer = TextBuffer::from_text("fn f() {}\n");
        let owner = (ViewId(1), Some(DocumentId(1)));
        let first = cache.get(owner, lang, &buffer);
        let second = cache.get(owner, lang, &buffer);
        assert!(matches!((&first, &second), (Some(a), Some(b)) if Arc::ptr_eq(a, b)));
    }

    /// A new version, another view or document, and different text at the
    /// same key (a placeholder replaced by its first snapshot) each parse
    /// afresh, and the fresh parse reads the current text.
    #[test]
    fn any_change_in_key_or_text_parses_again() {
        let lang = rust();
        let buffer = TextBuffer::from_text("fn f() {}\n");
        let mut edited = buffer.clone();
        let applied = edited.apply(
            &Change::new(
                buffer.version(),
                vec![TextEdit {
                    range: Range {
                        start: LineCol::new(0, 0),
                        end: LineCol::new(0, 0),
                    },
                    new_text: "pub ".into(),
                }],
            ),
            karet_text::EditContext::default(),
        );
        assert!(applied.is_ok());
        let owner = (ViewId(1), Some(DocumentId(1)));
        let other_text = TextBuffer::from_text("struct S;\n");
        let rows = [
            ("new version", owner, &edited),
            ("other view", (ViewId(2), Some(DocumentId(1))), &buffer),
            ("other document", (ViewId(1), Some(DocumentId(2))), &buffer),
            ("no document", (ViewId(1), None), &buffer),
            ("other text, same version", owner, &other_text),
        ];
        for (name, other, other_buffer) in rows {
            let cache = LayeredTreeCache::default();
            let first = cache.get(owner, lang, &buffer);
            let second = cache.get(other, lang, other_buffer);
            let (Some(first), Some(second)) = (first, second) else {
                unreachable!("{name}: a Rust buffer parses");
            };
            assert!(!Arc::ptr_eq(&first, &second), "{name}");
            assert_eq!(second.text, other_buffer.text(), "{name}");
        }
    }
}
