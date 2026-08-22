use super::*;

/// The default config minus MD041, so fixtures need not all start with a
/// top-level heading; the MD041 test uses the untouched default.
fn base_config() -> Config {
    Config::from_json(r#"{ "MD041": false }"#).unwrap_or_default()
}

fn ids(text: &str) -> Vec<&'static str> {
    lint(text, &base_config())
        .into_iter()
        .map(|i| i.rule)
        .collect()
}

fn fixed(text: &str) -> String {
    let issues = lint(text, &base_config());
    apply_fixes(text, &issues)
}

// --- per-line rules ---

#[test]
fn trailing_spaces_flag_and_trim_but_hard_breaks_survive() {
    assert_eq!(ids("text \n"), vec!["MD009"]);
    // Exactly two trailing spaces are a hard line break.
    assert!(ids("text  \nmore\n").is_empty());
    assert_eq!(fixed("text \n"), "text\n");
}

#[test]
fn hard_tabs_flag_and_expand() {
    assert_eq!(ids("a\tb\n"), vec!["MD010"]);
    assert_eq!(fixed("a\tb\n"), "a    b\n");
}

#[test]
fn multiple_blanks_collapse() {
    let text = "a\n\n\n\nb\n";
    assert_eq!(ids(text), vec!["MD012", "MD012"]);
    assert_eq!(fixed(text), "a\n\nb\n");
}

#[test]
fn long_lines_flag_with_configured_length() {
    let long = format!("{}\n", "x".repeat(81));
    assert_eq!(ids(&long), vec!["MD013"]);
    let mut config = base_config();
    config.line_length = 100;
    assert!(lint(&long, &config).is_empty());
    // Lines carrying URLs are exempt (they cannot wrap) — MD034 still fires.
    let url = format!("{} https://example.com/x\n", "x".repeat(70));
    assert!(!ids(&url).contains(&"MD013"));
}

#[test]
fn bare_urls_flag_and_wrap_in_angle_brackets() {
    assert_eq!(ids("see https://example.com now\n"), vec!["MD034"]);
    assert_eq!(
        fixed("see https://example.com now\n"),
        "see <https://example.com> now\n"
    );
    // Already-delimited URLs stay quiet.
    assert!(ids("see <https://example.com>\n").is_empty());
    assert!(ids("[x](https://example.com)\n").is_empty());
    assert!(ids("`https://example.com`\n").is_empty());
}

// --- heading rules ---

#[test]
fn heading_hash_spacing_fixes_both_ways() {
    assert_eq!(ids("#Title\n"), vec!["MD018"]);
    assert_eq!(fixed("#Title\n"), "# Title\n");
    assert!(ids("#  Title\n").contains(&"MD019"));
    assert_eq!(fixed("# Title\n\ncontent\n"), "# Title\n\ncontent\n");
}

#[test]
fn heading_increment_and_indent_and_punctuation() {
    let text = "# One\n\n### Three\n";
    assert_eq!(ids(text), vec!["MD001"]);
    assert!(ids("   # Indented\n").contains(&"MD023"));
    assert!(ids("# Title!\n").contains(&"MD026"));
    assert_eq!(fixed("# Title!\n"), "# Title\n");
}

#[test]
fn headings_need_surrounding_blanks() {
    let text = "# Title\ncontent\n";
    assert!(ids(text).contains(&"MD022"));
    assert_eq!(fixed(text), "# Title\n\ncontent\n");
}

#[test]
fn first_line_must_be_a_top_level_heading() {
    let with = |text: &str| {
        lint(text, &Config::default())
            .into_iter()
            .map(|i| i.rule)
            .collect::<Vec<_>>()
    };
    assert_eq!(with("plain intro\n"), vec!["MD041"]);
    assert!(with("## Second\n").contains(&"MD041"));
    assert!(with("# Proper\n").is_empty());
    // Front matter does not count as content.
    assert!(with("---\ntitle: x\n---\n\n# Proper\n").is_empty());
}

// --- structure rules ---

#[test]
fn fences_need_blanks_and_a_language() {
    let text = "para\n```\ncode\n```\npara\n";
    let found = ids(text);
    assert!(found.contains(&"MD031"));
    assert!(found.contains(&"MD040"));
    let fixed_text = fixed(text);
    assert!(fixed_text.contains("para\n\n```"));
    assert!(fixed_text.contains("```\n\npara"));
}

#[test]
fn lists_need_surrounding_blanks() {
    let text = "para\n- item\npara\n";
    let found = ids(text);
    assert_eq!(found.iter().filter(|r| **r == "MD032").count(), 2);
    assert_eq!(fixed(text), "para\n\n- item\n\npara\n");
}

// --- inline rules ---

#[test]
fn spaces_inside_spans_flag_and_fix() {
    assert_eq!(ids("a ` code ` b\n"), vec!["MD038"]);
    assert_eq!(fixed("a ` code ` b\n"), "a `code` b\n");
    assert_eq!(ids("a ** bold ** b\n"), vec!["MD037"]);
    assert_eq!(fixed("a ** bold ** b\n"), "a **bold** b\n");
    assert_eq!(ids("[ text ](url)\n"), vec!["MD039"]);
    assert_eq!(fixed("[ text ](url)\n"), "[text](url)\n");
}

