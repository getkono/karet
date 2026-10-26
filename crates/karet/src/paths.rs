//! Path normalization and display shared across the app.
//!
//! One home-directory abbreviator serves every surface that shows a workspace
//! path — the window title and the Open Recent picker — so the same directory
//! never reads `~/dev` in one place and `/var/mnt/scratch/ada/dev` in another.

use std::path::Path;
use std::path::PathBuf;

/// The window/process title's workspace path, resolved against the real
/// environment.
///
/// Falls back to the root as given when the current directory cannot be read (a
/// deleted cwd), so a cosmetic string never fails startup.
pub(crate) fn window_title_path(root: &Path) -> String {
    let Ok(cwd) = std::env::current_dir() else {
        return root.display().to_string();
    };
    let base_dirs = directories::BaseDirs::new();
    title_path(
        root,
        &cwd,
        base_dirs.as_ref().map(directories::BaseDirs::home_dir),
    )
}

/// Render `root` for the window title: absolute, lexically normalized, with `home`
/// abbreviated to `~`.
///
/// The root reaching this point is whatever the user typed — most often the default
/// `.`, which is useless as a title. `home` is a parameter rather than an ambient
/// lookup so tests never depend on the machine's real home directory.
fn title_path(root: &Path, cwd: &Path, home: Option<&Path>) -> String {
    // `join` returns `root` unchanged when it is already absolute.
    abbreviate_home(&cwd.join(root), home)
}

/// Render the absolute `path` for display: lexically normalized, with `home`
/// abbreviated to `~`.
///
/// The displayed path stays lexical on purpose: `fs::canonicalize` resolves symlinks,
/// which would show a path the user never typed, and fails outright on a path that
/// does not exist yet (`karet new-project/`). Symlinks are consulted only to decide
/// whether `path` lies under `home` (see [`strip_home`]).
pub(crate) fn abbreviate_home(path: &Path, home: Option<&Path>) -> String {
    let absolute = lexically_normalize(path);
    // An unusable `$HOME` abbreviates nothing: a relative one would be resolved
    // against the current directory, and `/` (some containers) would turn every
    // absolute path into a misleading `~/...`.
    let home = home
        .map(lexically_normalize)
        .filter(|home| home.is_absolute() && home.parent().is_some());
    match home.as_deref().and_then(|home| strip_home(&absolute, home)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        None => absolute.display().to_string(),
    }
}

/// The part of `absolute` below `home`, or `None` when it lies outside `home`.
///
/// Matching is component-wise, so a sibling like `/home/adaline` is never mistaken
/// for a child of `/home/ada`. When the lexical comparison fails, both sides are
/// resolved and compared again: `$HOME` and the current directory routinely disagree
/// about symlinks (`getcwd` reports the resolved path, `$HOME` whatever the login
/// record says), so `HOME=/mnt/scratch/ada` must still match a cwd of
/// `/var/mnt/scratch/ada/dev` when `/mnt` links to `/var/mnt`.
///
/// The returned remainder is always a lexical tail of `absolute`: only an ancestor
/// of `absolute` that itself resolves to `home` is replaced by `~`. A path whose
/// resolved form lands under `home` through a symlink below that point (`/srv/link`
/// -> `/home/ada/x`) is not under `home` as typed, so it stays absolute rather than
/// borrowing the resolved remainder (`~/x`) another entry may already show.
fn strip_home(absolute: &Path, home: &Path) -> Option<PathBuf> {
    if let Ok(rest) = absolute.strip_prefix(home) {
        return Some(rest.to_path_buf());
    }
    let real_home = std::fs::canonicalize(home).ok()?;
    // Ancestors that do not exist yet fail to resolve and are skipped, so a workspace
    // root the user is about to create still abbreviates through an existing parent.
    absolute
        .ancestors()
        .find(|ancestor| std::fs::canonicalize(ancestor).is_ok_and(|real| real == real_home))
        .and_then(|ancestor| absolute.strip_prefix(ancestor).ok())
        .map(Path::to_path_buf)
}

/// Resolve `path` through symlinks, even when its leaf does not exist yet.
///
/// `fs::canonicalize` needs every component to exist, so for a missing leaf this
/// resolves the nearest existing ancestor and appends the unresolved suffix. That
/// keeps symlinks from disguising where a path really points (macOS `/var` →
/// `/private/var`, a `/mnt` → `/var/mnt` link) before a new file or workspace root
/// is created. A path with no resolvable ancestor is returned unchanged.
pub(crate) fn canonical_nearest(path: &Path) -> PathBuf {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return resolved;
    }
    for ancestor in path.ancestors().skip(1) {
        let Ok(resolved) = std::fs::canonicalize(ancestor) else {
            continue;
        };
        let Ok(suffix) = path.strip_prefix(ancestor) else {
            continue;
        };
        return resolved.join(suffix);
    }
    path.to_path_buf()
}

