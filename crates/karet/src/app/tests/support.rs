use karet_session::GithubIssue;
use karet_session::GithubRepository;
use karet_vcs::StatusKind;

use crate::app::*;

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
                .filter(|(_, command)| matches!(command, SessionCommand::RefreshVcs))
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
                    SessionCommand::RetargetDocument { doc, path } => Some((*doc, path.clone())),
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
                    SessionCommand::Blame { doc, version, line } => {
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
        pane: app.focus_pane(),
        tabstrip_rect: Rect::default(),
        tab_hits: Vec::new(),
        action_hits: Vec::new(),
        breadcrumb_rect: Rect::default(),
        breadcrumb_hits: Vec::new(),
        content_rect: rect,
        editor_rect: rect,
        commit_file_hits: Vec::new(),
        commit_dir_hits: Vec::new(),
        commit_rail_rect: Rect::default(),
        commit_collapse_hits: Vec::new(),
        select_regions: Vec::new(),
    }
}

pub(crate) fn text_tab(name: &str, text: &str) -> Tab {
    use karet_syntax::Highlights;
    use karet_text::TextBuffer;
    Tab::new(
        name,
        TabKind::Code {
            path: PathBuf::from(name),
            language: "Rust",
            doc: None,
            next_version: 0,
            buffer: TextBuffer::from_text(text),
            text: text.to_string(),
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
            language: "Rust",
            doc: None,
            next_version: 0,
            buffer: TextBuffer::from_text("x\n"),
            text: "x\n".to_string(),
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

/// Run `git` in `dir`, returning whether it succeeded.
pub(crate) fn git(dir: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// A git repo in a fresh temp dir holding a single untracked file, or `None`
/// when `git` is unavailable (so the test skips rather than fails).
pub(crate) fn init_test_repo() -> Option<TempRepo> {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    static N: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "karet-scm-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).ok()?;
    let repo = TempRepo { path };
    if !git(&repo.path, &["init", "-q"])
        || !git(&repo.path, &["config", "user.email", "test@example.com"])
        || !git(&repo.path, &["config", "user.name", "karet test"])
    {
        return None;
    }
    std::fs::write(repo.path.join("new.rs"), "fn main() {}\n").ok()?;
    Some(repo)
}

pub(crate) fn code_tab_text(app: &App) -> String {
    match &app.tabs[app.active].kind {
        TabKind::Code { text, .. } => text.clone(),
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
    app.focus = Focus::Editor;
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
                    SessionCommand::Completion { position, .. } => Some((*id, *position)),
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
        short_hash: hash.chars().take(7).collect(),
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
                    SessionCommand::Save { doc } => Some(*doc),
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
    app.focus = Focus::Editor;
    app.push_tab(Tab::commit(
        Box::new(commit_detail(&"a".repeat(40), "responsive commit")),
        responsive_commit_files(),
    ));
    app
}

pub(crate) fn commit(hash: &str, summary: &str) -> Commit {
    Commit {
        hash: hash.to_string(),
        short_hash: hash.chars().take(7).collect(),
        summary: summary.to_string(),
        author: "T".to_string(),
        time: 0,
        parents: Vec::new(),
    }
}

/// Drain backend events into `app`, waiting briefly for the spawned actor.
pub(crate) async fn pump(app: &mut App, events: &mut EventRx) {
    while let Ok(Some((id, ev))) =
        tokio::time::timeout(std::time::Duration::from_millis(500), events.recv()).await
    {
        app.on_backend_event(id, ev);
    }
}

/// Drain backend events into `app` until `ready` holds, or five seconds pass.
///
/// [`pump`] stops after a 500 ms gap, which is a guess at "nothing more is
/// coming" — under a loaded parallel test run the answer a test is waiting for
/// can miss that window, and the assertion then runs against a half-filled
/// app. Waiting on the state the test actually needs removes the race without
/// making every other `pump` call sit through a longer timeout.
pub(crate) async fn pump_until(app: &mut App, events: &mut EventRx, ready: impl Fn(&App) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !ready(app) && std::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(250), events.recv()).await {
            Ok(Some((id, ev))) => app.on_backend_event(id, ev),
            Ok(None) => break,
            Err(_) => {},
        }
    }
}
