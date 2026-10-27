//! GitHub remote parsing and repository identity.

use karet_vcs::ForgeKind;

/// A public GitHub repository identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct RepositoryIdentity {
    /// Repository owner or organization.
    pub owner: String,
    /// Repository name without a trailing `.git`.
    pub repo: String,
}

impl RepositoryIdentity {
    /// The `owner/repository` display form.
    #[must_use]
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

/// A validated remote hosted on the public `github.com` service.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GitHubRemote {
    identity: RepositoryIdentity,
}

impl GitHubRemote {
    /// Parse a conventional public GitHub clone URL.
    ///
    /// Builds on [`karet_vcs::parse_remote`], so a remote is GitHub here exactly
    /// when that parser detects [`ForgeKind::GitHub`] for it: the SSH user and
    /// port are ignored and the host matches case-insensitively. On top of that,
    /// both path components must be plain GitHub names (ASCII alphanumerics,
    /// `-`, `_`, `.`). GitHub Enterprise hosts, aliases, subdomains, query
    /// strings, fragments, and paths other than exactly `owner/repository` are
    /// rejected.
    #[must_use]
    pub fn parse(input: &str) -> Option<Self> {
        let remote = karet_vcs::parse_remote(input)?;
        if remote.kind != ForgeKind::GitHub {
            return None;
        }
        let (owner, repo) = remote.repo_path.split_once('/')?;
        if !valid_component(owner) || !valid_component(repo) {
            return None;
        }
        Some(Self {
            identity: RepositoryIdentity {
                owner: owner.to_string(),
                repo: repo.to_string(),
            },
        })
    }

    /// Borrow the repository identity.
    #[must_use]
    pub fn identity(&self) -> &RepositoryIdentity {
        &self.identity
    }

    /// Consume the remote and return its repository identity.
    #[must_use]
    pub fn into_identity(self) -> RepositoryIdentity {
        self.identity
    }
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('%')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_public_github_clone_forms() {
        let expected = RepositoryIdentity {
            owner: "getkono".to_string(),
            repo: "karet".to_string(),
        };
        for input in [
            "https://github.com/getkono/karet.git",
            "http://github.com/getkono/karet",
            "git://github.com/getkono/karet.git",
            "ssh://git@github.com/getkono/karet.git",
            "git@github.com:getkono/karet.git",
            "  git@GitHub.com:getkono/karet  ",
            // The SSH user and the host's case do not change the repository, so
            // this parser agrees with `karet_vcs::parse_remote` on both.
            "alice@github.com:getkono/karet.git",
            "ssh://alice@github.com/getkono/karet.git",
            "https://GITHUB.com/getkono/karet",
        ] {
            assert_eq!(
                GitHubRemote::parse(input).map(GitHubRemote::into_identity),
                Some(expected.clone())
            );
        }
    }

    #[test]
    fn rejects_enterprise_aliases_lookalikes_and_nested_paths() {
        for input in [
            "https://github.example.com/getkono/karet.git",
            "https://www.github.com/getkono/karet.git",
            "https://github.com.evil.test/getkono/karet.git",
            "git@work:getkono/karet.git",
            "https://github.com/getkono/karet/extra",
            "https://github.com/getkono/karet?tab=readme",
            "https://github.com/getkono/karet#readme",
            "git@github.com:get kono/karet.git",
            "https://github.com/getkono%2fkaret/repo",
            "https://gitlab.com/getkono/karet.git",
            "git@example.com:a/b.git",
            "https://github.com/onlyowner",
            "not a url",
        ] {
            assert!(GitHubRemote::parse(input).is_none(), "accepted {input}");
        }
    }

    #[test]
    fn full_name_joins_owner_and_repository() {
        let identity = RepositoryIdentity {
            owner: "getkono".to_string(),
            repo: "karet".to_string(),
        };
        assert_eq!(identity.full_name(), "getkono/karet");
    }
}
