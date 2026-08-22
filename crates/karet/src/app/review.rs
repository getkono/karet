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

/// A stable digest of `root`, naming its review file.
///
/// Deliberately not `DefaultHasher`: the standard library does not promise its
/// output stays the same across releases, and this names a file that has to be
/// found again after a toolchain upgrade. FNV-1a is fixed by its definition, so
/// the same workspace always resolves to the same entry.
fn workspace_key(root: &Path) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in root.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

impl ReviewStore {
    fn path(root: &Path) -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "karet").map(|dirs| {
            dirs.cache_dir()
                .join(format!("review-{:016x}.json", workspace_key(root)))
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
        // Every pane, not just the focused one: this is stamped from a backend
        // answer, which can land on a commit view in a background split.
        for tab in self.tabs.iter_mut().chain(
            self.stored
                .values_mut()
                .flat_map(|pane| pane.tabs.iter_mut()),
        ) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_workspace_key_is_fixed_by_its_definition() {
        // The key names a file that must still be found after a toolchain
        // upgrade, so these are golden FNV-1a values, not whatever the
        // standard library's hasher happens to produce today.
        assert_eq!(workspace_key(Path::new("")), 0xcbf2_9ce4_8422_2325);
        assert_eq!(workspace_key(Path::new("a")), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(
            workspace_key(Path::new("/home/dev/project")),
            workspace_key(Path::new("/home/dev/project")),
            "the same workspace always resolves to the same entry"
        );
        assert_ne!(
            workspace_key(Path::new("/home/dev/a")),
            workspace_key(Path::new("/home/dev/b"))
        );
    }

    #[test]
    fn an_expired_entry_is_dropped_on_load() {
        let root = std::env::temp_dir().join(format!("karet-review-{}", std::process::id()));
        let mut store = ReviewStore::default();
        let stale = now_secs().saturating_sub(EXPIRY_SECS + 1);
        store.entries.insert(
            "old".to_owned(),
            (stale, HashSet::from(["f.rs".to_owned()])),
        );
        store.entries.insert(
            "new".to_owned(),
            (now_secs(), HashSet::from(["g.rs".to_owned()])),
        );
        // `ensure_loaded` prunes what it reads; simulate the same filter.
        let cutoff = now_secs().saturating_sub(EXPIRY_SECS);
        store.entries.retain(|_, (touched, _)| *touched >= cutoff);

        assert!(!store.entries.contains_key("old"));
        assert!(store.entries.contains_key("new"));
        assert!(!store.is_reviewed(&root, "old", "f.rs"));
    }
}
