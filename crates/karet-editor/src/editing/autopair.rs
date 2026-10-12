//! Auto-closing brackets and quotes across every caret at once.
//!
//! Typing an opening bracket or quote inserts its partner after the caret,
//! typing a closer the editor inserted that way steps over it instead of
//! doubling it, Backspace inside an empty pair deletes both halves, an opener
//! typed over a selection wraps it, Enter between a bracket pair opens an
//! indented line between them, and a closing bracket typed on a line holding
//! only indentation snaps to the indentation of the line that opened it.
//!
//! The pairs are the language's (a client looks them up per file type and passes
//! them in), and which of them close is two settings, one for brackets and one
//! for quotes. A quote never closes inside a string or a comment: the
//! tree-sitter highlight spans the editor paints say which text that is.
//!
//! Like the other whole-cursor-set edits, each builds one batch over every
//! selection, so every caret takes part and a single undo removes the pair.

use karet_core::Change;
use karet_core::CursorState;
use karet_core::Highlights;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::Selection;
use karet_core::StandardToken;
use karet_core::TextEdit;
use karet_text::TextBuffer;

use super::Edit;
use super::MultiEdit;
use super::compose;
use super::leading_ws;
use super::map_position;

#[cfg(test)]
mod tests;

/// When an opening bracket or quote inserts its partner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AutoClose {
    /// Whenever the opener is typed, wherever the caret is.
    Always,
    /// When the caret sits before whitespace, the end of the line, or a
    /// closing punctuation mark, and, for a quote, outside strings and
    /// comments and not straight after a word character (`don't`).
    #[default]
    LanguageDefined,
    /// Never.
    Never,
}

/// A language's auto-closing pairs and the settings that govern them.
#[derive(Clone, Copy, Debug)]
pub struct AutoPairs<'a> {
    /// The `(open, close)` pairs: a bracket pair has two different characters,
    /// a quote pair the same one twice.
    pub pairs: &'a [(char, char)],
    /// When a bracket pair closes itself.
    pub brackets: AutoClose,
    /// When a quote pair closes itself.
    pub quotes: AutoClose,
    /// The buffer's syntax highlights, which say what is string and comment;
    /// `None` treats all text as code.
    pub highlights: Option<&'a Highlights>,
}

impl AutoPairs<'_> {
    /// The pair `ch` opens.
    fn opened_by(&self, ch: char) -> Option<(char, char)> {
        self.pairs.iter().copied().find(|&(open, _)| open == ch)
    }

    /// The pair `ch` closes.
    fn closed_by(&self, ch: char) -> Option<(char, char)> {
        self.pairs.iter().copied().find(|&(_, close)| close == ch)
    }

    /// The setting that governs `pair`.
    fn mode(&self, pair: (char, char)) -> AutoClose {
        if pair.0 == pair.1 {
            self.quotes
        } else {
            self.brackets
        }
    }
}

/// What typing one character did across the cursor set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Typed {
    /// The change to submit; `None` when every caret only stepped over a closer.
    pub edit: Option<MultiEdit>,
    /// The cursor set after the keystroke (the same as `edit`'s when there is one).
    pub cursors: CursorState,
    /// Every closer the editor inserted that is still open to type-over, in the
    /// coordinates after the keystroke.
    pub closers: Vec<LineCol>,
}

/// What typing does at one selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Plan {
    /// Insert the character, replacing any selection.
    Plain,
    /// Insert the pair and leave the caret between its halves.
    Close(char, char),
    /// Surround the selection with the pair, keeping it selected.
    Wrap(char, char),
    /// Step over the closer after the caret.
    Over,
    /// Replace the line's indentation before the caret with this one, then the
    /// closing bracket.
    Dedent,
}

