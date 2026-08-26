//! The `--seam-query` surface: the Seam index as something a program can ask.
//!
//! The view is a renderer over a queryable index, and the index is the product. So the
//! same query language the filter box takes is available without a terminal, answering
//! with the same node set the view would show.
//!
//! Results carry node identities, locations, facets, and rollup counts — enough to cite a
//! finding, to re-navigate to it, and to judge seam density without materializing a
//! subtree. The identity is the citation unit precisely because it survives edits that do
//! not rename or reparent: a line number in a report goes stale by lunchtime.
//!
//! This runs the index directly rather than through the backend actor: there is no
//! session, no event loop, and nothing to keep alive for a single question.

use std::path::Path;

use karet_seam::IndexOptions;
use karet_seam::LENSES;
use karet_seam::SeamIndex;

/// Why a `--seam-query` run failed, and what to print.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub(crate) struct QueryFailure {
    /// The message for stderr.
    pub(crate) message: String,
    /// The process exit code.
    pub(crate) code: i32,
}

/// Answer one query against everything under `root`, printing JSON to stdout.
///
/// # Errors
/// Returns the message and exit code to fail with: the package could not be indexed,
/// or the query could not be read.
pub(crate) fn run(
    root: &Path,
    query: &str,
    configuration: Option<&str>,
) -> Result<String, QueryFailure> {
    let mut index =
        karet_seam::index_workspace(root, IndexOptions::default()).map_err(|error| {
            QueryFailure {
                message: format!("--seam-query: {error}"),
                code: 2,
            }
        })?;

    // A named configuration changes what the tree even contains, so it is applied
    // before the query rather than filtered afterwards.
    let active = configuration.map_or_else(karet_seam::Configuration::unconfigured, |name| {
        karet_seam::Configuration::named(name, Vec::new())
    });
    karet_seam::config::apply(&mut index, &active);

    let parsed = karet_seam::query::parse(query).map_err(|error| QueryFailure {
        // Positioned, so a caller can point at the offending term rather than
        // guessing which one this crate disliked.
        message: format!(
            "--seam-query: {} (at byte {}..{})",
            error.describe(),
            error.span.start,
            error.span.end
        ),
        code: 2,
    })?;

    let result = karet_seam::query::evaluate(&parsed, &index);
    Ok(render(&index, &result.nodes, query, &active.name))
}

/// Render a result set as JSON.
///
/// Hand-built rather than derived: this is an output format for other programs, and
/// keeping it written out means a change to it is visible in a diff rather than a
/// side effect of a field rename somewhere in the engine.
fn render(
    index: &SeamIndex,
    nodes: &[karet_seam::SeamId],
    query: &str,
    configuration: &str,
) -> String {
    let mut out = String::from("{\n");
    out.push_str(&format!("  \"configuration\": {},\n", quote(configuration)));
    out.push_str(&format!("  \"query\": {},\n", quote(query)));
    out.push_str(&format!("  \"matched\": {},\n", nodes.len()));
    out.push_str("  \"nodes\": [\n");
    let rendered: Vec<String> = nodes
        .iter()
        .filter_map(|id| node_json(index, *id))
        .collect();
    out.push_str(&rendered.join(",\n"));
    if !rendered.is_empty() {
        out.push('\n');
    }
    out.push_str("  ]\n}");
    out
}

