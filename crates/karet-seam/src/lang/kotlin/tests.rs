//! Kotlin mapping tests, run through the pipeline the app runs.

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

/// Index one snippet as a package root named `pkg`, regroup pass included.
fn index(source: &str) -> Option<SeamIndex> {
    let language = super::language_id()?;
    let mut index = SeamIndex::new();
    let root: SeamPath = "pkg".parse().ok()?;
    let root_id = index.intern(root);
    let file = index.intern_file(Path::new("src/main/kotlin/File.kt"));
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
            header: karet_core::Range::default(),
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
    let outcome = extract_file(&mut index, &mut pool, root_id, file, language, source).ok()?;
    crate::regroup::apply(&mut index, outcome.ownership);
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
        .map(|facet| facet.subtype.0.to_owned())
        .collect();
    out.sort();
    out
}

fn paths(index: &SeamIndex) -> Vec<String> {
    let mut out: Vec<String> = index
        .nodes()
        .filter_map(|node| index.path(node.id).map(ToString::to_string))
        .collect();
    out.sort();
    out
}

// --- classification ---------------------------------------------------------

#[test]
fn every_declaration_form_maps_onto_a_kind() {
    let Some(index) = index(
        r"
interface Renderable {
    fun render()
}
class Widget(val id: Int) {
    private val name: Int = 0
    fun render() {}
}
enum class Colour { RED, GREEN }
object Single { fun go() {} }
typealias Alias = Int
fun free(): Int = 1
const val LIMIT = 10
",
    ) else {
        return;
    };
    let kind = |path: &str| at(&index, path).map(|node| node.kind);
    assert_eq!(kind("pkg::Renderable"), Some(NodeKind::Interface));
    assert_eq!(kind("pkg::Renderable::render"), Some(NodeKind::Member));
    assert_eq!(kind("pkg::Widget"), Some(NodeKind::Type));
    // A primary-constructor `val` declares a property as surely as a body one does.
    assert_eq!(kind("pkg::Widget::id"), Some(NodeKind::Member));
    assert_eq!(kind("pkg::Widget::name"), Some(NodeKind::Member));
    assert_eq!(kind("pkg::Widget::render"), Some(NodeKind::Member));
    assert_eq!(kind("pkg::Colour"), Some(NodeKind::Type));
    assert_eq!(kind("pkg::Colour::RED"), Some(NodeKind::Member));
    // An object is a type with exactly one instance, which is still a type.
    assert_eq!(kind("pkg::Single"), Some(NodeKind::Type));
    assert_eq!(kind("pkg::Single::go"), Some(NodeKind::Member));
    assert_eq!(kind("pkg::Alias"), Some(NodeKind::Type));
    assert_eq!(kind("pkg::free"), Some(NodeKind::Function));
    assert_eq!(kind("pkg::LIMIT"), Some(NodeKind::Constant));
}

#[test]
fn the_keyword_that_tells_a_class_from_an_interface_is_read_from_the_text() {
    // Both are `class_declaration`, and the keyword is a token the walk never offers.
    let Some(index) = index("interface I {}\nclass C {}\nsealed class S {}") else {
        return;
    };
    assert_eq!(
        at(&index, "pkg::I").map(|n| n.kind),
        Some(NodeKind::Interface)
    );
    assert_eq!(at(&index, "pkg::C").map(|n| n.kind), Some(NodeKind::Type));
    assert_eq!(at(&index, "pkg::S").map(|n| n.kind), Some(NodeKind::Type));
}

#[test]
fn a_local_binding_inside_a_function_is_not_an_entity() {
    let Some(index) = index("fun go() {\n    val local = 1\n}") else {
        return;
    };
    assert_eq!(paths(&index), ["pkg", "pkg::go"]);
}

// --- where members end up ---------------------------------------------------

#[test]
fn an_extension_function_is_a_member_of_its_receiver() -> TestResult {
    let Some(index) = index(
        r"
class Widget(val id: Int)
fun Widget.render(): Int = id
",
    ) else {
        return Ok(());
    };
    // Written at the top level of whatever file its author chose, and still a member of
    // `Widget` — which is the only place a reader would look for it.
    let render = at(&index, "pkg::Widget::render").ok_or("no render")?;
    assert_eq!(render.kind, NodeKind::Member);
    assert!(
        subtypes(&index, "pkg::Widget::render", Lens::Substitution)
            .contains(&"extension".to_owned())
    );
    Ok(())
}

#[test]
fn an_extension_property_is_a_member_too() {
    let Some(index) = index("class Widget\nval Widget.area: Int get() = 0") else {
        return;
    };
    assert!(at(&index, "pkg::Widget::area").is_some());
}

#[test]
fn an_extension_on_a_type_this_package_does_not_declare_stays_where_it_was() {
    let Some(index) = index("fun String.shout(): String = this") else {
        return;
    };
    // Nothing here declares `String`; inventing a home for it would be worse than
    // leaving it at the top level, where it was written.
    assert!(paths(&index).contains(&"pkg::shout".to_owned()));
}

#[test]
fn a_companion_objects_members_belong_to_the_class_directly() -> TestResult {
    let Some(index) = index(
        r"
class Widget {
    fun render() {}
    companion object {
        const val LIMIT = 10
        fun make(): Widget = Widget()
    }
}
",
    ) else {
        return Ok(());
    };
    // A companion is already written inside its class and its members are members of
    // that class, so it is simply transparent — no level, and nothing to regroup.
    let widget = at(&index, "pkg::Widget").ok_or("no Widget")?;
    let names: Vec<&str> = widget
        .children
        .iter()
        .filter_map(|id| index.node(*id))
        .map(|node| node.name.as_str())
        .collect();
    assert_eq!(names, ["render", "LIMIT", "make"]);
    Ok(())
}

