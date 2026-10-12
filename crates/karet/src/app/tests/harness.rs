//! A scripted caret and selection conformance harness for the editor.
//!
//! A [`Rig`] is a whole [`App`] with one code tab, rendered into a test terminal
//! after every step exactly as the event loop renders between events, so mouse
//! input is hit-tested against the pane frames a real frame recorded rather than
//! against hand-built rectangles. Steps are real key presses (through the
//! keymap), commands, and mouse presses, drags, releases and wheel notches with
//! modifiers; [`Step::Wait`] stands in for the time between clicks.
//!
//! State is written as *marked text*, the buffer with its carets drawn in:
//!
//! | mark | meaning |
//! |---|---|
//! | `\|` | a caret (an empty selection) |
//! | `[ab\|]` | a forward selection over `ab`: anchor at `[`, head at `]` |
//! | `[\|ab]` | a backward selection over `ab`: head at `[`, anchor at `]` |
//! | `#` | the primary caret's head, used in place of `\|` when there are several |
//! | `\\x` | a literal `x`, for text that contains a mark character |
//!
//! With one selection the primary is implied, so `#` appears only when there
//! are several. [`Rig::marked`] writes the state back the same way, so a case
//! compares one string that holds the text, every anchor and head, and the
//! primary index together.
//!
//! The normative behaviour these cases check is `docs/editing-behavior.md`.

use std::fmt::Write as _;
use std::time::Duration;

use karet_core::CursorState;
use karet_core::Selection;

use super::support::*;
use crate::app::*;

/// The terminal a rig renders into unless a case asks for another size.
const DEFAULT_SIZE: (u16, u16) = (60, 12);

/// Where a mouse event lands.
#[derive(Clone, Copy, Debug)]
pub(crate) enum At {
    /// The cell drawing buffer position `(line, col)` in the focused pane.
    Text(u32, u32),
    /// The cell drawing `(line, col)` in the `n`th pane, in layout order.
    Pane(usize, u32, u32),
    /// The cell of `(line, col)` in the focused pane, moved by `(dx, dy)` cells:
    /// past the end of a line, or above and below the viewport.
    Offset(u32, u32, i16, i16),
    /// An absolute screen cell.
    Cell(u16, u16),
}

/// One scripted input.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Step {
    /// A key press, routed through the keymap like a real one.
    Key(KeyCode, KeyModifiers),
    /// A command dispatched directly, for verbs without a default binding.
    Cmd(Command),
    /// A left-button press.
    Down(At, KeyModifiers),
    /// A left-button drag to a cell.
    Drag(At, KeyModifiers),
    /// A left-button release.
    Up(At, KeyModifiers),
    /// A full left click: press then release.
    Click(At, KeyModifiers),
    /// A full right click: press then release.
    RightClick(At, KeyModifiers),
    /// A full middle click: press then release.
    MiddleClick(At, KeyModifiers),
    /// One wheel notch down (toward the end of the document).
    WheelDown(At, KeyModifiers),
    /// One wheel notch up.
    WheelUp(At, KeyModifiers),
    /// Let time pass, so the next click does not continue a multi-click streak.
    Wait(Duration),
    /// One drag-autoscroll tick, as the event loop's timer fires it.
    Tick,
}

/// A plain left click at `at`.
pub(crate) fn click(at: At) -> Step {
    Step::Click(at, KeyModifiers::NONE)
}

/// A key press with no modifiers.
pub(crate) fn key(code: KeyCode) -> Step {
    Step::Key(code, KeyModifiers::NONE)
}

/// Shorthand for [`At::Text`].
pub(crate) fn text(line: u32, col: u32) -> At {
    At::Text(line, col)
}

/// The pause that separates two clicks into two single clicks.
pub(crate) const PAUSE: Step = Step::Wait(Duration::from_millis(1000));

/// One table row: a starting state, the input, and the state it must end in.
pub(crate) struct Case {
    /// What the row checks, printed when it fails.
    pub(crate) name: &'static str,
    /// The marked text the rig starts from.
    pub(crate) start: &'static str,
    /// The input, in order.
    pub(crate) steps: Vec<Step>,
    /// The marked text the focused pane must end in.
    pub(crate) expect: &'static str,
    /// The first visible line the focused pane must end on, when the row checks
    /// scrolling.
    pub(crate) scroll: Option<u32>,
    /// The terminal size, when the row needs a particular viewport.
    pub(crate) size: (u16, u16),
}

