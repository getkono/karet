//! Render tests for [`Transcript`](super::Transcript).
//!
//! Split out of `transcript.rs` to keep both files under the workspace's
//! code-line ceiling (the precedent is `karet-lsp/src/lib.rs`).

use karet_core::ThemeRole;
use karet_theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;

use super::Transcript;
use super::TranscriptBody;
use super::TranscriptMessage;
use crate::scroll::PaintedTracks;
use crate::scroll::ScrollAxes;
use crate::scroll::ScrollTrack;
use crate::scroll::reserve_tracks;

/// Paint into a fresh buffer the size of `area`, handing back both the cells and
/// the tracks the widget reserved.
fn paint(transcript: &mut Transcript, area: Rect) -> (Buffer, PaintedTracks) {
    let mut buf = Buffer::empty(area);
    let tracks = transcript.paint(&mut buf, &Theme::dark(), area);
    (buf, tracks)
}

/// The painted content rows of `area` — the reserved scrollbar column excluded,
/// trailing blanks trimmed off each.
fn rows(buf: &Buffer, area: Rect) -> Vec<String> {
    let (content, _) = reserve_tracks(area, ScrollAxes::VERTICAL);
    (content.y..content.bottom())
        .map(|y| {
            let row: String = (content.x..content.right())
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect();
            row.trim_end().to_owned()
        })
        .collect()
}

/// A headerless message, so a row count is purely the body's.
fn body(text: &str) -> TranscriptMessage {
    TranscriptMessage::new("", text)
}

/// Six one-row messages: rows `0, 2, 4, 6, 8, 10` with blank spacers between.
fn six_lines() -> Transcript {
    let mut transcript = Transcript::new();
    for index in 0..6 {
        transcript.push(body(&format!("line {index}")));
    }
    transcript
}

#[test]
fn appending_a_message_never_rewraps_the_earlier_ones() {
    let area = Rect::new(0, 0, 20, 10);
    let mut transcript = Transcript::new();
    transcript.push(TranscriptMessage::new("@a", "hello there friend"));
    transcript.push(TranscriptMessage::new("@b", "second body here"));
    let _ = paint(&mut transcript, area);
    let settled = transcript.wrap_count();
    let first = transcript.message_rows(0);
    let second = transcript.message_rows(1);
    assert_eq!(settled, 2, "one wrap per message on the first pass");

    transcript.push(TranscriptMessage::new("@c", "third"));
    let _ = paint(&mut transcript, area);

    assert_eq!(
        transcript.wrap_count(),
        settled + 1,
        "only the appended message was wrapped"
    );
    assert_eq!(transcript.message_rows(0), first);
    assert_eq!(transcript.message_rows(1), second);
    assert_eq!(transcript.len(), 3);
}

#[test]
fn repainting_at_the_same_width_wraps_nothing_at_all() {
    let area = Rect::new(0, 0, 20, 10);
    let mut transcript = six_lines();
    let _ = paint(&mut transcript, area);
    let settled = transcript.wrap_count();

    let _ = paint(&mut transcript, area);
    let _ = paint(&mut transcript, area);

    assert_eq!(transcript.wrap_count(), settled, "the cache answered");
}

#[test]
fn a_width_change_invalidates_and_rewraps_every_message() {
    let wide = Rect::new(0, 0, 40, 10);
    let narrow = Rect::new(0, 0, 20, 10);
    let mut transcript = Transcript::new();
    for _ in 0..3 {
        transcript.push(body("aaa bbb ccc ddd eee fff ggg hhh"));
    }
    let _ = paint(&mut transcript, wide);
    let wide_rows = transcript.rows();
    assert_eq!(transcript.wrap_count(), 3);

    let _ = paint(&mut transcript, narrow);

    assert_eq!(transcript.wrap_count(), 6, "every message re-wrapped");
    assert!(
        transcript.rows() > wide_rows,
        "31 cells fit one row at 39 and two at 19: {} vs {wide_rows}",
        transcript.rows()
    );
    // And back again: the width is the only key, so the wider wrap is redone too.
    let _ = paint(&mut transcript, wide);
    assert_eq!(transcript.rows(), wide_rows);
}

