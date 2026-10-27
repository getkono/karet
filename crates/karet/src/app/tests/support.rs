pub(crate) use std::collections::BTreeSet;
pub(crate) use std::path::Path;

// The shared test vocabulary: every sibling globs this module, so the types
// its cases name without importing them are re-exported here, each from the
// module that owns it, rather than leaking through `app.rs`.
pub(crate) use crossterm::event::Event;
pub(crate) use crossterm::event::KeyCode;
pub(crate) use crossterm::event::KeyEvent;
pub(crate) use crossterm::event::MouseButton;
pub(crate) use karet_core::BlameAttribution;
pub(crate) use karet_core::Change;
pub(crate) use karet_core::Decoration;
pub(crate) use karet_core::DecorationKind;
pub(crate) use karet_core::Diagnostic;
pub(crate) use karet_core::LineCol;
pub(crate) use karet_core::Notification;
pub(crate) use karet_core::NotificationId;
pub(crate) use karet_core::Range;
pub(crate) use karet_core::Severity;
pub(crate) use karet_core::TextEdit;
pub(crate) use karet_core::ThemeRole;
pub(crate) use karet_editor::editing;
pub(crate) use karet_filetype::FileKind;
pub(crate) use karet_fileview::image::GraphicsProtocol;
pub(crate) use karet_session::BackendError;
pub(crate) use karet_session::ChangeSummary;
pub(crate) use karet_session::Command as SessionCommand;
pub(crate) use karet_session::DebugCommand;
pub(crate) use karet_session::DocSnapshot;
pub(crate) use karet_session::DocumentCommand;
pub(crate) use karet_session::DocumentSettings;
pub(crate) use karet_session::Event as SessionEvent;
pub(crate) use karet_session::EventRx;
pub(crate) use karet_session::GithubCommand;
use karet_session::GithubIssue;
use karet_session::GithubRepository;
pub(crate) use karet_session::GithubVerification;
pub(crate) use karet_session::LanguageServerChange;
pub(crate) use karet_session::LanguageServerCommand;
pub(crate) use karet_session::LanguageServerId;
pub(crate) use karet_session::LanguageServerMessageId;
pub(crate) use karet_session::LanguageServerPlanId;
pub(crate) use karet_session::LanguageServerRuntimeState;
pub(crate) use karet_session::LanguageServerStatus;
pub(crate) use karet_session::LocationKind;
pub(crate) use karet_session::LspCommand;
pub(crate) use karet_session::PreparedChange;
pub(crate) use karet_session::RangeSpec;
pub(crate) use karet_session::RepositorySnapshot;
pub(crate) use karet_session::SaveCause;
pub(crate) use karet_session::SessionConfig;
pub(crate) use karet_session::VcsCommand;
pub(crate) use karet_session::VcsOutcome;
pub(crate) use karet_session::config::schema::AutoSave;
pub(crate) use karet_session::local;
pub(crate) use karet_text::EditCause;
pub(crate) use karet_text::TextBuffer;
pub(crate) use karet_vcs::Commit;
pub(crate) use karet_vcs::CommitDetail;
pub(crate) use karet_vcs::StatusKind;
pub(crate) use karet_widgets::DropZone;
pub(crate) use karet_widgets::SplitDir;
pub(crate) use karet_widgets::breadcrumbs::BreadcrumbHit;
pub(crate) use karet_widgets::breadcrumbs::BreadcrumbTarget;

pub(crate) use crate::app::confirm::ConfirmAction;
pub(crate) use crate::app::hit::MarkdownLinkHit;
pub(crate) use crate::app::hit::PaneFrame;
pub(crate) use crate::app::hit::ScrollHit;
pub(crate) use crate::app::state::ContextMenuAction;
pub(crate) use crate::app::state::ContextMenuEntry;
pub(crate) use crate::app::state::DictionaryTarget;
pub(crate) use crate::app::state::SearchOption;
use crate::app::*;
pub(crate) use crate::keymap::EditorTab;
pub(crate) use crate::keymap::Focus;
pub(crate) use crate::keymap::FocusTarget;
pub(crate) use crate::keymap::Modal;
pub(crate) use crate::overlay::Overlay;
pub(crate) use crate::render::FileView;
pub(crate) use crate::render::Section;
pub(crate) use crate::tab::CommitFiles;
pub(crate) use crate::tab::CommitViewState;
pub(crate) use crate::tab::PagerState;
pub(crate) use crate::tab::SearchField;

