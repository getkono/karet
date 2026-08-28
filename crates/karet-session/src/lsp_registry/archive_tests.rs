//! Download verification and archive extraction.
//!
//! Fixtures are built in-test from the `zip`, `tar` and `flate2` dependencies
//! the extractor already uses, so there are no binary blobs in the repository
//! and each test states the archive shape it is about.

use std::io::Write as _;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).map_or(0, |metadata| metadata.permissions().mode() & 0o777)
}

/// A zip holding one entry at `name` with `mode`, plus an inert sibling.
fn zip_with(name: &str, mode: u32) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().unix_permissions(mode);
    writer.start_file(name, options)?;
    writer.write_all(b"#!/bin/sh\necho hi\n")?;
    writer.start_file("share/readme.txt", zip::write::SimpleFileOptions::default())?;
    writer.write_all(b"docs\n")?;
    Ok(writer.finish()?.into_inner())
}

/// A gzipped tar holding `entries` as `(path, contents, mode)`.
fn tar_gz_with(entries: &[(&str, &[u8], u32)]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, contents, mode) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(*mode);
        header.set_cksum();
        builder.append_data(&mut header, path, *contents)?;
    }
    let tarball = builder.into_inner()?;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&tarball)?;
    Ok(encoder.finish()?)
}

fn sha256_of(bytes: &[u8]) -> String {
    format!("{:x}", sha2::Sha256::digest(bytes))
}

// --- extraction ----------------------------------------------------------

/// A bundle is extracted whole and then run in place, so an entry that was
/// executable in the archive has to still be executable on disk. Zip extraction
/// wrote every entry with the default mode, which silently broke clangd (a zip
/// on every platform) and any bundle whose entry point is a wrapper script.
#[cfg(unix)]
#[test]
fn a_zipped_bundle_keeps_its_executable_bit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let bytes = zip_with("clangd_20/bin/clangd", 0o755)?;
    extract_archive(&bytes, Archive::Zip, dir.path(), true)?;
    assert_eq!(mode_of(&dir.path().join("clangd_20/bin/clangd")), 0o755);
    assert_eq!(mode_of(&dir.path().join("share/readme.txt")) & 0o111, 0);
    Ok(())
}

/// Downloaded archives are not trusted to hand out elevated bits.
#[cfg(unix)]
#[test]
fn setuid_and_sticky_bits_are_never_honoured() -> TestResult {
    let dir = tempfile::tempdir()?;
    let bytes = zip_with("bin/server", 0o4755)?;
    extract_archive(&bytes, Archive::Zip, dir.path(), true)?;
    assert_eq!(mode_of(&dir.path().join("bin/server")), 0o755);
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_tarred_bundle_keeps_its_executable_bit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let bytes = tar_gz_with(&[
        ("node-v24/bin/node", b"binary", 0o755),
        ("node-v24/README.md", b"docs", 0o644),
    ])?;
    extract_archive(&bytes, Archive::TarGzip, dir.path(), true)?;
    assert_eq!(mode_of(&dir.path().join("node-v24/bin/node")), 0o755);
    Ok(())
}

#[test]
fn a_single_executable_is_extracted_and_made_runnable() -> TestResult {
    let dir = tempfile::tempdir()?;
    let bytes = tar_gz_with(&[("texlab-1.2/texlab", b"binary", 0o644)])?;
    extract_executable(&bytes, Archive::TarGzip, "texlab", dir.path())?;
    let extracted = dir.path().join("texlab");
    assert!(extracted.is_file());
    #[cfg(unix)]
    assert_eq!(mode_of(&extracted) & 0o111, 0o111);
    Ok(())
}

#[test]
fn an_archive_missing_the_executable_says_so() -> TestResult {
    let dir = tempfile::tempdir()?;
    let bytes = tar_gz_with(&[("other", b"x", 0o755)])?;
    let error = extract_executable(&bytes, Archive::TarGzip, "texlab", dir.path())
        .err()
        .unwrap_or_default();
    assert!(error.contains("texlab"), "{error}");
    Ok(())
}

#[test]
fn a_zip_escaping_its_destination_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let bytes = zip_with("../escaped", 0o755)?;
    // `enclosed_name` rejects the entry, so nothing is written outside.
    let _ = extract_archive(&bytes, Archive::Zip, dir.path(), true);
    assert!(!dir.path().join("../escaped").exists());
    Ok(())
}

/// The `tar` crate refuses to *build* a traversing entry, so the header is
/// hand-rolled: this guard exists for archives karet did not create, and only a
/// hostile one can exercise it.
fn hostile_tar_gz(name: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut header = [0_u8; 512];
    header[..name.len()].copy_from_slice(name.as_bytes());
    header[100..107].copy_from_slice(b"0000644"); // mode
    header[108..115].copy_from_slice(b"0000000"); // uid
    header[116..123].copy_from_slice(b"0000000"); // gid
    header[124..135].copy_from_slice(b"00000000000"); // size: empty
    header[136..147].copy_from_slice(b"00000000000"); // mtime
    header[148..156].copy_from_slice(b"        "); // checksum placeholder
    header[156] = b'0'; // regular file
    let checksum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
    let encoded = format!("{checksum:06o}\0 ");
    header[148..156].copy_from_slice(encoded.as_bytes());

    let mut tarball = header.to_vec();
    tarball.extend_from_slice(&[0_u8; 1024]); // end-of-archive marker
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&tarball)?;
    Ok(encoder.finish()?)
}

