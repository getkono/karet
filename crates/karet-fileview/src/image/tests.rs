use super::*;

/// A 2×2 image with one translucent pixel (so lossy WebP is extended, with `ALPH`).
#[cfg(feature = "images")]
fn rgba_fixture(encoder: impl gamut::core::EncodeImage<Rgba8>) -> Vec<u8> {
    rgba_fixture_alpha(encoder, 128)
}

/// A 2×2 image whose last pixel has alpha `alpha`.
#[cfg(feature = "images")]
fn rgba_fixture_alpha(encoder: impl gamut::core::EncodeImage<Rgba8>, alpha: u8) -> Vec<u8> {
    use gamut::core::Dimensions;
    use gamut::core::ImageRef;

    let rgba = [
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, alpha,
    ];
    let Ok(dimensions) = Dimensions::new(2, 2) else {
        return Vec::new();
    };
    let Ok(image) = ImageRef::<Rgba8>::new(&rgba, dimensions) else {
        return Vec::new();
    };
    encoder.encode_to_vec(image).unwrap_or_default()
}

#[cfg(feature = "images")]
fn jpeg_fixture() -> Vec<u8> {
    use gamut::core::Dimensions;
    use gamut::core::EncodeImage as _;
    use gamut::core::ImageRef;

    let rgb = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];
    let Ok(dimensions) = Dimensions::new(2, 2) else {
        return Vec::new();
    };
    let Ok(image) = ImageRef::<Rgb8>::new(&rgb, dimensions) else {
        return Vec::new();
    };
    gamut::jpeg::JpegEncoder::new()
        .encode_to_vec(image)
        .unwrap_or_default()
}

#[cfg(feature = "images")]
fn empty() -> Image {
    Image {
        rgba: Vec::new(),
        width: 0,
        height: 0,
    }
}

#[cfg(feature = "images")]
#[test]
fn decode_and_dimensions() {
    let png = test_png();
    assert_eq!(dimensions(&png), Some((2, 2)));
    assert_eq!(dimensions(&png[..24]), Some((2, 2)));
    assert!(decode(&png[..24]).is_err());
    let img = decode(&png);
    assert!(img.is_ok());
    let img = img.unwrap_or_else(|_| empty());
    assert_eq!((img.width(), img.height()), (2, 2));
}

#[cfg(feature = "images")]
#[test]
fn gamut_decodes_all_supported_formats_to_the_shared_rgba_model() {
    let png = test_png();
    let jpeg = jpeg_fixture();
    let webp = rgba_fixture(gamut::webp::WebpEncoder::lossless());
    let tiff = rgba_fixture(gamut::tiff::TiffEncoder::new());
    assert!(is_png(&png));
    assert!(is_jpeg(&jpeg));
    assert!(is_webp(&webp));
    assert!(is_tiff(&tiff));
    for encoded in [&png, &jpeg, &webp, &tiff] {
        assert_eq!(dimensions(encoded), Some((2, 2)));
        let decoded = decode(encoded);
        assert!(decoded.is_ok());
        let image = decoded.unwrap_or_else(|_| empty());
        assert_eq!((image.width(), image.height()), (2, 2));
        assert_eq!(image.rgba.len(), 16);
        if is_jpeg(encoded) {
            assert!(image.rgba.chunks_exact(4).all(|pixel| pixel[3] == 255));
        }
    }
}

#[cfg(feature = "images")]
#[test]
fn decode_rejects_garbage() {
    assert!(matches!(decode(b"not an image"), Err(ImageError::Decode)));
}

#[cfg(feature = "raster")]
#[test]
fn from_rgba_keeps_dimensions_and_feeds_kitty() {
    // A 2×1 image supplied as raw RGBA reuses the Kitty escape path.
    let img = Image::from_rgba(vec![1, 2, 3, 4, 5, 6, 7, 8], 2, 1);
    assert_eq!((img.width(), img.height()), (2, 1));
    let esc = img.kitty_escape(2, 1);
    assert!(esc.contains("s=2"));
    assert!(esc.contains("v=1"));
}

#[cfg(feature = "raster")]
#[test]
fn from_rgba_pads_short_buffers_to_declared_size() {
    // Fewer bytes than width*height*4 are padded so the buffer stays valid.
    let img = Image::from_rgba(vec![255, 0, 0, 255], 2, 2);
    assert_eq!((img.width(), img.height()), (2, 2));
    let area = Rect::new(0, 0, 2, 2);
    let mut buf = Buffer::empty(area);
    ImageWidget::new(&img).render(area, &mut buf);
    assert!(buf.content().iter().any(|c| c.symbol() == "▀"));
}

