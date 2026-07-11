//! Remote-forge web links: parse a git remote URL into a browsable host +
//! `owner/repo`, detect which forge runs the host, and build file (blob) URLs.
//!
//! Everything here is pure — the app gathers the repository facts (HEAD, branch,
//! tracked state) and this module turns them into URLs or user-facing refusal
//! notes, so the pane context menu's enablement and its dispatch can never drift.

use std::path::Path;

/// A known git-forge kind, detected from the remote host name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ForgeKind {
    /// github.com.
    GitHub,
    /// A GitLab host (gitlab.com or a self-managed `*gitlab*` host).
    GitLab,
    /// A Gitea host.
    Gitea,
    /// A Forgejo host (including codeberg.org).
    Forgejo,
    /// An unrecognized host.
    Unknown,
}

impl ForgeKind {
    /// The human-readable forge name for notes and status messages.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::GitLab => "GitLab",
            Self::Gitea => "Gitea",
            Self::Forgejo => "Forgejo",
            Self::Unknown => "an unknown host",
        }
    }
}

/// A parsed git remote: the web host, the repository path on it (`owner/repo`,
/// possibly with GitLab subgroups), and the detected forge kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Remote {
    /// The host name (no user, no port).
    pub(crate) host: String,
    /// The repository path on the host: `owner/repo` (no leading/trailing slash,
    /// trailing `.git` stripped). GitLab subgroups keep their full path.
    pub(crate) repo_path: String,
    /// The forge detected from the host name.
    pub(crate) kind: ForgeKind,
}

/// Parse a git remote URL — scp-like `git@host:owner/repo.git`, `ssh://`,
/// `git://`, `http://`, or `https://` — into a [`Remote`]. Returns `None` for
/// anything unrecognizable (a local path, an empty repo path, …).
pub(crate) fn parse_remote(url: &str) -> Option<Remote> {
    let url = url.trim();
    let (host, path) = if let Some(rest) = url
        .strip_prefix("ssh://")
        .or_else(|| url.strip_prefix("git://"))
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("https://"))
    {
        let (authority, path) = rest.split_once('/')?;
        (authority, path)
    } else if !url.contains("://") && url.contains('@') && url.contains(':') {
        // scp-like: git@host:owner/repo.git (the colon separates host from path).
        let (authority, path) = url.split_once(':')?;
        (authority, path)
    } else {
        return None;
    };
    // Drop the user (git@) and any port (:22) from the authority.
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = host.split_once(':').map_or(host, |(h, _)| h);
    let repo_path = path
        .trim_matches('/')
        .trim_end_matches(".git")
        .trim_end_matches('/');
    if host.is_empty() || repo_path.is_empty() || !repo_path.contains('/') {
        return None;
    }
    Some(Remote {
        host: host.to_ascii_lowercase(),
        repo_path: repo_path.to_string(),
        kind: detect_forge(&host.to_ascii_lowercase()),
    })
}

/// Detect the forge kind from a (lowercased) host name.
fn detect_forge(host: &str) -> ForgeKind {
    if host == "github.com" {
        ForgeKind::GitHub
    } else if host.contains("gitlab") {
        ForgeKind::GitLab
    } else if host == "codeberg.org" || host.contains("forgejo") {
        ForgeKind::Forgejo
    } else if host.contains("gitea") {
        ForgeKind::Gitea
    } else {
        ForgeKind::Unknown
    }
}

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
    pub(crate) remote: &'a Remote,
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
fn file_url(remote: &Remote, reference: &str, rel_path: &Path, line: Option<u32>) -> String {
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

    fn remote(url: &str) -> Remote {
        parse_remote(url).unwrap_or(Remote {
            host: String::new(),
            repo_path: String::new(),
            kind: ForgeKind::Unknown,
        })
    }

    #[test]
    fn parses_scp_like_github_remote() {
        let r = remote("git@github.com:getkono/karet.git");
        assert_eq!(r.host, "github.com");
        assert_eq!(r.repo_path, "getkono/karet");
        assert_eq!(r.kind, ForgeKind::GitHub);
    }

    #[test]
    fn parses_https_remote_with_and_without_dot_git() {
        assert_eq!(
            remote("https://github.com/owner/repo.git").repo_path,
            "owner/repo"
        );
        assert_eq!(
            remote("https://github.com/owner/repo").repo_path,
            "owner/repo"
        );
    }

    #[test]
    fn keeps_dots_inside_the_repo_name() {
        // Only a trailing `.git` is stripped — dots inside the name survive.
        let r = remote("git@github.com:owner/repo.name.git");
        assert_eq!(r.repo_path, "owner/repo.name");
    }

    #[test]
    fn parses_ssh_url_with_user_and_port() {
        let r = remote("ssh://git@github.com:22/owner/repo.git");
        assert_eq!(r.host, "github.com");
        assert_eq!(r.repo_path, "owner/repo");
        assert_eq!(r.kind, ForgeKind::GitHub);
    }

    #[test]
    fn keeps_gitlab_subgroups_and_detects_gitlab() {
        let r = remote("https://gitlab.com/group/subgroup/repo.git");
        assert_eq!(r.repo_path, "group/subgroup/repo");
        assert_eq!(r.kind, ForgeKind::GitLab);
        assert_eq!(
            remote("git@gitlab.example.com:o/r.git").kind,
            ForgeKind::GitLab
        );
    }

    #[test]
    fn detects_forgejo_gitea_and_unknown_hosts() {
        assert_eq!(remote("git@codeberg.org:o/r.git").kind, ForgeKind::Forgejo);
        assert_eq!(
            remote("https://gitea.example.com/o/r").kind,
            ForgeKind::Gitea
        );
        assert_eq!(
            remote("git@git.example.com:o/r.git").kind,
            ForgeKind::Unknown
        );
    }

    #[test]
    fn rejects_local_paths_and_pathless_urls() {
        assert!(parse_remote("/srv/git/repo.git").is_none());
        assert!(parse_remote("https://github.com/").is_none());
        assert!(
            parse_remote("git@github.com:repo.git").is_none(),
            "no owner"
        );
    }

    fn target<'a>(remote: &'a Remote, tracked: bool) -> LinkTarget<'a> {
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
