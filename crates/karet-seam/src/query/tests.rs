//! Query parsing, evaluation, and the round-trip that makes UI state expressible.

use super::Query;
use super::TermKind;
use super::evaluate;
use super::parse;
use crate::edge::Edge;
use crate::edge::EdgeKind;
use crate::edge::Endpoint;
use crate::id::SeamPath;
use crate::index::SeamIndex;
use crate::model::ConfigMembership;
use crate::model::Facet;
use crate::model::FacetSubtype;
use crate::model::FileId;
use crate::model::Lens;
use crate::model::Node;
use crate::model::NodeKind;
use crate::model::SeamLocation;
use crate::model::Visibility;
use crate::rollup::Rollups;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Add a node with the given kind, visibility, and facets.
fn add(
    index: &mut SeamIndex,
    path: &str,
    kind: NodeKind,
    visibility: Option<Visibility>,
    facets: Vec<Facet>,
) -> crate::id::SeamId {
    let parsed: SeamPath = path.parse().unwrap_or_default();
    let parent = parsed.parent().and_then(|p| index.resolve(&p));
    let id = index.intern(parsed.clone());
    index.insert(Node {
        id,
        kind,
        name: parsed.leaf().unwrap_or_default().to_owned(),
        detail: None,
        location: SeamLocation {
            file: FileId(0),
            range: karet_core::Range::default(),
            span: karet_core::Span::default(),
            selection: karet_core::Range::default(),
            header: karet_core::Range::default(),
        },
        parent,
        children: Vec::new(),
        facets,
        visibility,
        rollups: Rollups::new(),
        membership: ConfigMembership::Active,
        provisional: false,
    });
    id
}

fn facet(lens: Lens, subtype: &'static str) -> Facet {
    Facet::new(lens, FacetSubtype(subtype))
}

/// A small index covering every term form.
fn fixture() -> SeamIndex {
    let mut index = SeamIndex::new();
    add(&mut index, "pkg", NodeKind::Package, None, vec![]);
    add(
        &mut index,
        "pkg::model",
        NodeKind::Module,
        Some(Visibility::Public),
        vec![],
    );
    add(
        &mut index,
        "pkg::model::Symbol",
        NodeKind::Type,
        Some(Visibility::Public),
        vec![facet(Lens::Api, "pub")],
    );
    add(
        &mut index,
        "pkg::model::Hidden",
        NodeKind::Type,
        Some(Visibility::Private),
        vec![facet(Lens::Api, "private")],
    );
    add(
        &mut index,
        "pkg::provider",
        NodeKind::Module,
        Some(Visibility::Crate),
        vec![],
    );
    add(
        &mut index,
        "pkg::provider::SymbolProvider",
        NodeKind::Interface,
        Some(Visibility::Public),
        vec![facet(Lens::Api, "pub"), facet(Lens::Substitution, "trait")],
    );
    add(
        &mut index,
        "pkg::provider::render",
        NodeKind::Function,
        Some(Visibility::Public),
        vec![
            facet(Lens::Substitution, "dyn"),
            facet(Lens::Hazard, "unsafe"),
        ],
    );
    add(
        &mut index,
        "pkg::gated",
        NodeKind::Module,
        Some(Visibility::Public),
        vec![facet(Lens::Variation, "cfg").with_detail("feature = \"view\"")],
    );
    index
}

/// The matching paths, sorted.
fn run(index: &SeamIndex, text: &str) -> Result<Vec<String>, super::QueryError> {
    let query = parse(text)?;
    let mut out: Vec<String> = evaluate(&query, index)
        .nodes
        .into_iter()
        .filter_map(|id| index.path(id).map(ToString::to_string))
        .collect();
    out.sort();
    Ok(out)
}

// --- parsing ----------------------------------------------------------------

#[test]
fn an_empty_query_selects_everything() -> TestResult {
    let index = fixture();
    assert!(parse("")?.is_empty());
    assert_eq!(run(&index, "")?.len(), index.len());
    Ok(())
}

#[test]
fn parses_each_term_form() -> TestResult {
    let query = parse("lens:hazard substitution:dyn vis:crate kind:type in:pkg::model cfg:unix")?;
    assert_eq!(query.terms.len(), 6);
    assert_eq!(query.terms[0].kind, TermKind::Lens(Lens::Hazard));
    assert_eq!(
        query.terms[1].kind,
        TermKind::Facet {
            lens: Lens::Substitution,
            subtype: "dyn".to_owned()
        }
    );
    assert_eq!(query.terms[2].kind, TermKind::Visibility(Visibility::Crate));
    assert_eq!(query.terms[3].kind, TermKind::Kind(NodeKind::Type));
    assert!(matches!(query.terms[4].kind, TermKind::In(_)));
    assert_eq!(query.terms[5].kind, TermKind::Cfg("unix".to_owned()));
    Ok(())
}

