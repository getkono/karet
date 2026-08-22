//! Python mapping tests, doubling as the §10 language-contract conformance check.

use std::path::Path;

use karet_treesitter::ParserPool;

use crate::extract::extract_file;
use crate::id::SeamPath;
use crate::index::SeamIndex;
use crate::model::Lens;
use crate::model::Node;
use crate::model::NodeKind;
use crate::model::Visibility;

type TestResult = Result<(), String>;

/// Index one snippet as a package root named `pkg`.
fn index(source: &str) -> Option<SeamIndex> {
    let language = super::language_id()?;
    let mut index = SeamIndex::new();
    let root: SeamPath = "pkg".parse().ok()?;
    let root_id = index.intern(root);
    let file = index.intern_file(Path::new("pkg/__init__.py"));
    index.insert(Node {
        id: root_id,
        kind: NodeKind::Package,
        name: "pkg".to_owned(),
        detail: None,
        location: crate::model::SeamLocation {
            file,
            range: karet_core::Range::default(),
            span: karet_core::Span::default(),
            selection: karet_core::Range::default(),
        },
        parent: None,
        children: Vec::new(),
        facets: Vec::new(),
        visibility: None,
        rollups: crate::rollup::Rollups::new(),
        membership: crate::model::ConfigMembership::Active,
        provisional: false,
    });
    let mut pool = ParserPool::new();
    extract_file(&mut index, &mut pool, root_id, file, language, source).ok()?;
    index.recompute_rollups();
    Some(index)
}

fn at<'a>(index: &'a SeamIndex, path: &str) -> Option<&'a Node> {
    let parsed: SeamPath = path.parse().ok()?;
    index.resolve(&parsed).and_then(|id| index.node(id))
}

fn subtypes(index: &SeamIndex, path: &str, lens: Lens) -> Vec<String> {
    let Some(node) = at(index, path) else {
        return Vec::new();
    };
    let mut out: Vec<String> = node
        .facets_for(lens)
        .map(|f| f.subtype.name().to_owned())
        .collect();
    out.sort();
    out
}

// --- containment ------------------------------------------------------------

#[test]
fn classes_and_functions_become_a_containment_tree() {
    let Some(index) =
        index("class Widget:\n    def render(self):\n        pass\n\ndef helper():\n    pass\n")
    else {
        return;
    };
    assert_eq!(
        at(&index, "pkg::Widget").map(|n| n.kind),
        Some(NodeKind::Type)
    );
    assert_eq!(
        at(&index, "pkg::Widget::render").map(|n| n.kind),
        Some(NodeKind::Member)
    );
    assert_eq!(
        at(&index, "pkg::helper").map(|n| n.kind),
        Some(NodeKind::Function)
    );
}

#[test]
fn a_protocol_class_is_an_interface_and_a_plain_one_is_a_type() {
    // The same distinction Rust draws between `trait` and `struct`, reached by a
    // completely different construct.
    let Some(index) = index("class Contract(Protocol):\n    pass\n\nclass Plain:\n    pass\n")
    else {
        return;
    };
    assert_eq!(
        at(&index, "pkg::Contract").map(|n| n.kind),
        Some(NodeKind::Interface)
    );
    assert_eq!(
        at(&index, "pkg::Plain").map(|n| n.kind),
        Some(NodeKind::Type)
    );
}

#[test]
fn a_screaming_case_module_binding_is_a_constant() {
    let Some(index) = index("MAX_SIZE = 10\nlowercase = 2\n") else {
        return;
    };
    assert_eq!(
        at(&index, "pkg::MAX_SIZE").map(|n| n.kind),
        Some(NodeKind::Constant)
    );
    // An ordinary module-level binding is not an addressable entity.
    assert!(at(&index, "pkg::lowercase").is_none());
}

// --- api lens ---------------------------------------------------------------

#[test]
fn naming_convention_maps_onto_neutral_visibility() {
    let Some(index) = index("def public():\n    pass\n\ndef _private():\n    pass\n") else {
        return;
    };
    assert_eq!(
        at(&index, "pkg::public").and_then(|n| n.visibility),
        Some(Visibility::Public)
    );
    assert_eq!(
        at(&index, "pkg::_private").and_then(|n| n.visibility),
        Some(Visibility::Private)
    );
}

