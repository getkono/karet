//! Verified downloads and safe archive extraction for managed servers.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;

use reqwest::blocking::Client;
use sha2::Digest;
use sha2::Sha256;

use super::catalog::Archive;

/// Ceiling on a download whose publisher declares no size.
///
/// The SHA-256 check bounds nothing on its own: it is computed only once the
/// whole body is in memory, so a hostile -- or merely broken -- host could
/// stream until the process died and the digest would never be consulted. The
/// largest payload the catalogue installs is the bundled Node runtime, around
/// 50 MB compressed; the biggest GitHub release is smaller still. This leaves a
/// fivefold margin over anything real.
const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;

/// Slack allowed over a publisher-declared asset size.
///
/// GitHub reports an exact byte count, so the tolerance only has to absorb an
/// asset re-uploaded slightly larger between discovery and install.
const DECLARED_SIZE_SLACK: u64 = 4 * 1024 * 1024;

/// Ceiling on the bytes one archive may write to disk.
///
/// Compression ratios are unbounded and this tree is extracted eagerly, so
/// without a cap a 1 MB zip filled 1 GB of disk before anything refused it.
/// The Node runtime unpacks to roughly 150 MB (the binary, the headers and
/// npm), the largest real extraction by a wide margin -- threefold headroom.
const MAX_EXTRACTED_BYTES: u64 = 512 * 1024 * 1024;

/// The file-type bits of a unix mode.
const FILE_TYPE_MASK: u32 = 0o170000;
/// `S_IFLNK`: the file type a symbolic link records in its mode.
const SYMLINK_TYPE: u32 = 0o120000;

/// How many bytes an extraction may still write before it is refused.
struct Budget {
    remaining: u64,
}

impl Budget {
    fn new() -> Self {
        Self {
            remaining: MAX_EXTRACTED_BYTES,
        }
    }

    fn exceeded() -> String {
        format!("archive expands past the {MAX_EXTRACTED_BYTES}-byte extraction limit")
    }

    /// Charge `bytes` against the budget, or refuse naming the limit.
    fn take(&mut self, bytes: u64) -> Result<(), String> {
        self.remaining = self
            .remaining
            .checked_sub(bytes)
            .ok_or_else(Self::exceeded)?;
        Ok(())
    }

    /// Refuse before writing anything when `bytes` alone cannot fit.
    ///
    /// A declared size is the archive's own claim, so it is never trusted to
    /// *permit* an entry -- only to refuse one early, which is what keeps a bomb
    /// from putting half a gigabyte on disk before the running total catches it.
    fn admits(&self, bytes: u64) -> Result<(), String> {
        if bytes > self.remaining {
            return Err(Self::exceeded());
        }
        Ok(())
    }

    /// Copy `reader` into `writer`, charging what it writes.
    ///
    /// Reads one byte past the remaining budget so an over-long stream is
    /// refused rather than silently truncated.
    fn copy(
        &mut self,
        reader: &mut impl Read,
        writer: &mut impl std::io::Write,
    ) -> Result<(), String> {
        let mut limited = reader.take(self.remaining.saturating_add(1));
        let written = std::io::copy(&mut limited, writer).map_err(|error| error.to_string())?;
        self.take(written)
    }
}

/// The largest body this download may accept.
///
/// A publisher-declared size past the absolute ceiling is refused on its own
/// terms: nothing the catalogue installs is remotely near it.
fn download_ceiling(declared: Option<u64>) -> u64 {
    declared
        .map_or(MAX_DOWNLOAD_BYTES, |size| {
            size.saturating_add(DECLARED_SIZE_SLACK)
        })
        .min(MAX_DOWNLOAD_BYTES)
}

pub(super) fn download_verified(
    client: &Client,
    url: &str,
    expected: &str,
    declared: Option<u64>,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<Vec<u8>, String> {
    let ceiling = download_ceiling(declared);
    let mut response = client
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| error.to_string())?;
    let total = response.content_length();
    if total.is_some_and(|length| length > ceiling) {
        return Err(format!("{url} exceeds the {ceiling}-byte download limit"));
    }
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        let read = response
            .read(&mut chunk)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() as u64 > ceiling {
            return Err(format!("{url} exceeds the {ceiling}-byte download limit"));
        }
        progress(bytes.len() as u64, total);
    }
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(format!("SHA-256 mismatch for {url}"));
    }
    Ok(bytes)
}

pub(super) fn extract_executable(
    bytes: &[u8],
    archive: Archive,
    executable_name: &str,
    destination: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    if matches!(archive, Archive::Raw) {
        let path = destination.join(executable_name);
        std::fs::write(&path, bytes).map_err(|error| error.to_string())?;
        return make_executable(&path);
    }
    if matches!(archive, Archive::Gzip) {
        let mut decoder = flate2::read::GzDecoder::new(bytes);
        let path = destination.join(executable_name);
        let mut file = File::create(&path).map_err(|error| error.to_string())?;
        Budget::new().copy(&mut decoder, &mut file)?;
        make_executable(&path)?;
        return Ok(());
    }
    let scratch = tempfile::tempdir_in(destination).map_err(|error| error.to_string())?;
    extract_archive(bytes, archive, scratch.path(), false)?;
    let source = find_file_named(scratch.path(), executable_name)
        .ok_or_else(|| format!("archive contains no {executable_name}"))?;
    let target = destination.join(executable_name);
    std::fs::copy(source, &target).map_err(|error| error.to_string())?;
    make_executable(&target)
}