#[test]
fn invalidating_forces_a_rewrap_at_an_unchanged_width() {
    // The escape hatch for the one input the width key cannot see: the theme.
    let area = Rect::new(0, 0, 20, 10);
    let mut transcript = six_lines();
    let _ = paint(&mut transcript, area);
    let settled = transcript.wrap_count();

    transcript.invalidate();
    let _ = paint(&mut transcript, area);

    assert_eq!(transcript.wrap_count(), settled + 6);
    assert_eq!(transcript.rows(), 11, "the rows themselves are unchanged");
}

#[test]
fn the_view_follows_the_tail_as_messages_arrive() {
    let area = Rect::new(0, 0, 20, 5);
    let mut transcript = six_lines();

    let (buf, _) = paint(&mut transcript, area);

    assert!(transcript.is_following());
    assert_eq!(transcript.rows(), 11);
    assert_eq!(transcript.viewport(), 5);
    assert_eq!(transcript.offset(), transcript.max_offset());
    assert_eq!(transcript.offset(), 6);
    assert_eq!(
        rows(&buf, area),
        vec![
            "line 3".to_owned(),
            String::new(),
            "line 4".to_owned(),
            String::new(),
            "line 5".to_owned(),
        ]
    );
}

#[test]
fn scrolling_up_releases_the_follow_and_the_bottom_re_engages_it() {
    let area = Rect::new(0, 0, 20, 5);
    let mut transcript = six_lines();
    let _ = paint(&mut transcript, area);

    transcript.scroll_by(-3);
    assert_eq!(transcript.offset(), 3);
    assert!(!transcript.is_following(), "the reader took over");

    transcript.scroll_by(3);
    assert_eq!(transcript.offset(), 6);
    assert!(transcript.is_following(), "back at the bottom");

    transcript.scroll_to_top();
    assert_eq!(transcript.offset(), 0);
    assert!(!transcript.is_following());

    transcript.scroll_to_bottom();
    assert!(transcript.is_following());
    assert_eq!(transcript.offset(), transcript.max_offset());
}

#[test]
fn a_view_whose_content_fits_never_leaves_the_follow() {
    // `max_offset` is zero, so "scrolled up" and "at the bottom" are the same
    // place: releasing there would silently stop the next append from showing.
    let area = Rect::new(0, 0, 20, 10);
    let mut transcript = Transcript::new();
    transcript.push(body("short"));
    let _ = paint(&mut transcript, area);

    transcript.scroll_by(-5);
    transcript.scroll_to_top();

    assert!(transcript.is_following());
    assert_eq!(transcript.offset(), 0);
}

#[test]
fn an_append_while_released_does_not_move_the_viewport() {
    let area = Rect::new(0, 0, 20, 5);
    let mut transcript = six_lines();
    let _ = paint(&mut transcript, area);
    transcript.scroll_by(-4);
    let (before, _) = paint(&mut transcript, area);
    assert_eq!(transcript.offset(), 2);

    transcript.push(body("line 6"));
    transcript.push(body("line 7"));
    let (after, _) = paint(&mut transcript, area);

    assert_eq!(transcript.offset(), 2, "the reader's place was kept");
    assert!(!transcript.is_following());
    assert_eq!(rows(&before, area), rows(&after, area));
    assert_eq!(transcript.rows(), 15, "the content grew beneath it");
}

