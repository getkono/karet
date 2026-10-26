//! Keeping the visible part of each editor annotated with code lenses, and
//! running the one a click lands on.
//!
//! A document's lenses are listed whole, but each one the server deferred
//! costs it a search to title ("3 references"), so the request names the
//! lines worth resolving: the viewport with the same overscan inlay hints
//! use, and it is asked again when the viewport leaves what was resolved or
//! the text changes. It rides the same schedule as the hint request -- after
//! a frame, for the front tab of each visible pane, once typing has paused --
//! and keeps the same bookkeeping, held in [`CodeLensState`]: one request
//! in flight per document, a failure that leaves what is shown standing and
//! backs off, and a held set that moves with the text until its replacement
//! arrives.
//!
//! The lenses are drawn as virtual lines above the lines they describe
//! ([`code_lens_rows`]); a click on a title runs its command on the server,
//! and only a failure is reported.

use karet_core::CodeLens;
use karet_core::CodeLensRow;
use karet_session::AppliedWorkspaceEdit;
use karet_session::LSP_CHANGE_DEBOUNCE;

use super::inlay::HintRange;
use super::inlay::PendingInlay;
use super::inlay::inlay_retry_delay;
use super::inlay::shift_position;
use super::*;

/// The lenses held for each document, and the requests for them.
#[derive(Default)]
pub(crate) struct CodeLensState {
    /// The latest lens set per document, ranges in buffer columns.
    pub(crate) held: HashMap<DocumentId, Vec<CodeLens>>,
    /// The buffer version each held set is positioned against: the version it
    /// was answered for, then each version an edit carried it to.
    version: HashMap<DocumentId, u64>,
    /// The lines each held set was resolved over, at its version.
    covered: HashMap<DocumentId, HintRange>,
    /// The outstanding request per document.
    pending: HashMap<DocumentId, PendingInlay>,
    /// Bumped when coverage is invalidated, so a request issued before it
    /// stops counting as covering anything.
    epoch: u64,
    /// When each recently edited or failing document may be asked again.
    quiet_until: HashMap<DocumentId, Instant>,
    /// Requests in a row each document's server has left unanswered.
    failures: HashMap<DocumentId, u32>,
    /// The lens run awaiting its answer, with its title for a failure.
    running: Option<(RequestId, String)>,
}

/// The rows `lenses` draw above their lines in `buffer`, each indented like
/// the line it sits above, a tab counting `tab_width` cells.
pub(crate) fn code_lens_rows(
    lenses: &[CodeLens],
    buffer: &TextBuffer,
    tab_width: u16,
) -> Vec<CodeLensRow> {
    CodeLensRow::build(lenses, |line| {
        let text = buffer.line(line as usize).unwrap_or_default();
        text.chars()
            .take_while(|c| c.is_whitespace() && *c != '\n' && *c != '\r')
            .map(|c| if c == '\t' { usize::from(tab_width) } else { 1 })
            .sum()
    })
}

impl App {
    /// Whether code lenses are drawn (`editor.codeLens`).
    pub(crate) fn code_lenses_enabled(&self) -> bool {
        self.settings.editor.code_lens
    }

    /// Ask for lenses covering every visible editor whose held set no longer
    /// does. Called once per frame, beside [`Self::request_inlay_hints`].
    pub(crate) fn request_code_lenses(&mut self) {
        self.request_code_lenses_at(Instant::now());
    }

    /// [`Self::request_code_lenses`] at a given moment, so the edit debounce
    /// can be tested without sleeping.
    pub(crate) fn request_code_lenses_at(&mut self, now: Instant) {
        self.code_lenses.quiet_until.retain(|_, until| *until > now);
        if !self.code_lenses_enabled() {
            return;
        }
        for (doc, wanted) in self.visible_hint_ranges() {
            let state = &self.code_lenses;
            let current = state
                .covered
                .get(&doc)
                .is_some_and(|have| have.covers(wanted))
                || state.pending.get(&doc).is_some_and(|pending| {
                    pending.epoch == state.epoch && pending.asked.covers(wanted)
                });
            if current || state.quiet_until.contains_key(&doc) {
                continue;
            }
            let lines = (wanted.first, wanted.last);
            let Some(id) = self.send(SessionCommand::CodeLenses { doc, lines }) else {
                continue;
            };
            let epoch = self.code_lenses.epoch;
            let pending = PendingInlay {
                id,
                asked: wanted,
                epoch,
            };
            self.code_lenses.pending.insert(doc, pending);
        }
    }

