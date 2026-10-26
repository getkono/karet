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
use super::whole_document_change;
use crate::api::DocumentId;
use crate::api::Event;
use crate::api::RequestId;

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
        code_href: None,
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

    /// Serve [`DocumentCommand::FixMarkdownLint`](crate::api::DocumentCommand::FixMarkdownLint):
    /// apply every autofix under the workspace lint configuration as one
    /// undoable change, and answer with how many issues it fixed.
    pub(super) fn fix_markdown_lint(&mut self, id: RequestId, doc_id: DocumentId) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let original = doc.buffer.text();
        let issues = lint::lint(&original, &self.lint_config);
        let fixable = issues.iter().filter(|issue| issue.fix.is_some()).count();
        let fixed_text = lint::apply_fixes(&original, &issues);
        let change = (fixable > 0 && fixed_text != original)
            .then(|| whole_document_change(doc, fixed_text))
            .flatten();
        let applied =
            change.is_some_and(|change| self.apply_format_edits(doc_id, change.edits, false));
        let fixed = if applied { fixable } else { 0 };
        self.emit(Some(id), Event::MarkdownLintFixed { doc: doc_id, fixed });
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::api::DocumentCommand;
    use crate::session::EventRx;
    use crate::session::SessionConfig;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A trailing space (MD009, fixable) under a proper title: nothing else fires.
    const TRAILING: &str = "# Title\n\ntext \n";

    /// A session over `roots` with `path` open, and its event stream drained.
    fn open(roots: Vec<PathBuf>, path: &Path) -> Result<(Session, EventRx, DocumentId), String> {
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots,
            ..SessionConfig::default()
        });
        session.handle(
            RequestId(1),
            DocumentCommand::Open {
                path: path.to_path_buf(),
                language: None,
            },
        );
        let mut doc = None;
        while let Some((_, event)) = events.try_recv() {
            if let Event::Opened { doc: opened, .. } = event {
                doc = Some(opened);
            }
        }
        let doc = doc.ok_or("the document did not open")?;
        Ok((session, events, doc))
    }

    /// Send a [`DocumentCommand::FixMarkdownLint`] as `id`, and answer the count its
    /// [`Event::MarkdownLintFixed`] carried.
    fn fix(session: &mut Session, events: &mut EventRx, id: u64, doc: DocumentId) -> Option<usize> {
        session.handle(RequestId(id), DocumentCommand::FixMarkdownLint { doc });
        let mut fixed = None;
        while let Some((answers, event)) = events.try_recv() {
            if let (Some(RequestId(answers)), Event::MarkdownLintFixed { doc: d, fixed: n }) =
                (answers, event)
                && answers == id
                && d == doc
            {
                fixed = Some(n);
            }
        }
        fixed
    }

    fn text(session: &Session, doc: DocumentId) -> String {
        session
            .document(doc)
            .map(|d| d.buffer().text())
            .unwrap_or_default()
    }

    /// The fixes land in the session's buffer as one undo step, and a second
    /// pass finds nothing left -- so the count is the edit, not a forecast.
    #[test]
    fn fixes_apply_as_one_undoable_change_and_report_their_count() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("doc.md");
        std::fs::write(&path, TRAILING)?;
        let (mut session, mut events, doc) = open(vec![dir.path().to_path_buf()], &path)?;

        assert_eq!(fix(&mut session, &mut events, 2, doc), Some(1));
        assert_eq!(text(&session, doc), "# Title\n\ntext\n");
        assert_eq!(fix(&mut session, &mut events, 3, doc), Some(0));
        assert_eq!(text(&session, doc), "# Title\n\ntext\n");

        session.handle(RequestId(4), DocumentCommand::Undo { doc });
        assert_eq!(
            text(&session, doc),
            TRAILING,
            "one undo restores the original"
        );
        Ok(())
    }

    /// The configuration is the session's, found across every root rather
    /// than only the first: a rule the second root turns off is not "fixed".
    #[test]
    fn a_rule_any_root_turns_off_is_left_alone() -> TestResult {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        std::fs::write(
            second.path().join(".markdownlint.json"),
            r#"{"MD009": false}"#,
        )?;
        let path = first.path().join("doc.md");
        std::fs::write(&path, TRAILING)?;
        let roots = vec![first.path().to_path_buf(), second.path().to_path_buf()];
        let (mut session, mut events, doc) = open(roots, &path)?;

        assert_eq!(fix(&mut session, &mut events, 2, doc), Some(0));
        assert_eq!(text(&session, doc), TRAILING);
        Ok(())
    }

    /// A document the session does not hold is refused on the request rather
    /// than left unanswered.
    #[test]
    fn an_unknown_document_is_answered_with_an_error() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("doc.md");
        std::fs::write(&path, TRAILING)?;
        let (mut session, mut events, doc) = open(Vec::new(), &path)?;
        let unknown = DocumentId(doc.0 + 100);

        session.handle(
            RequestId(2),
            DocumentCommand::FixMarkdownLint { doc: unknown },
        );
        let mut answered = false;
        while let Some((id, event)) = events.try_recv() {
            answered |= id == Some(RequestId(2))
                && matches!(
                    event,
                    Event::Notification {
                        severity: karet_core::Severity::Error,
                        ..
                    }
                );
        }
        assert!(answered, "the unknown document is reported on the request");
        Ok(())
    }
}
