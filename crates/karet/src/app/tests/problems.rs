//! The Problems panel, the F8 problem walk, and the status-bar counts, driven
//! the way the user drives them: published diagnostics in, keys and clicks in,
//! panel rows, caret positions, and painted cells out.

use super::support::*;
use crate::app::*;

/// A one-cell diagnostic at `line:col`.
fn diagnostic(line: u32, col: u32, severity: Severity, message: &str) -> Diagnostic {
    Diagnostic {
        range: Range {
            start: LineCol::new(line, col),
            end: LineCol::new(line, col + 1),
        },
        severity,
        message: message.to_owned(),
        source: Some("rustc".to_owned()),
        code: None,
        tags: Vec::new(),
        related: Vec::new(),
    }
}

/// An app with one code tab for `/repo/src/main.rs`, bound to `DocumentId(1)`.
fn bound_app() -> App {
    let mut app = app();
    app.root = PathBuf::from("/repo");
    let mut tab = text_tab(
        "/repo/src/main.rs",
        "fn main() {\n    let x = 1;\n    let y = 2;\n    let z = 3;\n}\n",
    );
    if let TabKind::Code { doc, .. } = &mut tab.kind {
        *doc = Some(DocumentId(1));
    }
    app.push_tab(tab);
    app
}

/// Publish `diagnostics` for `DocumentId(1)` through the backend-event router.
fn publish(app: &mut App, diagnostics: Vec<Diagnostic>) {
    app.on_backend_event(
        None,
        SessionEvent::DiagnosticsPublished {
            doc: DocumentId(1),
            diagnostics,
        },
    );
}

fn caret(app: &App) -> LineCol {
    app.tabs[app.active].editor.cursor()
}

#[test]
fn published_diagnostics_fill_the_panel_and_a_later_set_replaces_them() {
    let mut app = bound_app();
    publish(
        &mut app,
        vec![
            diagnostic(2, 8, Severity::Warning, "unused variable `y`"),
            diagnostic(1, 8, Severity::Error, "mismatched types"),
        ],
    );
    assert_eq!(
        app.problems.rows,
        vec![
            ProblemRow::File { entry: 0, count: 2 },
            ProblemRow::Item { entry: 0 },
            ProblemRow::Item { entry: 1 },
        ]
    );
    assert_eq!(
        app.problems.entries[0].diagnostic.message,
        "mismatched types"
    );
    assert_eq!(app.problems.counts(), (1, 1));

    // The event carries the document's complete set: an empty one clears it.
    publish(&mut app, Vec::new());
    assert!(app.problems.rows.is_empty());
    assert_eq!(app.problems.counts(), (0, 0));
}

#[test]
fn closing_a_document_drops_its_problems() {
    let mut app = bound_app();
    publish(&mut app, vec![diagnostic(1, 0, Severity::Error, "boom")]);
    assert_eq!(app.problems.counts(), (1, 0));

    app.on_backend_event(None, SessionEvent::Closed { doc: DocumentId(1) });
    assert!(app.problems.entries.is_empty());
}

#[test]
fn spelling_findings_stay_out_of_the_panel_and_the_counts() {
    let mut app = bound_app();
    let mut misspelled = diagnostic(0, 3, Severity::Warning, "Unknown word");
    misspelled.source = Some("karet-spell".to_owned());
    publish(
        &mut app,
        vec![misspelled, diagnostic(1, 0, Severity::Warning, "real")],
    );
    assert_eq!(app.problems.entries.len(), 1);
    assert_eq!(app.problems.counts(), (0, 1));
}

#[test]
fn f8_and_shift_f8_walk_the_active_files_problems_and_wrap() {
    let mut app = bound_app();
    publish(
        &mut app,
        vec![
            diagnostic(3, 4, Severity::Hint, "third"),
            diagnostic(1, 8, Severity::Error, "first"),
            diagnostic(2, 8, Severity::Warning, "second"),
        ],
    );
    app.focus = Focus::Sidebar;

    send_key(&mut app, KeyCode::F(8), KeyModifiers::NONE);
    assert_eq!(caret(&app), LineCol::new(1, 8));
    assert_eq!(
        app.focus,
        Focus::Editor,
        "the walk hands focus to the editor"
    );
    assert_eq!(
        last_report(&app).map(|(_, title)| title),
        Some("error: first".to_owned())
    );

    send_key(&mut app, KeyCode::F(8), KeyModifiers::NONE);
    send_key(&mut app, KeyCode::F(8), KeyModifiers::NONE);
    assert_eq!(caret(&app), LineCol::new(3, 4));
    send_key(&mut app, KeyCode::F(8), KeyModifiers::NONE);
    assert_eq!(caret(&app), LineCol::new(1, 8), "wraps to the first");

    send_key(&mut app, KeyCode::F(8), KeyModifiers::SHIFT);
    assert_eq!(caret(&app), LineCol::new(3, 4), "wraps back to the last");
    send_key(&mut app, KeyCode::F(8), KeyModifiers::SHIFT);
    assert_eq!(caret(&app), LineCol::new(2, 8));
    // The toast is replaced in place, not stacked once per step.
    let walk_toasts = app
        .notifications
        .active()
        .iter()
        .filter(|note| note.title.contains(": "))
        .count();
    assert_eq!(walk_toasts, 1);
}

