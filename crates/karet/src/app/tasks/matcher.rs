//! Problem matchers: turning a task's output lines into Problems-panel entries.
//!
//! A task's `problemMatcher` settings compile once, when it starts, into
//! [`Matcher`]s; [`scan`] then reads the whole output from the top each time,
//! so a scan is idempotent and its result simply replaces the last one.
//!
//! The lines come from the terminal grid, after the emulator has interpreted
//! colours and cursor movement, so no escape code ever reaches a pattern.

use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use karet_core::Diagnostic;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::Severity;
use karet_session::config::schema::BuiltinMatcher;
use karet_session::config::schema::MatcherSeverity;
use karet_session::config::schema::PatternMatcher;
use karet_session::config::schema::ProblemMatcher;
use regex::Captures;
use regex::Regex;

use crate::app::problems::ProblemEntry;

/// How many lines below a `rustc` heading its ` --> file:line:column` line may
/// be. rustc prints it on the very next line; the slack absorbs a stray blank.
const RUSTC_LOCATION_REACH: usize = 2;

/// A compiled problem matcher.
#[derive(Debug)]
pub(crate) enum Matcher {
    /// One line is one problem.
    Line(LinePattern),
    /// `rustc`'s two-line shape: a severity heading, then its location.
    Rustc {
        /// `error[E0308]: message` / `warning: message`.
        heading: Regex,
        /// ` --> file:line:column`.
        location: Regex,
    },
}

/// A single-line pattern and which capture groups hold what.
#[derive(Debug)]
pub(crate) struct LinePattern {
    regex: Regex,
    file: usize,
    line: usize,
    column: Option<usize>,
    severity: Option<usize>,
    code: Option<usize>,
    message: Option<usize>,
    default_severity: Severity,
}

impl LinePattern {
    fn new(
        regex: &str,
        groups: [Option<usize>; 6],
        default_severity: Severity,
    ) -> Result<Self, regex::Error> {
        let [file, line, column, severity, code, message] = groups;
        Ok(Self {
            regex: Regex::new(regex)?,
            file: file.unwrap_or(1),
            line: line.unwrap_or(2),
            column,
            severity,
            code,
            message,
            default_severity,
        })
    }
}

/// Compile a task's matcher settings; the error names the pattern that does
/// not compile.
pub(crate) fn compile(specs: &[ProblemMatcher]) -> Result<Vec<Matcher>, String> {
    let mut matchers = Vec::new();
    for spec in specs {
        match spec {
            ProblemMatcher::Builtin(builtin) => matchers.extend(builtin_matchers(*builtin)),
            ProblemMatcher::Pattern(pattern) => matchers.push(Matcher::Line(custom(pattern)?)),
        }
    }
    Ok(matchers)
}

fn custom(pattern: &PatternMatcher) -> Result<LinePattern, String> {
    LinePattern::new(
        &pattern.regexp,
        [
            Some(pattern.file),
            Some(pattern.line),
            pattern.column,
            pattern.severity,
            pattern.code,
            pattern.message,
        ],
        match pattern.default_severity {
            MatcherSeverity::Warning => Severity::Warning,
            MatcherSeverity::Info => Severity::Information,
            MatcherSeverity::Hint => Severity::Hint,
            // `MatcherSeverity` is exhaustive today; a newer value reads as an error.
            _ => Severity::Error,
        },
    )
    .map_err(|error| format!("problem matcher `{}`: {error}", pattern.regexp))
}

/// The matchers a built-in name stands for. The patterns are constants, so a
/// failure to compile one is a bug a test catches; it drops that pattern.
fn builtin_matchers(builtin: BuiltinMatcher) -> Vec<Matcher> {
    let line = |regex: &str, groups| {
        LinePattern::new(regex, groups, Severity::Error)
            .ok()
            .map(Matcher::Line)
    };
    match builtin {
        BuiltinMatcher::Rustc => {
            let heading = Regex::new(r"^(error|warning)(?:\[([^\]]+)\])?: (.+)$");
            let location = Regex::new(r"^\s*--> (.+?):(\d+):(\d+)\s*$");
            match (heading, location) {
                (Ok(heading), Ok(location)) => vec![Matcher::Rustc { heading, location }],
                _ => Vec::new(),
            }
        },
        BuiltinMatcher::Gcc => line(
            r"^(.+?):(\d+):(\d+): (?:fatal )?(error|warning|note|remark): (.*)$",
            [Some(1), Some(2), Some(3), Some(4), None, Some(5)],
        )
        .into_iter()
        .collect(),
        BuiltinMatcher::Tsc => [
            line(
                r"^(.+?)\((\d+),(\d+)\): (error|warning|info) (TS\d+): (.*)$",
                [Some(1), Some(2), Some(3), Some(4), Some(5), Some(6)],
            ),
            line(
                r"^(.+?):(\d+):(\d+) - (error|warning|info) (TS\d+): (.*)$",
                [Some(1), Some(2), Some(3), Some(4), Some(5), Some(6)],
            ),
        ]
        .into_iter()
        .flatten()
        .collect(),
        BuiltinMatcher::Go => line(
            r"^(\S.*?\.go):(\d+):(?:(\d+):)? (.*)$",
            [Some(1), Some(2), Some(3), None, None, Some(4)],
        )
        .into_iter()
        .collect(),
        BuiltinMatcher::EslintCompact => line(
            r"^(.+?): line (\d+), col (\d+), (Error|Warning|Info) - (.*?)(?: \(([^()]+)\))?$",
            [Some(1), Some(2), Some(3), Some(4), Some(6), Some(5)],
        )
        .into_iter()
        .collect(),
    }
}