#[cfg(feature = "raster")]
#[test]
fn built_in_resampler_bilinearly_blends_pixel_centers() {
    let pixel = |value: u8| [value, value, value, 255];
    let rgba = [pixel(0), pixel(100), pixel(200), pixel(255)].concat();
    let image = Image::from_rgba(rgba, 2, 2);
    assert_eq!(
        image.sample_resized(1, 1, 3, 3, u32::MAX),
        [139, 139, 139, 255]
    );
}

/// A `size`×`size` one-pixel black-and-white checkerboard.
#[cfg(feature = "raster")]
fn checkerboard(size: u32) -> Image {
    let rgba = (0..size * size)
        .flat_map(|i| {
            let value = if (i % size + i / size).is_multiple_of(2) {
                0
            } else {
                255
            };
            [value, value, value, 255]
        })
        .collect();
    Image::from_rgba(rgba, size, size)
}

#[cfg(feature = "raster")]
#[test]
fn shrinking_averages_every_covered_pixel_instead_of_aliasing() {
    // A fourfold shrink of a one-pixel checkerboard folds equal black and white into
    // every destination pixel; a four-tap bilinear read lands on one colour.
    let image = checkerboard(16);
    for (x, y) in [(0, 0), (1, 2), (3, 3)] {
        assert_eq!(
            image.sample_resized(x, y, 4, 4, u32::MAX),
            [128, 128, 128, 255]
        );
    }
    // A non-integral shrink weighs the partly covered edge pixels by their overlap.
    let image = Image::from_rgba(
        [[0, 0, 0, 255], [255, 255, 255, 255], [90, 90, 90, 255]].concat(),
        3,
        1,
    );
    assert_eq!(
        image.sample_resized(0, 0, 2, 1, u32::MAX),
        [85, 85, 85, 255]
    );
    assert_eq!(
        image.sample_resized(1, 0, 2, 1, u32::MAX),
        [145, 145, 145, 255]
    );
}

#[cfg(feature = "raster")]
#[test]
fn a_bounded_shrink_still_reads_every_phase_of_a_fine_pattern() {
    // A 16-fold shrink past the painters' four taps a side spreads its samples over
    // both colours of the checkerboard rather than landing on one.
    let image = checkerboard(64);
    for (x, y) in [(0, 0), (1, 2), (3, 3)] {
        assert_eq!(image.sample_resized(x, y, 4, 4, 4), [128, 128, 128, 255]);
    }
    // And the per-frame painter uses it: every cell of the painted box is grey.
    let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
    image.render_halfblocks_rows(4, 2, 0, Rect::new(0, 0, 4, 2), &mut buf);
    assert!(
        buf.content().iter().all(
            |cell| cell.fg == Color::Rgb(128, 128, 128) && cell.bg == Color::Rgb(128, 128, 128)
        )
    );
}

#[cfg(feature = "raster")]
#[test]
fn a_transparent_pixel_does_not_darken_its_neighbours() {
    // White beside fully transparent black: the average stays white, half covered.
    let image = Image::from_rgba([[255, 255, 255, 255], [0, 0, 0, 0]].concat(), 2, 1);
    assert_eq!(
        image.sample_resized(0, 0, 1, 1, u32::MAX),
        [255, 255, 255, 128]
    );
    // All transparent: no colour to weigh, and no division by zero.
    let clear = Image::from_rgba(vec![0; 8], 2, 1);
    assert_eq!(clear.sample_resized(0, 0, 1, 1, u32::MAX), [0, 0, 0, 0]);
}

#[cfg(feature = "raster")]
#[test]
fn resized_matches_the_sampler_and_paints_one_to_one() {
    let image = checkerboard(16);
    let small = image.resized(4, 8);
    assert_eq!((small.width(), small.height()), (4, 8));
    assert!(
        small
            .rgba()
            .chunks(4)
            .all(|pixel| pixel == [128, 128, 128, 255])
    );
    // Painting the resized copy at its own size equals painting the original into
    // the same box.
    let area = Rect::new(0, 0, 4, 4);
    let (mut direct, mut copied) = (Buffer::empty(area), Buffer::empty(area));
    image.render_halfblocks_rows(4, 4, 0, area, &mut direct);
    small.render_halfblocks_rows(4, 4, 0, area, &mut copied);
    assert_eq!(direct, copied);
    // Degenerate sizes give an empty image, not a panic.
    assert_eq!(image.resized(0, 3).rgba().len(), 0);
}

