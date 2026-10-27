//! Discover, parse, merge, and verify the layered JSONC configuration.
//!
//! Precedence (most specific wins), each layer merged over the one below and all
//! over [`Settings::default`]:
//!
//! 1. `$GIT_ROOT/.karet/setting.jsonc` — project
//! 2. `$XDG_CONFIG_HOME/karet/setting.jsonc` — user
//! 3. `<system config dir>/karet/setting.jsonc` — system
//!
//! Loading never fails: a missing file is skipped, and a malformed file (bad JSONC,
//! an unknown key, or a wrong-typed value) degrades to the default for the affected
//! section and yields a located [`ConfigDiagnostic`] the app surfaces as a startup
//! notification.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use karet_core::Severity;
use serde::de::DeserializeOwned;
use serde_json::Map;
use serde_json::Value;

use super::schema::Settings;

/// One problem found while loading configuration. Neutral so the app can render it
/// as a notification without knowing the config internals.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConfigDiagnostic {
    /// The configuration file the problem was found in.
    pub path: PathBuf,
    /// A human-readable, located description.
    pub message: String,
    /// How severe the problem is (a bad section is a `Warning` — its defaults are
    /// used and the rest of the file still applies).
    pub severity: Severity,
}

/// A configuration layer in the settings cascade.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum ConfigLayer {
    /// `<system config dir>/karet/setting.jsonc`.
    System,
    /// `$XDG_CONFIG_HOME/karet/setting.jsonc`.
    User,
    /// `$GIT_ROOT/.karet/setting.jsonc`.
    Project,
}

impl ConfigLayer {
    /// The stable display label for this layer.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

/// Whether a discovered configuration layer contributed to the loaded settings.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ConfigLayerStatus {
    /// The file existed, parsed as JSONC, and had an object at the top level.
    Loaded,
    /// The file was not present.
    Missing,
    /// The file was present but could not be read, parsed, or used as an object.
    Invalid(String),
}

/// One layer considered while loading configuration.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConfigLayerReport {
    /// Which layer this row describes.
    pub layer: ConfigLayer,
    /// The path that was checked.
    pub path: PathBuf,
    /// The outcome for that path.
    pub status: ConfigLayerStatus,
}

/// The loaded settings plus enough provenance for a UI to explain them.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LoadedConfig {
    /// The final validated settings in effect for this session.
    pub settings: Settings,
    /// Problems found while loading the settings.
    pub diagnostics: Vec<ConfigDiagnostic>,
    /// The files considered by the cascade.
    pub layers: Vec<ConfigLayerReport>,
    /// Effective setting key paths that were explicitly set by a valid layer.
    pub explicit: BTreeMap<String, ConfigLayer>,
    /// Top-level sections (`"tasks"`, `"editor"`, …) whose value here is a
    /// fallback rather than what the files say: the section failed to
    /// deserialize, or a layer that could set it is malformed (so its last valid
    /// value, or the default, stands in). A consumer that reconciles state against
    /// a section — dropping what it no longer lists — should skip one named here.
    #[serde(default)]
    pub failed_sections: BTreeSet<String>,
    /// Parsed per-layer values used to seed live reload without rereading files.
    /// Loader machinery, not report data: never crosses the seam.
    #[serde(skip)]
    sources: Option<Vec<CachedLayer>>,
}

impl LoadedConfig {
    /// Build a report for already-loaded settings when provenance was not captured
    /// (mostly tests and older callers). No values are marked explicit.
    #[must_use]
    pub fn from_settings(settings: Settings) -> Self {
        Self {
            settings,
            diagnostics: Vec::new(),
            layers: Vec::new(),
            explicit: BTreeMap::new(),
            failed_sections: BTreeSet::new(),
            sources: None,
        }
    }
}

/// One config layer's current disk status and last valid parsed object.
#[derive(Clone, Debug, PartialEq)]
struct CachedLayer {
    layer: ConfigLayer,
    path: PathBuf,
    value: Option<Map<String, Value>>,
    status: ConfigLayerStatus,
    diagnostic: Option<ConfigDiagnostic>,
}

