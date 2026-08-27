//! The seam index on disk, so a cold start is not a cold build.
//!
//! Reading a repository's seams means parsing every file in it. Nearly all of those files
//! are the same ones that were parsed last time, and they still say the same thing — so
//! what they said is kept, and a file that has not changed is replayed rather than read.
//!
//! # What makes an entry usable
//!
//! Per file, modification time and length together. Not a content hash: hashing means
//! reading, and reading is most of what this exists to avoid. The trade is one-sided and
//! deliberate — a file touched without being changed is parsed needlessly, which costs
//! time; a file that changed is never mistaken for one that did not, because writing to it
//! moves its mtime.
//!
//! Per cache, a header. The schema version, the engine's version, and the set of language
//! features this build was compiled with. Any mismatch discards the whole file rather than
//! trying to salvage part of it: a grammar change can alter what *every* file extracts to,
//! and a half-migrated cache is worse than none. The root path is stored too, so a digest
//! collision resolves to a miss rather than to another workspace's tree.
//!
//! # Why it is written whole
//!
//! One file per workspace, replaced atomically. Partial writes are the failure mode a
//! cache cannot survive, and the alternative — a file per package — buys resumability that
//! a sync fast enough to restart does not need.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use karet_seam::FileContribution;
use karet_seam::FileStamp;
use rayon::prelude::*;

/// Bumped whenever the stored shape changes in a way an older reader would misread.
const SCHEMA: u32 = 1;

/// The language mappings this build can read, which decide what a file extracts to.
///
/// A cache written with Swift compiled in describes files this build would skip, and one
/// written without Kotlin is missing nodes this build would produce. Either way the answer
/// is to rebuild, so the set is part of the header. Grammar ids are stable by contract —
/// the parse host never renumbers a shipped one — which is what makes them storable.
///
/// This does not catch a *grammar version* bump, which can change what a file extracts to
/// without changing which languages are mapped. The engine version below covers that
/// across releases, and a forced re-sync covers it within one.
fn languages() -> Vec<u16> {
    let mut out: Vec<u16> = karet_seam::lang::registered()
        .iter()
        .map(|language| language.language().0)
        .collect();
    out.sort_unstable();
    out
}

/// A stable digest of `root`, naming its cache file.
///
/// Deliberately not `DefaultHasher`: the standard library does not promise its output
/// stays the same across releases, and this names a file that has to be found again after
/// a toolchain upgrade. FNV-1a is fixed by its definition.
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

/// The per-user directory seam caches belong in, for a host that wants the default.
///
/// Kept beside the other per-workspace caches rather than in the data directory: losing
/// this costs a rebuild, and nothing else.
#[must_use]
pub fn default_cache_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "karet").map(|dirs| dirs.cache_dir().join("seam"))
}

/// Where this workspace's cache lives under `dir`.
#[must_use]
fn path_for(dir: &Path, root: &Path) -> PathBuf {
    dir.join(format!("{:016x}.cbor", workspace_key(root)))
}

/// The header a stored cache carries, and the whole of what is checked before trusting it.
#[derive(serde::Serialize, serde::Deserialize)]
struct Header {
    schema: u32,
    engine: String,
    languages: Vec<u16>,
    root: PathBuf,
}

impl Header {
    fn current(root: &Path) -> Self {
        Self {
            schema: SCHEMA,
            engine: env!("CARGO_PKG_VERSION").to_owned(),
            languages: languages(),
            root: root.to_path_buf(),
        }
    }

    /// Whether a stored header describes something this build can read.
    fn usable_for(&self, root: &Path) -> bool {
        self.schema == SCHEMA
            && self.engine == env!("CARGO_PKG_VERSION")
            && self.languages == languages()
            && self.root == root
    }
}

/// One independently decodable slice of the stored index.
///
/// A newtype rather than a bare `Vec<u8>` so it encodes as a CBOR *byte string*: the
/// default for a byte vector is an array of integers, which would roughly double the file.
struct Chunk(Vec<u8>);

impl serde::Serialize for Chunk {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for Chunk {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Bytes;
        impl serde::de::Visitor<'_> for Bytes {
            type Value = Chunk;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a byte string")
            }

            fn visit_bytes<E: serde::de::Error>(self, value: &[u8]) -> Result<Chunk, E> {
                Ok(Chunk(value.to_vec()))
            }

            fn visit_byte_buf<E: serde::de::Error>(self, value: Vec<u8>) -> Result<Chunk, E> {
                Ok(Chunk(value))
            }
        }
        deserializer.deserialize_byte_buf(Bytes)
    }
}

/// A whole workspace's stored contributions, in independently decodable pieces.
///
/// Split because decoding is otherwise the slowest part of a warm start — and the one
/// part that would still be serial while the cold path it replaces runs on every core.
/// The outer file decodes to nothing but byte strings, and the pieces decode in parallel.
#[derive(serde::Serialize, serde::Deserialize)]
struct Stored {
    header: Header,
    chunks: Vec<Chunk>,
}

