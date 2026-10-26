//! The vim grammar: counts, operators over motions and text objects, Normal
//! and Visual mode commands. Motions are shared with Visual mode and with
//! operators through [`Engine::vim_motion`].

use karet_core::EditCause;
use karet_core::LineCol;
use karet_core::Range;

use super::Clipboard;
use super::Done;
use super::Effect;
use super::Engine;
use super::Key;
use super::Mode;
use super::Op;
use super::Scroll;
use super::Sel;
use super::Snapshot;
use super::Wait;
use super::Write;
use super::motion;
use super::motion::CharSearch;
use super::object;
use super::object::Object;
use super::object::Span;
use super::ops;
use super::ops::Case;
use super::ops::Put;
use super::register;
use super::text;
use super::text::Doc;
pub(super) use super::vim_motion::Cmd;
use super::vim_motion::Kind;
pub(super) use super::vim_motion::Step;
use super::vim_motion::Target;
use crate::command::Command;

impl Engine {
    /// Handle a key in vim's Normal (or Visual) mode.
    pub(super) fn vim_key(
        &mut self,
        key: Key,
        doc: &Doc,
        snap: &Snapshot,
        clip: &mut dyn Clipboard,
        fx: &mut Vec<Effect>,
    ) -> Done {
        if let Some(wait) = self.pending.wait.take() {
            return self.vim_wait(wait, key, doc, snap, clip, fx);
        }
        if let Some(done) = self.count_or_register(key) {
            return done;
        }
        if matches!(self.mode, Mode::Visual | Mode::VisualLine) {
            return self.visual_key(Cmd::Key(key), doc, snap, clip, fx);
        }
        if key == Key::Esc {
            let typed = self.pending.op.is_some()
                || self.pending.count.is_some()
                || self.pending.register.is_some();
            return if typed { Done::Motion } else { Done::Unhandled };
        }
        self.vim_cmd(Cmd::Key(key), doc, snap, clip, fx)
    }

    /// A count digit or a `"` register prefix, if `key` is one.
    pub(super) fn count_or_register(&mut self, key: Key) -> Option<Done> {
        match key {
            Key::Char(c @ '0'..='9') => {
                let digit = c.to_digit(10)?;
                let slot = if self.pending.op.is_some() {
                    &mut self.pending.op_count
                } else {
                    &mut self.pending.count
                };
                if digit == 0 && slot.is_none() {
                    return None;
                }
                *slot = Some(
                    slot.unwrap_or(0)
                        .saturating_mul(10)
                        .saturating_add(digit)
                        .min(super::MAX_COUNT),
                );
                Some(Done::Pending)
            },
            Key::Char('"') if self.pending.op.is_none() => {
                self.pending.wait = Some(Wait::Register);
                Some(Done::Pending)
            },
            _ => None,
        }
    }

    /// Complete the command waiting on `key`.
    fn vim_wait(
        &mut self,
        wait: Wait,
        key: Key,
        doc: &Doc,
        snap: &Snapshot,
        clip: &mut dyn Clipboard,
        fx: &mut Vec<Effect>,
    ) -> Done {
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        let ch = match key {
            Key::Char(c) => Some(c),
            Key::Enter => Some('\n'),
            Key::Tab => Some('\t'),
            _ => None,
        };
        match wait {
            Wait::Register => match ch {
                Some(c) if register::is_valid(c) => {
                    self.pending.register = Some(c);
                    Done::Pending
                },
                _ => Done::Motion,
            },
            Wait::Prefix('g') if visual => self.visual_key(Cmd::G(key), doc, snap, clip, fx),
            Wait::Prefix('g') => self.vim_cmd(Cmd::G(key), doc, snap, clip, fx),
            Wait::Prefix(_) => self.vim_cmd(Cmd::Z(key), doc, snap, clip, fx),
            Wait::Find { forward, till } => {
                let Some(c) = ch.filter(|c| *c != '\n') else {
                    return Done::Motion;
                };
                let search = CharSearch {
                    ch: c,
                    forward,
                    till,
                };
                self.last_find = Some(search);
                let count = self.pending.total().unwrap_or(1);
                let kind = if forward {
                    Kind::Inclusive
                } else {
                    Kind::Exclusive
                };
                match motion::find_char(doc, self.sel.cursor, search, count, false) {
                    Some(pos) => self.take_target(Target::new(pos, kind), doc, snap, clip, fx),
                    None => Done::Motion,
                }
            },
            Wait::Replace => match ch {
                Some(c) if visual => self.visual_replace(c, doc, fx),
                Some(c) => self.replace_chars(c, doc, fx),
                None => Done::Motion,
            },
            Wait::Object { inner } => {
                let Some(object) = ch.and_then(Object::from_char) else {
                    return Done::Motion;
                };
                let count = self.pending.total().unwrap_or(1);
                let Some(span) = object::select(doc, self.sel.cursor, object, inner, count) else {
                    return Done::Motion;
                };
                if visual {
                    self.select_span(doc, span);
                    return Done::Motion;
                }
                match self.pending.op {
                    Some(op) => self.operate(op, span, doc, snap, clip, fx),
                    None => Done::Motion,
                }
            },
            Wait::Record => match ch {
                Some(c) if c.is_ascii_alphanumeric() => {
                    self.toggle_recording(Some(c));
                    Done::Motion
                },
                _ => Done::Motion,
            },
            Wait::Play => match ch {
                Some('@') => {
                    let name = self.last_macro.unwrap_or('@');
                    self.play(name, self.pending.total().unwrap_or(1), fx)
                },
                Some(c) if c.is_ascii_alphanumeric() => {
                    self.play(c, self.pending.total().unwrap_or(1), fx)
                },
                _ => Done::Motion,
            },
        }
    }