/// Cached live configuration owned by the session actor.
pub(crate) struct ConfigManager {
    sources: Vec<CachedLayer>,
    report: LoadedConfig,
}

impl ConfigManager {
    pub(crate) fn from_loaded(report: &LoadedConfig) -> Option<Self> {
        let sources = report.sources.clone()?;
        Some(Self {
            sources,
            report: report.clone(),
        })
    }

    pub(crate) fn paths(&self) -> Vec<PathBuf> {
        self.sources
            .iter()
            .map(|source| source.path.clone())
            .collect()
    }

    pub(crate) fn is_config_path(&self, path: &Path) -> bool {
        self.sources.iter().any(|source| source.path == path)
    }

    /// Reload only layers named by `paths`, retaining each layer's last valid value
    /// when the new file is malformed. Returns a new report only when observable
    /// settings, provenance, statuses, or diagnostics changed.
    pub(crate) fn reload(&mut self, paths: &[PathBuf]) -> Option<LoadedConfig> {
        let previous = self.report.clone();
        for index in 0..self.sources.len() {
            if !paths.iter().any(|path| path == &self.sources[index].path) {
                continue;
            }
            let old_value = self.sources[index].value.clone();
            let previous_validation: BTreeSet<String> = report_from_sources(&self.sources)
                .diagnostics
                .into_iter()
                .filter(|diag| diag.path == Path::new("<config>"))
                .map(|diag| diag.message)
                .collect();
            let layer = self.sources[index].layer;
            let path = self.sources[index].path.clone();
            let candidate = read_cached_layer(layer, path.clone());
            match candidate.status {
                ConfigLayerStatus::Loaded | ConfigLayerStatus::Missing => {
                    self.sources[index] = candidate;
                    let trial = report_from_sources(&self.sources);
                    let validation: Vec<String> = trial
                        .diagnostics
                        .iter()
                        .filter(|diag| diag.path == Path::new("<config>"))
                        .map(|diag| diag.message.clone())
                        .filter(|message| !previous_validation.contains(message))
                        .collect();
                    if !validation.is_empty() {
                        self.sources[index].value = old_value;
                        let message = validation.join("; ");
                        self.sources[index].status = ConfigLayerStatus::Invalid(message.clone());
                        self.sources[index].diagnostic =
                            Some(ConfigDiagnostic::warning(&path, message));
                    }
                },
                ConfigLayerStatus::Invalid(_) => {
                    self.sources[index].status = candidate.status;
                    self.sources[index].diagnostic = candidate.diagnostic;
                    self.sources[index].value = old_value;
                },
            }
        }
        self.report = report_from_sources(&self.sources);
        (self.report != previous).then(|| self.report.clone())
    }
}

impl Default for LoadedConfig {
    fn default() -> Self {
        Self::from_settings(Settings::default())
    }
}

impl ConfigDiagnostic {
    fn warning(path: &Path, message: impl Into<String>) -> Self {
        Self {
            path: path.to_path_buf(),
            message: message.into(),
            severity: Severity::Warning,
        }
    }
}

/// The known top-level section keys. Any other top-level key is an unknown-setting
/// warning; a value under a known key that fails to deserialize is a section warning.
const SECTIONS: &[&str] = &[
    "editor",
    "files",
    "workbench",
    "search",
    "seam",
    "spellcheck",
    "latex",
    "git",
    "lsp",
    "markdown",
    "wakatime",
    "deps",
    "toml",
    "debug",
    "notebook",
    "tasks",
];

/// Load the merged, verified [`Settings`] for a workspace rooted at `roots`, plus any
/// diagnostics. Reuses the same git discovery the session already performs to locate
/// the project layer.
#[must_use]
pub fn load(roots: &[PathBuf]) -> (Settings, Vec<ConfigDiagnostic>) {
    let report = load_report(roots);
    (report.settings, report.diagnostics)
}

