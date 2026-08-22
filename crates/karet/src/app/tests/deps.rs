//! The dependency-manifest commands: what they rewrite, and the stale-buffer
//! guard that stops them rewriting the wrong span.

use karet_session::ManifestHint;
use karet_session::ManifestHintState;

use super::support::*;
use crate::app::*;

const MANIFEST: &str = "[dependencies]\nserde = \"1.0.100\"\ntime = \"0.1.44\"\nregex = \"1\"\n";

/// A hint over the version value on `line`, spanning `cols` (quotes excluded).
fn hint(
    name: &str,
    line: u32,
    cols: (u32, u32),
    current: &str,
    latest: Option<&str>,
    state: ManifestHintState,
) -> ManifestHint {
    ManifestHint {
        name: name.to_owned(),
        line,
        col_start: cols.0,
        col_end: cols.1,
        current: current.to_owned(),
        latest: latest.map(str::to_owned),
        state,
        vulnerabilities: Vec::new(),
    }
}

/// An app over `MANIFEST` with `hints` attached at the buffer's current version.
fn manifest_app(hints: Vec<ManifestHint>, caret: LineCol) -> App {
    let mut app = app();
    app.backend = Some(Arc::new(RecordingBackend::new()));
    let mut tab = text_tab("Cargo.toml", MANIFEST);
    if let TabKind::Code {
        path,
        doc,
        language,
        ..
    } = &mut tab.kind
    {
        *path = std::path::PathBuf::from("Cargo.toml");
        *language = "TOML";
        *doc = Some(DocumentId(9));
    }
    app.push_tab(tab);
    app.focus = Focus::Editor;
    let idx = app.active;
    let version = match &app.tabs[idx].kind {
        TabKind::Code { buffer, .. } => buffer.version(),
        _ => 0,
    };
    app.tabs[idx].editor.set_carets(&[caret]);
    app.docs
        .manifest_hints
        .insert(DocumentId(9), (version, hints));
    app
}

/// `serde = "1.0.100"` — the value spans columns 9..16.
fn serde_hint(state: ManifestHintState) -> ManifestHint {
    hint("serde", 1, (9, 16), "1.0.100", Some("1.0.219"), state)
}

/// `time = "0.1.44"` — the value spans columns 8..14.
fn time_hint(state: ManifestHintState) -> ManifestHint {
    hint("time", 2, (8, 14), "0.1.44", Some("0.3.55"), state)
}

#[test]
fn update_at_caret_rewrites_only_that_line_s_version() {
    let mut app = manifest_app(
        vec![
            serde_hint(ManifestHintState::Outdated),
            time_hint(ManifestHintState::Vulnerable),
        ],
        LineCol::new(2, 0),
    );
    app.dispatch(Command::DepsUpdate);

    assert_eq!(
        code_tab_text(&app),
        "[dependencies]\nserde = \"1.0.100\"\ntime = \"0.3.55\"\nregex = \"1\"\n"
    );
    assert_eq!(app.status.as_deref(), Some("time → 0.3.55"));
}

#[test]
fn update_at_caret_on_a_current_dependency_says_so() {
    let mut app = manifest_app(
        vec![serde_hint(ManifestHintState::UpToDate)],
        LineCol::new(1, 0),
    );
    app.dispatch(Command::DepsUpdate);

    assert_eq!(code_tab_text(&app), MANIFEST);
    assert_eq!(
        app.status.as_deref(),
        Some("no update available on this line")
    );
}

#[test]
fn update_all_rewrites_every_stale_version_in_one_edit() {
    let mut app = manifest_app(
        vec![
            serde_hint(ManifestHintState::Outdated),
            time_hint(ManifestHintState::Vulnerable),
        ],
        LineCol::new(0, 0),
    );
    app.dispatch(Command::DepsUpdateAll);

    assert_eq!(
        code_tab_text(&app),
        "[dependencies]\nserde = \"1.0.219\"\ntime = \"0.3.55\"\nregex = \"1\"\n",
        "both spans move, and the later one is not shifted by the earlier"
    );
    assert_eq!(app.status.as_deref(), Some("updated 2 dependencies"));
}

#[test]
fn update_all_with_nothing_stale_leaves_the_manifest_alone() {
    let mut app = manifest_app(
        vec![serde_hint(ManifestHintState::UpToDate)],
        LineCol::new(0, 0),
    );
    app.dispatch(Command::DepsUpdateAll);

    assert_eq!(code_tab_text(&app), MANIFEST);
    assert_eq!(app.status.as_deref(), Some("every dependency is current"));
}

#[test]
fn one_stale_dependency_reads_as_singular() {
    let mut app = manifest_app(
        vec![time_hint(ManifestHintState::Outdated)],
        LineCol::new(0, 0),
    );
    app.dispatch(Command::DepsUpdateAll);
    assert_eq!(app.status.as_deref(), Some("updated 1 dependency"));
}

#[test]
fn hints_from_a_stale_buffer_version_are_refused() {
    // The spans were computed against an older buffer, so applying them could
    // rewrite the wrong characters entirely.
    let mut app = manifest_app(
        vec![time_hint(ManifestHintState::Outdated)],
        LineCol::new(2, 0),
    );
    app.docs
        .manifest_hints
        .entry(DocumentId(9))
        .and_modify(|(version, _)| *version = version.wrapping_add(1));

    app.dispatch(Command::DepsUpdate);

    assert_eq!(code_tab_text(&app), MANIFEST);
    assert_eq!(
        app.status.as_deref(),
        Some("no dependency hints for this tab")
    );
}
