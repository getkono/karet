//! Release-discovery tests: the shapes upstream registries actually publish.

use super::*;

/// The `(os, arch)` pairs karet claims a managed Node runtime for, each with the
/// download suffix and the `files` manifest key it must resolve to.
const NODE_TARGETS: [(&str, &str, &str, &str); 5] = [
    ("linux", "x86_64", "linux-x64.tar.gz", "linux-x64"),
    ("linux", "aarch64", "linux-arm64.tar.gz", "linux-arm64"),
    ("macos", "x86_64", "darwin-x64.tar.gz", "osx-x64-tar"),
    ("macos", "aarch64", "darwin-arm64.tar.gz", "osx-arm64-tar"),
    ("windows", "x86_64", "win-x64.zip", "win-x64-zip"),
];

/// A release whose `files` array carries the keys nodejs.org really publishes,
/// captured from the v24.20.0 entry of the dist index.
fn node_release() -> NodeRelease {
    NodeRelease {
        version: "v24.20.0".to_owned(),
        lts: serde_json::Value::String("Krypton".to_owned()),
        files: [
            "aix-ppc64",
            "headers",
            "linux-arm64",
            "linux-ppc64le",
            "linux-s390x",
            "linux-x64",
            "linux-x64-musl",
            "osx-arm64-tar",
            "osx-x64-pkg",
            "osx-x64-tar",
            "src",
            "win-arm64-7z",
            "win-arm64-zip",
            "win-x64-7z",
            "win-x64-exe",
            "win-x64-msi",
            "win-x64-zip",
        ]
        .map(str::to_owned)
        .to_vec(),
    }
}

#[test]
fn npm_latest_metadata_exposes_the_published_executable() -> Result<(), serde_json::Error> {
    let metadata: NpmMetadata = serde_json::from_str(
        r#"{
            "name": "typescript-language-server",
            "version": "5.3.0",
            "bin": {
                "typescript-language-server": "lib/cli.mjs"
            }
        }"#,
    )?;

    assert_eq!(metadata.version, "5.3.0");
    assert_eq!(
        metadata
            .bin
            .path("typescript-language-server", "typescript-language-server"),
        Some("lib/cli.mjs")
    );
    Ok(())
}

