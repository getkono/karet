//! Explorer filesystem actions, performed by the backend.
//!
//! Creating, renaming, copying and deleting used to happen here, synchronously,
//! with the error in hand. They happen on the machine that holds the files now,
//! so the answer arrives later — which is the only real change: what to do with a
//! success or a failure is unchanged, it just runs when the backend says so.

use std::path::PathBuf;

use karet_session::api::Command as SessionCommand;
use karet_session::api::PathMutation;
use karet_session::api::RequestId;
use karet_widgets::PendingEdit;

use super::App;

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
    },
}

/// A mutation in flight.
pub(super) struct PendingMutation {
    /// What was asked for, so the affected directories can be refreshed.
    pub(super) mutation: PathMutation,
    /// What to do with the answer.
    pub(super) follow_up: FollowUp,
}

impl App {
    /// Submit `mutation`, remembering what to do when it lands.
    pub(super) fn mutate_path(&mut self, mutation: PathMutation, follow_up: FollowUp) {
        let Some(id) = self.send(SessionCommand::MutatePath {
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
        // failure can still have created a parent directory on the way.
        self.invalidate_listings(&pending.mutation.dirty_parents());
        match result {
            Ok(()) => self.on_mutation_succeeded(pending.follow_up),
            Err(message) => self.on_mutation_failed(pending.follow_up, &message),
        }
        self.send_command(SessionCommand::RefreshVcs);
    }

    fn on_mutation_succeeded(&mut self, follow_up: FollowUp) {
        match follow_up {
            FollowUp::CreatedFile { path, .. } => self.open_path(&path),
            FollowUp::CreatedFolder { .. } => {},
            FollowUp::Renamed { from, to, .. } => self.retarget_open_paths(&from, &to),
            FollowUp::Deleted => {
                self.explorer_delete_done += 1;
                self.status = Some(format!("deleted {} item(s)", self.explorer_delete_done));
            },
            FollowUp::Pasted { moved } => {
                if let Some((from, to)) = moved {
                    self.retarget_open_paths(&from, &to);
                }
                self.explorer_paste_done += 1;
                self.status = Some(format!("pasted {} item(s)", self.explorer_paste_done));
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
            crate::app::Severity::Error,
            crate::app::NotificationKind::Io,
            format!("{verb} failed: {message}"),
        );
    }
}
