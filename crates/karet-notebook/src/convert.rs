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
            CellKind::Raw => {
                out.push_str("```\n");
                out.push_str(cell.source.text().trim_end());
                out.push_str("\n```");
            },
            CellKind::Code => {
                let counter = match cell.execution_count {
                    Some(Some(count)) => format!("In [{count}]"),
                    _ => "In [ ]".to_owned(),
                };
                out.push_str(&format!("_{counter}:_\n\n```{language}\n"));
                out.push_str(cell.source.text().trim_end());
                out.push_str("\n```");
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
    let info = if stderr { "stderr" } else { "text" };
    out.push_str(&format!("```{info}\n"));
    out.push_str(text.trim_end());
    out.push_str("\n```");
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

    #[test]
    fn strip_ansi_removes_styling_and_keeps_text() {
        assert_eq!(strip_ansi("\u{1b}[31mred\u{1b}[0m plain"), "red plain");
        assert_eq!(strip_ansi("no escapes"), "no escapes");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{7}body"), "body");
    }
}
