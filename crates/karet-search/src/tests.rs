use super::*;

#[test]
fn query_defaults() {
    let q = SearchQuery::default();
    assert!(!q.regex);
    assert!(q.includes.is_empty());
}

#[test]
fn error_displays() {
    assert_eq!(
        SearchError::InvalidPattern.to_string(),
        "invalid search pattern"
    );
}

fn literal(pattern: &str) -> SearchQuery {
    SearchQuery {
        pattern: pattern.to_string(),
        case_sensitive: true,
        ..Default::default()
    }
}

#[test]
fn literal_finds_all_occurrences() {
    let m = search_in_file("foo bar foo", &literal("foo")).unwrap_or_default();
    assert_eq!(m.len(), 2);
    assert_eq!((m[0].start, m[0].end, m[0].col), (0, 3, 0));
    assert_eq!((m[1].start, m[1].end, m[1].col), (8, 11, 8));
}

#[test]
fn case_insensitive_matches() {
    let q = SearchQuery {
        pattern: "FOO".into(),
        ..Default::default()
    };
    let m = search_in_file("a foo b", &q).unwrap_or_default();
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].start, 2);
}

#[test]
fn regex_matches() {
    let q = SearchQuery {
        pattern: "f.o".into(),
        regex: true,
        case_sensitive: true,
        ..Default::default()
    };
    let m = search_in_file("foo fao fxo", &q).unwrap_or_default();
    assert_eq!(m.len(), 3);
}

#[test]
fn whole_word_excludes_substrings() {
    let q = SearchQuery {
        pattern: "cat".into(),
        whole_word: true,
        case_sensitive: true,
        ..Default::default()
    };
    let m = search_in_file("cat category cat", &q).unwrap_or_default();
    assert_eq!(m.len(), 2);
    assert_eq!(m[0].start, 0);
    assert_eq!(m[1].start, 13);
}

#[test]
fn line_and_col_are_tracked() {
    // Both the literal fast-path and the regex path must agree on line/col.
    let lit = search_in_file("ab\ncd ef\ncd", &literal("cd")).unwrap_or_default();
    let re = SearchQuery {
        pattern: "cd".into(),
        regex: true,
        case_sensitive: true,
        ..Default::default()
    };
    let rex = search_in_file("ab\ncd ef\ncd", &re).unwrap_or_default();
    for m in [&lit, &rex] {
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].line, m[0].col), (1, 0));
        assert_eq!((m[1].line, m[1].col), (2, 0));
    }
}

/// The whole point of the public matcher: compile once, run many. A reused
/// matcher must agree with the one-shot `search_in_file` on every buffer.
#[test]
fn a_reused_matcher_agrees_with_search_in_file() {
    for query in [
        literal("foo"),
        SearchQuery {
            pattern: "fo+".into(),
            regex: true,
            ..Default::default()
        },
        SearchQuery {
            pattern: "foo".into(),
            whole_word: true,
            ..Default::default()
        },
        SearchQuery {
            pattern: "FOO".into(),
            ..Default::default()
        },
    ] {
        let matcher = match Matcher::new(&query) {
            Ok(matcher) => matcher,
            Err(_) => continue,
        };
        for text in ["foo bar foo", "nothing", "foofoo", "a foo\nfoo b", ""] {
            assert_eq!(
                matcher.find(text),
                search_in_file(text, &query).unwrap_or_default(),
                "pattern {:?} over {text:?}",
                query.pattern,
            );
        }
    }
}

#[test]
fn a_matcher_rejects_an_invalid_regex() {
    let q = SearchQuery {
        pattern: "(".into(),
        regex: true,
        ..Default::default()
    };
    assert!(Matcher::new(&q).is_err());
}

/// An empty pattern never matches, so a caller that reuses one matcher over a
/// whole tree cannot accidentally report every file as a hit.
#[test]
fn a_matcher_built_from_an_empty_pattern_never_matches() {
    let matcher = Matcher::new(&literal(""));
    assert_eq!(
        matcher.map(|m| m.find("anything at all").len()).ok(),
        Some(0)
    );
}

