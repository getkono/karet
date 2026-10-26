//! Keeping each visible document's language-server links and colours, and
//! following the link a Ctrl/Cmd+click lands on.
//!
//! Both are listed for the whole document in one request, so the schedule is
//! simpler than the inlay hints' it rides beside: after a frame, for the
//! front tab of each visible pane, once typing has paused, asked again only
//! when the text has changed (or a server's state has). The bookkeeping is
//! theirs otherwise, held in [`LinkColorState`]: one request in flight per
//! document, a failure that leaves what is shown standing and backs off, and
//! a held set that moves with the text until its replacement arrives. A span
//! an edit reaches into is dropped rather than moved: a colour whose literal
//! is being retyped no longer spells the colour it shows.
//!
//! The colours are drawn as swatches beside the built-in detector's (see
//! `ui::content::swatches`); the links answer the Ctrl/Cmd-hover underline
//! and Ctrl/Cmd+click ([`App::server_link_at`], [`App::follow_document_link`]).

use karet_core::DocumentColor;
use karet_core::DocumentLink;
use karet_session::LSP_CHANGE_DEBOUNCE;

use super::inlay::inlay_retry_delay;
use super::inlay::shift_position;
use super::*;

/// A held set, with the buffer version its ranges are positioned against.
pub(crate) type Held<T> = HashMap<DocumentId, (u64, Vec<T>)>;

/// The colours held for each document.
pub(crate) type HeldColors = Held<DocumentColor>;

/// The links and colours held for each document, and the requests for them.
#[derive(Default)]
pub(crate) struct LinkColorState {
    /// The latest links per document, ranges in buffer columns.
    pub(crate) links: Held<DocumentLink>,
    /// The latest colours per document, ranges in buffer columns.
    pub(crate) colors: HeldColors,
    /// The version each document was last answered whole for, in the epoch
    /// it was asked in: nothing to ask until the text moves on.
    covered: HashMap<DocumentId, (u64, u64)>,
    /// The outstanding request per document: its id, the version it asked
    /// against, and the epoch it was issued in.
    pending: HashMap<DocumentId, (RequestId, u64, u64)>,
    /// Bumped when coverage is invalidated, so a request issued before it
    /// stops counting as covering anything.
    epoch: u64,
    /// When each recently edited or failing document may be asked again.
    quiet_until: HashMap<DocumentId, Instant>,
    /// Requests in a row each document's server has left unanswered.
    failures: HashMap<DocumentId, u32>,
    /// The link resolve awaiting its answer.
    resolving: Option<RequestId>,
}

/// Where `range` lands once `edits` are applied, or `None` when an edit
/// reached into it -- replaced any of it, or inserted strictly inside it.
/// Typing right before or right after the span moves or leaves it.
#[must_use]
pub(crate) fn shift_span(range: Range, edits: &[TextEdit]) -> Option<Range> {
    let reached = edits
        .iter()
        .any(|edit| edit.range.start < range.end && edit.range.end > range.start);
    if reached {
        return None;
    }
    let start = shift_position(range.start, edits)?;
    let end = shift_position(range.end, edits)?;
    Some(Range { start, end })
}

/// Carry a held set across `edits`, which took its text from version `from`
/// to `to`; only a set positioned against `from` is moved, so a document
/// reached by two paths is not moved twice.
fn shift_held<T>(
    held: &mut Held<T>,
    doc: DocumentId,
    (from, to): (u64, u64),
    edits: &[TextEdit],
    range: impl Fn(&mut T) -> &mut Range,
) {
    let Some((version, items)) = held.get_mut(&doc) else {
        return;
    };
    if *version != from {
        return;
    }
    items.retain_mut(|item| {
        let span = range(item);
        match shift_span(*span, edits) {
            Some(moved) => {
                *span = moved;
                true
            },
            None => false,
        }
    });
    *version = to;
}

/// The items of `held` for `doc`, when they are positioned against
/// `version`; none otherwise, since they describe other text.
pub(crate) fn current_items<T>(held: &Held<T>, doc: DocumentId, version: u64) -> &[T] {
    held.get(&doc)
        .filter(|(at, _)| *at == version)
        .map_or(&[], |(_, items)| items.as_slice())
}