// --- api lens ----------------------------------------------------------------

#[test]
fn the_unmarked_default_is_public_which_is_the_opposite_of_rusts() {
    let Some(index) = index("class Open\ninternal class Module\nprivate class Hidden") else {
        return;
    };
    assert_eq!(
        at(&index, "pkg::Open").and_then(|n| n.visibility),
        Some(Visibility::Public)
    );
    assert_eq!(
        at(&index, "pkg::Module").and_then(|n| n.visibility),
        Some(Visibility::Crate)
    );
    assert_eq!(
        at(&index, "pkg::Hidden").and_then(|n| n.visibility),
        Some(Visibility::Private)
    );
    assert_eq!(subtypes(&index, "pkg::Open", Lens::Api), ["public"]);
}

// --- substitution lens ---------------------------------------------------------

#[test]
fn what_can_be_subclassed_overridden_or_handed_off_is_a_substitution_point() {
    let Some(index) = index(
        r"
interface Renderable { fun render() }
abstract class Base : Renderable {
    override fun render() {}
    open fun go() {}
}
sealed class Closed
class Deleg(b: Renderable) : Renderable by b
object Single
",
    ) else {
        return;
    };
    assert!(
        subtypes(&index, "pkg::Renderable", Lens::Substitution).contains(&"interface".to_owned())
    );
    assert!(subtypes(&index, "pkg::Base", Lens::Substitution).contains(&"abstract".to_owned()));
    assert!(
        subtypes(&index, "pkg::Base::render", Lens::Substitution).contains(&"override".to_owned())
    );
    // Everything is final here unless it says otherwise, so `open` is a decision.
    assert!(subtypes(&index, "pkg::Base::go", Lens::Substitution).contains(&"open".to_owned()));
    assert!(subtypes(&index, "pkg::Closed", Lens::Substitution).contains(&"sealed".to_owned()));
    assert!(subtypes(&index, "pkg::Deleg", Lens::Substitution).contains(&"delegation".to_owned()));
    assert!(subtypes(&index, "pkg::Single", Lens::Substitution).contains(&"object".to_owned()));
}

// --- variation lens ------------------------------------------------------------

#[test]
fn annotations_and_multiplatform_declarations_are_variation() {
    let Some(index) = index(
        r#"
@Deprecated("gone")
class Old
expect fun platform(): String
inline fun <reified T> shape(): T? = null
"#,
    ) else {
        return;
    };
    assert!(subtypes(&index, "pkg::Old", Lens::Variation).contains(&"annotation".to_owned()));
    // `expect` has no body here; the target supplies one, which is variation by any name.
    assert!(
        subtypes(&index, "pkg::platform", Lens::Variation).contains(&"expect-actual".to_owned())
    );
    let shape = subtypes(&index, "pkg::shape", Lens::Variation);
    assert!(shape.contains(&"inline".to_owned()), "{shape:?}");
    assert!(shape.contains(&"reified".to_owned()), "{shape:?}");
}

// --- boundary lens -------------------------------------------------------------

#[test]
fn the_ways_out_of_kotlin_are_boundary_crossings() {
    let Some(index) = index(
        r"
import kotlin.math.PI
external fun native(): Int
class Holder {
    @JvmStatic fun shared() {}
}
fun main() {}
",
    ) else {
        return;
    };
    assert!(subtypes(&index, "pkg::native", Lens::Boundary).contains(&"external-fn".to_owned()));
    assert!(
        subtypes(&index, "pkg::Holder::shared", Lens::Boundary).contains(&"jvm-surface".to_owned())
    );
    assert!(subtypes(&index, "pkg::main", Lens::Boundary).contains(&"entry-point".to_owned()));
    // An import sits at file level, so it lands on the package root.
    assert!(subtypes(&index, "pkg", Lens::Boundary).contains(&"external-import".to_owned()));
}

#[test]
fn only_a_top_level_main_is_an_entry_point() {
    let Some(index) = index("class Holder { fun main() {} }\nfun main() {}") else {
        return;
    };
    assert!(subtypes(&index, "pkg::main", Lens::Boundary).contains(&"entry-point".to_owned()));
    assert!(
        !subtypes(&index, "pkg::Holder::main", Lens::Boundary).contains(&"entry-point".to_owned()),
        "a method named main is an ordinary member"
    );
}

#[test]
fn a_boundary_annotation_is_not_counted_twice_as_variation() {
    let Some(index) = index("class H { @JvmStatic fun f() {} }") else {
        return;
    };
    assert!(subtypes(&index, "pkg::H::f", Lens::Variation).is_empty());
}

// --- hazard lens ----------------------------------------------------------------

#[test]
fn suspension_forced_nulls_and_unchecked_initialization_are_hazards() {
    let Some(index) = index(
        r"
class Holder {
    lateinit var late: String
    @Volatile var shared: Int = 0
    suspend fun load(v: String?): String = v!!
}
",
    ) else {
        return;
    };
    assert!(subtypes(&index, "pkg::Holder::late", Lens::Hazard).contains(&"lateinit".to_owned()));
    assert!(subtypes(&index, "pkg::Holder::shared", Lens::Hazard).contains(&"volatile".to_owned()));
    let load = subtypes(&index, "pkg::Holder::load", Lens::Hazard);
    assert!(load.contains(&"suspend".to_owned()), "{load:?}");
    assert!(load.contains(&"forced".to_owned()), "{load:?}");
}

#[test]
fn every_declared_subtype_has_a_lens_and_no_duplicates() {
    let mut seen: Vec<(&str, &str)> = super::facets::SUBTYPES
        .iter()
        .map(|(lens, subtype)| (lens.name(), subtype.0))
        .collect();
    let count = seen.len();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), count);
}
