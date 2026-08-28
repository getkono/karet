use super::support::*;
use crate::app::*;

#[test]
fn with_settings_applies_the_workbench_slice() {
    use karet_session::config::schema::IconStyleSetting;
    use karet_session::config::schema::StartupPanel;

    let mut settings = Settings::default();
    settings.workbench.icon_style = IconStyleSetting::Ascii;
    settings.workbench.startup_panel = StartupPanel::SourceControl;

    let app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false)
        .with_settings(settings, Vec::new());

    assert_eq!(app.icon_style, IconStyle::Ascii);
    assert_eq!(app.sidebar_panel, SidebarPanel::SourceControl);
    assert!(app.sidebar_visible);
}

#[test]
fn with_settings_none_panel_collapses_the_sidebar() {
    use karet_session::config::schema::StartupPanel;

    let mut settings = Settings::default();
    settings.workbench.startup_panel = StartupPanel::None;
    let app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false)
        .with_settings(settings, Vec::new());
    assert!(!app.sidebar_visible);
}

/// Raise the install prompt for Texlab, enabled or not.
fn offer_texlab(enabled: bool) -> App {
    let mut app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false);
    app.on_backend_event(
        None,
        SessionEvent::LanguageServerInstallRequired {
            server: karet_session::LanguageServerId::Texlab,
            language: "latex".into(),
            enabled,
        },
    );
    app
}

#[test]
fn a_missing_language_server_offers_to_install_and_declines_by_default() {
    let mut app = offer_texlab(true);
    let painted = screen(&mut app, 100, 20).join("\n");
    assert!(painted.contains("Install"), "{painted}");
    assert!(
        painted.contains("latex"),
        "the language is named: {painted}"
    );
    assert!(painted.contains("Not now"), "{painted}");

    // Installing spends bandwidth the user did not ask to spend, so the answer
    // Enter takes must be the one that spends none.
    let selected = app
        .confirm
        .as_ref()
        .and_then(|d| d.selected_choice())
        .map(|choice| choice.action.clone());
    assert_eq!(selected, Some(crate::app::confirm::ConfirmAction::Cancel));
}

#[test]
fn a_disabled_language_server_is_asked_about_differently() {
    let mut app = offer_texlab(false);
    let painted = screen(&mut app, 100, 20).join("\n");
    assert!(
        painted.contains("Enable and install"),
        "turning it on is part of the question: {painted}"
    );
}

