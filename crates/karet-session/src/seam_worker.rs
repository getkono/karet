//! The seam-index worker: a dedicated thread that owns the index.
//!
//! Indexing a package parses every file in it, which is blocking, filesystem-bound work
//! that must never run on the actor thread. The index also *stays* here rather than being
//! rebuilt per request: it holds a parser pool and an edge store, an edit re-indexes one
//! file rather than the package, and queries evaluate against the live structure.
//!
//! Jobs coalesce the way the user's intent does. A newer index request supersedes an
//! unstarted older one, and so does a newer query — a reader typing into the filter box
//! means the last thing they typed, not every prefix of it. A re-index is never dropped,
//! because dropping one would leave the index describing text that no longer exists.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;

use karet_seam::Configuration;
use karet_seam::IndexOptions;
use karet_seam::SeamIndex;
use karet_seam::SeamPath;
use karet_treesitter::ParserPool;
use tokio::sync::mpsc as tokio_mpsc;

use crate::api::Event;
use crate::api::RequestId;
use crate::api::SeamQueryError;

mod project;

pub(crate) use project::edges_of;
pub(crate) use project::node_view;
pub(crate) use project::summary_of;

/// One unit of background seam work.
pub(crate) enum SeamJob {
    /// Index a package and answer with [`Event::SeamIndexed`].
    Index {
        /// Correlates the answering event.
        id: RequestId,
        /// The package root to index.
        root: PathBuf,
        /// How much of it to index.
        options: IndexOptions,
    },
    /// Re-index one edited file in place.
    Reindex {
        /// Correlates the answering event.
        id: RequestId,
        /// The file that changed.
        path: PathBuf,
        /// Its current text, which may be unsaved buffer content.
        text: String,
    },
    /// Evaluate a query and answer with [`Event::SeamQueryResult`].
    Query {
        /// Correlates the answering event.
        id: RequestId,
        /// The query text, exactly as typed.
        text: String,
    },
    /// Fetch one node's edges and answer with [`Event::SeamNodeDetail`].
    Node {
        /// Correlates the answering event.
        id: RequestId,
        /// The node's identity.
        path: String,
    },
    /// Switch the active configuration and re-answer with [`Event::SeamIndexed`].
    SetConfiguration {
        /// Correlates the answering event.
        id: RequestId,
        /// The configuration to activate.
        name: String,
    },
}

