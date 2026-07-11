//! Per-file before/after content extraction for a [`Selection`].
//!
//! The key idea: rather than asking `gix` to format a diff, we recover the *old* and
//! *new* content of each changed file and hand both sides to the caller, who can run
//! whatever diff they like (e.g. `karet-diff`).

use std::ops::ControlFlow;
use std::path::Path;
use std::path::PathBuf;

use crate::Repository;
use crate::StatusKind;
use crate::VcsError;
use crate::repo::to_git;
use crate::selection::Selection;

/// One changed file with full before/after text, ready for `karet-diff`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// The new path (the old path for deletions).
    pub path: PathBuf,
    /// The previous path, set only for renames.
    pub old_path: Option<PathBuf>,
    /// The change kind.
    pub status: StatusKind,
    /// Whether either side is binary; if so, `old` and `new` are empty.
    pub is_binary: bool,
    /// The "before" content (empty for additions, untracked files, or binary files).
    pub old: String,
    /// The "after" content (empty for deletions or binary files).
    pub new: String,
}

impl Repository {
    /// Collect one [`FileChange`] per changed file for `selection`, each carrying the
    /// full before/after text so the caller can diff it.
    ///
    /// `pathspec` optionally limits the result to a single path (a file, or a directory
    /// prefix), given relative to the repository root or as an absolute path inside the
    /// worktree. The returned vector is sorted by [`FileChange::path`].
    ///
    /// # Errors
    /// Returns [`VcsError::Git`] on failure.
    pub fn changes(
        &self,
        selection: Selection,
        pathspec: Option<&Path>,
    ) -> Result<Vec<FileChange>, VcsError> {
        let mut out = match selection {
            Selection::Staged => self.staged_changes(pathspec)?,
            Selection::Unstaged => self.unstaged_changes(pathspec)?,
        };
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// `HEAD` vs index changes, with full blob content from the object database.
    fn staged_changes(&self, pathspec: Option<&Path>) -> Result<Vec<FileChange>, VcsError> {
        let head = self.inner.head_tree_id_or_empty().map_err(to_git)?;
        let index = self.inner.index_or_empty().map_err(to_git)?;
        // Collect first: the callback can't borrow `self` to read blobs while it runs.
        let mut raw: Vec<gix::diff::index::Change> = Vec::new();
        self.inner
            .tree_index_status(
                &head,
                &index,
                None,
                // Force rename detection on regardless of the user's `diff.renames`
                // config, so a staged rename always shows as `R` (matching how a
                // graphical SCM view detects renames for display). `AsConfigured`
                // would honour an explicit `diff.renames=false` and degrade the
                // rename to an add + delete pair.
                gix::status::tree_index::TrackRenames::Given(gix::diff::Rewrites::default()),
                |change, _, _| {
                    raw.push(change.into_owned());
                    Ok::<_, std::convert::Infallible>(ControlFlow::Continue(()))
                },
            )
            .map_err(to_git)?;
        let mut out = Vec::with_capacity(raw.len());
        for change in raw {
            if let Some(fc) = self.staged_change(change)? {
                out.push(fc);
            }
        }
        if let Some(rel) = pathspec.and_then(|p| repo_relative(&self.inner, p))
            && !rel.as_os_str().is_empty()
        {
            out.retain(|fc| fc.path.starts_with(&rel));
        }
        Ok(out)
    }

    /// Index vs worktree changes (including untracked files), reading worktree files
    /// straight from disk for the "after" side.
    fn unstaged_changes(&self, pathspec: Option<&Path>) -> Result<Vec<FileChange>, VcsError> {
        let patterns = pathspec_patterns(&self.inner, pathspec);
        let iter = self
            .inner
            .status(gix::progress::Discard)
            .map_err(to_git)?
            .untracked_files(gix::status::UntrackedFiles::Files)
            .index_worktree_rewrites(None)
            .into_index_worktree_iter(patterns)
            .map_err(to_git)?;
        let mut out = Vec::new();
        for item in iter {
            let item = item.map_err(to_git)?;
            out.extend(self.unstaged_item(item)?);
        }
        Ok(out)
    }

    /// Convert one `HEAD` vs index change into a [`FileChange`] (or `None` to skip).
    fn staged_change(
        &self,
        change: gix::diff::index::Change,
    ) -> Result<Option<FileChange>, VcsError> {
        use gix::diff::index::Change as C;
        let fc = match change {
            C::Addition {
                location,
                id,
                entry_mode,
                ..
            } => {
                if entry_mode.is_submodule() {
                    return Ok(None);
                }
                let (bin, new) = self.read_object_text(id.into_owned())?;
                let (is_binary, old, new) = finalize(bin, String::new(), new);
                FileChange {
                    path: bstr_to_path(location.as_ref()),
                    old_path: None,
                    status: StatusKind::Added,
                    is_binary,
                    old,
                    new,
                }
            },
            C::Deletion { location, id, .. } => {
                let (bin, old) = self.read_object_text(id.into_owned())?;
                let (is_binary, old, new) = finalize(bin, old, String::new());
                FileChange {
                    path: bstr_to_path(location.as_ref()),
                    old_path: None,
                    status: StatusKind::Deleted,
                    is_binary,
                    old,
                    new,
                }
            },
            C::Modification {
                location,
                previous_id,
                id,
                ..
            } => {
                let (b1, old) = self.read_object_text(previous_id.into_owned())?;
                let (b2, new) = self.read_object_text(id.into_owned())?;
                let (is_binary, old, new) = finalize(b1 || b2, old, new);
                FileChange {
                    path: bstr_to_path(location.as_ref()),
                    old_path: None,
                    status: StatusKind::Modified,
                    is_binary,
                    old,
                    new,
                }
            },
            C::Rewrite {
                source_location,
                source_id,
                location,
                id,
                ..
            } => {
                let (b1, old) = self.read_object_text(source_id.into_owned())?;
                let (b2, new) = self.read_object_text(id.into_owned())?;
                let (is_binary, old, new) = finalize(b1 || b2, old, new);
                FileChange {
                    path: bstr_to_path(location.as_ref()),
                    old_path: Some(bstr_to_path(source_location.as_ref())),
                    status: StatusKind::Renamed,
                    is_binary,
                    old,
                    new,
                }
            },
        };
        Ok(Some(fc))
    }

    /// Convert one index vs worktree status item into zero or more [`FileChange`]s.
    ///
    /// Most items yield a single change; an untracked *directory* expands to one
    /// change per regular file it contains (see [`Self::untracked_dir_changes`]).
    fn unstaged_item(
        &self,
        item: gix::status::index_worktree::Item,
    ) -> Result<Vec<FileChange>, VcsError> {
        use gix::dir::entry::Kind;
        use gix::dir::entry::Status as DirStatus;
        use gix::status::index_worktree::Item as I;
        use gix::status::plumbing::index_as_worktree::Change as WtChange;
        use gix::status::plumbing::index_as_worktree::EntryStatus;
        match item {
            I::Modification {
                entry,
                rela_path,
                status,
                ..
            } => {
                let path = bstr_to_path(rela_path.as_ref());
                let fc = match status {
                    EntryStatus::Change(WtChange::Removed) => {
                        let (bin, old) = self.read_object_text(entry.id)?;
                        let (is_binary, old, new) = finalize(bin, old, String::new());
                        FileChange {
                            path,
                            old_path: None,
                            status: StatusKind::Deleted,
                            is_binary,
                            old,
                            new,
                        }
                    },
                    EntryStatus::Change(WtChange::Modification { .. } | WtChange::Type { .. }) => {
                        let (b1, old) = self.read_object_text(entry.id)?;
                        let (b2, new) = self.read_worktree_text(rela_path.as_ref())?;
                        let (is_binary, old, new) = finalize(b1 || b2, old, new);
                        FileChange {
                            path,
                            old_path: None,
                            status: StatusKind::Modified,
                            is_binary,
                            old,
                            new,
                        }
                    },
                    EntryStatus::IntentToAdd => {
                        let (bin, new) = self.read_worktree_text(rela_path.as_ref())?;
                        let (is_binary, old, new) = finalize(bin, String::new(), new);
                        FileChange {
                            path,
                            old_path: None,
                            status: StatusKind::Added,
                            is_binary,
                            old,
                            new,
                        }
                    },
                    // An unresolved merge conflict: show the worktree file (with its
                    // conflict markers) as the "after" side.
                    EntryStatus::Conflict { .. } => {
                        let (bin, new) = self.read_worktree_text(rela_path.as_ref())?;
                        let (is_binary, old, new) = finalize(bin, String::new(), new);
                        FileChange {
                            path,
                            old_path: None,
                            status: StatusKind::Conflicted,
                            is_binary,
                            old,
                            new,
                        }
                    },
                    // Submodule changes and bookkeeping updates are skipped.
                    _ => return Ok(Vec::new()),
                };
                Ok(vec![fc])
            },
            I::DirectoryContents { entry, .. } => match (entry.status, entry.disk_kind) {
                (DirStatus::Untracked, Some(Kind::File | Kind::Symlink)) => {
                    let (bin, new) = self.read_worktree_text(entry.rela_path.as_ref())?;
                    let (is_binary, old, new) = finalize(bin, String::new(), new);
                    Ok(vec![FileChange {
                        path: bstr_to_path(entry.rela_path.as_ref()),
                        old_path: None,
                        status: StatusKind::Untracked,
                        is_binary,
                        old,
                        new,
                    }])
                },
                // gix collapses a wholly-untracked directory into a single entry
                // (rather than emitting its files); recurse so each file is listed.
                // It only collapses when it did *not* emit the inner files, so this
                // cannot produce duplicates.
                (DirStatus::Untracked, Some(Kind::Directory)) => {
                    self.untracked_dir_changes(&bstr_to_path(entry.rela_path.as_ref()))
                },
                _ => Ok(Vec::new()),
            },
            // Rewrite items don't fire while index-worktree rename tracking is disabled.
            I::Rewrite { .. } => Ok(Vec::new()),
        }
    }

    /// List every regular file inside the untracked directory at repo-relative
    /// `dir`, as one [`StatusKind::Untracked`] [`FileChange`] each. Symlinked
    /// subdirectories are skipped to avoid cycles.
    fn untracked_dir_changes(&self, dir: &Path) -> Result<Vec<FileChange>, VcsError> {
        let Some(root) = self.inner.workdir() else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        // (repo-relative path, absolute path) pairs still to visit.
        let mut stack = vec![(dir.to_path_buf(), root.join(dir))];
        while let Some((rel, abs)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&abs) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let child_rel = rel.join(&name);
                let child_abs = abs.join(&name);
                // `file_type` follows no symlink, so a symlinked dir reports as a
                // symlink and is treated as a file (never recursed into).
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_dir() {
                    stack.push((child_rel, child_abs));
                } else {
                    let (bin, new) = match std::fs::read(&child_abs) {
                        Ok(data) => classify(&data),
                        Err(_) => (true, String::new()),
                    };
                    let (is_binary, old, new) = finalize(bin, String::new(), new);
                    out.push(FileChange {
                        path: child_rel,
                        old_path: None,
                        status: StatusKind::Untracked,
                        is_binary,
                        old,
                        new,
                    });
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// Read object `id` as text, returning `(is_binary, text)`; `text` is empty when binary.
    fn read_object_text(&self, id: impl Into<gix::ObjectId>) -> Result<(bool, String), VcsError> {
        let object = self.inner.find_object(id).map_err(to_git)?;
        Ok(classify(&object.data))
    }

    /// Read the worktree file at repo-relative `rela` as text, returning `(is_binary, text)`.
    /// A missing or unreadable file is treated as an empty binary side.
    fn read_worktree_text(&self, rela: &gix::bstr::BStr) -> Result<(bool, String), VcsError> {
        match self.inner.workdir_path(rela) {
            Some(abs) => match std::fs::read(&abs) {
                Ok(data) => Ok(classify(&data)),
                Err(_) => Ok((true, String::new())),
            },
            None => Ok((true, String::new())),
        }
    }
}

impl Repository {
    /// Collect one [`FileChange`] per file this commit changed, each with the full
    /// before/after text so the caller can diff it (via `karet-diff`).
    ///
    /// The commit's tree is diffed against its **first parent** — GitHub's default view
    /// of a merge. A root commit (no parents) diffs against the empty tree, so every
    /// file reads as an addition. Rename detection is forced on (as in the staged view),
    /// and submodule/tree entries are skipped. The result is sorted by
    /// [`FileChange::path`].
    ///
    /// # Errors
    /// Returns [`VcsError::Git`] if `rev` does not resolve to a commit, or on failure.
    pub fn commit_changes(&self, rev: &str) -> Result<Vec<FileChange>, VcsError> {
        let commit = self.inner.find_commit(self.resolve(rev)?).map_err(to_git)?;
        let new_tree = commit.tree().map_err(to_git)?;
        let parent_tree = match commit.parent_ids().next() {
            Some(pid) => Some(
                self.inner
                    .find_commit(pid)
                    .map_err(to_git)?
                    .tree()
                    .map_err(to_git)?,
            ),
            None => None,
        };
        self.diff_trees(parent_tree.as_ref(), Some(&new_tree))
    }

    /// The raw bytes of the file at `path` as it existed in revision `rev` (a hash,
    /// ref, branch, `HEAD`, `HEAD~2`, …), or `None` when no blob exists there at that
    /// revision — the path is absent, or names a directory/submodule rather than a
    /// file.
    ///
    /// `path` is resolved the same way as [`file_history`](Self::file_history): a
    /// relative path is relative to the process's current directory, and a path
    /// outside the worktree yields `Ok(None)`. Bytes are returned verbatim (binary
    /// content included), so the caller decides how to interpret them.
    ///
    /// # Errors
    /// Returns [`VcsError::Git`] if `rev` does not resolve to a commit, or on read
    /// failure.
    pub fn file_at_rev(&self, path: &Path, rev: &str) -> Result<Option<Vec<u8>>, VcsError> {
        let Some(rel) = repo_relative(&self.inner, path) else {
            return Ok(None);
        };
        let commit = self.inner.find_commit(self.resolve(rev)?).map_err(to_git)?;
        let tree = commit.tree().map_err(to_git)?;
        let Some(entry) = tree.lookup_entry_by_path(&rel).map_err(to_git)? else {
            return Ok(None);
        };
        if !entry.mode().is_blob_or_symlink() {
            return Ok(None);
        }
        let object = self
            .inner
            .find_object(entry.id().detach())
            .map_err(to_git)?;
        Ok(Some(object.data.clone()))
    }

    /// Collect one [`FileChange`] per file that differs between two arbitrary revisions,
    /// each carrying full before/after text so the caller can diff it (via `karet-diff`).
    ///
    /// The diff runs from `base_rev` (the "before" side) to `head_rev` (the "after"
    /// side): a two-dot `base..head`. When `merge_base` is set it is a three-dot
    /// `base...head` — the base is replaced with the [merge base](Self::merge_base) of the
    /// two revisions, so the result is exactly what `head` introduced since it diverged
    /// from `base` (a pull-request-style diff), ignoring anything `base` gained meanwhile.
    /// Rename detection is forced on and submodule/tree entries are skipped, as in
    /// [`commit_changes`](Self::commit_changes). The result is sorted by
    /// [`FileChange::path`].
    ///
    /// # Errors
    /// Returns [`VcsError::Git`] if either revision does not resolve, if `merge_base` is
    /// set but the two revisions share no common ancestor, or on read failure.
    pub fn range_changes(
        &self,
        base_rev: &str,
        head_rev: &str,
        merge_base: bool,
    ) -> Result<Vec<FileChange>, VcsError> {
        let head_id = self.resolve(head_rev)?;
        let base_id = if merge_base {
            self.merge_base_id(self.resolve(base_rev)?, head_id)?
                .ok_or_else(|| VcsError::Git("the revisions share no common ancestor".into()))?
        } else {
            self.resolve(base_rev)?
        };
        let base_tree = self
            .inner
            .find_commit(base_id)
            .map_err(to_git)?
            .tree()
            .map_err(to_git)?;
        let head_tree = self
            .inner
            .find_commit(head_id)
            .map_err(to_git)?
            .tree()
            .map_err(to_git)?;
        self.diff_trees(Some(&base_tree), Some(&head_tree))
    }

    /// Diff two optional trees (`None` = the empty tree) into sorted [`FileChange`]s,
    /// reading both blob sides from the object database. Rename detection is forced on
    /// regardless of the user's `diff.renames` config, so a rename always shows as `R`.
    fn diff_trees(
        &self,
        old: Option<&gix::Tree<'_>>,
        new: Option<&gix::Tree<'_>>,
    ) -> Result<Vec<FileChange>, VcsError> {
        let opts = gix::diff::Options::default().with_rewrites(Some(Default::default()));
        let raw = self
            .inner
            .diff_tree_to_tree(old, new, Some(opts))
            .map_err(to_git)?;
        let mut out = Vec::with_capacity(raw.len());
        for change in raw {
            if let Some(fc) = self.tree_change(change)? {
                out.push(fc);
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// Resolve a revision spec (a hash, ref, `HEAD`, `HEAD~2`, `@{upstream}`, …) to its
    /// object id.
    fn resolve(&self, rev: &str) -> Result<gix::ObjectId, VcsError> {
        use gix::bstr::ByteSlice;
        Ok(self
            .inner
            .rev_parse_single(rev.as_bytes().as_bstr())
            .map_err(to_git)?
            .detach())
    }

    /// The best merge base of two object ids, or `None` when their histories are
    /// unrelated (no common ancestor).
    fn merge_base_id(
        &self,
        a: gix::ObjectId,
        b: gix::ObjectId,
    ) -> Result<Option<gix::ObjectId>, VcsError> {
        Ok(self
            .inner
            .merge_bases_many(a, &[b])
            .map_err(to_git)?
            .first()
            .map(|id| id.detach()))
    }

    /// The full hex hash of the best merge base of two revisions (their common ancestor),
    /// or `None` when they share no common ancestor.
    ///
    /// # Errors
    /// Returns [`VcsError::Git`] if either revision does not resolve, or on failure.
    pub fn merge_base(&self, a_rev: &str, b_rev: &str) -> Result<Option<String>, VcsError> {
        let a = self.resolve(a_rev)?;
        let b = self.resolve(b_rev)?;
        Ok(self.merge_base_id(a, b)?.map(|id| id.to_hex().to_string()))
    }

    /// The short name of the current branch's upstream (tracking) branch — e.g.
    /// `origin/main` — resolved from `branch.<name>.remote` / `.merge`, or `None` when
    /// `HEAD` is detached or the branch has no configured upstream. The returned name is
    /// itself a valid revision (it resolves to the remote-tracking ref), so it can be
    /// passed straight back to [`range_changes`](Self::range_changes).
    ///
    /// # Errors
    /// Returns [`VcsError::Git`] on failure to read the head or the tracking ref.
    pub fn upstream_of_head(&self) -> Result<Option<String>, VcsError> {
        use gix::bstr::ByteSlice;
        let Some(head) = self.inner.head_name().map_err(to_git)? else {
            return Ok(None); // detached HEAD
        };
        match self
            .inner
            .branch_remote_tracking_ref_name(head.as_ref(), gix::remote::Direction::Fetch)
        {
            Some(Ok(name)) => Ok(Some(name.shorten().to_str_lossy().into_owned())),
            Some(Err(e)) => Err(to_git(e)),
            None => Ok(None),
        }
    }

    /// A best-guess base branch to compare the current branch against, for a
    /// "changes since base" diff: the first of `main`, `master`, `develop`,
    /// `origin/main`, `origin/master` that exists and is not the current branch, or
    /// `None` when none apply. The returned name is a valid revision.
    #[must_use]
    pub fn default_base_branch(&self) -> Option<String> {
        use gix::bstr::ByteSlice;
        let current = self
            .inner
            .head_name()
            .ok()
            .flatten()
            .map(|n| n.shorten().to_str_lossy().into_owned());
        for cand in ["main", "master", "develop", "origin/main", "origin/master"] {
            if current.as_deref() == Some(cand) {
                continue;
            }
            if self.resolve(cand).is_ok() {
                return Some(cand.to_string());
            }
        }
        None
    }

    /// Convert one tree-to-tree change into a [`FileChange`] (or `None` to skip a
    /// submodule/tree entry), reading both blob sides from the object database.
    fn tree_change(
        &self,
        change: gix::object::tree::diff::ChangeDetached,
    ) -> Result<Option<FileChange>, VcsError> {
        use gix::object::tree::diff::ChangeDetached as C;
        let fc = match change {
            C::Addition {
                location,
                entry_mode,
                id,
                ..
            } => {
                if !entry_mode.is_blob_or_symlink() {
                    return Ok(None);
                }
                let (bin, new) = self.read_object_text(id)?;
                let (is_binary, old, new) = finalize(bin, String::new(), new);
                FileChange {
                    path: bstr_to_path(location.as_ref()),
                    old_path: None,
                    status: StatusKind::Added,
                    is_binary,
                    old,
                    new,
                }
            },
            C::Deletion {
                location,
                entry_mode,
                id,
                ..
            } => {
                if !entry_mode.is_blob_or_symlink() {
                    return Ok(None);
                }
                let (bin, old) = self.read_object_text(id)?;
                let (is_binary, old, new) = finalize(bin, old, String::new());
                FileChange {
                    path: bstr_to_path(location.as_ref()),
                    old_path: None,
                    status: StatusKind::Deleted,
                    is_binary,
                    old,
                    new,
                }
            },
            C::Modification {
                location,
                entry_mode,
                previous_id,
                id,
                ..
            } => {
                if !entry_mode.is_blob_or_symlink() {
                    return Ok(None);
                }
                let (b1, old) = self.read_object_text(previous_id)?;
                let (b2, new) = self.read_object_text(id)?;
                let (is_binary, old, new) = finalize(b1 || b2, old, new);
                FileChange {
                    path: bstr_to_path(location.as_ref()),
                    old_path: None,
                    status: StatusKind::Modified,
                    is_binary,
                    old,
                    new,
                }
            },
            C::Rewrite {
                source_location,
                source_id,
                location,
                id,
                ..
            } => {
                let (b1, old) = self.read_object_text(source_id)?;
                let (b2, new) = self.read_object_text(id)?;
                let (is_binary, old, new) = finalize(b1 || b2, old, new);
                FileChange {
                    path: bstr_to_path(location.as_ref()),
                    old_path: Some(bstr_to_path(source_location.as_ref())),
                    status: StatusKind::Renamed,
                    is_binary,
                    old,
                    new,
                }
            },
        };
        Ok(Some(fc))
    }
}

/// Convert a repo-relative byte path into a [`PathBuf`].
fn bstr_to_path(location: &gix::bstr::BStr) -> PathBuf {
    gix::path::from_bstr(location).into_owned()
}

/// Classify `bytes` as `(is_binary, text)`: binary if a NUL byte appears in the first
/// 8 KiB or the content is not valid UTF-8. The returned text is empty when binary.
fn classify(bytes: &[u8]) -> (bool, String) {
    if bytes.iter().take(8000).any(|&b| b == 0) {
        return (true, String::new());
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => (false, s.to_owned()),
        Err(_) => (true, String::new()),
    }
}

/// Force both sides empty when either is binary, producing the final `(is_binary, old, new)`.
fn finalize(is_binary: bool, old: String, new: String) -> (bool, String, String) {
    if is_binary {
        (true, String::new(), String::new())
    } else {
        (false, old, new)
    }
}

/// Turn `pathspec` into repo-relative pattern(s) for the index-worktree walk.
fn pathspec_patterns(repo: &gix::Repository, pathspec: Option<&Path>) -> Vec<gix::bstr::BString> {
    // A path equal to the repository root makes an empty relative pattern, which gix
    // rejects ("not a valid pathspec"); an empty pattern just means "no filter".
    pathspec
        .and_then(|p| repo_relative(repo, p))
        .map(|rel| gix::path::into_bstr(rel).into_owned())
        .filter(|pattern| !pattern.is_empty())
        .into_iter()
        .collect()
}

/// Make `path` repo-relative by stripping the worktree prefix.
///
/// `path` comes from the CLI, so a *relative* path is relative to the process's
/// current directory — not the repository root. Both `path` and the worktree are
/// canonicalized (resolving `.`, `..`, and symlinks) before the strip, so a
/// current-directory-relative path that points at or inside the worktree maps
/// correctly. Returns `None` — treated by the callers as "no filter" — when `path`
/// lies outside the worktree or cannot be resolved.
pub(crate) fn repo_relative(repo: &gix::Repository, path: &Path) -> Option<PathBuf> {
    let workdir = repo.workdir()?;
    let abs_path = std::fs::canonicalize(path).ok()?;
    let abs_workdir = std::fs::canonicalize(workdir).ok()?;
    abs_path
        .strip_prefix(&abs_workdir)
        .ok()
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicU32;
    use std::sync::atomic::Ordering;

    use crate::Repository;
    use crate::Selection;
    use crate::StatusKind;
    use crate::VcsError;

    static COUNTER: AtomicU32 = AtomicU32::new(0);
    /// Serializes the tests that mutate the process-wide current directory.
    static CWD_LOCK: Mutex<()> = Mutex::new(());

    /// Restores the working directory on drop, so a panic can't leak the change.
    struct CwdGuard(PathBuf);

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }

    /// A temp directory removed on drop.
    struct TempRepo {
        path: PathBuf,
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// Allocate a unique, not-yet-created temp directory path.
    fn unique_dir() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("karet-vcs-{}-{}", std::process::id(), n))
    }

    /// Run `git` in `dir`, surfacing any failure as a [`VcsError`].
    fn git(dir: &Path, args: &[&str]) -> Result<(), VcsError> {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .map_err(|e| VcsError::Git(e.to_string()))?;
        if status.success() {
            Ok(())
        } else {
            Err(VcsError::Git(format!("git {args:?} exited with {status}")))
        }
    }

    /// Create an initialized repository in a fresh temp directory.
    fn init_repo() -> Result<TempRepo, VcsError> {
        let path = unique_dir();
        std::fs::create_dir_all(&path).map_err(|e| VcsError::Git(e.to_string()))?;
        let repo = TempRepo { path };
        git(&repo.path, &["init", "-q"])?;
        git(&repo.path, &["config", "user.email", "test@example.com"])?;
        git(&repo.path, &["config", "user.name", "karet test"])?;
        git(&repo.path, &["config", "commit.gpgsign", "false"])?;
        Ok(repo)
    }

    /// Write `contents` to `rel` inside `dir`.
    fn write(dir: &Path, rel: &str, contents: &[u8]) -> Result<(), VcsError> {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| VcsError::Git(e.to_string()))?;
        }
        std::fs::write(&path, contents).map_err(|e| VcsError::Git(e.to_string()))
    }

    #[test]
    fn discover_non_repo_then_repo() -> Result<(), VcsError> {
        let dir = unique_dir();
        std::fs::create_dir_all(&dir).map_err(|e| VcsError::Git(e.to_string()))?;
        let guard = TempRepo { path: dir.clone() };

        assert!(matches!(
            Repository::discover(&dir),
            Err(VcsError::NotARepository)
        ));

        git(&dir, &["init", "-q"])?;
        assert!(Repository::discover(&dir).is_ok());

        drop(guard);
        Ok(())
    }

    #[test]
    fn untracked_file_is_unstaged() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.path, "new.txt", b"hello\n")?;
        let r = Repository::discover(&repo.path)?;

        assert_eq!(r.default_selection()?, Some(Selection::Unstaged));

        let changes = r.changes(Selection::Unstaged, None)?;
        assert_eq!(changes.len(), 1);
        let fc = &changes[0];
        assert_eq!(fc.path, PathBuf::from("new.txt"));
        assert_eq!(fc.status, StatusKind::Untracked);
        assert!(!fc.is_binary);
        assert_eq!(fc.old, "");
        assert_eq!(fc.new, "hello\n");
        Ok(())
    }

