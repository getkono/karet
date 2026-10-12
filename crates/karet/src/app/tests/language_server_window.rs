//! What a language server says to the user, as the client shows it: work in
//! the status bar and badge, questions in the confirmation dialog, and the log
//! in a tab.

use karet_session::LanguageServerWork;
use karet_session::WorkPhase;
use karet_session::WorkToken;

use super::support::*;
use crate::app::*;

/// An app with a rust file open, rust-analyzer running at `/workspace`, and a
/// recording backend.
fn rust_app() -> (Arc<RecordingBackend>, App) {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.push_tab(text_tab("/workspace/src/main.rs", "fn main() {}\n"));
    app.show_language_server_status(
        None,
        vec![language_server_status(
            LanguageServerId::RustAnalyzer,
            "rust",
            true,
        )],
    );
    (backend, app)
}

fn work(phase: WorkPhase, cancellable: bool) -> SessionEvent {
    SessionEvent::LanguageServerWork {
        work: LanguageServerWork {
            server: LanguageServerId::RustAnalyzer,
            root: PathBuf::from("/workspace"),
            token: WorkToken::Text("index".to_owned()),
            phase,
            title: (phase == WorkPhase::Begin).then(|| "Indexing".to_owned()),
            message: Some("3/10".to_owned()),
            percentage: Some(30),
            cancellable: Some(cancellable),
        },
    }
}

fn sent(backend: &RecordingBackend) -> Vec<SessionCommand> {
    backend
        .sent
        .lock()
        .map(|sent| sent.iter().map(|(_, command)| command.clone()).collect())
        .unwrap_or_default()
}

#[test]
fn work_shows_in_the_status_bar_and_badge_until_it_ends() {
    let (_backend, mut app) = rust_app();
    app.on_backend_event(None, work(WorkPhase::Begin, true));
    assert_eq!(
        app.active_language_server_badge().map(|badge| badge.state),
        Some(LanguageServerBadge::Busy)
    );
    let shown = screen(&mut app, 140, 12).join("\n");
    assert!(
        shown.contains("rust-analyzer: Indexing 3/10 30% \u{d7}"),
        "the status bar shows the work, cancellable: {shown}"
    );
    assert!(shown.contains("LSP busy"), "{shown}");

    app.on_backend_event(None, work(WorkPhase::End, true));
    assert_eq!(
        app.active_language_server_badge().map(|badge| badge.state),
        Some(LanguageServerBadge::Ready)
    );
    let shown = screen(&mut app, 140, 12).join("\n");
    assert!(!shown.contains("Indexing"), "{shown}");
}

#[test]
fn work_ends_with_the_connection_doing_it() {
    let (_backend, mut app) = rust_app();
    app.on_backend_event(None, work(WorkPhase::Begin, false));
    assert!(app.active_language_server_work().is_some());
    app.update_language_server_runtime(
        LanguageServerId::RustAnalyzer,
        PathBuf::from("/workspace"),
        LanguageServerRuntimeState::Retrying,
        None,
    );
    assert!(app.active_language_server_work().is_none());
}

#[test]
fn cancelling_sends_the_work_token_only_when_the_server_allows_it() {
    let (backend, mut app) = rust_app();
    app.dispatch(Command::CancelLanguageServerWork);
    app.on_backend_event(None, work(WorkPhase::Begin, false));
    app.dispatch(Command::CancelLanguageServerWork);
    let cancels = |backend: &RecordingBackend| {
        sent(backend)
            .into_iter()
            .filter(|command| matches!(command, SessionCommand::CancelLanguageServerWork { .. }))
            .count()
    };
    assert_eq!(cancels(&backend), 0, "nothing cancellable was running");

    app.on_backend_event(None, work(WorkPhase::Begin, true));
    app.dispatch(Command::CancelLanguageServerWork);
    assert!(sent(&backend).iter().any(|command| matches!(
        command,
        SessionCommand::CancelLanguageServerWork { server, root, token }
            if *server == LanguageServerId::RustAnalyzer
                && root == Path::new("/workspace")
                && *token == WorkToken::Text("index".to_owned())
    )));
}

