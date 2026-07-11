//! LSP wire types → neutral `karet-core` models.
//!
//! Positions cross this boundary **unconverted**: karet-lsp is faithful to the
//! negotiated LSP encoding (UTF-16), so an `lsp_types::Position`'s `character`
//! becomes a [`LineCol::col`] still counted in UTF-16 code units. The consumer
//! (karet-session) owns the text and performs the UTF-16 ↔ UTF-32 translation via
//! `karet_text::TextBuffer`.

use karet_core::Diagnostic;
use karet_core::DiagnosticTag;
use karet_core::LineCol;
use karet_core::Location;
use karet_core::Range;
use karet_core::RelatedInfo;
use karet_core::Severity;

use crate::uri::uri_to_path;

/// Map an LSP position (UTF-16 columns, passed through — see the module docs).
pub(crate) fn position_from_lsp(p: lsp_types::Position) -> LineCol {
    LineCol::new(p.line, p.character)
}

/// Map an LSP range, normalizing any (out-of-spec) reversed endpoints.
pub(crate) fn range_from_lsp(r: lsp_types::Range) -> Range {
    let a = position_from_lsp(r.start);
    let b = position_from_lsp(r.end);
    Range {
        start: a.min(b),
        end: a.max(b),
    }
}

/// Map an LSP severity; an absent severity is treated as an error, matching the
/// common client interpretation.
pub(crate) fn severity_from_lsp(s: Option<lsp_types::DiagnosticSeverity>) -> Severity {
    match s {
        Some(lsp_types::DiagnosticSeverity::WARNING) => Severity::Warning,
        Some(lsp_types::DiagnosticSeverity::INFORMATION) => Severity::Information,
        Some(lsp_types::DiagnosticSeverity::HINT) => Severity::Hint,
        _ => Severity::Error,
    }
}

/// Map one published diagnostic. Related locations whose URIs are not `file://`
/// are dropped (karet models locations as paths).
pub(crate) fn diagnostic_from_lsp(d: lsp_types::Diagnostic) -> Diagnostic {
    Diagnostic {
        range: range_from_lsp(d.range),
        severity: severity_from_lsp(d.severity),
        message: d.message,
        source: d.source,
        code: d.code.map(|c| match c {
            lsp_types::NumberOrString::Number(n) => n.to_string(),
            lsp_types::NumberOrString::String(s) => s,
        }),
        tags: d
            .tags
            .unwrap_or_default()
            .into_iter()
            .filter_map(|t| match t {
                lsp_types::DiagnosticTag::UNNECESSARY => Some(DiagnosticTag::Unnecessary),
                lsp_types::DiagnosticTag::DEPRECATED => Some(DiagnosticTag::Deprecated),
                _ => None,
            })
            .collect(),
        related: d
            .related_information
            .unwrap_or_default()
            .into_iter()
            .filter_map(|r| {
                Some(RelatedInfo {
                    location: Location {
                        path: uri_to_path(&r.location.uri)?,
                        range: range_from_lsp(r.location.range),
                    },
                    message: r.message,
                })
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::str::FromStr;

    use super::*;

    fn lsp_range(sl: u32, sc: u32, el: u32, ec: u32) -> lsp_types::Range {
        lsp_types::Range {
            start: lsp_types::Position {
                line: sl,
                character: sc,
            },
            end: lsp_types::Position {
                line: el,
                character: ec,
            },
        }
    }

    #[test]
    fn positions_pass_through_and_reversed_ranges_normalize() {
        assert_eq!(
            range_from_lsp(lsp_range(1, 2, 3, 4)),
            Range {
                start: LineCol::new(1, 2),
                end: LineCol::new(3, 4),
            }
        );
        // Reversed endpoints (seen from buggy servers) are normalized.
        assert_eq!(
            range_from_lsp(lsp_range(3, 4, 1, 2)),
            Range {
                start: LineCol::new(1, 2),
                end: LineCol::new(3, 4),
            }
        );
    }

    #[test]
    fn severity_defaults_to_error() {
        assert_eq!(severity_from_lsp(None), Severity::Error);
        assert_eq!(
            severity_from_lsp(Some(lsp_types::DiagnosticSeverity::ERROR)),
            Severity::Error
        );
        assert_eq!(
            severity_from_lsp(Some(lsp_types::DiagnosticSeverity::WARNING)),
            Severity::Warning
        );
        assert_eq!(
            severity_from_lsp(Some(lsp_types::DiagnosticSeverity::INFORMATION)),
            Severity::Information
        );
        assert_eq!(
            severity_from_lsp(Some(lsp_types::DiagnosticSeverity::HINT)),
            Severity::Hint
        );
    }

    #[test]
    fn maps_a_full_diagnostic() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let lsp = lsp_types::Diagnostic {
            range: lsp_range(0, 1, 0, 5),
            severity: Some(lsp_types::DiagnosticSeverity::WARNING),
            code: Some(lsp_types::NumberOrString::String("E0308".into())),
            source: Some("rustc".into()),
            message: "mismatched types".into(),
            tags: Some(vec![
                lsp_types::DiagnosticTag::UNNECESSARY,
                lsp_types::DiagnosticTag::DEPRECATED,
            ]),
            related_information: Some(vec![
                lsp_types::DiagnosticRelatedInformation {
                    location: lsp_types::Location {
                        uri: lsp_types::Uri::from_str("file:///src/lib.rs")?,
                        range: lsp_range(9, 0, 9, 3),
                    },
                    message: "expected due to this".into(),
                },
                // Non-file URIs are dropped.
                lsp_types::DiagnosticRelatedInformation {
                    location: lsp_types::Location {
                        uri: lsp_types::Uri::from_str("untitled:Untitled-1")?,
                        range: lsp_range(0, 0, 0, 0),
                    },
                    message: "ignored".into(),
                },
            ]),
            ..lsp_types::Diagnostic::default()
        };
        let core = diagnostic_from_lsp(lsp);
        assert_eq!(core.severity, Severity::Warning);
        assert_eq!(core.message, "mismatched types");
        assert_eq!(core.source.as_deref(), Some("rustc"));
        assert_eq!(core.code.as_deref(), Some("E0308"));
        assert_eq!(
            core.tags,
            vec![DiagnosticTag::Unnecessary, DiagnosticTag::Deprecated]
        );
        assert_eq!(core.related.len(), 1);
        assert_eq!(core.related[0].location.path, PathBuf::from("/src/lib.rs"));
        assert_eq!(core.related[0].message, "expected due to this");
        Ok(())
    }

    #[test]
    fn numeric_codes_become_strings() {
        let lsp = lsp_types::Diagnostic {
            range: lsp_range(0, 0, 0, 1),
            code: Some(lsp_types::NumberOrString::Number(404)),
            message: "x".into(),
            ..lsp_types::Diagnostic::default()
        };
        assert_eq!(diagnostic_from_lsp(lsp).code.as_deref(), Some("404"));
    }
}
