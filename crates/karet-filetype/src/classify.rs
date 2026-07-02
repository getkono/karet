//! Renderer routing: classify a file into the [`FileKind`] that decides which
//! widget opens it.
//!
//! This is a separate axis from the icon/name [`registry`](crate::registry):
//! routing also considers file size and magic bytes, so a mislabeled image or PDF
//! still opens sensibly and a NUL-containing file opens as a hex dump.

use std::path::Path;

/// Files larger than this are classified [`FileKind::TooLarge`] (10 MiB).
pub const SIZE_GUARD: u64 = 10 * 1024 * 1024;

/// The renderer a file should be opened with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
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
/// [`FileKind::Text`]; the application resolves a grammar from the path. Image and
/// PDF kinds are detected by extension and confirmed (or recovered) by magic
/// bytes, so a mislabeled file still routes sensibly.
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
    if let Some(ext) = ext.as_deref() {
        match ext {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tiff" | "tif" => {
                return FileKind::Image;
            },
            "pdf" => return FileKind::Pdf,
            "md" | "markdown" | "mdown" | "mkd" => return FileKind::Markdown,
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
    if looks_binary(head) {
        return FileKind::Binary;
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

/// Whether `head` looks like binary content: a NUL byte, or an invalid UTF-8 byte
/// that is not merely a multi-byte sequence truncated at the sample boundary.
fn looks_binary(head: &[u8]) -> bool {
    if head.contains(&0) {
        return true;
    }
    match std::str::from_utf8(head) {
        Ok(_) => false,
        // `error_len() == None` means the sample ended mid-character (truncation),
        // which is fine; `Some(_)` is a genuinely invalid byte → binary.
        Err(e) => e.error_len().is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_extension() {
        assert_eq!(classify(Path::new("a.png"), b"", 10), FileKind::Image);
        assert_eq!(classify(Path::new("a.pdf"), b"", 10), FileKind::Pdf);
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

    #[test]
    fn oversize_is_too_large() {
        let len = SIZE_GUARD + 1;
        assert_eq!(
            classify(Path::new("big.rs"), b"fn", len),
            FileKind::TooLarge { len }
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
