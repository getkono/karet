//! Fetching semantic tokens from the server that owns them for a language.
//!
//! Shaped like [`pull`](super::pull): each open document is asked at every
//! version the server has been sent, off the server task's serial path and at
//! most once at a time, a newer version aborting the fetch still in flight for
//! an older one. The answer travels as an [`LspUpdate::SemanticTokens`]
//! stamped with the version asked, so the session drops one the text has
//! since moved past.
//!
//! Only the slot that owns semantic tokens for its language fetches them --
//! the language's primary server unless `lsp.languages.<id>.semanticTokens`
//! names another -- and only while `editor.semanticHighlighting.enabled` is
//! on. Both arrive on a [`watch`] channel the manager holds the sending half
//! of, so turning the setting on or off reaches a running task without a
//! restart: on wakes the task to fetch, off stops every fetch.
//!
//! Each document's last answer is kept, so the next request can be a delta
//! against it. A delta that does not fit (the server forgot the result, or
//! its edits do not apply) is retried once as a whole-document request, and a
//! request the server cancels (`ServerCancelled`) is asked again, a bounded
//! number of times, as the advertised `serverCancelSupport` promises.
//! `workspace/semanticTokens/refresh` forgets which versions were asked, so
//! every open document is fetched again.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use karet_core::ServerFeature;
use karet_lsp::LspClient;
use karet_lsp::LspError;
use karet_lsp::SemanticTokens;
use karet_lsp::ServerRefresh;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::AbortHandle;
use tokio::task::JoinSet;

use super::commands::OpenDocument;
use super::folding::FoldFetches;
use super::message::LspUpdate;
use super::pull::Pulls;
use super::slot::SlotKey;
use super::slot::SlotToken;

/// Each document's last answer, shared with the fetches that replace it.
type Previous = Arc<Mutex<HashMap<PathBuf, SemanticTokens>>>;

/// One connection's semantic-token fetches.
pub(super) struct SemanticFetches {
    /// Whether this slot fetches at all: it owns semantic tokens for its
    /// language and the setting is on.
    enabled: watch::Receiver<bool>,
    /// The version last asked for each open document.
    asked: HashMap<PathBuf, i32>,
    /// The fetch in flight for each document, to abort when superseded.
    in_flight: HashMap<PathBuf, AbortHandle>,
    /// Every launched fetch. Each holds the client, so they are stopped
    /// before a polite shutdown needs to own it outright.
    tasks: JoinSet<()>,
    previous: Previous,
    /// The connection's refresh requests, subscribed on its first launch.
    refreshes: Option<broadcast::Receiver<ServerRefresh>>,
}

impl SemanticFetches {
    /// Fetches gated on `enabled`.
    pub(super) fn new(enabled: watch::Receiver<bool>) -> Self {
        Self {
            enabled,
            asked: HashMap::new(),
            in_flight: HashMap::new(),
            tasks: JoinSet::new(),
            previous: Previous::default(),
            refreshes: None,
        }
    }

    /// Fetch every open document whose sent version has not been asked yet.
    ///
    /// `pending` names the document with an edit still buffered, which is
    /// skipped like one awaiting a resync: the server has not seen the
    /// version an answer would be stamped with.
    pub(super) fn launch_ready(
        &mut self,
        client: &Arc<LspClient>,
        documents: &HashMap<PathBuf, OpenDocument>,
        pending: Option<&Path>,
        (updates, key, token): (&mpsc::UnboundedSender<LspUpdate>, &SlotKey, SlotToken),
    ) {
        if !*self.enabled.borrow() {
            return;
        }
        self.refreshes.get_or_insert_with(|| client.refreshes());
        while self.tasks.try_join_next().is_some() {}
        self.asked.retain(|path, _| documents.contains_key(path));
        if let Ok(mut previous) = self.previous.lock() {
            previous.retain(|path, _| documents.contains_key(path));
        }
        self.in_flight.retain(|path, handle| {
            let open = documents.contains_key(path);
            if !open {
                handle.abort();
            }
            open && !handle.is_finished()
        });
        for (path, document) in documents {
            if pending == Some(path.as_path())
                || document.resync
                || self.asked.get(path) == Some(&document.version)
                || !client.supports_for(ServerFeature::SemanticTokensFull, path)
            {
                continue;
            }
            self.asked.insert(path.clone(), document.version);
            if let Some(superseded) = self.in_flight.remove(path) {
                superseded.abort();
            }
            let handle = self.tasks.spawn(fetch(
                Arc::clone(client),
                path.clone(),
                document.version,
                Arc::clone(&self.previous),
                (updates.clone(), key.clone(), token),
            ));
            self.in_flight.insert(path.clone(), handle);
        }
    }

