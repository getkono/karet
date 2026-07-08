//! Workspace helpers: opening a file into the right tab, and collecting the files
//! shown in the explorer's quick-open list.
//!
//! These call the engines directly (`karet-text`, `karet-treesitter`,
//! `karet-syntax`, `karet-fileview`). Routing through `karet-session` is a deferred
//! step; its `Command`/`Event` variants already map onto this flow.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use karet_fileview::image;
use karet_fileview::viewer::FileKind;
use karet_fileview::viewer::{self};
use karet_syntax::FoldRegions;
use karet_syntax::Highlighter;
use karet_syntax::Highlights;
use karet_text::TextBuffer;
use karet_treesitter::ParserPool;
use karet_treesitter::SyntaxTree;
use karet_treesitter::language_id_from_path;
use karet_treesitter::language_name_from_path;

use crate::tab::Tab;
use crate::tab::TabKind;

/// How many leading bytes to sample for file-type classification.
const HEAD_BYTES: usize = 8192;

/// Open `path` as a tab, classifying its content and choosing a renderer. Files
/// larger than the [size guard](viewer::SIZE_GUARD) route to a too-large
/// placeholder; [`open_file_ignoring_size`] bypasses that guard. Failures degrade
/// gracefully to a placeholder rather than erroring.
#[must_use]
pub fn open_file(path: &Path, syntax: bool) -> Tab {
    let (bytes, len) = read_file(path);
    let head = &bytes[..bytes.len().min(HEAD_BYTES)];
    let kind = viewer::classify(path, head, len);
    open_classified(path, syntax, kind, bytes, len)
}

/// Open `path`, bypassing the [size guard](viewer::SIZE_GUARD) so an over-large
/// file opens with the renderer its content warrants (never a too-large
/// placeholder). Backs the TUI "open anyway" override on a too-large placeholder.
#[must_use]
pub fn open_file_ignoring_size(path: &Path, syntax: bool) -> Tab {
    let (bytes, len) = read_file(path);
    let head = &bytes[..bytes.len().min(HEAD_BYTES)];
    let kind = viewer::classify_ignoring_size(path, head);
    open_classified(path, syntax, kind, bytes, len)
}

/// Read `path`'s bytes (empty on error) and its length, the shared inputs to both
/// open paths.
fn read_file(path: &Path) -> (Vec<u8>, u64) {
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let bytes = std::fs::read(path).unwrap_or_default();
    (bytes, len)
}

/// Route an already-classified file to its renderer tab.
fn open_classified(path: &Path, syntax: bool, kind: FileKind, bytes: Vec<u8>, len: u64) -> Tab {
    match kind {
        FileKind::Text | FileKind::Markdown => open_text(path, &bytes, syntax),
        FileKind::Image => match image::decode(&bytes) {
            Ok(img) => Tab::new(
                title(path),
                TabKind::Image {
                    path: path.to_path_buf(),
                    image: img,
                },
            ),
            Err(_) => placeholder(path, kind, &bytes, len),
        },
        FileKind::Cbor => open_cbor(path, &bytes),
        FileKind::Binary => Tab::new(
            title(path),
            TabKind::Hex {
                path: path.to_path_buf(),
                bytes,
                scroll: 0,
            },
        ),
        FileKind::Pdf => open_document(path, bytes, len),
        FileKind::TooLarge { .. } => placeholder(path, kind, &bytes, len),
        // DOCX (rendering deferred — no pure-Rust rasterizer yet) and any future
        // `#[non_exhaustive]` kind route to a placeholder describing them.
        _ => placeholder(path, kind, &bytes, len),
    }
}

/// Build a read-only code/text tab, highlighting when a grammar is available.
fn open_text(path: &Path, bytes: &[u8], syntax: bool) -> Tab {
    let Ok(buffer) = TextBuffer::from_bytes(bytes) else {
        return Tab::new(
            title(path),
            TabKind::Hex {
                path: path.to_path_buf(),
                bytes: bytes.to_vec(),
                scroll: 0,
            },
        );
    };
    let text = buffer.text();
    let language = language_name_from_path(path).unwrap_or("plaintext");
    let highlights = if syntax {
        highlight(path, &text)
    } else {
        Highlights::default()
    };
    Tab::new(
        title(path),
        TabKind::Code {
            path: path.to_path_buf(),
            language,
            doc: None,
            next_version: 0,
            buffer,
            text,
            highlights,
            folds: FoldRegions::default(),
            folded: BTreeSet::new(),
            decos: Vec::new(),
            search_decos: Vec::new(),
        },
    )
}

