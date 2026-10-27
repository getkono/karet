//! Application wiring for the comment toggles (Ctrl+/, Shift+Alt+A): resolve the
//! comment syntax of the language at the caret — an injected one included, such as
//! a `<script>` body in HTML or a fenced code block in Markdown — and submit the
//! pure toggle from `karet_editor::editing` over every selection at once.

use std::path::Path;
use std::sync::Arc;

use karet_core::LineCol;
use karet_core::Range;
use karet_editor::editing;
use karet_editor::editing::LineComment;
use karet_filetype::CommentTokens;
use karet_text::EditCause;
use karet_text::TextBuffer;
use karet_treesitter::LanguageId;

use super::syntax_select::ParsedText;
use crate::app::App;
use crate::tab::Tab;
use crate::tab::TabKind;

impl App {
    /// Toggle a line comment (`block == false`) or a block comment on the active
    /// code tab. A language without the needed syntax leaves the buffer alone.
    pub(super) fn toggle_comment(&mut self, block: bool) {
        let Some(Tab {
            kind:
                TabKind::Code {
                    path,
                    buffer,
                    next_version,
                    ..
                },
            editor,
            ..
        }) = self.tabs.get(self.active)
        else {
            return;
        };
        let (cursors, base) = (editor.cursors(), *next_version);
        let pos = probe(buffer, cursors.primary().range());
        let tokens = comment_tokens_at(path, buffer, pos, || self.layered_tree());
        let edit = if block {
            tokens.block().and_then(|(open, close)| {
                editing::toggle_block_comment(buffer, cursors, base, open, close)
            })
        } else {
            let token = match (tokens.line(), tokens.block()) {
                (Some(prefix), _) => LineComment::Prefix(prefix),
                (None, Some((open, close))) => LineComment::Wrap { open, close },
                (None, None) => return,
            };
            editing::toggle_line_comment(buffer, cursors, base, token)
        };
        if let Some(edit) = edit {
            self.submit_multi_edit(EditCause::Replace, edit);
        }
    }
}

/// Where the comment language is read: the first non-blank char at or after the
/// primary selection's start (a bare caret's position) on its line, so a caret in a
/// line's indent, or a selection opening in the whitespace before a `<script>`
/// body's code, still resolves the language of the code it precedes, and a caret
/// in the second language of a mixed-language line resolves that language. With
/// nothing but whitespace after it, the last non-blank char before it is used; a
/// blank line uses the caret.
fn probe(buffer: &TextBuffer, range: Range) -> LineCol {
    let from = range.start.col as usize;
    let col = buffer.line(range.start.line as usize).and_then(|text| {
        let solid = || {
            text.chars()
                .enumerate()
                .filter(|(_, c)| !c.is_whitespace())
                .map(|(i, _)| i)
        };
        solid()
            .find(|&i| i >= from)
            .or_else(|| solid().take_while(|&i| i < from).last())
            .and_then(|i| u32::try_from(i).ok())
    });
    LineCol::new(range.start.line, col.unwrap_or(range.start.col))
}

/// The comment tokens of the innermost language at `pos` in the document at `path`
/// that has any, falling back outward to the file's own type.
///
/// Layers nested inside a comment of their host (Markdown injected into a Rust doc
/// comment, and a doctest fence inside that) are not candidates: the line is still
/// the host's source, so toggling it must use the host's comment syntax.
///
/// `parse` yields the document's layered parse; it is asked only when the
/// file's language can inject another.
fn comment_tokens_at(
    path: &Path,
    buffer: &TextBuffer,
    pos: LineCol,
    parse: impl FnOnce() -> Option<Arc<ParsedText>>,
) -> CommentTokens {
    let own = karet_filetype::file_type_for_path(path).comment_tokens();
    let Some(root) = karet_treesitter::language_id_from_path(path) else {
        return own;
    };
    if karet_treesitter::injections_query(root).is_none() {
        return own;
    }
    let Ok(byte) = buffer.line_col_to_byte(pos) else {
        return own;
    };
    let Some(parsed) = parse() else {
        return own;
    };
    let layers = parsed.tree.layers_at(byte);
    // Outermost first: keep descending until a layer is inside a comment of its own.
    let host = layers
        .iter()
        .rposition(|layer| {
            layer
                .named_ancestors_at(byte)
                .iter()
                .any(|node| node.kind.contains("comment"))
        })
        .unwrap_or(0);
    layers
        .iter()
        .skip(host)
        .map(|layer| layer.language())
        .map(|lang| {
            if lang == root {
                own
            } else {
                tokens_for_language(lang)
            }
        })
        .find(|tokens| !tokens.is_empty())
        .unwrap_or(own)
}

/// The comment tokens of the first file type whose grammar is `lang`, for an
/// injected layer (the root uses its own path's file type, which can differ from
/// another type sharing the grammar — `.jsonc` versus `.json`).
fn tokens_for_language(lang: LanguageId) -> CommentTokens {
    karet_filetype::all_file_types()
        .iter()
        .find(|ft| {
            ft.grammar()
                .and_then(karet_treesitter::language_id_from_injection_name)
                == Some(lang)
        })
        .map_or(
            CommentTokens::NONE,
            karet_filetype::FileType::comment_tokens,
        )
}
