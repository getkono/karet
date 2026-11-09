//! Public-API integration tests: nbformat round-trips across minor versions,
//! MIME priority, and the markdown rendering.

use karet_notebook::CellKind;
use karet_notebook::NotebookError;
use karet_notebook::parse;
use karet_notebook::to_json;
use karet_notebook::to_markdown;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// A 4.5 notebook: cell ids, joined and listed sources, every output type,
/// and unmodeled fields at every level.
const V45: &str = r##"{
  "nbformat": 4,
  "nbformat_minor": 5,
  "metadata": {
    "kernelspec": {"display_name": "Python 3", "language": "python", "name": "python3"},
    "language_info": {"name": "python", "version": "3.12.1"},
    "custom_tool": {"keep": true}
  },
  "cells": [
    {
      "id": "intro",
      "cell_type": "markdown",
      "metadata": {"editable": false},
      "source": ["# Title\n", "\n", "Prose *here*.\n"]
    },
    {
      "id": "compute",
      "cell_type": "code",
      "metadata": {},
      "execution_count": 3,
      "source": "x = 6 * 7\nprint(x)\nx",
      "outputs": [
        {"output_type": "stream", "name": "stdout", "text": ["42\n"]},
        {
          "output_type": "execute_result",
          "execution_count": 3,
          "data": {"text/plain": ["42"]},
          "metadata": {}
        },
        {
          "output_type": "display_data",
          "data": {"image/png": "iVBORw0KGgo=", "text/plain": ["<Figure>"]},
          "metadata": {"needs_background": "light"}
        },
        {
          "output_type": "error",
          "ename": "ValueError",
          "evalue": "boom",
          "traceback": ["\u001b[31mValueError\u001b[0m: boom"]
        }
      ]
    },
    {
      "id": "fresh",
      "cell_type": "code",
      "metadata": {},
      "execution_count": null,
      "source": [],
      "outputs": []
    }
  ]
}"##;

/// A 4.2-era notebook: no cell ids, joined sources.
const V42: &str = r#"{
  "nbformat": 4,
  "nbformat_minor": 2,
  "metadata": {},
  "cells": [
    {"cell_type": "markdown", "metadata": {}, "source": "plain **prose**"},
    {"cell_type": "raw", "metadata": {}, "source": "raw payload"},
    {"cell_type": "code", "metadata": {"collapsed": true}, "execution_count": 1,
     "source": "1 + 1", "outputs": []}
  ]
}"#;

/// Edge shapes: an empty joined source, a list source with no trailing
/// newline, non-ASCII and escaped text, attachments on a markdown cell, a code
/// cell with no `execution_count`/`outputs` keys, `transient`-style unmodeled
/// output fields, and nested nulls/floats in metadata.
const EDGES: &str = r##"{
  "nbformat": 4,
  "nbformat_minor": 4,
  "metadata": {"nested": {"null": null, "float": 0.5, "list": [1, "two", false]}},
  "cells": [
    {"cell_type": "markdown", "metadata": {}, "source": "",
     "attachments": {"a.png": {"image/png": "iVBORw0KGgo="}}},
    {"cell_type": "markdown", "metadata": {}, "source": ["no newline at end"]},
    {"cell_type": "raw", "metadata": {"format": "text/latex"},
     "source": ["café \"quoted\" \\ tab\t", "日本語 ✓"]},
    {"cell_type": "code", "metadata": {}, "source": "pass"},
    {"cell_type": "code", "metadata": {}, "execution_count": 9, "source": [],
     "outputs": [
       {"output_type": "stream", "name": "stderr", "text": "", "custom": 1},
       {"output_type": "display_data", "data": {}, "metadata": {},
        "transient": {"display_id": "d1"}},
       {"output_type": "execute_result", "execution_count": null,
        "data": {"text/plain": "x"}, "metadata": {}}
     ]}
  ]
}"##;

#[test]
fn round_trips_are_value_identical() -> TestResult {
    for fixture in [V45, V42, EDGES] {
        let notebook = parse(fixture)?;
        let rewritten = to_json(&notebook)?;
        let original: serde_json::Value = serde_json::from_str(fixture)?;
        let round_tripped: serde_json::Value = serde_json::from_str(&rewritten)?;
        assert_eq!(original, round_tripped);
    }
    Ok(())
}

#[test]
fn the_model_reads_both_minor_versions() -> TestResult {
    let new = parse(V45)?;
    assert_eq!(new.nbformat_minor, 5);
    assert_eq!(new.cells.len(), 3);
    assert_eq!(new.cells[0].id.as_deref(), Some("intro"));
    assert_eq!(new.cells[1].kind, CellKind::Code);
    assert_eq!(new.cells[1].execution_count, Some(Some(3)));
    assert_eq!(new.cells[2].execution_count, Some(None));
    assert_eq!(new.language(), "python");

    let old = parse(V42)?;
    assert_eq!(old.cells[0].id, None);
    assert_eq!(old.cells[2].outputs.as_deref().map(<[_]>::len), Some(0));
    Ok(())
}

#[test]
fn unsupported_majors_and_garbage_are_typed_errors() {
    let v3 = r#"{"nbformat": 3, "nbformat_minor": 0, "worksheets": []}"#;
    assert!(matches!(
        parse(v3),
        Err(NotebookError::UnsupportedVersion(3))
    ));
    assert!(matches!(parse("not json"), Err(NotebookError::Parse(_))));
}