fn question(id: u64) -> SessionEvent {
    SessionEvent::LanguageServerMessageRequest {
        request: LanguageServerMessageId(id),
        server: LanguageServerId::RustAnalyzer,
        severity: Severity::Warning,
        message: "Reload the workspace?".to_owned(),
        actions: vec!["Reload".to_owned(), "Later".to_owned()],
    }
}

fn answers(backend: &RecordingBackend) -> Vec<(LanguageServerMessageId, Option<usize>)> {
    sent(backend)
        .into_iter()
        .filter_map(|command| match command {
            SessionCommand::AnswerLanguageServerMessage { request, action } => {
                Some((request, action))
            },
            _ => None,
        })
        .collect()
}

#[test]
fn a_question_offers_its_actions_after_a_dismiss_that_backing_out_takes() {
    let (backend, mut app) = rust_app();
    app.on_backend_event(None, question(1));
    let labels: Vec<String> = app
        .confirm
        .as_ref()
        .map(|dialog| {
            dialog
                .choices
                .entries
                .iter()
                .filter_map(|choice| choice.label.clone())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(labels, vec!["Dismiss", "Reload", "Later"]);

    app.confirm_cancel();
    assert_eq!(answers(&backend), vec![(LanguageServerMessageId(1), None)]);

    app.on_backend_event(None, question(2));
    app.confirm_step(1);
    app.confirm_accept();
    assert_eq!(
        answers(&backend).last(),
        Some(&(LanguageServerMessageId(2), Some(0)))
    );
}

#[test]
fn a_question_waits_for_an_open_dialog_instead_of_declining_it() {
    let (backend, mut app) = rust_app();
    app.confirm_action(
        "Delete?",
        "the user's own question",
        "Keep",
        "Delete",
        ConfirmAction::Cancel,
    );
    app.on_backend_event(None, question(7));
    assert!(
        app.confirm
            .as_ref()
            .is_some_and(|dialog| dialog.title == "Delete?"),
        "the user's dialog stays up"
    );
    assert!(answers(&backend).is_empty());

    app.confirm_cancel();
    assert!(
        app.confirm
            .as_ref()
            .is_some_and(|dialog| dialog.title.starts_with("rust-analyzer asks")),
        "the server's question follows once the dialog closes"
    );
}

#[test]
fn the_log_tab_is_reserved_at_once_and_filled_by_the_answer() {
    let (backend, mut app) = rust_app();
    app.dispatch(Command::ShowLanguageServerLog);
    assert!(matches!(
        app.tabs.get(app.active).map(|tab| &tab.kind),
        Some(TabKind::ServerLog { server, entries, loading_since: Some(_), .. })
            if *server == LanguageServerId::RustAnalyzer && entries.is_empty()
    ));
    assert!(sent(&backend).iter().any(|command| matches!(
        command,
        SessionCommand::LanguageServerLog { server } if *server == LanguageServerId::RustAnalyzer
    )));

    app.on_backend_event(
        None,
        SessionEvent::LanguageServerLog {
            server: LanguageServerId::RustAnalyzer,
            entries: vec![karet_session::LanguageServerLogEntry {
                root: PathBuf::from("/workspace"),
                severity: Severity::Error,
                text: "cargo metadata failed".to_owned(),
            }],
        },
    );
    let shown = screen(&mut app, 100, 12).join("\n");
    assert!(shown.contains("[error] cargo metadata failed"), "{shown}");

    // Asking again reuses the tab.
    let tabs = app.tabs.len();
    app.select_tab(0);
    app.dispatch(Command::ShowLanguageServerLog);
    assert_eq!(app.tabs.len(), tabs);
}

#[test]
fn the_log_needs_a_file_a_server_serves() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.dispatch(Command::ShowLanguageServerLog);
    assert!(
        !app.all_tabs()
            .any(|tab| matches!(tab.kind, TabKind::ServerLog { .. }))
    );
    assert!(last_message(&app).is_some_and(|message| message.contains("language server")));
}
