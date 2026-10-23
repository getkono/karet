//! What one file says about calls, before anything is resolved.
//!
//! These are facts about a single file — a call written here, an import written there —
//! so they are what the index cache stores. Which function a call *reaches* depends on
//! every other file, so that is derived afresh by [`super::link`] and never stored.

use karet_core::Range;

/// The capture names a language's call query may use.
///
/// A query naming anything else fails the per-language test rather than being silently
/// ignored, since a misspelt capture would quietly drop every call it was meant to mark.
/// Only those tests read it, so it compiles only with them.
#[cfg(test)]
pub const CAPTURES: &[&str] = &[
    CALL,
    CALL_NAME,
    CALL_QUALIFIER,
    CALL_RECEIVER,
    CALL_SELF,
    CALL_SUPER,
    CALL_NEW,
    CALL_EXTERNAL,
    CALL_WEAK,
    BODY,
    COMMENT,
    IMPORT_PATH,
    IMPORT_PREFIX,
    IMPORT_ALIAS,
    IMPORT_GLOB,
    IMPORT_HEAD,
];

/// The whole call expression. Required: its span is the site, and matches sharing one
/// span describe the same call.
pub const CALL: &str = "call";
/// The callee's name as written. Absent when the callee is computed (`f()()`, `a[0]()`).
pub const CALL_NAME: &str = "call.name";
/// A scope path written before the name: `a::b` in `a::b::f()`.
pub const CALL_QUALIFIER: &str = "call.qualifier";
/// The receiver expression of a method call: `x` in `x.f()`.
pub const CALL_RECEIVER: &str = "call.receiver";
/// The receiver is the current instance: `self`, `this`.
pub const CALL_SELF: &str = "call.self";
/// The receiver is the parent implementation: `super`.
pub const CALL_SUPER: &str = "call.super";
/// The call constructs a value: `new X()`.
pub const CALL_NEW: &str = "call.new";
/// The callee is a builtin or prelude name the language always provides.
pub const CALL_EXTERNAL: &str = "call.external";
/// The site is only probably a call — a Rust macro's arguments are tokens, not syntax.
pub const CALL_WEAK: &str = "call.weak";
/// The body of a callable, which is what separates a definition from a declaration.
pub const BODY: &str = "body";
/// A comment, or a documentation string the language writes as an expression.
pub const COMMENT: &str = "comment";
/// An imported path, whose last segment is the name it binds.
pub const IMPORT_PATH: &str = "import.path";
/// The path an import list hangs from: `a::b` in `use a::b::{c, d}`.
pub const IMPORT_PREFIX: &str = "import.prefix";
/// The name an import binds instead of its last segment.
pub const IMPORT_ALIAS: &str = "import.alias";
/// A path every member of which is imported.
pub const IMPORT_GLOB: &str = "import.glob";
/// The import binds its path's first segment, as Python's `import a.b` binds `a`.
pub const IMPORT_HEAD: &str = "import.head";

/// Who a method call is made on.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum Receiver {
    /// The current instance.
    SelfRef,
    /// The parent implementation.
    Super,
    /// A plain path, as written: `config`, `self.inner`, `os.path`.
    Path(String),
    /// Anything else — a call result, an index, a literal.
    Expr,
}

/// One call, as written.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CallSite {
    /// The callee's name, when it has one.
    pub name: Option<String>,
    /// The scope path written before the name.
    pub qualifier: Option<String>,
    /// Who the call is made on, for a method call.
    pub receiver: Option<Receiver>,
    /// Whether the call constructs a value.
    pub construct: bool,
    /// Whether the language always provides the callee.
    pub external: bool,
    /// Whether the site is only probably a call.
    pub weak: bool,
    /// Where the call is written.
    pub range: Range,
}

/// One name an import statement binds.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Import {
    /// The imported path, split into segments, prefix included.
    pub segments: Vec<String>,
    /// The name it binds when that is not the last segment.
    pub alias: Option<String>,
    /// Whether every member of the path is imported rather than the path itself.
    pub glob: bool,
    /// Whether it binds the path's first segment rather than its last.
    #[cfg_attr(feature = "serde", serde(default))]
    pub binds_head: bool,
}

impl Import {
    /// The name this import makes available, or `None` for a glob.
    #[must_use]
    pub fn binds(&self) -> Option<&str> {
        if self.glob {
            return None;
        }
        if self.binds_head && self.alias.is_none() {
            return self.segments.first().map(String::as_str);
        }
        self.alias
            .as_deref()
            .or_else(|| self.segments.last().map(String::as_str))
    }

