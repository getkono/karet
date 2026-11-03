//! Inlay hints under the pointer and the toggle key: each label part is its
//! own hit region, resting on one shows its tooltip (resolved on demand),
//! Ctrl+click follows a part's location, and Toggle Inlay Hints flips them --
//! for the session, or only until the next key under an `…UnlessPressed`
//! setting.

use std::path::PathBuf;

use karet_core::InlayHint;
use karet_core::InlayHintKind;
use karet_core::InlayHintLabelPart;
use karet_core::LineCol;
use karet_core::Location;
use karet_core::Markup;
use karet_core::MarkupKind;
use karet_core::Range;
use karet_session::config::schema::InlayHintsEnabled;

use super::support::*;
use crate::app::*;

const DOC: DocumentId = DocumentId(9);

fn plain(value: &str) -> Markup {
    Markup {
        kind: MarkupKind::PlainText,
        value: value.to_owned(),
    }
}

fn part(value: &str) -> InlayHintLabelPart {
    InlayHintLabelPart {
        value: value.to_owned(),
        ..InlayHintLabelPart::default()
    }
}

/// A file a part can lead to, with three lines to land on.
fn target(name: &str) -> PathBuf {
    let dir = test_dir(name);
    write_file(&dir, "vec.rs", b"pub struct Vec;\nimpl Vec {}\nfn f() {}\n");
    dir.join("vec.rs")
}

fn at(path: &std::path::Path, line: u32, col: u32) -> Location {
    Location {
        path: path.to_path_buf(),
        range: Range {
            start: LineCol::new(line, col),
            end: LineCol::new(line, col),
        },
    }
}

/// `: Vec` after `let v` on the first line, split into `: ` and `Vec`.
fn split_hint() -> InlayHint {
    InlayHint {
        parts: vec![part(": "), part("Vec")],
        ..InlayHint::new(LineCol::new(0, 5), ": Vec", InlayHintKind::Type)
    }
}

/// An editor over a few lines, holding `hint`, painted once so its parts have
/// hit regions.
fn painted_app(hint: InlayHint) -> (std::sync::Arc<RecordingBackend>, App) {
    let (backend, mut app) = completion_app("let v\nlet w\nlet x\n", LineCol::new(2, 0));
    app.docs.inlay_hints.insert(DOC, vec![hint]);
    frame(&mut app, 100, 12);
    (backend, app)
}

/// The hit region of part `part` of the first hint.
fn part_hit(app: &App, part: usize) -> Option<InlayHintHit> {
    app.inlay_ui
        .hits
        .iter()
        .copied()
        .find(|hit| hit.hint == 0 && hit.part == part)
}

fn mouse(kind: MouseEventKind, (column, row): (u16, u16), modifiers: KeyModifiers) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers,
    }
}

fn rest_on(app: &mut App, hit: InlayHintHit) {
    app.handle_mouse(mouse(
        MouseEventKind::Moved,
        (hit.rect.x, hit.rect.y),
        KeyModifiers::NONE,
    ));
}

fn ctrl_click(app: &mut App, hit: InlayHintHit) {
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        (hit.rect.x, hit.rect.y),
        KeyModifiers::CONTROL,
    ));
}

/// The resolve requests a backend received, as `(id, hint)`.
fn resolve_requests(backend: &RecordingBackend) -> Vec<(RequestId, InlayHint)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::Lsp(LspCommand::ResolveInlayHint { hint, .. }) => {
                        Some((*id, hint.clone()))
                    },
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The active tab's file and caret, once a tab the follow opened has filled.
fn landed(app: &mut App) -> (Option<PathBuf>, LineCol) {
    fill_reserved_tabs(app);
    let tab = &app.tabs[app.active];
    (
        tab.path().map(std::path::Path::to_path_buf),
        tab.editor.cursor(),
    )
}

#[test]
fn each_label_part_is_its_own_hit_region_in_paint_order() {
    let (_, app) = painted_app(split_hint());
    let (Some(colon), Some(ty)) = (part_hit(&app, 0), part_hit(&app, 1)) else {
        panic!("both parts are on screen: {:?}", app.inlay_ui.hits);
    };
    assert_eq!(colon.doc, DOC);
    assert_eq!((colon.rect.width, ty.rect.width), (2, 3));
    assert_eq!(ty.rect.x, colon.rect.right(), "the parts sit side by side");
    assert_eq!(ty.rect.y, colon.rect.y);
}

