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
    assert!(parse("pivot:called-by:pkg::x").is_err());
    assert!(parse("pivot:calls:pkg::x").is_ok());
    Ok(())
}

#[test]
fn reach_and_callers_need_a_path_and_print_back_as_written() -> TestResult {
    for text in ["reach:pkg::run", "callers:pkg::a::b", "!reach:pkg::run"] {
        assert_eq!(parse(text)?.to_string(), text);
    }
    let Err(error) = parse("lens:api reach:") else {
        return Err("an empty path should be refused".into());
    };
    assert_eq!(error.span, 9..15);
    let Err(error) = parse("reachh:pkg") else {
        return Err("a misspelt prefix should be refused".into());
    };
    assert!(error.suggestions.contains(&"reach".to_owned()), "{error:?}");
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
fn a_value_with_whitespace_or_a_quote_is_quoted_and_reads_back() -> TestResult {
    let impl_block: SeamPath = "pkg::{impl Render for Widget}".parse()?;
    for (kind, text) in [
        (
            TermKind::In(impl_block.clone()),
            r#"in:"pkg::{impl Render for Widget}""#,
        ),
        (
            TermKind::Reach(impl_block.clone()),
            r#"reach:"pkg::{impl Render for Widget}""#,
        ),
        (
            TermKind::Callers(impl_block.clone()),
            r#"callers:"pkg::{impl Render for Widget}""#,
        ),
        (
            TermKind::Pivot {
                edge: EdgeKind::Calls,
                target: impl_block,
            },
            r#"pivot:calls:"pkg::{impl Render for Widget}""#,
        ),
        (
            TermKind::Cfg("all(unix, windows)".to_owned()),
            r#"cfg:"all(unix, windows)""#,
        ),
        (TermKind::Phrase(r#"a"b c"#.to_owned()), r#""a\"b c""#),
        (TermKind::Phrase(r"a\b".to_owned()), r#""a\\b""#),
        (TermKind::Cfg(r#"x"y"#.to_owned()), r#"cfg:"x\"y""#),
    ] {
        assert_eq!(kind.to_string(), text);
        let parsed = parse(text)?;
        let [term] = parsed.terms.as_slice() else {
            return Err(format!("{text} is not one term").into());
        };
        assert_eq!(term.kind, kind, "{text}");
    }

    let query = parse(r#"config:"my config" lens:api"#)?;
    assert_eq!(query.configuration.as_deref(), Some("my config"));
    assert_eq!(query.to_string(), r#"config:"my config" lens:api"#);
    Ok(())
}

#[test]
fn a_value_that_needs_no_quoting_keeps_its_bare_form() -> TestResult {
    // A balanced quote inside a value already tokenizes as one term, so quoting it would
    // only rewrite text the reader typed.
    for text in [
        r#"cfg:feature="view""#,
        r"cfg:a\b",
        "in:pkg::{impl}",
        "\"Symbol\"",
    ] {
        assert_eq!(parse(text)?.to_string(), text);
    }
    // A phrase written before escapes existed reads as it always did: a backslash before
    // anything but a quote or a backslash, and a quote short of the end, are literal.
    assert_eq!(
        parse(r#""a\nb""#)?.terms[0].kind,
        TermKind::Phrase(r"a\nb".to_owned())
    );
    assert_eq!(
        parse(r#""a"b"#)?.terms[0].kind,
        TermKind::Phrase(r#"a"b"#.to_owned())
    );
    Ok(())
}

#[test]
fn a_facet_subtype_is_quoted_and_unquoted_like_any_other_value() -> TestResult {
    // The generator draws only subtypes a language emits, none of which need quoting, so
    // the facet arm of both directions is pinned here.
    for (subtype, text) in [
        ("a b", r#"hazard:"a b""#),
        (r#"x"y"#, r#"hazard:"x\"y""#),
        ("unsafe", "hazard:unsafe"),
    ] {
        let kind = TermKind::Facet {
            lens: Lens::Hazard,
            subtype: subtype.to_owned(),
        };
        assert_eq!(kind.to_string(), text);
    }

    // A quoted known subtype reads as the bare one.
    assert_eq!(
        parse(r#"hazard:"unsafe""#)?.terms[0].kind,
        TermKind::Facet {
            lens: Lens::Hazard,
            subtype: "unsafe".to_owned(),
        }
    );

    // A subtype holding whitespace is unquoted before it is checked: with languages
    // compiled in it is refused by its unquoted name, and with none it reads back whole.
    let text = r#"hazard:"a b""#;
    if super::parse::known_subtypes(Lens::Hazard).is_empty() {
        assert_eq!(
            parse(text)?.terms[0].kind,
            TermKind::Facet {
                lens: Lens::Hazard,
                subtype: "a b".to_owned(),
            }
        );
    } else {
        let Err(error) = parse(text) else {
            return Err("expected an unknown hazard facet".into());
        };
        assert!(error.message.contains("`a b`"), "{}", error.message);
    }
    Ok(())
}

#[test]
fn an_empty_quoted_value_is_refused_like_an_empty_one() {
    assert!(parse(r#"in:"""#).is_err());
    assert!(parse(r#"cfg:"""#).is_err());
    assert!(parse(r#"config:"""#).is_err());
    assert!(parse(r#"pivot:calls:"""#).is_err());
}

/// A deterministic pseudo-random stream (an LCG), so a failure replays from its seed.
struct Rng(u64);

impl Rng {
    fn below(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from(self.0 >> 33).unwrap_or_default() % bound.max(1)
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }

    /// A word from `alphabet`, `min..=max` characters long.
    fn word(&mut self, alphabet: &[&str], min: usize, max: usize) -> String {
        let len = min + self.below(max - min + 1);
        (0..len).map(|_| *self.pick(alphabet)).collect()
    }
}

/// Characters a bare word (a name, a plain path segment) can hold and still survive its
/// text form, since a bare word has no quoting form: no whitespace, no quote, none of the
/// path syntax (`:`, `#`, braces), and no leading `!` — but multibyte text,
/// so a span that lands inside a character would show up.
const WORD: &[&str] = &[
    "a", "z", "Q", "_", "-", ".", "0", "7", "<", ">", "(", ")", ",", "=", "é", "日", "🦀",
];

/// Characters a quotable value can hold: [`WORD`] plus whitespace, quotes, and the
/// backslash that escapes them, so only the quoting form can carry the value.
const VALUE: &[&str] = &[
    "a", "z", "Q", "_", "-", ".", "0", "7", "<", ">", "(", ")", ",", "=", "é", "日", "🦀", " ",
    "\t", "\"", "\\",
];

/// A path segment: plain, numbered, or an anonymous `{…}` construct, which may hold
/// whitespace and quotes as `{impl Render for Widget}` does.
fn segment(rng: &mut Rng) -> crate::id::SeamSegment {
    let name = if rng.chance(4) {
        format!("{{{}}}", rng.word(VALUE, 1, 6))
    } else {
        rng.word(WORD, 1, 6)
    };
    let ordinal = if rng.chance(3) {
        u32::try_from(1 + rng.below(40)).unwrap_or(1)
    } else {
        0
    };
    crate::id::SeamSegment::numbered(name, ordinal)
}

fn path(rng: &mut Rng) -> SeamPath {
    let len = 1 + rng.below(4);
    SeamPath::new((0..len).map(|_| segment(rng)).collect())
}

/// One term of any kind, with a value its text form can carry.
fn term_kind(rng: &mut Rng) -> TermKind {
    let lens = *rng.pick(&crate::model::LENSES);
    match rng.below(11) {
        0 => TermKind::Lens(lens),
        1 => {
            let known = super::parse::known_subtypes(lens);
            let subtype = if known.is_empty() {
                rng.word(WORD, 1, 8)
            } else {
                rng.pick(&known).clone()
            };
            TermKind::Facet { lens, subtype }
        },
        2 => TermKind::Visibility(*rng.pick(Visibility::all())),
        3 => TermKind::Kind(*rng.pick(NodeKind::all())),
        4 => TermKind::In(path(rng)),
        // A predicate value may itself hold `:` and a balanced quote, as
        // `cfg:feature="view"` does, or whitespace, as `all(unix, windows)` does.
        5 => TermKind::Cfg(match rng.below(4) {
            0 => format!("{}=\"{}\"", rng.word(WORD, 1, 5), rng.word(WORD, 1, 5)),
            1 => format!("{}:{}", rng.word(WORD, 1, 5), rng.word(WORD, 1, 5)),
            2 => rng.word(VALUE, 1, 8),
            _ => rng.word(WORD, 1, 8),
        }),
        6 => TermKind::Pivot {
            edge: *rng.pick(EdgeKind::all()),
            target: path(rng),
        },
        7 => TermKind::Reach(path(rng)),
        8 => TermKind::Callers(path(rng)),
        9 => TermKind::Name(rng.word(WORD, 1, 10)),
        // Quoted, so whitespace is part of a phrase rather than the end of it, and an
        // escaped quote is part of it too.
        _ => TermKind::Phrase(rng.word(VALUE, 0, 10)),
    }
}

/// A query of up to six terms, sometimes under a configuration.
fn query(rng: &mut Rng) -> Query {
    let configuration = rng.chance(3).then(|| rng.word(VALUE, 1, 8));
    let terms = (0..rng.below(7))
        .map(|_| super::Term {
            negated: rng.chance(3),
            kind: term_kind(rng),
            span: 0..0,
        })
        .collect();
    Query {
        terms,
        configuration,
    }
}

#[test]
fn every_generated_query_round_trips_through_its_text_form() -> TestResult {
    // The table above checks the forms someone thought to write down; this checks every
    // term kind, negated or not, in any order and combination, with multibyte values.
    let mut rng = Rng(0x5EA4_0529);
    for case in 0..4000 {
        let query = query(&mut rng);
        let rendered = query.to_string();
        let reparsed = parse(&rendered).map_err(|error| {
            format!(
                "case {case}: {rendered:?} did not parse back: {}",
                error.describe()
            )
        })?;

        assert_eq!(
            reparsed.configuration, query.configuration,
            "case {case}: {rendered:?}"
        );
        let written: Vec<_> = query.terms.iter().map(|t| (t.negated, &t.kind)).collect();
        let read: Vec<_> = reparsed
            .terms
            .iter()
            .map(|t| (t.negated, &t.kind))
            .collect();
        assert_eq!(read, written, "case {case}: {rendered:?}");

        // Each span covers exactly the text that term renders to, so the filter box's
        // caret lands under the term a reader would point at.
        for term in &reparsed.terms {
            assert_eq!(
                rendered.get(term.span.clone()),
                Some(term.to_string().as_str()),
                "case {case}: {rendered:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn every_span_parse_reports_lands_on_the_text_that_produced_it() -> TestResult {
    // Arbitrary text, valid or not: fragments of real syntax, stray quotes,
    // escapes and negations, ASCII and non-ASCII whitespace, and multibyte characters. Whatever
    // `parse` returns, a span must slice the input on character boundaries — the filter
    // box draws its caret from it, and a span inside a character panics that slice.
    const FRAGMENTS: &[&str] = &[
        "lens:", "api", "kind:", "type", "vis:", "public", "in:", "pkg", "::", "#2", "{", "}",
        "cfg:", "config:", "pivot:", "calls:", "reach:", "callers:", "hazard:", "x", ":", "!",
        "\"", "\\", " ", "  ", "\t", "\n", "\u{a0}", "\u{3000}", "é", "日本", "🦀",
    ];
    let mut rng = Rng(0x0529_5EA4);
    for case in 0..6000 {
        let text = rng.word(FRAGMENTS, 0, 14);
        match parse(&text) {
            Ok(query) => {
                let mut end = 0;
                for term in &query.terms {
                    assert!(
                        term.span.start >= end && term.span.start < term.span.end,
                        "case {case}: {text:?} has a span out of order: {:?}",
                        term.span
                    );
                    end = term.span.end;
                    let Some(source) = text.get(term.span.clone()) else {
                        return Err(format!("case {case}: {text:?} split a character").into());
                    };
                    // A term's own text means the same thing alone as it did in place.
                    let alone = parse(source)?;
                    let [only] = alone.terms.as_slice() else {
                        return Err(format!("case {case}: {source:?} is not one term").into());
                    };
                    assert_eq!(
                        (only.negated, &only.kind, only.span.clone()),
                        (term.negated, &term.kind, 0..source.len()),
                        "case {case}: {text:?}"
                    );
                }
            },
            Err(error) => assert!(
                text.get(error.span.clone())
                    .is_some_and(|source| !source.is_empty()),
                "case {case}: {text:?} reported an error at {:?}",
                error.span
            ),
        }
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
        resolution: None,
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

#[test]
fn every_compiled_in_language_contributes_its_own_vocabulary() {
    // A term is valid if *any* compiled-in language can emit it. A language whose
    // subtypes were left out of this union would have every one of its terms rejected as
    // a typo, with a suggestion from someone else's grammar.
    #[cfg(feature = "lang-rust")]
    assert!(parse("substitution:blanket-impl").is_ok());
    #[cfg(feature = "lang-python")]
    assert!(parse("substitution:protocol").is_ok());
    #[cfg(feature = "lang-javascript")]
    assert!(parse("substitution:implements").is_ok());
}
