//! The helix grammar: selection first, then action. A motion selects (or, in
//! Select mode, extends), and `d`, `c`, `y`, `r`, `~`, `>`… act on whatever
//! is selected. Shares motions, text objects and registers with vim.

use karet_core::EditCause;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::TextEdit;
use karet_editor::editing::map_position;

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
use super::Write;
use super::motion;
use super::motion::CharSearch;
use super::object;
use super::object::Object;
use super::object::Span;
use super::ops;
use super::ops::Case;
use super::register;
use super::register::Register;
use super::text::Class;
use super::text::Doc;
use super::text::class;
use super::text::end_of_insert;

impl Engine {
    /// Handle a key in helix's Normal or Select mode.
    pub(super) fn helix_key(
        &mut self,
        key: Key,
        doc: &Doc,
        snap: &Snapshot,
        clip: &mut dyn Clipboard,
        fx: &mut Vec<Effect>,
    ) -> Done {
        if let Some(wait) = self.pending.wait.take() {
            return self.helix_wait(wait, key, doc, snap, clip, fx);
        }
        if let Some(done) = self.count_or_register(key) {
            return done;
        }
        let n = self.pending.total().unwrap_or(1);
        let at = self.sel.cursor;
        let goal = self.goal.unwrap_or(at.col);
        let half = (snap.height / 2).max(1);
        let page = snap.height.saturating_sub(2).max(1);
        match key {
            Key::Esc if self.mode == Mode::Select => {
                self.mode = Mode::Normal;
                Done::Motion
            },
            Key::Esc => {
                let typed = self.pending.count.is_some() || self.pending.register.is_some();
                if typed { Done::Motion } else { Done::Unhandled }
            },
            Key::Char('h') | Key::Left => {
                self.helix_goto(LineCol::new(at.line, at.col.saturating_sub(n)), false)
            },
            Key::Char('l') | Key::Right => {
                let col = at
                    .col
                    .saturating_add(n)
                    .min(doc.len(at.line).saturating_sub(1));
                self.helix_goto(LineCol::new(at.line, col), false)
            },
            Key::Char('j') | Key::Down => self.helix_vertical(doc, i64::from(n), goal),
            Key::Char('k') | Key::Up => self.helix_vertical(doc, -i64::from(n), goal),
            Key::Ctrl('d') => self.helix_page(doc, i64::from(half), goal, fx),
            Key::Ctrl('u') => self.helix_page(doc, -i64::from(half), goal, fx),
            Key::PageDown => self.helix_page(doc, i64::from(page), goal, fx),
            Key::PageUp => self.helix_page(doc, -i64::from(page), goal, fx),
            Key::Home => self.helix_goto(LineCol::new(at.line, 0), false),
            Key::End => self.helix_goto(doc.clamp_char(doc.line_end(at.line)), false),
            Key::Char(c @ ('w' | 'W' | 'e' | 'E' | 'b' | 'B')) => self.helix_word(c, doc, n),
            Key::Char(c @ ('f' | 't' | 'F' | 'T')) => {
                self.pending.wait = Some(Wait::Find {
                    forward: c.is_ascii_lowercase(),
                    till: c.eq_ignore_ascii_case(&'t'),
                });
                Done::Pending
            },
            Key::Char('x') => {
                let (start, end) = (self.sel.start(), self.sel.end());
                let full = start.col == 0 && end.col >= doc.len(end.line);
                let last = if full { end.line + n } else { end.line + n - 1 };
                let last = last.min(doc.last_line());
                self.sel = Sel {
                    anchor: LineCol::new(start.line, 0),
                    cursor: doc.line_end(last),
                };
                Done::Motion
            },
            Key::Char('X') => {
                let (start, end) = (self.sel.start(), self.sel.end());
                self.sel = Sel {
                    anchor: LineCol::new(start.line, 0),
                    cursor: doc.line_end(end.line),
                };
                Done::Motion
            },
            Key::Char('%') => {
                let last = doc.last_line();
                self.sel = Sel {
                    anchor: LineCol::new(0, 0),
                    cursor: doc
                        .prev(doc.line_end(last))
                        .filter(|_| doc.len(last) > 0)
                        .unwrap_or(doc.line_end(last)),
                };
                Done::Motion
            },
            Key::Char(';') => {
                self.sel = Sel::at(self.sel.cursor);
                Done::Motion
            },
            Key::Alt(';') => {
                std::mem::swap(&mut self.sel.anchor, &mut self.sel.cursor);
                Done::Motion
            },
            Key::Char('d') => self.helix_delete(doc, clip, true, fx),
            Key::Alt('d') => self.helix_delete(doc, clip, false, fx),
            Key::Char('c') => self.helix_change(doc, clip, true, fx),
            Key::Alt('c') => self.helix_change(doc, clip, false, fx),
            Key::Char('y') => {
                let reg = helix_register(doc.text(self.helix_range(doc)));
                self.store(reg, Write::Yank, clip);
                Done::Motion
            },
            Key::Char(c @ ('p' | 'P')) => self.helix_paste(c == 'p', doc, clip, n, fx),
            Key::Char('R') => self.helix_replace_with_register(doc, clip, fx),
            Key::Char('i') => self.insert_at(self.sel.start()),
            Key::Char('a') => self.insert_at(doc.after(self.sel.end())),
            Key::Char('I') => self.insert_at(doc.first_non_blank(self.sel.start().line)),
            Key::Char('A') => self.insert_at(doc.line_end(self.sel.end().line)),
            Key::Char('o') => self.open_below(self.sel.end().line, doc, fx),
            Key::Char('O') => self.open_above(self.sel.start().line, doc, fx),
            Key::Char('r') => {
                self.pending.wait = Some(Wait::Replace);
                Done::Pending
            },
            Key::Char('~') => self.helix_case(doc, Case::Toggle, fx),
            Key::Char('`') => self.helix_case(doc, Case::Lower, fx),
            Key::Alt('`') => self.helix_case(doc, Case::Upper, fx),
            Key::Char(c @ ('>' | '<')) => {
                let (first, last) = (self.sel.start().line, self.sel.end().line);
                let planned = ops::shift(doc, first, last, snap.indent, n, c == '<');
                self.helix_edit(planned.edits, EditCause::Replace, false, fx)
            },
            Key::Char('J') => {
                let (first, last) = (self.sel.start().line, self.sel.end().line);
                match ops::join(doc, first, last.max(first + 1), true) {
                    Some(planned) => {
                        self.apply(planned, EditCause::Replace, fx);
                        Done::Change
                    },
                    None => Done::Motion,
                }
            },
            Key::Char('v') => {
                self.mode = if self.mode == Mode::Select {
                    Mode::Normal
                } else {
                    Mode::Select
                };
                Done::Motion
            },
            Key::Char(c @ ('g' | 'm' | 'z')) => {
                self.pending.wait = Some(Wait::Prefix(c));
                Done::Pending
            },
            Key::Char('G') => {
                let line = self
                    .pending
                    .total()
                    .map_or(doc.last_line(), |c| (c - 1).min(doc.last_line()));
                self.helix_goto(LineCol::new(line, 0), true)
            },
            Key::Char('Q') => {
                let name = self.pending.register.unwrap_or('@');
                self.toggle_recording(Some(name));
                Done::Motion
            },
            Key::Char('q') => {
                let name = self.pending.register.unwrap_or('@');
                self.play(name, n, fx)
            },
            Key::Char('.') => self.repeat_change(n, fx),
            Key::Char(c @ ('n' | 'N')) => {
                let Some(search) = self.search.clone() else {
                    return Done::Motion;
                };
                let forward = search.forward == (c == 'n');
                let from = if forward {
                    self.sel.end()
                } else {
                    self.sel.start()
                };
                let Some(start) = motion::repeat_opt(doc, from, n, |d, p| {
                    motion::search(d, p, &search.text, forward, search.word)
                }) else {
                    return Done::Motion;
                };
                let len = u32::try_from(search.text.chars().count()).unwrap_or(1);
                let end = LineCol::new(start.line, start.col + len.saturating_sub(1));
                if self.mode == Mode::Select {
                    self.sel.cursor = end;
                } else {
                    self.sel = Sel {
                        anchor: start,
                        cursor: end,
                    };
                }
                Done::Motion
            },
            Key::Char('*') => {
                let text = doc.text(self.helix_range(doc));
                if !text.is_empty() && !text.contains('\n') {
                    self.search = Some(Search {
                        text,
                        forward: true,
                        word: false,
                    });
                }
                Done::Motion
            },
            Key::Ctrl(c @ ('a' | 'x')) => {
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
            Key::Tab => Done::Motion,
            _ => Done::Unhandled,
        }
    }

    /// Complete the command waiting on `key`.
    fn helix_wait(
        &mut self,
        wait: Wait,
        key: Key,
        doc: &Doc,
        snap: &Snapshot,
        clip: &mut dyn Clipboard,
        fx: &mut Vec<Effect>,
    ) -> Done {
        let ch = match key {
            Key::Char(c) => Some(c),
            Key::Enter => Some('\n'),
            _ => None,
        };
        let n = self.pending.total().unwrap_or(1);
        let at = self.sel.cursor;
        match wait {
            Wait::Register => match ch {
                Some(c) if register::is_valid(c) || c == '@' => {
                    self.pending.register = Some(c);
                    Done::Pending
                },
                _ => Done::Motion,
            },
            Wait::Prefix('g') => {
                let line = |c: Option<u32>| c.map_or(0, |c| (c - 1).min(doc.last_line()));
                let target = match ch {
                    Some('g') => LineCol::new(line(self.pending.total()), 0),
                    Some('e') => LineCol::new(doc.last_line(), 0),
                    Some('h') => LineCol::new(at.line, 0),
                    Some('l') => doc.clamp_char(doc.line_end(at.line)),
                    Some('s') => doc.first_non_blank(at.line),
                    _ => return Done::Motion,
                };
                let vertical = matches!(ch, Some('g' | 'e'));
                self.helix_goto(target, vertical)
            },
            Wait::Prefix('m') => match ch {
                Some('m') => match motion::matching_bracket(doc, at) {
                    Some(pos) => self.helix_goto(pos, false),
                    None => Done::Motion,
                },
                Some(c @ ('i' | 'a')) => {
                    self.pending.wait = Some(Wait::Object { inner: c == 'i' });
                    Done::Pending
                },
                _ => Done::Motion,
            },
            Wait::Prefix(_) => match ch {
                Some('z' | 'c') => self.scroll(Scroll::Center, fx),
                Some('t') => self.scroll(Scroll::Top, fx),
                Some('b') => self.scroll(Scroll::Bottom, fx),
                _ => Done::Motion,
            },
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
                match motion::find_char(doc, at, search, n, false) {
                    Some(pos) => {
                        if self.mode != Mode::Select {
                            self.sel.anchor = at;
                        }
                        self.sel.cursor = pos;
                        Done::Motion
                    },
                    None => Done::Motion,
                }
            },
            Wait::Replace => match ch {
                Some(c) => {
                    let range = self.helix_range(doc);
                    match ops::replace_chars(doc, range, c) {
                        Some(edit) => self.helix_edit(vec![edit], EditCause::Replace, true, fx),
                        None => Done::Motion,
                    }
                },
                None => Done::Motion,
            },
            Wait::Object { inner } => {
                let Some(object) = ch.and_then(Object::from_char) else {
                    return Done::Motion;
                };
                match object::select(doc, at, object, inner, n) {
                    Some(Span::Chars(range)) => {
                        self.sel = Sel {
                            anchor: range.start,
                            cursor: doc
                                .prev(range.end)
                                .filter(|p| *p >= range.start)
                                .unwrap_or(range.start),
                        };
                    },
                    Some(Span::Lines(first, last)) => {
                        self.sel = Sel {
                            anchor: LineCol::new(first, 0),
                            cursor: doc.line_end(last),
                        };
                    },
                    None => {},
                }
                Done::Motion
            },
            Wait::Record | Wait::Play => {
                let _ = (snap, clip);
                Done::Motion
            },
        }
    }