/// Every query kind other than the literal fast path compiles to a regex, and
/// an empty regex (or `\b(?:)\b`) matches at every position or word boundary.
/// The empty guard must hold for all of them, through every entry point.
#[test]
fn an_empty_pattern_never_matches_under_any_option() {
    let text = "alpha beta\ngamma";
    for (regex, case_sensitive, whole_word) in [
        (false, false, false),
        (false, true, true),
        (true, true, false),
        (true, false, true),
    ] {
        let q = SearchQuery {
            regex,
            case_sensitive,
            whole_word,
            ..Default::default()
        };
        let kind = (regex, case_sensitive, whole_word);
        let matcher = Matcher::new(&q);
        assert_eq!(
            matcher.as_ref().map(|m| m.find(text).len()).ok(),
            Some(0),
            "{kind:?}"
        );
        assert_eq!(
            matcher
                .as_ref()
                .map(|m| m.find_chunks(["alpha be", "ta\ngamma"]).len())
                .ok(),
            Some(0),
            "{kind:?}"
        );
        assert_eq!(
            matcher.map(|m| m.plan(text, "x", regex).len()).ok(),
            Some(0),
            "{kind:?}"
        );
    }
}

#[test]
fn invalid_regex_errors() {
    let q = SearchQuery {
        pattern: "(".into(),
        regex: true,
        ..Default::default()
    };
    assert_eq!(search_in_file("x", &q), Err(SearchError::InvalidPattern));
}

#[test]
fn empty_pattern_returns_nothing() {
    assert!(
        search_in_file("abc", &literal(""))
            .unwrap_or_default()
            .is_empty()
    );
}

#[test]
fn zero_width_pattern_terminates() {
    let q = SearchQuery {
        pattern: "x*".into(),
        regex: true,
        case_sensitive: true,
        ..Default::default()
    };
    // Iterating zero-width matches terminates, yielding one empty match at
    // every char boundary (the regex crate's documented semantics).
    let m = search_in_file("abc", &q).unwrap_or_default();
    let spans: Vec<(usize, usize)> = m.iter().map(|m| (m.start, m.end)).collect();
    assert_eq!(spans, [(0, 0), (1, 1), (2, 2), (3, 3)]);
}

use std::ops::ControlFlow;
use std::path::Path;

use tempfile::TempDir;

use crate::walk::BINARY_SNIFF_BYTES;
use crate::walk::MAX_FILE_BYTES;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Create a fresh temp directory, removed on drop.
fn temp_dir() -> std::io::Result<TempDir> {
    TempDir::new()
}

/// Write `contents` to `dir/rel`, creating parent directories.
fn write(dir: &Path, rel: &str, contents: &[u8]) -> std::io::Result<()> {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)
}

/// Collect every hit from a workspace search into a path-sorted vector.
fn collect(root: &Path, query: &SearchQuery) -> Vec<FileHit> {
    let mut hits = Vec::new();
    let _ = WorkspaceSearch::new().run(root, query, |hit| hits.push(hit));
    hits.sort_by(|a, b| a.path.cmp(&b.path));
    hits
}

#[test]
fn workspace_search_finds_matching_files() -> TestResult {
    let dir = temp_dir()?;
    write(dir.path(), "a.txt", b"needle here\nand again needle\n")?;
    write(dir.path(), "sub/b.txt", b"needle in subdir\n")?;
    write(dir.path(), "c.txt", b"nothing of interest\n")?;

    let hits = collect(dir.path(), &literal("needle"));
    assert_eq!(hits.len(), 2);
    // a.txt has two matches; the subdir file one.
    let total: usize = hits.iter().map(|h| h.matches.len()).sum();
    assert_eq!(total, 3);
    Ok(())
}

#[test]
fn workspace_search_respects_gitignore() -> TestResult {
    let dir = temp_dir()?;
    write(dir.path(), ".gitignore", b"ignored.txt\n")?;
    write(dir.path(), "kept.txt", b"needle\n")?;
    write(dir.path(), "ignored.txt", b"needle\n")?;

    let hits = collect(dir.path(), &literal("needle"));
    assert_eq!(hits.len(), 1);
    assert!(hits[0].path.ends_with("kept.txt"));
    Ok(())
}