/// How many pieces to split a stored index into.
///
/// Enough to keep a many-core machine busy without making the pieces so small that the
/// per-piece overhead shows. A piece that fails to decode costs only its own files, which
/// are re-read; the rest of the cache still stands.
fn chunk_count(files: usize) -> usize {
    files.div_ceil(32).clamp(1, 64)
}

/// What a build may replay instead of parsing.
#[derive(Default)]
pub(crate) struct SeamCache {
    entries: HashMap<PathBuf, FileContribution>,
}

impl SeamCache {
    /// An empty cache — what a cold build, or a forced re-sync, reads from.
    #[must_use]
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    /// Read the cache for `root` from `dir`, or an empty one if there is nothing usable.
    ///
    /// Every failure is a miss, never an error: a cache is an optimization, and a
    /// corrupt, truncated or foreign one costs a rebuild and nothing else. A host that
    /// configured no directory — which is every test — reads nothing and writes nothing.
    #[must_use]
    pub(crate) fn load(dir: Option<&Path>, root: &Path) -> Self {
        let Some(dir) = dir else {
            return Self::empty();
        };
        let path = path_for(dir, root);
        let Ok(bytes) = std::fs::read(&path) else {
            return Self::empty();
        };
        let Ok(stored) = ciborium::from_reader::<Stored, _>(bytes.as_slice()) else {
            return Self::empty();
        };
        if !stored.header.usable_for(root) {
            return Self::empty();
        }
        Self {
            entries: stored
                .chunks
                .par_iter()
                .flat_map_iter(|chunk| {
                    ciborium::from_reader::<Vec<FileContribution>, _>(chunk.0.as_slice())
                        .unwrap_or_default()
                })
                .map(|contribution| (contribution.file.clone(), contribution))
                .collect(),
        }
    }

    /// The stored contribution for `file`, if it still describes what is on disk.
    #[must_use]
    pub(crate) fn get(&self, file: &Path, stamp: FileStamp) -> Option<FileContribution> {
        let held = self.entries.get(file)?;
        held.matches(stamp).then(|| held.clone())
    }

    /// How many files the cache holds.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Write `contributions` as the cache for `root`, replacing whatever was there.
    ///
    /// Best-effort throughout: a cache that cannot be written is a slower next start, not
    /// a failure worth telling anyone about.
    pub(crate) fn save(dir: Option<&Path>, root: &Path, contributions: Vec<FileContribution>) {
        let Some(dir) = dir else {
            return;
        };
        let path = path_for(dir, root);
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        let per_chunk = contributions
            .len()
            .div_ceil(chunk_count(contributions.len()));
        let chunks: Vec<Chunk> = contributions
            .par_chunks(per_chunk.max(1))
            .map(|slice| {
                let mut encoded = Vec::new();
                if ciborium::into_writer(&slice.to_vec(), &mut encoded).is_err() {
                    encoded.clear();
                }
                Chunk(encoded)
            })
            .collect();
        let stored = Stored {
            header: Header::current(root),
            chunks,
        };
        let mut bytes = Vec::new();
        if ciborium::into_writer(&stored, &mut bytes).is_err() {
            return;
        }
        write_atomic(dir, &path, &bytes);
    }

    /// Delete the cache for `root`. This is what a forced re-sync does first.
    pub(crate) fn remove(dir: Option<&Path>, root: &Path) {
        if let Some(dir) = dir {
            let _ = std::fs::remove_file(path_for(dir, root));
        }
    }
}

