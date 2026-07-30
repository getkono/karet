//! Explicit release discovery for the built-in provider catalogue.

use std::path::Path;

use reqwest::blocking::Client;
use serde::Deserialize;

use crate::api::LanguageServerId;

#[derive(Clone, Copy)]
pub(super) struct ManagedRecipe {
    pub(super) server: &'static str,
    pub(super) source: ManagedSource,
    pub(super) arguments: &'static [&'static str],
}

#[derive(Clone, Copy)]
pub(super) enum ManagedSource {
    Github {
        repository: &'static str,
    },
    Npm {
        package: &'static str,
        companion: Option<&'static str>,
        binary: &'static str,
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
            binary: "typescript-language-server",
        },
        arguments: &["--stdio"],
    },
    ManagedRecipe {
        server: "pyright",
        source: ManagedSource::Npm {
            package: "pyright",
            companion: None,
            binary: "pyright-langserver",
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
    ManagedRecipe {
        server: "astro-language-server",
        source: ManagedSource::Npm {
            package: "@astrojs/language-server",
            companion: None,
            binary: "astro-ls",
        },
        arguments: &["--stdio"],
    },
    ManagedRecipe {
        server: "svelte-language-server",
        source: ManagedSource::Npm {
            package: "svelte-language-server",
            companion: None,
            binary: "svelteserver",
        },
        arguments: &["--stdio"],
    },
    ManagedRecipe {
        server: "vue-language-server",
        source: ManagedSource::Npm {
            package: "@vue/language-server",
            companion: None,
            binary: "vue-language-server",
        },
        arguments: &["--stdio"],
    },
    ManagedRecipe {
        server: "biome",
        source: ManagedSource::Npm {
            package: "@biomejs/biome",
            companion: None,
            binary: "biome",
        },
        arguments: &["lsp-proxy"],
    },
    ManagedRecipe {
        server: "yaml-language-server",
        source: ManagedSource::Npm {
            package: "yaml-language-server",
            companion: None,
            binary: "yaml-language-server",
        },
        arguments: &["--stdio"],
    },
    ManagedRecipe {
        server: "vscode-html-language-server",
        source: ManagedSource::Npm {
            package: "vscode-langservers-extracted",
            companion: None,
            binary: "vscode-html-language-server",
        },
        arguments: &["--stdio"],
    },
    ManagedRecipe {
        server: "vscode-css-language-server",
        source: ManagedSource::Npm {
            package: "vscode-langservers-extracted",
            companion: None,
            binary: "vscode-css-language-server",
        },
        arguments: &["--stdio"],
    },
    ManagedRecipe {
        server: "vscode-json-language-server",
        source: ManagedSource::Npm {
            package: "vscode-langservers-extracted",
            companion: None,
            binary: "vscode-json-language-server",
        },
        arguments: &["--stdio"],
    },
    ManagedRecipe {
        server: "bash-language-server",
        source: ManagedSource::Npm {
            package: "bash-language-server",
            companion: None,
            binary: "bash-language-server",
        },
        arguments: &["start"],
    },
    ManagedRecipe {
        server: "docker-langserver",
        source: ManagedSource::Npm {
            package: "dockerfile-language-server-nodejs",
            companion: None,
            binary: "docker-langserver",
        },
        arguments: &["--stdio"],
    },
    ManagedRecipe {
        server: "graphql-lsp",
        source: ManagedSource::Npm {
            package: "graphql-language-service-cli",
            companion: None,
            binary: "graphql-lsp",
        },
        arguments: &["server", "-m", "stream"],
    },
    ManagedRecipe {
        server: "clangd",
        source: ManagedSource::Github {
            repository: "clangd/clangd",
        },
        arguments: &[],
    },
    ManagedRecipe {
        server: "zls",
        source: ManagedSource::Github {
            repository: "zigtools/zls",
        },
        arguments: &[],
    },
    ManagedRecipe {
        server: "lua-language-server",
        source: ManagedSource::Github {
            repository: "LuaLS/lua-language-server",
        },
        arguments: &[],
    },
    ManagedRecipe {
        server: "clojure-lsp",
        source: ManagedSource::Github {
            repository: "clojure-lsp/clojure-lsp",
        },
        arguments: &[],
    },
    ManagedRecipe {
        server: "buf",
        source: ManagedSource::Github {
            repository: "bufbuild/buf",
        },
        arguments: &["beta", "lsp"],
    },
    ManagedRecipe {
        server: "marksman",
        source: ManagedSource::Github {
            repository: "artempyanykh/marksman",
        },
        arguments: &[],
    },
    ManagedRecipe {
        server: "neocmakelsp",
        source: ManagedSource::Github {
            repository: "neocmakelsp/neocmakelsp",
        },
        arguments: &[],
    },
];

pub(super) fn managed_recipes() -> &'static [ManagedRecipe] {
    MANAGED_RECIPES
}

