//! The editor-facing language-server badge: what it says, where it says it, and
//! what clicking it does.
//!
//! Split from `language_servers`, which covers the manager tab and the install
//! lifecycle. These tests are about the indicator alone.

use super::support::*;
use crate::app::*;

/// A provider status for `language`, resolved and running at `/workspace`.
fn language_server_status(
    server: LanguageServerId,
    language: &str,
    managed: bool,
) -> LanguageServerStatus {
    LanguageServerStatus {
        ever_installed: managed,
        declined: false,
        server,
        languages: vec![language.to_string()],
        enabled: true,
        managed,
        manual_install_reason: (!managed).then(|| "install with the project toolchain".to_string()),
        installed: managed.then(|| "1.2.3".to_string()),
        cleanup_pending: false,
        instances: vec![karet_session::LanguageServerInstanceStatus {
            root: PathBuf::from("/workspace"),
            source: if managed {
                karet_session::LanguageServerSource::Managed
            } else {
                karet_session::LanguageServerSource::Path
            },
            command: Some("/bin/server".to_string()),
            args: Vec::new(),
            runtime: LanguageServerRuntimeState::Running,
            open_documents: 1,
            error: None,
        }],
    }
}

/// A code tab whose language is set explicitly.
///
/// `text_tab` hardcodes `"Rust"`, which is fine for most tests but hides the very
/// thing these ones assert: that a badge is resolved per file's language.
fn tab_in(name: &str, text: &str, language: &'static str) -> Tab {
    let mut tab = text_tab(name, text);
    if let TabKind::Code {
        language: current, ..
    } = &mut tab.kind
    {
        *current = language;
    }
    tab
}

/// A status for `language` whose single instance is unresolved at `/workspace`,
/// i.e. a provider karet could install but has not.
fn uninstalled_status(server: LanguageServerId, language: &str) -> LanguageServerStatus {
    let mut status = language_server_status(server, language, true);
    status.installed = None;
    status.ever_installed = false;
    if let Some(instance) = status.instances.first_mut() {
        instance.command = None;
        instance.source = karet_session::LanguageServerSource::Unavailable;
        instance.runtime = LanguageServerRuntimeState::Idle;
    }
    status
}

#[test]
fn the_badge_separates_a_missing_install_from_a_crash() {
    // The regression this pins: both conditions used to render "LSP unavailable",
    // so the badge could not say whether the user should install something or
    // whether an installed server was failing.
    let mut app = app();
    app.push_tab(text_tab("/workspace/src/main.rs", "fn main() {}\n"));

    app.show_language_server_status(
        None,
        vec![uninstalled_status(LanguageServerId::RustAnalyzer, "rust")],
    );
    assert_eq!(
        app.active_language_server_badge().map(|badge| badge.state),
        Some(LanguageServerBadge::NotInstalled)
    );

    app.show_language_server_status(
        None,
        vec![language_server_status(
            LanguageServerId::RustAnalyzer,
            "rust",
            true,
        )],
    );
    app.update_language_server_runtime(
        LanguageServerId::RustAnalyzer,
        PathBuf::from("/workspace"),
        LanguageServerRuntimeState::CircuitOpen,
        Some("exited with signal 11".to_string()),
    );
    assert_eq!(
        app.active_language_server_badge().map(|badge| badge.state),
        Some(LanguageServerBadge::Failed)
    );
}

#[test]
fn a_provider_the_user_must_install_reads_as_needing_setup() {
    let mut app = app();
    app.push_tab(tab_in("/workspace/main.go", "package main\n", "Go"));
    let mut status = uninstalled_status(LanguageServerId::new("gopls"), "go");
    status.managed = false;
    status.manual_install_reason = Some("requires the project's Go toolchain".to_string());
    app.show_language_server_status(None, vec![status]);
    assert_eq!(
        app.active_language_server_badge().map(|badge| badge.state),
        Some(LanguageServerBadge::NeedsSetup)
    );
}

