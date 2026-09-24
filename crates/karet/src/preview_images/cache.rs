//! The decoded-image cache behind the markdown preview (see the parent module).

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc as std_mpsc;
use std::time::SystemTime;

use karet_fileview::image;
use karet_fileview::image::Image;
use karet_markdown::ImageRef;
use karet_markdown::ImageSizer;
use tokio::sync::mpsc;

use super::PROBE_BYTES;
use crate::app::Pending;
use crate::links::LinkTarget;

/// The largest image file the preview loads — the same guard that keeps any file
/// from opening as an image tab.
const MAX_FILE_BYTES: u64 = karet_filetype::SIZE_GUARD;
/// The most pixels an image may have; a larger one would cost its decode and its
/// memory for a picture at most a few dozen cells across.
const MAX_PIXELS: u64 = 4096 * 4096;
/// The decoded bytes kept before the least-recently-seen images are dropped.
const READY_BUDGET: u64 = 256 * 1024 * 1024;

/// What the preview may paint for an image.
#[derive(Clone, Debug)]
pub(crate) enum Lookup {
    /// Decoded and ready.
    Ready(Arc<Image>),
    /// Still decoding, since this moment.
    Loading(Pending),
    /// Never going to be painted: refused, unreadable, or undecodable.
    Missing,
}

/// A file's identity for caching: a changed file gets a new stamp and reloads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

impl Stamp {
    fn of(meta: &std::fs::Metadata) -> Self {
        Self {
            modified: meta.modified().ok(),
            len: meta.len(),
        }
    }
}

/// A finished decode, travelling back from the worker to the event loop.
#[derive(Debug)]
pub(crate) struct Decoded {
    path: PathBuf,
    stamp: Stamp,
    image: Option<Arc<Image>>,
}

/// A decode for the worker.
struct Job {
    path: PathBuf,
    stamp: Stamp,
}

#[derive(Debug)]
enum Load {
    Queued(Pending),
    Ready(Arc<Image>),
    /// Dropped to stay within [`READY_BUDGET`]; decoded again when next painted.
    Evicted,
    Failed,
}

#[derive(Debug)]
struct Entry {
    stamp: Stamp,
    /// The size the layout reserved, once known.
    dims: Option<(u32, u32)>,
    load: Load,
    /// The [`State::clock`] tick this image was last sized or painted at.
    last_used: u64,
}

#[derive(Debug, Default)]
struct State {
    /// Keyed by canonical path, so two spellings of one file share an entry.
    entries: HashMap<PathBuf, Entry>,
    /// `(document, src)` → the canonical workspace file it names, or `None` when the
    /// preview will not load it.
    resolved: HashMap<(PathBuf, String), Option<PathBuf>>,
    clock: u64,
    ready_bytes: u64,
}

/// The markdown preview's image cache. Interior mutability lets the draw path —
/// which only holds shared borrows of the app — size, queue and look up images.
#[derive(Debug)]
pub(crate) struct PreviewImages {
    state: RefCell<State>,
    /// Bumped whenever an image's reserved size changes, so every preview re-wraps.
    generation: Cell<u64>,
    /// The worker's queue, started on the first decode.
    jobs: RefCell<Option<std_mpsc::Sender<Job>>>,
    results: mpsc::UnboundedSender<Decoded>,
    /// Handed to the event loop once, which wakes on finished decodes.
    receiver: RefCell<Option<mpsc::UnboundedReceiver<Decoded>>>,
    /// The decoded bytes to keep: [`READY_BUDGET`], but for tests.
    budget: u64,
}

impl Default for PreviewImages {
    fn default() -> Self {
        let (results, receiver) = mpsc::unbounded_channel();
        Self {
            state: RefCell::default(),
            generation: Cell::new(0),
            jobs: RefCell::new(None),
            results,
            receiver: RefCell::new(Some(receiver)),
            budget: READY_BUDGET,
        }
    }
}

impl PreviewImages {
    /// The receiver of finished decodes, for the event loop. `None` after the first call.
    pub(crate) fn take_receiver(&self) -> Option<mpsc::UnboundedReceiver<Decoded>> {
        self.receiver.borrow_mut().take()
    }

    /// Changes whenever a reserved image size does; part of every preview's cache key.
    pub(crate) fn generation(&self) -> u64 {
        self.generation.get()
    }

    /// The loads still in flight, for the event loop's reveal-deadline wake.
    pub(crate) fn pendings(&self) -> Vec<Pending> {
        self.state
            .borrow()
            .entries
            .values()
            .filter_map(|entry| match entry.load {
                Load::Queued(pending) => Some(pending),
                _ => None,
            })
            .collect()
    }

