//! Rich per-commit detail for the commit view: the full message, author vs
//! committer with their timestamps, parents, and the (extracted, never verified)
//! commit signature.
//!
//! [`Repository::log`](crate::Repository::log) yields a lightweight [`Commit`](crate::Commit)
//! per history row; this module resolves a *single* revision to everything a
//! GitHub-style commit page shows.

use gix::bstr::ByteSlice;

use crate::Repository;
use crate::VcsError;
use crate::repo::to_git;

/// A person recorded on a commit (its author or its committer).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// The display name.
    pub name: String,
    /// The email address.
    pub email: String,
    /// This identity's timestamp, in seconds since the Unix epoch.
    pub time: i64,
    /// The timestamp's timezone offset from UTC, in seconds (east of UTC positive).
    pub offset: i32,
}

/// The scheme of a commit's cryptographic signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignatureKind {
    /// An SSH signature (`-----BEGIN SSH SIGNATURE-----`).
    Ssh,
    /// An OpenPGP / GPG signature (`-----BEGIN PGP SIGNATURE-----`).
    OpenPgp,
    /// An X.509 / S-MIME signature (`-----BEGIN SIGNED MESSAGE-----`).
    X509,
    /// A signature whose armor header was not recognised.
    Unknown,
}

/// A commit's cryptographic signature — **extracted but never verified**.
///
/// karet deliberately does not validate commit signatures locally. Signing keys
/// rotate, and a repository legitimately collects commits from many contributors and
/// machines whose keys this process has no basis to trust; a local "valid/invalid"
/// verdict would therefore be misleading more often than helpful. This type reports
/// only what the commit object *records* — the signature's kind, the signer's key when
/// it can be recovered offline, and the raw armored text. The authoritative "Verified"
/// status is fetched separately from the forge (see the session's GitHub integration).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitSignature {
    /// The signature scheme, detected from the armor header.
    pub kind: SignatureKind,
    /// The signer's key, when recoverable offline: an SSH public-key fingerprint
    /// (`SHA256:…`) for SSH signatures. `None` when the `signature` feature is off, the
    /// scheme is not SSH, or the blob could not be parsed.
    pub signer_key: Option<String>,
    /// The raw, armored signature text as stored in the commit object.
    pub raw: String,
}

/// Full detail for a single commit, backing the commit view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitDetail {
    /// The full commit hash (hex).
    pub hash: String,
    /// The abbreviated hash (first 7 hex characters).
    pub short_hash: String,
    /// The first line of the commit message (its subject).
    pub summary: String,
    /// The message body: everything after the subject's trailing blank line, trimmed.
    /// Empty when the commit has only a subject.
    pub body: String,
    /// Who wrote the change.
    pub author: Identity,
    /// Who committed it. Usually equal to [`author`](Self::author), but differs after a
    /// rebase, a web-UI merge, `git commit --amend --author=…`, and the like.
    pub committer: Identity,
    /// The full hex hashes of this commit's parents, first-parent first. Empty for a
    /// root commit; two or more for a merge.
    pub parents: Vec<String>,
    /// The commit signature, when the commit is signed.
    pub signature: Option<CommitSignature>,
}

impl Repository {
    /// Resolve `rev` — any git revision spec (a full or abbreviated hash, a ref name,
    /// `HEAD`, `HEAD~3`, `main^`, …) — to its [`CommitDetail`].
    ///
    /// # Errors
    /// Returns [`VcsError::Git`] if `rev` does not resolve to a commit, or on any read
    /// failure.
    pub fn commit_detail(&self, rev: &str) -> Result<CommitDetail, VcsError> {
        let id = self
            .inner
            .rev_parse_single(rev.as_bytes().as_bstr())
            .map_err(to_git)?;
        let commit = self.inner.find_commit(id).map_err(to_git)?;
        let hash = id.detach().to_hex().to_string();
        let short_hash = hash.chars().take(7).collect();

        let summary = commit
            .message()
            .map(|m| m.summary().to_string())
            .unwrap_or_default();
        // Everything after the subject's trailing blank line is the body. Working from
        // the raw message keeps this robust regardless of how the subject wrapped.
        let body = commit
            .message_raw_sloppy()
            .to_str_lossy()
            .split_once("\n\n")
            .map(|(_subject, rest)| rest.trim_end().to_string())
            .unwrap_or_default();

        let author = identity(commit.author().map_err(to_git)?);
        let committer = identity(commit.committer().map_err(to_git)?);
        let parents = commit
            .parent_ids()
            .map(|id| id.detach().to_hex().to_string())
            .collect();
        let signature = extract_signature(&commit)?;

        Ok(CommitDetail {
            hash,
            short_hash,
            summary,
            body,
            author,
            committer,
            parents,
            signature,
        })
    }
}

