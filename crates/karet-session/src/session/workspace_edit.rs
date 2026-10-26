//! The one path every workspace edit is applied through: a rename's, a
//! client's [`Command::ApplyWorkspaceEdit`], and a language server's
//! `workspace/applyEdit`.
//!
//! Text edits land in the session's documents, never straight on disk: a file
//! no client has open is opened first, so every change is unsaved, undoable
//! per document, and visible to the client like any other edit. File
//! operations (create, rename, delete) act on disk, and a rename retargets the
//! open documents it moves.
//!
//! Failure handling is what the language client advertises
//! (`textOnlyTransactional`): an edit of text alone is checked whole against
//! scratch copies before any document changes, so it applies entirely or not
//! at all; an edit with file operations runs its steps in order and stops at
//! the first that fails, keeping the steps before it.

use std::collections::HashMap;

use karet_core::ApplyWorkspaceEditResult;
use karet_core::DeleteOptions;
use karet_core::DocumentChange;
use karet_core::DocumentEdit;
use karet_core::FileOptions;
use karet_core::WorkspaceEdit;

use super::columns::edits_to_buffer_columns;
use super::*;
use crate::api::AppliedWorkspaceEdit;

/// Each document's version before an edit's first step reached it, keyed by
/// document so a rename between steps cannot lose it; `None` for a document
/// the edit itself opened, which no client could have versioned.
type Baselines = HashMap<DocumentId, Option<u64>>;

/// One step of an edit, with its index into `document_changes` when it has one
/// (the unordered `changes` half has none to report).
struct Step {
    index: Option<usize>,
    change: DocumentChange,
}

/// Flatten `edit` into the order it applies in: `changes`, then
/// `document_changes`.
fn steps(edit: WorkspaceEdit) -> Vec<Step> {
    let unordered = edit.changes.into_iter().map(|(path, edits)| Step {
        index: None,
        change: DocumentChange::Edit(DocumentEdit {
            path,
            version: None,
            edits,
        }),
    });
    let ordered = edit
        .document_changes
        .into_iter()
        .enumerate()
        .map(|(index, change)| Step {
            index: Some(index),
            change,
        });
    unordered.chain(ordered).collect()
}

/// Why a file cannot be edited as text, in words for the user.
fn load_failure(path: &Path, error: &DocumentLoadError) -> String {
    match error {
        DocumentLoadError::Missing => format!("{} does not exist", path.display()),
        error => format!("cannot edit {}: {error}", path.display()),
    }
}

/// Whether anything exists at `path`, a dangling symlink included.
fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Whether `to` is another name for the directory entry `from` is, rather
/// than a different file in the way: both resolve to one path, and neither is
/// itself a symlink (an entry of its own, distinct from what it points at).
fn same_entry(from: &Path, to: &Path) -> bool {
    let is_link = |path: &Path| {
        std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
    };
    !is_link(from)
        && !is_link(to)
        && matches!(
            (std::fs::canonicalize(from), std::fs::canonicalize(to)),
            (Ok(a), Ok(b)) if a == b
        )
}

/// Create `path`'s parent directories.
fn create_parent(path: &Path) -> Result<(), String> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display())),
        _ => Ok(()),
    }
}

