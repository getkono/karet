//! The background highlight worker.
//!
//! Layered (injection-aware) highlighting is markedly more work than the flat kind: a
//! markdown file re-parses its inline grammar and every code fence's language. Running
//! that inline on the session actor would make the actor's command queue wait on it, so
//! it lives on its own thread and answers asynchronously.
//!
//! The worker owns its parse host and highlighter cache outright. Sharing the session's
//! would mean synchronizing them; a second `ParserPool` costs one parser per language
//! and removes the question entirely.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::ops::RangeInclusive;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;

use karet_syntax::FoldRegions;
use karet_syntax::Folder;
use karet_syntax::Highlights;
use karet_syntax::LayeredHighlighter;
use karet_syntax::OutlineExtractor;
use karet_syntax::SemanticBlocker;
use karet_syntax::SemanticBlocks;
use karet_syntax::SemanticCommentConfig;
use karet_text::TextBuffer;
use karet_text::TextSnapshot;
use karet_treesitter::Edit;
use karet_treesitter::LanguageId;
use karet_treesitter::LayeredParser;
use karet_treesitter::LayeredTree;
use karet_treesitter::Query;
use karet_treesitter::SyntaxTree;
use tokio::sync::mpsc as tokio_mpsc;

use crate::api::DocumentId;

mod overlay;
pub(crate) use overlay::overlay_semantic;

/// A unit of work for the highlight worker.
pub(crate) enum HighlightJob {
    /// (Re)highlight a document.
    Update(HighlightRequest),
    /// The document closed; forget its trees.
    Drop(DocumentId),
}

/// A request to highlight `text` as `lang`.
///
/// `text` is a [`TextSnapshot`]: building the request shares the rope, and the
/// one contiguous copy the parser needs is made here on the worker thread, only
/// for the request that survives [`merge`] — not on the session actor per edit.
pub(crate) struct HighlightRequest {
    pub doc: DocumentId,
    /// The buffer version `text` corresponds to. The session discards a result whose
    /// version it has already moved past.
    pub version: u64,
    pub lang: LanguageId,
    pub text: TextSnapshot,
    /// Language-resolved semantic-comment settings for this document version.
    pub semantic: Option<SemanticCommentConfig>,
    /// `Some(edits)` advances the retained tree incrementally; `None` forces a full
    /// parse (a fresh open, a reload, or a language change).
    pub edits: Option<Vec<Edit>>,
}

/// The worker's answer for one document version.
pub(crate) struct HighlightResult {
    pub doc: DocumentId,
    pub version: u64,
    pub highlights: Arc<Highlights>,
    pub folds: Arc<FoldRegions>,
    pub semantic_blocks: Arc<SemanticBlocks>,
    /// Grammar-backed document symbols for the same version.
    pub symbols: Arc<Vec<karet_core::Symbol>>,
    /// Line ranges covered by syntax errors (see `SyntaxTree::error_lines`).
    pub error_lines: Arc<Vec<(u32, u32)>>,
    /// Inclusive 0-based line ranges of the GFM tables in the root layer (see
    /// [`TableFinder`]); empty for a language with no table node.
    pub table_lines: Arc<Vec<RangeInclusive<u32>>>,
}

/// The largest grammar-less document whose fold regions come from its
/// indentation: the scan runs on the session actor for every edit, so a file
/// past the bound goes without folds rather than stall typing.
const INDENT_FOLD_MAX_BYTES: usize = 4 * 1024 * 1024;

/// The fold regions of a document no grammar parses, from its indentation,
/// with tabs as wide as the tab size `settings` resolve for `language` (see
/// [`karet_syntax::fold_by_indent`]).
pub(crate) fn indent_folds(
    settings: &crate::config::Settings,
    language: Option<&'static str>,
    buffer: &TextBuffer,
) -> Arc<FoldRegions> {
    if buffer.len_bytes() > INDENT_FOLD_MAX_BYTES {
        return Arc::default();
    }
    let tab_size = settings.editor.for_language(language).tab_size();
    let text = buffer.text_snapshot();
    Arc::new(karet_syntax::fold_by_indent(&text, u32::from(tab_size)))
}

