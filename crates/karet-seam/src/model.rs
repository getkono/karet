//! The neutral seam model: lenses, node kinds, facets, and the nodes that carry them.
//!
//! Every type here is language-neutral. A language contributes a *mapping* into this
//! vocabulary; it never extends it. The five lenses in particular are a closed set — the
//! whole value of the taxonomy is that "what can be swapped" means the same thing whether
//! you are reading Rust or Python.

use karet_core::Range;
use karet_core::Span;

use crate::id::SeamId;

/// One of the five seam classes. This set is closed.
///
/// A sixth lens would mean the taxonomy failed to describe something; the correct
/// response is a new *subtype* under an existing lens, which languages may add freely.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum Lens {
    /// What is visible from outside?
    Api,
    /// What behavior can be swapped?
    Substitution,
    /// What changes shape before compiling?
    Variation,
    /// What crosses the package line?
    Boundary,
    /// Where is substitution dangerous?
    Hazard,
}

/// Every lens, in display order. Indexing this array by [`Lens::index`] round-trips.
pub const LENSES: [Lens; 5] = [
    Lens::Api,
    Lens::Substitution,
    Lens::Variation,
    Lens::Boundary,
    Lens::Hazard,
];

impl Lens {
    /// The lens's stable name, as written in a query (`lens:substitution`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Substitution => "substitution",
            Self::Variation => "variation",
            Self::Boundary => "boundary",
            Self::Hazard => "hazard",
        }
    }

    /// The question this lens answers, for the legend and the facet pane.
    #[must_use]
    pub fn question(self) -> &'static str {
        match self {
            Self::Api => "What is visible from outside?",
            Self::Substitution => "What behavior can be swapped?",
            Self::Variation => "What changes shape before compiling?",
            Self::Boundary => "What crosses the package line?",
            Self::Hazard => "Where is substitution dangerous?",
        }
    }

    /// Resolve a lens from its query name, case-sensitively.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        LENSES.into_iter().find(|lens| lens.name() == name)
    }

    /// This lens's position in [`LENSES`], for indexing per-lens arrays.
    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::Api => 0,
            Self::Substitution => 1,
            Self::Variation => 2,
            Self::Boundary => 3,
            Self::Hazard => 4,
        }
    }
}

/// The universal node kinds every language maps onto.
///
/// A construct with no sensible mapping becomes [`NodeKind::Other`] rather than forcing
/// a new kind — the spine renders it, and its facets still classify it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[non_exhaustive]
pub enum NodeKind {
    /// A package or crate: the root of a containment tree.
    Package,
    /// A module or namespace.
    Module,
    /// A concrete data type — Rust's `struct`, `enum`, `union`, and type aliases.
    Type,
    /// An abstract contract — Rust's `trait`.
    Interface,
    /// A binding of a contract to a type, or a bare inherent block.
    Implementation,
    /// A free function.
    Function,
    /// An item belonging to a type or implementation: method, field, variant, associated type.
    Member,
    /// A constant or static.
    Constant,
    /// A macro definition.
    MacroDef,
    /// A foreign-interface block.
    ForeignBlock,
    /// Anything the language could not map onto a kind above.
    Other,
}

impl NodeKind {
    /// The kind's stable name, as written in a query (`kind:interface`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Package => "package",
            Self::Module => "module",
            Self::Type => "type",
            Self::Interface => "interface",
            Self::Implementation => "implementation",
            Self::Function => "function",
            Self::Member => "member",
            Self::Constant => "constant",
            Self::MacroDef => "macro-def",
            Self::ForeignBlock => "foreign-block",
            Self::Other => "other",
        }
    }

    /// Every kind, for query-term suggestions and legends.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[
            Self::Package,
            Self::Module,
            Self::Type,
            Self::Interface,
            Self::Implementation,
            Self::Function,
            Self::Member,
            Self::Constant,
            Self::MacroDef,
            Self::ForeignBlock,
            Self::Other,
        ]
    }

    /// Resolve a kind from its query name, case-sensitively.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::all().iter().copied().find(|kind| kind.name() == name)
    }
}

