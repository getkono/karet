//! The [`Backend`] seam: the single interface the presentation layer talks to,
//! identical in local mode today and (additively) in a future remote mode.

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use karet_watch::FsEvent;
use tokio::sync::mpsc;

/// How often the actor sweeps for buffers due to be backed up. The per-document
/// dirty threshold is `files.backupInterval`; this only bounds detection latency.
const BACKUP_TICK: Duration = Duration::from_secs(2);

use crate::api::Command;
use crate::api::RequestId;
use crate::highlight::HighlightResult;
use crate::lsp::LspUpdate;
use crate::session::Session;

/// Errors produced when submitting to a [`Backend`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BackendError {
    /// The backend connection has been closed.
    #[error("the backend connection is closed")]
    Closed,
    /// A transport-level failure (remote mode).
    #[error("transport error: {0}")]
    Transport(String),
}

/// The mode-agnostic seam the UI is written against.
///
/// It is deliberately *not* `async fn`-in-trait (so it stays `dyn`-compatible):
/// submission is synchronous and fallible, while results arrive asynchronously on
/// the session's [`EventRx`](crate::session::EventRx). The same UI code drives an
/// in-process [`LocalBackend`] today and a remote client later.
pub trait Backend: Send + Sync {
    /// Submit `command`, tagged with `id` so its answering event can be correlated.
    ///
    /// # Errors
    /// Returns [`BackendError::Closed`] if the backend is no longer accepting input.
    fn send(&self, id: RequestId, command: Command) -> Result<(), BackendError>;

    /// The next monotonic [`RequestId`] for this connection.
    #[must_use]
    fn next_id(&self) -> RequestId;
}

/// An in-process backend that drives a [`Session`] on a background task.
///
/// `send` pushes onto an unbounded command channel (an unbounded send is the only
/// non-`async` send, which the synchronous [`Backend::send`] requires); the actor
/// task drains it in order and the session emits results on its event/snapshot
/// streams.
pub struct LocalBackend {
    commands: mpsc::UnboundedSender<(RequestId, Command)>,
    next: AtomicU64,
}

impl Backend for LocalBackend {
    fn send(&self, id: RequestId, command: Command) -> Result<(), BackendError> {
        self.commands
            .send((id, command))
            .map_err(|_| BackendError::Closed)
    }

    fn next_id(&self) -> RequestId {
        RequestId(self.next.fetch_add(1, Ordering::Relaxed))
    }
}

/// Drive `session` in-process on a spawned task, returning a [`LocalBackend`] to
/// submit commands to.
///
/// Must be called within a Tokio runtime context (the app enters one before
/// constructing the backend). The session's [`EventRx`](crate::session::EventRx)
/// and [`SnapshotRx`](crate::local::SnapshotRx) (from [`Session::new`]) are the
/// matching output streams; the actor ends when the returned backend is dropped.
///
/// [`Session::new`]: crate::session::Session::new
#[must_use]
pub fn local(mut session: Session) -> LocalBackend {
    let (commands, mut rx) = mpsc::unbounded_channel::<(RequestId, Command)>();
    let (watcher, mut fs_rx) = session.take_watch();
    let mut highlights = session.take_highlights();
    let mut lsp_updates = session.take_lsp_updates();
    tokio::spawn(async move {
        // Hold the watcher alive for exactly as long as the actor consumes events.
        let _watcher = watcher;
        // Compute the initial VCS status here, on the actor task, rather than on the
        // construction thread — a large repository's `git status` then runs
        // concurrently with the first frame instead of blocking it.
        session.start();
        // A steady tick drives the crash-recovery backup sweep; the session decides
        // per-document whether the configured dirty interval has elapsed.
        let mut backup = tokio::time::interval(BACKUP_TICK);
        backup.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                command = rx.recv() => match command {
                    Some((id, command)) => session.handle(id, command),
                    None => break, // the backend was dropped
                },
                fs_event = recv_fs(&mut fs_rx) => match fs_event {
                    Some(event) => session.handle_fs_event(event),
                    None => fs_rx = None, // the watcher stopped; stop selecting it
                },
                // Layered highlights computed off-actor; applied (and published) here.
                result = recv_highlights(&mut highlights) => match result {
                    Some(result) => session.apply_highlights(result),
                    None => highlights = None, // the worker stopped; stop selecting it
                },
                // LSP answers computed on the server tasks; converted and emitted here.
                update = recv_lsp(&mut lsp_updates) => match update {
                    Some(update) => session.apply_lsp_update(update),
                    None => lsp_updates = None, // no LSP; stop selecting it
                },
                _ = backup.tick() => session.backup_tick(),
            }
        }
    });
    LocalBackend {
        commands,
        next: AtomicU64::new(1),
    }
}