#[cfg(feature = "raster")]
#[test]
fn an_id_scoped_kitty_escape_is_deletable_by_that_id_alone() {
    let image = Image::from_rgba(vec![255; 4 * 4], 2, 2);
    let escape = image.kitty_escape_with_id(7, 3, 1);
    assert!(escape.starts_with("\x1b_Ga=T,i=7,f=32,s=2,v=2,c=3,r=1,q=2,m=0;"));
    assert_eq!(kitty_delete_image(7), "\x1b_Ga=d,d=I,i=7,q=2\x1b\\");
    // The anonymous escape keeps its exact form.
    assert!(
        image
            .kitty_escape(3, 1)
            .starts_with("\x1b_Ga=T,f=32,s=2,v=2,c=3,r=1,m=0;")
    );
}

#[cfg(feature = "images")]
#[test]
fn halfblocks_fill_cells() {
    let img = decode(&test_png()).unwrap_or_else(|_| empty());
    let area = Rect::new(0, 0, 4, 2);
    let mut buf = Buffer::empty(area);
    ImageWidget::new(&img).render(area, &mut buf);
    assert!(buf.content().iter().any(|c| c.symbol() == "▀"));
}

#[cfg(feature = "images")]
#[test]
fn kitty_escape_has_header_and_terminators() {
    let img = decode(&test_png()).unwrap_or_else(|_| empty());
    let esc = img.kitty_escape(4, 2);
    assert!(esc.starts_with("\x1b_G"));
    assert!(esc.ends_with("\x1b\\"));
    assert!(esc.contains("a=T"));
    assert!(esc.contains("f=32"));
    assert!(esc.contains("c=4"));
    assert!(esc.contains("r=2"));
}

#[cfg(feature = "images")]
#[test]
fn probe_reads_png_jpeg_and_webp_headers_from_a_prefix() {
    let png = test_png();
    assert_eq!(probe_dimensions(&png[..24]), Some((2, 2)));
    let webp = rgba_fixture(gamut::webp::WebpEncoder::lossless());
    assert_eq!(probe_dimensions(&webp[..25.min(webp.len())]), Some((2, 2)));
    let lossy = rgba_fixture_alpha(gamut::webp::WebpEncoder::lossy(80), 255);
    assert_eq!(
        probe_dimensions(&lossy[..30.min(lossy.len())]),
        Some((2, 2))
    );
    // Translucent lossy is extended (`VP8X`, `ALPH`, `VP8 `): the canvas alone is
    // not vouched for, so the prefix must reach the frame's header.
    let translucent = rgba_fixture(gamut::webp::WebpEncoder::lossy(80));
    assert_eq!(translucent.get(12..16), Some(&b"VP8X"[..]));
    assert_eq!(probe_dimensions(&translucent[..30]), None);
    assert_eq!(probe_dimensions(&translucent), Some((2, 2)));
    // The JPEG frame header sits past the tables; the prefix need only reach the
    // end of its segment (marker, 2-byte length, then that many bytes less two).
    let jpeg = jpeg_fixture();
    let sof = jpeg
        .windows(2)
        .position(|w| w == [0xff, 0xc0] || w == [0xff, 0xc2])
        .unwrap_or(jpeg.len());
    let length = jpeg
        .get(sof + 2..sof + 4)
        .map_or(0, |b| usize::from(u16::from_be_bytes([b[0], b[1]])));
    let end = (sof + 2 + length).min(jpeg.len());
    assert!(end < jpeg.len(), "the probe must not need the scan data");
    assert_eq!(probe_dimensions(&jpeg[..end]), Some((2, 2)));
}

/// Wrap the frame chunk of the simple WebP file `simple` in an extended file
/// whose `VP8X` header carries `flags` and a `width`×`height` canvas.
#[cfg(feature = "images")]
fn extended_webp(simple: &[u8], flags: u8, width: u32, height: u32) -> Vec<u8> {
    let mut file = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0".to_vec();
    file.extend_from_slice(&[flags, 0, 0, 0]);
    file.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
    file.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
    file.extend_from_slice(simple.get(12..).unwrap_or_default());
    let riff = u32::try_from(file.len() - 8).unwrap_or_default();
    file[4..8].copy_from_slice(&riff.to_le_bytes());
    file
}

