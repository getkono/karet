//! The gitignore-aware workspace walk: the filtered corpus of text files, and
//! the workspace search and replace that run over it.

use std::ops::ControlFlow;
use std::path::Path;
use std::path::PathBuf;

use crate::Match;
use crate::Matcher;
use crate::SearchError;
use crate::SearchQuery;
use crate::apply_replacements;

/// Directory names every workspace walk prunes outright, on top of the
/// gitignore/hidden filters (also pruned by `karet-watch`'s watcher
/// enumeration, which reads this list): VCS metadata and the classic
/// heavyweight build/dependency trees, which drown results even when a
/// workspace has no `.gitignore`.
pub const IGNORED_DIRS: &[&str] = &[".git", "target", "node_modules"];

/// A file together with its matches, streamed from a workspace search.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FileHit {
    /// The file path.
    pub path: PathBuf,
    /// The matches within the file.
    pub matches: Vec<Match>,
}

/// The maximum file size (in bytes) the workspace search will read; larger files
/// are skipped. Tune later alongside the deferred parallel walk.
pub(crate) const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// A gitignore-aware workspace search.
#[derive(Default)]
pub struct WorkspaceSearch {}

impl WorkspaceSearch {
    /// Create a workspace search.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Walk `root` and run `query`, invoking `sink` once per file with matches.
    ///
    /// The walk honors `.gitignore`/hidden-file conventions and the query's
    /// `includes`/`excludes` globs (ripgrep `-g` semantics), and skips binary and
    /// oversize files. Results stream through `sink` as each matching file is
    /// found.
    ///
    /// The current walk is single-threaded; a parallel
    /// [`ignore::WalkBuilder::build_parallel`]-based path is a deferred
    /// optimization.
    ///
    /// # Errors
    /// Returns [`SearchError::InvalidPattern`] if the pattern or an include/exclude
    /// glob is invalid.
    pub fn run(
        &self,
        root: &Path,
        query: &SearchQuery,
        mut sink: impl FnMut(FileHit),
    ) -> Result<(), SearchError> {
        if query.pattern.is_empty() {
            return Ok(());
        }
        let matcher = Matcher::new(query)?;
        walk_text_files(root, &query.includes, &query.excludes, |path, text| {
            let matches = matcher.find(&text);
            if !matches.is_empty() {
                sink(FileHit {
                    path: path.to_path_buf(),
                    matches,
                });
            }
            ControlFlow::Continue(())
        })
    }

    /// Walk `root` and replace every match of `query` with `replacement`, writing
    /// each changed file back to disk. Honors the same gitignore / glob / binary /
    /// size filters as [`run`](Self::run); returns a [`ReplaceSummary`] of what
    /// changed. Regex capture substitutions (`$1`, `${name}`) apply when
    /// [`SearchQuery::regex`] is set.
    ///
    /// A file that cannot be written back does not stop the walk: the other files
    /// are still rewritten, and the one that failed is listed in
    /// [`ReplaceSummary::failures`] (and counted nowhere else), so the caller can
    /// tell a partial replace from a complete one.
    ///
    /// # Errors
    /// Returns [`SearchError::InvalidPattern`] if the pattern or a glob is invalid.
    pub fn replace(
        &self,
        root: &Path,
        query: &SearchQuery,
        replacement: &str,
    ) -> Result<ReplaceSummary, SearchError> {
        if query.pattern.is_empty() {
            return Ok(ReplaceSummary::default());
        }
        let matcher = Matcher::new(query)?;
        let mut summary = ReplaceSummary::default();
        walk_text_files(root, &query.includes, &query.excludes, |path, text| {
            let plan = matcher.plan(&text, replacement, query.regex);
            if !plan.is_empty() {
                let updated = apply_replacements(&text, &plan);
                match write_replacing(path, updated.as_bytes()) {
                    Ok(()) => {
                        summary.files_changed += 1;
                        summary.replacements += plan.len();
                    },
                    Err(error) => summary.failures.push(WriteFailure {
                        path: path.to_path_buf(),
                        error: error.to_string(),
                    }),
                }
            }
            ControlFlow::Continue(())
        })?;
        Ok(summary)
    }
}

/// Write `bytes` over the existing file at `path` without ever leaving it empty
/// or half-written: the bytes go to a temp file in the same directory, which is
/// flushed to disk and renamed over `path` only once complete. Any error before
/// the rename (a full disk, a read-only directory) leaves `path` untouched and
/// removes the temp file. The original's permissions are carried over. Where the
/// rename itself is refused (a bind-mounted file), the complete bytes are written
/// in place instead, which is not atomic.
///
/// A file the caller may not write is refused rather than replaced, even though
/// the directory would allow the rename: a read-only file stays read-only.
fn write_replacing(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    // Opening for write without truncating probes write permission and leaves
    // the contents alone.
    drop(std::fs::OpenOptions::new().write(true).open(path)?);
    let permissions = std::fs::metadata(path)?.permissions();
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    std::fs::set_permissions(tmp.path(), permissions)?;
    match tmp.persist(path) {
        Ok(_) => Ok(()),
        // The rename alone can fail where a write would not: `path` is a bind
        // mount (EBUSY) or sits on another filesystem (EXDEV). The file was
        // probed writable and the complete bytes are in hand, so write it in
        // place, as karet-text's save does. Dropping the error removes the temp.
        Err(error) => {
            drop(error);
            std::fs::write(path, bytes)
        },
    }
}

