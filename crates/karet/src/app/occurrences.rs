//! Occurrence highlighting: once the caret has rested on a word for the edit
//! debounce, every occurrence of the symbol under it is painted behind the
//! text, writes stronger than reads.
//!
//! The language server knows which occurrences are the same symbol, so the
//! backend is asked first (`DocumentHighlights`). When no server answers --
//! none serves the document, it does not offer highlights, or the request
//! failed -- the word under the caret is matched as text instead, whole words
//! only and case-sensitively. A server that answers with nothing means the
//! caret is on no symbol it knows, and nothing is painted.
//!
//! A move within an occurrence keeps the paint until the next answer replaces
//! it; a move off every occurrence, an edit, or a selection takes it down at
//! once, since it no longer describes what the caret is on. An answer for a
//! caret or a text that has moved on is dropped.

use karet_session::LSP_CHANGE_DEBOUNCE;
use karet_session::api::DocumentHighlight;
use karet_session::api::HighlightKind;

use super::*;

/// Where the occurrence probe stands: a caret at one revision of one view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OccurrenceKey {
    /// The view the caret is in.
    pub(crate) view: ViewId,
    /// The document.
    pub(crate) doc: DocumentId,
    /// Its version.
    pub(crate) version: u64,
    /// The primary caret.
    pub(crate) caret: LineCol,
}

/// The probe in flight or waiting for the caret to settle.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OccurrenceProbe {
    /// What it asks about.
    pub(crate) key: OccurrenceKey,
    /// When it may be sent: the caret has to stay put until then.
    pub(crate) due: Instant,
    /// The request, once sent; `RequestId(0)` when it could not be.
    pub(crate) sent: Option<RequestId>,
}

/// The occurrences being painted, and what they were found for.
#[derive(Clone, Debug)]
pub(crate) struct ShownOccurrences {
    /// The view they are painted in.
    pub(crate) view: ViewId,
    /// The document version they describe.
    pub(crate) version: u64,
    /// One text-background decoration per occurrence.
    pub(crate) decorations: Vec<Decoration>,
}

/// The client's occurrence-highlight state.
#[derive(Clone, Debug, Default)]
pub(crate) struct Occurrences {
    /// The probe for the current caret.
    pub(crate) probe: Option<OccurrenceProbe>,
    /// What is painted.
    pub(crate) shown: Option<ShownOccurrences>,
}

/// The theme role an occurrence of `kind` is painted in: a write stands out
/// from the reads and textual mentions around it.
fn role_for(kind: HighlightKind) -> ThemeRole {
    match kind {
        HighlightKind::Write => ThemeRole::MatchingBracket,
        HighlightKind::Read | HighlightKind::Text => ThemeRole::HoverHighlight,
    }
}

fn decoration(range: Range, kind: HighlightKind) -> Decoration {
    Decoration::new(range, DecorationKind::TextBackground, Some(role_for(kind)))
}

/// The word (alphanumeric and `_`) the caret touches: the one it is on, else
/// the one ending at it.
fn word_at_caret(buffer: &TextBuffer, caret: LineCol) -> Option<Range> {
    let before = caret
        .col
        .checked_sub(1)
        .map(|col| LineCol::new(caret.line, col));
    let (start, end) = karet_editor::identifier_at(buffer, caret)
        .or_else(|| before.and_then(|at| karet_editor::identifier_at(buffer, at)))?;
    Some(Range { start, end })
}

/// The fallback when no server answered: every whole-word, case-sensitive
/// match of the word the caret touches, as textual occurrences. Empty off a
/// word.
pub(crate) fn text_occurrences(buffer: &TextBuffer, caret: LineCol) -> Vec<Decoration> {
    let Some(word) = word_at_caret(buffer, caret) else {
        return Vec::new();
    };
    let Some(line) = buffer.line(word.start.line as usize) else {
        return Vec::new();
    };
    let pattern: String = line
        .chars()
        .skip(word.start.col as usize)
        .take((word.end.col - word.start.col) as usize)
        .collect();
    let query = SearchQuery {
        pattern,
        case_sensitive: true,
        whole_word: true,
        ..SearchQuery::default()
    };
    search_chunks(buffer.chunks(), &query)
        .unwrap_or_default()
        .iter()
        .map(|found| {
            let range = Range {
                start: buffer.byte_to_line_col(BytePos(found.start)),
                end: buffer.byte_to_line_col(BytePos(found.end)),
            };
            decoration(range, HighlightKind::Text)
        })
        .collect()
}