/// The analyses the worker derives from each parse beyond highlighting, each
/// keeping its compiled queries across jobs.
struct Analyzers {
    blocker: SemanticBlocker,
    outline: OutlineExtractor,
    tables: TableFinder,
    folder: Folder,
}

impl Default for Analyzers {
    fn default() -> Self {
        Self {
            blocker: SemanticBlocker::new(),
            outline: OutlineExtractor::new(),
            tables: TableFinder::default(),
            folder: Folder::new(),
        }
    }
}

/// Start the worker thread, returning its job sender and result receiver.
///
/// If the thread cannot be spawned the result sender is dropped, the receiver closes,
/// and the session simply never receives highlights — degraded, not broken.
pub(crate) fn spawn() -> (
    Sender<HighlightJob>,
    tokio_mpsc::UnboundedReceiver<HighlightResult>,
) {
    let (jobs_tx, jobs_rx) = std::sync::mpsc::channel();
    let (results_tx, results_rx) = tokio_mpsc::unbounded_channel();
    // On spawn failure `results_tx` drops with the closure, closing `results_rx`; the
    // actor then stops selecting that arm and the editor runs without highlights.
    std::thread::Builder::new()
        .name("karet-highlight".to_owned())
        .spawn(move || run(&jobs_rx, &results_tx))
        .ok();
    (jobs_tx, results_rx)
}

/// The worker loop: absorb a burst of jobs, then compute one result per document.
///
/// Coalescing is by backpressure rather than a timer. After blocking for the first job
/// we drain everything already queued, so a keystroke burst that arrived while the
/// previous batch was computing collapses into a single reparse of the newest text. An
/// idle editor pays no added latency; a busy one batches automatically.
fn run(jobs: &Receiver<HighlightJob>, results: &tokio_mpsc::UnboundedSender<HighlightResult>) {
    let mut parser = LayeredParser::new();
    let mut highlighter = LayeredHighlighter::new();
    let mut analyzers = Analyzers::default();
    let mut trees: HashMap<DocumentId, Retained> = HashMap::new();
    let mut pending: HashMap<DocumentId, HighlightRequest> = HashMap::new();

    // NOTE: this is backpressure *coalescing*, deliberately not the spell
    // worker's deadline debounce: highlighting must start the moment the worker
    // is free (any added delay is visible as un-colored text), while burst
    // absorption below still collapses a fast edit run into one parse.
    loop {
        match jobs.recv() {
            Ok(job) => absorb(&mut pending, &mut trees, job),
            Err(_) => return, // the session dropped the sender
        }
        while let Ok(job) = jobs.try_recv() {
            absorb(&mut pending, &mut trees, job);
        }

        for (_, request) in pending.drain() {
            if let Some(result) = compute(
                &mut parser,
                &mut highlighter,
                &mut analyzers,
                &mut trees,
                request,
            ) && results.send(result).is_err()
            {
                return; // the session is gone
            }
        }
    }
}

/// What the worker keeps of a document between versions, so the next one can
/// reparse and re-highlight incrementally.
struct Retained {
    tree: LayeredTree,
    /// The highlights of `tree` before the semantic-comment pass, which runs
    /// afresh on every version.
    highlights: Arc<Highlights>,
}

/// Fold `job` into the pending set.
fn absorb(
    pending: &mut HashMap<DocumentId, HighlightRequest>,
    trees: &mut HashMap<DocumentId, Retained>,
    job: HighlightJob,
) {
    match job {
        HighlightJob::Drop(doc) => {
            pending.remove(&doc);
            trees.remove(&doc);
        },
        HighlightJob::Update(request) => {
            let merged = match pending.remove(&request.doc) {
                Some(previous) => merge(previous, request),
                None => request,
            };
            pending.insert(merged.doc, merged);
        },
    }
}

/// Collapse two queued requests for the same document into one.
///
/// The newer request's text and version win. Their edit lists concatenate, because the
/// retained tree must be walked through every intermediate state to stay valid — each
/// edit's coordinates are expressed in the frame its predecessors left behind. If
/// either side demands a full parse, or the language changed under us, so does the
/// merged request.
fn merge(previous: HighlightRequest, next: HighlightRequest) -> HighlightRequest {
    let edits = match (previous.edits, next.edits) {
        (Some(mut earlier), Some(later)) if previous.lang == next.lang => {
            earlier.extend(later);
            Some(earlier)
        },
        _ => None,
    };
    HighlightRequest { edits, ..next }
}

