use std::collections::HashMap;

use super::*;
use crate::WrappedDocument;
use crate::parse;
use crate::wrap::IMAGE_CHIP;
use crate::wrap::QUOTE_GUTTER;

/// Sizes the images whose source it knows.
struct Sizes(HashMap<&'static str, (u32, u32)>);

impl Sizes {
    fn of(entries: &[(&'static str, (u32, u32))]) -> Self {
        Self(entries.iter().copied().collect())
    }
}

impl ImageSizer for Sizes {
    fn dimensions(&self, image: &ImageRef) -> Option<(u32, u32)> {
        self.0.get(image.src.as_str()).copied()
    }
}

fn wrapped(source: &str, width: u16, sizes: &Sizes) -> WrappedDocument {
    parse(source).wrap_with(width, sizes)
}

/// Each line as `text` or, for an image row, `[src row/rows @col cols]`.
fn describe(doc: &WrappedDocument) -> Vec<String> {
    doc.lines
        .iter()
        .map(|line| match &line.image {
            Some(s) => format!(
                "{}[{} {}/{} @{} {}]",
                line.text(),
                s.src,
                s.row,
                s.rows,
                s.col,
                s.cols
            ),
            None => line.text(),
        })
        .collect()
}

const CELL: (u32, u32) = crate::DEFAULT_CELL_PIXELS;

#[test]
fn cell_box_fits_native_size_one_image_pixel_per_screen_pixel() {
    assert_eq!(cell_box((80, 32), (None, None), 100, CELL), Some((10, 2)));
    // Partial cells round up, so no pixel row is cut off.
    assert_eq!(cell_box((81, 33), (None, None), 100, CELL), Some((11, 3)));
    assert_eq!(cell_box((1, 1), (None, None), 100, CELL), Some((1, 1)));
    // The terminal's real cell size decides the box: 10×20-pixel cells.
    assert_eq!(
        cell_box((100, 100), (None, None), 100, (10, 20)),
        Some((10, 5))
    );
    // A zero-sized cell (a terminal that reports none) cannot divide by zero.
    assert!(cell_box((10, 10), (None, None), 100, (0, 0)).is_some());
}

#[test]
fn cell_box_shrinks_to_the_width_keeping_the_aspect() {
    // 800×160 px is 100×10 cells; at 50 columns it halves both ways.
    assert_eq!(cell_box((800, 160), (None, None), 50, CELL), Some((50, 5)));
}

#[test]
fn a_tall_image_keeps_its_native_height() {
    // 160×800 px is 20×50 cells: no line cap shrinks it.
    assert_eq!(
        cell_box((160, 800), (None, None), 100, CELL),
        Some((20, 50))
    );
    // A 4096-pixel-tall screenshot at a 16-pixel cell keeps all 256 of its rows.
    assert_eq!(
        cell_box((1024, 4096), (None, None), 200, CELL),
        Some((128, 256))
    );
}

#[test]
fn only_the_placeholder_ceiling_bounds_the_rows() {
    // Past the 297 addressable rows the image shrinks, keeping its aspect.
    assert_eq!(
        cell_box((160, 8000), (None, None), 100, CELL),
        Some((11, MAX_IMAGE_CELLS))
    );
    // Even a one-pixel-wide sliver keeps a column.
    assert_eq!(
        cell_box((1, 100_000), (None, None), 100, CELL),
        Some((1, MAX_IMAGE_CELLS))
    );
    // The width is bounded the same way, however wide the pane.
    assert_eq!(
        cell_box((8000, 16), (None, None), usize::MAX, CELL),
        Some((MAX_IMAGE_CELLS, 1))
    );
}

#[test]
fn cell_box_honours_size_hints_but_never_upscales() {
    // One hint scales the other side by the aspect.
    assert_eq!(
        cell_box((800, 400), (Some(200), None), 100, CELL),
        Some((25, 7))
    );
    assert_eq!(
        cell_box((800, 400), (None, Some(160)), 100, CELL),
        Some((40, 10))
    );
    // Both hints are taken as given.
    assert_eq!(
        cell_box((800, 400), (Some(80), Some(80)), 100, CELL),
        Some((10, 5))
    );
    // A hint beyond the native size is clamped to it.
    assert_eq!(
        cell_box((80, 32), (Some(8000), None), 100, CELL),
        cell_box((80, 32), (None, None), 100, CELL)
    );
}

#[test]
fn cell_box_refuses_an_empty_image_or_no_room_and_survives_huge_values() {
    assert_eq!(cell_box((0, 10), (None, None), 10, CELL), None);
    assert_eq!(cell_box((10, 0), (None, None), 10, CELL), None);
    assert_eq!(cell_box((10, 10), (None, None), 0, CELL), None);
    let huge = cell_box(
        (u32::MAX, u32::MAX),
        (Some(u32::MAX), Some(1)),
        usize::MAX,
        (u32::MAX, 1),
    );
    assert!(huge.is_some_and(|(cols, rows)| cols >= 1 && rows >= 1));
}

#[test]
fn the_sizer_cell_size_sizes_the_reserved_rows() {
    struct Big;
    impl ImageSizer for Big {
        fn dimensions(&self, _: &ImageRef) -> Option<(u32, u32)> {
            Some((80, 80))
        }
        fn cell_pixels(&self) -> (u32, u32) {
            (8, 20)
        }
    }
    let doc = parse("![a](a.png)").wrap_with(40, &Big);
    assert_eq!(
        describe(&doc),
        vec![
            "[a.png 0/4 @0 10]",
            "[a.png 1/4 @0 10]",
            "[a.png 2/4 @0 10]",
            "[a.png 3/4 @0 10]"
        ]
    );
}

#[test]
fn a_sized_image_reserves_a_line_per_row() {
    let doc = wrapped("![logo](a.png)\n", 40, &Sizes::of(&[("a.png", (80, 48))]));
    assert_eq!(
        describe(&doc),
        vec![
            "[a.png 0/3 @0 10]",
            "[a.png 1/3 @0 10]",
            "[a.png 2/3 @0 10]"
        ]
    );
    assert!(
        doc.lines
            .iter()
            .all(|line| line.image.as_ref().is_some_and(|s| s.alt == "logo"))
    );
}

#[test]
fn an_image_row_keeps_its_prefix_and_starts_past_it() {
    let doc = wrapped("> ![l](a.png)\n", 40, &Sizes::of(&[("a.png", (16, 16))]));
    assert_eq!(
        describe(&doc),
        vec![format!("{QUOTE_GUTTER}[a.png 0/1 @2 2]")]
    );
}

#[test]
fn an_image_in_a_list_item_sits_behind_the_marker() {
    let doc = wrapped("- ![l](a.png)\n", 40, &Sizes::of(&[("a.png", (16, 32))]));
    assert_eq!(
        describe(&doc),
        vec!["• [a.png 0/2 @2 2]", "  [a.png 1/2 @2 2]"]
    );
}

#[test]
fn mixed_sized_and_unsized_images_keep_their_order() {
    let doc = wrapped(
        "![a](a.png) ![b](b.svg) ![c](c.svg) ![d](d.png)\n",
        40,
        &Sizes::of(&[("a.png", (8, 16)), ("d.png", (8, 16))]),
    );
    assert_eq!(
        describe(&doc),
        vec![
            "[a.png 0/1 @0 1]".to_owned(),
            format!("{IMAGE_CHIP}b {IMAGE_CHIP}c"),
            "[d.png 0/1 @0 1]".to_owned(),
        ]
    );
}

#[test]
fn unsized_images_flow_as_chips_on_one_line() {
    let doc = wrapped("![a](a.svg) ![b](b.svg)\n", 40, &Sizes::of(&[]));
    assert_eq!(describe(&doc), vec![format!("{IMAGE_CHIP}a {IMAGE_CHIP}b")]);
}

#[test]
fn an_image_among_text_is_a_chip_even_when_sized() {
    let doc = wrapped(
        "see ![a](a.png) here\n",
        40,
        &Sizes::of(&[("a.png", (8, 16))]),
    );
    assert_eq!(describe(&doc), vec![format!("see {IMAGE_CHIP}a here")]);
}

#[test]
fn an_image_in_a_table_is_a_chip_even_when_sized() {
    let doc = wrapped(
        "| h |\n| - |\n| ![a](a.png) |\n",
        40,
        &Sizes::of(&[("a.png", (8, 16))]),
    );
    assert!(doc.lines.iter().all(|line| line.image.is_none()));
}

#[test]
fn a_centered_image_moves_its_column_not_its_text() {
    let doc = wrapped(
        "<p align=\"center\"><img src=\"a.png\"></p>\n",
        40,
        &Sizes::of(&[("a.png", (80, 16))]),
    );
    assert_eq!(describe(&doc), vec!["[a.png 0/1 @15 10]"]);
    let doc = wrapped(
        "<p align=\"right\"><img src=\"a.png\"></p>\n",
        40,
        &Sizes::of(&[("a.png", (80, 16))]),
    );
    assert_eq!(describe(&doc), vec!["[a.png 0/1 @30 10]"]);
}

#[test]
fn an_image_is_anchored_on_its_first_row_and_trailing_rows_survive() {
    let doc = wrapped(
        "# T\n\n![a](a.png)\n",
        40,
        &Sizes::of(&[("a.png", (8, 64))]),
    );
    assert_eq!(doc.lines.len(), 6, "{:?}", describe(&doc));
    assert_eq!(doc.wrapped_line_for_source(2), 2);
    assert!(doc.lines.last().is_some_and(|line| line.image.is_some()));
}

#[test]
fn a_linked_image_row_remembers_the_link() {
    let doc = wrapped(
        "[![a](a.png)](https://x)\n",
        40,
        &Sizes::of(&[("a.png", (8, 16))]),
    );
    assert_eq!(
        doc.lines
            .first()
            .and_then(|line| line.image.as_ref())
            .and_then(|s| s.link.as_deref()),
        Some("https://x")
    );
}

#[test]
fn plain_wrap_paints_every_image_as_a_chip() {
    let doc = parse("![a](a.png)\n").wrap(40);
    assert_eq!(describe(&doc), vec![format!("{IMAGE_CHIP}a")]);
}

/// Sizes one image and leads every chip with its own glyph, as an icon-set consumer does.
struct Glyphed;

impl ImageSizer for Glyphed {
    fn dimensions(&self, image: &ImageRef) -> Option<(u32, u32)> {
        (image.src == "big.png").then_some((8, 16))
    }

    fn chip_glyph(&self) -> &str {
        "[I]"
    }
}

#[test]
fn a_sizer_glyph_leads_every_chip_it_wraps() {
    let text = |source: &str| describe(&parse(source).wrap_with(40, &Glyphed));
    // Beside a sized image, among text, in a heading, and in a table cell alike.
    assert_eq!(
        text("![big](big.png) ![a](a.svg)\n"),
        vec!["[big.png 0/1 @0 1]", "[I] a"]
    );
    assert_eq!(text("see ![a](a.svg) here\n"), vec!["see [I] a here"]);
    assert_eq!(text("# ![a](a.svg)\n"), vec!["# [I] a"]);
    assert!(
        text("| h |\n| - |\n| ![a](a.svg) |\n")
            .iter()
            .any(|line| line.contains("[I] a")),
    );
}

#[test]
fn the_default_glyph_leads_a_chip_unless_the_sizer_overrides_it() {
    assert_eq!(IMAGE_CHIP, format!("{} ", Sizes::of(&[]).chip_glyph()));
    assert_eq!(IMAGE_CHIP, format!("{} ", crate::DEFAULT_CHIP_GLYPH));
}
