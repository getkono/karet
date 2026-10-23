//! Cross-platform development tasks for the karet workspace.

mod readme_svg;

use std::env;
use std::fs::File;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::ExitCode;

use flate2::read::GzDecoder;
use serde::Deserialize;
use tar::Archive;

const RUST_FILE_LINE_LIMIT: usize = 800;

#[derive(Debug, Deserialize)]
struct RustReport {
    name: String,
    stats: CodeStats,
}

#[derive(Debug, Deserialize)]
struct CodeStats {
    code: usize,
}

#[derive(Debug, Default, Deserialize)]
struct Language {
    #[serde(default)]
    reports: Vec<RustReport>,
}

#[derive(Debug, Deserialize)]
struct TokeiOutput {
    #[serde(rename = "Rust")]
    rust: Option<Language>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<MetadataPackage>,
}

#[derive(Debug, Deserialize)]
struct MetadataPackage {
    name: String,
    version: String,
    manifest_path: PathBuf,
    publish: Option<Vec<String>>,
    dependencies: Vec<MetadataDependency>,
}

#[derive(Debug, Deserialize)]
struct MetadataDependency {
    name: String,
    path: Option<PathBuf>,
    req: String,
    kind: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum PublishViolationKind {
    MissingVersion,
    UnpublishedDependency,
}

#[derive(Debug, Eq, PartialEq)]
struct PublishViolation {
    package: String,
    dependency: String,
    kind: PublishViolationKind,
}

#[derive(Debug, Eq, PartialEq)]
struct PackageVersion {
    name: String,
    version: String,
}

#[derive(Debug, Eq, PartialEq)]
struct Offender {
    name: String,
    code: usize,
}

fn rust_file_offenders(reports: &[RustReport], limit: usize) -> Vec<Offender> {
    let mut offenders = reports
        .iter()
        .filter(|report| report.stats.code > limit)
        .map(|report| Offender {
            name: report.name.clone(),
            code: report.stats.code,
        })
        .collect::<Vec<_>>();

    offenders.sort_by(|left, right| {
        right
            .code
            .cmp(&left.code)
            .then_with(|| left.name.cmp(&right.name))
    });
    offenders
}

fn tokei_rust_reports() -> Result<Vec<RustReport>, String> {
    let output = Command::new("tokei")
        .args(["--output", "json", "--type", "Rust", "."])
        .output()
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => {
                "tokei is required; install workspace tools with `mise install`".to_owned()
            },
            _ => format!("failed to run tokei: {error}"),
        })?;
    if !output.status.success() {
        return Err(format!(
            "tokei failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    parse_tokei_reports(&output.stdout)
}

/// The per-file Rust reports in `tokei --output json` output.
fn parse_tokei_reports(json: &[u8]) -> Result<Vec<RustReport>, String> {
    let result = serde_json::from_slice::<TokeiOutput>(json)
        .map_err(|error| format!("failed to parse tokei output: {error}"))?;
    Ok(result.rust.unwrap_or_default().reports)
}

fn check_rust_file_lines() -> ExitCode {
    let reports = match tokei_rust_reports() {
        Ok(reports) => reports,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        },
    };
    let offenders = rust_file_offenders(&reports, RUST_FILE_LINE_LIMIT);
    if offenders.is_empty() {
        println!("All Rust files are within the {RUST_FILE_LINE_LIMIT}-code-line limit.");
        return ExitCode::SUCCESS;
    }

    eprintln!("Rust files exceeding the {RUST_FILE_LINE_LIMIT}-code-line limit:");
    for offender in offenders {
        eprintln!("{}: {} code lines", offender.name, offender.code);
    }
    ExitCode::FAILURE
}

fn is_publishable(package: &MetadataPackage) -> bool {
    package
        .publish
        .as_ref()
        .is_none_or(|registries| !registries.is_empty())
}

fn publish_violations(packages: &[MetadataPackage]) -> Vec<PublishViolation> {
    let mut violations = Vec::new();
    for package in packages.iter().filter(|package| is_publishable(package)) {
        for dependency in package
            .dependencies
            .iter()
            .filter(|dependency| dependency.kind.as_deref() != Some("dev"))
        {
            let Some(path) = dependency.path.as_deref() else {
                continue;
            };
            let Some(target) = packages
                .iter()
                .find(|candidate| candidate.manifest_path.parent() == Some(path))
            else {
                continue;
            };
            if dependency.req == "*" {
                violations.push(PublishViolation {
                    package: package.name.clone(),
                    dependency: dependency.name.clone(),
                    kind: PublishViolationKind::MissingVersion,
                });
            }
            if !is_publishable(target) {
                violations.push(PublishViolation {
                    package: package.name.clone(),
                    dependency: dependency.name.clone(),
                    kind: PublishViolationKind::UnpublishedDependency,
                });
            }
        }
    }
    violations.sort_by(|left, right| {
        (&left.package, &left.dependency, left.kind).cmp(&(
            &right.package,
            &right.dependency,
            right.kind,
        ))
    });
    violations
}

fn cargo_metadata() -> Result<CargoMetadata, String> {
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
        .map_err(|error| format!("failed to run cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("failed to parse cargo metadata: {error}"))
}

fn check_publish_closure() -> ExitCode {
    let metadata = match cargo_metadata() {
        Ok(metadata) => metadata,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        },
    };
    let violations = publish_violations(&metadata.packages);
    if violations.is_empty() {
        println!("Every publishable crate has a publishable, versioned dependency closure.");
        return ExitCode::SUCCESS;
    }

    eprintln!("Invalid dependencies in publishable workspace crates:");
    for violation in violations {
        let reason = match violation.kind {
            PublishViolationKind::MissingVersion => "has no version requirement",
            PublishViolationKind::UnpublishedDependency => "is marked publish = false",
        };
        eprintln!(
            "{} -> {}: {reason}",
            violation.package, violation.dependency
        );
    }
    ExitCode::FAILURE
}

fn registry_lookup_result(success: bool, stderr: &str, package: &str) -> Result<bool, String> {
    if success {
        return Ok(true);
    }
    let missing = format!("could not find `{package}` in registry");
    if stderr.contains(&missing) {
        return Ok(false);
    }
    Err(format!(
        "failed to query crates.io for {package}: {}",
        stderr.trim()
    ))
}

fn is_published(package: &MetadataPackage) -> Result<bool, String> {
    let spec = format!("{}@{}", package.name, package.version);
    let output = Command::new("cargo")
        .args(["info", "--registry", "crates-io", "--color", "never", &spec])
        .output()
        .map_err(|error| format!("failed to run cargo info for {spec}: {error}"))?;
    registry_lookup_result(
        output.status.success(),
        &String::from_utf8_lossy(&output.stderr),
        &spec,
    )
}

fn unpublished_packages(packages: &[MetadataPackage]) -> Result<Vec<PackageVersion>, String> {
    let mut unpublished = Vec::new();
    let mut publishable = packages
        .iter()
        .filter(|package| is_publishable(package))
        .collect::<Vec<_>>();
    publishable.sort_by(|left, right| left.name.cmp(&right.name));
    for package in publishable {
        let spec = format!("{}@{}", package.name, package.version);
        if is_published(package)? {
            println!("Published: {spec}");
        } else {
            println!("Release candidate: {spec}");
            unpublished.push(PackageVersion {
                name: package.name.clone(),
                version: package.version.clone(),
            });
        }
    }
    Ok(unpublished)
}

fn package_directory(package: &PackageVersion) -> String {
    format!("{}-{}", package.name, package.version)
}

fn toml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('\"', "\\\""))
}

