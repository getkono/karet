//! LSP document selectors: which documents a dynamic registration covers.
//!
//! A `client/registerCapability` may scope what it turns on with
//! `registerOptions.documentSelector` -- eslint registers formatting for
//! JavaScript and TypeScript only, a polyglot server registers hover per
//! language. Ignoring the selector enabled the feature for every document the
//! connection serves, so a request for a document the server never offered it
//! for was issued anyway and failed like a broken server.
//!
//! The glob matcher is hand-rolled rather than a dependency. The LSP glob
//! grammar is small and fully specified (`*`, `?`, `**`, `{a,b}`, `[a-z]`,
//! `[!a-z]`), the patterns and paths it runs on are short, and the one glob
//! crate the workspace already pulls in transitively (`globset`, via `ignore`)
//! would bring a regex engine into a crate that is otherwise `serde` and
//! `tokio`.

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;

use serde_json::Value;

use crate::uri;

/// A parsed `DocumentSelector`: a document matches if any filter does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Selector(Vec<Filter>);

/// One `TextDocumentFilter`. Every field that is present must match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Filter {
    language: Option<String>,
    scheme: Option<String>,
    pattern: Option<Pattern>,
    /// A notebook-cell filter, which no plain document karet opens can match.
    notebook: bool,
}

/// A filter's `pattern`: a glob over the whole path, or one relative to a base.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pattern {
    Glob(String),
    Relative { base: PathBuf, glob: String },
}

impl Selector {
    /// Read `registerOptions.documentSelector`, where `None` means every
    /// document.
    ///
    /// Absent and `null` are both "every document" by the spec: a `null`
    /// selector defers to the client's own, and karet's covers everything the
    /// connection serves. A selector of an unrecognised shape is read the same
    /// way, with a debug log, for the reason `capability::provides` reads an
    /// unrecognised provider as a yes: refusing would silently disable a
    /// feature the server has.
    pub(crate) fn from_register_options(options: Option<&Value>) -> Option<Self> {
        let selector = options?.get("documentSelector")?;
        match selector {
            Value::Array(filters) => Some(Self(filters.iter().map(Filter::parse).collect())),
            Value::Null => None,
            other => {
                tracing::debug!(selector = %other, "unrecognised documentSelector; covering every document");
                None
            },
        }
    }

    /// Whether the document at `path`, opened as `language` (if it is open),
    /// is covered.
    pub(crate) fn matches(&self, path: &Path, language: Option<&str>) -> bool {
        self.0.iter().any(|filter| filter.matches(path, language))
    }
}

impl Filter {
    fn parse(value: &Value) -> Self {
        // A bare string is the language id, as in VS Code's own selectors; the
        // spec's filters are objects, but accepting both costs nothing.
        if let Some(language) = value.as_str() {
            return Self {
                language: Some(language.to_owned()),
                ..Self::default()
            };
        }
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        Self {
            language: text("language"),
            scheme: text("scheme"),
            pattern: value.get("pattern").and_then(Pattern::parse),
            notebook: value.get("notebook").is_some(),
        }
    }

    fn matches(&self, path: &Path, language: Option<&str>) -> bool {
        if self.notebook {
            return false;
        }
        // Every document karet opens is addressed by a `file` URI.
        let scheme = self.scheme.as_deref().is_none_or(|scheme| scheme == "file");
        let language = self
            .language
            .as_deref()
            .is_none_or(|wanted| language == Some(wanted));
        let pattern = self
            .pattern
            .as_ref()
            .is_none_or(|pattern| pattern.matches(path));
        scheme && language && pattern
    }
}

impl Pattern {
    /// `None` for a pattern karet cannot use -- an unrecognised shape, or a glob
    /// whose braces do not balance or expand too far. The filter then has no
    /// pattern and covers every document its other fields allow, failing open
    /// for the same reason an unrecognised selector does.
    fn parse(value: &Value) -> Option<Self> {
        let pattern = Self::parse_shape(value)?;
        let glob = match &pattern {
            Self::Glob(glob) | Self::Relative { glob, .. } => glob,
        };
        if !expand_braces(glob, &mut Vec::new()) {
            tracing::debug!(pattern = %glob, "unusable glob in a documentSelector; ignoring it");
            return None;
        }
        Some(pattern)
    }

    fn parse_shape(value: &Value) -> Option<Self> {
        if let Some(glob) = value.as_str() {
            return Some(Self::Glob(glob.to_owned()));
        }
        // A `RelativePattern`: `baseUri` is a URI or a workspace folder.
        let glob = value.get("pattern")?.as_str()?.to_owned();
        let base = value.get("baseUri")?;
        let base = base.as_str().or_else(|| base.get("uri")?.as_str())?;
        let base = uri::uri_to_path(&lsp_types::Uri::from_str(base).ok()?)?;
        Some(Self::Relative { base, glob })
    }

    fn matches(&self, path: &Path) -> bool {
        match self {
            Self::Glob(glob) => glob_matches(glob, &slashed(path)),
            Self::Relative { base, glob } => path
                .strip_prefix(base)
                .is_ok_and(|rest| glob_matches(glob, &slashed(rest))),
        }
    }
}