/// Open a CBOR file as an editable code tab holding its decoded diagnostic
/// notation, or fall back to a hex view if it cannot be decoded. The session
/// re-decodes authoritatively and re-encodes on save (see `karet-session`).
fn open_cbor(path: &Path, bytes: &[u8]) -> Tab {
    match karet_cbor::decode_to_text(bytes) {
        Ok(text) => {
            let buffer = TextBuffer::from_text(&text);
            Tab::new(
                title(path),
                TabKind::Code {
                    path: path.to_path_buf(),
                    language: "CBOR",
                    doc: None,
                    next_version: 0,
                    buffer,
                    text,
                    highlights: Highlights::default(),
                    folds: FoldRegions::default(),
                    folded: BTreeSet::new(),
                    decos: Vec::new(),
                    search_decos: Vec::new(),
                },
            )
        },
        Err(_) => Tab::new(
            title(path),
            TabKind::Hex {
                path: path.to_path_buf(),
                bytes: bytes.to_vec(),
                scroll: 0,
            },
        ),
    }
}

/// Highlight `text` for `path`'s language, or return empty highlights.
fn highlight(path: &Path, text: &str) -> Highlights {
    let Some(lang) = language_id_from_path(path) else {
        return Highlights::default();
    };
    let mut pool = ParserPool::new();
    let Ok(tree) = SyntaxTree::parse(&mut pool, lang, text) else {
        return Highlights::default();
    };
    let Ok(highlighter) = Highlighter::new(lang) else {
        return Highlights::default();
    };
    highlighter.highlight(&tree, text).unwrap_or_default()
}

/// Open a PDF as a document tab whose pages rasterize on demand (via `karet-pdf`),
/// or fall back to a placeholder if the bytes are not a parseable PDF.
fn open_document(path: &Path, bytes: Vec<u8>, len: u64) -> Tab {
    match karet_pdf::Document::load(bytes) {
        Ok(doc) => {
            let page_count = doc.page_count();
            let outline = doc.outline();
            Tab::new(
                title(path),
                TabKind::Document {
                    path: path.to_path_buf(),
                    doc,
                    page_count,
                    page: 0,
                    rendered: None,
                    outline,
                },
            )
        },
        Err(_) => placeholder(path, FileKind::Pdf, &[], len),
    }
}

/// Build a graceful placeholder tab (too-large / DOCX / undecodable image / PDF).
fn placeholder(path: &Path, kind: FileKind, bytes: &[u8], len: u64) -> Tab {
    let dims = if kind == FileKind::Image {
        image::dimensions(bytes)
    } else {
        None
    };
    Tab::new(
        title(path),
        TabKind::Placeholder {
            path: path.to_path_buf(),
            kind,
            dims,
            len,
        },
    )
}

/// The display title for a file path (its file name, or the whole path).
fn title(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .map_or_else(|| path.display().to_string(), str::to_string)
}