    /// Run `op` over `span`.
    pub(super) fn operate(
        &mut self,
        op: Op,
        span: Span,
        doc: &Doc,
        snap: &Snapshot,
        clip: &mut dyn Clipboard,
        fx: &mut Vec<Effect>,
    ) -> Done {
        let cursor = self.sel.cursor;
        match op {
            Op::Delete => {
                let reg = ops::yank(doc, span);
                if reg.text.is_empty() {
                    return Done::Motion;
                }
                self.store(reg, Write::Delete, clip);
                self.apply(ops::delete(doc, span), EditCause::Delete, fx);
                Done::Change
            },
            Op::Change => {
                let reg = ops::yank(doc, span);
                if !reg.text.is_empty() {
                    self.store(reg, Write::Delete, clip);
                }
                self.apply(ops::change(doc, span), EditCause::Delete, fx);
                self.mode = Mode::Insert;
                Done::Insert
            },
            Op::Yank => {
                self.store(ops::yank(doc, span), Write::Yank, clip);
                let at = match span {
                    Span::Chars(range) => range.start,
                    Span::Lines(first, _) if first < cursor.line => {
                        doc.clamp_char(LineCol::new(first, cursor.col))
                    },
                    Span::Lines(..) => cursor,
                };
                self.sel = Sel::at(at);
                Done::Motion
            },
            Op::Indent | Op::Outdent => {
                let (first, last) = span_lines(span);
                let count = if matches!(self.mode, Mode::Visual | Mode::VisualLine) {
                    self.pending.total().unwrap_or(1)
                } else {
                    1
                };
                let planned = ops::shift(
                    doc,
                    (first, last),
                    snap.indent,
                    snap.indent_width(),
                    count,
                    op == Op::Outdent,
                );
                if planned.edits.is_empty() {
                    self.sel = Sel::at(planned.cursor);
                    return Done::Motion;
                }
                self.apply(planned, EditCause::Replace, fx);
                Done::Change
            },
            Op::Case(case) => {
                let range = match span {
                    Span::Chars(range) => range,
                    Span::Lines(first, last) => ops::lines_chars(doc, first, last),
                };
                let at = match span {
                    Span::Chars(range) => range.start,
                    Span::Lines(first, last) if (first..=last).contains(&cursor.line) => {
                        // The cursor's column once the text before it is
                        // re-cased, which can change its length.
                        let line_start = LineCol::new(cursor.line, 0);
                        let before = Range {
                            start: line_start,
                            end: cursor,
                        };
                        let col = ops::case(doc, before, case).map_or(cursor.col, |edit| {
                            u32::try_from(edit.new_text.chars().count()).unwrap_or(cursor.col)
                        });
                        LineCol::new(cursor.line, col)
                    },
                    Span::Lines(first, _) => LineCol::new(first, 0),
                };
                let Some(edit) = ops::case(doc, range, case) else {
                    self.sel = Sel::at(doc.clamp_char(at));
                    return Done::Motion;
                };
                self.apply(
                    ops::Planned {
                        edits: vec![edit],
                        cursor: at,
                    },
                    EditCause::Replace,
                    fx,
                );
                Done::Change
            },
        }
    }

