//! Evaluating a query against an index.
//!
//! Evaluation is side-effect free: it reads the index and returns a node set, never
//! mutating anything. That is what lets the same call serve the filter box, a preview of
//! a narrowing the user has not committed to, and an agent asking a question.

use std::collections::HashSet;

use karet_fuzzy::Matcher;

use super::Query;
use super::TermKind;
use crate::id::SeamId;
use crate::index::SeamIndex;
use crate::model::ConfigMembership;

/// The node set a query selects, plus what produced it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryResult {
    /// The matching nodes, in the index's traversal order.
    pub nodes: Vec<SeamId>,
    /// The configuration the query asked to be evaluated under, if it named one.
    pub configuration: Option<String>,
}

impl QueryResult {
    /// How many nodes matched.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether nothing matched.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// Evaluate `query` against `index`.
///
/// An empty query selects every node, so the filter box starting empty shows the whole
/// tree rather than nothing.
#[must_use]
pub fn evaluate(query: &Query, index: &SeamIndex) -> QueryResult {
    // Start from the roots' traversal order so results are stable and parent-first.
    let mut candidates: Vec<SeamId> = index
        .roots()
        .iter()
        .flat_map(|root| index.subtree(*root))
        .collect();
    // A node whose parent is missing would otherwise never be reachable.
    if candidates.len() < index.len() {
        let seen: HashSet<SeamId> = candidates.iter().copied().collect();
        let mut orphans: Vec<SeamId> = index
            .nodes()
            .map(|node| node.id)
            .filter(|id| !seen.contains(id))
            .collect();
        orphans.sort();
        candidates.extend(orphans);
    }

    for term in &query.terms {
        let matched = matching(index, &candidates, &term.kind);
        candidates.retain(|id| matched.contains(id) != term.negated);
    }

    QueryResult {
        nodes: candidates,
        configuration: query.configuration.clone(),
    }
}

/// The subset of `candidates` a single term matches, before negation is applied.
fn matching(index: &SeamIndex, candidates: &[SeamId], kind: &TermKind) -> HashSet<SeamId> {
    match kind {
        TermKind::Name(pattern) => fuzzy(index, candidates, pattern),
        TermKind::Phrase(phrase) => candidates
            .iter()
            .copied()
            .filter(|id| {
                index
                    .node(*id)
                    .is_some_and(|node| node.name.contains(phrase.as_str()))
            })
            .collect(),
        TermKind::Pivot { edge, target } => {
            let Some(source) = index.resolve(target) else {
                return HashSet::new();
            };
            // Follow the relation in both directions: a trait's implementors point *at*
            // it, while a re-export points *away*, and the reader means "what is on the
            // other end" either way.
            index
                .edges()
                .from_of_kind(source, *edge)
                .filter_map(|e| e.to.resolved())
                .chain(index.edges().to_of_kind(source, *edge).map(|e| e.from))
                .collect()
        },
        _ => candidates
            .iter()
            .copied()
            .filter(|id| {
                index
                    .node(*id)
                    .is_some_and(|node| simple_match(index, node, kind))
            })
            .collect(),
    }
}

/// Match the terms that need only the node itself.
fn simple_match(index: &SeamIndex, node: &crate::model::Node, kind: &TermKind) -> bool {
    match kind {
        TermKind::Lens(lens) => node.has_lens(*lens),
        TermKind::Facet { lens, subtype } => node.has_subtype(*lens, subtype),
        // Ordered, so `vis:crate` means "at least as reachable as crate-visible" — the
        // question a reader asking about exposure actually has.
        TermKind::Visibility(level) => node
            .effective_visibility()
            .is_some_and(|actual| actual >= *level),
        TermKind::Kind(wanted) => node.kind == *wanted,
        TermKind::In(path) => index
            .path(node.id)
            .is_some_and(|actual| actual.is_under(path)),
        TermKind::Cfg(text) => {
            // An inactive or indeterminate node is by definition gated, and a node
            // carrying a matching predicate is too even when the gate currently holds.
            node.membership != ConfigMembership::Active
                && node.facets_for(crate::model::Lens::Variation).count() > 0
                || node
                    .facets_for(crate::model::Lens::Variation)
                    .filter_map(|facet| facet.detail.as_deref())
                    .any(|detail| detail.contains(text.as_str()))
        },
        TermKind::Name(_) | TermKind::Phrase(_) | TermKind::Pivot { .. } => false,
    }
}

/// Rank candidate names against a fuzzy pattern.
///
/// Ranking is a whole-set operation, so this collects the surviving names and ranks once
/// rather than scoring node by node — which is also what keeps the matching identical to
/// every other fuzzy surface in the application.
fn fuzzy(index: &SeamIndex, candidates: &[SeamId], pattern: &str) -> HashSet<SeamId> {
    if pattern.is_empty() {
        return candidates.iter().copied().collect();
    }
    let names: Vec<String> = candidates
        .iter()
        .map(|id| {
            index
                .node(*id)
                .map(|node| node.name.clone())
                .unwrap_or_default()
        })
        .collect();
    let mut matcher = Matcher::new();
    matcher
        .rank_indices(pattern, &names)
        .into_iter()
        .filter_map(|position| candidates.get(position).copied())
        .collect()
}