pub(super) fn managed_recipe(server: &LanguageServerId) -> Option<ManagedRecipe> {
    MANAGED_RECIPES
        .iter()
        .find(|recipe| {
            recipe.server == server.key()
                && recipe_available(recipe, std::env::consts::OS, std::env::consts::ARCH)
        })
        .copied()
}

pub(super) fn managed_servers() -> Vec<LanguageServerId> {
    MANAGED_RECIPES
        .iter()
        .filter(|recipe| recipe_available(recipe, std::env::consts::OS, std::env::consts::ARCH))
        .map(|recipe| LanguageServerId::new(recipe.server))
        .collect()
}

fn recipe_available(recipe: &ManagedRecipe, os: &str, arch: &str) -> bool {
    match recipe.source {
        ManagedSource::Github { .. } => {
            github_asset_for(&LanguageServerId::new(recipe.server), "0.0.0", os, arch).is_ok()
        },
        ManagedSource::Npm { .. } => node_platform(os, arch).is_some(),
    }
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
        retain_archive: bool,
        arguments: &'static [&'static str],
    },
    Npm {
        package: String,
        companion: Option<(String, String)>,
        entrypoint: String,
        arguments: &'static [&'static str],
        node_version: String,
        node_url: String,
        node_sha256: String,
        node_archive: Archive,
    },
}

#[derive(Clone, Copy)]
pub(super) enum Archive {
    Raw,
    Gzip,
    TarGzip,
    TarXz,
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
            binary,
        } => discover_npm(client, server, package, companion, binary, recipe.arguments),
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
    let version = release.tag_name.trim_start_matches('v');
    let (name, archive, executable_name, retain_archive) = github_asset_for(
        &server,
        version,
        std::env::consts::OS,
        std::env::consts::ARCH,
    )?;
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
            executable_name,
            retain_archive,
            arguments,
        },
        download_bytes: Some(asset.size),
    })
}

