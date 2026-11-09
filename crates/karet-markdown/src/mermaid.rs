//! Mermaid fence rendering (feature `mermaid`): diagram source in, Unicode
//! (or ASCII) box-drawing text out, via the pure-Rust `merman` engine.
//!
//! The engine is deliberately wrapped behind this one seam — `merman` is an
//! alpha crate with documented API churn, so swapping or upgrading it touches
//! this module alone. [`MarkdownDocument::substitute_mermaid`] swaps each
//! fence's code for its rendered diagram; anything the engine does not
//! recognize or fails on reports [`MermaidOutcome::Unsupported`], and the
//! fence keeps its source under a one-line note.

use crate::Block;
use crate::Inline;
use crate::MarkdownDocument;

/// The character set diagrams render with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MermaidCharset {
    /// Box-drawing characters (the default).
    #[default]
    Unicode,
    /// Plain ASCII, for limited terminals and copy-paste targets.
    Ascii,
}

/// What rendering a fence produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MermaidOutcome {
    /// The rendered diagram, line by line.
    Diagram(Vec<String>),
    /// The engine does not support this diagram type (or the source does not
    /// parse); the caller should show the fence source with `reason`.
    Unsupported {
        /// A short human-readable explanation.
        reason: String,
    },
}

/// A reusable mermaid renderer (parser + layout engine bundled).
pub struct MermaidRenderer {
    inner: merman::ascii::HeadlessAsciiRenderer,
}

impl MermaidRenderer {
    /// Build a renderer using `charset`.
    #[must_use]
    pub fn new(charset: MermaidCharset) -> Self {
        let charset = match charset {
            MermaidCharset::Unicode => merman::ascii::AsciiCharset::Unicode,
            MermaidCharset::Ascii => merman::ascii::AsciiCharset::Ascii,
        };
        Self {
            inner: merman::ascii::HeadlessAsciiRenderer::new().with_charset(charset),
        }
    }

    /// Render one fence's `source`. Never fails outright — an unparsable or
    /// unsupported diagram reports its reason for the source fallback.
    #[must_use]
    pub fn render(&self, source: &str) -> MermaidOutcome {
        match self.inner.render_ascii_sync(source) {
            Ok(Some(text)) => MermaidOutcome::Diagram(text.lines().map(str::to_owned).collect()),
            Ok(None) => MermaidOutcome::Unsupported {
                reason: "unsupported diagram type".to_owned(),
            },
            Err(error) => MermaidOutcome::Unsupported {
                reason: error.to_string(),
            },
        }
    }
}

impl MarkdownDocument {
    /// Replace every fence whose language is one of `languages` (compared
    /// ASCII-case-insensitively) with its diagram as rendered by `renderer`,
    /// recursing into quotes, list items and aligned HTML containers.
    ///
    /// A rendered fence becomes a language-less code block holding the diagram.
    /// A fence the engine cannot render keeps its source and gains a paragraph
    /// ahead of it, `mermaid: REASON — showing source`. A paragraph rather than a
    /// line inside the code block, because a code block is preformatted: a narrow
    /// pane would clip away the very words naming the diagram type. A note added
    /// at the top level begins on its fence's source line, so every block after it
    /// keeps its own [`block_line`](Self::block_line).
    pub fn substitute_mermaid(&mut self, renderer: &MermaidRenderer, languages: &[String]) {
        let blocks = std::mem::take(&mut self.blocks);
        let mut lines = std::mem::take(&mut self.block_lines).into_iter();
        self.blocks.reserve(blocks.len());
        self.block_lines.reserve(blocks.len());
        for block in blocks {
            let line = lines.next();
            let (note, block) = substitute_block(block, renderer, languages);
            if let Some(note) = note {
                self.blocks.push(note);
                self.block_lines.extend(line);
            }
            self.blocks.push(block);
            self.block_lines.extend(line);
        }
        self.block_lines.extend(lines);
    }
}

