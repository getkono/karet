//! A language server's folding ranges on each document: adopting the primary
//! server's answer for the version it was asked at, carrying it across edits
//! by line, dropping it when its server goes, and preferring it over the
//! grammar's (or indentation's) fold regions on every snapshot.
//!
//! The grammar's folds stay computed underneath ([`SyntaxState::folds`]), so a
//! document whose server stops answering -- it is retired, it stays down, the
//! file moves to another language -- falls back to them at once.

use karet_core::FoldRegion;

use super::*;
use crate::lsp::SlotKey;

/// One document's server-provided fold regions.
pub(super) struct ServerFolds {
    /// The slot whose server answered; its retirement takes the regions.
    server: SlotKey,
    /// The buffer version `folds` describe.
    version: u64,
    /// The regions, normalized and inside the text.
    folds: Arc<FoldRegions>,
}

impl Document {
    /// Keep the server's regions aligned with the text across `edits`, until
    /// the server answers for the new version. A version change with no edits
    /// to map through -- a reload replacing the text -- drops them instead.
    pub(super) fn translate_server_folds(&mut self, edits: Option<&[AppliedEdit]>) {
        let version = self.buffer.version();
        let Some(layer) = self.server_folds.as_mut() else {
            return;
        };
        match edits {
            Some(edits) => {
                for edit in edits {
                    layer.folds = Arc::new(layer.folds.translate(edit));
                }
                layer.version = version;
            },
            None if layer.version != version => self.server_folds = None,
            None => {},
        }
    }

    /// The fold regions a snapshot carries: the server's while it holds some,
    /// else the grammar's.
    pub(super) fn display_folds(&self) -> Arc<FoldRegions> {
        match &self.server_folds {
            Some(layer) => Arc::clone(&layer.folds),
            None => Arc::clone(self.syntax.folds()),
        }
    }
}

impl Session {
    /// Adopt an [`LspUpdate::FoldingRanges`]: the primary server's regions for
    /// a document at an LSP version, replacing the grammar's on its snapshots.
    ///
    /// Dropped when the document has moved past that version (the next one is
    /// asked for as soon as the server has it). An answer with no region in
    /// the text gives the grammar's folds back rather than folding nothing: a
    /// server still loading a project answers empty.
    pub(super) fn adopt_server_folds(&mut self, update: LspUpdate) {
        let LspUpdate::FoldingRanges {
            server,
            path,
            version,
            folds,
            ..
        } = update
        else {
            return;
        };
        self.adopt_folds(server, &path, version, &folds);
    }

    /// [`Self::adopt_server_folds`], unpacked.
    fn adopt_folds(&mut self, server: SlotKey, path: &Path, version: i32, folds: &FoldRegions) {
        let Some(&doc_id) = self.store.by_path.get(path) else {
            return;
        };
        let Some(document) = self.store.docs.get_mut(&doc_id) else {
            return;
        };
        if crate::lsp::version_i32(document.buffer.version()) != version {
            return;
        }
        let folds = inside(folds, document.buffer.line_count());
        let had = document.server_folds.is_some();
        document.server_folds = (!folds.regions().is_empty()).then(|| ServerFolds {
            server,
            version: document.buffer.version(),
            folds: Arc::new(folds),
        });
        if had || document.server_folds.is_some() {
            self.publish(doc_id, None);
        }
    }

    /// Drop every region `server` provided, republishing the documents that
    /// carried some: the slot was retired, or its connection is gone for good.
    pub(super) fn clear_server_folds_of(&mut self, server: &SlotKey) {
        let affected: Vec<DocumentId> = self
            .store
            .docs
            .iter_mut()
            .filter(|(_, document)| {
                document
                    .server_folds
                    .as_ref()
                    .is_some_and(|layer| layer.server == *server)
            })
            .map(|(&doc_id, document)| {
                document.server_folds = None;
                doc_id
            })
            .collect();
        for doc_id in affected {
            self.publish(doc_id, None);
        }
    }
}