fn packaged_workspace_manifest(packages: &[PackageVersion]) -> String {
    let mut manifest = String::from("[workspace]\nresolver = \"3\"\nmembers = [\n");
    for package in packages {
        manifest.push_str("    ");
        manifest.push_str(&toml_string(&format!(
            "sources/{}",
            package_directory(package)
        )));
        manifest.push_str(",\n");
    }
    manifest.push_str("]\n\n[patch.crates-io]\n");
    for package in packages {
        manifest.push_str(&toml_string(&package.name));
        manifest.push_str(" = { path = ");
        manifest.push_str(&toml_string(&format!(
            "sources/{}",
            package_directory(package)
        )));
        manifest.push_str(" }\n");
    }
    manifest
}

fn package_archives(
    workspace: &Path,
    target: &Path,
    packages: &[PackageVersion],
) -> Result<(), String> {
    let mut command = Command::new("cargo");
    command
        .current_dir(workspace)
        .args(["package", "--no-verify", "--allow-dirty", "--target-dir"])
        .arg(target);
    for package in packages {
        command
            .arg("--package")
            .arg(format!("{}@{}", package.name, package.version));
    }
    let status = command
        .status()
        .map_err(|error| format!("failed to run cargo package: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("cargo package failed while assembling release candidates".to_owned())
    }
}

fn extract_archives(
    target: &Path,
    sources: &Path,
    packages: &[PackageVersion],
) -> Result<(), String> {
    std::fs::create_dir_all(sources)
        .map_err(|error| format!("failed to create {}: {error}", sources.display()))?;
    for package in packages {
        let archive_path = target
            .join("package")
            .join(format!("{}.crate", package_directory(package)));
        let archive = File::open(&archive_path)
            .map_err(|error| format!("failed to open {}: {error}", archive_path.display()))?;
        Archive::new(GzDecoder::new(archive))
            .unpack(sources)
            .map_err(|error| format!("failed to extract {}: {error}", archive_path.display()))?;
    }
    Ok(())
}

fn verify_packaged_workspace(workspace: &Path, target: &Path) -> Result<(), String> {
    let status = Command::new("cargo")
        .current_dir(workspace)
        .args(["check", "--workspace", "--all-features", "--target-dir"])
        .arg(target)
        .status()
        .map_err(|error| format!("failed to check packaged workspace: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("packaged release candidates failed to compile".to_owned())
    }
}

fn check_publish_ready() -> Result<(), String> {
    let metadata = cargo_metadata()?;
    let packages = unpublished_packages(&metadata.packages)?;
    if packages.is_empty() {
        println!("Every publishable workspace version is already on crates.io.");
        return Ok(());
    }

    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| "xtask has no workspace parent".to_owned())?;
    let temporary = tempfile::tempdir()
        .map_err(|error| format!("failed to create temporary package workspace: {error}"))?;
    let package_target = temporary.path().join("package-target");
    let sources = temporary.path().join("sources");
    package_archives(workspace, &package_target, &packages)?;
    extract_archives(&package_target, &sources, &packages)?;
    let manifest = packaged_workspace_manifest(&packages);
    std::fs::write(temporary.path().join("Cargo.toml"), manifest)
        .map_err(|error| format!("failed to write packaged workspace manifest: {error}"))?;
    verify_packaged_workspace(
        temporary.path(),
        &workspace.join("target").join("publish-ready"),
    )?;
    println!("Every unpublished crate is package-ready.");
    Ok(())
}

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some(command), None) if command == "file-lines" => check_rust_file_lines(),
        (Some(command), None) if command == "publish-closure" => check_publish_closure(),
        (Some(command), None) if command == "publish-ready" => match check_publish_ready() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            },
        },
        (Some(command), None) if command == "readme-svg" => readme_svg::run(),
        _ => {
            eprintln!(
                "usage: cargo run --package xtask -- \
                 <file-lines|publish-closure|publish-ready|readme-svg>"
            );
            ExitCode::from(2)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(name: &str, code: usize) -> RustReport {
        RustReport {
            name: name.to_owned(),
            stats: CodeStats { code },
        }
    }

    fn package(name: &str, publish: Option<Vec<String>>) -> MetadataPackage {
        MetadataPackage {
            name: name.to_owned(),
            version: "1.0.0".to_owned(),
            manifest_path: PathBuf::from(format!("/workspace/crates/{name}/Cargo.toml")),
            publish,
            dependencies: Vec::new(),
        }
    }

    #[test]
    fn offenders_are_over_limit_and_sorted_by_size_then_name() {
        let reports = vec![
            report("within.rs", 800),
            report("b.rs", 801),
            report("largest.rs", 900),
            report("a.rs", 801),
        ];

        assert_eq!(
            rust_file_offenders(&reports, 800),
            vec![
                Offender {
                    name: "largest.rs".into(),
                    code: 900,
                },
                Offender {
                    name: "a.rs".into(),
                    code: 801,
                },
                Offender {
                    name: "b.rs".into(),
                    code: 801,
                },
            ]
        );
    }

    #[test]
    fn publish_closure_rejects_unversioned_and_unpublished_workspace_dependencies() {
        let mut consumer = package("consumer", None);
        consumer.dependencies = vec![
            MetadataDependency {
                name: "internal".into(),
                path: Some(PathBuf::from("/workspace/crates/internal")),
                req: "*".into(),
                kind: None,
            },
            MetadataDependency {
                name: "published".into(),
                path: Some(PathBuf::from("/workspace/crates/published")),
                req: "^1.0".into(),
                kind: None,
            },
        ];
        let packages = vec![
            consumer,
            package("internal", Some(Vec::new())),
            package("published", None),
        ];

        assert_eq!(
            publish_violations(&packages),
            vec![
                PublishViolation {
                    package: "consumer".into(),
                    dependency: "internal".into(),
                    kind: PublishViolationKind::MissingVersion,
                },
                PublishViolation {
                    package: "consumer".into(),
                    dependency: "internal".into(),
                    kind: PublishViolationKind::UnpublishedDependency,
                },
            ]
        );
    }

    fn dependency(
        name: &str,
        path: Option<&str>,
        req: &str,
        kind: Option<&str>,
    ) -> MetadataDependency {
        MetadataDependency {
            name: name.into(),
            path: path.map(PathBuf::from),
            req: req.into(),
            kind: kind.map(str::to_owned),
        }
    }

    #[test]
    fn publish_closure_skips_dev_unresolved_and_registry_dependencies() {
        let mut consumer = package("consumer", None);
        consumer.dependencies = vec![
            // Dev-dependencies are stripped by `cargo publish`, so an unpublished,
            // unversioned one is allowed.
            dependency(
                "internal",
                Some("/workspace/crates/internal"),
                "*",
                Some("dev"),
            ),
            // A path outside the workspace resolves to no package and is not judged.
            dependency("vendored", Some("/elsewhere/vendored"), "*", None),
            // A registry dependency carries no path.
            dependency("serde", None, "^1", None),
        ];
        let packages = vec![consumer, package("internal", Some(Vec::new()))];

        assert_eq!(publish_violations(&packages), Vec::new());
    }

    #[test]
    fn publish_closure_judges_build_dependencies_like_normal_ones() {
        let mut consumer = package("consumer", None);
        consumer.dependencies = vec![dependency(
            "internal",
            Some("/workspace/crates/internal"),
            "^1.0",
            Some("build"),
        )];
        let packages = vec![consumer, package("internal", Some(Vec::new()))];

        assert_eq!(
            publish_violations(&packages),
            vec![PublishViolation {
                package: "consumer".into(),
                dependency: "internal".into(),
                kind: PublishViolationKind::UnpublishedDependency,
            }]
        );
    }

    #[test]
    fn publish_closure_treats_a_crates_io_registry_list_as_publishable() {
        let crates_io = || Some(vec!["crates-io".to_owned()]);
        // A `publish = ["crates-io"]` consumer is checked ...
        let mut consumer = package("consumer", crates_io());
        consumer.dependencies = vec![
            dependency("internal", Some("/workspace/crates/internal"), "^1.0", None),
            dependency("listed", Some("/workspace/crates/listed"), "^1.0", None),
        ];
        // ... while a `publish = false` crate's own dependencies are not.
        let mut internal = package("internal", Some(Vec::new()));
        internal.dependencies = vec![dependency(
            "listed",
            Some("/workspace/crates/listed"),
            "*",
            None,
        )];
        // ... and a `publish = ["crates-io"]` target satisfies the closure.
        let packages = vec![consumer, internal, package("listed", crates_io())];

        assert_eq!(
            publish_violations(&packages),
            vec![PublishViolation {
                package: "consumer".into(),
                dependency: "internal".into(),
                kind: PublishViolationKind::UnpublishedDependency,
            }]
        );
    }

    /// A fragment of real `tokei 12.1.2 --output json --type Rust` output (the
    /// version `mise.toml` pins), trimmed to two reports.
    const TOKEI_12_1_2_FRAGMENT: &str = r#"{
        "Rust": {
            "blanks": 30,
            "children": {
                "Markdown": [
                    {"name": "src/big.rs", "stats": {"blanks": 1, "blobs": {}, "code": 0, "comments": 6}}
                ]
            },
            "code": 1601,
            "comments": 12,
            "inaccurate": false,
            "reports": [
                {
                    "name": "src/big.rs",
                    "stats": {
                        "blanks": 19,
                        "blobs": {"Markdown": {"blanks": 1, "blobs": {}, "code": 0, "comments": 6}},
                        "code": 801,
                        "comments": 11
                    }
                },
                {
                    "name": "src/fits.rs",
                    "stats": {"blanks": 11, "blobs": {}, "code": 800, "comments": 1}
                }
            ]
        },
        "Total": {
            "blanks": 30,
            "children": {"Rust": []},
            "code": 1601,
            "comments": 12,
            "inaccurate": false,
            "reports": []
        }
    }"#;

    #[test]
    fn tokei_output_yields_the_file_over_the_line_limit() -> Result<(), String> {
        let reports = parse_tokei_reports(TOKEI_12_1_2_FRAGMENT.as_bytes())?;

        assert_eq!(
            rust_file_offenders(&reports, RUST_FILE_LINE_LIMIT),
            vec![Offender {
                name: "src/big.rs".into(),
                code: 801,
            }]
        );
        Ok(())
    }

    #[test]
    fn registry_lookup_distinguishes_missing_versions_from_lookup_failures() {
        assert_eq!(registry_lookup_result(true, "", "demo@1.0.0"), Ok(true));
        assert_eq!(
            registry_lookup_result(
                false,
                "error: could not find `demo@1.0.0` in registry `crates-io`",
                "demo@1.0.0"
            ),
            Ok(false)
        );
        assert_eq!(
            registry_lookup_result(false, "network unavailable", "demo@1.0.0"),
            Err("failed to query crates.io for demo@1.0.0: network unavailable".to_owned())
        );
    }

    #[test]
    fn packaged_manifest_members_and_patches_release_candidates() {
        let packages = vec![
            PackageVersion {
                name: "alpha".to_owned(),
                version: "1.2.3".to_owned(),
            },
            PackageVersion {
                name: "beta-crate".to_owned(),
                version: "0.4.0".to_owned(),
            },
        ];
        assert_eq!(
            packaged_workspace_manifest(&packages),
            concat!(
                "[workspace]\n",
                "resolver = \"3\"\n",
                "members = [\n",
                "    \"sources/alpha-1.2.3\",\n",
                "    \"sources/beta-crate-0.4.0\",\n",
                "]\n\n",
                "[patch.crates-io]\n",
                "\"alpha\" = { path = \"sources/alpha-1.2.3\" }\n",
                "\"beta-crate\" = { path = \"sources/beta-crate-0.4.0\" }\n"
            )
        );
    }
}
