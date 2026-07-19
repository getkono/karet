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

    /// Drain the queued events and return the most recent [`Event::VcsStatus`].
    fn latest_vcs_status(events: &mut EventRx) -> Option<(Vec<FileChange>, Vec<FileChange>)> {
        let mut found = None;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::VcsStatus { staged, working } = ev {
                found = Some((staged, working));
            }
        }
        found
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
        let Some((staged, working)) = latest_vcs_status(&mut events) else {
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
        let Some((staged, _working)) = latest_vcs_status(&mut events) else {
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
        let mut detail_ready = None;
        let mut ready = None;
        while let Some((_, ev)) = events.try_recv() {
            match ev {
                Event::CommitDetailReady { detail } => {
                    detail_ready = Some(detail);
                },
                Event::CommitReady { detail, changes } => {
                    ready = Some((detail, changes));
                },
                _ => {},
            }
        }
        let Some(detail) = detail_ready else {
            return;
        };
        assert_eq!(detail.summary, "add b");
        let Some((detail, changes)) = ready else {
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
        let mut hist = None;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::FileHistory { commits, .. } = ev {
                hist = Some(commits);
            }
        }
        let Some(commits) = hist else {
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
        let mut ready = None;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::RangeReady {
                base_label,
                head_label,
                changes,
                ..
            } = ev
            {
                ready = Some((base_label, head_label, changes));
            }
        }
        let Some((base_label, head_label, changes)) = ready else {
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
        let mut notified = false;
        let mut range_ready = false;
        while let Some((_, ev)) = events.try_recv() {
            match ev {
                Event::Notification {
                    kind: NotificationKind::Vcs,
                    ..
                } => {
                    notified = true;
                },
                Event::RangeReady { .. } => range_ready = true,
                _ => {},
            }
        }
        assert!(notified, "no upstream yields a VCS notification");
        assert!(!range_ready, "an unresolvable range emits no RangeReady");
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
        let Some((_staged, working)) = latest_vcs_status(&mut events) else {
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
        let refreshed = latest_vcs_status(&mut events);
        assert!(refreshed.is_some(), "fs event should refresh the status");
        if let Some((_staged, working)) = refreshed {
            assert_eq!(working.len(), 2);
        }
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