/// Remove whatever is at `path`, a directory with everything in it.
fn remove_any(path: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(path)?.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// `path` moved from under `from` to under `to`, or `None` when it was not
/// under `from`.
fn rebase(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    let rest = path.strip_prefix(from).ok()?;
    Some(if rest.as_os_str().is_empty() {
        to.to_path_buf()
    } else {
        to.join(rest)
    })
}

impl Session {
    /// Apply `edit`, reporting what changed and whether it all applied.
    pub(super) fn apply_workspace_edit(
        &mut self,
        edit: WorkspaceEdit,
        columns: Columns,
        label: Option<String>,
    ) -> AppliedWorkspaceEdit {
        let mut applied = AppliedWorkspaceEdit {
            label,
            ..AppliedWorkspaceEdit::default()
        };
        let steps = steps(edit);
        let text_only = steps
            .iter()
            .all(|step| matches!(step.change, DocumentChange::Edit(_)));
        if text_only && let Err(result) = self.check_text_edits(&steps, columns) {
            applied.result = result;
            return applied;
        }
        let mut baselines = Baselines::new();
        for step in steps {
            if let Err(reason) = self.apply_step(step.change, columns, &mut baselines, &mut applied)
            {
                applied.result = ApplyWorkspaceEditResult::failed(reason, step.index);
                return applied;
            }
        }
        applied.result = ApplyWorkspaceEditResult::applied();
        applied
    }

    /// The open document at `path`: by the path it was opened under, else by
    /// what both resolve to, so a symlinked or relative spelling still finds
    /// the document instead of opening the file a second time.
    pub(super) fn document_at(&self, path: &Path) -> Option<DocumentId> {
        if let Some(&doc) = self.store.by_path.get(path) {
            return Some(doc);
        }
        let target = std::fs::canonicalize(path).ok()?;
        self.store.docs.iter().find_map(|(&doc, document)| {
            std::fs::canonicalize(&document.path)
                .is_ok_and(|resolved| resolved == target)
                .then_some(doc)
        })
    }

    /// Dry-run every text step against scratch copies, so a text-only edit
    /// that would fail anywhere changes nothing at all.
    fn check_text_edits(
        &self,
        steps: &[Step],
        columns: Columns,
    ) -> Result<(), ApplyWorkspaceEditResult> {
        let mut scratch: HashMap<PathBuf, TextBuffer> = HashMap::new();
        for step in steps {
            let DocumentChange::Edit(edit) = &step.change else {
                continue;
            };
            let fail = |reason: String| ApplyWorkspaceEditResult::failed(reason, step.index);
            let open = self
                .document_at(&edit.path)
                .and_then(|doc| self.store.docs.get(&doc));
            // Every step names the version the whole edit was computed
            // against, which the live document (untouched by a dry run) still
            // holds -- so each step is checked, not only the first per file.
            if let Some(document) = open {
                check_version(edit, document.buffer.version()).map_err(fail)?;
            }
            if !scratch.contains_key(&edit.path) {
                let buffer = match open {
                    Some(document) => document.buffer.clone(),
                    None => {
                        load_document(&edit.path)
                            .map_err(|error| fail(load_failure(&edit.path, &error)))?
                            .0
                    },
                };
                scratch.insert(edit.path.clone(), buffer);
            }
            let Some(buffer) = scratch.get_mut(&edit.path) else {
                continue;
            };
            let edits = edits_to_buffer_columns(buffer, edit.edits.clone(), columns);
            buffer
                .apply_simple(&Change::new(buffer.version(), edits))
                .map_err(|error| fail(format!("cannot edit {}: {error}", edit.path.display())))?;
        }
        Ok(())
    }

    /// Apply one step, recording what it changed in `applied`.
    fn apply_step(
        &mut self,
        change: DocumentChange,
        columns: Columns,
        baselines: &mut Baselines,
        applied: &mut AppliedWorkspaceEdit,
    ) -> Result<(), String> {
        match change {
            DocumentChange::Edit(edit) => self.apply_text_step(edit, columns, baselines, applied),
            DocumentChange::Create { path, options } => create_file(&path, options),
            DocumentChange::Rename { from, to, options } => {
                self.rename_path(&from, &to, options, applied)
            },
            DocumentChange::Delete { path, options } => delete_path(&path, options),
            // `DocumentChange` is non-exhaustive: a step this build does not
            // know cannot be applied as its producer meant.
            _ => Err("the edit holds a step this editor does not know".to_owned()),
        }
    }

    /// Edit one file's text through its document, opening it if no client has.
    fn apply_text_step(
        &mut self,
        edit: DocumentEdit,
        columns: Columns,
        baselines: &mut Baselines,
        applied: &mut AppliedWorkspaceEdit,
    ) -> Result<(), String> {
        let open = self
            .document_at(&edit.path)
            .and_then(|doc| Some((doc, self.store.docs.get(&doc)?)));
        let doc = match open {
            Some((doc, document)) => {
                // Checked against the version before this edit's own earlier
                // steps advanced it: that is the one the edit was computed
                // against. A document this edit opened has none to check.
                let baseline = *baselines
                    .entry(doc)
                    .or_insert(Some(document.buffer.version()));
                if let Some(version) = baseline {
                    check_version(&edit, version)?;
                }
                doc
            },
            None => {
                // Checked before opening, because `open` makes an empty
                // document for a missing file and reports one it cannot decode
                // as a request for a byte view.
                load_document(&edit.path).map_err(|error| load_failure(&edit.path, &error))?;
                self.open(None, edit.path.clone(), None);
                let doc = self
                    .document_at(&edit.path)
                    .ok_or_else(|| format!("cannot open {}", edit.path.display()))?;
                applied.opened.push((doc, edit.path.clone()));
                baselines.insert(doc, None);
                doc
            },
        };
        if edit.edits.is_empty() {
            return Ok(());
        }
        self.commit_edits(doc, edit.edits, columns)
            .map_err(|error| format!("cannot edit {}: {error}", edit.path.display()))?;
        if !applied.edited.contains(&edit.path) {
            applied.edited.push(edit.path);
        }
        Ok(())
    }

    /// Apply `edits` to an open document as one undoable step.
    fn commit_edits(
        &mut self,
        doc_id: DocumentId,
        edits: Vec<TextEdit>,
        columns: Columns,
    ) -> Result<(), TextError> {
        let tick = self.elapsed_ms();
        let mut retired = crate::lsp::Retired::default();
        let spell_without_syntax = {
            let highlight_tx = &self.highlight_tx;
            let settings = &self.config.settings;
            let lsp = &mut self.lsp;
            let Some(doc) = self.store.docs.get_mut(&doc_id) else {
                return Ok(());
            };
            let edits = edits_to_buffer_columns(&doc.buffer, edits, columns);
            let change = Change::new(doc.buffer.version(), edits);
            // The caret the client last reported, so undoing the edit puts the
            // user back where they were rather than at its first change.
            let ctx = save_edit_context(tick, doc, &change);
            let result = doc.buffer.apply(&change, ctx)?;
            let _ = update_syntax(settings, highlight_tx, doc_id, doc, Some(&result.edits));
            doc.sync_dirty_since(tick);
            retired.absorb(lsp.document_changed(
                doc.language_selector,
                &doc.path,
                &doc.buffer,
                Some(&result),
            ));
            doc.lang_id.is_none()
        };
        self.adopt_retirement(retired);
        self.publish(doc_id, None);
        if spell_without_syntax {
            self.schedule_spell(doc_id);
        }
        Ok(())
    }

    /// Move a file or directory, then retarget every open document under it.
    fn rename_path(
        &mut self,
        from: &Path,
        to: &Path,
        options: FileOptions,
        applied: &mut AppliedWorkspaceEdit,
    ) -> Result<(), String> {
        if !exists(from) {
            return Err(format!("{} does not exist", from.display()));
        }
        if from == to {
            return Ok(());
        }
        // `to` already names `from` itself -- a case-only rename on a
        // case-insensitive filesystem, or another spelling of the same path:
        // it is not in the way, and removing it would delete the source.
        if exists(to) && !same_entry(from, to) {
            if options.overwrite {
                remove_any(to)
                    .map_err(|error| format!("cannot replace {}: {error}", to.display()))?;
            } else if options.ignore_if_exists {
                return Ok(());
            } else {
                return Err(format!("{} already exists", to.display()));
            }
        }
        create_parent(to)?;
        std::fs::rename(from, to).map_err(|error| {
            format!(
                "cannot move {} to {}: {error}",
                from.display(),
                to.display()
            )
        })?;
        let moved: Vec<(DocumentId, PathBuf)> = self
            .store
            .docs
            .iter()
            .filter_map(|(&doc, document)| Some((doc, rebase(&document.path, from, to)?)))
            .collect();
        for (doc, path) in moved {
            self.retarget(None, doc, path);
        }
        for path in &mut applied.edited {
            if let Some(moved) = rebase(path, from, to) {
                *path = moved;
            }
        }
        for (_, path) in &mut applied.opened {
            if let Some(moved) = rebase(path, from, to) {
                *path = moved;
            }
        }
        applied.renamed.push((from.to_path_buf(), to.to_path_buf()));
        Ok(())
    }
}

/// Refuse an edit computed against a version other than `version`, the
/// document's version before the edit began.
fn check_version(edit: &DocumentEdit, version: u64) -> Result<(), String> {
    match edit.version {
        Some(named) if named != crate::lsp::version_i32(version) => Err(format!(
            "{} changed since the edit was computed",
            edit.path.display()
        )),
        _ => Ok(()),
    }
}

/// Create an empty file, as `options` says to treat one already there.
fn create_file(path: &Path, options: FileOptions) -> Result<(), String> {
    if exists(path) && !options.overwrite {
        return if options.ignore_if_exists {
            Ok(())
        } else {
            Err(format!("{} already exists", path.display()))
        };
    }
    create_parent(path)?;
    std::fs::write(path, b"").map_err(|error| format!("cannot create {}: {error}", path.display()))
}

/// Delete a file or directory, as `options` says.
fn delete_path(path: &Path, options: DeleteOptions) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return if options.ignore_if_not_exists {
                Ok(())
            } else {
                Err(format!("{} does not exist", path.display()))
            };
        },
        Err(error) => return Err(format!("cannot delete {}: {error}", path.display())),
    };
    let removed = if !metadata.is_dir() {
        std::fs::remove_file(path)
    } else if options.recursive {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_dir(path)
    };
    removed.map_err(|error| format!("cannot delete {}: {error}", path.display()))
}