/// Write via a temp file in the same directory plus a rename, so an interrupted write
/// never leaves a half-decoded cache behind for the next start to trip over.
fn write_atomic(dir: &Path, path: &Path, bytes: &[u8]) {
    let Ok(temp) = tempfile::Builder::new()
        .prefix(".karet-seam-")
        .tempfile_in(dir)
    else {
        return;
    };
    if std::fs::write(temp.path(), bytes).is_err() {
        return;
    }
    let _ = temp.persist(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn contribution(file: &Path) -> FileContribution {
        FileContribution {
            file: file.to_path_buf(),
            stamp: FileStamp {
                modified_nanos: 42,
                len: 7,
            },
            owner: "pkg".parse().unwrap_or_default(),
            depth: 0,
            crate_root: true,
            nodes: Vec::new(),
            external_modules: Vec::new(),
            ownership: Vec::new(),
            unresolved: Vec::new(),
        }
    }

    #[test]
    fn a_stored_cache_round_trips() -> TestResult {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        let file = root.join("src").join("lib.rs");
        SeamCache::save(Some(root), root, vec![contribution(&file)]);
        let read = SeamCache::load(Some(root), root);
        assert_eq!(read.len(), 1);
        assert!(
            read.get(
                &file,
                FileStamp {
                    modified_nanos: 42,
                    len: 7
                }
            )
            .is_some()
        );
        Ok(())
    }

    #[test]
    fn a_header_from_a_different_build_is_refused() -> TestResult {
        let dir = tempfile::tempdir()?;
        let root = dir.path();

        let mut wrong_schema = Header::current(root);
        wrong_schema.schema = SCHEMA.wrapping_add(1);
        assert!(!wrong_schema.usable_for(root));

        let mut wrong_engine = Header::current(root);
        wrong_engine.engine = "0.0.0-not-this".to_owned();
        assert!(!wrong_engine.usable_for(root));

        // A grammar this build lacks would have extracted files this build skips.
        let mut wrong_languages = Header::current(root);
        wrong_languages.languages.push(u16::MAX);
        assert!(!wrong_languages.usable_for(root));

        // A digest collision must resolve to a miss, not to another workspace's tree.
        let elsewhere = tempfile::tempdir()?;
        assert!(!Header::current(root).usable_for(elsewhere.path()));
        Ok(())
    }

    #[test]
    fn a_corrupt_file_is_a_miss_rather_than_a_failure() -> TestResult {
        let store = tempfile::tempdir()?;
        let workspace = tempfile::tempdir()?;
        let root = workspace.path();
        std::fs::write(path_for(store.path(), root), b"this is not CBOR at all")?;

        assert_eq!(SeamCache::load(Some(store.path()), root).len(), 0);
        Ok(())
    }

    #[test]
    fn a_host_with_no_directory_neither_reads_nor_writes() -> TestResult {
        // The test and headless default: nothing is stored, and no user directory is
        // touched to discover that.
        let workspace = tempfile::tempdir()?;
        let root = workspace.path();
        SeamCache::save(None, root, vec![contribution(&root.join("a.rs"))]);
        assert_eq!(SeamCache::load(None, root).len(), 0);
        SeamCache::remove(None, root);
        Ok(())
    }

    #[test]
    fn a_large_index_is_split_so_it_can_decode_in_parallel() -> TestResult {
        let store = tempfile::tempdir()?;
        let workspace = tempfile::tempdir()?;
        let root = workspace.path();
        let files: Vec<FileContribution> = (0..200)
            .map(|n| contribution(&root.join(format!("src/f{n}.rs"))))
            .collect();
        SeamCache::save(Some(store.path()), root, files);

        let bytes = std::fs::read(path_for(store.path(), root))?;
        let stored: Stored = ciborium::from_reader(bytes.as_slice())?;
        assert!(
            stored.chunks.len() > 1,
            "one piece cannot decode in parallel"
        );
        assert_eq!(SeamCache::load(Some(store.path()), root).len(), 200);
        Ok(())
    }

    #[test]
    fn a_damaged_piece_costs_only_the_files_it_held() -> TestResult {
        // Splitting buys parallelism, and this: the rest of the cache still stands, and
        // the files behind the bad piece are simply read again.
        let store = tempfile::tempdir()?;
        let workspace = tempfile::tempdir()?;
        let root = workspace.path();
        let files: Vec<FileContribution> = (0..200)
            .map(|n| contribution(&root.join(format!("src/f{n}.rs"))))
            .collect();
        SeamCache::save(Some(store.path()), root, files);

        let path = path_for(store.path(), root);
        let bytes = std::fs::read(&path)?;
        let mut stored: Stored = ciborium::from_reader(bytes.as_slice())?;
        let held = stored.chunks.len();
        if let Some(chunk) = stored.chunks.first_mut() {
            chunk.0 = b"not CBOR".to_vec();
        }
        let mut rewritten = Vec::new();
        ciborium::into_writer(&stored, &mut rewritten)?;
        std::fs::write(&path, &rewritten)?;

        let loaded = SeamCache::load(Some(store.path()), root).len();
        assert!(loaded > 0, "one bad piece emptied the whole cache");
        assert!(loaded < 200, "the bad piece was somehow still read");
        assert_eq!(held, chunk_count(200));
        Ok(())
    }

    #[test]
    fn an_entry_is_served_only_while_its_stamp_holds() {
        let file = PathBuf::from("/somewhere/src/lib.rs");
        let cache = SeamCache {
            entries: [(file.clone(), contribution(&file))].into_iter().collect(),
        };
        let matching = FileStamp {
            modified_nanos: 42,
            len: 7,
        };
        assert!(cache.get(&file, matching).is_some());

        let rewritten = FileStamp {
            modified_nanos: 43,
            len: 7,
        };
        assert!(cache.get(&file, rewritten).is_none());

        let resized = FileStamp {
            modified_nanos: 42,
            len: 8,
        };
        assert!(cache.get(&file, resized).is_none());
        assert!(cache.get(Path::new("/absent.rs"), matching).is_none());
    }

    #[test]
    fn saving_then_loading_returns_what_was_stored() -> TestResult {
        let store = tempfile::tempdir()?;
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        let file = root.join("src").join("lib.rs");
        SeamCache::save(Some(store.path()), root, vec![contribution(&file)]);

        let loaded = SeamCache::load(Some(store.path()), root);
        assert_eq!(loaded.len(), 1);
        assert!(
            loaded
                .get(
                    &file,
                    FileStamp {
                        modified_nanos: 42,
                        len: 7
                    }
                )
                .is_some()
        );

        SeamCache::remove(Some(store.path()), root);
        assert_eq!(SeamCache::load(Some(store.path()), root).len(), 0);
        Ok(())
    }
}
