//! Opening a file into the right tab.
//!
//! Opening is routing, not reading. The path is classified against the shared
//! `karet-filetype` registry — a lookup, no I/O — and the matching tab is
//! reserved immediately so the destination exists before anything is fetched.
//! Content arrives afterwards, from the backend: editable text and CBOR through
//! document snapshots, DOCX and notebooks through `DocumentCommand::Convert`,
//! and media bytes through `WorkspaceCommand::ReadFileBytes`.
//!
//! Nothing here touches the filesystem. That is what lets the shell render a
//! workspace on another machine — the client's own disk is simply not part of
//! the story — and it costs a co-located session nothing but a channel hop.
//!
//! A path-only guess can be wrong: an extension can lie, and a file can be too
//! large to load inline. The backend answers `WorkspaceCommand::ClassifyPath`
//! authoritatively (magic bytes, real length) and the tab is converted if the
//! guess did not hold; see `crate::app::open`.

use std::collections::BTreeSet;
use std::path::Path;

use karet_core::FoldRegions;
use karet_core::Highlights;
use karet_core::LineCol;
use karet_core::SemanticBlocks;
use karet_filetype::FileKind;
use karet_filetype::FileType;
#[cfg(feature = "images")]
use karet_fileview::image;
use karet_text::TextSnapshot;

use super::Tab;
use super::TabKind;
use crate::pending::Pending;

impl Tab {
    /// Reserve a tab for `path`, choosing a renderer from its file type.
    ///
    /// Returns immediately with an empty tab of the right shape; the content
    /// follows from the backend, which also corrects the guess when the bytes
    /// disagree with the name or the file is over the
    /// [size guard](karet_filetype::SIZE_GUARD). A text tab, and a DOCX or
    /// notebook preview, is marked [`size_unchecked`](Self::size_unchecked): the
    /// backend reads the whole file to load or convert it, so that is asked for
    /// only once the file is known to be under the guard.
    #[must_use]
    pub fn open(path: &Path) -> Self {
        let kind = kind_from_path(path);
        let mut tab = reserve(path, kind);
        tab.size_unchecked = is_text_kind(kind) || is_converted_kind(kind);
        tab
    }

    /// [`Self::open`] for the "open anyway" override on a too-large placeholder:
    /// the same reservation with the size check waived, which the caller pairs
    /// with a classification that bypasses the size guard.
    #[must_use]
    pub fn open_ignoring_size(path: &Path) -> Self {
        let mut tab = Self::open(path);
        tab.size_unchecked = false;
        tab
    }

    /// Open `path` as an editable code tab whatever its file type, its buffer
    /// reserved empty and filled from the session's snapshot once the document
    /// registers. Backs a document the backend already holds as text -- one a
    /// workspace edit opened and changed -- which must stay editable (and its
    /// unsaved change reachable) even where [`Self::open`] would route the path
    /// to a too-large placeholder, a notebook preview, or a byte view.
    #[must_use]
    pub fn open_as_text(path: &Path) -> Self {
        open_pending_code(path, karet_filetype::file_type_for_path(path))
    }
}

/// The renderer `path`'s file type calls for, from its name alone.
///
/// A guess, and knowingly so: only the bytes can settle a lying extension, and
/// only the backend can see them. It is a good enough guess to reserve the right
/// tab, which is all it is asked to do.
#[must_use]
pub(crate) fn kind_from_path(path: &Path) -> FileKind {
    karet_filetype::classify_ignoring_size(path, &[])
}

/// Reserve the tab `kind` calls for, with no content in it yet.
///
/// A tab the client fills from raw bytes (a hex view, an image or PDF still
/// being read) carries [`fill_since`](Tab::fill_since): it renders blank, then a
/// muted loading line past the shared reveal delay, rather than as the
/// placeholder an unreadable file settles at. A placeholder nothing will fill
/// (a too-large verdict, a kind this build cannot render) is already the
/// answer, so it is not loading.
#[must_use]
pub(crate) fn reserve(path: &Path, kind: FileKind) -> Tab {
    let mut tab = reserve_kind(path, kind);
    if needs_bytes(kind) {
        tab.fill_since = Some(Pending::start());
    }
    tab
}

