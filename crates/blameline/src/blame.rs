//! The gix-backed blame engine: run blame at HEAD, resolve commit metadata, and
//! collapse consecutive same-commit lines into [`BlameGroup`]s.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gix::ObjectId;

use crate::{BlameError, BlameGroup, LineRange};

/// Whole-file semantic blame at `HEAD`, grouped by commit.
///
/// `repo_root` is any path inside the repository (it is discovered upwards); `file`
/// is the file to blame, given absolute or relative to `repo_root`.
///
/// # Errors
/// Returns [`BlameError::NotARepository`] if no repository/worktree is found,
/// [`BlameError::NotCommitted`] if the file has no committed history in `HEAD` yet,
/// or [`BlameError::Git`] for any blame or object-lookup failure.
pub fn blame_file(repo_root: &Path, file: &Path) -> Result<Vec<BlameGroup>, BlameError> {
    let repo = gix::discover(repo_root).map_err(map_discover)?;
    let workdir = repo.workdir().ok_or(BlameError::NotARepository)?;

    // Resolve the file to a path relative to the worktree root (what gix blame wants).
    let absolute: PathBuf = if file.is_absolute() {
        file.to_path_buf()
    } else {
        repo_root.join(file)
    };
    // Canonicalize both sides before stripping: the file exists, so this resolves
    // symlinks (e.g. Fedora `/home`→`/var/home`) and any `.`-prefix, so the prefix
    // matches reliably instead of silently passing a bogus absolute path to gix.
    let abs_canon = std::fs::canonicalize(&absolute).unwrap_or(absolute);
    let workdir_canon = std::fs::canonicalize(workdir).unwrap_or_else(|_| workdir.to_path_buf());
    let relative = match abs_canon.strip_prefix(&workdir_canon) {
        Ok(relative) => relative,
        Err(_) => {
            return Err(BlameError::Git(format!(
                "{} is outside the repository worktree",
                abs_canon.display()
            )));
        }
    };

    // gix blame starts at HEAD and can only attribute lines that exist in HEAD's
    // committed tree. A file that isn't committed there (new or staged-but-uncommitted)
    // would surface gix's opaque `FileMissing` error — pre-check and report it clearly.
    if repo
        .head_tree()
        .map_err(to_git)?
        .lookup_entry_by_path(relative)
        .map_err(to_git)?
        .is_none()
    {
        return Err(BlameError::NotCommitted(relative.display().to_string()));
    }

    let relative = gix::path::into_bstr(relative);
    let head = repo.head_id().map_err(to_git)?;
    let options = gix::repository::blame_file::Options::default();
    let outcome = repo
        .blame_file(relative.as_ref(), head.detach(), options)
        .map_err(to_git)?;

    // Each gix `BlameEntry` is already a contiguous hunk attributed to one commit;
    // coalesce adjacent hunks that share a commit into a single group.
    let hunks: Vec<(u32, u32, ObjectId)> = outcome
        .entries
        .iter()
        .map(|e| (e.start_in_blamed_file, e.len.get(), e.commit_id))
        .collect();

    let mut meta: HashMap<ObjectId, CommitMeta> = HashMap::new();
    let mut groups = Vec::new();
    for (start0, end0, id) in group_consecutive(&hunks) {
        let info = match meta.get(&id) {
            Some(info) => info.clone(),
            None => {
                let info = commit_meta(&repo, id)?;
                meta.insert(id, info.clone());
                info
            }
        };
        groups.push(BlameGroup {
            lines: LineRange {
                start: start0 + 1,
                end: end0 + 1,
            },
            commit_hash: id.to_hex().to_string(),
            message: info.message,
            author: info.author,
            date: info.date,
        });
    }
    Ok(groups)
}

/// Resolved metadata for one commit (cached so each commit is decoded once).
#[derive(Clone)]
struct CommitMeta {
    message: String,
    author: String,
    date: String,
}

/// Decode a commit's full message, author name, and ISO-8601 author date.
fn commit_meta(repo: &gix::Repository, id: ObjectId) -> Result<CommitMeta, BlameError> {
    let commit = repo.find_commit(id).map_err(to_git)?;
    let message = commit.message_raw().map_err(to_git)?.to_string();
    let author = commit.author().map_err(to_git)?;
    let date = author.time().map_or_else(
        |_| author.time.to_string(),
        |t| t.format_or_unix(gix::date::time::format::ISO8601_STRICT),
    );
    Ok(CommitMeta {
        message: message.trim_end().to_string(),
        author: author.name.to_string(),
        date,
    })
}

/// Coalesce per-hunk blame entries into consecutive same-commit groups.
///
/// Input entries are `(start_line_0based, len_lines, key)`; output is
/// `(start_0based, end_0based_inclusive, key)`, sorted by start, merging entries that
/// share a key and are contiguous (or overlapping). Zero-length entries are skipped.
fn group_consecutive<K: Clone + PartialEq>(hunks: &[(u32, u32, K)]) -> Vec<(u32, u32, K)> {
    let mut sorted: Vec<&(u32, u32, K)> = hunks.iter().collect();
    sorted.sort_by_key(|h| h.0);

    let mut out: Vec<(u32, u32, K)> = Vec::new();
    for (start, len, key) in sorted.into_iter().filter(|h| h.1 > 0) {
        let end = start.saturating_add(len - 1);
        if let Some(last) = out.last_mut()
            && last.2 == *key
            && *start <= last.1.saturating_add(1)
        {
            last.1 = last.1.max(end);
            continue;
        }
        out.push((*start, end, key.clone()));
    }
    out
}

/// Map a gix discovery error to [`BlameError`], distinguishing "no repository" from
/// other failures.
fn map_discover(e: gix::discover::Error) -> BlameError {
    use gix::discover::upwards::Error as U;
    match e {
        gix::discover::Error::Discover(
            U::NoGitRepository { .. }
            | U::NoGitRepositoryWithinCeiling { .. }
            | U::NoGitRepositoryWithinFs { .. }
            | U::NoMatchingCeilingDir
            | U::NoTrustedGitRepository { .. },
        ) => BlameError::NotARepository,
        other => BlameError::Git(other.to_string()),
    }
}

/// Map any displayable error into [`BlameError::Git`].
fn to_git<E: std::fmt::Display>(e: E) -> BlameError {
    BlameError::Git(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_merge_contiguous_same_commit() {
        // Two adjacent hunks from commit `1`, then a hunk from commit `2`.
        let hunks = [(0u32, 3u32, 1u32), (3, 2, 1), (5, 4, 2)];
        let groups = group_consecutive(&hunks);
        assert_eq!(groups, vec![(0, 4, 1), (5, 8, 2)]);
    }

    #[test]
    fn groups_do_not_merge_across_a_gap() {
        // A gap (lines 3-4 absent) keeps two same-commit hunks separate.
        let hunks = [(0u32, 3u32, 7u32), (5, 2, 7)];
        let groups = group_consecutive(&hunks);
        assert_eq!(groups, vec![(0, 2, 7), (5, 6, 7)]);
    }

    #[test]
    fn groups_sort_unordered_input() {
        let hunks = [(5u32, 1u32, 9u32), (0, 2, 8), (2, 3, 8)];
        let groups = group_consecutive(&hunks);
        assert_eq!(groups, vec![(0, 4, 8), (5, 5, 9)]);
    }

    #[test]
    fn empty_hunks_yield_no_groups() {
        let hunks: [(u32, u32, u32); 0] = [];
        assert!(group_consecutive(&hunks).is_empty());
    }
}
