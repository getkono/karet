//! The catalogue of machine-maintained paths and the path → [`Generated`]
//! resolver.
//!
//! "Generated" is a property a *reader* cares about: a lockfile, a minified
//! bundle, or a vendored tree is real content, but nobody reviews it line by
//! line. Renderers use this to fold that content away by default and say why.
//!
//! One table per rule shape is the single source of truth, keyed by well-known
//! **filename**, by a **directory** segment, and by a path **suffix**. Adding a
//! convention is a one-line edit here.
//!
//! This is deliberately separate from [`crate::file_type_for_path`]: a file's
//! *identity* (a lockfile is TOML) is independent of whether it is
//! *hand-written*. `Cargo.lock` is both "Cargo lockfile" and [`Generated::Lockfile`].

use std::path::Path;

/// Why a path is considered machine-maintained rather than hand-written.
///
/// Resolve one from a path with [`generated_for_path`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum Generated {
    /// A dependency lockfile resolved by a package manager.
    Lockfile,
    /// A minified or bundled artifact built from readable sources.
    Minified,
    /// A third-party tree checked in verbatim.
    Vendored,
    /// Compiler, codegen, or build-system output.
    BuildOutput,
    /// A recorded test snapshot, rewritten by the test runner.
    Snapshot,
    /// Content carrying an explicit generated banner.
    Marker,
}

impl Generated {
    /// The terse reason to show beside a folded file, in the lowercase style the
    /// diff renderer already uses for its own placeholders.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::Lockfile => "lockfile",
            Self::Minified => "minified",
            Self::Vendored => "vendored",
            Self::BuildOutput => "build output",
            Self::Snapshot => "snapshot",
            Self::Marker => "generated",
        }
    }
}

/// Well-known filenames, matched case-insensitively against the last component.
///
/// Extension-only lockfiles (`*.lock`) are covered by [`SUFFIXES`]; these are the
/// ones whose name carries the meaning.
const FILENAMES: &[(&str, Generated)] = &[
    ("Cargo.lock", Generated::Lockfile),
    ("package-lock.json", Generated::Lockfile),
    ("npm-shrinkwrap.json", Generated::Lockfile),
    ("yarn.lock", Generated::Lockfile),
    ("pnpm-lock.yaml", Generated::Lockfile),
    ("bun.lock", Generated::Lockfile),
    ("bun.lockb", Generated::Lockfile),
    ("deno.lock", Generated::Lockfile),
    ("composer.lock", Generated::Lockfile),
    ("Pipfile.lock", Generated::Lockfile),
    ("poetry.lock", Generated::Lockfile),
    ("pdm.lock", Generated::Lockfile),
    ("uv.lock", Generated::Lockfile),
    ("Gemfile.lock", Generated::Lockfile),
    ("go.sum", Generated::Lockfile),
    ("Package.resolved", Generated::Lockfile),
    ("gradle.lockfile", Generated::Lockfile),
    ("mix.lock", Generated::Lockfile),
    ("flake.lock", Generated::Lockfile),
    ("packages.lock.json", Generated::Lockfile),
    ("cabal.project.freeze", Generated::Lockfile),
    ("conan.lock", Generated::Lockfile),
];

/// Directory names, matched case-sensitively against **any** segment of the path.
///
/// A vendored tree is vendored wherever it sits, so `crates/x/vendor/y.rs`
/// matches as surely as `vendor/y.rs`.
const DIRECTORIES: &[(&str, Generated)] = &[
    ("vendor", Generated::Vendored),
    ("vendored", Generated::Vendored),
    ("third_party", Generated::Vendored),
    ("thirdparty", Generated::Vendored),
    ("node_modules", Generated::Vendored),
    ("Pods", Generated::Vendored),
    ("bower_components", Generated::Vendored),
    ("target", Generated::BuildOutput),
    ("dist", Generated::BuildOutput),
    ("build", Generated::BuildOutput),
    (".next", Generated::BuildOutput),
    (".nuxt", Generated::BuildOutput),
    (".svelte-kit", Generated::BuildOutput),
    ("__pycache__", Generated::BuildOutput),
    ("__snapshots__", Generated::Snapshot),
    ("__fixtures__", Generated::Snapshot),
];

