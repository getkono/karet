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

    /// Formatting must never change what the document *means*, and a second
    /// pass must find nothing left to do.
    #[test]
    fn formatting_preserves_values_and_settles_in_one_pass() {
        for case in [
            "a=1",
            "[a.b.c]\nx=1",
            "[[t]]\nx=1\n[[t]]\nx=2",
            "s='''\nmulti\nline\n'''",
            "a.b.c = 1",
            "t = { x = 1, y = [1,2,{z=3}] }",
            "# only a comment\n",
            "k = 2026-08-21T00:00:00Z",
            "'quoted key' = 1",
            "日本 = \"語\"",
            "arr = [\n 1, # one\n 2, # two\n]",
            "[ a . b ]\nx=1",
            "b = 0b1010\no = 0o755\nh = 0xDEADBEEF",
        ] {
            let Ok(before) = case.parse::<toml::Table>() else {
                continue;
            };
            let once = format_toml(case, &[]).unwrap_or_else(|| case.to_owned());
            let after = once.parse::<toml::Table>();
            assert!(
                after.is_ok(),
                "formatted output no longer parses:\n{case}\n---\n{once}"
            );
            assert_eq!(Ok(before), after, "value changed:\n{case}\n---\n{once}");
            assert_eq!(
                format_toml(&once, &[]),
                None,
                "a second pass still wants changes:\n{once}"
            );
        }
    }

    #[test]
    fn comments_and_structure_survive() {
        let text = "# top\n[a]\nx = 1 # keep\n\n[b]\ny = 2\n";
        assert_eq!(format_toml(text, &[]), None);
    }
}