/// `folds` cut to a text of `lines` lines: a region running past the last
/// line ends on it, and one left hiding nothing goes.
fn inside(folds: &FoldRegions, lines: usize) -> FoldRegions {
    let last = u32::try_from(lines.saturating_sub(1)).unwrap_or(u32::MAX);
    FoldRegions::from_unordered(
        folds
            .regions()
            .iter()
            .map(|region| FoldRegion {
                end: region.end.min(last),
                ..*region
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use karet_core::FoldKind;

    use super::*;
    use crate::api::LanguageServerId;

    const TEXT: &str = "a\n  b\n  c\nd\n  e\n";

    /// A session with `name` (holding `text`) open, and the document's id.
    fn opened(
        dir: &tempfile::TempDir,
        name: &str,
        text: &str,
    ) -> Option<(Session, DocumentId, PathBuf)> {
        let path = dir.path().join(name);
        std::fs::write(&path, text).ok()?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            DocumentCommand::Open {
                path: path.clone(),
                language: None,
            },
        );
        let mut doc = None;
        while let Some((_, event)) = events.try_recv() {
            if let Event::Opened { doc: id, .. } = event {
                doc = Some(id);
            }
        }
        Some((session, doc?, path))
    }

    fn server(root: &str) -> SlotKey {
        SlotKey::new(LanguageServerId::RustAnalyzer, root)
    }

    /// The document's current LSP version, as a fetch would stamp it.
    fn version(session: &Session, doc: DocumentId) -> i32 {
        session.store.docs.get(&doc).map_or(-1, |document| {
            crate::lsp::version_i32(document.buffer.version())
        })
    }

    /// Relay `regions` from `server` for `path` at `version`, as the server
    /// task would.
    fn answer(
        session: &mut Session,
        (server, path): (SlotKey, &Path),
        version: i32,
        regions: Vec<FoldRegion>,
    ) {
        session.adopt_folds(server, path, version, &FoldRegions::from_unordered(regions));
    }

    /// The fold regions `doc`'s next snapshot would carry.
    fn shown(session: &Session, doc: DocumentId) -> Vec<FoldRegion> {
        session
            .store
            .docs
            .get(&doc)
            .map(|document| document.display_folds().regions().to_vec())
            .unwrap_or_default()
    }

    /// The grammar's (here indentation's) folds for `doc`.
    fn grammar(session: &Session, doc: DocumentId) -> Vec<FoldRegion> {
        session
            .store
            .docs
            .get(&doc)
            .map(|document| document.syntax.folds().regions().to_vec())
            .unwrap_or_default()
    }

    /// An answer for the current version replaces the grammar's folds, kinds
    /// and all; one for an older version is ignored. Falsified by adopting
    /// without the version check, which would show the stale `(1, 2)`.
    #[test]
    fn a_current_answer_replaces_grammar_folds_and_a_stale_one_is_ignored()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let (mut session, doc, path) = opened(&dir, "a.txt", TEXT).ok_or("open failed")?;
        assert!(!grammar(&session, doc).is_empty(), "no indentation folds");
        let current = version(&session, doc);
        let region = FoldRegion::new(0, 3).with_kind(FoldKind::Region);
        answer(&mut session, (server("/"), &path), current, vec![region]);
        assert_eq!(shown(&session, doc), [region]);
        answer(
            &mut session,
            (server("/"), &path),
            current - 1,
            vec![FoldRegion::new(1, 2)],
        );
        assert_eq!(shown(&session, doc), [region]);
        Ok(())
    }

    /// An answer with nothing inside the text gives the grammar's folds back,
    /// and a region running past the last line is cut to it. Falsified by
    /// adopting the empty answer as is (the snapshot would fold nothing).
    #[test]
    fn an_empty_answer_falls_back_and_regions_stay_inside_the_text()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let (mut session, doc, path) = opened(&dir, "a.txt", TEXT).ok_or("open failed")?;
        let current = version(&session, doc);
        let lines = u32::try_from(TEXT.lines().count())?;
        answer(
            &mut session,
            (server("/"), &path),
            current,
            vec![
                FoldRegion::new(3, 40),
                FoldRegion::new(lines + 2, lines + 9),
            ],
        );
        // The buffer's last line is the empty one after the final newline.
        assert_eq!(shown(&session, doc), [FoldRegion::new(3, lines)]);
        answer(&mut session, (server("/"), &path), current, Vec::new());
        assert_eq!(shown(&session, doc), grammar(&session, doc));
        Ok(())
    }

    /// An edit carries the server's regions by line until the next answer.
    /// Falsified by not translating (the region would stay at `(3, 4)`).
    #[test]
    fn an_edit_moves_the_regions() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let (mut session, doc, path) = opened(&dir, "a.txt", TEXT).ok_or("open failed")?;
        let current = version(&session, doc);
        answer(
            &mut session,
            (server("/"), &path),
            current,
            vec![FoldRegion::new(3, 4)],
        );
        let change = karet_core::Change::new(
            0,
            vec![karet_core::TextEdit {
                range: karet_core::Range {
                    start: LineCol::new(0, 0),
                    end: LineCol::new(0, 0),
                },
                new_text: "x\ny\n".to_owned(),
            }],
        );
        session.handle(
            RequestId(2),
            DocumentCommand::ApplyChange {
                doc,
                change,
                cause: EditCause::Replace,
            },
        );
        assert_eq!(shown(&session, doc), [FoldRegion::new(5, 6)]);
        Ok(())
    }

    /// A reload replaces the text with no edits to map the regions through,
    /// so they are dropped. Falsified by keeping regions whose version is
    /// stale.
    #[test]
    fn a_reload_drops_the_regions() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let (mut session, doc, path) = opened(&dir, "a.txt", TEXT).ok_or("open failed")?;
        let current = version(&session, doc);
        answer(
            &mut session,
            (server("/"), &path),
            current,
            vec![FoldRegion::new(3, 4)],
        );
        std::fs::write(&path, "z\n  w\n")?;
        session.handle_fs_event(karet_watch::FsEvent {
            kind: karet_watch::FsEventKind::Modified,
            paths: vec![path],
        });
        let reloaded = session
            .document(doc)
            .is_some_and(|view| view.buffer().text() == "z\n  w\n");
        assert!(reloaded, "the clean buffer did not reload");
        assert_eq!(shown(&session, doc), grammar(&session, doc));
        Ok(())
    }

    /// Clearing a server takes only its own regions: the same provider at
    /// another root keeps what it answered. Falsified by clearing every
    /// document's regions.
    #[test]
    fn clearing_a_server_takes_only_its_regions() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let (mut session, doc, path) = opened(&dir, "a.txt", TEXT).ok_or("open failed")?;
        let current = version(&session, doc);
        answer(
            &mut session,
            (server("/a"), &path),
            current,
            vec![FoldRegion::new(0, 3)],
        );
        session.clear_server_folds_of(&server("/b"));
        assert_eq!(shown(&session, doc), [FoldRegion::new(0, 3)]);
        session.clear_server_folds_of(&server("/a"));
        assert_eq!(shown(&session, doc), grammar(&session, doc));
        Ok(())
    }

    /// Renaming the file to another language drops the old server's regions,
    /// though the text and its version did not move. Falsified by leaving
    /// them to `update_syntax`, which keeps them at an unchanged version.
    #[test]
    fn retargeting_to_another_language_drops_the_regions() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let (mut session, doc, path) = opened(&dir, "a.txt", TEXT).ok_or("open failed")?;
        let current = version(&session, doc);
        answer(
            &mut session,
            (server("/"), &path),
            current,
            vec![FoldRegion::new(0, 3)],
        );
        session.handle(
            RequestId(2),
            DocumentCommand::Retarget {
                doc,
                path: dir.path().join("a.md"),
            },
        );
        let held = session
            .store
            .docs
            .get(&doc)
            .is_some_and(|document| document.server_folds.is_some());
        assert!(!held, "the old language's server folds survived the rename");
        Ok(())
    }
}
