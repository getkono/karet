//! End-to-end tests for the Rust mapping: source in, classified nodes and facets out.

use std::path::Path;

use karet_treesitter::ParserPool;

use crate::extract::extract_file;
use crate::id::SeamPath;
use crate::index::SeamIndex;
use crate::model::Lens;
use crate::model::NodeKind;
use crate::model::Visibility;

/// Index one snippet as a package root named `pkg`, returning the populated index.
///
/// Returns `None` when the Rust grammar is not compiled in, so the suite is inert rather
/// than failing in a build without `lang-rust`.
fn index(source: &str) -> Option<SeamIndex> {
    let language = super::language_id()?;
    let mut index = SeamIndex::new();
    let root: SeamPath = "pkg".parse().ok()?;
    let root_id = index.intern(root);
    let file = index.intern_file(Path::new("src/lib.rs"));
    // The package root is a real node: it is the tree's root, the parent every top-level
    // declaration hangs from, and the row that carries the whole-package rollups.
    index.insert(crate::model::Node {
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
    // The real pipeline, not a shortcut past it: an `impl` block's members do not live
    // where they were written, and a suite that skipped the regroup pass would be
    // asserting a tree the app never sees.
    crate::regroup::apply(&mut index, outcome.ownership);
    index.recompute_rollups();
    Some(index)
}

/// Tests that need to reach into a specific node return this, so a missing node
/// surfaces through `?` rather than an `expect` the lint policy bans.
type TestResult = Result<(), String>;

/// The paths of every node, sorted, for whole-shape assertions.
fn paths(index: &SeamIndex) -> Vec<String> {
    let mut out: Vec<String> = index
        .nodes()
        .filter_map(|node| index.path(node.id).map(ToString::to_string))
        .collect();
    out.sort();
    out
}

/// Look a node up by its full path.
fn at<'a>(index: &'a SeamIndex, path: &str) -> Option<&'a crate::model::Node> {
    let parsed: SeamPath = path.parse().ok()?;
    index.resolve(&parsed).and_then(|id| index.node(id))
}

/// The subtypes a node carries for one lens, sorted.
fn subtypes(index: &SeamIndex, path: &str, lens: Lens) -> Vec<String> {
    let Some(node) = at(index, path) else {
        return Vec::new();
    };
    let mut out: Vec<String> = node
        .facets_for(lens)
        .map(|facet| facet.subtype.name().to_owned())
        .collect();
    out.sort();
    out
}

// --- containment ------------------------------------------------------------

#[test]
fn builds_a_containment_tree_from_module_structure() {
    let Some(index) = index(
        r"
mod outer {
    struct S { field: u32 }
    fn free() {}
}
",
    ) else {
        return;
    };
    assert_eq!(
        paths(&index),
        [
            "pkg",
            "pkg::outer",
            "pkg::outer::S",
            "pkg::outer::S::field",
            "pkg::outer::free",
        ]
    );
}

#[test]
fn maps_constructs_onto_universal_kinds() {
    let Some(index) = index(
        r#"
struct S;
enum E { V }
trait T {}
impl T for S {}
fn f() {}
const K: u32 = 1;
static ST: u32 = 1;
macro_rules! m { () => {} }
type Alias = u32;
unsafe extern "C" {}
"#,
    ) else {
        return;
    };
    let kind = |path: &str| at(&index, path).map(|node| node.kind);
    assert_eq!(kind("pkg::S"), Some(NodeKind::Type));
    assert_eq!(kind("pkg::E"), Some(NodeKind::Type));
    assert_eq!(kind("pkg::E::V"), Some(NodeKind::Member));
    assert_eq!(kind("pkg::T"), Some(NodeKind::Interface));
    assert_eq!(kind("pkg::S::{impl T}"), Some(NodeKind::Implementation));
    assert_eq!(kind("pkg::f"), Some(NodeKind::Function));
    assert_eq!(kind("pkg::K"), Some(NodeKind::Constant));
    assert_eq!(kind("pkg::ST"), Some(NodeKind::Constant));
    assert_eq!(kind("pkg::m"), Some(NodeKind::MacroDef));
    assert_eq!(kind("pkg::Alias"), Some(NodeKind::Type));
}

