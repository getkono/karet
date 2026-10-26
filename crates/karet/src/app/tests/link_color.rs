//! Language-server links and colours: asked for once per text version,
//! carried with the text, and the links followed by a Ctrl/Cmd+click.

use std::sync::Arc;

use karet_core::DecorationKind;
use karet_core::DocumentColor;
use karet_core::DocumentLink;
use karet_core::LineCol;
use karet_core::Range;

use super::support::*;
use crate::app::link_color::shift_span;
use crate::app::*;

const DOC: DocumentId = DocumentId(9);
const TEXT: &str = "a { color: #ff0000; }\nsee https://example.com/x\n";

/// The link-and-colour requests a backend received.
fn listings(backend: &RecordingBackend) -> Vec<RequestId> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::Lsp(LspCommand::LinksAndColors { .. }) => Some(*id),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The links a backend was asked to resolve.
fn resolves(backend: &RecordingBackend) -> Vec<(RequestId, DocumentLink)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::Lsp(LspCommand::ResolveDocumentLink { link, .. }) => {
                        Some((*id, link.clone()))
                    },
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn span(line: u32, start: u32, end: u32) -> Range {
    Range {
        start: LineCol::new(line, start),
        end: LineCol::new(line, end),
    }
}

fn red(range: Range) -> DocumentColor {
    DocumentColor {
        range,
        rgba: [255, 0, 0, 255],
    }
}

fn link(range: Range, target: Option<&str>) -> DocumentLink {
    DocumentLink {
        range,
        target: target.map(str::to_owned),
        tooltip: None,
        payload: Some("{}".to_owned()),
    }
}

/// An app that asked once and was answered with these halves.
fn answered(
    links: Option<Vec<DocumentLink>>,
    colors: Option<Vec<DocumentColor>>,
) -> (Arc<RecordingBackend>, App) {
    let (backend, mut app) = completion_app(TEXT, LineCol::new(0, 0));
    app.request_links_and_colors();
    let id = listings(&backend).last().copied();
    let version = app.buffer_version(DOC).unwrap_or_default();
    app.on_links_and_colors(id, DOC, version, (links, colors));
    (backend, app)
}

fn held_colors(app: &App) -> Vec<Range> {
    app.links_colors
        .colors
        .get(&DOC)
        .into_iter()
        .flat_map(|(_, colors)| colors.iter().map(|colour| colour.range))
        .collect()
}

fn type_at(app: &mut App, at: LineCol, text: &'static str) {
    if let TabKind::Code { .. } = &app.tabs[app.active].kind {
        app.tabs[app.active].editor.set_carets(&[at]);
    }
    app.submit_edit(move |caret, selection, _, base| {
        Some(karet_editor::editing::insert(caret, selection, base, text))
    });
}

#[test]
fn a_document_is_asked_once_per_version_and_again_once_typing_pauses() {
    let (backend, mut app) = answered(Some(Vec::new()), Some(vec![red(span(0, 11, 18))]));
    assert_eq!(listings(&backend).len(), 1);
    app.request_links_and_colors();
    assert_eq!(listings(&backend).len(), 1, "a covered version re-asked");

    type_at(&mut app, LineCol::new(1, 0), "x");
    app.request_links_and_colors();
    assert_eq!(listings(&backend).len(), 1, "asked while still typing");
    assert!(app.link_color_next_wake(Instant::now()).is_some());
    let later = Instant::now() + karet_session::LSP_CHANGE_DEBOUNCE * 2;
    app.request_links_and_colors_at(later);
    app.request_links_and_colors_at(later);
    assert_eq!(listings(&backend).len(), 2, "one request for the new text");
}

#[test]
fn an_answer_the_buffer_has_moved_past_is_dropped() {
    let (backend, mut app) = completion_app(TEXT, LineCol::new(0, 0));
    app.request_links_and_colors();
    let id = listings(&backend).last().copied();
    let version = app.buffer_version(DOC).unwrap_or_default();
    type_at(&mut app, LineCol::new(1, 0), "x");
    app.on_links_and_colors(id, DOC, version, (None, Some(vec![red(span(0, 11, 18))])));
    assert!(held_colors(&app).is_empty());

    // Nor is an answer to a request that was never the outstanding one.
    let version = app.buffer_version(DOC).unwrap_or_default();
    app.on_links_and_colors(
        Some(RequestId(999)),
        DOC,
        version,
        (None, Some(vec![red(span(0, 11, 18))])),
    );
    assert!(held_colors(&app).is_empty());
}