/// How far a node is visible, ordered from least to most reachable.
///
/// The ordering is meaningful: `vis:crate` in a query matches everything at least as
/// reachable as crate-visible.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[non_exhaustive]
pub enum Visibility {
    /// Visible only within its own module.
    Private,
    /// Visible to the parent module — Rust's `pub(super)`.
    Super,
    /// Visible within an explicitly named subtree — Rust's `pub(in path)`.
    Restricted,
    /// Visible throughout the package — Rust's `pub(crate)`.
    Crate,
    /// Visible outside the package.
    Public,
}

impl Visibility {
    /// The stable name, as written in a query (`vis:crate`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Super => "super",
            Self::Restricted => "restricted",
            Self::Crate => "crate",
            Self::Public => "public",
        }
    }

    /// Every level, least reachable first.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[
            Self::Private,
            Self::Super,
            Self::Restricted,
            Self::Crate,
            Self::Public,
        ]
    }

    /// Resolve a level from its query name, case-sensitively.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::all().iter().copied().find(|vis| vis.name() == name)
    }
}

/// A facet's subtype within its lens — `dyn`, `unsafe`, `cfg`, and so on.
///
/// Lens membership is fixed, but subtypes are open: a language names its own using
/// `&'static str` constants, and the query language matches them by that name. Keeping
/// this `Copy` means facet sets stay cheap to filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct FacetSubtype(pub &'static str);

impl FacetSubtype {
    /// The subtype's name, as written in a query (`substitution:dyn`).
    #[must_use]
    pub fn name(self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for FacetSubtype {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// How far a node is *actually* reachable, once re-exports are accounted for.
///
/// Kept as a modifier on the declared visibility rather than a facet of its own, because
/// the two interesting states are both comparisons: declared public but unreachable, and
/// declared private but re-exported into the public API.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Effective {
    /// The reach after following re-export chains.
    pub reach: Visibility,
    /// The re-export that produced that reach, when one did.
    pub via: Option<SeamId>,
    /// The path the node is reachable at from outside, when it differs from its own.
    pub public_path: Option<String>,
}

/// A seam property attached to a node. Every facet belongs to exactly one lens.
///
/// A facet is present or absent — there is no severity and no score. Ranking seams would
/// mean deciding for the reader which ones matter, which is exactly the judgement the
/// view exists to support rather than replace.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Facet {
    /// Which lens this facet belongs to.
    pub lens: Lens,
    /// The subtype within that lens.
    pub subtype: FacetSubtype,
    /// Supporting text — the `cfg` predicate, the linked symbol name, the trait bound.
    pub detail: Option<String>,
    /// Sub-item occurrences inside this node, when the facet describes several.
    ///
    /// This is what keeps containment a tree: three `unsafe` blocks in one function are
    /// three sites on the function's facet, not three nodes.
    pub sites: Vec<Range>,
    /// Effective reach, on `api` facets only.
    pub effective: Option<Effective>,
}

impl Facet {
    /// A bare facet with no detail and no sites.
    #[must_use]
    pub fn new(lens: Lens, subtype: FacetSubtype) -> Self {
        Self {
            lens,
            subtype,
            detail: None,
            sites: Vec::new(),
            effective: None,
        }
    }

    /// This facet with supporting text attached.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// This facet with sub-item occurrence sites attached.
    #[must_use]
    pub fn with_sites(mut self, sites: Vec<Range>) -> Self {
        self.sites = sites;
        self
    }

    /// This facet with an effective-reach modifier attached.
    #[must_use]
    pub fn with_effective(mut self, effective: Effective) -> Self {
        self.effective = Some(effective);
        self
    }

    /// How many occurrences this facet stands for: its site count, or `1` when it
    /// describes the node itself.
    #[must_use]
    pub fn occurrences(&self) -> usize {
        self.sites.len().max(1)
    }
}

/// Whether a node is part of the tree under the active configuration.
///
/// Three states, not two. A node whose gate could not be evaluated is neither in nor
/// out, and saying so is the difference between "this is absent" and "I do not know" —
/// the distinction the whole view is built to preserve.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum ConfigMembership {
    /// Compiled under the active configuration.
    #[default]
    Active,
    /// Excluded by the active configuration; rendered present-but-inactive.
    Inactive,
    /// Gated by a predicate this index cannot evaluate.
    Indeterminate,
}