pub(super) fn extract_archive(
    bytes: &[u8],
    archive: Archive,
    destination: &Path,
    all_files: bool,
) -> Result<(), String> {
    std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    let mut budget = Budget::new();
    match archive {
        Archive::Raw | Archive::Gzip => Err("payload is not a multi-file archive".into()),
        Archive::TarGzip => {
            let decoder = flate2::read::GzDecoder::new(bytes);
            extract_tar(decoder, destination, all_files, &mut budget)
        },
        Archive::TarXz => {
            let decoder = lzma_rust2::XzReader::new(bytes, false);
            extract_tar(decoder, destination, all_files, &mut budget)
        },
        Archive::Zip => extract_zip(bytes, destination, all_files, &mut budget),
    }
}

/// Is `mode` a symbolic link rather than a regular file or directory?
fn is_symlink(mode: u32) -> bool {
    mode & FILE_TYPE_MASK == SYMLINK_TYPE
}

/// Extract a zip, honouring archived permissions but never archived link types.
///
/// A zip records a symlink as an ordinary entry whose *body* is the link target
/// and whose mode carries `S_IFLNK`. Writing that body as a regular file is the
/// worst of the three options: `bin/clangd -> ../lib/clangd` becomes a 13-byte
/// text file that [`find_file_named`] then hands back as the launch command, and
/// with the mode restored it is even executable. Such entries are skipped, so a
/// bundle that needs one installs visibly incomplete ("archive contains no
/// clangd") rather than launching a text file.
///
/// Recreating them was considered and rejected: a target can only be validated
/// against links the same archive already created, and a two-link chain
/// (`d -> .`, then `d/e -> ..`) defeats any purely lexical check. The tar path
/// keeps its links because `tar`'s unpacker canonicalizes each parent against
/// the destination before writing through it.
fn extract_zip(
    bytes: &[u8],
    destination: &Path,
    all_files: bool,
    budget: &mut Budget,
) -> Result<(), String> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|error| error.to_string())?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        let Some(path) = entry.enclosed_name() else {
            return Err("archive contains an unsafe path".into());
        };
        let output = destination.join(path);
        let mode = entry.unix_mode();
        if mode.is_some_and(is_symlink) {
            continue;
        }
        if entry.is_dir() {
            std::fs::create_dir_all(&output).map_err(|error| error.to_string())?;
        } else if all_files || entry.is_file() {
            budget.admits(entry.size())?;
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            let mut file = File::create(&output).map_err(|error| error.to_string())?;
            budget.copy(&mut entry, &mut file)?;
            drop(file);
            restore_mode(&output, mode)?;
        }
    }
    Ok(())
}

/// Extract a tar, reapplying each entry's permissions through [`restore_mode`].
///
/// `tar::Entry::unpack_in` writes the header mode verbatim -- it masks nothing
/// -- so an archive recording `0o777` left a language server this process execs
/// writable by every other local user on the machine. That is the whole of the
/// tar payload set: the bundled Node runtime is a `.tar.gz` on both Linux
/// targets and both macOS ones, as is `lua-language-server`.
///
/// Every entry `unpack_in` writes as a real file is re-chmod'd, which is wider
/// than "regular files and directories": POSIX requires an unrecognised typeflag
/// to be treated as a regular file, so `unpack_in` materialises a contiguous
/// entry (typeflag `7`), a FIFO entry (`6`) and anything it does not recognise
/// as an ordinary file holding the entry body -- with the header mode applied
/// verbatim. Gating on `is_file() || is_dir()` therefore skipped the mask for
/// exactly the entry types a hostile publisher would reach for.
///
/// The two exclusions are the link types. `set_permissions` follows symlinks, so
/// touching a link entry would repermission whatever it points at, and a hard
/// link shares its inode with an entry already handled.
fn extract_tar(
    reader: impl Read,
    destination: &Path,
    all_files: bool,
    budget: &mut Budget,
) -> Result<(), String> {
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries().map_err(|error| error.to_string())? {
        let mut entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path().map_err(|error| error.to_string())?;
        if path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::RootDir
            )
        }) {
            return Err("archive contains an unsafe path".into());
        }
        // The same path `unpack_in` derives: every normal component appended to
        // the destination, with `.` segments dropped.
        let mut output = destination.to_path_buf();
        for part in path.components() {
            if let std::path::Component::Normal(part) = part {
                output.push(part);
            }
        }
        let header = entry.header();
        let kind = header.entry_type();
        let mode = header.mode().ok();
        let size = header.size().unwrap_or_default();
        if !(all_files || kind.is_file()) {
            continue;
        }
        budget.take(size)?;
        let unpacked = entry
            .unpack_in(destination)
            .map_err(|error| error.to_string())?;
        // `output == destination` is the `./` entry, which `unpack_in` reports
        // as handled without writing anything: the install root's own mode is
        // not the archive's to set. `unpack_in` reports the same "handled" for
        // metadata entries it deliberately writes nothing for -- a pax global
        // header, or a long-name record whose magic it did not recognise -- so
        // what is actually on disk decides, not the header's claim.
        let written = unpacked
            && output != destination
            && !(kind.is_symlink() || kind.is_hard_link())
            && std::fs::symlink_metadata(&output).is_ok_and(|meta| !meta.is_symlink());
        if written {
            restore_mode(&output, mode)?;
        }
    }
    Ok(())
}

