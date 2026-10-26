//! The hover request/answer wiring: command out, `HoverResult` in, popup on
//! screen. The pure composition lives in [`crate::hover`]; what is covered
//! here is the seam between it, the backend, and the renderer.

use karet_core::Diagnostic;
use karet_core::Hover;
use karet_core::Markup;
use karet_core::MarkupKind;
use karet_core::Range;
use karet_core::Severity;

use super::support::*;
use crate::app::*;

/// The hover requests a backend received, as `(id, position)`.
fn hover_requests(backend: &RecordingBackend) -> Vec<(RequestId, LineCol)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::Lsp(LspCommand::Hover { position, .. }) => {
                        Some((*id, *position))
                    },
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn markdown(value: &str) -> Hover {
    Hover {
        contents: Markup {
            kind: MarkupKind::Markdown,
            value: value.to_owned(),
        },
        range: None,
    }
}

fn diagnostic(line: u32, cols: (u32, u32), severity: Severity, message: &str) -> Diagnostic {
    Diagnostic {
        range: Range {
            start: LineCol::new(line, cols.0),
            end: LineCol::new(line, cols.1),
        },
        severity,
        message: message.to_owned(),
        source: Some("rust-analyzer".to_owned()),
        code: None,
        code_href: None,
        tags: Vec::new(),
        related: Vec::new(),
    }
}

#[test]
fn the_command_requests_hover_at_the_caret() {
    let (backend, mut app) = completion_app("let x = target();\n", LineCol::new(0, 9));
    app.dispatch(Command::Hover);

    let requests = hover_requests(&backend);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].1, LineCol::new(0, 9));
}

#[test]
fn the_answer_opens_the_popup_anchored_at_the_caret() {
    let (backend, mut app) = completion_app("let x = target();\n", LineCol::new(0, 9));
    app.dispatch(Command::Hover);
    let (id, at) = hover_requests(&backend)[0];

    app.on_hover_result(Some(id), Some(markdown("fn target() -> i32")));

    let Some(ui) = &app.hover_ui else {
        panic!("a matching answer should open the popup");
    };
    assert_eq!(ui.at, at);
    assert_eq!(ui.markup.value, "fn target() -> i32");
}

#[test]
fn a_superseded_request_id_is_dropped() {
    let (backend, mut app) = completion_app("let x = target();\n", LineCol::new(0, 9));
    app.dispatch(Command::Hover);
    let (id, _) = hover_requests(&backend)[0];

    app.on_hover_result(Some(RequestId(id.0 + 1)), Some(markdown("stale")));
    assert!(
        app.hover_ui.is_none(),
        "an answer to a superseded request must not open a popup"
    );
}

#[test]
fn an_answer_for_a_caret_that_moved_on_is_dropped() {
    let (backend, mut app) = completion_app("let x = target();\n", LineCol::new(0, 9));
    app.dispatch(Command::Hover);
    let (id, _) = hover_requests(&backend)[0];

    let idx = app.active;
    app.tabs[idx].editor.set_carets(&[LineCol::new(0, 2)]);
    app.on_hover_result(Some(id), Some(markdown("moved on")));
    assert!(app.hover_ui.is_none());
}

#[test]
fn diagnostics_under_the_caret_compose_into_the_popup() {
    let (backend, mut app) = completion_app("let x = target();\n", LineCol::new(0, 9));
    app.docs.diagnostics.insert(
        DocumentId(9),
        vec![diagnostic(0, (8, 14), Severity::Error, "mismatched types")],
    );
    app.dispatch(Command::Hover);
    let (id, _) = hover_requests(&backend)[0];

    app.on_hover_result(Some(id), Some(markdown("fn target() -> i32")));

    let Some(ui) = &app.hover_ui else {
        panic!("diagnostics plus documentation should open the popup");
    };
    assert_eq!(ui.markup.kind, MarkupKind::Markdown);
    assert!(ui.markup.value.starts_with("**error** mismatched types"));
    assert!(ui.markup.value.contains("fn target() -> i32"));
}