fn reserve_kind(path: &Path, kind: FileKind) -> Tab {
    match kind {
        // The session loads and decodes the text once the document registers;
        // the tab is reserved empty and fills from the first snapshot. Invalid
        // UTF-8 answers `NotUtf8`, which converts the tab to the hex fallback.
        FileKind::Text | FileKind::Markdown => Tab::open_as_text(path),
        // The backend decodes CBOR authoritatively once the document registers;
        // the tab is reserved empty and fills from the first snapshot. A corrupt
        // file answers `NotUtf8`, which converts the tab to the hex fallback.
        FileKind::Cbor => open_pending_code(
            path,
            karet_filetype::file_type_for_language("cbor")
                .unwrap_or_else(|| karet_filetype::file_type_for_path(path)),
        ),
        FileKind::Binary => Tab::new(
            title(path),
            TabKind::Hex {
                path: path.to_path_buf(),
                bytes: Vec::new(),
                scroll: 0,
            },
        ),
        // The backend converts DOCX to markdown (`DocumentCommand::Convert`);
        // the preview tab is reserved immediately and fills when it answers.
        #[cfg(feature = "docx")]
        FileKind::Docx => Tab::document_converting(path.to_path_buf()),
        // Same conversion seam: the backend renders the notebook to markdown.
        #[cfg(feature = "notebook")]
        FileKind::Notebook => Tab::document_converting(path.to_path_buf()),
        // Media is reserved as a placeholder and upgraded once its bytes arrive:
        // a placeholder is what an undecodable file settles at anyway, so the
        // failure path needs no separate state.
        other => placeholder(path, other, &[], 0),
    }
}

/// Build the tab `bytes` warrant, once the backend has delivered them.
///
/// The counterpart of [`reserve`]: the same routing, now with content. Only the
/// kinds the client renders itself appear here — text arrives through snapshots
/// instead.
#[must_use]
pub(crate) fn realize(path: &Path, kind: FileKind, bytes: Vec<u8>, len: u64) -> Tab {
    match kind {
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
        other => placeholder(path, other, &bytes, len),
    }
}

/// Whether `kind` renders from a document rather than from raw bytes.
#[must_use]
pub(crate) fn is_text_kind(kind: FileKind) -> bool {
    matches!(kind, FileKind::Text | FileKind::Markdown | FileKind::Cbor)
}

/// Whether `kind` reserves a preview the backend fills by converting the file
/// (`DocumentCommand::Convert`) in this build.
#[must_use]
pub(crate) fn is_converted_kind(kind: FileKind) -> bool {
    match kind {
        #[cfg(feature = "docx")]
        FileKind::Docx => true,
        #[cfg(feature = "notebook")]
        FileKind::Notebook => true,
        _ => false,
    }
}

/// Whether a tab of this kind needs the file's bytes fetched from the backend.
///
/// Text does not: it arrives as a document snapshot, already decoded, already
/// highlighted. These are the kinds the client renders from raw bytes itself,
/// because rendering them needs its cell grid and its graphics protocol.
#[must_use]
pub(crate) fn needs_bytes(kind: FileKind) -> bool {
    match kind {
        FileKind::Binary => true,
        #[cfg(feature = "images")]
        FileKind::Image => true,
        #[cfg(feature = "pdf")]
        FileKind::Pdf => true,
        _ => false,
    }
}

