//! `karet-search` — code search & replace for the karet toolkit.
//!
//! A ripgrep-style engine usable standalone (depends on no other karet crate):
//! incremental in-file search plus a gitignore-aware parallel workspace walk with
//! streamed results and replace planning.
//!
//! # Responsibilities (to implement)
//! - `infile` — incremental search within a single buffer/string (regex/literal/word).
//! - `workspace` — parallel ignore-aware walk + grep, with streamed results.
//! - `replace` — replace single/all/in-selection and replace-in-files planning.
//! - `query` — case-sensitivity / whole-word / regex toggles + glob include/exclude.

// TODO: infile    — incremental in-file search.
// TODO: workspace — parallel find-in-files via ignore + grep, streamed.
// TODO: replace   — replace planning (single/all/selection/in-files).
// TODO: query     — search options & glob filters.
