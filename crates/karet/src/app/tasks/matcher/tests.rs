use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A scratch workspace holding `files` (empty), and its canonical root.
fn workspace(files: &[&str]) -> std::io::Result<(tempfile::TempDir, PathBuf)> {
    let dir = tempfile::tempdir()?;
    let root = std::fs::canonicalize(dir.path())?;
    for file in files {
        let path = root.join(file);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, "")?;
    }
    Ok((dir, root))
}

fn lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_owned).collect()
}

fn run(specs: &[ProblemMatcher], text: &str, cwd: &Path, root: &Path) -> Vec<ProblemEntry> {
    let matchers = compile(specs).unwrap_or_default();
    let context = ScanContext {
        cwd,
        root,
        source: "build",
    };
    scan(&matchers, &lines(text), &context)
}

fn builtin(builtin: BuiltinMatcher) -> Vec<ProblemMatcher> {
    vec![ProblemMatcher::Builtin(builtin)]
}

/// `(relative path, 0-based line, 0-based column, severity, code, message)`.
type Seen = (String, u32, u32, Severity, Option<String>, String);

fn seen(entries: &[ProblemEntry], root: &Path) -> Vec<Seen> {
    entries
        .iter()
        .map(|entry| {
            let path = entry.path.strip_prefix(root).unwrap_or(&entry.path);
            let d = &entry.diagnostic;
            (
                path.to_string_lossy().into_owned(),
                d.range.start.line,
                d.range.start.col,
                d.severity,
                d.code.clone(),
                d.message.clone(),
            )
        })
        .collect()
}

fn expect(
    path: &str,
    line: u32,
    col: u32,
    severity: Severity,
    code: Option<&str>,
    message: &str,
) -> Seen {
    (
        path.to_owned(),
        line,
        col,
        severity,
        code.map(str::to_owned),
        message.to_owned(),
    )
}

#[test]
fn every_builtin_compiles() {
    for (builtin, patterns) in [
        (BuiltinMatcher::Rustc, 1),
        (BuiltinMatcher::Gcc, 1),
        (BuiltinMatcher::Tsc, 2),
        (BuiltinMatcher::Go, 1),
        (BuiltinMatcher::EslintCompact, 1),
    ] {
        assert_eq!(builtin_matchers(builtin).len(), patterns, "{builtin:?}");
    }
}

