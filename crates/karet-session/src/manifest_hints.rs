//! Cargo.toml dependency hints: latest-version and vulnerability checks via
//! `dependable-fetch`, on a dedicated worker thread (the checks are async and
//! network-backed, so the worker hosts its own small current-thread runtime).
//!
//! Jobs coalesce newest-per-document like the highlight worker: a keystroke
//! burst re-checks once, against the newest text, with the registry answers
//! served from `dependable-fetch`'s in-process cache.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::sync::mpsc::TryRecvError;

use dependable_fetch::Checker;
use dependable_fetch::core::DependencyStatus;
use dependable_fetch::core::ManifestKind;
use tokio::sync::mpsc as tokio_mpsc;

use crate::api::DocumentId;
use crate::api::Event;
use crate::api::ManifestHint;
use crate::api::ManifestHintState;
use crate::api::RequestId;

/// One re-check request (the newest per document wins).
pub(crate) struct HintJob {
    /// The document whose hints these are.
    pub doc: DocumentId,
    /// The buffer version the text was taken at, echoed back so stale answers
    /// are droppable.
    pub version: u64,
    /// The manifest text (the live buffer).
    pub manifest: String,
    /// The sibling `Cargo.lock`, when present on disk.
    pub lockfile: Option<String>,
}

/// Start the worker; answers arrive as [`Event::ManifestHints`].
pub(crate) fn spawn(
    events: tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) -> Sender<HintJob> {
    let (jobs_tx, jobs_rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("karet-manifest-hints".to_owned())
        .spawn(move || run(&jobs_rx, &events));
    jobs_tx
}

fn run(jobs: &Receiver<HintJob>, events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        while jobs.recv().is_ok() {}
        return;
    };
    let Ok(checker) = Checker::new() else {
        while jobs.recv().is_ok() {}
        return;
    };
    while let Ok(first) = jobs.recv() {
        // Coalesce the backlog: only the newest job per document runs.
        let mut pending: HashMap<DocumentId, HintJob> = HashMap::new();
        pending.insert(first.doc, first);
        loop {
            match jobs.try_recv() {
                Ok(job) => {
                    pending.insert(job.doc, job);
                },
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        for job in pending.into_values() {
            let hints = runtime.block_on(check(&checker, &job));
            if events
                .send((
                    None,
                    Event::ManifestHints {
                        doc: job.doc,
                        version: job.version,
                        hints,
                    },
                ))
                .is_err()
            {
                return;
            }
        }
    }
}

/// Run one check, mapping results onto the neutral hint model. A check that
/// fails outright (offline, rate-limited) yields no hints — annotations
/// simply do not appear rather than shouting about the network.
async fn check(checker: &Checker, job: &HintJob) -> Vec<ManifestHint> {
    let Ok(report) = checker
        .check_manifest(
            ManifestKind::CargoToml,
            &job.manifest,
            job.lockfile.as_deref(),
        )
        .await
    else {
        return Vec::new();
    };
    let mut hints = Vec::new();
    for result in &report.results {
        let state = match &result.status {
            DependencyStatus::UpToDate => ManifestHintState::UpToDate,
            DependencyStatus::PatchAvailable => ManifestHintState::Patch,
            DependencyStatus::UpdateAvailable | DependencyStatus::Outdated => {
                ManifestHintState::Outdated
            },
            DependencyStatus::Vulnerable => ManifestHintState::Vulnerable,
            // Path/git/workspace dependencies have no registry story; keeping
            // quiet is the "silence version overflows" behavior.
            DependencyStatus::Local | DependencyStatus::Git => continue,
            DependencyStatus::Error(_) => ManifestHintState::Error,
            // The status set is non-exhaustive upstream; anything newer stays
            // quiet rather than mislabeled.
            _ => continue,
        };
        // The parser reports byte offsets within the line; the editor speaks
        // character columns.
        let line_text = job
            .manifest
            .lines()
            .nth(result.item.version_line)
            .unwrap_or("");
        let to_col = |byte: usize| {
            u32::try_from(line_text.get(..byte).map_or(0, |s| s.chars().count()))
                .unwrap_or(u32::MAX)
        };
        hints.push(ManifestHint {
            name: result.item.name.clone(),
            line: u32::try_from(result.item.version_line).unwrap_or(u32::MAX),
            col_start: to_col(result.item.version_col_start),
            col_end: to_col(result.item.version_col_end),
            current: result.item.version_constraint.clone(),
            latest: result
                .latest_available
                .clone()
                .or_else(|| result.latest_compatible.clone()),
            state,
            vulnerabilities: result.current_vulnerabilities.clone(),
        });
    }
    hints.sort_by_key(|hint| hint.line);
    hints
}
