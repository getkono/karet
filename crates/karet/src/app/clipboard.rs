//! Cut, copy and paste in a code tab, over every caret (the Clipboard rows of
//! `docs/editing-behavior.md`).
//!
//! A copy takes every non-empty selection, in document order, joined by
//! newlines. With no selection anywhere it takes the whole line under each
//! caret instead, and marks the clipboard entry as a line copy. A cut deletes
//! exactly what its copy took. A paste inserts at every caret; a line copy goes
//! in above the caret's line, and when the clipboard holds one piece (or one
//! line) per caret, each caret gets its own.

use std::collections::BTreeSet;

use karet_core::CursorState;
use karet_core::Selection;

use super::*;
use crate::clipboard::Copied;

/// What one copy or cut takes from a code tab.
struct Taken {
    /// The clipboard entry.
    copied: Copied,
    /// The deletion a cut makes, per selection in cursor-set order: `None` for a
    /// selection that keeps its text (an empty caret beside a selection).
    cuts: Vec<(Selection, Option<editing::Edit>)>,
}

/// The indexes of `cursors`' selections, ordered by where they start.
fn in_document_order(cursors: &CursorState) -> Vec<usize> {
    let mut order: Vec<usize> = (0..cursors.selections.len()).collect();
    order.sort_by_key(|&i| cursors.selections[i].range().start);
    order
}

/// The number of `char`s on `line`, without its line break.
fn line_len(buffer: &TextBuffer, line: u32) -> u32 {
    buffer
        .line(line as usize)
        .map_or(0, |l| u32::try_from(l.chars().count()).unwrap_or(u32::MAX))
}

/// One deletion of `range`, leaving its caret at `caret`.
fn deletion(range: Range, caret: LineCol, base: u64) -> editing::Edit {
    editing::Edit {
        change: Change::new(
            base,
            vec![TextEdit {
                range,
                new_text: String::new(),
            }],
        ),
        caret,
    }
}

/// Delete lines `first..=last` whole, for a caret at column `col` among them.
///
/// The line after the run moves up, and the caret keeps its column on it. A run
/// that ends the buffer takes the line break before it instead, so no empty
/// line is left behind, and the caret moves to the line above. `None` when the
/// run is the whole buffer and the buffer is already empty.
fn line_cut(
    buffer: &TextBuffer,
    (first, last): (u32, u32),
    col: u32,
    base: u64,
) -> Option<editing::Edit> {
    let final_line = u32::try_from(buffer.line_count().saturating_sub(1)).unwrap_or(u32::MAX);
    let (range, caret) = if last < final_line {
        let next = last + 1;
        (
            Range {
                start: LineCol::new(first, 0),
                end: LineCol::new(next, 0),
            },
            LineCol::new(first, col.min(line_len(buffer, next))),
        )
    } else if first > 0 {
        let above = first - 1;
        let above_len = line_len(buffer, above);
        (
            Range {
                start: LineCol::new(above, above_len),
                end: LineCol::new(last, line_len(buffer, last)),
            },
            LineCol::new(above, col.min(above_len)),
        )
    } else {
        let range = Range {
            start: LineCol::new(0, 0),
            end: LineCol::new(last, line_len(buffer, last)),
        };
        if range.is_empty() {
            return None;
        }
        (range, LineCol::new(0, 0))
    };
    Some(deletion(range, caret, base))
}

/// Group sorted, distinct `lines` into runs of consecutive lines.
fn runs(lines: &BTreeSet<u32>) -> Vec<(u32, u32)> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for &line in lines {
        match runs.last_mut() {
            Some((_, last)) if *last + 1 == line => *last = line,
            _ => runs.push((line, line)),
        }
    }
    runs
}

/// What a copy (or cut) of `cursors` over `buffer` (whose text is `source`)
/// takes, with the deletions a cut against version `base` makes.
fn take(buffer: &TextBuffer, source: &str, cursors: &CursorState, base: u64) -> Taken {
    let order = in_document_order(cursors);
    let selected = cursors.selections.iter().any(|s| !s.range().is_empty());
    if selected {
        // The text of each selection; a range whose text cannot be read is
        // neither copied nor cut.
        let texts: Vec<Option<String>> = cursors
            .selections
            .iter()
            .map(|s| Some(s.range()).filter(|r| !r.is_empty()))
            .map(|range| range.and_then(|range| selection_text(buffer, source, range)))
            .collect();
        let pieces = order.iter().filter_map(|&i| texts[i].clone()).collect();
        let cuts = cursors
            .selections
            .iter()
            .zip(&texts)
            .map(|(selection, text)| {
                let range = selection.range();
                let cut = text.as_ref().map(|_| deletion(range, range.start, base));
                (*selection, cut)
            })
            .collect();
        return Taken {
            copied: Copied {
                pieces,
                whole_lines: false,
            },
            cuts,
        };
    }
    let lines: BTreeSet<u32> = cursors.selections.iter().map(|s| s.head.line).collect();
    let pieces = lines
        .iter()
        .filter_map(|&line| buffer.line(line as usize))
        .map(|line| format!("{line}\n"))
        .collect();
    let runs = runs(&lines);
    let cuts = cursors
        .selections
        .iter()
        .map(|selection| {
            let head = selection.head;
            let run = runs
                .iter()
                .copied()
                .find(|(first, last)| (*first..=*last).contains(&head.line));
            let cut = run.and_then(|run| line_cut(buffer, run, head.col, base));
            (*selection, cut)
        })
        .collect();
    Taken {
        copied: Copied {
            pieces,
            whole_lines: true,
        },
        cuts,
    }
}

