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
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        while let Ok(event) = events.try_recv() {
            app.on_terminal_event(event);
        }
        if done(app) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
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
    assert_eq!(app.focus, Focus::Editor);
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
        app.overlay.is_none(),
        "Ctrl+P is the shell's, not Quick Open"
    );
    send_key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(app.focus, Focus::Editor, "Tab completes in the shell");
    send_key(&mut app, KeyCode::Char('b'), KeyModifiers::CONTROL);
    assert!(app.sidebar_visible, "Ctrl+B moves the shell's cursor");
    // The terminal layer keeps the escape hatches.
    send_key(
        &mut app,
        KeyCode::Char('p'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert!(
        app.overlay.is_some(),
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
        app.on_terminal_event(TerminalEvent {
            id,
            kind: TerminalEventKind::Clipboard("from vim".to_owned()),
        });
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
