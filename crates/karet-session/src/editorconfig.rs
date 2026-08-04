//! EditorConfig core integration and translation into karet's document behavior.

use std::path::Path;

use ec4rs::property::Charset;
use ec4rs::property::EndOfLine;
use ec4rs::property::FinalNewline;
use ec4rs::property::IndentSize;
use ec4rs::property::IndentStyle;
use ec4rs::property::SpellingLanguage as EditorConfigSpellingLanguage;
use ec4rs::property::TabWidth;
use ec4rs::property::TrimTrailingWs;

use crate::api::DocumentEncoding;
use crate::api::DocumentLineEnding;
use crate::api::DocumentSettings;
use crate::api::SpellingLanguage;
use crate::config::Settings;
use crate::config::schema::Eol;

/// Resolve application/language defaults, then overlay every matching
/// `.editorconfig` from the filesystem root down to `path`.
pub(crate) fn resolve(
    path: &Path,
    language: Option<&str>,
    settings: &Settings,
) -> Result<DocumentSettings, ec4rs::Error> {
    let mut resolved = defaults(language, settings);
    let mut properties = ec4rs::properties_of(path)?;
    properties.use_fallbacks();

    apply(&mut resolved, &properties);
    // EditorConfig selects the dictionary for projects that opt in globally; it
    // must not silently turn the feature on for an otherwise opted-out user.
    if !settings.spellcheck.enabled {
        resolved.spelling_language = None;
    }
    Ok(resolved)
}

/// Resolve karet's own global and language-specific defaults without reading disk.
pub(crate) fn defaults(language: Option<&str>, settings: &Settings) -> DocumentSettings {
    let editor = settings.editor.for_language(language);
    DocumentSettings {
        insert_spaces: editor.insert_spaces(),
        indent_size: u16::from(editor.tab_size()),
        tab_width: u16::from(editor.tab_size()),
        trim_trailing_whitespace: editor.trim_trailing_whitespace(),
        insert_final_newline: editor.insert_final_newline(),
        line_ending: match settings.files.eol {
            Eol::Auto => None,
            Eol::Lf => Some(DocumentLineEnding::Lf),
            Eol::Crlf => Some(DocumentLineEnding::Crlf),
        },
        encoding: None,
        spelling_language: settings
            .spellcheck
            .enabled
            .then(|| SpellingLanguage::parse(&settings.spellcheck.language))
            .flatten(),
    }
}

