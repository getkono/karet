//! Cut, copy and paste in a code tab, over every caret (the Clipboard rows of
//! `docs/editing-behavior.md`).
//!
//! A copy takes every non-empty selection, in document order, joined by
//! newlines. With no selection anywhere it takes the whole line under each
//! caret instead, and marks the clipboard entry as a line copy. A cut deletes
//! exactly what its copy took. A paste inserts at every caret; a line copy goes
//! in above the caret's line, and when the clipboard holds one piece (or one
//! line) per caret, each caret gets its own.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use karet_core::CursorState;
use karet_core::Selection;

use super::autopair::SyntaxIndent;
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

/// The leading spaces and tabs of `line`.
fn leading(line: &str) -> &str {
    let solid = line.find(|c| c != ' ' && c != '\t').unwrap_or(line.len());
    &line[..solid]
}

/// The leading whitespace of `buffer`'s `line`.
fn line_indent(buffer: &TextBuffer, line: u32) -> String {
    buffer
        .line(line as usize)
        .map_or_else(String::new, |l| leading(&l).to_owned())
}

/// `block` with its first line at `target` and every later line moved by the
/// same amount, from `base`, the level the first line had where it came from.
///
/// A later line indented by `base` and more keeps its depth beyond it; one
/// shallower than `base` keeps that shortfall below `target` (down to no
/// indentation at all). A blank line, and one whose indentation is neither a
/// prefix nor an extension of `base` (tabs against spaces), keep theirs.
fn reindent(block: &str, base: &str, target: &str) -> String {
    let mut out = String::with_capacity(block.len());
    for (row, line) in block.split('\n').enumerate() {
        if row > 0 {
            out.push('\n');
        }
        let ws = leading(line);
        let rest = &line[ws.len()..];
        if row == 0 {
            if !rest.is_empty() {
                out.push_str(target);
            }
            out.push_str(rest);
        } else if rest.trim_end_matches('\r').is_empty() {
            out.push_str(line);
        } else if let Some(deeper) = ws.strip_prefix(base) {
            out.push_str(target);
            out.push_str(deeper);
            out.push_str(rest);
        } else if base.starts_with(ws) {
            let short = base.len() - ws.len();
            out.push_str(
                target
                    .get(..target.len().saturating_sub(short))
                    .unwrap_or(""),
            );
            out.push_str(rest);
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Whether `text` spans more than one line, a final newline aside.
fn multi_line(text: &str) -> bool {
    text.strip_suffix('\n').unwrap_or(text).contains('\n')
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
        let indents = order
            .iter()
            .filter(|&&i| texts[i].is_some())
            .map(|&i| line_indent(buffer, cursors.selections[i].range().start.line))
            .collect();
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
                indents,
            },
            cuts,
        };
    }
    let lines: BTreeSet<u32> = cursors.selections.iter().map(|s| s.head.line).collect();
    let pieces: Vec<String> = lines
        .iter()
        .filter_map(|&line| buffer.line(line as usize))
        .map(|line| format!("{line}\n"))
        .collect();
    let indents = pieces
        .iter()
        .map(|piece| leading(piece).to_owned())
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
            indents,
        },
        cuts,
    }
}

/// `piece` pasted over `range` (empty for a bare caret) of `buffer` against
/// version `base`, re-indented as a block that starts its own line: the range
/// starts where only indentation precedes it, and that indentation is replaced
/// by the level `target` gives the line. `from` is the level the piece's first
/// line had where it was copied, else its own indentation. `None` when the
/// piece is one line, something precedes it on its line, or `target` gives the
/// line no level, so the piece goes in as it is.
fn own_line_paste(
    buffer: &TextBuffer,
    range: Range,
    piece: &str,
    from: Option<&str>,
    base: u64,
    target: &dyn Fn(u32) -> Option<String>,
) -> Option<editing::Edit> {
    if !multi_line(piece) {
        return None;
    }
    let start = range.start;
    let line = buffer.line(start.line as usize)?;
    if !line
        .chars()
        .take(start.col as usize)
        .all(|c| c == ' ' || c == '\t')
    {
        return None;
    }
    let to = target(start.line)?;
    let text = reindent(piece, from.unwrap_or_else(|| leading(piece)), &to);
    let line_start = LineCol::new(start.line, 0);
    let range = Range {
        start: line_start,
        end: range.end,
    };
    Some(editing::insert(line_start, Some(range), base, &text))
}

/// The level a block pasted to start line `line` of `buffer` takes from
/// `syntax`: the indentation a new line opened after the line above gets.
/// `None` on the first line, and where the language's indent query gives none.
fn paste_target(syntax: &SyntaxIndent, buffer: &TextBuffer, line: u32) -> Option<String> {
    let above = line.checked_sub(1)?;
    let end = LineCol::new(above, line_len(buffer, above));
    syntax.for_range(buffer, Range { start: end, end })
}

