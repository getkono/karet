//! Hover UI state and pure helpers.
//!
//! The app talks to LSP only through the session seam: `Command::Hover` goes
//! out, the answering `Event::HoverResult` (correlated by request id) fills
//! [`HoverUi`], and the popup renders through `karet_widgets::hover`. The
//! popup also carries the diagnostics under the caret, so one keystroke
//! answers both "what is this?" and "what is wrong with it?". Everything
//! stateful lives on `App`; this module holds the state types and the pure
//! composition logic so they are unit-testable without an `App`.

use karet_core::Diagnostic;
use karet_core::Hover;
use karet_core::LineCol;
use karet_core::Markup;
use karet_core::MarkupKind;
use karet_core::Severity;
use karet_session::DocumentId;
use karet_session::RequestId;

/// An in-flight hover request awaiting its `Event::HoverResult`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingHover {
    /// The request id the answering event must carry.
    pub id: RequestId,
    /// The document the request targeted.
    pub doc: DocumentId,
    /// The caret position the request was made at; the answer is dropped if
    /// the caret has moved on.
    pub at: LineCol,
}

/// The open hover popup.
#[derive(Debug)]
pub(crate) struct HoverUi {
    /// The composed markup the popup renders.
    pub markup: Markup,
    /// The document the popup describes.
    pub doc: DocumentId,
    /// The caret position the popup is anchored to; it dismisses when the
    /// caret leaves.
    pub at: LineCol,
}

/// The diagnostics whose range covers `at` (end-exclusive columns, matching
/// the editor's underline extent).
pub(crate) fn diagnostics_at(diagnostics: &[Diagnostic], at: LineCol) -> Vec<&Diagnostic> {
    diagnostics
        .iter()
        .filter(|d| {
            let r = d.range;
            if at.line < r.start.line || at.line > r.end.line {
                return false;
            }
            let lo = if at.line == r.start.line {
                r.start.col
            } else {
                0
            };
            let hi = if at.line == r.end.line {
                r.end.col
            } else {
                u32::MAX
            };
            at.col >= lo && at.col < hi
        })
        .collect()
}

/// A short lowercase label for a diagnostic severity.
fn severity_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Hint => "hint",
        // `Severity` is non-exhaustive; anything newer reads as informational.
        _ => "info",
    }
}

/// Compose the popup content from the diagnostics under the caret and the
/// server's hover answer. `None` means there is nothing to show at all.
///
/// With no diagnostics the server markup passes through untouched (its own
/// kind preserved). With diagnostics, everything is composed into one
/// markdown document: each diagnostic as a `**severity** message (source)`
/// paragraph, then a rule, then the hover — plain-text hover fenced so its
/// content cannot be misread as markdown.
pub(crate) fn hover_markup(
    diagnostics: &[&Diagnostic],
    hover: Option<&Hover>,
    extra: Option<&str>,
) -> Option<Markup> {
    if diagnostics.is_empty() && extra.is_none() {
        return hover.map(|h| h.contents.clone());
    }
    let mut value = String::new();
    if let Some(extra) = extra {
        value.push_str(extra);
    }
    for d in diagnostics {
        if !value.is_empty() {
            value.push_str("\n\n");
        }
        value.push_str("**");
        value.push_str(severity_label(d.severity));
        value.push_str("** ");
        value.push_str(&d.message);
        let origin = match (&d.source, &d.code) {
            (Some(source), Some(code)) => Some(format!("{source} {code}")),
            (Some(source), None) => Some(source.clone()),
            (None, Some(code)) => Some(code.clone()),
            (None, None) => None,
        };
        if let Some(origin) = origin {
            value.push_str(&format!(" _({origin})_"));
        }
    }
    if let Some(h) = hover {
        if !value.is_empty() {
            value.push_str("\n\n---\n\n");
        }
        match h.contents.kind {
            MarkupKind::Markdown => value.push_str(&h.contents.value),
            _ => {
                value.push_str("```text\n");
                value.push_str(&h.contents.value);
                value.push_str("\n```");
            },
        }
    }
    Some(Markup {
        kind: MarkupKind::Markdown,
        value,
    })
}

#[cfg(test)]
mod tests {
    use karet_core::Range;

    use super::*;

    fn diag(start: (u32, u32), end: (u32, u32), severity: Severity, message: &str) -> Diagnostic {
        Diagnostic {
            range: Range {
                start: LineCol::new(start.0, start.1),
                end: LineCol::new(end.0, end.1),
            },
            severity,
            message: message.to_owned(),
            source: Some("test-lsp".to_owned()),
            code: None,
            tags: Vec::new(),
            related: Vec::new(),
        }
    }