    /// An [`ImageSizer`] resolving sources against the document at `source`.
    pub(crate) fn sizer<'a>(&'a self, source: &'a Path, root: &'a Path) -> Sizer<'a> {
        Sizer {
            images: self,
            source,
            root,
        }
    }

    /// What to paint for `src` in the document at `source`. An evicted image is queued
    /// to decode again.
    pub(crate) fn lookup(&self, source: &Path, root: &Path, src: &str) -> Lookup {
        let Some(path) = self.resolve(source, root, src) else {
            return Lookup::Missing;
        };
        let mut state = self.state.borrow_mut();
        state.clock += 1;
        let now = state.clock;
        let Some(entry) = state.entries.get_mut(&path) else {
            return Lookup::Missing;
        };
        entry.last_used = now;
        match &entry.load {
            Load::Ready(image) => Lookup::Ready(Arc::clone(image)),
            Load::Queued(pending) => Lookup::Loading(*pending),
            Load::Failed => Lookup::Missing,
            Load::Evicted => {
                let pending = Pending::start();
                entry.load = Load::Queued(pending);
                let stamp = entry.stamp;
                drop(state);
                self.queue(path, stamp);
                Lookup::Loading(pending)
            },
        }
    }

    /// Take a finished decode. A result for a file that has since changed, or left the
    /// cache, is stale and dropped.
    pub(crate) fn accept(&self, decoded: Decoded) {
        let mut state = self.state.borrow_mut();
        let Some(entry) = state.entries.get_mut(&decoded.path) else {
            return;
        };
        if entry.stamp != decoded.stamp || !matches!(entry.load, Load::Queued(_)) {
            return;
        }
        let reserved = entry.dims;
        let bytes = match decoded.image {
            Some(image) => {
                entry.dims = Some((image.width(), image.height()));
                let bytes = u64::from(image.width()) * u64::from(image.height()) * 4;
                entry.load = Load::Ready(image);
                bytes
            },
            None => {
                entry.dims = None;
                entry.load = Load::Failed;
                0
            },
        };
        if entry.dims != reserved {
            self.generation.set(self.generation.get().wrapping_add(1));
        }
        state.ready_bytes = state.ready_bytes.saturating_add(bytes);
        evict_over_budget(&mut state, self.budget);
    }

    /// The native size of `src` in the document at `source`, reading at most the
    /// file's header; queues the decode. `None` for an image the preview will not load.
    fn dimensions(&self, source: &Path, root: &Path, src: &str) -> Option<(u32, u32)> {
        let path = self.resolve(source, root, src)?;
        let meta = std::fs::metadata(&path).ok()?;
        if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
            return None;
        }
        let stamp = Stamp::of(&meta);
        let mut state = self.state.borrow_mut();
        state.clock += 1;
        let now = state.clock;
        if let Some(entry) = state.entries.get_mut(&path)
            && entry.stamp == stamp
        {
            entry.last_used = now;
            return entry.dims;
        }
        let head = read_head(&path)?;
        let dims = image::probe_dimensions(&head);
        let over_cap = dims.is_some_and(|(w, h)| u64::from(w) * u64::from(h) > MAX_PIXELS);
        // Only a format the decoder knows is worth a decode: TIFF has no header size to
        // probe, and a JPEG's frame header can lie past the bytes read (behind a large
        // EXIF or ICC segment), but both decode; anything else (SVG, GIF, …) is a chip
        // straight away.
        let decodable = dims.is_some() || is_tiff(&head) || head.starts_with(b"\xff\xd8\xff");
        let load = if over_cap || !decodable {
            Load::Failed
        } else {
            Load::Queued(Pending::start())
        };
        let queue = matches!(load, Load::Queued(_));
        if let Some(previous) = state.entries.remove(&path)
            && let Load::Ready(image) = previous.load
        {
            let bytes = u64::from(image.width()) * u64::from(image.height()) * 4;
            state.ready_bytes = state.ready_bytes.saturating_sub(bytes);
        }
        let dims = dims.filter(|_| queue);
        state.entries.insert(
            path.clone(),
            Entry {
                stamp,
                dims,
                load,
                last_used: now,
            },
        );
        drop(state);
        if queue {
            self.queue(path, stamp);
        }
        dims
    }

    /// Resolve `src` against the document at `source`, remembering the answer: only a
    /// file inside the workspace is ever loaded.
    fn resolve(&self, source: &Path, root: &Path, src: &str) -> Option<PathBuf> {
        let key = (source.to_path_buf(), src.to_owned());
        if let Some(resolved) = self.state.borrow().resolved.get(&key) {
            return resolved.clone();
        }
        let resolved = match crate::links::resolve(src, source, root) {
            Ok(LinkTarget::WorkspaceFile { path, .. }) => Some(path),
            _ => None,
        };
        self.state
            .borrow_mut()
            .resolved
            .insert(key, resolved.clone());
        resolved
    }

    /// Hand a decode to the worker, starting it on first use. A worker that cannot be
    /// started (or has gone) fails the image, so it settles as a chip.
    fn queue(&self, path: PathBuf, stamp: Stamp) {
        let mut jobs = self.jobs.borrow_mut();
        if jobs.is_none() {
            *jobs = spawn_worker(self.results.clone());
        }
        let job = Job {
            path: path.clone(),
            stamp,
        };
        if jobs.as_ref().is_some_and(|jobs| jobs.send(job).is_ok()) {
            return;
        }
        *jobs = None;
        drop(jobs);
        self.accept(Decoded {
            path,
            stamp,
            image: None,
        });
    }
}

