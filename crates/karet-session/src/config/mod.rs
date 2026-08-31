//! The layered JSONC configuration system.
//!
//! [`Settings`] is the typed schema; [`load`] discovers and merges the project, user,
//! and system `setting.jsonc` files over the sane defaults, verifying each against the
//! schema and returning any [`ConfigDiagnostic`]s. [`json_schema`] emits the external
//! `settings.schema.json` (referenced by a file's `"$schema"` for editor
//! autocomplete) from the same [`Settings`] type, so the schema can never drift from
//! the parser.

pub mod load;
pub mod schema;

use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use jsonc_parser::cst::CstRootNode;
use jsonc_parser::json;
pub use load::ConfigDiagnostic;
pub use load::ConfigLayer;
pub use load::ConfigLayerReport;
pub use load::ConfigLayerStatus;
pub use load::LoadedConfig;
pub use load::load;
pub use load::load_report;
pub use schema::Seam;
pub use schema::SeamLensFilter;
pub use schema::SeamSpine;
pub use schema::Settings;

/// Errors while updating a user-owned JSONC setting.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigWriteError {
    /// The platform has no discoverable user configuration directory.
    #[error("user configuration directory is unavailable")]
    NoUserDirectory,
    /// Existing JSONC could not be parsed safely.
    #[error("invalid configuration: {0}")]
    Parse(String),
    /// None of the workspace roots belongs to a Git worktree, so there is no
    /// project settings location.
    #[error("workspace is not inside a Git worktree")]
    NoProjectDirectory,
    /// A project settings file is absent and the caller did not explicitly confirm
    /// creating it.
    #[error("creating project configuration at {} requires confirmation", .0.display())]
    ProjectCreationRequiresConfirmation(PathBuf),
    /// Reading, writing, or atomically replacing the file failed.
    #[error("configuration I/O failed: {0}")]
    Io(String),
}

/// Persist live-blame settings in the user layer while retaining JSONC comments and
/// unrelated formatting. Returns the updated file path.
///
/// # Errors
/// Returns [`ConfigWriteError`] when the user path is unavailable, the existing file
/// is invalid JSONC, or the atomic write fails.
pub fn set_user_blame(enabled: bool) -> Result<PathBuf, ConfigWriteError> {
    let path = load::user_config_path().ok_or(ConfigWriteError::NoUserDirectory)?;
    let current = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "{}\n".to_string(),
        Err(error) => return Err(ConfigWriteError::Io(error.to_string())),
    };
    let updated = update_blame_jsonc(&current, enabled)?;
    atomic_write(&path, updated.as_bytes())?;
    Ok(path)
}

/// Persist `git.aiCommit.*` in the user layer while retaining JSONC comments and
/// unrelated formatting. Returns the updated file path.
///
/// Only the keys that differ from the defaults are written: a settings file
/// should record what the user chose, not restate the whole schema back at them.
/// A key that returns to its default is removed rather than pinned, so a later
/// change of default is still inherited.
///
/// # Errors
/// Returns [`ConfigWriteError`] when the user path is unavailable, the existing file
/// is invalid JSONC, or the atomic write fails.
pub fn set_user_ai_commit(options: &schema::AiCommit) -> Result<PathBuf, ConfigWriteError> {
    let path = load::user_config_path().ok_or(ConfigWriteError::NoUserDirectory)?;
    let current = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "{}\n".to_string(),
        Err(error) => return Err(ConfigWriteError::Io(error.to_string())),
    };
    let updated = update_ai_commit_jsonc(&current, options)?;
    atomic_write(&path, updated.as_bytes())?;
    Ok(path)
}

fn update_ai_commit_jsonc(
    text: &str,
    options: &schema::AiCommit,
) -> Result<String, ConfigWriteError> {
    let defaults = schema::AiCommit::default();
    let root = CstRootNode::parse(text, &Default::default())
        .map_err(|error| ConfigWriteError::Parse(error.to_string()))?;
    let object = root.object_value_or_set();
    let git = object.object_value_or_set("git");
    let ai = git.object_value_or_set("aiCommit");

    // `set` writes a key, or removes it when the value is back to its default.
    let set =
        |key: &str, value: Option<jsonc_parser::cst::CstInputValue>| match (ai.get(key), value) {
            (Some(property), Some(value)) => property.set_value(value),
            (Some(property), None) => property.remove(),
            (None, Some(value)) => {
                ai.append(key, value);
            },
            (None, None) => {},
        };
    set(
        "enabled",
        (options.enabled != defaults.enabled).then(|| json!(options.enabled)),
    );
    set(
        "agent",
        (options.agent != defaults.agent).then(|| json!(options.agent.as_str())),
    );
    set(
        "model",
        (options.model != defaults.model).then(|| json!(options.model.clone())),
    );
    set(
        "effort",
        options.effort.map(|effort| json!(effort.as_str())),
    );
    set(
        "timeoutMs",
        (options.timeout_ms != defaults.timeout_ms).then(|| json!(options.timeout_ms as f64)),
    );
    set(
        "binary",
        options
            .binary
            .as_ref()
            .filter(|path| !path.trim().is_empty())
            .map(|path| json!(path.clone())),
    );
    set(
        "instructions",
        (options.instructions != defaults.instructions).then(|| {
            jsonc_parser::cst::CstInputValue::Array(
                options
                    .instructions
                    .iter()
                    .map(|line| json!(line.clone()))
                    .collect(),
            )
        }),
    );
    Ok(root.to_string())
}

