//! Explicit release discovery for the built-in provider catalogue.

use reqwest::blocking::Client;
use serde::Deserialize;

use crate::api::LanguageServerId;

#[derive(Clone, Copy)]
pub(super) struct ManagedRecipe {
    pub(super) server: &'static str,
    source: ManagedSource,
    pub(super) arguments: &'static [&'static str],
}

#[derive(Clone, Copy)]
enum ManagedSource {
    Github {
        repository: &'static str,
    },
    Npm {
        package: &'static str,
        companion: Option<&'static str>,
        entrypoints: &'static [&'static str],
    },
}

const MANAGED_RECIPES: &[ManagedRecipe] = &[
    ManagedRecipe {
        server: "rust-analyzer",
        source: ManagedSource::Github {
            repository: "rust-lang/rust-analyzer",
        },
        arguments: &[],
    },
    ManagedRecipe {
        server: "typescript-language-server",
        source: ManagedSource::Npm {
            package: "typescript-language-server",
            companion: Some("typescript"),
            entrypoints: &["cli.mjs", "typescript-language-server.js"],
        },
        arguments: &["--stdio"],
    },
    ManagedRecipe {
        server: "pyright",
        source: ManagedSource::Npm {
            package: "pyright",
            companion: None,
            entrypoints: &["langserver.index.js"],
        },
        arguments: &["--stdio"],
    },
    ManagedRecipe {
        server: "ruff",
        source: ManagedSource::Github {
            repository: "astral-sh/ruff",
        },
        arguments: &["server"],
    },
    ManagedRecipe {
        server: "texlab",
        source: ManagedSource::Github {
            repository: "latex-lsp/texlab",
        },
        arguments: &[],
    },
];

pub(super) fn managed_recipe(server: &LanguageServerId) -> Option<ManagedRecipe> {
    MANAGED_RECIPES
        .iter()
        .find(|recipe| recipe.server == server.key())
        .copied()
}

pub(super) fn managed_servers() -> Vec<LanguageServerId> {
    MANAGED_RECIPES
        .iter()
        .map(|recipe| LanguageServerId::new(recipe.server))
        .collect()
}

#[derive(Clone)]
pub(super) struct Release {
    pub(super) server: LanguageServerId,
    pub(super) version: String,
    /// Active version observed during discovery. `None` for a first install.
    pub(super) from_version: Option<String>,
    pub(super) kind: ReleaseKind,
    pub(super) download_bytes: Option<u64>,
}

#[derive(Clone)]
pub(super) enum ReleaseKind {
    Standalone {
        url: String,
        sha256: String,
        archive: Archive,
        executable_name: String,
        arguments: &'static [&'static str],
    },
    Npm {
        package: String,
        companion: Option<(String, String)>,
        entrypoints: &'static [&'static str],
        arguments: &'static [&'static str],
        node_version: String,
        node_url: String,
        node_sha256: String,
        node_archive: Archive,
    },
}

#[derive(Clone, Copy)]
pub(super) enum Archive {
    Gzip,
    TarGzip,
    Zip,
}

impl Release {
    pub(super) fn active_version(&self) -> String {
        match &self.kind {
            ReleaseKind::Standalone { .. } => self.version.clone(),
            ReleaseKind::Npm {
                companion,
                node_version,
                ..
            } => companion.as_ref().map_or_else(
                || {
                    format!(
                        "{}+node-{}",
                        self.version,
                        node_version.trim_start_matches('v')
                    )
                },
                |(package, version)| {
                    format!(
                        "{}+{package}-{version}+node-{}",
                        self.version,
                        node_version.trim_start_matches('v')
                    )
                },
            ),
        }
    }
}

