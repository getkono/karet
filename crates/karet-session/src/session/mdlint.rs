//! The markdown-lint producer: `karet_markdown::lint` issues become a
//! diagnostics layer (`source: "markdownlint"`) on Markdown documents.
//!
//! Linting is synchronous in the actor — the rules are line-based and
//! sub-millisecond on real documents, which is cheaper than a worker
//! round-trip (the spell checker needs one because Hunspell is not).

use std::path::PathBuf;

use karet_core::Diagnostic;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::Severity;
use karet_markdown::lint;

use super::Session;
use crate::api::DocumentId;

/// The `Diagnostic::source` tag of this producer's layer.
pub(crate) const LINT_SOURCE: &str = "markdownlint";

/// Load the workspace's `.markdownlint.json(c)`, falling back to defaults.
/// Read once at session start; a config edit applies on the next session.
pub(super) fn discover_config(roots: &[PathBuf]) -> lint::Config {
    for root in roots {
        for name in [".markdownlint.json", ".markdownlint.jsonc"] {
            if let Ok(text) = std::fs::read_to_string(root.join(name))
                && let Ok(config) = lint::Config::from_json(&text)
            {
                return config;
            }
        }
    }
    lint::Config::default()
}

/// Map one lint issue onto the neutral diagnostic model. Upstream's two-level
/// severity maps down one display step, as the VS Code extension does by
/// default: rule errors show as warnings, downgraded rules as information.
fn diagnostic(issue: &lint::Issue) -> Diagnostic {
    let line = u32::try_from(issue.line).unwrap_or(u32::MAX);
    let col = u32::try_from(issue.col).unwrap_or(u32::MAX);
    let len = u32::try_from(issue.len).unwrap_or(1);
    Diagnostic {
        range: Range {
            start: LineCol::new(line, col),
            end: LineCol::new(line, col.saturating_add(len)),
        },
        severity: match issue.severity {
            lint::LintSeverity::Error => Severity::Warning,
            lint::LintSeverity::Warning => Severity::Information,
        },
        message: format!("{} ({}/{})", issue.message, issue.rule, issue.alias),
        source: Some(LINT_SOURCE.to_owned()),
        code: Some(issue.rule.to_owned()),
        tags: Vec::new(),
        related: Vec::new(),
    }
}

impl Session {
    /// Re-lint `doc` when it is a Markdown document (clearing the layer when
    /// the setting is off), publishing only when the layer changed.
    pub(crate) fn refresh_markdown_lint(&mut self, doc_id: DocumentId) {
        let enabled = self.config.settings.markdown.lint.enabled;
        let Some(doc) = self.store.docs.get_mut(&doc_id) else {
            return;
        };
        let diagnostics = if enabled && doc.language == Some("Markdown") {
            lint::lint(&doc.buffer.text(), &self.lint_config)
                .iter()
                .map(diagnostic)
                .collect()
        } else {
            Vec::new()
        };
        if doc.lint_diagnostics == diagnostics {
            return;
        }
        doc.lint_diagnostics = diagnostics;
        self.publish_document_diagnostics(doc_id);
    }
}
