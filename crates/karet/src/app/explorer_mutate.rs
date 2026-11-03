//! Explorer filesystem actions, performed by the backend.
//!
//! Creating, renaming, copying and deleting used to happen here, synchronously,
//! with the error in hand. They happen on the machine that holds the files now,
//! so the answer arrives later — which is the only real change: what to do with a
//! success or a failure is unchanged, it just runs when the backend says so.

use std::collections::BTreeSet;
use std::path::PathBuf;

use karet_core::NotificationKind;
use karet_session::VcsCommand;
use karet_session::WorkspaceCommand;
use karet_session::api::PathMutation;
use karet_session::api::RequestId;
use karet_widgets::PendingEdit;

use super::App;
use crate::app::notifications::Report;
use crate::app::state::ExplorerFileOp;

/// The notification tag a paste or duplicate's running count shares.
const PASTE_TAG: &str = "explorer.paste";
/// The notification tag a delete's running count shares.
const DELETE_TAG: &str = "explorer.delete";

/// What to do once a mutation lands.
///
/// Held rather than inferred from the mutation, because the same rename means
/// different things depending on how it was started — an inline tree edit restores
/// its editor on failure, a drag does not.
pub(super) enum FollowUp {
    /// An inline "new file" edit: open the file, or restore the editor.
    CreatedFile {
        /// The file created.
        path: PathBuf,
        /// The edit to restore if it failed.
        edit: Box<PendingEdit>,
    },
    /// An inline "new folder" edit.
    CreatedFolder {
        /// The edit to restore if it failed.
        edit: Box<PendingEdit>,
    },
    /// An inline rename: retarget any open tab that was showing the old path.
    Renamed {
        /// Where it was.
        from: PathBuf,
        /// Where it went.
        to: PathBuf,
        /// The edit to restore if it failed.
        edit: Box<PendingEdit>,
    },
    /// One item of a delete, counted so the status line can report the total.
    Deleted,
    /// One item of a paste.
    Pasted {
        /// Set when the paste was a move, so open tabs follow it.
        moved: Option<(PathBuf, PathBuf)>,
        /// The names known taken when the destination was chosen. A refusal
        /// adds the refused one and picks the next free name from these.
        taken: BTreeSet<PathBuf>,
        /// How many refused names this item has already moved past.
        attempts: usize,
    },
}

/// How many names a paste tries before reporting the clash: enough for any
/// real run of `name copy N`, few enough that a backend refusing everything
/// cannot loop.
const PASTE_ATTEMPTS: usize = 64;

/// A mutation in flight.
pub(super) struct PendingMutation {
    /// What was asked for, so the affected directories can be refreshed.
    pub(super) mutation: PathMutation,
    /// What to do with the answer.
    pub(super) follow_up: FollowUp,
}

/// The same paste aimed at the next free name, when the backend refused the
/// last one because something already holds it; otherwise the follow-up to
/// report the failure with.
///
/// The explorer guesses free names from what it has listed, and it cannot have
/// listed everything: a collapsed folder, a hidden name, a file created a moment
/// ago elsewhere. The backend refuses to clobber, so a refusal is the answer
/// to the guess, and the paste moves on to `name copy`, `name copy 2`, ….
fn retry_paste(pending: PendingMutation, message: &str) -> Result<PendingMutation, FollowUp> {
    let FollowUp::Pasted {
        moved,
        mut taken,
        attempts,
    } = pending.follow_up
    else {
        return Err(pending.follow_up);
    };
    let give_up = |moved, taken| {
        Err(FollowUp::Pasted {
            moved,
            taken,
            attempts,
        })
    };
    let (from, to) = match &pending.mutation {
        PathMutation::Copy { from, to } | PathMutation::Rename { from, to } => (from, to),
        _ => return give_up(moved, taken),
    };
    let Some(dir) = to.parent() else {
        return give_up(moved, taken);
    };
    if !message.ends_with("already exists") || attempts >= PASTE_ATTEMPTS {
        return give_up(moved, taken);
    }
    taken.insert(to.clone());
    let next = super::explorer::unique_child_path(dir, from, &taken);
    taken.insert(next.clone());
    let mutation = match &pending.mutation {
        PathMutation::Rename { .. } => PathMutation::Rename {
            from: from.clone(),
            to: next.clone(),
        },
        _ => PathMutation::Copy {
            from: from.clone(),
            to: next.clone(),
        },
    };
    let moved = moved.map(|(source, _)| (source, next));
    Ok(PendingMutation {
        mutation,
        follow_up: FollowUp::Pasted {
            moved,
            taken,
            attempts: attempts + 1,
        },
    })
}