/// One node as a JSON object.
fn node_json(index: &SeamIndex, id: karet_seam::SeamId) -> Option<String> {
    let node = index.node(id)?;
    let path = index.path(id)?.to_string();
    let file = index
        .file_path(node.location.file)
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    let facets: Vec<String> = node
        .facets
        .iter()
        .map(|facet| {
            let detail = facet
                .detail
                .as_ref()
                .map_or_else(|| "null".to_owned(), |d| quote(d));
            format!(
                "{{\"lens\": {}, \"subtype\": {}, \"detail\": {detail}, \"occurrences\": {}}}",
                quote(facet.lens.name()),
                quote(facet.subtype.name()),
                facet.occurrences()
            )
        })
        .collect();

    // Rollups travel with every node so seam density is readable without walking
    // the subtree, which is what makes "where is the risk concentrated" a cheap question.
    let rollups: Vec<String> = LENSES
        .iter()
        .map(|lens| format!("{}: {}", quote(lens.name()), node.rollups.get(*lens)))
        .collect();

    Some(format!(
        "    {{\"id\": {}, \"name\": {}, \"kind\": {}, \"file\": {}, \"line\": {}, \
         \"visibility\": {}, \"membership\": {}, \"facets\": [{}], \"rollups\": {{{}}}}}",
        quote(&path),
        quote(&node.name),
        quote(node.kind.name()),
        quote(&file),
        node.location.range.start.line.saturating_add(1),
        node.effective_visibility()
            .map_or_else(|| "null".to_owned(), |v| quote(v.name())),
        quote(node.membership.name()),
        facets.join(", "),
        rollups.join(", ")
    ))
}

/// A JSON string literal, escaping what the format requires.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_are_escaped_for_json() {
        assert_eq!(quote("plain"), "\"plain\"");
        assert_eq!(quote("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote("a\\b"), "\"a\\\\b\"");
        assert_eq!(quote("a\nb"), "\"a\\nb\"");
        // A path or identifier could carry anything; control characters must not
        // produce output no JSON parser will read.
        assert_eq!(quote("a\u{1}b"), "\"a\\u0001b\"");
    }

    #[test]
    fn an_unreadable_query_fails_with_a_positioned_message()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"q\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::create_dir_all(dir.path().join("src"))?;
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn f() {}\n")?;

        let Err(failure) = run(dir.path(), "lens:hazrd", None) else {
            return Err("expected the query to be rejected".into());
        };
        assert!(
            failure.message.contains("unknown lens"),
            "{}",
            failure.message
        );
        assert!(failure.message.contains("at byte"), "{}", failure.message);
        assert_eq!(failure.code, 2);
        Ok(())
    }

    #[test]
    fn a_missing_package_fails_rather_than_printing_an_empty_result()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        // Empty output would read as "this package has no seams", which is not the
        // answer — there is no package here.
        let Err(failure) = run(dir.path(), "lens:api", None) else {
            return Err("expected indexing to fail".into());
        };
        assert_eq!(failure.code, 2);
        Ok(())
    }

    #[test]
    fn a_result_carries_identities_locations_facets_and_rollups()
    -> Result<(), Box<dyn std::error::Error>> {
        if karet_seam::lang::rust::language_id().is_none() {
            return Ok(());
        }
        let dir = tempfile::tempdir()?;
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"q\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::create_dir_all(dir.path().join("src"))?;
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "pub unsafe fn danger() {}\npub fn quiet() {}\n",
        )?;

        let json = run(dir.path(), "lens:hazard", None)?;
        assert!(json.contains("\"q::danger\""), "{json}");
        assert!(!json.contains("\"q::quiet\""), "{json}");
        // Enough to cite it and to navigate back to it.
        assert!(json.contains("\"file\""), "{json}");
        assert!(json.contains("\"line\""), "{json}");
        assert!(json.contains("\"unsafe\""), "{json}");
        // And to judge density without materializing the subtree.
        assert!(json.contains("\"rollups\""), "{json}");
        assert!(json.contains("\"matched\": 1"), "{json}");
        // Nothing renders unattributed, here either.
        assert!(json.contains("\"configuration\""), "{json}");
        Ok(())
    }

    #[test]
    fn a_query_matching_nothing_is_a_success_with_no_nodes()
    -> Result<(), Box<dyn std::error::Error>> {
        if karet_seam::lang::rust::language_id().is_none() {
            return Ok(());
        }
        let dir = tempfile::tempdir()?;
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"q\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::create_dir_all(dir.path().join("src"))?;
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn quiet() {}\n")?;

        // Distinct from a failure: the question was understood, and the answer is none.
        let json = run(dir.path(), "lens:hazard", None)?;
        assert!(json.contains("\"matched\": 0"), "{json}");
        Ok(())
    }
}
