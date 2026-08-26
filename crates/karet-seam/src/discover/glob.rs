//! Expanding Cargo `members`/`exclude` patterns against the filesystem.
//!
//! `dependable_core::parse_workspace` hands back the patterns exactly as written, because
//! expanding one needs the filesystem. This is that step, hand-rolled: Cargo member
//! patterns are `crates/*`, `libs/**`, and literals, and a full glob engine would drag
//! `globset` and its regex machinery into an engine whose entire dependency set is five
//! crates. The tradeoff is stated rather than hidden — character classes (`[a-z]`) and
//! alternation (`{a,b}`) are not supported, and a pattern using them expands to nothing
//! rather than to something wrong.

use std::path::Path;
use std::path::PathBuf;

/// How deep a `**` component is allowed to reach.
///
/// A recursive wildcard with no bound would walk a whole disk when a pattern is broader
/// than its author intended.
const RECURSIVE_DEPTH: usize = 8;

/// Expand one pattern, relative to `root`, to the directories that exist.
///
/// Results are sorted at every level, so the order a workspace's members appear in is the
/// same on every machine — the index's root order is user-visible as the view's first
/// column, and `read_dir` order is not stable.
#[must_use]
pub(crate) fn expand(root: &Path, pattern: &str) -> Vec<PathBuf> {
    let trimmed = pattern.trim_matches('/');
    if trimmed.is_empty() {
        return Vec::new();
    }
    let mut frontier = vec![root.to_path_buf()];
    for component in trimmed.split('/') {
        frontier = advance(&frontier, component);
        if frontier.is_empty() {
            break;
        }
    }
    frontier.retain(|path| path.is_dir());
    frontier.sort();
    frontier.dedup();
    frontier
}

/// Take one pattern component across every path in the frontier.
fn advance(frontier: &[PathBuf], component: &str) -> Vec<PathBuf> {
    if component == "**" {
        return frontier.iter().flat_map(|base| descendants(base)).collect();
    }
    if !component.contains(['*', '?']) {
        // A literal component needs no directory listing at all.
        return frontier
            .iter()
            .map(|base| base.join(component))
            .filter(|candidate| candidate.exists())
            .collect();
    }
    frontier
        .iter()
        .flat_map(|base| children(base))
        .filter(|child| {
            child
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| matches(component, name))
        })
        .collect()
}

/// A directory's immediate children, sorted by name.
fn children(base: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    out.sort();
    out
}

/// `base` and every directory beneath it, to [`RECURSIVE_DEPTH`].
fn descendants(base: &Path) -> Vec<PathBuf> {
    let mut out = vec![base.to_path_buf()];
    let mut level = vec![base.to_path_buf()];
    for _ in 0..RECURSIVE_DEPTH {
        let next: Vec<PathBuf> = level
            .iter()
            .flat_map(|dir| children(dir))
            .filter(|path| path.is_dir())
            .collect();
        if next.is_empty() {
            break;
        }
        out.extend(next.iter().cloned());
        level = next;
    }
    out
}