pub(super) fn discover(client: &Client, server: LanguageServerId) -> Result<Release, String> {
    let recipe = managed_recipe(&server).ok_or_else(|| {
        format!(
            "{} is available from the project or PATH but has no managed installer",
            server.display_name()
        )
    })?;
    match recipe.source {
        ManagedSource::Github { repository } => {
            discover_github(client, server, repository, recipe.arguments)
        },
        ManagedSource::Npm {
            package,
            companion,
            entrypoints,
        } => discover_npm(
            client,
            server,
            package,
            companion,
            entrypoints,
            recipe.arguments,
        ),
    }
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
    size: u64,
}

fn discover_github(
    client: &Client,
    server: LanguageServerId,
    repository: &str,
    arguments: &'static [&'static str],
) -> Result<Release, String> {
    let release: GithubRelease = client
        .get(format!(
            "https://api.github.com/repos/{repository}/releases/latest"
        ))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| error.to_string())?
        .json()
        .map_err(|error| error.to_string())?;
    let (name, archive, executable_name) = github_asset(&server)?;
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name == name)
        .ok_or_else(|| format!("release {} has no {name}", release.tag_name))?;
    let sha256 = asset
        .digest
        .and_then(|digest| digest.strip_prefix("sha256:").map(str::to_owned))
        .ok_or_else(|| format!("{name} has no publisher SHA-256 digest"))?;
    Ok(Release {
        server,
        version: release.tag_name.trim_start_matches('v').to_owned(),
        from_version: None,
        kind: ReleaseKind::Standalone {
            url: asset.browser_download_url,
            sha256,
            archive,
            executable_name: executable_name.into(),
            arguments,
        },
        download_bytes: Some(asset.size),
    })
}

fn github_asset(
    server: &LanguageServerId,
) -> Result<(&'static str, Archive, &'static str), String> {
    let platform = (std::env::consts::OS, std::env::consts::ARCH);
    match (server.key(), platform) {
        ("rust-analyzer", ("linux", "x86_64")) => Ok((
            "rust-analyzer-x86_64-unknown-linux-musl.gz",
            Archive::Gzip,
            "rust-analyzer",
        )),
        ("rust-analyzer", ("linux", "aarch64")) => Ok((
            "rust-analyzer-aarch64-unknown-linux-gnu.gz",
            Archive::Gzip,
            "rust-analyzer",
        )),
        ("rust-analyzer", ("macos", "x86_64")) => Ok((
            "rust-analyzer-x86_64-apple-darwin.gz",
            Archive::Gzip,
            "rust-analyzer",
        )),
        ("rust-analyzer", ("macos", "aarch64")) => Ok((
            "rust-analyzer-aarch64-apple-darwin.gz",
            Archive::Gzip,
            "rust-analyzer",
        )),
        ("rust-analyzer", ("windows", "x86_64")) => Ok((
            "rust-analyzer-x86_64-pc-windows-msvc.zip",
            Archive::Zip,
            "rust-analyzer.exe",
        )),
        ("texlab", ("linux", "x86_64")) => {
            Ok(("texlab-x86_64-linux.tar.gz", Archive::TarGzip, "texlab"))
        },
        ("texlab", ("linux", "aarch64")) => {
            Ok(("texlab-aarch64-linux.tar.gz", Archive::TarGzip, "texlab"))
        },
        ("texlab", ("macos", "x86_64")) => {
            Ok(("texlab-x86_64-macos.tar.gz", Archive::TarGzip, "texlab"))
        },
        ("texlab", ("macos", "aarch64")) => {
            Ok(("texlab-aarch64-macos.tar.gz", Archive::TarGzip, "texlab"))
        },
        ("texlab", ("windows", "x86_64")) => {
            Ok(("texlab-x86_64-windows.zip", Archive::Zip, "texlab.exe"))
        },
        ("ruff", ("linux", "x86_64")) => Ok((
            "ruff-x86_64-unknown-linux-gnu.tar.gz",
            Archive::TarGzip,
            "ruff",
        )),
        ("ruff", ("linux", "aarch64")) => Ok((
            "ruff-aarch64-unknown-linux-gnu.tar.gz",
            Archive::TarGzip,
            "ruff",
        )),
        ("ruff", ("macos", "x86_64")) => {
            Ok(("ruff-x86_64-apple-darwin.tar.gz", Archive::TarGzip, "ruff"))
        },
        ("ruff", ("macos", "aarch64")) => {
            Ok(("ruff-aarch64-apple-darwin.tar.gz", Archive::TarGzip, "ruff"))
        },
        ("ruff", ("windows", "x86_64")) => {
            Ok(("ruff-x86_64-pc-windows-msvc.zip", Archive::Zip, "ruff.exe"))
        },
        _ => Err(format!(
            "{} has no managed release for {}-{}",
            server.display_name(),
            platform.0,
            platform.1
        )),
    }
}

