//! Structured file identity tests kept beside the registry.

use std::path::Path;

use super::file_type_for_path;

#[test]
fn structured_named_files_route_to_native_grammar_identities() {
    for (path, grammar) in [
        (".editorconfig", "ini"),
        (".gitmodules", "ini"),
        (".env", "properties"),
        ("Cargo.lock", "toml"),
        ("poetry.lock", "toml"),
        ("package-lock.json", "json"),
        ("composer.lock", "json"),
        ("Pipfile.lock", "json"),
        ("pnpm-lock.yaml", "yaml"),
        ("yarn.lock", "lockfile"),
    ] {
        assert_eq!(
            file_type_for_path(Path::new(path)).grammar(),
            Some(grammar),
            "{path}"
        );
    }
}

#[test]
fn unsupported_ksh_is_labelled_without_claiming_a_parser() {
    let ksh = file_type_for_path(Path::new("script.ksh"));
    assert_eq!(ksh.name(), "Ksh");
    assert_eq!(ksh.grammar(), None);
    assert_eq!(ksh.lsp_language_id(), None);
    assert_eq!(ksh.config_selector(), None);
}