/// A table row that checks the end state only, in the default terminal.
pub(crate) fn case(
    name: &'static str,
    start: &'static str,
    steps: Vec<Step>,
    expect: &'static str,
) -> Case {
    Case {
        name,
        start,
        steps,
        expect,
        scroll: None,
        size: DEFAULT_SIZE,
    }
}

impl Case {
    /// Also require the focused pane to end scrolled to `line`.
    pub(crate) fn scrolled(mut self, line: u32) -> Self {
        self.scroll = Some(line);
        self
    }

    /// Run in a `width` x `height` terminal.
    pub(crate) fn sized(mut self, width: u16, height: u16) -> Self {
        self.size = (width, height);
        self
    }
}

/// Run every row and fail once with a report of every row that did not hold,
/// so one regression does not hide the others behind it.
pub(crate) fn check(cases: Vec<Case>) {
    let mut failures = String::new();
    for case in cases {
        let mut rig = Rig::sized(case.start, case.size.0, case.size.1);
        rig.run(&case.steps);
        let got = rig.marked();
        if got != case.expect {
            let _ = writeln!(
                failures,
                "{}\n  start:  {:?}\n  expect: {:?}\n  got:    {:?}",
                case.name, case.start, case.expect, got
            );
        }
        if let Some(want) = case.scroll {
            let scroll = rig.scroll_line();
            if scroll != want {
                let _ = writeln!(
                    failures,
                    "{}\n  expect scroll line {want}, got {scroll}",
                    case.name
                );
            }
        }
    }
    assert!(failures.is_empty(), "conformance cases failed:\n{failures}");
}

/// A running app with one code tab, driven step by step.
pub(crate) struct Rig {
    /// The app under test.
    pub(crate) app: App,
    width: u16,
    height: u16,
    /// Scripted time since the last press, advanced only by [`Step::Wait`].
    since_click: Duration,
}

impl Rig {
    /// A rig over `marked` in the default terminal.
    pub(crate) fn new(marked: &str) -> Self {
        Self::sized(marked, DEFAULT_SIZE.0, DEFAULT_SIZE.1)
    }

    /// A rig over `marked` in a `width` x `height` terminal: the sidebar hidden,
    /// the editor focused, edits applied locally through a recording backend,
    /// and one frame already drawn.
    pub(crate) fn sized(marked: &str, width: u16, height: u16) -> Self {
        let (body, cursors) = parse_marked(marked);
        let mut app = app();
        app.backend = Some(Arc::new(RecordingBackend::new()));
        app.clipboard = crate::clipboard::Clipboard::in_memory();
        app.sidebar_visible = false;
        app.push_tab(text_tab("t.rs", &body));
        app.focus = Focus::Editor;
        let mut rig = Self {
            app,
            width,
            height,
            since_click: Duration::ZERO,
        };
        // A first frame tells the editor its viewport height, so placing the
        // carets reveals them the way it would in a running app instead of
        // against a zero-row viewport.
        rig.render();
        let idx = rig.app.active;
        if let Tab {
            kind: TabKind::Code { doc, buffer, .. },
            editor,
            ..
        } = &mut rig.app.tabs[idx]
        {
            *doc = Some(DocumentId(9));
            editor.set_cursor_state(buffer, cursors);
        }
        rig.render();
        rig
    }

    /// Draw a frame, recording the pane rects the next mouse event hits.
    pub(crate) fn render(&mut self) {
        let _ = frame(&mut self.app, self.width, self.height);
    }

