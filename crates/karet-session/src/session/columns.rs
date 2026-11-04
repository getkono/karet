//! Column conversion between a language server and the session's buffers.
//!
//! A server counts columns in UTF-16 code units; a buffer, and every client,
//! counts characters (see the karet-lsp crate docs). Every position the session
//! forwards to a server and every range it hands back crosses that boundary,
//! and this module is the one place it is crossed: the per-model conversions
//! each request family needs live here rather than beside the request.

use karet_core::CodeAction;
use karet_core::CompletionItem;
use karet_core::DocumentChange;
use karet_core::InlayHint;
use karet_core::Location;
use karet_core::WorkspaceSymbol;
use karet_text::TextBuffer;

use super::*;

/// Which column encoding an edit's ranges count in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Columns {
    /// UTF-16 code units: anything a language server produced.
    Utf16,
    /// Buffer (character) columns: anything a client produced.
    Buffer,
}

/// `range`, from the server's UTF-16 columns to `buffer`'s.
pub(super) fn utf16_range_to_buffer(buffer: &TextBuffer, range: Range) -> Range {
    Range {
        start: buffer.utf16_to_line_col(range.start.line, range.start.col),
        end: buffer.utf16_to_line_col(range.end.line, range.end.col),
    }
}

/// A caret converted to the server's UTF-16 column encoding — the conversion
/// every positional LSP forwarder applies (see the karet-lsp crate docs).
pub(super) fn utf16_caret(doc: &Document, position: LineCol) -> LineCol {
    LineCol::new(position.line, doc.buffer.line_col_to_utf16(position))
}

/// Convert every symbol's ranges, its children's included, from UTF-16
/// columns to `buffer`'s.
pub(super) fn convert_symbol_columns(buffer: &TextBuffer, symbols: &mut [Symbol]) {
    for symbol in symbols {
        symbol.range = utf16_range_to_buffer(buffer, symbol.range);
        symbol.selection_range = utf16_range_to_buffer(buffer, symbol.selection_range);
        convert_symbol_columns(buffer, &mut symbol.children);
    }
}

/// `edits`, counted in `columns`, converted to `buffer`'s columns.
pub(super) fn edits_to_buffer_columns(
    buffer: &TextBuffer,
    mut edits: Vec<TextEdit>,
    columns: Columns,
) -> Vec<TextEdit> {
    if columns == Columns::Utf16 {
        for edit in &mut edits {
            edit.range = utf16_range_to_buffer(buffer, edit.range);
        }
    }
    edits
}

impl Session {
    /// The text of `path` for column conversion: the live buffer when the file is
    /// open, else its contents from disk.
    ///
    /// A server answers in UTF-16 columns for whatever file it names, which is often
    /// not one the client has open — so converting those columns needs the target's
    /// text, not the requesting document's. `None` when the file cannot be read; the
    /// caller then leaves the range as the server sent it, since an approximate
    /// column beats a dropped result.
    pub(super) fn buffer_for_path(&self, path: &Path) -> Option<TextBuffer> {
        self.store
            .by_path
            .get(path)
            .and_then(|doc| self.store.docs.get(doc))
            .map(|document| document.buffer.clone())
            .or_else(|| {
                std::fs::read_to_string(path)
                    .ok()
                    .map(|text| TextBuffer::from_text(&text))
            })
    }

