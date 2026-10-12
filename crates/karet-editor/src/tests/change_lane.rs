//! The gutter's change lane: a column of its own for
//! [`DecorationKind::ChangeMarker`], reserved by [`Editor::change_lane`], so a
//! change marker and a breakpoint on the same line are both drawn.

use crate::*;

/// A one-line decoration of `kind` on `line`, colored by `role`.
fn on_line(line: u32, kind: DecorationKind, role: Option<ThemeRole>) -> Decoration {
    Decoration {
        range: Range {
            start: LineCol::new(line, 0),
            end: LineCol::new(line, 0),
        },
        kind,
        role,
    }
}

fn breakpoint(line: u32) -> Decoration {
    on_line(
        line,
        DecorationKind::GutterMarker { glyph: '●' },
        Some(ThemeRole::DiagnosticError),
    )
}

fn changed(line: u32) -> Decoration {
    on_line(
        line,
        DecorationKind::ChangeMarker { glyph: '▎' },
        Some(ThemeRole::GutterAdded),
    )
}

/// Render `buffer` with `decorations` into a 16x`rows` area, the change lane
/// reserved or not, and return each row's text with the target buffer.
fn render(
    buffer: &TextBuffer,
    decorations: &[Decoration],
    change_lane: bool,
    numbers: LineNumbers,
    state: &mut EditorState,
) -> (Vec<String>, Buffer) {
    let rows = u16::try_from(buffer.line_count()).unwrap_or(1);
    let area = Rect::new(0, 0, 16, rows);
    let mut target = Buffer::empty(area);
    Editor::new(buffer)
        .theme(&Theme::dark())
        .decorations(decorations)
        .line_numbers(numbers)
        .change_lane(change_lane)
        .render(area, &mut target, state);
    let text = (0..rows)
        .map(|y| {
            (0..area.width)
                .map(|x| target[(x, y)].symbol().chars().next().unwrap_or(' '))
                .collect()
        })
        .collect();
    (text, target)
}

#[test]
fn a_change_marker_and_a_breakpoint_on_one_line_are_both_drawn() {
    let buffer = TextBuffer::from_text("alpha\nbravo");
    let decorations = [changed(0), breakpoint(0), changed(1)];
    let mut state = EditorState::new();
    let (rows, target) = render(
        &buffer,
        &decorations,
        true,
        LineNumbers::Absolute,
        &mut state,
    );
    assert_eq!(rows, ["●1▎ alpha       ", " 2▎ bravo       "]);
    let theme = Theme::dark();
    assert_eq!(
        target[(2, 0)].fg,
        theme.role(ThemeRole::GutterAdded).to_ratatui(),
        "the change glyph keeps its role's color"
    );
    assert_eq!(
        target[(0, 0)].fg,
        theme.role(ThemeRole::DiagnosticError).to_ratatui()
    );
    // The text starts after the wider gutter, and a click on it maps there.
    let area = Rect::new(0, 0, 16, 2);
    assert_eq!(state.pos_at(area, &buffer, &[], 4, 1), LineCol::new(1, 0));
    assert_eq!(state.primary_caret_cell(area, &buffer, &[]), Some((4, 0)));
}

#[test]
fn without_the_lane_a_change_marker_yields_the_marker_column() {
    let buffer = TextBuffer::from_text("alpha\nbravo");
    // The change marker comes first, yet the breakpoint wins its line.
    let decorations = [changed(0), breakpoint(0), changed(1)];
    let (rows, _) = render(
        &buffer,
        &decorations,
        false,
        LineNumbers::Absolute,
        &mut EditorState::new(),
    );
    assert_eq!(rows, ["●1 alpha        ", "▎2 bravo        "]);
}

#[test]
fn the_change_lane_sits_between_the_numbers_and_the_fold_lane() {
    use GutterLane::*;
    let buffer = TextBuffer::from_text(&"x\n".repeat(9));
    let lanes = |state: &EditorState| -> Vec<Option<GutterLane>> {
        let area = Rect::new(0, 0, 16, 10);
        (0..6)
            .map(|col| {
                state
                    .gutter_hit_at(area, &buffer, &[], col, 0)
                    .map(|hit| hit.lane)
            })
            .collect()
    };
    let mut state = EditorState::new();
    render(&buffer, &[], true, LineNumbers::Absolute, &mut state);
    assert_eq!(
        lanes(&state),
        [
            Some(Marker),
            Some(LineNumber),
            Some(LineNumber),
            Some(Change),
            Some(Fold),
            None
        ]
    );
    // Numbers off: marker, change and fold lanes side by side.
    render(&buffer, &[], true, LineNumbers::Off, &mut state);
    assert_eq!(
        lanes(&state),
        [Some(Marker), Some(Change), Some(Fold), None, None, None]
    );
    // Without the lane the gutter is as narrow as before and has no change lane.
    render(&buffer, &[], false, LineNumbers::Off, &mut state);
    assert_eq!(
        lanes(&state),
        [Some(Marker), Some(Fold), None, None, None, None]
    );
}

#[test]
fn a_wrap_continuation_row_shows_no_change_marker() {
    let buffer = TextBuffer::from_text("alpha bravo charlie");
    let area = Rect::new(0, 0, 12, 3);
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .decorations(&[changed(0)])
        .change_lane(true)
        .word_wrap(true)
        .render(area, &mut target, &mut EditorState::new());
    assert_eq!(target[(2, 0)].symbol(), "▎");
    assert_eq!(target[(2, 1)].symbol(), " ");
}
