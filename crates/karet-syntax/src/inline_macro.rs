//! Tree-sitter-aware inline editing macros.

use std::borrow::Borrow;

use karet_core::BytePos;
use karet_core::LineCol;
use karet_core::LineIndex;
use karet_core::Range;
use karet_core::Span;
use karet_treesitter::LanguageId;
use karet_treesitter::LayeredParser;
use karet_treesitter::LayeredTree;
use karet_treesitter::SyntaxNode;
use karet_treesitter::language_id_from_injection_name;

/// The editor gesture that can activate an inline macro.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineMacroTrigger {
    /// A printable character was typed.
    Character(char),
    /// Tab was pressed at an otherwise ordinary editing caret.
    Tab,
}

/// One atomic replacement produced by an inline macro.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineMacroExpansion {
    /// Source range replaced by the macro.
    pub range: Range,
    /// Replacement text.
    pub new_text: String,
    /// Caret position after the replacement applies.
    pub caret: LineCol,
}

#[derive(Clone, Copy)]
enum ExpansionKind {
    MarkdownLink,
    MarkdownTag,
    RustFunction,
}

#[derive(Clone, Copy)]
struct MacroDefinition {
    language: &'static str,
    trigger: InlineMacroTrigger,
    kind: ExpansionKind,
}

// Seeded v1 catalog. Each macro is intentionally named one-by-one so adding a
// language does not silently opt it into text-only heuristics. Follow-up: grow
// this catalog with per-language generated rules and explicit gap-fillers.
const DEFINITIONS: &[MacroDefinition] = &[
    MacroDefinition {
        language: "markdown",
        trigger: InlineMacroTrigger::Character('['),
        kind: ExpansionKind::MarkdownLink,
    },
    MacroDefinition {
        language: "markdown",
        trigger: InlineMacroTrigger::Character('<'),
        kind: ExpansionKind::MarkdownTag,
    },
    MacroDefinition {
        language: "rust",
        trigger: InlineMacroTrigger::Tab,
        kind: ExpansionKind::RustFunction,
    },
];

/// Reusable resolver for the configured inline-macro catalog.
///
/// The resolver parses only when a configured gesture is attempted, then checks
/// the selection/caret against named tree-sitter ancestors before emitting an edit.
#[derive(Default)]
pub struct InlineMacroEngine {
    parser: LayeredParser,
}

impl InlineMacroEngine {
    /// Create an empty parser-backed resolver.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve one macro for `language`, returning `None` when the gesture or syntax
    /// context does not match a configured definition.
    ///
    /// `indentation` is one configured indentation level. It is used by block macros
    /// and ignored by surrounds.
    #[must_use]
    pub fn expand(
        &mut self,
        language: LanguageId,
        text: &str,
        selection: Range,
        trigger: InlineMacroTrigger,
        indentation: &str,
    ) -> Option<InlineMacroExpansion> {
        let parser = &mut self.parser;
        Self::expand_with_tree(language, text, selection, trigger, indentation, || {
            parser.parse(language, text).ok()
        })
    }

    /// [`expand`](Self::expand) over a layered parse the caller already holds
    /// (for example one cached per document version), so an editor that parses
    /// the buffer for other features does not parse it again here.
    ///
    /// `tree` is called at most once, and only after the gesture and the text
    /// around the caret pass the cheap checks, so a keystroke that cannot
    /// expand never asks for a parse. It must return a parse of exactly `text`
    /// as `language`; `None` means no parse is available and no macro expands.
    #[must_use]
    pub fn expand_with_tree<T: Borrow<LayeredTree>>(
        language: LanguageId,
        text: &str,
        selection: Range,
        trigger: InlineMacroTrigger,
        indentation: &str,
        tree: impl FnOnce() -> Option<T>,
    ) -> Option<InlineMacroExpansion> {
        let definition = DEFINITIONS.iter().find(|definition| {
            definition.trigger == trigger
                && language_id_from_injection_name(definition.language) == Some(language)
        })?;
        match definition.kind {
            ExpansionKind::MarkdownLink | ExpansionKind::MarkdownTag => {
                expand_markdown(text, selection, definition.kind, tree)
            },
            ExpansionKind::RustFunction => expand_rust_function(text, selection, indentation, tree),
        }
    }
}