/// Whether one glob component matches one path component.
///
/// `*` consumes any run of characters *within* the component and `?` exactly one, so
/// neither ever crosses a `/` — that separation is what lets `expand` walk one level at a
/// time instead of matching whole paths.
#[must_use]
pub(crate) fn matches(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut p, mut n) = (0usize, 0usize);
    // Where to resume if the current `*` turns out to have consumed too little.
    let mut star: Option<(usize, usize)> = None;

    while n < name.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, n));
                p += 1;
            },
            Some('?') => {
                p += 1;
                n += 1;
            },
            Some(c) if *c == name[n] => {
                p += 1;
                n += 1;
            },
            _ => match star {
                Some((sp, sn)) => {
                    p = sp + 1;
                    n = sn + 1;
                    star = Some((sp, sn + 1));
                },
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|c| *c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Build a tree of directories under a fresh temp root.
    fn tree(dirs: &[&str]) -> Result<tempfile::TempDir, std::io::Error> {
        let root = tempfile::tempdir()?;
        for dir in dirs {
            std::fs::create_dir_all(root.path().join(dir))?;
        }
        Ok(root)
    }

    fn names(root: &Path, found: &[PathBuf]) -> Vec<String> {
        found
            .iter()
            .map(|path| {
                path.strip_prefix(root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    #[test]
    fn a_literal_pattern_matches_only_itself() {
        assert!(matches("xtask", "xtask"));
        assert!(!matches("xtask", "xtasks"));
        assert!(!matches("xtask", "xtas"));
    }

    #[test]
    fn a_star_consumes_any_run_including_none() {
        assert!(matches("*", "anything"));
        assert!(matches("*", ""));
        assert!(matches("karet-*", "karet-core"));
        assert!(matches("karet-*", "karet-"));
        assert!(!matches("karet-*", "blameline"));
    }

    #[test]
    fn a_star_matches_in_the_middle_and_at_the_front() {
        assert!(matches("*-core", "karet-core"));
        assert!(matches("karet*core", "karet-core"));
        assert!(matches("k*t*e", "karet-core"));
        assert!(!matches("*-core", "karet-diff"));
    }

    #[test]
    fn a_star_backtracks_rather_than_giving_up_on_its_first_guess() {
        // The greedy first attempt consumes "aab" and fails; the match only exists if the
        // star gives characters back.
        assert!(matches("a*b", "aaab"));
        assert!(matches("*ab", "aaab"));
        assert!(!matches("a*c", "aaab"));
    }

    #[test]
    fn a_question_mark_matches_exactly_one_character() {
        assert!(matches("?ore", "core"));
        assert!(!matches("?ore", "ore"));
        assert!(!matches("?ore", "score"));
    }

    #[test]
    fn several_trailing_stars_still_match_the_end() {
        assert!(matches("karet**", "karet-core"));
        assert!(matches("**", "anything"));
    }

    #[test]
    fn an_unsupported_class_matches_nothing_rather_than_something_wrong() {
        // Stated limitation: no character classes. Better to find nothing than to treat
        // the brackets as literals and quietly expand to the wrong member set.
        assert!(!matches("karet-[abc]", "karet-a"));
    }

    #[test]
    fn a_glob_component_expands_in_sorted_order() -> TestResult {
        let root = tree(&["crates/zeta", "crates/alpha", "crates/mid"])?;
        let found = expand(root.path(), "crates/*");
        assert_eq!(
            names(root.path(), &found),
            ["crates/alpha", "crates/mid", "crates/zeta"]
        );
        Ok(())
    }

    #[test]
    fn a_literal_pattern_needs_no_listing() -> TestResult {
        let root = tree(&["xtask"])?;
        assert_eq!(names(root.path(), &expand(root.path(), "xtask")), ["xtask"]);
        Ok(())
    }

    #[test]
    fn a_recursive_pattern_reaches_every_depth() -> TestResult {
        let root = tree(&["libs/a/b/c"])?;
        let found = names(root.path(), &expand(root.path(), "libs/**"));
        assert!(found.contains(&"libs".to_owned()), "got {found:?}");
        assert!(found.contains(&"libs/a/b/c".to_owned()), "got {found:?}");
        Ok(())
    }

    #[test]
    fn a_pattern_matching_a_file_expands_to_nothing() -> TestResult {
        let root = tree(&["crates"])?;
        std::fs::write(root.path().join("crates").join("notes.md"), "x")?;
        // Only directories can be package roots.
        assert!(expand(root.path(), "crates/notes.md").is_empty());
        Ok(())
    }

    #[test]
    fn a_pattern_matching_nothing_expands_to_nothing() -> TestResult {
        let root = tree(&["crates/alpha"])?;
        assert!(expand(root.path(), "vendor/*").is_empty());
        assert!(expand(root.path(), "").is_empty());
        assert!(expand(root.path(), "/").is_empty());
        Ok(())
    }

    #[test]
    fn trailing_and_leading_slashes_are_ignored() -> TestResult {
        let root = tree(&["crates/alpha"])?;
        assert_eq!(
            names(root.path(), &expand(root.path(), "crates/alpha/")),
            ["crates/alpha"]
        );
        Ok(())
    }
}