/// Drop the least-recently-seen decoded images until the rest fit `budget` bytes.
fn evict_over_budget(state: &mut State, budget: u64) {
    while state.ready_bytes > budget {
        let oldest = state
            .entries
            .iter_mut()
            .filter(|(_, entry)| matches!(entry.load, Load::Ready(_)))
            .min_by_key(|(_, entry)| entry.last_used);
        let Some((_, entry)) = oldest else {
            state.ready_bytes = 0;
            return;
        };
        if let Load::Ready(image) = std::mem::replace(&mut entry.load, Load::Evicted) {
            let bytes = u64::from(image.width()) * u64::from(image.height()) * 4;
            state.ready_bytes = state.ready_bytes.saturating_sub(bytes);
        }
    }
}

/// Start the decode worker: one thread, fed through `jobs`, answering on `results`.
fn spawn_worker(results: mpsc::UnboundedSender<Decoded>) -> Option<std_mpsc::Sender<Job>> {
    let (jobs, queue) = std_mpsc::channel::<Job>();
    std::thread::Builder::new()
        .name("karet-preview-images".to_owned())
        .spawn(move || {
            while let Ok(job) = queue.recv() {
                if results.send(decode_file(job)).is_err() {
                    break;
                }
            }
        })
        .ok()?;
    Some(jobs)
}

/// Decode one file, re-checking what the draw path checked: the file may have
/// changed in between.
fn decode_file(job: Job) -> Decoded {
    let image = std::fs::metadata(&job.path)
        .ok()
        .filter(|meta| meta.is_file() && meta.len() <= MAX_FILE_BYTES)
        .and_then(|_| std::fs::read(&job.path).ok())
        .filter(|bytes| admissible(bytes))
        .and_then(|bytes| image::decode(&bytes).ok())
        .filter(|image| u64::from(image.width()) * u64::from(image.height()) <= MAX_PIXELS)
        .map(Arc::new);
    Decoded {
        path: job.path,
        stamp: job.stamp,
        image,
    }
}

/// Whether a whole file may be decoded: its header's size, read from the full bytes
/// (a JPEG's frame header can lie past the draw path's probe), is within
/// [`MAX_PIXELS`]. The decoder allocates from that header, so a hostile file claiming
/// a vast image is refused before it can. TIFF, whose size is not in a fixed header,
/// relies on its decoder's own allocation cap.
pub(crate) fn admissible(bytes: &[u8]) -> bool {
    match image::probe_dimensions(bytes) {
        Some((w, h)) => u64::from(w) * u64::from(h) <= MAX_PIXELS,
        None => is_tiff(bytes),
    }
}

/// The first [`PROBE_BYTES`] of the file at `path`.
fn read_head(path: &Path) -> Option<Vec<u8>> {
    let mut head = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(PROBE_BYTES)
        .read_to_end(&mut head)
        .ok()?;
    Some(head)
}

fn is_tiff(head: &[u8]) -> bool {
    head.starts_with(b"II*\0") || head.starts_with(b"MM\0*")
}

/// Sizes the images of one document for the preview's layout.
pub(crate) struct Sizer<'a> {
    images: &'a PreviewImages,
    source: &'a Path,
    root: &'a Path,
}

impl ImageSizer for Sizer<'_> {
    fn dimensions(&self, image: &ImageRef) -> Option<(u32, u32)> {
        self.images.dimensions(self.source, self.root, &image.src)
    }
}

/// Decode `path` now, stamped with `meta`, as the worker would.
#[cfg(test)]
pub(crate) fn decode_now(path: PathBuf, meta: &std::fs::Metadata) -> Decoded {
    decode_file(Job {
        path,
        stamp: Stamp::of(meta),
    })
}

#[cfg(test)]
impl PreviewImages {
    /// Run every queued decode on this thread and accept the results, so a test sees
    /// the settled cache without a worker or an event loop.
    pub(crate) fn settle(&self) {
        let queued: Vec<(PathBuf, Stamp)> = self
            .state
            .borrow()
            .entries
            .iter()
            .filter(|(_, entry)| matches!(entry.load, Load::Queued(_)))
            .map(|(path, entry)| (path.clone(), entry.stamp))
            .collect();
        for (path, stamp) in queued {
            self.accept(decode_file(Job { path, stamp }));
        }
    }

    /// Mark every queued load as pending since long enough ago to show its placeholder.
    pub(crate) fn backdate_pending(&self) {
        for entry in self.state.borrow_mut().entries.values_mut() {
            if let Load::Queued(pending) = &mut entry.load {
                *pending = Pending::revealed();
            }
        }
    }

    /// The decoded bytes currently held.
    pub(crate) fn ready_bytes(&self) -> u64 {
        self.state.borrow().ready_bytes
    }

    /// A cache keeping at most `budget` decoded bytes.
    pub(crate) fn with_budget(budget: u64) -> Self {
        Self {
            budget,
            ..Self::default()
        }
    }
}