pub(crate) fn change(path: &str, status: StatusKind) -> ChangeSummary {
    ChangeSummary {
        path: PathBuf::from(path),
        old_path: None,
        status,
        is_binary: false,
        added: 1,
        removed: 0,
    }
}

/// A backend-prepared change as the session would deliver it (plaintext).
pub(crate) fn prepared_change(path: &str, status: StatusKind) -> karet_session::PreparedChange {
    prepared_from_texts(path, status, "", "x\n")
}

/// A backend-prepared change diffing two explicit texts (plaintext).
pub(crate) fn prepared_from_texts(
    path: &str,
    status: StatusKind,
    old: &str,
    new: &str,
) -> karet_session::PreparedChange {
    let diff = karet_diff::diff_text(
        old,
        new,
        &karet_diff::DiffOptions {
            path_hint: Some(path.to_string()),
            ..Default::default()
        },
    );
    karet_session::PreparedChange {
        path: PathBuf::from(path),
        old_path: None,
        status,
        language: "plaintext".to_string(),
        diff: karet_diff::PreparedDiff::new(diff, Vec::new(), Vec::new()),
    }
}

pub(crate) fn app() -> App {
    App::new(
        PathBuf::from("."),
        vec![change("a.rs", StatusKind::Modified)],
        vec![change("b.rs", StatusKind::Modified)],
        false,
    )
}

/// The newest active notification's title, the successor to the old status line.
///
/// `NotificationCenter::active` is newest-first, so this is whatever the action
/// under test just said.
pub(crate) fn last_message(app: &App) -> Option<String> {
    app.notifications
        .active()
        .first()
        .map(|note| note.title.clone())
}

/// The newest active notification's severity and title together, for the tests
/// that care that a refusal is not reported as a success.
pub(crate) fn last_report(app: &App) -> Option<(Severity, String)> {
    app.notifications
        .active()
        .first()
        .map(|note| (note.severity, note.title.clone()))
}

pub(crate) fn test_dir(name: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!("karet-{name}-{}-{unique}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub(crate) fn write_file(root: &Path, rel: &str, contents: &[u8]) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, contents);
}

pub(crate) fn select_explorer_path(app: &mut App, path: &Path) {
    app.explorer.ensure_built(&app.root);
    let Some(idx) = app.explorer.rows().iter().position(|row| row.path == path) else {
        panic!("missing explorer path {}", path.display());
    };
    app.explorer.select_visible(idx);
}

pub(crate) fn refresh_count(backend: &RecordingBackend) -> usize {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter(|(_, command)| matches!(command, SessionCommand::Vcs(VcsCommand::Refresh)))
                .count()
        })
        .unwrap_or_default()
}

pub(crate) fn retarget_commands(backend: &RecordingBackend) -> Vec<(DocumentId, PathBuf)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(_, command)| match command {
                    SessionCommand::Document(DocumentCommand::Retarget { doc, path }) => {
                        Some((*doc, path.clone()))
                    },
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn blame_commands(backend: &RecordingBackend) -> Vec<(RequestId, DocumentId, u64, u32)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::Vcs(VcsCommand::Blame { doc, version, line }) => {
                        Some((*id, *doc, *version, *line))
                    },
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) struct RecordingBackend {
    pub(crate) next: std::sync::atomic::AtomicU64,
    pub(crate) sent: std::sync::Mutex<Vec<(RequestId, SessionCommand)>>,
}