#[test]
fn workspace_search_prunes_ignored_dirs_without_a_gitignore() -> TestResult {
    let dir = temp_dir()?;
    write(dir.path(), "kept.txt", b"needle\n")?;
    // No .gitignore anywhere: pruning must come from IGNORED_DIRS itself.
    write(dir.path(), "target/debug/build.log", b"needle\n")?;
    write(dir.path(), "node_modules/pkg/index.js", b"needle\n")?;

    let hits = collect(dir.path(), &literal("needle"));
    assert_eq!(hits.len(), 1);
    assert!(hits[0].path.ends_with("kept.txt"));
    Ok(())
}

#[test]
fn workspace_search_skips_binary_files() -> TestResult {
    let dir = temp_dir()?;
    write(dir.path(), "text.txt", b"needle\n")?;
    write(dir.path(), "blob.bin", b"needle\x00\x01needle")?;

    let hits = collect(dir.path(), &literal("needle"));
    assert_eq!(hits.len(), 1);
    assert!(hits[0].path.ends_with("text.txt"));
    Ok(())
}

#[test]
fn workspace_search_applies_include_globs() -> TestResult {
    let dir = temp_dir()?;
    write(dir.path(), "keep.rs", b"needle\n")?;
    write(dir.path(), "skip.md", b"needle\n")?;

    let query = SearchQuery {
        pattern: "needle".into(),
        case_sensitive: true,
        includes: vec!["*.rs".into()],
        ..Default::default()
    };
    let hits = collect(dir.path(), &query);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].path.ends_with("keep.rs"));
    Ok(())
}

#[test]
fn literal_replace_plans_and_applies() {
    let text = "foo bar foo";
    let plan = plan_replacements(text, &literal("foo"), "baz").unwrap_or_default();
    assert_eq!(plan.len(), 2);
    assert_eq!(apply_replacements(text, &plan), "baz bar baz");
}

#[test]
fn regex_replace_expands_capture_groups() {
    let q = SearchQuery {
        pattern: r"(\w+)=(\d+)".into(),
        regex: true,
        case_sensitive: true,
        ..Default::default()
    };
    let plan = plan_replacements("a=1 b=2", &q, "$2=$1").unwrap_or_default();
    assert_eq!(apply_replacements("a=1 b=2", &plan), "1=a 2=b");
}

#[test]
fn non_regex_replacement_is_literal_even_with_dollar() {
    // A whole-word (non-regex) query compiles to a regex internally, but a `$1`
    // in the replacement must be inserted verbatim, not treated as a capture.
    let q = SearchQuery {
        pattern: "x".into(),
        whole_word: true,
        case_sensitive: true,
        ..Default::default()
    };
    let plan = plan_replacements("x y x", &q, "$1").unwrap_or_default();
    assert_eq!(apply_replacements("x y x", &plan), "$1 y $1");
}

#[test]
fn apply_is_offset_safe_for_length_changing_edits() {
    // Replacements of differing lengths must not corrupt neighbours (right-to-left).
    let text = "aa bb aa";
    let plan = plan_replacements(text, &literal("aa"), "wide").unwrap_or_default();
    assert_eq!(apply_replacements(text, &plan), "wide bb wide");
}

fn rep(start: usize, end: usize, text: &str) -> Replacement {
    Replacement {
        start,
        end,
        text: text.to_owned(),
    }
}

#[test]
fn apply_is_independent_of_the_input_order() {
    // A caller may hand spans in any order; they are applied right-to-left
    // against the original offsets either way.
    let forward = [rep(0, 1, "XYZ"), rep(4, 5, "")];
    let backward = [rep(4, 5, ""), rep(0, 1, "XYZ")];
    assert_eq!(apply_replacements("a b c", &forward), "XYZ b ");
    assert_eq!(apply_replacements("a b c", &backward), "XYZ b ");
}

#[test]
fn apply_skips_invalid_spans_and_keeps_valid_ones() {
    // "héllo": `é` occupies bytes 1..3, so byte 2 is not a char boundary.
    let text = "héllo";
    let plan = [
        rep(2, 3, "!"),   // splits `é`: skipped
        rep(10, 12, "!"), // past the end: skipped
        rep(3, 1, "!"),   // inverted: skipped
        rep(5, 9, "!"),   // end past the end: skipped
        rep(0, 0, "<"),   // empty span: a pure insertion
        rep(5, 6, "O"),   // valid
    ];
    assert_eq!(apply_replacements(text, &plan), "<héllO");
    assert_eq!(apply_replacements(text, &[]), text);
}

