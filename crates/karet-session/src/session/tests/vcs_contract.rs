    // The source-control Command/Event contract beyond staging and commits:
    // the remaining index writes (including the destructive Discard), the
    // requested refresh, log paging, the repository snapshot, the two ad-hoc
    // diff commands, and the log reconciliation a filesystem event triggers.

    /// `git`'s stdout in `root`, trimmed, failing the test if it does not succeed.
    fn git_output(
        root: &std::path::Path,
        args: &[&str],
    ) -> Result<String, Box<dyn std::error::Error>> {
        let output = std::process::Command::new("git").args(args).current_dir(root).output()?;
        if !output.status.success() {
            return Err(format!("git {args:?} failed: {}", output.status).into());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    /// A temp repository whose `a.txt` (`hello\n`) is committed once. `Ok(None)`
    /// only if `git` isn't available; any fixture failure past that is an error.
    fn committed_repo() -> Result<Option<(tempfile::TempDir, PathBuf)>, Box<dyn std::error::Error>> {
        let Some((dir, root, _)) = init_git_repo()? else {
            return Ok(None);
        };
        git_in(&root, &["add", "a.txt"])?;
        git_in(&root, &["commit", "-q", "-m", "c0"])?;
        Ok(Some((dir, root)))
    }

    /// Commit `file` with `body` and `message`.
    fn commit_file(
        root: &std::path::Path,
        file: &str,
        body: &str,
        message: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        std::fs::write(root.join(file), body)?;
        git_in(root, &["add", file])?;
        git_in(root, &["commit", "-q", "-m", message])
    }

    fn repo_session(root: &std::path::Path) -> (Session, EventRx) {
        let (session, events, _snaps) = Session::new(SessionConfig {
            roots: vec![root.to_path_buf()],
            ..SessionConfig::default()
        });
        (session, events)
    }

    /// Wait (5s) for the event answering `id` that `pick` accepts.
    fn wait_answer<T>(
        events: &mut EventRx,
        id: RequestId,
        mut pick: impl FnMut(Event) -> Option<T>,
    ) -> Option<T> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            while let Some((tag, event)) = events.try_recv() {
                if tag == Some(id)
                    && let Some(value) = pick(event)
                {
                    return Some(value);
                }
            }
            if std::time::Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    type Status = (Vec<crate::api::ChangeSummary>, Vec<crate::api::ChangeSummary>);

    fn answered_status(events: &mut EventRx, id: RequestId) -> Option<Status> {
        wait_answer(events, id, |event| match event {
            Event::VcsStatus { staged, working } => Some((staged, working)),
            _ => None,
        })
    }

    fn listed(changes: &[crate::api::ChangeSummary]) -> Vec<(String, karet_vcs::StatusKind)> {
        let mut listed: Vec<_> = changes
            .iter()
            .map(|change| (change.path.display().to_string(), change.status))
            .collect();
        listed.sort_by(|a, b| a.0.cmp(&b.0));
        listed
    }

    #[test]
    fn unstage_returns_a_change_to_the_worktree_and_discard_restores_the_file() -> TestResult {
        let Some((_dir, root)) = committed_repo()? else {
            // Skip: `git` is not on PATH.
            return Ok(());
        };
        std::fs::write(root.join("a.txt"), "changed\n")?;
        git_in(&root, &["add", "a.txt"])?;
        let (mut session, mut events) = repo_session(&root);

        session.handle(
            RequestId(1),
            Command::Unstage {
                paths: vec![PathBuf::from("a.txt")],
            },
        );
        let unstaged = answered_status(&mut events, RequestId(1));
        assert_eq!(
            unstaged.map(|(staged, working)| (listed(&staged), listed(&working))),
            Some((
                Vec::new(),
                vec![("a.txt".to_owned(), karet_vcs::StatusKind::Modified)]
            )),
            "the staged edit moves back to the worktree"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).ok().as_deref(),
            Some("changed\n"),
            "unstaging never touches the file"
        );

        session.handle(
            RequestId(2),
            Command::Discard {
                paths: vec![PathBuf::from("a.txt")],
            },
        );
        let discarded = answered_status(&mut events, RequestId(2));
        assert_eq!(
            discarded.map(|(staged, working)| (staged.len(), working.len())),
            Some((0, 0)),
            "nothing is left to commit"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).ok().as_deref(),
            Some("hello\n"),
            "discard puts the committed content back on disk"
        );
        Ok(())
    }

    #[test]
    fn stage_all_and_unstage_all_move_every_change_across_the_index() -> TestResult {
        let Some((_dir, root)) = committed_repo()? else {
            // Skip: `git` is not on PATH.
            return Ok(());
        };
        std::fs::write(root.join("a.txt"), "changed\n")?;
        std::fs::write(root.join("b.txt"), "new\n")?;
        let (mut session, mut events) = repo_session(&root);

        session.handle(RequestId(1), Command::StageAll);
        let staged = answered_status(&mut events, RequestId(1));
        assert_eq!(
            staged.map(|(staged, working)| (listed(&staged), working.len())),
            Some((
                vec![
                    ("a.txt".to_owned(), karet_vcs::StatusKind::Modified),
                    ("b.txt".to_owned(), karet_vcs::StatusKind::Added),
                ],
                0
            ))
        );

        session.handle(RequestId(2), Command::UnstageAll);
        let unstaged = answered_status(&mut events, RequestId(2));
        assert_eq!(
            unstaged.map(|(staged, working)| (staged.len(), listed(&working))),
            Some((
                0,
                vec![
                    ("a.txt".to_owned(), karet_vcs::StatusKind::Modified),
                    ("b.txt".to_owned(), karet_vcs::StatusKind::Untracked),
                ]
            ))
        );
        assert_eq!(
            std::fs::read_to_string(root.join("b.txt")).ok().as_deref(),
            Some("new\n"),
            "unstaging an addition keeps the file"
        );
        Ok(())
    }

    /// A spontaneous status is change-gated; a requested one must always answer,
    /// or a client refreshing an unchanged repository would wait forever.
    #[test]
    fn a_requested_refresh_answers_even_when_nothing_changed() -> TestResult {
        let Some((_dir, root)) = committed_repo()? else {
            // Skip: `git` is not on PATH.
            return Ok(());
        };
        let (mut session, mut events) = repo_session(&root);

        session.handle(RequestId(1), Command::RefreshVcs);
        assert!(answered_status(&mut events, RequestId(1)).is_some());
        session.handle(RequestId(2), Command::RefreshVcs);
        assert_eq!(
            answered_status(&mut events, RequestId(2)).map(|(s, w)| (s.len(), w.len())),
            Some((0, 0)),
            "the repeat answers too, with the same clean status"
        );
        Ok(())
    }

    /// A failed index write answers with a VCS error rather than a status.
    #[test]
    fn an_index_write_that_fails_answers_with_a_vcs_notification() -> TestResult {
        let Some((_dir, root)) = committed_repo()? else {
            // Skip: `git` is not on PATH.
            return Ok(());
        };
        let (mut session, mut events) = repo_session(&root);

        session.handle(
            RequestId(1),
            Command::ApplyIndexPatch {
                patch: "not a patch\n".to_owned(),
                reverse: false,
            },
        );
        let answer = wait_answer(&mut events, RequestId(1), |event| match event {
            Event::Notification {
                severity, kind, ..
            } => Some((severity, kind)),
            Event::VcsStatus { .. } => Some((Severity::Hint, NotificationKind::System)),
            _ => None,
        });
        assert_eq!(answer, Some((Severity::Error, NotificationKind::Vcs)));
        Ok(())
    }

    #[test]
    fn the_log_pages_and_says_whether_more_remain() -> TestResult {
        let Some((_dir, root)) = committed_repo()? else {
            // Skip: `git` is not on PATH.
            return Ok(());
        };
        commit_file(&root, "b.txt", "b\n", "c1")?;
        commit_file(&root, "c.txt", "c\n", "c2")?;
        let head = git_output(&root, &["rev-parse", "HEAD"])?;
        let (mut session, mut events) = repo_session(&root);

        let page = |events: &mut EventRx, id| {
            wait_answer(events, id, |event| match event {
                Event::VcsLog {
                    skip,
                    commits,
                    has_more,
                    labels,
                } => Some((skip, commits, has_more, labels)),
                _ => None,
            })
        };
        session.handle(RequestId(1), Command::VcsLog { skip: 0, limit: 2 });
        let (skip, commits, has_more, labels) =
            page(&mut events, RequestId(1)).ok_or("the first page never arrived")?;
        assert_eq!(skip, 0);
        let summaries: Vec<_> = commits.iter().map(|c| c.summary.as_str()).collect();
        assert_eq!(summaries, ["c2", "c1"]);
        assert!(has_more, "c0 is still beyond this page");
        assert!(labels.contains_key(&head), "the tip carries its ref labels");

        session.handle(RequestId(2), Command::VcsLog { skip: 2, limit: 2 });
        let (skip, commits, has_more, _) =
            page(&mut events, RequestId(2)).ok_or("the second page never arrived")?;
        assert_eq!(skip, 2);
        let summaries: Vec<_> = commits.iter().map(|c| c.summary.as_str()).collect();
        assert_eq!(summaries, ["c0"]);
        assert!(!has_more, "the history ends here");
        Ok(())
    }

    #[test]
    fn the_repository_snapshot_names_the_current_branch() -> TestResult {
        let Some((_dir, root)) = committed_repo()? else {
            // Skip: `git` is not on PATH.
            return Ok(());
        };
        let branch = git_output(&root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        let (mut session, mut events) = repo_session(&root);

        session.handle(RequestId(1), Command::RepositorySnapshot);
        let snapshot = wait_answer(&mut events, RequestId(1), |event| match event {
            Event::RepositorySnapshot { snapshot } => Some(snapshot),
            _ => None,
        });
        let snapshot = snapshot.ok_or("the snapshot never arrived")?;
        assert_eq!(snapshot.state.branch.as_deref(), Some(branch.as_str()));
        assert!(snapshot.branches.iter().any(|b| b.name == branch));
        assert!(snapshot.remotes.is_empty());
        assert!(snapshot.stashes.is_empty());
        Ok(())
    }

    /// The added lines of a prepared diff, in order.
    fn added_lines(change: &crate::api::PreparedChange) -> Vec<String> {
        change
            .diff
            .diff
            .hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .filter(|line| line.kind == karet_diff::LineKind::Add)
            .map(|line| line.content.clone())
            .collect()
    }

    type Prepared = (PathBuf, Result<Box<crate::api::PreparedChange>, String>);

    fn prepared(events: &mut EventRx, id: RequestId) -> Option<Prepared> {
        wait_answer(events, id, |event| match event {
            Event::DiffPrepared { path, result } => Some((path, result)),
            _ => None,
        })
    }

    #[test]
    fn prepare_diff_answers_with_the_diff_of_the_two_texts() -> TestResult {
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());

        session.handle(
            RequestId(1),
            Command::PrepareDiff {
                path: PathBuf::from("notes.txt"),
                old: "one\n".to_owned(),
                new: "one\ntwo\n".to_owned(),
            },
        );
        let (path, result) =
            prepared(&mut events, RequestId(1)).ok_or("PrepareDiff was never answered")?;
        assert_eq!(path, PathBuf::from("notes.txt"));
        assert_eq!(result.map(|change| added_lines(&change)), Ok(vec!["two".to_owned()]));
        Ok(())
    }

    #[test]
    fn diff_with_rev_compares_the_live_text_against_the_revision() -> TestResult {
        let Some((_dir, root)) = committed_repo()? else {
            // Skip: `git` is not on PATH.
            return Ok(());
        };
        let (mut session, mut events) = repo_session(&root);

        session.handle(
            RequestId(1),
            Command::DiffWithRev {
                path: root.join("a.txt"),
                rev: "HEAD".to_owned(),
                live: Some("hello\nunsaved\n".to_owned()),
            },
        );
        let (_, result) =
            prepared(&mut events, RequestId(1)).ok_or("DiffWithRev was never answered")?;
        assert_eq!(
            result.map(|change| added_lines(&change)),
            Ok(vec!["unsaved".to_owned()]),
            "the unsaved buffer is the new side, HEAD the old"
        );

        // A file the revision does not hold answers with a reason, not a diff.
        std::fs::write(root.join("fresh.txt"), "x\n")?;
        session.handle(
            RequestId(2),
            Command::DiffWithRev {
                path: root.join("fresh.txt"),
                rev: "HEAD".to_owned(),
                live: None,
            },
        );
        let (path, result) =
            prepared(&mut events, RequestId(2)).ok_or("DiffWithRev was never answered")?;
        assert_eq!(path, root.join("fresh.txt"));
        assert!(result.is_err(), "no old side exists at HEAD");
        Ok(())
    }

    /// A spontaneous or requested log page: its tag, offset, and summaries.
    type LogReload = (Option<RequestId>, usize, Vec<String>);

    /// Feed the session one filesystem event and collect the log reactions it
    /// emitted: the reconcile runs on the actor, so they are already queued.
    fn reconcile_after_fs_event(
        session: &mut Session,
        events: &mut EventRx,
        root: &std::path::Path,
    ) -> (Vec<Vec<String>>, Vec<LogReload>) {
        while events.try_recv().is_some() {}
        session.handle_fs_event(karet_watch::FsEvent {
            kind: karet_watch::FsEventKind::Modified,
            paths: vec![root.join(".git").join("HEAD")],
        });
        let summaries = |commits: &[karet_vcs::Commit]| {
            commits.iter().map(|c| c.summary.clone()).collect::<Vec<_>>()
        };
        let mut prepended = Vec::new();
        let mut reloaded = Vec::new();
        while let Some((id, event)) = events.try_recv() {
            match event {
                Event::VcsCommitsPrepended { commits } => prepended.push(summaries(&commits)),
                Event::VcsLog { skip, commits, .. } => {
                    reloaded.push((id, skip, summaries(&commits)));
                },
                _ => {},
            }
        }
        (prepended, reloaded)
    }

    #[test]
    fn a_new_commit_at_the_tip_is_prepended_alone() -> TestResult {
        let Some((_dir, root)) = committed_repo()? else {
            // Skip: `git` is not on PATH.
            return Ok(());
        };
        let (mut session, mut events) = repo_session(&root);
        commit_file(&root, "b.txt", "b\n", "c1")?;

        let (prepended, reloaded) = reconcile_after_fs_event(&mut session, &mut events, &root);
        assert_eq!(prepended, vec![vec!["c1".to_owned()]]);
        assert!(reloaded.is_empty(), "{reloaded:?}");

        // The tip has not moved since: a second event reconciles nothing.
        let (prepended, reloaded) = reconcile_after_fs_event(&mut session, &mut events, &root);
        assert!(prepended.is_empty() && reloaded.is_empty());
        Ok(())
    }

    /// An amend rewrites the tip: the old head is no longer in the history, so
    /// prepending the walk would leave the replaced commit in the client's log
    /// and duplicate everything below it. The log must reload instead.
    #[test]
    fn an_amended_tip_reloads_the_log_rather_than_prepending() -> TestResult {
        let Some((_dir, root)) = committed_repo()? else {
            // Skip: `git` is not on PATH.
            return Ok(());
        };
        commit_file(&root, "b.txt", "b\n", "c1")?;
        let (mut session, mut events) = repo_session(&root);
        git_in(&root, &["commit", "-q", "--amend", "-m", "c1 amended"])?;

        let (prepended, reloaded) = reconcile_after_fs_event(&mut session, &mut events, &root);
        assert!(prepended.is_empty(), "nothing is prepended: {prepended:?}");
        assert_eq!(
            reloaded,
            vec![(None, 0, vec!["c1 amended".to_owned(), "c0".to_owned()])],
            "a spontaneous first page replaces the client's log"
        );
        Ok(())
    }

    /// Moving the tip back to an ancestor brings no new commits, so the log
    /// reloads to drop the ones that left.
    #[test]
    fn a_tip_moved_to_an_ancestor_reloads_the_log() -> TestResult {
        let Some((_dir, root)) = committed_repo()? else {
            // Skip: `git` is not on PATH.
            return Ok(());
        };
        commit_file(&root, "b.txt", "b\n", "c1")?;
        let (mut session, mut events) = repo_session(&root);
        git_in(&root, &["reset", "-q", "--hard", "HEAD~1"])?;

        let (prepended, reloaded) = reconcile_after_fs_event(&mut session, &mut events, &root);
        assert!(prepended.is_empty(), "{prepended:?}");
        assert_eq!(reloaded, vec![(None, 0, vec!["c0".to_owned()])]);
        Ok(())
    }