    /// A Normal-mode command, motion or operator key.
    fn vim_cmd(
        &mut self,
        cmd: Cmd,
        doc: &Doc,
        snap: &Snapshot,
        clip: &mut dyn Clipboard,
        fx: &mut Vec<Effect>,
    ) -> Done {
        if let Some(op) = self.pending.op {
            return self.op_cmd(op, cmd, doc, snap, clip, fx);
        }
        match self.vim_motion(cmd, doc, snap, false) {
            Step::Target(target) => return self.take_target(target, doc, snap, clip, fx),
            Step::Wait(wait) => {
                self.pending.wait = Some(wait);
                return Done::Pending;
            },
            Step::Fail => return Done::Motion,
            Step::NotMotion => {},
        }
        let n = self.pending.total().unwrap_or(1);
        let at = self.sel.cursor;
        let op = |op: Op| Some(op);
        let operator = match cmd {
            Cmd::Key(Key::Char('d')) => op(Op::Delete),
            Cmd::Key(Key::Char('c')) => op(Op::Change),
            Cmd::Key(Key::Char('y')) => op(Op::Yank),
            Cmd::Key(Key::Char('>')) => op(Op::Indent),
            Cmd::Key(Key::Char('<')) => op(Op::Outdent),
            Cmd::G(Key::Char('u')) => op(Op::Case(Case::Lower)),
            Cmd::G(Key::Char('U')) => op(Op::Case(Case::Upper)),
            Cmd::G(Key::Char('~')) => op(Op::Case(Case::Toggle)),
            _ => None,
        };
        if let Some(operator) = operator {
            self.pending.op = Some(operator);
            return Done::Pending;
        }
        let shorthand = |op: Op, key: char| Some((op, Cmd::Key(Key::Char(key))));
        let abbreviation = match cmd {
            Cmd::Key(Key::Char('x') | Key::Delete) => shorthand(Op::Delete, 'l'),
            Cmd::Key(Key::Char('X')) => shorthand(Op::Delete, 'h'),
            Cmd::Key(Key::Char('D')) => shorthand(Op::Delete, '$'),
            Cmd::Key(Key::Char('C')) => shorthand(Op::Change, '$'),
            Cmd::Key(Key::Char('s')) => shorthand(Op::Change, 'l'),
            Cmd::Key(Key::Char('Y')) => shorthand(Op::Yank, '$'),
            _ => None,
        };
        if let Some((op, motion)) = abbreviation {
            self.pending.op = Some(op);
            return self.op_cmd(op, motion, doc, snap, clip, fx);
        }
        match cmd {
            Cmd::Key(Key::Char('S')) => {
                let last = at.line.saturating_add(n - 1);
                self.operate(Op::Change, Span::Lines(at.line, last), doc, snap, clip, fx)
            },
            Cmd::Key(Key::Char(c @ ('p' | 'P'))) => {
                let Some(reg) = self.fetch(clip) else {
                    return Done::Motion;
                };
                let put = if c == 'p' { Put::After } else { Put::Before };
                let planned = ops::paste(doc, at, &reg, put, n);
                self.apply(planned, EditCause::Paste, fx);
                Done::Change
            },
            Cmd::Key(Key::Char('J')) | Cmd::G(Key::Char('J')) => {
                let spaces = matches!(cmd, Cmd::Key(_));
                match ops::join(doc, at.line, at.line + n.max(2) - 1, spaces) {
                    Some(planned) => {
                        self.apply(planned, EditCause::Replace, fx);
                        Done::Change
                    },
                    None => Done::Motion,
                }
            },
            Cmd::Key(Key::Char('r')) => {
                self.pending.wait = Some(Wait::Replace);
                Done::Pending
            },
            Cmd::Key(Key::Char('~')) => {
                let end = LineCol::new(at.line, at.col.saturating_add(n).min(doc.len(at.line)));
                let next = doc.clamp_char(end);
                match ops::case(doc, Range { start: at, end }, Case::Toggle) {
                    Some(edit) => {
                        // After the rewritten text, which can be longer than
                        // what it replaced (`ß` → `SS`).
                        let written = u32::try_from(edit.new_text.chars().count()).unwrap_or(0);
                        let len = doc.len(at.line) - (end.col - at.col) + written;
                        let col = (at.col + written).min(len.saturating_sub(1));
                        self.apply(
                            ops::Planned {
                                edits: vec![edit],
                                cursor: LineCol::new(at.line, col),
                            },
                            EditCause::Replace,
                            fx,
                        );
                        Done::Change
                    },
                    None => {
                        self.sel = Sel::at(next);
                        Done::Motion
                    },
                }
            },
            Cmd::Key(Key::Char('i')) => self.insert_at(at),
            Cmd::Key(Key::Char('a')) => {
                self.insert_at(LineCol::new(at.line, (at.col + 1).min(doc.len(at.line))))
            },
            Cmd::Key(Key::Char('I')) => self.insert_at(doc.first_non_blank(at.line)),
            Cmd::G(Key::Char('I')) => self.insert_at(LineCol::new(at.line, 0)),
            Cmd::Key(Key::Char('A')) => self.insert_at(doc.line_end(at.line)),
            Cmd::Key(Key::Char('o')) => self.open_below(at.line, doc, fx),
            Cmd::Key(Key::Char('O')) => self.open_above(at.line, doc, fx),
            Cmd::Key(Key::Char('v')) => {
                self.mode = Mode::Visual;
                self.sel = Sel::at(at);
                Done::Motion
            },
            Cmd::Key(Key::Char('V')) => {
                self.mode = Mode::VisualLine;
                self.sel = Sel::at(at);
                Done::Motion
            },
            Cmd::Key(Key::Char('.')) => self.repeat_change(n, fx),
            Cmd::Key(Key::Char('q')) if self.recording.is_some() => {
                self.toggle_recording(None);
                Done::Motion
            },
            Cmd::Key(Key::Char('q')) => {
                self.pending.wait = Some(Wait::Record);
                Done::Pending
            },
            Cmd::Key(Key::Char('@')) => {
                self.pending.wait = Some(Wait::Play);
                Done::Pending
            },
            Cmd::Key(Key::Char(c @ ('g' | 'z'))) => {
                self.pending.wait = Some(Wait::Prefix(c));
                Done::Pending
            },
            Cmd::Z(Key::Char('z' | '.')) => self.scroll(Scroll::Center, fx),
            Cmd::Z(Key::Char('t') | Key::Enter) => self.scroll(Scroll::Top, fx),
            Cmd::Z(Key::Char('b' | '-')) => self.scroll(Scroll::Bottom, fx),
            Cmd::Key(Key::Ctrl('e')) => {
                self.scroll(Scroll::Lines(i32::try_from(n).unwrap_or(1)), fx)
            },
            Cmd::Key(Key::Ctrl('y')) => {
                self.scroll(Scroll::Lines(-i32::try_from(n).unwrap_or(1)), fx)
            },
            Cmd::Key(Key::Ctrl(c @ ('a' | 'x'))) => {
                let delta = if c == 'a' {
                    i64::from(n)
                } else {
                    -i64::from(n)
                };
                match ops::increment(doc, at, delta) {
                    Some(planned) => {
                        self.apply(planned, EditCause::Replace, fx);
                        Done::Change
                    },
                    None => Done::Motion,
                }
            },
            // Tab is vim's jump-forward, which karet keeps in the layers; in
            // Normal mode it must never indent.
            Cmd::Key(Key::Tab) => Done::Motion,
            Cmd::G(_) | Cmd::Z(_) => Done::Motion,
            _ => Done::Unhandled,
        }
    }