    /// How many leading segments the bound name stands for.
    ///
    /// Usually all of them: `use a::b` makes `b` mean `a::b`. Python's `import a.b` is
    /// the exception — it binds `a`, and `a.b.f()` is then written in full — which its
    /// query states with [`IMPORT_HEAD`].
    #[must_use]
    pub fn bound_prefix(&self) -> usize {
        if self.binds_head && self.alias.is_none() {
            return 1;
        }
        self.segments.len()
    }
}

/// What the structural tier knows about one callable node.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Callable {
    /// Lines whose first code byte this callable owns: not blank, not comment-only, and
    /// not inside a nested named function, which owns its own.
    pub code_lines: u32,
    /// Physical lines spanned, nested functions and comments included.
    pub span_lines: u32,
    /// Whether it has a body, as opposed to being a bare declaration.
    pub has_body: bool,
    /// Every call made directly from it, closures included, in source order.
    pub sites: Vec<CallSite>,
}

/// What one file says that belongs to no single callable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FileCalls {
    /// Every name the file imports.
    pub imports: Vec<Import>,
    /// Calls made outside any callable — at module level, in a field initializer.
    pub loose_sites: u32,
}

/// Split a written path into its segments, whatever the language's separator.
///
/// `a::b`, `a.b`, `./a/b` and `"./a/b.js"` all name the same two segments as far as
/// resolution by name is concerned. Generic arguments are dropped: `Vec::<u8>::new` is
/// `Vec::new` to a resolver that only has names.
#[must_use]
pub fn segments(path: &str) -> Vec<String> {
    let mut cleaned = String::with_capacity(path.len());
    let mut depth = 0u32;
    for ch in path
        .trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .chars()
    {
        match ch {
            '<' => depth = depth.saturating_add(1),
            '>' => depth = depth.saturating_sub(1),
            _ if depth > 0 => {},
            ':' | '.' | '/' | '\\' => cleaned.push(' '),
            c if c.is_whitespace() => {},
            c => cleaned.push(c),
        }
    }
    let mut out: Vec<String> = cleaned
        .split(' ')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    // A module path written as a file names the module, not the file.
    if out.len() > 1
        && out.last().is_some_and(|last| {
            ["js", "ts", "tsx", "jsx", "mjs", "cjs", "h", "hpp"].contains(&last.as_str())
        })
    {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_split_the_same_way_in_every_language() {
        assert_eq!(segments("a::b"), ["a", "b"]);
        assert_eq!(segments("a.b.c"), ["a", "b", "c"]);
        assert_eq!(segments("\"./util/strings.js\""), ["util", "strings"]);
        assert_eq!(segments("'../core'"), ["core"]);
        assert_eq!(segments("Vec::<u8>::new"), ["Vec", "new"]);
        assert_eq!(segments("HashMap<K, V>"), ["HashMap"]);
        assert_eq!(segments(""), Vec::<String>::new());
    }

    #[test]
    fn an_import_binds_its_alias_its_last_segment_or_nothing() {
        let plain = Import {
            segments: vec!["a".to_owned(), "b".to_owned()],
            alias: None,
            glob: false,
            binds_head: false,
        };
        assert_eq!(plain.binds(), Some("b"));
        let aliased = Import {
            alias: Some("c".to_owned()),
            ..plain.clone()
        };
        assert_eq!(aliased.binds(), Some("c"));
        assert_eq!(aliased.bound_prefix(), 2);
        let glob = Import {
            glob: true,
            ..plain.clone()
        };
        assert_eq!(glob.binds(), None);
        // Python's `import a.b` binds `a`, standing for itself alone.
        let dotted = Import {
            binds_head: true,
            ..plain.clone()
        };
        assert_eq!(dotted.binds(), Some("a"));
        assert_eq!(dotted.bound_prefix(), 1);
        // An alias that happens to equal the first segment is still an ordinary alias:
        // `import * as api from './api/client'` stands for the whole path.
        let coincident = Import {
            alias: Some("a".to_owned()),
            ..plain
        };
        assert_eq!(coincident.binds(), Some("a"));
        assert_eq!(coincident.bound_prefix(), 2);
    }

    #[test]
    fn every_capture_name_is_listed_once() {
        let mut names = CAPTURES.to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), CAPTURES.len());
    }
}