#[test]
fn the_install_prompt_offers_a_permanent_refusal() {
    let app = offer_texlab(true);
    let labels: Vec<String> = app
        .confirm
        .as_ref()
        .map(|d| {
            d.choices
                .entries
                .iter()
                .map(|entry| entry.label.clone().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        labels.iter().any(|label| label.starts_with("Never ask")),
        "{labels:?}"
    );
}

#[test]
fn update_plan_displays_exact_versions_before_approval() {
    let mut app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false);
    app.on_backend_event(
        Some(RequestId(8)),
        SessionEvent::LanguageServerUpdatePlan {
            plan: karet_session::LanguageServerPlanId(3),
            changes: vec![karet_session::LanguageServerChange {
                server: karet_session::LanguageServerId::Texlab,
                current: Some("5.25.0".into()),
                target: "5.26.0".into(),
                download_bytes: Some(1),
            }],
        },
    );
    let painted = screen(&mut app, 100, 16).join("\n");
    assert!(painted.contains("5.25.0"), "{painted}");
    assert!(painted.contains("5.26.0"), "{painted}");
    assert!(
        painted.contains("Keep current versions"),
        "the update is declined by default: {painted}"
    );
}

#[test]
fn live_config_preserves_cli_icons_and_current_sidebar_state() {
    use karet_session::config::schema::IconStyleSetting;
    use karet_session::config::schema::StartupPanel;

    let mut app =
        App::new(PathBuf::from("."), Vec::new(), Vec::new(), false).with_icons(IconStyle::Ascii);
    app.sidebar_panel = SidebarPanel::SourceControl;
    app.sidebar_visible = true;

    let mut settings = Settings::default();
    settings.editor.tab_size = 8;
    settings.workbench.icon_style = IconStyleSetting::Unicode;
    settings.workbench.startup_panel = StartupPanel::None;
    app.on_backend_event(
        None,
        SessionEvent::ConfigChanged {
            report: Box::new(LoadedConfig::from_settings(settings)),
        },
    );

    assert_eq!(app.settings.editor.tab_size, 8);
    assert_eq!(
        app.icon_style,
        IconStyle::Ascii,
        "CLI override remains authoritative"
    );
    assert_eq!(app.sidebar_panel, SidebarPanel::SourceControl);
    assert!(
        app.sidebar_visible,
        "startupPanel is not replayed on reload"
    );
}

#[test]
fn open_startup_goto_positions_caret_and_focuses_editor() {
    let dir = test_dir("goto");
    write_file(
        &dir,
        "src/main.rs",
        b"fn main() {\n    println!(\"hi\");\n}\n",
    );
    let path = dir.join("src/main.rs");

    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.open_startup_goto(&path, 2, 5);

    // The file opened as a code tab, focused, with the caret at 0-based (1, 4).
    assert!(matches!(app.tabs[app.active].kind, TabKind::Code { .. }));
    assert_eq!(app.focus, Focus::Editor);
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(1, 4));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn open_startup_goto_clamps_out_of_range_target() {
    let dir = test_dir("goto-clamp");
    write_file(&dir, "a.txt", b"one\ntwo\n");
    let path = dir.join("a.txt");

    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    // Line far past the end and a large column clamp into the buffer rather than
    // panicking or landing off the end.
    app.open_startup_goto(&path, 9999, 9999);
    let caret = app.tabs[app.active].editor.cursor();
    assert!(
        caret.line <= 2,
        "caret line {} should clamp within the 2-line buffer",
        caret.line
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn open_startup_split_creates_a_second_pane_with_the_file() {
    let dir = test_dir("split");
    write_file(&dir, "a.rs", b"fn a() {}\n");
    write_file(&dir, "b.rs", b"fn b() {}\n");

    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.main_rect = Rect::new(0, 0, 120, 40);
    app.open_initial(&dir.join("a.rs"));
    app.open_startup_split(&dir.join("b.rs"));

    assert_eq!(app.layout.panes().len(), 2, "the split adds a second pane");
    // The new pane is focused and holds exactly the split file.
    assert_eq!(app.focus, Focus::Editor);
    assert_eq!(app.tabs.len(), 1);
    assert_eq!(
        app.tabs[app.active].path(),
        Some(dir.join("b.rs")).as_deref()
    );
    // The first pane still holds the originally-opened file.
    let stored: Vec<_> = app
        .stored
        .values()
        .flat_map(|p| p.tabs.iter())
        .filter_map(Tab::path)
        .collect();
    assert_eq!(stored, vec![dir.join("a.rs").as_path()]);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn open_startup_split_chains_panes_left_to_right() {
    let dir = test_dir("split-chain");
    write_file(&dir, "a.rs", b"fn a() {}\n");
    write_file(&dir, "b.rs", b"fn b() {}\n");
    write_file(&dir, "c.rs", b"fn c() {}\n");

    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.main_rect = Rect::new(0, 0, 200, 40);
    app.open_initial(&dir.join("a.rs"));
    app.open_startup_split(&dir.join("b.rs"));
    app.open_startup_split(&dir.join("c.rs"));

    assert_eq!(app.layout.panes().len(), 3);
    // The last split pane is focused and shows the last file.
    assert_eq!(
        app.tabs[app.active].path(),
        Some(dir.join("c.rs")).as_deref()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn open_startup_split_falls_back_to_a_tab_when_there_is_no_room() {
    let dir = test_dir("split-narrow");
    write_file(&dir, "a.rs", b"fn a() {}\n");
    write_file(&dir, "b.rs", b"fn b() {}\n");

    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    // Too narrow for two panes at the minimum pane width.
    app.main_rect = Rect::new(0, 0, 12, 10);
    app.open_initial(&dir.join("a.rs"));
    app.open_startup_split(&dir.join("b.rs"));

    assert_eq!(app.layout.panes().len(), 1, "no second pane is created");
    assert_eq!(app.tabs.len(), 2, "the file still opens, as a tab");
    assert_eq!(
        app.tabs[app.active].path(),
        Some(dir.join("b.rs")).as_deref()
    );
    // The degradation is surfaced, not silent.
    assert!(
        app.notifications
            .active()
            .iter()
            .any(|n| n.title.contains("--split")),
        "a startup notification explains the fallback"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn open_startup_diff_opens_a_text_diff_tab() {
    let dir = test_dir("cli-diff");
    write_file(&dir, "old.rs", b"fn a() {}\n");
    write_file(&dir, "new.rs", b"fn b() {}\n");

    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.open_startup_diff(
        &dir.join("old.rs"),
        &dir.join("new.rs"),
        Some("fn a() {}\n".to_string()),
        Some("fn b() {}\n".to_string()),
    );

    // The tab is reserved immediately; the backend prepares the diff after
    // attach (the PrepareDiff command is queued with the captured texts).
    let view = app.tabs[app.active].view;
    match &app.tabs[app.active].kind {
        TabKind::Diff {
            path,
            file,
            loading_since,
            ..
        } => {
            assert!(file.is_none(), "the diff loads via the backend");
            assert!(loading_since.is_some());
            assert_eq!(*path, dir.join("new.rs"));
        },
        _ => panic!("expected a diff tab"),
    }
    assert_eq!(app.tabs[app.active].title, "old.rs ↔ new.rs");
    assert_eq!(app.focus, Focus::Editor);
    assert!(
        app.pending_startup_diffs
            .iter()
            .any(|(pending_view, path, old, new)| {
                *pending_view == view
                    && *path == dir.join("new.rs")
                    && old == "fn a() {}\n"
                    && new == "fn b() {}\n"
            }),
        "the texts are queued for the backend"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn open_startup_diff_marks_a_non_utf8_side_binary() {
    let dir = test_dir("cli-diff-bin");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    // A `None` side is what main.rs passes for non-UTF-8 bytes.
    app.open_startup_diff(
        &dir.join("a.bin"),
        &dir.join("b.bin"),
        None,
        Some("text\n".to_string()),
    );

    match &app.tabs[app.active].kind {
        TabKind::Diff { file, .. } => {
            let file = file.as_ref().expect("a binary pair needs no backend");
            assert!(file.change.diff.is_binary());
            assert_eq!(file.line_stats(), (0, 0));
        },
        _ => panic!("expected a diff tab"),
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn open_startup_diff_same_file_name_keeps_a_single_title() {
    let dir = test_dir("cli-diff-title");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.open_startup_diff(
        &dir.join("v1/config.toml"),
        &dir.join("v2/config.toml"),
        Some("a = 1\n".to_string()),
        Some("a = 2\n".to_string()),
    );
    assert_eq!(app.tabs[app.active].title, "config.toml");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn apply_startup_command_dispatches_in_order() {
    // The pair [SelectPanel(Search), ToggleSidebar] is order-observable: run in
    // this order the panel is Search and the sidebar ends hidden (SelectPanel
    // shows it, ToggleSidebar then hides it); reversed it would end visible.
    let mut app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false);
    assert!(app.sidebar_visible);
    app.apply_startup_command(Command::SelectPanel(SidebarPanel::Search));
    app.apply_startup_command(Command::ToggleSidebar);
    assert_eq!(app.sidebar_panel, SidebarPanel::Search);
    assert!(
        !app.sidebar_visible,
        "ToggleSidebar must run after SelectPanel"
    );

    // The reversed order ends with the sidebar visible, proving order matters.
    let mut app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false);
    app.apply_startup_command(Command::ToggleSidebar);
    app.apply_startup_command(Command::SelectPanel(SidebarPanel::Search));
    assert!(app.sidebar_visible);
}

#[test]
fn apply_startup_command_opens_views() {
    // A view-affecting palette command works from the startup path: SplitRight
    // creates a second pane synchronously (no backend round-trip needed).
    let mut app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false);
    assert_eq!(app.layout.panes().len(), 1);
    app.apply_startup_command(Command::SplitRight);
    assert_eq!(
        app.layout.panes().len(),
        2,
        "SplitRight should create a second pane"
    );
}

#[test]
fn bad_theme_path_becomes_a_diagnostic_and_keeps_default() {
    let mut settings = Settings::default();
    settings.workbench.color_theme = "/no/such/theme.json".to_string();
    let app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false)
        .with_settings(settings, Vec::new());
    // The default (dark) theme is retained and the failure is queued as a warning.
    assert_eq!(app.config_diagnostics.len(), 1);
    assert!(app.config_diagnostics[0].message.contains("theme"));
}

#[test]
fn load_theme_resolves_the_builtin_dark() {
    assert!(load_theme("dark").is_ok());
    assert!(load_theme("").is_ok());
    // Unsupported theme formats (e.g. TextMate) are rejected, not half-loaded.
    assert!(load_theme("/definitely/missing.tmTheme").is_err());
}

#[cfg(feature = "pdf")]
#[test]
fn outline_panel_toggles_and_jumps_to_a_bookmarked_page() {
    // A 2-page PDF whose single bookmark targets the second page (index 1). Like
    // the karet-pdf fixtures, it has no xref table, so hayro parses it via its
    // brute-force fallback.
    const PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj<</Type/Catalog/Pages 2 0 R/Outlines 5 0 R>>endobj\n\
2 0 obj<</Type/Pages/Kids[3 0 R 4 0 R]/Count 2>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]>>endobj\n\
4 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]>>endobj\n\
5 0 obj<</Type/Outlines/First 6 0 R/Last 6 0 R/Count 1>>endobj\n\
6 0 obj<</Title(Page Two)/Parent 5 0 R/Dest[4 0 R/Fit]>>endobj\n\
trailer<</Size 7/Root 1 0 R>>\n%%EOF";
    let Ok(doc) = karet_pdf::Document::load(PDF.to_vec()) else {
        return;
    };
    let page_count = doc.page_count();
    let outline = doc.outline();
    let mut app = app();
    app.tabs.push(Tab::new(
        "doc.pdf",
        TabKind::Document {
            path: PathBuf::from("doc.pdf"),
            doc,
            page_count,
            page: 0,
            rendered: None,
            outline,
        },
    ));
    app.active = app.tabs.len() - 1;

    // The panel is populated from the bookmark.
    let rows = app.active_outline_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows.first().map(|r| r.label.as_str()), Some("Page Two"));

    // Toggling shows and focuses the panel (it has content).
    app.dispatch(Command::ToggleOutline);
    assert!(app.outline.visible);
    assert_eq!(app.focus, Focus::Outline);

    // Activating the bookmark jumps the document to its page.
    app.dispatch(Command::OutlineActivate);
    let page = match app.tabs.get(app.active).map(|t| &t.kind) {
        Some(TabKind::Document { page, .. }) => Some(*page),
        _ => None,
    };
    assert_eq!(page, Some(1));

    // Toggling again hides the panel and returns focus to the editor.
    app.dispatch(Command::ToggleOutline);
    assert!(!app.outline.visible);
    assert_eq!(app.focus, Focus::Editor);
}

#[test]
fn sidebar_resize_sets_width_and_collapses_below_min() {
    let mut app = app();
    app.sidebar_rect = Rect::new(0, 0, DEFAULT_SIDEBAR_WIDTH, 20);
    app.sidebar_resizing = true;
    // Dragging the divider to column 45 widens the sidebar.
    app.resize_sidebar_to(45);
    assert_eq!(app.sidebar_width, 45);
    assert!(app.sidebar_visible);
    // Dragging narrower than the minimum collapses it and ends the drag, leaving
    // the last valid width intact so re-showing restores a sensible size.
    app.resize_sidebar_to(SIDEBAR_MIN_WIDTH - 1);
    assert!(!app.sidebar_visible);
    assert!(!app.sidebar_resizing);
    assert_eq!(app.sidebar_width, 45);
}

#[test]
fn scm_commit_divider_resizes_and_clamps() {
    let mut app = app();
    // A 20-row list area (rows 2..22); the changes list starts at row 2.
    app.sidebar_content_rect = Rect::new(0, 2, 30, 20);
    app.scm_ui.changes_rect = Rect::new(0, 2, 30, 10);
    // Drag the divider up to row 12 → commits region = rows 13..22 = 9 rows.
    app.resize_scm_commits_to(12);
    assert_eq!(app.scm_ui.commits_h, 9);
    // Dragging past the bottom clamps so the commits region keeps the minimum.
    app.resize_scm_commits_to(30);
    assert_eq!(app.scm_ui.commits_h, MIN_SCM_REGION);
    // Dragging to the very top clamps so the changes region keeps room too.
    app.resize_scm_commits_to(0);
    assert_eq!(app.scm_ui.commits_h, 20 - (MIN_SCM_REGION + 1));
}

#[test]
fn pointer_shape_hint_tracks_divider_hover_when_supported() {
    let mut app = app();
    app.caps.pointer_shapes = true;
    app.sidebar_visible = true;
    app.sidebar_divider_x = 30;

    let moved = |col, row| MouseEvent {
        kind: MouseEventKind::Moved,
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    };
    app.update_pointer_shape_hint(&moved(30, 5));
    assert_eq!(app.caps.pointer_shape, Some("col-resize"));

    app.update_pointer_shape_hint(&moved(10, 5));
    assert_eq!(
        app.caps.pointer_shape, None,
        "moving off the divider resets to the default shape"
    );
}

#[test]
fn pointer_shape_hint_is_a_no_op_when_unsupported() {
    let mut app = app();
    // `pointer_shapes_supported` defaults to false (never confirmed at startup).
    app.sidebar_visible = true;
    app.sidebar_divider_x = 30;
    app.update_pointer_shape_hint(&MouseEvent {
        kind: MouseEventKind::Moved,
        column: 30,
        row: 5,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.caps.pointer_shape, None,
        "an unconfirmed terminal must never get a pointer-shape hint"
    );
}

#[test]
fn graphical_cursor_requires_kitty_keyboard_and_graphics() {
    let mut app = app();
    app.caps.graphics = GraphicsProtocol::Kitty;
    app.caps.kitty_graphics = true;

    assert!(!app.graphical_cursor_compatible());

    app.caps.kitty_keyboard = true;
    assert!(app.graphical_cursor_compatible());

    app.caps.graphics = GraphicsProtocol::Halfblocks;
    assert!(
        !app.graphical_cursor_compatible(),
        "the graphical cursor must only ride the Kitty graphics path"
    );
}

#[test]
fn graphical_cursor_blink_schedules_a_repaint_when_active() {
    let mut app = app();
    app.push_tab(text_tab("t.rs", "x"));
    app.focus = Focus::Editor;
    app.editor_rect = Rect::new(0, 0, 20, 5);
    app.caps.graphics = GraphicsProtocol::Kitty;
    app.caps.kitty_graphics = true;
    app.caps.kitty_keyboard = true;

    let wake = app.next_wake().expect("an active graphical cursor blinks");
    assert!(wake <= GRAPHICS_CARET_BLINK_INTERVAL && wake > Duration::ZERO);
}

#[test]
fn graphical_cursor_is_suppressed_during_the_hidden_blink_phase() {
    let mut app = app();
    app.push_tab(text_tab("t.rs", "x"));
    app.focus = Focus::Editor;
    app.editor_rect = Rect::new(0, 0, 20, 5);
    app.caps.graphics = GraphicsProtocol::Kitty;
    app.caps.kitty_graphics = true;
    app.caps.kitty_keyboard = true;

    assert!(app.active_graphics_caret().is_some());
    app.graphics_caret_blink_epoch = Instant::now() - GRAPHICS_CARET_BLINK_INTERVAL;
    assert_eq!(app.active_graphics_caret(), None);
    assert!(
        app.active_graphics_caret_position().is_some(),
        "blink hides a valid caret without losing its placement"
    );
}

#[test]
fn queued_startup_commands_wait_for_the_active_document() {
    // `--command` is queued at construction and drained once the backend can
    // serve it; a code tab still waiting for its document id holds the queue.
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.push_tab(text_tab("main.rs", "fn main() {}\n"));
    app.focus = Focus::Editor;
    app.startup_commands = vec![Command::SelectPanel(SidebarPanel::Search)];

    app.run_startup_commands_when_ready();
    assert_eq!(
        app.startup_commands.len(),
        1,
        "a pending code tab holds the queue"
    );

    let idx = app.active;
    if let TabKind::Code { doc, .. } = &mut app.tabs[idx].kind {
        *doc = Some(DocumentId(9));
    }
    app.run_startup_commands_when_ready();
    assert!(app.startup_commands.is_empty(), "the queue drains once");
    assert_eq!(app.sidebar_panel, SidebarPanel::Search);
}

#[test]
fn queued_startup_commands_run_at_once_without_a_code_tab() {
    // Nothing to wait for: a session with no open document must not strand its
    // `--command` work forever.
    let mut app = app();
    app.backend = Some(Arc::new(RecordingBackend::new()));
    app.startup_commands = vec![Command::SelectPanel(SidebarPanel::Search)];

    app.run_startup_commands_when_ready();

    assert!(app.startup_commands.is_empty());
    assert_eq!(app.sidebar_panel, SidebarPanel::Search);
}
