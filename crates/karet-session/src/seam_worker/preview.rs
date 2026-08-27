//! Reading the source behind a seam node.
//!
//! The index keeps paths, not text — it drops each file's contents the moment it has
//! walked them — so a preview is a fresh read. That read belongs here, on the worker
//! thread that already owns the index and is already allowed to block, rather than on the
//! actor thread every other document is waiting on.
//!
//! One file is kept in memory between requests. One, not a map: arrowing through a column
//! walks siblings, which share a file by construction, so the access pattern is a walk
//! rather than a scatter and a second slot would buy an eviction rule and nothing else.

use std::path::Path;
use std::path::PathBuf;

use karet_core::Range;
use karet_diff::TokenSpan;
use karet_treesitter::language_id_from_path;

use crate::api::SeamPreview;

/// Lines of context fetched on each side of a node's own extent.
pub(crate) const CONTEXT: u32 = 3;

/// The most node lines one preview carries.
///
/// A four-thousand-line generated `impl` is not a preview, and no pane shows more than a
/// handful of rows in any case.
const MAX_BODY_LINES: u32 = 200;

/// Above this a file is read but not highlighted.
///
/// Parsing a megabyte to colour nine rows is not a trade worth making at key-repeat rate,
/// and unstyled text is a far smaller loss than a stalled view.
const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;

/// Above this a file is not previewed at all, and the pane says so.
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// The one file kept in memory so walking a column re-reads nothing.
pub(crate) struct PreviewSource {
    /// The file the text belongs to.
    path: PathBuf,
    /// Its lines, newline stripped, indexed the way `Range::start.line` counts.
    lines: Vec<String>,
    /// Per-line syntax token runs, resolved on first use.
    ///
    /// Lazy because re-indexing a saved file hands us its text whether or not anyone is
    /// previewing it, and highlighting text nobody will look at is pure cost.
    tokens: Option<Vec<Vec<TokenSpan>>>,
}

impl PreviewSource {
    /// Hold `text` as the current contents of `path`.
    pub(crate) fn adopt(path: &Path, text: &str) -> Self {
        Self {
            path: path.to_path_buf(),
            lines: split_lines(text),
            tokens: None,
        }
    }

    /// The token runs for this file, highlighting it on first use.
    fn tokens(&mut self) -> &[Vec<TokenSpan>] {
        if self.tokens.is_none() {
            let bytes: usize = self.lines.iter().map(|line| line.len() + 1).sum();
            let table = if bytes > MAX_HIGHLIGHT_BYTES {
                Vec::new()
            } else {
                let text = self.lines.join("\n");
                crate::diff_prepare::line_tokens(&text, language_id_from_path(&self.path))
            };
            self.tokens = Some(table);
        }
        self.tokens.as_deref().unwrap_or_default()
    }
}

/// Why a file could not be previewed, phrased for the reader.
///
/// One place, so "the file is gone" and "the file is enormous" read as the same kind of
/// sentence and a test can pin them.
enum Unavailable<'a> {
    /// The read itself failed.
    Unreadable(&'a str),
    /// The bytes are not UTF-8, so there are no lines to show.
    NotText,
    /// The file is past [`MAX_FILE_BYTES`].
    TooLarge(u64),
    /// The node's range points past the end of the file it names.
    Stale,
    /// Nothing has been indexed, so there is no node to be behind.
    NoIndex,
}

impl Unavailable<'_> {
    fn describe(&self) -> String {
        match self {
            Self::Unreadable(error) => format!("the file could not be read: {error}"),
            Self::NotText => "this file is not text".to_owned(),
            Self::TooLarge(bytes) => {
                format!(
                    "this file is {} MB — too large to preview",
                    bytes / (1 << 20)
                )
            },
            Self::Stale => "these lines are no longer in the file — save to re-index".to_owned(),
            Self::NoIndex => "nothing is indexed yet".to_owned(),
        }
    }
}

/// The message for a node whose index does not exist.
#[must_use]
pub(crate) fn no_index() -> String {
    Unavailable::NoIndex.describe()
}

/// Build the preview for `range` of `path`, serving `cache` when it already holds the
/// file and refilling it when it does not.
///
/// `header` is the node's declaration head, which is never elided; `range` is its whole
/// extent, which may be.
pub(crate) fn preview_for(
    cache: &mut Option<PreviewSource>,
    path: &Path,
    range: Range,
    header: Range,
) -> Result<SeamPreview, String> {
    if cache.as_ref().is_none_or(|held| held.path != path) {
        *cache = Some(read(path)?);
    }
    let Some(source) = cache.as_mut() else {
        return Err(Unavailable::Stale.describe());
    };
    build(source, range, header)
}