    /// Stop every fetch and forget what was asked and answered: the
    /// connection they were asked on is gone, and its successor -- with a
    /// legend and result ids of its own -- is asked afresh.
    pub(super) fn abandon(&mut self) {
        self.tasks.abort_all();
        self.forget();
        self.refreshes = None;
    }

    /// Stop every fetch and wait until each has let go of the client.
    pub(super) async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
        self.forget();
        self.refreshes = None;
    }

    fn forget(&mut self) {
        self.asked.clear();
        self.in_flight.clear();
        if let Ok(mut previous) = self.previous.lock() {
            previous.clear();
        }
    }

    /// Resolve when the task should look again: the setting changed, or the
    /// server asked for a refresh. Never, otherwise -- before the first
    /// launch, or once the manager has let go of the setting.
    ///
    /// Cancel-safe, as the server task's `select!` needs: each arm is.
    pub(super) async fn changed(&mut self) {
        loop {
            let look = {
                let Self {
                    enabled, refreshes, ..
                } = &mut *self;
                let refresh = async {
                    match refreshes.as_mut() {
                        Some(refreshes) => refreshes.recv().await,
                        None => std::future::pending().await,
                    }
                };
                tokio::select! {
                    changed = enabled.changed() => match changed {
                        Ok(()) => Look::Toggled,
                        // The manager dropped the slot; the task is going.
                        Err(_) => Look::Never,
                    },
                    received = refresh => match received {
                        // A lag lost only duplicates of signals that all mean
                        // "look again", so it is taken as one.
                        Ok(ServerRefresh::SemanticTokens)
                        | Err(broadcast::error::RecvError::Lagged(_)) => Look::Refresh,
                        Ok(_) => Look::Ignore,
                        Err(broadcast::error::RecvError::Closed) => Look::Closed,
                    },
                }
            };
            match look {
                Look::Toggled => {
                    // Off then on again before this task looked shows only
                    // as "on", yet the session dropped every overlay when it
                    // went off: so turning on asks every document afresh.
                    self.asked.clear();
                    if !*self.enabled.borrow_and_update() {
                        self.tasks.abort_all();
                        self.forget();
                    }
                    return;
                },
                Look::Refresh => {
                    self.asked.clear();
                    return;
                },
                Look::Ignore => {},
                Look::Closed => self.refreshes = None,
                Look::Never => std::future::pending::<()>().await,
            }
        }
    }
}

/// The per-version fetches a connection makes for its open documents --
/// diagnostic pulls, semantic tokens and folding ranges -- launched, abandoned
/// and stopped together, so the server task treats them as one.
pub(super) struct DocumentFetches {
    pulls: Pulls,
    semantic: SemanticFetches,
    folds: FoldFetches,
}

impl DocumentFetches {
    /// Fetches whose semantic tokens are gated on `semantic`, and whose
    /// folding ranges are asked only of a `primary` slot.
    pub(super) fn new(semantic: watch::Receiver<bool>, primary: bool) -> Self {
        Self {
            pulls: Pulls::default(),
            semantic: SemanticFetches::new(semantic),
            folds: FoldFetches::new(primary),
        }
    }