#[test]
fn empty_pattern_plans_nothing() {
    assert!(
        plan_replacements("abc", &literal(""), "z")
            .unwrap_or_default()
            .is_empty()
    );
}

#[test]
fn workspace_replace_writes_matching_files_only() -> TestResult {
    let dir = temp_dir()?;
    write(dir.path(), "a.txt", b"needle and needle\n")?;
    write(dir.path(), "b.txt", b"nothing\n")?;
    write(dir.path(), ".gitignore", b"ignored.txt\n")?;
    write(dir.path(), "ignored.txt", b"needle\n")?;

    let summary = WorkspaceSearch::new().replace(dir.path(), &literal("needle"), "pin")?;
    assert_eq!(summary.files_changed, 1);
    assert_eq!(summary.replacements, 2);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt"))?,
        "pin and pin\n"
    );
    // The gitignored file is untouched.
    assert_eq!(
        std::fs::read_to_string(dir.path().join("ignored.txt"))?,
        "needle\n"
    );
    assert!(summary.failures.is_empty());
    Ok(())
}

/// Mark `path` read-only, and report whether a write to it now fails (it does
/// not for a privileged user, whom a read-only bit does not stop).
fn make_unwritable(path: &Path) -> std::io::Result<bool> {
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(path, perms)?;
    Ok(std::fs::OpenOptions::new().write(true).open(path).is_err())
}

/// A file that cannot be written back is reported, not silently skipped, and
/// does not stop the other files from being rewritten.
#[test]
fn workspace_replace_reports_a_file_it_could_not_write() -> TestResult {
    let dir = temp_dir()?;
    write(dir.path(), "a.txt", b"needle\n")?;
    write(dir.path(), "locked.txt", b"needle needle\n")?;
    let locked = dir.path().join("locked.txt");
    if !make_unwritable(&locked)? {
        return Ok(());
    }

    let summary = WorkspaceSearch::new().replace(dir.path(), &literal("needle"), "pin")?;
    assert_eq!((summary.files_changed, summary.replacements), (1, 1));
    let failed: Vec<&Path> = summary.failures.iter().map(|f| f.path.as_path()).collect();
    assert_eq!(failed, [locked.as_path()]);
    assert!(summary.failures.iter().all(|f| !f.error.is_empty()));
    assert_eq!(std::fs::read_to_string(dir.path().join("a.txt"))?, "pin\n");
    assert_eq!(std::fs::read_to_string(&locked)?, "needle needle\n");
    Ok(())
}

/// A write that fails after the file was found writable (here: no room for the
/// temp file, because the directory is read-only) leaves the file exactly as it
/// was instead of truncated, and is reported.
#[test]
fn workspace_replace_never_leaves_a_failed_file_truncated() -> TestResult {
    let dir = temp_dir()?;
    let sub = dir.path().join("sealed");
    std::fs::create_dir(&sub)?;
    write(&sub, "a.txt", b"needle\n")?;
    let original = std::fs::metadata(&sub)?.permissions();
    let mut sealed_perms = original.clone();
    sealed_perms.set_readonly(true);
    std::fs::set_permissions(&sub, sealed_perms)?;
    // A privileged user (or a platform without directory write bits) can still
    // create files there, so the failure this test needs cannot be provoked.
    let sealed = std::fs::File::create(sub.join("probe")).is_err();

    let summary = WorkspaceSearch::new().replace(dir.path(), &literal("needle"), "pin");
    let text = std::fs::read_to_string(sub.join("a.txt"));
    std::fs::set_permissions(&sub, original)?;
    if !sealed {
        return Ok(());
    }
    let summary = summary?;
    assert_eq!((summary.files_changed, summary.replacements), (0, 0));
    let failed: Vec<&Path> = summary.failures.iter().map(|f| f.path.as_path()).collect();
    assert_eq!(failed, [sub.join("a.txt").as_path()]);
    assert_eq!(text?, "needle\n");
    Ok(())
}