    #[test]
    fn staged_modification_has_old_and_new() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.path, "a.txt", b"one\n")?;
        git(&repo.path, &["add", "a.txt"])?;
        git(&repo.path, &["commit", "-q", "-m", "init"])?;
        write(&repo.path, "a.txt", b"two\n")?;
        git(&repo.path, &["add", "a.txt"])?;
        let r = Repository::discover(&repo.path)?;

        assert_eq!(r.default_selection()?, Some(Selection::Staged));

        let changes = r.changes(Selection::Staged, None)?;
        assert_eq!(changes.len(), 1);
        let fc = &changes[0];
        assert_eq!(fc.path, PathBuf::from("a.txt"));
        assert_eq!(fc.status, StatusKind::Modified);
        assert_eq!(fc.old, "one\n");
        assert_eq!(fc.new, "two\n");
        Ok(())
    }

    #[test]
    fn unstaged_modification_has_old_and_new() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.path, "a.txt", b"one\n")?;
        git(&repo.path, &["add", "a.txt"])?;
        git(&repo.path, &["commit", "-q", "-m", "init"])?;
        write(&repo.path, "a.txt", b"two\n")?; // modify the worktree, do not stage
        let r = Repository::discover(&repo.path)?;

        let changes = r.changes(Selection::Unstaged, None)?;
        assert_eq!(changes.len(), 1);
        let fc = &changes[0];
        assert_eq!(fc.path, PathBuf::from("a.txt"));
        assert_eq!(fc.status, StatusKind::Modified);
        assert_eq!(fc.old, "one\n");
        assert_eq!(fc.new, "two\n");
        Ok(())
    }

    #[test]
    fn binary_file_is_marked_binary() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.path, "bin.dat", b"abc\x00def")?;
        let r = Repository::discover(&repo.path)?;

        let changes = r.changes(Selection::Unstaged, None)?;
        assert_eq!(changes.len(), 1);
        let fc = &changes[0];
        assert!(fc.is_binary);
        assert_eq!(fc.old, "");
        assert_eq!(fc.new, "");
        Ok(())
    }

    #[test]
    fn clean_repo_has_no_default_selection() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.path, "a.txt", b"one\n")?;
        git(&repo.path, &["add", "a.txt"])?;
        git(&repo.path, &["commit", "-q", "-m", "init"])?;
        let r = Repository::discover(&repo.path)?;

        assert_eq!(r.default_selection()?, None);
        Ok(())
    }

    #[test]
    fn pathspec_equal_to_repo_root_is_no_filter() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.path, "a.txt", b"hi\n")?;
        let r = Repository::discover(&repo.path)?;
        // Passing the repo root as the pathspec yields an empty relative pattern,
        // which must be treated as "no filter" rather than erroring.
        let changes = r.changes(Selection::Unstaged, Some(&repo.path))?;
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, PathBuf::from("a.txt"));
        Ok(())
    }

    #[test]
    fn cwd_relative_pathspec_resolves_against_current_dir() -> Result<(), VcsError> {
        // Mirrors `karet ../chat`: a relative pathspec is relative to the process's
        // current directory, not the repo root. Pointing it at the repo root itself
        // must reduce to an empty (no-op) filter, so the change is still reported.
        let repo = init_repo()?;
        write(&repo.path, "a.txt", b"hi\n")?;
        let parent = repo
            .path
            .parent()
            .ok_or_else(|| VcsError::Git("temp repo has no parent".into()))?
            .to_path_buf();
        let name = repo
            .path
            .file_name()
            .ok_or_else(|| VcsError::Git("temp repo has no name".into()))?
            .to_owned();

        let _lock = CWD_LOCK.lock().map_err(|e| VcsError::Git(e.to_string()))?;
        let original = std::env::current_dir().map_err(|e| VcsError::Git(e.to_string()))?;
        let _restore = CwdGuard(original);
        std::env::set_current_dir(&parent).map_err(|e| VcsError::Git(e.to_string()))?;

        let r = Repository::discover(Path::new(&name))?;
        let changes = r.changes(Selection::Unstaged, Some(Path::new(&name)))?;
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, PathBuf::from("a.txt"));
        Ok(())
    }

    #[test]
    fn untracked_files_in_new_dir_are_listed() -> Result<(), VcsError> {
        // A brand-new, wholly-untracked directory tree: every file inside it must
        // surface, not just the top-level ones.
        let repo = init_repo()?;
        write(&repo.path, "newdir/a.txt", b"aaa\n")?;
        write(&repo.path, "newdir/sub/b.txt", b"bbb\n")?;
        let r = Repository::discover(&repo.path)?;

        let changes = r.changes(Selection::Unstaged, None)?;
        let paths: Vec<PathBuf> = changes.iter().map(|c| c.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("newdir/a.txt"),
                PathBuf::from("newdir/sub/b.txt"),
            ]
        );
        assert!(changes.iter().all(|c| c.status == StatusKind::Untracked));
        Ok(())
    }

    #[test]
    fn staged_rename_is_detected_even_when_config_disables_renames() -> Result<(), VcsError> {
        let repo = init_repo()?;
        // Explicitly turn rename detection off in config; the SCM view forces it on.
        git(&repo.path, &["config", "diff.renames", "false"])?;
        write(&repo.path, "old.txt", b"one\ntwo\nthree\nfour\nfive\n")?;
        git(&repo.path, &["add", "old.txt"])?;
        git(&repo.path, &["commit", "-q", "-m", "init"])?;
        // `git mv` stages the rename (updates the index).
        git(&repo.path, &["mv", "old.txt", "new.txt"])?;
        let r = Repository::discover(&repo.path)?;

        let changes = r.changes(Selection::Staged, None)?;
        let fc = changes
            .iter()
            .find(|c| c.path.as_path() == Path::new("new.txt"))
            .ok_or_else(|| VcsError::Git("renamed file not reported".into()))?;
        assert_eq!(fc.status, StatusKind::Renamed);
        assert_eq!(fc.old_path.as_deref(), Some(Path::new("old.txt")));
        Ok(())
    }

    #[test]
    fn staged_changes_during_a_merge_do_not_error() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.path, "a.txt", b"base\n")?;
        git(&repo.path, &["add", "a.txt"])?;
        git(&repo.path, &["commit", "-q", "-m", "base"])?;
        git(&repo.path, &["checkout", "-q", "-b", "feature"])?;
        write(&repo.path, "a.txt", b"feature\n")?;
        git(&repo.path, &["commit", "-q", "-am", "feature"])?;
        git(&repo.path, &["checkout", "-q", "-"])?;
        write(&repo.path, "a.txt", b"main\n")?;
        git(&repo.path, &["commit", "-q", "-am", "main"])?;
        // The merge conflicts, leaving unmerged (stage 1/2/3) entries in the index.
        let _ = git(&repo.path, &["merge", "--no-edit", "feature"]);
        let r = Repository::discover(&repo.path)?;

        // The staged (HEAD↔index) read must succeed over an unmerged index rather
        // than error — otherwise `Session::compute_vcs` swallows it into an empty
        // staged section, and status "breaks" during a merge. (It may legitimately
        // be empty; the point is that the `?` below does not propagate an error.)
        r.changes(Selection::Staged, None)?;
        // The conflict is still surfaced on the unstaged side.
        let unstaged = r.changes(Selection::Unstaged, None)?;
        assert!(unstaged.iter().any(|c| c.status == StatusKind::Conflicted));
        Ok(())
    }

    #[test]
    fn commit_changes_diffs_against_first_parent() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.path, "a.txt", b"one\n")?;
        git(&repo.path, &["add", "."])?;
        git(&repo.path, &["commit", "-q", "-m", "init"])?;
        write(&repo.path, "a.txt", b"one\ntwo\n")?;
        write(&repo.path, "b.txt", b"new file\n")?;
        git(&repo.path, &["add", "."])?;
        git(&repo.path, &["commit", "-q", "-m", "second"])?;
        let r = Repository::discover(&repo.path)?;

        let changes = r.commit_changes("HEAD")?;
        assert_eq!(changes.len(), 2);
        let a = changes
            .iter()
            .find(|c| c.path.as_path() == Path::new("a.txt"))
            .ok_or_else(|| VcsError::Git("a.txt missing".into()))?;
        assert_eq!(a.status, StatusKind::Modified);
        assert_eq!(a.old, "one\n");
        assert_eq!(a.new, "one\ntwo\n");
        let b = changes
            .iter()
            .find(|c| c.path.as_path() == Path::new("b.txt"))
            .ok_or_else(|| VcsError::Git("b.txt missing".into()))?;
        assert_eq!(b.status, StatusKind::Added);
        assert_eq!(b.new, "new file\n");
        Ok(())
    }

    #[test]
    fn commit_changes_on_root_are_all_additions() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.path, "a.txt", b"one\n")?;
        git(&repo.path, &["add", "."])?;
        git(&repo.path, &["commit", "-q", "-m", "root"])?;
        let r = Repository::discover(&repo.path)?;

        let changes = r.commit_changes("HEAD")?;
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].status, StatusKind::Added);
        assert_eq!(changes[0].old, "");
        assert_eq!(changes[0].new, "one\n");
        Ok(())
    }

    #[test]
    fn commit_changes_detect_renames() -> Result<(), VcsError> {
        let repo = init_repo()?;
        git(&repo.path, &["config", "diff.renames", "false"])?;
        write(&repo.path, "old.txt", b"one\ntwo\nthree\nfour\nfive\n")?;
        git(&repo.path, &["add", "."])?;
        git(&repo.path, &["commit", "-q", "-m", "init"])?;
        git(&repo.path, &["mv", "old.txt", "new.txt"])?;
        git(&repo.path, &["commit", "-q", "-m", "rename"])?;
        let r = Repository::discover(&repo.path)?;

        let changes = r.commit_changes("HEAD")?;
        let fc = changes
            .iter()
            .find(|c| c.path.as_path() == Path::new("new.txt"))
            .ok_or_else(|| VcsError::Git("renamed file not reported".into()))?;
        assert_eq!(fc.status, StatusKind::Renamed);
        assert_eq!(fc.old_path.as_deref(), Some(Path::new("old.txt")));
        Ok(())
    }

    #[test]
    fn conflicted_file_is_reported() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.path, "a.txt", b"base\n")?;
        git(&repo.path, &["add", "a.txt"])?;
        git(&repo.path, &["commit", "-q", "-m", "base"])?;
        // Diverge the same line on a feature branch and on the original branch.
        git(&repo.path, &["checkout", "-q", "-b", "feature"])?;
        write(&repo.path, "a.txt", b"feature\n")?;
        git(&repo.path, &["commit", "-q", "-am", "feature"])?;
        git(&repo.path, &["checkout", "-q", "-"])?;
        write(&repo.path, "a.txt", b"main\n")?;
        git(&repo.path, &["commit", "-q", "-am", "main"])?;
        // The merge conflicts; `git merge` exits non-zero, which is expected here.
        let _ = git(&repo.path, &["merge", "--no-edit", "feature"]);
        let r = Repository::discover(&repo.path)?;

        let changes = r.changes(Selection::Unstaged, None)?;
        let fc = changes
            .iter()
            .find(|c| c.path.as_path() == Path::new("a.txt"))
            .ok_or_else(|| VcsError::Git("conflicted file not reported".into()))?;
        assert_eq!(fc.status, StatusKind::Conflicted);
        Ok(())
    }

    /// A repo whose `main` forks into `feature`, with `main` also advancing afterwards.
    /// Returns the repo so callers can diff `main` against `feature` two ways.
    fn forked_repo() -> Result<TempRepo, VcsError> {
        let repo = init_repo()?;
        write(&repo.path, "base.txt", b"base\n")?;
        git(&repo.path, &["add", "."])?;
        git(&repo.path, &["commit", "-q", "-m", "base"])?;
        git(&repo.path, &["branch", "-M", "main"])?;
        // feature adds feature.txt on top of the fork point.
        git(&repo.path, &["checkout", "-q", "-b", "feature"])?;
        write(&repo.path, "feature.txt", b"feature\n")?;
        git(&repo.path, &["add", "."])?;
        git(&repo.path, &["commit", "-q", "-m", "add feature"])?;
        // main advances independently, changing base.txt after the fork.
        git(&repo.path, &["checkout", "-q", "main"])?;
        write(&repo.path, "base.txt", b"base changed on main\n")?;
        git(&repo.path, &["commit", "-q", "-am", "advance main"])?;
        Ok(repo)
    }

    #[test]
    fn range_changes_two_dot_diffs_the_two_tips() -> Result<(), VcsError> {
        let repo = forked_repo()?;
        let r = Repository::discover(&repo.path)?;
        // main..feature is the raw tree diff between the tips: feature adds feature.txt,
        // and (relative to main's newer tip) reverts main's change to base.txt.
        let changes = r.range_changes("main", "feature", false)?;
        let paths: Vec<_> = changes.iter().map(|c| c.path.clone()).collect();
        assert!(paths.contains(&PathBuf::from("feature.txt")));
        assert!(
            paths.contains(&PathBuf::from("base.txt")),
            "two-dot reflects main's later change to base.txt"
        );
        Ok(())
    }

    #[test]
    fn range_changes_three_dot_uses_the_merge_base() -> Result<(), VcsError> {
        let repo = forked_repo()?;
        let r = Repository::discover(&repo.path)?;
        // main...feature diffs from the fork point, so only feature's own change shows;
        // main's later change to base.txt is excluded.
        let changes = r.range_changes("main", "feature", true)?;
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, PathBuf::from("feature.txt"));
        assert_eq!(changes[0].status, StatusKind::Added);
        Ok(())
    }

    #[test]
    fn merge_base_finds_the_fork_point() -> Result<(), VcsError> {
        let repo = forked_repo()?;
        let r = Repository::discover(&repo.path)?;
        let base = r
            .merge_base("main", "feature")?
            .ok_or_else(|| VcsError::Git("expected a merge base".into()))?;
        // The fork point is the "base" commit — the first-parent of feature's tip.
        let expected = r.commit_detail("feature~1")?.hash;
        assert_eq!(base, expected);
        Ok(())
    }

    #[test]
    fn merge_base_is_none_for_unrelated_histories() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.path, "a.txt", b"a\n")?;
        git(&repo.path, &["add", "."])?;
        git(&repo.path, &["commit", "-q", "-m", "first"])?;
        git(&repo.path, &["branch", "-M", "main"])?;
        // An orphan branch has no common ancestor with main.
        git(&repo.path, &["checkout", "-q", "--orphan", "other"])?;
        write(&repo.path, "b.txt", b"b\n")?;
        git(&repo.path, &["add", "."])?;
        git(&repo.path, &["commit", "-q", "-m", "orphan"])?;
        let r = Repository::discover(&repo.path)?;
        assert!(r.merge_base("main", "other")?.is_none());
        // A three-dot range over unrelated histories is an error, not a silent success.
        assert!(r.range_changes("main", "other", true).is_err());
        Ok(())
    }

    #[test]
    fn default_base_branch_skips_the_current_branch() -> Result<(), VcsError> {
        let repo = forked_repo()?; // leaves us on `main`
        let r = Repository::discover(&repo.path)?;
        // On main, main is excluded and no other candidate exists.
        assert_eq!(r.default_base_branch(), None);
        // On feature, main is the detected base.
        git(&repo.path, &["checkout", "-q", "feature"])?;
        let r = Repository::discover(&repo.path)?;
        assert_eq!(r.default_base_branch().as_deref(), Some("main"));
        Ok(())
    }

    #[test]
    fn upstream_of_head_is_none_without_a_tracking_branch() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.path, "a.txt", b"a\n")?;
        git(&repo.path, &["add", "."])?;
        git(&repo.path, &["commit", "-q", "-m", "first"])?;
        let r = Repository::discover(&repo.path)?;
        // A fresh local repo has no remote, so the branch has no upstream.
        assert!(r.upstream_of_head()?.is_none());
        Ok(())
    }

    #[test]
    fn file_at_rev_reads_each_revision_and_reports_absence() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.path, "a.txt", b"v0\n")?;
        git(&repo.path, &["add", "."])?;
        git(&repo.path, &["commit", "-q", "-m", "c0"])?;
        write(&repo.path, "a.txt", b"v1\n")?;
        git(&repo.path, &["commit", "-q", "-am", "c1"])?;
        let r = Repository::discover(&repo.path)?;

        let path = repo.path.join("a.txt");
        // HEAD holds the newest content; the previous commit holds the older one.
        assert_eq!(r.file_at_rev(&path, "HEAD")?.as_deref(), Some(&b"v1\n"[..]));
        assert_eq!(
            r.file_at_rev(&path, "HEAD~1")?.as_deref(),
            Some(&b"v0\n"[..])
        );
        // A path that does not exist at that revision is `None`, not an error.
        assert!(
            r.file_at_rev(&repo.path.join("missing.txt"), "HEAD")?
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn file_at_rev_returns_bytes_verbatim_for_binary_content() -> Result<(), VcsError> {
        let repo = init_repo()?;
        let bytes = b"\x00\x01\x02rust\xff\xfe";
        write(&repo.path, "bin.dat", bytes)?;
        git(&repo.path, &["add", "."])?;
        git(&repo.path, &["commit", "-q", "-m", "add binary"])?;
        let r = Repository::discover(&repo.path)?;
        assert_eq!(
            r.file_at_rev(&repo.path.join("bin.dat"), "HEAD")?
                .as_deref(),
            Some(&bytes[..])
        );
        Ok(())
    }

    #[test]
    fn file_at_rev_of_a_bad_revision_errors() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.path, "a.txt", b"x\n")?;
        git(&repo.path, &["add", "."])?;
        git(&repo.path, &["commit", "-q", "-m", "c0"])?;
        let r = Repository::discover(&repo.path)?;
        assert!(
            r.file_at_rev(&repo.path.join("a.txt"), "no-such-rev")
                .is_err()
        );
        Ok(())
    }

}