    /// Move the cursor to `pos`, extending in Select mode. A vertical move
    /// keeps the column vertical motions steer to.
    fn helix_goto(&mut self, pos: LineCol, vertical: bool) -> Done {
        if !vertical {
            self.goal = None;
        }
        if self.mode == Mode::Select {
            self.sel.cursor = pos;
        } else {
            self.sel = Sel::at(pos);
        }
        Done::Motion
    }

    fn helix_vertical(&mut self, doc: &Doc, delta: i64, goal: u32) -> Done {
        self.goal = Some(goal);
        let pos = motion::vertical(doc, self.sel.cursor, delta, goal, false);
        self.helix_goto(pos, true)
    }

    fn helix_page(&mut self, doc: &Doc, delta: i64, goal: u32, fx: &mut Vec<Effect>) -> Done {
        fx.push(Effect::Scroll(Scroll::Lines(
            i32::try_from(delta).unwrap_or(0),
        )));
        self.helix_vertical(doc, delta, goal)
    }

    /// `w`, `e`, `b` (and the `WORD` forms): select from the cursor to the
    /// next word start, the word end, or the previous word start.
    fn helix_word(&mut self, c: char, doc: &Doc, n: u32) -> Done {
        let big = c.is_ascii_uppercase();
        let anchor = self.sel.anchor;
        let mut sel = self.sel;
        for _ in 0..n.max(1) {
            let at = sel.cursor;
            let forward = !c.eq_ignore_ascii_case(&'b');
            // At a word boundary the new selection starts past it, so
            // pressing `w` again selects the next word, not the last letter.
            let neighbour = if forward { doc.next(at) } else { doc.prev(at) };
            let start = match neighbour {
                Some(p) if class(doc.ch(p), big) != class(doc.ch(at), big) => p,
                _ => at,
            };
            let end = if c.eq_ignore_ascii_case(&'w') {
                let next = motion::word_start_forward(doc, start, big);
                if next == start {
                    start
                } else {
                    doc.prev(next).filter(|p| *p >= start).unwrap_or(start)
                }
            } else {
                // `e` runs to the end of the word at (or after the blanks
                // after) `start`; `b` to the start of the one at (or before).
                let step = |p: LineCol| if forward { doc.next(p) } else { doc.prev(p) };
                let mut it = start;
                while matches!(class(doc.ch(it), big), Class::Blank | Class::Eol) {
                    match step(it) {
                        Some(p) => it = p,
                        None => break,
                    }
                }
                let run = class(doc.ch(it), big);
                while let Some(p) = step(it) {
                    if class(doc.ch(p), big) != run {
                        break;
                    }
                    it = p;
                }
                it
            };
            if end == sel.cursor && start == sel.cursor && n > 1 {
                break;
            }
            sel = Sel {
                anchor: start,
                cursor: end,
            };
        }
        if self.mode == Mode::Select {
            sel.anchor = anchor;
        }
        self.goal = None;
        self.sel = sel;
        Done::Motion
    }