/// A half that went unanswered keeps what is shown for it, adopts the half
/// that was answered, and backs off before asking again.
#[test]
fn a_failed_half_keeps_what_is_shown_and_backs_off() {
    let (backend, mut app) = answered(
        Some(vec![link(span(1, 4, 25), Some("https://example.com/x"))]),
        Some(vec![red(span(0, 11, 18))]),
    );
    type_at(&mut app, LineCol::new(1, 0), "x");
    let now = Instant::now() + karet_session::LSP_CHANGE_DEBOUNCE * 2;
    app.request_links_and_colors_at(now);
    let id = listings(&backend).last().copied();
    let version = app.buffer_version(DOC).unwrap_or_default();
    app.on_links_and_colors_at(id, DOC, version, (Some(Vec::new()), None), now);

    assert!(
        app.links_colors
            .links
            .get(&DOC)
            .is_some_and(|(at, links)| *at == version && links.is_empty()),
        "the answered half is adopted"
    );
    assert_eq!(
        held_colors(&app),
        [span(0, 11, 18)],
        "the failed half stands"
    );
    app.request_links_and_colors_at(now);
    assert_eq!(listings(&backend).len(), 2, "re-asked without waiting");
    app.request_links_and_colors_at(now + std::time::Duration::from_secs(31));
    assert_eq!(listings(&backend).len(), 3, "never re-asked");
}

/// Held spans move with the text: an edit before one carries it, an edit
/// into one drops it -- a colour being retyped no longer spells its colour.
#[test]
fn held_spans_move_with_the_text_and_an_edit_into_one_drops_it() {
    let (_, mut app) = answered(
        Some(vec![link(span(1, 4, 25), Some("https://example.com/x"))]),
        Some(vec![red(span(0, 11, 18))]),
    );
    type_at(&mut app, LineCol::new(0, 0), "  ");
    assert_eq!(held_colors(&app), [span(0, 13, 20)]);
    type_at(&mut app, LineCol::new(0, 15), "0");
    assert!(
        held_colors(&app).is_empty(),
        "the retyped literal kept its swatch"
    );
    let links: Vec<Range> = app
        .links_colors
        .links
        .get(&DOC)
        .into_iter()
        .flat_map(|(_, links)| links.iter().map(|link| link.range))
        .collect();
    assert_eq!(links, [span(1, 4, 25)], "an edit on another line leaves it");
}

#[test]
fn a_span_is_dropped_only_by_an_edit_that_reaches_into_it() {
    let edit = |start: LineCol, end: LineCol, text: &str| karet_core::TextEdit {
        range: Range { start, end },
        new_text: text.to_owned(),
    };
    let at = LineCol::new;
    let colour = span(2, 4, 8);
    // Typing right before or right after it moves or leaves it.
    assert_eq!(
        shift_span(colour, &[edit(at(2, 4), at(2, 4), "ab")]),
        Some(span(2, 6, 10))
    );
    assert_eq!(
        shift_span(colour, &[edit(at(2, 8), at(2, 8), "ab")]),
        Some(colour)
    );
    // A line inserted above carries it down.
    assert_eq!(
        shift_span(colour, &[edit(at(0, 0), at(0, 0), "\n")]),
        Some(span(3, 4, 8))
    );
    // Inside, or replacing any of it, drops it.
    assert_eq!(shift_span(colour, &[edit(at(2, 5), at(2, 5), "x")]), None);
    assert_eq!(shift_span(colour, &[edit(at(2, 7), at(2, 9), "")]), None);
    assert_eq!(shift_span(colour, &[edit(at(1, 0), at(2, 5), "")]), None);
}

