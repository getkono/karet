//! What a tab is written in: the file type a code tab stores, and everything
//! read from it — the status label, the language configuration resolves
//! against, Markdown detection, and the default wrap.

use karet_filetype::FileType;
use karet_filetype::WrapMode;

use super::ChangedFile;
use super::Tab;
use super::TabKind;

impl Tab {
    /// The file type a code tab was opened as, or `None` for any other kind.
    #[must_use]
    pub(crate) fn file_type(&self) -> Option<FileType> {
        match &self.kind {
            TabKind::Code { file_type, .. } => Some(*file_type),
            _ => None,
        }
    }

    /// Whether this is a code tab editing a Markdown document.
    #[must_use]
    pub(crate) fn is_markdown(&self) -> bool {
        self.file_type()
            .is_some_and(|file_type| file_type.name() == "Markdown")
    }

    /// Whether this tab soft-wraps long lines: the configured `override_` when
    /// there is one, else the default its file type carries (only a code tab
    /// has one).
    #[must_use]
    pub(crate) fn word_wrap(&self, override_: Option<bool>) -> bool {
        override_.unwrap_or_else(|| {
            self.file_type()
                .is_some_and(|file_type| file_type.wrap_mode() == WrapMode::Wrap)
        })
    }

    /// The language editor configuration resolves against, or `None` for a tab
    /// with no label at all (the welcome tab).
    ///
    /// A code tab answers with its file type's display name. Every other kind
    /// answers with its [status label](Self::status_label), which is what it
    /// has always resolved against; which keys config resolution should use
    /// for them is #617's to settle, not this accessor's.
    #[must_use]
    pub(crate) fn language_id(&self) -> Option<&str> {
        let language = match &self.kind {
            TabKind::Code { file_type, .. } => code_language(*file_type),
            _ => self.status_label(),
        };
        (!language.is_empty()).then_some(language)
    }

    /// A short language/kind label for the status bar.
    #[must_use]
    pub fn status_label(&self) -> &str {
        match &self.kind {
            TabKind::Code { file_type, .. } => code_language(*file_type),
            TabKind::MarkdownPreview { .. } => "markdown",
            #[cfg(feature = "images")]
            TabKind::Image { .. } => "image",
            #[cfg(feature = "pdf")]
            TabKind::Document { .. } => "pdf",
            TabKind::Hex { .. } => "binary",
            TabKind::Placeholder { .. } => "preview",
            TabKind::LatexPreview { .. } => "latex preview",
            TabKind::Diff { file, .. } => file.as_deref().map_or("diff", ChangedFile::language),
            TabKind::StashPreview { .. } => "stash",
            TabKind::ServerLog { .. } => "log",
            TabKind::Seam(_) => "seams",
            TabKind::Graph { .. } => "graph",
            TabKind::LoadedConfig { .. } => "settings",
            TabKind::CommitLoading { .. } => "commit",
            TabKind::Commit { .. } => "commit",
            TabKind::Compare { .. } => "compare",
            TabKind::CommitGraph { .. } => "commits",
            TabKind::Welcome => "",
            TabKind::LanguageServers(_) => "language servers",
            #[cfg(feature = "terminal")]
            TabKind::Terminal(_) => "terminal",
        }
    }
}

/// The language name a code tab of `file_type` shows and resolves settings
/// against: the registry's display name, or `plaintext` for a file the
/// registry does not recognize.
#[must_use]
pub(crate) fn code_language(file_type: FileType) -> &'static str {
    if file_type.is_recognized() {
        file_type.name()
    } else {
        "plaintext"
    }
}

/// Whether `file_type` is CBOR: the one code-tab type whose document the
/// session decodes and saves in a format other than text.
#[must_use]
pub(crate) fn is_cbor(file_type: FileType) -> bool {
    karet_filetype::file_type_for_language("cbor") == Some(file_type)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn code_tab_reports_path_and_language() {
        let tab = Tab::open_as_text(Path::new("/x/a.rs"));
        assert_eq!(tab.path(), Some(Path::new("/x/a.rs")));
        assert_eq!(tab.status_label(), "Rust");
        assert_eq!(tab.language_id(), Some("Rust"));
    }

    #[test]
    fn an_unrecognized_code_tab_is_plaintext() {
        let tab = Tab::open_as_text(Path::new("/x/notes.unknownext"));
        assert_eq!(tab.status_label(), "plaintext");
        assert_eq!(tab.language_id(), Some("plaintext"));
    }

    /// Non-code kinds keep resolving settings against their status label until
    /// #617 decides otherwise; only the label-less welcome tab answers `None`.
    #[test]
    fn non_code_tabs_resolve_against_their_label() {
        let stash = Tab::stash_preview("stash@{0}", String::new());
        assert_eq!(stash.status_label(), "stash");
        assert_eq!(stash.language_id(), Some("stash"));
        assert_eq!(Tab::welcome().status_label(), "");
        assert_eq!(Tab::welcome().language_id(), None);
    }

    #[test]
    fn markdown_and_wrap_follow_the_stored_file_type() {
        let markdown = Tab::open_as_text(Path::new("/x/notes.md"));
        let rust = Tab::open_as_text(Path::new("/x/main.rs"));
        assert!(markdown.is_markdown());
        assert!(!rust.is_markdown());
        assert!(!Tab::welcome().is_markdown());
        assert!(markdown.word_wrap(None));
        assert!(!rust.word_wrap(None));
        assert!(!markdown.word_wrap(Some(false)));
        assert!(rust.word_wrap(Some(true)));
        assert!(!Tab::welcome().word_wrap(None));
    }
}
