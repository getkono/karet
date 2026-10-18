//! Per-node call facts, kept beside the tree rather than inside it.
//!
//! A side table rather than a field on [`crate::model::Node`] because only callables have
//! these facts, and because node ids churn — regrouping, merging, replaying and removing
//! all re-key the tree. Each of those paths calls the matching hook here, so a fact never
//! outlives its node or stays attached to an id that now names something else.

use std::collections::HashMap;

use super::site::Callable;
use super::site::FileCalls;
use crate::id::SeamId;
use crate::model::FileId;

/// Call facts per callable node and per file.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CallTable {
    callables: HashMap<SeamId, Callable>,
    files: HashMap<FileId, FileCalls>,
}

impl CallTable {
    /// The facts recorded for `id`, when it is a callable.
    #[must_use]
    pub fn callable(&self, id: SeamId) -> Option<&Callable> {
        self.callables.get(&id)
    }

    /// Every callable, in id order.
    #[must_use]
    pub fn callables(&self) -> Vec<(SeamId, &Callable)> {
        let mut out: Vec<(SeamId, &Callable)> =
            self.callables.iter().map(|(id, c)| (*id, c)).collect();
        out.sort_unstable_by_key(|(id, _)| *id);
        out
    }

    /// How many callables are recorded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.callables.len()
    }

    /// Whether no callable is recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.callables.is_empty()
    }

    /// The file-level facts recorded for `file`.
    #[must_use]
    pub fn file(&self, file: FileId) -> Option<&FileCalls> {
        self.files.get(&file)
    }

    /// Every file's facts, in file order.
    #[must_use]
    pub fn files(&self) -> Vec<(FileId, &FileCalls)> {
        let mut out: Vec<(FileId, &FileCalls)> =
            self.files.iter().map(|(id, f)| (*id, f)).collect();
        out.sort_unstable_by_key(|(id, _)| *id);
        out
    }

    /// Record `callable` against `id`, replacing whatever was there.
    pub(crate) fn insert(&mut self, id: SeamId, callable: Callable) {
        self.callables.insert(id, callable);
    }

    /// Record a file's facts, replacing whatever was there.
    pub(crate) fn set_file(&mut self, file: FileId, calls: FileCalls) {
        self.files.insert(file, calls);
    }

    /// Re-key every callable that moved.
    pub(crate) fn remap(&mut self, remap: &HashMap<SeamId, SeamId>) {
        if remap.is_empty() || self.callables.is_empty() {
            return;
        }
        let moved: Vec<(SeamId, SeamId)> = self
            .callables
            .keys()
            .filter_map(|old| Some((*old, *remap.get(old)?)))
            .collect();
        // Taken out first and put back after, so a chain of moves (a → b while b → c)
        // cannot overwrite a fact that has not moved yet.
        let taken: Vec<(SeamId, Callable)> = moved
            .iter()
            .filter_map(|(old, new)| Some((*new, self.callables.remove(old)?)))
            .collect();
        self.callables.extend(taken);
    }

    /// Forget a node that left the tree.
    pub(crate) fn remove(&mut self, id: SeamId) {
        self.callables.remove(&id);
    }

    /// Forget a file's own facts, ahead of re-reading it.
    pub(crate) fn remove_file(&mut self, file: FileId) {
        self.files.remove(&file);
    }

    /// Take over another index's facts, re-keyed into this index's numbering.
    ///
    /// A callable or file the maps do not name is dropped: it belonged to a node the
    /// merge itself did not carry over.
    pub(crate) fn absorb(
        &mut self,
        other: Self,
        ids: &HashMap<SeamId, SeamId>,
        files: &HashMap<FileId, FileId>,
    ) {
        for (old, callable) in other.callables {
            if let Some(new) = ids.get(&old) {
                self.callables.insert(*new, callable);
            }
        }
        for (old, calls) in other.files {
            if let Some(new) = files.get(&old) {
                self.files.insert(*new, calls);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_lines(code_lines: u32) -> Callable {
        Callable {
            code_lines,
            ..Callable::default()
        }
    }

    #[test]
    fn a_chain_of_moves_keeps_every_fact() {
        let mut table = CallTable::default();
        table.insert(SeamId(1), with_lines(10));
        table.insert(SeamId(2), with_lines(20));
        // 1 moves to where 2 was while 2 moves on: neither may be lost.
        table.remap(&HashMap::from([
            (SeamId(1), SeamId(2)),
            (SeamId(2), SeamId(3)),
        ]));
        assert_eq!(table.callable(SeamId(1)), None);
        assert_eq!(table.callable(SeamId(2)).map(|c| c.code_lines), Some(10));
        assert_eq!(table.callable(SeamId(3)).map(|c| c.code_lines), Some(20));
        assert_eq!(table.len(), 2);
    }

    #[test]
    fn absorbing_drops_what_the_merge_did_not_carry() {
        let mut other = CallTable::default();
        other.insert(SeamId(1), with_lines(1));
        other.insert(SeamId(2), with_lines(2));
        other.set_file(FileId(0), FileCalls::default());
        other.set_file(FileId(1), FileCalls::default());

        let mut table = CallTable::default();
        table.absorb(
            other,
            &HashMap::from([(SeamId(1), SeamId(7))]),
            &HashMap::from([(FileId(1), FileId(4))]),
        );
        assert_eq!(table.callables().len(), 1);
        assert_eq!(table.callable(SeamId(7)).map(|c| c.code_lines), Some(1));
        assert!(table.file(FileId(4)).is_some());
        assert!(table.file(FileId(0)).is_none());
    }

    #[test]
    fn removal_forgets_nodes_and_files_independently() {
        let mut table = CallTable::default();
        table.insert(SeamId(1), with_lines(1));
        table.set_file(FileId(0), FileCalls::default());
        table.remove(SeamId(1));
        assert!(table.is_empty());
        assert_eq!(table.files().len(), 1);
        table.remove_file(FileId(0));
        assert!(table.files().is_empty());
    }
}