#[test]
fn closing_a_document_forgets_what_is_held_for_it() {
    let (_, mut app) = answered(Some(Vec::new()), Some(vec![red(span(0, 11, 18))]));
    app.forget_links_and_colors(DOC);
    assert!(held_colors(&app).is_empty());
    assert!(!app.links_colors.links.contains_key(&DOC));
}

/// A server's state change makes every document worth asking again, though
/// what is shown stays until the new answer.
#[test]
fn a_server_state_change_asks_again_without_dropping_what_is_shown() {
    let (backend, mut app) = answered(Some(Vec::new()), Some(vec![red(span(0, 11, 18))]));
    app.invalidate_link_color_coverage();
    app.request_links_and_colors();
    assert_eq!(listings(&backend).len(), 2);
    assert_eq!(held_colors(&app), [span(0, 11, 18)]);
}

/// The front tab drawn in one focused pane at the origin, so screen column
/// `3 + c` of row `l` is buffer column `c` of line `l`.
fn framed(app: &mut App) {
    app.pane_frames = vec![content_frame(app, Rect::new(0, 0, 60, 5))];
}

fn pointer(kind: MouseEventKind, col: u16, row: u16, mods: KeyModifiers) -> MouseEvent {
    MouseEvent {
        kind,
        column: 3 + col,
        row,
        modifiers: mods,
    }
}

/// Ctrl-hover underlines a server link over the span the server gave, which
/// is not the URL a scan of the line would find.
#[test]
fn ctrl_hover_underlines_a_server_link_over_its_own_span() {
    // The server links `see https` -- columns 0..9 -- not just the URL.
    let (_, mut app) = answered(
        Some(vec![link(span(1, 0, 9), Some("https://x.test/"))]),
        None,
    );
    framed(&mut app);
    app.update_definition_hover(&pointer(MouseEventKind::Moved, 1, 1, KeyModifiers::CONTROL));
    let underline = app.definition_underline_decoration();
    assert_eq!(
        underline.as_ref().map(|d| d.kind.clone()),
        Some(DecorationKind::Underline)
    );
    assert_eq!(underline.map(|d| d.range), Some(span(1, 0, 9)));
}

/// A held link describing other text -- positioned against an older
/// version -- answers no hover and no click.
#[test]
fn a_link_for_other_text_is_not_followed() {
    let (_, mut app) = answered(
        Some(vec![link(span(1, 0, 3), Some("https://x.test/"))]),
        None,
    );
    if let Some((version, _)) = app.links_colors.links.get_mut(&DOC) {
        *version = version.wrapping_add(1);
    }
    let version = app.buffer_version(DOC).unwrap_or_default();
    assert!(
        app.server_link_at(DOC, version, LineCol::new(1, 1))
            .is_none()
    );
}

/// An app over a workspace holding `src/lib.rs` (several lines) and
/// `notes.md`, editing `notes.md` as document 9, with a recording backend.
fn workspace(name: &str) -> (Arc<RecordingBackend>, App, PathBuf) {
    let root = test_dir(name);
    write_file(&root, "src/lib.rs", b"one\ntwo\nthree\nfour\n");
    write_file(&root, "notes.md", b"see lib here\n");
    let backend = Arc::new(RecordingBackend::new());
    let mut app = App::new(root.clone(), Vec::new(), Vec::new(), false);
    app.open_path(&root.join("notes.md"));
    fill_reserved_tabs(&mut app);
    app.backend = Some(backend.clone());
    app.focus = Focus::Editor;
    let idx = app.active;
    if let TabKind::Code { doc, .. } = &mut app.tabs[idx].kind {
        *doc = Some(DOC);
    }
    app.pane_frames = vec![content_frame(&app, Rect::new(0, 0, 60, 5))];
    (backend, app, root)
}

/// Hold `links` for document 9 at its current version.
fn hold_links(app: &mut App, links: Vec<DocumentLink>) {
    let version = app.buffer_version(DOC).unwrap_or_default();
    app.links_colors.links.insert(DOC, (version, links));
}

fn ctrl_click(app: &mut App, col: u16) -> bool {
    let down = MouseEventKind::Down(MouseButton::Left);
    app.handle_markdown_link_mouse(pointer(down, col, 0, KeyModifiers::CONTROL))
}

