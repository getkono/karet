//! Document-selector parsing and the LSP glob grammar it matches with.

use serde_json::json;

use super::*;

fn selector(value: Value) -> Option<Selector> {
    Selector::from_register_options(Some(&json!({ "documentSelector": value })))
}

#[test]
fn star_stays_inside_one_segment() {
    assert!(glob_matches("*.ts", "a.ts"));
    assert!(glob_matches("*.ts", ".ts"), "`*` may match nothing");
    assert!(!glob_matches("*.ts", "src/a.ts"));
    assert!(!glob_matches("*.ts", "a.tsx"));
}

#[test]
fn double_star_spans_any_number_of_segments() {
    assert!(glob_matches("**/*.ts", "/w/src/deep/a.ts"));
    assert!(glob_matches("**/*.ts", "a.ts"), "zero segments");
    assert!(
        glob_matches("src/**/x.rs", "src/x.rs"),
        "zero in the middle"
    );
    assert!(glob_matches("src/**/x.rs", "src/a/b/x.rs"));
    assert!(!glob_matches("src/**/x.rs", "lib/a/x.rs"));
    assert!(glob_matches("**", "/any/thing/at/all"));
}

#[test]
fn question_mark_is_one_character_but_never_a_separator() {
    assert!(glob_matches("a?c", "abc"));
    assert!(!glob_matches("a?c", "ac"));
    assert!(!glob_matches("a?c", "a/c"));
}

#[test]
fn braces_are_alternatives_and_may_nest() {
    assert!(glob_matches("**/*.{ts,js}", "/w/a.js"));
    assert!(glob_matches("**/*.{ts,js}", "/w/a.ts"));
    assert!(!glob_matches("**/*.{ts,js}", "/w/a.rs"));
    assert!(glob_matches("*.{c,h{pp,xx}}", "a.hxx"));
    assert!(glob_matches("*.{c,h{pp,xx}}", "a.c"));
    assert!(!glob_matches("*.{c,h{pp,xx}}", "a.h"));
}

#[test]
fn unbalanced_braces_match_nothing() {
    assert!(!glob_matches("*.{ts", "a.{ts"));
    assert!(!glob_matches("*.ts}", "a.ts}"));
}

#[test]
fn an_unusable_pattern_fails_open_like_an_unrecognised_selector() -> Result<(), String> {
    // Both directions of a broken registration enable the feature: an
    // unrecognised selector covers everything, and so does a filter whose
    // glob karet cannot use -- limited only by the filter's other fields.
    assert!(
        selector(json!("not-an-array")).is_none(),
        "covers every document"
    );

    let unbalanced = selector(json!([{"pattern": "**/*.{ts"}])).ok_or("no selector")?;
    assert!(unbalanced.matches(Path::new("/w/a.rs"), None));

    let groups = "{a,b}".repeat(20);
    let exploding =
        selector(json!([{"language": "rust", "pattern": groups}])).ok_or("no selector")?;
    assert!(exploding.matches(Path::new("/w/a.rs"), Some("rust")));
    assert!(
        !exploding.matches(Path::new("/w/a.rs"), Some("toml")),
        "the filter's other fields still apply"
    );
    Ok(())
}

#[test]
fn character_classes_take_ranges_and_negation() {
    assert!(glob_matches("[a-c]x", "bx"));
    assert!(!glob_matches("[a-c]x", "dx"));
    assert!(glob_matches("[!a-c]x", "dx"));
    assert!(!glob_matches("[!a-c]x", "ax"));
    assert!(glob_matches("[]]", "]"), "a leading `]` is a member");
    assert!(glob_matches("a[b", "a[b"), "an unclosed `[` is literal");
}

#[test]
fn a_pathological_glob_is_answered_promptly() {
    // Backtracking without a memo is exponential in the wildcards: eight stars
    // against a forty-character segment of `a`s that can never end in `b`
    // would try some forty-to-the-eighth splits before giving up.
    let text = "a".repeat(40);
    assert!(!glob_matches("*a*a*a*a*a*a*a*a*b", &text));
    assert!(!glob_matches("**a**a**a**a**a**a**b", &text));
}

