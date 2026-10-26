//! Start Linked Editing (Ctrl+Shift+F2) on the [`App`]: a caret in every
//! range the language server links to the caret's -- an HTML or JSX tag's
//! name and its closing tag's -- so what is typed next lands in all of them.
//!
//! The app asks through the session seam (`LspCommand::LinkedEditingRanges`) and
//! places the carets when the answering `Event::LinkedEditingRanges` arrives,
//! provided the text and the selections are still the ones it asked about.
//! Each caret sits at the same offset in its range as the caret does in its
//! own, so a selection inside the name is mirrored too. Linked editing then
//! is ordinary multi-caret editing: it ends with the carets.

use karet_core::CursorState;
use karet_core::Selection;
use karet_session::RequestId;

use super::*;

/// A Start Linked Editing waiting on the server's linked ranges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingLinked {
    /// The request the answer must carry.
    id: RequestId,
    /// The document asked about.
    doc: DocumentId,
    /// The buffer version when asked: an answer to older text is stale.
    version: u64,
    /// The selections when asked: an answer for selections the user has
    /// since changed is stale.
    cursors: CursorState,
}

/// One caret per range in `ranges`, each mirroring `primary`'s place in the
/// range that holds it, in document order with that one primary. `None` when
/// no range holds `primary`, or nothing else is linked to it.
///
/// A range spanning lines is skipped, since an offset into it means nothing
/// on another line; the protocol's ranges are a name's, on one line.
pub(crate) fn linked_cursors(ranges: &[Range], primary: Selection) -> Option<CursorState> {
    let on_one_line = |range: &&Range| range.start.line == range.end.line;
    let holds = |range: &Range, at: LineCol| range.start <= at && at <= range.end;
    let home = ranges
        .iter()
        .filter(on_one_line)
        .find(|range| holds(range, primary.anchor) && holds(range, primary.head))?;
    let offset = |at: LineCol| at.col - home.start.col;
    let (anchor, head) = (offset(primary.anchor), offset(primary.head));
    let mut selections: Vec<Selection> = ranges
        .iter()
        .filter(on_one_line)
        .map(|range| {
            let at = |offset: u32| {
                let col = range.start.col.saturating_add(offset).min(range.end.col);
                LineCol::new(range.start.line, col)
            };
            Selection {
                anchor: at(anchor),
                head: at(head),
            }
        })
        .collect();
    selections.sort_by_key(|selection| (selection.range().start, selection.range().end));
    selections.dedup();
    if selections.len() < 2 {
        return None;
    }
    let mirrored = Selection {
        anchor: LineCol::new(home.start.line, home.start.col + anchor),
        head: LineCol::new(home.start.line, home.start.col + head),
    };
    let primary = selections
        .iter()
        .position(|selection| *selection == mirrored)?;
    Some(CursorState {
        selections,
        primary,
    })
}

impl App {
    /// The active tab's session document, buffer version and selections:
    /// what a Start Linked Editing is about.
    fn linked_target(&self) -> Option<(DocumentId, u64, CursorState)> {
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

    /// Ask the language server what is linked to the caret.
    pub(super) fn start_linked_editing(&mut self) {
        let Some((doc, version, cursors)) = self.linked_target() else {
            return;
        };
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let id = backend.next_id();
        let position = cursors.primary().head;
        let command = LspCommand::LinkedEditingRanges { doc, position };
        if backend.send(id, command.into()).is_ok() {
            self.pending_linked = Some(PendingLinked {
                id,
                doc,
                version,
                cursors,
            });
        }
    }

    /// Forget a Start Linked Editing the server refused: the notice already
    /// said why, so its empty answer must not add "nothing linked" to it.
    pub(super) fn refuse_linked_editing(&mut self, id: RequestId) {
        self.pending_linked.take_if(|pending| pending.id == id);
    }

    /// Handle the `Event::LinkedEditingRanges` answering a pending Start
    /// Linked Editing: put a caret in every linked range. Dropped when it
    /// answers another request, or the active tab, its text or its selections
    /// have changed since.
    pub(super) fn on_linked_editing_ranges(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        ranges: Option<Vec<Range>>,
    ) {
        let Some(pending) = self
            .pending_linked
            .take_if(|pending| Some(pending.id) == id && pending.doc == doc)
        else {
            return;
        };
        let asked = (pending.doc, pending.version, pending.cursors.clone());
        if self.linked_target() != Some(asked) {
            return;
        }
        let primary = pending.cursors.primary();
        let Some(cursors) = ranges.and_then(|ranges| linked_cursors(&ranges, primary)) else {
            self.notify(
                Report::Refusal,
                NotificationKind::Lsp,
                "linked editing: nothing is linked to the cursor",
            );
            return;
        };
        if let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            editor.set_cursor_state(buffer, cursors);
            let head = editor.cursor();
            editor.reveal(buffer, head);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(line: u32, from: u32, to: u32) -> Range {
        Range {
            start: LineCol::new(line, from),
            end: LineCol::new(line, to),
        }
    }

    fn caret(line: u32, col: u32) -> Selection {
        Selection::caret(LineCol::new(line, col))
    }

    /// `<div>…</div>`: the caret two characters into the opening name lands
    /// two characters into the closing one too, and stays primary.
    #[test]
    fn each_caret_sits_where_the_caret_does_in_its_range() {
        let ranges = [on(2, 2, 5), on(0, 1, 4)];
        let cursors = linked_cursors(&ranges, caret(0, 3));
        assert_eq!(
            cursors,
            Some(CursorState {
                selections: vec![caret(0, 3), caret(2, 4)],
                primary: 0,
            })
        );
    }

    #[test]
    fn a_selection_inside_the_name_is_mirrored_and_keeps_its_direction() {
        let ranges = [on(0, 1, 4), on(2, 2, 5)];
        let backwards = Selection {
            anchor: LineCol::new(2, 5),
            head: LineCol::new(2, 2),
        };
        let cursors = linked_cursors(&ranges, backwards);
        let first = Selection {
            anchor: LineCol::new(0, 4),
            head: LineCol::new(0, 1),
        };
        assert_eq!(
            cursors,
            Some(CursorState {
                selections: vec![first, backwards],
                primary: 1,
            })
        );
    }

    /// Falsifier: a caret outside every range would be mirrored into ranges
    /// it has nothing to do with.
    #[test]
    fn a_caret_in_no_range_links_nothing() {
        assert_eq!(
            linked_cursors(&[on(0, 1, 4), on(2, 2, 5)], caret(1, 0)),
            None
        );
    }

    #[test]
    fn a_lone_range_links_nothing() {
        assert_eq!(linked_cursors(&[on(0, 1, 4)], caret(0, 2)), None);
    }

    /// A shorter range clamps the offset to its end rather than spilling
    /// past the name.
    #[test]
    fn an_offset_past_a_shorter_range_clamps_to_its_end() {
        let ranges = [on(0, 0, 6), on(1, 0, 2)];
        let cursors = linked_cursors(&ranges, caret(0, 5));
        assert_eq!(
            cursors.map(|cursors| cursors.selections),
            Some(vec![caret(0, 5), caret(1, 2)])
        );
    }

    #[test]
    fn a_range_spanning_lines_is_skipped() {
        let spanning = Range {
            start: LineCol::new(3, 0),
            end: LineCol::new(4, 1),
        };
        let cursors = linked_cursors(&[on(0, 1, 4), spanning, on(2, 2, 5)], caret(0, 1));
        assert_eq!(
            cursors.map(|cursors| cursors.selections),
            Some(vec![caret(0, 1), caret(2, 2)])
        );
    }
}