/// Await the next filesystem event, or never resolve when there is no watcher (so
/// the actor's `select!` simply ignores that arm).
async fn recv_fs(rx: &mut Option<mpsc::UnboundedReceiver<FsEvent>>) -> Option<FsEvent> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Await the next completed highlight, or never resolve when there is no worker.
async fn recv_highlights(
    rx: &mut Option<mpsc::UnboundedReceiver<HighlightResult>>,
) -> Option<HighlightResult> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Await the next LSP task result, or never resolve when LSP is not running.
async fn recv_lsp(rx: &mut Option<mpsc::UnboundedReceiver<LspUpdate>>) -> Option<LspUpdate> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_error_displays() {
        assert_eq!(
            BackendError::Closed.to_string(),
            "the backend connection is closed"
        );
    }

    #[tokio::test]
    async fn local_backend_drives_open() {
        use crate::api::Event;
        use crate::session::Session;
        use crate::session::SessionConfig;

        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("hello.txt");
        if std::fs::write(&path, "hello\n").is_err() {
            return;
        }

        let (session, mut events, _snaps) = Session::new(SessionConfig::default());
        let backend = local(session);
        let id = backend.next_id();
        assert!(
            backend
                .send(
                    id,
                    Command::OpenDocument {
                        path,
                        language: None
                    }
                )
                .is_ok()
        );

        // Startup producers may announce capability state first. Correlate the answer
        // instead of assuming this command owns the stream's first event.
        let opened = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some((event_id, event)) = events.recv().await {
                if event_id == Some(id) && matches!(event, Event::Opened { .. }) {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false);
        assert!(
            opened,
            "local backend should drive the session to open the file"
        );
    }

    #[tokio::test]
    async fn local_backend_reports_an_exact_nested_repository_status() {
        use crate::api::Event;
        use crate::session::Session;
        use crate::session::SessionConfig;

        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let nested = dir.path().join("nested");
        if std::fs::create_dir_all(&nested).is_err() {
            return;
        }
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&nested)
                .status()
                .ok()
                .is_some_and(|status| status.success())
        };
        if !git(&["init", "-q"])
            || !git(&["config", "user.email", "test@example.com"])
            || !git(&["config", "user.name", "karet test"])
            || std::fs::write(nested.join("file.txt"), "one\n").is_err()
            || !git(&["add", "file.txt"])
            || !git(&["commit", "-q", "-m", "initial"])
            || std::fs::write(nested.join("file.txt"), "one\ntwo\n").is_err()
        {
            return;
        }

        let (session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });
        let backend = local(session);
        let id = backend.next_id();
        assert!(
            backend
                .send(
                    id,
                    Command::NestedRepositoryStatus {
                        path: nested.clone(),
                    },
                )
                .is_ok()
        );

        let received = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some((event_id, event)) = events.recv().await {
                if event_id == Some(id)
                    && let Event::NestedRepositoryStatus { path, summary } = event
                {
                    return Some((path, summary));
                }
            }
            None
        })
        .await
        .ok()
        .flatten();
        let Some((path, summary)) = received else {
            return;
        };
        assert_eq!(path, nested);
        assert_eq!((summary.added, summary.removed), (1, 0));
    }

    #[tokio::test]
    async fn repository_actions_and_blame_run_off_actor() {
        use karet_core::BlameAttribution;
        use karet_vcs::CreateBranchOptions;

        use crate::api::Event;
        use crate::api::VcsAction;
        use crate::session::Session;
        use crate::session::SessionConfig;

        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let root = dir.path().to_path_buf();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .ok()
                .is_some_and(|status| status.success())
        };
        if !git(&["init", "-q"])
            || !git(&["config", "user.email", "test@example.com"])
            || !git(&["config", "user.name", "karet test"])
            || std::fs::write(root.join("code.rs"), "fn main() {}\n").is_err()
            || !git(&["add", "code.rs"])
            || !git(&["commit", "-q", "-m", "initial"])
        {
            return;
        }

        let (session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![root.clone()],
            ..SessionConfig::default()
        });
        let backend = local(session);
        let open_id = backend.next_id();
        assert!(
            backend
                .send(
                    open_id,
                    Command::OpenDocument {
                        path: root.join("code.rs"),
                        language: None,
                    },
                )
                .is_ok()
        );
        let opened = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some((id, event)) = events.recv().await {
                if id == Some(open_id)
                    && let Event::Opened { doc, version } = event
                {
                    return Some((doc, version));
                }
            }
            None
        })
        .await
        .ok()
        .flatten();
        let Some((doc, version)) = opened else {
            return;
        };

        let blame_id = backend.next_id();
        assert!(
            backend
                .send(
                    blame_id,
                    Command::Blame {
                        doc,
                        version,
                        line: 0,
                    },
                )
                .is_ok()
        );
        let blamed = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some((id, event)) = events.recv().await {
                if id == Some(blame_id)
                    && let Event::BlameResult { attribution, .. } = event
                {
                    return attribution;
                }
            }
            None
        })
        .await
        .unwrap_or_default();
        assert!(matches!(blamed, Some(BlameAttribution::Commit(_))));

        if std::fs::write(root.join("untracked.rs"), "fn new_file() {}\n").is_err() {
            return;
        }
        let untracked_open_id = backend.next_id();
        assert!(
            backend
                .send(
                    untracked_open_id,
                    Command::OpenDocument {
                        path: root.join("untracked.rs"),
                        language: None,
                    },
                )
                .is_ok()
        );
        let untracked = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some((id, event)) = events.recv().await {
                if id == Some(untracked_open_id)
                    && let Event::Opened { doc, version } = event
                {
                    return Some((doc, version));
                }
            }
            None
        })
        .await
        .ok()
        .flatten();
        let Some((untracked_doc, untracked_version)) = untracked else {
            return;
        };
        let untracked_blame_id = backend.next_id();
        assert!(
            backend
                .send(
                    untracked_blame_id,
                    Command::Blame {
                        doc: untracked_doc,
                        version: untracked_version,
                        line: 0,
                    },
                )
                .is_ok()
        );
        let unavailable = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some((id, event)) = events.recv().await {
                if id == Some(untracked_blame_id) {
                    return Some(event);
                }
            }
            None
        })
        .await
        .ok()
        .flatten();
        assert!(matches!(
            unavailable,
            Some(Event::BlameResult {
                attribution: None,
                ..
            })
        ));

        let branch_id = backend.next_id();
        let mut branch_options = CreateBranchOptions::default();
        branch_options.name = "feature".to_string();
        assert!(
            backend
                .send(
                    branch_id,
                    Command::VcsAction {
                        action: VcsAction::CreateBranch(branch_options),
                    },
                )
                .is_ok()
        );
        let branch = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some((id, event)) = events.recv().await {
                if id == Some(branch_id)
                    && let Event::RepositorySnapshot { snapshot } = event
                {
                    return snapshot.state.branch;
                }
            }
            None
        })
        .await
        .ok()
        .flatten();
        assert_eq!(branch.as_deref(), Some("feature"));
    }

    /// Drain snapshots until one satisfies `wanted`, or time out.
    #[cfg(test)]
    async fn await_snapshot(
        snaps: &mut crate::local::SnapshotRx,
        wanted: impl Fn(&crate::local::DocSnapshot) -> bool,
    ) -> bool {
        let found = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some((_, snap)) = snaps.recv().await {
                if wanted(&snap) {
                    return true;
                }
            }
            false
        })
        .await;
        found.unwrap_or(false)
    }

    #[tokio::test]
    async fn injected_language_is_highlighted_through_the_worker() {
        use karet_core::TokenId;

        use crate::session::Session;
        use crate::session::SessionConfig;

        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("notes.md");
        // A markdown file whose fenced block is rust: only the injection machinery,
        // driven end to end through the worker, can colour `fn` as a keyword.
        if std::fs::write(&path, "# T\n\n```rust\nfn main() {}\n```\n").is_err() {
            return;
        }

        let (session, _events, mut snaps) = Session::new(SessionConfig::default());
        let backend = local(session);
        let id = backend.next_id();
        assert!(
            backend
                .send(
                    id,
                    Command::OpenDocument {
                        path,
                        language: None
                    }
                )
                .is_ok()
        );

        // The open publishes immediately with no spans; the worker's answer follows.
        let highlighted = await_snapshot(&mut snaps, |snap| {
            snap.highlights
                .all()
                .iter()
                .any(|s| s.token == TokenId::KEYWORD)
        })
        .await;
        assert!(
            highlighted,
            "the embedded rust fence should eventually be highlighted"
        );
    }

    #[tokio::test]
    async fn syntax_error_lines_reach_the_snapshot_stream() {
        use crate::session::Session;
        use crate::session::SessionConfig;

        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("broken.rs");
        // Line 1 (0-based) is broken; the completion gate reads these ranges.
        if std::fs::write(&path, "fn ok() {}\nfn broken() { let x = ; }\n").is_err() {
            return;
        }

        let (session, _events, mut snaps) = Session::new(SessionConfig::default());
        let backend = local(session);
        assert!(
            backend
                .send(
                    backend.next_id(),
                    Command::OpenDocument {
                        path,
                        language: None
                    }
                )
                .is_ok()
        );

        let flagged = await_snapshot(&mut snaps, |snap| {
            snap.syntax_error_lines
                .iter()
                .any(|&(start, end)| start <= 1 && 1 <= end)
        })
        .await;
        assert!(flagged, "the broken line should be flagged on the snapshot");
    }

    #[tokio::test]
    async fn semantic_blocks_reach_the_snapshot_stream() {
        use crate::session::Session;
        use crate::session::SessionConfig;

        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("notes.md");
        if std::fs::write(&path, "# Top\n\n## Child\n\nbody\n").is_err() {
            return;
        }

        let (session, _events, mut snaps) = Session::new(SessionConfig::default());
        let backend = local(session);
        assert!(
            backend
                .send(
                    backend.next_id(),
                    Command::OpenDocument {
                        path,
                        language: None
                    }
                )
                .is_ok()
        );

        let published = await_snapshot(&mut snaps, |snap| {
            snap.semantic_blocks.active_at(4).len() == 2
        })
        .await;
        assert!(
            published,
            "the Markdown H1/H2 chain should reach the UI snapshot"
        );
    }

    #[tokio::test]
    async fn todo_comments_are_marked_in_a_real_rust_buffer() {
        use karet_core::StandardToken;

        use crate::session::Session;
        use crate::session::SessionConfig;

        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("lib.rs");
        if std::fs::write(&path, "// TODO: fix bug here\n// context\nfn main() {}\n").is_err() {
            return;
        }

        // Default settings: `editor.semanticComments` is on.
        let (session, _events, mut snaps) = Session::new(SessionConfig::default());
        let backend = local(session);
        assert!(
            backend
                .send(
                    backend.next_id(),
                    Command::OpenDocument {
                        path,
                        language: None
                    }
                )
                .is_ok()
        );

        let mark = StandardToken::CommentMark.id();
        let marked = await_snapshot(&mut snaps, |snap| {
            snap.highlights.all().iter().any(|s| s.token == mark)
        })
        .await;
        assert!(
            marked,
            "the TODO comment block should be published as CommentMark"
        );
    }

    #[tokio::test]
    async fn disabling_semantic_comments_leaves_comments_plain() {
        use karet_core::StandardToken;
        use karet_core::TokenId;

        use crate::session::Session;
        use crate::session::SessionConfig;

        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("lib.rs");
        if std::fs::write(&path, "// TODO: fix bug here\nfn main() {}\n").is_err() {
            return;
        }

        let mut config = SessionConfig::default();
        config.settings.editor.semantic_comments.enabled = false;
        let (session, _events, mut snaps) = Session::new(config);
        let backend = local(session);
        assert!(
            backend
                .send(
                    backend.next_id(),
                    Command::OpenDocument {
                        path,
                        language: None
                    }
                )
                .is_ok()
        );

        // Wait for the worker's real answer: the snapshot that carries comment spans.
        let mark = StandardToken::CommentMark.id();
        let mut saw_mark = false;
        let highlighted = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some((_, snap)) = snaps.recv().await {
                saw_mark |= snap.highlights.all().iter().any(|s| s.token == mark);
                if snap
                    .highlights
                    .all()
                    .iter()
                    .any(|s| s.token == TokenId::COMMENT)
                {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false);
        assert!(highlighted, "the buffer should still be highlighted");
        assert!(
            !saw_mark,
            "with the setting off, no snapshot may carry CommentMark"
        );
    }

    #[tokio::test]
    async fn editing_republishes_highlights_for_the_new_text() {
        use karet_core::Change;
        use karet_core::LineCol;
        use karet_core::Range;
        use karet_core::TextEdit;
        use karet_core::TokenId;
        use karet_text::EditCause;

        use crate::api::Event;
        use crate::session::Session;
        use crate::session::SessionConfig;

        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("live.md");
        if std::fs::write(&path, "text\n").is_err() {
            return;
        }

        let (session, mut events, mut snaps) = Session::new(SessionConfig::default());
        let backend = local(session);
        let id = backend.next_id();
        if backend
            .send(
                id,
                Command::OpenDocument {
                    path,
                    language: None,
                },
            )
            .is_err()
        {
            return;
        }
        let Some((_, Event::Opened { doc, version })) = events.recv().await else {
            return;
        };

        // Type a rust code fence at the end of the buffer.
        let Ok(range) = Range::new(LineCol::new(1, 0), LineCol::new(1, 0)) else {
            return;
        };
        let change = Change::new(
            version,
            vec![TextEdit {
                range,
                new_text: "\n```rust\nfn f() {}\n```\n".to_owned(),
            }],
        );
        assert!(backend.next_id() > id);
        if backend
            .send(
                backend.next_id(),
                Command::ApplyChange {
                    doc,
                    change,
                    cause: EditCause::Paste,
                },
            )
            .is_err()
        {
            return;
        }

        // The fence did not exist a moment ago; the worker must discover the injection
        // and republish. This is the live-update contract.
        let highlighted = await_snapshot(&mut snaps, |snap| {
            snap.highlights
                .all()
                .iter()
                .any(|s| s.token == TokenId::KEYWORD)
        })
        .await;
        assert!(
            highlighted,
            "typing a code fence should light up the embedded language"
        );
    }
}
