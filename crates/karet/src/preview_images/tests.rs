use std::path::Path;

use karet_markdown::ImageRef;
use karet_markdown::ImageSizer as _;

use super::*;

/// A valid PNG of `width`×`height` opaque pixels, all `rgb` — encoded here (stored
/// deflate blocks, no compression) so the tests need no image codec of their own.
pub(crate) fn png(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffff_u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(0).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }
    // Every scanline: filter byte 0 (none), then the row's RGBA pixels.
    let mut row = vec![0];
    for _ in 0..width {
        row.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    let raw = row.repeat(usize::try_from(height).unwrap_or(0));
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(0xffff).collect();
    for (index, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(index + 1 == blocks.len()));
        let len = u16::try_from(block.len()).unwrap_or(0);
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// A scratch workspace holding `files`, with the markdown document at its root.
fn workspace(files: &[(&str, &[u8])]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a scratch workspace");
    for (rel, bytes) in files {
        let path = dir.path().join(rel);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, bytes);
    }
    dir
}

fn size(images: &PreviewImages, root: &Path, src: &str) -> Option<(u32, u32)> {
    let source = root.join("README.md");
    images.sizer(&source, root).dimensions(&ImageRef {
        src: src.to_owned(),
        ..ImageRef::default()
    })
}

fn lookup(images: &PreviewImages, root: &Path, src: &str) -> Lookup {
    images.lookup(&root.join("README.md"), root, src)
}

#[test]
fn the_png_fixture_decodes() {
    let bytes = png(3, 2, [10, 20, 30]);
    let image = karet_fileview::image::decode(&bytes);
    assert!(image.is_ok_and(|image| (image.width(), image.height()) == (3, 2)));
}

#[test]
fn a_workspace_image_is_sized_from_its_header_and_queued() {
    let dir = workspace(&[("docs/logo.png", &png(40, 20, [1, 2, 3]))]);
    let images = PreviewImages::default();
    assert_eq!(size(&images, dir.path(), "docs/logo.png"), Some((40, 20)));
    assert_eq!(images.pendings().len(), 1);
    assert!(matches!(
        lookup(&images, dir.path(), "docs/logo.png"),
        Lookup::Loading(_)
    ));
    // Asking again reads nothing new and queues nothing new.
    assert_eq!(size(&images, dir.path(), "./docs/logo.png"), Some((40, 20)));
    assert_eq!(images.pendings().len(), 1);
}

#[test]
fn a_decoded_image_is_ready_and_leaves_the_layout_alone() {
    let dir = workspace(&[("logo.png", &png(4, 4, [9, 9, 9]))]);
    let images = PreviewImages::default();
    let _ = size(&images, dir.path(), "logo.png");
    let before = images.generation();
    images.settle();
    assert!(matches!(
        lookup(&images, dir.path(), "logo.png"),
        Lookup::Ready(image) if (image.width(), image.height()) == (4, 4)
    ));
    assert_eq!(images.generation(), before, "the reserved size was right");
    assert!(images.pendings().is_empty());
}

#[test]
fn images_the_preview_will_not_load_are_never_sized() {
    let parent = workspace(&[
        ("outside.png", &png(2, 2, [0, 0, 0])),
        ("ws/pic.svg", b"<svg xmlns='http://www.w3.org/2000/svg'/>"),
        ("ws/anim.gif", b"GIF89a\x01\x00\x01\x00"),
        ("ws/broken.png", b"\x89PNG\r\n\x1a\n"),
    ]);
    let root = parent.path().join("ws");
    let images = PreviewImages::default();
    let absolute = parent.path().join("outside.png").display().to_string();
    for src in [
        "https://example.com/logo.png",
        "http://example.com/badge.svg",
        "data:image/png;base64,AAAA",
        "../outside.png",
        absolute.as_str(),
        "pic.svg",
        "anim.gif",
        "broken.png",
        "missing.png",
        "",
    ] {
        assert_eq!(size(&images, &root, src), None, "{src:?}");
        assert!(
            matches!(lookup(&images, &root, src), Lookup::Missing),
            "{src:?}"
        );
    }
    assert!(images.pendings().is_empty(), "nothing was queued");
}