pub(super) fn github_asset_for(
    server: &LanguageServerId,
    version: &str,
    os: &str,
    arch: &str,
) -> Result<(String, Archive, String, bool), String> {
    let simple = |name: &str, archive, executable: &str| {
        Ok((name.to_owned(), archive, executable.to_owned(), false))
    };
    let bundle = |name: &str, archive, executable: &str| {
        Ok((name.to_owned(), archive, executable.to_owned(), true))
    };
    match (server.key(), os, arch) {
        ("rust-analyzer", "linux", "x86_64") => simple(
            "rust-analyzer-x86_64-unknown-linux-musl.gz",
            Archive::Gzip,
            "rust-analyzer",
        ),
        ("rust-analyzer", "linux", "aarch64") => simple(
            "rust-analyzer-aarch64-unknown-linux-gnu.gz",
            Archive::Gzip,
            "rust-analyzer",
        ),
        ("rust-analyzer", "macos", "x86_64") => simple(
            "rust-analyzer-x86_64-apple-darwin.gz",
            Archive::Gzip,
            "rust-analyzer",
        ),
        ("rust-analyzer", "macos", "aarch64") => simple(
            "rust-analyzer-aarch64-apple-darwin.gz",
            Archive::Gzip,
            "rust-analyzer",
        ),
        ("rust-analyzer", "windows", "x86_64") => simple(
            "rust-analyzer-x86_64-pc-windows-msvc.zip",
            Archive::Zip,
            "rust-analyzer.exe",
        ),
        ("texlab", "linux", "x86_64") => {
            simple("texlab-x86_64-linux.tar.gz", Archive::TarGzip, "texlab")
        },
        ("texlab", "linux", "aarch64") => {
            simple("texlab-aarch64-linux.tar.gz", Archive::TarGzip, "texlab")
        },
        ("texlab", "macos", "x86_64") => {
            simple("texlab-x86_64-macos.tar.gz", Archive::TarGzip, "texlab")
        },
        ("texlab", "macos", "aarch64") => {
            simple("texlab-aarch64-macos.tar.gz", Archive::TarGzip, "texlab")
        },
        ("texlab", "windows", "x86_64") => {
            simple("texlab-x86_64-windows.zip", Archive::Zip, "texlab.exe")
        },
        ("ruff", "linux", "x86_64") => simple(
            "ruff-x86_64-unknown-linux-gnu.tar.gz",
            Archive::TarGzip,
            "ruff",
        ),
        ("ruff", "linux", "aarch64") => simple(
            "ruff-aarch64-unknown-linux-gnu.tar.gz",
            Archive::TarGzip,
            "ruff",
        ),
        ("ruff", "macos", "x86_64") => {
            simple("ruff-x86_64-apple-darwin.tar.gz", Archive::TarGzip, "ruff")
        },
        ("ruff", "macos", "aarch64") => {
            simple("ruff-aarch64-apple-darwin.tar.gz", Archive::TarGzip, "ruff")
        },
        ("ruff", "windows", "x86_64") => {
            simple("ruff-x86_64-pc-windows-msvc.zip", Archive::Zip, "ruff.exe")
        },
        ("clangd", "linux", "x86_64") => bundle(
            &format!("clangd-linux-{version}.zip"),
            Archive::Zip,
            "clangd",
        ),
        ("clangd", "macos", "x86_64") => {
            bundle(&format!("clangd-mac-{version}.zip"), Archive::Zip, "clangd")
        },
        ("clangd", "windows", "x86_64") => bundle(
            &format!("clangd-windows-{version}.zip"),
            Archive::Zip,
            "clangd.exe",
        ),
        ("zls", "linux", "x86_64") => simple("zls-x86_64-linux.tar.xz", Archive::TarXz, "zls"),
        ("zls", "linux", "aarch64") => simple("zls-aarch64-linux.tar.xz", Archive::TarXz, "zls"),
        ("zls", "macos", "x86_64") => simple("zls-x86_64-macos.tar.xz", Archive::TarXz, "zls"),
        ("zls", "macos", "aarch64") => simple("zls-aarch64-macos.tar.xz", Archive::TarXz, "zls"),
        ("zls", "windows", "x86_64") => simple("zls-x86_64-windows.zip", Archive::Zip, "zls.exe"),
        ("lua-language-server", "linux", "x86_64") => bundle(
            &format!("lua-language-server-{version}-linux-x64.tar.gz"),
            Archive::TarGzip,
            "lua-language-server",
        ),
        ("lua-language-server", "linux", "aarch64") => bundle(
            &format!("lua-language-server-{version}-linux-arm64.tar.gz"),
            Archive::TarGzip,
            "lua-language-server",
        ),
        ("lua-language-server", "macos", "x86_64") => bundle(
            &format!("lua-language-server-{version}-darwin-x64.tar.gz"),
            Archive::TarGzip,
            "lua-language-server",
        ),
        ("lua-language-server", "macos", "aarch64") => bundle(
            &format!("lua-language-server-{version}-darwin-arm64.tar.gz"),
            Archive::TarGzip,
            "lua-language-server",
        ),
        ("lua-language-server", "windows", "x86_64") => bundle(
            &format!("lua-language-server-{version}-win32-x64.zip"),
            Archive::Zip,
            "lua-language-server.exe",
        ),
        ("clojure-lsp", "linux", "x86_64") => simple(
            "clojure-lsp-native-linux-amd64.zip",
            Archive::Zip,
            "clojure-lsp",
        ),
        ("clojure-lsp", "linux", "aarch64") => simple(
            "clojure-lsp-native-linux-aarch64.zip",
            Archive::Zip,
            "clojure-lsp",
        ),
        ("clojure-lsp", "macos", "x86_64") => simple(
            "clojure-lsp-native-macos-amd64.zip",
            Archive::Zip,
            "clojure-lsp",
        ),
        ("clojure-lsp", "macos", "aarch64") => simple(
            "clojure-lsp-native-macos-aarch64.zip",
            Archive::Zip,
            "clojure-lsp",
        ),
        ("clojure-lsp", "windows", "x86_64") => simple(
            "clojure-lsp-native-windows-amd64.zip",
            Archive::Zip,
            "clojure-lsp.exe",
        ),
        ("buf", "linux", "x86_64") => simple("buf-Linux-x86_64", Archive::Raw, "buf-Linux-x86_64"),
        ("buf", "linux", "aarch64") => {
            simple("buf-Linux-aarch64", Archive::Raw, "buf-Linux-aarch64")
        },
        ("buf", "macos", "x86_64") => {
            simple("buf-Darwin-x86_64", Archive::Raw, "buf-Darwin-x86_64")
        },
        ("buf", "macos", "aarch64") => simple("buf-Darwin-arm64", Archive::Raw, "buf-Darwin-arm64"),
        ("buf", "windows", "x86_64") => simple(
            "buf-Windows-x86_64.exe",
            Archive::Raw,
            "buf-Windows-x86_64.exe",
        ),
        ("marksman", "linux", "x86_64") => {
            simple("marksman-linux-x64", Archive::Raw, "marksman-linux-x64")
        },
        ("marksman", "linux", "aarch64") => {
            simple("marksman-linux-arm64", Archive::Raw, "marksman-linux-arm64")
        },
        ("marksman", "macos", "x86_64" | "aarch64") => {
            simple("marksman-macos", Archive::Raw, "marksman-macos")
        },
        ("marksman", "windows", "x86_64") => simple("marksman.exe", Archive::Raw, "marksman.exe"),
        ("neocmakelsp", "linux", "x86_64") => simple(
            "neocmakelsp-x86_64-unknown-linux-gnu.tar.gz",
            Archive::TarGzip,
            "neocmakelsp",
        ),
        ("neocmakelsp", "linux", "aarch64") => simple(
            "neocmakelsp-aarch64-unknown-linux-gnu.tar.gz",
            Archive::TarGzip,
            "neocmakelsp",
        ),
        ("neocmakelsp", "macos", "x86_64" | "aarch64") => simple(
            "neocmakelsp-universal-apple-darwin.tar.gz",
            Archive::TarGzip,
            "neocmakelsp",
        ),
        ("neocmakelsp", "windows", "x86_64") => simple(
            "neocmakelsp-x86_64-pc-windows-msvc.zip",
            Archive::Zip,
            "neocmakelsp.exe",
        ),
        _ => Err(format!(
            "{} has no managed release for {}-{}",
            server.display_name(),
            os,
            arch
        )),
    }
}