/// Load the merged, verified [`Settings`] plus layer and explicit-key provenance for
/// a workspace rooted at `roots`.
#[must_use]
pub fn load_report(roots: &[PathBuf]) -> LoadedConfig {
    load_layer_reports(ordered_layers(
        system_config_path(),
        user_config_path(),
        project_config_path(roots),
    ))
}

/// The discovered layer files in precedence order, lowest first: system, then user,
/// then project. [`report_from_sources`] merges later layers over earlier ones, so
/// this order is what makes a project setting beat a user one and a user setting
/// beat a system one. A tier with no file location is skipped.
pub(crate) fn ordered_layers(
    system: Option<PathBuf>,
    user: Option<PathBuf>,
    project: Option<PathBuf>,
) -> impl Iterator<Item = (ConfigLayer, PathBuf)> {
    [
        (ConfigLayer::System, system),
        (ConfigLayer::User, user),
        (ConfigLayer::Project, project),
    ]
    .into_iter()
    .filter_map(|(layer, path)| path.map(|path| (layer, path)))
}

/// The precedence-ordered (low → high) loader, factored out so tests can inject
/// explicit paths without touching the environment.
#[cfg(test)]
fn load_layers(paths: impl IntoIterator<Item = PathBuf>) -> (Settings, Vec<ConfigDiagnostic>) {
    let report = load_layer_reports(
        paths
            .into_iter()
            .enumerate()
            .map(|(i, path)| (test_layer(i), path)),
    );
    (report.settings, report.diagnostics)
}

/// The provenance-aware precedence loader.
pub(crate) fn load_layer_reports(
    paths: impl IntoIterator<Item = (ConfigLayer, PathBuf)>,
) -> LoadedConfig {
    let sources: Vec<CachedLayer> = paths
        .into_iter()
        .map(|(layer, path)| read_cached_layer(layer, path))
        .collect();
    report_from_sources(&sources)
}

fn read_cached_layer(layer: ConfigLayer, path: PathBuf) -> CachedLayer {
    match read_layer(&path) {
        Ok(None) => CachedLayer {
            layer,
            path,
            value: None,
            status: ConfigLayerStatus::Missing,
            diagnostic: None,
        },
        Ok(Some(Value::Object(value))) => CachedLayer {
            layer,
            path,
            value: Some(value),
            status: ConfigLayerStatus::Loaded,
            diagnostic: None,
        },
        Ok(Some(_)) => {
            let message = "expected a JSON object at the top level".to_string();
            CachedLayer {
                layer,
                diagnostic: Some(ConfigDiagnostic::warning(&path, message.clone())),
                path,
                value: None,
                status: ConfigLayerStatus::Invalid(message),
            }
        },
        Err(message) => CachedLayer {
            layer,
            diagnostic: Some(ConfigDiagnostic::warning(&path, message.clone())),
            path,
            value: None,
            status: ConfigLayerStatus::Invalid(message),
        },
    }
}

fn report_from_sources(sources: &[CachedLayer]) -> LoadedConfig {
    let mut diags: Vec<ConfigDiagnostic> = sources
        .iter()
        .filter_map(|source| source.diagnostic.clone())
        .collect();
    let reports: Vec<ConfigLayerReport> = sources
        .iter()
        .map(|source| ConfigLayerReport {
            layer: source.layer,
            path: source.path.clone(),
            status: source.status.clone(),
        })
        .collect();
    let mut explicit = BTreeMap::new();
    // Start from the defaults as a JSON object and merge each present layer over it.
    let mut merged = to_object(&Settings::default());
    for source in sources {
        if let Some(value) = &source.value {
            mark_explicit(
                &mut explicit,
                source.layer,
                "",
                &Value::Object(value.clone()),
            );
            deep_merge(&mut merged, value.clone());
        }
    }
    let mut invalid_sections = BTreeSet::new();
    let settings = deserialize_sections(&merged, &mut diags, &mut invalid_sections);
    explicit.retain(|path, _| {
        path.split('.').next().is_some_and(|section| {
            SECTIONS.contains(&section) && !invalid_sections.contains(section)
        })
    });
    let mut failed_sections = invalid_sections;
    // A malformed layer contributes its last valid value (or nothing), so no
    // section can be trusted to say what the files on disk now say.
    if sources
        .iter()
        .any(|source| matches!(source.status, ConfigLayerStatus::Invalid(_)))
    {
        failed_sections.extend(SECTIONS.iter().map(|section| (*section).to_owned()));
    }
    LoadedConfig {
        settings,
        diagnostics: diags,
        layers: reports,
        explicit,
        failed_sections,
        sources: Some(sources.to_vec()),
    }
}

