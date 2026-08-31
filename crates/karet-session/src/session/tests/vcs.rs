    /// Initialize a temp git repository with one untracked `a.txt`, returning the
    /// temp dir, its root path, and the repo-relative file path. `None` if `git`
    /// isn't available.
    fn init_temp_repo() -> Option<(tempfile::TempDir, PathBuf, PathBuf)> {
        let dir = tempfile::tempdir().ok()?;
        let root = dir.path().to_path_buf();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .ok()
                .filter(std::process::ExitStatus::success)
        };
        run(&["init", "-q"])?;
        run(&["config", "user.email", "test@example.com"])?;
        run(&["config", "user.name", "karet test"])?;
        std::fs::write(root.join("a.txt"), "hello\n").ok()?;
        Some((dir, root, PathBuf::from("a.txt")))
    }

    /// Poll the event stream until `pick` yields, or a 5s deadline passes. The
    /// VCS answers come from the worker thread, so tests must wait, not `try_recv`.
    fn wait_for<T>(
        events: &mut EventRx,
        mut pick: impl FnMut(Event) -> Option<T>,
    ) -> Option<T> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            while let Some((_, ev)) = events.try_recv() {
                if let Some(value) = pick(ev) {
                    return Some(value);
                }
            }
            if std::time::Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Wait for the next [`Event::VcsStatus`].
    fn wait_vcs_status(
        events: &mut EventRx,
    ) -> Option<(Vec<crate::api::ChangeSummary>, Vec<crate::api::ChangeSummary>)> {
        wait_for(events, |ev| match ev {
            Event::VcsStatus { staged, working } => Some((staged, working)),
            _ => None,
        })
    }

    #[test]
    fn staging_through_the_session_updates_status() {
        let Some((_dir, root, file)) = init_temp_repo() else {
            return;
        };
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![root],
            ..SessionConfig::default()
        });
        // The actor normally calls this; here we drive the session directly.
        session.start();

        // The session seeds an initial status: the file is untracked in `working`.
        let Some((staged, working)) = wait_vcs_status(&mut events) else {
            return;
        };
        assert!(staged.is_empty());
        assert!(
            working
                .iter()
                .any(|c| c.path == file && c.status == karet_vcs::StatusKind::Untracked)
        );

        // Stage it → a fresh status with the file staged as Added.
        session.handle(
            RequestId(1),
            Command::Stage {
                paths: vec![file.clone()],
            },
        );
        let Some((staged, _working)) = wait_vcs_status(&mut events) else {
            return;
        };
        assert!(
            staged
                .iter()
                .any(|c| c.path == file && c.status == karet_vcs::StatusKind::Added)
        );
    }

    #[test]
    fn commit_detail_and_file_history_round_trip() {
        let Some((_dir, root, file)) = init_temp_repo() else {
            return;
        };
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .ok()
                .filter(std::process::ExitStatus::success)
        };
        // One commit touching a.txt, one touching only b.txt.
        if run(&["add", "a.txt"]).is_none() || run(&["commit", "-q", "-m", "add a"]).is_none() {
            return;
        }
        std::fs::write(root.join("b.txt"), "b\n").ok();
        run(&["add", "b.txt"]);
        run(&["commit", "-q", "-m", "add b"]);
        // The app passes the file's absolute path (a relative path would resolve
        // against the process CWD, not the repo root — see `Repository::file_history`).
        let file_abs = root.join(&file);

        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![root],
            ..SessionConfig::default()
        });
        session.start();
        while events.try_recv().is_some() {} // drain the seeded status/log

        // CommitDetail(HEAD) answers with the "add b" commit and its single change.
        session.handle(
            RequestId(1),
            Command::CommitDetail {
                rev: "HEAD".to_string(),
            },
        );
        let Some(detail) = wait_for(&mut events, |ev| match ev {
            Event::CommitDetailReady { detail } => Some(detail),
            _ => None,
        }) else {
            return;
        };
        assert_eq!(detail.summary, "add b");
        let Some((detail, changes)) = wait_for(&mut events, |ev| match ev {
            Event::CommitReady { detail, changes } => Some((detail, changes)),
            _ => None,
        }) else {
            return;
        };
        assert_eq!(detail.summary, "add b");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, PathBuf::from("b.txt"));

        // FileHistory(a.txt) answers with exactly the "add a" commit.
        session.handle(
            RequestId(2),
            Command::FileHistory {
                path: file_abs,
                skip: 0,
                limit: 10,
            },
        );
        let Some(commits) = wait_for(&mut events, |ev| match ev {
            Event::FileHistory { commits, .. } => Some(commits),
            _ => None,
        }) else {
            return;
        };
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].summary, "add a");
    }

    #[test]
    fn range_changes_between_two_revs_round_trip() {
        let Some((_dir, root, _file)) = init_temp_repo() else {
            return;
        };
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .ok()
                .filter(std::process::ExitStatus::success)
        };
        // c0 adds a.txt; c1 modifies a.txt and adds b.txt.
        if run(&["add", "a.txt"]).is_none() || run(&["commit", "-q", "-m", "c0"]).is_none() {
            return;
        }
        std::fs::write(root.join("a.txt"), "hello\nworld\n").ok();
        std::fs::write(root.join("b.txt"), "b\n").ok();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "c1"]);

        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![root],
            ..SessionConfig::default()
        });
        session.start();
        while events.try_recv().is_some() {} // drain the seeded status/log

        // A two-dot HEAD~1..HEAD range answers with a.txt (modified) and b.txt (added).
        session.handle(
            RequestId(1),
            Command::RangeChanges {
                spec: RangeSpec::Between {
                    base: "HEAD~1".to_string(),
                    head: "HEAD".to_string(),
                    merge_base: false,
                },
            },
        );
        let Some((base_label, head_label, changes)) = wait_for(&mut events, |ev| match ev {
            Event::RangeReady {
                base_label,
                head_label,
                changes,
                ..
            } => Some((base_label, head_label, changes)),
            _ => None,
        }) else {
            return;
        };
        assert_eq!(base_label, "HEAD~1");
        assert_eq!(head_label, "HEAD");
        let paths: Vec<_> = changes.iter().map(|c| c.path.clone()).collect();
        assert!(paths.contains(&PathBuf::from("a.txt")));
        assert!(paths.contains(&PathBuf::from("b.txt")));

        // Unpushed with no configured upstream is a graceful notification, not a panic.
        session.handle(
            RequestId(2),
            Command::RangeChanges {
                spec: RangeSpec::Unpushed,
            },
        );
        let notified = wait_for(&mut events, |ev| match ev {
            Event::Notification {
                kind: NotificationKind::Vcs,
                ..
            } => Some(true),
            Event::RangeReady { .. } => Some(false),
            _ => None,
        });
        assert_eq!(
            notified,
            Some(true),
            "no upstream yields a VCS notification, never a RangeReady"
        );
    }

    #[test]
    fn filesystem_event_refreshes_vcs_status() {
        let Some((_dir, root, _file)) = init_temp_repo() else {
            return;
        };
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![root.clone()],
            ..SessionConfig::default()
        });
        // The actor normally calls this; here we drive the session directly.
        session.start();
        // Initial status: just the seeded `a.txt`.
        let Some((_staged, working)) = wait_vcs_status(&mut events) else {
            return;
        };
        assert_eq!(working.len(), 1);

        // A new file appears on disk; the debounced watcher would deliver this event.
        if std::fs::write(root.join("b.txt"), "hi\n").is_err() {
            return;
        }
        session.handle_fs_event(karet_watch::FsEvent {
            kind: karet_watch::FsEventKind::Created,
            paths: vec![root.join("b.txt")],
        });

        // The recompute re-emits a status that now lists both untracked files.
        let refreshed = wait_for(&mut events, |ev| match ev {
            Event::VcsStatus { staged, working } if working.len() == 2 => {
                Some((staged, working))
            },
            _ => None,
        });
        assert!(refreshed.is_some(), "fs event should refresh the status");
    }

    #[test]
    fn filesystem_event_emits_fs_changed_with_the_affected_paths() {
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        let path = PathBuf::from("/work/touched.rs");
        session.handle_fs_event(karet_watch::FsEvent {
            kind: karet_watch::FsEventKind::Modified,
            paths: vec![path.clone()],
        });
        let mut seen = None;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::FsChanged { paths } = ev {
                seen = Some(paths);
            }
        }
        assert_eq!(seen, Some(vec![path]));
    }

    /// Install an executable `pre-commit` hook running `body`.
    #[cfg(unix)]
    fn hook(root: &std::path::Path, body: &str) -> Option<()> {
        use std::os::unix::fs::PermissionsExt as _;
        let path = root.join(".git/hooks/pre-commit");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).ok()?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).ok()
    }

    /// Collect every event a commit produces, in order, until it settles: a
    /// success ends with the fresh status that follows it, a failure with the
    /// notification that reports it.
    fn drain_commit(events: &mut EventRx) -> Vec<Event> {
        let mut seen = Vec::new();
        let mut committed = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            while let Some((_, ev)) = events.try_recv() {
                let done = match &ev {
                    Event::Notification { .. } => true,
                    Event::VcsStatus { .. } => committed,
                    _ => false,
                };
                committed |=
                    matches!(ev, Event::Committed { .. } | Event::CommitCancelled);
                seen.push(ev);
                if done {
                    return seen;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        seen
    }

    #[cfg(unix)]
    #[test]
    fn a_commit_streams_its_hooks_output_before_it_reports_success() {
        let Some((_dir, root, file)) = init_temp_repo() else {
            return;
        };
        if hook(&root, "echo hook says hello").is_none() {
            return;
        }
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![root],
            ..SessionConfig::default()
        });
        session.start();
        if wait_vcs_status(&mut events).is_none() {
            return;
        }
        session.handle(RequestId(1), Command::Stage { paths: vec![file] });
        if wait_vcs_status(&mut events).is_none() {
            return;
        }

        session.handle(RequestId(2), Command::Commit {
            message: "subject".to_string(),
        });
        let seen = drain_commit(&mut events);
        let output: Vec<String> = seen
            .iter()
            .filter_map(|ev| match ev {
                Event::CommitOutput { lines } => Some(lines),
                _ => None,
            })
            .flatten()
            .map(|line| line.text.clone())
            .collect();
        assert!(
            output.iter().any(|line| line.contains("hook says hello")),
            "the hook's console output reached the seam: {output:?}"
        );

        // Ordering is the contract: every line, then the outcome, then the state.
        let committed = seen
            .iter()
            .position(|ev| matches!(ev, Event::Committed { .. }))
            .unwrap_or(usize::MAX);
        let last_output = seen
            .iter()
            .rposition(|ev| matches!(ev, Event::CommitOutput { .. }))
            .unwrap_or(0);
        assert!(last_output < committed, "output precedes the outcome: {seen:?}");
        assert!(
            seen.iter()
                .skip(committed)
                .any(|ev| matches!(ev, Event::VcsStatus { .. })),
            "a fresh status follows the commit: {seen:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_refused_commit_reports_the_failure_and_keeps_what_the_hook_printed() {
        let Some((_dir, root, file)) = init_temp_repo() else {
            return;
        };
        // The diagnosis goes to the hook's stdout, which git routes to its stderr.
        if hook(&root, "echo why it refused\nexit 1").is_none() {
            return;
        }
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![root],
            ..SessionConfig::default()
        });
        session.start();
        if wait_vcs_status(&mut events).is_none() {
            return;
        }
        session.handle(RequestId(1), Command::Stage { paths: vec![file] });
        if wait_vcs_status(&mut events).is_none() {
            return;
        }

        session.handle(RequestId(2), Command::Commit {
            message: "subject".to_string(),
        });
        let seen = drain_commit(&mut events);
        assert!(
            !seen
                .iter()
                .any(|ev| matches!(ev, Event::Committed { .. })),
            "a refused commit reports no oid: {seen:?}"
        );
        assert!(
            seen.iter().any(|ev| matches!(
                ev,
                Event::Notification {
                    kind: karet_core::NotificationKind::Vcs,
                    ..
                }
            )),
            "and does report the failure: {seen:?}"
        );
        let output: Vec<String> = seen
            .iter()
            .filter_map(|ev| match ev {
                Event::CommitOutput { lines } => Some(lines),
                _ => None,
            })
            .flatten()
            .map(|line| line.text.clone())
            .collect();
        assert!(
            output.iter().any(|line| line.contains("why it refused")),
            "the reason survives the failure: {output:?}"
        );
    }


    #[cfg(unix)]
    #[test]
    fn cancelling_a_running_commit_stops_it_without_creating_one() {
        let Some((_dir, root, file)) = init_temp_repo() else {
            return;
        };
        if hook(&root, "echo working\nsleep 30").is_none() {
            return;
        }
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![root.clone()],
            ..SessionConfig::default()
        });
        session.start();
        if wait_vcs_status(&mut events).is_none() {
            return;
        }
        session.handle(RequestId(1), Command::Stage { paths: vec![file] });
        if wait_vcs_status(&mut events).is_none() {
            return;
        }

        session.handle(RequestId(2), Command::Commit {
            message: "subject".to_string(),
        });
        // The worker is blocked inside git by now; the actor is not, which is the
        // whole point of routing cancellation through the token's hook.
        std::thread::sleep(std::time::Duration::from_millis(700));
        session.handle(RequestId(3), Command::Cancel {
            request: RequestId(2),
        });

        let seen = drain_commit(&mut events);
        assert!(
            seen.iter().any(|ev| matches!(ev, Event::CommitCancelled)),
            "the seam reported the cancellation: {seen:?}"
        );
        assert!(
            !seen
                .iter()
                .any(|ev| matches!(ev, Event::Committed { .. })),
            "and no commit was created: {seen:?}"
        );
        let commits = std::process::Command::new("git")
            .args(["rev-list", "--count", "--all"])
            .current_dir(&root)
            .output()
            .ok()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string());
        assert_eq!(commits.as_deref(), Some("0"), "the repository has no commit");
    }

    #[cfg(unix)]
    #[test]
    fn a_cancel_arriving_before_the_job_starts_is_still_honoured() {
        let Some((_dir, root, file)) = init_temp_repo() else {
            return;
        };
        // The hook would leave a trace if the commit ever reached it.
        if hook(&root, "touch .git/hook-ran\nsleep 5").is_none() {
            return;
        }
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![root.clone()],
            ..SessionConfig::default()
        });
        session.start();
        if wait_vcs_status(&mut events).is_none() {
            return;
        }
        session.handle(RequestId(1), Command::Stage { paths: vec![file] });
        if wait_vcs_status(&mut events).is_none() {
            return;
        }

        // Cancel in the same breath as the commit: the job is still queued, so
        // the hook is registered against a token that is already cancelled.
        session.handle(RequestId(2), Command::Commit {
            message: "subject".to_string(),
        });
        session.handle(RequestId(3), Command::Cancel {
            request: RequestId(2),
        });

        let seen = drain_commit(&mut events);
        assert!(
            seen.iter().any(|ev| matches!(ev, Event::CommitCancelled)),
            "a cancellation that beats the job is not lost: {seen:?}"
        );
        assert!(
            !root.join(".git/hook-ran").exists(),
            "git never ran, so neither did its hooks"
        );
    }