#[derive(Deserialize)]
struct NpmMetadata {
    #[serde(rename = "dist-tags")]
    dist_tags: NpmTags,
}

#[derive(Deserialize)]
struct NpmTags {
    latest: String,
}

#[derive(Deserialize)]
struct NodeRelease {
    version: String,
    lts: serde_json::Value,
    files: Vec<String>,
}

fn discover_npm(
    client: &Client,
    server: LanguageServerId,
    package: &str,
    companion: Option<&str>,
    entrypoints: &'static [&'static str],
    arguments: &'static [&'static str],
) -> Result<Release, String> {
    let npm = npm_metadata(client, package)?;
    let companion = companion
        .map(|package| {
            npm_metadata(client, package)
                .map(|metadata| (package.to_owned(), metadata.dist_tags.latest))
        })
        .transpose()?;
    let nodes: Vec<NodeRelease> = client
        .get("https://nodejs.org/dist/index.json")
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| error.to_string())?
        .json()
        .map_err(|error| error.to_string())?;
    let node = nodes
        .into_iter()
        .find(|release| !release.lts.is_boolean() || release.lts != serde_json::Value::Bool(false))
        .ok_or_else(|| "Node publishes no active LTS release".to_owned())?;
    let (file, archive) = node_asset(&node)?;
    let base = format!("https://nodejs.org/dist/{}/", node.version);
    let sums = client
        .get(format!("{base}SHASUMS256.txt"))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| error.to_string())?
        .text()
        .map_err(|error| error.to_string())?;
    let sha256 = sums
        .lines()
        .find_map(|line| {
            let (digest, candidate) = line.split_once("  ")?;
            (candidate == file).then(|| digest.to_owned())
        })
        .ok_or_else(|| format!("Node checksum manifest has no {file}"))?;
    Ok(Release {
        server,
        version: npm.dist_tags.latest,
        from_version: None,
        kind: ReleaseKind::Npm {
            package: package.into(),
            companion,
            entrypoints,
            arguments,
            node_version: node.version,
            node_url: format!("{base}{file}"),
            node_sha256: sha256,
            node_archive: archive,
        },
        download_bytes: None,
    })
}

fn npm_metadata(client: &Client, package: &str) -> Result<NpmMetadata, String> {
    client
        .get(format!("https://registry.npmjs.org/{package}"))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| error.to_string())?
        .json()
        .map_err(|error| error.to_string())
}

fn node_asset(node: &NodeRelease) -> Result<(String, Archive), String> {
    let suffix = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux-x64.tar.gz",
        ("linux", "aarch64") => "linux-arm64.tar.gz",
        ("macos", "x86_64") => "darwin-x64.tar.gz",
        ("macos", "aarch64") => "darwin-arm64.tar.gz",
        ("windows", "x86_64") => "win-x64.zip",
        other => {
            return Err(format!(
                "Node has no managed release for {}-{}",
                other.0, other.1
            ));
        },
    };
    let file = format!("node-{}-{suffix}", node.version);
    let key = suffix.trim_end_matches(".tar.gz").trim_end_matches(".zip");
    if !node.files.iter().any(|candidate| candidate == key) {
        return Err(format!("Node {} does not publish {key}", node.version));
    }
    Ok((
        file,
        if suffix.ends_with(".zip") {
            Archive::Zip
        } else {
            Archive::TarGzip
        },
    ))
}
