//! `editor.inlineDiagnostics.*` — which diagnostics show their message at the
//! end of their line.
//!
//! The gutter sign and the underline are drawn whatever this says: they cost
//! one cell and the text the diagnostic is about, where a message takes the
//! end-of-line slot that inline blame and dependency hints share.

use serde::Deserialize;
use serde::Serialize;

/// `editor.inlineDiagnostics.*` — which diagnostics show their message at the
/// end of their line.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct InlineDiagnostics {
    /// Which lines show their diagnostics' messages: `"off"`, `"currentLine"`
    /// (the lines a caret is on), or `"all"`.
    pub mode: InlineDiagnosticsMode,
    /// The least severe diagnostic whose message is shown: `"error"`,
    /// `"warning"`, `"info"` or `"hint"`.
    pub min_severity: DiagnosticSeverity,
}

/// Which lines show their diagnostics' messages
/// (`editor.inlineDiagnostics.mode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum InlineDiagnosticsMode {
    /// No line does.
    Off,
    /// Only the lines a caret is on. The default: the message is there when
    /// the caret is, without filling every line of a broken file with errors.
    #[default]
    CurrentLine,
    /// Every line a diagnostic starts on.
    All,
}

/// A diagnostic severity, as a settings threshold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticSeverity {
    /// Errors only.
    Error,
    /// Warnings and errors.
    Warning,
    /// Information, warnings and errors.
    Info,
    /// Every severity. The default.
    #[default]
    Hint,
}

impl DiagnosticSeverity {
    /// The model severity this threshold names.
    #[must_use]
    pub fn severity(self) -> karet_core::Severity {
        match self {
            Self::Error => karet_core::Severity::Error,
            Self::Warning => karet_core::Severity::Warning,
            Self::Info => karet_core::Severity::Information,
            Self::Hint => karet_core::Severity::Hint,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_shows_every_severity_on_the_caret_lines() {
        let parsed: Option<InlineDiagnostics> = serde_json::from_str("{}").ok();
        assert_eq!(
            parsed,
            Some(InlineDiagnostics {
                mode: InlineDiagnosticsMode::CurrentLine,
                min_severity: DiagnosticSeverity::Hint,
            })
        );
        assert_eq!(parsed, Some(InlineDiagnostics::default()));
    }

    #[test]
    fn the_settings_spellings_parse_and_name_their_model_severity() {
        let parsed: Option<InlineDiagnostics> =
            serde_json::from_str(r#"{ "mode": "all", "minSeverity": "info" }"#).ok();
        assert_eq!(
            parsed,
            Some(InlineDiagnostics {
                mode: InlineDiagnosticsMode::All,
                min_severity: DiagnosticSeverity::Info,
            })
        );
        assert_eq!(
            DiagnosticSeverity::Info.severity(),
            karet_core::Severity::Information
        );
        assert!(serde_json::from_str::<InlineDiagnostics>(r#"{ "mode": "on" }"#).is_err());
        assert!(serde_json::from_str::<InlineDiagnostics>(r#"{ "severity": "hint" }"#).is_err());
    }
}
