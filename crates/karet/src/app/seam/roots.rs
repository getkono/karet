//! The start points the Seam view can be opened on.
//!
//! Pure, so the ordering rules are testable without a terminal or a filesystem. The two
//! that matter:
//!
//! **The workspace root leads.** The picker and `Ctrl+K S` must not disagree about what
//! the obvious choice is — an Enter with nothing typed has to index the same thing the
//! chord does, or a mistyped query silently becomes a different view.
//!
//! **A start point is offered once.** The reader's context and the discovered packages
//! overlap constantly: the crate you have a file open in is also a workspace member. It
//! appears where the context put it, keeping the package name discovery knows.
//!
//! Discovery itself runs on a worker thread through [`RootDiscovery`]. It is bounded but
//! still a filesystem walk: on a root like `/usr` it takes most of a second with a warm
//! cache, which a picker opening on the UI thread must not wait for. The picker opens at
//! once over the context rows (and the packages the last discovery found), and the
//! packages fill in when the walk lands.

use std::path::Path;
use std::path::PathBuf;

use karet_seam::Discovered;
use tokio::sync::mpsc;

/// One finished discovery: the packages found under `root`.
#[derive(Debug)]
pub(crate) struct Discovery {
    /// The root the walk covered, so a result for another root is dropped.
    root: PathBuf,
    /// What discovery found there, in the order it found them; `None` when the walk
    /// panicked, so the worker still reports that it ended.
    packages: Option<Vec<Discovered>>,
}

/// The reader's context when the picker opened, kept until the discovery answering it
/// lands so the rows can be rebuilt around the same context.
#[derive(Debug, Clone)]
struct Context {
    current_file_dir: Option<PathBuf>,
    explorer_dir: Option<PathBuf>,
}

/// Package discovery for the Seam picker, run off the UI thread.
pub(crate) struct RootDiscovery {
    /// The packages the last discovery found and the root it covered: offered at once by
    /// the next picker, so a reopened picker does not lose its rows while it refreshes.
    known: Option<(PathBuf, Vec<Discovered>)>,
    /// The context of the picker the walk in flight answers.
    awaiting: Option<Context>,
    /// Whether a walk is running, so a picker reopened meanwhile does not start another.
    running: bool,
    tx: mpsc::UnboundedSender<Discovery>,
    rx: Option<mpsc::UnboundedReceiver<Discovery>>,
}

impl Default for RootDiscovery {
    fn default() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            known: None,
            awaiting: None,
            running: false,
            tx,
            rx: Some(rx),
        }
    }
}

impl RootDiscovery {
    /// The channel finished discoveries arrive on; the event loop takes it once.
    pub(crate) fn take_receiver(&mut self) -> Option<mpsc::UnboundedReceiver<Discovery>> {
        self.rx.take()
    }

    /// The packages last found under `root`, or none when nothing has been found there.
    #[must_use]
    pub(crate) fn known(&self, root: &Path) -> Vec<Discovered> {
        self.known
            .as_ref()
            .filter(|(known, _)| known == root)
            .map(|(_, packages)| packages.clone())
            .unwrap_or_default()
    }