/// The edit pasting `text` at every caret of `cursors` against version `base`.
///
/// `copied` is the entry karet wrote, when the clipboard still holds its text.
/// With several carets, each one takes its own piece when the entry has one per
/// caret, else its own line when `text` has one line per caret; otherwise every
/// caret takes all of `text`. A whole-line piece goes in above an empty caret's
/// line, with the caret staying on its text: a line holding several carets takes
/// every piece dealt to them, in document order, or `text` once when every caret
/// took all of it. Anywhere else the
/// piece replaces the selection or goes in at the caret.
///
/// A piece of several lines that starts a line of its own (a whole-line block,
/// or a piece going in where only indentation precedes it) is re-indented: its
/// first line takes the level `target` gives the line it starts on, and the
/// rest keep their depth relative to it. Where `target` gives none, or the
/// piece is one line, it goes in as it is.
fn paste_edit(
    buffer: &TextBuffer,
    cursors: &CursorState,
    (base, text): (u64, &str),
    copied: Option<&Copied>,
    target: &dyn Fn(u32) -> Option<String>,
) -> Option<editing::MultiEdit> {
    let count = cursors.selections.len();
    let lines: Vec<&str> = text
        .strip_suffix('\n')
        .unwrap_or(text)
        .split('\n')
        .collect();
    // `dealt`: each caret took a piece of its own, rather than all of `text`.
    // `indents`: the level each piece's first line had where karet copied it.
    let (pieces, indents, whole_lines, dealt): (Vec<&str>, Vec<Option<&str>>, bool, bool) =
        match copied {
            Some(copied) if count > 1 && copied.pieces.len() == count => (
                copied.pieces.iter().map(String::as_str).collect(),
                (0..count)
                    .map(|rank| copied.indents.get(rank).map(String::as_str))
                    .collect(),
                copied.whole_lines,
                true,
            ),
            _ if count > 1 && lines.len() == count => (lines, vec![None; count], false, true),
            _ => (
                vec![text; count],
                vec![copied.and_then(|c| c.indents.first()).map(String::as_str); count],
                copied.is_some_and(|c| c.whole_lines),
                false,
            ),
        };
    let order = in_document_order(cursors);
    let mut piece_of = vec![""; count];
    let mut indent_of = vec![None; count];
    for (rank, &i) in order.iter().enumerate() {
        piece_of[i] = pieces[rank];
        indent_of[i] = indents[rank];
    }
    // The whole-line text going in above each empty caret's line: every piece
    // dealt to a caret on that line, in document order, or `text` once when
    // every caret took all of it.
    let mut above: BTreeMap<u32, String> = BTreeMap::new();
    if whole_lines {
        for &i in &order {
            let selection = &cursors.selections[i];
            if !selection.range().is_empty() {
                continue;
            }
            let block = above.entry(selection.head.line).or_default();
            if dealt || block.is_empty() {
                block.push_str(piece_of[i]);
            }
        }
    }
    let mut pasted_lines = BTreeSet::new();
    let items: Vec<_> = cursors
        .selections
        .iter()
        .zip(piece_of.into_iter().zip(indent_of))
        .map(|(selection, (piece, from))| {
            let range = selection.range();
            if !range.is_empty() || !whole_lines {
                let edit = own_line_paste(buffer, range, piece, from, base, target)
                    .unwrap_or_else(|| editing::insert(selection.head, Some(range), base, piece));
                return (*selection, Some(edit));
            }
            let head = selection.head;
            let block = above.get(&head.line).map_or(piece, String::as_str);
            if !pasted_lines.insert(head.line) {
                // Another caret on the line already inserts its whole block,
                // this caret's piece included; this one moves down with it.
                return (*selection, None);
            }
            let reindented = multi_line(block)
                .then(|| target(head.line))
                .flatten()
                .map(|to| reindent(block, leading(block), &to));
            let block = reindented.as_deref().unwrap_or(block);
            let mut edit = editing::insert(LineCol::new(head.line, 0), None, base, block);
            let added = u32::try_from(block.matches('\n').count()).unwrap_or(u32::MAX);
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
        // Only a paste of several lines can re-indent, so only one parses.
        let syntax = multi_line(text).then(|| self.syntax_indent()).flatten();
        let edit = match self.tabs.get(self.active) {
            Some(Tab {
                kind:
                    TabKind::Code {
                        doc: Some(_),
                        buffer,
                        next_version,
                        ..
                    },
                editor,
                ..
            }) => {
                let target = |line: u32| {
                    syntax
                        .as_ref()
                        .and_then(|syntax| paste_target(syntax, buffer, line))
                };
                paste_edit(
                    buffer,
                    editor.cursors(),
                    (*next_version, text),
                    copied.as_ref(),
                    &target,
                )
            },
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
    fn reindent_moves_every_line_by_the_first_lines_shift() {
        // Deeper lines keep their depth past `base`; the first line's own
        // indentation gives way to `target`.
        assert_eq!(reindent("  a\n    b\n  c", "  ", "\t"), "\ta\n\t  b\n\tc");
        // A shortfall below `base` comes off `target`, down to nothing.
        assert_eq!(
            reindent("a\n  b\nc", "    ", "      "),
            "      a\n    b\n  c"
        );
        assert_eq!(reindent("a\nb", "    ", "  "), "  a\nb");
        // Blank lines, and indentation that does not extend `base`, stay.
        assert_eq!(reindent("a\n  \n\tb", "    ", "  "), "  a\n  \n\tb");
        // A blank first line takes no indentation; a final newline stays.
        assert_eq!(reindent("\n  b\n", "", "  "), "\n    b\n");
    }

    #[test]
    fn line_cut_of_an_empty_buffer_is_nothing() {
        let buffer = TextBuffer::from_text("");
        assert!(line_cut(&buffer, (0, 0), 0, 0).is_none());
    }
}
