//! Running one index, streaming each package as it lands.
//!
//! The engine walks; this decides what the walk is allowed to skip and what happens to
//! each package the moment it is finished. Both are the same seam —
//! [`karet_seam::IndexObserver`] — which is why the engine needs to know nothing about
//! caches or events.
//!
//! Everything here runs on rayon's workers, not the seam thread: [`Observer::cached`] is
//! asked once per file from whichever thread is reading it, and
//! [`Observer::package_indexed`] fires from whichever thread finished that package. The
//! event sender is the only thing that crosses back, and it is built for exactly that.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use karet_seam::Configuration;
use karet_seam::FileContribution;
use karet_seam::FileStamp;
use karet_seam::IndexObserver;
use karet_seam::IndexedPackage;
use tokio::sync::mpsc as tokio_mpsc;

use super::project;
use crate::api::Event;
use crate::api::RequestId;
use crate::seam_cache::SeamCache;

/// What one index run needs from its caller, and gives back to it.
pub(super) struct Observer<'a> {
    /// Correlates every event this run emits.
    id: RequestId,
    /// What may be replayed instead of read. Empty for a forced re-sync.
    cache: &'a SeamCache,
    /// The configuration each package is resolved under before it is sent.
    ///
    /// Applied per package rather than once at the end, because a package is sent the
    /// moment it is done and must be right when it arrives.
    configuration: Option<Configuration>,
    events: &'a tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
    /// Contributions from every package, for the cache write once the run is over.
    contributions: Mutex<Vec<FileContribution>>,
    /// How many files were parsed rather than replayed.
    parsed: AtomicUsize,
    /// How many source files the walk covered.
    ///
    /// Counted from the walk rather than read off the finished index's file table: that
    /// table also holds each package's manifest, which is an anchor the walk never reads,
    /// so reporting it would claim files were skipped that were never candidates.
    walked: AtomicUsize,
}

impl<'a> Observer<'a> {
    pub(super) fn new(
        id: RequestId,
        cache: &'a SeamCache,
        configuration: Option<Configuration>,
        events: &'a tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
    ) -> Self {
        Self {
            id,
            cache,
            configuration,
            events,
            contributions: Mutex::new(Vec::new()),
            parsed: AtomicUsize::new(0),
            walked: AtomicUsize::new(0),
        }
    }

    /// Everything the run collected, to be stored for next time.
    pub(super) fn into_contributions(self) -> Vec<FileContribution> {
        self.contributions.into_inner().unwrap_or_default()
    }

    /// How many files this run actually had to parse.
    pub(super) fn parsed(&self) -> usize {
        self.parsed.load(Ordering::Relaxed)
    }

    /// How many source files this run covered, parsed or replayed.
    pub(super) fn walked(&self) -> usize {
        self.walked.load(Ordering::Relaxed)
    }
}

impl IndexObserver for Observer<'_> {
    fn cached(&self, file: &Path, stamp: FileStamp) -> Option<FileContribution> {
        self.cache.get(file, stamp)
    }

    fn package_indexed(&self, indexed: &mut IndexedPackage) {
        // Resolved here, not after the merge: this package is about to be shown, and a
        // node whose membership is settled a second later would be shown wrong first.
        if let Some(configuration) = self.configuration.as_ref() {
            karet_seam::config::apply(&mut indexed.index, configuration);
        }

        self.parsed.fetch_add(indexed.parsed, Ordering::Relaxed);
        self.walked
            .fetch_add(indexed.contributions.len(), Ordering::Relaxed);
        // Taken rather than copied: this is every node the package produced, and the only
        // thing left to do with it is write it out once the run is over.
        if let Ok(mut held) = self.contributions.lock() {
            held.append(&mut indexed.contributions);
        }

        let index = &indexed.index;
        let Some(root) = index
            .roots()
            .first()
            .and_then(|id| index.path(*id))
            .map(ToString::to_string)
        else {
            return;
        };
        // Walked from the root rather than iterated out of the arena: the order nodes
        // cross the wire in is the order the view lists them in, and a map's order is not
        // stable between runs.
        let nodes = index
            .roots()
            .iter()
            .flat_map(|id| index.subtree(*id))
            .filter_map(|id| index.node(id))
            .filter_map(|node| project::node_view(index, node))
            .collect();
        let unresolved_modules = index
            .unresolved_modules()
            .iter()
            .filter_map(|(id, candidates)| Some((index.path(*id)?.to_string(), candidates.clone())))
            .collect();

        let _ = self.events.send((
            Some(self.id),
            Event::SeamPackageIndexed {
                order: indexed.order,
                root,
                nodes,
                unresolved_modules,
            },
        ));
    }
}