#[test]
fn rustc_pairs_each_heading_with_the_location_under_it() -> TestResult {
    let (_dir, root) = workspace(&["src/main.rs"])?;
    let output = "\
   Compiling demo v0.1.0 (/somewhere)
warning: unused variable: `x`
 --> src/main.rs:2:9
  |
2 |     let x = 1;
  |         ^ help: if this is intentional, prefix it with an underscore: `_x`
  |
  = note: `#[warn(unused_variables)]` on by default

error[E0308]: mismatched types
  --> src/main.rs:3:18
   |
3  |     let y: u8 = \"no\";
   |            --   ^^^^ expected `u8`, found `&str`
   |
  ::: src/lib.rs:9:1
error: aborting due to 1 previous error
For more information about this error, try `rustc --explain E0308`.
error: could not compile `demo` (bin \"demo\") due to 1 previous error";
    let found = run(&builtin(BuiltinMatcher::Rustc), output, &root, &root);
    assert_eq!(
        seen(&found, &root),
        vec![
            expect(
                "src/main.rs",
                1,
                8,
                Severity::Warning,
                None,
                "unused variable: `x`"
            ),
            expect(
                "src/main.rs",
                2,
                17,
                Severity::Error,
                Some("E0308"),
                "mismatched types"
            ),
        ],
        "a heading with no location of its own (aborting, could not compile) is no problem, \
         and a `:::` secondary location belongs to nobody"
    );
    assert_eq!(found[0].diagnostic.source.as_deref(), Some("build"));
    Ok(())
}

#[test]
fn a_rustc_location_too_far_below_its_heading_is_not_paired() -> TestResult {
    let (_dir, root) = workspace(&["src/main.rs"])?;
    let output = "error: something\none\ntwo\nthree\n --> src/main.rs:1:1";
    assert!(run(&builtin(BuiltinMatcher::Rustc), output, &root, &root).is_empty());
    Ok(())
}

#[test]
fn the_same_problem_printed_twice_is_listed_once() -> TestResult {
    let (_dir, root) = workspace(&["src/lib.rs"])?;
    let block = "warning: unused import\n --> src/lib.rs:1:5\n";
    let found = run(
        &builtin(BuiltinMatcher::Rustc),
        &format!("{block}{block}"),
        &root,
        &root,
    );
    assert_eq!(found.len(), 1, "{found:?}");
    Ok(())
}

#[test]
fn gcc_reads_file_line_column_and_severity() -> TestResult {
    let (_dir, root) = workspace(&["main.c"])?;
    let output = "\
main.c:4:12: error: expected ';' before '}' token
main.c:2:5: warning: unused variable 'n' [-Wunused-variable]
main.c:1:1: fatal error: missing.h: No such file or directory
main.c: In function 'main':";
    assert_eq!(
        seen(
            &run(&builtin(BuiltinMatcher::Gcc), output, &root, &root),
            &root
        ),
        vec![
            expect(
                "main.c",
                3,
                11,
                Severity::Error,
                None,
                "expected ';' before '}' token"
            ),
            expect(
                "main.c",
                1,
                4,
                Severity::Warning,
                None,
                "unused variable 'n' [-Wunused-variable]"
            ),
            expect(
                "main.c",
                0,
                0,
                Severity::Error,
                None,
                "missing.h: No such file or directory"
            ),
        ]
    );
    Ok(())
}

#[test]
fn tsc_reads_both_its_plain_and_its_pretty_forms() -> TestResult {
    let (_dir, root) = workspace(&["src/a.ts"])?;
    let output = "\
src/a.ts(3,7): error TS2322: Type 'string' is not assignable to type 'number'.
src/a.ts:9:1 - warning TS6133: 'z' is declared but its value is never read.";
    assert_eq!(
        seen(
            &run(&builtin(BuiltinMatcher::Tsc), output, &root, &root),
            &root
        ),
        vec![
            expect(
                "src/a.ts",
                2,
                6,
                Severity::Error,
                Some("TS2322"),
                "Type 'string' is not assignable to type 'number'."
            ),
            expect(
                "src/a.ts",
                8,
                0,
                Severity::Warning,
                Some("TS6133"),
                "'z' is declared but its value is never read."
            ),
        ]
    );
    Ok(())
}

#[test]
fn go_takes_the_column_when_there_is_one() -> TestResult {
    let (_dir, root) = workspace(&["cmd/main.go"])?;
    let output = "\
# example.com/cmd
cmd/main.go:7:2: undefined: fmt.Printl
cmd/main.go:12: missing return";
    assert_eq!(
        seen(
            &run(&builtin(BuiltinMatcher::Go), output, &root, &root),
            &root
        ),
        vec![
            expect(
                "cmd/main.go",
                6,
                1,
                Severity::Error,
                None,
                "undefined: fmt.Printl"
            ),
            expect(
                "cmd/main.go",
                11,
                0,
                Severity::Error,
                None,
                "missing return"
            ),
        ]
    );
    Ok(())
}

#[test]
fn eslint_compact_reads_the_rule_as_the_code() -> TestResult {
    let (_dir, root) = workspace(&["src/app.js"])?;
    let output = "\
src/app.js: line 3, col 7, Error - 'x' is assigned a value but never used. (no-unused-vars)
src/app.js: line 5, col 1, Warning - Unexpected console statement.

2 problems";
    assert_eq!(
        seen(
            &run(
                &builtin(BuiltinMatcher::EslintCompact),
                output,
                &root,
                &root
            ),
            &root
        ),
        vec![
            expect(
                "src/app.js",
                2,
                6,
                Severity::Error,
                Some("no-unused-vars"),
                "'x' is assigned a value but never used."
            ),
            expect(
                "src/app.js",
                4,
                0,
                Severity::Warning,
                None,
                "Unexpected console statement."
            ),
        ]
    );
    Ok(())
}

fn pattern(regexp: &str) -> PatternMatcher {
    PatternMatcher {
        regexp: regexp.to_owned(),
        file: 1,
        line: 2,
        column: None,
        severity: None,
        code: None,
        message: None,
        default_severity: MatcherSeverity::Warning,
    }
}

#[test]
fn a_custom_pattern_reads_the_groups_it_names_and_defaults_the_rest() -> TestResult {
    let (_dir, root) = workspace(&["lib/x.py"])?;
    let bare = pattern(r"^(\S+):(\d+): (.*)$");
    let full = PatternMatcher {
        column: Some(3),
        severity: Some(4),
        code: Some(5),
        message: Some(6),
        ..pattern(r"^(\S+) (\d+) (\d+) (\w+) (\w+) (.*)$")
    };
    let output =
        "lib/x.py:4: line too long\nlib/x.py 9 3 note C0301 spelled out\nlib/x.py 1 1 shrug X1 odd";
    let found = run(
        &[ProblemMatcher::Pattern(bare), ProblemMatcher::Pattern(full)],
        output,
        &root,
        &root,
    );
    assert_eq!(
        seen(&found, &root),
        vec![
            expect(
                "lib/x.py",
                3,
                0,
                Severity::Warning,
                None,
                "lib/x.py:4: line too long"
            ),
            expect(
                "lib/x.py",
                8,
                2,
                Severity::Information,
                Some("C0301"),
                "spelled out"
            ),
            expect("lib/x.py", 0, 0, Severity::Warning, Some("X1"), "odd"),
        ],
        "no message group uses the whole line; an unknown severity word uses the default"
    );
    Ok(())
}

#[test]
fn a_pattern_that_does_not_compile_is_named_in_the_error() {
    let error = compile(&[ProblemMatcher::Pattern(pattern("(unclosed"))]).err();
    assert!(
        error.as_deref().is_some_and(|e| e.contains("`(unclosed`")),
        "{error:?}"
    );
}

#[test]
fn a_group_past_the_last_one_reads_as_nothing() -> TestResult {
    let (_dir, root) = workspace(&["a.txt"])?;
    let wide = PatternMatcher {
        column: Some(9),
        message: Some(9),
        ..pattern(r"^(\S+):(\d+)$")
    };
    let found = run(&[ProblemMatcher::Pattern(wide)], "a.txt:3", &root, &root);
    assert_eq!(
        seen(&found, &root),
        vec![expect("a.txt", 2, 0, Severity::Warning, None, "")]
    );
    Ok(())
}

#[test]
fn paths_resolve_under_the_cwd_then_the_root_and_fold_dot_dot() -> TestResult {
    let (_dir, root) = workspace(&["crates/app/src/main.rs", "crates/core/src/lib.rs"])?;
    let cwd = root.join("crates/app");
    let context = ScanContext {
        cwd: &cwd,
        root: &root,
        source: "build",
    };
    assert_eq!(resolve("src/main.rs", &context), cwd.join("src/main.rs"));
    assert_eq!(
        resolve("crates/core/src/lib.rs", &context),
        root.join("crates/core/src/lib.rs"),
        "cargo prints workspace-relative paths from a member"
    );
    assert_eq!(
        resolve("../core/./src/lib.rs", &context),
        root.join("crates/core/src/lib.rs")
    );
    assert_eq!(
        resolve("gone.rs", &context),
        cwd.join("gone.rs"),
        "a file nowhere resolves under the cwd"
    );
    let absolute = root.join("crates/app/../core/src/lib.rs");
    assert_eq!(
        resolve(&absolute.to_string_lossy(), &context),
        root.join("crates/core/src/lib.rs")
    );
    Ok(())
}

#[test]
fn normalizing_never_climbs_above_the_root() {
    assert_eq!(normalize(Path::new("/../a/../../b")), PathBuf::from("/b"));
    assert_eq!(normalize(Path::new("../a/./b/..")), PathBuf::from("../a"));
}

#[test]
fn severity_words_are_read_in_any_case() {
    assert_eq!(severity_word("ERROR"), Some(Severity::Error));
    assert_eq!(severity_word(" Warning "), Some(Severity::Warning));
    assert_eq!(severity_word("note"), Some(Severity::Information));
    assert_eq!(severity_word("help"), Some(Severity::Hint));
    assert_eq!(severity_word("shrug"), None);
}
