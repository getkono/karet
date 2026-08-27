//! The workspace-filesystem worker: a dedicated thread for path I/O.
//!
//! Listing a directory, walking a workspace, reading a PDF and copying a tree are
//! all blocking, filesystem-bound work. Running any of them on the session actor
//! would stall every other document, so they get their own thread and answer on
//! the shared [`Event`] stream — the same shape [`search_worker`](crate::search_worker)
//! and [`vcs_worker`](crate::vcs_worker) already use.
//!
//! These operations exist as commands because the presentation layer may not
//! share a machine with the workspace. A local client pays one channel hop; a
//! remote one gets the only path it could have had.

use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;

use tokio::sync::mpsc as tokio_mpsc;

use crate::api::Event;
use crate::api::FileChunk;
use crate::api::PathClass;
use crate::api::PathMutation;
use crate::api::RequestId;

mod listing;
mod mutate;

#[cfg(test)]
mod tests;

/// How many leading bytes classification samples.
///
/// Matches the window [`karet_filetype::classify`] documents: enough for every
/// magic number it recognizes, small enough that classifying a huge file is still
/// one short read.
const HEAD_BYTES: usize = 8 * 1024;

/// The largest chunk a single [`FsJob::ReadBytes`] answers with.
///
/// Media is delivered in pieces so a large PDF cannot monopolize the event stream
/// (or, in remote mode, the connection) ahead of an interactive edit.
const MAX_CHUNK: u64 = 1024 * 1024;

/// One unit of background filesystem work.
pub(crate) enum FsJob {
    /// Classify a path and answer with [`Event::PathClassified`].
    Classify {
        /// Correlates the answering event.
        id: RequestId,
        /// The path to classify.
        path: PathBuf,
        /// Bypass the size guard.
        ignore_size: bool,
    },
    /// Read a byte range and answer with [`Event::FileBytes`].
    ReadBytes {
        /// Correlates the answering event.
        id: RequestId,
        /// The file to read.
        path: PathBuf,
        /// Where to start.
        offset: u64,
        /// How much to read, capped at [`MAX_CHUNK`].
        len: u64,
    },
    /// Walk the workspace and answer with [`Event::FilesListed`].
    ListFiles {
        /// Correlates the answering event.
        id: RequestId,
        /// The workspace root to walk.
        root: PathBuf,
        /// Stop after this many paths.
        limit: usize,
    },
    /// List one directory and answer with [`Event::DirectoryListed`].
    ReadDirectory {
        /// Correlates the answering event.
        id: RequestId,
        /// The directory to list.
        path: PathBuf,
        /// Include dotfiles.
        show_hidden: bool,
        /// Flag gitignored entries rather than listing them plainly.
        respect_gitignore: bool,
    },
    /// Mutate a path and answer with [`Event::PathMutated`].
    Mutate {
        /// Correlates the answering event.
        id: RequestId,
        /// What to do.
        mutation: PathMutation,
    },
}

/// Start the worker; the session sends [`FsJob`]s and answers arrive on the
/// shared event stream.
pub(crate) fn spawn(
    events: tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) -> Sender<FsJob> {
    let (jobs_tx, jobs_rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("karet-fs".to_owned())
        .spawn(move || run(&jobs_rx, &events));
    jobs_tx
}

fn run(jobs: &Receiver<FsJob>, events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>) {
    while let Ok(job) = jobs.recv() {
        let (id, event) = answer(job);
        if events.send((Some(id), event)).is_err() {
            break; // the session shut down
        }
    }
}

/// Run one job to its answering event.
///
/// Split from [`run`] so every job is testable without a thread or a channel.
fn answer(job: FsJob) -> (RequestId, Event) {
    match job {
        FsJob::Classify {
            id,
            path,
            ignore_size,
        } => {
            let result = classify(&path, ignore_size);
            (id, Event::PathClassified { path, result })
        },
        FsJob::ReadBytes {
            id,
            path,
            offset,
            len,
        } => {
            let result = read_chunk(&path, offset, len);
            (id, Event::FileBytes { path, result })
        },
        FsJob::ListFiles { id, root, limit } => {
            let (files, truncated) = listing::workspace_files(&root, limit);
            (id, Event::FilesListed { files, truncated })
        },
        FsJob::ReadDirectory {
            id,
            path,
            show_hidden,
            respect_gitignore,
        } => {
            let result = listing::directory(&path, show_hidden, respect_gitignore);
            (id, Event::DirectoryListed { path, result })
        },
        FsJob::Mutate { id, mutation } => {
            let result = mutate::run(&mutation);
            (id, Event::PathMutated { mutation, result })
        },
    }
}

/// Classify `path` from its leading bytes and length.
fn classify(path: &Path, ignore_size: bool) -> Result<PathClass, String> {
    let len = std::fs::metadata(path)
        .map_err(|error| error.to_string())?
        .len();
    let head = read_head(path)?;
    let kind = if ignore_size {
        karet_filetype::classify_ignoring_size(path, &head)
    } else {
        karet_filetype::classify(path, &head, len)
    };
    Ok(PathClass { kind, len, head })
}

/// Read at most [`HEAD_BYTES`] from the start of `path`.
///
/// A short read is not an error: classification is defined over whatever leading
/// bytes exist, and an empty file is legitimately empty.
fn read_head(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut head = Vec::new();
    file.by_ref()
        .take(HEAD_BYTES as u64)
        .read_to_end(&mut head)
        .map_err(|error| error.to_string())?;
    Ok(head)
}

/// Read `len` bytes of `path` from `offset`, capped at [`MAX_CHUNK`].
fn read_chunk(path: &Path, offset: u64, len: u64) -> Result<FileChunk, String> {
    use std::io::Read;
    use std::io::Seek;

    let total_len = std::fs::metadata(path)
        .map_err(|error| error.to_string())?
        .len();
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    // Seeking past the end is legal and yields no bytes, which reads as a final
    // chunk — the right answer for a client that asked one range too many.
    file.seek(std::io::SeekFrom::Start(offset))
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(len.min(MAX_CHUNK))
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(FileChunk {
        offset,
        bytes,
        total_len,
    })
}
