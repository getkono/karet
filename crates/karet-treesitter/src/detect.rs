//! File-extension → language detection.
//!
//! Two layers so a viewer can label a file even when its grammar isn't built in:
//! [`language_id_from_path`] resolves only grammars compiled into this build, while
//! [`language_name_from_path`] also recognizes common languages without a bundled
//! grammar (for the UI label), falling back to "plaintext" rendering.

use std::path::Path;

use crate::LanguageId;
use crate::registry;

/// The [`LanguageId`] of a bundled grammar for `path`'s extension, if one is
/// compiled in. `None` means the caller should render plaintext.
#[must_use]
pub fn language_id_from_path(path: &Path) -> Option<LanguageId> {
    let ext = extension(path)?;
    registry::all()
        .iter()
        .find(|g| g.extensions.contains(&ext.as_str()))
        .map(|g| g.id)
}

/// A human-readable language name for `path`, for UI labels.
///
/// Prefers a bundled grammar's name; otherwise defers to the shared
/// [`karet_filetype`] catalogue (so the display-name table lives in one place).
/// `None` for unrecognized files (the caller should show "plaintext").
#[must_use]
pub fn language_name_from_path(path: &Path) -> Option<&'static str> {
    if let Some(ext) = extension(path)
        && let Some(g) = registry::all()
            .iter()
            .find(|g| g.extensions.contains(&ext.as_str()))
    {
        return Some(g.name);
    }
    let ft = karet_filetype::file_type_for_path(path);
    ft.is_recognized().then_some(ft.name())
}

/// Lowercased extension of `path`, without the dot.
fn extension(path: &Path) -> Option<String> {
    path.extension()?.to_str().map(str::to_ascii_lowercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_extension_is_unlabelled() {
        let p = Path::new("file.unknownext");
        assert_eq!(language_id_from_path(p), None);
        assert_eq!(language_name_from_path(p), None);
    }

    #[test]
    fn non_compiled_language_still_named() {
        // Kotlin has no bundled grammar but is still recognized for labelling.
        let p = Path::new("Main.kt");
        assert_eq!(language_id_from_path(p), None);
        assert_eq!(language_name_from_path(p), Some("Kotlin"));
    }

    #[test]
    fn extension_is_case_insensitive() {
        assert_eq!(extension(Path::new("X.MD")).as_deref(), Some("md"));
        assert_eq!(
            language_name_from_path(Path::new("README.MD")),
            Some("Markdown")
        );
    }
}