/// Add `word` to the user-layer spell-check dictionary while preserving JSONC
/// comments and unrelated settings. A missing user settings file is created.
///
/// # Errors
/// Returns [`ConfigWriteError`] when the user path is unavailable, the existing file
/// has an incompatible JSON shape, or the atomic write fails.
pub fn add_user_dictionary_word(word: &str) -> Result<PathBuf, ConfigWriteError> {
    if word.trim().is_empty() {
        return Err(ConfigWriteError::Parse(
            "dictionary word cannot be empty".to_string(),
        ));
    }
    let path = load::user_config_path().ok_or(ConfigWriteError::NoUserDirectory)?;
    add_dictionary_word_at(&path, word)
}

/// Add `word` to the project-layer spell-check dictionary while preserving JSONC
/// comments and unrelated settings.
///
/// The project file is updated directly when it already exists. When it is missing,
/// this function refuses to create it unless `allow_create` is `true`, making the
/// caller's confirmation step explicit and fail-safe.
///
/// # Errors
/// Returns [`ConfigWriteError`] when no project layer can be resolved, creation was
/// not confirmed, the existing file has an incompatible JSON shape, or the atomic
/// write fails.
pub fn add_project_dictionary_word(
    roots: &[PathBuf],
    word: &str,
    allow_create: bool,
) -> Result<PathBuf, ConfigWriteError> {
    let path = load::project_config_path(roots).ok_or(ConfigWriteError::NoProjectDirectory)?;
    let current = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !allow_create => {
            return Err(ConfigWriteError::ProjectCreationRequiresConfirmation(path));
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "{}\n".to_string(),
        Err(error) => return Err(ConfigWriteError::Io(error.to_string())),
    };
    write_dictionary_word(&path, &current, word)?;
    Ok(path)
}

fn add_dictionary_word_at(path: &Path, word: &str) -> Result<PathBuf, ConfigWriteError> {
    let current = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "{}\n".to_string(),
        Err(error) => return Err(ConfigWriteError::Io(error.to_string())),
    };
    write_dictionary_word(path, &current, word)?;
    Ok(path.to_path_buf())
}

fn write_dictionary_word(path: &Path, current: &str, word: &str) -> Result<(), ConfigWriteError> {
    let updated = update_dictionary_jsonc(current, word)?;
    atomic_write(path, updated.as_bytes())
}

fn update_blame_jsonc(text: &str, enabled: bool) -> Result<String, ConfigWriteError> {
    let root = CstRootNode::parse(text, &Default::default())
        .map_err(|error| ConfigWriteError::Parse(error.to_string()))?;
    let object = root.object_value_or_set();
    let git = object.object_value_or_set("git");
    if let Some(property) = git.get("blame") {
        property.set_value(json!(enabled));
    } else {
        git.append("blame", json!(enabled));
    }
    if let Some(property) = git.get("blameMode") {
        property.remove();
    }
    Ok(root.to_string())
}