impl RecordingBackend {
    pub(crate) fn new() -> Self {
        Self {
            next: std::sync::atomic::AtomicU64::new(1),
            sent: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl Backend for RecordingBackend {
    fn send(&self, id: RequestId, command: SessionCommand) -> Result<(), BackendError> {
        if let Ok(mut sent) = self.sent.lock() {
            sent.push((id, command));
        }
        Ok(())
    }

    fn next_id(&self) -> RequestId {
        RequestId(self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }

    fn take_events(&self) -> Option<karet_session::EventRx> {
        // The recording backend answers nothing; tests feed events manually.
        None
    }
}

/// Draw the whole shell into a test terminal and return the painted cells, so a
/// test can assert on styling rather than only on the glyphs.
pub(crate) fn frame(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    app.prepare_frame(Rect::new(0, 0, width, height));
    terminal
        .draw(|f| crate::ui::draw(f, app))
        .expect("draw the shell");
    terminal.backend().buffer().clone()
}

/// Draw the whole shell into a test terminal and return the screen, row by row.
pub(crate) fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
    let buffer = frame(app, width, height);
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
        })
        .collect()
}

/// A single focused-pane frame whose content covers `rect`, so editor-click
/// tests route through the pane hit-testing.
pub(crate) fn content_frame(app: &App, rect: Rect) -> PaneFrame {
    PaneFrame {
        content_rect: rect,
        editor_rect: rect,
        ..PaneFrame::empty(app.focus_pane())
    }
}

/// The registry file type named `language` (a protocol id, config selector or
/// display name), or the unrecognized fallback when there is none.
pub(crate) fn language_type(language: &str) -> karet_filetype::FileType {
    karet_filetype::file_type_for_language(language)
        .unwrap_or_else(|| karet_filetype::file_type_for_path(Path::new("")))
}

/// A code tab over `name` holding `text`, typed by its path.
pub(crate) fn text_tab(name: &str, text: &str) -> Tab {
    use karet_syntax::Highlights;
    use karet_text::TextBuffer;
    Tab::new(
        name,
        TabKind::Code {
            path: PathBuf::from(name),
            file_type: karet_filetype::file_type_for_path(std::path::Path::new(name)),
            doc: None,
            next_version: 0,
            buffer: TextBuffer::from_text(text).into(),
            text: text.into(),
            highlights: Highlights::default(),
            semantic_blocks: karet_syntax::SemanticBlocks::default(),
            folds: FoldRegions::default(),
            folded: BTreeSet::new(),
            decos: Vec::new(),
            search_decos: Vec::new(),
            syntax_errors: Vec::new(),
        },
    )
}

pub(crate) fn code_tab(name: &str) -> Tab {
    use karet_syntax::Highlights;
    use karet_text::TextBuffer;
    Tab::new(
        name,
        TabKind::Code {
            path: PathBuf::from(name),
            file_type: karet_filetype::file_type_for_path(std::path::Path::new(name)),
            doc: None,
            next_version: 0,
            buffer: TextBuffer::from_text("x\n").into(),
            text: "x\n".into(),
            highlights: Highlights::default(),
            semantic_blocks: karet_syntax::SemanticBlocks::default(),
            folds: FoldRegions::default(),
            folded: BTreeSet::new(),
            decos: Vec::new(),
            search_decos: Vec::new(),
            syntax_errors: Vec::new(),
        },
    )
}

/// A temp directory removed on drop, so a panicking test can't leak it.
pub(crate) struct TempRepo {
    pub(crate) path: PathBuf,
}

// These drive the real `Session` + `local()` backend over a temp git repo, so
// they exercise the whole key → focus/layer → dispatch → backend actor → git2 →
// VcsStatus → apply loop that unit tests skip.

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A `git` command in `dir` that sees none of the developer's own setup.
///
/// The global and system config files are hidden, so a signing key, a hooks
/// directory or a default-branch preference on the machine running the tests
/// cannot change what a fixture builds, and the overrides pin the two settings
/// that would otherwise make a fixture commit fail or prompt.
fn isolated_git(dir: &Path, args: &[&str]) -> std::process::Command {
    let mut cmd = std::process::Command::new("git");
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .current_dir(dir);
    cmd
}

/// Whether a `git` binary can be run at all.
fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

/// Run an isolated `git` in `dir`, panicking with its stderr when it fails.
///
/// Fixtures call this only once [`init_test_repo`] has found `git`, so a
/// failure here is a broken fixture, never a missing tool: it must fail the
/// test rather than let it return early and pass.
pub(crate) fn git(dir: &Path, args: &[&str]) {
    let out = isolated_git(dir, args)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?} could not start: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A git repo in a fresh temp dir holding a single untracked file, or `None`
/// when `git` is unavailable (so the test skips rather than fails).
///
/// With `git` present every setup step must succeed, or the test panics. The
/// repository's own config also pins signing and hooks off, which overrides
/// the developer's global config, so the git the backend runs for its writes
/// can neither sign nor run a hook either.
pub(crate) fn init_test_repo() -> Option<TempRepo> {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    static N: AtomicUsize = AtomicUsize::new(0);
    if !git_available() {
        return None;
    }
    let path = std::env::temp_dir().join(format!(
        "karet-scm-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).expect("create the temp repo dir");
    let repo = TempRepo { path };
    git(&repo.path, &["init", "-q"]);
    for (key, value) in [
        ("user.email", "test@example.com"),
        ("user.name", "karet test"),
        ("commit.gpgsign", "false"),
        ("tag.gpgsign", "false"),
        ("core.hooksPath", "/dev/null"),
    ] {
        git(&repo.path, &["config", key, value]);
    }
    std::fs::write(repo.path.join("new.rs"), "fn main() {}\n").expect("write the untracked file");
    Some(repo)
}

pub(crate) fn code_tab_text(app: &App) -> String {
    match &app.tabs[app.active].kind {
        TabKind::Code { text, .. } => text.as_str().to_owned(),
        _ => panic!("expected the active tab to be a code tab"),
    }
}

/// A focused editor over `text` (doc 9) wired to a recording backend, with
/// the caret at `caret`.
pub(crate) fn completion_app(text: &str, caret: LineCol) -> (Arc<RecordingBackend>, App) {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.push_tab(text_tab("main.rs", text));
    app.keyboard.focus_editor();
    let idx = app.active;
    if let TabKind::Code { doc, .. } = &mut app.tabs[idx].kind {
        *doc = Some(DocumentId(9));
    }
    app.tabs[idx].editor.set_carets(&[caret]);
    (backend, app)
}

/// The completion requests a backend received, as `(id, position)`.
pub(crate) fn completion_requests(backend: &RecordingBackend) -> Vec<(RequestId, LineCol)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::Lsp(LspCommand::Completion { position, .. }) => {
                        Some((*id, *position))
                    },
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn commit_detail(hash: &str, summary: &str) -> CommitDetail {
    let id = karet_vcs::Identity {
        name: "Tester".to_string(),
        email: "t@example.com".to_string(),
        time: 0,
        offset: 0,
    };
    CommitDetail {
        hash: hash.to_string(),
        summary: summary.to_string(),
        body: String::new(),
        author: id.clone(),
        committer: id,
        parents: Vec::new(),
        signature: None,
    }
}

pub(crate) fn send_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    app.handle_key(KeyEvent::new(code, mods));
}

/// The documents a backend was asked to save, in order.
pub(crate) fn saved_docs(backend: &RecordingBackend) -> Vec<DocumentId> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(_, command)| match command {
                    SessionCommand::Document(DocumentCommand::Save { doc, .. }) => Some(*doc),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn repository() -> GithubRepository {
    GithubRepository {
        owner: "getkono".to_string(),
        repo: "karet".to_string(),
    }
}

pub(crate) fn issue(number: u64) -> GithubIssue {
    GithubIssue {
        number,
        title: format!("Issue {number}"),
        body: Some("description".to_string()),
        state: "open".to_string(),
        creator: Some("octocat".to_string()),
        creator_id: Some(1),
        created_unix: 1,
        updated_unix: 2,
        labels: Vec::new(),
        blocked: false,
        html_url: format!("https://github.com/getkono/karet/issues/{number}"),
    }
}

pub(crate) fn responsive_commit_files() -> CommitFiles {
    CommitFiles::ready(
        ["src/first.rs", "src/second.rs", "src/third.rs"]
            .into_iter()
            .map(|path| FileView::new(prepared_change(path, StatusKind::Modified)))
            .collect(),
    )
}

pub(crate) fn responsive_commit_app() -> App {
    let mut app = app();
    app.sidebar_visible = false;
    app.keyboard.focus_editor();
    app.push_tab(Tab::commit(
        Box::new(commit_detail(&"a".repeat(40), "responsive commit")),
        responsive_commit_files(),
    ));
    app
}

pub(crate) fn commit(hash: &str, summary: &str) -> Commit {
    Commit {
        hash: hash.to_string(),
        summary: summary.to_string(),
        author: "T".to_string(),
        time: 0,
        parents: Vec::new(),
    }
}

/// Drain backend events into `app` until `ready` holds, panicking if it still
/// does not after ten seconds or once the backend has hung up.
///
/// Waiting on the state the test actually needs, rather than stopping after a
/// quiet gap, is what keeps a loaded parallel run from asserting against a
/// half-filled app. The deadline only bounds a broken test; a passing one
/// returns as soon as its answer lands. Panicking there, instead of returning,
/// keeps a condition that never comes true from reaching an assertion that
/// happens to hold on the unfilled app.
pub(crate) async fn pump_until(app: &mut App, events: &mut EventRx, ready: impl Fn(&App) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !ready(app) {
        assert!(
            std::time::Instant::now() < deadline,
            "the awaited state never arrived within 10s"
        );
        match tokio::time::timeout(std::time::Duration::from_millis(250), events.recv()).await {
            Ok(Some((id, ev))) => app.on_backend_event(id, ev),
            Ok(None) => panic!("the backend closed its event stream before the awaited state"),
            Err(_) => {},
        }
    }
}

/// [`pump_until`] over both of a local backend's streams: events and the
/// document snapshots that carry state such as the dirty flag.
///
/// Events go first, as in the app's own loop: the session sends a document's
/// `Opened` before its first snapshot, and only the bound tab can take that
/// snapshot, so reading the two in either order would drop the text a newly
/// opened tab is waiting for.
pub(crate) async fn pump_with_snapshots_until(
    app: &mut App,
    events: &mut EventRx,
    snaps: &mut karet_session::SnapshotRx,
    ready: impl Fn(&App) -> bool,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !ready(app) {
        assert!(
            std::time::Instant::now() < deadline,
            "the awaited state never arrived within 10s"
        );
        tokio::select! {
            biased;
            event = events.recv() => {
                let (id, ev) = event.expect("the backend closed its event stream");
                app.on_backend_event(id, ev);
            },
            snap = snaps.recv() => {
                let (doc, snap) = snap.expect("the backend closed its snapshot stream");
                app.on_snapshot(doc, &snap);
            },
            () = tokio::time::sleep(std::time::Duration::from_millis(250)) => {},
        }
    }
}

/// Whether the active tab is a code tab the backend has opened a document for.
pub(crate) fn active_doc_open(app: &App) -> bool {
    matches!(
        app.tabs.get(app.active).map(|tab| &tab.kind),
        Some(TabKind::Code { doc: Some(_), .. })
    )
}

/// Whether the active tab is a code tab holding its document's text: bound to
/// a document and filled by the session's first snapshot.
pub(crate) fn active_doc_filled(app: &App) -> bool {
    active_doc_open(app)
        && app
            .tabs
            .get(app.active)
            .is_some_and(|tab| tab.fill_caret.is_none())
}

/// A clean snapshot of `buffer` at its own version, with no syntax yet.
pub(crate) fn buffer_snapshot(buffer: TextBuffer) -> DocSnapshot {
    DocSnapshot {
        version: buffer.version(),
        buffer,
        highlights: Arc::new(karet_syntax::Highlights::default()),
        semantic_blocks: Arc::new(karet_syntax::SemanticBlocks::default()),
        folds: Arc::new(FoldRegions::default()),
        syntax_error_lines: Arc::new(Vec::new()),
        table_lines: Arc::default(),
        language: None,
        dirty: false,
        cursor: None,
    }
}

/// Stand in for the session's first snapshot on every reserved code tab, for a
/// test app whose backend never publishes one: load each file from disk and
/// place the caret where the open asked, as the real first fill does.
pub(crate) fn fill_reserved_tabs(app: &mut App) {
    for tab in app.all_tabs_mut() {
        let Some(position) = tab.fill_caret else {
            continue;
        };
        let TabKind::Code {
            path, buffer, text, ..
        } = &mut tab.kind
        else {
            continue;
        };
        let bytes = std::fs::read(&*path).unwrap_or_default();
        let Ok(loaded) = TextBuffer::from_bytes(&bytes) else {
            continue;
        };
        *text = loaded.text_snapshot();
        *buffer = Arc::new(loaded);
        tab.fill_caret = None;
        tab.fill_since = None;
        crate::tab::place_fill(&mut tab.editor, buffer, position, tab.fill_view.take());
    }
}

/// Answer the manager tab's outstanding inventory request, the way the session
/// does.
///
/// The session only ever sends an inventory tagged with the request that asked
/// for it; an untagged payload is no longer part of the vocabulary. A test that
/// passes `None` while the tab has a request out is therefore describing
/// something that cannot happen, and the client now refuses it -- so seeding the
/// view means answering what it actually asked.
pub(crate) fn answer_inventory(app: &mut App, servers: Vec<LanguageServerStatus>) {
    let pending = app.all_tabs().find_map(|tab| match &tab.kind {
        TabKind::LanguageServers(view) => Some(view.inventory_request),
        _ => None,
    });
    app.show_language_server_status(pending.flatten(), servers);
}

/// A provider status for `language`, resolved and running at `/workspace`.
///
/// Shared rather than per-module: the manager-tab, badge, and scrollbar suites all
/// want the same shape, and two copies of it had already drifted apart.
pub(crate) fn language_server_status(
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
            args: vec!["--stdio".to_string()],
            runtime: karet_session::LanguageServerRuntimeState::Running,
            open_documents: 2,
            error: None,
        }],
    }
}
