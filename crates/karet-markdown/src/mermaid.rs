//! Mermaid fence rendering (feature `mermaid`): diagram source in, Unicode
//! (or ASCII) box-drawing text out, via the pure-Rust `merman` engine.
//!
//! The engine is deliberately wrapped behind this one seam — `merman` is an
//! alpha crate with documented API churn, so swapping or upgrading it touches
//! this module alone. A consumer (the preview) substitutes the rendered text
//! for the fence's code; anything the engine does not recognize or fails on
//! reports [`MermaidOutcome::Unsupported`] so the caller can show the source
//! instead.

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

#[cfg(test)]
mod tests {
    use super::*;

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
