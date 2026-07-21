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
pub use schema::Settings;

/// Errors while updating a user-owned JSONC setting.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigWriteError {
    /// The platform has no discoverable user configuration directory.
    #[error("user configuration directory is unavailable")]
    NoUserDirectory,
    /// Existing JSONC could not be parsed safely.
    #[error("invalid user configuration: {0}")]
    Parse(String),
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
pub fn json_schema() -> String {
    let schema = schemars::schema_for!(Settings);
    // Serializing a generated schema cannot fail; fall back to an empty object.
    serde_json::to_string_pretty(&schema).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
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

    /// Guards the checked-in schema against drift: regenerate with
    /// `cargo run -p karet --bin karet -- ...` is not needed — the schema is emitted
    /// by this crate, so if this fails, refresh `settings.schema.json` from
    /// [`json_schema`]. Skipped when the file is absent (e.g. isolated crate checkout).
    #[test]
    fn checked_in_schema_is_current() {
        let repo_schema = concat!(env!("CARGO_MANIFEST_DIR"), "/../../settings.schema.json");
        let Ok(on_disk) = std::fs::read_to_string(repo_schema) else {
            return;
        };
        assert_eq!(
            on_disk.trim_end(),
            json_schema().trim_end(),
            "settings.schema.json is stale — regenerate it from config::json_schema()"
        );
    }
}