fn caret(app: &App) -> LineCol {
    app.tabs[app.active].editor.cursor()
}

/// Ctrl+click on a server link to a workspace file opens it at the line and
/// column its fragment names.
#[test]
fn ctrl_click_opens_a_server_link_at_its_fragment() {
    let (_, mut app, root) = workspace("server-link-open");
    let lib = url::Url::from_file_path(root.join("src/lib.rs")).map(String::from);
    let target = lib.map(|lib| format!("{lib}#L3,2")).unwrap_or_default();
    hold_links(&mut app, vec![link(span(0, 4, 7), Some(&target))]);

    assert!(!ctrl_click(&mut app, 1), "off the link");
    assert!(ctrl_click(&mut app, 5));
    // The jump waits for the file's text, and lands once it arrives.
    fill_reserved_tabs(&mut app);
    let opened = std::fs::canonicalize(root.join("src/lib.rs")).ok();
    assert_eq!(app.tabs[app.active].path().map(Path::to_path_buf), opened);
    assert_eq!(caret(&app), LineCol::new(2, 1));
    let _ = std::fs::remove_dir_all(root);
}

/// A deferred link is resolved first, and followed when the answer comes; a
/// stale answer is ignored, and a failed one is reported.
#[test]
fn ctrl_click_resolves_a_deferred_link_then_follows_it() {
    let (backend, mut app, root) = workspace("server-link-resolve");
    let deferred = link(span(0, 4, 7), None);
    hold_links(&mut app, vec![deferred.clone()]);

    assert!(ctrl_click(&mut app, 5));
    let asked = resolves(&backend);
    let [(id, sent)] = asked.as_slice() else {
        panic!("expected one resolve, got {asked:?}");
    };
    assert_eq!(sent, &deferred);
    assert!(
        app.tabs[app.active]
            .path()
            .is_some_and(|p| p.ends_with("notes.md"))
    );

    let lib = url::Url::from_file_path(root.join("src/lib.rs")).map(String::from);
    let resolved = DocumentLink {
        target: lib.ok(),
        ..deferred.clone()
    };
    app.on_document_link_resolved(Some(RequestId(999)), DOC, Ok(resolved.clone()));
    assert!(
        app.tabs[app.active]
            .path()
            .is_some_and(|p| p.ends_with("notes.md")),
        "an answer to no click followed"
    );
    app.on_document_link_resolved(Some(*id), DOC, Ok(resolved.clone()));
    assert!(
        app.tabs[app.active]
            .path()
            .is_some_and(|p| p.ends_with("lib.rs"))
    );
    let held = app
        .links_colors
        .links
        .get(&DOC)
        .map(|(_, links)| links.clone());
    assert_eq!(held, Some(vec![resolved]), "the resolved link is kept");

    // A resolve that failed is reported, not followed.
    app.links_colors.links.clear();
    app.open_path(&root.join("notes.md"));
    let idx = app.active;
    if let TabKind::Code { doc, .. } = &mut app.tabs[idx].kind {
        *doc = Some(DOC);
    }
    hold_links(&mut app, vec![deferred]);
    assert!(ctrl_click(&mut app, 5));
    let id = resolves(&backend).last().map(|(id, _)| *id);
    app.on_document_link_resolved(id, DOC, Err("no target".to_owned()));
    assert!(
        last_message(&app).is_some_and(|message| message.contains("no target")),
        "{:?}",
        last_message(&app)
    );
    let _ = std::fs::remove_dir_all(root);
}

/// A server link pointing outside the workspace asks before it opens.
#[test]
fn a_server_link_outside_the_workspace_asks_first() {
    let (_, mut app, root) = workspace("server-link-outside");
    let outside = std::env::temp_dir().join("karet-server-link-outside.rs");
    let target = url::Url::from_file_path(&outside)
        .map(String::from)
        .unwrap_or_default();
    app.open_server_link_target(&target);
    assert!(
        app.tabs[app.active]
            .path()
            .is_some_and(|p| p.ends_with("notes.md"))
    );
    assert!(app.confirm.is_some(), "no confirmation was asked");
    let _ = std::fs::remove_dir_all(root);
}
