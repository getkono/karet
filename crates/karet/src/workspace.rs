//! Workspace helpers: opening a file into the right tab, collecting the files
//! shown in the quick-open list (kept off the UI thread by [`FileIndex`]), and
//! the recently opened files and workspaces ([`RecentStore`]).
//!
//! Opening is routing, not decoding: the path is classified (via the shared
//! `karet-filetype` registry) and the matching tab is reserved immediately.
//! Decoding belongs to the backend — editable text/CBOR content arrives through
//! the session's document snapshots, and DOCX previews through
//! `DocumentCommand::Convert`. Only presentation media (images, PDF pages, hex
//! bytes) are read directly, since rendering them is the app's job.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use karet_core::LineCol;
use karet_filetype::FileKind;
#[cfg(feature = "images")]
use karet_fileview::image;
use karet_syntax::FoldRegions;
use karet_syntax::Highlights;
use karet_syntax::SemanticBlocks;
use karet_text::TextSnapshot;

use crate::tab::Tab;
use crate::tab::TabKind;

mod file_index;
mod recent;

pub(crate) use file_index::FileIndex;
pub(crate) use file_index::Scan;
pub(crate) use recent::RecentStore;
pub(crate) use recent::absolute;

/// How many leading bytes to sample for file-type classification.
pub(crate) const HEAD_BYTES: usize = 8192;

/// Open `path` as a tab, classifying its content and choosing a renderer. Files
/// larger than the [size guard](karet_filetype::SIZE_GUARD) route to a too-large
/// placeholder; [`open_file_ignoring_size`] bypasses that guard. Failures degrade
/// gracefully to a placeholder rather than erroring.
///
/// Only the leading [`HEAD_BYTES`] and the length are read to classify. Editable
/// text is not read here at all: its tab is reserved empty and the session's
/// document snapshot fills it, so the file is loaded once, off the UI thread.
#[must_use]
pub fn open_file(path: &Path) -> Tab {
    let (head, len) = read_head(path);
    let kind = karet_filetype::classify(path, &head, len);
    open_classified(path, kind, len)
}

/// Open `path`, bypassing the [size guard](karet_filetype::SIZE_GUARD) so an over-large
/// file opens with the renderer its content warrants (never a too-large
/// placeholder). Backs the TUI "open anyway" override on a too-large placeholder.
#[must_use]
pub fn open_file_ignoring_size(path: &Path) -> Tab {
    let (head, len) = read_head(path);
    let kind = karet_filetype::classify_ignoring_size(path, &head);
    open_classified(path, kind, len)
}

/// Open `path` as an editable code tab whatever its file type, its buffer
/// reserved empty and filled from the session's snapshot once the document
/// registers. Backs a document the backend already holds as text -- one a
/// workspace edit opened and changed -- which must stay editable (and its
/// unsaved change reachable) even where [`open_file`] would route the path to
/// a too-large placeholder, a notebook preview, or a byte view.
#[must_use]
pub fn open_as_text(path: &Path) -> Tab {
    let file_type = karet_filetype::file_type_for_path(path);
    let language = if file_type.is_recognized() {
        file_type.name()
    } else {
        "plaintext"
    };
    open_pending_code(path, language)
}

/// Read at most [`HEAD_BYTES`] leading bytes of `path` (empty on error) and its
/// length: everything classification needs, without reading the rest.
fn read_head(path: &Path) -> (Vec<u8>, u64) {
    use std::io::Read as _;
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut head = Vec::new();
    if let Ok(file) = std::fs::File::open(path) {
        // `HEAD_BYTES` is a small constant; the conversion cannot truncate.
        let _ = file.take(HEAD_BYTES as u64).read_to_end(&mut head);
    }
    (head, len)
}

/// Route an already-classified file to its renderer tab. Only the presentation
/// media the app renders itself (images, PDF pages, hex bytes) read the whole
/// file, and only once their kind is known.
fn open_classified(path: &Path, kind: FileKind, len: u64) -> Tab {
    let read = || std::fs::read(path).unwrap_or_default();
    match kind {
        // The session loads and decodes the text once the document registers;
        // the tab is reserved empty and fills from the first snapshot. Invalid
        // UTF-8 past the sampled head answers `NotUtf8`, which converts the tab
        // to the hex fallback.
        FileKind::Text | FileKind::Markdown => open_as_text(path),
        #[cfg(feature = "images")]
        FileKind::Image => {
            let bytes = read();
            match image::decode(&bytes) {
                Ok(img) => Tab::new(
                    title(path),
                    TabKind::Image {
                        path: path.to_path_buf(),
                        image: img,
                    },
                ),
                Err(_) => placeholder(path, kind, &bytes, len),
            }
        },
        // The backend decodes CBOR authoritatively once the document registers;
        // the tab is reserved empty and fills from the first snapshot. A corrupt
        // file answers `NotUtf8`, which converts the tab to the hex fallback.
        FileKind::Cbor => open_pending_code(path, "CBOR"),
        FileKind::Binary => Tab::new(
            title(path),
            TabKind::Hex {
                path: path.to_path_buf(),
                bytes: read(),
                scroll: 0,
            },
        ),
        #[cfg(feature = "pdf")]
        FileKind::Pdf => open_document(path, read(), len),
        // The backend converts DOCX to markdown (`DocumentCommand::Convert`);
        // the preview tab is reserved immediately and fills when it answers.
        #[cfg(feature = "docx")]
        FileKind::Docx => Tab::document_converting(path.to_path_buf()),
        // Same conversion seam: the backend renders the notebook to markdown.
        #[cfg(feature = "notebook")]
        FileKind::Notebook => Tab::document_converting(path.to_path_buf()),
        // The placeholder needs the bytes only to size an undecodable image
        // (handled above), so a too-large file is never read past its head.
        FileKind::TooLarge { .. } => placeholder(path, kind, &[], len),
        // DOCX/PDF (without their features) and any future `#[non_exhaustive]`
        // kind route to a placeholder describing them.
        _ => placeholder(path, kind, &[], len),
    }
}