    /// Adopt an answered lens set, if it answers the request still
    /// outstanding for `doc` against the text the buffer still holds.
    pub(crate) fn on_code_lenses(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        version: u64,
        lenses: Vec<CodeLens>,
    ) {
        let Some(pending) = self.take_code_lens_pending(id, doc, version) else {
            return;
        };
        if self
            .buffer_version(doc)
            .is_some_and(|current| current != version)
        {
            return; // the buffer moved on while the server was thinking
        }
        let state = &mut self.code_lenses;
        state.failures.remove(&doc);
        state.held.insert(doc, lenses);
        state.version.insert(doc, version);
        // Coverage only from the current epoch, as for inlay hints: an answer
        // from before the last invalidation is shown, but asked again.
        if pending.epoch == state.epoch {
            state.covered.insert(doc, pending.asked);
        }
    }

    /// Note that the request outstanding for `doc` went unanswered.
    pub(crate) fn on_code_lenses_failed(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        version: u64,
    ) {
        self.on_code_lenses_failed_at(id, doc, version, Instant::now());
    }

    /// [`Self::on_code_lenses_failed`] at a given moment, so the back-off can
    /// be tested without sleeping.
    ///
    /// Nothing was learned, so what is shown stays and nothing counts as
    /// covered; the re-ask waits the edit pause, doubling with each failure
    /// in a row up to the inlay-hint ceiling.
    pub(crate) fn on_code_lenses_failed_at(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        version: u64,
        now: Instant,
    ) {
        if self.take_code_lens_pending(id, doc, version).is_none() {
            return;
        }
        let failures = self.code_lenses.failures.entry(doc).or_insert(0);
        *failures = failures.saturating_add(1);
        let retry_at = now + inlay_retry_delay(*failures);
        self.code_lenses.quiet_until.insert(doc, retry_at);
    }

    /// Take the request outstanding for `doc` when `id` answers it at the
    /// version it asked; `None` for a late answer to a retired request.
    fn take_code_lens_pending(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        version: u64,
    ) -> Option<PendingInlay> {
        let pending = self.code_lenses.pending.get(&doc).copied()?;
        if id != Some(pending.id) || pending.asked.version != version {
            return None;
        }
        self.code_lenses.pending.remove(&doc)
    }

    /// Drop everything held for `doc`, on close.
    pub(crate) fn forget_code_lenses(&mut self, doc: DocumentId) {
        let state = &mut self.code_lenses;
        state.held.remove(&doc);
        state.version.remove(&doc);
        state.covered.remove(&doc);
        state.pending.remove(&doc);
        state.quiet_until.remove(&doc);
        state.failures.remove(&doc);
    }

    /// Drop every document's lenses and requests when `editor.codeLens` is
    /// off, so what was shown goes at once and a late answer finds nothing
    /// outstanding. Turning lenses back on asks afresh.
    pub(crate) fn apply_code_lens_visibility(&mut self) {
        if !self.code_lenses_enabled() {
            let running = self.code_lenses.running.take();
            self.code_lenses = CodeLensState {
                running,
                ..CodeLensState::default()
            };
        }
    }

    /// Forget what every document is covered for, without dropping what is
    /// shown: a language server's state changed, so an answer given while it
    /// was not running is suspect (see `invalidate_inlay_coverage`).
    pub(crate) fn invalidate_code_lens_coverage(&mut self) {
        let state = &mut self.code_lenses;
        state.covered.clear();
        for doc in std::mem::take(&mut state.failures).into_keys() {
            state.quiet_until.remove(&doc);
        }
        state.epoch = state.epoch.saturating_add(1);
    }

