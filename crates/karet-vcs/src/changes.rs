//! Per-file before/after content extraction for a [`Selection`].
//!
//! The key idea: rather than asking `gix` to format a diff, we recover the *old* and
//! *new* content of each changed file and hand both sides to the caller, who can run
//! whatever diff they like (e.g. `karet-diff`).

use crate::{Repository, StatusKind, VcsError, repo::to_git, selection::Selection};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

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
                gix::status::tree_index::TrackRenames::AsConfigured,
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
            if let Some(fc) = self.unstaged_item(item)? {
                out.push(fc);
            }
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
            }
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
            }
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
            }
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
            }
        };
        Ok(Some(fc))
    }

    /// Convert one index vs worktree status item into a [`FileChange`] (or `None`).
    fn unstaged_item(
        &self,
        item: gix::status::index_worktree::Item,
    ) -> Result<Option<FileChange>, VcsError> {
        use gix::dir::entry::{Kind, Status as DirStatus};
        use gix::status::index_worktree::Item as I;
        use gix::status::plumbing::index_as_worktree::{Change as WtChange, EntryStatus};
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
                    }
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
                    }
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
                    }
                    // Submodule changes, conflicts, and bookkeeping updates are skipped.
                    _ => return Ok(None),
                };
                Ok(Some(fc))
            }
            I::DirectoryContents { entry, .. } => {
                if entry.status == DirStatus::Untracked
                    && matches!(entry.disk_kind, Some(Kind::File | Kind::Symlink))
                {
                    let (bin, new) = self.read_worktree_text(entry.rela_path.as_ref())?;
                    let (is_binary, old, new) = finalize(bin, String::new(), new);
                    Ok(Some(FileChange {
                        path: bstr_to_path(entry.rela_path.as_ref()),
                        old_path: None,
                        status: StatusKind::Untracked,
                        is_binary,
                        old,
                        new,
                    }))
                } else {
                    Ok(None)
                }
            }
            // Rewrite items don't fire while index-worktree rename tracking is disabled.
            I::Rewrite { .. } => Ok(None),
        }
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

/// Make `path` repo-relative: strip the worktree prefix if absolute, else use it as-is.
fn repo_relative(repo: &gix::Repository, path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        let workdir = repo.workdir()?;
        path.strip_prefix(workdir).ok().map(Path::to_path_buf)
    } else {
        Some(path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use crate::{Repository, Selection, StatusKind, VcsError};
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

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
}