#[cfg(feature = "images")]
#[test]
fn probe_reads_an_extended_webp_from_its_frame() {
    for simple in [
        rgba_fixture(gamut::webp::WebpEncoder::lossless()),
        rgba_fixture_alpha(gamut::webp::WebpEncoder::lossy(80), 255),
    ] {
        assert_ne!(simple.get(12..16), Some(&b"VP8X"[..]));
        let file = extended_webp(&simple, 0, 2, 2);
        assert_eq!(probe_dimensions(&file), Some((2, 2)));
        // The size vouched for is the size decoding produces.
        let decoded = decode(&file).map(|image| (image.width(), image.height()));
        assert_eq!(decoded.ok(), Some((2, 2)));
        // A metadata chunk (odd-sized, so padded) before the frame is walked past.
        let mut padded = file[..30].to_vec();
        padded.extend_from_slice(b"EXIF\x03\0\0\0abc\0");
        padded.extend_from_slice(&file[30..]);
        assert_eq!(probe_dimensions(&padded), Some((2, 2)));
    }
}

#[cfg(feature = "images")]
#[test]
fn probe_refuses_an_extended_webp_whose_frame_outgrows_its_canvas() {
    // A 1×1 canvas wrapping a 5000×5000 lossless frame header: the decoder would
    // allocate the frame's size, so the canvas must not vouch for it.
    let side = 5000 - 1;
    let bits: u32 = side | side << 14;
    let mut simple = b"RIFF\0\0\0\0WEBPVP8L\x05\0\0\0\x2f".to_vec();
    simple.extend_from_slice(&bits.to_le_bytes());
    assert_eq!(probe_dimensions(&simple), Some((5000, 5000)));
    assert_eq!(probe_dimensions(&extended_webp(&simple, 0, 1, 1)), None);
    let lossy = rgba_fixture_alpha(gamut::webp::WebpEncoder::lossy(80), 255);
    assert_eq!(probe_dimensions(&extended_webp(&lossy, 0, 3, 2)), None);
}

#[cfg(feature = "images")]
#[test]
fn probe_refuses_an_extended_webp_cut_before_its_frame_or_animated() {
    let simple = rgba_fixture(gamut::webp::WebpEncoder::lossless());
    let file = extended_webp(&simple, 0, 2, 2);
    // A bare canvas header, and one cut inside the frame's header, vouch for nothing.
    assert_eq!(probe_dimensions(&file[..30]), None);
    assert_eq!(probe_dimensions(&file[..42]), None);
    assert_eq!(probe_dimensions(&file[..43]), Some((2, 2)));
    // An oversized chunk sends the walk past the end rather than wrapping.
    let mut huge = file[..30].to_vec();
    huge.extend_from_slice(b"EXIF\xff\xff\xff\xff");
    assert_eq!(probe_dimensions(&huge), None);
    // Animation is refused even with a matching top-level frame.
    assert_eq!(probe_dimensions(&extended_webp(&simple, 0x02, 2, 2)), None);
}

#[cfg(feature = "images")]
#[test]
fn dimensions_falls_back_to_a_declared_webp_canvas_only_when_nothing_decodes() {
    let simple = rgba_fixture(gamut::webp::WebpEncoder::lossless());
    // An animated canvas whose frames never decode still labels its placeholder.
    let animated = extended_webp(&simple, 0x02, 7, 5);
    assert_eq!(dimensions(&animated[..30]), Some((7, 5)));
    // A file that decodes reports what it decodes to, not what its canvas claims.
    let mismatched = extended_webp(&simple, 0, 7, 5);
    assert_eq!(dimensions(&mismatched), Some((2, 2)));
    assert_eq!(dimensions(b"RIFF\0\0\0\0WEBPVP8X"), None);
}

