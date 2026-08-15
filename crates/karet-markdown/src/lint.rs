//! A markdown linter implementing the high-signal core of the `markdownlint`
//! rule set (the rules real CI configurations actually fire), with autofixes.
//!
//! Text in, issues out — no IO, no presentation. Rule identifiers, aliases,
//! config keys (`.markdownlint.json` shape), and inline
//! `<!-- markdownlint-disable -->` directives follow upstream so existing
//! project configurations keep working. Rules run at their upstream default
//! parameters except where [`Config`] overrides them.

mod rules;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

/// How serious an issue is. Upstream markdownlint has two levels; consumers
/// map them onto their own diagnostic vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LintSeverity {
    /// The default level.
    Error,
    /// A downgraded level, set per rule in the config.
    Warning,
}

/// A single fixable defect's repair, in whole-line terms (applied bottom-up).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fix {
    /// Replace line `line` with `text` (no trailing newline).
    Replace {
        /// The 0-based line.
        line: usize,
        /// The replacement text.
        text: String,
    },
    /// Insert a blank line before `line`.
    InsertBlankBefore {
        /// The 0-based line.
        line: usize,
    },
    /// Delete line `line`.
    Delete {
        /// The 0-based line.
        line: usize,
    },
    /// Make the file end with exactly one newline.
    EnsureTrailingNewline,
}

/// One lint finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    /// The 0-based line the issue is on.
    pub line: usize,
    /// The 0-based character column the issue starts at.
    pub col: usize,
    /// How many characters the issue spans (at least 1).
    pub len: usize,
    /// The upstream rule id (`MD009`, …).
    pub rule: &'static str,
    /// The upstream rule alias (`no-trailing-spaces`, …).
    pub alias: &'static str,
    /// A human-readable description.
    pub message: String,
    /// The severity after config mapping.
    pub severity: LintSeverity,
    /// The repair, when the rule can fix itself.
    pub fix: Option<Fix>,
}

/// Per-rule configuration state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuleState {
    On(LintSeverity),
    Off,
}

/// Linter configuration, shaped like `.markdownlint.json`.
#[derive(Clone, Debug)]
pub struct Config {
    /// The baseline for rules not named explicitly (the `default` key).
    default_on: bool,
    /// Per-rule overrides, keyed by uppercase id.
    rules: HashMap<String, RuleState>,
    /// `MD013.line_length`.
    pub line_length: usize,
    /// `MD009.br_spaces`: this many trailing spaces are a hard line break,
    /// not trailing whitespace.
    pub br_spaces: usize,
}

impl Default for Config {
    /// Everything on, upstream default parameters.
    fn default() -> Self {
        Self {
            default_on: true,
            rules: HashMap::new(),
            line_length: 80,
            br_spaces: 2,
        }
    }
}

impl Config {
    /// Parse a `.markdownlint.json` document. Unknown rules and unsupported
    /// parameter shapes are ignored rather than errors — an upstream config
    /// keeps working with the subset implemented here.
    ///
    /// # Errors
    ///
    /// [`LintConfigError`] when `json` is not a JSON object at all.
    pub fn from_json(json: &str) -> Result<Self, LintConfigError> {
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|e| LintConfigError::Json(e.to_string()))?;
        let serde_json::Value::Object(map) = value else {
            return Err(LintConfigError::NotAnObject);
        };
        let mut config = Self::default();
        for (key, v) in &map {
            if key == "default" {
                config.default_on = v.as_bool().unwrap_or(true);
                continue;
            }
            let Some(id) = rules::canonical_id(key) else {
                continue; // unknown or unimplemented rule: leave it alone
            };
            let state = match v {
                serde_json::Value::Bool(false) => RuleState::Off,
                serde_json::Value::Bool(true) => RuleState::On(LintSeverity::Error),
                serde_json::Value::String(s) if s == "warning" => {
                    RuleState::On(LintSeverity::Warning)
                },
                serde_json::Value::String(_) => RuleState::On(LintSeverity::Error),
                serde_json::Value::Object(params) => {
                    if id == "MD013"
                        && let Some(n) = params.get("line_length").and_then(|n| n.as_u64())
                    {
                        config.line_length = usize::try_from(n).unwrap_or(80);
                    }
                    if id == "MD009"
                        && let Some(n) = params.get("br_spaces").and_then(|n| n.as_u64())
                    {
                        config.br_spaces = usize::try_from(n).unwrap_or(2);
                    }
                    match params.get("severity").and_then(|s| s.as_str()) {
                        Some("warning") => RuleState::On(LintSeverity::Warning),
                        _ => RuleState::On(LintSeverity::Error),
                    }
                },
                _ => continue,
            };
            config.rules.insert(id.to_owned(), state);
        }
        Ok(config)
    }

    /// Whether `rule` runs, and at what severity.
    fn state(&self, rule: &'static str) -> Option<LintSeverity> {
        match self.rules.get(rule) {
            Some(RuleState::Off) => None,
            Some(RuleState::On(severity)) => Some(*severity),
            None if self.default_on => Some(LintSeverity::Error),
            None => None,
        }
    }
}