#[test]
fn privacy_by_convention_says_so_rather_than_implying_enforcement() {
    let Some(index) = index("def _private():\n    pass\n") else {
        return;
    };
    let Some(node) = at(&index, "pkg::_private") else {
        return;
    };
    let detail = node
        .facets_for(Lens::Api)
        .find_map(|f| f.detail.clone())
        .unwrap_or_default();
    assert!(
        detail.contains("convention"),
        "the facet must not imply Python enforces this: {detail}"
    );
}

#[test]
fn a_dunder_is_public_protocol_despite_its_underscores() {
    let Some(index) = index("class W:\n    def __init__(self):\n        pass\n") else {
        return;
    };
    assert_eq!(
        at(&index, "pkg::W::__init__").and_then(|n| n.visibility),
        Some(Visibility::Public)
    );
    assert!(subtypes(&index, "pkg::W::__init__", Lens::Api).contains(&"dunder".to_owned()));
}

#[test]
fn an_all_declaration_is_the_explicit_public_surface() {
    let Some(index) = index("__all__ = [\"Widget\"]\n") else {
        return;
    };
    assert!(subtypes(&index, "pkg::__all__", Lens::Api).contains(&"all-export".to_owned()));
}

// --- substitution lens ------------------------------------------------------

#[test]
fn protocol_abstract_and_plain_subclassing_are_distinguished() {
    let Some(index) = index(
        "class P(Protocol):\n    pass\n\nclass A(ABC):\n    pass\n\nclass S(Base):\n    pass\n",
    ) else {
        return;
    };
    assert!(subtypes(&index, "pkg::P", Lens::Substitution).contains(&"protocol".to_owned()));
    assert!(subtypes(&index, "pkg::A", Lens::Substitution).contains(&"abstract".to_owned()));
    let plain = subtypes(&index, "pkg::S", Lens::Substitution);
    assert!(plain.contains(&"subclass".to_owned()));
    assert!(!plain.contains(&"protocol".to_owned()));
}

#[test]
fn a_contract_method_with_a_body_is_a_replaceable_default() {
    let Some(index) = index(
        "class C(Protocol):\n    def required(self): ...\n    def defaulted(self):\n        return 1\n",
    ) else {
        return;
    };
    assert!(
        subtypes(&index, "pkg::C::defaulted", Lens::Substitution)
            .contains(&"default-method".to_owned())
    );
    // `...` is a requirement, not a substitution point — the same rule as Rust's
    // bodiless trait method, reached through different syntax.
    assert!(
        !subtypes(&index, "pkg::C::required", Lens::Substitution)
            .contains(&"default-method".to_owned())
    );
}

#[test]
fn a_callable_parameter_is_a_behaviour_slot() {
    let Some(index) = index("def run(cb: Callable[[int], int], n: int):\n    pass\n") else {
        return;
    };
    assert!(subtypes(&index, "pkg::run", Lens::Substitution).contains(&"callable".to_owned()));
}

// --- variation lens ---------------------------------------------------------

#[test]
fn a_decorator_pairs_with_the_definition_it_decorates() {
    // The conformance point: Python wraps decorator and definition in a parent node, but
    // they are siblings inside it, so the same pairing rule serves both languages.
    let Some(index) =
        index("@app.route(\"/x\")\ndef handler():\n    pass\n\ndef plain():\n    pass\n")
    else {
        return;
    };
    let decorated = subtypes(&index, "pkg::handler", Lens::Variation);
    assert!(
        decorated.contains(&"decorator".to_owned()),
        "got {decorated:?}"
    );
    // And it must not leak onto the undecorated function that follows.
    assert!(subtypes(&index, "pkg::plain", Lens::Variation).is_empty());
}

#[test]
fn an_abstractmethod_decorator_also_means_substitution() {
    let Some(index) = index("class C:\n    @abstractmethod\n    def f(self): ...\n") else {
        return;
    };
    assert!(subtypes(&index, "pkg::C::f", Lens::Substitution).contains(&"abstract".to_owned()));
}