/// Start the worker; the session sends [`SeamJob`]s and answers arrive on the shared
/// event stream.
pub(crate) fn spawn(
    events: tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) -> Sender<SeamJob> {
    let (jobs_tx, jobs_rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("karet-seam".to_owned())
        .spawn(move || run(&jobs_rx, &events));
    jobs_tx
}

/// The worker's own state: the index it owns and what it was built under.
#[derive(Default)]
struct Worker {
    index: Option<SeamIndex>,
    root: Option<PathBuf>,
    configuration: Option<Configuration>,
    available: Vec<Configuration>,
    pool: ParserPool,
}

fn run(jobs: &Receiver<SeamJob>, events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>) {
    let mut worker = Worker::default();
    while let Ok(mut job) = jobs.recv() {
        // Coalesce what the user's intent coalesces: a newer index or query supersedes
        // an unstarted older one. A re-index is never dropped — losing one would leave
        // the index describing text that no longer exists.
        while let Ok(next) = jobs.try_recv() {
            let supersedes = matches!(
                (&job, &next),
                (SeamJob::Index { .. }, SeamJob::Index { .. })
                    | (SeamJob::Query { .. }, SeamJob::Query { .. })
            );
            if supersedes {
                job = next;
            } else {
                worker.execute(job, events);
                job = next;
            }
        }
        worker.execute(job, events);
    }
}

impl Worker {
    /// Run one job and emit its answer.
    fn execute(
        &mut self,
        job: SeamJob,
        events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
    ) {
        match job {
            SeamJob::Index { id, root, options } => self.index_package(id, &root, options, events),
            SeamJob::Reindex { id, path, text } => self.reindex(id, &path, &text, events),
            SeamJob::Query { id, text } => self.query(id, &text, events),
            SeamJob::Node { id, path } => self.node(id, &path, events),
            SeamJob::SetConfiguration { id, name } => self.set_configuration(id, &name, events),
        }
    }

    /// Build the index for a package from scratch.
    fn index_package(
        &mut self,
        id: RequestId,
        root: &std::path::Path,
        options: IndexOptions,
        events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
    ) {
        match karet_seam::index_package(root, options) {
            Ok(index) => {
                self.index = Some(index);
                self.root = Some(root.to_path_buf());
                self.available = project::configurations_for(root);
                self.configuration = self.available.first().cloned();
                self.apply_configuration();
                self.emit_index(Some(id), events);
            },
            Err(error) => {
                let _ = events.send((
                    Some(id),
                    Event::SeamIndexFailed {
                        message: error.to_string(),
                    },
                ));
            },
        }
    }

    /// Re-index one file, then re-answer with the whole tree.
    ///
    /// The re-index itself is incremental — only this file's nodes are rebuilt — but the
    /// answer carries the full tree, because the presentation layer holds a copy and a
    /// partial update would leave it inconsistent for no saving worth having.
    fn reindex(
        &mut self,
        id: RequestId,
        path: &std::path::Path,
        text: &str,
        events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
    ) {
        let Some(index) = self.index.as_mut() else {
            return;
        };
        if karet_seam::reindex_file(index, &mut self.pool, path, text).is_err() {
            // A file that will not parse leaves the previous tree standing rather than
            // emptying the view mid-edit.
            return;
        }
        self.apply_configuration();
        self.emit_index(Some(id), events);
    }

    /// Evaluate a query against the live index.
    fn query(
        &mut self,
        id: RequestId,
        text: &str,
        events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
    ) {
        let Some(index) = self.index.as_ref() else {
            return;
        };
        match karet_seam::query::parse(text) {
            Ok(query) => {
                // A `config:` directive in the query switches the evaluation context, so
                // the same string means the same thing whether typed or sent by an agent.
                let result = karet_seam::query::evaluate(&query, index);
                let nodes = result
                    .nodes
                    .into_iter()
                    .filter_map(|node| index.path(node).map(ToString::to_string))
                    .collect();
                let _ = events.send((
                    Some(id),
                    Event::SeamQueryResult {
                        nodes,
                        configuration: result.configuration,
                        error: None,
                    },
                ));
            },
            Err(error) => {
                let _ = events.send((
                    Some(id),
                    Event::SeamQueryResult {
                        nodes: Vec::new(),
                        configuration: None,
                        error: Some(SeamQueryError {
                            message: error.describe(),
                            start: error.span.start,
                            end: error.span.end,
                            suggestions: error.suggestions,
                        }),
                    },
                ));
            },
        }
    }

    /// Answer with one node's edges.
    fn node(
        &mut self,
        id: RequestId,
        path: &str,
        events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
    ) {
        let Some(index) = self.index.as_ref() else {
            return;
        };
        let target = path
            .parse::<SeamPath>()
            .ok()
            .and_then(|parsed| index.resolve(&parsed));
        let edges = target.map(|node| edges_of(index, node)).unwrap_or_default();
        let _ = events.send((
            Some(id),
            Event::SeamNodeDetail {
                node: path.to_owned(),
                edges,
            },
        ));
    }

    /// Switch configuration and re-evaluate every node's membership.
    fn set_configuration(
        &mut self,
        id: RequestId,
        name: &str,
        events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
    ) {
        if let Some(found) = self
            .available
            .iter()
            .find(|candidate| candidate.name == name)
            .cloned()
        {
            self.configuration = Some(found);
        }
        self.apply_configuration();
        self.emit_index(Some(id), events);
    }

    /// Re-evaluate membership and rollups under the active configuration.
    fn apply_configuration(&mut self) {
        let (Some(index), Some(configuration)) = (self.index.as_mut(), self.configuration.as_ref())
        else {
            return;
        };
        karet_seam::config::apply(index, configuration);
    }

    /// Emit the whole tree plus its summary.
    fn emit_index(
        &self,
        id: Option<RequestId>,
        events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
    ) {
        let Some(index) = self.index.as_ref() else {
            return;
        };
        let nodes = index
            .nodes()
            .filter_map(|node| node_view(index, node))
            .collect();
        let summary = summary_of(index, self.configuration.as_ref(), &self.available);
        let _ = events.send((id, Event::SeamIndexed { summary, nodes }));
    }
}
