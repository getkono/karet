//! The workspace spelling scan's session half: seed the open documents from
//! their live buffers, then hand the rest of the tree to the scan worker.

use std::collections::HashSet;

use super::*;
use crate::api::SpellingHit;
use crate::api::SpellingLanguage;
use crate::spell::check::word_in_line;

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
            open.insert(document.path.clone());
            for diagnostic in &document.spell_diagnostics {
                if hits.len() >= limit {
                    break;
                }
                let line = diagnostic.range.start.line;
                let text = document.buffer.line(line as usize).unwrap_or_default();
                hits.push(SpellingHit {
                    word: word_in_line(&text, diagnostic.range),
                    path: document.path.clone(),
                    range: diagnostic.range,
                    line_text: text.trim().to_owned(),
                });
            }
        }
        if !hits.is_empty() {
            self.emit(
                Some(id),
                Event::SpellingScanProgress {
                    hits,
                    files_scanned: 0,
                },
            );
        }

        let job = crate::spell_scan::SpellScanJob {
            id,
            root,
            spelling_language,
            settings: self.config.settings.clone(),
            open,
            limit,
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
}