    /// Launch every fetch the documents' sent versions call for (see
    /// [`Pulls::launch_ready`], [`SemanticFetches::launch_ready`] and
    /// [`FoldFetches::launch_ready`]).
    pub(super) fn launch_ready(
        &mut self,
        client: &Arc<LspClient>,
        documents: &HashMap<PathBuf, OpenDocument>,
        pending: Option<&Path>,
        slot: (&mpsc::UnboundedSender<LspUpdate>, &SlotKey, SlotToken),
    ) {
        self.pulls.launch_ready(client, documents, pending, slot);
        self.semantic.launch_ready(client, documents, pending, slot);
        self.folds.launch_ready(client, documents, pending, slot);
    }

    /// Stop every fetch; the connection is gone.
    pub(super) fn abandon(&mut self) {
        self.pulls.abandon();
        self.semantic.abandon();
        self.folds.abandon();
    }

    /// Stop every fetch and wait until each has let go of the client.
    pub(super) async fn shutdown(&mut self) {
        self.pulls.shutdown().await;
        self.semantic.shutdown().await;
        self.folds.shutdown().await;
    }

    /// Resolve when a document skipped or already asked may need fetching
    /// now: a capability registration, a semantic-highlighting change, or a
    /// semantic-token or diagnostic refresh. Cancel-safe.
    pub(super) async fn woken(&mut self) {
        tokio::select! {
            () = self.pulls.woken() => {},
            () = self.semantic.changed() => {},
        }
    }
}

/// What woke [`SemanticFetches::changed`].
enum Look {
    /// The setting changed.
    Toggled,
    /// The server asked for a refresh.
    Refresh,
    /// The server asked to refresh something else.
    Ignore,
    /// The refresh stream ended with its connection.
    Closed,
    /// The setting's sender is gone with the slot.
    Never,
}

/// Fetch `path`'s semantic tokens at `version` and relay them decoded.
async fn fetch(
    client: Arc<LspClient>,
    path: PathBuf,
    version: i32,
    previous: Previous,
    (updates, key, token): (mpsc::UnboundedSender<LspUpdate>, SlotKey, SlotToken),
) {
    let last = previous
        .lock()
        .ok()
        .and_then(|previous| previous.get(&path).cloned());
    let answer = ask(&client, &path, last.as_ref()).await;
    let answer = match answer {
        // The delta did not fit what is kept, or the server no longer knows
        // the result it was against: ask for the whole document once.
        Err(LspError::Protocol(_) | LspError::Server(_)) if last.is_some() => {
            ask(&client, &path, None).await
        },
        answer => answer,
    };
    let answer = match answer {
        Ok(answer) => answer,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "semantic tokens failed");
            return;
        },
    };
    let tokens = client
        .semantic_tokens_legend()
        .map(|legend| answer.decode(&legend))
        .unwrap_or_default();
    if let Ok(mut previous) = previous.lock() {
        previous.insert(path.clone(), answer);
    }
    let _ = updates.send(LspUpdate::SemanticTokens {
        token,
        server: key,
        path,
        version,
        tokens,
    });
}

/// How many times a request the server cancels is asked again before the
/// document waits for its next version or a refresh.
const CANCELLED_RETRIES: usize = 3;

/// Ask for `path`'s tokens, asking again when the server cancels.
///
/// The client advertises `serverCancelSupport`, which promises the server
/// that a request it cancels (`ServerCancelled`, `-32802`) is re-triggered;
/// without this a cancelled first request would leave the document with no
/// overlay until its next edit. Bounded, so a server that cancels every
/// request cannot keep the fetch spinning.
async fn ask(
    client: &LspClient,
    path: &Path,
    last: Option<&SemanticTokens>,
) -> Result<SemanticTokens, LspError> {
    let mut answer = client.semantic_tokens(path, last).await;
    for _ in 0..CANCELLED_RETRIES {
        match &answer {
            Err(error) if is_server_cancelled(error) => {
                answer = client.semantic_tokens(path, last).await;
            },
            _ => break,
        }
    }
    answer
}

/// Whether `error` is LSP's `ServerCancelled` (`-32802`): the server gave up
/// on the request and expects the client to ask again. The code survives only
/// in the message `karet-lsp` formats for a peer error.
pub(super) fn is_server_cancelled(error: &LspError) -> bool {
    matches!(error, LspError::Server(message) if message.contains("code -32802"))
}
