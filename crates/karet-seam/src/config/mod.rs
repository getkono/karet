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

use std::collections::HashMap;
use std::path::Path;

use crate::id::SeamId;
use crate::index::SeamIndex;
use crate::model::ConfigMembership;
use crate::model::FileId;
use crate::model::Lens;

mod predicate;

pub use predicate::CfgEnv;
pub use predicate::CfgError;
pub use predicate::CfgPredicate;
pub use predicate::Truth;
pub use predicate::parse as parse_predicate;

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
    ///
    /// The target is folded into the name (`unconfigured @ <triple>` becomes
    /// `unconfigured:test @ <triple>`), so a configuration and its per-target variants
    /// stay distinct and each can be selected by name.
    #[must_use]
    pub fn for_target(mut self, target: &str) -> Self {
        if matches!(target, "test" | "bench") {
            self.env.flags.insert("test".to_owned());
        }
        self.name = match self.name.rsplit_once(" @ ") {
            Some((base, triple)) => format!("{base}:{target} @ {triple}"),
            None => format!("{}:{target}", self.name),
        };
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
/// already settled it as inactive. Outside Rust, a file that is [test code by
/// convention](is_test_path) is a gate too: inactive unless the configuration builds tests.
pub fn apply(index: &mut SeamIndex, configuration: &Configuration) {
    for (id, membership) in evaluate(index, configuration) {
        if let Some(node) = index.node_mut(id) {
            node.membership = membership;
        }
    }
    index.recompute_rollups();
}

/// Every node's membership under `configuration`, without changing the index.
///
/// What [`apply`] writes, as a map — so something measuring under one configuration (the
/// call graph excluding tests, say) does not have to switch the configuration the view
/// is showing to find out.
#[must_use]
pub fn evaluate(
    index: &SeamIndex,
    configuration: &Configuration,
) -> HashMap<SeamId, ConfigMembership> {
    let mut out = HashMap::with_capacity(index.len());
    let mut test_files: HashMap<FileId, bool> = HashMap::new();
    let mut stack: Vec<(SeamId, ConfigMembership, Option<&Path>)> = index
        .roots()
        .iter()
        .rev()
        .map(|root| {
            // A package node sits in its manifest, so its directory is the package root
            // every test path is read relative to.
            let package = index
                .node(*root)
                .and_then(|node| index.file_path(node.location.file))
                .and_then(Path::parent);
            (*root, ConfigMembership::Active, package)
        })
        .collect();
    while let Some((id, inherited, package)) = stack.pop() {
        let membership = resolve_membership(index, id, inherited, configuration, &mut |file| {
            // Outside its package root there is no relative path to read a convention
            // from, and an absolute one would let `/home/test/…` make everything a test.
            *test_files.entry(file).or_insert_with(|| {
                index
                    .file_path(file)
                    .zip(package)
                    .and_then(|(path, root)| path.strip_prefix(root).ok())
                    .is_some_and(is_test_path)
            })
        });
        out.insert(id, membership);
        stack.extend(
            index
                .children(id)
                .iter()
                .rev()
                .map(|child| (*child, membership, package)),
        );
    }
    out
}

/// Whether a file is test code by where it sits and what it is called.
///
/// The languages without a `cfg(test)` mark their tests by convention instead, and a
/// convention is as decidable as a gate: `tests/`, `__tests__/`, `test_x.py`,
/// `x.spec.ts`, `XTest.kt`. Rust is left to its gates — a Rust file is test code exactly
/// when a `cfg(test)` says so, and a `tests.rs` module is already gated that way.
///
/// `path` should be relative to the package: a checkout that happens to live under a
/// directory called `test` is not test code.
#[must_use]
pub fn is_test_path(path: &Path) -> bool {
    if path.extension().is_some_and(|extension| extension == "rs") {
        return false;
    }
    let in_test_dir = path.parent().is_some_and(|parent| {
        parent.components().any(|component| {
            matches!(
                component.as_os_str().to_str(),
                Some("tests" | "test" | "Tests" | "__tests__")
            )
        })
    });
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return in_test_dir;
    };
    // `FooTest` is a convention of the class-per-file languages only; elsewhere a name
    // ending in `Test` is just a name.
    let class_per_file = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e, "java" | "kt" | "kts" | "swift" | "scala"));
    let by_name = stem.starts_with("test_")
        || stem.ends_with("_test")
        || stem.ends_with("_tests")
        || stem.ends_with(".test")
        || stem.ends_with(".spec")
        || (class_per_file && (stem.ends_with("Test") || stem.ends_with("Tests")))
        || stem == "conftest";
    in_test_dir || by_name
}