impl App {
    /// Submit `mutation`, remembering what to do when it lands.
    pub(super) fn mutate_path(&mut self, mutation: PathMutation, follow_up: FollowUp) {
        let Some(id) = self.send(WorkspaceCommand::MutatePath {
            mutation: mutation.clone(),
        }) else {
            return;
        };
        self.pending_mutations.insert(
            id,
            PendingMutation {
                mutation,
                follow_up,
            },
        );
    }

    /// Apply the outcome of a filesystem mutation.
    pub(super) fn on_path_mutated(&mut self, id: RequestId, result: Result<(), String>) {
        let Some(pending) = self.pending_mutations.remove(&id) else {
            return;
        };
        // The listings for both ends of the mutation are stale either way: a
        // failure can still have created a parent directory on the way. They
        // are read again rather than forgotten, so the rows — and the cursor on
        // them — stay until each answer replaces them.
        self.refresh_mutated_listings(&pending.mutation.dirty_parents());
        match result {
            Ok(()) => self.on_mutation_succeeded(pending.follow_up),
            Err(message) => match retry_paste(pending, &message) {
                Ok(retry) => self.mutate_path(retry.mutation, retry.follow_up),
                Err(follow_up) => self.on_mutation_failed(follow_up, &message),
            },
        }
        self.send_command(VcsCommand::Refresh);
    }

    fn on_mutation_succeeded(&mut self, follow_up: FollowUp) {
        match follow_up {
            FollowUp::CreatedFile { path, .. } => self.open_path(&path),
            FollowUp::CreatedFolder { .. } => {},
            FollowUp::Renamed { from, to, .. } => self.retarget_open_paths(&from, &to),
            // One card per batch, rewritten in place as each item lands, rather
            // than one per item.
            FollowUp::Deleted => {
                self.explorer_delete_done += 1;
                let message = format!("deleted {} item(s)", self.explorer_delete_done);
                self.notify_tagged(
                    Report::Outcome,
                    NotificationKind::Io,
                    message,
                    Some(DELETE_TAG.to_owned()),
                );
            },
            FollowUp::Pasted { moved, .. } => {
                if let Some((from, to)) = moved {
                    // The cut is spent once something it named has moved. A
                    // clipboard cut since then names other paths and stays.
                    if self.explorer_clipboard.as_ref().is_some_and(|clipboard| {
                        clipboard.op == ExplorerFileOp::Cut && clipboard.paths.contains(&from)
                    }) {
                        self.explorer_clipboard = None;
                    }
                    self.retarget_open_paths(&from, &to);
                }
                self.explorer_paste_done += 1;
                let message = format!("pasted {} item(s)", self.explorer_paste_done);
                self.notify_tagged(
                    Report::Outcome,
                    NotificationKind::Io,
                    message,
                    Some(PASTE_TAG.to_owned()),
                );
            },
        }
    }

    fn on_mutation_failed(&mut self, follow_up: FollowUp, message: &str) {
        // An inline edit gets its editor back so the user can correct the name
        // rather than retype it.
        let (verb, edit) = match follow_up {
            FollowUp::CreatedFile { edit, .. } | FollowUp::CreatedFolder { edit } => {
                ("create", Some(edit))
            },
            FollowUp::Renamed { edit, .. } => ("rename", Some(edit)),
            FollowUp::Deleted => ("delete", None),
            FollowUp::Pasted { .. } => ("paste", None),
        };
        if let Some(edit) = edit {
            self.explorer.restore_edit(&edit);
        }
        self.notify(
            Report::Failure,
            NotificationKind::Io,
            format!("{verb} failed: {message}"),
        );
    }
}
