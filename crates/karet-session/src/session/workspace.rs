//! The workspace-filesystem commands: path classification, byte reads, listings,
//! mutations, plus the client-facing viewport and view-state bookkeeping.
//!
//! Everything here used to be something the presentation layer did for itself.
//! Routing it through the seam is what lets a client render a workspace it has no
//! filesystem access to; a co-located client pays one channel hop for the same
//! answers.

use super::*;

impl Session {
    /// Hand a filesystem job to the worker.
    pub(super) fn submit_fs(&self, job: FsJob) {
        // A dead worker means the session is shutting down; the client's request
        // simply goes unanswered, exactly as it would if the process had exited.
        let _ = self.fs_worker.send(job);
    }

    /// List the primary root's files for quick-open; a rootless session has
    /// none, and says so at once.
    pub(super) fn list_files(&self, id: RequestId, limit: usize) {
        let Some(root) = self.config.roots.first().cloned() else {
            self.emit(
                Some(id),
                Event::FilesListed {
                    files: Vec::new(),
                    truncated: false,
                },
            );
            return;
        };
        self.submit_fs(FsJob::ListFiles { id, root, limit });
    }

    /// Record what `view` is displaying of `doc`.
    ///
    /// Highlights are scoped to this window, so a stale viewport costs correctness
    /// of *coverage*, never correctness of content: a client that scrolled past
    /// what it declared renders those lines unhighlighted for one round trip and
    /// then catches up.
    pub(super) fn set_viewport(
        &mut self,
        doc: DocumentId,
        view: ViewId,
        first_line: u32,
        last_line: u32,
    ) {
        if !self.store.docs.contains_key(&doc) {
            return;
        }
        let (first, last) = (first_line.min(last_line), first_line.max(last_line));
        self.viewports.insert((doc, view), (first, last));
        let Some((first, last)) = self.declared_lines(doc) else {
            return;
        };
        // Re-publish only when a view reaches outside the window last published:
        // the client sends its viewport on every scroll, and most scrolls land
        // inside the margin it already holds spans for.
        let windows = self
            .highlight_windows
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if windows
            .get(&doc)
            .is_some_and(|window| window.lines.0 <= first && last <= window.lines.1)
        {
            return;
        }
        let lines = pad_viewport(first, last);
        windows
            .entry(doc)
            .and_modify(|window| window.lines = lines)
            .or_insert(HighlightWindow {
                lines,
                narrowed: None,
            });
        self.publish(doc, None);
    }

    /// Forget the range `view` declared for `doc`, once that view closed.
    ///
    /// A document stays open while any view shows it, so this cannot wait for
    /// the document to close: a closed view's stale range would keep widening
    /// the window every other view is highlighted within.
    pub(super) fn clear_viewport(&mut self, doc: DocumentId, view: ViewId) {
        if self.viewports.remove(&(doc, view)).is_none() {
            return;
        }
        let declared = self.declared_lines(doc);
        let windows = self
            .highlight_windows
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match declared {
            Some((first, last)) => {
                if let Some(window) = windows.get_mut(&doc) {
                    window.lines = pad_viewport(first, last);
                }
            },
            // The last view went: whole-document highlights again.
            None => {
                windows.remove(&doc);
            },
        }
        self.publish(doc, None);
    }

    /// The widest line range any view of `doc` is displaying, padded by
    /// [`VIEWPORT_MARGIN`](crate::session::VIEWPORT_MARGIN).
    ///
    /// `None` when no view has declared one — a client that never sends a
    /// viewport gets whole-document highlights, which is what local mode wants
    /// and what keeps the command optional.
    #[cfg(test)]
    pub(crate) fn viewport_lines(&self, doc: DocumentId) -> Option<(u32, u32)> {
        let (first, last) = self.declared_lines(doc)?;
        Some(pad_viewport(first, last))
    }

    /// The line window `doc`'s highlights are narrowed to: the padded viewport
    /// last published, which a scroll inside it leaves in place.
    #[cfg(test)]
    pub(crate) fn highlight_window(&self, doc: DocumentId) -> Option<(u32, u32)> {
        self.highlight_windows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&doc)
            .map(|window| window.lines)
    }

    /// The widest line range any view of `doc` is displaying, unpadded.
    fn declared_lines(&self, doc: DocumentId) -> Option<(u32, u32)> {
        let mut bounds: Option<(u32, u32)> = None;
        for ((viewed, _), (first, last)) in &self.viewports {
            if *viewed != doc {
                continue;
            }
            bounds = Some(match bounds {
                Some((lo, hi)) => (lo.min(*first), hi.max(*last)),
                None => (*first, *last),
            });
        }
        bounds
    }

    /// Drop the viewports a closed document leaves behind.
    pub(crate) fn forget_viewports(&mut self, doc: DocumentId) {
        self.viewports.retain(|(viewed, _), _| *viewed != doc);
        self.highlight_windows
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&doc);
    }

    /// The view state a client checkpointed, for a later attach to restore.
    ///
    /// Test-only until an attach replays it: the server answers every attach
    /// with no view state yet (see `docs/remote.md`).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn view_state(&self) -> &[u8] {
        &self.view_state
    }
}

/// A declared line range widened by [`VIEWPORT_MARGIN`] on each side.
fn pad_viewport(first: u32, last: u32) -> (u32, u32) {
    (
        first.saturating_sub(VIEWPORT_MARGIN),
        last.saturating_add(VIEWPORT_MARGIN),
    )
}
