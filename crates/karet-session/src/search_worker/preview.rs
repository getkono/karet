//! Turning a raw [`karet_search::Match`] into the line a results list renders.
//!
//! Every step here re-bases the match's offsets onto the string it just produced,
//! because the panel highlights the match by *slicing* the preview. Getting a cut
//! point wrong is not a cosmetic bug: a byte index that lands mid-character panics
//! on slicing, and the clippy no-panic policy cannot catch an index expression.
//! Hence [`char_floor`] at every cut, and `get(..)` rather than `[..]` throughout.

use karet_core::LineCol;
use karet_core::Range;
use karet_search::Match;

use crate::api::SearchMatch;

/// How many bytes of a matched line a preview may carry.
///
/// A minified bundle is one line of megabytes; without this cap a single file
/// could hold the whole results list's memory.
const PREVIEW_MAX_BYTES: usize = 240;
/// How many bytes of context to keep before the match when windowing a long line.
const PREVIEW_CONTEXT_BYTES: usize = 48;
/// The marker shown where windowing cut a line. Three bytes in UTF-8 — the
/// offsets shift by exactly this much when one is prepended.
const ELLIPSIS: &str = "…";

/// The largest byte index `<= index` that lands on a character boundary.
///
/// `str::floor_char_boundary` is still unstable, so this is the hand-rolled
/// equivalent. Used at *every* cut point.
fn char_floor(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// The line `m` starts on, without its terminator.
///
/// [`Match::col`] is a **byte** column, so the line start is `m.start - m.col`.
fn line_of<'a>(text: &'a str, m: &Match) -> &'a str {
    let start = m.start.saturating_sub(m.col as usize);
    let rest = text.get(start..).unwrap_or_default();
    let end = rest.find('\n').unwrap_or(rest.len());
    rest.get(..end).unwrap_or_default().trim_end_matches('\r')
}

/// Build the display form of one match: its character-coordinate range plus the
/// trimmed, windowed line with the match's byte offsets re-based onto it.
pub(crate) fn search_match(text: &str, m: &Match) -> SearchMatch {
    let line = line_of(text, m);
    let col = (m.col as usize).min(line.len());
    let match_len = m.end.saturating_sub(m.start);

    // Navigation coordinates: characters, not bytes.
    let start_char = line.get(..col).unwrap_or_default().chars().count();
    let start_line = m.line;
    // A regex may span lines; the end line is the start plus the newlines inside
    // the match, and the end column only means anything on a single-line match.
    let spanned = text.get(m.start..m.end).unwrap_or_default();
    let newlines = spanned.matches('\n').count();
    let end_line = start_line.saturating_add(u32::try_from(newlines).unwrap_or(0));
    let end_char = if newlines == 0 {
        let end_col = char_floor(line, col.saturating_add(match_len));
        line.get(..end_col).unwrap_or_default().chars().count()
    } else {
        start_char
    };
    let range = Range {
        start: LineCol::new(start_line, u32::try_from(start_char).unwrap_or(u32::MAX)),
        end: LineCol::new(end_line, u32::try_from(end_char).unwrap_or(u32::MAX)),
    };

    // Trim indentation so previews read flush-left in a narrow sidebar, re-basing
    // the offsets. A query that matches whitespace can start inside the trimmed
    // run, so the subtraction is saturating and the end is clamped below.
    let trimmed_start = line.len() - line.trim_start().len();
    let preview = line.trim();
    let mut start = col.saturating_sub(trimmed_start).min(preview.len());
    let mut end = col
        .saturating_add(match_len)
        .saturating_sub(trimmed_start)
        .clamp(start, preview.len());

    // Window a long line around the match rather than from its start, so a hit
    // deep inside a minified line is still the thing you see.
    let (mut preview, cut_head) = if preview.len() > PREVIEW_MAX_BYTES {
        let head = char_floor(preview, start.saturating_sub(PREVIEW_CONTEXT_BYTES));
        let tail = char_floor(preview, head.saturating_add(PREVIEW_MAX_BYTES));
        let window = preview.get(head..tail).unwrap_or_default();
        start = start.saturating_sub(head).min(window.len());
        end = end.saturating_sub(head).clamp(start, window.len());
        let cut_tail = tail < preview.len();
        let mut owned = window.to_owned();
        if cut_tail {
            owned.push_str(ELLIPSIS);
        }
        (owned, head > 0)
    } else {
        (preview.to_owned(), false)
    };
    if cut_head {
        // The ellipsis is part of the string the panel slices, so both offsets
        // shift by its byte length.
        preview.insert_str(0, ELLIPSIS);
        start = start.saturating_add(ELLIPSIS.len());
        end = end.saturating_add(ELLIPSIS.len());
    }

    SearchMatch {
        range,
        preview_start: u32::try_from(start).unwrap_or(u32::MAX),
        preview_end: u32::try_from(end).unwrap_or(u32::MAX),
        line_text: preview,
    }
}