#[cfg(unix)]
#[test]
fn a_symlink_out_of_the_workspace_and_a_fifo_are_refused() {
    let parent = workspace(&[
        ("outside.png", &png(2, 2, [0, 0, 0])),
        ("ws/README.md", b""),
    ]);
    let root = parent.path().join("ws");
    let _ = std::os::unix::fs::symlink(parent.path().join("outside.png"), root.join("link.png"));
    let images = PreviewImages::default();
    assert_eq!(size(&images, &root, "link.png"), None);
    // A FIFO would block the read forever; it is no regular file, so it is never opened.
    let fifo = root.join("pipe.png");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .is_ok_and(|status| status.success());
    if made {
        assert_eq!(size(&images, &root, "pipe.png"), None);
    }
}

#[test]
fn an_oversized_file_is_refused_unread() {
    let dir = workspace(&[]);
    let path = dir.path().join("huge.png");
    let file = std::fs::File::create(&path);
    assert!(file.is_ok_and(|file| file.set_len(karet_filetype::SIZE_GUARD + 1).is_ok()));
    let images = PreviewImages::default();
    assert_eq!(size(&images, dir.path(), "huge.png"), None);
}

#[test]
fn an_image_over_the_pixel_cap_is_refused() {
    // Only the header is read, so the claim of a vast image is all it takes.
    let mut bytes = png(1, 1, [0, 0, 0]);
    bytes[16..24].copy_from_slice(&[0, 0, 0x20, 0, 0, 0, 0x20, 0]);
    let dir = workspace(&[("vast.png", &bytes)]);
    let images = PreviewImages::default();
    assert_eq!(size(&images, dir.path(), "vast.png"), None);
    assert!(images.pendings().is_empty());
}

#[test]
fn a_failed_decode_releases_its_rows() {
    // A header that claims a size, over a body that cannot decode.
    let mut bytes = png(8, 8, [0, 0, 0]);
    bytes.truncate(40);
    let dir = workspace(&[("cut.png", &bytes)]);
    let images = PreviewImages::default();
    assert_eq!(size(&images, dir.path(), "cut.png"), Some((8, 8)));
    let before = images.generation();
    images.settle();
    assert_ne!(images.generation(), before, "the layout must re-wrap");
    assert_eq!(size(&images, dir.path(), "cut.png"), None, "now a chip");
    assert!(matches!(
        lookup(&images, dir.path(), "cut.png"),
        Lookup::Missing
    ));
}

#[test]
fn a_decode_for_a_file_that_has_since_changed_is_dropped() {
    let dir = workspace(&[("logo.png", &png(2, 2, [0, 0, 0]))]);
    let images = PreviewImages::default();
    let _ = size(&images, dir.path(), "logo.png");
    let stale = decode_for_test(dir.path().join("logo.png"));
    // The file changes (a different length is a different stamp), and is sized again.
    let _ = std::fs::write(dir.path().join("logo.png"), png(3, 3, [0, 0, 0]));
    assert_eq!(size(&images, dir.path(), "logo.png"), Some((3, 3)));
    if let Some(stale) = stale {
        images.accept(stale);
    }
    assert!(matches!(
        lookup(&images, dir.path(), "logo.png"),
        Lookup::Loading(_)
    ));
}

/// A finished decode of `path` as it is now, stamped as it is now.
fn decode_for_test(path: std::path::PathBuf) -> Option<Decoded> {
    let meta = std::fs::metadata(&path).ok()?;
    Some(cache::decode_now(path, &meta))
}