#[test]
fn the_scroll_extent_counts_wrapped_rows_not_source_lines() {
    // The bug in the `ui/github/detail.rs` precedent: it feeds `lines.len()` to
    // `ScrollExtent`, which is the count *before* the soft wrap. Here the extent
    // is the count of rows that were actually painted.
    let area = Rect::new(0, 0, 20, 10);
    let mut transcript = Transcript::new();
    let source = "word word word word word word word word word word word word";
    assert_eq!(source.lines().count(), 1, "one source line");
    transcript.push(body(source));

    let (buf, _) = paint(&mut transcript, area);

    let painted = rows(&buf, area)
        .iter()
        .filter(|row| !row.is_empty())
        .count();
    assert_eq!(painted, 3, "19 content cells fit four words a row");
    assert_eq!(transcript.extent().content, painted);
    assert_eq!(transcript.extent().viewport, 10);
    assert_eq!(transcript.extent().position, 0);
    assert_ne!(transcript.extent().content, source.lines().count());
}

#[test]
fn the_wrap_width_is_derived_after_the_reservation() {
    // 21 cells of area, minus the reserved track, is a 20-cell wrap width. A body
    // of exactly 20 cells must fit one row and 21 must not — if the width had been
    // taken before reserving, both would fit.
    let area = Rect::new(0, 0, 21, 10);
    let (content, _) = reserve_tracks(area, ScrollAxes::VERTICAL);
    assert_eq!(content.width, 20);

    let mut exact = Transcript::new();
    exact.push(body(&"a".repeat(20)));
    let _ = paint(&mut exact, area);
    let mut over = Transcript::new();
    over.push(body(&"a".repeat(21)));
    let _ = paint(&mut over, area);

    assert_eq!(exact.message_rows(0), 1);
    assert_eq!(over.message_rows(0), 2, "the reserved column narrowed it");
}

#[test]
fn adding_content_never_changes_whether_a_column_is_reserved() {
    // The feedback loop the invariant exists to break: content -> rows -> overflow
    // -> reservation -> narrower wrap -> more rows.
    let area = Rect::new(0, 0, 20, 10);
    let expected = reserve_tracks(area, ScrollAxes::VERTICAL).1.vertical;

    let mut transcript = Transcript::new();
    transcript.push(body("one"));
    let (_, empty) = paint(&mut transcript, area);
    let first = transcript.message_rows(0);
    for index in 0..200 {
        transcript.push(body(&format!("filler {index}")));
    }
    let (_, full) = paint(&mut transcript, area);

    assert_eq!(empty.vertical.map(ScrollTrack::rect), expected);
    assert_eq!(full.vertical.map(ScrollTrack::rect), expected);
    assert_eq!(
        transcript.message_rows(0),
        first,
        "the first message wrapped the same either way"
    );
    assert!(transcript.extent().overflows());
}

#[test]
fn amending_the_tail_rewraps_only_the_tail() {
    let area = Rect::new(0, 0, 20, 10);
    let mut transcript = Transcript::new();
    transcript.push(TranscriptMessage::new("@a", "a settled message"));
    transcript.push(TranscriptMessage::new("@b", "streaming"));
    let _ = paint(&mut transcript, area);
    let settled = transcript.wrap_count();
    let first = transcript.message_rows(0);

    assert!(transcript.extend_tail(" reply"));
    let _ = paint(&mut transcript, area);

    assert_eq!(transcript.wrap_count(), settled + 1);
    assert_eq!(transcript.message_rows(0), first);
    assert_eq!(
        transcript.message(1).map(|m| m.body.source()),
        Some("streaming reply")
    );

    assert!(transcript.amend_tail("replaced entirely"));
    let _ = paint(&mut transcript, area);
    assert_eq!(transcript.wrap_count(), settled + 2);
    assert_eq!(
        transcript.message(1).map(|m| m.body.source()),
        Some("replaced entirely")
    );
}

#[test]
fn a_streamed_tail_keeps_the_tail_in_view() {
    let area = Rect::new(0, 0, 20, 4);
    let mut transcript = Transcript::new();
    transcript.push(body("start"));
    for _ in 0..8 {
        assert!(transcript.extend_tail("\nmore"));
        let _ = paint(&mut transcript, area);
    }
    let (buf, _) = paint(&mut transcript, area);

    assert!(transcript.is_following());
    assert_eq!(transcript.rows(), 9);
    assert_eq!(transcript.offset(), 5);
    assert_eq!(rows(&buf, area).last().map(String::as_str), Some("more"));
}