/// Read `path` into a fresh cache entry.
fn read(path: &Path) -> Result<PreviewSource, String> {
    let size = std::fs::metadata(path)
        .map_err(|error| Unavailable::Unreadable(&error.to_string()).describe())?
        .len();
    if size > MAX_FILE_BYTES {
        return Err(Unavailable::TooLarge(size).describe());
    }
    let bytes = std::fs::read(path)
        .map_err(|error| Unavailable::Unreadable(&error.to_string()).describe())?;
    let text = String::from_utf8(bytes).map_err(|_| Unavailable::NotText.describe())?;
    Ok(PreviewSource::adopt(path, &text))
}

/// Cut the window `range` names, with [`CONTEXT`] lines on each side of it.
///
/// The two sides are cut independently, and the trailing side is measured from the node's
/// *true* end rather than from wherever [`MAX_BODY_LINES`] stopped the fetch. A four
/// hundred line module clamped to two hundred would otherwise report lines 201–203 as the
/// code that follows it, when they are the code inside it — the one thing surrounding
/// context must never be confused with. The gap that leaves is carried in the row numbers.
fn build(source: &mut PreviewSource, range: Range, header: Range) -> Result<SeamPreview, String> {
    let total = u32::try_from(source.lines.len()).unwrap_or(u32::MAX);
    if range.start.line >= total {
        // The index describes text that has since been edited away. Saying so beats
        // showing whatever now happens to sit at that line number.
        return Err(Unavailable::Stale.describe());
    }
    let last = total.saturating_sub(1);
    let body_first = range.start.line;
    let body_last = range.end.line.max(body_first).min(last);
    let dropped = (body_last - body_first + 1).saturating_sub(MAX_BODY_LINES);
    let kept_last = body_last - dropped;
    // The head is clamped into what was kept: a declaration longer than the whole fetch
    // cap is not a declaration anyone reads, and an out-of-range head index would be a
    // trap for every renderer downstream.
    let head_last = header.end.line.max(body_first).min(kept_last);

    let lead = body_first.saturating_sub(CONTEXT)..body_first;
    let body = body_first..=kept_last;
    let tail = (body_last + 1).min(total)..(body_last.saturating_add(CONTEXT) + 1).min(total);

    let rows: Vec<u32> = lead.clone().chain(body).chain(tail).collect();
    let take = |line: u32| usize::try_from(line).unwrap_or(usize::MAX);
    let lines: Vec<String> = rows
        .iter()
        .map(|line| source.lines.get(take(*line)).cloned().unwrap_or_default())
        .collect();
    let tokens: Vec<Vec<TokenSpan>> = {
        let table = source.tokens();
        rows.iter()
            .map(|line| table.get(take(*line)).cloned().unwrap_or_default())
            .collect()
    };

    let body_start = take(body_first - lead.start);
    Ok(SeamPreview {
        file: source.path.clone(),
        lines,
        numbers: rows,
        body_start,
        head_end: body_start + take(head_last - body_first) + 1,
        body_end: body_start + take(kept_last - body_first) + 1,
        dropped,
        context: CONTEXT,
        tokens,
    })
}

