//! Vim's Visual modes: motions move the cursor end of the selection, and an
//! operator key acts on the selection at once.

use karet_core::EditCause;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::TextEdit;

use super::Clipboard;
use super::Done;
use super::Effect;
use super::Engine;
use super::Key;
use super::Mode;
use super::Op;
use super::Sel;
use super::Snapshot;
use super::Wait;
use super::Write;
use super::object::Span;
use super::ops;
use super::ops::Case;
use super::text::Doc;
use super::vim::Cmd;
use super::vim::Step;
use super::vim::span_lines;

impl Engine {
    /// The selection as an operator span: the characters (the inclusive end
    /// taken in), or the lines in linewise Visual mode.
    pub(super) fn visual_span(&self, doc: &Doc) -> Span {
        let (start, end) = (self.sel.start(), self.sel.end());
        if self.mode == Mode::VisualLine {
            return Span::Lines(start.line, end.line);
        }
        Span::Chars(Range {
            start,
            end: doc.after(end),
        })
    }

    /// Select a text object's span (`viw`).
    pub(super) fn select_span(&mut self, doc: &Doc, span: Span) {
        match span {
            Span::Chars(range) => {
                self.mode = Mode::Visual;
                self.sel = Sel {
                    anchor: range.start,
                    cursor: doc
                        .prev(range.end)
                        .filter(|p| *p >= range.start)
                        .unwrap_or(range.start),
                };
            },
            Span::Lines(first, last) => {
                self.mode = Mode::VisualLine;
                self.sel = Sel {
                    anchor: LineCol::new(first, 0),
                    cursor: LineCol::new(last, 0),
                };
            },
        }
    }

    /// A key in Visual mode.
    pub(super) fn visual_key(
        &mut self,
        cmd: Cmd,
        doc: &Doc,
        snap: &Snapshot,
        clip: &mut dyn Clipboard,
        fx: &mut Vec<Effect>,
    ) -> Done {
        match self.vim_motion(cmd, doc, snap, false) {
            Step::Target(target) => {
                self.move_cursor(target, doc, fx);
                return Done::Motion;
            },
            Step::Wait(wait) => {
                self.pending.wait = Some(wait);
                return Done::Pending;
            },
            Step::Fail => return Done::Motion,
            Step::NotMotion => {},
        }
        let span = self.visual_span(doc);
        let (first, last) = span_lines(span);
        let lines = Span::Lines(first, last);
        let line_mode = self.mode == Mode::VisualLine;
        let op = |op: Op, span: Span| Some((op, span));
        let operation = match cmd {
            Cmd::Key(Key::Char('d' | 'x') | Key::Delete) => op(Op::Delete, span),
            Cmd::Key(Key::Char('X' | 'D')) => op(Op::Delete, lines),
            Cmd::Key(Key::Char('c' | 's')) => op(Op::Change, span),
            Cmd::Key(Key::Char('C' | 'S' | 'R')) => op(Op::Change, lines),
            Cmd::Key(Key::Char('y')) => op(Op::Yank, span),
            Cmd::Key(Key::Char('Y')) => op(Op::Yank, lines),
            Cmd::Key(Key::Char('>')) => op(Op::Indent, lines),
            Cmd::Key(Key::Char('<')) => op(Op::Outdent, lines),
            Cmd::Key(Key::Char('~')) | Cmd::G(Key::Char('~')) => op(Op::Case(Case::Toggle), span),
            Cmd::Key(Key::Char('u')) | Cmd::G(Key::Char('u')) => op(Op::Case(Case::Lower), span),
            Cmd::Key(Key::Char('U')) | Cmd::G(Key::Char('U')) => op(Op::Case(Case::Upper), span),
            _ => None,
        };
        if let Some((op, span)) = operation {
            // A change made from a selection is not repeated by `.`: the
            // keys that made it do not say how much text it covered. So
            // whatever the operator reports, this is not a change to record.
            let _ = self.operate(op, span, doc, snap, clip, fx);
            if self.mode != Mode::Insert {
                self.mode = Mode::Normal;
                if op == Op::Yank {
                    self.sel = Sel::at(doc.clamp_char(match span {
                        Span::Chars(range) => range.start,
                        Span::Lines(first, _) => LineCol::new(first, self.sel.cursor.col),
                    }));
                }
            }
            return Done::Motion;
        }
        match cmd {
            Cmd::Key(Key::Esc | Key::Ctrl('c')) => self.leave_visual(doc),
            Cmd::Key(Key::Char('v')) if line_mode => {
                self.mode = Mode::Visual;
                Done::Motion
            },
            Cmd::Key(Key::Char('v')) => self.leave_visual(doc),
            Cmd::Key(Key::Char('V')) if line_mode => self.leave_visual(doc),
            Cmd::Key(Key::Char('V')) => {
                self.mode = Mode::VisualLine;
                Done::Motion
            },
            Cmd::Key(Key::Char('o' | 'O')) => {
                std::mem::swap(&mut self.sel.anchor, &mut self.sel.cursor);
                Done::Motion
            },
            Cmd::Key(Key::Char(c @ ('i' | 'a'))) => {
                self.pending.wait = Some(Wait::Object { inner: c == 'i' });
                Done::Pending
            },
            Cmd::Key(Key::Char('g')) => {
                self.pending.wait = Some(Wait::Prefix('g'));
                Done::Pending
            },
            Cmd::Key(Key::Char('r')) => {
                self.pending.wait = Some(Wait::Replace);
                Done::Pending
            },
            Cmd::Key(Key::Char('J')) | Cmd::G(Key::Char('J')) => {
                self.mode = Mode::Normal;
                let spaces = matches!(cmd, Cmd::Key(_));
                match ops::join(doc, first, last.max(first + 1), spaces) {
                    Some(planned) => self.apply(planned, EditCause::Replace, fx),
                    None => self.sel = Sel::at(self.sel.start()),
                }
                Done::Motion
            },
            Cmd::Key(Key::Char(c @ ('p' | 'P'))) => self.visual_put(c == 'p', span, doc, clip, fx),
            Cmd::G(_) => Done::Motion,
            _ => Done::Unhandled,
        }
    }