#[test]
fn a_tar_escaping_its_destination_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let error = extract_archive(
        &hostile_tar_gz("../escaped")?,
        Archive::TarGzip,
        dir.path(),
        true,
    )
    .err()
    .unwrap_or_default();
    assert!(error.contains("unsafe"), "{error}");
    assert!(!dir.path().join("../escaped").exists());
    Ok(())
}

#[test]
fn a_tar_with_an_absolute_path_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let error = extract_archive(
        &hostile_tar_gz("/etc/escaped")?,
        Archive::TarGzip,
        dir.path(),
        true,
    )
    .err()
    .unwrap_or_default();
    assert!(error.contains("unsafe"), "{error}");
    Ok(())
}

#[test]
fn a_single_file_payload_is_not_treated_as_an_archive() {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(_) => return,
    };
    for archive in [Archive::Raw, Archive::Gzip] {
        assert!(extract_archive(b"payload", archive, dir.path(), true).is_err());
    }
}

// --- locating the installed executable ------------------------------------

/// The located file becomes the command karet records and launches, so a name
/// appearing twice must not resolve differently on different machines.
/// `read_dir` order is the filesystem's, not alphabetical.
#[test]
fn the_shallowest_match_wins_and_ties_break_alphabetically() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    for nested in ["b/deep/nested", "a", "b"] {
        std::fs::create_dir_all(root.join(nested))?;
    }
    std::fs::write(root.join("b/deep/nested/node"), b"vendored")?;
    std::fs::write(root.join("a/node"), b"real")?;
    std::fs::write(root.join("b/node"), b"other")?;

    let found = find_file_named(root, "node").ok_or("nothing found")?;
    assert_eq!(found, root.join("a/node"), "shallowest, then alphabetical");
    Ok(())
}

/// A directory sharing the name must not shadow the file.
#[test]
fn a_directory_with_the_target_name_is_not_mistaken_for_it() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    std::fs::create_dir_all(root.join("include/node"))?;
    std::fs::create_dir_all(root.join("bin"))?;
    std::fs::write(root.join("bin/node"), b"binary")?;
    assert_eq!(find_file_named(root, "node"), Some(root.join("bin/node")));
    Ok(())
}

#[test]
fn a_missing_name_is_reported_rather_than_guessed() -> TestResult {
    let dir = tempfile::tempdir()?;
    std::fs::create_dir_all(dir.path().join("bin"))?;
    assert_eq!(find_file_named(dir.path(), "node"), None);
    Ok(())
}

// --- version directory naming ---------------------------------------------

#[test]
fn a_version_can_never_escape_the_provider_directory() {
    assert_eq!(safe_version("1.2.3"), "1.2.3");
    assert_eq!(safe_version("5.6.0+node-24.20.0"), "5.6.0_node-24.20.0");
    assert!(!safe_version("../../etc").contains('/'));
    assert!(!safe_version("a/b").contains('/'));
}

// --- download verification -------------------------------------------------

#[test]
fn a_verified_download_matches_its_publisher_digest() -> TestResult {
    let payload = b"language server payload";
    let server = fixed_response_server(payload.to_vec())?;
    let client = Client::builder().build()?;
    let bytes = download_verified(
        &client,
        &format!("http://{}/asset", server.address),
        &sha256_of(payload),
        |_, _| {},
    )?;
    assert_eq!(bytes, payload);
    Ok(())
}

/// The digest is the only thing standing between a download and execution.
#[test]
fn a_download_whose_digest_does_not_match_is_refused() -> TestResult {
    let server = fixed_response_server(b"tampered".to_vec())?;
    let client = Client::builder().build()?;
    let error = download_verified(
        &client,
        &format!("http://{}/asset", server.address),
        &sha256_of(b"expected"),
        |_, _| {},
    )
    .err()
    .unwrap_or_default();
    assert!(error.contains("SHA-256 mismatch"), "{error}");
    Ok(())
}

#[test]
fn digests_compare_case_insensitively() -> TestResult {
    let payload = b"payload";
    let server = fixed_response_server(payload.to_vec())?;
    let client = Client::builder().build()?;
    assert!(
        download_verified(
            &client,
            &format!("http://{}/asset", server.address),
            &sha256_of(payload).to_uppercase(),
            |_, _| {},
        )
        .is_ok()
    );
    Ok(())
}

/// A loopback server returning fixed bytes once.
///
/// Local and hermetic: this exercises the verification, not the network, and
/// deliberately does not reach any real host.
struct FixedResponseServer {
    address: std::net::SocketAddr,
}

fn fixed_response_server(
    payload: Vec<u8>,
) -> Result<FixedResponseServer, Box<dyn std::error::Error>> {
    use std::io::Read as _;
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let address = listener.local_addr()?;
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request);
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        );
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(&payload);
        let _ = stream.flush();
    });
    Ok(FixedResponseServer { address })
}
