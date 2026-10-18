//! The `--seam-report` and `--seam-call-tree` surfaces: the call tier as documents.
//!
//! Both answer without a session or a terminal, like `--seam-query`, and share its
//! indexing and configuration so the three agree on what the tree is. The report is the
//! one meant to be handed whole to a reader — a person or a language model — deciding
//! where structure and encapsulation could improve.

use std::path::Path;

use karet_seam::Bound;
use karet_seam::CallGraph;
use karet_seam::GraphOptions;
use karet_seam::SeamPath;
use karet_seam::calls::ReportOptions;
use karet_seam::calls::TreeLimits;

use crate::cli::SeamFormat;
use crate::seam_query::QueryFailure;
use crate::seam_query::parse_query;
use crate::seam_query::prepare;

/// The call-structure report for everything under `root`, narrowed to `query`.
///
/// An empty query lists every function. Whatever it selects, every function is still
/// measured, so a narrowed report's numbers are the same as the whole one's.
///
/// # Errors
/// The root holds nothing indexable, or the query does not parse.
pub(crate) fn run_report(
    root: &Path,
    query: &str,
    configuration: Option<&str>,
    include_tests: bool,
    format: SeamFormat,
) -> Result<String, QueryFailure> {
    let (index, active) = prepare(root, "--seam-report", configuration, include_tests)?;
    let scope = if query.trim().is_empty() {
        Vec::new()
    } else {
        let parsed = parse_query("--seam-report", query)?;
        let nodes = karet_seam::query::evaluate(&parsed, &index).nodes;
        if nodes.is_empty() {
            // An empty scope means "everything", so a query matching nothing must not
            // silently become one.
            return Err(QueryFailure {
                message: format!("--seam-report: `{query}` matches nothing"),
                code: 2,
            });
        }
        nodes
    };
    let report = karet_seam::calls::report(
        &index,
        &GraphOptions {
            configuration: active,
            ..GraphOptions::default()
        },
        &scope,
        &ReportOptions::default(),
    );
    // A query can match nodes and still select no function — `kind:type`, or a subtree
    // the configuration excludes — and an empty report would read as "nothing to say".
    if report.functions.is_empty() {
        return Err(QueryFailure {
            message: if query.trim().is_empty() {
                "--seam-report: no function here is included by this configuration".to_owned()
            } else {
                format!(
                    "--seam-report: `{query}` selects no function this configuration includes \
                     (to report on what a node contains, use `in:<path>`)"
                )
            },
            code: 2,
        });
    }
    match format {
        SeamFormat::Markdown => Ok(report.markdown()),
        SeamFormat::Json => to_json("--seam-report", &report),
    }
}

/// The call tree of the function at `function`, a seam path.
///
/// # Errors
/// The root holds nothing indexable, the path does not parse, or it names no function
/// the active configuration includes.
pub(crate) fn run_call_tree(
    root: &Path,
    function: &str,
    configuration: Option<&str>,
    include_tests: bool,
    format: SeamFormat,
) -> Result<String, QueryFailure> {
    let (index, active) = prepare(root, "--seam-call-tree", configuration, include_tests)?;
    let fail = |message: String| QueryFailure {
        message: format!("--seam-call-tree: {message}"),
        code: 2,
    };
    let path: SeamPath = function
        .parse()
        .map_err(|error| fail(format!("`{function}` is not a seam path ({error})")))?;
    let id = index
        .resolve(&path)
        .filter(|id| index.node(*id).is_some())
        .ok_or_else(|| fail(format!("no node at `{function}`")))?;
    let graph = CallGraph::build(
        &index,
        &GraphOptions {
            configuration: active,
            ..GraphOptions::default()
        },
    );
    let tree = karet_seam::calls::call_tree(&index, &graph, id, Bound::Lower, TreeLimits::FULL)
        .ok_or_else(|| {
            fail(format!(
                "`{function}` is not a function this configuration includes"
            ))
        })?;
    match format {
        SeamFormat::Markdown => Ok(tree.markdown()),
        SeamFormat::Json => to_json("--seam-call-tree", &tree),
    }
}

