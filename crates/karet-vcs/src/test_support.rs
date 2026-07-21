//! Shared real-Git fixtures for the engine's unit tests.

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

use crate::VcsError;

static COUNTER: AtomicU32 = AtomicU32::new(0);

pub(crate) struct TestRepo(pub(crate) PathBuf);

impl Drop for TestRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<String, VcsError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_EDITOR", "true")
        .output()
        .map_err(|error| VcsError::Git(error.to_string()))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(VcsError::Git(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ))
    }
}

pub(crate) fn init(name: &str) -> Result<TestRepo, VcsError> {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("karet-vcs-{name}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&path).map_err(|error| VcsError::Git(error.to_string()))?;
    let repo = TestRepo(path);
    git(&repo.0, &["init", "-q", "-b", "main"])?;
    git(&repo.0, &["config", "user.email", "test@example.com"])?;
    git(&repo.0, &["config", "user.name", "karet test"])?;
    git(&repo.0, &["config", "commit.gpgsign", "false"])?;
    Ok(repo)
}

pub(crate) fn commit(repo: &TestRepo, body: &str, message: &str) -> Result<String, VcsError> {
    write(&repo.0, "file.txt", body.as_bytes())?;
    git(&repo.0, &["add", "file.txt"])?;
    git(&repo.0, &["commit", "-q", "-m", message])?;
    git(&repo.0, &["rev-parse", "HEAD"])
}

pub(crate) fn write(dir: &Path, relative: &str, body: &[u8]) -> Result<(), VcsError> {
    let path = dir.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| VcsError::Git(error.to_string()))?;
    }
    std::fs::write(path, body).map_err(|error| VcsError::Git(error.to_string()))
}

pub(crate) fn bare_remote(name: &str) -> Result<TestRepo, VcsError> {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("karet-vcs-{name}-bare-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&path).map_err(|error| VcsError::Git(error.to_string()))?;
    let repo = TestRepo(path);
    git(&repo.0, &["init", "--bare", "-q"])?;
    Ok(repo)
}