/// Both surfaces show a diagnostic's related locations relative to the
/// workspace root, and link its code to the code's documentation.
#[test]
fn related_locations_and_code_links_reach_the_popup_and_the_detail_view() {
    let (backend, mut app) = completion_app("let x = target();\n", LineCol::new(0, 9));
    let mut d = diagnostic(0, (8, 14), Severity::Error, "mismatched types");
    d.code = Some("E0308".to_owned());
    d.code_href = Some("https://example.com/E0308".to_owned());
    d.related = vec![karet_core::RelatedInfo {
        location: karet_core::Location {
            path: app.root.join("src/lib.rs"),
            range: Range {
                start: LineCol::new(2, 0),
                end: LineCol::new(2, 1),
            },
        },
        message: "expected due to this".to_owned(),
    }];
    app.docs.diagnostics.insert(DocumentId(9), vec![d]);
    let related = "- `src/lib.rs:3:1` expected due to this";
    let link = "[E0308](<https://example.com/E0308>)";

    app.dispatch(Command::Hover);
    let (id, _) = hover_requests(&backend)[0];
    app.on_hover_result(Some(id), Some(markdown("fn target() -> i32")));
    let popup = app.hover_ui.as_ref().map(|ui| ui.markup.value.clone());
    let popup = popup.unwrap_or_default();
    assert!(popup.contains(related), "{popup}");
    assert!(popup.contains(link), "{popup}");

    app.show_diagnostic();
    let view = app
        .diagnostic_view
        .as_ref()
        .map(|view| view.markdown.clone());
    let view = view.unwrap_or_default();
    assert!(view.contains(related), "{view}");
    assert!(view.contains(link), "{view}");
}

#[test]
fn an_empty_answer_with_no_diagnostics_says_so_instead_of_opening() {
    let (backend, mut app) = completion_app("let x = target();\n", LineCol::new(0, 9));
    app.dispatch(Command::Hover);
    let (id, _) = hover_requests(&backend)[0];

    app.on_hover_result(Some(id), None);
    assert!(app.hover_ui.is_none());
    assert_eq!(last_message(&app).as_deref(), Some("no hover information"));
}

#[test]
fn the_disabled_setting_explains_itself_and_sends_nothing() {
    let (backend, mut app) = completion_app("let x = target();\n", LineCol::new(0, 9));
    app.settings.editor.hover.enabled = false;
    app.dispatch(Command::Hover);

    assert!(hover_requests(&backend).is_empty());
    assert_eq!(
        last_message(&app).as_deref(),
        Some("hover is disabled (editor.hover.enabled)")
    );
}

#[test]
fn moving_the_caret_dismisses_an_open_popup() {
    let (backend, mut app) = completion_app("let x = target();\n", LineCol::new(0, 9));
    app.dispatch(Command::Hover);
    let (id, _) = hover_requests(&backend)[0];
    app.on_hover_result(Some(id), Some(markdown("fn target() -> i32")));
    assert!(app.hover_ui.is_some());

    let idx = app.active;
    app.tabs[idx].editor.set_carets(&[LineCol::new(0, 3)]);
    app.reconcile_hover();
    assert!(app.hover_ui.is_none());
}

#[test]
fn esc_dismisses_the_popup_and_consumes_the_key() {
    let (backend, mut app) = completion_app("let x = target();\n", LineCol::new(0, 9));
    app.dispatch(Command::Hover);
    let (id, _) = hover_requests(&backend)[0];
    app.on_hover_result(Some(id), Some(markdown("fn target() -> i32")));

    assert!(app.dismiss_hover(), "Esc consumes the key while open");
    assert!(app.hover_ui.is_none());
    assert!(
        !app.dismiss_hover(),
        "with nothing open it consumes nothing"
    );
}

#[test]
fn the_popup_paints_its_content_over_the_editor() {
    let (backend, mut app) = completion_app("let x = target();\n", LineCol::new(0, 9));
    app.docs.diagnostics.insert(
        DocumentId(9),
        vec![diagnostic(0, (8, 14), Severity::Error, "mismatched types")],
    );
    app.dispatch(Command::Hover);
    let (id, _) = hover_requests(&backend)[0];
    app.on_hover_result(Some(id), Some(markdown("fn target() -> i32")));

    let rows = screen(&mut app, 90, 24);
    let painted = rows.join("\n");
    assert!(
        painted.contains("mismatched types"),
        "the diagnostic message reaches the screen:\n{painted}"
    );
    assert!(
        painted.contains("target"),
        "the documentation reaches the screen:\n{painted}"
    );
}