fn expand_markdown<T: Borrow<LayeredTree>>(
    text: &str,
    selection: Range,
    kind: ExpansionKind,
    tree: impl FnOnce() -> Option<T>,
) -> Option<InlineMacroExpansion> {
    if selection.is_empty() {
        return None;
    }
    let span = range_to_span(text, selection)?;
    let tree = tree()?;
    let tree = tree.borrow();
    let start_nodes = ancestors_covering(tree, span.start);
    let end_byte = BytePos(span.end.0.saturating_sub(1));
    let end_nodes = ancestors_covering(tree, end_byte);
    if !markdown_text_context(&start_nodes) || !markdown_text_context(&end_nodes) {
        return None;
    }
    let selected = text.get(span.start.0..span.end.0)?;
    let (new_text, cursor_byte) = match kind {
        ExpansionKind::MarkdownLink => (format!("[{selected}]()"), selected.len() + 3),
        ExpansionKind::MarkdownTag => (format!("<{selected}>"), selected.len() + 1),
        ExpansionKind::RustFunction => return None,
    };
    Some(expansion(selection, &new_text, cursor_byte))
}

fn expand_rust_function<T: Borrow<LayeredTree>>(
    text: &str,
    selection: Range,
    indentation: &str,
    tree: impl FnOnce() -> Option<T>,
) -> Option<InlineMacroExpansion> {
    if !selection.is_empty() {
        return None;
    }
    let caret = line_col_to_byte(text, selection.start)?;
    let trigger_start = preceding_word_start(text, caret);
    if text.get(trigger_start..caret)? != "fn" {
        return None;
    }

    let original = tree()?;
    let at_trigger = ancestors_covering(original.borrow(), BytePos(trigger_start));
    if at_trigger.iter().any(|node| {
        matches!(
            node.kind.as_str(),
            "line_comment"
                | "block_comment"
                | "string_literal"
                | "raw_string_literal"
                | "char_literal"
        )
    }) {
        return None;
    }

    // The incomplete token is normally an ERROR node, but its ancestors still
    // identify whether it belongs to a file/module/impl/trait item list or an
    // executable block.
    let _nearest_container = at_trigger.iter().find(|node| {
        matches!(
            node.kind.as_str(),
            "block" | "declaration_list" | "source_file"
        )
    })?;

    let line_start = text[..trigger_start]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let leading = text.get(line_start..trigger_start)?;
    if !leading.chars().all(char::is_whitespace) {
        return None;
    }
    let new_text = format!("fn () {{\n{leading}{indentation}\n{leading}}}");
    let range = Range {
        start: LineIndex::new(text).line_col(text, BytePos(trigger_start)),
        end: selection.end,
    };
    Some(expansion(range, &new_text, 3))
}

fn ancestors_covering(tree: &LayeredTree, byte: BytePos) -> Vec<SyntaxNode> {
    tree.layers()
        .filter(|layer| layer.span().start.0 <= byte.0 && byte.0 <= layer.span().end.0)
        .flat_map(|layer| layer.named_ancestors_at(byte))
        .collect()
}

fn markdown_text_context(nodes: &[SyntaxNode]) -> bool {
    let allowed = nodes.iter().any(|node| {
        matches!(
            node.kind.as_str(),
            "paragraph" | "atx_heading" | "setext_heading"
        )
    });
    let forbidden = nodes.iter().any(|node| {
        matches!(
            node.kind.as_str(),
            "fenced_code_block"
                | "indented_code_block"
                | "code_span"
                | "inline_link"
                | "full_reference_link"
                | "collapsed_reference_link"
                | "shortcut_link"
                | "uri_autolink"
                | "email_autolink"
        )
    });
    allowed && !forbidden
}

