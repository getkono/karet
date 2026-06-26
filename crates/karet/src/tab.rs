//! Editor tabs: the open documents the main area can show.
//!
//! Each [`Tab`] carries a [`TabKind`] (the content + how to render it) plus an
//! [`EditorState`] used by code tabs for scroll/cursor. Diff and hex tabs keep
//! their own scroll inside the kind.

use std::path::{Path, PathBuf};

use karet_editor::EditorState;
use karet_syntax::Highlights;
use karet_text::TextBuffer;
use karet_widgets::image::Image;
use karet_widgets::viewer::FileKind;

use crate::render::FileView;

/// How a diff tab is laid out.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    /// One column: removals then additions.
    Unified,
    /// Two columns: old on the left, new on the right.
    SideBySide,
}

/// The content of a tab and how to render it.
pub enum TabKind {
    /// The landing page shown when nothing is open.
    Welcome,
    /// A read-only code/text view.
    Code {
        /// The file path.
        path: PathBuf,
        /// The display language name.
        language: &'static str,
        /// The loaded buffer.
        buffer: TextBuffer,
        /// Syntax highlight spans (empty when no grammar / disabled).
        highlights: Highlights,
    },
    /// A raster image.
    Image {
        /// The file path.
        path: PathBuf,
        /// The decoded image.
        image: Image,
    },
    /// A hex dump of binary content.
    Hex {
        /// The file path.
        path: PathBuf,
        /// The raw bytes.
        bytes: Vec<u8>,
        /// The first visible 16-byte row.
        scroll: usize,
    },
    /// A graceful placeholder (PDF, too-large, or undecodable image).
    Placeholder {
        /// The file path.
        path: PathBuf,
        /// Why it is not shown inline.
        kind: FileKind,
        /// Image dimensions, when known.
        dims: Option<(u32, u32)>,
        /// The file length in bytes.
        len: u64,
    },
    /// A single-file diff (opened from the Source Control panel).
    Diff {
        /// The prepared file diff.
        file: Box<FileView>,
        /// The current layout.
        view: ViewMode,
        /// Vertical scroll offset (display rows).
        scroll: u16,
    },
}

/// An open tab: a title, its content, and per-view editor state.
pub struct Tab {
    /// The tab title (usually a file name).
    pub title: String,
    /// The content + renderer.
    pub kind: TabKind,
    /// Code-tab scroll/cursor state.
    pub editor: EditorState,
}

impl Tab {
    /// Build a tab from a title and content.
    #[must_use]
    pub fn new(title: impl Into<String>, kind: TabKind) -> Self {
        Self {
            title: title.into(),
            kind,
            editor: EditorState::new(),
        }
    }

    /// The welcome tab.
    #[must_use]
    pub fn welcome() -> Self {
        Self::new("Welcome", TabKind::Welcome)
    }

    /// The file path backing this tab, if any.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match &self.kind {
            TabKind::Code { path, .. }
            | TabKind::Image { path, .. }
            | TabKind::Hex { path, .. }
            | TabKind::Placeholder { path, .. } => Some(path),
            TabKind::Diff { file, .. } => Some(&file.change.path),
            TabKind::Welcome => None,
        }
    }

    /// Whether this is a diff tab (enables diff-specific keys).
    #[must_use]
    pub fn is_diff(&self) -> bool {
        matches!(self.kind, TabKind::Diff { .. })
    }

    /// A short language/kind label for the status bar.
    #[must_use]
    pub fn language(&self) -> &str {
        match &self.kind {
            TabKind::Code { language, .. } => language,
            TabKind::Image { .. } => "image",
            TabKind::Hex { .. } => "binary",
            TabKind::Placeholder { .. } => "preview",
            TabKind::Diff { file, .. } => file.language,
            TabKind::Welcome => "",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn welcome_has_no_path() {
        let tab = Tab::welcome();
        assert!(tab.path().is_none());
        assert!(!tab.is_diff());
    }

    #[test]
    fn code_tab_reports_path_and_language() {
        let tab = Tab::new(
            "a.rs",
            TabKind::Code {
                path: PathBuf::from("/x/a.rs"),
                language: "Rust",
                buffer: TextBuffer::from_text("fn main() {}"),
                highlights: Highlights::default(),
            },
        );
        assert_eq!(tab.path(), Some(Path::new("/x/a.rs")));
        assert_eq!(tab.language(), "Rust");
    }
}