/// A successful replace keeps the file's permissions.
#[cfg(unix)]
#[test]
fn workspace_replace_keeps_file_permissions() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = temp_dir()?;
    write(dir.path(), "run.sh", b"needle\n")?;
    let path = dir.path().join("run.sh");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o751))?;

    let summary = WorkspaceSearch::new().replace(dir.path(), &literal("needle"), "pin")?;
    assert_eq!(summary.files_changed, 1);
    assert_eq!(std::fs::read_to_string(&path)?, "pin\n");
    assert_eq!(
        std::fs::metadata(&path)?.permissions().mode() & 0o777,
        0o751
    );
    Ok(())
}

/// Collect every file the walk visits, as root-relative path + contents.
fn walked(root: &Path, includes: &[String], excludes: &[String]) -> Vec<(String, String)> {
    let mut files = Vec::new();
    let _ = walk_text_files(root, includes, excludes, |path, text| {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        files.push((rel, text));
        ControlFlow::Continue(())
    });
    files.sort();
    files
}

#[test]
fn walk_visits_text_files_and_honors_gitignore() -> TestResult {
    let dir = temp_dir()?;
    write(dir.path(), "a.txt", b"alpha\n")?;
    write(dir.path(), "sub/b.txt", b"beta\n")?;
    write(dir.path(), ".gitignore", b"ignored.txt\n")?;
    write(dir.path(), "ignored.txt", b"gamma\n")?;

    let files = walked(dir.path(), &[], &[]);
    let names: Vec<&str> = files.iter().map(|(rel, _)| rel.as_str()).collect();
    assert!(names.contains(&"a.txt"), "{names:?}");
    assert!(names.contains(&"sub/b.txt"), "{names:?}");
    assert!(!names.contains(&"ignored.txt"), "{names:?}");
    assert_eq!(
        files.iter().find(|(rel, _)| rel == "a.txt").map(|(_, t)| t),
        Some(&"alpha\n".to_owned()),
        "the walk hands the sink the full file contents"
    );
    Ok(())
}

#[test]
fn walk_prunes_heavyweight_dirs_and_binary_files() -> TestResult {
    let dir = temp_dir()?;
    write(dir.path(), "keep.txt", b"text\n")?;
    // Pruned by IGNORED_DIRS even with no .gitignore mentioning them.
    write(dir.path(), "target/built.txt", b"artifact\n")?;
    write(dir.path(), "node_modules/dep.txt", b"vendored\n")?;
    // A NUL byte in the head marks the file binary.
    write(dir.path(), "blob.bin", b"pre\0post\n")?;

    assert_eq!(walked_names(dir.path()), ["keep.txt"]);
    Ok(())
}

/// The root-relative names the unfiltered walk visits.
fn walked_names(root: &Path) -> Vec<String> {
    walked(root, &[], &[])
        .into_iter()
        .map(|(rel, _)| rel)
        .collect()
}

#[test]
fn walk_skips_non_utf8_files_without_a_nul() -> TestResult {
    let dir = temp_dir()?;
    write(dir.path(), "keep.txt", b"text\n")?;
    // Latin-1 `é` (0xE9) is invalid UTF-8 but carries no NUL byte, so only
    // the UTF-8 decode rejects it.
    write(dir.path(), "latin1.txt", b"caf\xe9\n")?;
    assert_eq!(walked_names(dir.path()), ["keep.txt"]);
    Ok(())
}

#[test]
fn walk_reads_a_file_at_the_size_cap_and_skips_one_past_it() -> TestResult {
    let dir = temp_dir()?;
    // Text up front (so the binary sniff passes), then sparse NUL padding
    // past the sniff window: NUL is valid UTF-8, so size is the only skip.
    for (name, len) in [
        ("at_cap.txt", MAX_FILE_BYTES),
        ("over_cap.txt", MAX_FILE_BYTES + 1),
    ] {
        write(dir.path(), name, &[b'a'; BINARY_SNIFF_BYTES])?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(dir.path().join(name))?
            .set_len(len)?;
    }
    assert_eq!(walked_names(dir.path()), ["at_cap.txt"]);
    Ok(())
}

#[test]
fn walk_applies_include_and_exclude_globs() -> TestResult {
    let dir = temp_dir()?;
    write(dir.path(), "a.rs", b"rust\n")?;
    write(dir.path(), "b.md", b"markdown\n")?;
    write(dir.path(), "skip.rs", b"rust\n")?;

    let names: Vec<String> = walked(dir.path(), &["*.rs".to_owned()], &["skip.rs".to_owned()])
        .into_iter()
        .map(|(rel, _)| rel)
        .collect();
    assert_eq!(names, vec!["a.rs".to_owned()]);
    Ok(())
}