#[test]
fn each_pane_badges_its_own_file() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = app();
    app.push_tab(text_tab("/workspace/src/main.rs", "fn main() {}\n"));
    app.dispatch(Command::SplitRight);
    app.push_tab(tab_in("/workspace/src/app.py", "import os\n", "Python"));
    assert_eq!(app.layout.pane_count(), 2);

    // Rust is healthy; Python's only provider is missing. The two panes must not
    // report the same condition.
    app.show_language_server_status(
        None,
        vec![
            language_server_status(LanguageServerId::RustAnalyzer, "rust", true),
            uninstalled_status(LanguageServerId::Pyright, "python"),
        ],
    );

    let mut terminal = Terminal::new(TestBackend::new(100, 12)).expect("test terminal");
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .expect("draw shell");

    // Both panes recorded a badge, at different columns, and each carries the
    // colour of its own file's condition.
    let hits = app
        .pane_frames
        .iter()
        .filter_map(|frame| frame.lsp_badge_hit.map(|hit| (frame.breadcrumb_rect, hit)))
        .collect::<Vec<_>>();
    assert_eq!(hits.len(), 2, "each pane badges its own breadcrumb");
    assert_ne!(
        hits[0].1, hits[1].1,
        "the two badges occupy distinct columns"
    );

    let buffer = terminal.backend().buffer();
    let colours = hits
        .iter()
        .map(|(rect, (start, _))| buffer[(*start, rect.y)].fg)
        .collect::<Vec<_>>();
    assert!(
        colours.contains(&app.theme.role(ThemeRole::DiagnosticHint).to_ratatui()),
        "the healthy pane reads as healthy: {colours:?}"
    );
    assert!(
        colours.contains(&app.theme.role(ThemeRole::DiagnosticError).to_ratatui()),
        "the pane with no server reads as an error: {colours:?}"
    );
}

#[test]
fn clicking_a_pane_badge_opens_the_manager_and_nothing_else() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;
    use ratatui::crossterm::event::MouseButton;
    use ratatui::crossterm::event::MouseEvent;
    use ratatui::crossterm::event::MouseEventKind;

    let mut app = app();
    app.push_tab(text_tab("/workspace/src/main.rs", "fn main() {}\n"));
    app.show_language_server_status(
        None,
        vec![uninstalled_status(LanguageServerId::RustAnalyzer, "rust")],
    );

    let mut terminal = Terminal::new(TestBackend::new(100, 12)).expect("test terminal");
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .expect("draw shell");
    let frame = app
        .pane_frames
        .first()
        .cloned()
        .expect("the pane recorded a frame");
    let (start, _) = frame.lsp_badge_hit.expect("the badge recorded its columns");

    let before = app.tabs.len();
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: start,
        row: frame.breadcrumb_rect.y,
        modifiers: KeyModifiers::NONE,
    });

    assert!(
        app.tabs
            .iter()
            .any(|tab| matches!(tab.kind, TabKind::LanguageServers { .. })),
        "the click opened the Language Servers tab"
    );
    assert_eq!(
        app.tabs.len(),
        before.saturating_add(1),
        "exactly one tab was opened"
    );
    // The click must not have asked for anything: an install spends the user's
    // bandwidth, so it stays behind an explicit action in the manager.
    assert!(
        app.confirm.is_none(),
        "clicking the badge raised no confirmation"
    );
}

#[test]
fn the_status_bar_badge_is_clickable() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    // The regression this pins: the status bar's right-hand strip recorded no hit
    // regions at all, while `handle_status_mouse` consumed every click inside the
    // bar -- so a click on the badge was swallowed in silence.
    let mut app = app();
    app.push_tab(text_tab("/workspace/src/main.rs", "fn main() {}\n"));
    app.show_language_server_status(
        None,
        vec![uninstalled_status(LanguageServerId::RustAnalyzer, "rust")],
    );

    let mut terminal = Terminal::new(TestBackend::new(100, 12)).expect("test terminal");
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .expect("draw shell");

    let row = (0..100)
        .map(|x| terminal.backend().buffer()[(x, 11)].symbol())
        .collect::<String>();
    let label_x =
        u16::try_from(row.find("LSP not installed").expect("badge label")).unwrap_or_default();
    assert_eq!(
        app.status_command_at(label_x),
        Some(Command::ManageLanguageServers),
        "the badge's columns dispatch the manager"
    );
}
