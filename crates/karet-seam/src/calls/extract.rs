//! Running a language's call query over one parsed file.
//!
//! The query does the language-specific part — which syntax is a call, a body, a
//! comment, an import — and everything here is neutral: matches are merged per call,
//! attributed to the innermost callable around them, and turned into the per-file facts
//! the index stores. Nothing here resolves a name; that needs every file.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

use karet_core::LineIndex;
use karet_core::Range;
use karet_core::Span;
use karet_treesitter::LanguageId;
use karet_treesitter::Query;
use karet_treesitter::SyntaxTree;

use super::lines;
use super::site;
use super::site::CallSite;
use super::site::Callable;
use super::site::FileCalls;
use super::site::Import;
use super::site::Receiver;
use crate::id::SeamId;
use crate::index::SeamIndex;
use crate::lang::SeamLanguage;
use crate::model::FileId;
use crate::model::NodeKind;

/// Receivers longer than this are kept as [`Receiver::Expr`]: a path that long is a
/// chained expression, and nothing resolves by its text.
const MAX_RECEIVER: usize = 80;

/// Compiled call queries, one per grammar, shared by every thread.
type Compiled = HashMap<LanguageId, Option<Arc<Query>>>;

/// The compiled call query for `language`, compiling it on first use.
///
/// `None` when the language has no call query or its query does not compile — the
/// per-language tests make the second impossible to ship, and the precision ledger says
/// which languages had none.
#[must_use]
pub fn compiled(mapping: &dyn SeamLanguage, language: LanguageId) -> Option<Arc<Query>> {
    static COMPILED: OnceLock<Mutex<Compiled>> = OnceLock::new();
    let cache = COMPILED.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().ok()?;
    cache
        .entry(language)
        .or_insert_with(|| {
            mapping
                .call_query(language)
                .and_then(|source| Query::compile(language, source).ok())
                .map(Arc::new)
        })
        .clone()
}

/// A stable digest of every compiled-in call query.
///
/// What a file's call sites extract to depends on the query that found them, so a store
/// of extracted sites is only good for the queries it was written with. FNV-1a, because
/// the digest is written down and read back by a later build.
#[must_use]
pub fn fingerprint() -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    // Every grammar a mapping serves, not just its first: C and C++ share a mapping and
    // read with different queries.
    for mapping in crate::lang::registered() {
        for language in mapping.language_ids() {
            let source = mapping.call_query(language).unwrap_or_default();
            for byte in language.get().to_le_bytes().iter().chain(source.as_bytes()) {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(PRIME);
            }
        }
    }
    hash
}

/// One call's captures, gathered from every match that names its span.
#[derive(Default)]
struct Draft {
    name: Option<Span>,
    qualifier: Option<Span>,
    receiver: Option<Span>,
    is_self: bool,
    is_super: bool,
    construct: bool,
    external: bool,
    weak: bool,
}

/// What one file's matches produced, before attribution.
#[derive(Default)]
struct Harvest {
    calls: BTreeMap<(usize, usize), Draft>,
    bodies: Vec<Span>,
    comments: Vec<Span>,
    imports: Vec<Import>,
}