fn apply(resolved: &mut DocumentSettings, properties: &ec4rs::Properties) {
    if let Ok(style) = properties.get::<IndentStyle>() {
        resolved.insert_spaces = style == IndentStyle::Spaces;
    }
    if let Ok(TabWidth::Value(width)) = properties.get::<TabWidth>()
        && let Ok(width) = u16::try_from(width)
    {
        resolved.tab_width = width;
    }
    match properties.get::<IndentSize>() {
        Ok(IndentSize::Value(size)) => {
            if let Ok(size) = u16::try_from(size) {
                resolved.indent_size = size;
            }
        },
        Ok(IndentSize::UseTabWidth) => resolved.indent_size = resolved.tab_width,
        Err(_) => {},
    }
    if let Ok(TrimTrailingWs::Value(value)) = properties.get::<TrimTrailingWs>() {
        resolved.trim_trailing_whitespace = value;
    }
    if let Ok(FinalNewline::Value(value)) = properties.get::<FinalNewline>() {
        resolved.insert_final_newline = value;
    }
    if let Ok(eol) = properties.get::<EndOfLine>() {
        resolved.line_ending = match eol {
            EndOfLine::Lf => Some(DocumentLineEnding::Lf),
            EndOfLine::CrLf => Some(DocumentLineEnding::Crlf),
            // The in-memory engine intentionally supports only LF/CRLF. Per the
            // plugin rules, an unsupported value leaves the editor default intact.
            EndOfLine::Cr => resolved.line_ending,
        };
    }
    if let Ok(charset) = properties.get::<Charset>() {
        resolved.encoding = match charset {
            Charset::Utf8 => Some(DocumentEncoding::Utf8),
            Charset::Utf8Bom => Some(DocumentEncoding::Utf8Bom),
            // karet edits UTF-8. Other valid core values are unsupported plugin
            // values and therefore do not override the detected encoding.
            Charset::Latin1 | Charset::Utf16Le | Charset::Utf16Be => resolved.encoding,
        };
    }
    if let Ok(EditorConfigSpellingLanguage::Value(language)) =
        properties.get::<EditorConfigSpellingLanguage>()
    {
        resolved.spelling_language = SpellingLanguage::parse(&language.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_files_follow_root_globs_precedence_and_unset()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let nested = dir.path().join("src");
        std::fs::create_dir_all(&nested)?;
        std::fs::write(
            dir.path().join(".editorconfig"),
            "root = true\n[*]\nindent_size = 2\ntrim_trailing_whitespace = false\n",
        )?;
        std::fs::write(
            nested.join(".editorconfig"),
            "[*.rs]\nindent_size = unset\ninsert_final_newline = false\n",
        )?;
        let file = nested.join("main.rs");
        std::fs::write(&file, "fn main() {}\n")?;
        let mut settings = Settings::default();
        settings.editor.tab_size = 7;

        let resolved = resolve(&file, Some("Rust"), &settings)?;

        assert_eq!(resolved.indent_size, 7, "unset reveals the editor default");
        assert!(
            !resolved.trim_trailing_whitespace,
            "parent glob still applies"
        );
        assert!(!resolved.insert_final_newline, "nearer file wins last");
        Ok(())
    }

    #[test]
    fn standard_supported_pairs_translate_to_document_behavior()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n")?;
        std::fs::write(
            dir.path().join(".editorconfig"),
            "root = TRUE\n[*.rs]\nindent_style = TAB\nindent_size = 6\n\
             tab_width = 4\nend_of_line = crlf\ncharset = utf-8-bom\n\
             trim_trailing_whitespace = false\ninsert_final_newline = false\n",
        )?;

        let resolved = resolve(&file, Some("Rust"), &Settings::default())?;

        assert_eq!(
            resolved,
            DocumentSettings {
                insert_spaces: false,
                indent_size: 6,
                tab_width: 4,
                trim_trailing_whitespace: false,
                insert_final_newline: false,
                line_ending: Some(DocumentLineEnding::Crlf),
                encoding: Some(DocumentEncoding::Utf8Bom),
                spelling_language: None,
            }
        );
        Ok(())
    }

    #[test]
    fn invalid_editorconfig_is_reported_without_hiding_its_source()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "")?;
        let config = dir.path().join(".editorconfig");
        std::fs::write(&config, "[*.rs]\nthis is not valid\n")?;

        let error = resolve(&file, Some("Rust"), &Settings::default())
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();

        assert!(error.contains(".editorconfig:2"), "{error}");
        Ok(())
    }

    #[test]
    fn spelling_language_overrides_enabled_application_default()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("notes.md");
        std::fs::write(&file, "text\n")?;
        std::fs::write(
            dir.path().join(".editorconfig"),
            "root = true\n[*.md]\nspelling_language = en-GB\n",
        )?;
        let mut settings = Settings::default();
        settings.spellcheck.enabled = true;

        let resolved = resolve(&file, Some("Markdown"), &settings)?;

        assert_eq!(
            resolved.spelling_language,
            Some(SpellingLanguage::EnglishUnitedKingdom)
        );
        Ok(())
    }

    #[test]
    fn spelling_language_does_not_enable_spellcheck() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("notes.md");
        std::fs::write(&file, "text\n")?;
        std::fs::write(
            dir.path().join(".editorconfig"),
            "root = true\n[*.md]\nspelling_language = en-GB\n",
        )?;

        let resolved = resolve(&file, Some("Markdown"), &Settings::default())?;

        assert_eq!(resolved.spelling_language, None);
        Ok(())
    }
}