fn update_dictionary_jsonc(text: &str, word: &str) -> Result<String, ConfigWriteError> {
    if word.trim().is_empty() {
        return Err(ConfigWriteError::Parse(
            "dictionary word cannot be empty".to_string(),
        ));
    }
    let root = CstRootNode::parse(text, &Default::default())
        .map_err(|error| ConfigWriteError::Parse(error.to_string()))?;
    let object = root.object_value_or_create().ok_or_else(|| {
        ConfigWriteError::Parse("expected a JSON object at the top level".to_string())
    })?;
    let spellcheck = object
        .object_value_or_create("spellcheck")
        .ok_or_else(|| ConfigWriteError::Parse("`spellcheck` must be a JSON object".to_string()))?;
    let words = spellcheck.array_value_or_create("words").ok_or_else(|| {
        ConfigWriteError::Parse("`spellcheck.words` must be a JSON array".to_string())
    })?;
    let already_present = words.elements().iter().any(|element| {
        element
            .as_string_lit()
            .and_then(|value| value.decoded_value().ok())
            .is_some_and(|existing| existing.eq_ignore_ascii_case(word))
    });
    if !already_present {
        words.append(json!(word));
    }
    Ok(root.to_string())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ConfigWriteError> {
    let parent = path.parent().ok_or_else(|| {
        ConfigWriteError::Io("configuration path has no parent directory".to_string())
    })?;
    std::fs::create_dir_all(parent).map_err(|error| ConfigWriteError::Io(error.to_string()))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| ConfigWriteError::Io(error.to_string()))?;
    temp.write_all(bytes)
        .and_then(|()| temp.flush())
        .map_err(|error| ConfigWriteError::Io(error.to_string()))?;
    temp.persist(path)
        .map_err(|error| ConfigWriteError::Io(error.error.to_string()))?;
    Ok(())
}