/// Resolve `.` and `..` components without touching the filesystem.
pub(crate) fn lexically_normalize(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {},
            Component::ParentDir => {
                // Only a real segment can be popped. With nothing to pop, a relative
                // path keeps the `..` (it still means something) while a filesystem
                // root swallows it — `/..` is `/`.
                let poppable = !matches!(
                    out.components().next_back(),
                    None | Some(Component::RootDir | Component::Prefix(_) | Component::ParentDir)
                );
                if poppable {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_root_titles_the_window_with_the_current_directory() {
        let home = Path::new("/home/ada");
        let cwd = home.join("dev").join("karet");

        assert_eq!(
            title_path(Path::new("."), &cwd, Some(home)),
            format!("~{sep}dev{sep}karet", sep = std::path::MAIN_SEPARATOR)
        );
    }

    #[test]
    fn a_root_outside_the_home_directory_stays_absolute() {
        let absolute = Path::new("/etc").join("nginx");

        assert_eq!(
            title_path(&absolute, Path::new("/tmp"), Some(Path::new("/home/ada"))),
            absolute.display().to_string()
        );
    }

    #[test]
    fn the_home_directory_itself_is_just_a_tilde() {
        let home = Path::new("/home/ada");

        assert_eq!(title_path(Path::new("."), home, Some(home)), "~");
        assert_eq!(title_path(home, Path::new("/tmp"), Some(home)), "~");
    }

    #[test]
    fn dot_and_dot_dot_components_are_resolved_without_touching_the_filesystem() {
        let home = Path::new("/home/ada");
        let cwd = home.join("dev").join("karet").join("crates");

        assert_eq!(
            title_path(Path::new("../.."), &cwd, Some(home)),
            format!("~{sep}dev", sep = std::path::MAIN_SEPARATOR)
        );
        // A `..` at the filesystem root has nothing to pop and is dropped.
        assert_eq!(
            title_path(Path::new("/../.."), &cwd, Some(home)),
            Path::new("/").display().to_string()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_home_directory_reached_through_a_symlink_is_still_abbreviated()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        // `HOME=/mnt/scratch/ada` with `/mnt` -> `/var/mnt` leaves `$HOME` and the
        // current directory (which the kernel reports resolved) spelling the same
        // directory differently.
        let dir = tempfile::tempdir()?;
        let real_home = dir.path().join("var").join("ada");
        std::fs::create_dir_all(real_home.join("dev"))?;
        symlink(dir.path().join("var"), dir.path().join("link"))?;
        let home = dir.path().join("link").join("ada");

        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            title_path(Path::new("."), &real_home.join("dev"), Some(&home)),
            format!("~{sep}dev")
        );
        assert_eq!(title_path(&real_home, Path::new("/tmp"), Some(&home)), "~");
        // The other direction too: a root typed through the symlink under a resolved
        // `$HOME`.
        assert_eq!(
            title_path(&home.join("dev"), Path::new("/tmp"), Some(&real_home)),
            format!("~{sep}dev")
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_that_does_not_exist_yet_is_abbreviated_through_a_symlink()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        // `karet new/project` must title correctly before the directory exists, so the
        // resolved comparison walks up to the deepest ancestor that does.
        let dir = tempfile::tempdir()?;
        let real_home = dir.path().join("var").join("ada");
        std::fs::create_dir_all(&real_home)?;
        symlink(dir.path().join("var"), dir.path().join("link"))?;
        let home = dir.path().join("link").join("ada");

        assert_eq!(
            title_path(Path::new("new/project"), &real_home, Some(&home)),
            format!("~{sep}new{sep}project", sep = std::path::MAIN_SEPARATOR)
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_from_outside_home_into_home_keeps_its_own_path()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        // `/srv/link` -> `/home/ada/x` resolves under home, but labelling it `~/x`
        // would show the resolved path and collide with a real `~/x` entry.
        let dir = tempfile::tempdir()?;
        let home = dir.path().join("home").join("ada");
        std::fs::create_dir_all(home.join("x"))?;
        std::fs::create_dir_all(dir.path().join("srv"))?;
        let link = dir.path().join("srv").join("link");
        symlink(home.join("x"), &link)?;

        assert_eq!(
            abbreviate_home(&link, Some(&home)),
            link.display().to_string()
        );
        assert_eq!(
            abbreviate_home(&link.join("sub"), Some(&home)),
            link.join("sub").display().to_string()
        );
        // A link that points at home itself does stand for home.
        let home_link = dir.path().join("srv").join("home");
        symlink(&home, &home_link)?;
        assert_eq!(
            abbreviate_home(&home_link.join("x"), Some(&home)),
            format!("~{}x", std::path::MAIN_SEPARATOR)
        );
        Ok(())
    }

    #[test]
    fn a_sibling_of_the_home_directory_is_never_abbreviated()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let home = dir.path().join("ada");
        let sibling = dir.path().join("adaline");
        std::fs::create_dir_all(&home)?;
        std::fs::create_dir_all(&sibling)?;

        assert_eq!(
            title_path(Path::new("."), &sibling, Some(&home)),
            sibling.display().to_string()
        );
        Ok(())
    }

    #[test]
    fn an_unusable_home_directory_abbreviates_nothing() {
        let cwd = Path::new("/etc").join("nginx");
        let absolute = cwd.display().to_string();

        // `HOME=/` would make every absolute path a misleading `~/...`.
        assert_eq!(
            title_path(Path::new("."), &cwd, Some(Path::new("/"))),
            absolute
        );
        // A relative `$HOME` would otherwise be resolved against the current directory.
        assert_eq!(
            title_path(Path::new("."), &cwd, Some(Path::new("etc"))),
            absolute
        );
        assert_eq!(
            title_path(Path::new("."), &cwd, Some(Path::new(""))),
            absolute
        );
    }

    #[test]
    fn an_unnormalized_home_directory_still_abbreviates() {
        let cwd = Path::new("/home/ada/dev");

        for home in ["/home/ada/", "/home/ada/.", "/home/ada/dev/.."] {
            assert_eq!(
                title_path(Path::new("."), cwd, Some(Path::new(home))),
                format!("~{sep}dev", sep = std::path::MAIN_SEPARATOR),
                "home: {home}"
            );
        }
    }

    #[test]
    fn a_machine_without_a_home_directory_still_gets_an_absolute_title() {
        let cwd = Path::new("/srv/work");

        assert_eq!(
            title_path(Path::new("."), cwd, None),
            cwd.display().to_string()
        );
    }

    #[test]
    fn an_absolute_path_abbreviates_exactly_like_the_window_title() {
        let home = Path::new("/home/me");
        let sep = std::path::MAIN_SEPARATOR;

        // The Open Recent picker passes stored absolute roots, with no cwd.
        let rows = [
            ("/home/me/src/k", format!("~{sep}src{sep}k")),
            ("/home/me", "~".to_owned()),
            ("/home/meow", "/home/meow".to_owned()),
            ("/srv/k", "/srv/k".to_owned()),
        ];
        for (path, expect) in rows {
            let path = Path::new(path);
            assert_eq!(abbreviate_home(path, Some(home)), expect, "{path:?}");
            assert_eq!(
                abbreviate_home(path, Some(home)),
                title_path(path, Path::new("/tmp"), Some(home)),
                "{path:?}"
            );
        }
        assert_eq!(abbreviate_home(Path::new("/srv/k"), None), "/srv/k");
        // `HOME=/` abbreviates nothing here too, where the old picker helper turned
        // every path into `~/...`.
        assert_eq!(
            abbreviate_home(Path::new("/srv/k"), Some(Path::new("/"))),
            "/srv/k"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_recent_workspace_under_a_symlinked_home_is_abbreviated()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        // Recent roots are stored as typed (lexical); `$HOME` may spell the same
        // directory through a symlink, which a plain prefix strip never matched.
        let dir = tempfile::tempdir()?;
        let real_home = dir.path().join("var").join("ada");
        std::fs::create_dir_all(real_home.join("src"))?;
        symlink(dir.path().join("var"), dir.path().join("link"))?;
        let home = dir.path().join("link").join("ada");

        assert_eq!(
            abbreviate_home(&real_home.join("src"), Some(&home)),
            format!("~{}src", std::path::MAIN_SEPARATOR)
        );
        Ok(())
    }

    #[test]
    fn lexical_normalization_keeps_a_leading_parent_of_a_relative_path() {
        assert_eq!(
            lexically_normalize(Path::new("../a/./b/../c")),
            Path::new("../a/c")
        );
        assert_eq!(lexically_normalize(Path::new("a/../..")), Path::new(".."));
        assert_eq!(
            lexically_normalize(Path::new("/a/../../b")),
            Path::new("/b")
        );
    }
}
