//! How much of the report is established, and how much is a bound.
//!
//! Counted per call *site*, not per edge: one ambiguous site fans out into several edges,
//! and counting those would make guesswork look like coverage. A site is classified by
//! the best answer it got — exact over dispatch over ambiguous over external over
//! unresolved.

use std::collections::BTreeMap;

use karet_core::Range;

use super::path_of;
use crate::calls::graph::FunctionMetrics;
use crate::edge::EdgeKind;
use crate::edge::Endpoint;
use crate::edge::Resolution;
use crate::id::SeamId;
use crate::index::SeamIndex;

/// One language's share of the scope, by how its call sites resolved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct LanguageLedger {
    /// The language.
    pub language: String,
    /// Functions in scope written in it.
    pub functions: usize,
    /// Their call sites.
    pub sites: usize,
    /// Sites with exactly one candidate.
    pub exact: usize,
    /// Sites through a contract.
    pub dispatch: usize,
    /// Sites with several candidates, each counted in the upper bound.
    pub ambiguous: usize,
    /// Sites with too many candidates to count in either bound.
    pub too_ambiguous: usize,
    /// Sites calling code outside the index.
    pub external: usize,
    /// Sites nothing plausible was found for.
    pub unresolved: usize,
    /// How test code is told apart in this language.
    pub test_detection: String,
}

/// A function whose numbers rest most on guesswork.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Uncertain {
    /// The function.
    pub path: String,
    /// Its sites.
    pub sites: usize,
    /// Its ambiguous and too-ambiguous sites.
    pub ambiguous: usize,
    /// Its unresolved sites.
    pub unresolved: usize,
}

/// The whole ledger.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct PrecisionLedger {
    /// Per language, in name order.
    pub languages: Vec<LanguageLedger>,
    /// The functions with the most uncertain sites, most first, then by path; at most
    /// [`ReportOptions::least_certain_n`](super::ReportOptions::least_certain_n) of them.
    pub least_certain: Vec<Uncertain>,
    /// Calls made outside any function — at module level, in initializers — which no
    /// function's numbers include. Counted over the whole index, whatever the scope.
    pub loose_sites: u64,
}

/// How a site resolved, best answer first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    Exact,
    Dispatch,
    Ambiguous,
    TooAmbiguous,
    External,
    Unresolved,
}

/// The ledger over the functions in scope.
pub(super) fn account(
    index: &SeamIndex,
    measured: &[&FunctionMetrics],
    least_certain_n: usize,
) -> PrecisionLedger {
    let mut languages: BTreeMap<String, LanguageLedger> = BTreeMap::new();
    let mut least_certain = Vec::new();
    for function in measured {
        let language = language_of(index, function.id);
        let outcomes = site_outcomes(index, function.id);
        let entry = languages
            .entry(language.clone())
            .or_insert_with(|| LanguageLedger {
                test_detection: if language.eq_ignore_ascii_case("rust") {
                    "cfg(test)".to_owned()
                } else {
                    "path convention".to_owned()
                },
                language,
                ..LanguageLedger::default()
            });
        let sites = outcomes.len();
        entry.functions += 1;
        entry.sites += sites;
        let mut ambiguous = 0;
        let mut unresolved = 0;
        for outcome in outcomes {
            match outcome {
                Outcome::Exact => entry.exact += 1,
                Outcome::Dispatch => entry.dispatch += 1,
                Outcome::Ambiguous => {
                    entry.ambiguous += 1;
                    ambiguous += 1;
                },
                Outcome::TooAmbiguous => {
                    entry.too_ambiguous += 1;
                    ambiguous += 1;
                },
                Outcome::External => entry.external += 1,
                Outcome::Unresolved => {
                    entry.unresolved += 1;
                    unresolved += 1;
                },
            }
        }
        if ambiguous + unresolved > 0 {
            least_certain.push(Uncertain {
                path: path_of(index, function.id),
                sites,
                ambiguous,
                unresolved,
            });
        }
    }
    least_certain.sort_by(|a, b| {
        (b.ambiguous + b.unresolved)
            .cmp(&(a.ambiguous + a.unresolved))
            .then_with(|| a.path.cmp(&b.path))
    });
    least_certain.truncate(least_certain_n);
    PrecisionLedger {
        languages: languages.into_values().collect(),
        least_certain,
        loose_sites: index
            .calls()
            .files()
            .iter()
            .map(|(_, calls)| u64::from(calls.loose_sites))
            .sum(),
    }
}

/// The language a function is written in, by its file.
fn language_of(index: &SeamIndex, id: SeamId) -> String {
    index
        .node(id)
        .and_then(|node| index.file_path(node.location.file))
        .and_then(karet_treesitter::language_name_from_path)
        .unwrap_or("unknown")
        .to_owned()
}

/// Every call site of a function, with the best answer it got.
fn site_outcomes(index: &SeamIndex, id: SeamId) -> Vec<Outcome> {
    let mut sites: BTreeMap<SiteKey, Outcome> = BTreeMap::new();
    for edge in index.edges().from_of_kind(id, EdgeKind::Calls) {
        let outcome = match (edge.resolution, &edge.to) {
            (Some(Resolution::Exact), _) => Outcome::Exact,
            (Some(Resolution::Dispatch | Resolution::DispatchImpl), _) => Outcome::Dispatch,
            (Some(Resolution::Ambiguous), Endpoint::Resolved(_)) => Outcome::Ambiguous,
            (Some(Resolution::Ambiguous), _) => Outcome::TooAmbiguous,
            (Some(Resolution::External), _) => Outcome::External,
            _ => Outcome::Unresolved,
        };
        let key = SiteKey::from(edge.site);
        let best = sites.entry(key).or_insert(outcome);
        *best = (*best).min(outcome);
    }
    sites.into_values().collect()
}

/// A site's position, as an ordered key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct SiteKey(u32, u32, u32, u32);

impl From<Option<Range>> for SiteKey {
    fn from(range: Option<Range>) -> Self {
        range.map_or(Self(0, 0, 0, 0), |r| {
            Self(r.start.line, r.start.col, r.end.line, r.end.col)
        })
    }
}
