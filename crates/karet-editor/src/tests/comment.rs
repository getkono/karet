//! Comment-toggle tests: the pure `editing::toggle_*_comment` builders applied to a
//! real buffer, so each case checks the text it produces and where the carets land.

use karet_core::CursorState;
use karet_core::LineCol;
use karet_core::Selection;
use karet_text::TextBuffer;

use crate::editing::LineComment;
use crate::editing::MultiEdit;
use crate::editing::toggle_block_comment;
use crate::editing::toggle_line_comment;

type Outcome = Result<(), &'static str>;

const SLASHES: LineComment<'static> = LineComment::Prefix("//");
const NO_EDIT: &str = "expected an edit";

fn at(line: u32, col: u32) -> LineCol {
    LineCol::new(line, col)
}

fn carets(points: &[(u32, u32)]) -> CursorState {
    CursorState {
        selections: points
            .iter()
            .map(|&(l, c)| Selection::caret(at(l, c)))
            .collect(),
        primary: 0,
    }
}

fn select(anchor: (u32, u32), head: (u32, u32)) -> CursorState {
    CursorState::single(Selection {
        anchor: at(anchor.0, anchor.1),
        head: at(head.0, head.1),
    })
}

/// Apply `edit` to `text`, returning the new text (empty on a rejected change).
fn apply(text: &str, edit: &MultiEdit) -> String {
    let mut buffer = TextBuffer::from_text(text);
    if buffer.apply_simple(&edit.change).is_err() {
        return String::new();
    }
    buffer.text()
}

/// Toggle a line comment over `text`, returning the new text and cursor set.
fn line(
    text: &str,
    cursors: &CursorState,
    token: LineComment<'_>,
) -> Result<(String, CursorState), &'static str> {
    let edit =
        toggle_line_comment(&TextBuffer::from_text(text), cursors, 0, token).ok_or(NO_EDIT)?;
    Ok((apply(text, &edit), edit.cursors))
}

/// Toggle a `/* */` block comment over `text`.
fn block(text: &str, cursors: &CursorState) -> Result<(String, CursorState), &'static str> {
    let edit = toggle_block_comment(&TextBuffer::from_text(text), cursors, 0, "/*", "*/")
        .ok_or(NO_EDIT)?;
    Ok((apply(text, &edit), edit.cursors))
}

#[test]
fn line_comment_aligns_markers_at_the_minimum_indent_and_skips_blank_lines() -> Outcome {
    let (out, _) = line("    a\n\n  b\n      c\n", &select((0, 0), (3, 1)), SLASHES)?;
    assert_eq!(out, "  //   a\n\n  // b\n  //     c\n");
    Ok(())
}

#[test]
fn line_comment_round_trips() -> Outcome {
    let text = "fn main() {\n    let x = 1;\n\n    x\n}\n";
    let (commented, after) = line(text, &select((1, 0), (3, 5)), SLASHES)?;
    assert_eq!(commented, "fn main() {\n    // let x = 1;\n\n    // x\n}\n");
    let (restored, _) = line(&commented, &after, SLASHES)?;
    assert_eq!(restored, text);
    Ok(())
}

#[test]
fn line_comment_moves_the_caret_with_its_text() -> Outcome {
    let (out, after) = line("  let x;\n", &carets(&[(0, 4)]), SLASHES)?;
    assert_eq!(out, "  // let x;\n");
    assert_eq!(after.primary().head, at(0, 7));
    // Uncommenting moves it back; a caret inside the removed marker collapses onto
    // the marker's start.
    let (back, after) = line(&out, &carets(&[(0, 3)]), SLASHES)?;
    assert_eq!(back, "  let x;\n");
    assert_eq!(after.primary().head, at(0, 2));
    Ok(())
}

#[test]
fn mixed_lines_comment_rather_than_uncomment() -> Outcome {
    // One line is already commented, one is not: the whole block gains a level.
    let (out, _) = line("// a\nb\n", &select((0, 0), (1, 1)), SLASHES)?;
    assert_eq!(out, "// // a\n// b\n");
    Ok(())
}

#[test]
fn uncomment_accepts_a_marker_without_its_padding_space() -> Outcome {
    let (out, _) = line("  //x\n  // y\n", &select((0, 0), (1, 1)), SLASHES)?;
    assert_eq!(out, "  x\n  y\n");
    Ok(())
}

#[test]
fn every_caret_contributes_its_line_once() -> Outcome {
    // Two carets on line 0 and one on line 2: three carets, two lines, one toggle.
    let (out, after) = line("a\nb\nc\n", &carets(&[(0, 0), (0, 1), (2, 1)]), SLASHES)?;
    assert_eq!(out, "// a\nb\n// c\n");
    let heads: Vec<LineCol> = after.selections.iter().map(|s| s.head).collect();
    assert_eq!(heads, vec![at(0, 3), at(0, 4), at(2, 4)]);
    Ok(())
}