/// Path suffixes, matched case-insensitively against the last component.
///
/// Ordered longest-convention-first so `.min.js` wins over a bare `.js` rule and
/// `.pb.go` over `.go`; the resolver takes the first match.
const SUFFIXES: &[(&str, Generated)] = &[
    (".min.js", Generated::Minified),
    (".min.css", Generated::Minified),
    (".min.mjs", Generated::Minified),
    (".bundle.js", Generated::Minified),
    (".bundle.css", Generated::Minified),
    (".js.map", Generated::Minified),
    (".css.map", Generated::Minified),
    (".pb.go", Generated::BuildOutput),
    (".pb.cc", Generated::BuildOutput),
    (".pb.h", Generated::BuildOutput),
    ("_pb2.py", Generated::BuildOutput),
    ("_pb2_grpc.py", Generated::BuildOutput),
    (".pb.dart", Generated::BuildOutput),
    ("_generated.go", Generated::BuildOutput),
    (".generated.ts", Generated::BuildOutput),
    (".g.dart", Generated::BuildOutput),
    (".freezed.dart", Generated::BuildOutput),
    (".designer.cs", Generated::BuildOutput),
    (".snap", Generated::Snapshot),
    (".ambr", Generated::Snapshot),
    (".lock", Generated::Lockfile),
];

/// Banners that mark a file as generated in its own first bytes.
///
/// `@generated` is the linguist/Phabricator convention; the "DO NOT EDIT"
/// phrasings are what Go's, Protobuf's, and most codegen tools actually emit.
/// Matched case-insensitively.
const BANNERS: &[&str] = &[
    "@generated",
    "do not edit",
    "code generated by",
    "auto-generated",
    "autogenerated",
    "automatically generated",
    "generated by the protocol buffer compiler",
];

/// How much of a file's head [`generated_for_content`] inspects.
///
/// A banner is a header convention: tools emit it in the first line or two. Any
/// further and a passing mention of "do not edit" in prose starts matching.
pub const BANNER_GUARD: usize = 1024;

/// Whether `path` is machine-maintained, judged from the path alone.
///
/// Matches a well-known filename first, then any directory segment, then a path
/// suffix — first match wins. Returns `None` for ordinary hand-written files.
///
/// This is a heuristic over naming conventions, not a guarantee: it answers "may
/// a reader safely skip this by default", so a caller should always leave the
/// content reachable.
#[must_use]
pub fn generated_for_path(path: &Path) -> Option<Generated> {
    if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
        for (candidate, kind) in FILENAMES {
            if candidate.eq_ignore_ascii_case(name) {
                return Some(*kind);
            }
        }
    }
    for segment in path.iter().filter_map(|segment| segment.to_str()) {
        for (candidate, kind) in DIRECTORIES {
            if *candidate == segment {
                return Some(*kind);
            }
        }
    }
    if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
        let lowered = name.to_ascii_lowercase();
        for (candidate, kind) in SUFFIXES {
            // A suffix rule describes a decorated name, never the whole name:
            // `.lock` must not swallow a file literally called `.lock`.
            if lowered.len() > candidate.len() && lowered.ends_with(candidate) {
                return Some(*kind);
            }
        }
    }
    None
}

/// Whether `path` is machine-maintained, falling back to a banner sniff of
/// `head` when the path alone is inconclusive.
///
/// `head` is the file's leading text; only its first [`BANNER_GUARD`] bytes are
/// inspected, and only up to the first blank line, so a banner has to sit in the
/// header block where codegen tools put it.
#[must_use]
pub fn generated_for_content(path: &Path, head: &str) -> Option<Generated> {
    generated_for_path(path).or_else(|| has_banner(head).then_some(Generated::Marker))
}

/// Whether the header block of `head` carries a generated banner.
fn has_banner(head: &str) -> bool {
    // Walk the guard down to a char boundary rather than giving up and scanning
    // the whole string, which would defeat the guard on any non-ASCII header.
    let mut guard = head.len().min(BANNER_GUARD);
    while guard > 0 && !head.is_char_boundary(guard) {
        guard -= 1;
    }
    let window = head.get(..guard).unwrap_or_default();
    let header = match window.split_once("\n\n") {
        Some((header, _)) => header,
        None => window,
    };
    let lowered = header.to_ascii_lowercase();
    BANNERS.iter().any(|banner| lowered.contains(banner))
}

#[cfg(test)]
#[path = "generated/tests.rs"]
mod tests;
