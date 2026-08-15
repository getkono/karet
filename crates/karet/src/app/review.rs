//! Code-review bookkeeping: which changed files of which commits have been
//! looked at, persisted per workspace so a review survives restarts.
//!
//! Stored in the karet cache directory (keyed by workspace root), never in
//! the repository. Entries expire after ninety days — a review that old is a
//! new review.

use std::collections::HashMap;
use std::collections::HashSet;

use super::*;

/// Seconds in ninety days.
const EXPIRY_SECS: u64 = 90 * 24 * 60 * 60;

/// The persisted review state.
#[derive(Default)]
pub(crate) struct ReviewStore {
    /// Reviewed file paths per commit hash, with the last-touched time.
    entries: HashMap<String, (u64, HashSet<String>)>,
    loaded: bool,
    dirty: bool,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

impl ReviewStore {
    fn path(root: &Path) -> Option<PathBuf> {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        use std::hash::Hash as _;
        use std::hash::Hasher as _;
        root.hash(&mut hash);
        directories::ProjectDirs::from("", "", "karet").map(|dirs| {
            dirs.cache_dir()
                .join(format!("review-{:016x}.json", hash.finish()))
        })
    }

    fn ensure_loaded(&mut self, root: &Path) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let Some(text) = Self::path(root).and_then(|path| std::fs::read_to_string(path).ok())
        else {
            return;
        };
        let Ok(parsed) = serde_json::from_str::<HashMap<String, (u64, HashSet<String>)>>(&text)
        else {
            return;
        };
        let cutoff = now_secs().saturating_sub(EXPIRY_SECS);
        self.entries = parsed
            .into_iter()
            .filter(|(_, (touched, _))| *touched >= cutoff)
            .collect();
    }

    fn save(&mut self, root: &Path) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let Some(path) = Self::path(root) else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string(&self.entries) {
            let _ = std::fs::write(path, text);
        }
    }

    /// Whether `file` of `commit` is marked reviewed.
    pub(crate) fn is_reviewed(&mut self, root: &Path, commit: &str, file: &str) -> bool {
        self.ensure_loaded(root);
        self.entries
            .get(commit)
            .is_some_and(|(_, files)| files.contains(file))
    }

    /// Toggle `file` of `commit`, persisting immediately; returns the new state.
    pub(crate) fn toggle(&mut self, root: &Path, commit: &str, file: &str) -> bool {
        self.ensure_loaded(root);
        let (touched, files) = self
            .entries
            .entry(commit.to_owned())
            .or_insert_with(|| (now_secs(), HashSet::new()));
        *touched = now_secs();
        let reviewed = if files.remove(file) {
            false
        } else {
            files.insert(file.to_owned());
            true
        };
        self.dirty = true;
        self.save(root);
        reviewed
    }
}

impl App {
    /// Stamp the review flags onto a commit's file views.
    pub(super) fn apply_review_flags(&mut self, commit: &str) {
        let root = self.root.clone();
        let commit = commit.to_owned();
        let review = &mut self.review;
        for tab in self.tabs.iter_mut() {
            if let TabKind::Commit { detail, files, .. } = &mut tab.kind
                && detail.hash == commit
            {
                for file in &mut files.files {
                    file.reviewed =
                        review.is_reviewed(&root, &commit, &file.change.path.to_string_lossy());
                }
            }
        }
    }

    /// Toggle the current file's reviewed mark in a commit view (`x`).
    pub(super) fn commit_toggle_reviewed(&mut self) {
        let root = self.root.clone();
        let Some(TabKind::Commit {
            detail,
            files,
            view,
            ..
        }) = self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
        else {
            return;
        };
        if files.files.is_empty() {
            return;
        }
        let current = view
            .file_anchors
            .iter()
            .rposition(|anchor| *anchor <= view.scroll)
            .unwrap_or(0);
        let Some(file) = files.files.get_mut(current) else {
            return;
        };
        let path = file.change.path.to_string_lossy().into_owned();
        let reviewed = self.review.toggle(&root, &detail.hash, &path);
        file.reviewed = reviewed;
        let done = files.files.iter().filter(|f| f.reviewed).count();
        self.status = Some(format!(
            "{path}: {} ({done}/{} reviewed)",
            if reviewed { "reviewed" } else { "unreviewed" },
            files.files.len()
        ));
    }
}
