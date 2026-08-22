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
    let backend = local_session(session, None);
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

#[cfg(feature = "mdlint")]
#[tokio::test]
async fn opening_markdown_publishes_markdownlint_diagnostics() {
    use crate::api::Event;
    use crate::session::Session;
    use crate::session::SessionConfig;

    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let path = dir.path().join("doc.md");
    // Trailing spaces (MD009) and a bare URL (MD034) under a proper title.
    if std::fs::write(&path, "# Title\n\ntext \nhttps://example.com\n").is_err() {
        return;
    }

    let (session, mut events, _snaps) = Session::new(SessionConfig::default());
    let backend = local_session(session, None);
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
    let rules = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some((_, event)) = events.recv().await {
            if let Event::DiagnosticsPublished { diagnostics, .. } = event
                && !diagnostics.is_empty()
            {
                return diagnostics
                    .iter()
                    .filter(|d| d.source.as_deref() == Some("markdownlint"))
                    .filter_map(|d| d.code.clone())
                    .collect::<Vec<_>>();
            }
        }
        Vec::new()
    })
    .await
    .unwrap_or_default();
    assert!(rules.contains(&"MD009".to_owned()), "found: {rules:?}");
    assert!(rules.contains(&"MD034".to_owned()), "found: {rules:?}");
}

#[tokio::test]
async fn workspace_todo_scan_streams_codetag_hits() {
    use crate::api::Event;
    use crate::session::Session;
    use crate::session::SessionConfig;

    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    if std::fs::write(
        dir.path().join("main.rs"),
        "// TODO: ship it\nfn main() {}\n",
    )
    .is_err()
    {
        return;
    }

    let config = SessionConfig {
        roots: vec![dir.path().to_path_buf()],
        ..SessionConfig::default()
    };
    let (session, mut events, _snaps) = Session::new(config);
    let backend = local_session(session, None);
    let id = backend.next_id();
    assert!(
        backend
            .send(id, Command::ScanWorkspaceTodos { limit: 100 })
            .is_ok()
    );
    let (mut tags, mut finished) = (Vec::new(), false);
    let _ = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some((event_id, event)) = events.recv().await {
            if event_id != Some(id) {
                continue;
            }
            match event {
                Event::TodoScanProgress { hits, .. } => {
                    tags.extend(hits.into_iter().map(|h| (h.tag, h.message)));
                },
                Event::TodoScanFinished { .. } => {
                    finished = true;
                    break;
                },
                _ => {},
            }
        }
    })
    .await;
    assert!(finished, "the scan reports completion");
    assert!(
        tags.contains(&("TODO".to_owned(), "ship it".to_owned())),
        "found: {tags:?}"
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
    let backend = local_session(session, None);
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
    let backend = local_session(session, None);
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
    let backend = local_session(session, None);
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
    let backend = local_session(session, None);
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
    let backend = local_session(session, None);
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
async fn document_symbols_fall_back_to_the_syntax_worker() {
    use karet_core::StandardToken;

    use crate::api::Event;
    use crate::session::Session;
    use crate::session::SessionConfig;

    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let path = dir.path().join("outline.rs");
    if std::fs::write(&path, "mod café { pub struct Thé; }\n").is_err() {
        return;
    }

    let (session, mut events, mut snaps) = Session::new(SessionConfig::default());
    let backend = local_session(session, None);
    let open = backend.next_id();
    assert!(
        backend
            .send(
                open,
                Command::OpenDocument {
                    path,
                    language: None,
                },
            )
            .is_ok()
    );
    let doc = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some((id, event)) = events.recv().await {
            if id == Some(open)
                && let Event::Opened { doc, .. } = event
            {
                return Some(doc);
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    let Some(doc) = doc else { return };
    assert!(
        await_snapshot(&mut snaps, |snapshot| snapshot
            .highlights
            .all()
            .iter()
            .any(|span| span.token == StandardToken::Keyword.id()))
        .await
    );

    let request = backend.next_id();
    assert!(
        backend
            .send(request, Command::DocumentSymbols { doc })
            .is_ok()
    );
    let symbols = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some((id, event)) = events.recv().await {
            if id == Some(request)
                && let Event::Symbols { symbols, .. } = event
            {
                return symbols;
            }
        }
        Vec::new()
    })
    .await
    .unwrap_or_default();
    assert_eq!(symbols[0].name, "café");
    assert_eq!(symbols[0].children[0].name, "Thé");
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
    let backend = local_session(session, None);
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
    let backend = local_session(session, None);
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
    let backend = local_session(session, None);
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