    fn hover(kind: MarkupKind, value: &str) -> Hover {
        Hover {
            contents: Markup {
                kind,
                value: value.to_owned(),
            },
            range: None,
        }
    }

    #[test]
    fn diagnostics_at_uses_end_exclusive_columns() {
        let diags = [diag((0, 4), (0, 8), Severity::Error, "boom")];
        assert_eq!(diagnostics_at(&diags, LineCol::new(0, 4)).len(), 1);
        assert_eq!(diagnostics_at(&diags, LineCol::new(0, 7)).len(), 1);
        assert!(diagnostics_at(&diags, LineCol::new(0, 8)).is_empty());
        assert!(diagnostics_at(&diags, LineCol::new(1, 5)).is_empty());
    }

    #[test]
    fn diagnostics_at_spans_interior_lines_fully() {
        let diags = [diag((1, 6), (3, 2), Severity::Warning, "spans lines")];
        assert_eq!(diagnostics_at(&diags, LineCol::new(2, 0)).len(), 1);
        assert_eq!(diagnostics_at(&diags, LineCol::new(2, 999)).len(), 1);
        assert!(diagnostics_at(&diags, LineCol::new(1, 5)).is_empty());
        assert!(diagnostics_at(&diags, LineCol::new(3, 2)).is_empty());
    }

    #[test]
    fn bare_hover_passes_through_with_its_own_kind() {
        let h = hover(MarkupKind::PlainText, "just text");
        let markup = hover_markup(&[], Some(&h), None).unwrap_or(Markup {
            kind: MarkupKind::Markdown,
            value: String::new(),
        });
        assert_eq!(markup.kind, MarkupKind::PlainText);
        assert_eq!(markup.value, "just text");
    }

    #[test]
    fn nothing_at_all_yields_none() {
        assert!(hover_markup(&[], None, None).is_none());
    }

    #[test]
    fn diagnostics_compose_before_the_hover_with_a_rule_between() {
        let diags = [diag((0, 0), (0, 4), Severity::Error, "mismatched types")];
        let refs: Vec<&Diagnostic> = diags.iter().collect();
        let h = hover(MarkupKind::Markdown, "## fn main");
        let markup = hover_markup(&refs, Some(&h), None).unwrap_or(Markup {
            kind: MarkupKind::PlainText,
            value: String::new(),
        });
        assert_eq!(markup.kind, MarkupKind::Markdown);
        assert!(markup.value.starts_with("**error** mismatched types"));
        assert!(markup.value.contains("_(test-lsp)_"));
        assert!(markup.value.contains("\n\n---\n\n## fn main"));
    }

    #[test]
    fn plain_text_hover_is_fenced_when_composed_with_diagnostics() {
        let diags = [diag((0, 0), (0, 4), Severity::Hint, "unused")];
        let refs: Vec<&Diagnostic> = diags.iter().collect();
        let h = hover(MarkupKind::PlainText, "*not markdown*");
        let markup = hover_markup(&refs, Some(&h), None).unwrap_or(Markup {
            kind: MarkupKind::PlainText,
            value: String::new(),
        });
        assert!(markup.value.contains("```text\n*not markdown*\n```"));
    }

    #[test]
    fn an_extra_section_leads_and_composes_with_the_hover() {
        let h = hover(MarkupKind::Markdown, "docs");
        let markup =
            hover_markup(&[], Some(&h), Some("**serde** `1.0` — up to date")).unwrap_or(Markup {
                kind: MarkupKind::PlainText,
                value: String::new(),
            });
        assert_eq!(markup.kind, MarkupKind::Markdown);
        assert!(markup.value.starts_with("**serde**"));
        assert!(markup.value.contains("---\n\ndocs"));
    }

    #[test]
    fn diagnostics_alone_still_open_the_popup() {
        let diags = [diag((0, 0), (0, 4), Severity::Warning, "shadowed")];
        let refs: Vec<&Diagnostic> = diags.iter().collect();
        let markup = hover_markup(&refs, None, None).unwrap_or(Markup {
            kind: MarkupKind::PlainText,
            value: String::new(),
        });
        assert_eq!(markup.kind, MarkupKind::Markdown);
        assert!(markup.value.contains("**warning** shadowed"));
    }
}
