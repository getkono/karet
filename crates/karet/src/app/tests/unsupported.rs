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
fn go_to_declaration_on_a_server_without_it_names_declaration_once() {
    let (backend, mut app) = completion_app("let x = target();\n", LineCol::new(0, 9));
    app.dispatch(Command::GoToDeclaration);
    let Some(id) = sent_id(&backend, |c| {
        matches!(c, SessionCommand::FindLocations { .. })
    }) else {
        unreachable!("the command issues a request");
    };

    app.on_backend_event(
        Some(id),
        SessionEvent::FeatureUnsupported {
            server: LanguageServerId::RustAnalyzer,
            feature: ServerFeature::Declaration,
        },
    );
    app.on_backend_event(
        Some(id),
        SessionEvent::Locations {
            kind: karet_session::LocationKind::Declaration,
            locations: Vec::new(),
        },
    );

    assert_eq!(titles(&app), vec![notice("go to declaration")]);
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

/// Format Selection sends every selection as it stands -- a bare caret
/// included, which the session widens to its line -- and a server that does
/// not format ranges is named on that request, once.
#[test]
fn format_selection_on_a_server_without_it_says_so() {
    let (backend, mut app) = completion_app("let x = 1;\nlet y = 2;\n", LineCol::new(1, 3));
    app.dispatch(Command::FormatSelection);
    let Some(id) = sent_id(&backend, |c| {
        matches!(
            c,
            SessionCommand::Format(karet_session::FormatRequest {
                doc: DocumentId(9),
                scope: karet_session::FormatScope::Ranges(ranges),
            }) if *ranges == [karet_core::Range {
                start: LineCol::new(1, 3),
                end: LineCol::new(1, 3),
            }]
        )
    }) else {
        unreachable!("the command issues a request for the caret");
    };

    app.on_backend_event(
        Some(id),
        SessionEvent::FeatureUnsupported {
            server: LanguageServerId::RustAnalyzer,
            feature: ServerFeature::RangeFormatting,
        },
    );

    assert_eq!(titles(&app), vec![notice("formatting a selection")]);
}

/// Format Document asks for the whole of the active document.
#[test]
fn format_document_asks_for_the_whole_document() {
    let (backend, mut app) = completion_app("let x = 1;\n", LineCol::new(0, 3));
    app.dispatch(Command::FormatDocument);
    assert!(
        sent_id(&backend, |c| {
            matches!(
                c,
                SessionCommand::Format(karet_session::FormatRequest {
                    doc: DocumentId(9),
                    scope: karet_session::FormatScope::Document,
                })
            )
        })
        .is_some()
    );
}
