//! Builds the reviewed GitHub REST wire client into `OUT_DIR`.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::io;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Map;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;

const SPEC: &str = "openapi/api.github.com.json";
const GENERATED_OPERATIONS: &str = "openapi/generated-operations.json";
const MANUAL_OPERATIONS: &str = "openapi/manual-operations.json";
const SURFACE_OPERATIONS: &str = "openapi/surface-operations.json";
const EXPECTED_SPEC_SHA256: &str =
    "d88008d8198becda210d59fbe64a6554bcc4c979be2348e2e356638b369eee47";
const SPARGEN_ISSUE_PREFIX: &str = "https://github.com/getkono/spargen/issues/";

#[derive(Debug, Deserialize)]
struct ManualOperation {
    #[serde(rename = "operationId")]
    operation_id: String,
    diagnostic: String,
    upstream: Option<String>,
}

fn main() -> Result<(), Box<dyn Error>> {
    for path in [
        SPEC,
        GENERATED_OPERATIONS,
        MANUAL_OPERATIONS,
        SURFACE_OPERATIONS,
    ] {
        println!("cargo:rerun-if-changed={path}");
    }

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let spec_path = manifest_dir.join(SPEC);
    let spec_bytes = fs::read(&spec_path)?;
    verify_checksum(&spec_bytes)?;

    let document: Value = serde_json::from_slice(&spec_bytes)?;
    let generated = read_string_list(&manifest_dir.join(GENERATED_OPERATIONS))?;
    let reviewed = read_string_list(&manifest_dir.join(SURFACE_OPERATIONS))?;
    let manual = read_manual_operations(&manifest_dir.join(MANUAL_OPERATIONS))?;
    let generated = unique_set("generated operations", generated)?;
    let reviewed = unique_set("reviewed surface", reviewed)?;
    let manual = validate_manual_operations(manual)?;
    validate_partition(&generated, &manual, &reviewed)?;

    let filtered = filter_document(&document, &generated)?;
    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
    let surface_path = out_dir.join("github-openapi.json");
    fs::write(&surface_path, serde_json::to_vec(&filtered)?)?;
    generate_client(&surface_path, &out_dir.join("github-generated.rs"))
}

fn build_error(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}

fn verify_checksum(bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual != EXPECTED_SPEC_SHA256 {
        return Err(build_error(format!(
            "GitHub OpenAPI checksum mismatch: expected {EXPECTED_SPEC_SHA256}, got {actual}; \
             review the upstream change and update build.rs plus openapi/README.md"
        ))
        .into());
    }
    Ok(())
}