/// Read one layer file into a JSON value. `Ok(None)` for a missing file; `Err` with a
/// located message for an IO error or a JSONC syntax error.
fn read_layer(path: &Path) -> Result<Option<Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("could not read config file: {err}")),
    };
    jsonc_parser::parse_to_serde_value(&text, &Default::default())
        .map_err(|err| format!("invalid JSONC: {err}"))
}

/// Deserialize each known section from `merged` independently, so a single bad
/// section only resets itself (and warns) rather than discarding the whole file.
/// Unknown top-level keys warn but are otherwise ignored. Diagnostics from a bad
/// section are attributed to the layer that last set the offending key.
fn deserialize_sections(
    merged: &Map<String, Value>,
    diags: &mut Vec<ConfigDiagnostic>,
    invalid_sections: &mut BTreeSet<String>,
) -> Settings {
    let mut settings = Settings::default();
    for (key, value) in merged {
        if !SECTIONS.contains(&key.as_str()) {
            diags.push(ConfigDiagnostic {
                path: PathBuf::from("<config>"),
                message: format!("unknown setting `{key}`"),
                severity: Severity::Warning,
            });
            continue;
        }
        // A section is only reported when it deviates from a bare default (which the
        // defaults baseline always provides); on error keep the section default.
        let outcome = match key.as_str() {
            "editor" => section(value, |v| settings.editor = v),
            "files" => section(value, |v| settings.files = v),
            "workbench" => section(value, |v| settings.workbench = v),
            "search" => section(value, |v| settings.search = v),
            "seam" => section(value, |v| settings.seam = v),
            "spellcheck" => section(value, |v| settings.spellcheck = v),
            "latex" => section(value, |v| settings.latex = v),
            "git" => section(value, |v| settings.git = v),
            "lsp" => section(value, |v| settings.lsp = v),
            "markdown" => section(value, |v| settings.markdown = v),
            "wakatime" => section(value, |v| settings.wakatime = v),
            "deps" => section(value, |v| settings.deps = v),
            "toml" => section(value, |v| settings.toml = v),
            "debug" => section(value, |v| settings.debug = v),
            "notebook" => section(value, |v| settings.notebook = v),
            "tasks" => section(value, |v| settings.tasks = v),
            // Unreachable: `SECTIONS` gates the loop above, and
            // `every_section_round_trips_a_value` proves each entry lands.
            _ => Ok(()),
        };
        if let Err(err) = outcome {
            invalid_sections.insert(key.clone());
            diags.push(ConfigDiagnostic {
                path: PathBuf::from("<config>"),
                message: format!("invalid `{key}` settings: {err}"),
                severity: Severity::Warning,
            });
        }
    }
    settings
}

/// Deserialize one section's value and, on success, store it via `set`.
fn section<T: DeserializeOwned>(value: &Value, set: impl FnOnce(T)) -> Result<(), String> {
    let parsed: T = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    set(parsed);
    Ok(())
}

/// Recursively merge `overlay` into `base`: nested objects merge key-by-key, every
/// other value (scalars, arrays) replaces wholesale. Standard config-cascade
/// semantics — arrays are replaced, not concatenated.
fn deep_merge(base: &mut Map<String, Value>, overlay: Map<String, Value>) {
    for (key, over) in overlay {
        match (base.get_mut(&key), over) {
            (Some(Value::Object(base_obj)), Value::Object(over_obj)) => {
                deep_merge(base_obj, over_obj);
            },
            (_, over) => {
                base.insert(key, over);
            },
        }
    }
}

