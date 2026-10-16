//! Crash-recovery **swap files** for unsaved buffers.
//!
//! When a document has been dirty for longer than the configured interval — or a save
//! fails — the session writes the edit buffer to a self-describing swap file under the
//! user's data directory. Each swap records the original path and a fingerprint of the
//! file it would overwrite, so on the next launch the session can offer to recover any
//! swap left behind by a crash and warn when the underlying file changed meanwhile.
//!
//! A swap is a single-line JSON header ([`SwapMeta`]) followed by a newline and the raw
//! UTF-8 buffer contents. Swaps are removed on a successful save, on close, and when
//! the user declines to save on exit.
//!
//! The same directory holds **persisted undo history** (`files.persistentUndo`): one
//! JSON file per document, keyed by path alone so the next session finds it. Its
//! history carries a fingerprint of the text it was taken at, so it is restored only
//! onto a file whose content still matches.

use std::borrow::Cow;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use karet_text::HistorySnapshot;
use serde::Deserialize;
use serde::Serialize;

/// The on-disk swap-file extension.
const SWAP_EXT: &str = "karet-swap";

/// The swap-format version, bumped when [`SwapMeta`] changes incompatibly. Swaps with
/// a different schema are ignored on scan.
const SCHEMA_VERSION: u32 = 1;

/// The persisted-undo file extension (written beside the swaps; [`scan`] skips it).
const UNDO_EXT: &str = "karet-undo";

/// The persisted-undo format version; a file of another version is discarded.
const UNDO_SCHEMA_VERSION: u32 = 1;

/// The largest persisted-undo file written. The step cap bounds how many steps
/// a history holds, not how big each is (a whole-file reformat stores the file
/// twice), so a history past this is not persisted at all.
const MAX_UNDO_BYTES: usize = 16 * 1024 * 1024;

/// Metadata describing one swap: enough to identify the file it backs up, detect that
/// the original changed underneath, and present a recovery prompt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapMeta {
    /// Swap-format version (see [`SCHEMA_VERSION`]).
    pub schema: u32,
    /// The absolute path of the document being backed up.
    pub original: PathBuf,
    /// Fingerprint of the original file's on-disk bytes when the swap was written
    /// (`None` for a never-saved buffer). Compared against the current file to detect
    /// an external change (see [`SwapRecord::conflicts_with_disk`]).
    pub orig_hash: Option<u64>,
    /// Size of the original file's on-disk bytes when the swap was written.
    pub orig_size: Option<u64>,
    /// The editor process (session) that wrote the swap.
    pub session_id: u64,
    /// The buffer version captured in this swap.
    pub doc_version: u64,
    /// Wall-clock time the swap was last written (milliseconds since the Unix epoch).
    pub updated_unix_ms: u128,
}

/// A swap recovered from disk: its metadata, buffer contents, and swap-file path.
#[derive(Clone, Debug)]
pub struct SwapRecord {
    /// The swap's metadata.
    pub meta: SwapMeta,
    /// The unsaved buffer contents.
    pub content: String,
    /// The swap file this record was read from (pass to [`discard`] to remove it).
    pub swap_path: PathBuf,
}

impl SwapRecord {
    /// Whether the original file on disk has changed since the swap was written — its
    /// current content fingerprint differs from the recorded one. A conflict means
    /// recovering the swap would discard newer on-disk content, so the caller should
    /// warn. A missing original (or a never-saved buffer) is not a conflict.
    #[must_use]
    pub fn conflicts_with_disk(&self) -> bool {
        let Some(recorded) = self.meta.orig_hash else {
            return false;
        };
        match std::fs::read(&self.meta.original) {
            Ok(bytes) => karet_text::content_fingerprint(&bytes) != recorded,
            Err(_) => false, // the original is gone; recovery is the whole point
        }
    }
}

/// The per-session swap store, rooted at the user's data directory.
pub struct SwapStore {
    dir: PathBuf,
    session_id: u64,
}