#[test]
fn a_pending_load_is_offered_for_the_reveal_wake() {
    let dir = workspace(&[("logo.png", &png(2, 2, [0, 0, 0]))]);
    let images = PreviewImages::default();
    let _ = size(&images, dir.path(), "logo.png");
    assert!(
        images
            .pendings()
            .iter()
            .all(|pending| pending.wake(std::time::Instant::now()).is_some())
    );
    images.backdate_pending();
    assert!(images.pendings().iter().all(|pending| pending.visible()));
}

#[test]
fn over_budget_the_least_recently_seen_image_is_evicted_and_reloads_on_sight() {
    let dir = workspace(&[
        ("a.png", &png(4, 4, [1, 1, 1])),
        ("b.png", &png(4, 4, [2, 2, 2])),
    ]);
    // Room for one 4×4 RGBA image (64 bytes), not two.
    let images = PreviewImages::with_budget(100);
    let _ = size(&images, dir.path(), "a.png");
    images.settle();
    let _ = size(&images, dir.path(), "b.png");
    images.settle();
    assert_eq!(images.ready_bytes(), 64);
    assert!(matches!(
        lookup(&images, dir.path(), "b.png"),
        Lookup::Ready(_)
    ));
    // `a` was evicted; seeing it queues it again.
    assert!(matches!(
        lookup(&images, dir.path(), "a.png"),
        Lookup::Loading(_)
    ));
    images.settle();
    assert!(matches!(
        lookup(&images, dir.path(), "a.png"),
        Lookup::Ready(_)
    ));
    assert_eq!(images.ready_bytes(), 64, "and `b` made room for it");
}

#[test]
fn the_receiver_is_handed_out_once() {
    let images = PreviewImages::default();
    assert!(images.take_receiver().is_some());
    assert!(images.take_receiver().is_none());
}

#[test]
fn a_jpeg_whose_frame_header_lies_past_the_probe_is_still_queued() {
    // SOI, then an APP1 segment run longer than the probe reads, then no frame at all.
    let mut bytes = b"\xff\xd8".to_vec();
    for _ in 0..2 {
        bytes.extend_from_slice(&[0xff, 0xe1, 0xff, 0xff]);
        bytes.extend(std::iter::repeat_n(0u8, 0xfffd));
    }
    let dir = workspace(&[("big-exif.jpg", &bytes)]);
    let images = PreviewImages::default();
    assert_eq!(
        size(&images, dir.path(), "big-exif.jpg"),
        None,
        "no rows yet"
    );
    assert_eq!(images.pendings().len(), 1, "but the decode is queued");
    images.settle();
    assert!(matches!(
        lookup(&images, dir.path(), "big-exif.jpg"),
        Lookup::Missing
    ));
}

/// A JPEG whose frame header — past the probe, behind a long APP1 segment — claims
/// `width`×`height`, with no scan after it.
fn jpeg_claiming(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = b"\xff\xd8".to_vec();
    for _ in 0..2 {
        bytes.extend_from_slice(&[0xff, 0xe1, 0xff, 0xff]);
        bytes.extend(std::iter::repeat_n(0u8, 0xfffd));
    }
    bytes.extend_from_slice(&[0xff, 0xc2, 0x00, 0x11, 8]);
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&[3, 1, 0x11, 0, 2, 0x11, 1, 3, 0x11, 1]);
    bytes
}

#[test]
fn a_file_claiming_a_vast_image_is_refused_before_decoding() {
    // The decoder would allocate for the claimed size; the claim alone refuses it.
    assert!(!cache::admissible(&jpeg_claiming(40_000, 40_000)));
    assert!(cache::admissible(&jpeg_claiming(64, 64)));
    assert!(cache::admissible(&png(2, 2, [0, 0, 0])));
    assert!(!cache::admissible(b"GIF89a"));
    let dir = workspace(&[("vast.jpg", &jpeg_claiming(40_000, 40_000))]);
    let images = PreviewImages::default();
    assert_eq!(size(&images, dir.path(), "vast.jpg"), None);
    images.settle();
    assert!(matches!(
        lookup(&images, dir.path(), "vast.jpg"),
        Lookup::Missing
    ));
}