#[test]
fn a_method_is_a_member_but_a_free_function_is_not() {
    let Some(index) = index("fn free() {}\nstruct S;\nimpl S { fn method(&self) {} }") else {
        return;
    };
    assert_eq!(
        at(&index, "pkg::free").map(|n| n.kind),
        Some(NodeKind::Function)
    );
    assert_eq!(
        at(&index, "pkg::S::method").map(|n| n.kind),
        Some(NodeKind::Member)
    );
}

#[test]
fn identical_impl_blocks_get_stable_ordinals() {
    let Some(index) = index("struct S;\nimpl S { fn a(&self) {} }\nimpl S { fn b(&self) {} }")
    else {
        return;
    };
    // Both blocks dissolve into the type, so their members become siblings — which is
    // what they always were to a reader. Neither block survives as a level.
    assert!(at(&index, "pkg::S::a").is_some());
    assert!(at(&index, "pkg::S::b").is_some());
    assert!(at(&index, "pkg::{impl S}").is_none());
}

#[test]
fn identity_ignores_generic_parameters_so_adding_a_bound_is_not_a_rename() {
    let Some(before) = index("struct S;\nimpl S { fn f<T>(&self) {} }") else {
        return;
    };
    let Some(after) = index("struct S;\nimpl S { fn f<T: Clone + Send>(&self) {} }") else {
        return;
    };
    assert_eq!(paths(&before), paths(&after));
}

#[test]
fn a_broken_buffer_still_yields_the_intact_declarations() {
    let Some(index) = index("fn good() {}\nfn broken( {\nstruct S;") else {
        return;
    };
    // The whole point of the structural tier: something usable from invalid input.
    assert!(at(&index, "pkg::good").is_some());
    assert!(index.len() > 1);
}

// --- api lens ---------------------------------------------------------------

#[test]
fn reads_every_declared_visibility_level() {
    let Some(index) = index(
        r"
pub fn a() {}
pub(crate) fn b() {}
pub(super) fn c() {}
pub(in crate::x) fn d() {}
fn e() {}
",
    ) else {
        return;
    };
    let vis = |path: &str| at(&index, path).and_then(|node| node.visibility);
    assert_eq!(vis("pkg::a"), Some(Visibility::Public));
    assert_eq!(vis("pkg::b"), Some(Visibility::Crate));
    assert_eq!(vis("pkg::c"), Some(Visibility::Super));
    assert_eq!(vis("pkg::d"), Some(Visibility::Restricted));
    assert_eq!(vis("pkg::e"), Some(Visibility::Private));

    // Private is reported as a fact, not left absent — "nothing is exposed" is a finding.
    assert_eq!(subtypes(&index, "pkg::e", Lens::Api), ["private"]);
    assert_eq!(subtypes(&index, "pkg::a", Lens::Api), ["pub"]);
}

#[test]
fn a_public_use_is_a_reexport_on_its_module_and_a_private_one_is_not() -> TestResult {
    let Some(index) = index("mod m {\n    pub use other::Thing;\n    use quiet::Import;\n}") else {
        return Ok(());
    };
    let module = at(&index, "pkg::m").ok_or("module pkg::m")?;
    let reexports: Vec<_> = module
        .facets_for(Lens::Api)
        .filter(|f| f.subtype == super::REEXPORT)
        .collect();
    assert_eq!(reexports.len(), 1, "only the pub use republishes a name");
    assert_eq!(reexports[0].detail.as_deref(), Some("other::Thing"));
    // A re-export is not a declaration, so it never becomes a row of its own.
    assert!(at(&index, "pkg::m::Thing").is_none());
    Ok(())
}

// --- substitution lens ------------------------------------------------------

#[test]
fn classifies_traits_impls_and_blanket_impls() {
    let Some(index) = index(
        r"
trait T {}
struct S;
impl T for S {}
impl S {}
impl<X: Clone> T for X {}
",
    ) else {
        return;
    };
    assert!(subtypes(&index, "pkg::T", Lens::Substitution).contains(&"trait".to_owned()));
    assert!(subtypes(&index, "pkg::S::{impl T}", Lens::Substitution).contains(&"impl".to_owned()));
    // The self type *is* a type parameter of the block, so it covers every such type —
    // and has no local type to sit beneath, so it sits beneath the trait.
    assert!(
        subtypes(&index, "pkg::T::{impl for X}", Lens::Substitution)
            .contains(&"blanket-impl".to_owned()),
        "got {:?}",
        subtypes(&index, "pkg::T::{impl for X}", Lens::Substitution)
    );
}