    /// A key while operator `op` waits for its motion.
    fn op_cmd(
        &mut self,
        op: Op,
        cmd: Cmd,
        doc: &Doc,
        snap: &Snapshot,
        clip: &mut dyn Clipboard,
        fx: &mut Vec<Effect>,
    ) -> Done {
        let n = self.pending.total().unwrap_or(1);
        let at = self.sel.cursor;
        let doubled = matches!(
            (op, cmd),
            (Op::Delete, Cmd::Key(Key::Char('d')))
                | (Op::Change, Cmd::Key(Key::Char('c')))
                | (Op::Yank, Cmd::Key(Key::Char('y')))
                | (Op::Indent, Cmd::Key(Key::Char('>')))
                | (Op::Outdent, Cmd::Key(Key::Char('<')))
                | (
                    Op::Case(Case::Lower),
                    Cmd::Key(Key::Char('u')) | Cmd::G(Key::Char('u'))
                )
                | (
                    Op::Case(Case::Upper),
                    Cmd::Key(Key::Char('U')) | Cmd::G(Key::Char('U'))
                )
                | (
                    Op::Case(Case::Toggle),
                    Cmd::Key(Key::Char('~')) | Cmd::G(Key::Char('~'))
                )
        );
        if doubled {
            let last = at.line.saturating_add(n - 1).min(doc.last_line());
            return self.operate(op, Span::Lines(at.line, last), doc, snap, clip, fx);
        }
        match cmd {
            Cmd::Key(Key::Esc) => return Done::Motion,
            Cmd::Key(Key::Char(c @ ('i' | 'a'))) => {
                self.pending.wait = Some(Wait::Object { inner: c == 'i' });
                return Done::Pending;
            },
            Cmd::Key(Key::Char('g')) => {
                self.pending.wait = Some(Wait::Prefix('g'));
                return Done::Pending;
            },
            _ => {},
        }
        // `cw` on a word changes to the word's end, keeping the blank after
        // it — the end of *this* word even on its last letter, where `ce`
        // would run on to the next.
        if let Cmd::Key(Key::Char(c @ ('w' | 'W'))) = cmd
            && op == Op::Change
            && !doc.ch(at).is_whitespace()
            && doc.ch(at) != '\n'
        {
            let big = c == 'W';
            let run = text::class(doc.ch(at), big);
            let mut end = at;
            while let Some(next) = doc
                .next(end)
                .filter(|p| p.line == at.line && text::class(doc.ch(*p), big) == run)
            {
                end = next;
            }
            let end = if n > 1 {
                motion::repeat(doc, end, n - 1, |d, p| motion::word_end_forward(d, p, big))
            } else {
                end
            };
            return self.take_target(Target::new(end, Kind::Inclusive), doc, snap, clip, fx);
        }
        match self.vim_motion(cmd, doc, snap, true) {
            Step::Target(target) => self.take_target(target, doc, snap, clip, fx),
            Step::Wait(wait) => {
                self.pending.wait = Some(wait);
                Done::Pending
            },
            Step::Fail | Step::NotMotion => Done::Motion,
        }
    }