#[test]
fn f8_in_a_file_without_problems_says_so_and_leaves_the_caret() {
    let mut app = bound_app();
    send_key(&mut app, KeyCode::F(8), KeyModifiers::NONE);
    assert_eq!(caret(&app), LineCol::new(0, 0));
    assert_eq!(
        last_report(&app).map(|(_, title)| title),
        Some("no problems in this file".to_owned())
    );
}

#[test]
fn enter_on_a_problem_row_puts_the_caret_on_it() {
    let mut app = bound_app();
    publish(&mut app, vec![diagnostic(2, 8, Severity::Warning, "w")]);
    app.dispatch(Command::SelectPanel(SidebarPanel::Problems));
    assert_eq!(app.sidebar_panel, SidebarPanel::Problems);
    assert_eq!(app.focus, Focus::Sidebar);

    send_key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    send_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(app.focus, Focus::Editor);
    assert_eq!(caret(&app), LineCol::new(2, 8));
}

#[test]
fn typing_after_slash_filters_and_esc_drops_the_filter() {
    let mut app = bound_app();
    publish(
        &mut app,
        vec![
            diagnostic(1, 0, Severity::Error, "mismatched types"),
            diagnostic(2, 0, Severity::Warning, "unused variable"),
        ],
    );
    app.dispatch(Command::SelectPanel(SidebarPanel::Problems));

    send_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
    // `j` and `k` are sidebar motions; while filtering they are text.
    for c in "unk".chars() {
        send_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
    }
    assert_eq!(app.problems.filter, "unk");
    assert!(app.problems.rows.is_empty());
    send_key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
    assert_eq!(app.problems.rows.len(), 2, "`un` matches the warning only");

    // Enter keeps the filter and gives the keys back to the list.
    send_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(!app.problems.filter_input);
    assert_eq!(app.problems.filter, "un");

    send_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
    send_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.problems.filter.is_empty());
    assert_eq!(app.problems.rows.len(), 3);
}

#[test]
fn reopening_the_panel_lands_on_the_list_not_an_abandoned_filter_edit() {
    let mut app = bound_app();
    app.dispatch(Command::SelectPanel(SidebarPanel::Problems));
    send_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
    assert!(app.problems.filter_input);
    app.focus = Focus::Editor;

    app.dispatch(Command::SelectPanel(SidebarPanel::Problems));
    assert!(!app.problems.filter_input);
    assert_eq!(app.input_context().modal, None);
}

#[test]
fn s_cycles_the_severity_filter() {
    let mut app = bound_app();
    publish(
        &mut app,
        vec![
            diagnostic(1, 0, Severity::Error, "e"),
            diagnostic(2, 0, Severity::Hint, "h"),
        ],
    );
    app.dispatch(Command::SelectPanel(SidebarPanel::Problems));
    send_key(&mut app, KeyCode::Char('s'), KeyModifiers::NONE);
    assert_eq!(
        app.problems.rows.len(),
        2,
        "warnings and up: the hint drops"
    );
}

#[test]
fn the_status_bar_counts_are_painted_and_open_the_panel_when_clicked() {
    let mut app = bound_app();
    publish(
        &mut app,
        vec![
            diagnostic(1, 0, Severity::Error, "e"),
            diagnostic(2, 0, Severity::Warning, "w1"),
            diagnostic(3, 0, Severity::Warning, "w2"),
        ],
    );
    app.focus = Focus::Editor;
    app.sidebar_panel = SidebarPanel::Explorer;

    let buffer = frame(&mut app, 120, 12);
    let cells: Vec<&str> = (0..120).map(|x| buffer[(x, 11)].symbol()).collect();
    let row = cells.concat();
    // Searched by cell, not by byte: the hints left of the strip hold
    // multi-byte glyphs, so a byte offset is not a column.
    let errors_x = cells
        .windows(3)
        .position(|w| w == ["E", " ", "1"])
        .and_then(|x| u16::try_from(x).ok());
    let Some(errors_x) = errors_x else {
        panic!("no error count in the status bar: {row:?}");
    };
    assert!(row.contains("W 2"), "{row:?}");
    let error_fg = app
        .theme
        .role(karet_core::ThemeRole::DiagnosticError)
        .to_ratatui();
    assert_eq!(buffer[(errors_x, 11)].fg, error_fg);

    let command = app.status_command_at(errors_x);
    assert_eq!(command, Some(Command::SelectPanel(SidebarPanel::Problems)));
    if let Some(command) = command {
        app.dispatch(command);
    }
    assert_eq!(app.sidebar_panel, SidebarPanel::Problems);
    assert!(app.sidebar_visible);
}

#[test]
fn the_panel_paints_a_file_heading_and_severity_marked_rows() {
    let mut app = bound_app();
    publish(
        &mut app,
        vec![diagnostic(1, 8, Severity::Error, "mismatched types")],
    );
    app.dispatch(Command::SelectPanel(SidebarPanel::Problems));

    let rows = screen(&mut app, 120, 12);
    let painted = rows.join("\n");
    assert!(painted.contains("PROBLEMS"), "{painted}");
    assert!(painted.contains("src/main.rs (1)"), "{painted}");
    assert!(painted.contains("E mismatched types 2:9"), "{painted}");
}
