//! Configurations: the named build-time variation a tree is read under.
//!
//! A package is not one tree. It is a family of trees indexed by feature set and build
//! target, and presenting the default one as "the package" is a correctness bug rather
//! than a simplification — the reader who asks "what's exposed here?" gets an answer that
//! is silently conditional on choices nobody showed them.
//!
//! So exactly one configuration is active at a time, it is always named, and everything
//! derived from it — membership, rollups — is attributed to that name. Test-only and
//! generated code are configurations too, not special cases.

use crate::index::SeamIndex;
use crate::model::ConfigMembership;
use crate::model::Lens;

pub mod predicate;

pub use predicate::CfgEnv;
pub use predicate::CfgError;
pub use predicate::CfgPredicate;
pub use predicate::Truth;

/// A named set of build-time choices to evaluate a package under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configuration {
    /// The display name, shown in the header and used by `config:` in a query.
    pub name: String,
    /// What is known about the compilation environment.
    pub env: CfgEnv,
    /// The build target this configuration builds, when it names one.
    pub target: Option<String>,
    /// Whether the manifest was available when this configuration was derived.
    ///
    /// Without it the feature set is a guess, so the `variation` lens cannot claim to be
    /// complete and the header says so.
    pub manifest_known: bool,
}

/// The name used when no manifest could be read.
///
/// A view still has to say *something* about what it is showing, and "unconfigured" is
/// the honest answer — better than borrowing the name `default` for a set of choices
/// nobody actually resolved.
pub const UNCONFIGURED: &str = "unconfigured";

impl Configuration {
    /// A configuration that knows only the host, for a package with no readable manifest.
    #[must_use]
    pub fn unconfigured() -> Self {
        Self {
            name: format!("{UNCONFIGURED} @ {}", host_triple()),
            env: host_env(),
            target: None,
            manifest_known: false,
        }
    }

    /// A named configuration over an explicit feature set.
    #[must_use]
    pub fn named(name: impl Into<String>, features: impl IntoIterator<Item = String>) -> Self {
        Self {
            name: format!("{} @ {}", name.into(), host_triple()),
            env: host_env().with_features(features),
            target: None,
            manifest_known: true,
        }
    }

    /// This configuration built for a named target, which decides `test` and friends.
    #[must_use]
    pub fn for_target(mut self, target: &str) -> Self {
        if matches!(target, "test" | "bench") {
            self.env.flags.insert("test".to_owned());
        }
        self.target = Some(target.to_owned());
        self
    }

    /// Whether the `variation` lens can claim completeness under this configuration.
    #[must_use]
    pub fn variation_is_complete(&self) -> bool {
        self.manifest_known
    }
}

/// The host target triple, as reported by the compiler that built this crate.
///
/// Target-keyed `cfg`s are evaluated against the host rather than guessed, and the triple
/// is part of the configuration's name so the reader can see which one they are seeing.
#[must_use]
pub fn host_triple() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

/// What is known about the host, with the standard keys marked fully enumerated.
#[must_use]
pub fn host_env() -> CfgEnv {
    let mut flags = Vec::new();
    if cfg!(unix) {
        flags.push("unix".to_owned());
    }
    if cfg!(windows) {
        flags.push("windows".to_owned());
    }
    CfgEnv::new()
        .with_key("target_os", std::env::consts::OS)
        .with_key("target_arch", std::env::consts::ARCH)
        .with_key("target_family", if cfg!(unix) { "unix" } else { "windows" })
        .with_flags(flags)
        // These are the flags the index can actually decide; anything else stays unknown.
        .with_known_flags(
            [
                "unix",
                "windows",
                "test",
                "doc",
                "doctest",
                "debug_assertions",
            ]
            .into_iter()
            .map(str::to_owned),
        )
}

