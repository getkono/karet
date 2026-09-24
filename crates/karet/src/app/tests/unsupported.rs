//! A server that does not offer what the user asked for says so, instead of
//! the empty answer that follows reading as "nothing found".

use karet_core::ServerFeature;
use karet_session::LanguageServerId;

use super::support::*;
use crate::app::*;

/// The id of the one request of the kind `pick` selects.
fn sent_id(backend: &RecordingBackend, pick: fn(&SessionCommand) -> bool) -> Option<RequestId> {
    backend.sent.lock().ok().and_then(|sent| {
        sent.iter()
            .find(|(_, command)| pick(command))
            .map(|(id, _)| *id)
    })
}

fn notice(feature: &str) -> String {
    format!(
        "{} does not support {feature}",
        LanguageServerId::RustAnalyzer.display_name()
    )
}

/// Every notification title currently on screen.
fn titles(app: &App) -> Vec<String> {
    app.notifications
        .active()
        .iter()
        .map(|note| note.title.clone())
        .collect()
}

#[test]
fn go_to_definition_on_a_server_without_it_says_so_once() {
    let (backend, mut app) = completion_app("let x = target();\n", LineCol::new(0, 9));
    app.dispatch(Command::GoToDefinition);
    let Some(id) = sent_id(&backend, |c| matches!(c, SessionCommand::Definition { .. })) else {
        unreachable!("the command issues a request");
    };

    app.on_backend_event(
        Some(id),
        SessionEvent::FeatureUnsupported {
            server: LanguageServerId::RustAnalyzer,
            feature: ServerFeature::Definition,
        },
    );
    // The empty answer the session still owes follows the notice, and must
    // not contradict it with a claim about the symbol.
    app.on_backend_event(
        Some(id),
        SessionEvent::Definitions {
            locations: Vec::new(),
        },
    );

    assert_eq!(titles(&app), vec![notice("go to definition")]);
}

#[test]
fn hover_on_a_server_without_it_says_so_instead_of_no_information() {
    let (backend, mut app) = completion_app("let x = target();\n", LineCol::new(0, 9));
    app.request_hover();
    let Some(id) = sent_id(&backend, |c| matches!(c, SessionCommand::Hover { .. })) else {
        unreachable!("the command issues a request");
    };

    app.on_backend_event(
        Some(id),
        SessionEvent::FeatureUnsupported {
            server: LanguageServerId::RustAnalyzer,
            feature: ServerFeature::Hover,
        },
    );
    app.on_backend_event(Some(id), SessionEvent::HoverResult { hover: None });

    assert_eq!(titles(&app), vec![notice("hover")]);
}

#[test]
fn an_empty_answer_without_a_refusal_still_reads_as_nothing_found() {
    // The notice replaces the generic message only when the server said so;
    // a server that offers hover and found nothing is still "no information".
    let (backend, mut app) = completion_app("let x = target();\n", LineCol::new(0, 9));
    app.request_hover();
    let Some(id) = sent_id(&backend, |c| matches!(c, SessionCommand::Hover { .. })) else {
        unreachable!("the command issues a request");
    };
    app.on_backend_event(Some(id), SessionEvent::HoverResult { hover: None });
    assert_eq!(last_message(&app).as_deref(), Some("no hover information"));
}