#[derive(Deserialize)]
struct NpmMetadata {
    #[serde(rename = "dist-tags")]
    dist_tags: NpmTags,
    #[serde(default)]
    bin: std::collections::BTreeMap<String, String>,
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
    binary: &str,
    arguments: &'static [&'static str],
) -> Result<Release, String> {
    let npm = npm_metadata(client, package)?;
    let entrypoint = npm
        .bin
        .get(binary)
        .filter(|path| safe_relative_path(path))
        .cloned()
        .ok_or_else(|| format!("{package} publishes no safe {binary} executable"))?;
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
            entrypoint,
            arguments,
            node_version: node.version,
            node_url: format!("{base}{file}"),
            node_sha256: sha256,
            node_archive: archive,
        },
        download_bytes: None,
    })
}

fn safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
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
    let platform = (std::env::consts::OS, std::env::consts::ARCH);
    let suffix = match node_platform(platform.0, platform.1) {
        Some(suffix) => suffix,
        None => {
            return Err(format!(
                "Node has no managed release for {}-{}",
                platform.0, platform.1
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

fn node_platform(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some("linux-x64.tar.gz"),
        ("linux", "aarch64") => Some("linux-arm64.tar.gz"),
        ("macos", "x86_64") => Some("darwin-x64.tar.gz"),
        ("macos", "aarch64") => Some("darwin-arm64.tar.gz"),
        ("windows", "x86_64") => Some("win-x64.zip"),
        _ => None,
    }
}