/// Find `name` under `root`, shallowest first and then alphabetically.
///
/// Both orderings are deliberate. `read_dir` yields entries in whatever order
/// the filesystem stores them, so a depth-first search returned a different
/// file on different machines whenever an archive contained the name twice --
/// and the result becomes the executable karet records and launches. Shallowest
/// first also picks the real payload over a vendored copy nested inside it.
///
/// Symlinked directories are not descended into: an archive that points at
/// itself would otherwise loop. No managed release puts its payload behind one,
/// and a search that can be made to spin on a downloaded archive is the worse
/// trade.
pub(super) fn find_file_named(root: &Path, name: &str) -> Option<PathBuf> {
    let mut frontier = vec![root.to_path_buf()];
    while !frontier.is_empty() {
        let mut directories = Vec::new();
        let mut matches = Vec::new();
        for directory in frontier.drain(..) {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    directories.push(path);
                } else if path.file_name().is_some_and(|candidate| candidate == name)
                    && path.is_file()
                    && resolves_within(&path, root)
                {
                    matches.push(path);
                }
            }
        }
        if !matches.is_empty() {
            matches.sort();
            return matches.into_iter().next();
        }
        directories.sort();
        frontier = directories;
    }
    None
}

/// Whether `path` still resolves inside `root` once symlinks are followed.
///
/// [`Path::is_file`] follows links, so without this a hostile archive could
/// name a symlink after the executable being looked for and make the recorded
/// launch command point anywhere on the filesystem -- `tar` preserves links
/// verbatim, and the search runs over freshly downloaded content.
///
/// Containment rather than absence is the property worth enforcing: links that
/// stay inside the payload are ordinary, and the bundled Node runtime ships
/// `bin/npm` as one.
fn resolves_within(path: &Path, root: &Path) -> bool {
    let (Ok(resolved), Ok(root)) = (path.canonicalize(), root.canonicalize()) else {
        return false;
    };
    resolved.starts_with(root)
}

/// Reapply an archived file's unix permissions after extraction.
///
/// Zip extraction wrote every entry with the default mode, which silently
/// stripped the executable bit from bundles extracted whole -- clangd on every
/// platform, since its release is a zip, and the Windows Node runtime. Only the
/// single file `activation` later located was ever chmod'd, so a bundle whose
/// entry point is a wrapper script calling a sibling binary was installed
/// broken.
///
/// The archive chooses the read and execute bits, and the owner's write bit;
/// nothing else. setuid, setgid and the sticky bit are never honoured from a
/// download, and neither is group or other **write**: an archive is free to
/// record `0o777`, and karet execs binaries out of this tree, so honouring that
/// would let any other local user on a shared machine replace a language
/// server. `0o777` lands as `0o755` and `0o666` as `0o644`.
///
/// Applied on both paths. `tar` was the wider hole of the two: `unpack_in`
/// masks nothing at all, and every npm-backed provider extracts a `.tar.gz`
/// Node runtime on Linux and macOS.
#[cfg(unix)]
fn restore_mode(path: &Path, mode: Option<u32>) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let Some(mode) = mode else {
        return Ok(());
    };
    let safe = mode & 0o777 & !0o022;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(safe))
        .map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn restore_mode(_path: &Path, _mode: Option<u32>) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
pub(super) fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)
        .map_err(|error| error.to_string())?
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
pub(super) fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// A version turned into a single directory name that cannot be navigation.
///
/// Filtering the characters is not enough on its own: `.`, `..` and the empty
/// string survive it intact and none of them names a *new* directory. A release
/// tagged `v..` yielded `versions/..`, which is the provider root itself -- an
/// install that silently no-ops because the destination already exists, and a
/// retirement that hands the provider root to `remove_dir_all`, taking the
/// activation journals with it. All three are rewritten to underscores, which
/// are ordinary names of the same length.
pub(super) fn safe_version(version: &str) -> String {
    let name: String = version
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if matches!(name.as_str(), "" | "." | "..") {
        return "_".repeat(name.len().max(1));
    }
    name
}

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;
