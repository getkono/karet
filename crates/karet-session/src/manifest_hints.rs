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
use dependable_fetch::core::CheckResult;
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
    hints(&job.manifest, &report.results)
}

/// The hint state a dependency status shows as, or `None` for one that stays
/// quiet.
fn hint_state(status: &DependencyStatus) -> Option<ManifestHintState> {
    Some(match status {
        DependencyStatus::UpToDate => ManifestHintState::UpToDate,
        DependencyStatus::PatchAvailable => ManifestHintState::Patch,
        DependencyStatus::UpdateAvailable | DependencyStatus::Outdated => {
            ManifestHintState::Outdated
        },
        DependencyStatus::Vulnerable => ManifestHintState::Vulnerable,
        // Path/git/workspace dependencies have no registry story; keeping
        // quiet is the "silence version overflows" behavior.
        DependencyStatus::Local | DependencyStatus::Git => return None,
        DependencyStatus::Error(_) => ManifestHintState::Error,
        // The status set is non-exhaustive upstream; anything newer stays
        // quiet rather than mislabeled.
        _ => return None,
    })
}

/// Map check results onto the neutral hint model, sorted by line.
fn hints(manifest: &str, results: &[CheckResult]) -> Vec<ManifestHint> {
    let mut hints = Vec::new();
    for result in results {
        let Some(state) = hint_state(&result.status) else {
            continue;
        };
        // The parser reports byte offsets within the line; the editor speaks
        // character columns.
        let line_text = manifest.lines().nth(result.item.version_line).unwrap_or("");
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The dependencies a real Cargo.toml parse yields, so tests start from the
    /// items the checker would report on.
    fn items(manifest: &str) -> Vec<dependable_fetch::core::Item> {
        dependable_fetch::core::parse(ManifestKind::CargoToml, manifest)
            .map(|parsed| parsed.items)
            .unwrap_or_default()
    }

    #[test]
    fn every_status_maps_to_its_hint_state_or_stays_quiet() {
        for (status, expected) in [
            (
                DependencyStatus::UpToDate,
                Some(ManifestHintState::UpToDate),
            ),
            (
                DependencyStatus::PatchAvailable,
                Some(ManifestHintState::Patch),
            ),
            (
                DependencyStatus::UpdateAvailable,
                Some(ManifestHintState::Outdated),
            ),
            (
                DependencyStatus::Outdated,
                Some(ManifestHintState::Outdated),
            ),
            (
                DependencyStatus::Vulnerable,
                Some(ManifestHintState::Vulnerable),
            ),
            (
                DependencyStatus::Error("offline".to_owned()),
                Some(ManifestHintState::Error),
            ),
            (DependencyStatus::Local, None),
            (DependencyStatus::Git, None),
        ] {
            assert_eq!(hint_state(&status), expected, "{status:?}");
        }
    }

    #[test]
    fn hints_carry_the_version_span_sorted_by_line_and_skip_quiet_results() {
        let manifest =
            "[dependencies]\nzeta = \"0.2\"\nlocal = { path = \"x\" }\nalpha = \"1.0\"\n";
        let mut results: Vec<CheckResult> = items(manifest)
            .into_iter()
            .map(|item| {
                let status = if item.name == "local" {
                    DependencyStatus::Local
                } else {
                    DependencyStatus::Outdated
                };
                CheckResult::new(item, status)
            })
            .collect();
        if results.len() != 3 {
            return; // the parser did not see the fixture as expected
        }
        // Report out of source order: the hints still come back by line.
        results.reverse();
        for result in &mut results {
            result.latest_compatible = Some("compatible".to_owned());
            if result.item.name == "alpha" {
                result.latest_available = Some("2.0.0".to_owned());
                result.current_vulnerabilities = vec!["RUSTSEC-1".to_owned()];
            }
        }

        let hints = hints(manifest, &results);
        let summary: Vec<(&str, u32, u32, u32, Option<&str>)> = hints
            .iter()
            .map(|hint| {
                (
                    hint.name.as_str(),
                    hint.line,
                    hint.col_start,
                    hint.col_end,
                    hint.latest.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("zeta", 1, 8, 11, Some("compatible")),
                ("alpha", 3, 9, 12, Some("2.0.0")),
            ],
            "the latest available version wins over the latest compatible one"
        );
        assert_eq!(hints[1].current, "1.0");
        assert_eq!(hints[1].vulnerabilities, vec!["RUSTSEC-1".to_owned()]);
    }

    #[test]
    fn byte_offsets_become_character_columns() {
        let Some(item) = items("[dependencies]\nserde = \"1.0\"\n")
            .into_iter()
            .next()
        else {
            return;
        };
        let mut result = CheckResult::new(item, DependencyStatus::UpToDate);
        // A key holding a two-byte character before the version: bytes 8..11
        // are characters 7..10.
        let manifest = "[dependencies]\n\"é\" = \"1.0\"\n";
        result.item.version_line = 1;
        result.item.version_col_start = 8;
        result.item.version_col_end = 11;
        let hint = hints(manifest, &[result.clone()]);
        assert_eq!(
            hint.iter().map(|h| (h.col_start, h.col_end)).next(),
            Some((7, 10))
        );

        // An offset past the line, or inside a character, cannot be measured
        // and falls back to column zero rather than panicking.
        result.item.version_col_start = 2;
        result.item.version_col_end = 400;
        let hint = hints(manifest, &[result]);
        assert_eq!(
            hint.iter().map(|h| (h.col_start, h.col_end)).next(),
            Some((0, 0))
        );
    }
}
