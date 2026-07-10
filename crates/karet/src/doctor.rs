//! `karet --doctor`: terminal-capability diagnostics.
//!
//! Runs the same capability probes the editor performs at startup (via
//! [`crate::term_caps`]) and prints a one-line-per-check report to stdout instead
//! of starting the TUI. Raw mode is enabled only long enough to read the query
//! replies; the alternate screen is never entered.
//!
//! The module is layered so the interesting logic is pure and unit-testable:
//! [`Probes`] is plain data, [`evaluate`] maps probes + settings to [`Finding`]s,
//! [`exit_code`] and [`render`] derive the process result and the report text,
//! and only [`run`] touches the terminal.

use std::io::IsTerminal;

use karet_fileview::image::GraphicsProtocol;
use karet_session::Settings;

use crate::term_caps::TerminalCapabilities;
use crate::term_caps::{self};

/// How serious a single check's outcome is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Severity {
    /// The capability is present.
    Ok,
    /// The capability is absent (or merely informational) and karet degrades
    /// gracefully.
    Info,
    /// The capability is absent and karet cannot start or cannot honor the
    /// user's configuration.
    Error,
}

/// One line of the doctor report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    /// The severity marker for this check.
    pub severity: Severity,
    /// The short check name (the part before the colon).
    pub name: String,
    /// The human-readable outcome (the part after the colon).
    pub detail: String,
}

impl Finding {
    fn new(severity: Severity, name: &str, detail: impl Into<String>) -> Self {
        Self {
            severity,
            name: name.to_string(),
            detail: detail.into(),
        }
    }
}

/// The raw probe results the findings are evaluated from: the terminal
/// capabilities plus the identifying environment variables. Plain data, so
/// [`evaluate`] stays pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Probes {
    /// The probed terminal capabilities.
    pub caps: TerminalCapabilities,
    /// `$TERM`, when set.
    pub term: Option<String>,
    /// `$TERM_PROGRAM`, when set.
    pub term_program: Option<String>,
}

/// Restores the terminal's cooked mode on drop, so an early return (or a panic
/// unwinding through the probes) never leaves the user's shell in raw mode.
struct RawModeGuard {
    enabled: bool,
}

