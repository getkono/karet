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
                    let mut file = File::create(output).map_err(|error| error.to_string())?;
                    std::io::copy(&mut entry, &mut file).map_err(|error| error.to_string())?;
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

pub(super) fn find_file_named(root: &Path, name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.file_name().is_some_and(|candidate| candidate == name) {
            return Some(path);
        }
        if path.is_dir()
            && let Some(found) = find_file_named(&path, name)
        {
            return Some(found);
        }
    }
    None
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
