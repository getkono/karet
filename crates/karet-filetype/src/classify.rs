//! Renderer routing: classify a file into the [`FileKind`] that decides which
//! widget opens it.
//!
//! This is a separate axis from the icon/name [`registry`](crate::registry):
//! routing also considers file size and magic bytes, so a mislabeled image or PDF
//! still opens sensibly and a NUL-containing file opens as a hex dump.

use std::path::Path;

/// Files larger than this are classified [`FileKind::TooLarge`] (10 MiB).
pub const SIZE_GUARD: u64 = 10 * 1024 * 1024;

/// The extensions that route to [`FileKind::Markdown`], shared with the
/// registry's Markdown entry so the two cannot drift.
pub(crate) const MARKDOWN_EXTENSIONS: &[&str] = &["md", "markdown", "mdown", "mkd"];

/// The extensions that route to [`FileKind::Image`], shared with the
/// registry's Image entry so the two cannot drift.
pub(crate) const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tiff", "tif",
];

/// The renderer a file should be opened with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum FileKind {
    /// UTF-8 text — code or plain prose (the application resolves the grammar).
    Text,
    /// Markdown source.
    Markdown,
    /// A CBOR document, shown decoded as editable diagnostic notation.
    Cbor,
    /// A raster image (png/jpg/gif/webp/bmp/…).
    Image,
    /// A PDF document.
    Pdf,
    /// A Word (OOXML `.docx`) document.
    Docx,
    /// A Jupyter notebook (`.ipynb`, nbformat JSON).
    Notebook,
    /// Binary content (shown as a hex dump).
    Binary,
    /// A file too large to load inline.
    TooLarge {
        /// The file length in bytes.
        len: u64,
    },
}

/// Classify a file from its `path`, a `head` sample of its leading bytes (e.g. the
/// first 8 KiB), and its total `len` in bytes.
///
/// This deliberately does **not** distinguish code from plain text — both are
/// [`FileKind::Text`]; the application resolves a grammar from the path. A known
/// image, PDF, DOCX, notebook, or CBOR extension routes on the extension alone
/// (the content is not checked against it). Otherwise magic bytes recover a PDF,
/// image, or self-described CBOR file whatever its name; then content that
/// [`looks_binary`] (a NUL in the first [`BINARY_SNIFF_LEN`] bytes) or fails
/// [`is_utf8_text`] routes to [`FileKind::Binary`], and only text-shaped
/// content with a markdown extension becomes [`FileKind::Markdown`].
///
/// Files larger than [`SIZE_GUARD`] are [`FileKind::TooLarge`]. To route with a
/// different ceiling — an inline preview and a full-file reader typically use very
/// different budgets — call [`classify_with_guard`] instead.
#[must_use]
pub fn classify(path: &Path, head: &[u8], len: u64) -> FileKind {
    classify_with_guard(path, head, len, SIZE_GUARD)
}

/// Classify a file like [`classify`], but with a caller-supplied `size_guard`
/// instead of the fixed [`SIZE_GUARD`]: files with `len > size_guard` are
/// [`FileKind::TooLarge`].
///
/// This lets a consumer pick the ceiling appropriate to the context — for example
/// a small budget for an inline preview and a larger one for a full-file reader —
/// without relying on this crate's default. All other routing (extension and
/// magic-byte sniffing) is identical to [`classify`].
#[must_use]
pub fn classify_with_guard(path: &Path, head: &[u8], len: u64, size_guard: u64) -> FileKind {
    if len > size_guard {
        return FileKind::TooLarge { len };
    }
    classify_content(path, head)
}

/// Classify a file by its `path` and `head` sample **without** the [`SIZE_GUARD`]
/// check, so an over-large file still resolves to the renderer its content
/// warrants — this never returns [`FileKind::TooLarge`].
///
/// [`classify`] guards by size for the default open path; a caller that has made
/// the deliberate choice to load a large file regardless (e.g. an "open anyway"
/// override in the UI) routes it through here instead.
#[must_use]
pub fn classify_ignoring_size(path: &Path, head: &[u8]) -> FileKind {
    classify_content(path, head)
}