/// Split `text` into newline-stripped lines, tolerating CRLF.
///
/// A stray carriage return would otherwise paint as a control glyph in the middle of the
/// pane, which reads as corruption rather than as a line ending.
fn split_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use karet_core::LineCol;

    use super::*;

    fn range(start: u32, end: u32) -> Range {
        Range {
            start: LineCol::new(start, 0),
            end: LineCol::new(end, 0),
        }
    }

    /// Build a preview whose head is its first line, the shape of most declarations.
    fn preview_of(
        cache: &mut Option<PreviewSource>,
        path: &Path,
        start: u32,
        end: u32,
    ) -> Result<SeamPreview, String> {
        preview_for(cache, path, range(start, end), range(start, start))
    }

    /// A scratch file whose lines are their own 0-based numbers.
    fn numbered(dir: &std::path::Path, name: &str, count: u32) -> PathBuf {
        let path = dir.join(name);
        let text: String = (0..count)
            .map(|n| format!("line {n}\n"))
            .collect::<Vec<_>>()
            .join("");
        let _ = std::fs::write(&path, text);
        path
    }

    #[test]
    fn a_preview_carries_three_lines_of_context_on_each_side() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = numbered(dir.path(), "a.txt", 40);
        let mut cache = None;
        let built = preview_of(&mut cache, &path, 20, 21);
        assert!(built.is_ok(), "{built:?}");
        let Ok(preview) = built else {
            return;
        };
        assert_eq!(preview.line_number(0), 17);
        assert_eq!(preview.body_start, 3);
        assert_eq!(preview.body_end, 5);
        assert_eq!(preview.lines.len(), 8);
        assert_eq!(preview.context, CONTEXT);
        assert_eq!(preview.lines[preview.body_start], "line 20");
        // Contiguous: nothing was cut, so no row follows a gap.
        assert!(!(0..preview.lines.len()).any(|row| preview.is_after_gap(row)));
    }

    #[test]
    fn the_head_is_reported_wherever_the_declaration_ends() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = numbered(dir.path(), "a.txt", 40);
        let mut cache = None;
        // A node spanning lines 20..30 whose signature runs to line 23.
        let built = preview_for(&mut cache, &path, range(20, 30), range(20, 23));
        assert!(built.is_ok(), "{built:?}");
        let Ok(preview) = built else {
            return;
        };
        assert_eq!(preview.body_start, 3);
        assert_eq!(preview.head_end, 7);
        assert_eq!(preview.lines[preview.head_end - 1], "line 23");
        assert!(preview.is_head(preview.head_end - 1));
        assert!(!preview.is_head(preview.head_end));
    }

    #[test]
    fn a_head_that_outruns_the_node_is_clamped_into_it() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = numbered(dir.path(), "a.txt", 40);
        let mut cache = None;
        // A header range that reaches past the node cannot be trusted; clamping it keeps
        // every downstream index inside the window.
        let built = preview_for(&mut cache, &path, range(20, 21), range(20, 99));
        assert!(built.is_ok(), "{built:?}");
        let Ok(preview) = built else {
            return;
        };
        assert_eq!(preview.head_end, preview.body_end);
    }

    #[test]
    fn a_node_at_the_top_of_a_file_reports_the_context_it_could_not_fetch() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = numbered(dir.path(), "a.txt", 40);
        let mut cache = None;
        let built = preview_of(&mut cache, &path, 1, 1);
        assert!(built.is_ok(), "{built:?}");
        let Ok(preview) = built else {
            return;
        };
        // Reported, never padded: only the view can tell a missing line from a blank one.
        assert_eq!(preview.line_number(0), 0);
        assert_eq!(preview.body_start, 1);
    }

    #[test]
    fn a_node_at_the_end_of_a_file_stops_at_the_last_line() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = numbered(dir.path(), "a.txt", 10);
        let mut cache = None;
        let built = preview_of(&mut cache, &path, 9, 9);
        assert!(built.is_ok(), "{built:?}");
        let Ok(preview) = built else {
            return;
        };
        assert_eq!(preview.lines.last().map(String::as_str), Some("line 9"));
        assert_eq!(preview.body_end, preview.lines.len());
    }

    #[test]
    fn a_node_longer_than_the_cap_says_how_many_lines_it_dropped() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = numbered(dir.path(), "a.txt", 1000);
        let mut cache = None;
        let built = preview_of(&mut cache, &path, 10, 509);
        assert!(built.is_ok(), "{built:?}");
        let Ok(preview) = built else {
            return;
        };
        assert_eq!(preview.dropped, 300);
        assert_eq!(preview.body_end - preview.body_start, 200);
    }

    #[test]
    fn a_truncated_node_takes_its_trailing_context_from_after_the_node() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = numbered(dir.path(), "a.txt", 1000);
        let mut cache = None;
        // The node runs 10..=509. Lines 210..212 are *inside* it; the context that
        // follows it is 510..512, and calling the former "context" would be a lie.
        let built = preview_of(&mut cache, &path, 10, 509);
        assert!(built.is_ok(), "{built:?}");
        let Ok(preview) = built else {
            return;
        };
        let tail: Vec<&str> = preview.lines[preview.body_end..]
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(tail, ["line 510", "line 511", "line 512"]);
        assert!(preview.is_after_gap(preview.body_end));
        assert_eq!(preview.line_number(preview.body_end), 510);
    }

    #[test]
    fn a_truncated_node_at_the_end_of_its_file_has_no_trailing_context() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = numbered(dir.path(), "a.txt", 300);
        let mut cache = None;
        let built = preview_of(&mut cache, &path, 10, 299);
        assert!(built.is_ok(), "{built:?}");
        let Ok(preview) = built else {
            return;
        };
        assert_eq!(preview.body_end, preview.lines.len());
    }

    #[test]
    fn a_missing_file_is_a_reason_rather_than_an_empty_preview() {
        let mut cache = None;
        let built = preview_of(&mut cache, Path::new("/nope/gone.rs"), 0, 0);
        assert!(built.is_err(), "{built:?}");
        let Err(message) = built else {
            return;
        };
        assert!(message.contains("could not be read"), "{message}");
    }

    #[test]
    fn a_file_that_is_not_text_is_refused_by_name() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("blob.bin");
        let _ = std::fs::write(&path, [0xff_u8, 0xfe, 0x00]);
        let mut cache = None;
        let built = preview_of(&mut cache, &path, 0, 0);
        assert!(built.is_err(), "{built:?}");
        let Err(message) = built else {
            return;
        };
        assert!(message.contains("not text"), "{message}");
    }

    #[test]
    fn a_range_past_the_end_of_the_file_is_a_reason_rather_than_a_blank_block() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = numbered(dir.path(), "a.txt", 5);
        let mut cache = None;
        let built = preview_of(&mut cache, &path, 99, 99);
        assert!(built.is_err(), "{built:?}");
        let Err(message) = built else {
            return;
        };
        assert!(message.contains("no longer in the file"), "{message}");
    }

    #[test]
    fn the_cache_serves_a_second_node_in_the_same_file_without_re_reading() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = numbered(dir.path(), "a.txt", 40);
        let mut cache = None;
        let _ = preview_of(&mut cache, &path, 10, 10);
        let _ = std::fs::remove_file(&path);
        // Walking a column must not pay a read per row.
        assert!(preview_of(&mut cache, &path, 20, 20).is_ok());
    }

    #[test]
    fn a_second_file_replaces_the_cached_one() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let first = numbered(dir.path(), "a.txt", 40);
        let second = numbered(dir.path(), "b.txt", 40);
        let mut cache = None;
        let _ = preview_of(&mut cache, &first, 10, 10);
        let built = preview_of(&mut cache, &second, 10, 10);
        assert!(built.is_ok(), "{built:?}");
        let Ok(preview) = built else {
            return;
        };
        assert_eq!(preview.file, second);
    }

    #[test]
    fn adopted_text_is_previewed_instead_of_what_is_on_disk() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = numbered(dir.path(), "a.txt", 40);
        // The pane has to quote what the index actually read, not what disk still says.
        let mut cache = Some(PreviewSource::adopt(&path, "one\ntwo\nthree\n"));
        let built = preview_of(&mut cache, &path, 1, 1);
        assert!(built.is_ok(), "{built:?}");
        let Ok(preview) = built else {
            return;
        };
        assert_eq!(preview.lines, ["one", "two", "three"]);
    }

    #[test]
    fn a_file_with_no_grammar_previews_as_plain_text_rather_than_failing() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = numbered(dir.path(), "a.unknownext", 10);
        let mut cache = None;
        let built = preview_of(&mut cache, &path, 4, 4);
        assert!(built.is_ok(), "{built:?}");
        let Ok(preview) = built else {
            return;
        };
        assert!(preview.tokens.iter().all(Vec::is_empty));
    }

    #[test]
    fn every_row_carries_a_token_slot_whether_or_not_it_has_runs() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = numbered(dir.path(), "a.txt", 40);
        let mut cache = None;
        let built = preview_of(&mut cache, &path, 20, 21);
        assert!(built.is_ok(), "{built:?}");
        let Ok(preview) = built else {
            return;
        };
        // The renderer indexes the two in lockstep; a short table would misalign colour.
        assert_eq!(preview.tokens.len(), preview.lines.len());
        assert_eq!(preview.numbers.len(), preview.lines.len());
    }

    #[test]
    fn a_crlf_file_leaves_no_stray_carriage_returns() {
        assert_eq!(split_lines("a\r\nb\r\n"), ["a", "b"]);
    }
}
