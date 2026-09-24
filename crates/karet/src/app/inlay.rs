//! Keeping the visible part of each editor annotated with inlay hints.
//!
//! Hints are a function of the viewport, not of the document: a server asked
//! to annotate a 10,000-line file so the editor can decorate forty rows spends
//! the whole file's inference on every scroll. So the request is ranged, and
//! the range is only re-asked when the viewport leaves what the current set
//! covers or the buffer changes underneath it.
//!
//! The request is issued *after* a frame, beside `graph_prefetch`, for the
//! same reason that one is: the viewport is a property of having been painted.

use karet_core::InlayHint;
use karet_core::LineCol;
use karet_core::Range;
use karet_session::api::Command as SessionCommand;
use karet_session::api::DocumentId;
use karet_session::api::RequestId;

use super::App;
use crate::tab::TabKind;

/// Lines requested above and below the viewport.
///
/// Without an overscan every scrolled row is a fresh round trip; with one,
/// ordinary scrolling stays inside the covered range and asks for nothing.
const OVERSCAN: u32 = 64;

/// A span of a document at one revision — the unit a hint set is asked for and
/// answered over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HintRange {
    /// The buffer version it was computed against.
    pub(crate) version: u64,
    /// First line, inclusive.
    pub(crate) first: u32,
    /// Last line, inclusive.
    pub(crate) last: u32,
}

impl HintRange {
    /// Whether this range already answers everything `wanted` asks for.
    fn covers(self, wanted: Self) -> bool {
        self.version == wanted.version && self.first <= wanted.first && self.last >= wanted.last
    }
}

/// A hint request that has not been answered yet.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingInlay {
    /// The request to match the answer against.
    pub(crate) id: RequestId,
    /// What it asked for.
    pub(crate) asked: HintRange,
    /// The invalidation epoch it was issued in.
    ///
    /// A request issued before the last invalidation no longer counts as
    /// covering anything, so it cannot suppress the re-ask that invalidation
    /// exists to trigger — but it is still *adopted* if it arrives, because a
    /// real answer is worth having whatever prompted the re-ask.
    pub(crate) epoch: u64,
}

impl App {
    /// Ask for hints covering every visible editor whose current set no longer
    /// does.
    ///
    /// Called once per frame. Cheap in the common case: a scroll inside the
    /// overscan, or a document already covered at this version, sends nothing.
    pub(crate) fn request_inlay_hints(&mut self) {
        if !self.settings.editor.inlay_hints.enabled {
            return;
        }
        for (doc, wanted) in self.visible_hint_ranges() {
            if self.hints_are_current(doc, wanted) {
                continue;
            }
            let Some(id) = self.send(SessionCommand::InlayHints {
                doc,
                range: Range {
                    start: LineCol::new(wanted.first, 0),
                    // The end of the last line, not its start: ending at
                    // column 0 excludes every hint on the final requested line.
                    end: LineCol::new(wanted.last, u32::MAX),
                },
            }) else {
                continue;
            };
            let epoch = self.docs.inlay_epoch;
            self.docs.inlay_pending.insert(
                doc,
                PendingInlay {
                    id,
                    asked: wanted,
                    epoch,
                },
            );
        }
    }

    /// The `(version, line range)` each open code document wants covered.
    ///
    /// `all_tabs`, not `self.tabs`: the latter is only the *focused* pane's,
    /// so a split's background pane would never be asked about and would keep
    /// a stale set until its document closed.
    ///
    /// One document open in two panes contributes one range spanning both
    /// viewports, because the cache is keyed by document. Taking whichever
    /// pane was seen last instead would make the annotations vanish from the
    /// other pane every time focus moved between them.
    fn visible_hint_ranges(&self) -> Vec<(DocumentId, HintRange)> {
        let mut wanted: Vec<(DocumentId, HintRange)> = Vec::new();
        for range in self.hint_ranges_per_view() {
            match wanted.iter_mut().find(|(doc, _)| *doc == range.0) {
                // Same document in another pane: widen to cover both.
                Some((_, have)) if have.version == range.1.version => {
                    have.first = have.first.min(range.1.first);
                    have.last = have.last.max(range.1.last);
                },
                Some(_) => {},
                None => wanted.push(range),
            }
        }
        wanted
    }

