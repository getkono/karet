//! Application wiring for the comment toggles (Ctrl+/, Shift+Alt+A): resolve the
//! comment syntax of the language at the caret — an injected one included, such as
//! a `<script>` body in HTML or a fenced code block in Markdown — and submit the
//! pure toggle from `karet_editor::editing` over every selection at once.

use karet_editor::editing::LineComment;
use karet_filetype::CommentTokens;
use karet_treesitter::LanguageId;

use super::*;

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
        let tokens = comment_tokens_at(path, buffer, probe(buffer, cursors.primary().range()));
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

/// Where the comment language is read: the first non-blank char of the primary
/// selection's first line at or after its start, so a caret in a line's indent (or
/// a selection opening in the whitespace before a `<script>` body's code) still
/// resolves the language of the code on that line. A blank line uses the caret.
fn probe(buffer: &TextBuffer, range: Range) -> LineCol {
    let from = if range.is_empty() { 0 } else { range.start.col };
    let col = buffer.line(range.start.line as usize).and_then(|text| {
        text.chars()
            .enumerate()
            .skip(from as usize)
            .find(|(_, c)| !c.is_whitespace())
            .and_then(|(i, _)| u32::try_from(i).ok())
    });
    LineCol::new(range.start.line, col.unwrap_or(range.start.col))
}

/// The comment tokens of the innermost language at `pos` in the document at `path`
/// that has any, falling back outward to the file's own type.
fn comment_tokens_at(path: &Path, buffer: &TextBuffer, pos: LineCol) -> CommentTokens {
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
    let text = buffer.text();
    let Ok(tree) = karet_treesitter::LayeredParser::new().parse(root, &text) else {
        return own;
    };
    tree.languages_at(byte)
        .into_iter()
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