#[test]
fn amending_an_empty_transcript_reports_that_there_is_no_tail() {
    let mut transcript = Transcript::new();

    assert!(!transcript.extend_tail("token"));
    assert!(!transcript.amend_tail("body"));
    assert!(transcript.is_empty());
    assert_eq!(transcript.wrap_count(), 0);
}

#[test]
fn clearing_empties_the_transcript_and_returns_to_the_tail() {
    let area = Rect::new(0, 0, 20, 5);
    let mut transcript = six_lines();
    let _ = paint(&mut transcript, area);
    transcript.scroll_to_top();

    transcript.clear();
    let (buf, _) = paint(&mut transcript, area);

    assert!(transcript.is_empty());
    assert_eq!(transcript.len(), 0);
    assert_eq!(transcript.rows(), 0);
    assert_eq!(transcript.offset(), 0);
    assert!(transcript.is_following());
    assert!(rows(&buf, area).iter().all(String::is_empty));
}

#[test]
fn paging_moves_a_viewport_at_a_time() {
    let area = Rect::new(0, 0, 20, 5);
    let mut transcript = six_lines();
    let _ = paint(&mut transcript, area);

    transcript.page_by(-1);
    assert_eq!(transcript.offset(), 1);
    transcript.page_by(1);
    assert_eq!(transcript.offset(), 6);
    assert!(transcript.is_following());
}

#[test]
fn setting_an_offset_past_the_end_clamps_to_the_tail() {
    let area = Rect::new(0, 0, 20, 5);
    let mut transcript = six_lines();
    let _ = paint(&mut transcript, area);

    transcript.set_offset(usize::MAX);
    assert_eq!(transcript.offset(), transcript.max_offset());
    assert!(transcript.is_following());

    transcript.scroll_by(i32::MIN);
    assert_eq!(transcript.offset(), 0, "a saturating step, not a wrap");
}

#[test]
fn a_header_paints_bold_in_its_accent() {
    let area = Rect::new(0, 0, 20, 4);
    let theme = Theme::dark();
    let mut transcript = Transcript::new();
    transcript.push(TranscriptMessage::new("@agent", "hello").with_accent(ThemeRole::Muted));
    let mut buf = Buffer::empty(area);
    transcript.paint(&mut buf, &theme, area);

    let cell = buf.cell((0, 0)).cloned().unwrap_or_default();
    assert_eq!(cell.symbol(), "@");
    assert!(cell.modifier.contains(Modifier::BOLD));
    assert_eq!(cell.fg, theme.role(ThemeRole::Muted).to_ratatui());
    assert_eq!(rows(&buf, area).first().map(String::as_str), Some("@agent"));
    assert_eq!(rows(&buf, area).get(1).map(String::as_str), Some("hello"));
}

#[test]
fn a_message_exposes_its_body_and_accent() {
    let mut transcript = Transcript::new();
    transcript.push(TranscriptMessage::new("@a", "hi".to_owned()));

    assert_eq!(
        transcript.message(0).map(|message| message.accent),
        Some(ThemeRole::DiagnosticInfo),
        "the default accent"
    );
    assert_eq!(
        transcript.message(0).map(|message| message.body.source()),
        Some("hi")
    );
    assert!(transcript.message(1).is_none());
    assert_eq!(transcript.message_rows(9), 0, "out of range, not painted");

    let mut body = TranscriptBody::from("hi");
    body.push_str(" there");
    assert_eq!(body.source(), "hi there");
    assert!(!body.is_empty());
    assert!(TranscriptBody::from(String::new()).is_empty());
}