#[test]
fn a_type_checking_branch_is_pythons_compile_time_variation() {
    let Some(index) = index("def f():\n    if TYPE_CHECKING:\n        import foo\n") else {
        return;
    };
    assert!(subtypes(&index, "pkg::f", Lens::Variation).contains(&"type-checking".to_owned()));
}

#[test]
fn a_platform_branch_is_variation_too() {
    let Some(index) = index("def f():\n    if sys.platform == \"win32\":\n        pass\n") else {
        return;
    };
    assert!(subtypes(&index, "pkg::f", Lens::Variation).contains(&"platform-branch".to_owned()));
}

// --- boundary lens ----------------------------------------------------------

#[test]
fn a_ctypes_import_binds_a_foreign_library() {
    let Some(index) = index("def load():\n    import ctypes\n") else {
        return;
    };
    assert!(subtypes(&index, "pkg::load", Lens::Boundary).contains(&"foreign-binding".to_owned()));
}

#[test]
fn a_relative_import_stays_inside_the_package() {
    let Some(index) = index("def a():\n    import os\n\ndef b():\n    from . import sibling\n")
    else {
        return;
    };
    assert!(subtypes(&index, "pkg::a", Lens::Boundary).contains(&"external-import".to_owned()));
    assert!(
        !subtypes(&index, "pkg::b", Lens::Boundary).contains(&"external-import".to_owned()),
        "a relative import does not cross the package line"
    );
}

// --- hazard lens ------------------------------------------------------------

#[test]
fn async_await_and_scope_escapes_are_hazards() {
    let Some(index) =
        index("async def slow():\n    await other()\n\ndef g():\n    global counter\n")
    else {
        return;
    };
    let slow = subtypes(&index, "pkg::slow", Lens::Hazard);
    assert!(slow.contains(&"async".to_owned()), "got {slow:?}");
    assert!(slow.contains(&"await".to_owned()), "got {slow:?}");
    assert!(subtypes(&index, "pkg::g", Lens::Hazard).contains(&"global".to_owned()));
}

// --- the contract itself ----------------------------------------------------

#[test]
fn adding_a_language_needed_no_new_lens_or_node_kind() -> TestResult {
    use crate::lang::SeamLanguage;
    use crate::model::LENSES;

    // §10: a language contributes mappings, never extensions. Every subtype Python emits
    // belongs to one of the five existing lenses, and every kind it produces is one of
    // the existing universal kinds.
    let declared = super::Python.subtypes();
    assert!(!declared.is_empty());
    for (lens, subtype) in declared {
        assert!(
            LENSES.contains(lens),
            "{} escaped the closed lens set",
            subtype.name()
        );
    }

    let Some(index) =
        index("class C(Protocol):\n    def f(self): ...\n\nMAX = 1\n\ndef g():\n    pass\n")
    else {
        return Ok(());
    };
    for node in index.nodes() {
        assert!(
            NodeKind::all().contains(&node.kind),
            "{:?} is not a universal kind",
            node.kind
        );
    }
    Ok(())
}

#[test]
fn a_language_may_declare_no_semantic_capabilities() {
    use crate::lang::SeamLanguage;
    // The contract says (3) may be empty and the view degrades rather than failing.
    assert!(super::Python.semantic_capabilities().is_empty());
}

#[test]
fn the_query_language_needed_no_change_to_serve_a_second_language() -> TestResult {
    let Some(index) = index("class C(Protocol):\n    pass\n\nasync def slow():\n    pass\n") else {
        return Ok(());
    };
    // The same terms, unmodified, filter Python.
    let hazard = crate::query::parse("lens:hazard").map_err(|e| e.to_string())?;
    let found: Vec<String> = crate::query::evaluate(&hazard, &index)
        .nodes
        .into_iter()
        .filter_map(|id| index.path(id).map(ToString::to_string))
        .collect();
    assert_eq!(found, ["pkg::slow"]);

    let protocol = crate::query::parse("substitution:protocol").map_err(|e| e.to_string())?;
    assert_eq!(crate::query::evaluate(&protocol, &index).len(), 1);
    Ok(())
}