#[test]
fn empty_links_and_missing_alt_text_flag() {
    assert_eq!(ids("[click]()\n"), vec!["MD042"]);
    assert_eq!(ids("![](img.png)\n"), vec!["MD045"]);
    assert!(ids("![alt](img.png)\n").is_empty());
}

// --- document rules ---

#[test]
fn a_single_trailing_newline_is_required() {
    assert_eq!(ids("# T\n\ntext"), vec!["MD047"]);
    assert_eq!(fixed("# T\n\ntext"), "# T\n\ntext\n");
    assert!(ids("# T\n\ntext\n").is_empty());
}

// --- fences gate content rules ---

#[test]
fn code_interiors_are_exempt_from_prose_rules() {
    let text = "# T\n\n```text\n#not a heading\nhttps://example.com\n- list\n```\n";
    let found = ids(text);
    assert!(!found.contains(&"MD018"));
    assert!(!found.contains(&"MD034"));
    assert!(!found.contains(&"MD032"));
}

// --- directives ---

#[test]
fn inline_directives_suppress_and_restore() {
    let all_off = "<!-- markdownlint-disable -->\ntext \nhttps://example.com\n";
    assert!(ids(all_off).is_empty());
    let one_off = "# T\n\n<!-- markdownlint-disable MD009 -->\ntext \n";
    assert!(!ids(one_off).contains(&"MD009"));
    let restored =
        "# T\n\n<!-- markdownlint-disable MD009 -->\n<!-- markdownlint-enable MD009 -->\ntext \n";
    assert!(ids(restored).contains(&"MD009"));
    let next_line = "# T\n\n<!-- markdownlint-disable-next-line no-trailing-spaces -->\ntext \n";
    assert!(!ids(next_line).contains(&"MD009"));
}

// --- config ---

#[test]
fn config_json_disables_downgrades_and_parameterizes() -> Result<(), LintConfigError> {
    let config = Config::from_json(
        r#"{
            "default": true,
            "MD009": false,
            "no-hard-tabs": "warning",
            "MD013": { "line_length": 120 },
            "MD999": false
        }"#,
    )?;
    assert_eq!(config.line_length, 120);
    let text = "# T\n\ntext \na\tb\n";
    let issues = lint(text, &config);
    assert!(!issues.iter().any(|i| i.rule == "MD009"));
    let tab = issues.iter().find(|i| i.rule == "MD010");
    assert_eq!(tab.map(|i| i.severity), Some(LintSeverity::Warning));
    Ok(())
}

#[test]
fn default_false_turns_everything_unnamed_off() -> Result<(), LintConfigError> {
    let config = Config::from_json(r#"{ "default": false, "MD047": true }"#)?;
    let issues = lint("text \nno newline", &config);
    assert_eq!(
        issues.iter().map(|i| i.rule).collect::<Vec<_>>(),
        vec!["MD047"]
    );
    Ok(())
}

#[test]
fn malformed_config_reports_an_error() {
    assert!(Config::from_json("not json").is_err());
    assert!(Config::from_json("[1, 2]").is_err());
}

#[test]
fn md034_leaves_link_reference_definitions_alone() {
    // `[label]: https://…` is a link destination, not a bare URL — upstream
    // markdownlint does not flag it, and "fixing" it rewrites a good definition.
    let text = "See [ratatui].\n\n[ratatui]: https://crates.io/crates/ratatui\n";
    let issues = lint(text, &Config::default());
    assert!(
        !issues.iter().any(|i| i.rule == "MD034"),
        "a definition must not be reported: {issues:?}"
    );
    assert_eq!(apply_fixes(text, &issues), text);
}

#[test]
fn md034_still_reports_a_genuinely_bare_url() {
    let text = "Visit https://example.com for more.\n";
    let issues = lint(text, &Config::default());
    assert!(issues.iter().any(|i| i.rule == "MD034"), "{issues:?}");
    assert_eq!(
        apply_fixes(text, &issues),
        "Visit <https://example.com> for more.\n"
    );
}

#[test]
fn md034_skips_a_titled_definition_but_not_a_lookalike_paragraph() {
    let definition = "[a]: https://example.com \"Title\"\n";
    assert!(
        !lint(definition, &Config::default())
            .iter()
            .any(|i| i.rule == "MD034")
    );
    // A bracketed phrase that is not a definition (no colon) still gets linted.
    let paragraph = "[not a label] https://example.com\n";
    assert!(
        lint(paragraph, &Config::default())
            .iter()
            .any(|i| i.rule == "MD034"),
        "only a real `]:` definition is exempt"
    );
}

#[test]
fn md037_survives_odd_runs_of_emphasis_markers() {
    // Overlapping `**` matches inside `***` used to pair position 0 with
    // position 1 and slice `2..1`, panicking on ordinary bold-italic text.
    for text in [
        "***\n",
        "***bold italic***\n",
        "a *** b\n",
        "___\n",
        "____\n",
        "*****\n",
        "**a** *** **b**\n",
    ] {
        let issues = lint(text, &Config::default());
        // The fixes must round-trip too, not just the scan.
        let _ = apply_fixes(text, &issues);
    }
}

#[test]
fn md037_still_reports_padded_emphasis() {
    let text = "this is ** padded ** emphasis\n";
    let issues = lint(text, &Config::default());
    assert!(issues.iter().any(|i| i.rule == "MD037"), "{issues:?}");
    assert_eq!(apply_fixes(text, &issues), "this is **padded** emphasis\n");
}
