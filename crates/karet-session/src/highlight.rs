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
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;

use karet_syntax::FoldRegions;
use karet_syntax::Highlights;
use karet_syntax::LayeredHighlighter;
use karet_syntax::SemanticCommentConfig;
use karet_treesitter::Edit;
use karet_treesitter::LanguageId;
use karet_treesitter::LayeredParser;
use karet_treesitter::LayeredTree;
use tokio::sync::mpsc as tokio_mpsc;

use crate::api::DocumentId;

/// A unit of work for the highlight worker.
pub(crate) enum HighlightJob {
    /// (Re)highlight a document.
    Update(HighlightRequest),
    /// The document closed; forget its trees.
    Drop(DocumentId),
}

/// A request to highlight `text` as `lang`.
pub(crate) struct HighlightRequest {
    pub doc: DocumentId,
    /// The buffer version `text` corresponds to. The session discards a result whose
    /// version it has already moved past.
    pub version: u64,
    pub lang: LanguageId,
    pub text: String,
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
    /// Line ranges covered by syntax errors (see `SyntaxTree::error_lines`).
    pub error_lines: Arc<Vec<(u32, u32)>>,
}

/// Start the worker thread, returning its job sender and result receiver.
///
/// `semantic` is the semantic-comment pass to run over every fresh highlight
/// (`None` when `editor.semanticComments` is disabled). It is fixed at spawn
/// because settings are loaded once per session; a settings-reload seam would
/// respawn or re-message the worker.
///
/// If the thread cannot be spawned the result sender is dropped, the receiver closes,
/// and the session simply never receives highlights — degraded, not broken.
pub(crate) fn spawn(
    semantic: Option<SemanticCommentConfig>,
) -> (
    Sender<HighlightJob>,
    tokio_mpsc::UnboundedReceiver<HighlightResult>,
) {
    let (jobs_tx, jobs_rx) = std::sync::mpsc::channel();
    let (results_tx, results_rx) = tokio_mpsc::unbounded_channel();
    // On spawn failure `results_tx` drops with the closure, closing `results_rx`; the
    // actor then stops selecting that arm and the editor runs without highlights.
    std::thread::Builder::new()
        .name("karet-highlight".to_owned())
        .spawn(move || run(&jobs_rx, &results_tx, semantic.as_ref()))
        .ok();
    (jobs_tx, results_rx)
}

/// The worker loop: absorb a burst of jobs, then compute one result per document.
///
/// Coalescing is by backpressure rather than a timer. After blocking for the first job
/// we drain everything already queued, so a keystroke burst that arrived while the
/// previous batch was computing collapses into a single reparse of the newest text. An
/// idle editor pays no added latency; a busy one batches automatically.
fn run(
    jobs: &Receiver<HighlightJob>,
    results: &tokio_mpsc::UnboundedSender<HighlightResult>,
    semantic: Option<&SemanticCommentConfig>,
) {
    let mut parser = LayeredParser::new();
    let mut highlighter = LayeredHighlighter::new();
    let mut trees: HashMap<DocumentId, LayeredTree> = HashMap::new();
    let mut pending: HashMap<DocumentId, HighlightRequest> = HashMap::new();

    loop {
        match jobs.recv() {
            Ok(job) => absorb(&mut pending, &mut trees, job),
            Err(_) => return, // the session dropped the sender
        }
        while let Ok(job) = jobs.try_recv() {
            absorb(&mut pending, &mut trees, job);
        }

        for (_, request) in pending.drain() {
            if let Some(result) =
                compute(&mut parser, &mut highlighter, &mut trees, request, semantic)
                && results.send(result).is_err()
            {
                return; // the session is gone
            }
        }
    }
}

/// Fold `job` into the pending set.
fn absorb(
    pending: &mut HashMap<DocumentId, HighlightRequest>,
    trees: &mut HashMap<DocumentId, LayeredTree>,
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
fn compute(
    parser: &mut LayeredParser,
    highlighter: &mut LayeredHighlighter,
    trees: &mut HashMap<DocumentId, LayeredTree>,
    request: HighlightRequest,
    semantic: Option<&SemanticCommentConfig>,
) -> Option<HighlightResult> {
    let reusable = request.edits.is_some()
        && trees
            .get(&request.doc)
            .is_some_and(|tree| tree.root().language() == request.lang);

    if reusable {
        let edits = request.edits.as_deref().unwrap_or(&[]);
        let failed = trees
            .get_mut(&request.doc)
            .is_some_and(|tree| parser.reparse(tree, edits, &request.text).is_err());
        if failed {
            // Fall back to a cold parse rather than keep a tree of unknown validity.
            trees.remove(&request.doc);
        }
    } else {
        trees.remove(&request.doc);
    }

    if let Entry::Vacant(slot) = trees.entry(request.doc) {
        slot.insert(parser.parse(request.lang, &request.text).ok()?);
    }
    let tree = trees.get(&request.doc)?;

    let mut highlights = highlighter.highlight(tree, &request.text);
    if let Some(config) = semantic {
        highlights = karet_syntax::mark_semantic_comments(&request.text, &highlights, config);
    }
    Some(HighlightResult {
        doc: request.doc,
        version: request.version,
        highlights: Arc::new(highlights),
        folds: Arc::new(karet_syntax::fold(tree.root())),
        error_lines: Arc::new(tree.error_lines()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(start: usize, old_end: usize, new_end: usize) -> Edit {
        Edit {
            start_byte: start,
            old_end_byte: old_end,
            new_end_byte: new_end,
            start_point: (0, start),
            old_end_point: (0, old_end),
            new_end_point: (0, new_end),
        }
    }

    fn request(version: u64, text: &str, edits: Option<Vec<Edit>>) -> HighlightRequest {
        HighlightRequest {
            doc: DocumentId(1),
            version,
            lang: LanguageId(1),
            text: text.to_owned(),
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
        assert_eq!(merged.text, "ab");
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
        let request = pending.remove(&DocumentId(1)).map(|r| (r.version, r.text));
        assert_eq!(request, Some((2, "ab".to_owned())));
    }

    /// Run `compute` over a rust buffer with a TODO comment, with the pass
    /// configured or not, and return the produced token ids.
    fn compute_tokens(semantic: Option<&SemanticCommentConfig>) -> Option<Vec<u16>> {
        let rust = karet_treesitter::language_id_from_injection_name("rust")?;
        let text = "// TODO: fix this\nfn main() {}\n";
        let mut parser = LayeredParser::new();
        let mut highlighter = LayeredHighlighter::new();
        let mut trees = HashMap::new();
        let request = HighlightRequest {
            doc: DocumentId(1),
            version: 1,
            lang: rust,
            text: text.to_owned(),
            edits: None,
        };
        let result = compute(&mut parser, &mut highlighter, &mut trees, request, semantic)?;
        Some(result.highlights.all().iter().map(|s| s.token.0).collect())
    }

    #[test]
    fn compute_runs_the_semantic_pass_when_configured() {
        let mark = karet_core::StandardToken::CommentMark.id().0;
        let config = SemanticCommentConfig::default();
        let Some(tokens) = compute_tokens(Some(&config)) else {
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
}