#[test]
fn a_bare_word_is_a_fuzzy_name_and_a_quoted_one_is_literal() -> TestResult {
    let query = parse("symbol \"Symbol Provider\"")?;
    assert_eq!(query.terms[0].kind, TermKind::Name("symbol".to_owned()));
    assert_eq!(
        query.terms[1].kind,
        TermKind::Phrase("Symbol Provider".to_owned())
    );
    Ok(())
}

#[test]
fn negation_applies_per_term() -> TestResult {
    let query = parse("!kind:member lens:api")?;
    assert!(query.terms[0].negated);
    assert!(!query.terms[1].negated);
    Ok(())
}

#[test]
fn an_unknown_term_is_a_positioned_error_with_suggestions() -> TestResult {
    let Err(error) = parse("lens:hazrd") else {
        return Err("expected a parse error".into());
    };
    assert!(error.message.contains("unknown lens"));
    assert!(
        error.suggestions.contains(&"hazard".to_owned()),
        "{:?}",
        error.suggestions
    );
    assert!(error.describe().contains("did you mean"));

    let Err(error) = parse("kind:member bogus:x") else {
        return Err("expected a parse error".into());
    };
    // The span must point at the offending term, not the whole query.
    assert_eq!(&"kind:member bogus:x"[error.span.clone()], "bogus:x");
    Ok(())
}

#[test]
fn an_unknown_facet_subtype_is_an_error_with_the_real_alternatives() -> TestResult {
    // Accepting it would match nothing while looking like it worked — the same silent
    // wrong answer as ignoring an unknown term.
    let Err(error) = parse("substitution:dynn") else {
        return Err("expected a parse error".into());
    };
    assert!(
        error.message.contains("substitution facet"),
        "{}",
        error.message
    );
    assert!(
        error.suggestions.contains(&"dyn".to_owned()),
        "{:?}",
        error.suggestions
    );
    // A real subtype still parses.
    assert!(parse("substitution:dyn").is_ok());
    assert!(parse("hazard:unsafe").is_ok());
    Ok(())
}

#[test]
fn an_unknown_term_is_never_silently_ignored() {
    // Ignoring it would hand back a different node set than was asked for, with no
    // indication anything was wrong — the failure that makes a filter untrustworthy.
    assert!(parse("nonsense:value").is_err());
    assert!(parse("vis:enormous").is_err());
    assert!(parse("kind:klass").is_err());
}

#[test]
fn a_term_with_no_value_is_an_error() {
    assert!(parse("lens:").is_err());
    assert!(parse("!").is_err());
}

#[test]
fn config_is_a_context_directive_not_a_filter() -> TestResult {
    let query = parse("config:all-features lens:api")?;
    assert_eq!(query.configuration.as_deref(), Some("all-features"));
    assert_eq!(query.terms.len(), 1, "config: is not a filtering term");

    // Negating or repeating it is a contradiction rather than a narrowing.
    assert!(parse("!config:default").is_err());
    assert!(parse("config:a config:b").is_err());
    assert!(parse("config:").is_err());
    Ok(())
}

#[test]
fn pivot_needs_an_edge_kind_and_a_path() -> TestResult {
    let query = parse("pivot:implements:pkg::provider::SymbolProvider")?;
    let TermKind::Pivot { edge, target } = &query.terms[0].kind else {
        return Err("expected a pivot term".into());
    };
    assert_eq!(*edge, EdgeKind::Implements);
    assert_eq!(target.to_string(), "pkg::provider::SymbolProvider");

    assert!(parse("pivot:implements").is_err());
    assert!(parse("pivot:calls:pkg::x").is_err());
    Ok(())
}

// --- totality ---------------------------------------------------------------

#[test]
fn every_query_round_trips_through_its_text_form() -> TestResult {
    // §8's totality requirement in practice: any state the UI can narrow to must be
    // expressible as a string, and that string must parse back to the same state.
    for text in [
        "lens:substitution",
        "!kind:member",
        "in:pkg::model kind:type",
        "substitution:dyn hazard:unsafe",
        "vis:public !lens:variation",
        "config:all-features lens:api",
        "pivot:implements:pkg::provider::SymbolProvider",
        "\"Symbol\" lens:api",
        "cfg:feature",
    ] {
        let parsed = parse(text)?;
        let rendered = parsed.to_string();
        let reparsed = parse(&rendered)?;
        assert_eq!(parsed, reparsed, "{text} did not survive a round trip");
    }
    Ok(())
}

#[test]
fn ui_narrowing_state_is_readable_back_out_of_a_query() -> TestResult {
    // Rerooting and lens toggles are what the breadcrumb is made of, so a query has to
    // surrender them again for the state to be restorable.
    let query = parse("in:pkg::model lens:api lens:hazard")?;
    assert_eq!(
        query.root().map(ToString::to_string),
        Some("pkg::model".to_owned())
    );
    assert_eq!(query.lenses(), vec![Lens::Api, Lens::Hazard]);
    Ok(())
}

// --- evaluation -------------------------------------------------------------

