//! Notebook → markdown, for karet's read-only document preview.
//!
//! Markdown cells pass through verbatim; code cells become fenced blocks in
//! the notebook's language; outputs render text-first — MIME priority
//! `image/*` (placeholder) → `text/markdown` (verbatim) → `text/plain`
//! (fenced) — with ANSI escapes stripped, since the preview is prose.

use serde_json::Value;

use crate::CellKind;
use crate::Notebook;
use crate::Output;
use crate::Source;

/// Render `notebook` as one markdown document.
#[must_use]
pub fn to_markdown(notebook: &Notebook) -> String {
    let language = notebook.language();
    let mut out = String::new();
    for cell in &notebook.cells {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        match cell.kind {
            CellKind::Markdown => out.push_str(cell.source.text().trim_end()),
            CellKind::Raw => push_fence(&mut out, "", &cell.source.text()),
            CellKind::Code => {
                let counter = match cell.execution_count {
                    Some(Some(count)) => format!("In [{count}]"),
                    _ => "In [ ]".to_owned(),
                };
                out.push_str(&format!("_{counter}:_\n\n"));
                push_fence(&mut out, language, &cell.source.text());
                for output in cell.outputs.as_deref().unwrap_or_default() {
                    out.push_str("\n\n");
                    push_output(&mut out, output);
                }
            },
        }
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Append one output's markdown rendering.
fn push_output(out: &mut String, output: &Output) {
    match output {
        Output::Stream { name, text, .. } => {
            push_fenced(out, &strip_ansi(&text.text()), name == "stderr");
        },
        Output::ExecuteResult { data, .. } | Output::DisplayData { data, .. } => {
            push_mime_bundle(out, data);
        },
        Output::Error {
            ename,
            evalue,
            traceback,
            ..
        } => {
            out.push_str(&format!("**{ename}**: {evalue}\n\n"));
            let joined: String = traceback
                .iter()
                .map(|line| strip_ansi(line))
                .collect::<Vec<_>>()
                .join("\n");
            push_fenced(out, &joined, false);
        },
    }
}

/// Append the best representation of a MIME bundle.
fn push_mime_bundle(out: &mut String, data: &serde_json::Map<String, Value>) {
    if data.keys().any(|mime| mime.starts_with("image/")) {
        out.push_str("*\\[image output\\]*");
        return;
    }
    let text_of = |value: &Value| -> String {
        serde_json::from_value::<Source>(value.clone())
            .map(|source| source.text())
            .unwrap_or_default()
    };
    if let Some(markdown) = data.get("text/markdown") {
        out.push_str(text_of(markdown).trim_end());
        return;
    }
    if let Some(plain) = data.get("text/plain") {
        push_fenced(out, &strip_ansi(&text_of(plain)), false);
        return;
    }
    out.push_str("*\\[unrenderable output\\]*");
}

/// Append `text` as a fenced block (empty text still fences, keeping the
/// "this cell produced output" shape).
fn push_fenced(out: &mut String, text: &str, stderr: bool) {
    push_fence(out, if stderr { "stderr" } else { "text" }, text);
}

/// Append `body` (trailing whitespace trimmed) as a fenced code block tagged
/// `info`. The fence is one backtick longer than the longest backtick run in
/// `body` (minimum three), so no line of the body can close it early.
fn push_fence(out: &mut String, info: &str, body: &str) {
    let body = body.trim_end();
    let longest_run = body
        .split(|ch| ch != '`')
        .map(str::len)
        .max()
        .unwrap_or_default();
    let fence = "`".repeat(longest_run.saturating_add(1).max(3));
    out.push_str(&format!("{fence}{info}\n{body}\n{fence}"));
}

/// Drop ANSI escape sequences (CSI and OSC), keeping the plain text.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                for ch in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&ch) {
                        break;
                    }
                }
            },
            Some(']') => {
                chars.next();
                let mut last = '\0';
                for ch in chars.by_ref() {
                    if ch == '\u{7}' || (last == '\u{1b}' && ch == '\\') {
                        break;
                    }
                    last = ch;
                }
            },
            _ => {
                chars.next();
            },
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), crate::NotebookError>;

    #[test]
    fn strip_ansi_removes_styling_and_keeps_text() {
        assert_eq!(strip_ansi("\u{1b}[31mred\u{1b}[0m plain"), "red plain");
        assert_eq!(strip_ansi("no escapes"), "no escapes");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{7}body"), "body");
    }

    #[test]
    fn strip_ansi_handles_every_escape_shape() {
        // Multi-parameter CSI ends at its final byte, whatever the params.
        assert_eq!(strip_ansi("\u{1b}[1;38;5;196mhot\u{1b}[0m"), "hot");
        // A CSI cut off at end of input swallows the tail, not the text before.
        assert_eq!(strip_ansi("kept\u{1b}[31"), "kept");
        // OSC terminated by ST (ESC \) rather than BEL.
        assert_eq!(strip_ansi("\u{1b}]8;;http://x\u{1b}\\link"), "link");
        // An unterminated OSC drops everything after it.
        assert_eq!(strip_ansi("a\u{1b}]0;never closed"), "a");
        // A non-CSI/OSC escape drops exactly the one byte after ESC.
        assert_eq!(strip_ansi("\u{1b}(Bx"), "Bx");
        assert_eq!(strip_ansi("\u{1b}7saved"), "saved");
        // A lone ESC at end of input vanishes.
        assert_eq!(strip_ansi("end\u{1b}"), "end");
        // Non-ASCII text around escapes survives intact.
        assert_eq!(strip_ansi("\u{1b}[32mé✓\u{1b}[0m日本"), "é✓日本");
    }

    #[test]
    fn to_markdown_renders_every_cell_and_output_shape_exactly() -> TestResult {
        let notebook = crate::parse(
            r##"{
              "nbformat": 4, "nbformat_minor": 5,
              "metadata": {"kernelspec": {"language": "julia"}},
              "cells": [
                {"cell_type": "markdown", "metadata": {}, "source": ["# Head\n", "\n"]},
                {"cell_type": "raw", "metadata": {}, "source": "raw\n\n"},
                {"cell_type": "code", "metadata": {}, "execution_count": 7, "source": "x\n",
                 "outputs": [
                   {"output_type": "stream", "name": "stderr", "text": "warn\u001b[0m\n"},
                   {"output_type": "stream", "name": "stdout", "text": ""},
                   {"output_type": "execute_result", "execution_count": 7,
                    "data": {"text/plain": "\u001b[1m5\u001b[0m"}, "metadata": {}},
                   {"output_type": "display_data",
                    "data": {"application/json": {"a": 1}}, "metadata": {}},
                   {"output_type": "display_data",
                    "data": {"text/markdown": "**b**\n"}, "metadata": {}},
                   {"output_type": "error", "ename": "E", "evalue": "v",
                    "traceback": ["l1", "\u001b[31ml2\u001b[0m"]}
                 ]},
                {"cell_type": "code", "metadata": {}, "source": ""}
              ]
            }"##,
        )?;
        let expected = concat!(
            "# Head\n\n",
            "```\nraw\n```\n\n",
            "_In [7]:_\n\n```julia\nx\n```\n\n",
            "```stderr\nwarn\n```\n\n",
            "```text\n\n```\n\n",
            "```text\n5\n```\n\n",
            "*\\[unrenderable output\\]*\n\n",
            "**b**\n\n",
            "**E**: v\n\n```text\nl1\nl2\n```\n\n",
            "_In [ ]:_\n\n```julia\n\n```\n",
        );
        assert_eq!(to_markdown(&notebook), expected);
        Ok(())
    }

    #[test]
    fn a_fence_outgrows_every_backtick_run_in_its_body() -> TestResult {
        // Each cell or output carries a "```" line (or a longer run) that a
        // fixed three-backtick fence would treat as its close.
        let notebook = crate::parse(
            r#"{"nbformat": 4, "nbformat_minor": 5, "metadata": {},
                "cells": [
                  {"cell_type": "raw", "metadata": {}, "source": "a\n```\nb\n"},
                  {"cell_type": "code", "metadata": {}, "execution_count": 1,
                   "source": "s = \"\"\"\n```\n\"\"\"\n", "outputs": [
                     {"output_type": "stream", "name": "stdout", "text": "```\nout\n"},
                     {"output_type": "execute_result", "execution_count": 1,
                      "metadata": {}, "data": {"text/plain": "`````\nv"}},
                     {"output_type": "error", "ename": "E", "evalue": "v",
                      "traceback": ["```", "t"]}
                   ]},
                  {"cell_type": "code", "metadata": {}, "execution_count": 2,
                   "source": "x = `a` + ``b``", "outputs": []}
                ]}"#,
        )?;
        assert_eq!(
            to_markdown(&notebook),
            concat!(
                "````\na\n```\nb\n````\n\n",
                "_In [1]:_\n\n````python\ns = \"\"\"\n```\n\"\"\"\n````\n\n",
                "````text\n```\nout\n````\n\n",
                "``````text\n`````\nv\n``````\n\n",
                "**E**: v\n\n````text\n```\nt\n````\n\n",
                // Runs shorter than three keep the minimum fence.
                "_In [2]:_\n\n```python\nx = `a` + ``b``\n```\n",
            )
        );
        Ok(())
    }

    #[test]
    fn an_empty_notebook_renders_nothing_and_language_defaults_to_python() -> TestResult {
        let empty = crate::parse(r#"{"nbformat": 4, "nbformat_minor": 5}"#)?;
        assert_eq!(to_markdown(&empty), "");

        let bare = crate::parse(
            r#"{"nbformat": 4, "nbformat_minor": 5, "metadata": {},
                "cells": [{"cell_type": "code", "metadata": {}, "execution_count": null,
                           "source": "1", "outputs": []}]}"#,
        )?;
        assert_eq!(to_markdown(&bare), "_In [ ]:_\n\n```python\n1\n```\n");
        Ok(())
    }

    #[test]
    fn an_image_beats_markdown_and_unparseable_text_renders_empty() -> TestResult {
        let notebook = crate::parse(
            r#"{"nbformat": 4, "nbformat_minor": 5, "metadata": {},
                "cells": [{"cell_type": "code", "metadata": {}, "execution_count": 1,
                  "source": "x", "outputs": [
                    {"output_type": "display_data", "metadata": {},
                     "data": {"text/markdown": "md", "image/svg+xml": "<svg/>"}},
                    {"output_type": "display_data", "metadata": {},
                     "data": {"text/plain": 42}}
                  ]}]}"#,
        )?;
        assert_eq!(
            to_markdown(&notebook),
            concat!(
                "_In [1]:_\n\n```python\nx\n```\n\n",
                "*\\[image output\\]*\n\n",
                // A text/plain value that is neither a string nor a list of
                // strings still fences, empty.
                "```text\n\n```\n",
            )
        );
        Ok(())
    }
}