/// Pretty JSON, or a failure naming the surface.
fn to_json<T: serde::Serialize>(flag: &str, value: &T) -> Result<String, QueryFailure> {
    serde_json::to_string_pretty(value).map_err(|error| QueryFailure {
        message: format!("{flag}: could not encode JSON ({error})"),
        code: 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn package() -> Result<Option<tempfile::TempDir>, Box<dyn std::error::Error>> {
        if karet_seam::lang::rust::language_id().is_none() {
            return Ok(None);
        }
        let dir = tempfile::tempdir()?;
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"q\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::create_dir_all(dir.path().join("src"))?;
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "pub fn top() { mid(); }\nfn mid() { leaf(); }\nfn leaf() {}\n\
             fn tested() {}\n#[cfg(test)]\nmod tests {\n    fn t() { super::tested(); }\n}\n",
        )?;
        Ok(Some(dir))
    }

    #[test]
    fn a_report_is_markdown_by_default_and_json_on_request() -> TestResult {
        let Some(dir) = package()? else {
            return Ok(());
        };
        let markdown = run_report(dir.path(), "", None, false, SeamFormat::Markdown)?;
        assert!(
            markdown.starts_with("# Call structure report"),
            "{markdown}"
        );
        assert!(markdown.contains("| `q::top` |"), "{markdown}");

        let json = run_report(dir.path(), "", None, false, SeamFormat::Json)?;
        let value: serde_json::Value = serde_json::from_str(&json)?;
        for key in [
            "configuration",
            "thresholds",
            "summary",
            "hotspots",
            "coupling",
            "ledger",
            "functions",
        ] {
            assert!(value.get(key).is_some(), "missing {key} in {json}");
        }
        Ok(())
    }

    #[test]
    fn including_tests_changes_who_calls_what() -> TestResult {
        let Some(dir) = package()? else {
            return Ok(());
        };
        let without = run_report(dir.path(), "", None, false, SeamFormat::Json)?;
        let with = run_report(dir.path(), "", None, true, SeamFormat::Json)?;
        let functions = |json: &str| -> Result<usize, Box<dyn std::error::Error>> {
            let value: serde_json::Value = serde_json::from_str(json)?;
            Ok(value["functions"].as_array().map_or(0, Vec::len))
        };
        // The test function itself exists only when tests are built.
        assert_eq!(functions(&with)?, functions(&without)? + 1);
        Ok(())
    }

    #[test]
    fn a_report_query_narrows_and_one_matching_nothing_fails() -> TestResult {
        let Some(dir) = package()? else {
            return Ok(());
        };
        let narrowed = run_report(dir.path(), "top", None, false, SeamFormat::Markdown)?;
        assert!(narrowed.contains("| `q::top` |"), "{narrowed}");
        assert!(!narrowed.contains("| `q::leaf` |"), "{narrowed}");

        let Err(failure) = run_report(
            dir.path(),
            "kind:interface",
            None,
            false,
            SeamFormat::Markdown,
        ) else {
            return Err("a query matching nothing should fail".into());
        };
        assert_eq!(failure.code, 2);
        // Matching nodes that are not functions is no better.
        let Err(failure) = run_report(
            dir.path(),
            "kind:package",
            None,
            false,
            SeamFormat::Markdown,
        ) else {
            return Err("a query selecting no function should fail".into());
        };
        assert!(
            failure.message.contains("selects no function"),
            "{}",
            failure.message
        );
        // Nor is a subtree the configuration leaves out.
        assert!(run_report(dir.path(), "in:q::tests", None, false, SeamFormat::Markdown).is_err());
        assert!(run_report(dir.path(), "in:q::tests", None, true, SeamFormat::Markdown).is_ok());
        let Err(failure) = run_report(dir.path(), "lens:nope", None, false, SeamFormat::Markdown)
        else {
            return Err("a bad query should fail".into());
        };
        assert!(failure.message.contains("at byte"), "{}", failure.message);
        Ok(())
    }

    #[test]
    fn a_call_tree_prints_a_function_and_refuses_anything_else() -> TestResult {
        let Some(dir) = package()? else {
            return Ok(());
        };
        let tree = run_call_tree(dir.path(), "q::top", None, false, SeamFormat::Markdown)?;
        assert!(
            tree.contains("q::mid") && tree.contains("q::leaf"),
            "{tree}"
        );
        let json = run_call_tree(dir.path(), "q::top", None, false, SeamFormat::Json)?;
        let value: serde_json::Value = serde_json::from_str(&json)?;
        assert_eq!(value["root"]["path"], "q::top");

        for (path, why) in [
            ("q::nowhere", "no node"),
            ("q", "not a function"),
            // Present in the tree, but left out by the default configuration.
            ("q::tests::t", "not a function"),
        ] {
            let Err(failure) = run_call_tree(dir.path(), path, None, false, SeamFormat::Markdown)
            else {
                return Err(format!("{path} should be refused").into());
            };
            assert_eq!(failure.code, 2);
            assert!(failure.message.contains(why), "{path}: {}", failure.message);
        }
        // Built with tests, the test function is a function like any other.
        assert!(run_call_tree(dir.path(), "q::tests::t", None, true, SeamFormat::Markdown).is_ok());
        Ok(())
    }
}