    /// The selection as an exclusive range.
    fn helix_range(&self, doc: &Doc) -> Range {
        Range {
            start: self.sel.start(),
            end: doc.after(self.sel.end()),
        }
    }

    fn helix_delete(
        &mut self,
        doc: &Doc,
        clip: &mut dyn Clipboard,
        yank: bool,
        fx: &mut Vec<Effect>,
    ) -> Done {
        let range = self.helix_range(doc);
        let text = doc.text(range);
        if text.is_empty() {
            return Done::Motion;
        }
        if yank {
            self.store(helix_register(text), Write::Delete, clip);
        }
        let planned = ops::delete(doc, Span::Chars(range));
        self.mode = Mode::Normal;
        let cursor = doc.clamp(range.start);
        self.apply(
            ops::Planned {
                cursor: if planned.cursor.line == cursor.line {
                    planned.cursor
                } else {
                    cursor
                },
                ..planned
            },
            EditCause::Delete,
            fx,
        );
        Done::Change
    }

    fn helix_change(
        &mut self,
        doc: &Doc,
        clip: &mut dyn Clipboard,
        yank: bool,
        fx: &mut Vec<Effect>,
    ) -> Done {
        let range = self.helix_range(doc);
        let text = doc.text(range);
        if yank && !text.is_empty() {
            self.store(helix_register(text), Write::Delete, clip);
        }
        self.apply(ops::change(doc, Span::Chars(range)), EditCause::Delete, fx);
        self.mode = Mode::Insert;
        Done::Insert
    }

