//! Off-UI preparation of repository changes for diff rendering.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use karet_session::RequestId;
use karet_theme::Theme;
use karet_vcs::FileChange;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;

use crate::render::FileView;
use crate::render::Section;

pub(super) struct PrepareJob {
    pub(super) request: RequestId,
    pub(super) changes: Vec<FileChange>,
    pub(super) syntax: bool,
    pub(super) theme: Theme,
    pub(super) cancelled: Arc<AtomicBool>,
}

pub(super) struct PrepareResult {
    pub(super) request: RequestId,
    pub(super) files: Vec<FileView>,
}

pub(super) fn spawn() -> (
    std::sync::mpsc::Sender<PrepareJob>,
    UnboundedReceiver<PrepareResult>,
) {
    let (jobs_tx, jobs_rx) = std::sync::mpsc::channel::<PrepareJob>();
    let (results_tx, results_rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("karet-diff-prepare".to_owned())
        .spawn(move || run(&jobs_rx, &results_tx))
        .ok();
    (jobs_tx, results_rx)
}

fn run(jobs: &std::sync::mpsc::Receiver<PrepareJob>, results: &UnboundedSender<PrepareResult>) {
    while let Ok(job) = jobs.recv() {
        let mut files = Vec::with_capacity(job.changes.len());
        for change in job.changes {
            if job.cancelled.load(Ordering::Acquire) {
                break;
            }
            let file = FileView::new(change, Section::Staged, job.syntax);
            file.prime_unified(&job.theme);
            files.push(file);
        }
        if !job.cancelled.load(Ordering::Acquire)
            && results
                .send(PrepareResult {
                    request: job.request,
                    files,
                })
                .is_err()
        {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use karet_vcs::StatusKind;

    use super::*;

    fn change(path: &str) -> FileChange {
        FileChange {
            path: path.into(),
            old_path: None,
            status: StatusKind::Modified,
            is_binary: false,
            old: "old\n".to_owned(),
            new: "new\n".to_owned(),
        }
    }

    #[test]
    fn cancelled_job_does_not_publish_a_partial_result() {
        let (tx, mut rx) = spawn();
        let cancelled = Arc::new(AtomicBool::new(true));
        assert!(
            tx.send(PrepareJob {
                request: RequestId(7),
                changes: vec![change("a.rs")],
                syntax: false,
                theme: Theme::dark(),
                cancelled,
            })
            .is_ok()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(rx.try_recv().is_err());
    }
}