/// Combine a node's own gates with what it inherits from its ancestors.
fn resolve_membership(
    index: &SeamIndex,
    id: SeamId,
    inherited: ConfigMembership,
    configuration: &Configuration,
    is_test_file: &mut dyn FnMut(FileId) -> bool,
) -> ConfigMembership {
    // An excluded ancestor excludes everything beneath it, whatever its own gates say.
    if inherited == ConfigMembership::Inactive {
        return ConfigMembership::Inactive;
    }
    let Some(node) = index.node(id) else {
        return inherited;
    };
    // A package node sits in its manifest, and test code is never a whole package.
    if !configuration.env.flags.contains("test")
        && node.kind != crate::model::NodeKind::Package
        && is_test_file(node.location.file)
    {
        return ConfigMembership::Inactive;
    }
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
                header: karet_core::Range::default(),
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
    fn a_target_is_folded_into_the_name_before_the_triple() {
        let base = Configuration::unconfigured();
        let testing = Configuration::unconfigured().for_target("test");
        assert_eq!(
            testing.name,
            format!("{UNCONFIGURED}:test @ {}", host_triple())
        );
        assert_ne!(
            testing.name, base.name,
            "a target must not share its base's name"
        );
        assert_eq!(testing.target.as_deref(), Some("test"));
    }

    #[test]
    fn a_target_on_a_name_with_no_triple_is_appended() {
        let mut configuration = Configuration::named("default", []);
        configuration.name = "bare".to_owned();
        assert_eq!(configuration.for_target("bench").name, "bare:bench");
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

    #[test]
    fn evaluating_reports_what_apply_would_write_without_writing_it() {
        let mut index = SeamIndex::new();
        add(&mut index, "pkg", None);
        let suite = add(&mut index, "pkg::suite", Some("test"));
        let inner = add(&mut index, "pkg::suite::inner", None);

        let found = evaluate(&index, &Configuration::named("lib", []));
        assert_eq!(found.get(&suite), Some(&ConfigMembership::Inactive));
        assert_eq!(found.get(&inner), Some(&ConfigMembership::Inactive));
        // Read-only: the view's own membership is untouched.
        assert_eq!(
            membership(&index, "pkg::suite"),
            Some(ConfigMembership::Active)
        );
    }

    #[test]
    fn test_code_is_recognized_by_convention_outside_rust() {
        for path in [
            "tests/test_core.py",
            "app/test_core.py",
            "app/core_test.go",
            "src/__tests__/widget.ts",
            "src/widget.spec.ts",
            "src/widget.test.tsx",
            "src/test/kotlin/WidgetTest.kt",
            "Tests/AppTests/WidgetTests.swift",
            "conftest.py",
        ] {
            assert!(is_test_path(Path::new(path)), "{path}");
        }
        for path in [
            "src/widget.ts",
            "app/contest.py",
            "mylib/testing/fixtures.py",
            "com/acme/spec/Parser.kt",
            "src/abTest.ts",
            "src/main/kotlin/Widget.kt",
            "Sources/App/Testing.swift",
            // Rust test code is whatever `cfg(test)` says it is, never a file name.
            "src/tests.rs",
            "tests/integration.rs",
        ] {
            assert!(!is_test_path(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn test_files_are_read_relative_to_their_package() {
        // A checkout under a directory called `test` is not test code.
        let mut index = SeamIndex::new();
        let manifest = index.intern_file(Path::new("/home/test/web/package.json"));
        let source = index.intern_file(Path::new("/home/test/web/src/widget.ts"));
        let spec = index.intern_file(Path::new("/home/test/web/src/widget.spec.ts"));
        let elsewhere = index.intern_file(Path::new("/srv/tests/shared.spec.ts"));
        let package = add(&mut index, "web", None);
        let plain = add(&mut index, "web::widget", None);
        let tested = add(&mut index, "web::widget_spec", None);
        // Outside the package there is no convention to read, so nothing is assumed.
        let outside = add(&mut index, "web::shared", None);
        for (id, file) in [
            (package, manifest),
            (plain, source),
            (tested, spec),
            (outside, elsewhere),
        ] {
            if let Some(node) = index.node_mut(id) {
                node.location.file = file;
            }
        }
        if let Some(node) = index.node_mut(package) {
            node.kind = NodeKind::Package;
        }

        let found = evaluate(&index, &Configuration::unconfigured());
        assert_eq!(found.get(&plain), Some(&ConfigMembership::Active));
        assert_eq!(found.get(&tested), Some(&ConfigMembership::Inactive));
        assert_eq!(found.get(&outside), Some(&ConfigMembership::Active));
        let testing = evaluate(&index, &Configuration::unconfigured().for_target("test"));
        assert_eq!(testing.get(&tested), Some(&ConfigMembership::Active));
    }
}