    /// Discover the packages under `root` for a picker opened in this context.
    ///
    /// A walk already running answers this picker instead of a second one starting: it
    /// began a moment ago, which is as fresh as a new one would be.
    pub(crate) fn start(
        &mut self,
        root: &Path,
        current_file_dir: Option<PathBuf>,
        explorer_dir: Option<PathBuf>,
    ) {
        self.awaiting = Some(Context {
            current_file_dir,
            explorer_dir,
        });
        if self.running {
            return;
        }
        let root = root.to_path_buf();
        let tx = self.tx.clone();
        let started = std::thread::Builder::new()
            .name("karet-seam-discovery".to_owned())
            .spawn(move || {
                // A panicking walk must still answer, or `running` stays set and no later
                // picker ever starts another discovery.
                let packages = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    karet_seam::discover(&root, karet_seam::DiscoveryOptions::default())
                }))
                .ok();
                // The receiver is gone only when the app is shutting down.
                let _ = tx.send(Discovery { root, packages });
            });
        match started {
            Ok(_) => self.running = true,
            // The picker still offers the reader's context; only the packages are lost.
            Err(error) => tracing::warn!("seam: cannot start package discovery: {error}"),
        }
    }

    /// Adopt a finished discovery under `root`, returning the rows the picker that asked
    /// for it should now show. `None` when it covered another root, no picker asked, or
    /// the walk failed: the open picker then keeps the rows it opened with.
    pub(crate) fn accept(
        &mut self,
        root: &Path,
        discovery: Discovery,
    ) -> Option<Vec<(String, PathBuf)>> {
        self.running = false;
        if discovery.root != root {
            return None;
        }
        let Some(packages) = discovery.packages else {
            tracing::warn!("seam: package discovery failed under {}", root.display());
            self.awaiting = None;
            return None;
        };
        self.known = Some((discovery.root, packages.clone()));
        let context = self.awaiting.take()?;
        Some(candidates(
            root,
            context.current_file_dir,
            context.explorer_dir,
            packages,
        ))
    }
}

/// Where a candidate came from, shown as the row's tail so the order reads rather than
/// having to be memorized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    WorkspaceRoot,
    Explorer,
    CurrentFile,
    Package,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Self::WorkspaceRoot => "workspace root",
            Self::Explorer => "explorer",
            Self::CurrentFile => "current file",
            Self::Package => "package",
        }
    }
}

/// Build the picker rows: `(label, root)` pairs, in offer order.
///
/// Both context paths are already directories — the caller knows whether it holds a file
/// or a directory, which is better information than this could recover by asking the
/// filesystem, and keeps the ordering rules testable without one.
#[must_use]
pub(crate) fn candidates(
    root: &Path,
    current_file_dir: Option<PathBuf>,
    explorer_dir: Option<PathBuf>,
    discovered: Vec<Discovered>,
) -> Vec<(String, PathBuf)> {
    // Indexed first, so a context row still gets the package name discovery found for it.
    let named: Vec<(PathBuf, String)> = discovered
        .iter()
        .map(|package| (package.root.clone(), package.name.clone()))
        .collect();

    let mut offered: Vec<PathBuf> = Vec::new();
    let mut rows: Vec<(String, PathBuf)> = Vec::new();
    let mut offer = |path: PathBuf, source: Source| {
        if offered.contains(&path) {
            return;
        }
        let name = named
            .iter()
            .find(|(candidate, _)| *candidate == path)
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| directory_name(&path));
        rows.push((label(&name, &path, root, source), path.clone()));
        offered.push(path);
    };

    offer(root.to_path_buf(), Source::WorkspaceRoot);
    if let Some(path) = explorer_dir {
        offer(resolve(root, &path), Source::Explorer);
    }
    if let Some(path) = current_file_dir {
        offer(resolve(root, &path), Source::CurrentFile);
    }
    for (path, _) in named.clone() {
        offer(path, Source::Package);
    }
    rows
}

/// One row: the name, where it lives, and why it is on offer.
///
/// Both the name and the path are in the label so a fuzzy match finds either — a reader
/// types `karet-core` or `crates/karet` depending on which they hold in mind.
fn label(name: &str, path: &Path, root: &Path, source: Source) -> String {
    let relative = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    if relative.is_empty() {
        format!("{name}  · {}", source.label())
    } else {
        format!("{name}  {relative}  · {}", source.label())
    }
}

