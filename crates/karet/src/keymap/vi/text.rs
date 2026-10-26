//! A read-only, character-addressed view of a [`TextBuffer`] for the modal
//! grammars: positions are `(line, char column)` and every line but the last
//! ends in a virtual `'\n'` at column `len`, so a motion can walk the whole
//! document one character at a time. A file's final line break ends its last
//! line, as in vim: the empty "line" after it is not one the grammars visit.

use std::cell::RefCell;
use std::rc::Rc;

use karet_core::LineCol;
use karet_core::Range;
use karet_text::TextBuffer;

/// The class a character belongs to for word motions. A `WORD` (`big`) motion
/// folds [`Class::Punct`] into [`Class::Word`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Class {
    /// Spaces and tabs.
    Blank,
    /// Letters, digits and `_` (and, for a `WORD`, every other non-blank).
    Word,
    /// Every other non-blank character.
    Punct,
    /// A line break, or the end of the document.
    Eol,
}

/// The [`Class`] of `c`.
pub(super) fn class(c: char, big: bool) -> Class {
    if c == '\n' {
        Class::Eol
    } else if c.is_whitespace() {
        Class::Blank
    } else if big || c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

/// A document the grammars read: the buffer plus a one-line cache, since a
/// motion asks for the same line many times in a row.
pub(super) struct Doc<'a> {
    buffer: &'a TextBuffer,
    last_line: u32,
    cache: RefCell<Option<(u32, Rc<[char]>)>>,
}

impl<'a> Doc<'a> {
    /// A view of `buffer`.
    pub(super) fn new(buffer: &'a TextBuffer) -> Self {
        let count = buffer.line_count();
        // A trailing line break ends the last line rather than starting an
        // empty one, so the rope's final, empty line is not counted.
        let phantom = count > 1 && buffer.line(count - 1).is_some_and(|line| line.is_empty());
        let last = count.saturating_sub(if phantom { 2 } else { 1 });
        Self {
            buffer,
            last_line: u32::try_from(last).unwrap_or(u32::MAX),
            cache: RefCell::new(None),
        }
    }

    /// The index of the last line (a buffer always has at least one). The
    /// empty line after a final line break is not counted.
    pub(super) fn last_line(&self) -> u32 {
        self.last_line
    }

    /// The characters of `line`, without its break; empty past the end.
    pub(super) fn chars(&self, line: u32) -> Rc<[char]> {
        if let Some((cached, chars)) = self.cache.borrow().as_ref()
            && *cached == line
        {
            return Rc::clone(chars);
        }
        let chars: Rc<[char]> = self
            .buffer
            .line(line as usize)
            .unwrap_or_default()
            .chars()
            .collect();
        *self.cache.borrow_mut() = Some((line, Rc::clone(&chars)));
        chars
    }

    /// The number of characters on `line`.
    pub(super) fn len(&self, line: u32) -> u32 {
        u32::try_from(self.chars(line).len()).unwrap_or(u32::MAX)
    }

    /// The character at `p`: `'\n'` at or past the end of a line.
    pub(super) fn ch(&self, p: LineCol) -> char {
        self.chars(p.line)
            .get(p.col as usize)
            .copied()
            .unwrap_or('\n')
    }

    /// The position after `p`, crossing a line break; `None` at the very end.
    pub(super) fn next(&self, p: LineCol) -> Option<LineCol> {
        if p.col < self.len(p.line) {
            Some(LineCol::new(p.line, p.col + 1))
        } else if p.line < self.last_line() {
            Some(LineCol::new(p.line + 1, 0))
        } else {
            None
        }
    }

    /// The position before `p`, crossing a line break; `None` at the start.
    pub(super) fn prev(&self, p: LineCol) -> Option<LineCol> {
        if p.col > 0 {
            Some(LineCol::new(p.line, p.col.min(self.len(p.line)) - 1))
        } else if p.line > 0 {
            Some(LineCol::new(p.line - 1, self.len(p.line - 1)))
        } else {
            None
        }
    }