impl RawModeGuard {
    /// Enable raw mode, best-effort: on failure (e.g. stdin is not a tty) the
    /// probes still run — they simply time out unanswered.
    fn enable() -> Self {
        Self {
            enabled: crossterm::terminal::enable_raw_mode().is_ok(),
        }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.enabled {
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
}

/// Probe the terminal and snapshot the identifying environment. Enables raw mode
/// for the duration of the probes (required to read the query replies) and
/// restores it before returning.
fn gather() -> Probes {
    let _raw = RawModeGuard::enable();
    Probes {
        caps: term_caps::probe_all(),
        term: std::env::var("TERM").ok(),
        term_program: std::env::var("TERM_PROGRAM").ok(),
    }
}

/// Evaluate the probe results against the loaded settings into report findings.
///
/// Severity rules:
/// - kitty keyboard protocol is a hard requirement → `error` when unsupported.
/// - kitty graphics protocol → `error` only when `editor.graphicalCursor` is
///   explicitly `true` (the user demanded the graphical caret); otherwise plain
///   `info` (images degrade to halfblocks).
/// - OSC 22 pointer shapes are a nicety → never worse than `info`.
pub(crate) fn evaluate(probes: &Probes, settings: &Settings) -> Vec<Finding> {
    let caps = &probes.caps;
    let unset = || "(unset)".to_string();
    vec![
        if caps.keyboard_enhancement {
            Finding::new(Severity::Ok, "kitty keyboard protocol", "supported")
        } else {
            Finding::new(
                Severity::Error,
                "kitty keyboard protocol",
                "not supported — karet will refuse to start; use a modern terminal \
                 (kitty, ghostty, WezTerm, foot, …)",
            )
        },
        if caps.kitty_graphics_supported() {
            Finding::new(Severity::Ok, "kitty graphics protocol", "supported")
        } else if settings.editor.graphical_cursor == Some(true) {
            Finding::new(
                Severity::Error,
                "kitty graphics protocol",
                "not supported — editor.graphicalCursor is set to true, which \
                 requires the kitty graphics protocol",
            )
        } else {
            Finding::new(
                Severity::Info,
                "kitty graphics protocol",
                "not supported — images render as unicode halfblocks",
            )
        },
        match caps.osc22_pointer_shape {
            Some(true) => Finding::new(Severity::Ok, "OSC 22 pointer shapes", "supported"),
            _ => Finding::new(
                Severity::Info,
                "OSC 22 pointer shapes",
                "not supported — mouse pointer-shape hints are disabled",
            ),
        },
        Finding::new(
            Severity::Info,
            "TERM",
            probes.term.clone().unwrap_or_else(unset),
        ),
        Finding::new(
            Severity::Info,
            "TERM_PROGRAM",
            probes.term_program.clone().unwrap_or_else(unset),
        ),
        Finding::new(
            Severity::Info,
            "graphics protocol",
            match caps.effective_graphics() {
                GraphicsProtocol::Kitty => "kitty",
                GraphicsProtocol::Halfblocks => "halfblocks",
            },
        ),
        Finding::new(Severity::Info, "karet version", crate::cli::VERSION_LINE),
    ]
}

/// The process exit code for a report: 1 when any check is an error, else 0.
#[must_use]
pub(crate) fn exit_code(findings: &[Finding]) -> i32 {
    if findings.iter().any(|f| f.severity == Severity::Error) {
        1
    } else {
        0
    }
}

/// Render the findings as the report text: one line per check, each prefixed by
/// its severity marker. `color` adds ANSI colors to the markers (pass whether
/// stdout is a tty); the text is fully readable without them.
#[must_use]
pub(crate) fn render(findings: &[Finding], color: bool) -> String {
    let mut out = String::new();
    for finding in findings {
        let marker = match finding.severity {
            Severity::Ok => "ok",
            Severity::Info => "info",
            Severity::Error => "error",
        };
        // Pad to the widest marker (`[error]`) so the check names line up.
        let tag = format!("{:<7}", format!("[{marker}]"));
        if color {
            let code = match finding.severity {
                Severity::Ok => "\x1b[32m",    // green
                Severity::Info => "\x1b[36m",  // cyan
                Severity::Error => "\x1b[31m", // red
            };
            out.push_str(&format!(
                "{code}{tag}\x1b[0m {}: {}\n",
                finding.name, finding.detail
            ));
        } else {
            out.push_str(&format!("{tag} {}: {}\n", finding.name, finding.detail));
        }
    }
    out
}

/// Run the doctor: probe the terminal (raw mode on, restored before printing),
/// evaluate the findings against `settings`, print the report to stdout, and
/// return the process exit code.
pub(crate) fn run(settings: &Settings) -> i32 {
    let probes = gather();
    let findings = evaluate(&probes, settings);
    print!("{}", render(&findings, std::io::stdout().is_terminal()));
    exit_code(&findings)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fully-capable terminal's probe results.
    fn all_supported() -> Probes {
        Probes {
            caps: TerminalCapabilities {
                keyboard_enhancement: true,
                graphics_env: GraphicsProtocol::Kitty,
                kitty_graphics: Some(true),
                osc22_pointer_shape: Some(true),
            },
            term: Some("xterm-kitty".to_string()),
            term_program: None,
        }
    }

    /// A bare terminal's probe results: nothing supported, nothing answered.
    fn none_supported() -> Probes {
        Probes {
            caps: TerminalCapabilities {
                keyboard_enhancement: false,
                graphics_env: GraphicsProtocol::Halfblocks,
                kitty_graphics: None,
                osc22_pointer_shape: None,
            },
            term: Some("xterm-256color".to_string()),
            term_program: None,
        }
    }

    fn settings_with_graphical_cursor(value: Option<bool>) -> Settings {
        let mut settings = Settings::default();
        settings.editor.graphical_cursor = value;
        settings
    }

    fn finding<'a>(findings: &'a [Finding], name: &str) -> &'a Finding {
        findings
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| unreachable!("missing finding `{name}`"))
    }

    #[test]
    fn fully_capable_terminal_reports_no_errors() {
        let findings = evaluate(&all_supported(), &Settings::default());
        assert!(findings.iter().all(|f| f.severity != Severity::Error));
        assert_eq!(
            finding(&findings, "kitty keyboard protocol").severity,
            Severity::Ok
        );
        assert_eq!(
            finding(&findings, "kitty graphics protocol").severity,
            Severity::Ok
        );
        assert_eq!(
            finding(&findings, "OSC 22 pointer shapes").severity,
            Severity::Ok
        );
        assert_eq!(exit_code(&findings), 0);
    }

    #[test]
    fn missing_kitty_keyboard_is_an_error() {
        let findings = evaluate(&none_supported(), &Settings::default());
        let keyboard = finding(&findings, "kitty keyboard protocol");
        assert_eq!(keyboard.severity, Severity::Error);
        assert!(
            keyboard.detail.contains("refuse to start"),
            "the error must explain the consequence: {}",
            keyboard.detail
        );
        assert_eq!(exit_code(&findings), 1);
    }

    #[test]
    fn missing_graphics_is_info_unless_the_graphical_cursor_is_demanded() {
        // `null` (auto) and `false` (disabled) both degrade gracefully → info.
        for value in [None, Some(false)] {
            let findings = evaluate(&none_supported(), &settings_with_graphical_cursor(value));
            assert_eq!(
                finding(&findings, "kitty graphics protocol").severity,
                Severity::Info,
                "graphicalCursor={value:?} must not escalate"
            );
        }
    }

    #[test]
    fn missing_graphics_with_demanded_graphical_cursor_is_an_error() {
        let mut probes = none_supported();
        probes.caps.keyboard_enhancement = true; // isolate the graphics check
        let findings = evaluate(&probes, &settings_with_graphical_cursor(Some(true)));
        let graphics = finding(&findings, "kitty graphics protocol");
        assert_eq!(graphics.severity, Severity::Error);
        assert!(
            graphics.detail.contains("graphicalCursor"),
            "the error must say why it escalated: {}",
            graphics.detail
        );
        assert_eq!(exit_code(&findings), 1);
    }

    #[test]
    fn demanded_graphical_cursor_with_supported_graphics_stays_ok() {
        let findings = evaluate(
            &all_supported(),
            &settings_with_graphical_cursor(Some(true)),
        );
        assert_eq!(
            finding(&findings, "kitty graphics protocol").severity,
            Severity::Ok
        );
        assert_eq!(exit_code(&findings), 0);
    }

    #[test]
    fn osc22_is_never_an_error() {
        // Unanswered (None) and answered-no (Some(false)) are both plain info.
        for (probes, expected) in [
            (none_supported(), Severity::Info),
            (all_supported(), Severity::Ok),
        ] {
            let findings = evaluate(&probes, &Settings::default());
            assert_eq!(
                finding(&findings, "OSC 22 pointer shapes").severity,
                expected
            );
        }
    }

    #[test]
    fn environment_and_version_are_reported_as_info() {
        let findings = evaluate(&none_supported(), &Settings::default());
        assert_eq!(finding(&findings, "TERM").detail, "xterm-256color");
        assert_eq!(finding(&findings, "TERM_PROGRAM").detail, "(unset)");
        assert_eq!(finding(&findings, "graphics protocol").detail, "halfblocks");
        assert_eq!(
            finding(&findings, "karet version").detail,
            crate::cli::VERSION_LINE
        );
        for name in ["TERM", "TERM_PROGRAM", "graphics protocol", "karet version"] {
            assert_eq!(finding(&findings, name).severity, Severity::Info);
        }
    }

    #[test]
    fn report_renders_one_marked_line_per_finding() {
        let findings = evaluate(&none_supported(), &Settings::default());
        let report = render(&findings, false);
        let lines: Vec<&str> = report.lines().collect();
        assert_eq!(lines.len(), findings.len());
        assert!(lines[0].starts_with("[error]"));
        assert!(report.contains("[info]"));
        assert!(
            !report.contains('\x1b'),
            "colorless render must not emit ANSI escapes"
        );
        // Every line carries a `name: detail` body after its marker.
        for (line, finding) in lines.iter().zip(&findings) {
            assert!(line.contains(&format!("{}: {}", finding.name, finding.detail)));
        }
    }

    #[test]
    fn colored_report_wraps_markers_in_ansi() {
        let findings = evaluate(&all_supported(), &Settings::default());
        let report = render(&findings, true);
        assert!(report.contains("\x1b[32m")); // green ok markers
        assert!(report.contains("\x1b[0m")); // and resets
    }

    #[test]
    fn exit_code_is_zero_only_without_errors() {
        let ok = Finding::new(Severity::Ok, "a", "b");
        let info = Finding::new(Severity::Info, "a", "b");
        let error = Finding::new(Severity::Error, "a", "b");
        assert_eq!(exit_code(&[]), 0);
        assert_eq!(exit_code(&[ok.clone(), info.clone()]), 0);
        assert_eq!(exit_code(&[ok, info, error]), 1);
    }
}