impl ConfigMembership {
    /// The stable name, for the header and for serialized output.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
            Self::Indeterminate => "indeterminate",
        }
    }
}

/// An index-local handle for a source file, keeping node locations small.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct FileId(pub u32);

/// Where a node lives in the source.
///
/// Byte span and line/column range are both kept: the span drives incremental re-index
/// splicing, the range drives navigation. Neither participates in identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct SeamLocation {
    /// Which file, resolved through the index's file table.
    pub file: FileId,
    /// The node's full extent, for navigation.
    pub range: Range,
    /// The node's byte extent, for splicing.
    pub span: Span,
    /// The range to reveal when jumping here — usually just the name.
    pub selection: Range,
    /// The declaration head: everything up to the body the construct opens.
    ///
    /// This is the part a reader needs in order to decide anything — a signature with
    /// its parameters, a `struct` line with its generics, an `impl` with what it binds.
    /// It is a *range*, not a line count, because a signature is as tall as it is: three
    /// lines for a wrapped parameter list, one for `fn now() -> Instant`. A construct
    /// that opens no body has none, and this is then its whole extent.
    pub header: Range,
}

/// One node in the containment tree.
///
/// Containment is a tree and everything else is an edge; a node therefore has exactly
/// one parent and never points sideways. Relations that are not containment live in
/// [`crate::edge`].
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Node {
    /// This node's interned identity.
    pub id: SeamId,
    /// Which universal kind it maps onto.
    pub kind: NodeKind,
    /// The display name, without path or disambiguator.
    pub name: String,
    /// A short signature or descriptor, when the language offers one.
    pub detail: Option<String>,
    /// Where it lives in the source.
    pub location: SeamLocation,
    /// Its parent, or `None` at a tree root.
    pub parent: Option<SeamId>,
    /// Its children, in source order.
    pub children: Vec<SeamId>,
    /// The seam properties it carries.
    pub facets: Vec<Facet>,
    /// Its declared visibility, when the language expresses one.
    pub visibility: Option<Visibility>,
    /// Per-lens counts over this node's whole subtree, under the active configuration.
    pub rollups: crate::rollup::Rollups,
    /// Whether the active configuration includes it.
    pub membership: ConfigMembership,
    /// Whether the subtree this node came from failed to parse cleanly.
    ///
    /// Structural facts from a damaged subtree are provisional, not absent — the
    /// distinction the view must render.
    pub provisional: bool,
}

impl Node {
    /// Every facet belonging to `lens`.
    pub fn facets_for(&self, lens: Lens) -> impl Iterator<Item = &Facet> {
        self.facets.iter().filter(move |facet| facet.lens == lens)
    }

    /// Whether the node carries any facet of `lens`.
    #[must_use]
    pub fn has_lens(&self, lens: Lens) -> bool {
        self.facets.iter().any(|facet| facet.lens == lens)
    }

    /// Whether the node carries a facet with this exact subtype.
    #[must_use]
    pub fn has_subtype(&self, lens: Lens, subtype: &str) -> bool {
        self.facets
            .iter()
            .any(|facet| facet.lens == lens && facet.subtype.name() == subtype)
    }

