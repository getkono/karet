//! The decoded-image cache behind the markdown preview (see the parent module).

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc as std_mpsc;
use std::time::Duration;
use std::time::Instant;
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
/// The most pixels an image may have: a larger one would cost its decode, its memory
/// and its transmission to the terminal before a single cell of it showed.
const MAX_PIXELS: u64 = 4096 * 4096;
/// The decoded bytes kept before the least-recently-seen images are dropped.
const READY_BUDGET: u64 = 256 * 1024 * 1024;
/// How long a sized image's stamp is trusted before a re-wrap stats the file again.
///
/// The preview re-wraps on every keystroke, and a `stat` (plus the containment check)
/// per image per keystroke is wasted work for files that almost never change. So an
/// image checked within this interval is answered from the cache without touching
/// the filesystem. The staleness this adds is bounded and small: a file changed on
/// disk was only ever noticed at the next re-wrap, and now it is noticed at the first
/// re-wrap at least this long after the previous check. The decode worker re-checks
/// the file regardless, so a stale answer here never reads anything it should not.
const RESTAT_INTERVAL: Duration = Duration::from_secs(1);

/// What the preview may paint for an image.
#[derive(Clone, Debug)]
pub(crate) enum Lookup {
    /// Decoded and ready.
    Ready(Paint),
    /// Still decoding, since this moment.
    Loading(Pending),
    /// Never going to be painted: refused, unreadable, or undecodable.
    Missing,
}

/// How a ready image is painted into the cell box it was looked up for.
#[derive(Clone, Debug)]
pub(crate) enum Paint {
    /// As halfblocks: the image already resampled to the box — its columns wide and
    /// twice its rows tall — so the painter maps it one to one.
    Pixels(Arc<Image>),
    /// As Kitty unicode placeholders naming the image the terminal holds at full
    /// resolution, and its placement sized to the box (see [`super::kitty`]).
    Placeholder {
        /// The image id.
        id: u32,
        /// The placement id.
        placement: u32,
    },
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
    /// The escapes transmitting the image to a Kitty terminal, built off the UI
    /// thread when the job asked for them.
    payload: Option<String>,
}

/// A decode for the worker.
struct Job {
    path: PathBuf,
    stamp: Stamp,
    /// The id to build the Kitty transmission under, when the terminal takes one.
    kitty_id: Option<u32>,
}

/// A decoded image, and what painting it has derived from it.
#[derive(Debug)]
struct Ready {
    image: Arc<Image>,
    /// The Kitty transmission, until it is sent.
    payload: Option<String>,
    /// The halfblock resamples for the boxes it was last painted in, most recent
    /// last: a few, so two panes showing it at different widths do not thrash.
    resampled: Vec<((u16, u16), Arc<Image>)>,
}

/// How many halfblock resamples of one image are kept.
const RESAMPLES_KEPT: usize = 4;

impl Ready {
    /// The image resampled to `(cols, rows)` halfblock cells, resampled only when the
    /// box changes rather than on every frame.
    fn resampled(&mut self, (cols, rows): (u16, u16)) -> Arc<Image> {
        if let Some(index) = self
            .resampled
            .iter()
            .position(|(cells, _)| *cells == (cols, rows))
        {
            let hit = self.resampled.remove(index);
            let image = Arc::clone(&hit.1);
            self.resampled.push(hit);
            return image;
        }
        let image = Arc::new(self.image.resized(u32::from(cols), u32::from(rows) * 2));
        if self.resampled.len() == RESAMPLES_KEPT {
            self.resampled.remove(0);
        }
        self.resampled.push(((cols, rows), Arc::clone(&image)));
        image
    }
}

#[derive(Debug)]
enum Load {
    /// Sized from its header, but never painted, so never decoded: the decode is
    /// queued by the first [`PreviewImages::lookup`], not by layout, so a long
    /// document decodes only the images the reader actually scrolls to.
    Unrequested,
    Queued(Pending),
    Ready(Ready),
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
    /// The [`State::clock`] tick this image was last painted (or first sized) at.
    last_used: u64,
    /// The [`State::epoch`] this image was last painted in, if ever.
    seen: Option<u64>,
    /// When the file was last stat'ed and checked, for [`RESTAT_INTERVAL`].
    checked: Instant,
    /// The image's Kitty id, unique to this entry (a changed file gets a new one).
    id: u32,
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
    /// The current frame, advanced by [`PreviewImages::end_frame`]. An image looked
    /// up in the current frame or the one before it is on screen and is never
    /// evicted — see [`evict_over_budget`].
    epoch: u64,
    /// Paths the preview refused (missing, oversized, not a regular file, no longer
    /// canonical, unreadable), with when they were checked: a broken image link is
    /// not re-checked on every keystroke either, for [`RESTAT_INTERVAL`].
    refused: HashMap<PathBuf, Instant>,
    /// The last Kitty image id handed out.
    last_id: u32,
    kitty: Kitty,
}