#[test]
fn a_trait_method_with_a_body_is_a_default_and_a_bodiless_one_is_not() {
    let Some(index) = index("trait T {\n    fn required(&self);\n    fn defaulted(&self) {}\n}")
    else {
        return;
    };
    assert!(
        subtypes(&index, "pkg::T::defaulted", Lens::Substitution)
            .contains(&"default-method".to_owned())
    );
    assert!(
        !subtypes(&index, "pkg::T::required", Lens::Substitution)
            .contains(&"default-method".to_owned()),
        "a requirement is not a substitution point"
    );
}

#[test]
fn dyn_impl_trait_and_function_pointers_become_sites_on_the_enclosing_entity() {
    let Some(index) =
        index("fn f(x: &dyn Read, y: impl Write) -> Box<dyn Send> { }\nstruct H { cb: fn(u8) }")
    else {
        return;
    };
    let found = subtypes(&index, "pkg::f", Lens::Substitution);
    assert!(found.contains(&"dyn".to_owned()), "got {found:?}");
    assert!(found.contains(&"impl-trait".to_owned()), "got {found:?}");
    // Sub-item constructs never become rows.
    assert!(at(&index, "pkg::f::dyn").is_none());

    assert!(subtypes(&index, "pkg::H::cb", Lens::Substitution).contains(&"fn-ptr".to_owned()));
}

#[test]
fn a_boxed_closure_is_distinguished_from_a_plain_trait_object() {
    let Some(index) = index("struct H { boxed: Box<dyn Fn(u8)>, other: Box<dyn Read> }") else {
        return;
    };
    assert!(
        subtypes(&index, "pkg::H::boxed", Lens::Substitution).contains(&"boxed-closure".to_owned())
    );
    assert!(subtypes(&index, "pkg::H::other", Lens::Substitution).contains(&"dyn".to_owned()));
}

#[test]
fn generic_bounds_and_associated_types_are_substitution_points() {
    let Some(index) = index("trait T { type Assoc; }\nfn f<X: Clone>(x: X) where X: Send {}")
    else {
        return;
    };
    assert!(
        subtypes(&index, "pkg::T::Assoc", Lens::Substitution).contains(&"assoc-type".to_owned())
    );
    assert!(subtypes(&index, "pkg::f", Lens::Substitution).contains(&"generic-bound".to_owned()));
}

// --- variation lens ---------------------------------------------------------

#[test]
fn attributes_bind_to_the_item_that_follows_them() {
    let Some(index) = index(
        "#[cfg(feature = \"view\")]\npub fn gated() {}\n#[derive(Clone, Debug)]\npub struct D;\nfn plain() {}",
    ) else {
        return;
    };
    let gated = subtypes(&index, "pkg::gated", Lens::Variation);
    assert!(gated.contains(&"cfg".to_owned()), "got {gated:?}");
    // A feature gate is called out specifically as well as generically.
    assert!(gated.contains(&"feature".to_owned()), "got {gated:?}");
    assert!(subtypes(&index, "pkg::D", Lens::Variation).contains(&"derive".to_owned()));
    // Crucially, the attributes must not leak onto an undecorated later item.
    assert!(subtypes(&index, "pkg::plain", Lens::Variation).is_empty());
}

#[test]
fn an_attribute_on_a_discarded_construct_does_not_leak_forward() {
    // `use` is not an entity, so its `#[cfg]` must be consumed rather than left queued.
    let Some(index) = index("#[cfg(unix)]\nuse std::io;\nfn untouched() {}") else {
        return;
    };
    assert!(subtypes(&index, "pkg::untouched", Lens::Variation).is_empty());
}