/// Record the leaf setting paths explicitly present in one layer.
fn mark_explicit(
    out: &mut BTreeMap<String, ConfigLayer>,
    layer: ConfigLayer,
    prefix: &str,
    value: &Value,
) {
    match value {
        Value::Object(obj) if !obj.is_empty() => {
            for (key, child) in obj {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                mark_explicit(out, layer, &path, child);
            }
        },
        Value::Object(_) if !prefix.is_empty() => {
            out.insert(prefix.to_string(), layer);
        },
        _ if !prefix.is_empty() => {
            out.insert(prefix.to_string(), layer);
        },
        _ => {},
    }
}

/// Serialize a value into a JSON object map (infallible for `Settings`).
fn to_object<T: serde::Serialize>(value: &T) -> Map<String, Value> {
    match serde_json::to_value(value) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

/// `$GIT_ROOT/.karet/setting.jsonc` for the first root that sits inside a git
/// worktree. Ascends parents looking for a `.git` entry (file or directory), mirroring
/// git's own discovery, so this stays unit-testable without a live repository.
pub(crate) fn project_config_path(roots: &[PathBuf]) -> Option<PathBuf> {
    project_config_dir(roots).map(|dir| dir.join("setting.jsonc"))
}

/// The project configuration directory, `$GIT_ROOT/.karet`, for the first root
/// that sits inside a git worktree. Other per-project config files (the app's
/// `keymap.jsonc`) live beside `setting.jsonc` here, so they share its discovery.
#[must_use]
pub fn project_config_dir(roots: &[PathBuf]) -> Option<PathBuf> {
    roots
        .iter()
        .find_map(|root| git_root(root))
        .map(|git_root| git_root.join(".karet"))
}

/// The nearest ancestor of `start` (inclusive) that contains a `.git` entry.
fn git_root(start: &Path) -> Option<PathBuf> {
    let mut dir: Option<&Path> = Some(start);
    while let Some(current) = dir {
        if current.join(".git").exists() {
            return Some(current.to_path_buf());
        }
        dir = current.parent();
    }
    None
}

/// `$XDG_CONFIG_HOME/karet/setting.jsonc` (platform config dir on macOS/Windows).
pub(crate) fn user_config_path() -> Option<PathBuf> {
    user_config_dir().map(|dir| dir.join("setting.jsonc"))
}

/// The user configuration directory, `$XDG_CONFIG_HOME/karet` (the platform
/// config dir on macOS/Windows), where `setting.jsonc` and the app's other user
/// config files live.
#[must_use]
pub fn user_config_dir() -> Option<PathBuf> {
    crate::dirs::config_dir()
}

/// `<system config dir>/karet/setting.jsonc`. On Unix this is the first entry of
/// `$XDG_CONFIG_DIRS` (default `/etc/xdg`); other platforms have no system tier.
fn system_config_path() -> Option<PathBuf> {
    system_config_path_in(std::env::var_os("XDG_CONFIG_DIRS"))
}

/// [`system_config_path`] for a given `$XDG_CONFIG_DIRS` value.
fn system_config_path_in(xdg_config_dirs: Option<std::ffi::OsString>) -> Option<PathBuf> {
    if cfg!(unix) {
        let base = xdg_config_dirs
            .and_then(|dirs| {
                dirs.to_str()
                    .and_then(|s| s.split(':').next())
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
            })
            .unwrap_or_else(|| PathBuf::from("/etc/xdg"));
        Some(base.join("karet").join("setting.jsonc"))
    } else {
        None
    }
}

#[cfg(test)]
fn test_layer(index: usize) -> ConfigLayer {
    match index {
        0 => ConfigLayer::System,
        1 => ConfigLayer::User,
        _ => ConfigLayer::Project,
    }
}

#[cfg(test)]
mod tests;