#[test]
fn resting_on_a_part_shows_its_tooltip_and_leaving_hides_it() {
    let hint = InlayHint {
        tooltip: Some(plain("the whole hint")),
        parts: vec![
            part(": "),
            InlayHintLabelPart {
                tooltip: Some(plain("struct Vec<T>")),
                ..part("Vec")
            },
        ],
        ..split_hint()
    };
    let (backend, mut app) = painted_app(hint);
    let ty = part_hit(&app, 1).expect("painted");
    rest_on(&mut app, ty);
    let tooltip = app.inlay_ui.tooltip.clone().expect("a tooltip");
    assert_eq!(tooltip.markup, plain("struct Vec<T>"));
    assert_eq!(tooltip.anchor, ty.rect);
    let screen = screen(&mut app, 100, 12).join("\n");
    assert!(screen.contains("struct Vec<T>"), "drawn:\n{screen}");

    let colon = part_hit(&app, 0).expect("painted");
    rest_on(&mut app, colon);
    assert_eq!(
        app.inlay_ui.tooltip.as_ref().map(|t| t.markup.clone()),
        Some(plain("the whole hint")),
        "a part without its own tooltip shows the hint's"
    );

    app.handle_mouse(mouse(MouseEventKind::Moved, (0, 11), KeyModifiers::NONE));
    assert_eq!(app.inlay_ui.tooltip, None);
    assert!(
        resolve_requests(&backend).is_empty(),
        "nothing was deferred, so nothing is resolved"
    );
}

#[test]
fn a_deferred_tooltip_is_resolved_on_hover_and_shown_when_it_arrives() {
    let hint = InlayHint {
        payload: Some("{\"data\":1}".to_owned()),
        ..split_hint()
    };
    let (backend, mut app) = painted_app(hint.clone());
    let ty = part_hit(&app, 1).expect("painted");
    rest_on(&mut app, ty);
    assert_eq!(app.inlay_ui.tooltip, None);
    // Moving within the same part asks once, not once per motion.
    rest_on(&mut app, ty);
    let requests = resolve_requests(&backend);
    assert_eq!(requests.len(), 1);
    let (id, asked) = requests[0].clone();
    assert_eq!(asked, hint, "the hint goes out with its payload");

    let mut resolved = hint;
    resolved.parts[1].tooltip = Some(plain("struct Vec<T>"));
    app.on_backend_event(
        Some(id),
        SessionEvent::InlayHintResolved {
            doc: DOC,
            hint: resolved,
        },
    );
    assert_eq!(
        app.inlay_ui.tooltip.as_ref().map(|t| t.markup.clone()),
        Some(plain("struct Vec<T>"))
    );
    assert_eq!(
        app.docs.inlay_hints[&DOC][0].payload, None,
        "a resolved hint is not resolved again"
    );
}

#[test]
fn a_late_resolve_for_a_hint_no_longer_hovered_shows_nothing() {
    let hint = InlayHint {
        payload: Some("{\"data\":1}".to_owned()),
        ..split_hint()
    };
    let (backend, mut app) = painted_app(hint.clone());
    let ty = part_hit(&app, 1).expect("painted");
    rest_on(&mut app, ty);
    let (id, _) = resolve_requests(&backend)[0].clone();
    app.handle_mouse(mouse(MouseEventKind::Moved, (0, 11), KeyModifiers::NONE));
    let resolved = InlayHint {
        tooltip: Some(plain("late")),
        ..hint
    };
    app.on_backend_event(
        Some(id),
        SessionEvent::InlayHintResolved {
            doc: DOC,
            hint: resolved,
        },
    );
    assert_eq!(app.inlay_ui.tooltip, None);
    // Still adopted: the next hover needs no round trip.
    assert_eq!(app.docs.inlay_hints[&DOC][0].tooltip, Some(plain("late")));
}