#[test]
fn macro_definitions_and_invocations_are_variation() {
    let Some(index) = index("macro_rules! m { () => {} }\nfn f() { m!(); }") else {
        return;
    };
    let def = subtypes(&index, "pkg::m", Lens::Variation);
    assert!(def.contains(&"macro-def".to_owned()));
    assert!(def.contains(&"macro-rules".to_owned()));
    assert!(subtypes(&index, "pkg::f", Lens::Variation).contains(&"macro-call".to_owned()));
}

#[test]
fn an_include_invocation_is_its_own_subtype() {
    let Some(index) = index("fn f() { include_str!(\"x\"); }") else {
        return;
    };
    assert!(subtypes(&index, "pkg::f", Lens::Variation).contains(&"include".to_owned()));
}

#[test]
fn an_unrecognized_attribute_reads_as_an_attribute_macro() {
    let Some(index) = index("#[tokio::main]\nfn main() {}\n#[inline]\nfn known() {}") else {
        return;
    };
    assert!(subtypes(&index, "pkg::main", Lens::Variation).contains(&"attr-macro".to_owned()));
    // A built-in attribute is not a macro and must not be reported as variation.
    assert!(subtypes(&index, "pkg::known", Lens::Variation).is_empty());
}

// --- boundary lens ----------------------------------------------------------

#[test]
fn foreign_blocks_and_their_declarations_cross_the_line() -> TestResult {
    let Some(index) = index(
        "unsafe extern \"C\" {\n    #[link_name = \"c_fn\"]\n    pub fn c_fn(x: u32) -> u32;\n}",
    ) else {
        return Ok(());
    };
    let block = at(&index, "pkg::{extern \"C\"}").ok_or("the foreign block")?;
    assert_eq!(block.kind, NodeKind::ForeignBlock);
    assert!(block.has_subtype(Lens::Boundary, "extern-block"));

    let declared = subtypes(&index, "pkg::{extern \"C\"}::c_fn", Lens::Boundary);
    assert!(
        declared.contains(&"extern-fn".to_owned()),
        "got {declared:?}"
    );
    assert!(
        declared.contains(&"link-name".to_owned()),
        "got {declared:?}"
    );
    Ok(())
}

#[test]
fn an_exported_symbol_is_boundary_in_both_attribute_spellings() {
    let Some(index) = index(
        "#[no_mangle]\npub extern \"C\" fn old() {}\n#[unsafe(no_mangle)]\npub extern \"C\" fn new() {}",
    ) else {
        return;
    };
    for path in ["pkg::old", "pkg::new"] {
        let found = subtypes(&index, path, Lens::Boundary);
        assert!(
            found.contains(&"no-mangle".to_owned()),
            "{path}: got {found:?}"
        );
        assert!(
            found.contains(&"extern-fn".to_owned()),
            "{path}: got {found:?}"
        );
    }
}

#[test]
fn only_a_top_level_main_is_an_entry_point() {
    let Some(index) = index("fn main() {}\nstruct S;\nimpl S { fn main(&self) {} }") else {
        return;
    };
    assert!(subtypes(&index, "pkg::main", Lens::Boundary).contains(&"entry-point".to_owned()));
    assert!(
        !subtypes(&index, "pkg::S::main", Lens::Boundary).contains(&"entry-point".to_owned()),
        "a method named main is an ordinary member"
    );
}

// --- hazard lens ------------------------------------------------------------

#[test]
fn unsafe_and_async_are_reported_on_the_declaration() {
    let Some(index) = index("pub unsafe fn danger() {}\npub async fn slow() {}") else {
        return;
    };
    assert!(subtypes(&index, "pkg::danger", Lens::Hazard).contains(&"unsafe".to_owned()));
    assert!(subtypes(&index, "pkg::slow", Lens::Hazard).contains(&"async".to_owned()));
}

#[test]
fn interior_unsafe_blocks_and_awaits_become_sites_not_nodes() -> TestResult {
    let Some(index) = index("async fn f() { unsafe { } unsafe { } other().await; }") else {
        return Ok(());
    };
    let node = at(&index, "pkg::f").ok_or("function pkg::f")?;
    let unsafe_facet = node
        .facets_for(Lens::Hazard)
        .find(|facet| facet.subtype.name() == "unsafe")
        .ok_or("an unsafe facet")?;
    // Two blocks merge into one facet carrying two sites — containment stays a tree.
    assert_eq!(unsafe_facet.sites.len(), 2);
    assert_eq!(unsafe_facet.occurrences(), 2);
    assert!(node.has_subtype(Lens::Hazard, "await"));
    assert_eq!(index.children(node.id).len(), 0);
    Ok(())
}