#[test]
fn walk_stops_early_on_break() -> TestResult {
    let dir = temp_dir()?;
    for i in 0..5 {
        write(dir.path(), &format!("f{i}.txt"), b"x\n")?;
    }
    let mut seen = 0_usize;
    walk_text_files(dir.path(), &[], &[], |_, _| {
        seen += 1;
        // A result cap / cancelled request stops the walk mid-tree.
        if seen == 2 {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })?;
    assert_eq!(seen, 2);
    Ok(())
}

#[test]
fn walk_surfaces_an_invalid_glob() -> TestResult {
    let dir = temp_dir()?;
    write(dir.path(), "a.txt", b"x\n")?;
    let result = walk_text_files(dir.path(), &["[".to_owned()], &[], |_, _| {
        ControlFlow::Continue(())
    });
    assert_eq!(result, Err(SearchError::InvalidPattern));
    Ok(())
}

#[test]
fn workspace_search_surfaces_invalid_pattern() -> TestResult {
    let dir = temp_dir()?;
    write(dir.path(), "a.txt", b"x\n")?;
    let query = SearchQuery {
        pattern: "(".into(),
        regex: true,
        ..Default::default()
    };
    let result = WorkspaceSearch::new().run(dir.path(), &query, |_| {});
    assert_eq!(result, Err(SearchError::InvalidPattern));
    Ok(())
}

/// The root-relative paths [`walk_files`] yields, sorted.
fn listed(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = walk_files(root)
        .map(|path| {
            path.strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    names.sort();
    names
}

#[test]
fn walk_files_honors_gitignore_and_prunes_ignored_dirs() -> TestResult {
    let dir = temp_dir()?;
    write(dir.path(), "a.txt", b"alpha\n")?;
    write(dir.path(), "sub/b.rs", b"beta\n")?;
    write(dir.path(), ".gitignore", b"ignored.txt\n")?;
    write(dir.path(), "ignored.txt", b"gamma\n")?;
    write(dir.path(), ".hidden", b"delta\n")?;
    // Pruned by IGNORED_DIRS even though no ignore file names them.
    write(dir.path(), "target/debug/out", b"x\n")?;
    write(dir.path(), "node_modules/pkg/index.js", b"x\n")?;

    assert_eq!(listed(dir.path()), ["a.txt", "sub/b.rs"]);
    Ok(())
}

#[test]
fn walk_files_lists_files_the_text_walk_skips() -> TestResult {
    let dir = temp_dir()?;
    write(dir.path(), "blob.bin", b"x\x00y")?;
    write(dir.path(), "latin1.txt", b"caf\xe9\n")?;
    write(dir.path(), "empty.txt", b"")?;

    // A file picker lists every openable file; contents are never read.
    assert_eq!(listed(dir.path()), ["blob.bin", "empty.txt", "latin1.txt"]);
    Ok(())
}

#[test]
fn walk_files_honors_an_ignore_file_above_the_root() -> TestResult {
    let dir = temp_dir()?;
    write(dir.path(), ".gitignore", b"*.log\n")?;
    write(dir.path(), "sub/keep.rs", b"x\n")?;
    write(dir.path(), "sub/drop.log", b"x\n")?;

    assert_eq!(listed(&dir.path().join("sub")), ["keep.rs"]);
    Ok(())
}

#[cfg(unix)]
#[test]
fn walk_files_never_follows_or_lists_symlinks() -> TestResult {
    use std::os::unix::fs::symlink;
    let dir = temp_dir()?;
    write(dir.path(), "real/file.txt", b"x\n")?;
    symlink(dir.path().join("real"), dir.path().join("dir_link"))?;
    symlink(dir.path().join("real/file.txt"), dir.path().join("file_link"))?;
    // A link back to the root would loop forever if followed.
    symlink(dir.path(), dir.path().join("real/cycle"))?;

    assert_eq!(listed(dir.path()), ["real/file.txt"]);
    Ok(())
}

#[test]
fn walk_files_of_a_missing_root_is_empty() -> TestResult {
    let dir = temp_dir()?;
    assert!(listed(&dir.path().join("absent")).is_empty());
    Ok(())
}