    /// Resize the terminal to `width` x `height` and draw a frame at the new size.
    pub(crate) fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.render();
    }

    /// Apply every step in order.
    pub(crate) fn run(&mut self, steps: &[Step]) {
        for step in steps {
            self.step(*step);
        }
    }

    /// Apply one step, then render as the event loop would.
    pub(crate) fn step(&mut self, step: Step) {
        match step {
            Step::Key(code, modifiers) => self.app.handle_key(KeyEvent::new(code, modifiers)),
            Step::Cmd(command) => self.app.dispatch(command),
            Step::Down(at, modifiers) => {
                self.mouse(MouseEventKind::Down(MouseButton::Left), at, modifiers);
            },
            Step::Drag(at, modifiers) => {
                self.mouse(MouseEventKind::Drag(MouseButton::Left), at, modifiers);
            },
            Step::Up(at, modifiers) => {
                self.mouse(MouseEventKind::Up(MouseButton::Left), at, modifiers);
            },
            Step::Click(at, modifiers) => {
                // The pointer does not move between press and release, even if
                // the press scrolled the text under it.
                let (col, row) = self.cell(at);
                let at = At::Cell(col, row);
                self.mouse(MouseEventKind::Down(MouseButton::Left), at, modifiers);
                self.render();
                self.mouse(MouseEventKind::Up(MouseButton::Left), at, modifiers);
            },
            Step::RightClick(at, modifiers) => self.click_with(MouseButton::Right, at, modifiers),
            Step::MiddleClick(at, modifiers) => {
                self.click_with(MouseButton::Middle, at, modifiers);
            },
            Step::WheelDown(at, modifiers) => self.mouse(MouseEventKind::ScrollDown, at, modifiers),
            Step::WheelUp(at, modifiers) => self.mouse(MouseEventKind::ScrollUp, at, modifiers),
            Step::Wait(gap) => self.since_click += gap,
            Step::Tick => self.app.tick_drag_autoscroll(Instant::now()),
        }
        self.render();
    }

    /// Press and release `button` at `at`, the pointer held still between.
    fn click_with(&mut self, button: MouseButton, at: At, modifiers: KeyModifiers) {
        let (col, row) = self.cell(at);
        let at = At::Cell(col, row);
        self.mouse(MouseEventKind::Down(button), at, modifiers);
        self.render();
        self.mouse(MouseEventKind::Up(button), at, modifiers);
    }

    fn mouse(&mut self, kind: MouseEventKind, at: At, modifiers: KeyModifiers) {
        if matches!(kind, MouseEventKind::Down(_)) {
            // The click streak reads the wall clock. Restate the last click as
            // exactly the scripted time ago, so a streak depends only on the
            // script's waits and never on how fast the machine runs it.
            if let Some((_, col, row)) = self.app.last_click {
                let now = Instant::now();
                let when = now.checked_sub(self.since_click).unwrap_or(now);
                self.app.last_click = Some((when, col, row));
            }
            self.since_click = Duration::ZERO;
        }
        let (column, row) = self.cell(at);
        self.app.handle_mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers,
        });
    }

    /// The screen cell `at` names, as of the last frame.
    pub(crate) fn cell(&self, at: At) -> (u16, u16) {
        match at {
            At::Cell(col, row) => (col, row),
            At::Text(line, col) => self.text_cell(self.focused_frame(), line, col),
            At::Pane(n, line, col) => {
                let frame = self.frames().get(n).copied();
                let Some(frame) = frame else {
                    panic!("no pane {n}");
                };
                self.text_cell(frame, line, col)
            },
            At::Offset(line, col, dx, dy) => {
                let (x, y) = self.text_cell(self.focused_frame(), line, col);
                (offset(x, dx), offset(y, dy))
            },
        }
    }

    /// Every pane's id and editor rect, in layout order.
    fn frames(&self) -> Vec<(PaneId, Rect)> {
        let order = self.app.layout.layout(self.app.main_rect);
        order
            .into_iter()
            .filter_map(|(pane, _)| {
                self.app
                    .pane_frames
                    .iter()
                    .find(|frame| frame.pane == pane)
                    .map(|frame| (pane, frame.editor_rect))
            })
            .collect()
    }

    fn focused_frame(&self) -> (PaneId, Rect) {
        let focused = self.app.focus_pane();
        let found = self.frames().into_iter().find(|(pane, _)| *pane == focused);
        let Some(found) = found else {
            panic!("the focused pane was not drawn");
        };
        found
    }

    fn text_cell(&self, (pane, area): (PaneId, Rect), line: u32, col: u32) -> (u16, u16) {
        let tab = self.pane_tab(pane);
        let TabKind::Code {
            buffer,
            folds,
            folded,
            ..
        } = &tab.kind
        else {
            panic!("pane {pane:?} is not showing a code tab");
        };
        let folds = resolve_folds(folds, folded);
        let cell = tab
            .editor
            .screen_cell(area, buffer, &folds, LineCol::new(line, col));
        let Some(cell) = cell else {
            panic!("({line}, {col}) is not on screen in pane {pane:?}");
        };
        cell
    }

    /// The active tab of `pane`, whether or not it is focused.
    fn pane_tab(&self, pane: PaneId) -> &Tab {
        if pane == self.app.focus_pane() {
            return &self.app.tabs[self.app.active];
        }
        let Some(stored) = self.app.stored.get(&pane) else {
            panic!("pane {pane:?} holds no tabs");
        };
        &stored.tabs[stored.active]
    }

    /// The focused pane's active tab.
    pub(crate) fn tab(&self) -> &Tab {
        &self.app.tabs[self.app.active]
    }

    /// The focused pane's text, carets, selections and primary, as marked text.
    pub(crate) fn marked(&self) -> String {
        marked_of(self.tab())
    }

    /// The `n`th pane's state as marked text, in layout order.
    pub(crate) fn marked_in(&self, n: usize) -> String {
        let frames = self.frames();
        let Some((pane, _)) = frames.get(n) else {
            panic!("no pane {n}");
        };
        marked_of(self.pane_tab(*pane))
    }

    /// The focused pane's first visible line.
    pub(crate) fn scroll_line(&self) -> u32 {
        self.tab().editor.scroll_line
    }

    /// The `n`th pane's first visible line, in layout order.
    pub(crate) fn scroll_line_in(&self, n: usize) -> u32 {
        let frames = self.frames();
        let Some((pane, _)) = frames.get(n) else {
            panic!("no pane {n}");
        };
        self.pane_tab(*pane).editor.scroll_line
    }

    /// The index, in layout order, of the focused pane.
    pub(crate) fn focused_index(&self) -> Option<usize> {
        let focused = self.app.focus_pane();
        self.frames().iter().position(|(pane, _)| *pane == focused)
    }
}

