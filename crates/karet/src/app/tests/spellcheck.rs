use super::support::*;
use crate::app::*;

fn spell_diagnostic(message: &str) -> Diagnostic {
    Diagnostic {
        range: Range {
            start: LineCol::new(0, 0),
            end: LineCol::new(0, 4),
        },
        severity: Severity::Warning,
        message: message.to_string(),
        source: Some("karet-spell".to_string()),
        code: Some("en_US".to_string()),
        code_href: None,
        tags: Vec::new(),
        related: Vec::new(),
    }
}

fn spelling_app(root: PathBuf, message: &str) -> App {
    let mut app = App::new(root, Vec::new(), Vec::new(), false);
    app.push_tab(text_tab("notes.md", "wrod\n"));
    if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
        *doc = Some(DocumentId(9));
    }
    app.docs
        .diagnostics
        .insert(DocumentId(9), vec![spell_diagnostic(message)]);
    app.last_frame.panes = vec![content_frame(
        &app,
        Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 5,
        },
    )];
    app
}

/// One-line editors have a three-cell gutter; column 4 is inside "wrod".
const ON_WROD: (u16, u16) = (4, 0);

fn right_click_at(app: &mut App, (column, row): (u16, u16)) {
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    });
}

fn open_spelling_menu(app: &mut App) {
    right_click_at(app, ON_WROD);
}

/// Whether the backend's answer has added "wrod" to the live dictionary.
fn word_added(app: &App) -> bool {
    app.settings
        .spellcheck
        .words
        .iter()
        .any(|word| word == "wrod")
}

fn double_click_at(app: &mut App, (column, row): (u16, u16)) {
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_editor_click(click);
    app.handle_editor_click(click);
}

#[test]
fn double_clicking_a_misspelled_word_selects_it_and_opens_no_menu() {
    let mut app = spelling_app(PathBuf::from("."), "Unknown word “wrod”; try word, rod");

    double_click_at(&mut app, ON_WROD);

    assert!(app.context_menu.is_none(), "the keys stay with the text");
    assert_eq!(
        app.tabs[app.active].editor.selection_range(),
        Some(Range {
            start: LineCol::new(0, 0),
            end: LineCol::new(0, 4),
        })
    );
}

#[test]
fn right_clicking_off_a_misspelled_word_offers_no_spelling_rows() {
    // Column 7 lies past the end of "wrod"; column 2 is the gutter beside it,
    // which a pointer lookup would clamp onto the word's first letter.
    for (point, what) in [((7, 0), "past the line end"), ((2, 0), "the gutter")] {
        let mut app = spelling_app(PathBuf::from("."), "Unknown word “wrod”; try word, rod");
        right_click_at(&mut app, point);

        let menu = app.context_menu.as_ref().expect("the pane menu");
        assert!(
            menu.entries().iter().all(|entry| !matches!(
                entry.action,
                ContextMenuAction::ReplaceSpelling { .. }
                    | ContextMenuAction::AddSpellingToDictionary { .. }
            )),
            "{what}"
        );
        assert_eq!(menu.entries()[0].command(), Some(Command::Cut), "{what}");
    }
}

