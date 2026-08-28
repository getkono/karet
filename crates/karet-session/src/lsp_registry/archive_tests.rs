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

/// A gzipped tar holding `entries` as `(path, contents, mode, typeflag)`.
///
/// `tar::Header::new_gnu` defaults to a regular file, so the typeflags that
/// matter to the permission mask -- contiguous, FIFO, and one the unpacker has
/// never heard of -- have to be written in explicitly.
fn tar_gz_typed(
    entries: &[(&str, &[u8], u32, tar::EntryType)],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, contents, mode, kind) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(*mode);
        header.set_entry_type(*kind);
        header.set_cksum();
        builder.append_data(&mut header, path, *contents)?;
    }
    let tarball = builder.into_inner()?;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&tarball)?;
    Ok(encoder.finish()?)
}

/// A gzipped tar holding one symlink entry at `name` pointing at `target`.
fn tar_gz_symlink(name: &str, target: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_mode(0o777);
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_link_name(target)?;
    builder.append_data(&mut header, name, std::io::empty())?;
    let tarball = builder.into_inner()?;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&tarball)?;
    Ok(encoder.finish()?)
}

/// A zip holding one symlink entry: the body is the link target and the mode
/// carries `S_IFLNK`, which is exactly how a real toolchain zip records one.
fn zip_symlink(name: &str, target: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer.add_symlink(name, target, zip::write::SimpleFileOptions::default())?;
    writer.start_file(
        "lib/clangd",
        zip::write::SimpleFileOptions::default().unix_permissions(0o755),
    )?;
    writer.write_all(b"real binary\n")?;
    Ok(writer.finish()?.into_inner())
}

fn sha256_of(bytes: &[u8]) -> String {
    format!("{:x}", sha2::Sha256::digest(bytes))
}

/// The mode `name` lands with when an archive of `archive` records it as `mode`.
#[cfg(unix)]
fn extracted_mode(
    archive: Archive,
    name: &str,
    mode: u32,
) -> Result<u32, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let bytes = match archive {
        Archive::Zip => zip_with(name, mode)?,
        _ => tar_gz_with(&[(name, b"payload", mode)])?,
    };
    extract_archive(&bytes, archive, dir.path(), true)?;
    Ok(mode_of(&dir.path().join(name)))
}

/// Both multi-file formats, since a permission rule that holds for one and not
/// the other is the shape this file has already been wrong in once: the tar
/// path is what installs the bundled Node runtime on Linux and macOS.
#[cfg(unix)]
const MULTI_FILE_ARCHIVES: &[(Archive, &str)] =
    &[(Archive::Zip, "zip"), (Archive::TarGzip, "tar.gz")];

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
    for (archive, label) in MULTI_FILE_ARCHIVES {
        assert_eq!(
            extracted_mode(*archive, "bin/setuid", 0o4755)?,
            0o755,
            "{label}"
        );
        assert_eq!(
            extracted_mode(*archive, "bin/sticky", 0o1755)?,
            0o755,
            "{label}"
        );
    }
    Ok(())
}

/// karet execs binaries out of this tree, so an archive that records `0o777`
/// must not leave one writable by every other local user.
///
/// Asserted for both formats. `tar::Entry::unpack_in` applies the header mode
/// with no mask at all, so this held for zip alone while every `.tar.gz` --
/// the Node runtime on both Linux targets and both macOS ones, and
/// `lua-language-server` -- extracted `0o777` verbatim.
#[cfg(unix)]
#[test]
fn an_archive_cannot_make_an_installed_file_group_or_world_writable() -> TestResult {
    for (archive, label) in MULTI_FILE_ARCHIVES {
        assert_eq!(
            extracted_mode(*archive, "bin/server", 0o777)?,
            0o755,
            "{label}"
        );
        assert_eq!(
            extracted_mode(*archive, "share/data", 0o666)?,
            0o644,
            "{label}"
        );
        assert_eq!(
            extracted_mode(*archive, "share/group", 0o664)?,
            0o644,
            "{label}"
        );
    }
    Ok(())
}

