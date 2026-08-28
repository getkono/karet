//! Verified downloads and safe archive extraction for managed servers.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;

use reqwest::blocking::Client;
use sha2::Digest;
use sha2::Sha256;

use super::catalog::Archive;

pub(super) fn download_verified(
    client: &Client,
    url: &str,
    expected: &str,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<Vec<u8>, String> {
    let mut response = client
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| error.to_string())?;
    let total = response.content_length();
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
        std::io::copy(&mut decoder, &mut file).map_err(|error| error.to_string())?;
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
    match archive {
        Archive::Raw | Archive::Gzip => Err("payload is not a multi-file archive".into()),
        Archive::TarGzip => {
            let decoder = flate2::read::GzDecoder::new(bytes);
            extract_tar(decoder, destination, all_files)
        },
        Archive::TarXz => {
            let decoder = lzma_rust2::XzReader::new(bytes, false);
            extract_tar(decoder, destination, all_files)
        },
        Archive::Zip => {
            let cursor = std::io::Cursor::new(bytes);
            let mut archive = zip::ZipArchive::new(cursor).map_err(|error| error.to_string())?;
            for index in 0..archive.len() {
                let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
                let Some(path) = entry.enclosed_name() else {
                    return Err("archive contains an unsafe path".into());
                };
                let output = destination.join(path);
                if entry.is_dir() {
                    std::fs::create_dir_all(&output).map_err(|error| error.to_string())?;
                } else if all_files || entry.is_file() {
                    if let Some(parent) = output.parent() {
                        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                    }
                    let mode = entry.unix_mode();
                    let mut file = File::create(&output).map_err(|error| error.to_string())?;
                    std::io::copy(&mut entry, &mut file).map_err(|error| error.to_string())?;
                    drop(file);
                    restore_mode(&output, mode)?;
                }
            }
            Ok(())
        },
    }
}

fn extract_tar(reader: impl Read, destination: &Path, all_files: bool) -> Result<(), String> {
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
        if all_files || entry.header().entry_type().is_file() {
            entry
                .unpack_in(destination)
                .map_err(|error| error.to_string())?;
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

/// Reapply an archived file's unix permissions after extraction.
///
/// Zip extraction wrote every entry with the default mode, which silently
/// stripped the executable bit from bundles extracted whole -- clangd on every
/// platform, since its release is a zip, and the Windows Node runtime. Only the
/// single file `activation` later located was ever chmod'd, so a bundle whose
/// entry point is a wrapper script calling a sibling binary was installed
/// broken.
///
/// Only the read and execute bits are taken from the archive. setuid, setgid
/// and the sticky bit are never honoured from a download, and neither is group
/// or other **write**: an archive is free to record `0o777`, and karet execs
/// binaries out of this tree, so honouring that would let any other local user
/// on a shared machine replace a language server.
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

pub(super) fn safe_version(version: &str) -> String {
    version
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;