/// Type `ch` at every selection, auto-closing, wrapping, stepping over and
/// dedenting as the pairs and settings say.
///
/// `closers` holds the closers an earlier keystroke inserted (positions in
/// `buffer`); only those are stepped over, so a closer the user typed or that
/// was already in the file is never swallowed. The client owns forgetting them
/// once the caret moves away or something else edits the buffer.
///
/// `None` when no selection does anything but insert `ch`, so the caller's
/// ordinary typing path applies.
#[must_use]
pub fn type_char(
    buffer: &TextBuffer,
    cursors: &CursorState,
    base: u64,
    ch: char,
    rules: &AutoPairs<'_>,
    closers: &[LineCol],
) -> Option<Typed> {
    let plans: Vec<Plan> = cursors
        .selections
        .iter()
        .map(|selection| plan(buffer, *selection, ch, rules, closers))
        .collect();
    if plans.iter().all(|plan| *plan == Plan::Plain) {
        return None;
    }
    let items: Vec<(Selection, Option<Edit>)> = cursors
        .selections
        .iter()
        .zip(&plans)
        .map(|(selection, plan)| {
            let edit = build(buffer, *selection, ch, *plan, rules.highlights, base);
            (*selection, Some(edit))
        })
        .collect();
    let consumed: Vec<LineCol> = cursors
        .selections
        .iter()
        .zip(&plans)
        .filter(|(_, plan)| **plan == Plan::Over)
        .map(|(selection, _)| selection.head)
        .collect();
    let kept = closers.iter().filter(|c| !consumed.contains(c));
    let Some(mut edit) = compose(base, cursors.primary, &items) else {
        // Every caret stepped over a closer: nothing to apply, only to move.
        let selections = cursors
            .selections
            .iter()
            .map(|s| Selection::caret(LineCol::new(s.head.line, s.head.col + 1)))
            .collect();
        return Some(Typed {
            edit: None,
            cursors: CursorState {
                selections,
                primary: cursors.primary,
            },
            closers: kept.copied().collect(),
        });
    };
    let edits = edit.change.edits.clone();
    for ((before, plan), after) in cursors
        .selections
        .iter()
        .zip(&plans)
        .zip(edit.cursors.selections.iter_mut())
    {
        if let Plan::Wrap(..) = plan {
            *after = wrapped(*before, &edits);
        }
    }
    let mut closers: Vec<LineCol> = kept.map(|c| map_position(*c, &edits)).collect();
    closers.extend(
        plans
            .iter()
            .zip(&edit.cursors.selections)
            .filter(|(plan, _)| matches!(plan, Plan::Close(..)))
            .map(|(_, after)| after.head),
    );
    closers.sort_unstable();
    closers.dedup();
    let cursors = edit.cursors.clone();
    Some(Typed {
        edit: Some(edit),
        cursors,
        closers,
    })
}

/// Backspace at every selection, deleting both halves of an empty pair a caret
/// sits inside (`(|)`, `"|"`) when that pair's setting is not [`AutoClose::Never`];
/// every other selection backspaces as usual.
///
/// `None` when no caret sits inside an empty pair, so the caller's ordinary
/// Backspace applies.
#[must_use]
pub fn backspace(
    buffer: &TextBuffer,
    cursors: &CursorState,
    base: u64,
    rules: &AutoPairs<'_>,
) -> Option<MultiEdit> {
    let between = |selection: &Selection| {
        let (prev, next) = around(buffer, selection.head);
        selection.is_empty()
            && rules.pairs.iter().any(|&pair| {
                rules.mode(pair) != AutoClose::Never && prev == Some(pair.0) && next == Some(pair.1)
            })
    };
    if !cursors.selections.iter().any(between) {
        return None;
    }
    let items: Vec<(Selection, Option<Edit>)> = cursors
        .selections
        .iter()
        .map(|selection| {
            let edit = if between(selection) {
                let caret = selection.head;
                let start = LineCol::new(caret.line, caret.col - 1);
                let end = LineCol::new(caret.line, caret.col + 1);
                Some(Edit {
                    change: Change::new(base, vec![text_edit(start, end, String::new())]),
                    caret: start,
                })
            } else {
                let range = selection.range();
                super::backspace(
                    selection.head,
                    (!range.is_empty()).then_some(range),
                    buffer,
                    base,
                )
            };
            (*selection, edit)
        })
        .collect();
    compose(base, cursors.primary, &items)
}