/// The JSON Schema for [`Settings`], pretty-printed. This is the single source the
/// checked-in `settings.schema.json` is generated from; a test asserts they match.
#[must_use]
#[cfg(feature = "schema")]
pub fn json_schema() -> String {
    let schema = schemars::schema_for!(Settings);
    // Serializing a generated schema cannot fail; fall back to an empty object.
    serde_json::to_string_pretty(&schema).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "schema")]
    fn json_schema_describes_the_settings_sections() {
        let schema = json_schema();
        assert!(schema.contains("\"editor\""));
        assert!(schema.contains("\"tabSize\""));
        assert!(schema.contains("\"formatOnSave\""));
    }

    #[test]
    fn blame_update_preserves_comments_and_unrelated_settings() -> Result<(), ConfigWriteError> {
        let source = r#"{
  // retain this explanation
  "editor": { "tabSize": 2 },
  "git": { "decorations": false, "blameMode": "line" }
}"#;
        let updated = update_blame_jsonc(source, true)?;
        assert!(updated.contains("// retain this explanation"));
        assert!(updated.contains("\"tabSize\": 2"));
        assert!(updated.contains("\"decorations\": false"));
        assert!(updated.contains("\"blame\": true"));
        assert!(!updated.contains("blameMode"));
        Ok(())
    }

    #[test]
    fn project_dictionary_requires_confirmation_before_creating_settings()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join(".git"))?;
        let path = dir.path().join(".karet/setting.jsonc");

        let error = add_project_dictionary_word(&[dir.path().to_path_buf()], "Karet", false).err();

        assert!(matches!(
            error,
            Some(ConfigWriteError::ProjectCreationRequiresConfirmation(candidate))
                if candidate == path
        ));
        assert!(!path.exists(), "refusal must not create the settings tree");
        Ok(())
    }

    #[test]
    fn confirmed_project_dictionary_creation_writes_a_valid_layer()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join(".git"))?;

        let path = add_project_dictionary_word(&[dir.path().to_path_buf()], "Karet", true)?;
        let text = std::fs::read_to_string(&path)?;
        let parsed: Option<serde_json::Value> =
            jsonc_parser::parse_to_serde_value(&text, &Default::default())?;

        assert_eq!(
            parsed.and_then(|value| value.pointer("/spellcheck/words/0").cloned()),
            Some(serde_json::Value::String("Karet".to_string()))
        );
        Ok(())
    }

    #[test]
    fn project_dictionary_update_preserves_jsonc_and_avoids_case_duplicates()
    -> Result<(), ConfigWriteError> {
        let source = r#"{
  // project convention
  "editor": { "tabSize": 2 },
  "spellcheck": { "enabled": true, "words": ["Karet"] }
}"#;

        let updated = update_dictionary_jsonc(source, "karet")?;

        assert!(updated.contains("// project convention"));
        assert!(updated.contains("\"tabSize\": 2"));
        assert_eq!(updated.matches("\"Karet\"").count(), 1);
        assert!(!updated.contains("\"karet\""));
        Ok(())
    }

    #[test]
    fn user_dictionary_update_creates_a_layer_and_preserves_jsonc()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("karet/setting.jsonc");
        std::fs::create_dir_all(path.parent().unwrap_or(dir.path()))?;
        std::fs::write(
            &path,
            "{\n  // personal words\n  \"editor\": { \"tabSize\": 2 }\n}\n",
        )?;

        let written = add_dictionary_word_at(&path, "Karet")?;
        let text = std::fs::read_to_string(&path)?;

        assert_eq!(written, path);
        assert!(text.contains("// personal words"));
        assert!(text.contains("\"tabSize\": 2"));
        assert!(text.contains("\"Karet\""));
        Ok(())
    }

    #[test]
    fn ai_commit_writer_records_only_what_differs_from_the_defaults() -> Result<(), ConfigWriteError>
    {
        let source = "{\n  // keep me\n  \"editor\": { \"tabSize\": 2 },\n}\n";
        let options = schema::AiCommit {
            agent: schema::AiCommitAgent::Codex,
            effort: Some(schema::AiCommitEffort::High),
            ..schema::AiCommit::default()
        };

        let updated = update_ai_commit_jsonc(source, &options)?;
        assert!(
            updated.contains("// keep me"),
            "comments survive: {updated}"
        );
        assert!(updated.contains("\"tabSize\""), "unrelated keys survive");
        assert!(updated.contains("\"agent\""), "the changed key is written");
        assert!(updated.contains("codex"), "{updated}");
        assert!(updated.contains("\"effort\""), "{updated}");
        // Defaults are not restated back at the user, so a later change of
        // default is still inherited rather than silently pinned.
        assert!(
            !updated.contains("\"enabled\""),
            "default omitted: {updated}"
        );
        assert!(!updated.contains("\"model\""), "default omitted: {updated}");
        assert!(!updated.contains("timeoutMs"), "default omitted: {updated}");
        assert!(!updated.contains("\"binary\""), "unset omitted: {updated}");
        Ok(())
    }

    #[test]
    fn ai_commit_writer_removes_a_key_that_returns_to_its_default() -> Result<(), ConfigWriteError>
    {
        let source =
            "{ \"git\": { \"aiCommit\": { \"agent\": \"codex\", \"effort\": \"high\" } } }";
        // Back to stock: both keys must disappear rather than be pinned.
        let updated = update_ai_commit_jsonc(source, &schema::AiCommit::default())?;
        assert!(!updated.contains("codex"), "{updated}");
        assert!(!updated.contains("\"effort\""), "{updated}");
        Ok(())
    }

    #[test]
    fn ai_commit_writer_round_trips_through_the_settings_parser() -> Result<(), ConfigWriteError> {
        let options = schema::AiCommit {
            agent: schema::AiCommitAgent::Codex,
            effort: Some(schema::AiCommitEffort::XHigh),
            model: "gpt-5".to_string(),
            instructions: vec!["be terse".to_string()],
            binary: Some("/opt/bin/codex".to_string()),
            timeout_ms: 30_000,
            ..schema::AiCommit::default()
        };

        // What we write must be what we read back — the property that keeps the
        // form's saved state and the running configuration from diverging.
        let updated = update_ai_commit_jsonc("{}", &options)?;
        let value: Option<serde_json::Value> =
            jsonc_parser::parse_to_serde_value(&updated, &Default::default())
                .map_err(|e| ConfigWriteError::Parse(e.to_string()))?;
        let section = value
            .as_ref()
            .and_then(|root| root.get("git"))
            .and_then(|git| git.get("aiCommit"))
            .cloned()
            .unwrap_or_default();
        let parsed: schema::AiCommit =
            serde_json::from_value(section).map_err(|e| ConfigWriteError::Parse(e.to_string()))?;
        assert_eq!(parsed, options);
        Ok(())
    }

    #[test]
    fn public_user_dictionary_writer_rejects_empty_words_before_path_discovery() {
        assert!(matches!(
            add_user_dictionary_word("  "),
            Err(ConfigWriteError::Parse(message)) if message == "dictionary word cannot be empty"
        ));
    }

    /// Guards the checked-in schema against drift: regenerate with
    /// `cargo run -p karet --bin karet -- ...` is not needed — the schema is emitted
    /// by this crate, so if this fails, refresh `settings.schema.json` from
    /// [`json_schema`]. Skipped when the file is absent (e.g. isolated crate checkout).
    #[cfg(feature = "schema")]
    #[test]
    fn checked_in_schema_is_current() {
        let repo_schema = concat!(env!("CARGO_MANIFEST_DIR"), "/../../settings.schema.json");
        let Ok(on_disk) = std::fs::read_to_string(repo_schema) else {
            return;
        };
        // Compared as parsed values, not strings: JSON object ordering swings
        // with `serde_json/preserve_order`, which workspace feature
        // unification toggles depending on which crates are in the build.
        let on_disk: serde_json::Value = serde_json::from_str(&on_disk).unwrap_or_default();
        let generated: serde_json::Value = serde_json::from_str(&json_schema()).unwrap_or_default();
        assert!(
            on_disk == generated && on_disk != serde_json::Value::default(),
            "settings.schema.json is stale — regenerate it from config::json_schema()"
        );
    }
}