/// The default swap directory under the platform data directory (`…/karet/swaps`),
/// or `None` when no data directory can be determined. The application passes this to
/// the session; tests leave it unset so they never touch the real user directory.
#[must_use]
pub fn default_swap_dir() -> Option<PathBuf> {
    Some(
        directories::ProjectDirs::from("", "getkono", "karet")?
            .data_local_dir()
            .join("swaps"),
    )
}

impl SwapStore {
    /// Open the swap store for `session_id` under the platform data directory
    /// (`…/karet/swaps`). Returns `None` when no data directory can be determined.
    #[must_use]
    pub fn new(session_id: u64) -> Option<Self> {
        Some(Self {
            dir: default_swap_dir()?,
            session_id,
        })
    }

    /// Open a swap store rooted at an explicit directory (for tests).
    #[must_use]
    pub fn with_dir(dir: PathBuf, session_id: u64) -> Self {
        Self { dir, session_id }
    }

    /// The directory swaps are written to.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// This session's swap-file path for `original` (stable per path + session).
    fn swap_path(&self, original: &Path) -> PathBuf {
        let key = karet_text::content_fingerprint(original.to_string_lossy().as_bytes());
        self.dir
            .join(format!("{key:016x}-{:016x}.{SWAP_EXT}", self.session_id))
    }

    /// Write (or overwrite) the swap for `original` with the current buffer `content`.
    ///
    /// # Errors
    /// Returns an [`std::io::Error`] if the swap directory or file cannot be written.
    pub fn write(
        &self,
        original: &Path,
        content: &str,
        orig_hash: Option<u64>,
        orig_size: Option<u64>,
        doc_version: u64,
    ) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(&self.dir)?;
        let meta = SwapMeta {
            schema: SCHEMA_VERSION,
            original: original.to_path_buf(),
            orig_hash,
            orig_size,
            session_id: self.session_id,
            doc_version,
            updated_unix_ms: now_unix_ms(),
        };
        let header = serde_json::to_string(&meta).unwrap_or_default();
        let mut bytes = header.into_bytes();
        bytes.push(b'\n');
        bytes.extend_from_slice(content.as_bytes());
        let path = self.swap_path(original);
        write_atomic(&self.dir, &path, &bytes)?;
        Ok(path)
    }

    /// Remove this session's swap for `original` (a no-op if none exists).
    pub fn remove(&self, original: &Path) {
        let _ = std::fs::remove_file(self.swap_path(original));
    }

    /// The persisted-undo path for `original`. Unlike a swap it carries no
    /// session id: it exists to be found by the *next* session.
    fn undo_path(&self, original: &Path) -> PathBuf {
        let key = karet_text::content_fingerprint(original.to_string_lossy().as_bytes());
        self.dir.join(format!("{key:016x}.{UNDO_EXT}"))
    }

    /// Persist `history` as the undo history of `original`, replacing any
    /// earlier one. An empty history removes the file instead: there is
    /// nothing to restore, and a stale one must not outlive it.
    ///
    /// # Errors
    /// Returns an [`std::io::Error`] if the directory or file cannot be written.
    pub fn write_undo(&self, original: &Path, history: &HistorySnapshot) -> std::io::Result<()> {
        if history.is_empty() {
            self.remove_undo(original);
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir)?;
        let record = UndoRecord {
            schema: UNDO_SCHEMA_VERSION,
            original: Cow::Borrowed(original),
            history: Cow::Borrowed(history),
        };
        // Serialized straight into the temp file through a writer that stops at
        // the bound, so an oversized history costs at most `MAX_UNDO_BYTES` of
        // serialization and never a whole in-memory copy.
        let tmp = tempfile::Builder::new()
            .prefix(".karet-swap-")
            .tempfile_in(&self.dir)?;
        let mut out = BoundedWriter {
            inner: std::io::BufWriter::new(tmp.as_file()),
            remaining: MAX_UNDO_BYTES,
            exceeded: false,
        };
        let written = serde_json::to_writer(&mut out, &record)
            .map_err(std::io::Error::other)
            .and_then(|()| out.flush());
        if out.exceeded {
            // Too big to be worth rewriting on every save; an older, smaller
            // history no longer describes the file, so it goes too. Dropping
            // `tmp` deletes the partial write.
            self.remove_undo(original);
            return Ok(());
        }
        written?;
        drop(out);
        tmp.persist(self.undo_path(original))
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(())
    }

    /// The persisted undo history of `original`, provided one exists, parses,
    /// and was written for this very path (the file name is a hash of it). A
    /// file that fails any of those is removed, so it is not read again.
    ///
    /// Whether the history still fits the file's text is a further question the
    /// caller asks of [`karet_text::TextBuffer::restore_history`], which checks
    /// the content fingerprint the snapshot carries.
    #[must_use]
    pub fn read_undo(&self, original: &Path) -> Option<HistorySnapshot> {
        let path = self.undo_path(original);
        let bytes = std::fs::read(&path).ok()?;
        match serde_json::from_slice::<UndoRecord>(&bytes) {
            Ok(record) if record.schema == UNDO_SCHEMA_VERSION && record.original == original => {
                Some(record.history.into_owned())
            },
            _ => {
                discard(&path);
                None
            },
        }
    }

    /// Remove the persisted undo history of `original` (a no-op if none exists).
    pub fn remove_undo(&self, original: &Path) {
        discard(&self.undo_path(original));
    }
}

