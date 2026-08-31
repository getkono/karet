//! The markdown editing commands as the user meets them: emphasis toggles,
//! task checkboxes, and list-aware Enter. The pure helpers are covered in
//! `karet_markdown::edit`; what is covered here is the gating to Markdown
//! tabs, the buffer edits, and the caret each command leaves behind.

use super::support::*;
use crate::app::*;

/// A focused Markdown editor over `text` with the caret at `caret`, wired to a
/// recording backend (`submit_edit` needs one, and applies the change locally).
fn markdown_app(text: &str, caret: LineCol) -> App {
    let mut app = app();
    app.backend = Some(Arc::new(RecordingBackend::new()));
    let mut tab = text_tab("notes.md", text);
    if let TabKind::Code {
        path,
        language,
        doc,
        ..
    } = &mut tab.kind
    {
        *path = std::path::PathBuf::from("notes.md");
        *language = "Markdown";
        *doc = Some(DocumentId(9));
    }
    app.push_tab(tab);
    app.focus = Focus::Editor;
    let idx = app.active;
    app.tabs[idx].editor.set_carets(&[caret]);
    app
}

/// A focused non-Markdown editor, for the fall-through cases.
fn rust_app(text: &str) -> App {
    let mut app = app();
    app.backend = Some(Arc::new(RecordingBackend::new()));
    let mut tab = text_tab("main.rs", text);
    if let TabKind::Code { doc, .. } = &mut tab.kind {
        *doc = Some(DocumentId(9));
    }
    app.push_tab(tab);
    app.focus = Focus::Editor;
    app
}

/// The primary caret of the active tab.
fn caret_of(app: &App) -> LineCol {
    app.tabs[app.active].editor.cursor()
}

#[test]
fn the_markdown_gate_recognizes_a_markdown_path() {
    let app = markdown_app("# title\n", LineCol::new(0, 0));
    // The gate keys off the filetype registry, not the tab's language label.
    assert_eq!(
        karet_filetype::file_type_for_path(std::path::Path::new("notes.md")).name(),
        "Markdown"
    );
    assert!(matches!(&app.tabs[app.active].kind, TabKind::Code { .. }));
}

#[test]
fn bold_wraps_the_word_under_the_caret_and_selects_it() {
    let mut app = markdown_app("hello world\n", LineCol::new(0, 2));
    app.dispatch(Command::ToggleBold);

    assert_eq!(code_tab_text(&app), "**hello** world\n");
    // The same text stays selected, so a second press unwraps it.
    let range = app.tabs[app.active].editor.selection_range();
    assert!(range.is_some(), "the toggled span stays selected");
}

#[test]
fn bold_is_its_own_inverse() {
    let mut app = markdown_app("**hello** world\n", LineCol::new(0, 4));
    app.dispatch(Command::ToggleBold);
    assert_eq!(code_tab_text(&app), "hello world\n");
}

#[test]
fn italic_and_strikethrough_use_their_own_markers() {
    let mut app = markdown_app("word\n", LineCol::new(0, 1));
    app.dispatch(Command::ToggleItalic);
    assert_eq!(code_tab_text(&app), "*word*\n");

    let mut app = markdown_app("word\n", LineCol::new(0, 1));
    app.dispatch(Command::ToggleStrikethrough);
    assert_eq!(code_tab_text(&app), "~~word~~\n");

    let mut app = markdown_app("word\n", LineCol::new(0, 1));
    app.dispatch(Command::ToggleInlineCode);
    assert_eq!(code_tab_text(&app), "`word`\n");
}

#[test]
fn ctrl_b_outside_markdown_still_toggles_the_sidebar() {
    // The chord is registered on the whole Editor layer, so a Rust file must
    // keep the global meaning rather than silently doing nothing.
    let mut app = rust_app("fn main() {}\n");
    let before = app.sidebar_visible;

    app.dispatch(Command::ToggleBold);

    assert_eq!(
        code_tab_text(&app),
        "fn main() {}\n",
        "the buffer is untouched"
    );
    assert_ne!(app.sidebar_visible, before, "the sidebar toggled instead");
}

#[test]
fn italic_outside_markdown_explains_itself_instead_of_editing() {
    let mut app = rust_app("fn main() {}\n");

    app.dispatch(Command::ToggleItalic);

    assert_eq!(code_tab_text(&app), "fn main() {}\n");
    assert_eq!(
        last_message(&app).as_deref(),
        Some("markdown formatting applies to Markdown files")
    );
}

