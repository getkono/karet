//! Workspace-search result models: what the search worker streams and the Search
//! panel renders.
//!
//! Deliberately *not* [`karet_search::FileHit`]. A results list needs the matched
//! *line* as text, and the engine stays lean: its
//! [`Match`](karet_search::Match) is `Copy` and carries byte offsets only, and it
//! is returned by the per-keystroke in-file find path where a `String` per match
//! would allocate on every key. Trimming and windowing a line for display is
//! presentation policy, so it lives on this side of the seam — the same split
//! [`SpellingHit`](super::SpellingHit) already makes.

use std::path::PathBuf;

/// One file's workspace-search matches, streamed by [`Command::Search`](super::Command::Search).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SearchHit {
    /// The absolute path of the matching file.
    pub path: PathBuf,
    /// The file's matches, in file order.
    pub matches: Vec<SearchMatch>,
}

/// One match, with the source line a results list renders around it.
///
/// Two coordinate systems, each with one job — mixing them is the byte-versus-character
/// bug this type exists to prevent:
///
/// - [`range`](Self::range) is **character**-based, for *navigation*: it feeds the
///   editor exactly like every other karet position.
/// - [`preview_start`](Self::preview_start) / [`preview_end`](Self::preview_end) are
///   **byte** offsets, for *slicing* [`line_text`](Self::line_text) into highlight
///   spans in one step.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SearchMatch {
    /// The match's span in the file's 0-based line/character coordinates.
    ///
    /// `karet_search::Match::col` is a *byte* column; the conversion to characters
    /// happens once, here on the backend, so every consumer navigates correctly.
    pub range: karet_core::Range,
    /// The match's line, trimmed of surrounding whitespace and windowed to a
    /// display cap, with `…` marking either cut end.
    pub line_text: String,
    /// Byte offset of the match start within [`line_text`](Self::line_text).
    pub preview_start: u32,
    /// Byte offset one past the match end within [`line_text`](Self::line_text),
    /// clamped to the preview. Equals `preview_start` for a zero-width match, or
    /// when windowing cut the match away entirely.
    pub preview_end: u32,
}
