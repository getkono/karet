//! Vim's motions as the grammar uses them: what a key moves to
//! ([`Engine::vim_motion`]), and what an operator's span over that move is
//! ([`Engine::span_for`]). Shared by Normal mode, operators and Visual mode.

use karet_core::LineCol;
use karet_core::Range;

use super::Clipboard;
use super::Done;
use super::Effect;
use super::Engine;
use super::Key;
use super::Mode;
use super::Scroll;
use super::Search;
use super::Sel;
use super::Snapshot;
use super::Wait;
use super::motion;
use super::object::Span;
use super::text::Doc;

/// How a motion's target bounds an operator's span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// Up to, not including, the target.
    Exclusive,
    /// Up to and including the target.
    Inclusive,
    /// Every line from the cursor's to the target's.
    Linewise,
}

/// What a motion does to the remembered column vertical motions steer to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Goal {
    /// Forget it (a horizontal motion).
    Reset,
    /// Keep it (a vertical motion).
    Keep,
    /// The end of every line (`$`).
    End,
}

/// Where a motion lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Target {
    pub(super) pos: LineCol,
    pub(super) kind: Kind,
    pub(super) goal: Goal,
    /// A `w`/`W` motion, whose operator span stops at the line's end.
    pub(super) word: bool,
    /// Lines to scroll along with the move (`Ctrl+D`, …).
    pub(super) scroll: i32,
}

impl Target {
    pub(super) fn new(pos: LineCol, kind: Kind) -> Self {
        Self {
            pos,
            kind,
            goal: Goal::Reset,
            word: false,
            scroll: 0,
        }
    }

    fn keep_goal(self) -> Self {
        Self {
            goal: Goal::Keep,
            ..self
        }
    }
}

/// A command key, after a `g` or `z` prefix is folded in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Cmd {
    Key(Key),
    G(Key),
    Z(Key),
}

/// What a key is as a motion.
pub(super) enum Step {
    /// A motion to here.
    Target(Target),
    /// The start of a motion that needs another key (`f`, `g`).
    Wait(Wait),
    /// A motion that cannot move (no match): the command is abandoned.
    Fail,
    /// Not a motion.
    NotMotion,
}

