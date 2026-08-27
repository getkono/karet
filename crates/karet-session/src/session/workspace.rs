//! The workspace-filesystem commands: path classification, byte reads, listings,
//! mutations, plus the client-facing viewport and view-state bookkeeping.
//!
//! Everything here used to be something the presentation layer did for itself.
//! Routing it through the seam is what lets a client render a workspace it has no
//! filesystem access to; a co-located client pays one channel hop for the same
//! answers.

use super::*;
use crate::fs_worker::FsJob;

impl Session {
    /// Handle a workspace-filesystem command, reporting whether it was consumed.
    ///
    /// Runs as a pre-dispatch arm of [`Session::handle`], like the LSP and debug
    /// handlers, so the main match keeps its shape.
    pub(super) fn handle_workspace_command(&mut self, id: RequestId, command: &Command) -> bool {
        let job = match command {
            Command::SetViewport {
                doc,
                view,
                first_line,
                last_line,
            } => {
                self.set_viewport(*doc, *view, *first_line, *last_line);
                return true;
            },
            Command::CheckpointViewState { blob } => {
                blob.clone_into(&mut self.view_state);
                return true;
            },
            Command::ClassifyPath { path, ignore_size } => FsJob::Classify {
                id,
                path: path.clone(),
                ignore_size: *ignore_size,
            },
            Command::ReadFileBytes { path, offset, len } => FsJob::ReadBytes {
                id,
                path: path.clone(),
                offset: *offset,
                len: *len,
            },
            Command::ListFiles { limit } => {
                let Some(root) = self.config.roots.first().cloned() else {
                    self.emit(
                        Some(id),
                        Event::FilesListed {
                            files: Vec::new(),
                            truncated: false,
                        },
                    );
                    return true;
                };
                FsJob::ListFiles {
                    id,
                    root,
                    limit: *limit,
                }
            },
            Command::ReadDirectory {
                path,
                show_hidden,
                respect_gitignore,
            } => FsJob::ReadDirectory {
                id,
                path: path.clone(),
                show_hidden: *show_hidden,
                respect_gitignore: *respect_gitignore,
            },
            Command::MutatePath { mutation } => FsJob::Mutate {
                id,
                mutation: mutation.clone(),
            },
            _ => return false,
        };
        // A dead worker means the session is shutting down; the client's request
        // simply goes unanswered, exactly as it would if the process had exited.
        let _ = self.fs_worker.send(job);
        true
    }

    /// Record what `view` is displaying of `doc`.
    ///
    /// Highlights are scoped to this window, so a stale viewport costs correctness
    /// of *coverage*, never correctness of content: a client that scrolled past
    /// what it declared renders those lines unhighlighted for one round trip and
    /// then catches up.
    fn set_viewport(&mut self, doc: DocumentId, view: ViewId, first_line: u32, last_line: u32) {
        if !self.store.docs.contains_key(&doc) {
            return;
        }
        let (first, last) = (first_line.min(last_line), first_line.max(last_line));
        let previous = self.viewports.insert((doc, view), (first, last));
        // Re-publish only on a real move: the client sends its viewport on every
        // scroll, and most scrolls land inside the margin already sent.
        if previous != Some((first, last)) {
            self.publish(doc, None);
        }
    }

    /// The widest line range any view of `doc` is displaying, padded by
    /// [`VIEWPORT_MARGIN`](crate::session::VIEWPORT_MARGIN).
    ///
    /// `None` when no view has declared one — a client that never sends a
    /// viewport gets whole-document highlights, which is what local mode wants
    /// and what keeps the command optional.
    pub(crate) fn viewport_lines(&self, doc: DocumentId) -> Option<(u32, u32)> {
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
        let (first, last) = bounds?;
        Some((
            first.saturating_sub(VIEWPORT_MARGIN),
            last.saturating_add(VIEWPORT_MARGIN),
        ))
    }

    /// Drop the viewports a closed document leaves behind.
    pub(crate) fn forget_viewports(&mut self, doc: DocumentId) {
        self.viewports.retain(|(viewed, _), _| *viewed != doc);
    }

    /// The view state a client checkpointed, for a later attach to restore.
    #[must_use]
    pub fn view_state(&self) -> &[u8] {
        &self.view_state
    }
}
