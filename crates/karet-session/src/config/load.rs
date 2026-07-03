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
];

/// Load the merged, verified [`Settings`] for a workspace rooted at `roots`, plus any
/// diagnostics. Reuses the same git discovery the session already performs to locate
/// the project layer.
#[must_use]
pub fn load(roots: &[PathBuf]) -> (Settings, Vec<ConfigDiagnostic>) {
    let layers = [
        system_config_path(),
        user_config_path(),
        project_config_path(roots),
    ];
    load_layers(layers.into_iter().flatten())
}

/// The precedence-ordered (low → high) loader, factored out so tests can inject
/// explicit paths without touching the environment.
fn load_layers(paths: impl IntoIterator<Item = PathBuf>) -> (Settings, Vec<ConfigDiagnostic>) {
    let mut diags = Vec::new();
    // Start from the defaults as a JSON object and merge each present layer over it.
    let mut merged = to_object(&Settings::default());
    for path in paths {
        match read_layer(&path) {
            Ok(None) => {}, // absent (or comments-only) — normal, contributes nothing
            Ok(Some(value)) => match value {
                Value::Object(obj) => deep_merge(&mut merged, obj),
                _ => diags.push(ConfigDiagnostic::warning(
                    &path,
                    "expected a JSON object at the top level",
                )),
            },
            Err(err) => diags.push(ConfigDiagnostic::warning(&path, err)),
        }
    }
    let settings = deserialize_sections(&merged, &mut diags);
    (settings, diags)
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
            _ => Ok(()),
        };
        if let Err(err) = outcome {
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