/// Parse (incrementally where possible) and highlight one document version,
/// running the semantic-comment pass over the result when `semantic` is set.
///
/// An incremental reparse re-highlights only the ranges it reports changed
/// (see [`LayeredHighlighter::rehighlight`]); the result is the same as a full
/// highlight. Folds, blocks, symbols, error lines and tables are still derived
/// from the whole tree.
fn compute(
    parser: &mut LayeredParser,
    highlighter: &mut LayeredHighlighter,
    analyzers: &mut Analyzers,
    trees: &mut HashMap<DocumentId, Retained>,
    request: HighlightRequest,
) -> Option<HighlightResult> {
    let text = request.text.as_str();
    let reusable = request.edits.is_some()
        && trees
            .get(&request.doc)
            .is_some_and(|kept| kept.tree.root().language() == request.lang);
    let edits = request.edits.as_deref().unwrap_or(&[]);

    let mut changed = None;
    if reusable {
        let reparsed = trees
            .get_mut(&request.doc)
            .map(|kept| parser.reparse_changed(&mut kept.tree, edits, text));
        match reparsed {
            Some(Ok(ranges)) => changed = Some(ranges),
            // Fall back to a cold parse rather than keep a tree of unknown validity.
            _ => drop(trees.remove(&request.doc)),
        }
    } else {
        trees.remove(&request.doc);
    }

    let kept = match trees.entry(request.doc) {
        Entry::Occupied(slot) => slot.into_mut(),
        Entry::Vacant(slot) => {
            let tree = parser.parse(request.lang, text).ok()?;
            let highlights = Arc::new(highlighter.highlight(&tree, text));
            slot.insert(Retained { tree, highlights })
        },
    };
    if let Some(changed) = changed {
        let fresh = highlighter.rehighlight(&kept.tree, text, &kept.highlights, edits, &changed);
        kept.highlights = Arc::new(fresh);
    }
    let tree = &kept.tree;

    let highlights = match request.semantic.as_ref() {
        Some(config) => Arc::new(karet_syntax::mark_semantic_comments(
            text,
            &kept.highlights,
            config,
        )),
        None => Arc::clone(&kept.highlights),
    };
    Some(HighlightResult {
        doc: request.doc,
        version: request.version,
        highlights,
        folds: Arc::new(analyzers.folder.analyze(tree.root(), text)),
        semantic_blocks: Arc::new(analyzers.blocker.analyze(tree.root(), text)),
        symbols: Arc::new(analyzers.outline.analyze_layers(tree, text)),
        error_lines: Arc::new(tree.error_lines()),
        table_lines: Arc::new(analyzers.tables.line_ranges(tree.root(), text)),
    })
}

/// The query naming a GFM table: tree-sitter-md's block grammar parses one as
/// a `pipe_table`, and never inside a fenced code block.
const TABLE_QUERY: &str = "(pipe_table) @table";

/// Finds the source lines GFM tables occupy, from the tree the worker already
/// parsed, so no reader has to re-parse the document to learn them.
///
/// Only the root layer is read: a table inside an injected layer (a fenced
/// `markdown` block) is fence content in the document, not a table of it.
#[derive(Default)]
struct TableFinder {
    /// The compiled query per root language; `None` for a grammar without a
    /// table node, remembered so it is not recompiled for every version.
    queries: HashMap<LanguageId, Option<Query>>,
}

impl TableFinder {
    /// The inclusive 0-based line ranges of every table in `tree`, whose
    /// source `text` is — the same ranges `karet_markdown::table_line_ranges`
    /// reports: a table's trailing newline does not claim the next line.
    fn line_ranges(&mut self, tree: &SyntaxTree, text: &str) -> Vec<RangeInclusive<u32>> {
        let lang = tree.language();
        let query = self
            .queries
            .entry(lang)
            .or_insert_with(|| Query::compile(lang, TABLE_QUERY).ok());
        let Some(query) = query.as_ref() else {
            return Vec::new();
        };
        let mut lines = LineCounter::new(text);
        tree.captures(query, text)
            .into_iter()
            .map(|capture| {
                let (start, end) = (capture.span.start.0, capture.span.end.0);
                let first = lines.line_of(start);
                // The line holding the table's last byte.
                let last = lines.line_of(end.saturating_sub(1).max(start));
                first..=last.max(first)
            })
            .collect()
    }
}