#[test]
fn a_multi_line_selection_refuses_rather_than_mangling_it() {
    let mut app = markdown_app("one\ntwo\n", LineCol::new(0, 0));
    let idx = app.active;
    let buffer = match &app.tabs[idx].kind {
        TabKind::Code { buffer, .. } => buffer.clone(),
        _ => return,
    };
    app.tabs[idx].editor.set_cursor_state(
        &buffer,
        karet_core::CursorState {
            selections: vec![karet_core::Selection {
                anchor: LineCol::new(0, 0),
                head: LineCol::new(1, 3),
            }],
            primary: 0,
        },
    );
    app.dispatch(Command::ToggleBold);

    assert_eq!(code_tab_text(&app), "one\ntwo\n");
    assert_eq!(
        last_message(&app).as_deref(),
        Some("select within one line to toggle formatting")
    );
}

#[test]
fn the_checkbox_toggles_without_moving_the_caret() {
    let mut app = markdown_app("- [ ] buy milk\n", LineCol::new(0, 9));
    app.dispatch(Command::ToggleTaskCheckbox);
    assert_eq!(code_tab_text(&app), "- [x] buy milk\n");
    assert_eq!(caret_of(&app), LineCol::new(0, 9));

    app.dispatch(Command::ToggleTaskCheckbox);
    assert_eq!(code_tab_text(&app), "- [ ] buy milk\n");
}

#[test]
fn a_line_without_a_checkbox_says_so() {
    let mut app = markdown_app("just prose\n", LineCol::new(0, 3));
    app.dispatch(Command::ToggleTaskCheckbox);
    assert_eq!(code_tab_text(&app), "just prose\n");
    assert_eq!(
        last_message(&app).as_deref(),
        Some("no task checkbox on this line")
    );
}

#[test]
fn enter_continues_a_bullet() {
    let mut app = markdown_app("- first\n", LineCol::new(0, 7));
    app.dispatch(Command::InsertNewline);
    assert_eq!(code_tab_text(&app), "- first\n- \n");
    assert_eq!(caret_of(&app), LineCol::new(1, 2));
}

#[test]
fn enter_carries_an_unchecked_checkbox_forward() {
    let mut app = markdown_app("- [x] done\n", LineCol::new(0, 10));
    app.dispatch(Command::InsertNewline);
    assert_eq!(code_tab_text(&app), "- [x] done\n- [ ] \n");
}

#[test]
fn enter_on_an_empty_item_ends_the_list() {
    let mut app = markdown_app("- first\n- \n", LineCol::new(1, 2));
    app.dispatch(Command::InsertNewline);
    assert_eq!(code_tab_text(&app), "- first\n\n");
}

#[test]
fn enter_advances_an_ordered_marker() {
    let mut app = markdown_app("1. first\n", LineCol::new(0, 8));
    app.dispatch(Command::InsertNewline);
    assert_eq!(code_tab_text(&app), "1. first\n2. \n");
    assert_eq!(caret_of(&app), LineCol::new(1, 3));
}

#[test]
fn enter_mid_run_renumbers_the_following_siblings() {
    // Splitting after item 1 makes the new item 2, so the old 2 and 3 must
    // become 3 and 4 in the same undoable edit.
    let mut app = markdown_app("1. one\n2. two\n3. three\n", LineCol::new(0, 6));
    app.dispatch(Command::InsertNewline);
    assert_eq!(
        code_tab_text(&app),
        "1. one\n2. \n3. two\n4. three\n",
        "the inserted item takes 2 and the tail shifts down"
    );
}

#[test]
fn enter_inside_a_fenced_code_block_stays_inert() {
    let mut app = markdown_app("```\n- not a list\n```\n", LineCol::new(1, 12));
    app.dispatch(Command::InsertNewline);
    assert_eq!(
        code_tab_text(&app),
        "```\n- not a list\n\n```\n",
        "a fenced line gets an ordinary newline, no marker"
    );
}

#[test]
fn the_setting_turns_list_continuation_off() {
    let mut app = markdown_app("- first\n", LineCol::new(0, 7));
    app.settings.markdown.list_continuation = false;
    app.dispatch(Command::InsertNewline);
    assert_eq!(
        code_tab_text(&app),
        "- first\n\n",
        "with continuation off Enter is an ordinary newline"
    );
}

#[test]
fn enter_on_a_plain_paragraph_is_an_ordinary_newline() {
    let mut app = markdown_app("just prose\n", LineCol::new(0, 10));
    app.dispatch(Command::InsertNewline);
    assert_eq!(code_tab_text(&app), "just prose\n\n");
}

#[test]
fn create_inserts_a_toc_block_at_the_caret() {
    let mut app = markdown_app("# Title\n\n## Alpha\n\n## Beta\n", LineCol::new(1, 0));
    app.dispatch(Command::MarkdownTocCreate);

    assert_eq!(
        code_tab_text(&app),
        "# Title\n<!-- toc -->\n- [Alpha](#alpha)\n- [Beta](#beta)\n<!-- /toc -->\n\n\n## Alpha\n\n## Beta\n"
    );
    assert_eq!(
        last_message(&app).as_deref(),
        Some("table of contents inserted")
    );
}