/// What a Kitty terminal holds of the preview's images, and what it still needs told.
#[derive(Debug, Default)]
struct Kitty {
    /// Whether images are painted as Kitty unicode placeholders.
    enabled: bool,
    /// The images transmitted, each with the cell boxes placed for it — the `n`th
    /// box is placement `n + 1`.
    sent: HashMap<u32, Vec<(u16, u16)>>,
    /// Escapes to write after the frame is drawn.
    output: String,
}

impl Kitty {
    /// Tell the terminal to drop image `id`, if it holds it.
    fn forget(&mut self, id: u32) {
        if self.sent.remove(&id).is_some() {
            self.output.push_str(&image::kitty_delete_image(id));
        }
    }
}

impl State {
    /// A fresh Kitty image id: 24 bits, as a placeholder's colour carries it, and
    /// never 0.
    fn next_id(&mut self) -> u32 {
        self.last_id = self.last_id % 0x00ff_ffff + 1;
        self.last_id
    }

    /// Forget a removed entry: its decoded bytes stop counting against the budget,
    /// and a Kitty terminal drops its image.
    fn release(&mut self, entry: &Entry) {
        if let Load::Ready(ready) = &entry.load {
            self.ready_bytes = self.ready_bytes.saturating_sub(image_bytes(&ready.image));
        }
        self.kitty.forget(entry.id);
    }
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
    /// How long a sized image goes unchecked: [`RESTAT_INTERVAL`], but for tests.
    restat: Duration,
    /// The pixel size of one terminal cell, which sizes every image.
    cell_px: Cell<(u32, u32)>,
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
            restat: RESTAT_INTERVAL,
            cell_px: Cell::new(karet_markdown::DEFAULT_CELL_PIXELS),
        }
    }
}

impl PreviewImages {
    /// The receiver of finished decodes, for the event loop. `None` after the first call.
    pub(crate) fn take_receiver(&self) -> Option<mpsc::UnboundedReceiver<Decoded>> {
        self.receiver.borrow_mut().take()
    }

    /// Paint images as Kitty unicode placeholders (`kitty`) or as halfblocks, on cells
    /// of `cell_px` pixels. A new cell size changes every image's box, so it bumps the
    /// generation and every preview re-wraps.
    pub(crate) fn configure(&self, kitty: bool, cell_px: (u32, u32)) {
        self.state.borrow_mut().kitty.enabled = kitty;
        if self.cell_px.replace(cell_px) != cell_px {
            self.generation.set(self.generation.get().wrapping_add(1));
        }
    }

    /// The Kitty escapes the frame just drawn needs written after it: transmissions,
    /// placements, and deletions.
    pub(crate) fn take_output(&self) -> String {
        std::mem::take(&mut self.state.borrow_mut().kitty.output)
    }

