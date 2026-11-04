//! The static catalogue of recognized file types and the path → [`FileType`]
//! resolver.
//!
//! One table ([`table`]) is the single source of truth, keyed by well-known
//! **filename** (matched first, case-insensitively) and by lowercase
//! **extension**. Adding a format is a one-row edit there; see
//! [`docs/file-formats.md`] for the rendered catalogue.
//!
//! [`docs/file-formats.md`]: https://github.com/getkono/karet/blob/master/docs/file-formats.md

use std::path::Path;

use crate::comment::CommentTokens;
use crate::icon::Category;
use crate::icon::IconStyle;

#[cfg(test)]
mod structured_tests;
mod table;

use table::REGISTRY;

/// The default long-line behavior for an editable file type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum WrapMode {
    /// Soft-wrap long lines to the editor viewport.
    Wrap,
    /// Keep logical lines intact and allow horizontal scrolling.
    Overflow,
}

/// Static presentation metadata for one recognized file type.
///
/// Resolve one from a path with [`file_type_for_path`]. Icons are resolved per
/// [`IconStyle`] via [`FileType::icon`]: Nerd Font uses a per-type glyph (falling
/// back to the [`Category`]'s glyph), while the Unicode/ASCII tiers use the
/// category's fallback glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileType {
    name: &'static str,
    grammar: Option<&'static str>,
    lsp_language_id: Option<&'static str>,
    config_selector: Option<&'static str>,
    category: Category,
    nerd: Option<char>,
    extensions: &'static [&'static str],
    filenames: &'static [&'static str],
    wrap_mode: WrapMode,
    comments: CommentTokens,
    pairs: &'static [(char, char)],
}

impl FileType {
    /// This row with its comment tokens and auto-closing pairs set (a row that
    /// never calls it has no comment syntax and closes brackets and double
    /// quotes only).
    const fn syntax(mut self, comments: CommentTokens, pairs: &'static [(char, char)]) -> Self {
        self.comments = comments;
        self.pairs = pairs;
        self
    }

    /// The human-readable display name (e.g. `"Rust"`, `"Markdown"`).
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The tree-sitter grammar identity for this file type, when syntax parsing
    /// is meaningful. This is independent from the display name and does not
    /// imply that the grammar was compiled into a consumer's feature set.
    #[must_use]
    pub fn grammar(&self) -> Option<&'static str> {
        self.grammar
    }

    /// The protocol `languageId` sent to language servers for this file type.
    #[must_use]
    pub fn lsp_language_id(&self) -> Option<&'static str> {
        self.lsp_language_id
    }

    /// The stable selector used by per-language configuration and server routing.
    ///
    /// Selectors intentionally preserve the keys accepted before language
    /// identities were separated (for example, `"c++"`, `"c#"`, and `"tsx"`).
    #[must_use]
    pub fn config_selector(&self) -> Option<&'static str> {
        self.config_selector
    }

    /// The coarse [`Category`] of this file type.
    #[must_use]
    pub fn category(&self) -> Category {
        self.category
    }

    /// Whether this is a recognized type (as opposed to the `"File"` fallback
    /// returned for unknown paths).
    #[must_use]
    pub fn is_recognized(&self) -> bool {
        !matches!(self.category, Category::Unknown)
    }

    /// The icon glyph for this file type in the given [`IconStyle`].
    #[must_use]
    pub fn icon(&self, style: IconStyle) -> char {
        match style {
            IconStyle::NerdFont => self.nerd.unwrap_or(self.category.nerd_icon()),
            IconStyle::Unicode => self.category.unicode_icon(),
            IconStyle::Ascii => self.category.ascii_icon(),
        }
    }

    /// The default long-line behavior for this file type.
    #[must_use]
    pub fn wrap_mode(&self) -> WrapMode {
        self.wrap_mode
    }

    /// The line/block comment tokens a "toggle comment" command uses for this
    /// file type ([`CommentTokens::NONE`] for data formats and unknown files).
    #[must_use]
    pub fn comment_tokens(&self) -> CommentTokens {
        self.comments
    }

    /// The `(open, close)` bracket and quote pairs an editor auto-closes for this
    /// file type: `()`, `[]` and `{}` everywhere, plus the quotes that are always
    /// quotes in it (no `'` in Rust, whose lifetimes start with one, or in prose).
    /// A pair with the same character twice is a quote.
    #[must_use]
    pub fn auto_pairs(&self) -> &'static [(char, char)] {
        self.pairs
    }
}

/// The fallback for an unrecognized file.
const UNKNOWN: FileType = FileType {
    name: "File",
    grammar: None,
    lsp_language_id: None,
    config_selector: None,
    category: Category::Unknown,
    nerd: None,
    extensions: &[],
    filenames: &[],
    wrap_mode: WrapMode::Overflow,
    comments: CommentTokens::NONE,
    pairs: crate::pairs::DOUBLE,
};

/// Resolve a path to its [`FileType`], or the `"File"` fallback when unrecognized.
///
/// Matches a well-known filename first (case-insensitively), then a lowercase
/// extension.
#[must_use]
pub fn file_type_for_path(path: &Path) -> FileType {
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        for entry in REGISTRY {
            if entry.filenames.iter().any(|f| f.eq_ignore_ascii_case(name)) {
                return *entry;
            }
        }
    }
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext = ext.to_ascii_lowercase();
        for entry in REGISTRY {
            if entry.extensions.iter().any(|e| *e == ext) {
                return *entry;
            }
        }
    }
    UNKNOWN
}

/// Every registered [`FileType`], in registry order.
///
/// This is the whole catalogue — for consumers that audit or enumerate the
/// vocabulary (e.g. the cross-registry consistency test in `karet-treesitter`)
/// rather than resolve a single path.
#[must_use]
pub fn all_file_types() -> &'static [FileType] {
    REGISTRY
}

/// Resolve an explicit language identifier to its [`FileType`], or `None` when
/// no recognized type answers to it.
///
/// `id` is matched, ASCII case-insensitively, against each type's
/// [`FileType::lsp_language_id`] (e.g. `"typescriptreact"`), then its
/// [`FileType::config_selector`] (e.g. `"tsx"`, `"c++"`), then its
/// [`FileType::name`] (e.g. `"Rust"`). Each tier scans the whole registry
/// before the next is tried, so a protocol id never loses to a display name
/// that happens to spell it; within a tier the first entry in registry order
/// wins. The unrecognized `"File"` fallback is never returned.
#[must_use]
pub fn file_type_for_language(id: &str) -> Option<FileType> {
    let tiers: [fn(&FileType) -> Option<&'static str>; 3] = [
        FileType::lsp_language_id,
        FileType::config_selector,
        |file_type| Some(file_type.name()),
    ];
    tiers.iter().find_map(|identity| {
        REGISTRY
            .iter()
            .filter(|file_type| file_type.is_recognized())
            .find(|file_type| identity(file_type).is_some_and(|key| key.eq_ignore_ascii_case(id)))
            .copied()
    })
}

/// The icon glyph for a path in the given [`IconStyle`] — a convenience wrapper
/// over [`file_type_for_path`] + [`FileType::icon`].
#[must_use]
pub fn icon_for_path(path: &Path, style: IconStyle) -> char {
    file_type_for_path(path).icon(style)
}

/// The coarse [`Category`] for a path — a convenience wrapper over
/// [`file_type_for_path`] + [`FileType::category`], used by renderers to tint icons.
#[must_use]
pub fn category_for_path(path: &Path) -> Category {
    file_type_for_path(path).category()
}

#[cfg(test)]
#[path = "registry/tests.rs"]
mod tests;
