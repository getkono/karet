use super::*;

/// The row and column a placeholder cell names.
fn position(symbol: &str) -> Option<(usize, usize)> {
    let mut chars = symbol.chars();
    if chars.next() != Some(PLACEHOLDER) {
        return None;
    }
    let index = |c: Option<char>| c.and_then(|c| DIACRITICS.iter().position(|&d| d == c));
    Some((index(chars.next())?, index(chars.next())?))
}

#[test]
fn placeholder_cells_name_their_image_placement_row_and_column() {
    let mut buf = Buffer::empty(Rect::new(0, 0, 6, 4));
    // Rows 5 and 6 of the image, three columns, at (2, 1).
    paint(
        &mut buf,
        Rect::new(2, 1, 3, 2),
        0x01_02_03,
        0x00_00_04,
        5,
        Color::Rgb(9, 9, 9),
    );
    for (x, y) in [(2, 1), (4, 1), (3, 2)] {
        let cell = buf.cell((x, y)).cloned().unwrap_or_default();
        assert_eq!(
            position(cell.symbol()),
            Some((usize::from(y) + 4, usize::from(x) - 2)),
            "cell ({x}, {y})"
        );
        assert_eq!(cell.fg, Color::Rgb(1, 2, 3), "the image id");
        assert_eq!(
            cell.underline_color,
            Color::Rgb(0, 0, 4),
            "the placement id"
        );
        assert_eq!(cell.bg, Color::Rgb(9, 9, 9), "transparency shows the theme");
    }
    // Nothing outside the rect is touched.
    assert_eq!(
        buf.cell((1, 1)).map(|c| c.symbol().to_owned()).as_deref(),
        Some(" ")
    );
    assert_eq!(
        buf.cell((2, 3)).map(|c| c.symbol().to_owned()).as_deref(),
        Some(" ")
    );
}

#[test]
fn rows_past_the_diacritics_are_left_blank() {
    let mut buf = Buffer::empty(Rect::new(0, 0, 2, 2));
    paint(&mut buf, Rect::new(0, 0, 2, 2), 1, 1, 296, Color::Reset);
    assert!(
        position(
            buf.cell((0, 0))
                .map(|c| c.symbol().to_owned())
                .unwrap_or_default()
                .as_str()
        )
        .is_some()
    );
    assert_eq!(
        buf.cell((0, 1)).map(|c| c.symbol().to_owned()).as_deref(),
        Some(" ")
    );
}

#[test]
fn the_diacritics_are_distinct_zero_width_marks() {
    use unicode_width::UnicodeWidthChar as _;
    let mut seen = std::collections::HashSet::new();
    assert!(
        DIACRITICS
            .iter()
            .all(|&d| seen.insert(d) && d.width() == Some(0))
    );
}