/// The mask has to survive every typeflag, not just the one a well-behaved
/// packer writes.
///
/// POSIX requires an unrecognised typeflag to be treated as a regular file, and
/// `tar::Entry::unpack_in` obliges: a contiguous entry (`7`), a FIFO entry (`6`)
/// and any typeflag it does not know are all materialised as ordinary files
/// holding the entry body, with the header mode applied verbatim. Gating the
/// re-chmod on `is_file() || is_dir()` skipped exactly those, so a hostile
/// publisher shipping every file as typeflag `7` with mode `0o777` still landed
/// a world-writable tree -- and karet execs binaries out of it.
#[cfg(unix)]
#[test]
fn an_unusual_tar_entry_type_is_masked_like_a_regular_file() -> TestResult {
    for kind in [
        tar::EntryType::Continuous,
        tar::EntryType::Fifo,
        tar::EntryType::Char,
        tar::EntryType::new(b'X'),
    ] {
        for (recorded, expected) in [(0o777, 0o755), (0o666, 0o644), (0o4755, 0o755)] {
            let dir = tempfile::tempdir()?;
            let bytes = tar_gz_typed(&[("bin/server", b"payload", recorded, kind)])?;
            extract_archive(&bytes, Archive::TarGzip, dir.path(), true)?;
            let path = dir.path().join("bin/server");
            assert!(path.exists(), "{kind:?} wrote nothing");
            assert_eq!(mode_of(&path), expected, "{kind:?} recorded {recorded:o}");
        }
    }
    Ok(())
}

/// A pax global header is a real entry in real tarballs (`git archive` writes
/// one), and the unpacker reports it as handled while deliberately writing
/// nothing. Re-chmod'ing on the header's word alone would fail the whole
/// extraction on a file that was never created.
#[cfg(unix)]
#[test]
fn a_metadata_only_tar_entry_does_not_fail_the_extraction() -> TestResult {
    let dir = tempfile::tempdir()?;
    let bytes = tar_gz_typed(&[
        (
            "pax_global_header",
            b"52 comment=0000\n",
            0o666,
            tar::EntryType::XGlobalHeader,
        ),
        ("bin/server", b"payload", 0o777, tar::EntryType::Regular),
    ])?;
    extract_archive(&bytes, Archive::TarGzip, dir.path(), true)?;
    assert_eq!(mode_of(&dir.path().join("bin/server")), 0o755);
    assert!(!dir.path().join("pax_global_header").exists());
    Ok(())
}

/// A zip records a symlink as an entry whose body is the link target. Written
/// as a regular file, `bin/clangd -> ../lib/clangd` becomes a 13-byte text file
/// that `find_file_named` hands back as the launch command -- and the restored
/// mode makes it executable, so karet execs text.
#[cfg(unix)]
#[test]
fn a_zipped_symlink_is_never_written_as_a_regular_file() -> TestResult {
    let dir = tempfile::tempdir()?;
    extract_archive(
        &zip_symlink("bin/clangd", "../lib/clangd")?,
        Archive::Zip,
        dir.path(),
        true,
    )?;
    let link = dir.path().join("bin/clangd");
    assert!(!link.is_file(), "a link target was written as a file");
    // `bin/clangd` would have sorted before `lib/clangd` at the same depth and
    // become the recorded launch command.
    assert_eq!(
        find_file_named(dir.path(), "clangd"),
        Some(dir.path().join("lib/clangd"))
    );
    Ok(())
}

/// An absolute target is the same shape and must not become a file either.
#[cfg(unix)]
#[test]
fn a_zipped_symlink_pointing_outside_writes_nothing() -> TestResult {
    let dir = tempfile::tempdir()?;
    extract_archive(
        &zip_symlink("evil", "/etc/passwd")?,
        Archive::Zip,
        dir.path(),
        true,
    )?;
    assert!(!dir.path().join("evil").exists());
    Ok(())
}

/// The tar path keeps its links: the Node runtime ships `bin/npm` as one, and
/// `tar` validates each parent against the destination before writing through
/// it. Dropping them here would break every npm-backed provider.
#[cfg(unix)]
#[test]
fn a_tarred_symlink_is_recreated_as_a_link() -> TestResult {
    let dir = tempfile::tempdir()?;
    extract_archive(
        &tar_gz_symlink("bin/npm", "../lib/node_modules/npm/bin/npm-cli.js")?,
        Archive::TarGzip,
        dir.path(),
        true,
    )?;
    let link = dir.path().join("bin/npm");
    assert!(std::fs::symlink_metadata(&link)?.file_type().is_symlink());
    assert_eq!(
        std::fs::read_link(&link)?,
        Path::new("../lib/node_modules/npm/bin/npm-cli.js")
    );
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
    let error = extract_archive(&bytes, Archive::Zip, dir.path(), true)
        .err()
        .unwrap_or_default();
    // Asserting the refusal, not merely the absence: a sanitising writer could
    // have turned the entry into `escaped`, which would land inside the
    // destination and leave "nothing escaped" true but nothing proven.
    assert!(error.contains("unsafe"), "{error}");
    assert!(!dir.path().join("../escaped").exists());
    Ok(())
}

