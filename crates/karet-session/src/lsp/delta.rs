//! Incremental document sync: turning a buffer edit into the LSP content
//! changes that reproduce it, and composing those across the debounce.
//!
//! Every change still carries the document's whole text, because the server
//! task keeps it to replay `didOpen` after a reconnect. The delta rides beside
//! it, and is dropped -- falling back to the whole text -- wherever it cannot
//! be trusted to take the server's copy to the buffer's:
//!
//! - the edit is not expressible as ranges in one frame ([`content_changes`]);
//! - the delta does not start from the version the server task last saw for
//!   the document (a change the session failed to queue was lost in between);
//! - the text holds a line break the rope counts and LSP does not, or a `\r`
//!   (see [`lines_agree`]), so a rope row would name the wrong server line;
//! - the server did not ask for incremental sync (decided at the flush).

use std::path::PathBuf;

use karet_core::BytePos;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::Span;
use karet_core::TextEdit;
use karet_lsp::LspClient;
use karet_lsp::LspError;
use karet_text::Applied;
use karet_text::TextBuffer;

/// The edits that take a document from `base_version` to the version of the
/// change carrying them, as LSP content changes: UTF-16 ranges, applied in
/// order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DocumentDelta {
    /// The document version the first edit applies to.
    pub(crate) base_version: i32,
    /// The content changes, in application order.
    pub(crate) edits: Vec<TextEdit>,
}

/// The content changes reproducing `applied` on the text `buffer` held before
/// it, read from `buffer` *after* it -- or `None` when they cannot be derived.
///
/// `applied.edits` lists one batch descending by start byte, every edit in the
/// pre-edit frame. Replayed in reverse (ascending), each edit meets a document
/// whose text before it is already final, so its start is read from the final
/// buffer and its end is the start advanced over the text it replaced. An
/// undo of several changes concatenates batches in different frames; that is
/// detected by the order breaking, and refused.
pub(crate) fn content_changes(buffer: &TextBuffer, applied: &Applied) -> Option<Vec<TextEdit>> {
    let edits = &applied.edits;
    if edits.is_empty()
        || edits
            .windows(2)
            .any(|pair| pair[1].old_end_byte > pair[0].start_byte)
    {
        return None;
    }
    let mut shift = 0isize;
    let mut out = Vec::with_capacity(edits.len());
    for edit in edits.iter().rev() {
        let new_len = edit.new_end_byte.checked_sub(edit.start_byte)?;
        let old_len = edit.old_end_byte.checked_sub(edit.start_byte)?;
        if edit.replaced.len() != old_len {
            return None;
        }
        let start = edit.start_byte.checked_add_signed(shift)?;
        let new_text = buffer.slice(Span {
            start: BytePos(start),
            end: BytePos(start.checked_add(new_len)?),
        })?;
        let at = buffer.byte_to_line_col(BytePos(start));
        let begin = LineCol::new(at.line, buffer.line_col_to_utf16(at));
        out.push(TextEdit {
            range: Range {
                start: begin,
                end: advance(begin, &edit.replaced),
            },
            new_text,
        });
        shift = shift
            .checked_add_unsigned(new_len)?
            .checked_sub_unsigned(old_len)?;
    }
    Some(out)
}

/// `from` moved past `text`, in LSP coordinates (UTF-16 columns, `\n` lines).
fn advance(from: LineCol, text: &str) -> LineCol {
    text.chars().fold(from, |at, ch| {
        if ch == '\n' {
            LineCol::new(at.line + 1, 0)
        } else {
            LineCol::new(at.line, at.col + ch.len_utf16() as u32)
        }
    })
}

/// Whether the rope and LSP number `text`'s lines alike.
///
/// LSP breaks lines at `\n`, `\r\n` and `\r`; the rope also at VT, FF, NEL,
/// LS and PS, so a document holding one has rope rows the server does not.
/// A `\r` is refused too: the buffer is LF-normalized, so one is rare, and a
/// position between it and a `\n` names no LSP position at all.
pub(crate) fn lines_agree(text: &str) -> bool {
    !text.contains(['\r', '\u{0B}', '\u{0C}', '\u{85}', '\u{2028}', '\u{2029}'])
}

/// The `didChange` the server task holds back for the debounce: the latest
/// version and text, and the deltas since the last one sent when every edit
/// between had one.
#[derive(Debug)]
pub(super) struct PendingChange {
    /// The document path.
    pub(super) path: PathBuf,
    /// The latest version.
    pub(super) version: i32,
    /// The latest full text.
    pub(super) text: String,
    /// The composed content changes, `None` once any edit lacked them.
    pub(super) edits: Option<Vec<TextEdit>>,
}

impl PendingChange {
    /// Fold a change for `path` into whatever is pending for it (the caller
    /// flushes a pending change for another path first).
    ///
    /// LSP applies a notification's content changes in order, so composing two
    /// deltas is appending them. The composition falls back to the whole text
    /// once its edits would carry as much text as the document holds.
    pub(super) fn coalesce(
        previous: Option<Self>,
        (path, version, text): (PathBuf, i32, String),
        delta: Option<DocumentDelta>,
    ) -> Self {
        let edits = match (previous, delta) {
            (None, delta) => delta.map(|delta| delta.edits),
            (Some(previous), Some(delta)) => previous.edits.map(|mut edits| {
                edits.extend(delta.edits);
                edits
            }),
            (Some(_), None) => None,
        };
        let edits = edits.filter(|edits| {
            edits.iter().map(|edit| edit.new_text.len()).sum::<usize>() < text.len()
        });
        Self {
            path,
            version,
            text,
            edits,
        }
    }

    /// Send this change: as ranges when it has them and the server takes them,
    /// else as the whole text.
    pub(super) async fn send(self, client: &LspClient) -> Result<(), LspError> {
        if let Some(edits) = self.edits.as_deref().filter(|edits| !edits.is_empty()) {
            match client
                .did_change_incremental(&self.path, self.version, edits)
                .await
            {
                Err(LspError::Unsupported { .. }) => {},
                sent => return sent,
            }
        }
        client
            .did_change(&self.path, self.version, &self.text)
            .await
    }
}

#[cfg(test)]
#[path = "delta_tests.rs"]
mod tests;
