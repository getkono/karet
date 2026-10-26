//! The modal grammars, driven key by key over a real buffer. A small editor
//! double applies the engine's effects the way the app does — edits, the
//! selection, `o`'s newline, Insert-mode typing, replays — so each case reads
//! as keys in, text and cursor out.

mod helix;
mod objects;
mod registers;
mod repeat;
mod vim;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use karet_core::Change;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::Selection;
use karet_core::TextEdit;
use karet_session::config::schema::KeymapPreset;
use karet_text::TextBuffer;

use super::Clipboard;
use super::Effect;
use super::Engine;
use super::Mode;
use super::Outcome;
use super::Scroll;
use super::Snapshot;
use crate::command::Command;

/// A clipboard held in memory.
#[derive(Default)]
struct Clip(Option<String>);

impl Clipboard for Clip {
    fn get(&mut self) -> Option<String> {
        self.0.clone()
    }

    fn set(&mut self, text: &str) {
        self.0 = Some(text.to_string());
    }
}

/// A buffer, a selection and an engine, wired like the app.
struct Ed {
    preset: KeymapPreset,
    buffer: TextBuffer,
    selection: Selection,
    engine: Engine,
    clip: Clip,
    scrolls: Vec<Scroll>,
    commands: Vec<Command>,
    outcomes: Vec<Outcome>,
}

/// The position of the first `|` in `marked`, and the text without it.
fn parse(marked: &str) -> (String, LineCol) {
    let mut text = String::new();
    let mut at = LineCol::new(0, 0);
    let (mut line, mut col) = (0, 0);
    for c in marked.chars() {
        match c {
            '|' => at = LineCol::new(line, col),
            '\n' => {
                text.push(c);
                line += 1;
                col = 0;
            },
            _ => {
                text.push(c);
                col += 1;
            },
        }
    }
    (text, at)
}

/// The keys a vim-style key string names: plain characters, and `<Esc>`,
/// `<CR>`, `<BS>`, `<Del>`, `<Tab>`, `<Up>`/`<Down>`/`<Left>`/`<Right>`,
/// `<C-x>` and `<A-x>`.
fn keys(spec: &str) -> Vec<KeyEvent> {
    let mut out = Vec::new();
    let mut chars = spec.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '<' || chars.peek().is_none() {
            out.push(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
            continue;
        }
        let mut name = String::new();
        for n in chars.by_ref() {
            if n == '>' {
                break;
            }
            name.push(n);
        }
        let plain = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let event = match name.as_str() {
            "Esc" => plain(KeyCode::Esc),
            "CR" => plain(KeyCode::Enter),
            "BS" => plain(KeyCode::Backspace),
            "Del" => plain(KeyCode::Delete),
            "Tab" => plain(KeyCode::Tab),
            "Up" => plain(KeyCode::Up),
            "Down" => plain(KeyCode::Down),
            "Left" => plain(KeyCode::Left),
            "Right" => plain(KeyCode::Right),
            "lt" => plain(KeyCode::Char('<')),
            other => {
                let (mods, key) = match other.split_once('-') {
                    Some(("C", key)) => (KeyModifiers::CONTROL, key),
                    Some(("A", key)) => (KeyModifiers::ALT, key),
                    _ => (KeyModifiers::NONE, other),
                };
                KeyEvent::new(KeyCode::Char(key.chars().next().unwrap_or(' ')), mods)
            },
        };
        out.push(event);
    }
    out
}

impl Ed {
    fn new(preset: KeymapPreset, marked: &str) -> Self {
        let (text, at) = parse(marked);
        Self {
            preset,
            buffer: TextBuffer::from_text(&text),
            selection: Selection::caret(at),
            engine: Engine::default(),
            clip: Clip::default(),
            scrolls: Vec::new(),
            commands: Vec::new(),
            outcomes: Vec::new(),
        }
    }

    fn vim(marked: &str) -> Self {
        Self::new(KeymapPreset::Vim, marked)
    }

    fn helix(marked: &str) -> Self {
        Self::new(KeymapPreset::Helix, marked)
    }

    /// Type `spec`, recording each key for a macro as the app does.
    fn type_keys(&mut self, spec: &str) -> &mut Self {
        for key in keys(spec) {
            self.engine.record(key);
            self.key(key);
        }
        self
    }

