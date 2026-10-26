//! Remote-forge web links: build file (blob) URLs for a git remote that
//! [`karet_vcs::parse_remote`] has already resolved to a host, a repository
//! path, and a forge.
//!
//! Everything here is pure — the app gathers the repository facts (HEAD, branch,
//! tracked state) and this module turns them into URLs or user-facing refusal
//! notes, so the pane context menu's enablement and its dispatch can never drift.

use std::path::Path;

use karet_vcs::ForgeKind;
use karet_vcs::RemoteUrl;

/// Which web link to build for a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkKind {
    /// A host-aware URL for the file at the current `HEAD` commit, on whatever
    /// forge the origin remote points at.
    RemoteFile,
    /// A GitHub permalink: the file at the `HEAD` commit hash, with an optional
    /// line anchor.
    GithubPermalink,
    /// A GitHub link to the file on the current branch.
    GithubHeadLink,
}

/// The repository facts a link is built from, gathered by the app.
#[derive(Clone, Debug)]
pub(crate) struct LinkTarget<'a> {
    /// The parsed origin remote.
    pub(crate) remote: &'a RemoteUrl,
    /// The full `HEAD` commit hash, or `None` on an unborn branch.
    pub(crate) head: Option<&'a str>,
    /// The current branch's short name, or `None` when `HEAD` is detached.
    pub(crate) branch: Option<&'a str>,
    /// The file's path relative to the repository worktree root.
    pub(crate) rel_path: &'a Path,
    /// Whether the file exists in the `HEAD` commit's tree.
    pub(crate) tracked: bool,
}

/// Build the `kind` web link for `target` (with an optional 1-based `line`
/// anchor), or a user-facing note explaining why it cannot be built. The `Err`
/// side doubles as the context menu's disabled-entry note.
pub(crate) fn link(
    target: &LinkTarget<'_>,
    kind: LinkKind,
    line: Option<u32>,
) -> Result<String, String> {
    if matches!(kind, LinkKind::GithubPermalink | LinkKind::GithubHeadLink)
        && target.remote.kind != ForgeKind::GitHub
    {
        return Err(match target.remote.kind {
            ForgeKind::GitLab | ForgeKind::Gitea | ForgeKind::Forgejo => format!(
                "remote is {}; GitHub link integration only supports github.com for now",
                target.remote.kind.name()
            ),
            _ => "remote host is not GitHub".to_string(),
        });
    }
    if kind == LinkKind::RemoteFile && target.remote.kind == ForgeKind::Unknown {
        return Err(format!(
            "unknown remote host {}; supported: GitHub, GitLab, Gitea, Forgejo",
            target.remote.host
        ));
    }
    let Some(head) = target.head else {
        return Err("repository has no commits yet".to_string());
    };
    if !target.tracked {
        return Err("file is not tracked at HEAD".to_string());
    }
    let reference = match kind {
        LinkKind::RemoteFile | LinkKind::GithubPermalink => head,
        LinkKind::GithubHeadLink => target
            .branch
            .ok_or_else(|| "no branch checked out (detached HEAD)".to_string())?,
    };
    Ok(file_url(target.remote, reference, target.rel_path, line))
}

/// The blob URL for `rel_path` at `reference` on `remote`, with an optional
/// 1-based `line` anchor. GitLab inserts its `/-/` route prefix; GitHub, Gitea,
/// and Forgejo share the `/blob/` form.
fn file_url(remote: &RemoteUrl, reference: &str, rel_path: &Path, line: Option<u32>) -> String {
    let blob = match remote.kind {
        ForgeKind::GitLab => "/-/blob/",
        _ => "/blob/",
    };
    let mut url = format!(
        "https://{}/{}{blob}{reference}/{}",
        remote.host,
        remote.repo_path,
        path_for_url(rel_path)
    );
    if let Some(line) = line {
        url.push_str(&format!("#L{line}"));
    }
    url
}