#[test]
fn an_empty_body_occupies_no_rows_of_its_own() {
    // A streamed message exists before its first token arrives; it must not open a
    // blank row that closes again the moment text lands.
    let area = Rect::new(0, 0, 20, 6);
    let mut transcript = Transcript::new();
    transcript.push(TranscriptMessage::new("@agent", ""));
    let _ = paint(&mut transcript, area);

    assert_eq!(transcript.message_rows(0), 1, "the header only");
}

#[test]
fn degenerate_areas_paint_nothing_and_never_panic() {
    let theme = Theme::dark();
    let mut transcript = six_lines();
    let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));

    for area in [
        Rect::new(0, 0, 0, 0),
        Rect::new(0, 0, 4, 0),
        Rect::new(0, 0, 0, 2),
    ] {
        let tracks = transcript.paint(&mut buf, &theme, area);
        assert_eq!(
            tracks,
            PaintedTracks::default(),
            "no track can be painted into {area:?}"
        );
        assert_eq!(transcript.rows(), 0, "an unpaintable area wraps nothing");
        assert_eq!(transcript.wrap_count(), 0);
    }
    assert!(
        rows(&buf, Rect::new(0, 0, 4, 2))
            .iter()
            .all(String::is_empty),
        "nothing was painted"
    );

    // A one-cell area is too small to reserve a track, but still wraps and paints.
    let tracks = transcript.paint(&mut buf, &theme, Rect::new(0, 0, 1, 1));
    assert_eq!(tracks, PaintedTracks::default(), "no room for a bar");
    assert!(transcript.rows() > 11, "one cell per row is a lot of rows");
    assert_ne!(
        buf.cell((0, 0)).map(|cell| cell.symbol().to_owned()),
        Some(" ".to_owned()),
        "the tail's last cell is what a one-cell viewport shows"
    );
    assert_eq!(
        Transcript::default().extent(),
        crate::scroll::ScrollExtent::default()
    );
}

#[test]
fn drawing_through_a_frame_reports_the_track_it_reserved() -> Result<(), std::convert::Infallible> {
    let area = Rect::new(0, 0, 20, 6);
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(area.width, area.height))?;
    let theme = Theme::dark();
    let mut transcript = six_lines();
    let mut painted = PaintedTracks::default();

    let _ = terminal.draw(|f| painted = transcript.draw(f, &theme, area));

    assert_eq!(
        painted.vertical.map(ScrollTrack::rect),
        reserve_tracks(area, ScrollAxes::VERTICAL).1.vertical
    );
    assert_eq!(
        painted.vertical.map(ScrollTrack::extent),
        Some(transcript.extent())
    );
    assert_eq!(
        painted.horizontal, None,
        "the transcript never scrolls sideways"
    );
    Ok(())
}

#[cfg(feature = "markdown")]
#[test]
fn a_markdown_body_renders_through_karet_markdown() {
    let area = Rect::new(0, 0, 24, 6);
    let theme = Theme::dark();
    let mut transcript = Transcript::new();
    transcript.push(TranscriptMessage::new(
        "",
        TranscriptBody::Markdown("# Title\n\n- one\n- two".to_owned()),
    ));
    let mut buf = Buffer::empty(area);
    transcript.paint(&mut buf, &theme, area);

    let cell = buf.cell((0, 0)).cloned().unwrap_or_default();
    assert_eq!(cell.symbol(), "#", "the heading marker is kept");
    assert!(cell.modifier.contains(Modifier::BOLD));
    assert_eq!(
        cell.fg,
        theme
            .color(karet_core::StandardToken::MarkupHeading.id())
            .to_ratatui()
    );
    let painted = rows(&buf, area);
    assert!(
        painted.iter().any(|row| row.contains("one")),
        "the list rendered: {painted:?}"
    );
    assert!(transcript.rows() > 1);
    // The markdown body streams and re-wraps like any other.
    assert!(transcript.extend_tail("\n- three"));
    let _ = paint(&mut transcript, area);
    assert_eq!(
        transcript.message(0).map(|message| message.body.source()),
        Some("# Title\n\n- one\n- two\n- three")
    );
}