impl App {
    /// Ask for the links and colours of every visible document whose held
    /// set does not answer its current text. Called once per frame, beside
    /// [`Self::request_inlay_hints`].
    pub(crate) fn request_links_and_colors(&mut self) {
        self.request_links_and_colors_at(Instant::now());
    }

    /// [`Self::request_links_and_colors`] at a given moment, so the edit
    /// debounce can be tested without sleeping.
    pub(crate) fn request_links_and_colors_at(&mut self, now: Instant) {
        self.links_colors
            .quiet_until
            .retain(|_, until| *until > now);
        for (doc, wanted) in self.visible_hint_ranges() {
            let state = &self.links_colors;
            let version = wanted.version;
            let epoch = state.epoch;
            let current = state.covered.get(&doc) == Some(&(version, epoch))
                || state
                    .pending
                    .get(&doc)
                    .is_some_and(|&(_, asked, at)| asked == version && at == epoch);
            if current || state.quiet_until.contains_key(&doc) {
                continue;
            }
            let Some(id) = self.send(SessionCommand::LinksAndColors { doc }) else {
                continue;
            };
            self.links_colors.pending.insert(doc, (id, version, epoch));
        }
    }

    /// Adopt an answered listing, if it answers the request still
    /// outstanding for `doc` against the text the buffer still holds.
    ///
    /// Each half that was answered replaces what is held; a half that was
    /// not leaves it standing, and the document is asked again after the
    /// back-off, as for a listing that failed whole.
    pub(crate) fn on_links_and_colors(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        version: u64,
        (links, colors): (Option<Vec<DocumentLink>>, Option<Vec<DocumentColor>>),
    ) {
        self.on_links_and_colors_at(id, doc, version, (links, colors), Instant::now());
    }

    /// [`Self::on_links_and_colors`] at a given moment, so the back-off can
    /// be tested without sleeping.
    pub(crate) fn on_links_and_colors_at(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        version: u64,
        (links, colors): (Option<Vec<DocumentLink>>, Option<Vec<DocumentColor>>),
        now: Instant,
    ) {
        let state = &mut self.links_colors;
        let Some(&(pending, asked, epoch)) = state.pending.get(&doc) else {
            return;
        };
        if id != Some(pending) || asked != version {
            return; // a late answer to a retired request
        }
        state.pending.remove(&doc);
        if self
            .buffer_version(doc)
            .is_some_and(|current| current != version)
        {
            return; // the buffer moved on while the server was thinking
        }
        let state = &mut self.links_colors;
        let whole = links.is_some() && colors.is_some();
        if let Some(links) = links {
            state.links.insert(doc, (version, links));
        }
        if let Some(colors) = colors {
            state.colors.insert(doc, (version, colors));
        }
        if whole {
            state.failures.remove(&doc);
            // Coverage only from the current epoch, as for inlay hints: an
            // answer from before the last invalidation is shown, but asked
            // again.
            if epoch == state.epoch {
                state.covered.insert(doc, (version, epoch));
            }
            return;
        }
        let failures = state.failures.entry(doc).or_insert(0);
        *failures = failures.saturating_add(1);
        let retry_at = now + inlay_retry_delay(*failures);
        state.quiet_until.insert(doc, retry_at);
    }

    /// Drop everything held for `doc`, on close.
    pub(crate) fn forget_links_and_colors(&mut self, doc: DocumentId) {
        let state = &mut self.links_colors;
        state.links.remove(&doc);
        state.colors.remove(&doc);
        state.covered.remove(&doc);
        state.pending.remove(&doc);
        state.quiet_until.remove(&doc);
        state.failures.remove(&doc);
    }

    /// Forget what every document is covered for, without dropping what is
    /// shown: a language server's state changed, so an answer given while it
    /// was not running is suspect (see `invalidate_inlay_coverage`).
    pub(crate) fn invalidate_link_color_coverage(&mut self) {
        let state = &mut self.links_colors;
        state.covered.clear();
        for doc in std::mem::take(&mut state.failures).into_keys() {
            state.quiet_until.remove(&doc);
        }
        state.epoch = state.epoch.saturating_add(1);
    }

