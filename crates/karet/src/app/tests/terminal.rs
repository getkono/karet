//! The integrated terminal in the shell: Ctrl+` toggling, several terminals,
//! keys and pastes reaching the shell rather than the app, the pointer, and
//! what the terminals' threads report.

use std::time::Duration;

use super::support::*;
use crate::app::*;
use crate::clipboard::Clipboard;
use crate::terminal::TerminalEvent;
use crate::terminal::TerminalEventKind;
use crate::terminal::TerminalEvents;

/// An app with one code tab and terminals that run `script` under `/bin/sh`
/// instead of the user's shell.
fn terminal_app(script: &str) -> (App, TerminalEvents) {
    let mut app = app();
    app.clipboard = Clipboard::in_memory();
    app.push_tab(code_tab("a.rs"));
    app.terminals.use_program(&["/bin/sh", "-c", script]);
    let events = app.terminals.take_receiver();
    (
        app,
        events.unwrap_or_else(|| tokio::sync::mpsc::unbounded_channel().1),
    )
}

fn active_title(app: &App) -> &str {
    app.tabs
        .get(app.active)
        .map_or("<none>", |tab| tab.title.as_str())
}

fn terminal_count(app: &App) -> usize {
    app.all_tabs().filter(|tab| tab.is_terminal()).count()
}

/// Apply terminal events to `app` until `done` holds, or ten seconds pass.
fn pump(app: &mut App, events: &mut TerminalEvents, done: impl Fn(&App) -> bool) -> bool {
    pump_counting(app, events, |app, _| done(app)).is_some()
}

/// [`pump`], also counting the output announcements applied; `done` sees the
/// count so far. Returns the count once `done` holds, `None` on the bound.
fn pump_counting(
    app: &mut App,
    events: &mut TerminalEvents,
    done: impl Fn(&App, usize) -> bool,
) -> Option<usize> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut outputs = 0;
    loop {
        while let Ok(event) = events.try_recv() {
            outputs += usize::from(event.kind == TerminalEventKind::Output);
            app.on_terminal_event(event, Instant::now());
        }
        if done(app, outputs) {
            return Some(outputs);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The active terminal's history scroll position (lines above the live screen).
fn scrolled(app: &App) -> Option<usize> {
    app.active_terminal()
        .map(|pane| pane.term().grid().display_offset())
}

/// The active terminal's history size.
fn history(app: &App) -> usize {
    use alacritty_terminal::grid::Dimensions;
    app.active_terminal()
        .map_or(0, |pane| pane.term().grid().history_size())
}

/// The active terminal's id.
fn active_id(app: &App) -> Option<crate::terminal::TerminalId> {
    app.active_terminal().map(crate::terminal::TerminalPane::id)
}

/// Select text on the active terminal's first painted row, from `from` to `to`.
fn drag_select(app: &mut App, from: u16, to: u16) {
    let area = app
        .active_terminal()
        .map(|pane| pane.area)
        .unwrap_or_default();
    let mouse = |kind, column| MouseEvent {
        kind,
        column: area.x + column,
        row: area.y,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), from));
    app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), to));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), to));
}

/// Whether any screen row of the active terminal contains `needle`.
fn shows(app: &App, needle: &str) -> bool {
    let Some(pane) = app.active_terminal() else {
        return false;
    };
    let term = pane.term();
    let grid = term.grid();
    use alacritty_terminal::grid::Dimensions;
    (0..term.screen_lines()).any(|row| {
        let line = alacritty_terminal::index::Line(i32::try_from(row).unwrap_or_default());
        let text: String = (0..term.columns())
            .map(|column| grid[line][alacritty_terminal::index::Column(column)].c)
            .collect();
        text.contains(needle)
    })
}

#[test]
fn ctrl_backtick_opens_a_terminal_then_toggles_back_and_forth() {
    let (mut app, _events) = terminal_app("cat");
    send_key(&mut app, KeyCode::Char('`'), KeyModifiers::CONTROL);
    assert_eq!(active_title(&app), "Terminal 1");
    assert_eq!(app.keyboard.focus(), Focus::Editor);
    assert_eq!(app.focus_target(), FocusTarget::Terminal);

    // From the terminal, the same key goes back to the editor…
    send_key(&mut app, KeyCode::Char('`'), KeyModifiers::CONTROL);
    assert_eq!(active_title(&app), "a.rs");
    // …and from the editor it returns to that terminal rather than starting one.
    send_key(&mut app, KeyCode::Char('`'), KeyModifiers::CONTROL);
    assert_eq!(active_title(&app), "Terminal 1");
    assert_eq!(terminal_count(&app), 1);
}