/// Enter at every selection: a caret between the halves of a bracket pair
/// (`{|}`) opens a blank line between them indented one `indentation` level
/// deeper than the opener's line, with the closer on its own line below at the
/// opener's indentation; every other selection gets the ordinary auto-indented
/// newline.
///
/// `None` when no caret sits between a bracket pair, so the caller's ordinary
/// Enter applies.
#[must_use]
pub fn newline(
    buffer: &TextBuffer,
    cursors: &CursorState,
    base: u64,
    rules: &AutoPairs<'_>,
    indentation: &str,
) -> Option<MultiEdit> {
    let between = |selection: &Selection| {
        let (prev, next) = around(buffer, selection.head);
        selection.is_empty()
            && rules
                .pairs
                .iter()
                .any(|&(open, close)| open != close && prev == Some(open) && next == Some(close))
    };
    if !cursors.selections.iter().any(between) {
        return None;
    }
    let items: Vec<(Selection, Option<Edit>)> = cursors
        .selections
        .iter()
        .map(|selection| {
            let range = selection.range();
            let edit = if between(selection) {
                let caret = selection.head;
                let indent = leading_ws(buffer, caret.line);
                let inner = format!("{indent}{indentation}");
                let landed = LineCol::new(caret.line + 1, char_count(&inner));
                Edit {
                    change: Change::new(
                        base,
                        vec![text_edit(caret, caret, format!("\n{inner}\n{indent}"))],
                    ),
                    caret: landed,
                }
            } else {
                super::newline(
                    selection.head,
                    (!range.is_empty()).then_some(range),
                    buffer,
                    base,
                )
            };
            (*selection, Some(edit))
        })
        .collect();
    compose(base, cursors.primary, &items)
}

/// What typing `ch` does at `selection`.
fn plan(
    buffer: &TextBuffer,
    selection: Selection,
    ch: char,
    rules: &AutoPairs<'_>,
    closers: &[LineCol],
) -> Plan {
    if !selection.is_empty() {
        return match rules.opened_by(ch) {
            Some(pair) if rules.mode(pair) != AutoClose::Never => Plan::Wrap(pair.0, pair.1),
            _ => Plan::Plain,
        };
    }
    let caret = selection.head;
    let (prev, next) = around(buffer, caret);
    if next == Some(ch) && closers.contains(&caret) && rules.closed_by(ch).is_some() {
        return Plan::Over;
    }
    if let Some(pair) = rules.opened_by(ch) {
        return if closes(buffer, caret, pair, prev, next, rules) {
            Plan::Close(pair.0, pair.1)
        } else {
            Plan::Plain
        };
    }
    match rules.closed_by(ch) {
        Some(_) if dedent_to(buffer, caret, ch, rules.highlights).is_some() => Plan::Dedent,
        _ => Plan::Plain,
    }
}

/// Whether typing `pair`'s opener at `caret` inserts its closer too.
fn closes(
    buffer: &TextBuffer,
    caret: LineCol,
    pair: (char, char),
    prev: Option<char>,
    next: Option<char>,
    rules: &AutoPairs<'_>,
) -> bool {
    match rules.mode(pair) {
        AutoClose::Never => false,
        AutoClose::Always => true,
        AutoClose::LanguageDefined => {
            let before_ok = next.is_none_or(|c| c.is_whitespace() || ";:.,=}])>".contains(c));
            if pair.0 != pair.1 {
                return before_ok;
            }
            let after_word = prev.is_some_and(|c| c.is_alphanumeric() || c == '_');
            before_ok && !after_word && !in_string_or_comment(buffer, rules.highlights, caret)
        },
    }
}

/// The indentation a closing bracket `close` typed at `caret` snaps to: the
/// indentation of the line holding its unclosed opener, when the caret's line
/// holds only whitespace and its indentation before the caret differs from it.
fn dedent_to(
    buffer: &TextBuffer,
    caret: LineCol,
    close: char,
    highlights: Option<&Highlights>,
) -> Option<String> {
    let line = buffer.line(caret.line as usize)?;
    if !line.chars().all(|c| c == ' ' || c == '\t') {
        return None;
    }
    let prefix: String = line.chars().take(caret.col as usize).collect();
    let opener = crate::bracket::unclosed_opener(buffer, highlights, caret, close)?;
    let indent = leading_ws(buffer, opener.line);
    (opener.line != caret.line && indent != prefix).then_some(indent)
}