    /// The escapes deleting every image a Kitty terminal holds for the preview, for
    /// when the editor exits.
    pub(crate) fn teardown(&self) -> String {
        let mut state = self.state.borrow_mut();
        let ids: Vec<u32> = state.kitty.sent.keys().copied().collect();
        for id in ids {
            state.kitty.forget(id);
        }
        std::mem::take(&mut state.kitty.output)
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

    /// What to paint for `src` in the document at `source`, in a box of `cells`
    /// `(columns, rows)` — called by the draw path for the visible image rows only. An
    /// image not decoded yet (never painted, or evicted) is queued to decode here, and
    /// its reveal delay runs from now. On a Kitty terminal, a ready image's first
    /// lookup queues its transmission, and a box new to it queues a placement.
    pub(crate) fn lookup(
        &self,
        source: &Path,
        root: &Path,
        src: &str,
        cells: (u16, u16),
    ) -> Lookup {
        let Some(path) = self.resolve(source, root, src) else {
            return Lookup::Missing;
        };
        let mut state = self.state.borrow_mut();
        state.clock += 1;
        let (now, epoch) = (state.clock, state.epoch);
        let State { entries, kitty, .. } = &mut *state;
        let Some(entry) = entries.get_mut(&path) else {
            return Lookup::Missing;
        };
        entry.last_used = now;
        entry.seen = Some(epoch);
        match &mut entry.load {
            Load::Ready(ready) if kitty.enabled => {
                Lookup::Ready(place(kitty, entry.id, ready, cells))
            },
            Load::Ready(ready) => Lookup::Ready(Paint::Pixels(ready.resampled(cells))),
            Load::Queued(pending) => Lookup::Loading(*pending),
            Load::Failed => Lookup::Missing,
            Load::Unrequested | Load::Evicted => {
                let pending = Pending::start();
                entry.load = Load::Queued(pending);
                let (stamp, id) = (entry.stamp, entry.id);
                drop(state);
                self.queue(path, stamp, id);
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
                let bytes = image_bytes(&image);
                entry.load = Load::Ready(Ready {
                    image,
                    payload: decoded.payload,
                    resampled: Vec::new(),
                });
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
        if bytes == 0 {
            return;
        }
        state.ready_bytes = state.ready_bytes.saturating_add(bytes);
        evict_over_budget(&mut state, self.budget, Some(&decoded.path));
    }

    /// End a drawn frame: every image the frame did not paint (nor the one before it)
    /// may now be evicted, so a screen scrolled away from its images gives their
    /// pixels back even if no decode lands again.
    pub(crate) fn end_frame(&self) {
        let mut state = self.state.borrow_mut();
        state.epoch = state.epoch.wrapping_add(1);
        evict_over_budget(&mut state, self.budget, None);
    }

    /// The native size of `src` in the document at `source`, reading at most the
    /// file's header. `None` for an image the preview will not load.
    ///
    /// Layout sizes every image in the document, so this only reserves rows; the
    /// decode waits for [`Self::lookup`] to see the image on screen.
    fn dimensions(&self, source: &Path, root: &Path, src: &str) -> Option<(u32, u32)> {
        let path = self.resolve(source, root, src)?;
        let checked = Instant::now();
        {
            let state = self.state.borrow();
            let fresh = |at: Instant| checked.saturating_duration_since(at) < self.restat;
            if state.refused.get(&path).is_some_and(|&at| fresh(at)) {
                return None;
            }
            if let Some(entry) = state.entries.get(&path)
                && fresh(entry.checked)
            {
                return entry.dims;
            }
        }
        // The path was canonical — so inside the workspace — when it was resolved; a
        // file (or directory) since swapped for a symlink leaves it no longer its own
        // canonical form, and it is refused rather than followed.
        let meta = still_canonical(&path)
            .then(|| std::fs::metadata(&path).ok())
            .flatten()
            .filter(|meta| meta.is_file() && meta.len() <= MAX_FILE_BYTES);
        let Some(meta) = meta else {
            self.refuse(&path, checked);
            return None;
        };
        let stamp = Stamp::of(&meta);
        if let Some(entry) = self.state.borrow_mut().entries.get_mut(&path)
            && entry.stamp == stamp
        {
            entry.checked = checked;
            return entry.dims;
        }
        let Some(head) = read_head(&path) else {
            self.refuse(&path, checked);
            return None;
        };
        let mut state = self.state.borrow_mut();
        state.clock += 1;
        let now = state.clock;
        let dims = image::probe_dimensions(&head);
        let over_cap = dims.is_some_and(|(w, h)| u64::from(w) * u64::from(h) > MAX_PIXELS);
        // Only a format the decoder knows is worth a decode: TIFF has no header size to
        // probe, a JPEG's frame header can lie past the bytes read (behind a large EXIF
        // or ICC segment), and so can an extended WebP's frame (behind a large `ALPH`
        // or `ICCP` chunk) — the probe vouches for its canvas only once it has seen
        // that frame — but all of them decode; anything else (SVG, GIF, …) is a chip
        // straight away. [`admissible`] re-probes the whole file before any decode, so
        // the pixel cap holds for these too. A JPEG or WebP read whole, or an animated
        // WebP, has nothing past the probe to vouch for it: the probe's refusal stands.
        let cut = u64::try_from(head.len()).is_ok_and(|len| len == PROBE_BYTES);
        let decodable = dims.is_some()
            || is_tiff(&head)
            || (cut && head.starts_with(b"\xff\xd8\xff"))
            || (cut && is_webp(&head) && !is_animated_webp(&head));
        let load = if over_cap || !decodable {
            Load::Failed
        } else if dims.is_some() {
            Load::Unrequested
        } else {
            // The exception to decoding on paint: with no size from the header (TIFF, or
            // a JPEG or WebP whose frame lies past the probe), layout reserves no
            // rows until the decode reports one — and a row-less image is never
            // painted, so it would never be looked up. It decodes now instead.
            Load::Queued(Pending::start())
        };
        let queue = matches!(load, Load::Queued(_));
        let dims = dims.filter(|_| !matches!(load, Load::Failed));
        state.refused.remove(&path);
        let id = state.next_id();
        let previous = state.entries.insert(
            path.clone(),
            Entry {
                stamp,
                dims,
                load,
                last_used: now,
                seen: None,
                checked,
                id,
            },
        );
        if let Some(previous) = previous {
            state.release(&previous);
        }
        drop(state);
        if queue {
            self.queue(path, stamp, id);
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

    /// Drop the entry for `path`, which the preview no longer loads, and its pixels,
    /// remembering the refusal (made at `checked`) so it is not re-checked per wrap.
    fn refuse(&self, path: &Path, checked: Instant) {
        let mut state = self.state.borrow_mut();
        if let Some(entry) = state.entries.remove(path) {
            state.release(&entry);
        }
        state.refused.insert(path.to_path_buf(), checked);
    }

    /// Hand a decode to the worker, starting it on first use — with the Kitty id to
    /// build its transmission under, on a Kitty terminal. A worker that cannot be
    /// started (or has gone) fails the image, so it settles as a chip.
    fn queue(&self, path: PathBuf, stamp: Stamp, id: u32) {
        let kitty_id = self.state.borrow().kitty.enabled.then_some(id);
        let mut jobs = self.jobs.borrow_mut();
        if jobs.is_none() {
            *jobs = spawn_worker(self.results.clone());
        }
        let job = Job {
            path: path.clone(),
            stamp,
            kitty_id,
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
            payload: None,
        });
    }
}

/// Queue what a Kitty terminal needs to show image `id` in a `cells` box — its
/// transmission, the first time, and a placement for a box new to it — and name the
/// placement to paint.
fn place(kitty: &mut Kitty, id: u32, ready: &mut Ready, cells: (u16, u16)) -> Paint {
    let Kitty { sent, output, .. } = kitty;
    let boxes = sent.entry(id).or_insert_with(|| {
        let payload = ready
            .payload
            .take()
            .unwrap_or_else(|| super::kitty::transmit(&ready.image, id));
        output.push_str(&payload);
        Vec::new()
    });
    let index = boxes
        .iter()
        .position(|&placed| placed == cells)
        .unwrap_or_else(|| {
            boxes.push(cells);
            let index = boxes.len() - 1;
            let placement = u32::try_from(index + 1).unwrap_or(u32::MAX);
            output.push_str(&super::kitty::place(id, placement, cells.0, cells.1));
            index
        });
    let placement = u32::try_from(index + 1).unwrap_or(u32::MAX);
    Paint::Placeholder { id, placement }
}

/// The decoded size of `image`: RGBA, four bytes a pixel.
fn image_bytes(image: &Image) -> u64 {
    u64::from(image.width()) * u64::from(image.height()) * 4
}

/// Drop the least-recently-seen decoded images until the rest fit `budget` bytes.
///
/// Never `accepted` (the decode just taken) nor an image painted in the current or
/// the last finished frame: evicting either would have the next frame queue it again,
/// and a screen whose images together exceed the budget would decode forever. Such a
/// screen is let run over budget instead, until a frame stops showing some of them
/// ([`PreviewImages::end_frame`] evicts then).
fn evict_over_budget(state: &mut State, budget: u64, accepted: Option<&Path>) {
    let recent = state.epoch.saturating_sub(1);
    while state.ready_bytes > budget {
        let State {
            entries,
            kitty,
            ready_bytes,
            ..
        } = &mut *state;
        let oldest = entries
            .iter_mut()
            .filter(|(path, entry)| {
                matches!(entry.load, Load::Ready(_))
                    && Some(path.as_path()) != accepted
                    && entry.seen.is_none_or(|seen| seen < recent)
            })
            .min_by_key(|(_, entry)| entry.last_used);
        let Some((_, entry)) = oldest else {
            return;
        };
        if let Load::Ready(ready) = std::mem::replace(&mut entry.load, Load::Evicted) {
            *ready_bytes = ready_bytes.saturating_sub(image_bytes(&ready.image));
        }
        // An evicted image is decoded and transmitted again when next painted.
        kitty.forget(entry.id);
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
/// changed in between — even into a symlink out of the workspace.
fn decode_file(job: Job) -> Decoded {
    let image = still_canonical(&job.path)
        .then(|| std::fs::metadata(&job.path).ok())
        .flatten()
        .filter(|meta| meta.is_file() && meta.len() <= MAX_FILE_BYTES)
        .and_then(|_| std::fs::read(&job.path).ok())
        .filter(|bytes| admissible(bytes))
        .and_then(|bytes| image::decode(&bytes).ok())
        .filter(|image| u64::from(image.width()) * u64::from(image.height()) <= MAX_PIXELS)
        .map(Arc::new);
    let payload = job
        .kitty_id
        .zip(image.as_deref())
        .map(|(id, image)| super::kitty::transmit(image, id));
    Decoded {
        path: job.path,
        stamp: job.stamp,
        image,
        payload,
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

/// Whether `path` — canonical, and so inside the workspace, when it was resolved — is
/// still its own canonical form: no component of it has since become a symlink.
fn still_canonical(path: &Path) -> bool {
    std::fs::canonicalize(path).is_ok_and(|canonical| canonical == path)
}

fn is_webp(head: &[u8]) -> bool {
    head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP")
}

/// Whether `head` is an extended WebP flagged as animated, which the still decoder
/// cannot decode.
fn is_animated_webp(head: &[u8]) -> bool {
    head.get(12..16) == Some(b"VP8X") && head.get(20).is_some_and(|flags| flags & 0x02 != 0)
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

    fn cell_pixels(&self) -> (u32, u32) {
        self.images.cell_px.get()
    }
}

/// Decode `path` now, stamped with `meta`, as the worker would.
#[cfg(test)]
pub(crate) fn decode_now(path: PathBuf, meta: &std::fs::Metadata) -> Decoded {
    decode_file(Job {
        path,
        stamp: Stamp::of(meta),
        kitty_id: None,
    })
}

#[cfg(test)]
impl PreviewImages {
    /// Run every decode the preview could want on this thread and accept the results,
    /// so a test sees the settled cache without a worker or an event loop: each queued
    /// decode, and each image sized but not yet painted — as if the whole document had
    /// been on screen. (An evicted image stays evicted until it is looked up again.)
    pub(crate) fn settle(&self) {
        let mut state = self.state.borrow_mut();
        let mut wanted = Vec::new();
        let kitty = state.kitty.enabled;
        for (path, entry) in &mut state.entries {
            if matches!(entry.load, Load::Unrequested) {
                entry.load = Load::Queued(Pending::start());
            }
            if matches!(entry.load, Load::Queued(_)) {
                wanted.push((path.clone(), entry.stamp, kitty.then_some(entry.id)));
            }
        }
        drop(state);
        for (path, stamp, kitty_id) in wanted {
            self.accept(decode_file(Job {
                path,
                stamp,
                kitty_id,
            }));
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

    /// Age every disk check past the restat interval, as if it had just elapsed.
    pub(crate) fn backdate_checks(&self) {
        let mut state = self.state.borrow_mut();
        let past = Instant::now().checked_sub(self.restat);
        let Some(past) = past else {
            return;
        };
        for entry in state.entries.values_mut() {
            entry.checked = past;
        }
        for checked in state.refused.values_mut() {
            *checked = past;
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

    /// A cache trusting a sized image's stamp for `restat` before checking it again.
    pub(crate) fn with_restat(restat: Duration) -> Self {
        Self {
            restat,
            ..Self::default()
        }
    }

    /// The decodes queued so far — each one handed to the worker.
    pub(crate) fn queued(&self) -> usize {
        self.pendings().len()
    }
}
