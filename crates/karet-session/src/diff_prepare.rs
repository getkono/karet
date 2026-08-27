//! Preparing displayable diffs on the repository worker.
//!
//! Diffing and syntax-highlighting a changed file is the expensive half of
//! showing it; it runs here — on the serialized VCS worker, never a client
//! thread — and ships as plain [`PreparedChange`] data over the event stream.
//! Painting stays with the presentation layer (see `karet-diff`'s `view`
//! feature).

use std::path::PathBuf;

use karet_core::BytePos;
use karet_core::Span as ByteSpan;
use karet_diff::DiffOptions;
use karet_diff::PreparedDiff;
use karet_diff::TokenSpan;
use karet_diff::diff_text;
use karet_filetype::file_type_for_path;
use karet_syntax::LayeredHighlighter;
use karet_treesitter::LanguageId;
use karet_treesitter::LayeredParser;
use karet_treesitter::language_id_from_path;
use karet_vcs::FileChange;
use karet_vcs::StatusKind;

use crate::api::ChangeSummary;
use crate::api::PreparedChange;

/// Reduce a change to its status listing entry: identity plus `(added, removed)`
/// line counts, dropping the file contents.
pub(crate) fn summarize(change: &FileChange) -> ChangeSummary {
    let (added, removed) = if change.is_binary {
        (0, 0)
    } else {
        karet_diff::line_stats(&diff_text(
            &change.old,
            &change.new,
            &DiffOptions::default(),
        ))
    };
    ChangeSummary {
        path: change.path.clone(),
        old_path: change.old_path.clone(),
        status: change.status,
        is_binary: change.is_binary,
        added,
        removed,
    }
}

/// Prepare a repository change for display: diff it and (when `syntax`)
/// syntax-highlight both sides.
pub(crate) fn prepare_change(change: FileChange, syntax: bool) -> PreparedChange {
    prepare(
        change.path,
        change.old_path,
        change.status,
        change.is_binary,
        &change.old,
        &change.new,
        syntax,
    )
}

/// Prepare an ad-hoc diff of two texts for display. `is_binary` marks a change
/// whose sides are not text (the texts are then ignored).
pub(crate) fn prepare_texts(
    path: PathBuf,
    old: &str,
    new: &str,
    is_binary: bool,
    syntax: bool,
) -> PreparedChange {
    prepare(
        path,
        None,
        StatusKind::Modified,
        is_binary,
        old,
        new,
        syntax,
    )
}

fn prepare(
    path: PathBuf,
    old_path: Option<PathBuf>,
    status: StatusKind,
    is_binary: bool,
    old: &str,
    new: &str,
    syntax: bool,
) -> PreparedChange {
    let language = file_type_for_path(&path).name().to_string();
    let mut diff = diff_text(
        old,
        new,
        &DiffOptions {
            path_hint: Some(path.to_string_lossy().into_owned()),
            ..Default::default()
        },
    );
    diff.is_binary = is_binary;
    let lang = if syntax {
        language_id_from_path(&path)
    } else {
        None
    };
    let old_tokens = line_tokens(old, lang);
    let new_tokens = line_tokens(new, lang);
    PreparedChange {
        path,
        old_path,
        status,
        language,
        diff: PreparedDiff::new(diff, old_tokens, new_tokens),
    }
}

/// Parse and highlight `content`, returning the syntax token runs for each line.
/// Returns an empty table (plaintext) when there is no grammar or parsing fails.
///
/// Shared with the seam worker's source previews, which colour a window of a file nobody
/// has open — the same problem this solves for a diff of one.
pub(crate) fn line_tokens(content: &str, lang: Option<LanguageId>) -> Vec<Vec<TokenSpan>> {
    let Some(lang) = lang.filter(|_| !content.is_empty()) else {
        return Vec::new();
    };
    // Layered, so a diff of a markdown file still colours its code fences.
    let highlights = (|| {
        let mut parser = LayeredParser::new();
        let tree = parser.parse(lang, content).ok()?;
        Some(LayeredHighlighter::new().highlight(&tree, content))
    })();
    let Some(highlights) = highlights else {
        return Vec::new();
    };

    let mut table = Vec::new();
    let mut line_start = 0usize;
    for line in content.split_inclusive('\n') {
        let line_end = line_start + line.len();
        let spans = highlights.spans_in(ByteSpan {
            start: BytePos(line_start),
            end: BytePos(line_end),
        });
        let toks = spans
            .iter()
            .filter_map(|s| {
                let start = s.span.start.0.max(line_start) - line_start;
                let end = s.span.end.0.min(line_end) - line_start;
                (end > start).then_some(TokenSpan {
                    start,
                    end,
                    token: s.token,
                })
            })
            .collect();
        table.push(toks);
        line_start = line_end;
    }
    table
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn change(path: &str, old: &str, new: &str) -> FileChange {
        FileChange {
            path: PathBuf::from(path),
            old_path: None,
            status: StatusKind::Modified,
            is_binary: false,
            old: old.to_owned(),
            new: new.to_owned(),
        }
    }

    #[test]
    fn summarize_counts_lines_without_contents() {
        let summary = summarize(&change("notes.txt", "a\nb\nc\n", "a\nB\nc\nd\n"));
        assert_eq!((summary.added, summary.removed), (2, 1));
        assert_eq!(summary.path, Path::new("notes.txt"));
    }

    #[test]
    fn summarize_binary_change_has_zero_counts() {
        let mut c = change("img.png", "", "");
        c.is_binary = true;
        let summary = summarize(&c);
        assert!(summary.is_binary);
        assert_eq!((summary.added, summary.removed), (0, 0));
    }

    #[test]
    fn rust_change_is_highlighted_when_syntax_is_on() {
        // The workspace test build compiles the Rust grammar in.
        let prepared = prepare_change(change("src/main.rs", "fn a() {}\n", "fn b() {}\n"), true);
        assert_eq!(prepared.language, "Rust");
        assert!(prepared.diff.old_tokens.iter().any(|line| !line.is_empty()));
    }

    #[test]
    fn syntax_disabled_produces_no_tokens() {
        let prepared = prepare_change(change("src/main.rs", "fn a() {}\n", "fn b() {}\n"), false);
        assert_eq!(prepared.language, "Rust"); // label still shown
        assert!(prepared.diff.old_tokens.is_empty() && prepared.diff.new_tokens.is_empty());
    }

    #[test]
    fn unknown_extension_falls_back_to_plaintext_tokens() {
        let prepared = prepare_change(change("notes.unknownext", "alpha\n", "beta\n"), true);
        assert!(prepared.diff.old_tokens.is_empty() && prepared.diff.new_tokens.is_empty());
        assert_eq!(prepared.diff.line_stats(), (1, 1));
    }

    #[test]
    fn prepare_texts_marks_binary() {
        let prepared = prepare_texts(PathBuf::from("img.png"), "", "", true, true);
        assert!(prepared.diff.is_binary());
    }
}