/// Whether `caret` is inside `range` or at either end of it.
fn touches(range: Range, caret: LineCol) -> bool {
    range.start <= caret && caret <= range.end
}

impl App {
    /// The active code view's caret, when it is a bare caret in a document
    /// the backend holds: a selection asks for no occurrences.
    fn occurrence_target(&self) -> Option<(OccurrenceKey, &TextBuffer)> {
        let tab = self.tabs.get(self.active)?;
        let TabKind::Code {
            doc: Some(doc),
            buffer,
            ..
        } = &tab.kind
        else {
            return None;
        };
        if tab.editor.selection_range().is_some() {
            return None;
        }
        let key = OccurrenceKey {
            view: tab.view,
            doc: *doc,
            version: buffer.version(),
            caret: tab.editor.cursor(),
        };
        Some((key, buffer))
    }

    /// Move the occurrence probe along with the caret, and send it once the
    /// caret has settled on a word. Called after every frame.
    pub(crate) fn probe_occurrences(&mut self, now: Instant) {
        let Some((key, buffer)) = self.occurrence_target() else {
            self.occurrences = Occurrences::default();
            return;
        };
        let on_word = word_at_caret(buffer, key.caret).is_some();
        let state = &mut self.occurrences;
        if state.shown.as_ref().is_some_and(|shown| {
            shown.view != key.view
                || shown.version != key.version
                || !shown
                    .decorations
                    .iter()
                    .any(|deco| touches(deco.range, key.caret))
        }) {
            state.shown = None;
        }
        match &mut state.probe {
            Some(probe) if probe.key == key => {
                if probe.sent.is_some() || now < probe.due {
                    return;
                }
            },
            probe => {
                *probe = on_word.then(|| OccurrenceProbe {
                    key,
                    due: now + LSP_CHANGE_DEBOUNCE,
                    sent: None,
                });
                return;
            },
        }
        let sent = self.send(SessionCommand::DocumentHighlights {
            doc: key.doc,
            position: key.caret,
        });
        if let Some(probe) = self.occurrences.probe.as_mut() {
            // Marked sent even when there is no backend to send to, so an
            // unanswerable probe is not re-sent every frame.
            probe.sent = Some(sent.unwrap_or(RequestId(0)));
        }
        if sent.is_none() {
            self.show_occurrences(key, None);
        }
    }

    /// How long until the probe is due, so the loop wakes to send it without
    /// another keystroke.
    pub(crate) fn occurrence_next_wake(&self, now: Instant) -> Option<Duration> {
        let probe = self.occurrences.probe?;
        probe
            .sent
            .is_none()
            .then(|| probe.due.saturating_duration_since(now))
    }

    /// The backend's answer to the probe `id`. An answer for any other
    /// request, or for a caret or text that has moved on since, is dropped.
    pub(crate) fn on_document_highlights(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        version: u64,
        highlights: Option<Vec<DocumentHighlight>>,
    ) {
        let Some(probe) = self.occurrences.probe else {
            return;
        };
        if probe.sent.is_none_or(|sent| Some(sent) != id)
            || probe.key.doc != doc
            || probe.key.version != version
        {
            return;
        }
        self.show_occurrences(probe.key, highlights);
    }

    /// Paint what was found for `key`, when the caret is still there: the
    /// server's `highlights`, or with `None` the word matched as text.
    fn show_occurrences(&mut self, key: OccurrenceKey, highlights: Option<Vec<DocumentHighlight>>) {
        let Some((current, buffer)) = self.occurrence_target() else {
            return;
        };
        if current != key {
            return;
        }
        let decorations = match highlights {
            Some(found) => found
                .into_iter()
                .map(|highlight| decoration(highlight.range, highlight.kind))
                .collect(),
            None => text_occurrences(buffer, key.caret),
        };
        self.occurrences.shown = Some(ShownOccurrences {
            view: key.view,
            version: key.version,
            decorations,
        });
    }

    /// The occurrence decorations for the active view, when they describe its
    /// current text.
    pub(crate) fn occurrence_decorations(&self) -> Vec<Decoration> {
        let Some(shown) = self.occurrences.shown.as_ref() else {
            return Vec::new();
        };
        let current = self.tabs.get(self.active).and_then(|tab| match &tab.kind {
            TabKind::Code { buffer, .. } => Some((tab.view, buffer.version())),
            _ => None,
        });
        if current != Some((shown.view, shown.version)) {
            return Vec::new();
        }
        shown.decorations.clone()
    }
}