/// Render a repo-relative path with `/` separators and the URL-hostile
/// characters (`%`, space, `#`, `?`) percent-encoded.
fn path_for_url(rel: &Path) -> String {
    let joined = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    joined
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('#', "%23")
        .replace('?', "%3F")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parsing itself is `karet_vcs::parse_remote`'s, tested there; these tests
    /// only need a parsed remote to build links from.
    fn remote(url: &str) -> RemoteUrl {
        karet_vcs::parse_remote(url).unwrap_or(RemoteUrl {
            host: String::new(),
            repo_path: String::new(),
            kind: ForgeKind::Unknown,
        })
    }

    fn target<'a>(remote: &'a RemoteUrl, tracked: bool) -> LinkTarget<'a> {
        LinkTarget {
            remote,
            head: Some("abc123"),
            branch: Some("main"),
            rel_path: Path::new("src/app.rs"),
            tracked,
        }
    }

    #[test]
    fn remote_file_link_uses_the_head_commit_per_forge() {
        let gh = remote("git@github.com:o/r.git");
        assert_eq!(
            link(&target(&gh, true), LinkKind::RemoteFile, None).as_deref(),
            Ok("https://github.com/o/r/blob/abc123/src/app.rs")
        );
        // GitLab routes blobs under /-/.
        let gl = remote("https://gitlab.com/g/sub/r.git");
        assert_eq!(
            link(&target(&gl, true), LinkKind::RemoteFile, None).as_deref(),
            Ok("https://gitlab.com/g/sub/r/-/blob/abc123/src/app.rs")
        );
        // Forgejo shares GitHub's /blob/ form.
        let fj = remote("git@codeberg.org:o/r.git");
        assert_eq!(
            link(&target(&fj, true), LinkKind::RemoteFile, None).as_deref(),
            Ok("https://codeberg.org/o/r/blob/abc123/src/app.rs")
        );
    }

    #[test]
    fn permalink_anchors_the_line_and_head_link_uses_the_branch() {
        let gh = remote("git@github.com:o/r.git");
        assert_eq!(
            link(&target(&gh, true), LinkKind::GithubPermalink, Some(42)).as_deref(),
            Ok("https://github.com/o/r/blob/abc123/src/app.rs#L42")
        );
        assert_eq!(
            link(&target(&gh, true), LinkKind::GithubHeadLink, None).as_deref(),
            Ok("https://github.com/o/r/blob/main/src/app.rs")
        );
    }

    #[test]
    fn github_links_refuse_known_non_github_forges_with_a_note() {
        let gl = remote("https://gitlab.com/o/r.git");
        let Err(note) = link(&target(&gl, true), LinkKind::GithubPermalink, None) else {
            unreachable!("gitlab remote must refuse a github link");
        };
        assert!(
            note.contains("GitLab") && note.contains("github.com"),
            "note names the forge: {note}"
        );
        // A Forgejo remote gets the same treatment on the head link.
        let fj = remote("git@codeberg.org:o/r.git");
        let Err(note) = link(&target(&fj, true), LinkKind::GithubHeadLink, None) else {
            unreachable!("forgejo remote must refuse a github link");
        };
        assert!(note.contains("Forgejo"), "{note}");
    }

    #[test]
    fn links_refuse_untracked_files_unknown_hosts_and_detached_head() {
        let gh = remote("git@github.com:o/r.git");
        // Untracked file.
        let Err(note) = link(&target(&gh, false), LinkKind::GithubPermalink, None) else {
            unreachable!("untracked file must refuse");
        };
        assert!(note.contains("not tracked"), "{note}");
        // Unknown host for the remote-file link.
        let other = remote("git@git.example.com:o/r.git");
        let Err(note) = link(&target(&other, true), LinkKind::RemoteFile, None) else {
            unreachable!("unknown host must refuse");
        };
        assert!(note.contains("git.example.com"), "{note}");
        // Detached HEAD only blocks the branch (head) link.
        let mut detached = target(&gh, true);
        detached.branch = None;
        assert!(link(&detached, LinkKind::GithubPermalink, None).is_ok());
        let Err(note) = link(&detached, LinkKind::GithubHeadLink, None) else {
            unreachable!("detached HEAD must refuse a branch link");
        };
        assert!(note.contains("detached"), "{note}");
        // An unborn repository blocks everything.
        let mut unborn = target(&gh, false);
        unborn.head = None;
        let Err(note) = link(&unborn, LinkKind::RemoteFile, None) else {
            unreachable!("unborn repo must refuse");
        };
        assert!(note.contains("no commits"), "{note}");
    }

    #[test]
    fn url_paths_escape_hostile_characters() {
        let gh = remote("git@github.com:o/r.git");
        let mut t = target(&gh, true);
        t.rel_path = Path::new("docs/my notes#1.md");
        assert_eq!(
            link(&t, LinkKind::RemoteFile, None).as_deref(),
            Ok("https://github.com/o/r/blob/abc123/docs/my%20notes%231.md")
        );
    }
}