#[test]
fn a_selection_ending_at_column_zero_leaves_that_line_alone() -> Outcome {
    let (out, _) = line("a\nb\n", &select((0, 0), (1, 0)), SLASHES)?;
    assert_eq!(out, "// a\nb\n");
    Ok(())
}

#[test]
fn a_lone_blank_line_is_commented() -> Outcome {
    let (out, after) = line("  \n", &carets(&[(0, 2)]), SLASHES)?;
    assert_eq!(out, "  // \n");
    assert_eq!(after.primary().head, at(0, 5));
    Ok(())
}

#[test]
fn wrap_style_line_comments_round_trip() -> Outcome {
    let wrap = LineComment::Wrap {
        open: "<!--",
        close: "-->",
    };
    let text = "<p>a</p>\n  <p>b</p>\n";
    let (out, after) = line(text, &select((0, 0), (1, 3)), wrap)?;
    assert_eq!(out, "<!-- <p>a</p> -->\n<!--   <p>b</p> -->\n");
    let (back, _) = line(&out, &after, wrap)?;
    assert_eq!(back, text);
    Ok(())
}

#[test]
fn columns_count_chars_not_bytes_and_tabs_as_indent() -> Outcome {
    // A multi-byte char before the caret and a tab indent: both are one column.
    let (out, after) = line("\tñx\n", &carets(&[(0, 3)]), LineComment::Prefix("#"))?;
    assert_eq!(out, "\t# ñx\n");
    assert_eq!(after.primary().head, at(0, 5));
    let (out, _) = block("é\n", &carets(&[(0, 1)]))?;
    assert_eq!(out, "/* é */\n");
    Ok(())
}

#[test]
fn delimiters_with_no_inner_text_unwrap_to_nothing() -> Outcome {
    let wrap = LineComment::Wrap {
        open: "<!--",
        close: "-->",
    };
    let (out, _) = line("<!---->\n", &carets(&[(0, 0)]), wrap)?;
    assert_eq!(out, "\n");
    Ok(())
}

#[test]
fn an_empty_token_is_a_no_op() {
    let buffer = TextBuffer::from_text("a");
    let cursors = carets(&[(0, 0)]);
    assert!(toggle_line_comment(&buffer, &cursors, 0, LineComment::Prefix("")).is_none());
    assert!(toggle_block_comment(&buffer, &cursors, 0, "", "*/").is_none());
}

#[test]
fn block_comment_wraps_and_unwraps_a_selection() -> Outcome {
    let text = "let x = 1 + 2;\n";
    let (out, after) = block(text, &select((0, 8), (0, 13)))?;
    assert_eq!(out, "let x = /* 1 + 2 */;\n");
    // The selection still covers exactly the original text.
    assert_eq!(after.primary().range().start, at(0, 11));
    assert_eq!(after.primary().range().end, at(0, 16));
    // Selecting the comment itself unwraps it.
    let (back, _) = block(&out, &select((0, 8), (0, 19)))?;
    assert_eq!(back, text);
    Ok(())
}

#[test]
fn block_comment_spans_lines() -> Outcome {
    let text = "a\nb\nc\n";
    let (out, after) = block(text, &select((0, 0), (2, 1)))?;
    assert_eq!(out, "/* a\nb\nc */\n");
    assert_eq!(after.primary().range().end, at(2, 1));
    // Unwrapping from a selection that also covers surrounding whitespace.
    let (back, _) = block(&format!(" {out}"), &select((0, 0), (3, 0)))?;
    assert_eq!(back, format!(" {text}"));
    Ok(())
}

#[test]
fn a_bare_caret_toggles_its_line_content() -> Outcome {
    let text = "    call();\n";
    let (out, after) = block(text, &carets(&[(0, 6)]))?;
    assert_eq!(out, "    /* call(); */\n");
    assert_eq!(after.primary().head, at(0, 9));
    let (back, after) = block(&out, &after)?;
    assert_eq!(back, text);
    assert_eq!(after.primary().head, at(0, 6));
    Ok(())
}

#[test]
fn a_caret_on_a_blank_line_lands_inside_an_empty_comment() -> Outcome {
    let (out, after) = block("\n", &carets(&[(0, 0)]))?;
    assert_eq!(out, "/*  */\n");
    assert_eq!(after.primary().head, at(0, 3));
    // The empty comment toggles away again.
    let (back, _) = block(&out, &after)?;
    assert_eq!(back, "\n");
    Ok(())
}

#[test]
fn block_comment_applies_at_every_caret_once_per_line() -> Outcome {
    let (out, after) = block("a\nb\n", &carets(&[(0, 0), (0, 1), (1, 1)]))?;
    assert_eq!(out, "/* a */\n/* b */\n");
    assert_eq!(after.selections.len(), 3);
    Ok(())
}