/// Convert a `gix` signature reference into an owned [`Identity`].
fn identity(sig: gix::actor::SignatureRef<'_>) -> Identity {
    let (time, offset) = sig
        .time()
        .map(|t| (t.seconds, t.offset))
        .unwrap_or((sig.seconds(), 0));
    Identity {
        name: sig.name.to_str_lossy().into_owned(),
        email: sig.email.to_str_lossy().into_owned(),
        time,
        offset,
    }
}

/// Read the `gpgsig` header from a commit object and classify it, without verifying.
fn extract_signature(commit: &gix::Commit<'_>) -> Result<Option<CommitSignature>, VcsError> {
    let decoded = commit.decode().map_err(to_git)?;
    let Some(raw) = decoded.extra_headers().pgp_signature() else {
        return Ok(None);
    };
    let raw = raw.to_str_lossy().into_owned();
    let kind = signature_kind(&raw);
    let signer_key = signer_key(kind, &raw);
    Ok(Some(CommitSignature {
        kind,
        signer_key,
        raw,
    }))
}

/// Classify a signature by its armor header line.
fn signature_kind(raw: &str) -> SignatureKind {
    let head = raw.trim_start();
    if head.starts_with("-----BEGIN SSH SIGNATURE-----") {
        SignatureKind::Ssh
    } else if head.starts_with("-----BEGIN PGP SIGNATURE-----") {
        SignatureKind::OpenPgp
    } else if head.starts_with("-----BEGIN SIGNED MESSAGE-----") {
        SignatureKind::X509
    } else {
        SignatureKind::Unknown
    }
}

/// Recover the signer's key fingerprint from an SSH signature (feature `signature`).
#[cfg(feature = "signature")]
fn signer_key(kind: SignatureKind, raw: &str) -> Option<String> {
    if kind != SignatureKind::Ssh {
        return None;
    }
    let sig = ssh_key::SshSig::from_pem(raw.as_bytes()).ok()?;
    Some(
        sig.public_key()
            .fingerprint(ssh_key::HashAlg::Sha256)
            .to_string(),
    )
}

