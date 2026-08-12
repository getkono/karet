//! The workspace spelling scan's session half: seed the open documents from
//! their live buffers, then hand the rest of the tree to the scan worker.

use std::collections::HashSet;

use super::*;
use crate::api::SpellingHit;
use crate::api::SpellingLanguage;
use crate::spell::check::word_in_line;
use crate::spell_scan::resolve_path;

impl Session {
    /// Spell-check the whole workspace on the scan worker, answering
    /// [`Command::ScanWorkspaceSpelling`].
    ///
    /// Open documents are answered here, from their live buffers, and their paths are
    /// handed to the worker as a skip list — the scan reads from disk, which is stale
    /// for an unsaved edit. Everything else is the worker's walk.
    pub(crate) fn scan_workspace_spelling(&mut self, id: RequestId, limit: usize) {
        let spelling_language = self
            .config
            .settings
            .spellcheck
            .enabled
            .then(|| SpellingLanguage::parse(&self.config.settings.spellcheck.language))
            .flatten();
        let (Some(root), Some(spelling_language)) =
            (self.config.roots.first().cloned(), spelling_language)
        else {
            // Disabled, unsupported, or root-less: finish immediately rather than
            // leaving the client waiting on a scan that will never run.
            self.emit(
                Some(id),
                Event::SpellingScanFinished {
                    files_scanned: 0,
                    truncated: false,
                    cancelled: false,
                },
            );
            return;
        };

        let mut open = HashSet::new();
        let mut hits = Vec::new();
        for document in self.store.docs.values() {
            // The walk yields paths as `ignore` composed them, which need not be
            // spelled the way the client opened the document (`notes.md` against
            // `./notes.md`, a symlinked root against its target). Comparing the
            // resolved form is what keeps a file from being reported twice — once
            // correctly from the buffer, once from stale disk text.
            open.insert(resolve_path(&document.path));
            let room = limit.saturating_sub(hits.len());
            hits.extend(document_hits(document).take(room));
        }
        // The seeded hits are part of the same list, so they spend the same
        // budget; leaving the worker a fresh `limit` let the panel hold twice the
        // cap and report `truncated: false` for a list that was in fact cut off.
        let seeded = hits.len();
        if seeded > 0 {
            self.emit(
                Some(id),
                Event::SpellingScanProgress {
                    hits,
                    files_scanned: 0,
                },
            );
        }
        if seeded >= limit {
            self.emit(
                Some(id),
                Event::SpellingScanFinished {
                    files_scanned: 0,
                    truncated: true,
                    cancelled: false,
                },
            );
            return;
        }

        let job = crate::spell_scan::SpellScanJob {
            id,
            root,
            spelling_language,
            settings: self.config.settings.clone(),
            open,
            limit: limit - seeded,
            cancel: self.cancellations.register(id),
        };
        if self.spell_scan_worker.send(job).is_err() {
            self.emit(
                Some(id),
                Event::SpellingScanFinished {
                    files_scanned: 0,
                    truncated: false,
                    cancelled: false,
                },
            );
        }
    }

    /// Publish one document's spelling layer, so a client holding workspace scan
    /// results can replace what it has for this file.
    ///
    /// Called wherever the layer changes. A scan is a photograph of the workspace
    /// and starts going stale the moment it is taken; this keeps the file the user
    /// is actually looking at in step with what the editor underlines.
    pub(crate) fn publish_spelling(&self, doc_id: DocumentId) {
        let Some(document) = self.store.docs.get(&doc_id) else {
            return;
        };
        self.emit(
            None,
            Event::SpellingUpdated {
                path: document.path.clone(),
                hits: document_hits(document).collect(),
            },
        );
    }
}

/// This document's spell diagnostics as [`SpellingHit`]s, each carrying its line
/// as list context.
fn document_hits(document: &Document) -> impl Iterator<Item = SpellingHit> + '_ {
    document.spell_diagnostics.iter().map(|diagnostic| {
        let line = diagnostic.range.start.line;
        let text = document.buffer.line(line as usize).unwrap_or_default();
        SpellingHit {
            word: word_in_line(&text, diagnostic.range),
            path: document.path.clone(),
            range: diagnostic.range,
            line_text: text.trim().to_owned(),
        }
    })
}