/// The edit `plan` makes at `selection`.
fn build(
    buffer: &TextBuffer,
    selection: Selection,
    ch: char,
    plan: Plan,
    highlights: Option<&Highlights>,
    base: u64,
) -> Edit {
    let caret = selection.head;
    let range = selection.range();
    match plan {
        Plan::Plain => super::insert(
            caret,
            (!range.is_empty()).then_some(range),
            base,
            &ch.to_string(),
        ),
        Plan::Close(open, close) => Edit {
            change: Change::new(
                base,
                vec![text_edit(caret, caret, format!("{open}{close}"))],
            ),
            caret: LineCol::new(caret.line, caret.col + 1),
        },
        Plan::Wrap(open, close) => Edit {
            change: Change::new(
                base,
                vec![
                    text_edit(range.start, range.start, open.to_string()),
                    text_edit(range.end, range.end, close.to_string()),
                ],
            ),
            caret: range.end,
        },
        Plan::Over => Edit {
            change: Change::new(base, Vec::new()),
            caret: LineCol::new(caret.line, caret.col + 1),
        },
        Plan::Dedent => {
            let indent = dedent_to(buffer, caret, ch, highlights).unwrap_or_default();
            let text = format!("{indent}{ch}");
            let start = LineCol::new(caret.line, 0);
            Edit {
                caret: LineCol::new(caret.line, char_count(&text)),
                change: Change::new(base, vec![text_edit(start, caret, text)]),
            }
        },
    }
}

/// `before`, a selection the batch `edits` wrapped, still covering the text it
/// covered and keeping its direction: its start moves past the inserted opener
/// and its end stays before the inserted closer.
fn wrapped(before: Selection, edits: &[TextEdit]) -> Selection {
    let range = before.range();
    let start = map_position(range.start, edits);
    let end = map_position(range.end, edits);
    let end = LineCol::new(end.line, end.col.saturating_sub(1));
    if before.anchor <= before.head {
        Selection {
            anchor: start,
            head: end,
        }
    } else {
        Selection {
            anchor: end,
            head: start,
        }
    }
}

/// The characters just before and just after `pos` on its line.
fn around(buffer: &TextBuffer, pos: LineCol) -> (Option<char>, Option<char>) {
    let Some(line) = buffer.line(pos.line as usize) else {
        return (None, None);
    };
    let col = pos.col as usize;
    let prev = col.checked_sub(1).and_then(|c| line.chars().nth(c));
    (prev, line.chars().nth(col))
}

/// Whether `pos` sits inside a string or comment: the character before it is
/// comment text, or both it and the character at `pos` are string text (so a
/// caret just past a string's closing quote is outside it).
fn in_string_or_comment(
    buffer: &TextBuffer,
    highlights: Option<&Highlights>,
    pos: LineCol,
) -> bool {
    let Some(highlights) = highlights else {
        return false;
    };
    let (Some(line), Some(span)) = (
        buffer.line(pos.line as usize),
        buffer.line_to_byte_range(pos.line as usize),
    ) else {
        return false;
    };
    let byte_of = |col: usize| {
        span.start.0
            + line
                .char_indices()
                .nth(col)
                .map_or(line.len(), |(off, _)| off)
    };
    let scope = |byte: usize| {
        let spans = highlights.spans_in(span);
        let token = spans
            .iter()
            .find(|s| s.span.start.0 <= byte && byte < s.span.end.0)?
            .token;
        scope_of(token)
    };
    let col = pos.col as usize;
    let Some(before) = col.checked_sub(1).and_then(|c| scope(byte_of(c))) else {
        return false;
    };
    match before {
        Scope::Comment => true,
        Scope::String => scope(byte_of(col)) == Some(Scope::String),
    }
}

/// The kind of text a highlight token marks, for the scope check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    String,
    Comment,
}

/// The [`Scope`] of `token`, or `None` for code.
fn scope_of(token: karet_core::TokenId) -> Option<Scope> {
    let is = |tokens: &[StandardToken]| tokens.iter().any(|t| t.id() == token);
    if is(&[StandardToken::String, StandardToken::StringEscape]) {
        Some(Scope::String)
    } else if is(&[
        StandardToken::Comment,
        StandardToken::CommentDoc,
        StandardToken::CommentMark,
    ]) {
        Some(Scope::Comment)
    } else {
        None
    }
}

/// A single [`TextEdit`] replacing `[start, end)` with `new_text`.
fn text_edit(start: LineCol, end: LineCol, new_text: String) -> TextEdit {
    TextEdit {
        range: Range { start, end },
        new_text,
    }
}

/// `text`'s length in `char`s, as a column count.
fn char_count(text: &str) -> u32 {
    u32::try_from(text.chars().count()).unwrap_or(u32::MAX)
}