#[test]
fn send_and_sync_bounds_are_hazards() {
    let Some(index) = index("fn f<T: Send + Sync>(x: T) {}") else {
        return;
    };
    let found = subtypes(&index, "pkg::f", Lens::Hazard);
    assert!(found.contains(&"send-bound".to_owned()), "got {found:?}");
    assert!(found.contains(&"sync-bound".to_owned()), "got {found:?}");
}

#[test]
fn locks_and_spawns_are_not_guessed_from_names() {
    // The structural tier must not claim a hazard it cannot decide. A method called
    // `lock` proves nothing without types, and a lens that sometimes lies is worse than
    // one that admits it does not know.
    let Some(index) = index("fn f() { let g = mutex.lock(); tokio::spawn(task); }") else {
        return;
    };
    let found = subtypes(&index, "pkg::f", Lens::Hazard);
    assert!(!found.contains(&"lock".to_owned()), "got {found:?}");
    assert!(!found.contains(&"spawn".to_owned()), "got {found:?}");
}

// --- rollups ----------------------------------------------------------------

#[test]
fn rollups_let_a_collapsed_module_advertise_what_is_under_it() -> TestResult {
    let Some(index) = index(
        r"
mod deep {
    pub unsafe fn a() {}
    pub fn b() {}
}
",
    ) else {
        return Ok(());
    };
    let module = at(&index, "pkg::deep").ok_or("module pkg::deep")?;
    assert!(module.rollups.get(Lens::Hazard) >= 1);
    assert!(module.rollups.get(Lens::Api) >= 2);

    let root = at(&index, "pkg").ok_or("the package root")?;
    assert_eq!(
        root.rollups.get(Lens::Hazard),
        module.rollups.get(Lens::Hazard)
    );
    Ok(())
}

// --- language contract ------------------------------------------------------

#[test]
fn every_subtype_the_mapping_emits_is_declared() {
    use crate::lang::SeamLanguage;
    let declared = super::Rust.subtypes();
    assert!(!declared.is_empty());
    // Each declared subtype belongs to exactly one lens.
    for (lens, subtype) in declared {
        let matches = declared
            .iter()
            .filter(|(_, other)| other.name() == subtype.name())
            .count();
        assert_eq!(
            matches,
            1,
            "{} is declared under more than one lens",
            subtype.name()
        );
        assert!(!subtype.name().is_empty());
        let _ = lens;
    }
}

#[test]
fn attribute_arguments_split_across_all_three_written_shapes() {
    use crate::lang::SeamLanguage;
    let Some(index) = index(
        "#[no_mangle]\npub fn a() {}\n#[link_name = \"c\"]\npub fn b() {}\n#[link(name = \"z\")]\npub fn c() {}",
    ) else {
        return;
    };
    // Bare, `= value`, and `(args)` all have to reach the boundary lens intact.
    assert!(subtypes(&index, "pkg::a", Lens::Boundary).contains(&"no-mangle".to_owned()));
    assert!(subtypes(&index, "pkg::b", Lens::Boundary).contains(&"link-name".to_owned()));
    assert!(subtypes(&index, "pkg::c", Lens::Boundary).contains(&"link".to_owned()));
    let _ = super::Rust.subtypes();
}

#[test]
fn the_mapping_declares_what_its_semantic_tier_can_resolve() {
    use crate::lang::SeamLanguage;
    let capabilities = super::Rust.semantic_capabilities();
    assert!(capabilities.contains(&crate::edge::EdgeKind::Implements));
}

// --- the declaration head ---------------------------------------------------