#[test]
fn update_rewrites_an_existing_region_in_place() {
    let text = "# Title\n\
                <!-- toc -->\n\
                - [Stale](#stale)\n\
                <!-- /toc -->\n\
                \n\
                ## Alpha\n\
                \n\
                ## Beta\n";
    let mut app = markdown_app(text, LineCol::new(0, 0));
    app.dispatch(Command::MarkdownTocUpdate);

    assert_eq!(
        code_tab_text(&app),
        "# Title\n\
         <!-- toc -->\n\
         - [Alpha](#alpha)\n\
         - [Beta](#beta)\n\
         <!-- /toc -->\n\
         \n\
         ## Alpha\n\
         \n\
         ## Beta\n"
    );
    assert_eq!(
        last_message(&app).as_deref(),
        Some("table of contents updated")
    );
}

#[test]
fn update_without_markers_points_at_the_create_command() {
    let mut app = markdown_app("# Title\n\n## Alpha\n", LineCol::new(0, 0));
    app.dispatch(Command::MarkdownTocUpdate);

    assert_eq!(code_tab_text(&app), "# Title\n\n## Alpha\n");
    assert_eq!(
        last_message(&app).as_deref(),
        Some("no <!-- toc --> markers — use Markdown: Create Table of Contents")
    );
}

#[test]
fn a_document_with_no_headings_in_range_says_so() {
    let mut app = markdown_app("# Only an h1\n\nprose\n", LineCol::new(0, 0));
    app.dispatch(Command::MarkdownTocCreate);
    // The default range is 2..=6, so a lone h1 yields nothing to list.
    assert_eq!(code_tab_text(&app), "# Only an h1\n\nprose\n");
    assert_eq!(
        last_message(&app).as_deref(),
        Some("no headings in the table's level range")
    );
}

#[test]
fn nested_headings_indent_under_their_parent() {
    let mut app = markdown_app("## Top\n\n### Nested\n", LineCol::new(0, 0));
    app.dispatch(Command::MarkdownTocCreate);
    let text = code_tab_text(&app);
    assert!(text.contains("- [Top](#top)"), "{text}");
    assert!(text.contains("  - [Nested](#nested)"), "{text}");
}

#[test]
fn increasing_the_level_adds_a_hash_and_decreasing_removes_one() {
    let mut app = markdown_app("## Section\n", LineCol::new(0, 4));
    app.dispatch(Command::MarkdownHeadingUp);
    assert_eq!(code_tab_text(&app), "### Section\n");

    app.dispatch(Command::MarkdownHeadingDown);
    assert_eq!(code_tab_text(&app), "## Section\n");
}

#[test]
fn decreasing_past_h1_demotes_the_title_back_to_prose() {
    let mut app = markdown_app("# Title\n", LineCol::new(0, 3));
    app.dispatch(Command::MarkdownHeadingDown);
    assert_eq!(code_tab_text(&app), "Title\n");
}

#[test]
fn the_heading_level_clamps_at_six() {
    let mut app = markdown_app("###### Deep\n", LineCol::new(0, 8));
    app.dispatch(Command::MarkdownHeadingUp);
    assert_eq!(
        code_tab_text(&app),
        "###### Deep\n",
        "there is no h7 to shift into"
    );
}

#[test]
fn the_toc_commands_stay_out_of_non_markdown_files() {
    let mut app = rust_app("fn main() {}\n");
    app.dispatch(Command::MarkdownTocCreate);
    assert_eq!(code_tab_text(&app), "fn main() {}\n");
    assert_eq!(
        last_message(&app).as_deref(),
        Some("the table of contents applies to Markdown files")
    );

    app.dispatch(Command::MarkdownHeadingUp);
    assert_eq!(code_tab_text(&app), "fn main() {}\n");
    assert_eq!(
        last_message(&app).as_deref(),
        Some("heading levels apply to Markdown files")
    );
}

#[test]
fn a_heading_inside_a_fence_never_reaches_the_toc() {
    let text = "## Real\n\n```\n## Not a heading\n```\n\n## Also real\n";
    let mut app = markdown_app(text, LineCol::new(0, 0));
    app.dispatch(Command::MarkdownTocCreate);
    let out = code_tab_text(&app);
    // The fenced line stays in the document; what matters is that the generated
    // block between the markers does not link it.
    let toc: String = out
        .lines()
        .skip_while(|l| l.trim() != karet_markdown::toc::TOC_START)
        .take_while(|l| l.trim() != karet_markdown::toc::TOC_END)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(toc.contains("- [Real](#real)"), "{toc}");
    assert!(toc.contains("- [Also real](#also-real)"), "{toc}");
    assert!(!toc.contains("Not a heading"), "{toc}");
}
