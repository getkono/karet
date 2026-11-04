use super::*;

#[test]
fn typing_filters_and_accept_opens() {
    let files = vec![
        ("app.rs".to_string(), PathBuf::from("/x/app.rs")),
        ("main.rs".to_string(), PathBuf::from("/x/main.rs")),
    ];
    let mut overlay = Overlay::quick_open(files);
    // Type "ma" -> only main.rs remains.
    overlay.push_char('m');
    overlay.push_char('a');
    assert_eq!(overlay.rows(), vec!["main.rs"]);
    match overlay.accept() {
        OverlayEvent::AcceptFile(p) => assert_eq!(p, PathBuf::from("/x/main.rs")),
        _ => unreachable!("accept opens the single match"),
    }
}

#[test]
fn quick_open_is_recognised_and_reselects_a_file() {
    let files = vec![
        ("a.rs".to_string(), PathBuf::from("/x/a.rs")),
        ("b.rs".to_string(), PathBuf::from("/x/b.rs")),
        ("c.rs".to_string(), PathBuf::from("/x/c.rs")),
    ];
    let mut overlay = Overlay::quick_open(files);
    assert!(overlay.is_quick_open());
    assert!(!Overlay::command_palette().is_quick_open());
    assert_eq!(overlay.selected_file(), Some(Path::new("/x/a.rs")));
    overlay.select_file(Path::new("/x/c.rs"));
    assert_eq!(overlay.selected_file(), Some(Path::new("/x/c.rs")));
    overlay.select_file(Path::new("/x/gone.rs"));
    assert_eq!(overlay.selected_file(), Some(Path::new("/x/c.rs")));
}

#[test]
fn pickers_are_told_apart_by_kind_not_title() {
    let quick_open = Overlay::quick_open(Vec::new());
    assert_eq!(quick_open.picker_kind(), Some(PickerKind::QuickOpen));
    assert_eq!(quick_open.quick_open_mode(), Some(QuickOpenMode::Files));
    // Another picker that happens to carry quick-open's title is not quick-open.
    let lookalike = Overlay::commands(quick_open.title().to_string(), vec![Command::Quit]);
    assert_eq!(lookalike.picker_kind(), Some(PickerKind::Commands));
    assert_eq!(lookalike.quick_open_mode(), None);
    assert!(!lookalike.is_quick_open());
    assert_eq!(Overlay::stash_form().picker_kind(), None);
}

#[test]
fn seam_roots_refill_only_the_seam_roots_picker() {
    let mut seam = Overlay::seam_roots(Vec::new());
    seam.set_seam_roots(vec![("karet".to_string(), PathBuf::from("/x/karet"))]);
    assert_eq!(seam.rows(), vec!["karet"]);
    // A different picker under the same title keeps its own rows.
    let title = seam.title().to_string();
    let mut lookalike = Overlay::commands(title, vec![Command::Quit]);
    let before: Vec<String> = lookalike.rows().into_iter().map(String::from).collect();
    lookalike.set_seam_roots(vec![("karet".to_string(), PathBuf::from("/x/karet"))]);
    assert_eq!(lookalike.rows(), before);
}

#[test]
fn recent_workspace_picker_accepts_a_root() {
    let items = vec![("~/src/karet".to_string(), PathBuf::from("/h/src/karet"))];
    let overlay = Overlay::recent_workspaces(items);
    assert!(!overlay.is_quick_open());
    match overlay.accept() {
        OverlayEvent::AcceptWorkspace(root) => assert_eq!(root, PathBuf::from("/h/src/karet")),
        _ => unreachable!("accept reopens on the workspace"),
    }
}

#[test]
fn palette_accepts_a_command() {
    let mut overlay = Overlay::command_palette();
    // "quit" filters to the Quit command.
    for c in "quit".chars() {
        overlay.push_char(c);
    }
    match overlay.accept() {
        OverlayEvent::AcceptCommand(cmd) => assert_eq!(cmd, Command::Quit),
        _ => unreachable!("accept runs the filtered command"),
    }
}

#[test]
fn diff_target_picker_filters_and_accepts_a_revision() {
    let items = vec![
        (
            "abc1234 first commit".to_string(),
            DiffTarget {
                rev: "abc1234deadbeef".to_string(),
                label: "abc1234".to_string(),
            },
        ),
        (
            "feature".to_string(),
            DiffTarget {
                rev: "feature".to_string(),
                label: "feature".to_string(),
            },
        ),
    ];
    let mut overlay = Overlay::diff_target("Open Changes: With Revision", items);
    assert_eq!(overlay.title(), "Open Changes: With Revision");
    for c in "feat".chars() {
        overlay.push_char(c);
    }
    assert_eq!(overlay.rows(), vec!["feature"]);
    match overlay.accept() {
        OverlayEvent::AcceptDiffTarget { rev, label } => {
            assert_eq!(rev, "feature");
            assert_eq!(label, "feature");
        },
        _ => unreachable!("accept picks the filtered revision"),
    }
}

#[test]
fn palette_rows_have_aligned_hints() {
    let overlay = Overlay::command_palette();
    assert_eq!(overlay.rows().len(), overlay.row_hints().len());
    // The Quit row carries its Ctrl+Q hint.
    let quit = overlay
        .rows()
        .iter()
        .position(|r| *r == Command::Quit.label())
        .expect("quit row present");
    assert_eq!(overlay.row_hints()[quit].as_deref(), Some("Ctrl+Q"));
}

#[test]
fn create_branch_form_exposes_every_common_control() {
    let mut overlay = Overlay::create_branch(vec!["origin".to_string()]);
    let rows = overlay.rows();
    assert!(rows.iter().any(|row| row.contains("Name")));
    assert!(rows.iter().any(|row| row.contains("Start point")));
    assert!(rows.iter().any(|row| row.contains("Switch now")));
    assert!(rows.iter().any(|row| row.contains("Publish remote")));
    assert!(rows.iter().any(|row| row.contains("Set upstream")));
    for character in "feature/test".chars() {
        overlay.push_char(character);
    }
    match overlay.accept() {
        OverlayEvent::AcceptCreateBranch(options) => {
            assert_eq!(options.name, "feature/test");
            assert_eq!(options.start_point, "HEAD");
            assert!(options.switch);
        },
        _ => unreachable!("branch form submits its options"),
    }
}

#[test]
fn stash_form_edits_message_and_toggles_options() {
    let mut overlay = Overlay::stash_form();
    for character in "work".chars() {
        overlay.push_char(character);
    }
    overlay.select_down();
    overlay.push_char(' ');
    overlay.select_down();
    overlay.push_char(' ');
    match overlay.accept() {
        OverlayEvent::AcceptStash(options) => {
            assert_eq!(options.message.as_deref(), Some("work"));
            assert!(options.include_untracked);
            assert!(options.keep_index);
        },
        _ => unreachable!("stash form submits its options"),
    }
}

#[test]
fn remote_branch_deletion_picker_preserves_remote_and_name() {
    let overlay =
        Overlay::delete_remote_branches(vec![("upstream".to_string(), "feature".to_string())]);
    match overlay.accept() {
        OverlayEvent::AcceptDeleteRemoteBranch { remote, branch } => {
            assert_eq!(remote, "upstream");
            assert_eq!(branch, "feature");
        },
        _ => unreachable!("remote branch picker submits both parts"),
    }
}
