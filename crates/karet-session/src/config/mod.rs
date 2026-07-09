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

pub use load::ConfigDiagnostic;
pub use load::ConfigLayer;
pub use load::ConfigLayerReport;
pub use load::ConfigLayerStatus;
pub use load::LoadedConfig;
pub use load::load;
pub use load::load_report;
pub use schema::Settings;

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
