    /// A single focused-pane frame whose content covers `rect`, so editor-click
    /// tests route through the pane hit-testing.
    fn content_frame(app: &App, rect: Rect) -> PaneFrame {
        PaneFrame {
            pane: app.focus_pane(),
            tabstrip_rect: Rect::default(),
            tab_hits: Vec::new(),
            breadcrumb_rect: Rect::default(),
            breadcrumb_hits: Vec::new(),
            content_rect: rect,
            commit_file_hits: Vec::new(),
        }
    }

    fn code_tab(name: &str) -> Tab {
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

    #[test]
    fn tab_navigation_wraps_and_jumps() {
        let mut app = app();
        app.push_tab(code_tab("a.rs"));
        app.push_tab(code_tab("b.rs"));
        app.push_tab(code_tab("c.rs"));
        assert_eq!(app.active, 2);
        app.next_tab();
        assert_eq!(app.active, 0, "next wraps to the first tab");
        app.prev_tab();
        assert_eq!(app.active, 2, "prev wraps to the last tab");
        app.go_to_tab(1);
        assert_eq!(app.active, 0);
        app.go_to_tab(9);
        assert_eq!(app.active, 2, "9 selects the last tab");
    }

    #[test]
    fn move_active_tab_reorders_and_clamps() {
        let mut app = app();
        app.push_tab(code_tab("a.rs"));
        app.push_tab(code_tab("b.rs"));
        app.active = 0;
        app.move_active_tab(1);
        assert_eq!(app.tabs[1].title, "a.rs");
        assert_eq!(app.active, 1);
        app.move_active_tab(1); // already last: clamped, no change
        assert_eq!(app.active, 1);
    }

    fn text_tab(name: &str, text: &str) -> Tab {
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

    #[test]
    fn wrap_mode_uses_file_defaults_and_global_overrides() {
        let markdown = text_tab("notes.md", "a long prose line");
        let rust = text_tab("main.rs", "fn main() {}");
        assert!(effective_word_wrap(&markdown, None));
        assert!(!effective_word_wrap(&rust, None));
        assert!(!effective_word_wrap(&markdown, Some(false)));
        assert!(effective_word_wrap(&rust, Some(true)));
    }

    #[test]
    fn word_wrap_resolves_against_the_tab_language() {
        let settings = Settings {
            editor: serde_json::from_str(
                r#"{
                    "wordWrap": false,
                    "[rust]": { "wordWrap": true }
                }"#,
            )
            .unwrap_or_default(),
            ..Settings::default()
        };
        let rust = text_tab("main.rs", "fn main() {}");
        let resolved = settings
            .editor
            .for_language(tab_language(&rust))
            .word_wrap();
        assert!(effective_word_wrap(&rust, resolved));

        let mut python = text_tab("main.py", "print('hi')");
        if let TabKind::Code { language, .. } = &mut python.kind {
            *language = "Python";
        }
        let resolved = settings
            .editor
            .for_language(tab_language(&python))
            .word_wrap();
        assert!(!effective_word_wrap(&python, resolved));
    }

    #[test]
    fn horizontal_mouse_events_scroll_only_overflow_views() {
        let mut app = app();
        app.sidebar_visible = false;
        app.push_tab(text_tab(
            "main.rs",
            "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz\nsecond\nthird\nfourth",
        ));
        let _ = screen(&mut app, 24, 8);
        let column = app.editor_rect.x.saturating_add(5);
        let row = app.editor_rect.y;
        let mouse = |kind, modifiers| MouseEvent {
            kind,
            column,
            row,
            modifiers,
        };

        app.handle_mouse(mouse(MouseEventKind::ScrollRight, KeyModifiers::NONE));
        assert_eq!(app.tabs[app.active].editor.scroll_col, 3);
        app.handle_mouse(mouse(MouseEventKind::ScrollUp, KeyModifiers::SHIFT));
        assert_eq!(app.tabs[app.active].editor.scroll_col, 0);
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, KeyModifiers::SHIFT));
        assert_eq!(app.tabs[app.active].editor.scroll_col, 3);
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, KeyModifiers::NONE));
        assert_eq!(app.tabs[app.active].editor.scroll_line, 3);

        app.tabs[app.active] = text_tab("notes.md", "prose that is much wider than the pane");
        let _ = screen(&mut app, 24, 8);
        app.handle_mouse(mouse(MouseEventKind::ScrollRight, KeyModifiers::NONE));
        assert_eq!(app.tabs[app.active].editor.scroll_col, 0);
    }

    /// An app with one Markdown code tab, in a pane wide enough to split.
    fn markdown_app(text: &str) -> App {
        let mut app = app();
        let mut tab = text_tab("notes.md", text);
        if let TabKind::Code { language, .. } = &mut tab.kind {
            *language = "Markdown";
        }
        app.push_tab(tab);
        app.main_rect = Rect::new(0, 0, 80, 24);
        app
    }

    /// The preview tab of the only stored (non-focused) pane, if any.
    fn stored_preview(app: &App) -> Option<&Tab> {
        app.stored
            .values()
            .flat_map(|pane| pane.tabs.iter())
            .find(|t| matches!(t.kind, TabKind::MarkdownPreview { .. }))
    }

    #[test]
    fn markdown_preview_opens_a_pane_to_the_side_and_keeps_focus_on_the_source() {
        let mut app = markdown_app("# Title\n\nbody\n");
        let source_view = app.tabs[app.active].view;
        let source_pane = app.focus_pane();

        app.dispatch(Command::MarkdownPreviewSide);

        assert_eq!(app.layout.pane_count(), 2);
        assert_eq!(app.focus_pane(), source_pane, "focus stays in the editor");
        assert!(matches!(app.tabs[app.active].kind, TabKind::Code { .. }));
        let preview = stored_preview(&app).expect("a preview tab in the new pane");
        assert!(App::previews_view(preview, source_view));
        assert_eq!(preview.title, "Preview notes.md");
    }

    #[test]
    fn markdown_preview_is_a_no_op_on_a_non_markdown_tab() {
        let mut app = app();
        app.push_tab(text_tab("main.rs", "fn main() {}"));
        app.main_rect = Rect::new(0, 0, 80, 24);

        app.dispatch(Command::MarkdownPreviewSide);

        assert_eq!(app.layout.pane_count(), 1, "no pane was opened");
        assert!(stored_preview(&app).is_none());
        assert!(app.status.is_some(), "the refusal is surfaced, not silent");
    }

    #[test]
    fn re_invoking_markdown_preview_reveals_the_existing_one() {
        let mut app = markdown_app("# Title\n");
        app.dispatch(Command::MarkdownPreviewSide);
        assert_eq!(app.layout.pane_count(), 2);

        app.dispatch(Command::MarkdownPreviewSide);

        assert_eq!(app.layout.pane_count(), 2, "no second preview pane");
        // Revealing focuses the preview itself.
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::MarkdownPreview { .. }
        ));
    }

    #[test]
    fn a_pane_too_narrow_to_split_gets_the_preview_as_a_tab() {
        let mut app = markdown_app("# Title\n");
        app.main_rect = Rect::new(0, 0, 4, 2);

        app.dispatch(Command::MarkdownPreviewSide);

        assert_eq!(app.layout.pane_count(), 1);
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::MarkdownPreview { .. }
        ));
    }

    #[test]
    fn a_markdown_preview_keeps_its_document_open() {
        // `reconcile_open_docs` ref-counts through `tab_doc`, so a preview must report the
        // document it mirrors or closing the source tab would close the document under it.
        let mut app = markdown_app("# Title\n");
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(7));
        }
        app.dispatch(Command::MarkdownPreviewSide);

        let preview = stored_preview(&app).expect("a preview tab");
        assert_eq!(App::tab_doc(preview), Some(DocumentId(7)));
    }

    #[test]
    fn a_markdown_preview_is_a_pager_for_the_keymap() {
        let mut app = markdown_app("# Title\n");
        app.dispatch(Command::MarkdownPreviewSide);
        app.dispatch(Command::MarkdownPreviewSide); // reveal: focuses the preview
        assert_eq!(app.active_editor_tab(), EditorTab::Pager);
    }

    #[test]
    fn scrolling_a_markdown_preview_moves_it_within_the_wrapped_document() {
        let mut app = markdown_app("# Title\n");
        app.dispatch(Command::MarkdownPreviewSide);
        app.dispatch(Command::MarkdownPreviewSide); // focus the preview

        // Nothing is wrapped until the first draw, so the scroll is pinned at the top.
        app.dispatch(Command::ScrollDown);
        let TabKind::MarkdownPreview {
            wrapped, scroll, ..
        } = &mut app.tabs[app.active].kind
        else {
            panic!("expected a preview tab");
        };
        assert_eq!(*scroll, 0, "an unwrapped preview cannot scroll");

        // Stand in for a draw by wrapping the document, then scroll for real.
        *wrapped = karet_markdown::parse("a\n\nb\n\nc\n").wrap(20);
        app.dispatch(Command::ScrollDown);
        app.dispatch(Command::ScrollDown);
        let TabKind::MarkdownPreview { scroll, .. } = &app.tabs[app.active].kind else {
            panic!("expected a preview tab");
        };
        assert_eq!(*scroll, 2);

        app.dispatch(Command::ScrollUp);
        let TabKind::MarkdownPreview { scroll, .. } = &app.tabs[app.active].kind else {
            panic!("expected a preview tab");
        };
        assert_eq!(*scroll, 1);
    }

    #[test]
    fn a_snapshot_refreshes_the_markdown_preview_buffer() {
        use karet_syntax::Highlights;
        use karet_text::TextBuffer;
        let mut app = markdown_app("# Title\n");
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(3));
        }
        app.dispatch(Command::MarkdownPreviewSide);

        let buffer = TextBuffer::from_text("# Changed\n");
        let version = buffer.version();
        app.on_snapshot(
            DocumentId(3),
            &DocSnapshot {
                version,
                buffer,
                highlights: Arc::new(Highlights::default()),
                semantic_blocks: Arc::new(karet_syntax::SemanticBlocks::default()),
                folds: Arc::new(FoldRegions::default()),
                decorations: Arc::new(Vec::new()),
                syntax_error_lines: Arc::new(Vec::new()),
                language: Some("Markdown"),
                dirty: true,
                cursor: None,
            },
        );

        let preview = stored_preview(&app).expect("a preview tab");
        let TabKind::MarkdownPreview { buffer, .. } = &preview.kind else {
            panic!("expected a preview tab");
        };
        assert_eq!(buffer.text(), "# Changed\n");
    }

    /// A source doc whose blocks sit on known lines: headings on 0, 2, 4, 6.
    const SYNC_DOC: &str = "# a\n\n# b\n\n# c\n\n# d\n";

    /// Open a preview for `SYNC_DOC` and give it a wrapped model, standing in for the
    /// first draw (which is what normally populates it).
    fn synced_app() -> App {
        let mut app = markdown_app(SYNC_DOC);
        app.dispatch(Command::MarkdownPreviewSide);
        let preview = app
            .stored_active_mut(|t| matches!(t.kind, TabKind::MarkdownPreview { .. }))
            .expect("a preview tab");
        if let TabKind::MarkdownPreview { wrapped, .. } = &mut preview.kind {
            *wrapped = karet_markdown::parse(SYNC_DOC).wrap(40);
        }
        app
    }

    /// The preview's scroll, wherever the preview currently lives.
    fn preview_scroll(app: &App) -> u16 {
        let find = |t: &Tab| match &t.kind {
            TabKind::MarkdownPreview { scroll, .. } => Some(*scroll),
            _ => None,
        };
        app.tabs
            .iter()
            .chain(app.stored.values().flat_map(|p| p.tabs.iter()))
            .find_map(find)
            .expect("a preview tab")
    }

    /// The source tab's scroll, wherever it lives.
    fn source_scroll(app: &App) -> u32 {
        app.tabs
            .iter()
            .chain(app.stored.values().flat_map(|p| p.tabs.iter()))
            .find(|t| matches!(t.kind, TabKind::Code { .. }))
            .expect("a source tab")
            .editor
            .scroll_line
    }

    #[test]
    fn scrolling_the_source_scrolls_the_preview_to_the_matching_block() {
        let mut app = synced_app();
        // Source line 4 is the third heading; it renders on wrapped line 4 ("# a", "",
        // "# b", "", "# c").
        app.tabs[app.active].editor.scroll_line = 4;
        app.sync_markdown_preview();
        assert_eq!(preview_scroll(&app), 4);
        assert_eq!(source_scroll(&app), 4, "the driver never moves itself");
    }

    #[test]
    fn scrolling_the_preview_scrolls_the_source_back() {
        let mut app = synced_app();
        app.dispatch(Command::MarkdownPreviewSide); // focus the preview
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::MarkdownPreview { .. }
        ));

        for _ in 0..4 {
            app.dispatch(Command::ScrollDown);
        }
        app.sync_markdown_preview();
        assert_eq!(preview_scroll(&app), 4, "the driver never moves itself");
        assert_eq!(source_scroll(&app), 4);
    }

    #[test]
    fn merely_focusing_the_preview_does_not_nudge_the_source() {
        let mut app = synced_app();
        app.tabs[app.active].editor.scroll_line = 3; // a blank line, mid-round-trip
        app.sync_markdown_preview();
        let settled = preview_scroll(&app);

        app.dispatch(Command::MarkdownPreviewSide); // focus the preview
        app.sync_markdown_preview();

        assert_eq!(
            source_scroll(&app),
            3,
            "a bare focus change must not move it"
        );
        assert_eq!(preview_scroll(&app), settled);
    }

    #[test]
    fn syncing_is_idempotent_and_cannot_oscillate() {
        let mut app = synced_app();
        app.tabs[app.active].editor.scroll_line = 3;
        for _ in 0..10 {
            app.sync_markdown_preview();
        }
        let (source, preview) = (source_scroll(&app), preview_scroll(&app));

        app.dispatch(Command::MarkdownPreviewSide); // hand the wheel to the preview
        for _ in 0..10 {
            app.sync_markdown_preview();
        }
        assert_eq!(source_scroll(&app), source, "the pair settled, not drifted");
        assert_eq!(preview_scroll(&app), preview);
    }

    #[test]
    fn syncing_a_source_with_no_preview_is_a_no_op() {
        let mut app = markdown_app(SYNC_DOC);
        app.tabs[app.active].editor.scroll_line = 2;
        app.sync_markdown_preview();
        assert_eq!(source_scroll(&app), 2);
    }

    #[test]
    fn a_preview_scrolled_past_the_source_clamps_to_the_last_source_line() {
        let mut app = synced_app();
        app.dispatch(Command::MarkdownPreviewSide); // focus the preview
        if let TabKind::MarkdownPreview { scroll, .. } = &mut app.tabs[app.active].kind {
            *scroll = u16::MAX;
        }
        app.sync_markdown_preview();
        let last = app
            .stored_active(|t| matches!(t.kind, TabKind::Code { .. }))
            .and_then(|t| match &t.kind {
                TabKind::Code { buffer, .. } => Some(buffer.line_count().saturating_sub(1) as u32),
                _ => None,
            })
            .expect("a source tab");
        assert_eq!(source_scroll(&app), last, "clamped to the last buffer line");
    }

    /// A standalone DOCX-style preview tab over `md`, with a wrapped model (standing
    /// in for the first draw) and an initial `scroll`.
    #[cfg(feature = "docx")]
    fn docx_preview_tab(md: &str, scroll: u16) -> Tab {
        let mut tab = Tab::document_preview(PathBuf::from("report.docx"), md);
        if let TabKind::MarkdownPreview {
            wrapped, scroll: s, ..
        } = &mut tab.kind
        {
            *wrapped = karet_markdown::parse(md).wrap(40);
            *s = scroll;
        }
        tab
    }

    /// A source scrolled to line 4 drives a *real* preview to wrapped line 4 (see
    /// `scrolling_the_source_scrolls_the_preview_to_the_matching_block`); a detached
    /// document preview must never be adopted as that source's preview.
    #[cfg(feature = "docx")]
    #[test]
    fn a_docx_preview_is_never_adopted_by_a_markdown_source() {
        let mut app = synced_app();
        // Swap the stored real preview for a detached docx preview at scroll 0.
        let preview = app
            .stored_active_mut(|t| matches!(t.kind, TabKind::MarkdownPreview { .. }))
            .expect("a preview tab");
        preview.kind = docx_preview_tab(SYNC_DOC, 0).kind;

        app.tabs[app.active].editor.scroll_line = 4;
        app.sync_markdown_preview();

        assert_eq!(
            preview_scroll(&app),
            0,
            "the sentinel source_view must not pair with a real source"
        );
        assert_eq!(source_scroll(&app), 4, "the source itself is unaffected");
    }

    /// A *real* focused preview at scroll 4 writes the source back to line 4 (see
    /// `scrolling_the_preview_scrolls_the_source_back`); a focused detached preview
    /// must drive nothing.
    #[cfg(feature = "docx")]
    #[test]
    fn a_focused_docx_preview_never_drives_a_stored_source() {
        let mut app = synced_app();
        app.dispatch(Command::MarkdownPreviewSide); // focus the preview pane
        // Turn the focused preview into a detached docx preview, scrolled well away
        // from where the stored source (line 0) projects.
        app.tabs[app.active].kind = docx_preview_tab(SYNC_DOC, 4).kind;

        app.sync_markdown_preview();

        assert_eq!(
            source_scroll(&app),
            0,
            "a detached preview must not scroll any source tab"
        );
        // And its own scroll is left alone (nothing wrote it back).
        if let TabKind::MarkdownPreview { scroll, .. } = &app.tabs[app.active].kind {
            assert_eq!(*scroll, 4);
        }
    }

    /// Invoking the preview command over a markdown source must open a fresh real
    /// preview, not reveal/hijack an open docx preview (whose sentinel `source_view`
    /// can never match the source's view).
    #[cfg(feature = "docx")]
    #[test]
    fn preview_side_opens_a_real_preview_instead_of_hijacking_a_docx_tab() {
        let mut app = markdown_app(SYNC_DOC);
        app.push_tab(docx_preview_tab("# doc", 0));
        app.select_tab(0); // back to the markdown source
        let source_view = app.tabs[app.active].view;

        app.dispatch(Command::MarkdownPreviewSide);

        assert_eq!(app.layout.pane_count(), 2, "a new preview pane opened");
        let preview = stored_preview(&app).expect("a preview tab in the new pane");
        assert!(
            App::previews_view(preview, source_view),
            "the new preview pairs with the source"
        );
        // The docx tab is still in the source pane, untouched.
        assert!(app.tabs.iter().any(
            |t| matches!(&t.kind, TabKind::MarkdownPreview { source_view, .. }
                if *source_view == crate::tab::DETACHED_SOURCE_VIEW)
        ));
    }

    /// The preview command refuses politely on a focused docx preview — there is no
    /// markdown source file behind it to preview.
    #[cfg(feature = "docx")]
    #[test]
    fn preview_side_is_a_no_op_on_a_focused_docx_preview() {
        let mut app = app();
        app.push_tab(docx_preview_tab("# doc", 0));
        app.main_rect = Rect::new(0, 0, 80, 24);

        app.dispatch(Command::MarkdownPreviewSide);

        assert_eq!(app.layout.pane_count(), 1, "no pane was opened");
        assert!(app.status.is_some(), "the refusal is surfaced, not silent");
    }

    /// The unified close guard (#51) protects dirty *documents*; a docx preview has
    /// none (`tab_doc` is `None`), so closing it never prompts — even if the dirty
    /// flag were somehow set — and `reconcile_open_docs` has nothing to release.
    #[cfg(feature = "docx")]
    #[test]
    fn closing_a_docx_preview_never_arms_the_close_guard() {
        let mut app = app();
        app.push_tab(docx_preview_tab("# doc", 0));
        let view = app.tabs[app.active].view;
        assert_eq!(App::tab_doc(&app.tabs[app.active]), None);
        app.tabs[app.active].dirty = true; // impossible in practice; the guard still passes

        assert!(app.docs_at_risk(CloseRequest::Tab { view }).is_empty());
        app.guarded_close(CloseRequest::Tab { view });

        assert!(app.pending_close.is_none(), "no confirmation was armed");
        assert!(
            !app.tabs.iter().any(|t| t.view == view),
            "the tab closed immediately"
        );
    }

    /// Document snapshots refresh previews by their bound `DocumentId`; a docx
    /// preview is bound to none, so no snapshot can ever overwrite its content.
    #[cfg(feature = "docx")]
    #[test]
    fn a_snapshot_never_touches_a_docx_preview() {
        use karet_syntax::Highlights;
        use karet_text::TextBuffer;
        let mut app = app();
        app.push_tab(docx_preview_tab("# original", 0));

        let buffer = TextBuffer::from_text("# changed\n");
        let version = buffer.version();
        app.on_snapshot(
            DocumentId(7),
            &DocSnapshot {
                version,
                buffer,
                highlights: Arc::new(Highlights::default()),
                semantic_blocks: Arc::new(karet_syntax::SemanticBlocks::default()),
                folds: Arc::new(FoldRegions::default()),
                decorations: Arc::new(Vec::new()),
                syntax_error_lines: Arc::new(Vec::new()),
                language: Some("Markdown"),
                dirty: true,
                cursor: None,
            },
        );

        let TabKind::MarkdownPreview { buffer, .. } = &app.tabs[app.active].kind else {
            panic!("expected the docx preview tab");
        };
        assert_eq!(buffer.text(), "# original");
    }

    /// A minimal DOCX zipped in-memory (no fixture on disk).
    #[cfg(feature = "docx")]
    fn tiny_docx() -> Vec<u8> {
        use std::io::Write as _;
        const DOCUMENT_XML: &str = r#"<?xml version="1.0"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Report</w:t></w:r></w:p>
</w:body></w:document>"#;
        let mut buf = Vec::new();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        writer
            .start_file(
                "word/document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .expect("start_file");
        writer
            .write_all(DOCUMENT_XML.as_bytes())
            .expect("write_all");
        writer.finish().expect("finish");
        buf
    }

    #[cfg(feature = "docx")]
    #[test]
    fn reopening_the_same_docx_focuses_the_existing_tab() {
        let dir = test_dir("docx-dedup");
        let file = dir.join("report.docx");
        std::fs::write(&file, tiny_docx()).expect("write the docx");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);

        app.open_path(&file);
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::MarkdownPreview { .. }
        ));
        assert_eq!(app.tabs.len(), 1);
        let view = app.tabs[app.active].view;

        // Move focus elsewhere, then open the same file again.
        app.push_tab(text_tab("other.rs", "fn x() {}"));
        assert_eq!(app.tabs.len(), 2);
        app.open_path(&file);

        assert_eq!(app.tabs.len(), 2, "no duplicate tab for the same document");
        assert_eq!(app.tabs[app.active].view, view, "the existing tab focused");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Draw the whole shell into a test terminal and return the screen, row by row.
    fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
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

    /// End-to-end: the shell splits, wraps and paints the preview through `ui::draw`.
    ///
    /// A list is the giveaway — the source pane shows `- one`, the rendered preview shows
    /// a `•` bullet — so this proves the preview is rendered, not echoed source.
    #[test]
    fn the_preview_pane_paints_rendered_markdown_beside_the_source() {
        let mut app = markdown_app("- one\n- two\n");
        app.dispatch(Command::MarkdownPreviewSide);

        let painted = screen(&mut app, 100, 12).join("\n");
        assert!(
            painted.contains("- one"),
            "the source pane still shows markup:\n{painted}"
        );
        assert!(
            painted.contains('\u{2022}'),
            "the preview pane should render a bullet:\n{painted}"
        );
    }

    /// The draw-time render cache is keyed on the document version, so an edit re-renders
    /// the preview on the next frame. Drives the edit through `TextBuffer::apply` — the
    /// same path the session takes — because that is what moves the version.
    #[test]
    fn editing_the_source_re_renders_the_preview_on_the_next_draw() {
        let mut app = markdown_app("# before\n");
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(11));
        }
        app.dispatch(Command::MarkdownPreviewSide);
        let before = screen(&mut app, 100, 12).join("\n");
        assert!(before.contains("before"), "{before}");

        // "# before" -> "# after": delete "before", insert "after". Applying bumps the
        // version, which is exactly what invalidates the cache.
        let mut edited = karet_text::TextBuffer::from_text("# before\n");
        let change = karet_core::Change::new(
            edited.version(),
            vec![karet_core::TextEdit {
                range: Range {
                    start: LineCol::new(0, 2),
                    end: LineCol::new(0, 8),
                },
                new_text: "after".to_string(),
            }],
        );
        edited.apply_simple(&change).expect("apply the edit");
        assert!(edited.version() > 0, "the edit must move the version");

        for tab in app.all_tabs_mut() {
            match &mut tab.kind {
                TabKind::Code { buffer, text, .. } => {
                    *buffer = edited.content_snapshot();
                    *text = edited.text();
                },
                TabKind::MarkdownPreview { buffer, .. } => *buffer = edited.content_snapshot(),
                _ => {},
            }
        }

        let after = screen(&mut app, 100, 12).join("\n");
        assert!(
            after.contains("after") && !after.contains("before"),
            "the preview must re-render once the document version moves:\n{after}"
        );
    }