/// The on-disk persisted-undo file: which document it belongs to, and its
/// history (which carries the fingerprint of the text it applies to). Writing
/// borrows both; reading owns them.
#[derive(Serialize, Deserialize)]
struct UndoRecord<'a> {
    schema: u32,
    original: Cow<'a, Path>,
    history: Cow<'a, HistorySnapshot>,
}

/// A writer that refuses any write taking it past `remaining` bytes, setting
/// `exceeded` so the caller can tell the bound from a real I/O failure.
struct BoundedWriter<W> {
    inner: W,
    remaining: usize,
    exceeded: bool,
}

impl<W: Write> Write for BoundedWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.len() > self.remaining {
            self.exceeded = true;
            return Err(std::io::Error::other("undo history over the size bound"));
        }
        let n = self.inner.write(buf)?;
        self.remaining -= n;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Scan `dir` for all recoverable swaps (from any session). Malformed or
/// wrong-schema files are skipped.
#[must_use]
pub fn scan(dir: &Path) -> Vec<SwapRecord> {
    let mut records = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return records;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some(SWAP_EXT) {
            continue;
        }
        if let Some(record) = read_swap(&path) {
            records.push(record);
        }
    }
    records
}

/// Delete a swap file (used after the user recovers or discards it).
pub fn discard(swap_path: &Path) {
    let _ = std::fs::remove_file(swap_path);
}

/// Parse one swap file into a [`SwapRecord`], or `None` if it is malformed or of a
/// different schema version.
fn read_swap(path: &Path) -> Option<SwapRecord> {
    let bytes = std::fs::read(path).ok()?;
    let split = bytes.iter().position(|&b| b == b'\n')?;
    let (header, rest) = bytes.split_at(split);
    let meta: SwapMeta = serde_json::from_slice(header).ok()?;
    if meta.schema != SCHEMA_VERSION {
        return None;
    }
    // `rest` starts at the newline; skip it.
    let content = String::from_utf8(rest.get(1..).unwrap_or(&[]).to_vec()).ok()?;
    Some(SwapRecord {
        meta,
        content,
        swap_path: path.to_path_buf(),
    })
}