    /// `p`/`P`: paste after (before) the selection — on the next (this)
    /// line when the register holds whole lines — and select what was pasted.
    fn helix_paste(
        &mut self,
        after: bool,
        doc: &Doc,
        clip: &mut dyn Clipboard,
        n: u32,
        fx: &mut Vec<Effect>,
    ) -> Done {
        let Some(reg) = self.fetch(clip) else {
            return Done::Motion;
        };
        let text = reg.text.repeat(n.clamp(1, 10_000) as usize);
        if text.is_empty() {
            return Done::Motion;
        }
        let (at, text) = if reg.linewise {
            let text = if text.ends_with('\n') {
                text
            } else {
                format!("{text}\n")
            };
            if after {
                let line = self.sel.end().line;
                if line < doc.last_line() {
                    (LineCol::new(line + 1, 0), text)
                } else {
                    let body = text.strip_suffix('\n').unwrap_or(&text);
                    (doc.line_end(line), format!("\n{body}"))
                }
            } else {
                (LineCol::new(self.sel.start().line, 0), text)
            }
        } else if after {
            (doc.after(self.sel.end()), text)
        } else {
            (self.sel.start(), text)
        };
        let start = if text.starts_with('\n') && reg.linewise {
            LineCol::new(at.line + 1, 0)
        } else {
            at
        };
        let end = end_of_insert(at, &text);
        let last = if end.col > 0 {
            LineCol::new(end.line, end.col - 1)
        } else {
            LineCol::new(end.line.saturating_sub(1), u32::MAX)
        };
        fx.push(Effect::Edit {
            edits: vec![TextEdit {
                range: Range { start: at, end: at },
                new_text: text.clone(),
            }],
            cause: EditCause::Paste,
            anchor: start,
            head: start,
        });
        let last = if last.col == u32::MAX {
            // A paste ending in a line break selects through that break.
            let line_text = text.lines().last().unwrap_or_default();
            let col = if last.line == at.line { at.col } else { 0 };
            LineCol::new(
                last.line,
                col + u32::try_from(line_text.chars().count()).unwrap_or(0),
            )
        } else {
            last
        };
        self.sel = Sel {
            anchor: start,
            cursor: last.max(start),
        };
        Done::Change
    }