/// Reserve an editable code tab whose content the session loads and decodes
/// (text as is, CBOR → diagnostic notation): the buffer starts empty and fills
/// from the session's first snapshot once the document registers, with
/// highlighting deferred to the session worker.
fn open_pending_code(path: &Path, language: &'static str) -> Tab {
    let mut tab = Tab::new(
        title(path),
        TabKind::Code {
            path: path.to_path_buf(),
            language,
            doc: None,
            next_version: 0,
            buffer: Default::default(),
            text: TextSnapshot::default(),
            highlights: Highlights::default(),
            semantic_blocks: SemanticBlocks::default(),
            folds: FoldRegions::default(),
            folded: BTreeSet::new(),
            decos: Vec::new(),
            search_decos: Vec::new(),
            syntax_errors: Vec::new(),
        },
    );
    tab.fill_caret = Some(LineCol::default());
    tab.fill_since = Some(crate::app::Pending::start());
    tab
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

/// Collect every file under `root` (gitignore-aware) for the quick-open list,
/// sorted by display path. Returns workspace-relative display paths paired with
/// their absolute path.
///
/// Uncapped and blocking: a large workspace takes a while to walk, so the app
/// only ever calls this off the UI thread, through [`FileIndex`].
#[must_use]
pub fn list_files(root: &Path) -> Vec<(String, PathBuf)> {
    // The workspace-search walk's own file listing, so quick-open covers the
    // same tree a search does: symlinks never followed, ignore files applied
    // (those above `root` included), heavyweight dirs pruned.
    let mut out: Vec<(String, PathBuf)> = karet_search::walk_files(root)
        .map(|abs| (display_path(root, &abs), abs))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// `path` relative to `root` for display, or the whole path when it lies outside.
fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
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
    fn text_opens_as_a_pending_code_tab_the_session_will_fill() {
        let dir = temp_dir();
        let file = dir.path.join("notes.md");
        let _ = std::fs::write(&file, "```rust\nfn main() {}\n```\n");
        let tab = open_file(&file);
        assert_eq!(tab.fill_caret, Some(LineCol::default()));
        let TabKind::Code {
            language,
            text,
            doc,
            highlights,
            ..
        } = tab.kind
        else {
            panic!("expected a code tab");
        };
        assert_eq!(language, "Markdown");
        // Not decoded here: the session loads the file once and its first
        // snapshot supplies the text and, later, the syntax.
        assert!(text.is_empty());
        assert!(doc.is_none());
        assert!(highlights.all().is_empty());
    }

    #[test]
    fn invalid_utf8_past_the_head_opens_pending_for_the_session_to_reject() {
        let dir = temp_dir();
        let file = dir.path.join("bad.rs");
        // Clean UTF-8 for the whole sampled head, so it classifies as text; the
        // session's decode answers `NotUtf8` and the app falls back to hex.
        let mut bytes = vec![b'a'; HEAD_BYTES];
        bytes.push(0xff);
        let _ = std::fs::write(&file, bytes);
        let tab = open_file(&file);
        assert!(matches!(tab.kind, TabKind::Code { doc: None, .. }));
    }

    #[test]
    fn read_head_samples_only_the_leading_bytes_and_reports_the_length() {
        let dir = temp_dir();
        let file = dir.path.join("long.txt");
        let _ = std::fs::write(&file, vec![b'x'; HEAD_BYTES * 3]);
        let (head, len) = read_head(&file);
        assert_eq!(head.len(), HEAD_BYTES);
        assert_eq!(len, (HEAD_BYTES * 3) as u64);

        let (head, len) = read_head(&dir.path.join("missing.txt"));
        assert!(head.is_empty());
        assert_eq!(len, 0);
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
        let _ = std::fs::write(&file, vec![0u8; karet_filetype::SIZE_GUARD as usize + 1]);
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

    /// A lean build has no PDF renderer, so even a valid PDF routes to the
    /// placeholder describing it rather than a document tab.
    #[cfg(not(feature = "pdf"))]
    #[test]
    fn opens_valid_pdf_as_placeholder_without_the_pdf_feature() {
        let dir = temp_dir();
        let file = dir.path.join("a.pdf");
        let _ = std::fs::write(&file, MINIMAL_PDF);
        assert!(matches!(
            open_file(&file).kind,
            TabKind::Placeholder {
                kind: FileKind::Pdf,
                ..
            }
        ));
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
        let files = list_files(&dir.path);
        let names: Vec<&str> = files.iter().map(|(d, _)| d.as_str()).collect();
        assert_eq!(names, vec!["a.txt", "b.txt"]);
    }
}
