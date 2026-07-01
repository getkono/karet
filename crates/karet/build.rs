//! Build script: bakes build provenance into the binary so `karet -V` can report
//! exactly which build is running — the commit (with a `(dirty)` marker), the
//! build profile, the `rustc` that compiled it, and when it was built.
//!
//! Each fact is exported as a `cargo:rustc-env` variable that `src/cli.rs`
//! stitches into a compile-time version string. Every variable is emitted
//! unconditionally — even when git data is unavailable (e.g. building from a
//! source tarball with no `.git`) — so the `env!` lookups in `cli.rs` always
//! resolve and the crate still compiles.

use std::error::Error;
use std::path::Path;
use std::process::Command;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use gix::date::time::format::ISO8601_STRICT;

fn main() {
    // Pieces that are always available, independent of the git state.
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".into());
    println!("cargo:rustc-env=KARET_BUILD_PROFILE={profile}");
    println!("cargo:rustc-env=KARET_RUSTC={}", rustc_version());
    println!(
        "cargo:rustc-env=KARET_BUILD_TIMESTAMP={}",
        build_timestamp()
    );
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");

    // Git provenance. Fall back to placeholders (plus a warning) when there is no
    // repository to read, so a tarball build still compiles cleanly.
    match git_info() {
        Ok(info) => {
            println!("cargo:rustc-env=KARET_GIT_SHA={}", info.sha);
            println!("cargo:rustc-env=KARET_GIT_DIRTY={}", info.dirty_marker);
            println!(
                "cargo:rustc-env=KARET_GIT_COMMIT_TIMESTAMP={}",
                info.commit_timestamp
            );
        },
        Err(e) => {
            println!("cargo:warning=karet: git build-info unavailable: {e}");
            println!("cargo:rustc-env=KARET_GIT_SHA=unknown");
            println!("cargo:rustc-env=KARET_GIT_DIRTY=");
            println!("cargo:rustc-env=KARET_GIT_COMMIT_TIMESTAMP=unknown");
        },
    }
}

/// Resolved git provenance for the current `HEAD`.
struct GitInfo {
    /// Abbreviated (12-hex-digit) commit SHA.
    sha: String,
    /// Either an empty string or `" (dirty)"`, ready to splice directly after the SHA.
    dirty_marker: String,
    /// Committer timestamp in strict ISO-8601, preserving the original UTC offset.
    commit_timestamp: String,
}

/// Read the short SHA, dirty flag, and commit timestamp for `HEAD`, registering the
/// git files whose change should re-run this script.
fn git_info() -> Result<GitInfo, Box<dyn Error>> {
    let repo = gix::discover(".")?;

    // Re-run when HEAD moves (`logs/HEAD` gains a line on every commit/checkout/reset),
    // when the branch itself switches, or when the index changes (affects dirtiness).
    let git_dir = repo.git_dir();
    rerun_if_changed(&git_dir.join("HEAD"));
    rerun_if_changed(&git_dir.join("logs").join("HEAD"));
    rerun_if_changed(&git_dir.join("index"));

    let commit = repo.head_commit()?;
    let sha = commit.id().to_hex_with_len(12).to_string();
    let commit_timestamp = commit.time()?.format(ISO8601_STRICT)?;
    let dirty_marker = if repo.is_dirty()? { " (dirty)" } else { "" }.to_string();

    Ok(GitInfo {
        sha,
        dirty_marker,
        commit_timestamp,
    })
}

/// `rustc --version` output (e.g. `rustc 1.96.0 (ac68faa20 2026-05-25)`), or
/// `"unknown"` if the compiler cannot be queried.
fn rustc_version() -> String {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Build time as a UTC RFC-3339 timestamp ending in `Z`. Honors `SOURCE_DATE_EPOCH`
/// for reproducible builds, otherwise uses the current wall-clock time.
fn build_timestamp() -> String {
    let seconds = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| i64::try_from(d.as_secs()).unwrap_or(0))
                .unwrap_or(0)
        });
    // Offset 0 renders as a trailing `+00:00`; normalize that to the conventional `Z`.
    match gix::date::Time::new(seconds, 0).format(ISO8601_STRICT) {
        Ok(s) => match s.strip_suffix("+00:00") {
            Some(head) => format!("{head}Z"),
            None => s,
        },
        Err(_) => "unknown".to_string(),
    }
}

/// Emit a `cargo:rerun-if-changed` directive for `path`.
fn rerun_if_changed(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
}
