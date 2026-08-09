//! Workspace helpers: opening a file into the right tab, and collecting the files
//! shown in the explorer's quick-open list.
//!
//! Opening is routing, not decoding: the path is classified (via the shared
//! `karet-filetype` registry) and the matching tab is reserved immediately.
//! Decoding belongs to the backend — editable text/CBOR content arrives through
//! the session's document snapshots, and DOCX previews through
//! `Command::ConvertDocument`. Only presentation media (images, PDF pages, hex
//! bytes) are read directly, since rendering them is the app's job.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

#[cfg(feature = "images")]
use karet_fileview::image;
use karet_fileview::viewer::FileKind;
use karet_fileview::viewer::{self};
use karet_syntax::FoldRegions;
use karet_syntax::Highlights;
use karet_syntax::SemanticBlocks;
use karet_text::TextBuffer;
use karet_treesitter::language_name_from_path;

use crate::tab::Tab;
use crate::tab::TabKind;

/// How many leading bytes to sample for file-type classification.
pub(crate) const HEAD_BYTES: usize = 8192;

/// Open `path` as a tab, classifying its content and choosing a renderer. Files
/// larger than the [size guard](viewer::SIZE_GUARD) route to a too-large
/// placeholder; [`open_file_ignoring_size`] bypasses that guard. Failures degrade
/// gracefully to a placeholder rather than erroring.
#[must_use]
pub fn open_file(path: &Path) -> Tab {
    let (bytes, len) = read_file(path);
    let head = &bytes[..bytes.len().min(HEAD_BYTES)];
    let kind = viewer::classify(path, head, len);
    open_classified(path, kind, bytes, len)
}

/// Open `path`, bypassing the [size guard](viewer::SIZE_GUARD) so an over-large
/// file opens with the renderer its content warrants (never a too-large
/// placeholder). Backs the TUI "open anyway" override on a too-large placeholder.
#[must_use]
pub fn open_file_ignoring_size(path: &Path) -> Tab {
    let (bytes, len) = read_file(path);
    let head = &bytes[..bytes.len().min(HEAD_BYTES)];
    let kind = viewer::classify_ignoring_size(path, head);
    open_classified(path, kind, bytes, len)
}

/// Read `path`'s bytes (empty on error) and its length, the shared inputs to both
/// open paths.
fn read_file(path: &Path) -> (Vec<u8>, u64) {
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let bytes = std::fs::read(path).unwrap_or_default();
    (bytes, len)
}

/// Route an already-classified file to its renderer tab.
fn open_classified(path: &Path, kind: FileKind, bytes: Vec<u8>, len: u64) -> Tab {
    match kind {
        FileKind::Text | FileKind::Markdown => open_text(path, &bytes),
        #[cfg(feature = "images")]
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
        // The backend decodes CBOR authoritatively once the document registers;
        // the tab is reserved empty and fills from the first snapshot. A corrupt
        // file answers `NotUtf8`, which converts the tab to the hex fallback.
        FileKind::Cbor => open_pending_code(path, "CBOR"),
        FileKind::Binary => Tab::new(
            title(path),
            TabKind::Hex {
                path: path.to_path_buf(),
                bytes,
                scroll: 0,
            },
        ),
        #[cfg(feature = "pdf")]
        FileKind::Pdf => open_document(path, bytes, len),
        // The backend converts DOCX to markdown (`Command::ConvertDocument`);
        // the preview tab is reserved immediately and fills when it answers.
        #[cfg(feature = "docx")]
        FileKind::Docx => Tab::document_converting(path.to_path_buf()),
        FileKind::TooLarge { .. } => placeholder(path, kind, &bytes, len),
        // DOCX/PDF (without their features) and any future `#[non_exhaustive]`
        // kind route to a placeholder describing them.
        _ => placeholder(path, kind, &bytes, len),
    }
}

/// Build a code/text tab with highlighting deferred to the session worker.
fn open_text(path: &Path, bytes: &[u8]) -> Tab {
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
    Tab::new(
        title(path),
        TabKind::Code {
            path: path.to_path_buf(),
            language,
            doc: None,
            next_version: 0,
            buffer,
            text,
            highlights: Highlights::default(),
            semantic_blocks: SemanticBlocks::default(),
            folds: FoldRegions::default(),
            folded: BTreeSet::new(),
            decos: Vec::new(),
            search_decos: Vec::new(),
            syntax_errors: Vec::new(),
        },
    )
}

