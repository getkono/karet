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
        use gix::bstr::ByteSlice;
        let id = self
            .inner
            .rev_parse_single(rev.as_bytes().as_bstr())
            .map_err(to_git)?;
        let commit = self.inner.find_commit(id).map_err(to_git)?;
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
        // Force rename detection on regardless of the user's `diff.renames` config, so a
        // rename always shows as `R` (matching how the staged view detects renames).
        let opts = gix::diff::Options::default().with_rewrites(Some(Default::default()));
        let raw = self
            .inner
            .diff_tree_to_tree(parent_tree.as_ref(), Some(&new_tree), Some(opts))
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
            .find(|c| c.path == PathBuf::from("a.txt"))
            .ok_or_else(|| VcsError::Git("a.txt missing".into()))?;
        assert_eq!(a.status, StatusKind::Modified);
        assert_eq!(a.old, "one\n");
        assert_eq!(a.new, "one\ntwo\n");
        let b = changes
            .iter()
            .find(|c| c.path == PathBuf::from("b.txt"))
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
            .find(|c| c.path == PathBuf::from("new.txt"))
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
}
