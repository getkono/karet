//! The default diff [`Selection`] and the cheap dirty-state predicates behind it.

use crate::{Repository, VcsError, repo::to_git};
use std::ops::ControlFlow;

/// Which diff to show, mirroring VS Code's default behaviour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Selection {
    /// `HEAD` vs the index: the staged changes.
    Staged,
    /// The index vs the worktree, including untracked files: the unstaged changes.
    Unstaged,
}

impl Repository {
    /// The selection to show by default.
    ///
    /// Returns [`Selection::Staged`] if there are staged changes, else
    /// [`Selection::Unstaged`] if there are unstaged or untracked changes, else
    /// `None` when the working tree is clean.
    ///
    /// # Errors
    /// Returns [`VcsError::Git`] on failure.
    pub fn default_selection(&self) -> Result<Option<Selection>, VcsError> {
        if self.has_staged_changes()? {
            Ok(Some(Selection::Staged))
        } else if self.has_unstaged_changes()? {
            Ok(Some(Selection::Unstaged))
        } else {
            Ok(None)
        }
    }

    /// Whether `HEAD` and the index differ, stopping at the first observed change.
    fn has_staged_changes(&self) -> Result<bool, VcsError> {
        let head = self.inner.head_tree_id_or_empty().map_err(to_git)?;
        let index = self.inner.index_or_empty().map_err(to_git)?;
        let mut dirty = false;
        self.inner
            .tree_index_status(
                &head,
                &index,
                None,
                gix::status::tree_index::TrackRenames::Disabled,
                |_, _, _| {
                    dirty = true;
                    Ok::<_, std::convert::Infallible>(ControlFlow::Break(()))
                },
            )
            .map_err(to_git)?;
        Ok(dirty)
    }

    /// Whether the index and worktree differ (including untracked files), stopping at
    /// the first observed item.
    fn has_unstaged_changes(&self) -> Result<bool, VcsError> {
        let mut iter = self
            .inner
            .status(gix::progress::Discard)
            .map_err(to_git)?
            .untracked_files(gix::status::UntrackedFiles::Files)
            .into_index_worktree_iter(Vec::new())
            .map_err(to_git)?;
        match iter.next() {
            None => Ok(false),
            Some(Ok(_)) => Ok(true),
            Some(Err(e)) => Err(to_git(e)),
        }
    }
}