/// The size-independent core of [`classify`]: extension and magic-byte routing.
fn classify_content(path: &Path, head: &[u8]) -> FileKind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    // Markdown is text-shaped (unlike image/PDF/DOCX/CBOR below), so its extension
    // alone must not skip the binary sniff below — a non-UTF-8 `.md` file needs to
    // still route to `Binary`, not silently open as broken editable text.
    let is_markdown_ext = ext
        .as_deref()
        .is_some_and(|ext| MARKDOWN_EXTENSIONS.contains(&ext));
    if let Some(ext) = ext.as_deref() {
        match ext {
            _ if IMAGE_EXTENSIONS.contains(&ext) => return FileKind::Image,
            "pdf" => return FileKind::Pdf,
            // DOCX is a ZIP container (`PK\x03\x04`, ambiguous with any archive), so it is
            // routed by extension only — there is no cheap magic-byte signal in the head.
            "docx" => return FileKind::Docx,
            "ipynb" => return FileKind::Notebook,
            "cbor" => return FileKind::Cbor,
            _ => {},
        }
    }
    if is_pdf(head) {
        return FileKind::Pdf;
    }
    if is_image(head) {
        return FileKind::Image;
    }
    if is_cbor(head) {
        return FileKind::Cbor;
    }
    if looks_binary(head) || !is_utf8_text(head) {
        return FileKind::Binary;
    }
    if is_markdown_ext {
        return FileKind::Markdown;
    }
    FileKind::Text
}

/// Whether `head` begins with a PDF signature.
fn is_pdf(head: &[u8]) -> bool {
    head.starts_with(b"%PDF-")
}

/// Whether `head` begins with the CBOR self-describe tag (`0xD9D9F7`, RFC 8949
/// §3.4.6). Plain CBOR has no universal magic, so this only recovers files that
/// carry the optional prefix; the `.cbor` extension is the primary signal.
fn is_cbor(head: &[u8]) -> bool {
    head.starts_with(&[0xD9, 0xD9, 0xF7])
}