impl Engine {
    /// Where `cmd` moves the cursor, as a motion, from `self.sel.cursor`.
    /// `past_end` lets a horizontal motion reach the end of the line (for an
    /// operator's exclusive end).
    pub(super) fn vim_motion(
        &mut self,
        cmd: Cmd,
        doc: &Doc,
        snap: &Snapshot,
        past_end: bool,
    ) -> Step {
        let at = self.sel.cursor;
        let count = self.pending.total();
        let n = count.unwrap_or(1);
        let goal = self.goal.unwrap_or(at.col);
        let lines = |delta: i64| motion::vertical(doc, at, delta, goal, false);
        let half = (snap.height / 2).max(1);
        let page = snap.height.saturating_sub(2).max(1);
        let target = match cmd {
            Cmd::Key(Key::Char('h') | Key::Left) => Target::new(
                LineCol::new(at.line, at.col.saturating_sub(n)),
                Kind::Exclusive,
            ),
            Cmd::Key(Key::Backspace) => Target::new(
                motion::repeat(doc, at, n, |d, p| d.prev(p).unwrap_or(p)),
                Kind::Exclusive,
            ),
            Cmd::Key(Key::Char('l') | Key::Right) => {
                let len = doc.len(at.line);
                let max = if past_end { len } else { len.saturating_sub(1) };
                Target::new(
                    LineCol::new(at.line, at.col.saturating_add(n).min(max)),
                    Kind::Exclusive,
                )
            },
            Cmd::Key(Key::Char(' ')) => Target::new(
                motion::repeat(doc, at, n, |d, p| d.next(p).unwrap_or(p)),
                Kind::Exclusive,
            ),
            Cmd::Key(Key::Char('j') | Key::Down) => {
                Target::new(lines(i64::from(n)), Kind::Linewise).keep_goal()
            },
            Cmd::Key(Key::Char('k') | Key::Up) => {
                Target::new(lines(-i64::from(n)), Kind::Linewise).keep_goal()
            },
            Cmd::Key(Key::Char('+') | Key::Enter) => Target::new(
                doc.first_non_blank(lines(i64::from(n)).line),
                Kind::Linewise,
            ),
            Cmd::Key(Key::Char('-')) => Target::new(
                doc.first_non_blank(lines(-i64::from(n)).line),
                Kind::Linewise,
            ),
            Cmd::Key(Key::Char('_')) => Target::new(
                doc.first_non_blank(lines(i64::from(n) - 1).line),
                Kind::Linewise,
            ),
            Cmd::Key(Key::Char('0') | Key::Home) => {
                Target::new(LineCol::new(at.line, 0), Kind::Exclusive)
            },
            Cmd::Key(Key::Char('^')) => Target::new(doc.first_non_blank(at.line), Kind::Exclusive),
            Cmd::Key(Key::Char('$') | Key::End) => {
                let line = lines(i64::from(n) - 1).line;
                Target {
                    goal: Goal::End,
                    ..Target::new(
                        LineCol::new(line, doc.len(line).saturating_sub(1)),
                        Kind::Inclusive,
                    )
                }
            },
            Cmd::G(Key::Char('_')) => {
                let line = lines(i64::from(n) - 1).line;
                Target::new(doc.last_non_blank(line), Kind::Inclusive)
            },
            Cmd::Key(Key::Char('|')) => Target::new(
                LineCol::new(at.line, (n - 1).min(doc.len(at.line))),
                Kind::Exclusive,
            ),
            Cmd::Key(Key::Char(c @ ('w' | 'W'))) => Target {
                word: true,
                ..Target::new(
                    motion::repeat(doc, at, n, |d, p| {
                        motion::word_start_forward(d, p, c == 'W')
                    }),
                    Kind::Exclusive,
                )
            },
            Cmd::Key(Key::Char(c @ ('b' | 'B'))) => Target::new(
                motion::repeat(doc, at, n, |d, p| {
                    motion::word_start_backward(d, p, c == 'B')
                }),
                Kind::Exclusive,
            ),
            Cmd::Key(Key::Char(c @ ('e' | 'E'))) => Target::new(
                motion::repeat(doc, at, n, |d, p| motion::word_end_forward(d, p, c == 'E')),
                Kind::Inclusive,
            ),
            Cmd::G(Key::Char(c @ ('e' | 'E'))) => Target::new(
                motion::repeat(doc, at, n, |d, p| motion::word_end_backward(d, p, c == 'E')),
                Kind::Inclusive,
            ),
            Cmd::Key(Key::Char(c @ ('f' | 't' | 'F' | 'T'))) => {
                return Step::Wait(Wait::Find {
                    forward: c.is_ascii_lowercase(),
                    till: c.eq_ignore_ascii_case(&'t'),
                });
            },
            Cmd::Key(Key::Char(c @ (';' | ','))) => {
                let Some(search) = self.last_find else {
                    return Step::Fail;
                };
                let search = if c == ',' { search.reversed() } else { search };
                let kind = if search.forward {
                    Kind::Inclusive
                } else {
                    Kind::Exclusive
                };
                match motion::find_char(doc, at, search, n, true) {
                    Some(pos) => Target::new(pos, kind),
                    None => return Step::Fail,
                }
            },
            Cmd::Key(Key::Char('G')) => {
                let line = count.map_or(doc.last_line(), |c| (c - 1).min(doc.last_line()));
                Target::new(doc.first_non_blank(line), Kind::Linewise)
            },
            Cmd::G(Key::Char('g')) => {
                let line = count.map_or(0, |c| (c - 1).min(doc.last_line()));
                Target::new(doc.first_non_blank(line), Kind::Linewise)
            },
            Cmd::Key(Key::Char('%')) if count.is_some() => {
                let total = u64::from(doc.last_line()) + 1;
                let line = (u64::from(n.min(100)) * total)
                    .div_ceil(100)
                    .saturating_sub(1);
                Target::new(
                    doc.first_non_blank(u32::try_from(line).unwrap_or_default()),
                    Kind::Linewise,
                )
            },
            Cmd::Key(Key::Char('%')) => match motion::matching_bracket(doc, at) {
                Some(pos) => Target::new(pos, Kind::Inclusive),
                None => return Step::Fail,
            },
            Cmd::Key(Key::Char('}')) => Target::new(
                motion::repeat(doc, at, n, motion::paragraph_forward),
                Kind::Exclusive,
            ),
            Cmd::Key(Key::Char('{')) => Target::new(
                motion::repeat(doc, at, n, motion::paragraph_backward),
                Kind::Exclusive,
            ),
            Cmd::Key(Key::Char('H')) => Target::new(
                doc.first_non_blank(snap.top.saturating_add(n - 1).min(doc.last_line())),
                Kind::Linewise,
            ),
            Cmd::Key(Key::Char('L')) => {
                let bottom = snap
                    .top
                    .saturating_add(snap.height.saturating_sub(1))
                    .min(doc.last_line());
                Target::new(
                    doc.first_non_blank(bottom.saturating_sub(n - 1).max(snap.top)),
                    Kind::Linewise,
                )
            },
            Cmd::Key(Key::Char('M')) => {
                let shown = snap
                    .height
                    .min(doc.last_line().saturating_sub(snap.top) + 1);
                Target::new(
                    doc.first_non_blank(snap.top + shown.saturating_sub(1) / 2),
                    Kind::Linewise,
                )
            },
            Cmd::Key(Key::Char(c @ ('n' | 'N'))) => {
                let Some(search) = self.search.clone() else {
                    return Step::Fail;
                };
                let forward = search.forward == (c == 'n');
                match motion::repeat_opt(doc, at, n, |d, p| {
                    motion::search(d, p, &search.text, forward, search.word)
                }) {
                    Some(pos) => Target::new(pos, Kind::Exclusive),
                    None => return Step::Fail,
                }
            },
            Cmd::Key(Key::Char(c @ ('*' | '#'))) => {
                let Some((start, word)) = motion::word_under(doc, at) else {
                    return Step::Fail;
                };
                let forward = c == '*';
                self.search = Some(Search {
                    text: word.clone(),
                    forward,
                    word: true,
                });
                let from = if forward { at } else { start };
                match motion::repeat_opt(doc, from, n, |d, p| {
                    motion::search(d, p, &word, forward, true)
                }) {
                    Some(pos) => Target::new(pos, Kind::Exclusive),
                    None => return Step::Fail,
                }
            },
            Cmd::Key(Key::Ctrl('d')) => Target {
                scroll: i32::try_from(half).unwrap_or(i32::MAX),
                ..Target::new(lines(i64::from(half)), Kind::Linewise).keep_goal()
            },
            Cmd::Key(Key::Ctrl('u')) => Target {
                scroll: -i32::try_from(half).unwrap_or(i32::MAX),
                ..Target::new(lines(-i64::from(half)), Kind::Linewise).keep_goal()
            },
            Cmd::Key(Key::PageDown) => Target {
                scroll: i32::try_from(page).unwrap_or(i32::MAX),
                ..Target::new(lines(i64::from(page)), Kind::Linewise).keep_goal()
            },
            Cmd::Key(Key::PageUp) => Target {
                scroll: -i32::try_from(page).unwrap_or(i32::MAX),
                ..Target::new(lines(-i64::from(page)), Kind::Linewise).keep_goal()
            },
            _ => return Step::NotMotion,
        };
        Step::Target(target)
    }