/// The edit pasting `text` at every caret of `cursors` against version `base`.
///
/// `copied` is the entry karet wrote, when the clipboard still holds its text.
/// With several carets, each one takes its own piece when the entry has one per
/// caret, else its own line when `text` has one line per caret; otherwise every
/// caret takes all of `text`. A whole-line piece goes in above an empty caret's
/// line (once per line), with the caret staying on its text; anywhere else the
/// piece replaces the selection or goes in at the caret.
fn paste_edit(
    cursors: &CursorState,
    base: u64,
    text: &str,
    copied: Option<&Copied>,
) -> Option<editing::MultiEdit> {
    let count = cursors.selections.len();
    let lines: Vec<&str> = text
        .strip_suffix('\n')
        .unwrap_or(text)
        .split('\n')
        .collect();
    let (pieces, whole_lines): (Vec<&str>, bool) = match copied {
        Some(copied) if count > 1 && copied.pieces.len() == count => (
            copied.pieces.iter().map(String::as_str).collect(),
            copied.whole_lines,
        ),
        _ if count > 1 && lines.len() == count => (lines, false),
        _ => (vec![text; count], copied.is_some_and(|c| c.whole_lines)),
    };
    let mut piece_of = vec![""; count];
    for (rank, &i) in in_document_order(cursors).iter().enumerate() {
        piece_of[i] = pieces[rank];
    }
    let mut pasted_lines = BTreeSet::new();
    let items: Vec<_> = cursors
        .selections
        .iter()
        .zip(piece_of)
        .map(|(selection, piece)| {
            let range = selection.range();
            if !range.is_empty() {
                return (
                    *selection,
                    Some(editing::insert(selection.head, Some(range), base, piece)),
                );
            }
            if !whole_lines {
                return (
                    *selection,
                    Some(editing::insert(selection.head, None, base, piece)),
                );
            }
            let head = selection.head;
            if !pasted_lines.insert(head.line) {
                // The line already takes the piece from another caret; this one
                // moves down with it.
                return (*selection, None);
            }
            let mut edit = editing::insert(LineCol::new(head.line, 0), None, base, piece);
            let added = u32::try_from(piece.matches('\n').count()).unwrap_or(u32::MAX);
            edit.caret = LineCol::new(head.line.saturating_add(added), head.col);
            (*selection, Some(edit))
        })
        .collect();
    editing::compose(base, cursors.primary, &items)
}

impl App {
    /// What a copy of the active code tab takes, or `None` when the active tab
    /// is not a code tab.
    fn take_from_active(&self) -> Option<Taken> {
        match self.tabs.get(self.active) {
            Some(Tab {
                kind:
                    TabKind::Code {
                        buffer,
                        text,
                        next_version,
                        ..
                    },
                editor,
                ..
            }) => Some(take(buffer, text, editor.cursors(), *next_version)),
            _ => None,
        }
    }

    /// Put `copied` on the clipboard, reporting the outcome either way; `true`
    /// when it landed.
    fn copy_entry(&mut self, copied: Copied) -> bool {
        let result = self.clipboard.set_copied(copied);
        let landed = result.is_ok();
        self.report_copy(result, "selection");
        landed
    }

    /// Copy every selection of the active code tab, or every caret's line when
    /// nothing is selected. `false` when the active tab is not a code tab.
    pub(super) fn copy_code_selection(&mut self) -> bool {
        let Some(taken) = self.take_from_active() else {
            return false;
        };
        self.copy_entry(taken.copied);
        true
    }

    /// Cut in the active code tab: copy as
    /// [`copy_code_selection`](Self::copy_code_selection) does, then delete
    /// exactly what was copied.
    /// Nothing is deleted when the copy did not reach the clipboard.
    pub(super) fn cut_code_selection(&mut self) {
        let Some(taken) = self.take_from_active() else {
            return;
        };
        if !self.copy_entry(taken.copied) {
            return;
        }
        let Some(Tab {
            kind: TabKind::Code { next_version, .. },
            editor,
            ..
        }) = self.tabs.get(self.active)
        else {
            return;
        };
        if let Some(edit) = editing::compose(*next_version, editor.cursors().primary, &taken.cuts) {
            self.submit_multi_edit(EditCause::Cut, edit);
        }
    }

    /// Paste `text` (newlines already normalized) into the active code tab at
    /// every caret, using the entry karet copied when the clipboard still holds
    /// its text.
    pub(super) fn paste_into_editor(&mut self, text: &str) {
        let copied = self.clipboard.copied(text).cloned();
        let edit = match self.tabs.get(self.active) {
            Some(Tab {
                kind:
                    TabKind::Code {
                        doc: Some(_),
                        next_version,
                        ..
                    },
                editor,
                ..
            }) => paste_edit(editor.cursors(), *next_version, text, copied.as_ref()),
            _ => None,
        };
        if let Some(edit) = edit {
            self.submit_multi_edit(EditCause::Paste, edit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_join_consecutive_lines() {
        let lines: BTreeSet<u32> = [0, 1, 3, 5, 6, 7].into_iter().collect();
        assert_eq!(runs(&lines), vec![(0, 1), (3, 3), (5, 7)]);
        assert!(runs(&BTreeSet::new()).is_empty());
    }

    #[test]
    fn line_cut_of_an_empty_buffer_is_nothing() {
        let buffer = TextBuffer::from_text("");
        assert!(line_cut(&buffer, (0, 0), 0, 0).is_none());
    }
}