#[cfg(test)]
mod tests {
    use karet_search::SearchQuery;

    use super::*;

    /// Find `needle` in `text` the way the worker does, then build its preview.
    fn preview_of(text: &str, needle: &str) -> SearchMatch {
        let query = SearchQuery {
            pattern: needle.to_owned(),
            case_sensitive: true,
            ..Default::default()
        };
        let matches = karet_search::search_in_file(text, &query).unwrap_or_default();
        let first = matches.first().copied().unwrap_or(Match {
            start: 0,
            end: 0,
            line: 0,
            col: 0,
        });
        search_match(text, &first)
    }

    /// The slice the panel will paint as the highlight. If the offsets are wrong,
    /// this is what goes wrong on screen.
    fn highlighted(m: &SearchMatch) -> &str {
        m.line_text
            .get(m.preview_start as usize..m.preview_end as usize)
            .unwrap_or("<INVALID>")
    }

    #[test]
    fn an_indented_line_is_trimmed_and_the_offsets_follow_it() {
        let m = preview_of("\t\t    let needle = 1;\n", "needle");
        assert_eq!(m.line_text, "let needle = 1;");
        assert_eq!(highlighted(&m), "needle");
    }

    #[test]
    fn a_crlf_line_keeps_no_stray_carriage_return() {
        let m = preview_of("first\r\nlet needle = 1;\r\nlast\r\n", "needle");
        assert_eq!(m.line_text, "let needle = 1;");
        assert_eq!(highlighted(&m), "needle");
    }

    /// The offsets index bytes, so a multi-byte prefix must not shift the
    /// highlight — while the navigation column must count characters, not bytes.
    #[test]
    fn a_multibyte_prefix_shifts_bytes_but_not_the_character_column() {
        let m = preview_of("let héllo = needle;\n", "needle");
        assert_eq!(highlighted(&m), "needle");
        // "let héllo = " is 12 characters but 13 bytes.
        assert_eq!(m.range.start.col, 12);
        assert_eq!(m.preview_start, 13);
    }

    #[test]
    fn a_long_line_windows_around_the_match_with_both_ellipses() {
        let line = format!("{}needle{}", "a".repeat(4000), "b".repeat(4000));
        let m = preview_of(&line, "needle");
        assert_eq!(highlighted(&m), "needle");
        assert!(m.line_text.starts_with('…'), "{:?}", m.line_text);
        assert!(m.line_text.ends_with('…'), "{:?}", m.line_text);
        assert!(m.line_text.len() <= PREVIEW_MAX_BYTES + 2 * ELLIPSIS.len());
    }

    /// The prepended ellipsis is three bytes and lives inside the string the panel
    /// slices — the likeliest off-by-N in the whole feature.
    #[test]
    fn the_leading_ellipsis_is_counted_in_the_offsets() {
        let line = format!("{}needle", "a".repeat(4000));
        let m = preview_of(&line, "needle");
        assert!(m.line_text.starts_with('…'));
        assert!(m.preview_start >= ELLIPSIS.len() as u32);
        assert_eq!(highlighted(&m), "needle");
    }

