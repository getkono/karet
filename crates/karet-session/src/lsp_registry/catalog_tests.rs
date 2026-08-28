//! Release-discovery tests: the shapes upstream registries actually publish.

use super::*;

#[test]
fn npm_latest_metadata_exposes_the_published_executable() -> Result<(), serde_json::Error> {
    let metadata: NpmMetadata = serde_json::from_str(
        r#"{
            "name": "typescript-language-server",
            "version": "5.3.0",
            "bin": {
                "typescript-language-server": "lib/cli.mjs"
            }
        }"#,
    )?;

    assert_eq!(metadata.version, "5.3.0");
    assert_eq!(
        metadata.bin.get("typescript-language-server"),
        Some(&"lib/cli.mjs".to_owned())
    );
    Ok(())
}

#[test]
fn npm_executable_paths_must_stay_inside_the_package() {
    assert!(safe_relative_path("lib/cli.mjs"));
    assert!(safe_relative_path("./bin/nodeServer.js"));
    assert!(!safe_relative_path("../outside.js"));
    assert!(!safe_relative_path("/tmp/outside.js"));
    assert!(!safe_relative_path("./"));
    assert!(!safe_relative_path(""));
}