/// The `tar` crate refuses to *build* a traversing entry, so the header is
/// hand-rolled: this guard exists for archives karet did not create, and only a
/// hostile one can exercise it.
fn hostile_tar_gz(name: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    hostile_tar_gz_sized(name, 0)
}

/// As above, with `size` declared in the header.
///
/// A tar entry's size is authoritative, so a bomb is refused off the header
/// alone -- no gigabyte has to be decompressed, and none is written here.
fn hostile_tar_gz_sized(name: &str, size: u64) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut header = [0_u8; 512];
    header[..name.len()].copy_from_slice(name.as_bytes());
    header[100..107].copy_from_slice(b"0000644"); // mode
    header[108..115].copy_from_slice(b"0000000"); // uid
    header[116..123].copy_from_slice(b"0000000"); // gid
    header[124..135].copy_from_slice(format!("{size:011o}").as_bytes()); // size
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

// --- expansion limits -----------------------------------------------------

/// Compression ratios are unbounded: a 1 MB archive expanded to 1 GB on disk
/// with nothing to refuse it, which is a disk-filling install away from a
/// hostile or merely corrupt release.
#[test]
fn an_archive_that_expands_past_the_limit_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let error = extract_archive(
        &hostile_tar_gz_sized("bomb.bin", MAX_EXTRACTED_BYTES + 1)?,
        Archive::TarGzip,
        dir.path(),
        true,
    )
    .err()
    .unwrap_or_default();
    assert!(error.contains("extraction limit"), "{error}");
    assert!(!dir.path().join("bomb.bin").exists());
    Ok(())
}

/// The zip path is driven with a deliberately small budget: writing half a
/// gigabyte to prove the constant would only make the suite slow.
#[test]
fn a_zip_is_refused_once_its_budget_runs_out() -> TestResult {
    let dir = tempfile::tempdir()?;
    // `bin/server` is 18 bytes and its sibling 5, so the first entry fits and
    // the pair does not.
    let mut budget = Budget { remaining: 20 };
    let error = extract_zip(
        &zip_with("bin/server", 0o755)?,
        dir.path(),
        true,
        &mut budget,
    )
    .err()
    .unwrap_or_default();
    assert!(error.contains("extraction limit"), "{error}");
    assert!(
        dir.path().join("bin/server").is_file(),
        "the first entry fit"
    );
    Ok(())
}

/// A declared size is the archive's claim, so it is only ever trusted to refuse
/// early. What actually lands is what the budget charges.
#[test]
fn a_stream_longer_than_it_declared_is_still_refused() -> TestResult {
    let mut budget = Budget { remaining: 8 };
    let mut sink = Vec::new();
    // Admitted on a declared size of nothing, then caught by what it writes.
    assert!(budget.admits(0).is_ok());
    let error = budget
        .copy(&mut &b"far more than eight bytes"[..], &mut sink)
        .err()
        .unwrap_or_default();
    assert!(error.contains("extraction limit"), "{error}");
    Ok(())
}

/// A whole archive shares one budget: many small entries are a bomb too.
#[test]
fn the_extraction_budget_is_shared_across_entries() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut budget = Budget { remaining: 20 };
    // Two eleven-byte entries: the first fits, the pair does not.
    let bytes = tar_gz_with(&[("a", b"12345678901", 0o644), ("b", b"12345678901", 0o644)])?;
    let error = extract_tar(
        flate2::read::GzDecoder::new(&bytes[..]),
        dir.path(),
        true,
        &mut budget,
    )
    .err()
    .unwrap_or_default();
    assert!(error.contains("extraction limit"), "{error}");
    assert!(dir.path().join("a").is_file(), "the first entry fit");
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

/// `tar` preserves symlinks verbatim, so the search runs over links an archive
/// chose. One pointing out of the payload must never become a launch command.
#[cfg(unix)]
#[test]
fn a_symlink_escaping_the_payload_is_never_the_launch_command() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().join("payload");
    std::fs::create_dir_all(root.join("bin"))?;
    let outside = dir.path().join("passwd");
    std::fs::write(&outside, b"root:x:0:0")?;
    std::os::unix::fs::symlink(&outside, root.join("bin/node"))?;

    assert_eq!(
        find_file_named(&root, "node"),
        None,
        "a link out of the payload must not be selected"
    );
    Ok(())
}