/// Where a scan resolves paths from, and what it stamps on what it finds.
pub(crate) struct ScanContext<'a> {
    /// The task's working directory: relative paths resolve here first.
    pub(crate) cwd: &'a Path,
    /// The workspace root: a relative path that does not exist under `cwd`
    /// resolves here (cargo prints workspace-relative paths from a member).
    pub(crate) root: &'a Path,
    /// The diagnostics' `source`: the task's label.
    pub(crate) source: &'a str,
}

/// Every problem `matchers` find in `lines`, without duplicates, in output order.
pub(crate) fn scan(
    matchers: &[Matcher],
    lines: &[String],
    context: &ScanContext,
) -> Vec<ProblemEntry> {
    let mut found: Vec<ProblemEntry> = Vec::new();
    let mut push = |entry: ProblemEntry| {
        if !found.contains(&entry) {
            found.push(entry);
        }
    };
    for matcher in matchers {
        match matcher {
            Matcher::Line(pattern) => {
                for text in lines {
                    if let Some(captures) = pattern.regex.captures(text) {
                        push(line_problem(pattern, &captures, text, context));
                    }
                }
            },
            Matcher::Rustc { heading, location } => {
                // The most recent heading still waiting for its location, and
                // the line it was on.
                let mut pending: Option<(usize, Severity, Option<String>, String)> = None;
                for (index, text) in lines.iter().enumerate() {
                    if let Some(captures) = heading.captures(text) {
                        let severity = severity_word(group(&captures, Some(1)).unwrap_or_default())
                            .unwrap_or(Severity::Error);
                        let code = group(&captures, Some(2)).map(str::to_owned);
                        let message = group(&captures, Some(3)).unwrap_or_default().to_owned();
                        pending = Some((index, severity, code, message));
                        continue;
                    }
                    let Some(captures) = location.captures(text) else {
                        continue;
                    };
                    match pending.take() {
                        Some((at, severity, code, message))
                            if index - at <= RUSTC_LOCATION_REACH =>
                        {
                            push(problem(
                                context,
                                group(&captures, Some(1)).unwrap_or_default(),
                                number(&captures, Some(2)),
                                number(&captures, Some(3)),
                                severity,
                                code,
                                message,
                            ));
                        },
                        _ => {},
                    }
                }
            },
        }
    }
    found
}

fn line_problem(
    pattern: &LinePattern,
    captures: &Captures,
    text: &str,
    context: &ScanContext,
) -> ProblemEntry {
    let severity = group(captures, pattern.severity)
        .and_then(severity_word)
        .unwrap_or(pattern.default_severity);
    let message = match pattern.message {
        Some(_) => group(captures, pattern.message).unwrap_or_default(),
        None => text.trim(),
    };
    problem(
        context,
        group(captures, Some(pattern.file)).unwrap_or_default(),
        number(captures, Some(pattern.line)),
        number(captures, pattern.column),
        severity,
        group(captures, pattern.code).map(str::to_owned),
        message.to_owned(),
    )
}

fn problem(
    context: &ScanContext,
    file: &str,
    line: u32,
    column: u32,
    severity: Severity,
    code: Option<String>,
    message: String,
) -> ProblemEntry {
    let at = LineCol::new(line.saturating_sub(1), column.saturating_sub(1));
    ProblemEntry {
        path: resolve(file, context),
        diagnostic: Diagnostic {
            range: Range { start: at, end: at },
            severity,
            message,
            source: Some(context.source.to_owned()),
            code,
            code_href: None,
            tags: Vec::new(),
            related: Vec::new(),
        },
    }
}

/// Capture group `index`'s text, when the group exists and took part.
fn group<'t>(captures: &Captures<'t>, index: Option<usize>) -> Option<&'t str> {
    captures.get(index?).map(|m| m.as_str())
}

/// A 1-based number from group `index`; `0` (meaning "the start") when the
/// group is unset, absent or not a number.
fn number(captures: &Captures, index: Option<usize>) -> u32 {
    group(captures, index)
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

/// The severity a tool's word for it names, in any case.
fn severity_word(word: &str) -> Option<Severity> {
    match word.trim().to_ascii_lowercase().as_str() {
        "error" | "err" | "fatal" | "fatal error" | "e" => Some(Severity::Error),
        "warning" | "warn" | "w" => Some(Severity::Warning),
        "info" | "information" | "note" | "remark" | "i" => Some(Severity::Information),
        "hint" | "help" => Some(Severity::Hint),
        _ => None,
    }
}

/// The absolute path `file` names: absolute already, else under the task's
/// working directory when it exists there, else under the workspace root when
/// it exists there, else under the working directory anyway.
fn resolve(file: &str, context: &ScanContext) -> PathBuf {
    let file = Path::new(file.trim());
    if file.is_absolute() {
        return normalize(file);
    }
    let in_cwd = normalize(&context.cwd.join(file));
    if in_cwd.exists() {
        return in_cwd;
    }
    let in_root = normalize(&context.root.join(file));
    if in_root.exists() { in_root } else { in_cwd }
}

/// `path` with `.` dropped and each `..` folding away the component before it,
/// without touching the filesystem, so the panel and the tab it opens name the
/// file the way every other path in the app does.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {},
            // Above the root is the root; a relative path keeps a leading `..`.
            Component::ParentDir => {
                if !out.pop() && !out.has_root() {
                    out.push(component);
                }
            },
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests;