/// Reserve an editable code tab whose content the session decodes (CBOR →
/// diagnostic notation): the buffer starts empty and fills from the session's
/// first snapshot once the document registers.
fn open_pending_code(path: &Path, language: &'static str) -> Tab {
    Tab::new(
        title(path),
        TabKind::Code {
            path: path.to_path_buf(),
            language,
            doc: None,
            next_version: 0,
            buffer: TextBuffer::new(),
            text: String::new(),
            highlights: Highlights::default(),
            semantic_blocks: SemanticBlocks::default(),
            folds: FoldRegions::default(),
            folded: BTreeSet::new(),
            decos: Vec::new(),
            search_decos: Vec::new(),
            syntax_errors: Vec::new(),
        },
    )
}

/// Open a PDF as a document tab whose pages rasterize on demand (via `karet-pdf`),
/// or fall back to a placeholder if the bytes are not a parseable PDF.
#[cfg(feature = "pdf")]
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
    #[cfg(feature = "images")]
    let dims = if kind == FileKind::Image {
        image::dimensions(bytes)
    } else {
        None
    };
    #[cfg(not(feature = "images"))]
    let dims = {
        let _ = bytes;
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
    // Aligned with the workspace-search walk: symlinks are never followed and
    // the heavyweight dirs are pruned even without an ignore file.
    for entry in ignore::WalkBuilder::new(root)
        .standard_filters(true)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|entry| {
            entry
                .file_name()
                .to_str()
                .is_none_or(|name| !karet_search::IGNORED_DIRS.contains(&name))
        })
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
        let tab = open_file(&file);
        assert!(matches!(tab.kind, TabKind::Code { .. }));
    }

    #[test]
    fn missing_text_path_opens_as_an_empty_code_tab() {
        let dir = temp_dir();
        let file = dir.path.join("NEW.md");

        let tab = open_file(&file);

        assert_eq!(tab.path(), Some(file.as_path()));
        assert!(matches!(tab.kind, TabKind::Code { ref text, .. } if text.is_empty()));
        assert!(!file.exists());
    }

    #[cfg(unix)]
    #[test]
    fn opening_a_symlink_keeps_its_filesystem_identity() -> std::io::Result<()> {
        use std::os::unix::fs::symlink;

        let dir = temp_dir();
        let target = dir.path.join("target.rs");
        let alias = dir.path.join("alias.rs");
        std::fs::write(&target, "fn target() {}\n")?;
        symlink("target.rs", &alias)?;

        let tab = open_file(&alias);
        assert!(tab.is_symlink);
        assert_eq!(tab.path(), Some(alias.as_path()));
        assert!(matches!(tab.kind, TabKind::Code { .. }));
        Ok(())
    }

    #[test]
    fn code_opens_with_text_and_defers_highlighting() {
        let dir = temp_dir();
        let file = dir.path.join("notes.md");
        let _ = std::fs::write(&file, "```rust\nfn main() {}\n```\n");
        let tab = open_file(&file);
        let TabKind::Code {
            text, highlights, ..
        } = tab.kind
        else {
            panic!("expected a code tab");
        };
        assert_eq!(text, "```rust\nfn main() {}\n```\n");
        assert!(
            highlights.all().is_empty(),
            "the session worker supplies syntax after the tab opens"
        );
    }

    #[test]
    fn invalid_utf8_text_opens_as_hex() {
        let dir = temp_dir();
        let file = dir.path.join("bad.rs");
        let _ = std::fs::write(&file, b"fn main() {}\n\xff");
        let tab = open_file(&file);
        assert!(matches!(tab.kind, TabKind::Hex { .. }));
    }

    #[test]
    fn opens_cbor_as_a_pending_code_tab_the_session_will_fill() {
        let dir = temp_dir();
        let file = dir.path.join("data.cbor");
        // 0x82 0x01 0x02: the CBOR array [1, 2].
        let _ = std::fs::write(&file, [0x82u8, 0x01, 0x02]);
        let tab = open_file(&file);
        let TabKind::Code {
            language,
            text,
            doc,
            ..
        } = tab.kind
        else {
            panic!("expected a code tab for a .cbor file");
        };
        assert_eq!(language, "CBOR");
        // The buffer is reserved empty; the session decodes authoritatively and
        // the first snapshot fills it (see the app save/open seam tests).
        assert!(text.is_empty());
        assert!(doc.is_none());
    }

    #[test]
    fn open_file_ignoring_size_bypasses_the_too_large_guard() {
        let dir = temp_dir();
        let file = dir.path.join("big.bin");
        // Just over the size guard: the default open path shows a too-large
        // placeholder…
        let _ = std::fs::write(&file, vec![0u8; viewer::SIZE_GUARD as usize + 1]);
        assert!(matches!(
            open_file(&file).kind,
            TabKind::Placeholder {
                kind: FileKind::TooLarge { .. },
                ..
            }
        ));
        // …while the override opens it with the renderer its content warrants (a
        // NUL-filled blob is binary → the hex view).
        assert!(matches!(
            open_file_ignoring_size(&file).kind,
            TabKind::Hex { .. }
        ));
    }

    #[test]
    fn opens_corrupt_cbor_as_a_pending_code_tab() {
        let dir = temp_dir();
        let file = dir.path.join("broken.cbor");
        // Truncated / invalid CBOR (a map header promising entries, with none).
        // Routing cannot know it is corrupt — the session's decode answers
        // `NotUtf8`, and the app's handler converts the tab to the hex fallback.
        let _ = std::fs::write(&file, [0xa1u8]);
        let tab = open_file(&file);
        assert!(matches!(tab.kind, TabKind::Code { doc: None, .. }));
    }

    /// A minimal single-page PDF (empty US-Letter page), inline (no fixture).
    #[cfg(feature = "pdf")]
    const MINIMAL_PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]>>endobj\n\