/// Whether `head` begins with a known raster-image signature.
fn is_image(head: &[u8]) -> bool {
    head.starts_with(b"\x89PNG\r\n\x1a\n")            // PNG
        || head.starts_with(&[0xFF, 0xD8, 0xFF])     // JPEG
        || head.starts_with(b"GIF87a")
        || head.starts_with(b"GIF89a")
        || head.starts_with(b"BM")                    // BMP
        || (head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP"))
}

/// How many leading bytes [`looks_binary`] inspects for a NUL byte (8000, the
/// window git uses to decide whether a file is binary).
pub const BINARY_SNIFF_LEN: usize = 8000;

/// Whether `head` looks like binary content: a NUL byte within its first
/// [`BINARY_SNIFF_LEN`] bytes, git's heuristic. Bytes past the window are ignored,
/// so a caller may pass a whole file or any longer sample.
///
/// This is the one binary sniff the toolkit shares, so the file explorer, the diff
/// engine, workspace search, and git change enumeration agree about the same file. It says nothing about
/// encoding: whether NUL-free bytes decode as UTF-8 is the separate
/// [`is_utf8_text`] question.
#[must_use]
pub fn looks_binary(head: &[u8]) -> bool {
    head.get(..BINARY_SNIFF_LEN).unwrap_or(head).contains(&0)
}

/// Whether `head` is UTF-8 text: valid UTF-8, allowing a multi-byte sequence
/// truncated at the end of the sample (a head cut mid-character is still text).
///
/// [`classify`] routes content that fails this to [`FileKind::Binary`] (the hex
/// view), because it cannot open as editable text. A NUL byte is valid UTF-8, so
/// this does not replace [`looks_binary`].
#[must_use]
pub fn is_utf8_text(head: &[u8]) -> bool {
    match std::str::from_utf8(head) {
        Ok(_) => true,
        // `error_len() == None` means the sample ended mid-character (truncation),
        // which is fine; `Some(_)` is a genuinely invalid byte → not text.
        Err(e) => e.error_len().is_none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Routing and the registry agree: a path the registry names Markdown or
    /// Image opens in that renderer, whatever the extension's case.
    #[test]
    fn registry_markdown_and_image_extensions_route_to_their_renderer() {
        for (ext, kind) in MARKDOWN_EXTENSIONS
            .iter()
            .map(|e| (*e, FileKind::Markdown))
            .chain(IMAGE_EXTENSIONS.iter().map(|e| (*e, FileKind::Image)))
        {
            for name in [format!("a.{ext}"), format!("a.{}", ext.to_uppercase())] {
                let path = Path::new(&name);
                let expected = if kind == FileKind::Markdown {
                    "Markdown"
                } else {
                    "Image"
                };
                assert_eq!(crate::file_type_for_path(path).name(), expected, "{name}");
                assert_eq!(classify(path, b"plain", 5), kind, "{name}");
            }
        }
    }

    #[test]
    fn classifies_by_extension() {
        assert_eq!(classify(Path::new("a.png"), b"", 10), FileKind::Image);
        assert_eq!(classify(Path::new("a.pdf"), b"", 10), FileKind::Pdf);
        assert_eq!(
            classify(Path::new("a.docx"), b"PK\x03\x04", 4),
            FileKind::Docx
        );
        assert_eq!(classify(Path::new("a.md"), b"# hi", 4), FileKind::Markdown);
        assert_eq!(
            classify(Path::new("a.rs"), b"fn main(){}", 11),
            FileKind::Text
        );
    }

    #[test]
    fn classifies_by_magic_bytes() {
        // A PDF mislabeled as .bin is still recognized.
        assert_eq!(classify(Path::new("x.bin"), b"%PDF-1.7", 8), FileKind::Pdf);
        let png = b"\x89PNG\r\n\x1a\n....";
        assert_eq!(classify(Path::new("x.bin"), png, 12), FileKind::Image);
    }

    #[test]
    fn classifies_cbor_by_extension_and_self_describe_tag() {
        // The extension is the primary signal (content is arbitrary bytes).
        assert_eq!(classify(Path::new("a.cbor"), &[0x01], 1), FileKind::Cbor);
        // A self-describing CBOR blob mislabeled `.bin` is still recognized.
        assert_eq!(
            classify(Path::new("x.bin"), &[0xD9, 0xD9, 0xF7, 0x01], 4),
            FileKind::Cbor
        );
    }

    #[test]
    fn non_utf8_markdown_is_binary_not_broken_text() {
        // Regression: the `.md` extension arm used to return `Markdown`
        // unconditionally, bypassing the binary sniff that every other
        // text-shaped extension (or extensionless file) goes through.
        assert_eq!(classify(Path::new("a.md"), b"a\x00b", 3), FileKind::Binary);
        assert_eq!(classify(Path::new("a.md"), b"# hi", 4), FileKind::Markdown);
    }

    #[test]
    fn nul_byte_is_binary_utf8_is_text() {
        assert_eq!(classify(Path::new("x"), b"a\x00b", 3), FileKind::Binary);
        assert_eq!(
            classify(Path::new("x"), "héllo".as_bytes(), 6),
            FileKind::Text
        );
    }

    #[test]
    fn truncated_utf8_head_is_not_binary() {
        // "é" is two bytes; a head cut after the first byte must not read as binary.
        let full = "café".as_bytes();
        let head = &full[..full.len() - 1];
        assert_eq!(
            classify(Path::new("x.txt"), head, full.len() as u64),
            FileKind::Text
        );
    }

    /// The sniff window is git's 8000 bytes: a NUL on its last byte is binary, one
    /// just past it is not, and a sample shorter than the window is read whole.
    #[test]
    fn looks_binary_finds_a_nul_only_inside_the_sniff_window() {
        assert!(!looks_binary(b""));
        assert!(looks_binary(b"\x00"));
        let mut bytes = vec![b'a'; BINARY_SNIFF_LEN + 1];
        assert!(!looks_binary(&bytes));
        bytes[BINARY_SNIFF_LEN - 1] = 0;
        assert!(looks_binary(&bytes));
        bytes[BINARY_SNIFF_LEN - 1] = b'a';
        bytes[BINARY_SNIFF_LEN] = 0;
        assert!(!looks_binary(&bytes));
    }

    /// Encoding is a separate question from the NUL sniff: NUL is valid UTF-8, a
    /// truncated trailing character is still text, a stray invalid byte is not.
    #[test]
    fn is_utf8_text_allows_a_truncated_tail_but_not_an_invalid_byte() {
        assert!(is_utf8_text(b""));
        assert!(is_utf8_text(b"a\x00b"));
        assert!(is_utf8_text(&"café".as_bytes()[..4]));
        assert!(!is_utf8_text(b"caf\xe9!"));
        assert!(!is_utf8_text(b"\xff"));
    }

    /// The explorer routes with the shared window: a NUL past it no longer makes
    /// a file binary (diff and search agree it is text).
    #[test]
    fn nul_past_the_sniff_window_classifies_as_text() {
        let mut head = vec![b'a'; BINARY_SNIFF_LEN + 1];
        head[BINARY_SNIFF_LEN] = 0;
        assert_eq!(
            classify(Path::new("x.txt"), &head, head.len() as u64),
            FileKind::Text
        );
    }

    #[test]
    fn oversize_is_too_large() {
        let len = SIZE_GUARD + 1;
        assert_eq!(
            classify(Path::new("big.rs"), b"fn", len),
            FileKind::TooLarge { len }
        );
    }

    #[test]
    fn size_guard_is_inclusive_at_the_boundary() {
        // `len > guard` is the rule: a file of exactly SIZE_GUARD bytes still opens.
        assert_eq!(
            classify(Path::new("big.rs"), b"fn", SIZE_GUARD),
            FileKind::Text
        );
        assert_eq!(
            classify(Path::new("big.rs"), b"fn", SIZE_GUARD - 1),
            FileKind::Text
        );
        // The guard runs before routing, so even an extension-routed kind is guarded.
        let len = SIZE_GUARD + 1;
        assert_eq!(
            classify(Path::new("big.png"), b"", len),
            FileKind::TooLarge { len }
        );
    }

    #[test]
    fn notebook_routes_by_extension() {
        assert_eq!(
            classify(Path::new("analysis.ipynb"), b"{\"cells\": []}", 13),
            FileKind::Notebook
        );
    }

    #[test]
    fn extensions_match_case_insensitively() {
        let cases = [
            ("PHOTO.PNG", FileKind::Image),
            ("Photo.JpEg", FileKind::Image),
            ("REPORT.PDF", FileKind::Pdf),
            ("Letter.DOCX", FileKind::Docx),
            ("Analysis.IPYNB", FileKind::Notebook),
            ("DATA.CBOR", FileKind::Cbor),
            ("README.MD", FileKind::Markdown),
        ];
        for (name, expected) in cases {
            assert_eq!(classify(Path::new(name), b"# hi", 4), expected, "{name}");
        }
    }

    #[test]
    fn every_image_signature_is_recovered_from_a_mislabeled_file() {
        let cases: &[(&str, &[u8])] = &[
            ("jpeg", &[0xFF, 0xD8, 0xFF, 0xE0]),
            ("gif87a", b"GIF87a\x01\x00"),
            ("gif89a", b"GIF89a\x01\x00"),
            ("bmp", b"BM\x36\x00"),
            ("webp", b"RIFF\x24\x00\x00\x00WEBPVP8 "),
        ];
        for &(label, head) in cases {
            assert_eq!(
                classify(Path::new("x.bin"), head, head.len() as u64),
                FileKind::Image,
                "{label}"
            );
        }
    }

    #[test]
    fn near_miss_image_signatures_are_not_images() {
        // A RIFF container that is not WEBP (e.g. WAVE audio) is not an image; its
        // NUL-bearing size field falls through to the binary sniff.
        let wave = b"RIFF\x24\x00\x00\x00WAVEfmt ";
        assert_eq!(classify(Path::new("x.bin"), wave, 16), FileKind::Binary);
        // A RIFF header cut short before the form type is not an image either.
        assert_eq!(classify(Path::new("x.bin"), b"RIFFabcd", 8), FileKind::Text);
        // Only the two real GIF versions count.
        assert_eq!(classify(Path::new("x.bin"), b"GIF88a", 6), FileKind::Text);
        // Two JPEG marker bytes without the third are invalid UTF-8, not an image.
        assert_eq!(
            classify(Path::new("x.bin"), &[0xFF, 0xD8, 0x41], 3),
            FileKind::Binary
        );
    }

    #[test]
    fn custom_guard_overrides_default_ceiling() {
        // A file under SIZE_GUARD but over a caller's smaller budget is TooLarge.
        let len = 300 * 1024;
        assert_eq!(
            classify_with_guard(Path::new("a.rs"), b"fn main(){}", len, 256 * 1024),
            FileKind::TooLarge { len }
        );
        // A larger caller budget routes a file the default would reject.
        let big = SIZE_GUARD + 1;
        assert_eq!(
            classify_with_guard(Path::new("a.rs"), b"fn main(){}", big, SIZE_GUARD * 2),
            FileKind::Text
        );
    }

    #[test]
    fn classify_matches_guard_with_default() {
        let cases: &[(&str, &[u8], u64)] = &[
            ("a.png", b"", 10),
            ("a.rs", b"fn main(){}", 11),
            ("x", b"a\x00b", 3),
        ];
        for &(name, head, len) in cases {
            assert_eq!(
                classify(Path::new(name), head, len),
                classify_with_guard(Path::new(name), head, len, SIZE_GUARD)
            );
        }
    }

    #[test]
    fn ignoring_size_routes_oversize_to_its_real_renderer() {
        // The same over-large inputs that `classify` guards as `TooLarge` resolve to
        // the content's real kind when the size guard is bypassed — never `TooLarge`.
        assert_eq!(
            classify_ignoring_size(Path::new("big.cbor"), &[0x01]),
            FileKind::Cbor
        );
        assert_eq!(
            classify_ignoring_size(Path::new("big.rs"), b"fn main(){}"),
            FileKind::Text
        );
        assert_eq!(
            classify_ignoring_size(Path::new("big.bin"), b"a\x00b"),
            FileKind::Binary
        );
    }
}