    fn key(&mut self, key: KeyEvent) {
        if self.engine.mode() == Mode::Insert {
            self.engine.observe_insert(key);
            self.insert_key(key);
            return;
        }
        let snap = Snapshot {
            buffer: &self.buffer,
            selection: self.selection,
            selections: 1,
            indent: "    ",
            top: 0,
            height: 20,
        };
        let (outcome, effects) = self.engine.key(self.preset, key, &snap, &mut self.clip);
        self.outcomes.push(outcome);
        self.apply(effects);
    }

    fn insert_key(&mut self, key: KeyEvent) {
        let head = self.selection.head;
        match key.code {
            KeyCode::Esc => {
                let snap = Snapshot {
                    buffer: &self.buffer,
                    selection: self.selection,
                    selections: 1,
                    indent: "    ",
                    top: 0,
                    height: 20,
                };
                let effects = self.engine.enter_normal(self.preset, &snap);
                self.apply(effects);
            },
            KeyCode::Char(c) => self.insert_text(head, &c.to_string()),
            KeyCode::Enter => self.insert_text(head, "\n"),
            KeyCode::Backspace if head.col > 0 => {
                let start = LineCol::new(head.line, head.col - 1);
                self.edit(vec![TextEdit {
                    range: Range { start, end: head },
                    new_text: String::new(),
                }]);
                self.selection = Selection::caret(start);
            },
            _ => {},
        }
    }

    fn insert_text(&mut self, at: LineCol, text: &str) {
        self.edit(vec![TextEdit {
            range: Range { start: at, end: at },
            new_text: text.to_string(),
        }]);
        self.selection = Selection::caret(karet_editor::editing::caret_after_insert(at, text));
    }

    fn edit(&mut self, edits: Vec<TextEdit>) {
        let change = Change::new(self.buffer.version(), edits);
        assert!(self.buffer.apply_simple(&change).is_ok(), "edit applies");
    }

    fn apply(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Edit {
                    edits,
                    anchor,
                    head,
                    ..
                } => {
                    self.edit(edits);
                    self.selection = Selection { anchor, head };
                },
                Effect::Select { anchor, head } => self.selection = Selection { anchor, head },
                Effect::Scroll(scroll) => self.scrolls.push(scroll),
                Effect::Command(Command::InsertNewline) => {
                    let head = self.selection.head;
                    let indent: String = self
                        .buffer
                        .line(head.line as usize)
                        .unwrap_or_default()
                        .chars()
                        .take_while(|c| c.is_whitespace())
                        .collect();
                    self.insert_text(head, &format!("\n{indent}"));
                },
                Effect::Command(command) => self.commands.push(command),
                Effect::Replay { keys, dot } => {
                    self.engine.set_replaying_dot(dot);
                    for key in keys {
                        self.key(key);
                    }
                    self.engine.set_replaying_dot(false);
                },
            }
        }
    }

    /// The text with `|` at the caret.
    fn marked(&self) -> String {
        let mut out = String::new();
        let text = self.buffer.text();
        let head = self.selection.head;
        let (mut line, mut col) = (0, 0);
        for c in text.chars() {
            if LineCol::new(line, col) == head {
                out.push('|');
            }
            out.push(c);
            if c == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        if LineCol::new(line, col) == head {
            out.push('|');
        }
        out
    }

    /// The editor selection's exclusive range, as the app holds it.
    fn selected(&self) -> (LineCol, LineCol) {
        let Selection { anchor, head } = self.selection;
        (anchor.min(head), anchor.max(head))
    }

    fn mode(&self) -> Mode {
        self.engine.mode()
    }

    fn clipboard(&self) -> Option<&str> {
        self.clip.0.as_deref()
    }
}

/// Run `cases` of `(start, keys, expected)` under `preset`, reporting every
/// mismatch at once.
fn check(preset: KeymapPreset, cases: &[(&str, &str, &str)]) {
    let mut failures = String::new();
    for (start, typed, expected) in cases {
        let mut ed = Ed::new(preset, start);
        ed.type_keys(typed);
        let got = ed.marked();
        if got != *expected {
            failures.push_str(&format!(
                "\n  {start:?} + {typed:?}\n    expected {expected:?}\n    got      {got:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{failures}");
}