trailer<</Size 4/Root 1 0 R>>\n%%EOF";

    #[cfg(feature = "pdf")]
    #[test]
    fn opens_pdf_as_document_tab() {
        let dir = temp_dir();
        let file = dir.path.join("a.pdf");
        let _ = std::fs::write(&file, MINIMAL_PDF);
        let tab = open_file(&file);
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
        let tab = open_file(&file);
        assert!(matches!(
            tab.kind,
            TabKind::Placeholder {
                kind: FileKind::Pdf,
                ..
            }
        ));
    }

    /// A minimal DOCX (a Heading1 + a bold run) zipped in-memory (no fixture).
    #[cfg(feature = "docx")]
    fn tiny_docx() -> Vec<u8> {
        use std::io::Write as _;
        const DOCUMENT_XML: &str = r#"<?xml version="1.0"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Report</w:t></w:r></w:p>
<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>bold</w:t></w:r></w:p>
</w:body></w:document>"#;
        let mut buf = Vec::new();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        writer
            .start_file(
                "word/document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .expect("start_file");
        writer
            .write_all(DOCUMENT_XML.as_bytes())
            .expect("write_all");
        writer.finish().expect("finish");
        buf
    }

    #[cfg(feature = "docx")]
    #[test]
    fn opens_docx_as_a_pending_markdown_preview() {
        let dir = temp_dir();
        let file = dir.path.join("report.docx");
        let _ = std::fs::write(&file, tiny_docx());
        let tab = open_file(&file);
        assert_eq!(tab.title, "report.docx");
        let TabKind::MarkdownPreview {
            buffer,
            pending_since,
            ..
        } = tab.kind
        else {
            panic!("expected a markdown preview tab for a .docx file");
        };
        // Reserved empty; the backend converts and answers DocumentConverted.
        assert!(buffer.text().is_empty());
        assert!(pending_since.is_some());
    }

    #[cfg(feature = "docx")]
    #[test]
    fn opens_corrupt_docx_as_a_pending_preview() {
        let dir = temp_dir();
        let file = dir.path.join("broken.docx");
        // The `.docx` extension classifies Docx, but the bytes are not a ZIP.
        // Routing reserves the preview; the backend's failed conversion degrades
        // it to a placeholder (covered by the app's DocumentConverted handler).
        let _ = std::fs::write(&file, b"this is not a zip archive");
        let tab = open_file(&file);
        assert!(matches!(
            tab.kind,
            TabKind::MarkdownPreview {
                pending_since: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn opens_binary_as_hex_tab() {
        let dir = temp_dir();
        let file = dir.path.join("blob.bin");
        let _ = std::fs::write(&file, [0u8, 1, 2, 3]);
        let tab = open_file(&file);
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