fn read_string_list(path: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn read_manual_operations(path: &Path) -> Result<Vec<ManualOperation>, Box<dyn Error>> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn unique_set(label: &str, values: Vec<String>) -> Result<BTreeSet<String>, Box<dyn Error>> {
    let count = values.len();
    let set: BTreeSet<_> = values.into_iter().collect();
    if set.len() != count {
        return Err(build_error(format!("{label} contains duplicate operation IDs")).into());
    }
    Ok(set)
}

fn validate_manual_operations(
    operations: Vec<ManualOperation>,
) -> Result<BTreeSet<String>, Box<dyn Error>> {
    let count = operations.len();
    let mut ids = BTreeSet::new();
    for operation in operations {
        if operation.diagnostic.trim().is_empty() {
            return Err(build_error(format!(
                "manual operation {} has no diagnostic",
                operation.operation_id
            ))
            .into());
        }
        let upstream = operation.upstream.as_deref().ok_or_else(|| {
            build_error(format!(
                "manual operation {} has no upstream spargen issue",
                operation.operation_id
            ))
        })?;
        if !upstream.starts_with(SPARGEN_ISSUE_PREFIX) {
            return Err(build_error(format!(
                "manual operation {} has an invalid upstream issue URL: {upstream}",
                operation.operation_id
            ))
            .into());
        }
        ids.insert(operation.operation_id);
    }
    if ids.len() != count {
        return Err(build_error("manual operations contains duplicate operation IDs").into());
    }
    Ok(ids)
}

fn validate_partition(
    generated: &BTreeSet<String>,
    manual: &BTreeSet<String>,
    reviewed: &BTreeSet<String>,
) -> Result<(), Box<dyn Error>> {
    let overlap: Vec<_> = generated.intersection(manual).cloned().collect();
    if !overlap.is_empty() {
        return Err(build_error(format!(
            "operations cannot be both generated and manual: {}",
            overlap.join(", ")
        ))
        .into());
    }
    let partition: BTreeSet<_> = generated.union(manual).cloned().collect();
    if &partition != reviewed {
        let missing: Vec<_> = reviewed.difference(&partition).cloned().collect();
        let extra: Vec<_> = partition.difference(reviewed).cloned().collect();
        return Err(build_error(format!(
            "generated/manual manifests do not partition the reviewed surface; missing: [{}], \
             extra: [{}]",
            missing.join(", "),
            extra.join(", ")
        ))
        .into());
    }
    Ok(())
}

fn filter_document(document: &Value, allowed: &BTreeSet<String>) -> Result<Value, Box<dyn Error>> {
    let root = document
        .as_object()
        .ok_or_else(|| build_error("GitHub OpenAPI root must be an object"))?;
    let source_paths = root
        .get("paths")
        .and_then(Value::as_object)
        .ok_or_else(|| build_error("GitHub OpenAPI document has no paths object"))?;

    let source_operations = collect_operation_ids(source_paths)?;
    let missing: Vec<_> = allowed.difference(&source_operations).cloned().collect();
    if !missing.is_empty() {
        return Err(build_error(format!(
            "generated operations missing from vendored GitHub OpenAPI: {}",
            missing.join(", ")
        ))
        .into());
    }

    let mut selected_paths = Map::new();
    let mut found = BTreeSet::new();
    for (path, item) in source_paths {
        let item = item
            .as_object()
            .ok_or_else(|| build_error(format!("path item {path} must be an object")))?;
        let mut selected_item = Map::new();
        let mut has_operation = false;
        for (key, value) in item {
            if let Some(operation_id) = value.get("operationId").and_then(Value::as_str) {
                if allowed.contains(operation_id) {
                    selected_item.insert(key.clone(), value.clone());
                    found.insert(operation_id.to_owned());
                    has_operation = true;
                }
            } else if matches!(
                key.as_str(),
                "parameters" | "summary" | "description" | "servers"
            ) {
                selected_item.insert(key.clone(), value.clone());
            }
        }
        if has_operation {
            selected_paths.insert(path.clone(), Value::Object(selected_item));
        }
    }
    if &found != allowed {
        return Err(build_error("not every generated operation survived path filtering").into());
    }

    let paths = Value::Object(selected_paths);
    let components = referenced_components(document, &paths)?;
    let mut filtered = Map::new();
    copy_root_field(root, &mut filtered, "openapi", true)?;
    copy_root_field(root, &mut filtered, "jsonSchemaDialect", false)?;
    copy_root_field(root, &mut filtered, "info", true)?;
    copy_root_field(root, &mut filtered, "servers", false)?;
    copy_root_field(root, &mut filtered, "security", false)?;
    filtered.insert("paths".to_owned(), paths);
    filtered.insert("components".to_owned(), Value::Object(components));
    Ok(Value::Object(filtered))
}

fn collect_operation_ids(paths: &Map<String, Value>) -> Result<BTreeSet<String>, Box<dyn Error>> {
    let mut ids = BTreeSet::new();
    for (path, item) in paths {
        let item = item
            .as_object()
            .ok_or_else(|| build_error(format!("path item {path} must be an object")))?;
        for value in item.values() {
            if let Some(operation_id) = value.get("operationId").and_then(Value::as_str)
                && !ids.insert(operation_id.to_owned())
            {
                return Err(build_error(format!(
                    "duplicate operationId in GitHub OpenAPI: {operation_id}"
                ))
                .into());
            }
        }
    }
    Ok(ids)
}

fn copy_root_field(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
    required: bool,
) -> Result<(), Box<dyn Error>> {
    if let Some(value) = source.get(key) {
        target.insert(key.to_owned(), value.clone());
    } else if required {
        return Err(build_error(format!("GitHub OpenAPI document has no {key} field")).into());
    }
    Ok(())
}

fn referenced_components(
    document: &Value,
    paths: &Value,
) -> Result<Map<String, Value>, Box<dyn Error>> {
    let mut pending = BTreeSet::new();
    collect_component_refs(paths, &mut pending);
    let mut visited = BTreeSet::new();
    while let Some(reference) = pending.pop_first() {
        if !visited.insert(reference.clone()) {
            continue;
        }
        let pointer = reference
            .strip_prefix('#')
            .ok_or_else(|| build_error(format!("invalid component reference: {reference}")))?;
        let target = document
            .pointer(pointer)
            .ok_or_else(|| build_error(format!("unresolved component reference: {reference}")))?;
        collect_component_refs(target, &mut pending);
    }

    let source_components = document
        .get("components")
        .and_then(Value::as_object)
        .ok_or_else(|| build_error("GitHub OpenAPI document has no components object"))?;
    let mut selected: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for reference in visited {
        let suffix = reference
            .strip_prefix("#/components/")
            .ok_or_else(|| build_error(format!("invalid component reference: {reference}")))?;
        let mut segments = suffix.split('/');
        let kind = segments
            .next()
            .ok_or_else(|| build_error(format!("invalid component reference: {reference}")))?;
        let name = segments
            .next()
            .ok_or_else(|| build_error(format!("invalid component reference: {reference}")))?;
        selected
            .entry(decode_pointer_token(kind)?)
            .or_default()
            .insert(decode_pointer_token(name)?);
    }

    let mut components = Map::new();
    for (kind, names) in selected {
        let source_kind = source_components
            .get(&kind)
            .and_then(Value::as_object)
            .ok_or_else(|| build_error(format!("missing component group: {kind}")))?;
        let mut target_kind = Map::new();
        for name in names {
            let value = source_kind
                .get(&name)
                .ok_or_else(|| build_error(format!("missing component: {kind}/{name}")))?;
            target_kind.insert(name, value.clone());
        }
        components.insert(kind, Value::Object(target_kind));
    }
    Ok(components)
}

fn collect_component_refs(value: &Value, refs: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str)
                && reference.starts_with("#/components/")
            {
                refs.insert(reference.to_owned());
            }
            for child in object.values() {
                collect_component_refs(child, refs);
            }
        },
        Value::Array(array) => {
            for child in array {
                collect_component_refs(child, refs);
            }
        },
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {},
    }
}