/// Collect files under `root` (gitignore-aware) for the quick-open list, capped at
/// `limit` to keep startup cheap. Returns repo-relative-ish display paths paired
/// with their absolute path.
#[must_use]
pub fn list_files(root: &Path, limit: usize) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for entry in ignore::WalkBuilder::new(root)
        .standard_filters(true)
        .require_git(false)
        .build()
        .flatten()
    {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let abs = entry.path().to_path_buf();
        let display = abs
            .strip_prefix(root)
            .unwrap_or(&abs)
            .to_string_lossy()
            .into_owned();
        out.push((display, abs));
        if out.len() >= limit {
            break;
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::tab::TabKind;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempDir {
        path: PathBuf,
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
    fn temp_dir() -> TempDir {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("karet-ws-{}-{}", std::process::id(), n));
        let _ = std::fs::create_dir_all(&path);
        TempDir { path }
    }

    #[test]
    fn opens_text_as_code_tab() {
        let dir = temp_dir();
        let file = dir.path.join("a.rs");
        let _ = std::fs::write(&file, "fn main() {}\n");
        let tab = open_file(&file, true);
        assert!(matches!(tab.kind, TabKind::Code { .. }));
    }

    #[test]
    fn invalid_utf8_text_opens_as_hex() {
        let dir = temp_dir();
        let file = dir.path.join("bad.rs");
        let _ = std::fs::write(&file, b"fn main() {}\n\xff");
        let tab = open_file(&file, true);
        assert!(matches!(tab.kind, TabKind::Hex { .. }));
    }

    #[test]
    fn opens_cbor_as_decoded_code_tab() {
        let dir = temp_dir();
        let file = dir.path.join("data.cbor");
        let value = karet_cbor::CborValue::Array(vec![
            karet_cbor::CborValue::Integer(1),
            karet_cbor::CborValue::Integer(2),
        ]);
        let bytes = karet_cbor::encode(&value).unwrap_or_default();
        let _ = std::fs::write(&file, &bytes);
        let tab = open_file(&file, true);
        let TabKind::Code { language, text, .. } = tab.kind else {
            panic!("expected a decoded code tab for a .cbor file");
        };
        assert_eq!(language, "CBOR");
        assert_eq!(text, "[\n  1,\n  2\n]");
    }

    #[test]
    fn open_file_ignoring_size_bypasses_the_too_large_guard() {
        let dir = temp_dir();
        let file = dir.path.join("big.bin");
        // Just over the size guard: the default open path shows a too-large
        // placeholder…
        let _ = std::fs::write(&file, vec![0u8; viewer::SIZE_GUARD as usize + 1]);
        assert!(matches!(
            open_file(&file, false).kind,
            TabKind::Placeholder {
                kind: FileKind::TooLarge { .. },
                ..
            }
        ));
        // …while the override opens it with the renderer its content warrants (a
        // NUL-filled blob is binary → the hex view).
        assert!(matches!(
            open_file_ignoring_size(&file, false).kind,
            TabKind::Hex { .. }
        ));
    }

    #[test]
    fn opens_corrupt_cbor_as_hex_tab() {
        let dir = temp_dir();
        let file = dir.path.join("broken.cbor");
        // Truncated / invalid CBOR (a map header promising entries, with none).
        let _ = std::fs::write(&file, [0xa1u8]);
        let tab = open_file(&file, true);
        assert!(matches!(tab.kind, TabKind::Hex { .. }));
    }

    /// A minimal single-page PDF (empty US-Letter page), inline (no fixture).
    const MINIMAL_PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]>>endobj\n\
trailer<</Size 4/Root 1 0 R>>\n%%EOF";

    #[test]
    fn opens_pdf_as_document_tab() {
        let dir = temp_dir();
        let file = dir.path.join("a.pdf");
        let _ = std::fs::write(&file, MINIMAL_PDF);
        let tab = open_file(&file, true);
        let TabKind::Document { page_count, .. } = tab.kind else {
            panic!("expected a document tab for a .pdf file");
        };
        assert_eq!(page_count, 1);
    }

    #[test]
    fn opens_corrupt_pdf_as_placeholder() {
        let dir = temp_dir();
        let file = dir.path.join("broken.pdf");
        // A `.pdf` extension classifies Pdf, but the bytes are not a parseable PDF.
        let _ = std::fs::write(&file, b"this is not a pdf at all");
        let tab = open_file(&file, true);
        assert!(matches!(
            tab.kind,
            TabKind::Placeholder {
                kind: FileKind::Pdf,
                ..
            }
        ));
    }

    #[test]
    fn opens_binary_as_hex_tab() {
        let dir = temp_dir();
        let file = dir.path.join("blob.bin");
        let _ = std::fs::write(&file, [0u8, 1, 2, 3]);
        let tab = open_file(&file, true);
        assert!(matches!(tab.kind, TabKind::Hex { .. }));
    }

    #[test]
    fn list_files_finds_and_sorts() {
        let dir = temp_dir();
        let _ = std::fs::write(dir.path.join("b.txt"), "b");
        let _ = std::fs::write(dir.path.join("a.txt"), "a");
        let files = list_files(&dir.path, 100);
        let names: Vec<&str> = files.iter().map(|(d, _)| d.as_str()).collect();
        assert_eq!(names, vec!["a.txt", "b.txt"]);
    }
}