/// Milliseconds since the Unix epoch (0 if the clock is before the epoch).
fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Write `bytes` to `path` via a temp file in `dir` + rename, so a crash never leaves
/// a half-written swap.
fn write_atomic(dir: &Path, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = tempfile::Builder::new()
        .prefix(".karet-swap-")
        .tempfile_in(dir)?;
    std::fs::write(tmp.path(), bytes)?;
    tmp.persist(path)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Option<(tempfile::TempDir, SwapStore)> {
        let dir = tempfile::tempdir().ok()?;
        let store = SwapStore::with_dir(dir.path().to_path_buf(), 42);
        Some((dir, store))
    }

    #[test]
    fn write_scan_round_trip() {
        let Some((_dir, store)) = store() else {
            return;
        };
        let original = PathBuf::from("/work/main.rs");
        if store
            .write(&original, "fn main() {}\n", Some(7), Some(12), 3)
            .is_err()
        {
            return;
        }
        let found = scan(store.dir());
        assert_eq!(found.len(), 1);
        let rec = &found[0];
        assert_eq!(rec.meta.original, original);
        assert_eq!(rec.meta.orig_hash, Some(7));
        assert_eq!(rec.meta.doc_version, 3);
        assert_eq!(rec.content, "fn main() {}\n");
    }

    #[test]
    fn remove_deletes_the_swap() {
        let Some((_dir, store)) = store() else {
            return;
        };
        let original = PathBuf::from("/work/a.txt");
        if store.write(&original, "x", None, None, 1).is_err() {
            return;
        }
        assert_eq!(scan(store.dir()).len(), 1);
        store.remove(&original);
        assert!(scan(store.dir()).is_empty());
    }

    #[test]
    fn overwriting_updates_in_place() {
        let Some((_dir, store)) = store() else {
            return;
        };
        let original = PathBuf::from("/work/a.txt");
        let _ = store.write(&original, "first", None, None, 1);
        let _ = store.write(&original, "second", None, None, 2);
        let found = scan(store.dir());
        assert_eq!(found.len(), 1, "same path+session reuses one swap file");
        assert_eq!(found[0].content, "second");
        assert_eq!(found[0].meta.doc_version, 2);
    }

    #[test]
    fn conflict_detection_matches_the_content_fingerprint() {
        let Some((tmp, store)) = store() else {
            return;
        };
        let original = tmp.path().join("f.txt");
        if std::fs::write(&original, "on disk\n").is_err() {
            return;
        }
        let hash = karet_text::content_fingerprint(b"on disk\n");
        // Same fingerprint recorded → no conflict.
        if store
            .write(&original, "edited\n", Some(hash), Some(8), 1)
            .is_err()
        {
            return;
        }
        let found = scan(store.dir());
        assert_eq!(found.len(), 1);
        assert!(!found[0].conflicts_with_disk());

        // The file changes underneath → conflict.
        if std::fs::write(&original, "changed by someone else\n").is_err() {
            return;
        }
        assert!(found[0].conflicts_with_disk());
    }

    #[test]
    fn malformed_and_wrong_schema_swaps_are_skipped() {
        let Some((tmp, store)) = store() else {
            return;
        };
        // Not JSON at all.
        let _ = std::fs::write(tmp.path().join("junk.karet-swap"), b"not a swap");
        // Valid JSON header but a future schema version.
        let bad = "{\"schema\":999,\"original\":\"/x\",\"orig_hash\":null,\"orig_size\":null,\"session_id\":1,\"doc_version\":1,\"updated_unix_ms\":0}\nbody";
        let _ = std::fs::write(tmp.path().join("future.karet-swap"), bad);
        // A real one still parses.
        let _ = store.write(&PathBuf::from("/work/ok.txt"), "ok", None, None, 1);
        assert_eq!(scan(store.dir()).len(), 1);
    }

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A non-empty history taken from a buffer now holding `"ab"`.
    fn edited_history() -> Result<HistorySnapshot, karet_text::TextError> {
        let mut buffer = karet_text::TextBuffer::from_text("a");
        let change = karet_core::Change::new(
            0,
            vec![karet_core::TextEdit {
                range: karet_core::Range {
                    start: karet_core::LineCol::new(0, 1),
                    end: karet_core::LineCol::new(0, 1),
                },
                new_text: "b".to_string(),
            }],
        );
        buffer.apply_simple(&change)?;
        Ok(buffer.history_snapshot())
    }

    #[test]
    fn undo_history_round_trips_across_sessions() -> TestResult {
        let dir = tempfile::tempdir()?;
        let original = PathBuf::from("/work/main.rs");
        let history = edited_history()?;
        SwapStore::with_dir(dir.path().to_path_buf(), 1).write_undo(&original, &history)?;
        // A later session (another id) finds it: the file is keyed by path alone.
        let next = SwapStore::with_dir(dir.path().to_path_buf(), 2);
        assert_eq!(next.read_undo(&original), Some(history));
        assert!(
            scan(next.dir()).is_empty(),
            "an undo file is never offered as a swap"
        );
        Ok(())
    }

    #[test]
    fn an_empty_history_removes_the_undo_file() -> TestResult {
        let dir = tempfile::tempdir()?;
        let store = SwapStore::with_dir(dir.path().to_path_buf(), 1);
        let original = PathBuf::from("/work/a.txt");
        store.write_undo(&original, &edited_history()?)?;
        store.write_undo(&original, &karet_text::TextBuffer::new().history_snapshot())?;
        assert_eq!(store.read_undo(&original), None);
        assert_eq!(std::fs::read_dir(dir.path())?.count(), 0);
        Ok(())
    }

    #[test]
    fn an_oversized_history_is_not_persisted_and_drops_the_old_one() -> TestResult {
        let dir = tempfile::tempdir()?;
        let store = SwapStore::with_dir(dir.path().to_path_buf(), 1);
        let original = PathBuf::from("/work/big.txt");
        store.write_undo(&original, &edited_history()?)?;

        let mut buffer = karet_text::TextBuffer::new();
        let big = karet_core::Change::new(
            0,
            vec![karet_core::TextEdit {
                range: karet_core::Range::default(),
                new_text: "x".repeat(MAX_UNDO_BYTES + 1),
            }],
        );
        buffer.apply_simple(&big)?;
        store.write_undo(&original, &buffer.history_snapshot())?;
        assert_eq!(store.read_undo(&original), None);
        assert_eq!(std::fs::read_dir(dir.path())?.count(), 0);
        Ok(())
    }

    #[test]
    fn the_bounded_writer_stops_at_its_bound() {
        let mut sink = Vec::new();
        let mut out = BoundedWriter {
            inner: &mut sink,
            remaining: 4,
            exceeded: false,
        };
        assert!(out.write_all(b"abcd").is_ok());
        assert!(!out.exceeded, "a write that fits exactly is not over");
        assert!(out.write_all(b"e").is_err());
        assert!(out.exceeded);
        assert_eq!(sink, b"abcd", "nothing past the bound reaches the sink");
    }

    #[test]
    fn remove_undo_deletes_it() -> TestResult {
        let dir = tempfile::tempdir()?;
        let store = SwapStore::with_dir(dir.path().to_path_buf(), 1);
        let original = PathBuf::from("/work/a.txt");
        store.write_undo(&original, &edited_history()?)?;
        store.remove_undo(&original);
        assert_eq!(store.read_undo(&original), None);
        Ok(())
    }

    #[test]
    fn unreadable_or_foreign_undo_files_are_discarded() -> TestResult {
        let dir = tempfile::tempdir()?;
        let store = SwapStore::with_dir(dir.path().to_path_buf(), 1);
        let original = PathBuf::from("/work/a.txt");
        let path = store.undo_path(&original);

        std::fs::write(&path, b"not json")?;
        assert_eq!(store.read_undo(&original), None);
        assert!(!path.exists(), "a corrupt file is removed, not re-read");

        // Well-formed, but recorded for a different path under this name.
        let foreign = UndoRecord {
            schema: UNDO_SCHEMA_VERSION,
            original: Cow::Borrowed(Path::new("/elsewhere.txt")),
            history: Cow::Owned(edited_history()?),
        };
        std::fs::write(&path, serde_json::to_vec(&foreign)?)?;
        assert_eq!(store.read_undo(&original), None);
        assert!(!path.exists());

        let future = UndoRecord {
            schema: UNDO_SCHEMA_VERSION + 1,
            original: Cow::Borrowed(original.as_path()),
            history: Cow::Owned(edited_history()?),
        };
        std::fs::write(&path, serde_json::to_vec(&future)?)?;
        assert_eq!(store.read_undo(&original), None);
        Ok(())
    }
}