#[test]
fn markdown_renders_cells_and_prioritizes_mime() -> TestResult {
    let markdown = to_markdown(&parse(V45)?);
    assert!(markdown.contains("# Title"), "{markdown}");
    assert!(markdown.contains("_In [3]:_"), "{markdown}");
    assert!(markdown.contains("```python\nx = 6 * 7"), "{markdown}");
    assert!(markdown.contains("```text\n42\n```"), "{markdown}");
    // The display_data bundle offers image/png and text/plain: the image wins
    // as a placeholder (the preview is prose).
    assert!(markdown.contains("*\\[image output\\]*"), "{markdown}");
    // The traceback is ANSI-stripped.
    assert!(!markdown.contains("ValueError\u{1b}[0m"), "{markdown}");
    assert!(markdown.contains("**ValueError**: boom"), "{markdown}");
    // The never-executed cell renders an empty counter.
    assert!(markdown.contains("_In [ ]:_"), "{markdown}");
    Ok(())
}

#[test]
fn markdown_falls_back_to_plain_and_markdown_mimes() -> TestResult {
    let fixture = r##"{
      "nbformat": 4, "nbformat_minor": 4, "metadata": {},
      "cells": [{
        "cell_type": "code", "metadata": {}, "execution_count": 1, "source": "df",
        "outputs": [{
          "output_type": "execute_result", "execution_count": 1,
          "data": {"text/markdown": ["| a |\n", "|---|\n"], "text/plain": ["table"]},
          "metadata": {}
        }]
      }]
    }"##;
    let markdown = to_markdown(&parse(fixture)?);
    assert!(markdown.contains("| a |"), "markdown mime wins: {markdown}");
    assert!(!markdown.contains("```text\ntable"), "{markdown}");
    Ok(())
}

#[test]
fn edge_fixture_keeps_absent_keys_absent_and_encodings_as_read() -> TestResult {
    let notebook = parse(EDGES)?;
    let pass = &notebook.cells[3];
    assert_eq!(pass.execution_count, None);
    assert_eq!(pass.outputs, None);
    assert_eq!(
        notebook.cells[0].source,
        karet_notebook::Source::Joined(String::new())
    );
    assert!(notebook.cells[0].extra.contains_key("attachments"));
    assert_eq!(
        notebook.cells[1].source,
        karet_notebook::Source::Lines(vec!["no newline at end".to_owned()])
    );
    assert_eq!(
        notebook.cells[2].source.text(),
        "café \"quoted\" \\ tab\t日本語 ✓"
    );
    Ok(())
}

#[test]
fn edge_fixture_renders_exactly() -> TestResult {
    assert_eq!(
        to_markdown(&parse(EDGES)?),
        concat!(
            // The empty first markdown cell contributes nothing, not a blank
            // line.
            "no newline at end\n\n",
            "```\ncafé \"quoted\" \\ tab\t日本語 ✓\n```\n\n",
            "_In [ ]:_\n\n```python\npass\n```\n\n",
            "_In [9]:_\n\n```python\n\n```\n\n",
            "```stderr\n\n```\n\n",
            "*\\[unrenderable output\\]*\n\n",
            "```text\nx\n```\n",
        )
    );
    Ok(())
}

#[test]
fn absent_top_level_collections_are_written_back_as_empty() -> TestResult {
    // `metadata` and `cells` default when absent, so the rewrite is not
    // value-identical: it gains the two empty collections.
    let notebook = parse(r#"{"nbformat": 4, "nbformat_minor": 0}"#)?;
    let rewritten: serde_json::Value = serde_json::from_str(&to_json(&notebook)?)?;
    assert_eq!(
        rewritten,
        serde_json::json!({"nbformat": 4, "nbformat_minor": 0, "metadata": {}, "cells": []})
    );
    Ok(())
}

#[test]
fn malformed_shapes_are_parse_errors() {
    for (why, input) in [
        ("missing nbformat", r#"{"nbformat_minor": 5, "cells": []}"#),
        (
            "unknown cell type",
            r#"{"nbformat": 4, "nbformat_minor": 5,
                "cells": [{"cell_type": "sql", "source": ""}]}"#,
        ),
        (
            "unknown output type",
            r#"{"nbformat": 4, "nbformat_minor": 5, "cells": [{"cell_type": "code",
                "source": "", "outputs": [{"output_type": "widget"}]}]}"#,
        ),
        (
            "stream without a name",
            r#"{"nbformat": 4, "nbformat_minor": 5, "cells": [{"cell_type": "code",
                "source": "", "outputs": [{"output_type": "stream", "text": ""}]}]}"#,
        ),
        (
            "numeric source",
            r#"{"nbformat": 4, "nbformat_minor": 5,
                "cells": [{"cell_type": "markdown", "source": 7}]}"#,
        ),
        ("truncated document", r#"{"nbformat": 4, "cells": ["#),
    ] {
        assert!(
            matches!(parse(input), Err(NotebookError::Parse(_))),
            "{why}: {:?}",
            parse(input)
        );
    }
    assert!(matches!(
        parse(r#"{"nbformat": 5, "nbformat_minor": 0}"#),
        Err(NotebookError::UnsupportedVersion(5))
    ));
}