#[test]
fn a_wrapped_signature_is_all_head() -> TestResult {
    // The whole reason the head is a range rather than a line count: this signature is
    // four lines, and a preview that shows two of them has shown nothing useful.
    let source = "\
pub fn render(
    widget: &Widget,
    area: Rect,
) -> Result<(), Error> {
    Ok(())
}
";
    let Some(index) = index(source) else {
        return Ok(());
    };
    let node = at(&index, "pkg::render").ok_or("no render")?;
    assert_eq!(node.location.header.start.line, 0);
    // Through the line the body opens on, not the line the name sits on.
    assert_eq!(node.location.header.end.line, 3);
    assert_eq!(node.location.range.end.line, 5);
    Ok(())
}

#[test]
fn a_construct_with_no_body_is_all_head() -> TestResult {
    let Some(index) = index("pub const LIMIT: usize = 20_000;\n") else {
        return Ok(());
    };
    let node = at(&index, "pkg::LIMIT").ok_or("no LIMIT")?;
    assert_eq!(node.location.header, node.location.range);
    Ok(())
}

#[test]
fn a_type_and_an_impl_both_cut_at_their_brace() -> TestResult {
    let source = "\
pub struct Widget<T>
where
    T: Clone,
{
    pub id: u32,
}

impl<T: Clone> Render for Widget<T> {
    fn render(&self) {}
}
";
    let Some(index) = index(source) else {
        return Ok(());
    };
    let widget = at(&index, "pkg::Widget").ok_or("no Widget")?;
    // Through the `where` clause, but not the brace sitting alone on line 3: a line
    // holding nothing but the body's opening is not part of the declaration.
    assert_eq!(widget.location.header.end.line, 2);
    let block = at(&index, "pkg::Widget::{impl Render}").ok_or("no impl")?;
    assert_eq!(block.location.header.start.line, 7);
    assert_eq!(block.location.header.end.line, 7);
    Ok(())
}

// --- where members end up ---------------------------------------------------

#[test]
fn an_inherent_impl_dissolves_into_the_type_it_belongs_to() -> TestResult {
    let Some(index) = index("struct Widget { id: u32 }\nimpl Widget { fn render(&self) {} }")
    else {
        return Ok(());
    };
    // A field written inside the type and a method written beside it are both members of
    // `Widget`, which is what a reader was looking for all along.
    let widget = at(&index, "pkg::Widget").ok_or("no Widget")?;
    let children: Vec<&str> = widget
        .children
        .iter()
        .filter_map(|id| index.node(*id))
        .map(|node| node.name.as_str())
        .collect();
    assert_eq!(children, ["id", "render"]);
    assert_eq!(
        at(&index, "pkg::Widget::render").map(|n| n.kind),
        Some(NodeKind::Member)
    );
    Ok(())
}

#[test]
fn a_trait_impl_stays_a_level_under_the_type_and_drops_the_redundant_half() -> TestResult {
    let Some(index) = index(
        r"
trait Render {}
struct Widget;
impl Render for Widget { fn go(&self) {} }
",
    ) else {
        return Ok(());
    };
    // `impl Render for Widget` under `Widget` repeats what the reader can already see.
    let block = at(&index, "pkg::Widget::{impl Render}").ok_or("no impl")?;
    assert_eq!(block.kind, NodeKind::Implementation);
    assert_eq!(block.name, "impl Render");
    assert!(at(&index, "pkg::Widget::{impl Render}::go").is_some());
    Ok(())
}

#[test]
fn an_impl_for_a_foreign_type_sits_under_the_trait_instead() -> TestResult {
    let Some(index) = index("trait Render {}\nimpl Render for String { fn go(&self) {} }") else {
        return Ok(());
    };
    // Nothing in this package declares `String`, so the trait is the only thing here the
    // block is about — and it is where a reader looks for its implementors.
    let block = at(&index, "pkg::Render::{impl for String}").ok_or("no impl")?;
    assert_eq!(block.name, "impl for String");
    assert!(at(&index, "pkg::Render::{impl for String}::go").is_some());
    Ok(())
}

#[test]
fn a_blanket_impl_sits_under_its_trait() -> TestResult {
    let Some(index) = index("trait Render {}\nimpl<T: Clone> Render for T { fn go(&self) {} }")
    else {
        return Ok(());
    };
    // The self type *is* a type parameter, so there is no type to sit beneath.
    assert!(at(&index, "pkg::Render::{impl for T}").is_some());
    Ok(())
}

