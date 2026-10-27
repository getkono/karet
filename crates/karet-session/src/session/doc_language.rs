//! What an open document is written in: every language identity it carries,
//! resolved together on open and on rename (split from `documents` for the
//! file-size ceiling).

use super::*;

/// The registry selector of the language a decoded CBOR document carries.
const CBOR_LANGUAGE: &str = "cbor";

/// Every language identity a document carries, resolved together so the
/// highlighter, the display name, per-language settings, and the language server
/// never disagree about what the document is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DocumentLanguage {
    /// The tree-sitter grammar that parses the document, when one is compiled in.
    pub(super) lang_id: Option<LanguageId>,
    /// The display name (e.g. `"Rust"`).
    pub(super) name: Option<&'static str>,
    /// The registry file type: per-language settings resolve against it, and its
    /// [`config_selector`](karet_filetype::FileType::config_selector) routes the
    /// document to a language server.
    pub(super) file_type: karet_filetype::FileType,
    /// The protocol `languageId` announced to the language server.
    pub(super) lsp_id: Option<&'static str>,
}

impl DocumentLanguage {
    /// The language `path` is detected as.
    pub(super) fn for_path(path: &Path) -> Self {
        Self {
            lang_id: language_id_from_path(path),
            name: language_name_for_path(path),
            file_type: karet_filetype::file_type_for_path(path),
            lsp_id: lsp_language_id_for_path(path),
        }
    }

    /// The per-language configuration and server-routing selector.
    pub(super) fn selector(&self) -> Option<&'static str> {
        self.file_type.config_selector()
    }

    /// The language of a document loaded from `path` in `format`, given an
    /// explicit `id` (see [`Self::resolve`]).
    ///
    /// A CBOR document is CBOR whatever its path says: it may have been
    /// recognized by content (an extensionless `payload`) or kept its format
    /// through a rename, and its buffer is diagnostic notation either way. The
    /// client's tab types it the same way, so both sides resolve the same
    /// `[cbor]` override.
    pub(super) fn for_format(path: &Path, format: DocFormat, id: Option<&str>) -> Self {
        let id = if format.is_cbor() {
            Some(CBOR_LANGUAGE)
        } else {
            id
        };
        Self::resolve(path, id)
    }

    /// The language `doc` takes when renamed to `path`: the new path decides, as
    /// it would on a fresh open, within the format the document was loaded in,
    /// which a rename does not change. A CBOR document stays CBOR, and a text
    /// document renamed to a CBOR path is still text, so it keeps the language
    /// it had. The client's tab retypes itself by the same rule.
    pub(super) fn for_rename(doc: &Document, path: &Path) -> Self {
        let renamed = Self::for_format(path, doc.format, None);
        if !doc.format.is_cbor() && renamed.selector() == Some(CBOR_LANGUAGE) {
            return Self {
                lang_id: doc.lang_id,
                name: doc.language,
                file_type: doc.file_type,
                lsp_id: doc.lsp_language_id,
            };
        }
        renamed
    }

    /// The language an explicit `id` names (an LSP language id such as
    /// `"typescriptreact"`, a configuration selector such as `"c++"`, or a display
    /// name such as `"Rust"`), falling back to detection from `path` when `id` is
    /// `None` or names no known language.
    pub(super) fn resolve(path: &Path, id: Option<&str>) -> Self {
        let Some(file_type) = id.and_then(karet_filetype::file_type_for_language) else {
            return Self::for_path(path);
        };
        Self {
            lang_id: file_type
                .grammar()
                .and_then(karet_treesitter::language_id_from_injection_name),
            name: Some(file_type.name()),
            file_type,
            lsp_id: file_type.lsp_language_id(),
        }
    }
}