#[test]
fn right_clicking_a_spell_warning_leads_the_menu_with_replacements_and_add_word() {
    let mut app = spelling_app(PathBuf::from("."), "Unknown word “wrod”; try word, rod");

    open_spelling_menu(&mut app);

    let labels: Vec<&str> = app
        .context_menu
        .as_ref()
        .map(|menu| {
            menu.entries()
                .iter()
                .filter_map(|entry| entry.label.as_deref())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        labels,
        vec![
            "Replace with “word”",
            "Replace with “rod”",
            "Add “wrod” to Project Dictionary",
            "Add “wrod” to User Dictionary",
        ]
    );
    // The spelling rows lead; the ordinary pane menu follows them.
    let commands: Vec<Command> = app
        .context_menu
        .as_ref()
        .map(|menu| menu.entries().iter().filter_map(|e| e.command()).collect())
        .unwrap_or_default();
    assert_eq!(
        commands.get(..3),
        Some(&[Command::Cut, Command::Copy, Command::Paste][..])
    );
    assert_eq!(
        app.context_menu
            .as_ref()
            .and_then(|menu| menu.entries()[4].command()),
        Some(Command::Cut)
    );
    // The right click moved the caret to the click, as it does anywhere else.
    assert_eq!(app.tabs[app.active].editor.selection_range(), None);
    let targets: Vec<DictionaryTarget> = app
        .context_menu
        .as_ref()
        .map(|menu| {
            menu.entries()
                .iter()
                .filter_map(|entry| match entry.action {
                    ContextMenuAction::AddSpellingToDictionary { target, .. } => Some(target),
                    ContextMenuAction::Command(_)
                    | ContextMenuAction::ReplaceSpelling { .. }
                    | ContextMenuAction::CodeAction { .. }
                    | ContextMenuAction::Heading => None,
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        targets,
        vec![DictionaryTarget::Project, DictionaryTarget::User]
    );
}

#[test]
fn warning_without_matches_renders_a_clear_empty_message() {
    let mut app = spelling_app(PathBuf::from("."), "Unknown word “wrod”");

    open_spelling_menu(&mut app);

    let menu = app.context_menu.as_ref().expect("spelling menu");
    assert_eq!(
        menu.entries()[0].label.as_deref(),
        Some("No similar words found")
    );
    assert!(!menu.entries()[0].enabled);
    assert_eq!(
        menu.entries()[1].label.as_deref(),
        Some("Add “wrod” to Project Dictionary")
    );
    assert_eq!(
        menu.entries()[2].label.as_deref(),
        Some("Add “wrod” to User Dictionary")
    );
    let painted = screen(&mut app, 80, 12).join("\n");
    assert!(painted.contains("No similar words found"), "{painted}");
}

#[test]
fn moving_the_pointer_over_the_menu_highlights_the_row_it_rests_on() {
    let mut app = spelling_app(PathBuf::from("."), "Unknown word “wrod”; try word, rod");
    open_spelling_menu(&mut app);
    // The menu records its rect when painted; hit-testing needs it.
    let _ = screen(&mut app, 80, 12);
    let rect = app
        .context_menu
        .as_ref()
        .map(|menu| menu.rect())
        .unwrap_or_default();

    let move_to = |app: &mut App, column: u16, row: u16| {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
    };

    // Row 2 of the menu body is "Add … to Project Dictionary".
    move_to(&mut app, rect.x + 1, rect.y + 3);
    assert_eq!(
        app.context_menu.as_ref().and_then(|menu| menu.hover()),
        Some(2)
    );

    move_to(&mut app, rect.x + 1, rect.y + 1);
    assert_eq!(
        app.context_menu.as_ref().and_then(|menu| menu.hover()),
        Some(0)
    );

    // Leaving the menu clears the accent rather than stranding it on a row.
    move_to(&mut app, rect.x + 1, rect.bottom() + 2);
    assert_eq!(
        app.context_menu.as_ref().and_then(|menu| menu.hover()),
        None
    );
}

#[test]
fn clicking_a_disabled_menu_row_still_refuses_it_rather_than_running_another() {
    let mut app = spelling_app(PathBuf::from("."), "Unknown word “wrod”");
    open_spelling_menu(&mut app);
    let _ = screen(&mut app, 80, 12);
    let rect = app
        .context_menu
        .as_ref()
        .map(|menu| menu.rect())
        .unwrap_or_default();

    // Row 0 is the disabled "No similar words found"; the initial selection sits
    // on row 1, so a click that fell through would add a dictionary word.
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + 1,
        row: rect.y + 1,
        modifiers: KeyModifiers::NONE,
    });

    assert!(app.context_menu.is_some(), "the menu stays open");
    assert_eq!(
        app.context_menu.as_ref().map(|menu| menu.selected()),
        Some(1),
        "the cursor never rests on the disabled row"
    );
    assert_eq!(
        last_message(&app).as_deref(),
        Some("The dictionary has no close matches"),
        "the refused row still explains itself"
    );
}

#[test]
fn accepting_a_spelling_suggestion_is_one_atomic_edit() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = spelling_app(PathBuf::from("."), "Unknown word “wrod”; try word");
    app.backend = Some(backend.clone());
    open_spelling_menu(&mut app);

    app.accept_context_menu();

    assert_eq!(code_tab_text(&app), "word\n");
    let changes = backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter(|(_, command)| {
                    matches!(
                        command,
                        SessionCommand::Document(DocumentCommand::ApplyChange { .. })
                    )
                })
                .count()
        })
        .unwrap_or_default();
    assert_eq!(changes, 1);
}

#[tokio::test]
async fn add_word_updates_an_existing_project_settings_file_without_prompt()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join(".git"))?;
    write_file(
        dir.path(),
        ".karet/setting.jsonc",
        b"{\n  // keep\n  \"spellcheck\": { \"enabled\": true }\n}\n",
    );
    // The dictionary write runs on the backend; the answering event closes
    // the loop.
    let (local_backend, _snaps) = local(SessionConfig {
        roots: vec![dir.path().to_path_buf()],
        ..SessionConfig::default()
    });
    let backend: Arc<dyn Backend> = Arc::new(local_backend);
    let mut events = backend.take_events().expect("backend event stream");
    let mut app = spelling_app(dir.path().to_path_buf(), "Unknown word “wrod”");
    app.backend = Some(backend);
    open_spelling_menu(&mut app);
    if let Some(menu) = app.context_menu.as_mut() {
        assert!(menu.select_row(1));
    }

    app.accept_context_menu();
    pump_until(&mut app, &mut events, word_added).await;

    assert!(app.overlay.is_none());
    let text = std::fs::read_to_string(dir.path().join(".karet/setting.jsonc"))?;
    assert!(text.contains("// keep"));
    assert!(text.contains("\"wrod\""));
    Ok(())
}