/// The bundled Node runtime ships `bin/npm` as a link into its own `lib`, so
/// refusing links outright would break every npm-backed provider.
#[cfg(unix)]
#[test]
fn a_symlink_that_stays_inside_the_payload_is_still_found() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    std::fs::create_dir_all(root.join("bin"))?;
    std::fs::create_dir_all(root.join("lib"))?;
    std::fs::write(root.join("lib/npm-cli.js"), b"#!/usr/bin/env node")?;
    std::os::unix::fs::symlink(root.join("lib/npm-cli.js"), root.join("bin/npm"))?;

    assert_eq!(find_file_named(root, "npm"), Some(root.join("bin/npm")));
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

/// Stripping the separator is not enough: `.` and `..` are made entirely of
/// pass-through characters, and neither names a new directory. `versions/..`
/// *is* the provider root -- an install that no-ops because the destination
/// already exists, and a retirement that `remove_dir_all`s the whole provider,
/// journals included.
#[test]
fn a_version_is_never_a_navigation_name() {
    for version in ["..", ".", "", "v..", "../.."] {
        let name = safe_version(version);
        assert!(
            !matches!(name.as_str(), "" | "." | ".."),
            "{version:?} became {name:?}"
        );
    }
    assert_eq!(safe_version(".."), "__");
    assert_eq!(safe_version("."), "_");
    assert_eq!(safe_version(""), "_");
    // A tag that merely starts with dots still names a directory of its own.
    assert_eq!(safe_version("..1"), "..1");
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
        None,
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
        None,
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
            None,
            |_, _| {},
        )
        .is_ok()
    );
    Ok(())
}

/// The digest verifies nothing until the body is complete, so the ceiling is
/// the only thing bounding a host that streams for ever.
#[test]
fn a_body_that_outgrows_its_declared_size_is_refused_mid_stream() -> TestResult {
    // No advertised length, so the refusal can only come from the running total.
    let oversized = vec![0_u8; (DECLARED_SIZE_SLACK as usize) + 4096];
    let server = responding_server(oversized, None)?;
    let client = Client::builder().build()?;
    let error = download_verified(
        &client,
        &format!("http://{}/asset", server.address),
        &sha256_of(b"never reached"),
        Some(64),
        |_, _| {},
    )
    .err()
    .unwrap_or_default();
    assert!(error.contains("download limit"), "{error}");
    Ok(())
}

/// An absurd advertised length is refused before a byte of it is read.
#[test]
fn a_body_advertising_more_than_the_ceiling_is_refused_up_front() -> TestResult {
    let server = responding_server(b"tiny".to_vec(), Some(MAX_DOWNLOAD_BYTES + 1))?;
    let client = Client::builder().build()?;
    let error = download_verified(
        &client,
        &format!("http://{}/asset", server.address),
        &sha256_of(b"tiny"),
        None,
        |_, _| {},
    )
    .err()
    .unwrap_or_default();
    assert!(error.contains("download limit"), "{error}");
    Ok(())
}

/// GitHub declares an exact asset size; nodejs.org declares none.
#[test]
fn the_ceiling_follows_the_publisher_but_never_exceeds_the_absolute_limit() {
    assert_eq!(download_ceiling(None), MAX_DOWNLOAD_BYTES);
    assert_eq!(download_ceiling(Some(1_000)), 1_000 + DECLARED_SIZE_SLACK);
    assert_eq!(download_ceiling(Some(u64::MAX)), MAX_DOWNLOAD_BYTES);
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
    let advertised = payload.len() as u64;
    responding_server(payload, Some(advertised))
}

/// As above, but free to advertise a length it does not send -- or none at all,
/// which is how a chunked or connection-terminated body arrives.
fn responding_server(
    payload: Vec<u8>,
    advertised: Option<u64>,
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
        let length = advertised
            .map(|length| format!("Content-Length: {length}\r\n"))
            .unwrap_or_default();
        let header = format!("HTTP/1.1 200 OK\r\n{length}Connection: close\r\n\r\n");
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(&payload);
        let _ = stream.flush();
    });
    Ok(FixedResponseServer { address })
}