/// Walk `root` gitignore-aware and hand every readable text file to `sink`.
///
/// The walk applies exactly the filters [`WorkspaceSearch::run`] does — `.gitignore`
/// and hidden-file conventions, the pruned [`IGNORED_DIRS`], ripgrep `-g` semantics
/// for `includes`/`excludes`, and the binary/oversize/non-UTF-8 skips — so a consumer
/// that needs the *files* rather than pattern matches (a workspace linter, a
/// spell-check pass, an indexer) sees the same corpus a search would.
///
/// `sink` receives each file's path and its full contents, and returns
/// [`ControlFlow::Break`] to stop the walk early (a result cap, a cancelled request).
/// The walk is single-threaded, matching [`WorkspaceSearch::run`].
///
/// # Errors
/// Returns [`SearchError::InvalidPattern`] if an include/exclude glob is invalid.
pub fn walk_text_files(
    root: &Path,
    includes: &[String],
    excludes: &[String],
    mut sink: impl FnMut(&Path, String) -> ControlFlow<()>,
) -> Result<(), SearchError> {
    for entry in build_walk(root, includes, excludes)?.flatten() {
        let Some(text) = read_searchable(&entry) else {
            continue;
        };
        if sink(entry.path(), text).is_break() {
            break;
        }
    }
    Ok(())
}

/// Walk `root` gitignore-aware and yield the path of every regular file in it.
///
/// The walk prunes exactly what [`walk_text_files`] does — `.gitignore` and
/// hidden-file conventions (ignore files above `root` included, and honored even
/// outside a git repository) and the [`IGNORED_DIRS`] — and never follows
/// symlinks, so a link cycle cannot stall it and a link is never listed. Unlike
/// [`walk_text_files`] it reads no contents: binary, oversize and non-UTF-8 files
/// are yielded too, which suits a file picker that lists every file it could open.
///
/// The iterator is lazy and single-threaded; entries the walk cannot read (a
/// directory without permission) are skipped. Paths are yielded in walk order,
/// so a consumer wanting a stable listing sorts them.
pub fn walk_files(root: &Path) -> impl Iterator<Item = PathBuf> {
    // With no include/exclude globs there is nothing to compile, so the build
    // cannot fail; an `Err` would yield an empty walk rather than a panic.
    build_walk(root, &[], &[])
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
        .map(ignore::DirEntry::into_path)
}

/// The result of a workspace [`replace`](WorkspaceSearch::replace).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReplaceSummary {
    /// The number of files written.
    pub files_changed: usize,
    /// The total number of replacements applied (in the files written).
    pub replacements: usize,
    /// The files that had matches but could not be written back, left as they
    /// were; empty when every write succeeded.
    pub failures: Vec<WriteFailure>,
}

/// A file a workspace [`replace`](WorkspaceSearch::replace) could not write back.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WriteFailure {
    /// The file that was not written.
    pub path: PathBuf,
    /// Why the write failed, as the operating system reported it.
    pub error: String,
}

/// Build the gitignore-aware workspace walk (shared by every walking entry point).
fn build_walk(
    root: &Path,
    includes: &[String],
    excludes: &[String],
) -> Result<ignore::Walk, SearchError> {
    let mut builder = ignore::WalkBuilder::new(root);
    builder.standard_filters(true);
    // Honor `.gitignore` even outside a git repository (matches editor expectations
    // and keeps non-repo workspaces filtered).
    builder.require_git(false);
    // Never follow symlinks (a cycle must not stall the walk) and always prune
    // the heavyweight dirs, even when no ignore file mentions them.
    builder.follow_links(false);
    builder.filter_entry(|entry| {
        entry
            .file_name()
            .to_str()
            .is_none_or(|name| !IGNORED_DIRS.contains(&name))
    });
    if !includes.is_empty() || !excludes.is_empty() {
        let mut overrides = ignore::overrides::OverrideBuilder::new(root);
        for inc in includes {
            overrides
                .add(inc)
                .map_err(|_| SearchError::InvalidPattern)?;
        }
        for exc in excludes {
            // `!glob` excludes in override syntax.
            overrides
                .add(&format!("!{exc}"))
                .map_err(|_| SearchError::InvalidPattern)?;
        }
        let overrides = overrides.build().map_err(|_| SearchError::InvalidPattern)?;
        builder.overrides(overrides);
    }
    Ok(builder.build())
}

/// Read a walked entry as UTF-8 text, or `None` if it is not a searchable file
/// (a directory, oversize, binary, or non-UTF-8).
fn read_searchable(entry: &ignore::DirEntry) -> Option<String> {
    if !entry.file_type().is_some_and(|t| t.is_file()) {
        return None;
    }
    if entry.metadata().is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
        return None;
    }
    let bytes = std::fs::read(entry.path()).ok()?;
    // Skip binary files: the shared NUL sniff, or invalid UTF-8.
    if karet_filetype::looks_binary(&bytes) {
        return None;
    }
    String::from_utf8(bytes).ok()
}
