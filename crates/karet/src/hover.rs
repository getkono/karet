//! Hover UI state and pure helpers.
//!
//! The app talks to LSP only through the session seam: `Command::Hover` goes
//! out, the answering `Event::HoverResult` (correlated by request id) fills
//! [`HoverUi`], and the popup renders through `karet_widgets::hover`. The
//! popup also carries the diagnostics under the caret, so one keystroke
//! answers both "what is this?" and "what is wrong with it?". Everything
//! stateful lives on `App`; this module holds the state types and the pure
//! composition logic so they are unit-testable without an `App`.

use std::path::Path;

use karet_core::Diagnostic;
use karet_core::Hover;
use karet_core::LineCol;
use karet_core::Markup;
use karet_core::MarkupKind;
use karet_core::Severity;
use karet_session::DocumentId;
use karet_session::RequestId;
use ratatui::layout::Rect;

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
    /// Whether the server said it does not offer hover, so an empty answer
    /// has already been explained and must not be reported again.
    pub refused: bool,
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
    /// The first content row shown: the wheel scrolls a popup taller than the
    /// room the editor leaves it. Clamped on every draw.
    pub scroll: u16,
    /// Where the last frame painted the popup (empty when it was not painted),
    /// so the pointer over it scrolls it rather than the text beneath.
    pub rect: Rect,
}