#[test]
fn an_impl_for_something_nothing_here_declares_stays_where_it_was_written() -> TestResult {
    let Some(index) = index("impl std::fmt::Display for Vec<u8> { fn go(&self) {} }") else {
        return Ok(());
    };
    // Neither end resolves inside the package. Inventing a home for it would be worse
    // than leaving it where the author put it.
    assert!(at(&index, "pkg::{impl std::fmt::Display for Vec<u8>}").is_some());
    Ok(())
}

#[test]
fn a_reference_or_generic_self_type_still_finds_its_type() -> TestResult {
    let Some(index) = index(
        r"
trait Render {}
struct Widget<T> { id: T }
impl<T> Render for &Widget<T> { fn go(&self) {} }
",
    ) else {
        return Ok(());
    };
    assert!(at(&index, "pkg::Widget::{impl Render}::go").is_some());
    Ok(())
}

#[test]
fn an_impl_in_another_module_reaches_the_type_it_names() -> TestResult {
    let Some(index) = index(
        r"
mod model { pub struct Widget; }
mod behaviour { impl crate::model::Widget { pub fn render(&self) {} } }
",
    ) else {
        return Ok(());
    };
    // Written two modules away, and still a member of `Widget`.
    assert!(at(&index, "pkg::model::Widget::render").is_some());
    Ok(())
}

#[test]
fn a_config_gated_inherent_impl_keeps_its_level() -> TestResult {
    let Some(index) = index(
        r#"
struct Widget;
impl Widget { fn always(&self) {} }
#[cfg(unix)]
impl Widget { fn only_on_unix(&self) {} }
"#,
    ) else {
        return Ok(());
    };
    // Dissolving this one would put `only_on_unix` beside `always` as though both always
    // existed. The gate decides whether those members exist at all, and the type cannot
    // carry that fact, so the block stays to carry it.
    assert!(at(&index, "pkg::Widget::always").is_some());
    let gated = at(&index, "pkg::Widget::{impl Widget}").ok_or("no gated block")?;
    assert_eq!(gated.kind, NodeKind::Implementation);
    assert!(
        subtypes(&index, "pkg::Widget::{impl Widget}", Lens::Variation).contains(&"cfg".to_owned())
    );
    assert!(at(&index, "pkg::Widget::{impl Widget}::only_on_unix").is_some());
    Ok(())
}

#[test]
fn two_trait_impls_on_one_type_each_keep_their_own_level() -> TestResult {
    let Some(index) = index(
        r"
trait Draw {}
trait Save {}
struct Widget;
impl Draw for Widget { fn draw(&self) {} }
impl Save for Widget { fn save(&self) {} }
",
    ) else {
        return Ok(());
    };
    // Two traits with a same-named method would collide if the levels were flattened.
    assert!(at(&index, "pkg::Widget::{impl Draw}::draw").is_some());
    assert!(at(&index, "pkg::Widget::{impl Save}::save").is_some());
    Ok(())
}

#[test]
fn members_arrive_under_the_type_in_source_order() -> TestResult {
    let Some(index) = index(
        r"
struct Widget { id: u32 }
impl Widget { fn first(&self) {} }
impl Widget { fn second(&self) {} }
",
    ) else {
        return Ok(());
    };
    let widget = at(&index, "pkg::Widget").ok_or("no Widget")?;
    let names: Vec<&str> = widget
        .children
        .iter()
        .filter_map(|id| index.node(*id))
        .map(|node| node.name.as_str())
        .collect();
    assert_eq!(names, ["id", "first", "second"]);
    Ok(())
}

#[test]
fn a_types_rollups_now_count_the_members_written_beside_it() -> TestResult {
    let Some(index) = index(
        r"
struct Widget;
impl Widget {
    pub fn shown(&self) {}
    pub fn also(&self) {}
}
",
    ) else {
        return Ok(());
    };
    // The point of the whole change, in one number: asking `Widget` how much api surface
    // it has used to answer zero.
    let widget = at(&index, "pkg::Widget").ok_or("no Widget")?;
    assert_eq!(widget.rollups.get(Lens::Api), 3);
    Ok(())
}
