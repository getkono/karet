//! Workspace helpers: opening a file into the right tab, and collecting the files
//! shown in the explorer's quick-open list.
//!
//! These call the engines directly (`karet-text`, `karet-treesitter`,
//! `karet-syntax`, `karet-widgets`). Routing through `karet-session` is a deferred
//! step; its `Command`/`Event` variants already map onto this flow.

use std::path::Path;
use std::path::PathBuf;

use karet_syntax::Highlighter;
use karet_syntax::Highlights;
use karet_text::TextBuffer;
use karet_treesitter::ParserPool;
use karet_treesitter::SyntaxTree;
use karet_treesitter::language_id_from_path;
use karet_treesitter::language_name_from_path;
use karet_widgets::image;
use karet_widgets::viewer::FileKind;
use karet_widgets::viewer::{self};

use crate::tab::Tab;
use crate::tab::TabKind;

/// How many leading bytes to sample for file-type classification.
const HEAD_BYTES: usize = 8192;

/// Open `path` as a tab, classifying its content and choosing a renderer. Failures
/// degrade gracefully to a placeholder rather than erroring.
#[must_use]
pub fn open_file(path: &Path, syntax: bool) -> Tab {
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let bytes = std::fs::read(path).unwrap_or_default();
    let head = &bytes[..bytes.len().min(HEAD_BYTES)];
    let kind = viewer::classify(path, head, len);
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
        FileKind::Binary => Tab::new(
            title(path),
            TabKind::Hex {
                path: path.to_path_buf(),
                bytes,
                scroll: 0,
            },
        ),
        FileKind::Pdf | FileKind::TooLarge { .. } => placeholder(path, kind, &bytes, len),
        // `FileKind` is `#[non_exhaustive]`; route any future kind to a placeholder.
        _ => placeholder(path, kind, &bytes, len),
    }
}

/// Build a read-only code/text tab, highlighting when a grammar is available.
fn open_text(path: &Path, bytes: &[u8], syntax: bool) -> Tab {
    let text = String::from_utf8_lossy(bytes).into_owned();
    let language = language_name_from_path(path).unwrap_or("plaintext");
    let highlights = if syntax {
        highlight(path, &text)
    } else {
        Highlights::default()
    };
    let buffer = TextBuffer::from_text(&text);
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
            decos: Vec::new(),
        },
    )
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

/// Build a graceful placeholder tab (PDF / too-large / undecodable image).
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