    /// A multi-byte character straddling a window cut must not split — a raw byte
    /// slice here panics in release.
    #[test]
    fn windowing_never_splits_a_multibyte_character() {
        for pad in 0..8 {
            let line = format!(
                "{}{}needle{}",
                "é".repeat(400),
                "x".repeat(pad),
                "é".repeat(400)
            );
            let m = preview_of(&line, "needle");
            assert_eq!(highlighted(&m), "needle", "pad {pad}");
            // Constructing the String at all proves every cut was on a boundary.
            assert!(m.line_text.contains("needle"), "pad {pad}");
        }
    }

    #[test]
    fn a_match_at_the_very_start_of_a_file_previews() {
        let m = preview_of("needle at the start\n", "needle");
        assert_eq!(m.line_text, "needle at the start");
        assert_eq!((m.preview_start, m.range.start.col), (0, 0));
        assert_eq!(highlighted(&m), "needle");
    }

    /// A whitespace query starts inside the run that trimming removes; the offsets
    /// floor at zero rather than wrapping or panicking.
    #[test]
    fn a_match_inside_the_trimmed_indentation_collapses_to_the_start() {
        let m = preview_of("\t\tlet x = 1;\n", "\t");
        assert_eq!(m.line_text, "let x = 1;");
        assert_eq!(m.preview_start, 0);
        assert!(m.preview_end >= m.preview_start);
    }

    #[test]
    fn a_match_on_the_last_line_without_a_trailing_newline_previews() {
        let m = preview_of("first\nlet needle = 1;", "needle");
        assert_eq!(m.line_text, "let needle = 1;");
        assert_eq!((m.range.start.line, highlighted(&m)), (1, "needle"));
    }

    /// A regex spanning lines reports the later end line, and the preview shows
    /// the start line with the highlight clamped to it.
    #[test]
    fn a_multiline_match_clamps_its_preview_to_the_start_line() {
        let query = SearchQuery {
            pattern: "a(?s).*b".to_owned(),
            regex: true,
            case_sensitive: true,
            ..Default::default()
        };
        let text = "xx a1\n22\n3b yy\n";
        let matches = karet_search::search_in_file(text, &query).unwrap_or_default();
        let first = matches.first().copied().unwrap_or(Match {
            start: 0,
            end: 0,
            line: 0,
            col: 0,
        });
        let m = search_match(text, &first);
        assert_eq!(m.line_text, "xx a1");
        assert_eq!((m.range.start.line, m.range.end.line), (0, 2));
        assert!(m.preview_end as usize <= m.line_text.len());
        assert_ne!(highlighted(&m), "<INVALID>");
    }

    #[test]
    fn a_zero_width_match_highlights_nothing_and_stays_in_range() {
        let query = SearchQuery {
            pattern: "^".to_owned(),
            regex: true,
            ..Default::default()
        };
        let matches = karet_search::search_in_file("let x = 1;\n", &query).unwrap_or_default();
        let first = matches.first().copied().unwrap_or(Match {
            start: 0,
            end: 0,
            line: 0,
            col: 0,
        });
        let m = search_match("let x = 1;\n", &first);
        assert_eq!(m.preview_start, m.preview_end);
        assert_eq!(highlighted(&m), "");
    }

    /// A match longer than the window runs to the preview's end rather than past it.
    #[test]
    fn a_match_longer_than_the_window_clamps_to_the_preview() {
        let query = SearchQuery {
            pattern: "a+".to_owned(),
            regex: true,
            ..Default::default()
        };
        let text = "a".repeat(5000);
        let matches = karet_search::search_in_file(&text, &query).unwrap_or_default();
        let first = matches.first().copied().unwrap_or(Match {
            start: 0,
            end: 0,
            line: 0,
            col: 0,
        });
        let m = search_match(&text, &first);
        assert!(m.preview_end as usize <= m.line_text.len());
        assert_ne!(highlighted(&m), "<INVALID>");
    }
}