    /// Back to Normal mode with the cursor where it is (off a line break
    /// `$` took in).
    fn leave_visual(&mut self, doc: &Doc) -> Done {
        self.mode = Mode::Normal;
        self.sel = Sel::at(doc.clamp_char(self.sel.cursor));
        Done::Motion
    }

    /// `r<c>` over the selection.
    pub(super) fn visual_replace(&mut self, c: char, doc: &Doc, fx: &mut Vec<Effect>) -> Done {
        let range = match self.visual_span(doc) {
            Span::Chars(range) => range,
            Span::Lines(first, last) => ops::lines_chars(doc, first, last),
        };
        self.mode = Mode::Normal;
        let start = range.start;
        match ops::replace_chars(doc, range, c) {
            Some(edit) => self.apply(
                ops::Planned {
                    edits: vec![edit],
                    cursor: start,
                },
                EditCause::Replace,
                fx,
            ),
            None => self.sel = Sel::at(start),
        }
        Done::Motion
    }

    /// `p`/`P` over a selection: replace it with the register. `p` puts the
    /// replaced text in the unnamed register; `P` leaves the registers alone.
    fn visual_put(
        &mut self,
        swap: bool,
        span: Span,
        doc: &Doc,
        clip: &mut dyn Clipboard,
        fx: &mut Vec<Effect>,
    ) -> Done {
        let Some(reg) = self.fetch(clip) else {
            return self.leave_visual(doc);
        };
        let replaced = ops::yank(doc, span);
        let (range, text) = match span {
            Span::Lines(first, last) if reg.linewise => (
                Range {
                    start: LineCol::new(first, 0),
                    end: doc.line_end(last),
                },
                reg.text.strip_suffix('\n').unwrap_or(&reg.text).to_string(),
            ),
            Span::Lines(first, last) => (ops::lines_chars(doc, first, last), reg.text.clone()),
            Span::Chars(range) if reg.linewise => (range, format!("\n{}", reg.text)),
            Span::Chars(range) => (range, reg.text.clone()),
        };
        let cursor = if reg.linewise {
            let line = if matches!(span, Span::Chars(_)) {
                range.start.line + 1
            } else {
                range.start.line
            };
            let lead = reg
                .text
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .count();
            LineCol::new(line, u32::try_from(lead).unwrap_or(0))
        } else {
            let end = super::text::end_of_insert(range.start, &text);
            LineCol::new(end.line, end.col.saturating_sub(1))
        };
        if swap {
            self.registers.write(None, replaced, Write::Delete, clip);
        }
        self.mode = Mode::Normal;
        self.apply(
            ops::Planned {
                edits: vec![TextEdit {
                    range,
                    new_text: text,
                }],
                cursor,
            },
            EditCause::Paste,
            fx,
        );
        Done::Motion
    }
}