/// Maps byte offsets to 0-based line numbers by counting newlines forward, so
/// offsets asked for in ascending order cost one pass over the text in total.
struct LineCounter<'a> {
    text: &'a [u8],
    /// The byte offset counted up to so far.
    at: usize,
    /// The newlines in `text[..at]`.
    line: usize,
}

impl<'a> LineCounter<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text: text.as_bytes(),
            at: 0,
            line: 0,
        }
    }

    /// The line `byte` sits on (clamped to the text's end).
    fn line_of(&mut self, byte: usize) -> u32 {
        let byte = byte.min(self.text.len());
        if byte < self.at {
            // Out of order: count again from the start.
            self.at = 0;
            self.line = 0;
        }
        let newlines = self
            .text
            .get(self.at..byte)
            .map_or(0, |bytes| bytes.iter().filter(|b| **b == b'\n').count());
        self.line += newlines;
        self.at = byte;
        u32::try_from(self.line).unwrap_or(u32::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(start: usize, old_end: usize, new_end: usize) -> Edit {
        Edit {
            start_byte: start,
            old_end_byte: old_end,
            new_end_byte: new_end,
            start_point: karet_core::BytePoint::new(0, start),
            old_end_point: karet_core::BytePoint::new(0, old_end),
            new_end_point: karet_core::BytePoint::new(0, new_end),
            replaced: String::new(),
        }
    }

    fn request(version: u64, text: &str, edits: Option<Vec<Edit>>) -> HighlightRequest {
        HighlightRequest {
            doc: DocumentId(1),
            version,
            lang: LanguageId(1),
            text: text.into(),
            semantic: None,
            edits,
        }
    }

    #[test]
    fn merge_takes_the_newer_text_and_concatenates_edits() {
        let merged = merge(
            request(1, "a", Some(vec![edit(0, 0, 1)])),
            request(2, "ab", Some(vec![edit(1, 1, 2)])),
        );
        assert_eq!(merged.version, 2);
        assert_eq!(merged.text.as_str(), "ab");
        let edits = merged.edits.unwrap_or_default();
        assert_eq!(edits.len(), 2, "both edits must reach the retained tree");
        assert_eq!(edits[0].new_end_byte, 1);
        assert_eq!(edits[1].new_end_byte, 2);
    }

    #[test]
    fn merge_forces_a_full_parse_when_either_side_demands_one() {
        // A reload (edits: None) invalidates the queued incremental edits before it.
        let merged = merge(
            request(1, "a", Some(vec![edit(0, 0, 1)])),
            request(2, "xyz", None),
        );
        assert!(merged.edits.is_none());

        // ...and an incremental request cannot ride a tree that was never advanced.
        let merged = merge(
            request(1, "a", None),
            request(2, "ab", Some(vec![edit(1, 1, 2)])),
        );
        assert!(merged.edits.is_none());
    }

    #[test]
    fn merge_forces_a_full_parse_when_the_language_changed() {
        let mut older = request(1, "a", Some(vec![edit(0, 0, 1)]));
        older.lang = LanguageId(2);
        let merged = merge(older, request(2, "ab", Some(vec![edit(1, 1, 2)])));
        assert!(merged.edits.is_none());
        assert_eq!(merged.lang, LanguageId(1), "the newer language wins");
    }

    #[test]
    fn absorb_collapses_repeat_updates_for_one_document() {
        let mut pending = HashMap::new();
        let mut trees = HashMap::new();
        absorb(
            &mut pending,
            &mut trees,
            HighlightJob::Update(request(1, "a", Some(vec![edit(0, 0, 1)]))),
        );
        absorb(
            &mut pending,
            &mut trees,
            HighlightJob::Update(request(2, "ab", Some(vec![edit(1, 1, 2)]))),
        );
        assert_eq!(pending.len(), 1, "a burst collapses to one request");
        let request = pending.remove(&DocumentId(1));
        assert!(
            request.as_ref().is_some_and(|r| !r.text.is_materialized()),
            "queueing and merging must not copy the document; only compute does"
        );
        let request = request.map(|r| (r.version, r.text.as_str().to_owned()));
        assert_eq!(request, Some((2, "ab".to_owned())));
    }

    /// Run `compute` over a rust buffer with a TODO comment, with the pass
    /// configured or not, and return the produced token ids.
    fn compute_tokens(semantic: Option<SemanticCommentConfig>) -> Option<Vec<u16>> {
        let rust = karet_treesitter::language_id_from_injection_name("rust")?;
        let text = "// TODO: fix this\nfn main() {}\n";
        let mut parser = LayeredParser::new();
        let mut highlighter = LayeredHighlighter::new();
        let mut trees = HashMap::new();
        let request = HighlightRequest {
            doc: DocumentId(1),
            version: 1,
            lang: rust,
            text: text.into(),
            semantic,
            edits: None,
        };
        let result = compute(
            &mut parser,
            &mut highlighter,
            &mut Analyzers::default(),
            &mut trees,
            request,
        )?;
        Some(result.highlights.all().iter().map(|s| s.token.0).collect())
    }

    /// The token runs `compute` publishes for `text`, against `trees`.
    fn compute_spans(
        trees: &mut HashMap<DocumentId, Retained>,
        lang: LanguageId,
        text: &str,
        edits: Option<Vec<Edit>>,
    ) -> Option<Vec<(usize, usize, u16)>> {
        let request = HighlightRequest {
            doc: DocumentId(1),
            version: 1,
            lang,
            text: text.into(),
            semantic: Some(SemanticCommentConfig::default()),
            edits,
        };
        let result = compute(
            &mut LayeredParser::new(),
            &mut LayeredHighlighter::new(),
            &mut Analyzers::default(),
            trees,
            request,
        )?;
        Some(
            result
                .highlights
                .all()
                .iter()
                .map(|s| (s.span.start.0, s.span.end.0, s.token.0))
                .collect(),
        )
    }

    #[test]
    fn incremental_versions_publish_what_a_cold_highlight_would() {
        let Some(rust) = karet_treesitter::language_id_from_injection_name("rust") else {
            return; // rust grammar not compiled in; nothing to test
        };
        let mut text = String::from("fn a() {}\n\n/// Doc.\nfn b() { let x = 1; }\n");
        let mut trees = HashMap::new();
        assert!(compute_spans(&mut trees, rust, &text, None).is_some());
        // Each step: (needle to replace, replacement); the version after each is
        // computed incrementally and must match a cold compute of the same text.
        for (needle, with) in [
            ("let x", "// TODO: x\n    let x"),
            ("1;", "\"open"),
            ("\"open", "\"closed\";"),
            ("fn a", "/* fn a"),
            ("/* ", ""),
        ] {
            let edit = replace(&mut text, needle, with);
            let incremental = compute_spans(&mut trees, rust, &text, Some(vec![edit]));
            let cold = compute_spans(&mut HashMap::new(), rust, &text, None);
            assert!(incremental.is_some());
            assert_eq!(incremental, cold, "after replacing {needle:?} in {text:?}");
        }
    }

    /// Replace the first `needle` in `text` with `with`, returning the edit that
    /// describes the change in the frame of the text before it.
    fn replace(text: &mut String, needle: &str, with: &str) -> Edit {
        let start = text.find(needle).unwrap_or_default();
        let end = start + needle.len();
        let old = text.clone();
        text.replace_range(start..end, with);
        let point = |s: &str, byte: usize| {
            let before = s.get(..byte).unwrap_or("");
            let row = before.matches('\n').count();
            let col = before.rfind('\n').map_or(byte, |i| byte - i - 1);
            karet_core::BytePoint::new(row, col)
        };
        Edit {
            start_byte: start,
            old_end_byte: end,
            new_end_byte: start + with.len(),
            start_point: point(&old, start),
            old_end_point: point(&old, end),
            new_end_point: point(text, start + with.len()),
            replaced: needle.to_owned(),
        }
    }

    #[test]
    fn two_merged_edits_publish_what_a_cold_highlight_would() {
        let Some(rust) = karet_treesitter::language_id_from_injection_name("rust") else {
            return; // rust grammar not compiled in; nothing to test
        };
        let mut text = String::from("fn a() {}\n\nfn b() { let x = 1; }\n");
        let mut trees = HashMap::new();
        assert!(compute_spans(&mut trees, rust, &text, None).is_some());
        // Two queued versions, the second's coordinates in the frame the first
        // left behind; the worker merges them and walks the tree through both.
        // The second edit opens a string the first edit's text precedes, so
        // applying only one of them, or both in the wrong frame, changes spans.
        let first = replace(&mut text, "fn a() {}", "// TODO: a\nfn a() {}");
        let v1 = HighlightRequest {
            lang: rust,
            ..request(1, &text, Some(vec![first]))
        };
        let second = replace(&mut text, "1;", "\"open");
        let v2 = HighlightRequest {
            lang: rust,
            ..request(2, &text, Some(vec![second]))
        };
        let merged = merge(v1, v2);
        assert_eq!(merged.text.as_str(), text, "the newer text wins");
        let edits = merged.edits.unwrap_or_default();
        assert_eq!(
            edits.len(),
            2,
            "same-language incremental requests stay incremental and keep both edits"
        );

        let incremental = compute_spans(&mut trees, rust, &text, Some(edits));
        let cold = compute_spans(&mut HashMap::new(), rust, &text, None);
        assert!(incremental.is_some());
        assert_eq!(
            incremental, cold,
            "merged edits must match a cold parse of {text:?}"
        );
    }

    #[test]
    fn compute_runs_the_semantic_pass_when_configured() {
        let mark = karet_core::StandardToken::CommentMark.id().0;
        let config = SemanticCommentConfig::default();
        let Some(tokens) = compute_tokens(Some(config)) else {
            return; // rust grammar not compiled in; nothing to test
        };
        assert!(
            tokens.contains(&mark),
            "the TODO comment should be restamped CommentMark, got {tokens:?}"
        );
    }

    #[test]
    fn compute_skips_the_semantic_pass_when_disabled() {
        let mark = karet_core::StandardToken::CommentMark.id().0;
        let comment = karet_core::TokenId::COMMENT.0;
        let Some(tokens) = compute_tokens(None) else {
            return; // rust grammar not compiled in; nothing to test
        };
        // The comment is still highlighted — just as an ordinary comment.
        assert!(tokens.contains(&comment));
        assert!(
            !tokens.contains(&mark),
            "disabled pass must leave comments unmarked, got {tokens:?}"
        );
    }

    /// The table line ranges `compute` publishes for `text` parsed as Markdown.
    fn table_lines(text: &str) -> Option<Vec<RangeInclusive<u32>>> {
        let markdown = karet_treesitter::language_id_from_path(std::path::Path::new("a.md"))?;
        let mut trees = HashMap::new();
        let request = HighlightRequest {
            doc: DocumentId(1),
            version: 1,
            lang: markdown,
            text: text.into(),
            semantic: None,
            edits: None,
        };
        let result = compute(
            &mut LayeredParser::new(),
            &mut LayeredHighlighter::new(),
            &mut Analyzers::default(),
            &mut trees,
            request,
        )?;
        Some(result.table_lines.as_ref().clone())
    }

    #[test]
    fn a_table_publishes_the_lines_it_occupies() {
        let found = table_lines("intro\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nafter\n");
        assert_eq!(found, Some(vec![2..=4]));
    }

    #[test]
    fn a_table_ending_the_file_without_a_newline_keeps_its_last_line() {
        let found = table_lines("| a | b |\n|---|---|\n| 1 | 2 |");
        assert_eq!(found, Some(vec![0..=2]));
    }

    #[test]
    fn every_table_is_found_in_document_order() {
        let source = "| a |\n|---|\n\ntext\n\n| b |\n|---|\n| 2 |\n";
        assert_eq!(table_lines(source), Some(vec![0..=1, 5..=7]));
    }

    #[test]
    fn a_table_inside_a_fenced_block_is_not_a_table() {
        // Even a `markdown` fence, whose injected layer parses the table: it is
        // fence content in this document, so only the root layer counts.
        for fence in ["```", "```markdown"] {
            let source = format!("{fence}\n| a | b |\n|---|---|\n| 1 | 2 |\n```\n");
            assert_eq!(table_lines(&source), Some(Vec::new()), "{fence}");
        }
    }

    #[test]
    fn a_quoted_table_is_a_table() {
        let found = table_lines("> | a | b |\n> |---|---|\n> | 1 | 2 |\n\nafter\n");
        assert_eq!(found, Some(vec![0..=2]));
    }

    #[test]
    fn a_table_in_a_list_item_is_a_table() {
        let found = table_lines("- item\n\n  | a | b |\n  |---|---|\n  | 1 | 2 |\n");
        assert_eq!(found, Some(vec![2..=4]));
    }

    #[test]
    fn pipes_without_a_delimiter_row_are_not_a_table() {
        assert_eq!(table_lines("a | b\nc | d\n"), Some(Vec::new()));
    }

    #[test]
    fn a_grammar_without_tables_publishes_none() {
        let Some(rust) = karet_treesitter::language_id_from_injection_name("rust") else {
            return; // rust grammar not compiled in; nothing to test
        };
        let mut pool = LayeredParser::new();
        let Ok(tree) = pool.parse(rust, "fn main() { let x = a | b; }\n") else {
            return;
        };
        let mut finder = TableFinder::default();
        assert!(
            finder
                .line_ranges(tree.root(), "fn main() { let x = a | b; }\n")
                .is_empty()
        );
        assert!(finder.queries.get(&rust).is_some_and(Option::is_none));
    }

    #[test]
    fn line_counter_answers_out_of_order_offsets() {
        let mut lines = LineCounter::new("a\nb\nc\n");
        assert_eq!(lines.line_of(4), 2);
        assert_eq!(lines.line_of(2), 1);
        assert_eq!(lines.line_of(0), 0);
        assert_eq!(lines.line_of(99), 3);
    }

    #[test]
    fn absorb_drop_forgets_pending_work_and_trees() {
        let mut pending = HashMap::new();
        let mut trees = HashMap::new();
        absorb(
            &mut pending,
            &mut trees,
            HighlightJob::Update(request(1, "a", None)),
        );
        absorb(&mut pending, &mut trees, HighlightJob::Drop(DocumentId(1)));
        assert!(pending.is_empty());
        assert!(trees.is_empty());
    }

    #[test]
    fn compute_publishes_the_fold_query_regions_with_their_kinds() {
        use karet_syntax::FoldKind;
        let Some(rust) = karet_treesitter::language_id_from_injection_name("rust") else {
            return;
        };
        let text = "use a;\nuse b;\n// one\n// two\nfn f() {\n    1;\n}\n";
        let request = HighlightRequest {
            doc: DocumentId(1),
            version: 1,
            lang: rust,
            text: text.into(),
            semantic: None,
            edits: None,
        };
        let result = compute(
            &mut LayeredParser::new(),
            &mut LayeredHighlighter::new(),
            &mut Analyzers::default(),
            &mut HashMap::new(),
            request,
        );
        let folds: Vec<_> = result
            .iter()
            .flat_map(|r| r.folds.regions().iter().map(|f| (f.start, f.end, f.kind)))
            .collect();
        assert_eq!(
            folds,
            [
                (0, 1, FoldKind::Imports),
                (2, 3, FoldKind::Comment),
                (4, 6, FoldKind::Code),
            ]
        );
    }

    #[test]
    fn indent_folds_honour_the_tab_size_and_the_size_bound() {
        let buffer = TextBuffer::from_text("a\n\tb\n    c\n");
        // At widths 4 and 8 the four spaces are no deeper than the tab, so both
        // lines sit directly under `a`; at width 2 they nest inside the tab's line.
        let ends = |tab| {
            let mut settings = crate::config::Settings::default();
            settings.editor.tab_size = tab;
            indent_folds(&settings, None, &buffer)
                .regions()
                .iter()
                .map(|r| (r.start, r.end))
                .collect::<Vec<_>>()
        };
        assert_eq!(ends(4), [(0, 2)]);
        assert_eq!(ends(8), [(0, 2)]);
        assert_eq!(ends(2), [(0, 2), (1, 2)]);
        let huge = TextBuffer::from_text(&format!("a\n  b\n{}", " ".repeat(INDENT_FOLD_MAX_BYTES)));
        let settings = crate::config::Settings::default();
        assert!(indent_folds(&settings, None, &huge).regions().is_empty());
    }
}