/// A malformed lint configuration.
#[derive(Debug, thiserror::Error)]
pub enum LintConfigError {
    /// The file is not valid JSON.
    #[error("invalid JSON: {0}")]
    Json(String),
    /// The document is valid JSON but not an object.
    #[error("the configuration must be a JSON object")]
    NotAnObject,
}

/// Which rules an inline directive names (empty = all).
fn directive_rules(rest: &str) -> Vec<String> {
    rest.split_whitespace()
        .filter_map(|w| rules::canonical_id(w).map(str::to_owned))
        .collect()
}

/// The disabled-rule state per line, driven by
/// `<!-- markdownlint-disable/enable/disable-line/disable-next-line -->`.
struct Suppressions {
    /// Per line: rules disabled there (`None` in the set = all rules).
    by_line: Vec<(Vec<String>, bool)>,
}

impl Suppressions {
    fn build(lines: &[&str]) -> Self {
        let mut running: Option<Vec<String>> = None; // Some(vec) = disabled set; empty vec = all
        let mut by_line = Vec::with_capacity(lines.len());
        let mut next_line: Option<Vec<String>> = None;
        for line in lines {
            // The state a line sees combines the running disable and any
            // single-line directive aimed at it.
            let mut disabled_all = false;
            let mut disabled: Vec<String> = Vec::new();
            if let Some(set) = &running {
                if set.is_empty() {
                    disabled_all = true;
                } else {
                    disabled.extend(set.iter().cloned());
                }
            }
            if let Some(set) = next_line.take() {
                if set.is_empty() {
                    disabled_all = true;
                } else {
                    disabled.extend(set);
                }
            }
            let trimmed = line.trim();
            if let Some(rest) = trimmed
                .strip_prefix("<!-- markdownlint-disable-line")
                .and_then(|r| r.strip_suffix("-->"))
            {
                let set = directive_rules(rest);
                if set.is_empty() {
                    disabled_all = true;
                } else {
                    disabled.extend(set);
                }
            } else if let Some(rest) = trimmed
                .strip_prefix("<!-- markdownlint-disable-next-line")
                .and_then(|r| r.strip_suffix("-->"))
            {
                next_line = Some(directive_rules(rest));
            } else if let Some(rest) = trimmed
                .strip_prefix("<!-- markdownlint-disable")
                .and_then(|r| r.strip_suffix("-->"))
            {
                running = Some(directive_rules(rest));
            } else if trimmed
                .strip_prefix("<!-- markdownlint-enable")
                .and_then(|r| r.strip_suffix("-->"))
                .is_some()
            {
                running = None;
            }
            by_line.push((disabled, disabled_all));
        }
        Self { by_line }
    }

    fn allows(&self, line: usize, rule: &str) -> bool {
        match self.by_line.get(line) {
            Some((_, true)) => false,
            Some((set, false)) => !set.iter().any(|r| r == rule),
            None => true,
        }
    }
}

/// Lint `text` under `config`.
#[must_use]
pub fn lint(text: &str, config: &Config) -> Vec<Issue> {
    let lines: Vec<&str> = text.lines().collect();
    let suppressions = Suppressions::build(&lines);
    let cx = rules::Context::new(text, &lines, config);
    let mut issues = Vec::new();
    rules::run_all(&cx, &mut issues);
    issues.retain(|issue| {
        config.state(issue.rule).is_some() && suppressions.allows(issue.line, issue.rule)
    });
    for issue in &mut issues {
        if let Some(severity) = config.state(issue.rule) {
            issue.severity = severity;
        }
    }
    issues.sort_by_key(|i| (i.line, i.col));
    issues
}

/// Apply every fix carried by `issues` to `text`, bottom-up so earlier lines
/// keep their coordinates. Overlapping fixes on one line keep the first.
#[must_use]
pub fn apply_fixes(text: &str, issues: &[Issue]) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let mut fixes: Vec<&Fix> = issues.iter().filter_map(|i| i.fix.as_ref()).collect();
    let mut ensure_trailing = false;
    fixes.retain(|f| {
        if matches!(f, Fix::EnsureTrailingNewline) {
            ensure_trailing = true;
            false
        } else {
            true
        }
    });
    let line_of = |f: &&Fix| match f {
        Fix::Replace { line, .. } | Fix::InsertBlankBefore { line } | Fix::Delete { line } => *line,
        Fix::EnsureTrailingNewline => usize::MAX,
    };
    fixes.sort_by_key(|f| std::cmp::Reverse(line_of(f)));
    let mut touched: Vec<usize> = Vec::new();
    for fix in fixes {
        match fix {
            Fix::Replace { line, text } => {
                if *line < lines.len() && !touched.contains(line) {
                    lines[*line].clone_from(text);
                    touched.push(*line);
                }
            },
            Fix::InsertBlankBefore { line } => {
                if *line <= lines.len() {
                    lines.insert(*line, String::new());
                }
            },
            Fix::Delete { line } => {
                if *line < lines.len() && !touched.contains(line) {
                    lines.remove(*line);
                    touched.push(*line);
                }
            },
            Fix::EnsureTrailingNewline => {},
        }
    }
    let mut out = lines.join("\n");
    let had_trailing = text.ends_with('\n');
    if had_trailing || ensure_trailing {
        out.push('\n');
    }
    if ensure_trailing {
        while out.ends_with("\n\n") {
            out.pop();
        }
    }
    out
}