/// Make a path absolute against the workspace root.
fn resolve(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

/// A directory's own name, for a path discovery had no package name for.
///
/// Resolved when the path does not name itself: a session launched as `karet .` has a
/// root of `.`, and a row reading "/" names nothing the reader would recognize.
fn directory_name(path: &Path) -> String {
    let named = |path: &Path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
    };
    named(path)
        .or_else(|| path.canonicalize().ok().as_deref().and_then(named))
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use karet_seam::PackageKind;

    use super::*;

    fn package(name: &str, root: &str) -> Discovered {
        Discovered {
            name: name.to_owned(),
            root: PathBuf::from(root),
            anchor: PathBuf::from(root).join("Cargo.toml"),
            kind: PackageKind::Cargo,
        }
    }

    fn roots(rows: &[(String, PathBuf)]) -> Vec<&Path> {
        rows.iter().map(|(_, path)| path.as_path()).collect()
    }

    #[test]
    fn the_workspace_root_is_always_offered_first() {
        // An Enter with nothing typed must index what `Ctrl+K S` would, or the two
        // disagree about the obvious choice.
        let rows = candidates(
            Path::new("/repo"),
            Some(PathBuf::from("/repo/crates/a/src")),
            Some(PathBuf::from("/repo/crates/b")),
            vec![package("a", "/repo/crates/a")],
        );
        assert_eq!(roots(&rows).first(), Some(&Path::new("/repo")));
        assert!(rows[0].0.contains("workspace root"));
    }

    #[test]
    fn a_start_point_reached_two_ways_is_offered_once() {
        let rows = candidates(
            Path::new("/repo"),
            None,
            Some(PathBuf::from("/repo/crates/a")),
            vec![
                package("a", "/repo/crates/a"),
                package("b", "/repo/crates/b"),
            ],
        );
        let a: Vec<_> = roots(&rows)
            .into_iter()
            .filter(|path| *path == Path::new("/repo/crates/a"))
            .collect();
        assert_eq!(a.len(), 1, "offered twice: {rows:?}");
    }

    #[test]
    fn a_context_row_still_carries_the_name_discovery_found() {
        // The reader thinks of it as `karet-core`, not as `crates/karet-core`, wherever
        // it happens to be listed.
        let rows = candidates(
            Path::new("/repo"),
            None,
            Some(PathBuf::from("/repo/crates/core")),
            vec![package("karet-core", "/repo/crates/core")],
        );
        let explorer = rows
            .iter()
            .find(|(label, _)| label.contains("explorer"))
            .map(|(label, _)| label.clone())
            .unwrap_or_default();
        assert!(explorer.starts_with("karet-core"), "got {explorer:?}");
    }

    #[test]
    fn the_directory_a_reader_is_in_is_offered() {
        let rows = candidates(
            Path::new("/repo"),
            Some(PathBuf::from("/repo/crates/a/src")),
            None,
            Vec::new(),
        );
        assert!(
            roots(&rows).contains(&Path::new("/repo/crates/a/src")),
            "got {rows:?}"
        );
    }

    #[test]
    fn a_relative_context_path_is_resolved_against_the_root() {
        let rows = candidates(
            Path::new("/repo"),
            None,
            Some(PathBuf::from("crates/a")),
            Vec::new(),
        );
        assert!(
            roots(&rows).contains(&Path::new("/repo/crates/a")),
            "got {rows:?}"
        );
    }

    #[test]
    fn every_row_names_both_the_package_and_where_it_lives() {
        // Fuzzy matching has to find it either way — a reader types `karet-core` or
        // `crates/karet` depending on which they hold in mind.
        let rows = candidates(
            Path::new("/repo"),
            None,
            None,
            vec![package("karet-core", "/repo/crates/karet-core")],
        );
        let row = rows
            .iter()
            .find(|(_, path)| path == Path::new("/repo/crates/karet-core"))
            .map(|(label, _)| label.clone())
            .unwrap_or_default();
        assert!(row.contains("karet-core"), "got {row:?}");
        assert!(row.contains("crates/karet-core"), "got {row:?}");
        assert!(row.contains("package"), "got {row:?}");
    }

    #[test]
    fn a_root_that_does_not_name_itself_is_resolved() {
        // `karet .` has a root of `.`; a row reading "/" names nothing recognizable.
        let rows = candidates(Path::new("."), None, None, Vec::new());
        assert!(!rows[0].0.starts_with("/"), "got {:?}", rows[0].0);
        assert!(!rows[0].0.starts_with("."), "got {:?}", rows[0].0);
    }

    #[test]
    fn the_root_row_omits_an_empty_relative_path() {
        let rows = candidates(Path::new("/repo"), None, None, Vec::new());
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].0.contains("  ·  "), "got {:?}", rows[0].0);
        assert!(rows[0].0.starts_with("repo  ·"), "got {:?}", rows[0].0);
    }

    #[test]
    fn a_directory_with_no_packages_still_offers_where_the_reader_is() {
        // Nothing discovered is a normal answer, not a reason to offer nothing.
        let rows = candidates(
            Path::new("/repo"),
            Some(PathBuf::from("/repo/notes")),
            None,
            Vec::new(),
        );
        assert!(roots(&rows).contains(&Path::new("/repo")));
        assert!(roots(&rows).contains(&Path::new("/repo/notes")));
    }

    fn discovery(root: &str, packages: Vec<Discovered>) -> Discovery {
        Discovery {
            root: PathBuf::from(root),
            packages: Some(packages),
        }
    }

    /// Discovery as a picker opened with `current_file_dir` left it: a walk in flight.
    fn awaiting(current_file_dir: Option<PathBuf>) -> RootDiscovery {
        RootDiscovery {
            awaiting: Some(Context {
                current_file_dir,
                explorer_dir: None,
            }),
            running: true,
            ..RootDiscovery::default()
        }
    }

    #[test]
    fn a_discovery_of_another_root_is_dropped() {
        let mut discoveries = awaiting(None);
        let rows = discoveries.accept(
            Path::new("/repo"),
            discovery("/elsewhere", vec![package("a", "/elsewhere/a")]),
        );
        assert!(rows.is_none());
        assert!(discoveries.known(Path::new("/elsewhere")).is_empty());
    }

    #[test]
    fn a_discovery_nobody_awaits_is_kept_for_the_next_picker() {
        let mut discoveries = RootDiscovery::default();
        let found = vec![package("a", "/repo/crates/a")];
        let rows = discoveries.accept(Path::new("/repo"), discovery("/repo", found.clone()));
        assert!(rows.is_none());
        assert_eq!(discoveries.known(Path::new("/repo")), found);
        assert!(discoveries.known(Path::new("/other")).is_empty());
    }

    #[test]
    fn a_landed_discovery_rebuilds_the_rows_around_the_context_that_asked() {
        let mut discoveries = awaiting(Some(PathBuf::from("/repo/notes")));
        let rows = discoveries
            .accept(
                Path::new("/repo"),
                discovery("/repo", vec![package("a", "/repo/crates/a")]),
            )
            .unwrap_or_default();
        assert_eq!(
            roots(&rows),
            [
                Path::new("/repo"),
                Path::new("/repo/notes"),
                Path::new("/repo/crates/a"),
            ]
        );
        // Answered once: a second landing has no picker to fill.
        assert!(
            discoveries
                .accept(Path::new("/repo"), discovery("/repo", Vec::new()))
                .is_none()
        );
    }

    #[test]
    fn a_failed_discovery_ends_the_walk_and_keeps_what_was_known() {
        // A panicked walk reports `None`: the next picker must be free to start another,
        // and the packages found before must survive it.
        let found = vec![package("a", "/repo/crates/a")];
        let mut discoveries = awaiting(None);
        discoveries.known = Some((PathBuf::from("/repo"), found.clone()));
        let failed = Discovery {
            root: PathBuf::from("/repo"),
            packages: None,
        };
        assert!(discoveries.accept(Path::new("/repo"), failed).is_none());
        assert!(!discoveries.running);
        assert!(discoveries.awaiting.is_none());
        assert_eq!(discoveries.known(Path::new("/repo")), found);
    }

    #[test]
    fn discovered_packages_keep_the_order_they_were_found_in() {
        let rows = candidates(
            Path::new("/repo"),
            None,
            None,
            vec![
                package("alpha", "/repo/crates/alpha"),
                package("beta", "/repo/crates/beta"),
            ],
        );
        assert_eq!(
            roots(&rows),
            [
                Path::new("/repo"),
                Path::new("/repo/crates/alpha"),
                Path::new("/repo/crates/beta"),
            ]
        );
    }
}
