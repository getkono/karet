use std::path::Path;
use std::path::PathBuf;

use super::*;
use crate::api::Event;
use crate::session::SessionConfig;

#[tokio::test]
async fn local_backend_reports_both_merge_conflict_sides() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .status()
            .ok()
            .is_some_and(|status| status.success())
    };
    if !git(&["init", "-q"])
        || !git(&["config", "user.email", "test@example.com"])
        || !git(&["config", "user.name", "karet test"])
        || std::fs::write(dir.path().join("a.txt"), "base\n").is_err()
        || !git(&["add", "a.txt"])
        || !git(&["commit", "-q", "-m", "base"])
        || !git(&["checkout", "-q", "-b", "incoming"])
        || std::fs::write(dir.path().join("a.txt"), "incoming\n").is_err()
        || !git(&["commit", "-q", "-am", "incoming"])
        || !git(&["checkout", "-q", "-"])
        || std::fs::write(dir.path().join("a.txt"), "current\n").is_err()
        || !git(&["commit", "-q", "-am", "current"])
    {
        return;
    }
    let _ = git(&["merge", "--no-edit", "incoming"]);
    let (session, mut events, _snaps) = Session::new(SessionConfig {
        roots: vec![dir.path().to_path_buf()],
        ..SessionConfig::default()
    });
    let backend = local(session);
    let id = backend.next_id();
    assert!(
        backend
            .send(
                id,
                Command::MergeConflict {
                    path: PathBuf::from("a.txt"),
                },
            )
            .is_ok()
    );

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
}