/// Substitute `block`, returning the note to insert ahead of it, if any.
fn substitute_block(
    mut block: Block,
    renderer: &MermaidRenderer,
    languages: &[String],
) -> (Option<Block>, Block) {
    let mut note = None;
    match &mut block {
        Block::CodeBlock {
            lang: lang @ Some(_),
            code,
        } if lang
            .as_deref()
            .is_some_and(|l| languages.iter().any(|name| name.eq_ignore_ascii_case(l))) =>
        {
            *lang = None;
            match renderer.render(code) {
                MermaidOutcome::Diagram(lines) => *code = lines.join("\n"),
                MermaidOutcome::Unsupported { reason } => {
                    note = Some(Block::Paragraph(vec![Inline::Text(format!(
                        "mermaid: {reason} — showing source"
                    ))]));
                },
            }
        },
        Block::Quote(inner) | Block::Aligned { blocks: inner, .. } => {
            substitute_nested(inner, renderer, languages);
        },
        Block::List { items, .. } => {
            for item in items {
                substitute_nested(&mut item.blocks, renderer, languages);
            }
        },
        _ => {},
    }
    (note, block)
}

/// Substitute nested `blocks`, which carry no source lines of their own.
fn substitute_nested(blocks: &mut Vec<Block>, renderer: &MermaidRenderer, languages: &[String]) {
    let mut out = Vec::with_capacity(blocks.len());
    for block in std::mem::take(blocks) {
        let (note, block) = substitute_block(block, renderer, languages);
        out.extend(note);
        out.push(block);
    }
    *blocks = out;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Alignment;

    fn substitute(source: &str) -> MarkdownDocument {
        let mut doc = crate::parse(source);
        doc.substitute_mermaid(
            &MermaidRenderer::new(MermaidCharset::Unicode),
            &["mermaid".to_owned()],
        );
        doc
    }

    /// The text of a paragraph block, if it is one.
    fn paragraph_text(block: Option<&Block>) -> Option<String> {
        let Some(Block::Paragraph(inlines)) = block else {
            return None;
        };
        Some(
            inlines
                .iter()
                .map(|i| match i {
                    Inline::Text(t) => t.as_str(),
                    _ => "",
                })
                .collect(),
        )
    }

    #[test]
    fn a_renderable_diagram_replaces_the_fence_in_place() {
        let doc = substitute("```mermaid\nflowchart TD\n    A[Start] --> B[End]\n```\n");
        assert_eq!(doc.blocks().len(), 1, "no explanatory paragraph is added");
        assert!(
            matches!(doc.blocks().first(), Some(Block::CodeBlock { lang: None, code }) if code.contains('─')),
            "the diagram replaced the source: {:?}",
            doc.blocks()
        );
    }

    #[test]
    fn an_unsupported_diagram_explains_itself_in_a_wrappable_paragraph() {
        let doc = substitute("```Mermaid\ngitGraph\n    commit\n```\n");
        assert_eq!(doc.blocks().len(), 2, "reason paragraph, then the source");
        let note = paragraph_text(doc.blocks().first()).unwrap_or_default();
        assert!(note.starts_with("mermaid: "), "{note}");
        assert!(note.ends_with(" — showing source"), "{note}");
        assert!(
            matches!(doc.blocks().get(1), Some(Block::CodeBlock { lang: None, code })
                if code.contains("gitGraph") && !code.contains("mermaid:")),
            "the source survives without the reason: {:?}",
            doc.blocks()
        );
    }

    #[test]
    fn a_note_keeps_every_later_block_on_its_own_source_line() {
        // Regression: the note used to be inserted without a source line, so every
        // later block was anchored to the line of the block before it.
        let doc = substitute("```mermaid\ngitGraph\n    commit\n```\n\n# After\n");
        assert!(matches!(doc.blocks().get(2), Some(Block::Heading { .. })));
        assert_eq!(
            (0..4).map(|i| doc.block_line(i)).collect::<Vec<_>>(),
            [Some(0), Some(0), Some(5), None]
        );

        let wrapped = doc.wrap(80);
        let heading = wrapped.wrapped_line_for_source(5);
        let text = wrapped.lines.get(heading).map(crate::WrappedLine::text);
        assert!(
            text.as_deref().is_some_and(|t| t.contains("After")),
            "{text:?}"
        );
        assert_eq!(wrapped.source_line_for_wrapped(heading), 5);
        // The fence's own line scrolls to the note explaining it, not past it.
        assert_eq!(wrapped.wrapped_line_for_source(0), 0);
    }

    #[test]
    fn diagrams_nested_in_quotes_lists_and_aligned_containers_are_substituted() {
        let doc = substitute(
            "> ```mermaid\n> gitGraph\n>     commit\n> ```\n\n- ```mermaid\n  gitGraph\n      commit\n  ```\n\n<div align=\"center\">\n\n```mermaid\nflowchart TD\n    A --> B\n```\n\n</div>\n",
        );
        let quoted = match doc.blocks().first() {
            Some(Block::Quote(inner)) => Some(inner.len()),
            _ => None,
        };
        assert_eq!(quoted, Some(2), "the quote gained its reason paragraph");
        let listed = match doc.blocks().get(1) {
            Some(Block::List { items, .. }) => items.first().map(|item| item.blocks.len()),
            _ => None,
        };
        assert_eq!(listed, Some(2), "the list item gained its reason paragraph");
        assert!(
            matches!(doc.blocks().get(2), Some(Block::Aligned { align: Alignment::Center, blocks })
                if matches!(blocks.first(), Some(Block::CodeBlock { lang: None, .. }))),
            "{:?}",
            doc.blocks()
        );
        assert_eq!(doc.blocks().len(), 3, "nested notes add no top-level block");
    }

    #[test]
    fn a_fence_in_another_language_is_left_alone() {
        let source = "```rust\nfn main() {}\n```\n";
        assert_eq!(substitute(source).blocks(), crate::parse(source).blocks());
    }

    #[test]
    fn a_flowchart_renders_as_box_drawing_text() {
        let renderer = MermaidRenderer::new(MermaidCharset::Unicode);
        let outcome = renderer.render("flowchart LR\n    a[Start] --> b[End]\n");
        let lines = match outcome {
            MermaidOutcome::Diagram(lines) => lines,
            MermaidOutcome::Unsupported { reason } => {
                assert_eq!(reason, "<supported>", "flowcharts must render");
                return;
            },
        };
        let joined = lines.join("\n");
        assert!(joined.contains("Start"));
        assert!(joined.contains("End"));
        assert!(
            joined.contains('─') || joined.contains('│'),
            "unicode charset draws with box-drawing characters"
        );
    }

    #[test]
    fn a_sequence_diagram_renders() {
        let renderer = MermaidRenderer::new(MermaidCharset::Unicode);
        let outcome =
            renderer.render("sequenceDiagram\n    Alice->>Bob: Hello\n    Bob-->>Alice: Hi\n");
        assert!(
            matches!(outcome, MermaidOutcome::Diagram(lines) if lines.join("\n").contains("Alice"))
        );
    }

    #[test]
    fn ascii_mode_avoids_box_drawing_characters() {
        let renderer = MermaidRenderer::new(MermaidCharset::Ascii);
        let outcome = renderer.render("flowchart LR\n    a[Start] --> b[End]\n");
        let lines = match outcome {
            MermaidOutcome::Diagram(lines) => lines,
            MermaidOutcome::Unsupported { reason } => {
                assert_eq!(reason, "<supported>", "flowcharts must render");
                return;
            },
        };
        assert!(!lines.join("\n").contains('─'));
    }

    #[test]
    fn garbage_and_unsupported_types_fall_back_with_a_reason() {
        let renderer = MermaidRenderer::new(MermaidCharset::Unicode);
        assert!(matches!(
            renderer.render("not a diagram at all"),
            MermaidOutcome::Unsupported { .. }
        ));
        // Gantt is outside the ascii renderer's supported families today.
        assert!(matches!(
            renderer.render("gantt\n    title A Gantt\n"),
            MermaidOutcome::Unsupported { .. }
        ));
    }
}
