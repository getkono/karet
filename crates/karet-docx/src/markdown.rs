//! Convert the neutral [`Document`] model to markdown text.
//!
//! karet already has a full markdown render pipeline (`karet-markdown`), so DOCX
//! display is DOCX → markdown → that existing machinery. The mapping:
//!
//! - headings → `#`-prefixed lines;
//! - bold → `**…**`, italic → `*…*`, strikethrough → `~~…~~`;
//! - **underline has no markdown form, so underlined text passes through as plain
//!   text** (the emphasis is dropped, the text is kept);
//! - list items → `-` (bulleted) or `1.` (ordered), indented two spaces per level;
//! - hyperlinks → `[text](url)`;
//! - tables → GitHub pipe tables, with `|` escaped inside cells;
//! - blocks are separated by a blank line.

use crate::model::Block;
use crate::model::Document;
use crate::model::ParaStyle;
use crate::model::Span;

/// Render a parsed [`Document`] to a markdown string.
#[must_use]
pub fn to_markdown(document: &Document) -> String {
    let mut out = String::new();
    for block in &document.blocks {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        match block {
            Block::Paragraph { style, spans } => render_paragraph(&mut out, *style, spans),
            Block::Table { rows } => render_table(&mut out, rows),
        }
    }
    out
}

/// Render one paragraph block, honoring its heading/list/normal style.
fn render_paragraph(out: &mut String, style: ParaStyle, spans: &[Span]) {
    let inline = render_inline(spans);
    match style {
        ParaStyle::Heading(level) => {
            for _ in 0..level.clamp(1, 6) {
                out.push('#');
            }
            out.push(' ');
            out.push_str(&inline);
        },
        ParaStyle::ListItem { ordered, level } => {
            out.push_str(&"  ".repeat(level as usize));
            out.push_str(if ordered { "1. " } else { "- " });
            out.push_str(&inline);
        },
        ParaStyle::Normal => out.push_str(&inline),
    }
}

/// Render a run of spans to inline markdown.
fn render_inline(spans: &[Span]) -> String {
    let mut out = String::new();
    for span in spans {
        out.push_str(&render_span(span));
    }
    out
}

/// Render a single span, applying its emphasis and hyperlink wrapping.
fn render_span(span: &Span) -> String {
    // A whitespace-only (or empty) run gets no markers — `** **` renders literally
    // rather than as emphasis — so its raw text passes straight through.
    if span.text.trim().is_empty() {
        return span.text.clone();
    }
    let mut s = span.text.clone();
    if span.strike {
        s = format!("~~{s}~~");
    }
    if span.bold {
        s = format!("**{s}**");
    }
    if span.italic {
        s = format!("*{s}*");
    }
    // Underline has no markdown equivalent: the text is kept, the emphasis dropped.
    if let Some(url) = &span.link {
        s = format!("[{s}]({url})");
    }
    s
}

/// Render a table as a GitHub pipe table (first row treated as the header).
fn render_table(out: &mut String, rows: &[Vec<Vec<Span>>]) {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    if columns == 0 {
        return;
    }
    let mut lines: Vec<String> = Vec::with_capacity(rows.len() + 1);
    lines.push(render_row(&rows[0], columns));
    lines.push(format!("|{}", " --- |".repeat(columns)));
    for row in &rows[1..] {
        lines.push(render_row(row, columns));
    }
    out.push_str(&lines.join("\n"));
}

/// Render one table row to a pipe-delimited line padded to `columns` cells.
fn render_row(cells: &[Vec<Span>], columns: usize) -> String {
    let mut line = String::from("|");
    for column in 0..columns {
        let content = cells
            .get(column)
            .map(|c| render_cell(c))
            .unwrap_or_default();
        line.push(' ');
        line.push_str(&content);
        line.push_str(" |");
    }
    line
}

/// Render a cell's spans inline, escaping `|` and flattening newlines so the cell
/// stays on one table row.
fn render_cell(spans: &[Span]) -> String {
    render_inline(spans).replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(style: ParaStyle, spans: Vec<Span>) -> Block {
        Block::Paragraph { style, spans }
    }

    #[test]
    fn headings_are_hash_prefixed_and_clamped() {
        let doc = Document {
            blocks: vec![
                para(ParaStyle::Heading(1), vec![Span::text("Title")]),
                para(ParaStyle::Heading(9), vec![Span::text("Deep")]),
            ],
        };
        assert_eq!(to_markdown(&doc), "# Title\n\n###### Deep");
    }

    #[test]
    fn emphasis_wraps_and_nests() {
        let span = Span {
            text: "x".into(),
            bold: true,
            italic: true,
            strike: true,
            ..Span::default()
        };
        assert_eq!(render_span(&span), "***~~x~~***");
    }

    #[test]
    fn underline_passes_through_without_markers() {
        let span = Span {
            text: "note".into(),
            underline: true,
            ..Span::default()
        };
        assert_eq!(render_span(&span), "note");
    }

    #[test]
    fn link_wraps_outermost() {
        let span = Span {
            text: "docs".into(),
            bold: true,
            link: Some("https://example.com".into()),
            ..Span::default()
        };
        assert_eq!(render_span(&span), "[**docs**](https://example.com)");
    }

    #[test]
    fn whitespace_span_keeps_raw_text() {
        let span = Span {
            text: " ".into(),
            bold: true,
            ..Span::default()
        };
        assert_eq!(render_span(&span), " ");
    }

    #[test]
    fn ordered_and_bulleted_lists_indent_per_level() {
        let doc = Document {
            blocks: vec![
                para(
                    ParaStyle::ListItem {
                        ordered: false,
                        level: 0,
                    },
                    vec![Span::text("a")],
                ),
                para(
                    ParaStyle::ListItem {
                        ordered: true,
                        level: 1,
                    },
                    vec![Span::text("b")],
                ),
            ],
        };
        assert_eq!(to_markdown(&doc), "- a\n\n  1. b");
    }

    #[test]
    fn table_renders_as_pipe_table_and_escapes_pipes() {
        let doc = Document {
            blocks: vec![Block::Table {
                rows: vec![
                    vec![vec![Span::text("H1")], vec![Span::text("H2")]],
                    vec![vec![Span::text("a|b")], vec![Span::text("c")]],
                ],
            }],
        };
        assert_eq!(
            to_markdown(&doc),
            "| H1 | H2 |\n| --- | --- |\n| a\\|b | c |"
        );
    }

    #[test]
    fn blocks_are_blank_line_separated() {
        let doc = Document {
            blocks: vec![
                para(ParaStyle::Normal, vec![Span::text("one")]),
                para(ParaStyle::Normal, vec![Span::text("two")]),
            ],
        };
        assert_eq!(to_markdown(&doc), "one\n\ntwo");
    }
}