/// `path` as a string with `/` separators, the form LSP globs are written for.
fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Whether `text` matches the LSP glob `pattern`.
///
/// Braces are expanded first into plain alternatives, which the matcher then
/// tries one by one. A pattern whose braces do not balance, or that expands to
/// more than [`MAX_ALTERNATIVES`], matches nothing here; a selector never asks,
/// because `Pattern::parse` drops such a pattern and so covers every document.
pub(crate) fn glob_matches(pattern: &str, text: &str) -> bool {
    let text: Vec<char> = text.chars().collect();
    let mut alternatives = Vec::new();
    if !expand_braces(pattern, &mut alternatives) {
        tracing::debug!(pattern, "unusable glob in a documentSelector");
        return false;
    }
    alternatives.iter().any(|alternative| {
        let pattern: Vec<char> = alternative.chars().collect();
        Matcher::default().matches(&pattern, &text)
    })
}

/// The most alternatives one brace pattern may expand to.
///
/// Each `{a,b}` group doubles the count, so a pattern of twenty groups is a
/// million alternatives. No real selector comes close; the cap is there so a
/// hostile or broken one is ignored rather than holding the gate's lock.
const MAX_ALTERNATIVES: usize = 256;

/// Push every alternative a brace pattern stands for onto `out`, returning
/// `false` if the braces do not balance or the expansion grows past
/// [`MAX_ALTERNATIVES`].
fn expand_braces(pattern: &str, out: &mut Vec<String>) -> bool {
    let Some(open) = pattern.find('{') else {
        if pattern.contains('}') || out.len() >= MAX_ALTERNATIVES {
            return false;
        }
        out.push(pattern.to_owned());
        return true;
    };
    let (prefix, rest) = pattern.split_at(open);
    let mut depth = 0_usize;
    let mut close = None;
    let mut bounds = Vec::new();
    for (at, ch) in rest.char_indices() {
        match ch {
            '{' => {
                depth += 1;
                if depth == 1 {
                    bounds.push(at);
                }
            },
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    close = Some(at);
                    break;
                }
            },
            ',' if depth == 1 => bounds.push(at),
            _ => {},
        }
    }
    let Some(close) = close else {
        return false;
    };
    bounds.push(close);
    // Every delimiter is one ASCII byte, so `at + 1` is the next char boundary.
    let suffix = rest.get(close + 1..).unwrap_or_default();
    bounds.windows(2).all(|pair| {
        let choice = rest.get(pair[0] + 1..pair[1]).unwrap_or_default();
        expand_braces(&format!("{prefix}{choice}{suffix}"), out)
    })
}

/// A brace-free glob matcher that remembers which states have failed.
///
/// Every state is a suffix of the pattern against a suffix of the text, so a
/// pair of remaining lengths names it exactly. Without the memo, backtracking
/// is exponential in the number of wildcards; with it, a state is tried at
/// most once.
#[derive(Default)]
struct Matcher {
    failed: HashSet<(usize, usize)>,
}

impl Matcher {
    /// Whether `pattern` matches all of `text`.
    ///
    /// `*` and `?` stay inside one path segment; `**` spans any number of them,
    /// including none.
    fn matches(&mut self, pattern: &[char], text: &[char]) -> bool {
        let state = (pattern.len(), text.len());
        if self.failed.contains(&state) {
            return false;
        }
        let hit = self.step(pattern, text);
        if !hit {
            self.failed.insert(state);
        }
        hit
    }

    fn step(&mut self, pattern: &[char], text: &[char]) -> bool {
        match pattern {
            [] => text.is_empty(),
            ['*', '*', '/', rest @ ..] => {
                // Zero segments, or resume just after any separator.
                self.matches(rest, text)
                    || (0..text.len())
                        .any(|at| text[at] == '/' && self.matches(rest, &text[at + 1..]))
            },
            ['*', '*', rest @ ..] => (0..=text.len()).any(|at| self.matches(rest, &text[at..])),
            ['*', rest @ ..] => {
                let segment = text.iter().position(|ch| *ch == '/').unwrap_or(text.len());
                (0..=segment).any(|at| self.matches(rest, &text[at..]))
            },
            ['?', rest @ ..] => text
                .split_first()
                .is_some_and(|(ch, tail)| *ch != '/' && self.matches(rest, tail)),
            ['[', rest @ ..] => match class(rest) {
                Some((member, after)) => text.split_first().is_some_and(|(ch, tail)| {
                    *ch != '/' && member(*ch) && self.matches(after, tail)
                }),
                // No closing bracket: a literal `[`.
                None => text
                    .split_first()
                    .is_some_and(|(ch, tail)| *ch == '[' && self.matches(rest, tail)),
            },
            [literal, rest @ ..] => text
                .split_first()
                .is_some_and(|(ch, tail)| ch == literal && self.matches(rest, tail)),
        }
    }
}

/// Parse a character class after its `[`, returning its test and what follows
/// the closing `]`, or `None` if it never closes.
fn class(pattern: &[char]) -> Option<(impl Fn(char) -> bool + '_, &[char])> {
    let (negated, body) = match pattern {
        ['!', body @ ..] => (true, body),
        body => (false, body),
    };
    // A `]` straight after the opening bracket is a member, not the close.
    let close = body
        .iter()
        .skip(1)
        .position(|ch| *ch == ']')
        .map(|at| at + 1)?;
    let (members, after) = (&body[..close], &body[close + 1..]);
    let test = move |ch: char| {
        let mut hit = false;
        let mut at = 0;
        while at < members.len() {
            if let [low, '-', high, ..] = members[at..] {
                hit |= (low..=high).contains(&ch);
                at += 3;
            } else {
                hit |= members[at] == ch;
                at += 1;
            }
        }
        hit != negated
    };
    Some((test, after))
}

#[cfg(test)]
#[path = "selector_tests.rs"]
mod tests;