/// Record the call facts of the nodes in `added` and of `file` itself.
///
/// `added` is what extraction just put in the index, so every callable this file
/// declares is among them.
pub(crate) fn annotate(
    index: &mut SeamIndex,
    tree: &SyntaxTree,
    mapping: &dyn SeamLanguage,
    language: LanguageId,
    text: &str,
    file: FileId,
    added: &[SeamId],
) {
    let Some(query) = compiled(mapping, language) else {
        return;
    };
    let harvest = harvest(tree, &query, text);

    // Every function and member is a candidate; a member is a callable only when it has
    // a body or declares a contract's method, which is what tells a method from a field.
    let candidates: Vec<(SeamId, Span, bool)> = added
        .iter()
        .filter_map(|id| {
            let node = index.node(*id)?;
            let in_contract = node
                .parent
                .and_then(|parent| index.node(parent))
                .is_some_and(|parent| parent.kind == NodeKind::Interface);
            matches!(node.kind, NodeKind::Function | NodeKind::Member).then_some((
                *id,
                node.location.span,
                node.kind == NodeKind::Member && !in_contract,
            ))
        })
        .collect();
    let spans: Vec<Span> = candidates.iter().map(|(_, span, _)| *span).collect();
    let body_starts: Vec<usize> = harvest.bodies.iter().map(|body| body.start.0).collect();
    let mut has_body = vec![false; candidates.len()];
    for owner in lines::innermost(&spans, &body_starts).into_iter().flatten() {
        if let Some(flag) = has_body.get_mut(owner) {
            *flag = true;
        }
    }
    let callables: Vec<(SeamId, Span, bool)> = candidates
        .iter()
        .zip(&has_body)
        .filter(|((_, _, needs_body), body)| !*needs_body || **body)
        .map(|((id, span, _), body)| (*id, *span, *body))
        .collect();

    let spans: Vec<Span> = callables.iter().map(|(_, span, _)| *span).collect();
    let code = lines::owned_code_lines(text, &harvest.comments, &spans);
    let line_index = LineIndex::new(text);
    let mut sites: Vec<Vec<CallSite>> = vec![Vec::new(); callables.len()];
    let starts: Vec<usize> = harvest.calls.keys().map(|(start, _)| *start).collect();
    let owners = lines::innermost(&spans, &starts);
    let mut loose = 0u32;
    for (((start, end), draft), owner) in harvest.calls.iter().zip(owners) {
        let span = Span {
            start: karet_core::BytePos(*start),
            end: karet_core::BytePos(*end),
        };
        let range = line_index.range(text, span);
        let site = finish(draft, text, range);
        match owner.and_then(|owner| sites.get_mut(owner)) {
            Some(list) => list.push(site),
            None => loose = loose.saturating_add(1),
        }
    }

    for (((id, _, body), code_lines), sites) in callables.iter().zip(code).zip(sites) {
        let Some(node) = index.node(*id) else {
            continue;
        };
        let range = node.location.range;
        let callable = Callable {
            // A bare declaration's signature is not code anyone runs.
            code_lines: if *body { code_lines } else { 0 },
            span_lines: range
                .end
                .line
                .saturating_sub(range.start.line)
                .saturating_add(1),
            has_body: *body,
            sites,
        };
        index.calls_mut().insert(*id, callable);
    }
    index.calls_mut().set_file(
        file,
        FileCalls {
            imports: harvest.imports,
            loose_sites: loose,
        },
    );
}

/// Run the query and sort its captures by what they describe.
fn harvest(tree: &SyntaxTree, query: &Query, text: &str) -> Harvest {
    let names = query.capture_names();
    let mut out = Harvest::default();
    for matched in tree.matches(query, text) {
        let mut call = None;
        let mut draft = Draft::default();
        let mut import_path = None;
        let mut import_prefix = None;
        let mut import_alias = None;
        let mut import_glob = None;
        let mut import_head = false;
        for capture in &matched.captures {
            let Some(name) = names.get(capture.capture as usize) else {
                continue;
            };
            let span = capture.span;
            match *name {
                site::CALL => call = Some(span),
                site::CALL_NAME => draft.name = Some(span),
                site::CALL_QUALIFIER => draft.qualifier = Some(span),
                site::CALL_RECEIVER => draft.receiver = Some(span),
                site::CALL_SELF => draft.is_self = true,
                site::CALL_SUPER => draft.is_super = true,
                site::CALL_NEW => draft.construct = true,
                site::CALL_EXTERNAL => draft.external = true,
                site::CALL_WEAK => draft.weak = true,
                site::BODY => out.bodies.push(span),
                site::COMMENT => out.comments.push(span),
                site::IMPORT_PATH => import_path = Some(span),
                site::IMPORT_PREFIX => import_prefix = Some(span),
                site::IMPORT_ALIAS => import_alias = Some(span),
                site::IMPORT_GLOB => import_glob = Some(span),
                site::IMPORT_HEAD => import_head = true,
                // Captures prefixed `_` are the query's own scratch, used by predicates.
                _ => {},
            }
        }
        if let Some(call) = call {
            let merged = out.calls.entry((call.start.0, call.end.0)).or_default();
            merge(merged, draft);
        }
        if import_path.is_some() || import_glob.is_some() {
            let mut segments = import_prefix
                .map(|span| site::segments(slice(text, span)))
                .unwrap_or_default();
            let glob = import_glob.is_some();
            if let Some(path) = import_path.or(import_glob) {
                segments.extend(site::segments(slice(text, path)));
            }
            out.imports.push(Import {
                segments,
                alias: import_alias.map(|span| slice(text, span).to_owned()),
                glob,
                binds_head: import_head,
            });
        }
    }
    out
}