    /// Enter Insert mode at `p`.
    pub(super) fn insert_at(&mut self, p: LineCol) -> Done {
        self.sel = Sel::at(p);
        self.mode = Mode::Insert;
        Done::Insert
    }

    /// `o`: a new line below `line`, indented by the language's rules, in
    /// Insert mode.
    pub(super) fn open_below(&mut self, line: u32, doc: &Doc, fx: &mut Vec<Effect>) -> Done {
        self.sel = Sel::at(doc.line_end(line));
        self.mode = Mode::Insert;
        fx.push(Effect::Command(Command::InsertNewline));
        Done::Insert
    }

    /// `O`: a new line above `line`, with its indentation, in Insert mode.
    pub(super) fn open_above(&mut self, line: u32, doc: &Doc, fx: &mut Vec<Effect>) -> Done {
        let indent = doc.indent_of(line);
        let cursor = LineCol::new(line, u32::try_from(indent.chars().count()).unwrap_or(0));
        let at = LineCol::new(line, 0);
        self.apply(
            ops::Planned {
                edits: vec![karet_core::TextEdit {
                    range: Range { start: at, end: at },
                    new_text: format!("{indent}\n"),
                }],
                cursor,
            },
            EditCause::Newline,
            fx,
        );
        self.mode = Mode::Insert;
        Done::Insert
    }

    /// Scroll the viewport, leaving the cursor.
    pub(super) fn scroll(&mut self, scroll: Scroll, fx: &mut Vec<Effect>) -> Done {
        fx.push(Effect::Scroll(scroll));
        Done::Motion
    }

    /// `r<c>`: replace `count` characters from the cursor.
    fn replace_chars(&mut self, c: char, doc: &Doc, fx: &mut Vec<Effect>) -> Done {
        let n = self.pending.total().unwrap_or(1);
        let at = self.sel.cursor;
        let end = at.col.saturating_add(n);
        if end > doc.len(at.line) {
            return Done::Motion;
        }
        let range = Range {
            start: at,
            end: LineCol::new(at.line, end),
        };
        let planned = if c == '\n' {
            ops::Planned {
                edits: vec![karet_core::TextEdit {
                    range,
                    new_text: "\n".to_string(),
                }],
                cursor: LineCol::new(at.line + 1, 0),
            }
        } else {
            let Some(edit) = ops::replace_chars(doc, range, c) else {
                return Done::Motion;
            };
            ops::Planned {
                edits: vec![edit],
                cursor: LineCol::new(at.line, end - 1),
            }
        };
        self.apply(planned, EditCause::Replace, fx);
        Done::Change
    }
}

/// The lines a span touches.
pub(super) fn span_lines(span: Span) -> (u32, u32) {
    match span {
        Span::Lines(first, last) => (first, last),
        Span::Chars(range) => {
            let last = if range.end.col == 0 && range.end.line > range.start.line {
                range.end.line - 1
            } else {
                range.end.line
            };
            (range.start.line, last)
        },
    }
}
