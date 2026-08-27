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

/// Deliver `text` as the backend's content for the tab already open at `path`.
///
/// Opening a file reserves a tab and asks the backend for the content; the
/// content arrives later, as a document snapshot. A test that needs a buffer has
/// to play the backend's part, and doing it through `on_snapshot` means it
/// exercises the same path a real session does — including the deferred caret a
/// jump-to-line records while the file is still empty.
pub(crate) fn deliver_content(app: &mut App, path: &Path, text: &str) -> DocumentId {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let doc = DocumentId(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    for tab in app.all_tabs_mut() {
        let matches = tab.path() == Some(path);
        if let TabKind::Code { doc: slot, .. } = &mut tab.kind
            && matches
            && slot.is_none()
        {
            *slot = Some(doc);
        }
    }
    let buffer = karet_text::TextBuffer::from_bytes(text.as_bytes()).unwrap_or_default();
    let snapshot = karet_session::local::DocSnapshot {
        version: buffer.version(),
        buffer,
        highlights: std::sync::Arc::default(),
        folds: std::sync::Arc::default(),
        semantic_blocks: std::sync::Arc::default(),
        decorations: std::sync::Arc::default(),
        syntax_error_lines: std::sync::Arc::default(),
        language: None,
        dirty: false,
        cursor: None,
    };
    app.on_snapshot(doc, &snapshot);
    doc
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

/// An app rooted at `root` with a recording backend attached.
///
/// A shell with no backend cannot ask for anything — every request is dropped
/// before it is made — so any test that exercises the explorer's filesystem
/// actions needs one.
pub(crate) fn app_at(root: PathBuf) -> App {
    let mut app = App::new(root, Vec::new(), Vec::new(), false);
    app.backend = Some(Arc::new(RecordingBackend::new()));
    app
}

/// Run every filesystem mutation `app` has in flight, the way the backend does.
///
/// The shell asks its backend to create, rename, copy and delete; a test with a
/// recording backend gets the request but not the effect. This performs the same
/// mutations through the session's own worker and feeds the answers back, so a
/// test exercises the full round trip rather than a stub.
pub(crate) fn settle_mutations(app: &mut App) {
    for _ in 0..64 {
        let pending: Vec<(RequestId, karet_session::api::PathMutation)> = app
            .pending_mutations
            .iter()
            .map(|(id, pending)| (*id, pending.mutation.clone()))
            .collect();
        if pending.is_empty() {
            break;
        }
        for (id, mutation) in pending {
            let result = apply_mutation(&mutation);
            app.on_backend_event(Some(id), SessionEvent::PathMutated { mutation, result });
        }
    }
}

/// Perform `mutation`, mirroring what the session's filesystem worker does.
fn apply_mutation(mutation: &karet_session::api::PathMutation) -> Result<(), String> {
    use karet_session::api::PathMutation;

    let created = |path: &Path| -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        Ok(())
    };
    let copy = |from: &Path, to: &Path| -> Result<(), String> {
        fn walk(from: &Path, to: &Path) -> Result<(), String> {
            if from.is_dir() {
                std::fs::create_dir_all(to).map_err(|error| error.to_string())?;
                for entry in std::fs::read_dir(from).map_err(|error| error.to_string())? {
                    let entry = entry.map_err(|error| error.to_string())?;
                    walk(&entry.path(), &to.join(entry.file_name()))?;
                }
                return Ok(());
            }
            std::fs::copy(from, to)
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
        if to.symlink_metadata().is_ok() {
            return Err(format!("{} already exists", to.display()));
        }
        created(to)?;
        walk(from, to)
    };
    match mutation {
        PathMutation::CreateFile { path } => {
            created(path)?;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map(|_| ())
                .map_err(|error| error.to_string())
        },
        PathMutation::CreateDirectory { path } => {
            std::fs::create_dir_all(path).map_err(|error| error.to_string())
        },
        PathMutation::Rename { from, to } => {
            if to.symlink_metadata().is_ok() {
                return Err(format!("{} already exists", to.display()));
            }
            created(to)?;
            std::fs::rename(from, to).map_err(|error| error.to_string())
        },
        PathMutation::Copy { from, to } => copy(from, to),
        PathMutation::Delete { path } => {
            let metadata = path.symlink_metadata().map_err(|error| error.to_string())?;
            if metadata.is_dir() {
                std::fs::remove_dir_all(path).map_err(|error| error.to_string())
            } else {
                std::fs::remove_file(path).map_err(|error| error.to_string())
            }
        },
        // The enum is non-exhaustive; a mutation this helper has not learned yet
        // must fail loudly rather than quietly report success.
        other => Err(format!("unhandled mutation: {other:?}")),
    }
}

/// Build `app`'s explorer, answering every listing it asks for from the local
/// disk.
///
/// The shell asks its backend for directory listings, so a test whose backend
/// records rather than answers has to play that part. Answers go back through
/// `on_directory_listed` — the same entry point a real session uses — so what is
/// exercised is the whole round trip, including a reveal completing as levels
/// arrive.
pub(crate) fn build_explorer_from_disk(app: &mut App) {
    app.build_explorer();
    // Bounded so a bug cannot hang the suite; one round resolves a whole level.
    for _ in 0..64 {
        let asked: Vec<(RequestId, PathBuf)> = app
            .pending_listings
            .iter()
            .map(|(id, path)| (*id, path.clone()))
            .collect();
        // A shell with no backend keeps its misses rather than sending them.
        let unsent = app.explorer.take_missing();
        if asked.is_empty() && unsent.is_empty() {
            break;
        }
        for dir in unsent {
            let children = list_dir(
                &dir,
                app.explorer.show_hidden(),
                app.explorer.respect_gitignore(),
            );
            app.explorer.supply(dir, children);
        }
        for (id, dir) in asked {
            let children = list_dir(
                &dir,
                app.explorer.show_hidden(),
                app.explorer.respect_gitignore(),
            );
            app.on_backend_event(
                Some(id),
                SessionEvent::DirectoryListed {
                    path: dir,
                    result: Ok(children),
                },
            );
        }
        app.build_explorer();
        app.finish_reveal();
    }
}

/// List `dir` the way the session's filesystem worker does.
fn list_dir(dir: &Path, show_hidden: bool, respect_gitignore: bool) -> Vec<karet_core::DirEntry> {
    let walk = |git_ignore: bool| -> Vec<(PathBuf, bool, bool)> {
        ignore::WalkBuilder::new(dir)
            .max_depth(Some(1))
            .standard_filters(git_ignore)
            .hidden(!show_hidden)
            .require_git(false)
            .follow_links(false)
            .filter_entry(|entry| entry.file_name() != ".git")
            .build()
            .flatten()
            .filter(|entry| entry.path() != dir)
            .map(|entry| {
                let path = entry.path().to_path_buf();
                let is_symlink = entry.path_is_symlink();
                let is_dir = path.is_dir();
                (path, is_dir, is_symlink)
            })
            .collect()
    };
    let visible: BTreeSet<PathBuf> = if respect_gitignore {
        walk(true).into_iter().map(|(path, _, _)| path).collect()
    } else {
        BTreeSet::new()
    };
    let mut entries: Vec<karet_core::DirEntry> = walk(false)
        .into_iter()
        .map(|(path, is_dir, is_symlink)| karet_core::DirEntry {
            ignored: respect_gitignore && !visible.contains(&path),
            is_repository: is_dir && path.join(".git").exists(),
            path,
            is_dir,
            is_symlink,
        })
        .collect();
    karet_core::sort_entries(&mut entries);
    entries
}

pub(crate) fn select_explorer_path(app: &mut App, path: &Path) {
    build_explorer_from_disk(app);
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

/// Draw the whole shell into a test terminal and return the screen, row by row.
pub(crate) fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal
        .draw(|f| crate::ui::draw(f, app))
        .expect("draw the shell");
    let buffer = terminal.backend().buffer();
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
        commit_collapse_hits: Vec::new(),
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

/// Drain document snapshots into `app` until they stop arriving.
///
/// A backend has two streams and a document's *content* comes on this one, so a
/// test that asserts on buffer text has to drain it as well as the event stream.
pub(crate) async fn pump_snapshots(app: &mut App, snaps: &mut karet_session::local::SnapshotRx) {
    while let Ok(Some((doc, snapshot))) =
        tokio::time::timeout(std::time::Duration::from_millis(500), snaps.recv()).await
    {
        app.on_snapshot(doc, &snapshot);
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