    /// Use a motion's target: move to it, or (with an operator pending)
    /// operate over the span it bounds.
    pub(super) fn take_target(
        &mut self,
        target: Target,
        doc: &Doc,
        snap: &Snapshot,
        clip: &mut dyn Clipboard,
        fx: &mut Vec<Effect>,
    ) -> Done {
        if matches!(self.mode, Mode::Visual | Mode::VisualLine) {
            self.move_cursor(target, doc, fx);
            return Done::Motion;
        }
        match self.pending.op {
            Some(op) => match self.span_for(doc, self.sel.cursor, target) {
                Some(span) => self.operate(op, span, doc, snap, clip, fx),
                None => Done::Motion,
            },
            None => {
                self.move_cursor(target, doc, fx);
                Done::Motion
            },
        }
    }

    /// Move the cursor to `target` (Visual mode keeps the anchor).
    pub(super) fn move_cursor(&mut self, target: Target, doc: &Doc, fx: &mut Vec<Effect>) {
        let pos = doc.clamp_char(target.pos);
        self.goal = match target.goal {
            Goal::Reset => None,
            Goal::Keep => Some(self.goal.unwrap_or(self.sel.cursor.col)),
            Goal::End => Some(u32::MAX),
        };
        if target.scroll != 0 {
            fx.push(Effect::Scroll(Scroll::Lines(target.scroll)));
        }
        if matches!(self.mode, Mode::Visual | Mode::VisualLine) {
            self.sel.cursor = pos;
        } else {
            self.sel = Sel::at(pos);
        }
    }

    /// The span an operator covers from `from` to a motion's `target`, with
    /// vim's adjustments: `dw` stops at the line's end, and an exclusive
    /// motion ending at a line's start ends at the previous line's end
    /// (becoming linewise when it began at the first non-blank).
    pub(super) fn span_for(&self, doc: &Doc, from: LineCol, target: Target) -> Option<Span> {
        let (start, end) = (from.min(target.pos), from.max(target.pos));
        match target.kind {
            Kind::Linewise => Some(Span::Lines(start.line, end.line)),
            Kind::Inclusive => {
                let end = if end.col >= doc.len(end.line) {
                    doc.line_end(end.line)
                } else {
                    doc.after(end)
                };
                (end > start).then_some(Span::Chars(Range { start, end }))
            },
            Kind::Exclusive => {
                let mut end = end;
                if target.word && end.line > start.line {
                    end = doc.line_end(start.line);
                } else if end.col == 0 && end.line > start.line && !target.word {
                    end = doc.line_end(end.line - 1);
                    if start.col <= doc.first_non_blank(start.line).col {
                        return Some(Span::Lines(start.line, end.line));
                    }
                }
                (end > start).then_some(Span::Chars(Range { start, end }))
            },
        }
    }
}
