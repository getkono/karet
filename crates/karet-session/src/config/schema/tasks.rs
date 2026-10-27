//! `tasks.*` — named shell commands the Run Task picker offers, each run in an
//! integrated terminal, with problem matchers that turn its output into
//! Problems-panel entries.
//!
//! `tasks` is a map keyed by the task's label rather than a list, so a project
//! layer can override or extend a user-level task by name: config layers merge
//! objects key by key, while a list would replace the whole set.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;

/// One `tasks` entry: a command line and how to read its output.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Task {
    /// The command line, run by the platform shell: `/bin/sh -c` on Unix,
    /// `cmd /C` on Windows.
    pub command: String,
    /// The working directory: absolute, or relative to the workspace root.
    /// Empty runs in the workspace root.
    pub cwd: String,
    /// Environment variables set for the command, over the editor's own.
    pub env: BTreeMap<String, String>,
    /// The matchers that read the command's output for problems, which the
    /// Problems panel lists until the task runs again. Empty matches nothing.
    pub problem_matcher: Vec<ProblemMatcher>,
}

/// A problem matcher: a built-in by name, or a regular expression of your own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum ProblemMatcher {
    /// A built-in matcher, named with a leading `$`.
    Builtin(BuiltinMatcher),
    /// A matcher of your own: one regular expression tried on every output line.
    Pattern(PatternMatcher),
}

/// The built-in problem matchers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum BuiltinMatcher {
    /// `rustc` and `cargo` (clippy included): an `error[E0308]: message` or
    /// `warning: message` heading followed by its ` --> file:line:column` line.
    #[serde(rename = "$rustc")]
    Rustc,
    /// GCC and Clang: `file:line:column: error: message`.
    #[serde(rename = "$gcc")]
    Gcc,
    /// The TypeScript compiler, plain or `--pretty`:
    /// `file(line,column): error TS2322: message` or
    /// `file:line:column - error TS2322: message`.
    #[serde(rename = "$tsc")]
    Tsc,
    /// The Go toolchain: `file.go:line:column: message`, column optional.
    #[serde(rename = "$go")]
    Go,
    /// ESLint's `compact` formatter:
    /// `file: line 3, col 7, Error - message (rule)`.
    #[serde(rename = "$eslint-compact")]
    EslintCompact,
}

/// A matcher of your own. `regexp` is tried on every output line (after the
/// terminal has interpreted colours and cursor movement, so escape codes never
/// reach it); a line it matches is a problem, read from its capture groups.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PatternMatcher {
    /// The regular expression (Rust `regex` syntax).
    pub regexp: String,
    /// The capture group holding the file: absolute, or relative to the task's
    /// working directory.
    #[serde(default = "default_file_group")]
    pub file: usize,
    /// The capture group holding the 1-based line.
    #[serde(default = "default_line_group")]
    pub line: usize,
    /// The capture group holding the 1-based column; unset puts the problem at
    /// the start of its line.
    #[serde(default)]
    pub column: Option<usize>,
    /// The capture group holding the severity (`error`, `warning`, `info`,
    /// `note`, `hint`, any case); unset, or a word it does not know, uses
    /// `defaultSeverity`.
    #[serde(default)]
    pub severity: Option<usize>,
    /// The capture group holding a diagnostic code such as `E0308`.
    #[serde(default)]
    pub code: Option<usize>,
    /// The capture group holding the message; unset uses the whole line.
    #[serde(default)]
    pub message: Option<usize>,
    /// The severity of a problem whose own severity is not captured.
    #[serde(default)]
    pub default_severity: MatcherSeverity,
}

const fn default_file_group() -> usize {
    1
}

const fn default_line_group() -> usize {
    2
}

/// A problem's severity as a matcher names it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum MatcherSeverity {
    /// An error.
    #[default]
    Error,
    /// A warning.
    Warning,
    /// Information.
    Info,
    /// A hint.
    Hint,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(json: &str) -> Result<Task, serde_json::Error> {
        serde_json::from_str(json)
    }

    #[test]
    fn a_task_reads_builtin_and_pattern_matchers() -> Result<(), serde_json::Error> {
        let task = task(
            r#"{
                "command": "cargo build",
                "cwd": "crates/app",
                "env": { "RUST_LOG": "debug" },
                "problemMatcher": [
                    "$rustc",
                    { "regexp": "^(.*):(\\d+): (.*)$", "message": 3, "defaultSeverity": "warning" }
                ]
            }"#,
        )?;
        assert_eq!(task.command, "cargo build");
        assert_eq!(task.cwd, "crates/app");
        assert_eq!(task.env.get("RUST_LOG").map(String::as_str), Some("debug"));
        assert_eq!(
            task.problem_matcher,
            vec![
                ProblemMatcher::Builtin(BuiltinMatcher::Rustc),
                ProblemMatcher::Pattern(PatternMatcher {
                    regexp: "^(.*):(\\d+): (.*)$".to_owned(),
                    file: 1,
                    line: 2,
                    column: None,
                    severity: None,
                    code: None,
                    message: Some(3),
                    default_severity: MatcherSeverity::Warning,
                }),
            ]
        );
        Ok(())
    }

    #[test]
    fn unknown_matchers_and_keys_are_rejected() {
        assert!(task(r#"{ "problemMatcher": ["$nope"] }"#).is_err());
        assert!(
            task(r#"{ "problemMatcher": [{ "file": 1 }] }"#).is_err(),
            "no regexp"
        );
        assert!(
            task(r#"{ "problemMatcher": [{ "regexp": "x", "lines": 2 }] }"#).is_err(),
            "a typo in a pattern key"
        );
        assert!(task(r#"{ "comand": "make" }"#).is_err());
    }

    #[test]
    fn a_task_round_trips_through_its_serialized_form() -> Result<(), serde_json::Error> {
        let task = Task {
            command: "make".to_owned(),
            problem_matcher: vec![
                ProblemMatcher::Builtin(BuiltinMatcher::EslintCompact),
                ProblemMatcher::Pattern(PatternMatcher {
                    regexp: "x".to_owned(),
                    file: 1,
                    line: 2,
                    column: Some(3),
                    severity: Some(4),
                    code: Some(5),
                    message: Some(6),
                    default_severity: MatcherSeverity::Hint,
                }),
            ],
            ..Task::default()
        };
        let text = serde_json::to_string(&task)?;
        assert_eq!(serde_json::from_str::<Task>(&text)?, task);
        Ok(())
    }
}