#[test]
fn new_terminal_starts_another_and_toggle_returns_to_the_latest() {
    let (mut app, _events) = terminal_app("cat");
    app.dispatch(Command::NewTerminal);
    send_key(
        &mut app,
        KeyCode::Char('`'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert_eq!(terminal_count(&app), 2);
    assert_eq!(active_title(&app), "Terminal 2");

    app.select_tab(0);
    assert_eq!(active_title(&app), "a.rs");
    app.dispatch(Command::ToggleTerminal);
    assert_eq!(active_title(&app), "Terminal 2", "the most recent terminal");
}

#[test]
fn app_chords_that_belong_to_the_shell_do_not_fire_in_a_terminal() {
    let (mut app, _events) = terminal_app("cat");
    app.dispatch(Command::ToggleTerminal);
    send_key(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
    assert!(
        app.keyboard.overlay().is_none(),
        "Ctrl+P is the shell's, not Quick Open"
    );
    send_key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(
        app.keyboard.focus(),
        Focus::Editor,
        "Tab completes in the shell"
    );
    send_key(&mut app, KeyCode::Char('b'), KeyModifiers::CONTROL);
    assert!(app.sidebar_visible, "Ctrl+B moves the shell's cursor");
    // The terminal layer keeps the escape hatches.
    send_key(
        &mut app,
        KeyCode::Char('p'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert!(
        app.keyboard.overlay().is_some(),
        "Ctrl+Shift+P still opens the palette"
    );
}

#[test]
fn typed_keys_and_pastes_reach_the_shell_in_the_workspace_root() {
    let (mut app, mut events) =
        terminal_app("read typed; read pasted; printf '[%s|%s]' \"$typed\" \"$pasted\"; read w");
    app.dispatch(Command::ToggleTerminal);
    for c in ['h', 'i'] {
        send_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
    }
    send_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    app.handle_paste("pasted text\n".to_owned());
    assert!(
        pump(&mut app, &mut events, |app| shows(app, "[hi|pasted text]")),
        "the shell read both lines"
    );
}

#[test]
fn a_terminal_starts_in_the_workspace_root() {
    let (mut app, mut events) = terminal_app("pwd -P; read w");
    let dir = tempfile::tempdir();
    assert!(dir.is_ok());
    let dir = dir.ok();
    let root = dir
        .as_ref()
        .and_then(|dir| std::fs::canonicalize(dir.path()).ok())
        .unwrap_or_default();
    app.root = root.clone();
    let root = root.to_string_lossy().into_owned();
    assert!(!root.is_empty() && root.len() < 80, "{root}");
    app.dispatch(Command::ToggleTerminal);
    assert!(pump(&mut app, &mut events, |app| shows(app, &root)));
}

#[test]
fn a_program_title_and_exit_retitle_the_tab_and_a_key_closes_it() {
    let (mut app, mut events) = terminal_app("printf '\\033]2;build\\007'; exit 2");
    app.dispatch(Command::ToggleTerminal);
    assert!(pump(&mut app, &mut events, |app| active_title(app)
        == "build (exited)"));
    let painted = screen(&mut app, 100, 20).join("\n");
    assert!(
        painted.contains("[shell exited with code 2 - press any key to close this terminal]"),
        "{painted}"
    );
    send_key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
    assert_eq!(terminal_count(&app), 0);
    assert_eq!(active_title(&app), "a.rs");
}

#[test]
fn osc52_copies_from_a_program_land_on_the_clipboard() {
    let (mut app, _events) = terminal_app("cat");
    app.dispatch(Command::ToggleTerminal);
    let id = app.active_terminal().map(crate::terminal::TerminalPane::id);
    assert!(id.is_some(), "a terminal is open");
    if let Some(id) = id {
        app.on_terminal_event(
            TerminalEvent {
                id,
                kind: TerminalEventKind::Clipboard("from vim".to_owned()),
            },
            Instant::now(),
        );
    }
    assert_eq!(app.clipboard.get().unwrap_or_default(), "from vim");
}

#[test]
fn dragging_selects_and_ctrl_c_copies_instead_of_interrupting() {
    let (mut app, mut events) = terminal_app("printf 'alpha beta'; read w");
    app.dispatch(Command::ToggleTerminal);
    // Paint once so the pane knows where its grid is.
    screen(&mut app, 100, 20);
    assert!(pump(&mut app, &mut events, |app| shows(app, "alpha beta")));
    screen(&mut app, 100, 20);
    let area = app
        .active_terminal()
        .map(|pane| pane.area)
        .unwrap_or_default();
    assert!(area.width > 0, "the terminal was painted");
    // The program's output is the grid's first row.
    let mouse = |kind, column| MouseEvent {
        kind,
        column: area.x + column,
        row: area.y,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 6));
    app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 9));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 9));
    send_key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(app.clipboard.get().unwrap_or_default(), "beta");
}

const CTRL_SHIFT: KeyModifiers = KeyModifiers::CONTROL.union(KeyModifiers::SHIFT);

