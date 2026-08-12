//! Which parts of a file spell-checking is allowed to look at.
//!
//! Both passes ask these questions — the per-document worker in [`super`] as it
//! classifies each word, and the workspace scan in [`crate::spell_scan`] as it
//! decides whether a file is worth parsing at all. They lived as two independent
//! copies of the same language list; keeping one copy is what stops the panel and
//! the editor from quietly disagreeing about what counts as prose.

use crate::config::schema::Spellcheck;

/// Whether `language` names a prose format, whose *whole* body is checked under
/// `spellcheck.documents` rather than only its comments and strings.
///
/// The names are `karet-filetype`'s display names, matched case-insensitively.
pub(crate) fn is_prose_document(language: Option<&str>) -> bool {
    language.is_some_and(|language| {
        matches!(
            language.to_ascii_lowercase().as_str(),
            "markdown" | "plain text" | "asciidoc" | "restructuredtext" | "tex"
        )
    })
}

/// Whether any scope this file could contribute is enabled — the cheap gate that
/// skips parsing a source file outright when comments, strings, and identifiers
/// are all off.
pub(crate) fn can_match(language: Option<&str>, settings: &Spellcheck) -> bool {
    if is_prose_document(language) {
        return settings.documents;
    }
    settings.comments || settings.strings || settings.identifiers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(comments: bool, strings: bool, identifiers: bool, documents: bool) -> Spellcheck {
        Spellcheck {
            comments,
            strings,
            identifiers,
            documents,
            ..Spellcheck::default()
        }
    }

    #[test]
    fn prose_document_names_match_the_filetype_registry() {
        for language in [
            "Markdown",
            "Plain Text",
            "reStructuredText",
            "AsciiDoc",
            "TeX",
        ] {
            assert!(is_prose_document(Some(language)), "{language}");
        }
        assert!(!is_prose_document(Some("Rust")));
        assert!(!is_prose_document(None));
    }

    #[test]
    fn source_files_are_skipped_when_every_source_scope_is_off() {
        assert!(!can_match(
            Some("Rust"),
            &settings(false, false, false, true)
        ));
        assert!(can_match(
            Some("Rust"),
            &settings(true, false, false, false)
        ));
        assert!(can_match(
            Some("Rust"),
            &settings(false, true, false, false)
        ));
        assert!(can_match(
            Some("Rust"),
            &settings(false, false, true, false)
        ));
    }

    #[test]
    fn prose_files_follow_the_documents_toggle_alone() {
        assert!(can_match(
            Some("Markdown"),
            &settings(false, false, false, true)
        ));
        assert!(!can_match(
            Some("Markdown"),
            &settings(true, true, true, false)
        ));
        // An unrecognized language is treated as source, not prose.
        assert!(!can_match(None, &settings(false, false, false, true)));
    }
}