#[tokio::test]
async fn add_word_requires_confirmation_before_creating_project_settings()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join(".git"))?;
    let path = dir.path().join(".karet/setting.jsonc");
    let (local_backend, _snaps) = local(SessionConfig {
        roots: vec![dir.path().to_path_buf()],
        ..SessionConfig::default()
    });
    let backend: Arc<dyn Backend> = Arc::new(local_backend);
    let mut events = backend.take_events().expect("backend event stream");
    let mut app = spelling_app(dir.path().to_path_buf(), "Unknown word “wrod”");
    app.backend = Some(backend);
    open_spelling_menu(&mut app);
    if let Some(menu) = app.context_menu.as_mut() {
        assert!(menu.select_row(1));
    }

    app.accept_context_menu();
    pump_until(&mut app, &mut events, |app| app.confirm.is_some()).await;

    assert!(!path.exists());
    let dialog = app.confirm.as_ref().expect("creation confirmation");
    assert!(
        dialog.title.contains("project dictionary"),
        "{}",
        dialog.title
    );
    assert!(
        dialog.body.source().contains(&path.display().to_string()),
        "the body names the file it would create"
    );

    // Enter alone takes the safe answer and creates nothing.
    send_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.confirm.is_none(), "Enter answers the dialog");
    assert!(!path.exists(), "the safe answer created no file");
    assert!(!word_added(&app), "the safe answer added no word");

    // Asking again doubles as a barrier: the backend answers in order, so a
    // file the cancelled dialog had created would show here as a direct add
    // instead of a second creation prompt.
    open_spelling_menu(&mut app);
    if let Some(menu) = app.context_menu.as_mut() {
        assert!(menu.select_row(1));
    }
    app.accept_context_menu();
    pump_until(&mut app, &mut events, |app| app.confirm.is_some()).await;
    assert!(!path.exists(), "the safe answer created no file");
    send_key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    send_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    pump_until(&mut app, &mut events, word_added).await;
    assert!(path.exists());
    assert!(std::fs::read_to_string(path)?.contains("\"wrod\""));
    Ok(())
}

#[test]
fn debounced_spell_warning_replaces_pending_lsp_with_spelling_completions() {
    let (backend, mut app) = completion_app("wrod\n", LineCol::new(0, 4));
    app.trigger_completion(false);
    assert_eq!(completion_requests(&backend).len(), 1);
    assert!(app.completion.pending.is_some());

    app.on_backend_event(
        None,
        SessionEvent::DiagnosticsPublished {
            doc: DocumentId(9),
            diagnostics: vec![spell_diagnostic("Unknown word “wrod”; try word, wood")],
        },
    );

    assert!(app.completion.pending.is_none());
    assert!(matches!(
        app.completion.popup.as_ref().map(|ui| ui.mode),
        Some(crate::completion::CompletionMode::Spelling {
            caret: LineCol { line: 0, col: 4 }
        })
    ));
    assert_eq!(app.completion_filter().as_deref(), Some(""));
    assert_eq!(app.completion_ranked(), Some(vec![0, 1]));

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(code_tab_text(&app), "word\n");
}

#[test]
fn manual_completion_still_offers_spelling_when_auto_trigger_is_disabled() {
    let (backend, mut app) = completion_app("wrod\n", LineCol::new(0, 4));
    app.settings.editor.completion.auto_trigger = false;
    app.on_backend_event(
        None,
        SessionEvent::DiagnosticsPublished {
            doc: DocumentId(9),
            diagnostics: vec![spell_diagnostic("Unknown word “wrod”; try word")],
        },
    );
    assert!(app.completion.popup.is_none());

    app.trigger_completion(true);

    assert!(matches!(
        app.completion.popup.as_ref().map(|ui| ui.mode),
        Some(crate::completion::CompletionMode::Spelling { .. })
    ));
    assert!(
        completion_requests(&backend).is_empty(),
        "local spelling candidates avoid an unnecessary backend request"
    );
}

#[test]
fn spelling_completion_dismisses_as_soon_as_the_word_changes() {
    let (_backend, mut app) = completion_app("wrod\n", LineCol::new(0, 4));
    app.on_backend_event(
        None,
        SessionEvent::DiagnosticsPublished {
            doc: DocumentId(9),
            diagnostics: vec![spell_diagnostic("Unknown word “wrod”; try word")],
        },
    );
    assert!(app.completion.popup.is_some());

    app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));

    assert!(app.completion.popup.is_none());
}