fn decode_pointer_token(token: &str) -> Result<String, Box<dyn Error>> {
    let mut decoded = String::with_capacity(token.len());
    let mut chars = token.chars();
    while let Some(ch) = chars.next() {
        if ch != '~' {
            decoded.push(ch);
            continue;
        }
        match chars.next() {
            Some('0') => decoded.push('~'),
            Some('1') => decoded.push('/'),
            Some(other) => {
                return Err(build_error(format!("invalid JSON Pointer escape: ~{other}")).into());
            },
            None => return Err(build_error("trailing ~ in JSON Pointer token").into()),
        }
    }
    Ok(decoded)
}

fn generate_client(surface: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    let surface = surface
        .to_str()
        .ok_or_else(|| build_error("OUT_DIR surface path is not UTF-8"))?;
    let output = output
        .to_str()
        .ok_or_else(|| build_error("OUT_DIR generated path is not UTF-8"))?;
    let mut config = spargen::Config::new(surface, spargen::OutputTarget::Module(output.into()));
    config.features.uuid = false;
    let report = spargen::generate(&config);
    for diagnostic in &report.diagnostics {
        println!(
            "cargo:warning=spargen {} at {}: {}",
            diagnostic.code, diagnostic.pointer, diagnostic.message
        );
    }
    if report.outcome != spargen::Outcome::Generated {
        return Err(build_error(format!("spargen failed: {report:#?}")).into());
    }
    Ok(())
}