#[test]
fn scrollback_keys_the_wheel_and_typing_move_the_history_view() {
    let (mut app, mut events) =
        terminal_app("i=0; while [ $i -lt 60 ]; do echo line$i; i=$((i+1)); done; read w");
    app.dispatch(Command::ToggleTerminal);
    screen(&mut app, 100, 20);
    assert!(pump(&mut app, &mut events, |app| shows(app, "line59")));
    let history = history(&app);
    assert!(history > 20, "{history} lines of history");

    send_key(&mut app, KeyCode::PageUp, KeyModifiers::SHIFT);
    let page = scrolled(&app).unwrap_or_default();
    assert!(page > 0, "Shift+PageUp scrolls back a page");
    send_key(&mut app, KeyCode::PageDown, KeyModifiers::SHIFT);
    assert_eq!(scrolled(&app), Some(0), "Shift+PageDown comes back");
    send_key(&mut app, KeyCode::Home, CTRL_SHIFT);
    assert_eq!(scrolled(&app), Some(history), "Ctrl+Shift+Home: the top");
    send_key(&mut app, KeyCode::End, CTRL_SHIFT);
    assert_eq!(scrolled(&app), Some(0), "Ctrl+Shift+End: the live screen");
    app.dispatch(Command::TerminalScrollPageUp);
    assert_eq!(scrolled(&app), Some(page));
    app.dispatch(Command::TerminalScrollPageDown);
    app.dispatch(Command::TerminalScrollToTop);
    assert_eq!(scrolled(&app), Some(history));
    app.dispatch(Command::TerminalScrollToBottom);
    assert_eq!(scrolled(&app), Some(0));

    // A wheel notch over a shell (no mouse tracking, primary screen) scrolls
    // the history by three lines.
    screen(&mut app, 100, 20);
    let area = app
        .active_terminal()
        .map(|pane| pane.area)
        .unwrap_or_default();
    let wheel = |kind| MouseEvent {
        kind,
        column: area.x + 1,
        row: area.y + 1,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse(wheel(MouseEventKind::ScrollUp));
    assert_eq!(scrolled(&app), Some(3));
    app.handle_mouse(wheel(MouseEventKind::ScrollDown));
    assert_eq!(scrolled(&app), Some(0));

    // Typing over a scrolled-back view with a selection returns to the live
    // screen and drops the selection.
    send_key(&mut app, KeyCode::Home, CTRL_SHIFT);
    screen(&mut app, 100, 20);
    drag_select(&mut app, 0, 4);
    assert!(
        app.active_terminal()
            .and_then(crate::terminal::TerminalPane::selection_text)
            .is_some(),
        "the drag selected history text"
    );
    send_key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
    assert_eq!(scrolled(&app), Some(0));
    assert_eq!(
        app.active_terminal()
            .and_then(crate::terminal::TerminalPane::selection_text),
        None
    );
}

#[test]
fn ctrl_shift_c_copies_a_selection_and_refuses_without_one() {
    let (mut app, mut events) = terminal_app("printf 'alpha beta'; read w");
    app.dispatch(Command::ToggleTerminal);
    screen(&mut app, 100, 20);
    assert!(pump(&mut app, &mut events, |app| shows(app, "alpha beta")));

    let _ = app.clipboard.set("before");
    send_key(&mut app, KeyCode::Char('c'), CTRL_SHIFT);
    assert_eq!(app.clipboard.get().unwrap_or_default(), "before");
    assert!(
        app.notifications
            .active()
            .iter()
            .any(|note| note.title.contains("copy: no text selected")),
        "the refusal is reported"
    );

    screen(&mut app, 100, 20);
    drag_select(&mut app, 0, 4);
    send_key(&mut app, KeyCode::Char('c'), CTRL_SHIFT);
    assert_eq!(app.clipboard.get().unwrap_or_default(), "alpha");
}

#[test]
fn ctrl_shift_v_pastes_the_clipboard_into_the_shell() {
    let (mut app, mut events) = terminal_app("read pasted; printf '[%s]' \"$pasted\"; read w");
    app.dispatch(Command::ToggleTerminal);
    let _ = app.clipboard.set("from the clipboard\n");
    send_key(&mut app, KeyCode::Char('v'), CTRL_SHIFT);
    assert!(
        pump(&mut app, &mut events, |app| shows(
            app,
            "[from the clipboard]"
        )),
        "the shell read the pasted line"
    );
}

#[test]
fn toggling_reaches_a_terminal_in_another_pane_and_its_events_apply_there() {
    let (mut app, mut events) = terminal_app("printf ready; read w; exit 4");
    app.dispatch(Command::ToggleTerminal);
    let terminal_pane = app.focus_pane();
    let id = active_id(&app);
    assert!(id.is_some(), "a terminal is open");
    // The split leaves the terminal in a stored pane and focuses the new one.
    app.dispatch(Command::SplitRight);
    let other_pane = app.focus_pane();
    assert_ne!(other_pane, terminal_pane);
    assert!(app.active_terminal().is_none());

    app.dispatch(Command::ToggleTerminal);
    assert_eq!(app.focus_pane(), terminal_pane, "focus moved to its pane");
    assert_eq!(active_id(&app), id);
    assert_eq!(terminal_count(&app), 1, "no second terminal was started");
    app.dispatch(Command::ToggleTerminal);
    assert_eq!(app.focus_pane(), other_pane, "and back again");

    // Reports reach the terminal where it sits, out of focus.
    let Some(id) = id else { return };
    app.on_terminal_event(
        TerminalEvent {
            id,
            kind: TerminalEventKind::Title("watch".to_owned()),
        },
        Instant::now(),
    );
    let title = |app: &App| {
        app.all_tabs()
            .find(|tab| tab.is_terminal())
            .map(|tab| tab.title.clone())
            .unwrap_or_default()
    };
    assert_eq!(title(&app), "watch");
    assert!(pump(&mut app, &mut events, |app| shows_in(
        app, id, "ready"
    )));
    app.dispatch(Command::ToggleTerminal);
    send_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    app.dispatch(Command::ToggleTerminal);
    assert_eq!(app.focus_pane(), other_pane);
    assert!(pump(&mut app, &mut events, |app| title(app) == "watch (exited)"));
}

/// Whether terminal `id`, in whichever pane, shows `needle` on its screen.
fn shows_in(app: &App, id: crate::terminal::TerminalId, needle: &str) -> bool {
    use alacritty_terminal::grid::Dimensions;
    app.all_tabs().any(|tab| {
        let TabKind::Terminal(pane) = &tab.kind else {
            return false;
        };
        let term = pane.term();
        pane.id() == id
            && (0..term.screen_lines()).any(|row| {
                let line = alacritty_terminal::index::Line(i32::try_from(row).unwrap_or_default());
                (0..term.columns())
                    .map(|column| term.grid()[line][alacritty_terminal::index::Column(column)].c)
                    .collect::<String>()
                    .contains(needle)
            })
    })
}

#[test]
fn output_repaints_only_a_shown_terminal_and_at_most_once_a_frame() {
    let (mut app, _events) = terminal_app("cat");
    app.dispatch(Command::ToggleTerminal);
    let id = active_id(&app);
    assert!(id.is_some(), "a terminal is open");
    let Some(id) = id else { return };
    let output = TerminalEvent {
        id,
        kind: TerminalEventKind::Output,
    };
    let start = Instant::now();
    assert!(
        app.on_terminal_event(output.clone(), start),
        "shown: repaint"
    );
    app.terminal_frame.drawn(start);
    let soon = start + Duration::from_millis(1);
    assert!(
        !app.on_terminal_event(output.clone(), soon),
        "too soon after the last frame"
    );
    let wake = app.next_wake();
    assert!(
        wake.is_some_and(|wake| wake <= crate::app::terminal::TERMINAL_FRAME_INTERVAL),
        "the held-back output gets its own frame: {wake:?}"
    );
    app.terminal_frame
        .drawn(soon + crate::app::terminal::TERMINAL_FRAME_INTERVAL);
    assert_eq!(app.terminal_frame.next_wake(Instant::now()), None);

    // Hidden behind another tab of its pane, it asks for nothing.
    app.select_tab(0);
    let later = start + Duration::from_secs(1);
    assert!(!app.on_terminal_event(output, later));
    assert_eq!(app.terminal_frame.next_wake(later), None);
    // The bell paints nothing either; a title does.
    let bell = TerminalEvent {
        id,
        kind: TerminalEventKind::Bell,
    };
    assert!(!app.on_terminal_event(bell, later));
}

#[test]
fn a_terminal_announces_output_again_only_once_painted() {
    let (mut app, mut events) =
        terminal_app("printf one; read a; printf two; read b; printf three; read c");
    app.dispatch(Command::ToggleTerminal);
    let first = pump_counting(&mut app, &mut events, |app, _| shows(app, "one"));
    assert!(
        first.is_some_and(|n| n >= 1),
        "the first output is announced"
    );

    // Unpainted, the next output reaches the grid without another announcement.
    send_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    let unpainted = pump_counting(&mut app, &mut events, |app, _| shows(app, "two"));
    assert_eq!(
        unpainted,
        Some(0),
        "no wake-up for a terminal nobody painted"
    );

    // Painting acknowledges it, so the output after that is announced again.
    screen(&mut app, 100, 20);
    send_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    let painted = pump_counting(&mut app, &mut events, |app, n| {
        n >= 1 && shows(app, "three")
    });
    assert!(painted.is_some(), "output after a paint is announced");
}