/// The open scrollable diagnostic detail view (Ctrl+K Ctrl+M): the modal
/// surface for errors too long for the anchored hover popup. The markdown is
/// wrapped lazily at draw time and cached per width.
#[derive(Debug, Default)]
pub(crate) struct DiagnosticView {
    /// The composed markdown document.
    pub markdown: String,
    /// The wrap cache: `(width, wrapped)` for the width last drawn at.
    pub wrapped: Option<(u16, karet_markdown::WrappedDocument)>,
    /// The scroll offset, clamped by the view widget on every draw.
    pub scroll: u16,
    /// Where the last frame painted the view, so a click inside it is kept and
    /// a click outside it dismisses it.
    pub rect: Rect,
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
/// paragraph (its code linked to the code's documentation when the producer
/// gave one) and a list of its related locations, then a rule, then the hover
/// — plain-text hover fenced so its content cannot be misread as markdown.
/// Related paths are shown relative to `root` where they lie under it.
pub(crate) fn hover_markup(
    diagnostics: &[&Diagnostic],
    hover: Option<&Hover>,
    extra: Option<&str>,
    (pretty, root): (bool, &Path),
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
        value.push_str(&display_message(d, pretty));
        if let Some(origin) = origin(d) {
            value.push_str(&format!(" _({origin})_"));
        }
        value.push_str(&related_list(d, root));
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

/// The message text a surface should render: the formatter's markdown when
/// pretty rendering is on and a formatter claims the diagnostic, else the raw
/// message.
fn display_message(d: &Diagnostic, pretty: bool) -> std::borrow::Cow<'_, str> {
    if pretty && let Some(formatted) = karet_session::ts_errors::format_diagnostic(d) {
        return std::borrow::Cow::Owned(formatted.markdown);
    }
    std::borrow::Cow::Borrowed(&d.message)
}

/// Where a diagnostic came from: its source and its code, the code linked to
/// its documentation when the producer gave one (a link with no code reads
/// `docs`). `None` when the producer said nothing about either.
fn origin(d: &Diagnostic) -> Option<String> {
    let code = match (&d.code, &d.code_href) {
        (Some(code), Some(href)) => Some(format!("[{code}](<{href}>)")),
        (None, Some(href)) => Some(format!("[docs](<{href}>)")),
        (Some(code), None) => Some(code.clone()),
        (None, None) => None,
    };
    match (&d.source, code) {
        (Some(source), Some(code)) => Some(format!("{source} {code}")),
        (Some(source), None) => Some(source.clone()),
        (None, code) => code,
    }
}

/// A diagnostic's related locations as a markdown list, each
/// `` `path:line:col` `` (one-based) and its message, led by a blank line so
/// it follows the diagnostic's own paragraph; empty when it has none.
fn related_list(d: &Diagnostic, root: &Path) -> String {
    let mut list = String::new();
    for related in &d.related {
        let path = related
            .location
            .path
            .strip_prefix(root)
            .unwrap_or(&related.location.path);
        let start = related.location.range.start;
        list.push_str(if list.is_empty() { "\n\n" } else { "\n" });
        list.push_str(&format!(
            "- `{}:{}:{}` {}",
            path.display(),
            start.line + 1,
            start.col + 1,
            related.message
        ));
    }
    list
}

/// Compose the standalone markdown document for the diagnostic detail view:
/// each diagnostic as a `**severity** _(origin)_` heading paragraph followed
/// by its (optionally formatted) message and the list of its related
/// locations, shown as [`hover_markup`] shows them.
pub(crate) fn diagnostic_document(
    diagnostics: &[&Diagnostic],
    (pretty, root): (bool, &Path),
) -> String {
    let mut value = String::new();
    for d in diagnostics {
        if !value.is_empty() {
            value.push_str("\n\n---\n\n");
        }
        value.push_str("**");
        value.push_str(severity_label(d.severity));
        value.push_str("**");
        if let Some(origin) = origin(d) {
            value.push_str(&format!(" _({origin})_"));
        }
        value.push_str("\n\n");
        value.push_str(&display_message(d, pretty));
        value.push_str(&related_list(d, root));
    }
    value
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
            code_href: None,
            tags: Vec::new(),
            related: Vec::new(),
        }
    }

    /// Raw messages, related paths shown relative to `/w`.
    fn plain() -> (bool, &'static Path) {
        (false, Path::new("/w"))
    }

    /// A diagnostic with a linked code and two related locations, one inside
    /// the root and one outside it.
    fn linked_and_related() -> Diagnostic {
        let mut d = diag((0, 0), (0, 4), Severity::Error, "mismatched types");
        d.source = Some("rustc".to_owned());
        d.code = Some("E0308".to_owned());
        d.code_href = Some("https://doc.rust-lang.org/error_codes/E0308.html".to_owned());
        let at = |path: &str, line: u32, col: u32| karet_core::Location {
            path: std::path::PathBuf::from(path),
            range: Range {
                start: LineCol::new(line, col),
                end: LineCol::new(line, col + 1),
            },
        };
        d.related = vec![
            karet_core::RelatedInfo {
                location: at("/w/src/lib.rs", 9, 4),
                message: "expected due to this".to_owned(),
            },
            karet_core::RelatedInfo {
                location: at("/elsewhere/dep.rs", 0, 0),
                message: "defined here".to_owned(),
            },
        ];
        d
    }

    #[test]
    fn a_documented_code_is_linked_in_both_surfaces() {
        let d = linked_and_related();
        let link = "_(rustc [E0308](<https://doc.rust-lang.org/error_codes/E0308.html>))_";
        let markup = hover_markup(&[&d], None, None, plain()).map(|m| m.value);
        assert!(
            markup.as_deref().is_some_and(|value| value.contains(link)),
            "{markup:?}"
        );
        let doc = diagnostic_document(&[&d], plain());
        assert!(doc.starts_with(&format!("**error** {link}")), "{doc}");
    }

    #[test]
    fn a_link_without_a_code_reads_docs() {
        let mut d = diag((0, 0), (0, 4), Severity::Warning, "w");
        d.code_href = Some("https://example.com/w".to_owned());
        let doc = diagnostic_document(&[&d], plain());
        assert!(
            doc.contains("_(test-lsp [docs](<https://example.com/w>))_"),
            "{doc}"
        );
    }

    #[test]
    fn related_locations_list_under_their_diagnostic_in_both_surfaces() {
        let d = linked_and_related();
        let list =
            "\n\n- `src/lib.rs:10:5` expected due to this\n- `/elsewhere/dep.rs:1:1` defined here";
        let markup = hover_markup(&[&d], None, None, plain())
            .map(|m| m.value)
            .unwrap_or_default();
        assert!(markup.contains(&format!("_{list}")), "{markup}");
        let doc = diagnostic_document(&[&d], plain());
        assert!(doc.ends_with(&format!("mismatched types{list}")), "{doc}");
        // The list stays with its own diagnostic, ahead of the hover's rule.
        let h = hover(MarkupKind::Markdown, "docs");
        let composed = hover_markup(&[&d], Some(&h), None, plain())
            .map(|m| m.value)
            .unwrap_or_default();
        assert!(
            composed.ends_with("defined here\n\n---\n\ndocs"),
            "{composed}"
        );
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
        let markup = hover_markup(&[], Some(&h), None, plain()).unwrap_or(Markup {
            kind: MarkupKind::Markdown,
            value: String::new(),
        });
        assert_eq!(markup.kind, MarkupKind::PlainText);
        assert_eq!(markup.value, "just text");
    }

    #[test]
    fn nothing_at_all_yields_none() {
        assert!(hover_markup(&[], None, None, plain()).is_none());
    }

    #[test]
    fn diagnostics_compose_before_the_hover_with_a_rule_between() {
        let diags = [diag((0, 0), (0, 4), Severity::Error, "mismatched types")];
        let refs: Vec<&Diagnostic> = diags.iter().collect();
        let h = hover(MarkupKind::Markdown, "## fn main");
        let markup = hover_markup(&refs, Some(&h), None, plain()).unwrap_or(Markup {
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
        let markup = hover_markup(&refs, Some(&h), None, plain()).unwrap_or(Markup {
            kind: MarkupKind::PlainText,
            value: String::new(),
        });
        assert!(markup.value.contains("```text\n*not markdown*\n```"));
    }

    #[test]
    fn an_extra_section_leads_and_composes_with_the_hover() {
        let h = hover(MarkupKind::Markdown, "docs");
        let markup = hover_markup(&[], Some(&h), Some("**serde** `1.0` — up to date"), plain())
            .unwrap_or(Markup {
                kind: MarkupKind::PlainText,
                value: String::new(),
            });
        assert_eq!(markup.kind, MarkupKind::Markdown);
        assert!(markup.value.starts_with("**serde**"));
        assert!(markup.value.contains("---\n\ndocs"));
    }

    #[test]
    fn pretty_rendering_reformats_typescript_messages() {
        let mut d = diag(
            (0, 0),
            (0, 4),
            Severity::Error,
            "Type 'string' is not assignable to type 'number'.",
        );
        d.source = Some("typescript".to_owned());
        let refs = [&d];
        let markup = hover_markup(&refs, None, None, (true, Path::new("/w"))).unwrap_or(Markup {
            kind: MarkupKind::PlainText,
            value: String::new(),
        });
        assert!(
            markup
                .value
                .contains("`string` is not assignable to type `number`"),
            "{}",
            markup.value
        );
        let raw = hover_markup(&refs, None, None, plain()).unwrap_or(Markup {
            kind: MarkupKind::PlainText,
            value: String::new(),
        });
        assert!(raw.value.contains("'string'"), "{}", raw.value);
    }

    #[test]
    fn diagnostic_document_labels_and_separates() {
        let a = diag((0, 0), (0, 4), Severity::Error, "first");
        let b = diag((0, 0), (0, 4), Severity::Warning, "second");
        let doc = diagnostic_document(&[&a, &b], plain());
        assert!(doc.starts_with("**error** _(test-lsp)_\n\nfirst"), "{doc}");
        assert!(doc.contains("\n\n---\n\n**warning**"), "{doc}");
    }

    #[test]
    fn diagnostics_alone_still_open_the_popup() {
        let diags = [diag((0, 0), (0, 4), Severity::Warning, "shadowed")];
        let refs: Vec<&Diagnostic> = diags.iter().collect();
        let markup = hover_markup(&refs, None, None, plain()).unwrap_or(Markup {
            kind: MarkupKind::PlainText,
            value: String::new(),
        });
        assert_eq!(markup.kind, MarkupKind::Markdown);
        assert!(markup.value.contains("**warning** shadowed"));
    }
}
