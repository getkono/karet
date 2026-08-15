//! The built-in TOML formatter (feature `toml-format`): `taplo`'s lossless
//! formatter with the workspace's `.taplo.toml` options, used when no
//! language server offers formatting for a TOML document.

use std::path::PathBuf;

/// Format `text`, honoring the first `.taplo.toml`/`taplo.toml` found in
/// `roots`. `None` when the text is already formatted.
pub(crate) fn format_toml(text: &str, roots: &[PathBuf]) -> Option<String> {
    let mut options = taplo::formatter::Options::default();
    if let Some(incomplete) = workspace_options(roots) {
        options.update(incomplete);
    }
    let formatted = taplo::formatter::format(text, options);
    (formatted != text).then_some(formatted)
}

/// The `[formatting]` table of the workspace's taplo configuration, so karet
/// formats exactly as the project's CLI/CI does.
fn workspace_options(roots: &[PathBuf]) -> Option<taplo::formatter::OptionsIncomplete> {
    for root in roots {
        for name in [".taplo.toml", "taplo.toml"] {
            let Ok(text) = std::fs::read_to_string(root.join(name)) else {
                continue;
            };
            let Ok(value) = text.parse::<toml::Value>() else {
                continue;
            };
            if let Some(formatting) = value.get("formatting")
                && let Ok(options) = formatting.clone().try_into()
            {
                return Some(options);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_normalizes_and_reports_only_changes() {
        let messy = "[package]\nname=\"x\"\n";
        let formatted = format_toml(messy, &[]);
        assert_eq!(formatted.as_deref(), Some("[package]\nname = \"x\"\n"));
        // Already-clean text reports nothing to do.
        assert_eq!(format_toml("[package]\nname = \"x\"\n", &[]), None);
    }

    #[test]
    fn comments_and_structure_survive() {
        let text = "# top\n[a]\nx = 1 # keep\n\n[b]\ny = 2\n";
        assert_eq!(format_toml(text, &[]), None);
    }
}