fn preceding_word_start(text: &str, end: usize) -> usize {
    text.get(..end)
        .and_then(|prefix| {
            prefix
                .char_indices()
                .rev()
                .find(|(_, ch)| !ch.is_alphanumeric() && *ch != '_')
                .map(|(index, ch)| index + ch.len_utf8())
        })
        .unwrap_or(0)
}

fn expansion(range: Range, new_text: &str, cursor_byte: usize) -> InlineMacroExpansion {
    InlineMacroExpansion {
        range,
        new_text: new_text.to_owned(),
        caret: advance(range.start, new_text.get(..cursor_byte).unwrap_or_default()),
    }
}

fn advance(mut position: LineCol, text: &str) -> LineCol {
    for ch in text.chars() {
        if ch == '\n' {
            position.line += 1;
            position.col = 0;
        } else {
            position.col += 1;
        }
    }
    position
}

fn range_to_span(text: &str, range: Range) -> Option<Span> {
    Some(Span {
        start: BytePos(line_col_to_byte(text, range.start)?),
        end: BytePos(line_col_to_byte(text, range.end)?),
    })
}

fn line_col_to_byte(text: &str, position: LineCol) -> Option<usize> {
    let line_start = if position.line == 0 {
        0
    } else {
        text.match_indices('\n')
            .nth(position.line.saturating_sub(1) as usize)
            .map(|(index, _)| index + 1)?
    };
    let line = text
        .get(line_start..)?
        .split('\n')
        .next()
        .unwrap_or_default();
    let byte_col = line.char_indices().nth(position.col as usize).map_or_else(
        || (position.col as usize == line.chars().count()).then_some(line.len()),
        |(index, _)| Some(index),
    )?;
    Some(line_start + byte_col)
}

#[cfg(test)]
mod tests {
    use karet_treesitter::language_id_from_injection_name;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn language(name: &str) -> Result<LanguageId, Box<dyn std::error::Error>> {
        language_id_from_injection_name(name).ok_or_else(|| "missing grammar".into())
    }

    #[test]
    fn markdown_surrounds_selected_prose_and_places_the_caret() -> TestResult {
        let lang = language("markdown")?;
        let range = Range {
            start: LineCol::new(0, 0),
            end: LineCol::new(0, 4),
        };
        let mut engine = InlineMacroEngine::new();
        let link = engine
            .expand(
                lang,
                "café is good",
                range,
                InlineMacroTrigger::Character('['),
                "    ",
            )
            .ok_or("missing link macro")?;
        assert_eq!(link.new_text, "[café]()");
        assert_eq!(link.caret, LineCol::new(0, 7));

        let tag = engine
            .expand(
                lang,
                "café is good",
                range,
                InlineMacroTrigger::Character('<'),
                "    ",
            )
            .ok_or("missing tag macro")?;
        assert_eq!(tag.new_text, "<café>");
        assert_eq!(tag.caret, LineCol::new(0, 5));
        Ok(())
    }

    #[test]
    fn markdown_macros_reject_code_and_existing_links() -> TestResult {
        let lang = language("markdown")?;
        let mut engine = InlineMacroEngine::new();
        for (text, start, end) in [
            ("`code`", 1, 5),
            ("[label](url)", 1, 6),
            ("```rust\ncode\n```", 8, 12),
        ] {
            assert!(
                engine
                    .expand(
                        lang,
                        text,
                        Range {
                            start: LineCol::new(
                                if start == 8 { 1 } else { 0 },
                                if start == 8 { 0 } else { start }
                            ),
                            end: LineCol::new(
                                if end == 12 { 1 } else { 0 },
                                if end == 12 { 4 } else { end }
                            ),
                        },
                        InlineMacroTrigger::Character('['),
                        "    ",
                    )
                    .is_none(),
                "{text}"
            );
        }
        Ok(())
    }