    /// `R`: replace the selection with the register.
    fn helix_replace_with_register(
        &mut self,
        doc: &Doc,
        clip: &mut dyn Clipboard,
        fx: &mut Vec<Effect>,
    ) -> Done {
        let Some(reg) = self.fetch(clip) else {
            return Done::Motion;
        };
        let range = self.helix_range(doc);
        let end = end_of_insert(range.start, &reg.text);
        fx.push(Effect::Edit {
            edits: vec![TextEdit {
                range,
                new_text: reg.text.clone(),
            }],
            cause: EditCause::Paste,
            anchor: range.start,
            head: range.start,
        });
        let cursor = if end.col > 0 {
            LineCol::new(end.line, end.col - 1)
        } else {
            range.start
        };
        self.sel = Sel {
            anchor: range.start,
            cursor: cursor.max(range.start),
        };
        Done::Change
    }

    fn helix_case(&mut self, doc: &Doc, case: Case, fx: &mut Vec<Effect>) -> Done {
        let range = self.helix_range(doc);
        match ops::case(doc, range, case) {
            Some(edit) => self.helix_edit(vec![edit], EditCause::Replace, true, fx),
            None => Done::Motion,
        }
    }

    /// Push `edits`, carrying the selection through them. `in_place` edits
    /// rewrite the selected characters one for one (`r`, `~`), so the
    /// selection stays exactly where it is.
    fn helix_edit(
        &mut self,
        edits: Vec<TextEdit>,
        cause: EditCause,
        in_place: bool,
        fx: &mut Vec<Effect>,
    ) -> Done {
        if edits.is_empty() {
            return Done::Motion;
        }
        if in_place {
            let (anchor, head) = (self.sel.anchor, self.sel.cursor);
            fx.push(Effect::Edit {
                edits,
                cause,
                anchor,
                head,
            });
            return Done::Change;
        }
        let keep_start = self.sel.start().col == 0;
        let map = |p: LineCol| map_position(p, &edits);
        let (anchor, cursor) = (map(self.sel.anchor), map(self.sel.cursor));
        let pin = |p: LineCol, original: LineCol| {
            if keep_start && original == self.sel.start() {
                LineCol::new(p.line, 0)
            } else {
                p
            }
        };
        self.sel = Sel {
            anchor: pin(anchor, self.sel.anchor),
            cursor: pin(cursor, self.sel.cursor),
        };
        fx.push(Effect::Edit {
            edits,
            cause,
            anchor,
            head: cursor,
        });
        Done::Change
    }
}

/// Text as a helix register: whole lines when it ends in a line break.
fn helix_register(text: String) -> Register {
    Register {
        linewise: text.ends_with('\n'),
        text,
    }
}