/// npm allows `bin` to be a bare string for a single-executable package. That
/// shape used to fail the whole document, killing discovery with a parse error.
#[test]
fn npm_metadata_accepts_a_single_string_bin() -> Result<(), serde_json::Error> {
    let metadata: NpmMetadata =
        serde_json::from_str(r#"{"version": "1.0.0", "bin": "./out/cli.js"}"#)?;
    assert_eq!(
        metadata
            .bin
            .path("some-language-server", "some-language-server"),
        Some("./out/cli.js")
    );
    // The single binary is named after the package, so nothing else matches it.
    assert_eq!(
        metadata.bin.path("some-language-server", "other-binary"),
        None
    );
    Ok(())
}

/// A scoped package publishes its single binary under the unscoped name.
#[test]
fn npm_metadata_unscopes_a_single_string_bin() -> Result<(), serde_json::Error> {
    let metadata: NpmMetadata =
        serde_json::from_str(r#"{"version": "2.0.0", "bin": "bin/server.js"}"#)?;
    assert_eq!(
        metadata
            .bin
            .path("@scope/language-server", "language-server"),
        Some("bin/server.js")
    );
    Ok(())
}

#[test]
fn npm_metadata_without_a_bin_resolves_nothing_instead_of_failing() -> Result<(), serde_json::Error>
{
    let metadata: NpmMetadata = serde_json::from_str(r#"{"version": "1.0.0"}"#)?;
    assert_eq!(metadata.bin.path("pkg", "pkg"), None);
    Ok(())
}

#[test]
fn npm_executable_paths_must_stay_inside_the_package() {
    assert!(safe_relative_path("lib/cli.mjs"));
    assert!(safe_relative_path("./bin/nodeServer.js"));
    assert!(!safe_relative_path("../outside.js"));
    assert!(!safe_relative_path("/tmp/outside.js"));
    assert!(!safe_relative_path("./"));
    assert!(!safe_relative_path(""));
}

#[test]
fn every_supported_platform_resolves_a_node_download() {
    for (os, arch, suffix, _) in NODE_TARGETS {
        assert_eq!(
            node_platform(os, arch).map(|platform| platform.suffix),
            Some(suffix),
            "{os}-{arch} has no Node download suffix"
        );
    }
    assert!(node_platform("linux", "riscv64").is_none());
}

/// Node names an archive `darwin-arm64.tar.gz` but lists its availability as
/// `osx-arm64-tar`, and `win-x64.zip` as `win-x64-zip`. The manifest key is
/// therefore not derivable by trimming the archive extension off the suffix —
/// doing that declared every macOS and Windows build missing, which made all
/// twelve npm-backed providers uninstallable on both platforms.
#[test]
fn every_supported_platform_is_found_in_the_published_file_manifest() {
    let release = node_release();
    for (os, arch, suffix, key) in NODE_TARGETS {
        assert_eq!(
            node_platform(os, arch).map(|platform| platform.manifest_key),
            Some(key),
            "{os}-{arch} looks for the wrong key in Node's files array"
        );
        assert!(
            release.files.iter().any(|candidate| candidate == key),
            "{os}-{arch} looks for {key}, which Node does not publish"
        );
        let resolved = node_asset(&release, os, arch);
        assert_eq!(
            resolved.as_ref().map(|(file, _)| file.as_str()),
            Ok(format!("node-v24.20.0-{suffix}").as_str()),
            "{os}-{arch} resolves no Node asset"
        );
    }
}

#[test]
fn a_windows_node_runtime_arrives_as_a_zip_and_the_rest_as_tarballs() {
    let release = node_release();
    for (os, arch, suffix, _) in NODE_TARGETS {
        let archive = node_asset(&release, os, arch).map(|(_, archive)| archive);
        let zipped = matches!(archive, Ok(Archive::Zip));
        let tarred = matches!(archive, Ok(Archive::TarGzip));
        assert_eq!(
            (zipped, tarred),
            (suffix.ends_with(".zip"), !suffix.ends_with(".zip")),
            "{os}-{arch} resolves the wrong archive kind for {suffix}"
        );
    }
}

#[test]
fn an_unsupported_platform_names_itself_rather_than_a_missing_file() {
    let error = node_asset(&node_release(), "freebsd", "x86_64")
        .err()
        .unwrap_or_default();
    assert!(error.contains("freebsd"), "{error}");
}

/// A platform karet supports whose build the release genuinely lacks must still
/// be reported as missing, so a bad Node release cannot be installed blindly.
#[test]
fn a_platform_missing_from_the_manifest_is_still_refused() {
    let mut release = node_release();
    release.files.retain(|file| file != "osx-arm64-tar");
    let error = node_asset(&release, "macos", "aarch64")
        .err()
        .unwrap_or_default();
    assert!(error.contains("osx-arm64-tar"), "{error}");
}

/// TypeScript 7 is a ground-up rewrite whose `lib` directory contains no
/// `tsserver.js`, so every server that drives tsserver broke the moment it
/// became `latest`. The companion is pinned to 5 for that reason, and the
/// selection has to be numeric: `5.10.0` is newer than `5.9.3`, which string
/// ordering gets backwards.
#[test]
fn a_pinned_companion_takes_the_newest_release_in_its_major() {
    let versions = [
        "4.9.5",
        "5.0.4",
        "5.9.3",
        "5.10.0",
        "5.11.0-beta",
        "6.0.0-beta",
        "7.0.2",
    ];
    assert_eq!(
        highest_stable_in_major(versions.into_iter(), 5).as_deref(),
        Some("5.10.0")
    );
    assert_eq!(
        highest_stable_in_major(versions.into_iter(), 7).as_deref(),
        Some("7.0.2")
    );
    assert_eq!(highest_stable_in_major(versions.into_iter(), 9), None);
}

#[test]
fn a_prerelease_is_never_selected_even_when_it_is_the_only_candidate() {
    let versions = ["5.0.0-rc", "5.0.0-beta.1"];
    assert_eq!(highest_stable_in_major(versions.into_iter(), 5), None);
}

/// Every server that drives tsserver must pin, or it silently gets TypeScript 7
/// the next time the catalogue is touched.
#[test]
fn servers_needing_typescript_pin_it_rather_than_taking_latest() {
    for recipe in managed_recipes() {
        let ManagedSource::Npm {
            companion: Some(companion),
            ..
        } = recipe.source
        else {
            continue;
        };
        assert_eq!(
            (companion.package, companion.major),
            ("typescript", Some(5)),
            "{} takes an unpinned companion",
            recipe.server
        );
    }
    // Both are known to need it; a third arriving unpinned should be noticed.
    let with_companion = managed_recipes()
        .iter()
        .filter(|recipe| {
            matches!(
                recipe.source,
                ManagedSource::Npm {
                    companion: Some(_),
                    ..
                }
            )
        })
        .map(|recipe| recipe.server)
        .collect::<Vec<_>>();
    assert_eq!(
        with_companion,
        vec!["typescript-language-server", "astro-language-server"]
    );
}