/// Reserve an editable code tab whose content the session loads and decodes
/// (text as is, CBOR → diagnostic notation): the buffer starts empty and fills
/// from the session's first snapshot once the document registers, with
/// highlighting deferred to the session worker.
fn open_pending_code(path: &Path, file_type: FileType) -> Tab {
    let mut tab = Tab::new(
        title(path),
        TabKind::Code {
            path: path.to_path_buf(),
            file_type,
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
    tab.fill_since = Some(Pending::start());
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use super::*;

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
        let tab = Tab::open(&file);
        assert!(matches!(tab.kind, TabKind::Code { .. }));
    }

    #[test]
    fn missing_text_path_opens_as_an_empty_code_tab() {
        let dir = temp_dir();
        let file = dir.path.join("NEW.md");

        let tab = Tab::open(&file);

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

        let tab = Tab::open(&alias);
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
        let tab = Tab::open(&file);
        assert_eq!(tab.fill_caret, Some(LineCol::default()));
        let TabKind::Code {
            file_type,
            text,
            doc,
            highlights,
            ..
        } = tab.kind
        else {
            panic!("expected a code tab");
        };
        assert_eq!(crate::tab::code_language(file_type), "Markdown");
        // Not decoded here: the session loads the file once and its first
        // snapshot supplies the text and, later, the syntax.
        assert!(text.is_empty());
        assert!(doc.is_none());
        assert!(highlights.all().is_empty());
    }

    /// Routing cannot see bytes, so a `.rs` full of binary still reserves a code
    /// tab. The correction comes from the backend, which answers `NotUtf8` and
    /// converts the tab to a hex view — the one place that judgement can be made.
    #[test]
    fn a_source_extension_reserves_a_code_tab_whatever_the_bytes_are() {
        let dir = temp_dir();
        let file = dir.path.join("bad.rs");
        let _ = std::fs::write(&file, b"fn main() {}\n\xff");
        let tab = Tab::open(&file);
        assert!(matches!(tab.kind, TabKind::Code { doc: None, .. }));
    }

    /// Opening reads nothing: a path that names another machine's file reserves
    /// the same tab a local one does.
    #[test]
    fn opening_reserves_by_name_without_touching_the_disk() {
        let tab = Tab::open(Path::new("/elsewhere/report.pdf"));
        assert!(matches!(
            tab.kind,
            TabKind::Placeholder {
                kind: FileKind::Pdf,
                ..
            }
        ));
        let hex = reserve(Path::new("/elsewhere/blob"), FileKind::Binary);
        assert!(matches!(hex.kind, TabKind::Hex { ref bytes, .. } if bytes.is_empty()));
        assert!(needs_bytes(FileKind::Binary));
        assert!(!needs_bytes(kind_from_path(Path::new("/elsewhere/a.rs"))));
    }

    #[test]
    fn opens_cbor_as_a_pending_code_tab_the_session_will_fill() {
        let dir = temp_dir();
        let file = dir.path.join("data.cbor");
        // 0x82 0x01 0x02: the CBOR array [1, 2].
        let _ = std::fs::write(&file, [0x82u8, 0x01, 0x02]);
        let tab = Tab::open(&file);
        let TabKind::Code {
            file_type,
            text,
            doc,
            ..
        } = tab.kind
        else {
            panic!("expected a code tab for a .cbor file");
        };
        assert_eq!(crate::tab::code_language(file_type), "CBOR");
        // The buffer is reserved empty; the session decodes authoritatively and
        // the first snapshot fills it (see the app save/open seam tests).
        assert!(text.is_empty());
        assert!(doc.is_none());
    }

    /// CBOR recognized by its content, not its name: the path alone reserves a
    /// code tab, whose document the session sniffs and decodes as CBOR; a
    /// classification that says CBOR reserves the same CBOR code tab.
    #[test]
    fn cbor_sniffed_from_content_is_reserved_as_an_editable_document() {
        let file = Path::new("/elsewhere/payload");
        assert!(matches!(Tab::open(file).kind, TabKind::Code { .. }));
        let tab = reserve(file, FileKind::Cbor);
        assert!(matches!(tab.kind, TabKind::Code { .. }));
        assert_eq!(tab.status_label(), "CBOR");
    }

    /// Only the machine holding the file knows how big it is, so the size guard
    /// is the backend's. The shell renders the verdict, and the override — the
    /// same file, with the renderer its content warrants — when asked for it.
    #[test]
    fn a_file_the_backend_reports_as_too_large_realizes_as_a_placeholder() {
        let file = Path::new("/elsewhere/big.bin");
        let len = karet_filetype::SIZE_GUARD + 1;

        let guarded = realize(file, FileKind::TooLarge { len }, Vec::new(), len);
        let overridden = realize(file, FileKind::Binary, vec![0, 0, 0], len);

        assert!(matches!(
            guarded.kind,
            TabKind::Placeholder {
                kind: FileKind::TooLarge { .. },
                ..
            }
        ));
        assert!(matches!(overridden.kind, TabKind::Hex { .. }));
    }

    #[test]
    fn opens_corrupt_cbor_as_a_pending_code_tab() {
        let dir = temp_dir();
        let file = dir.path.join("broken.cbor");
        // Truncated / invalid CBOR (a map header promising entries, with none).
        // Routing cannot know it is corrupt — the session's decode answers
        // `NotUtf8`, and the app's handler converts the tab to the hex fallback.
        let _ = std::fs::write(&file, [0xa1u8]);
        let tab = Tab::open(&file);
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
    fn a_pdf_becomes_a_document_tab_once_its_bytes_arrive() {
        let file = Path::new("/elsewhere/a.pdf");
        // Reserved from the name alone: a placeholder, which is also where an
        // unreadable PDF settles, so the failure path needs no separate state.
        assert!(matches!(Tab::open(file).kind, TabKind::Placeholder { .. }));

        let filled = realize(
            file,
            FileKind::Pdf,
            MINIMAL_PDF.to_vec(),
            MINIMAL_PDF.len() as u64,
        );
        let TabKind::Document { page_count, .. } = filled.kind else {
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
            Tab::open(&file).kind,
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
        let bytes = b"this is not a pdf at all".to_vec();
        let len = bytes.len() as u64;
        let tab = realize(&file, FileKind::Pdf, bytes, len);
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
        let tab = Tab::open(&file);
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
        let tab = Tab::open(&file);
        assert!(matches!(
            tab.kind,
            TabKind::MarkdownPreview {
                pending_since: Some(_),
                ..
            }
        ));
    }

    /// A name that says nothing about binary content reserves a document: the
    /// session answers `NotUtf8` for bytes that are not text, and the app turns
    /// the tab into the hex view then.
    #[test]
    fn an_opaque_name_reserves_a_document_for_the_session_to_judge() {
        let tab = Tab::open(Path::new("/elsewhere/blob.bin"));
        assert!(matches!(tab.kind, TabKind::Code { doc: None, .. }));
    }
}
