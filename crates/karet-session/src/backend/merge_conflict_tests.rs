use std::path::Path;
use std::path::PathBuf;

use super::*;
use crate::api::Event;
use crate::api::VcsCommand;
use crate::session::SessionConfig;

#[tokio::test]
async fn local_backend_reports_both_merge_conflict_sides() -> Result<(), Box<dyn std::error::Error>>
{
    let git_on_path = std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success());
    if !git_on_path {
        // Skip: `git` is not on PATH.
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let git = |args: &[&str]| -> Result<std::process::ExitStatus, std::io::Error> {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .status()
    };
    let git_ok = |args: &[&str]| -> Result<(), Box<dyn std::error::Error>> {
        let status = git(args)?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("git {args:?} failed: {status}").into())
        }
    };
    git_ok(&["init", "-q"])?;
    git_ok(&["config", "user.email", "test@example.com"])?;
    git_ok(&["config", "user.name", "karet test"])?;
    std::fs::write(dir.path().join("a.txt"), "base\n")?;
    git_ok(&["add", "a.txt"])?;
    git_ok(&["commit", "-q", "-m", "base"])?;
    git_ok(&["checkout", "-q", "-b", "incoming"])?;
    std::fs::write(dir.path().join("a.txt"), "incoming\n")?;
    git_ok(&["commit", "-q", "-am", "incoming"])?;
    git_ok(&["checkout", "-q", "-"])?;
    std::fs::write(dir.path().join("a.txt"), "current\n")?;
    git_ok(&["commit", "-q", "-am", "current"])?;
    // The merge is meant to fail: it leaves the conflict under test.
    let merged = git(&["merge", "--no-edit", "incoming"])?;
    assert!(!merged.success(), "the two branches must conflict");
    let (session, mut events, _snaps) = Session::new(SessionConfig {
        roots: vec![dir.path().to_path_buf()],
        ..SessionConfig::default()
    });
    let backend = local_session(session, None);
    let id = backend.next_id();
    backend.send(
        id,
        VcsCommand::MergeConflict {
            path: PathBuf::from("a.txt"),
        }
        .into(),
    )?;

    let ready = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some((event_id, event)) = events.recv().await {
            if event_id != Some(id) {
                continue;
            }
            if let Event::MergeConflictReady {
                path,
                current,
                incoming,
            } = event
            {
                return path.as_path() == Path::new("a.txt")
                    && current == "current\n"
                    && incoming == "incoming\n";
            }
            return false;
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(ready, "the session should preserve both index-stage texts");
    Ok(())
}