    /// The node's effective reach, falling back to its declared visibility.
    #[must_use]
    pub fn effective_visibility(&self) -> Option<Visibility> {
        self.facets_for(Lens::Api)
            .find_map(|facet| facet.effective.as_ref().map(|e| e.reach))
            .or(self.visibility)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lens_names_round_trip_and_index_matches_the_table() {
        for lens in LENSES {
            assert_eq!(Lens::from_name(lens.name()), Some(lens));
            assert_eq!(LENSES[lens.index()], lens);
            assert!(!lens.question().is_empty());
        }
        assert_eq!(Lens::from_name("nope"), None);
        assert_eq!(Lens::from_name("API"), None, "names are case-sensitive");
    }

    #[test]
    fn the_lens_set_is_closed_at_five() {
        assert_eq!(LENSES.len(), 5);
    }

    #[test]
    fn node_kind_names_round_trip() {
        for kind in NodeKind::all() {
            assert_eq!(NodeKind::from_name(kind.name()), Some(*kind));
        }
        assert_eq!(NodeKind::from_name("klass"), None);
    }

    #[test]
    fn visibility_orders_from_least_to_most_reachable() {
        assert!(Visibility::Private < Visibility::Crate);
        assert!(Visibility::Crate < Visibility::Public);
        assert!(Visibility::Super < Visibility::Crate);
        for vis in Visibility::all() {
            assert_eq!(Visibility::from_name(vis.name()), Some(*vis));
        }
    }

    #[test]
    fn a_facet_builds_up_from_its_bare_form() {
        let subtype = FacetSubtype("dyn");
        let facet = Facet::new(Lens::Substitution, subtype).with_detail("Box<dyn Read>");
        assert_eq!(facet.lens, Lens::Substitution);
        assert_eq!(facet.subtype.name(), "dyn");
        assert_eq!(facet.detail.as_deref(), Some("Box<dyn Read>"));
        assert!(facet.sites.is_empty());
        assert_eq!(facet.subtype.to_string(), "dyn");
    }

    #[test]
    fn occurrences_counts_sites_but_never_reports_zero() {
        let bare = Facet::new(Lens::Hazard, FacetSubtype("unsafe"));
        assert_eq!(
            bare.occurrences(),
            1,
            "a facet on the node itself is one occurrence"
        );
        let with_sites = Facet::new(Lens::Hazard, FacetSubtype("unsafe")).with_sites(vec![
            Range::default(),
            Range::default(),
            Range::default(),
        ]);
        assert_eq!(with_sites.occurrences(), 3);
    }

    fn node_with(facets: Vec<Facet>, visibility: Option<Visibility>) -> Node {
        Node {
            id: SeamId(0),
            kind: NodeKind::Function,
            name: "f".to_owned(),
            detail: None,
            location: SeamLocation {
                file: FileId(0),
                range: Range::default(),
                span: Span::default(),
                selection: Range::default(),
                header: Range::default(),
            },
            parent: None,
            children: Vec::new(),
            facets,
            visibility,
            rollups: crate::rollup::Rollups::default(),
            membership: ConfigMembership::Active,
            provisional: false,
        }
    }

    #[test]
    fn facet_lookups_filter_by_lens_and_subtype() {
        let node = node_with(
            vec![
                Facet::new(Lens::Api, FacetSubtype("pub")),
                Facet::new(Lens::Hazard, FacetSubtype("unsafe")),
                Facet::new(Lens::Hazard, FacetSubtype("async")),
            ],
            Some(Visibility::Public),
        );
        assert_eq!(node.facets_for(Lens::Hazard).count(), 2);
        assert!(node.has_lens(Lens::Api));
        assert!(!node.has_lens(Lens::Variation));
        assert!(node.has_subtype(Lens::Hazard, "unsafe"));
        assert!(
            !node.has_subtype(Lens::Api, "unsafe"),
            "subtype must match within its lens"
        );
    }

    #[test]
    fn effective_reach_wins_over_the_declared_visibility() {
        let reexported = Facet::new(Lens::Api, FacetSubtype("private")).with_effective(Effective {
            reach: Visibility::Public,
            via: Some(SeamId(7)),
            public_path: Some("pkg::Thing".to_owned()),
        });
        let node = node_with(vec![reexported], Some(Visibility::Private));
        assert_eq!(node.effective_visibility(), Some(Visibility::Public));

        let plain = node_with(vec![], Some(Visibility::Crate));
        assert_eq!(plain.effective_visibility(), Some(Visibility::Crate));
    }

    #[test]
    fn membership_defaults_to_active_and_names_all_three_states() {
        assert_eq!(ConfigMembership::default(), ConfigMembership::Active);
        assert_eq!(ConfigMembership::Active.name(), "active");
        assert_eq!(ConfigMembership::Inactive.name(), "inactive");
        assert_eq!(ConfigMembership::Indeterminate.name(), "indeterminate");
    }
}
