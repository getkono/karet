use std::io::Read as _;

use super::*;

/// The keys of each escape in `escapes`, and the base64 payload they carry, joined.
fn split(escapes: &str) -> (Vec<String>, String) {
    let mut keys = Vec::new();
    let mut payload = String::new();
    for escape in escapes.split("\x1b\\").filter(|escape| !escape.is_empty()) {
        let body = escape.trim_start_matches("\x1b_G");
        let (head, data) = body.split_once(';').unwrap_or((body, ""));
        keys.push(head.to_owned());
        payload.push_str(data);
    }
    (keys, payload)
}

/// A `width`×`height` image whose every pixel differs.
fn noisy(width: u32, height: u32) -> Image {
    let rgba = (0..width * height * 4)
        .map(|i| u8::try_from(i.wrapping_mul(2_654_435_761) >> 24).unwrap_or_default())
        .collect();
    Image::from_rgba(rgba, width, height)
}

#[test]
fn a_transmission_carries_every_pixel_losslessly_under_its_id() {
    let image = noisy(64, 48);
    let (keys, payload) = split(&transmit(&image, 7));
    assert!(keys.len() > 1, "a payload past one chunk is chunked");
    assert_eq!(
        keys.first().map(String::as_str),
        Some("a=t,i=7,f=32,s=64,v=48,o=z,q=2,m=1")
    );
    assert!(keys[1..keys.len() - 1].iter().all(|k| k == "m=1,q=2"));
    assert_eq!(keys.last().map(String::as_str), Some("m=0,q=2"));
    // The full-resolution RGBA, not a downscaled copy, is what arrives.
    let compressed = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .unwrap_or_default();
    let mut rgba = Vec::new();
    let _ = flate2::read::ZlibDecoder::new(compressed.as_slice()).read_to_end(&mut rgba);
    assert_eq!(rgba, image.rgba());
}

#[test]
fn a_small_image_is_one_escape() {
    let (keys, _) = split(&transmit(&noisy(2, 2), 1));
    assert_eq!(keys, vec!["a=t,i=1,f=32,s=2,v=2,o=z,q=2,m=0"]);
}

#[test]
fn a_placement_is_virtual_and_sized_in_cells() {
    assert_eq!(
        place(9, 2, 40, 12),
        "\x1b_Ga=p,U=1,i=9,p=2,c=40,r=12,q=2\x1b\\"
    );
}

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