#[test]
fn a_brace_bomb_matches_nothing_rather_than_expanding() {
    // Twenty groups would be a million alternatives.
    let bomb = "{a,b}".repeat(20);
    assert!(!glob_matches(&bomb, &"a".repeat(20)));
    // A realistic handful still works.
    assert!(glob_matches(&"{a,b}".repeat(4), "abba"));
}

#[test]
fn an_absent_or_null_selector_covers_every_document() {
    assert_eq!(Selector::from_register_options(None), None);
    assert_eq!(Selector::from_register_options(Some(&json!({}))), None);
    assert_eq!(selector(Value::Null), None);
    assert_eq!(
        selector(json!("not a selector")),
        None,
        "unrecognised shape"
    );
}

#[test]
fn a_language_filter_matches_the_language_the_document_was_opened_as() -> Result<(), String> {
    let only_ts = selector(json!([{"language": "typescript"}])).ok_or("no selector")?;
    let path = Path::new("/w/a.ts");
    assert!(only_ts.matches(path, Some("typescript")));
    assert!(!only_ts.matches(path, Some("javascript")));
    assert!(
        !only_ts.matches(path, None),
        "an unopened document has no language"
    );
    Ok(())
}

#[test]
fn every_field_of_a_filter_must_match() -> Result<(), String> {
    let filter = selector(json!([
        {"language": "rust", "scheme": "file", "pattern": "**/src/**/*.rs"}
    ]))
    .ok_or("no selector")?;
    assert!(filter.matches(Path::new("/w/src/lib.rs"), Some("rust")));
    assert!(!filter.matches(Path::new("/w/build.rs"), Some("rust")));
    assert!(!filter.matches(Path::new("/w/src/lib.rs"), Some("toml")));

    let untitled = selector(json!([{"scheme": "untitled"}])).ok_or("no selector")?;
    assert!(
        !untitled.matches(Path::new("/w/a.rs"), Some("rust")),
        "every karet document is a `file` URI"
    );
    Ok(())
}

#[test]
fn any_filter_of_several_may_match() -> Result<(), String> {
    let either = selector(json!([{"language": "javascript"}, {"pattern": "**/*.ts"}]))
        .ok_or("no selector")?;
    assert!(either.matches(Path::new("/w/a.js"), Some("javascript")));
    assert!(either.matches(Path::new("/w/a.ts"), None));
    assert!(!either.matches(Path::new("/w/a.rs"), Some("rust")));
    Ok(())
}

#[test]
fn a_relative_pattern_is_matched_below_its_base() -> Result<(), String> {
    let relative = selector(json!([
        {"pattern": {"baseUri": "file:///w/app", "pattern": "**/*.py"}}
    ]))
    .ok_or("no selector")?;
    assert!(relative.matches(Path::new("/w/app/pkg/a.py"), None));
    assert!(!relative.matches(Path::new("/w/other/a.py"), None));

    let folder = selector(json!([
        {"pattern": {"baseUri": {"uri": "file:///w/app", "name": "app"}, "pattern": "*.py"}}
    ]))
    .ok_or("no selector")?;
    assert!(folder.matches(Path::new("/w/app/a.py"), None));
    assert!(!folder.matches(Path::new("/w/app/pkg/a.py"), None));
    Ok(())
}

#[test]
fn a_bare_string_is_a_language_and_a_notebook_filter_never_matches() -> Result<(), String> {
    let bare = selector(json!(["python"])).ok_or("no selector")?;
    assert!(bare.matches(Path::new("/w/a.py"), Some("python")));

    let cells = selector(json!([{"notebook": "jupyter-notebook", "language": "python"}]))
        .ok_or("no selector")?;
    assert!(!cells.matches(Path::new("/w/a.py"), Some("python")));
    Ok(())
}