/// Fold one match's captures into the call they describe, keeping what is already known.
fn merge(into: &mut Draft, from: Draft) {
    into.name = into.name.or(from.name);
    into.qualifier = into.qualifier.or(from.qualifier);
    into.receiver = into.receiver.or(from.receiver);
    into.is_self |= from.is_self;
    into.is_super |= from.is_super;
    into.construct |= from.construct;
    into.external |= from.external;
    into.weak |= from.weak;
}

/// Turn a merged draft into the site the index stores.
fn finish(draft: &Draft, text: &str, range: Range) -> CallSite {
    let receiver = if draft.is_self {
        Some(Receiver::SelfRef)
    } else if draft.is_super {
        Some(Receiver::Super)
    } else {
        draft.receiver.map(|span| receiver(slice(text, span)))
    };
    CallSite {
        name: draft.name.map(|span| slice(text, span).to_owned()),
        qualifier: draft.qualifier.map(|span| slice(text, span).to_owned()),
        receiver,
        construct: draft.construct,
        external: draft.external,
        weak: draft.weak,
        range,
    }
}

/// Classify a receiver by its text.
fn receiver(written: &str) -> Receiver {
    let written = written.trim();
    match written {
        "self" | "this" => Receiver::SelfRef,
        "super" => Receiver::Super,
        _ if written.len() <= MAX_RECEIVER
            && !written.is_empty()
            && written
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '$')) =>
        {
            Receiver::Path(written.to_owned())
        },
        _ => Receiver::Expr,
    }
}

/// The text of a span, or nothing when it does not fall on character boundaries.
fn slice(text: &str, span: Span) -> &str {
    text.get(span.start.0..span.end.0).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receivers_are_classified_by_their_text() {
        assert_eq!(receiver("self"), Receiver::SelfRef);
        assert_eq!(receiver("this"), Receiver::SelfRef);
        assert_eq!(receiver("super"), Receiver::Super);
        assert_eq!(receiver("os.path"), Receiver::Path("os.path".to_owned()));
        assert_eq!(receiver("a::b"), Receiver::Path("a::b".to_owned()));
        assert_eq!(receiver("make()"), Receiver::Expr);
        assert_eq!(receiver("xs[0]"), Receiver::Expr);
        assert_eq!(receiver(&"a".repeat(81)), Receiver::Expr);
        assert_eq!(receiver(""), Receiver::Expr);
    }

    #[test]
    fn a_merged_call_keeps_the_first_name_and_every_flag() {
        let mut into = Draft {
            receiver: Some(Span::default()),
            ..Draft::default()
        };
        merge(
            &mut into,
            Draft {
                is_self: true,
                external: true,
                ..Draft::default()
            },
        );
        assert!(into.is_self && into.external);
        assert!(into.receiver.is_some());
        assert!(!into.weak);
    }
}