    #[test]
    fn rust_function_macro_requires_an_item_context() -> TestResult {
        let lang = language("rust")?;
        let mut engine = InlineMacroEngine::new();
        let top = engine
            .expand(
                lang,
                "fn",
                Range {
                    start: LineCol::new(0, 2),
                    end: LineCol::new(0, 2),
                },
                InlineMacroTrigger::Tab,
                "  ",
            )
            .ok_or("missing top-level function macro")?;
        assert_eq!(top.new_text, "fn () {\n  \n}");
        assert_eq!(top.caret, LineCol::new(0, 3));

        let in_impl = "impl Service {\n    fn";
        assert!(
            engine
                .expand(
                    lang,
                    in_impl,
                    Range {
                        start: LineCol::new(1, 6),
                        end: LineCol::new(1, 6),
                    },
                    InlineMacroTrigger::Tab,
                    "    ",
                )
                .is_some()
        );

        // Rust permits item declarations in blocks, so a nested function is a valid
        // semantic item context too.
        let in_body = "fn main() {\n    fn";
        assert!(
            engine
                .expand(
                    lang,
                    in_body,
                    Range {
                        start: LineCol::new(1, 6),
                        end: LineCol::new(1, 6),
                    },
                    InlineMacroTrigger::Tab,
                    "    ",
                )
                .is_some()
        );

        for (text, caret) in [
            ("// fn", LineCol::new(0, 5)),
            ("const S: &str = \"fn\";", LineCol::new(0, 19)),
            ("let value = fn", LineCol::new(0, 14)),
        ] {
            assert!(
                engine
                    .expand(
                        lang,
                        text,
                        Range {
                            start: caret,
                            end: caret,
                        },
                        InlineMacroTrigger::Tab,
                        "    ",
                    )
                    .is_none(),
                "{text}"
            );
        }
        Ok(())
    }

    #[test]
    fn unconfigured_gestures_do_not_expand() -> TestResult {
        let mut engine = InlineMacroEngine::new();
        assert!(
            engine
                .expand(
                    language("rust")?,
                    "fn",
                    Range {
                        start: LineCol::new(0, 2),
                        end: LineCol::new(0, 2),
                    },
                    InlineMacroTrigger::Character('f'),
                    "    ",
                )
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn a_held_tree_expands_and_is_asked_for_only_past_the_cheap_checks() -> TestResult {
        let lang = language("rust")?;
        let caret = Range {
            start: LineCol::new(0, 2),
            end: LineCol::new(0, 2),
        };
        let tree = LayeredParser::new().parse(lang, "fn")?;
        let expanded = InlineMacroEngine::expand_with_tree(
            lang,
            "fn",
            caret,
            InlineMacroTrigger::Tab,
            "  ",
            || Some(&tree),
        )
        .ok_or("missing function macro over a held tree")?;
        assert_eq!(expanded.new_text, "fn () {\n  \n}");

        // Tab after anything but `fn`, an empty markdown selection, and an
        // unconfigured gesture never need a parse.
        let lang_md = language("markdown")?;
        for (lang, text, trigger) in [
            (lang, "let", InlineMacroTrigger::Tab),
            (lang_md, "te", InlineMacroTrigger::Character('[')),
            (lang, "fn", InlineMacroTrigger::Character('f')),
        ] {
            let mut asked = false;
            let result =
                InlineMacroEngine::expand_with_tree(lang, text, caret, trigger, "  ", || {
                    asked = true;
                    None::<LayeredTree>
                });
            assert!(result.is_none() && !asked, "{text}");
        }

        // No parse available means no expansion.
        assert!(
            InlineMacroEngine::expand_with_tree(
                lang,
                "fn",
                caret,
                InlineMacroTrigger::Tab,
                "  ",
                || None::<LayeredTree>,
            )
            .is_none()
        );
        Ok(())
    }
}