    /// One range per *view*, before the per-document union above.
    fn hint_ranges_per_view(&self) -> Vec<(DocumentId, HintRange)> {
        self.all_tabs()
            .filter_map(|tab| {
                let TabKind::Code {
                    doc: Some(doc),
                    buffer,
                    ..
                } = &tab.kind
                else {
                    return None;
                };
                let top = tab.editor.scroll_line;
                // A tab that has never been painted reports no visible lines;
                // one screenful is a better guess than none.
                let visible = tab.editor.visible_lines().max(1);
                let last = (buffer.line_count() as u32).saturating_sub(1);
                let first = top.saturating_sub(OVERSCAN);
                Some((
                    *doc,
                    HintRange {
                        version: buffer.version(),
                        first,
                        // Clamped up to `first`: a document that shrank under a
                        // scrolled tab before the next paint would otherwise
                        // produce an inverted range and send it to the server.
                        last: top
                            .saturating_add(visible)
                            .saturating_add(OVERSCAN)
                            .min(last)
                            .max(first),
                    },
                ))
            })
            .collect()
    }

    /// The version of `doc`'s buffer as the app currently holds it.
    fn buffer_version(&self, doc: DocumentId) -> Option<u64> {
        self.all_tabs().find_map(|tab| match &tab.kind {
            TabKind::Code {
                doc: Some(id),
                buffer,
                ..
            } if *id == doc => Some(buffer.version()),
            _ => None,
        })
    }

    /// Whether `doc` is already covered, or already being asked about, over a
    /// range that includes everything `wanted` needs.
    fn hints_are_current(&self, doc: DocumentId, wanted: HintRange) -> bool {
        if self
            .docs
            .inlay_covered
            .get(&doc)
            .is_some_and(|have| have.covers(wanted))
        {
            return true;
        }
        // An identical request already in flight: asking twice would only race
        // two answers for the same range. Only one from the current epoch,
        // though -- a request issued before the last invalidation may be about
        // to be answered by a provider that was not running when it was made.
        self.docs.inlay_pending.get(&doc).is_some_and(|pending| {
            pending.epoch == self.docs.inlay_epoch && pending.asked.covers(wanted)
        })
    }

    /// Adopt an answered hint set.
    ///
    /// A set is taken only if it answers the request still outstanding for the
    /// document: the buffer can be edited while a request is in flight, and a
    /// hint positioned against text that has since changed annotates the wrong
    /// column.
    pub(crate) fn on_inlay_hints(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        version: u64,
        hints: Vec<InlayHint>,
    ) {
        let Some(pending) = self.docs.inlay_pending.get(&doc).copied() else {
            return; // nothing outstanding: a late answer to a retired request
        };
        if id != Some(pending.id) || pending.asked.version != version {
            return; // superseded
        }
        self.docs.inlay_pending.remove(&doc);
        if self
            .buffer_version(doc)
            .is_some_and(|current| current != version)
        {
            return; // the buffer moved on while the server was thinking
        }
        self.docs.inlay_hints.insert(doc, hints);
        // Coverage only from the current epoch. An answer from before the last
        // invalidation is worth painting, but it was produced under conditions
        // that have since changed -- during startup, by a provider that was not
        // running -- so it must not stop the next frame asking again.
        if pending.epoch == self.docs.inlay_epoch {
            self.docs.inlay_covered.insert(doc, pending.asked);
        }
    }

    /// Drop everything cached for `doc`, on close or on a language change.
    pub(crate) fn forget_inlay_hints(&mut self, doc: DocumentId) {
        self.docs.inlay_hints.remove(&doc);
        self.docs.inlay_covered.remove(&doc);
        self.docs.inlay_pending.remove(&doc);
    }

    /// Forget what every document is covered for, without dropping what is on
    /// screen.
    ///
    /// Called when a language server's runtime state changes. An empty set
    /// answered while nothing was running is indistinguishable, at this layer,
    /// from a document that genuinely has no hints -- so a provider coming up
    /// has to be treated as making every previous answer suspect. Startup is
    /// exactly this race: the first frame is painted long before a server is
    /// ready, and its empty answer would otherwise be cached forever.
    pub(crate) fn invalidate_inlay_coverage(&mut self) {
        self.docs.inlay_covered.clear();
        // The epoch moves rather than the pending map being cleared. An
        // in-flight request is not abandoned -- clearing it would discard an
        // answer that is about to arrive, and during startup these events land
        // constantly -- but it stops counting as coverage, so a request that
        // will never be answered (a settings reload bumps the manager
        // generation, and `apply_lsp_update` drops updates from the previous
        // one) can no longer suppress the re-ask forever.
        self.docs.inlay_epoch = self.docs.inlay_epoch.saturating_add(1);
    }

    /// Invalidate the coverage for `doc` without dropping what is on screen.
    ///
    /// Used when the buffer changes: the hints are now positioned against
    /// stale text, but blanking them on every keystroke would make the
    /// annotations strobe. They stay until the replacement arrives.
    pub(crate) fn stale_inlay_hints(&mut self, doc: DocumentId) {
        self.docs.inlay_covered.remove(&doc);
    }
}