fn offset(base: u16, delta: i16) -> u16 {
    base.saturating_add_signed(delta)
}

fn marked_of(tab: &Tab) -> String {
    let TabKind::Code { text, .. } = &tab.kind else {
        panic!("not a code tab");
    };
    render_marked(text, tab.editor.cursors())
}

/// Split marked text into the plain text and the cursor set it draws.
pub(crate) fn parse_marked(marked: &str) -> (String, CursorState) {
    let mut body = String::new();
    let mut selections = Vec::new();
    let mut primary = None;
    // The open bracket's position, and the head if a bar followed it at once.
    let mut open: Option<(LineCol, Option<LineCol>)> = None;
    let mut pos = LineCol::new(0, 0);
    let mut chars = marked.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                let Some(literal) = chars.next() else {
                    panic!("a trailing escape in {marked:?}");
                };
                push_char(&mut body, &mut pos, literal);
            },
            '|' | '#' => {
                if ch == '#' {
                    primary = Some(selections.len());
                }
                match &mut open {
                    // The close checks the head sits at one end or the other.
                    Some((_, head @ None)) => *head = Some(pos),
                    Some(_) => panic!("two heads in one selection in {marked:?}"),
                    None => selections.push(Selection::caret(pos)),
                }
            },
            '[' => {
                assert!(open.is_none(), "nested selection in {marked:?}");
                open = Some((pos, None));
            },
            ']' => {
                let Some((start, head)) = open.take() else {
                    panic!("an unopened selection in {marked:?}");
                };
                let Some(head) = head else {
                    panic!("a selection without a head in {marked:?}");
                };
                let anchor = if head == start { pos } else { start };
                assert!(
                    head == start || head == pos,
                    "a head inside a selection in {marked:?}"
                );
                selections.push(Selection { anchor, head });
            },
            other => push_char(&mut body, &mut pos, other),
        }
    }
    assert!(open.is_none(), "an unclosed selection in {marked:?}");
    if selections.is_empty() {
        selections.push(Selection::caret(LineCol::new(0, 0)));
    }
    let primary = primary.unwrap_or(0);
    (
        body,
        CursorState {
            selections,
            primary,
        },
    )
}

fn push_char(body: &mut String, pos: &mut LineCol, ch: char) {
    body.push(ch);
    if ch == '\n' {
        *pos = LineCol::new(pos.line + 1, 0);
    } else {
        pos.col += 1;
    }
}

