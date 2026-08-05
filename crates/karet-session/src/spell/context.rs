//! Language-aware exclusions for structural string literals.

use karet_core::StandardToken;
use karet_core::TokenId;
use karet_syntax::Highlights;

// First-pass inline call/macro vocabulary. Keep this deliberately narrow: ordinary
// formatting and logging macros carry user-facing prose and must remain checked.
// TODO: replace these textual contexts with grammar queries when spell jobs carry
// layered syntax trees rather than only flattened highlight spans.
const NON_PROSE_CALLS: &[&str] = &[
    "__import__",
    "cfg",
    "cfg_attr",
    "embed",
    "env",
    "import",
    "include",
    "include_bytes",
    "include_str",
    "load",
    "option_env",
    "require",
];

const NON_PROSE_LABELS: &[&str] = &[
    "alias",
    "crate",
    "feature",
    "module",
    "package",
    "path",
    "rename",
    "target",
    "target_arch",
    "target_env",
    "target_family",
    "target_feature",
    "target_os",
    "target_pointer_width",
    "target_vendor",
];

pub(super) fn is_non_prose_string(
    text: &str,
    language: Option<&str>,
    highlights: &Highlights,
    byte: usize,
) -> bool {
    let Some(start) = string_start(highlights, byte) else {
        return false;
    };
    let line_start = text[..start].rfind('\n').map_or(0, |index| index + 1);
    let Some(prefix) = text.get(line_start..start) else {
        return false;
    };
    let language = language.unwrap_or_default().to_ascii_lowercase();
    structural_line(prefix, &language)
        || context_identifier(prefix, '=').is_some_and(|name| NON_PROSE_LABELS.contains(&name))
        || unmatched_call(prefix).is_some_and(|name| NON_PROSE_CALLS.contains(&name))
        || (language == "go" && inside_go_import(text, start))
}

fn string_start(highlights: &Highlights, byte: usize) -> Option<usize> {
    let spans = highlights.all();
    let mut index = spans.partition_point(|span| span.span.start.0 <= byte);
    while index > 0 {
        index -= 1;
        let span = spans[index];
        if span.span.start.0 <= byte && byte < span.span.end.0 && span.token == TokenId::STRING {
            let mut start = span.span.start.0;
            while index > 0 {
                let previous = spans[index - 1];
                if previous.span.end.0 != start
                    || (previous.token != TokenId::STRING
                        && previous.token != StandardToken::StringEscape.id())
                {
                    break;
                }
                index -= 1;
                start = previous.span.start.0;
            }
            return Some(start);
        }
    }
    None
}

fn structural_line(prefix: &str, language: &str) -> bool {
    let trimmed = prefix.trim_start();
    match language {
        "rust" => {
            let attribute = prefix.rfind("#[").or_else(|| prefix.rfind("#!["));
            attribute.is_some_and(|start| !prefix[start..].contains(']'))
        },
        "javascript" | "typescript" | "typescriptreact" | "javascriptreact" => {
            trimmed.starts_with("import ")
                || (trimmed.starts_with("export ") && trimmed.contains(" from "))
        },
        "c" | "c++" => trimmed.starts_with("#include"),
        "bash" => trimmed.starts_with("source "),
        "java" | "kotlin" => trimmed.starts_with('@'),
        "c#" => trimmed.starts_with('[') && trimmed.contains('('),
        _ => false,
    }
}

fn context_identifier(prefix: &str, separator: char) -> Option<&str> {
    let before = prefix.trim_end().strip_suffix(separator)?.trim_end();
    trailing_identifier(before)
}

fn unmatched_call(prefix: &str) -> Option<&str> {
    let open = prefix.rfind('(')?;
    if prefix[open..].contains(')') {
        return None;
    }
    trailing_identifier(prefix[..open].trim_end_matches([' ', '\t', '!']))
}

fn trailing_identifier(text: &str) -> Option<&str> {
    let start = text
        .char_indices()
        .rev()
        .find(|(_, character)| !character.is_ascii_alphanumeric() && *character != '_')
        .map_or(0, |(index, character)| index + character.len_utf8());
    let identifier = text.get(start..)?;
    (!identifier.is_empty()).then_some(identifier)
}

fn inside_go_import(text: &str, start: usize) -> bool {
    let before = &text[..start];
    let Some(import) = before.rfind("import (") else {
        return false;
    };
    !before[import..].contains(')')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_identifier_requires_a_structural_separator() {
        assert_eq!(context_identifier("feature = ", '='), Some("feature"));
        assert_eq!(
            unmatched_call("let value = include_str!("),
            Some("include_str")
        );
        assert_eq!(unmatched_call("println!("), Some("println"));
        assert_eq!(context_identifier("let feature", '='), None);
    }

    #[test]
    fn cross_language_structural_forms_are_recognized() {
        assert!(structural_line("import value from ", "javascript"));
        assert!(structural_line("#include ", "c++"));
        assert!(structural_line("@JsonProperty(", "java"));
        let go_import = "package p\nimport (\n  ";
        assert!(inside_go_import(go_import, go_import.len()));
        assert!(!structural_line("console.log(", "javascript"));
        let go_value = "package p\nvar label = ";
        assert!(!inside_go_import(go_value, go_value.len()));
    }
}