    /// `p` clamped into the document, allowing the end-of-line column and,
    /// past the last line, [`Self::end_of_doc`].
    pub(super) fn clamp(&self, p: LineCol) -> LineCol {
        if p.line > self.last_line() {
            return self.end_of_doc();
        }
        LineCol::new(p.line, p.col.min(self.len(p.line)))
    }

    /// The position after the whole document: past the final line break
    /// when the file ends in one, else the last line's end.
    pub(super) fn end_of_doc(&self) -> LineCol {
        let last = self.last_line();
        if self.trailing_break() {
            LineCol::new(last + 1, 0)
        } else {
            self.line_end(last)
        }
    }

    /// Whether the document ends in a line break.
    fn trailing_break(&self) -> bool {
        (self.last_line as usize) + 1 < self.buffer.line_count()
    }

    /// `p` clamped onto a character, as Normal mode keeps the cursor: never
    /// past the last character of a non-empty line.
    pub(super) fn clamp_char(&self, p: LineCol) -> LineCol {
        let line = p.line.min(self.last_line());
        LineCol::new(line, p.col.min(self.len(line).saturating_sub(1)))
    }

    /// The first non-blank character of `line` (its end when it is blank).
    pub(super) fn first_non_blank(&self, line: u32) -> LineCol {
        let chars = self.chars(line);
        let col = chars
            .iter()
            .position(|c| !c.is_whitespace())
            .unwrap_or(chars.len());
        LineCol::new(line, u32::try_from(col).unwrap_or(u32::MAX))
    }

    /// The last non-blank character of `line` (column 0 when it is blank).
    pub(super) fn last_non_blank(&self, line: u32) -> LineCol {
        let chars = self.chars(line);
        let col = chars
            .iter()
            .rposition(|c| !c.is_whitespace())
            .unwrap_or_default();
        LineCol::new(line, u32::try_from(col).unwrap_or(u32::MAX))
    }

    /// Whether `line` holds nothing but blanks.
    pub(super) fn is_blank_line(&self, line: u32) -> bool {
        self.chars(line).iter().all(|c| c.is_whitespace())
    }

    /// The leading blanks of `line`.
    pub(super) fn indent_of(&self, line: u32) -> String {
        self.chars(line)
            .iter()
            .take_while(|c| **c == ' ' || **c == '\t')
            .collect()
    }

    /// The end of `line`: the column of its break.
    pub(super) fn line_end(&self, line: u32) -> LineCol {
        LineCol::new(line, self.len(line))
    }

    /// The exclusive end of an inclusive selection ending at `p`: the
    /// character after it, which is the next line's start when `p` is a break.
    pub(super) fn after(&self, p: LineCol) -> LineCol {
        let len = self.len(p.line);
        if p.col < len {
            LineCol::new(p.line, p.col + 1)
        } else if p.line < self.last_line() {
            LineCol::new(p.line + 1, 0)
        } else {
            LineCol::new(p.line, len)
        }
    }

    /// The text of `range` (`start` inclusive, `end` exclusive).
    pub(super) fn text(&self, range: Range) -> String {
        let mut out = String::new();
        let (start, end) = (self.clamp(range.start), self.clamp(range.end));
        if end <= start {
            return out;
        }
        for line in start.line..=end.line {
            let chars = self.chars(line);
            let from = if line == start.line { start.col } else { 0 } as usize;
            let to = if line == end.line {
                end.col as usize
            } else {
                chars.len()
            };
            out.extend(chars.get(from..to.max(from)).unwrap_or_default());
            if line != end.line {
                out.push('\n');
            }
        }
        out
    }

    /// The text of whole lines `first..=last`, each with its break.
    pub(super) fn lines_text(&self, first: u32, last: u32) -> String {
        let mut out = String::new();
        for line in first..=last.min(self.last_line()) {
            out.extend(self.chars(line).iter());
            out.push('\n');
        }
        out
    }
}

/// The position after inserting `text` at `start`.
pub(super) fn end_of_insert(start: LineCol, text: &str) -> LineCol {
    karet_editor::editing::caret_after_insert(start, text)
}