    /// Convert every location from the server's UTF-16 columns to buffer columns,
    /// each against its own file.
    ///
    /// An open file's live buffer is borrowed, not cloned, and a file that is not
    /// open is read from disk at most once per answer, so a references list with
    /// many hits in one file costs one read rather than one per hit. A file that
    /// cannot be read keeps the server's columns, as [`Self::buffer_for_path`]
    /// explains.
    pub(super) fn locations_to_buffer_columns<'a>(
        &self,
        locations: impl IntoIterator<Item = &'a mut Location>,
    ) {
        let mut read: HashMap<PathBuf, Option<TextBuffer>> = HashMap::new();
        for location in locations {
            let open = self
                .store
                .by_path
                .get(&location.path)
                .and_then(|doc| self.store.docs.get(doc))
                .map(|document| &document.buffer);
            let buffer = match open {
                Some(buffer) => Some(buffer),
                None => read
                    .entry(location.path.clone())
                    .or_insert_with(|| {
                        std::fs::read_to_string(&location.path)
                            .ok()
                            .map(|text| TextBuffer::from_text(&text))
                    })
                    .as_ref(),
            };
            if let Some(buffer) = buffer {
                location.range = utf16_range_to_buffer(buffer, location.range);
            }
        }
    }

    /// Move workspace symbols from the server's UTF-16 columns to buffer
    /// columns, each against its own file when that file is open here.
    ///
    /// A symbol in a file that is not open keeps the server's columns. A
    /// workspace search answers every typing pause with hits across any number
    /// of files, so reading each from disk here would put that much blocking
    /// I/O on the session for rows nobody may pick -- the trade inlay-hint
    /// part locations make (see [`Self::hints_to_buffer_columns`]). The line is
    /// right either way; the column differs only where an astral-plane
    /// character precedes the symbol on its line.
    pub(super) fn workspace_symbols_to_buffer_columns(&self, symbols: &mut [WorkspaceSymbol]) {
        for symbol in symbols {
            let Some(range) = symbol.range.as_mut() else {
                continue;
            };
            let open = self
                .store
                .by_path
                .get(&symbol.path)
                .and_then(|doc| self.store.docs.get(doc));
            if let Some(document) = open {
                *range = utf16_range_to_buffer(&document.buffer, *range);
            }
        }
    }

    /// Move `hints` from the server's UTF-16 columns to buffer columns: each
    /// position against `doc`'s buffer, and each label part's location
    /// against the file it names when that file is open. Hints for a document
    /// closed since they were asked for are left as they are; nobody will
    /// adopt them.
    ///
    /// A part location into a file that is not open keeps the server's
    /// columns. Hint lists arrive on every edit pause and scroll, so reading
    /// each named file from disk here would put blocking I/O on that path for
    /// locations nobody may follow; UTF-16 and buffer columns differ only
    /// where an astral-plane character precedes the target on its line. A
    /// resolve, which is asked for one hint on demand, converts against the
    /// file on disk (see [`Self::part_locations_to_buffer_columns`]).
    pub(super) fn hints_to_buffer_columns(&self, doc: DocumentId, hints: &mut [InlayHint]) {
        let Some(document) = self.store.docs.get(&doc) else {
            return;
        };
        for hint in hints.iter_mut() {
            hint.position = document
                .buffer
                .utf16_to_line_col(hint.position.line, hint.position.col);
        }
        let locations = hints.iter_mut().flat_map(|hint| {
            hint.parts
                .iter_mut()
                .filter_map(|part| part.location.as_mut())
        });
        for location in locations {
            let open = self
                .store
                .by_path
                .get(&location.path)
                .and_then(|doc| self.store.docs.get(doc));
            if let Some(open) = open {
                location.range = utf16_range_to_buffer(&open.buffer, location.range);
            }
        }
    }

    /// Convert every label part's location in `hints` to buffer columns, each
    /// against the file it names, as a definition's is. A resolved hint needs
    /// only this: it keeps the position the client asked about.
    pub(super) fn part_locations_to_buffer_columns(&self, hints: &mut [InlayHint]) {
        self.locations_to_buffer_columns(hints.iter_mut().flat_map(|hint| {
            hint.parts
                .iter_mut()
                .filter_map(|part| part.location.as_mut())
        }));
    }

    /// Convert every listed action's edit from the server's UTF-16 columns to
    /// buffer columns, reading each file at most once.
    pub(super) fn actions_to_buffer_columns(&self, actions: &mut [CodeAction]) {
        let mut buffers: HashMap<PathBuf, Option<TextBuffer>> = HashMap::new();
        let mut convert = |path: &Path, edits: &mut [TextEdit]| {
            let buffer = buffers
                .entry(path.to_path_buf())
                .or_insert_with(|| self.buffer_for_path(path));
            // A file that cannot be read keeps the server's columns: its edit
            // cannot apply anyway, and that is reported when it is run.
            if let Some(buffer) = buffer {
                for edit in edits {
                    edit.range = utf16_range_to_buffer(buffer, edit.range);
                }
            }
        };
        for edit in actions.iter_mut().filter_map(|action| action.edit.as_mut()) {
            for (path, edits) in &mut edit.changes {
                convert(path, edits);
            }
            for change in &mut edit.document_changes {
                if let DocumentChange::Edit(document) = change {
                    convert(&document.path, &mut document.edits);
                }
            }
        }
    }

    /// Convert every range `item` carries -- its edit, its replace range, its
    /// additional edits -- from the server's UTF-16 columns to `doc`'s buffer
    /// columns. A closed document leaves them as they are.
    pub(super) fn completion_to_buffer_columns(&self, doc: DocumentId, item: &mut CompletionItem) {
        let Some(open) = self.store.docs.get(&doc) else {
            return;
        };
        let ranges = item
            .edit
            .iter_mut()
            .chain(item.additional_edits.iter_mut())
            .map(|edit| &mut edit.range)
            .chain(item.replace_range.as_mut());
        for range in ranges {
            *range = utf16_range_to_buffer(&open.buffer, *range);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An edit just past an astral-plane character, which is two UTF-16
    /// code units but one buffer column.
    fn edit_after_emoji() -> TextEdit {
        TextEdit {
            range: Range {
                start: LineCol::new(0, 2),
                end: LineCol::new(0, 3),
            },
            new_text: "b".to_owned(),
        }
    }

    #[test]
    fn server_edits_move_to_buffer_columns() {
        let buffer = TextBuffer::from_text("😀a\n");
        let edits = edits_to_buffer_columns(&buffer, vec![edit_after_emoji()], Columns::Utf16);
        let ranges: Vec<Range> = edits.iter().map(|edit| edit.range).collect();
        assert_eq!(
            ranges,
            [Range {
                start: LineCol::new(0, 1),
                end: LineCol::new(0, 2),
            }]
        );
    }

    #[test]
    fn client_edits_keep_their_columns() {
        let buffer = TextBuffer::from_text("😀a\n");
        let edits = edits_to_buffer_columns(&buffer, vec![edit_after_emoji()], Columns::Buffer);
        assert_eq!(edits, [edit_after_emoji()]);
    }
}