/// Fallback when the `signature` feature is disabled: report only the kind, not the key.
#[cfg(not(feature = "signature"))]
fn signer_key(_kind: SignatureKind, _raw: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::AtomicU32;
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::Repository;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TempDir(PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn unique_dir(tag: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("karet-vcs-detail-{tag}-{}-{n}", std::process::id()))
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "git {args:?} failed");
    }

    #[test]
    fn signature_kind_detects_armor_headers() {
        assert_eq!(
            signature_kind("-----BEGIN SSH SIGNATURE-----\n..."),
            SignatureKind::Ssh
        );
        assert_eq!(
            signature_kind("-----BEGIN PGP SIGNATURE-----\n..."),
            SignatureKind::OpenPgp
        );
        assert_eq!(
            signature_kind("-----BEGIN SIGNED MESSAGE-----\n..."),
            SignatureKind::X509
        );
        assert_eq!(signature_kind("garbage"), SignatureKind::Unknown);
    }

    #[test]
    fn commit_detail_reports_message_author_and_committer() -> Result<(), VcsError> {
        let dir = unique_dir("meta");
        std::fs::create_dir_all(&dir).map_err(|e| VcsError::Git(e.to_string()))?;
        let _guard = TempDir(dir.clone());
        git(&dir, &["init", "-q"]);
        git(&dir, &["config", "user.name", "Author Person"]);
        git(&dir, &["config", "user.email", "author@example.com"]);
        git(&dir, &["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.join("f.txt"), "hi\n").map_err(|e| VcsError::Git(e.to_string()))?;
        git(&dir, &["add", "."]);
        // Distinct committer via env, and a multi-paragraph message.
        let ok = Command::new("git")
            .args([
                "commit",
                "-q",
                "-m",
                "subject line\n\nbody paragraph one.\n\nbody two.",
            ])
            .current_dir(&dir)
            .env("GIT_COMMITTER_NAME", "Committer Person")
            .env("GIT_COMMITTER_EMAIL", "committer@example.com")
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "signed commit failed");

        let repo = Repository::discover(&dir)?;
        let detail = repo.commit_detail("HEAD")?;
        assert_eq!(detail.summary, "subject line");
        assert_eq!(detail.body, "body paragraph one.\n\nbody two.");
        assert_eq!(detail.author.name, "Author Person");
        assert_eq!(detail.author.email, "author@example.com");
        assert_eq!(detail.committer.name, "Committer Person");
        assert_eq!(detail.short_hash.len(), 7);
        assert!(detail.hash.starts_with(&detail.short_hash));
        assert!(detail.parents.is_empty(), "root commit has no parents");
        assert!(
            detail.signature.is_none(),
            "unsigned commit has no signature"
        );
        Ok(())
    }

    #[test]
    fn commit_detail_resolves_revspecs() -> Result<(), VcsError> {
        let dir = unique_dir("rev");
        std::fs::create_dir_all(&dir).map_err(|e| VcsError::Git(e.to_string()))?;
        let _guard = TempDir(dir.clone());
        git(&dir, &["init", "-q"]);
        git(&dir, &["config", "user.name", "Tester"]);
        git(&dir, &["config", "user.email", "t@example.com"]);
        git(&dir, &["config", "commit.gpgsign", "false"]);
        for i in 0..3 {
            std::fs::write(dir.join("f.txt"), format!("v{i}\n"))
                .map_err(|e| VcsError::Git(e.to_string()))?;
            git(&dir, &["add", "."]);
            git(&dir, &["commit", "-q", "-m", &format!("commit {i}")]);
        }
        let repo = Repository::discover(&dir)?;
        assert_eq!(repo.commit_detail("HEAD")?.summary, "commit 2");
        assert_eq!(repo.commit_detail("HEAD~2")?.summary, "commit 0");
        // The abbreviated hash of HEAD resolves to the same commit.
        let head = repo.commit_detail("HEAD")?;
        assert_eq!(repo.commit_detail(&head.short_hash)?.hash, head.hash);
        Ok(())
    }

    #[cfg(feature = "signature")]
    #[test]
    fn ssh_signed_commit_exposes_a_fingerprint() -> Result<(), VcsError> {
        let dir = unique_dir("ssh");
        std::fs::create_dir_all(&dir).map_err(|e| VcsError::Git(e.to_string()))?;
        let _guard = TempDir(dir.clone());
        let key = dir.join("id");
        // Generate an unencrypted ed25519 key to sign with; if ssh-keygen is missing
        // from the environment, there is nothing to exercise, so skip.
        let keygen = Command::new("ssh-keygen")
            .args(["-t", "ed25519", "-N", "", "-C", "test", "-f"])
            .arg(&key)
            .status();
        if !matches!(keygen, Ok(s) if s.success()) {
            return Ok(());
        }
        git(&dir, &["init", "-q"]);
        git(&dir, &["config", "user.name", "Signer"]);
        git(&dir, &["config", "user.email", "signer@example.com"]);
        git(&dir, &["config", "gpg.format", "ssh"]);
        git(
            &dir,
            &[
                "config",
                "user.signingkey",
                &format!("{}.pub", key.display()),
            ],
        );
        std::fs::write(dir.join("f.txt"), "hi\n").map_err(|e| VcsError::Git(e.to_string()))?;
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-S", "-m", "signed"]);

        let repo = Repository::discover(&dir)?;
        let sig = repo
            .commit_detail("HEAD")?
            .signature
            .ok_or_else(|| VcsError::Git("expected a signature".into()))?;
        assert_eq!(sig.kind, SignatureKind::Ssh);
        let fp = sig
            .signer_key
            .ok_or_else(|| VcsError::Git("expected a signer fingerprint".into()))?;
        assert!(fp.starts_with("SHA256:"), "fingerprint was {fp:?}");
        Ok(())
    }
}