#[test]
fn filters_by_lens_and_by_specific_subtype() -> TestResult {
    let index = fixture();
    assert_eq!(run(&index, "lens:hazard")?, ["pkg::provider::render"]);
    assert_eq!(run(&index, "substitution:dyn")?, ["pkg::provider::render"]);
    assert_eq!(
        run(&index, "substitution:trait")?,
        ["pkg::provider::SymbolProvider"]
    );
    Ok(())
}

#[test]
fn terms_are_implicitly_conjoined() -> TestResult {
    let index = fixture();
    assert_eq!(
        run(&index, "lens:substitution lens:hazard")?,
        ["pkg::provider::render"]
    );
    Ok(())
}

#[test]
fn negation_removes_a_matching_set() -> TestResult {
    let index = fixture();
    let all_types = run(&index, "kind:type")?;
    assert_eq!(all_types, ["pkg::model::Hidden", "pkg::model::Symbol"]);
    assert_eq!(
        run(&index, "kind:type !api:private")?,
        ["pkg::model::Symbol"]
    );
    Ok(())
}

#[test]
fn containment_selects_a_whole_subtree_including_its_root() -> TestResult {
    let index = fixture();
    assert_eq!(
        run(&index, "in:pkg::model")?,
        ["pkg::model", "pkg::model::Hidden", "pkg::model::Symbol"]
    );
    Ok(())
}

#[test]
fn visibility_matches_at_least_as_reachable() -> TestResult {
    let index = fixture();
    // `vis:crate` includes public things, because they are more reachable, not less.
    let crate_visible = run(&index, "vis:crate")?;
    assert!(crate_visible.contains(&"pkg::provider".to_owned()));
    assert!(crate_visible.contains(&"pkg::model::Symbol".to_owned()));
    assert!(!crate_visible.contains(&"pkg::model::Hidden".to_owned()));
    Ok(())
}

#[test]
fn a_quoted_phrase_matches_a_literal_substring() -> TestResult {
    let index = fixture();
    assert_eq!(
        run(&index, "\"Symbol\"")?,
        ["pkg::model::Symbol", "pkg::provider::SymbolProvider"]
    );
    assert!(run(&index, "\"nothing here\"")?.is_empty());
    Ok(())
}

#[test]
fn a_bare_word_matches_fuzzily() -> TestResult {
    let index = fixture();
    let matched = run(&index, "symbolprov")?;
    assert!(
        matched.contains(&"pkg::provider::SymbolProvider".to_owned()),
        "got {matched:?}"
    );
    Ok(())
}

#[test]
fn a_pivot_follows_an_edge_in_both_directions() -> TestResult {
    let mut index = fixture();
    let Some(trait_id) = index.resolve(&"pkg::provider::SymbolProvider".parse()?) else {
        return Err("trait".into());
    };
    let Some(impl_id) = index.resolve(&"pkg::provider::render".parse()?) else {
        return Err("impl".into());
    };
    index.edges_mut().insert(Edge {
        kind: EdgeKind::Implements,
        from: impl_id,
        to: Endpoint::Resolved(trait_id),
        site: None,
    });

    // From the trait, the pivot reaches its implementor even though the edge points the
    // other way — "what is on the other end" is the question either way.
    assert_eq!(
        run(&index, "pivot:implements:pkg::provider::SymbolProvider")?,
        ["pkg::provider::render"]
    );
    assert_eq!(
        run(&index, "pivot:implements:pkg::provider::render")?,
        ["pkg::provider::SymbolProvider"]
    );
    Ok(())
}

#[test]
fn a_pivot_from_an_unknown_node_selects_nothing_rather_than_everything() -> TestResult {
    let index = fixture();
    assert!(run(&index, "pivot:implements:pkg::absent")?.is_empty());
    Ok(())
}

#[test]
fn a_cfg_term_finds_gated_nodes() -> TestResult {
    let index = fixture();
    assert_eq!(run(&index, "cfg:feature")?, ["pkg::gated"]);
    Ok(())
}

#[test]
fn evaluation_leaves_the_index_untouched() -> TestResult {
    let index = fixture();
    let before = index.len();
    let query = parse("lens:api !kind:module")?;
    let _ = evaluate(&query, &index);
    let _ = evaluate(&query, &index);
    assert_eq!(index.len(), before, "evaluation must be side-effect free");
    Ok(())
}

#[test]
fn the_configuration_directive_reaches_the_result() -> TestResult {
    let index = fixture();
    let query = parse("config:all-features lens:api")?;
    assert_eq!(
        evaluate(&query, &index).configuration.as_deref(),
        Some("all-features")
    );
    Ok(())
}

#[test]
fn results_are_stable_across_repeated_evaluation() -> TestResult {
    let index = fixture();
    let query = parse("lens:api")?;
    assert_eq!(
        evaluate(&query, &index).nodes,
        evaluate(&query, &index).nodes
    );
    Ok(())
}

#[test]
fn an_empty_index_yields_an_empty_result_without_panicking() -> TestResult {
    let index = SeamIndex::new();
    let query = parse("lens:api kind:type in:pkg")?;
    assert!(evaluate(&query, &index).is_empty());
    assert_eq!(Query::default().to_string(), "");
    Ok(())
}