    /// Note that `doc`'s text just changed, holding its next request until
    /// the edits pause.
    pub(crate) fn note_link_color_edit(&mut self, doc: DocumentId, now: Instant) {
        self.links_colors.failures.remove(&doc);
        let until = now + LSP_CHANGE_DEBOUNCE;
        self.links_colors.quiet_until.insert(doc, until);
    }

    /// Carry `doc`'s held links and colours across `edits`, which took its
    /// text from version `from` to `to`.
    pub(crate) fn shift_links_and_colors(
        &mut self,
        doc: DocumentId,
        from: u64,
        to: u64,
        edits: &[TextEdit],
    ) {
        let state = &mut self.links_colors;
        shift_held(&mut state.links, doc, (from, to), edits, |link| {
            &mut link.range
        });
        shift_held(&mut state.colors, doc, (from, to), edits, |colour| {
            &mut colour.range
        });
    }

    /// How long until a document being edited, or backing off, may be asked
    /// again, so the event loop wakes to ask without further input.
    pub(crate) fn link_color_next_wake(&self, now: Instant) -> Option<Duration> {
        let quiet = self.links_colors.quiet_until.values();
        quiet
            .filter(|until| **until > now)
            .map(|until| until.saturating_duration_since(now))
            .min()
    }

    /// The language server's link under `pos` in `doc`, whose buffer is at
    /// `version`: `None` when there is none, or the held links describe other
    /// text.
    pub(crate) fn server_link_at(
        &self,
        doc: DocumentId,
        version: u64,
        pos: LineCol,
    ) -> Option<&DocumentLink> {
        current_items(&self.links_colors.links, doc, version)
            .iter()
            .find(|link| link.contains(pos))
    }

    /// Follow `link`, listed for `doc`: open its target, or first ask its
    /// server for the target it deferred, following it once it arrives.
    pub(crate) fn follow_document_link(&mut self, doc: DocumentId, link: DocumentLink) {
        if let Some(target) = &link.target {
            self.open_server_link_target(target);
            return;
        }
        let id = self.send(SessionCommand::ResolveDocumentLink { doc, link });
        self.links_colors.resolving = id;
    }

    /// Settle the answer to a link resolve: keep the resolved link, so the
    /// next click needs no round trip, and follow it -- only when it answers
    /// the click still waiting, since a newer click replaced an older one.
    pub(crate) fn on_document_link_resolved(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        result: Result<DocumentLink, String>,
    ) {
        if self
            .links_colors
            .resolving
            .take_if(|pending| Some(*pending) == id)
            .is_none()
        {
            return;
        }
        match result {
            Ok(link) => {
                if let Some((_, links)) = self.links_colors.links.get_mut(&doc)
                    && let Some(held) = links.iter_mut().find(|held| held.range == link.range)
                {
                    *held = link.clone();
                }
                if let Some(target) = &link.target {
                    self.open_server_link_target(target);
                }
            },
            Err(reason) => self.notify(
                Report::Failure,
                NotificationKind::Lsp,
                format!("could not follow the link: {reason}"),
            ),
        }
    }

    /// Open a language server's link target: a web or mail link with the
    /// desktop, a workspace file in the editor (at the `#L<line>,<column>`
    /// its fragment names, if any), and a file outside the workspace only
    /// once the user confirms it.
    pub(crate) fn open_server_link_target(&mut self, target: &str) {
        use crate::links::LinkTarget;
        match crate::links::resolve_absolute(target, &self.root) {
            Ok((LinkTarget::ExternalUrl(url), _)) => {
                if let Err(error) = crate::links::open_external(&url) {
                    self.notify(
                        Report::Failure,
                        NotificationKind::System,
                        format!("could not open link: {error}"),
                    );
                }
            },
            Ok((LinkTarget::WorkspaceFile { path, .. }, Some(position))) if path.is_file() => {
                self.jump_to_location(&path, position);
            },
            Ok((LinkTarget::WorkspaceFile { path, .. }, _)) => self.open_markdown_file_link(&path),
            Ok((LinkTarget::OutsideWorkspaceFile(path), _)) => {
                self.confirm_outside_workspace_link(path);
            },
            Err(error) => self.notify(
                Report::Refusal,
                NotificationKind::System,
                format!("link blocked: {error}"),
            ),
        }
    }
}