#[cfg(feature = "images")]
#[test]
fn probe_refuses_tiff_garbage_and_cut_headers() {
    let tiff = rgba_fixture(gamut::tiff::TiffEncoder::new());
    assert_eq!(probe_dimensions(&tiff), None);
    assert_eq!(probe_dimensions(b"not an image"), None);
    assert_eq!(probe_dimensions(&test_png()[..20]), None);
    assert_eq!(probe_dimensions(b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0\x2f"), None);
    assert_eq!(
        probe_dimensions(b"RIFF\0\0\0\0WEBPVP8 \0\0\0\0\0\0\0\0\0\0"),
        None
    );
    // A zero-sized PNG is no image.
    let mut png = test_png();
    png[16..24].fill(0);
    assert_eq!(probe_dimensions(&png), None);
    // `dimensions` still gets TIFF right, by decoding.
    assert_eq!(dimensions(&tiff), Some((2, 2)));
}

/// A 3×4 image whose every pixel is distinct, so a misplaced row shows.
#[cfg(feature = "raster")]
fn gradient() -> Image {
    let rgba = (0..12u8)
        .flat_map(|i| [i * 20, 255 - i * 20, i, 255])
        .collect();
    Image::from_rgba(rgba, 3, 4)
}

#[cfg(feature = "raster")]
#[test]
fn a_row_window_paints_exactly_the_rows_a_whole_render_paints_there() {
    let image = gradient();
    let whole_area = Rect::new(0, 0, 6, 5);
    let mut whole = Buffer::empty(whole_area);
    image.render_halfblocks_rows(6, 5, 0, whole_area, &mut whole);
    for first in 0..5u16 {
        let window_area = Rect::new(10, 20, 6, 2);
        let mut window = Buffer::empty(Rect::new(10, 20, 6, 2));
        image.render_halfblocks_rows(6, 5, first, window_area, &mut window);
        for dy in 0..2u16 {
            for x in 0..6u16 {
                let expected = whole.cell((x, first + dy)).cloned().unwrap_or_default();
                let got = window.cell((10 + x, 20 + dy)).cloned().unwrap_or_default();
                assert_eq!(got, expected, "row {first}+{dy}, column {x}");
            }
        }
    }
    // The whole box is painted, and nothing past it.
    assert!(whole.content().iter().all(|cell| cell.symbol() == "▀"));
}

#[cfg(feature = "raster")]
#[test]
fn a_row_window_is_clipped_to_its_area_and_the_box() {
    let image = gradient();
    // Narrower than the box: only the columns that fit.
    let area = Rect::new(0, 0, 2, 1);
    let mut buf = Buffer::empty(Rect::new(0, 0, 4, 1));
    image.render_halfblocks_rows(4, 2, 0, area, &mut buf);
    assert_eq!(
        buf.cell((1, 0)).map(|c| c.symbol().to_owned()).as_deref(),
        Some("▀")
    );
    assert_eq!(
        buf.cell((2, 0)).map(|c| c.symbol().to_owned()).as_deref(),
        Some(" ")
    );
    // A window starting past the box paints nothing.
    let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
    image.render_halfblocks_rows(4, 2, 2, Rect::new(0, 0, 4, 2), &mut buf);
    assert!(buf.content().iter().all(|cell| cell.symbol() == " "));
    // A degenerate box paints nothing.
    image.render_halfblocks_rows(0, 2, 0, Rect::new(0, 0, 4, 2), &mut buf);
    assert!(buf.content().iter().all(|cell| cell.symbol() == " "));
}

#[test]
fn fit_rect_preserves_aspect_and_centers() {
    // A tall page (612×792 px) into a wide area keeps its portrait aspect and
    // never exceeds the area.
    let area = Rect::new(0, 0, 80, 24);
    let fit = fit_rect(area, 612, 792);
    assert!(fit.width <= area.width && fit.height <= area.height);
    assert!(fit.width > 0 && fit.height > 0);
    // Portrait page → height should hit the limiting dimension.
    assert_eq!(fit.height, area.height);
    // Centered within the area (±1 cell from integer rounding on odd sizes).
    let fit_center = i32::from(fit.x) + i32::from(fit.width) / 2;
    let area_center = i32::from(area.x) + i32::from(area.width) / 2;
    assert!((fit_center - area_center).abs() <= 1);
    // Degenerate inputs fall back to the whole area.
    assert_eq!(fit_rect(area, 0, 10), area);
}

#[test]
fn detect_protocol_returns_a_variant() {
    assert!(matches!(
        detect_protocol(),
        GraphicsProtocol::Kitty | GraphicsProtocol::Halfblocks
    ));
    assert_eq!(kitty_delete_all(), "\x1b_Ga=d\x1b\\");
}
