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

    /// The file type editor configuration resolves against
    /// ([`Editor::for_language`](karet_session::config::schema::Editor::for_language)),
    /// whose `config_selector` picks the `[language]` override: the same key
    /// the session resolves the document's own settings with, so an override
    /// such as `[toml]` reaches a `Cargo.lock` here as it does there.
    ///
    /// - A code tab answers with the file type it was opened as.
    /// - A diff tab answers with its file's path-detected type: it shows that
    ///   file's text, so a `[rust]` patch applies to a Rust diff.
    /// - A Markdown preview answers with Markdown: it shows Markdown converted
    ///   from its source (a DOCX, a notebook), never the source's own text.
    /// - Every other kind (image, PDF, hex, logs, commit and graph views, …)
    ///   shows no document text and answers `None`, the global settings.
    #[must_use]
    pub(crate) fn config_file_type(&self) -> Option<FileType> {
        match &self.kind {
            TabKind::Code { file_type, .. } => Some(*file_type),
            TabKind::Diff { path, .. } => Some(karet_filetype::file_type_for_path(path)),
            TabKind::MarkdownPreview { .. } => karet_filetype::file_type_for_language("markdown"),
            _ => None,
        }
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

/// The language name a code tab of `file_type` shows in its status label and
/// hands the language-server badge and routing: the registry's display name, or
/// `plaintext` for a file the registry does not recognize. Editor settings do
/// not resolve against it; they go through [`Tab::config_file_type`].
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
        assert_eq!(
            tab.config_file_type().and_then(|ft| ft.config_selector()),
            Some("rust")
        );
    }

    #[test]
    fn an_unrecognized_code_tab_is_plaintext() {
        let tab = Tab::open_as_text(Path::new("/x/notes.unknownext"));
        assert_eq!(tab.status_label(), "plaintext");
        let file_type = tab.config_file_type();
        assert!(file_type.is_some_and(|ft| !ft.is_recognized()));
        assert_eq!(file_type.and_then(|ft| ft.config_selector()), None);
    }

    /// A `Cargo.lock` tab is labelled by its display name but configured by its
    /// selector. Falsified by resolving app settings against the label (#617).
    #[test]
    fn a_cargo_lock_tab_configures_as_toml() {
        let tab = Tab::open_as_text(Path::new("/x/Cargo.lock"));
        assert_eq!(tab.status_label(), "Cargo lockfile");
        assert_eq!(
            tab.config_file_type().and_then(|ft| ft.config_selector()),
            Some("toml")
        );
    }

    /// A diff configures as its file and a Markdown preview as Markdown; kinds
    /// that show no document text resolve to the global settings, whatever their
    /// status label happens to spell.
    #[test]
    fn non_code_tabs_pick_an_explicit_config_file_type() {
        let selector = |tab: &Tab| tab.config_file_type().and_then(|ft| ft.config_selector());
        let diff = Tab::diff(
            "Cargo.lock".to_owned(),
            std::path::PathBuf::from("/x/Cargo.lock"),
            super::super::Section::Working,
            None,
            super::super::ViewMode::Unified,
        );
        assert_eq!(selector(&diff), Some("toml"));
        let preview = Tab::document_preview(std::path::PathBuf::from("/x/report.docx"), "# Hi");
        assert_eq!(selector(&preview), Some("markdown"));
        let stash = Tab::stash_preview("stash@{0}", String::new());
        assert_eq!(stash.status_label(), "stash");
        assert_eq!(stash.config_file_type(), None);
        assert_eq!(Tab::welcome().config_file_type(), None);
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