/// Evaluate every node's gates under `configuration`, then recompute rollups.
///
/// Membership is three-valued and inherited: a node inside an inactive module is itself
/// inactive, and a node whose own gate is undecidable is indeterminate unless an ancestor
/// already settled it as inactive.
pub fn apply(index: &mut SeamIndex, configuration: &Configuration) {
    for root in index.roots().to_vec() {
        apply_subtree(index, root, ConfigMembership::Active, configuration);
    }
    index.recompute_rollups();
}

/// Evaluate one node, then its children under the membership it inherits.
fn apply_subtree(
    index: &mut SeamIndex,
    id: crate::id::SeamId,
    inherited: ConfigMembership,
    configuration: &Configuration,
) {
    let membership = resolve_membership(index, id, inherited, configuration);
    if let Some(node) = index.node_mut(id) {
        node.membership = membership;
    }
    for child in index.children(id).to_vec() {
        apply_subtree(index, child, membership, configuration);
    }
}

/// Combine a node's own gates with what it inherits from its ancestors.
fn resolve_membership(
    index: &SeamIndex,
    id: crate::id::SeamId,
    inherited: ConfigMembership,
    configuration: &Configuration,
) -> ConfigMembership {
    // An excluded ancestor excludes everything beneath it, whatever its own gates say.
    if inherited == ConfigMembership::Inactive {
        return ConfigMembership::Inactive;
    }
    let Some(node) = index.node(id) else {
        return inherited;
    };
    let verdict = Truth::all(
        node.facets_for(Lens::Variation)
            .filter(|facet| facet.subtype.name() == "cfg")
            .filter_map(|facet| facet.detail.as_deref())
            .map(|text| match predicate::parse(text) {
                Ok(parsed) => configuration.env.eval(&parsed),
                // A predicate this parser cannot read is not a predicate that is false.
                Err(_) => Truth::Unknown,
            }),
    );
    match verdict {
        Truth::True => inherited,
        Truth::False => ConfigMembership::Inactive,
        // An undecidable gate under an indeterminate ancestor stays indeterminate.
        Truth::Unknown => ConfigMembership::Indeterminate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::SeamPath;
    use crate::model::Facet;
    use crate::model::FacetSubtype;
    use crate::model::FileId;
    use crate::model::Node;
    use crate::model::NodeKind;
    use crate::model::SeamLocation;
    use crate::rollup::Rollups;

    fn add(index: &mut SeamIndex, path: &str, gate: Option<&str>) -> crate::id::SeamId {
        let parsed: SeamPath = path.parse().unwrap_or_default();
        let parent = parsed.parent().and_then(|p| index.resolve(&p));
        let id = index.intern(parsed.clone());
        let mut facets = vec![Facet::new(Lens::Api, FacetSubtype("pub"))];
        if let Some(gate) = gate {
            facets.push(
                Facet::new(Lens::Variation, FacetSubtype("cfg")).with_detail(gate.to_owned()),
            );
        }
        index.insert(Node {
            id,
            kind: NodeKind::Module,
            name: parsed.leaf().unwrap_or_default().to_owned(),
            detail: None,
            location: SeamLocation {
                file: FileId(0),
                range: karet_core::Range::default(),
                span: karet_core::Span::default(),
                selection: karet_core::Range::default(),
            },
            parent,
            children: Vec::new(),
            facets,
            visibility: None,
            rollups: Rollups::new(),
            membership: ConfigMembership::Active,
            provisional: false,
        });
        id
    }

    fn membership(index: &SeamIndex, path: &str) -> Option<ConfigMembership> {
        let parsed: SeamPath = path.parse().ok()?;
        index
            .resolve(&parsed)
            .and_then(|id| index.node(id))
            .map(|node| node.membership)
    }

    #[test]
    fn an_unconfigured_view_names_itself_honestly() {
        let configuration = Configuration::unconfigured();
        assert!(configuration.name.starts_with(UNCONFIGURED));
        assert!(!configuration.variation_is_complete());
    }

    #[test]
    fn a_named_configuration_carries_the_host_triple() {
        let configuration = Configuration::named("default", ["view".to_owned()]);
        assert!(configuration.name.starts_with("default @ "));
        assert!(configuration.variation_is_complete());
    }

    #[test]
    fn a_gate_that_holds_leaves_a_node_active() {
        let mut index = SeamIndex::new();
        add(&mut index, "pkg", None);
        add(&mut index, "pkg::viewer", Some("feature = \"view\""));
        apply(
            &mut index,
            &Configuration::named("view", ["view".to_owned()]),
        );
        assert_eq!(
            membership(&index, "pkg::viewer"),
            Some(ConfigMembership::Active)
        );
    }

    #[test]
    fn a_gate_that_fails_marks_a_node_inactive_but_keeps_it() {
        let mut index = SeamIndex::new();
        add(&mut index, "pkg", None);
        add(&mut index, "pkg::viewer", Some("feature = \"view\""));
        apply(&mut index, &Configuration::named("bare", []));
        // Present-but-inactive, never removed: hiding is the user's choice, not ours.
        assert_eq!(
            membership(&index, "pkg::viewer"),
            Some(ConfigMembership::Inactive)
        );
        assert!(
            index
                .resolve(&"pkg::viewer".parse().unwrap_or_default())
                .is_some()
        );
    }

    #[test]
    fn an_undecidable_gate_is_indeterminate_rather_than_excluded() {
        let mut index = SeamIndex::new();
        add(&mut index, "pkg", None);
        add(&mut index, "pkg::vendor", Some("some_vendor_key"));
        apply(&mut index, &Configuration::named("default", []));
        assert_eq!(
            membership(&index, "pkg::vendor"),
            Some(ConfigMembership::Indeterminate)
        );
    }

    #[test]
    fn an_unparseable_gate_is_indeterminate_not_false() {
        let mut index = SeamIndex::new();
        add(&mut index, "pkg", None);
        add(&mut index, "pkg::weird", Some("!!! not a predicate"));
        apply(&mut index, &Configuration::named("default", []));
        assert_eq!(
            membership(&index, "pkg::weird"),
            Some(ConfigMembership::Indeterminate)
        );
    }

    #[test]
    fn exclusion_is_inherited_by_everything_beneath() {
        let mut index = SeamIndex::new();
        add(&mut index, "pkg", None);
        add(&mut index, "pkg::gated", Some("feature = \"view\""));
        add(&mut index, "pkg::gated::inner", None);
        apply(&mut index, &Configuration::named("bare", []));
        assert_eq!(
            membership(&index, "pkg::gated::inner"),
            Some(ConfigMembership::Inactive),
            "a child of an excluded module is excluded"
        );
    }

    #[test]
    fn rollups_are_recomputed_under_the_active_configuration() {
        let mut index = SeamIndex::new();
        let pkg = add(&mut index, "pkg", None);
        add(&mut index, "pkg::gated", Some("feature = \"view\""));

        apply(
            &mut index,
            &Configuration::named("view", ["view".to_owned()]),
        );
        let with_feature = index.node(pkg).map(|n| n.rollups.get(Lens::Api));

        apply(&mut index, &Configuration::named("bare", []));
        let without = index.node(pkg).map(|n| n.rollups.get(Lens::Api));

        assert!(
            with_feature > without,
            "counts must follow the configuration: {with_feature:?} vs {without:?}"
        );
    }

    #[test]
    fn a_test_target_turns_on_the_test_flag() {
        let mut index = SeamIndex::new();
        add(&mut index, "pkg", None);
        add(&mut index, "pkg::suite", Some("test"));

        apply(&mut index, &Configuration::named("lib", []));
        assert_eq!(
            membership(&index, "pkg::suite"),
            Some(ConfigMembership::Inactive)
        );

        apply(
            &mut index,
            &Configuration::named("tests", []).for_target("test"),
        );
        assert_eq!(
            membership(&index, "pkg::suite"),
            Some(ConfigMembership::Active)
        );
    }
}