    /// Note that `doc`'s text just changed, holding its next request until
    /// the edits pause.
    pub(crate) fn note_code_lens_edit(&mut self, doc: DocumentId, now: Instant) {
        self.code_lenses.failures.remove(&doc);
        let until = now + LSP_CHANGE_DEBOUNCE;
        self.code_lenses.quiet_until.insert(doc, until);
    }

    /// Carry `doc`'s held lenses across `edits`, which took its text from
    /// version `from` to `to`. A lens whose anchor an edit replaced is
    /// dropped; only a set positioned against `from` is moved, so a document
    /// reached by two paths is not moved twice.
    pub(crate) fn shift_code_lenses(
        &mut self,
        doc: DocumentId,
        from: u64,
        to: u64,
        edits: &[TextEdit],
    ) {
        let state = &mut self.code_lenses;
        if state.version.get(&doc) != Some(&from) {
            return;
        }
        if let Some(lenses) = state.held.get_mut(&doc) {
            lenses.retain_mut(|lens| {
                let Some(start) = shift_position(lens.range.start, edits) else {
                    return false;
                };
                let end = shift_position(lens.range.end, edits).unwrap_or(start);
                lens.range = Range { start, end };
                true
            });
        }
        state.version.insert(doc, to);
    }

    /// How long until a document being edited, or backing off, may be asked
    /// again, so the event loop wakes to ask without further input.
    pub(crate) fn code_lens_next_wake(&self, now: Instant) -> Option<Duration> {
        let quiet = self.code_lenses.quiet_until.values();
        quiet
            .filter(|until| **until > now)
            .map(|until| until.saturating_duration_since(now))
            .min()
    }

    /// Run the lens under a left click on a code lens row of the focused
    /// editor. Claims every left click on such a row, so a click between
    /// titles moves no caret; anything else is left alone.
    pub(crate) fn handle_code_lens_mouse(&mut self, mouse: &MouseEvent) -> bool {
        // Within the editor as last drawn: a view drawn over it clears the
        // rect, while the editor's own record of its rows outlives the frame.
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            || !rect_contains(self.editor_rect, (mouse.column, mouse.row))
        {
            return false;
        }
        let Some(tab) = self.tabs.get(self.active) else {
            return false;
        };
        let TabKind::Code {
            doc: Some(doc),
            buffer,
            ..
        } = &tab.kind
        else {
            return false;
        };
        let editor = &tab.editor;
        let Some(index) = editor.virtual_line_at(mouse.column, mouse.row) else {
            return false;
        };
        let Some(lenses) = self.code_lenses.held.get(doc) else {
            return false;
        };
        let rows = code_lens_rows(lenses, buffer, self.tab_width_for(tab));
        let column = editor.virtual_line_rect(index).map_or(0, |rect| {
            usize::from(mouse.column.saturating_sub(rect.x)) + editor.scroll_col() as usize
        });
        let lens = rows
            .get(index)
            .and_then(|row| row.lens_at(column))
            .and_then(|lens| lenses.get(lens));
        let Some(lens) = lens.filter(|lens| lens.command.is_some()).cloned() else {
            return true;
        };
        let (doc, title) = (*doc, lens.title.clone().unwrap_or_default());
        if let Some(id) = self.send(SessionCommand::RunCodeLens { doc, lens }) {
            self.code_lenses.running = Some((id, title));
        }
        true
    }

    /// Settle the answer to a lens run, reporting a failure under the lens's
    /// title. `false` when `id` answers something else.
    pub(crate) fn settle_code_lens_run(
        &mut self,
        id: Option<RequestId>,
        applied: &AppliedWorkspaceEdit,
    ) -> bool {
        let Some((_, title)) = self
            .code_lenses
            .running
            .take_if(|(pending, _)| Some(*pending) == id)
        else {
            return false;
        };
        if !applied.result.applied {
            let reason = applied.result.failure_reason.as_deref();
            let reason = reason.unwrap_or("the command did not run");
            let message = format!("{title}: {reason}");
            self.notify(Report::Failure, NotificationKind::Lsp, message);
        }
        true
    }
}