#[test]
fn ctrl_click_on_a_part_with_a_location_goes_there() {
    let vec = target("inlay-follow");
    let hint = InlayHint {
        parts: vec![
            part(": "),
            InlayHintLabelPart {
                location: Some(at(&vec, 1, 4)),
                ..part("Vec")
            },
        ],
        ..split_hint()
    };
    let (backend, mut app) = painted_app(hint);
    let ty = part_hit(&app, 1).expect("painted");
    ctrl_click(&mut app, ty);
    assert_eq!(landed(&mut app), (Some(vec.clone()), LineCol::new(1, 4)));
    assert!(resolve_requests(&backend).is_empty());
    let _ = std::fs::remove_dir_all(vec.parent().unwrap_or(&vec));
}

#[test]
fn ctrl_click_on_a_deferred_part_resolves_then_follows() {
    let hint = InlayHint {
        payload: Some("{\"data\":2}".to_owned()),
        ..split_hint()
    };
    let vec = target("inlay-resolve-follow");
    let (backend, mut app) = painted_app(hint.clone());
    let before = landed(&mut app);
    let ty = part_hit(&app, 1).expect("painted");
    ctrl_click(&mut app, ty);
    assert_eq!(landed(&mut app), before, "nothing to follow yet");
    let (id, _) = resolve_requests(&backend)
        .last()
        .cloned()
        .expect("a resolve request");

    let mut resolved = hint;
    resolved.parts[1].location = Some(at(&vec, 1, 2));
    app.on_backend_event(
        Some(id),
        SessionEvent::InlayHintResolved {
            doc: DOC,
            hint: resolved,
        },
    );
    assert_eq!(landed(&mut app), (Some(vec.clone()), LineCol::new(1, 2)));
    let _ = std::fs::remove_dir_all(vec.parent().unwrap_or(&vec));
}

/// A Ctrl+click whose answer is still in flight is followed even when the
/// pointer moves on to another deferred hint, whose hover asks too.
///
/// Falsified by: the hover's resolve replacing the pending follow, so the
/// click's answer no longer matches and is dropped.
#[test]
fn a_pending_follow_survives_hovering_another_deferred_hint() {
    let first = InlayHint {
        payload: Some("{\"data\":3}".to_owned()),
        ..split_hint()
    };
    let second = InlayHint {
        payload: Some("{\"data\":4}".to_owned()),
        ..InlayHint {
            position: LineCol::new(1, 5),
            ..split_hint()
        }
    };
    let vec = target("inlay-follow-then-hover");
    let (backend, mut app) = completion_app("let v\nlet w\nlet x\n", LineCol::new(2, 0));
    app.docs
        .inlay_hints
        .insert(DOC, vec![first.clone(), second]);
    frame(&mut app, 100, 12);
    let ty = part_hit(&app, 1).expect("painted");
    let other = app
        .inlay_ui
        .hits
        .iter()
        .copied()
        .find(|hit| hit.hint == 1 && hit.part == 1)
        .expect("painted");

    ctrl_click(&mut app, ty);
    let (follow_id, _) = resolve_requests(&backend)
        .last()
        .cloned()
        .expect("a resolve request");
    rest_on(&mut app, other);
    assert_eq!(
        resolve_requests(&backend).len(),
        2,
        "the hover asks about its own hint"
    );

    let mut resolved = first;
    resolved.parts[1].location = Some(at(&vec, 2, 3));
    app.on_backend_event(
        Some(follow_id),
        SessionEvent::InlayHintResolved {
            doc: DOC,
            hint: resolved,
        },
    );
    assert_eq!(landed(&mut app), (Some(vec.clone()), LineCol::new(2, 3)));
    let _ = std::fs::remove_dir_all(vec.parent().unwrap_or(&vec));
}

/// A plain click, or a Ctrl+click on a part that leads nowhere, is the
/// editor's: the hint does not swallow it.
#[test]
fn a_click_a_hint_cannot_serve_falls_through_to_the_editor() {
    let (_, mut app) = painted_app(split_hint());
    let ty = part_hit(&app, 1).expect("painted");
    let plain_click = mouse(
        MouseEventKind::Down(MouseButton::Left),
        (ty.rect.x, ty.rect.y),
        KeyModifiers::NONE,
    );
    assert!(!app.handle_inlay_hint_mouse(&plain_click));
    let ctrl = MouseEvent {
        modifiers: KeyModifiers::CONTROL,
        ..plain_click
    };
    assert!(!app.handle_inlay_hint_mouse(&ctrl));
}

fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    app.handle_key(KeyEvent::new(code, modifiers));
}

/// The bound chord, Ctrl+K Ctrl+T.
fn toggle_key(app: &mut App) {
    press(app, KeyCode::Char('k'), KeyModifiers::CONTROL);
    press(app, KeyCode::Char('t'), KeyModifiers::CONTROL);
}

#[test]
fn the_toggle_key_hides_hints_until_pressed_again() {
    let (_, mut app) = painted_app(split_hint());
    assert!(app.inlay_hints_visible());
    toggle_key(&mut app);
    assert!(!app.inlay_hints_visible());
    assert!(
        app.docs.inlay_hints.is_empty(),
        "hidden hints are dropped, not left painted"
    );
    press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    assert!(
        !app.inlay_hints_visible(),
        "a plain toggle outlasts other keys"
    );
    toggle_key(&mut app);
    assert!(app.inlay_hints_visible());
}

#[test]
fn the_palette_offers_toggle_inlay_hints() {
    assert_eq!(Command::ToggleInlayHints.label(), "Toggle Inlay Hints");
    assert_eq!(
        crate::command::resolve_named("Toggle Inlay Hints"),
        Ok(Command::ToggleInlayHints)
    );
}

/// `onUnlessPressed`: the toggle key hides them, and the next key brings
/// them back -- a terminal's stand-in for "while held".
#[test]
fn an_unless_pressed_mode_flips_only_until_the_next_key() {
    let (_, mut app) = painted_app(split_hint());
    app.settings.editor.inlay_hints.enabled = InlayHintsEnabled::OnUnlessPressed;
    toggle_key(&mut app);
    assert!(
        !app.inlay_hints_visible(),
        "the key that toggled keeps its flip"
    );
    press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    assert!(app.inlay_hints_visible());

    app.settings.editor.inlay_hints.enabled = InlayHintsEnabled::OffUnlessPressed;
    app.apply_inlay_visibility();
    assert!(!app.inlay_hints_visible());
    toggle_key(&mut app);
    assert!(app.inlay_hints_visible());
    press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    assert!(!app.inlay_hints_visible());
}

/// Changing the setting is the user's latest word and ends a toggle made
/// against the old one.
#[test]
fn a_changed_setting_ends_the_toggle() {
    let (_, mut app) = painted_app(split_hint());
    toggle_key(&mut app);
    assert!(!app.inlay_hints_visible());
    let mut settings = app.settings.clone();
    settings.editor.inlay_hints.enabled = InlayHintsEnabled::OnUnlessPressed;
    app.on_backend_event(
        None,
        SessionEvent::ConfigChanged {
            report: Box::new(karet_session::LoadedConfig::from_settings(settings)),
        },
    );
    assert!(app.inlay_hints_visible());
}

#[test]
fn each_kind_paints_in_its_own_color() {
    let (_, mut app) = completion_app("f(a)\nlet v\n", LineCol::new(1, 0));
    app.docs.inlay_hints.insert(
        DOC,
        vec![
            InlayHint::new(LineCol::new(0, 2), "x:", InlayHintKind::Parameter),
            InlayHint::new(LineCol::new(1, 5), ": u8", InlayHintKind::Type),
        ],
    );
    let painted = frame(&mut app, 100, 12);
    let fg = |hit: &InlayHintHit| painted[(hit.rect.x, hit.rect.y)].fg;
    let hits = app.inlay_ui.hits.clone();
    let param = hits.iter().find(|hit| hit.hint == 0).expect("painted");
    let ty = hits.iter().find(|hit| hit.hint == 1).expect("painted");
    let theme = &app.theme;
    assert_eq!(
        fg(param),
        theme
            .role(karet_core::ThemeRole::InlayHintParameter)
            .to_ratatui()
    );
    assert_eq!(
        fg(ty),
        theme
            .role(karet_core::ThemeRole::InlayHintType)
            .to_ratatui()
    );
}
