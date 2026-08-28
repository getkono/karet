//! The workspace search's session half: resolve the roots and the configured
//! excludes, then hand the walk to the search worker.

use super::*;
use crate::search_worker::SearchJob;

impl Session {
    /// Submit a streaming workspace search over every root.
    ///
    /// The query's own excludes are *additive* to the workspace's configured
    /// `search.exclude`, the same way the todo and spelling scans treat them, so a
    /// user narrowing a search never accidentally widens it past the project's
    /// settings.
    pub(crate) fn search_workspace(
        &mut self,
        id: RequestId,
        mut query: karet_search::SearchQuery,
        file_limit: usize,
        match_limit: usize,
    ) {
        let roots = self.config.roots.clone();
        if roots.is_empty() || query.pattern.is_empty() {
            self.finish_search_now(id);
            return;
        }
        query
            .excludes
            .extend(self.config.settings.search.exclude.iter().cloned());
        let job = SearchJob::Search {
            id,
            roots,
            query,
            file_limit,
            match_limit,
            cancel: self.cancellations.register(id),
        };
        if self.search_worker.send(job).is_err() {
            self.finish_search_now(id);
        }
    }

    /// Submit a workspace replace-all over every root.
    ///
    /// Replace spans the same roots the search does; leaving it on the first root
    /// alone would let the panel list a match that "replace all" refuses to touch.
    pub(crate) fn search_replace_all(
        &mut self,
        id: RequestId,
        mut query: karet_search::SearchQuery,
        replacement: String,
    ) {
        let roots = self.config.roots.clone();
        if roots.is_empty() || query.pattern.is_empty() {
            self.emit(
                Some(id),
                Event::SearchReplaced {
                    files_changed: 0,
                    replacements: 0,
                },
            );
            return;
        }
        query
            .excludes
            .extend(self.config.settings.search.exclude.iter().cloned());
        let job = SearchJob::ReplaceAll {
            id,
            roots,
            query,
            replacement,
        };
        if self.search_worker.send(job).is_err() {
            self.emit(
                Some(id),
                Event::SearchReplaced {
                    files_changed: 0,
                    replacements: 0,
                },
            );
        }
    }

    /// Answer a search that never reached the worker, so no request is left
    /// unresolved.
    fn finish_search_now(&mut self, id: RequestId) {
        self.emit(
            Some(id),
            Event::SearchFinished {
                files_scanned: 0,
                matches_found: 0,
                truncated: false,
                cancelled: false,
                error: None,
            },
        );
    }
}
