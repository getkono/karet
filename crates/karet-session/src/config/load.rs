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
#[derive(Clone, Debug, PartialEq, Eq)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigLayerStatus {
    /// The file existed, parsed as JSONC, and had an object at the top level.
    Loaded,
    /// The file was not present.
    Missing,
    /// The file was present but could not be read, parsed, or used as an object.
    Invalid(String),
}

/// One layer considered while loading configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigLayerReport {
    /// Which layer this row describes.
    pub layer: ConfigLayer,
    /// The path that was checked.
    pub path: PathBuf,
    /// The outcome for that path.
    pub status: ConfigLayerStatus,
}

/// The loaded settings plus enough provenance for a UI to explain them.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedConfig {
    /// The final validated settings in effect for this session.
    pub settings: Settings,
    /// Problems found while loading the settings.
    pub diagnostics: Vec<ConfigDiagnostic>,
    /// The files considered by the cascade.
    pub layers: Vec<ConfigLayerReport>,
    /// Effective setting key paths that were explicitly set by a valid layer.
    pub explicit: BTreeMap<String, ConfigLayer>,
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
        }
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
    "spellcheck",
    "git",
    "lsp",
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
    let layers = [
        system_config_path().map(|path| (ConfigLayer::System, path)),
        user_config_path().map(|path| (ConfigLayer::User, path)),
        project_config_path(roots).map(|path| (ConfigLayer::Project, path)),
    ];
    load_layer_reports(layers.into_iter().flatten())
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
fn load_layer_reports(paths: impl IntoIterator<Item = (ConfigLayer, PathBuf)>) -> LoadedConfig {
    let mut diags = Vec::new();
    let mut reports = Vec::new();
    let mut explicit = BTreeMap::new();
    // Start from the defaults as a JSON object and merge each present layer over it.
    let mut merged = to_object(&Settings::default());
    for (layer, path) in paths {
        match read_layer(&path) {
            Ok(None) => reports.push(ConfigLayerReport {
                layer,
                path,
                status: ConfigLayerStatus::Missing,
            }),
            Ok(Some(value)) => match value {
                Value::Object(obj) => {
                    mark_explicit(&mut explicit, layer, "", &Value::Object(obj.clone()));
                    deep_merge(&mut merged, obj);
                    reports.push(ConfigLayerReport {
                        layer,
                        path,
                        status: ConfigLayerStatus::Loaded,
                    });
                },
                _ => {
                    let message = "expected a JSON object at the top level";
                    diags.push(ConfigDiagnostic::warning(&path, message));
                    reports.push(ConfigLayerReport {
                        layer,
                        path,
                        status: ConfigLayerStatus::Invalid(message.to_string()),
                    });
                },
            },
            Err(err) => {
                diags.push(ConfigDiagnostic::warning(&path, err.clone()));
                reports.push(ConfigLayerReport {
                    layer,
                    path,
                    status: ConfigLayerStatus::Invalid(err),
                });
            },
        }
    }
    let mut invalid_sections = BTreeSet::new();
    let settings = deserialize_sections(&merged, &mut diags, &mut invalid_sections);
    explicit.retain(|path, _| {
        path.split('.').next().is_some_and(|section| {
            SECTIONS.contains(&section) && !invalid_sections.contains(section)
        })
    });
    LoadedConfig {
        settings,
        diagnostics: diags,
        layers: reports,
        explicit,
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
            "spellcheck" => section(value, |v| settings.spellcheck = v),
            "git" => section(value, |v| settings.git = v),
            "lsp" => section(value, |v| settings.lsp = v),
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
fn project_config_path(roots: &[PathBuf]) -> Option<PathBuf> {
    roots
        .iter()
        .find_map(|root| git_root(root))
        .map(|git_root| git_root.join(".karet").join("setting.jsonc"))
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
fn user_config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "getkono", "karet")
        .map(|dirs| dirs.config_dir().join("setting.jsonc"))
}

/// `<system config dir>/karet/setting.jsonc`. On Unix this is the first entry of
/// `$XDG_CONFIG_DIRS` (default `/etc/xdg`); other platforms have no system tier.
fn system_config_path() -> Option<PathBuf> {
    if cfg!(unix) {
        let base = std::env::var_os("XDG_CONFIG_DIRS")
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
mod tests {
    use super::*;
    use crate::config::schema::AutoSave;
    use crate::config::schema::LineNumbers;

    /// Create a temp dir and write `body` to `name` inside it. Returns `None` (so the
    /// test skips) on any IO failure, matching the crate's unwrap-free test idiom.
    fn scratch(name: &str, body: &str) -> Option<(tempfile::TempDir, PathBuf)> {
        let dir = tempfile::tempdir().ok()?;
        let path = dir.path().join(name);
        std::fs::write(&path, body).ok()?;
        Some((dir, path))
    }

    #[test]
    fn absent_files_yield_defaults_without_diagnostics() {
        let (settings, diags) = load_layers([PathBuf::from("/nonexistent/setting.jsonc")]);
        assert_eq!(settings, Settings::default());
        assert!(diags.is_empty());
    }

    #[test]
    fn jsonc_comments_and_trailing_commas_parse() {
        let Some((_dir, path)) = scratch(
            "setting.jsonc",
            r#"{
                // a line comment
                "editor": { "tabSize": 2, }, /* trailing comma + block comment */
            }"#,
        ) else {
            return;
        };
        let (settings, diags) = load_layers([path]);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(settings.editor.tab_size, 2);
        // Unset fields keep their sane defaults.
        assert!(settings.editor.insert_spaces);
    }

    #[test]
    fn later_layers_override_earlier_and_merge_deeply() {
        let Some((dir, _system)) = scratch(
            "system.jsonc",
            r#"{ "editor": { "tabSize": 8, "lineNumbers": "relative" } }"#,
        ) else {
            return;
        };
        let system = dir.path().join("system.jsonc");
        let project = dir.path().join("project.jsonc");
        if std::fs::write(&project, r#"{ "editor": { "tabSize": 2 } }"#).is_err() {
            return;
        }
        // Order is low → high precedence.
        let (settings, diags) = load_layers([system, project]);
        assert!(diags.is_empty(), "{diags:?}");
        // project wins tabSize; system's lineNumbers survives the deep merge.
        assert_eq!(settings.editor.tab_size, 2);
        assert_eq!(settings.editor.line_numbers, LineNumbers::Relative);
    }

    #[test]
    fn report_tracks_loaded_layers_and_explicit_leaf_sources() {
        let Some((dir, _system)) = scratch(
            "system.jsonc",
            r#"{ "editor": { "tabSize": 4, "lineNumbers": "relative" } }"#,
        ) else {
            return;
        };
        let system = dir.path().join("system.jsonc");
        let user = dir.path().join("user.jsonc");
        let project = dir.path().join("project.jsonc");
        if std::fs::write(&project, r#"{ "editor": { "tabSize": 2 } }"#).is_err() {
            return;
        }

        let report = load_layer_reports([
            (ConfigLayer::System, system),
            (ConfigLayer::User, user),
            (ConfigLayer::Project, project),
        ]);
        assert_eq!(report.settings.editor.tab_size, 2);
        assert_eq!(
            report.explicit.get("editor.tabSize"),
            Some(&ConfigLayer::Project)
        );
        assert_eq!(
            report.explicit.get("editor.lineNumbers"),
            Some(&ConfigLayer::System)
        );
        assert_eq!(report.layers.len(), 3);
        assert!(matches!(
            report.layers[1].status,
            ConfigLayerStatus::Missing
        ));
    }

    #[test]
    fn invalid_sections_do_not_remain_explicit() {
        let Some((_dir, path)) = scratch(
            "setting.jsonc",
            r#"{ "editor": { "wat": 1 }, "files": { "backup": true } }"#,
        ) else {
            return;
        };
        let report = load_layer_reports([(ConfigLayer::Project, path)]);
        assert!(!report.explicit.contains_key("editor.wat"));
        assert!(!report.explicit.contains_key("editor.tabSize"));
        assert_eq!(
            report.explicit.get("files.backup"),
            Some(&ConfigLayer::Project)
        );
    }

    #[test]
    fn unknown_key_warns_but_keeps_valid_settings() {
        let Some((_dir, path)) = scratch(
            "setting.jsonc",
            r#"{ "editor": { "tabSize": 3 }, "nonsense": true }"#,
        ) else {
            return;
        };
        let (settings, diags) = load_layers([path]);
        assert_eq!(settings.editor.tab_size, 3);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("unknown setting `nonsense`"));
    }

    #[test]
    fn bad_section_resets_only_itself_and_warns() {
        // `editor.wat` is unknown (deny_unknown_fields) → the editor section is
        // rejected; `files` is untouched and still applies.
        let Some((_dir, path)) = scratch(
            "setting.jsonc",
            r#"{ "editor": { "wat": 1 }, "files": { "autoSave": "afterDelay" } }"#,
        ) else {
            return;
        };
        let (settings, diags) = load_layers([path]);
        assert_eq!(settings.editor, Default::default());
        assert_eq!(settings.files.auto_save, AutoSave::AfterDelay);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("invalid `editor` settings"));
    }

    #[test]
    fn syntax_error_is_reported_and_degrades_to_defaults() {
        let Some((_dir, path)) = scratch("setting.jsonc", "{ this is not json ") else {
            return;
        };
        let (settings, diags) = load_layers([path]);
        assert_eq!(settings, Settings::default());
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("invalid JSONC"));
    }

    #[test]
    fn project_layer_is_discovered_under_the_git_root() {
        let Some(tmp) = tempfile::tempdir().ok() else {
            return;
        };
        let root = tmp.path().join("repo");
        let nested = root.join("crates").join("thing");
        if std::fs::create_dir_all(&nested).is_err()
            || std::fs::create_dir_all(root.join(".git")).is_err()
        {
            return;
        }
        // From a nested workspace root, the project layer resolves to the git root.
        assert_eq!(
            project_config_path(&[nested]),
            Some(root.join(".karet").join("setting.jsonc"))
        );
    }

    #[test]
    fn git_root_is_found_by_ascending() {
        let Some(tmp) = tempfile::tempdir().ok() else {
            return;
        };
        let root = tmp.path().join("repo");
        let nested = root.join("crates").join("thing");
        if std::fs::create_dir_all(&nested).is_err()
            || std::fs::create_dir_all(root.join(".git")).is_err()
        {
            return;
        }
        assert_eq!(git_root(&nested).as_deref(), Some(root.as_path()));
    }
}