/// Draw `cursors` into `text` with the marks [`parse_marked`] reads.
pub(crate) fn render_marked(text: &str, cursors: &CursorState) -> String {
    let several = cursors.selections.len() > 1;
    // (position, rank, mark): at one position a selection closes before a caret
    // sits, and both before the next selection opens.
    let mut marks: Vec<(LineCol, u8, String)> = Vec::new();
    for (idx, sel) in cursors.selections.iter().enumerate() {
        let bar = if several && idx == cursors.primary {
            "#"
        } else {
            "|"
        };
        let range = sel.range();
        if sel.is_empty() {
            marks.push((sel.head, 1, bar.to_string()));
        } else if sel.head == range.end {
            marks.push((range.start, 2, "[".to_string()));
            marks.push((range.end, 0, format!("{bar}]")));
        } else {
            marks.push((range.start, 2, format!("[{bar}")));
            marks.push((range.end, 0, "]".to_string()));
        }
    }
    marks.sort_by_key(|(pos, rank, _)| (*pos, *rank));
    let mut out = String::new();
    let mut pending = marks.into_iter().peekable();
    let mut pos = LineCol::new(0, 0);
    let mut emit = |out: &mut String, pos: LineCol| {
        while let Some((_, _, mark)) = pending.next_if(|(at, ..)| *at <= pos) {
            out.push_str(&mark);
        }
    };
    for ch in text.chars() {
        emit(&mut out, pos);
        if matches!(ch, '|' | '#' | '[' | ']' | '\\') {
            out.push('\\');
        }
        out.push(ch);
        if ch == '\n' {
            pos = LineCol::new(pos.line + 1, 0);
        } else {
            pos.col += 1;
        }
    }
    // Anything left sits at or past the end of the text.
    emit(&mut out, LineCol::new(u32::MAX, u32::MAX));
    out
}

#[test]
fn marked_text_round_trips_every_shape() {
    for marked in [
        "|",
        "ab|c",
        "a[bc|]d",
        "a[|bc]d",
        "|ab\n#cd",
        "[ab|]\n[#cd]\n",
        "a\\|b\\[c\\]\\#\\\\|",
        "line\n|",
    ] {
        let (body, cursors) = parse_marked(marked);
        assert_eq!(render_marked(&body, &cursors), marked, "{marked:?}");
    }
}

#[test]
fn parsing_reads_anchor_head_and_primary() {
    let (body, cursors) = parse_marked("a[|bc]\nd#e");
    assert_eq!(body, "abc\nde");
    assert_eq!(
        cursors.selections,
        vec![
            Selection {
                anchor: LineCol::new(0, 3),
                head: LineCol::new(0, 1),
            },
            Selection::caret(LineCol::new(1, 1)),
        ]
    );
    assert_eq!(cursors.primary, 1);
}

#[test]
fn a_rig_starts_in_the_state_it_was_given() {
    let rig = Rig::new("fn [main|]() {}\n");
    assert_eq!(rig.marked(), "fn [main|]() {}\n");
    assert_eq!(rig.app.focus, Focus::Editor);
    assert_eq!(rig.focused_index(), Some(0));
}

#[test]
fn text_positions_resolve_to_the_cells_they_are_drawn_in() {
    let rig = Rig::new("|alpha\nbeta\n");
    let (x0, y0) = rig.cell(text(0, 0));
    let (x1, y1) = rig.cell(text(1, 3));
    assert_eq!(
        (x1 - x0, y1 - y0),
        (3, 1),
        "one row down, three cells right"
    );
    assert_eq!(rig.cell(At::Offset(0, 0, 2, 1)), (x0 + 2, y0 + 1));
}

#[test]
fn a_wait_ends_a_click_streak() {
    let mut rig = Rig::new("|foo bar baz");
    rig.run(&[click(text(0, 5)), click(text(0, 5))]);
    assert_eq!(
        rig.marked(),
        "foo [bar|] baz",
        "two quick clicks are a double click"
    );

    let mut rig = Rig::new("|foo bar baz");
    rig.run(&[click(text(0, 5)), PAUSE, click(text(0, 5))]);
    assert_eq!(
        rig.marked(),
        "foo b|ar baz",
        "two slow clicks are two single clicks"
    );
}

#[test]
#[should_panic(expected = "a row that cannot hold")]
fn a_failing_row_fails_the_table_by_name() {
    check(vec![case(
        "a row that cannot hold",
        "|ab",
        vec![key(KeyCode::End)],
        "|ab",
    )]);
}
