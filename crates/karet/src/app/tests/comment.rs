//! The comment toggles as the user meets them: the comment syntax resolved from
//! the file type *and* the injected language at the caret, applied to the buffer
//! at every caret. The pure toggles are covered in `karet_editor`.

use super::support::*;
use crate::app::*;

/// A focused editor over `text` in a file named `name`, with a bare caret at each
/// of `carets`, wired to a recording backend (edits need one).
fn editor_app(name: &str, text: &str, carets: &[LineCol]) -> App {
    let mut app = app();
    app.backend = Some(Arc::new(RecordingBackend::new()));
    let mut tab = text_tab(name, text);
    if let TabKind::Code { doc, .. } = &mut tab.kind {
        *doc = Some(DocumentId(9));
    }
    app.push_tab(tab);
    app.keyboard.focus_editor();
    let idx = app.active;
    app.tabs[idx].editor.set_carets(carets);
    app
}

#[test]
fn line_comment_uses_the_file_types_prefix_at_every_caret() {
    let mut app = editor_app(
        "main.rs",
        "fn a() {}\nfn b() {}\n",
        &[LineCol::new(0, 0), LineCol::new(1, 3)],
    );
    app.dispatch(Command::ToggleLineComment);
    assert_eq!(code_tab_text(&app), "// fn a() {}\n// fn b() {}\n");
    let carets = app.tabs[app.active].editor.cursors().selections.clone();
    assert_eq!(carets.len(), 2, "both carets survive the toggle");
    assert_eq!(carets[1].head, LineCol::new(1, 6));

    app.dispatch(Command::ToggleLineComment);
    assert_eq!(code_tab_text(&app), "fn a() {}\nfn b() {}\n");
}

#[test]
fn a_script_body_in_html_takes_javascript_comments() {
    let text = "<p>hi</p>\n<script>\n  let x = 1;\n</script>\n";
    let mut app = editor_app("index.html", text, &[LineCol::new(2, 0)]);
    app.dispatch(Command::ToggleLineComment);
    assert_eq!(
        code_tab_text(&app),
        "<p>hi</p>\n<script>\n  // let x = 1;\n</script>\n"
    );

    // Outside the script the document's own markup comment applies.
    let mut app = editor_app("index.html", text, &[LineCol::new(0, 1)]);
    app.dispatch(Command::ToggleLineComment);
    assert_eq!(
        code_tab_text(&app),
        "<!-- <p>hi</p> -->\n<script>\n  let x = 1;\n</script>\n"
    );
}

#[test]
fn a_markdown_code_fence_takes_its_languages_comments() {
    let text = "Prose here.\n\n```python\nx = 1\n```\n";
    let mut app = editor_app("notes.md", text, &[LineCol::new(3, 0)]);
    app.dispatch(Command::ToggleLineComment);
    assert_eq!(
        code_tab_text(&app),
        "Prose here.\n\n```python\n# x = 1\n```\n"
    );

    // Prose sits in the inline layer, which has no comments: the toggle falls back
    // to Markdown's own HTML comment.
    let mut app = editor_app("notes.md", text, &[LineCol::new(0, 2)]);
    app.dispatch(Command::ToggleLineComment);
    assert_eq!(
        code_tab_text(&app),
        "<!-- Prose here. -->\n\n```python\nx = 1\n```\n"
    );
}

#[test]
fn block_comment_wraps_the_caret_line_and_is_a_no_op_without_block_syntax() {
    let mut app = editor_app("main.rs", "    call();\n", &[LineCol::new(0, 6)]);
    app.dispatch(Command::ToggleBlockComment);
    assert_eq!(code_tab_text(&app), "    /* call(); */\n");

    // Python has only line comments, so a block toggle leaves the text alone.
    let mut app = editor_app("main.py", "x = 1\n", &[LineCol::new(0, 0)]);
    app.dispatch(Command::ToggleBlockComment);
    assert_eq!(code_tab_text(&app), "x = 1\n");
}

#[test]
fn a_file_without_comment_syntax_is_left_alone() {
    let mut app = editor_app("data.json", "{}\n", &[LineCol::new(0, 0)]);
    app.dispatch(Command::ToggleLineComment);
    assert_eq!(code_tab_text(&app), "{}\n");
}

#[test]
fn a_caret_in_the_second_language_of_a_mixed_line_takes_that_language() {
    // The caret sits inside the script body, past the `<p>` that opens the line.
    let text = "<p>hi</p><script>let x = 1;</script>\n";
    let mut app = editor_app("index.html", text, &[LineCol::new(0, 20)]);
    app.dispatch(Command::ToggleLineComment);
    assert_eq!(
        code_tab_text(&app),
        "// <p>hi</p><script>let x = 1;</script>\n"
    );
}

#[test]
fn a_caret_after_trailing_whitespace_takes_the_last_solid_chars_language() {
    // The caret sits past the line's trailing whitespace, which already belongs to
    // the script body; the last non-blank char before it is the `<script>` tag's
    // `>`, so the line resolves to HTML, not JavaScript.
    let text = "<p>hi</p><script>   \nlet x = 1;\n</script>\n";
    let mut app = editor_app("index.html", text, &[LineCol::new(0, 20)]);
    app.dispatch(Command::ToggleLineComment);
    assert_eq!(
        code_tab_text(&app),
        "<!-- <p>hi</p><script> -->   \nlet x = 1;\n</script>\n"
    );
}

#[test]
fn doc_comment_text_in_rust_takes_rusts_comments_not_markdowns() {
    // A caret on a `///` line's text, and on a doctest line inside it, sits in
    // injected Markdown (and Rust again), but the line is still Rust source: `//`
    // applies, and a `///` line already starts with it, so the toggle uncomments.
    let text = "/// Adds one.\n///\n/// ```\n/// let y = 2;\n/// ```\nfn f() {}\n";
    for (caret, want) in [
        (LineCol::new(0, 6), "/ Adds one."),
        (LineCol::new(3, 8), "/ let y = 2;"),
    ] {
        let mut app = editor_app("lib.rs", text, &[caret]);
        app.dispatch(Command::ToggleLineComment);
        let after = code_tab_text(&app);
        let line = after.lines().nth(caret.line as usize).unwrap_or_default();
        assert_eq!(line, want, "{caret:?}");
    }

    // An inner line of a `/** */` block with no leading `*`.
    let text = "/**\n Adds one.\n */\nfn f() {}\n";
    let mut app = editor_app("lib.rs", text, &[LineCol::new(1, 3)]);
    app.dispatch(Command::ToggleLineComment);
    assert_eq!(code_tab_text(&app), "/**\n // Adds one.\n */\nfn f() {}\n");
}
