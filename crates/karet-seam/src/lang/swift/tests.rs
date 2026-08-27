//! Swift mapping tests, run through the pipeline the app runs.

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
    let file = index.intern_file(Path::new("Sources/pkg/File.swift"));
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
public protocol Renderable {
    associatedtype Item
    func render()
}
public struct Widget: Renderable {
    public var id: Int = 0
    public init(id: Int) { self.id = id }
    public func render() {}
}
public enum Colour { case red, green }
public typealias Alias = Int
public func free() {}
public let LIMIT = 10
",
    ) else {
        return;
    };
    let kind = |path: &str| at(&index, path).map(|node| node.kind);
    assert_eq!(kind("pkg::Renderable"), Some(NodeKind::Interface));
    assert_eq!(kind("pkg::Renderable::Item"), Some(NodeKind::Member));
    assert_eq!(kind("pkg::Renderable::render"), Some(NodeKind::Member));
    assert_eq!(kind("pkg::Widget"), Some(NodeKind::Type));
    assert_eq!(kind("pkg::Widget::id"), Some(NodeKind::Member));
    assert_eq!(kind("pkg::Widget::init"), Some(NodeKind::Member));
    assert_eq!(kind("pkg::Widget::render"), Some(NodeKind::Member));
    assert_eq!(kind("pkg::Colour"), Some(NodeKind::Type));
    assert_eq!(kind("pkg::Alias"), Some(NodeKind::Type));
    assert_eq!(kind("pkg::free"), Some(NodeKind::Function));
    assert_eq!(kind("pkg::LIMIT"), Some(NodeKind::Constant));
}

#[test]
fn one_case_line_holding_several_names_yields_several_members() {
    let Some(index) = index("enum Colour { case red, green }") else {
        return;
    };
    // `case red, green` is one node with two names and wraps neither.
    assert!(at(&index, "pkg::Colour::red").is_some());
    assert!(at(&index, "pkg::Colour::green").is_some());
}

#[test]
fn a_local_binding_inside_a_function_is_not_an_entity() {
    let Some(index) = index("func go() { let local = 1 }") else {
        return;
    };
    assert_eq!(paths(&index), ["pkg", "pkg::go"]);
}

#[test]
fn the_keyword_that_tells_declarations_apart_is_read_from_the_text() {
    // `struct`, `class`, `enum` and `extension` are all `class_declaration`, and the
    // keyword is an anonymous token the walk never offers.
    let Some(index) = index("struct S {}\nclass C {}\nenum E {}\nextension S { func x() {} }")
    else {
        return;
    };
    // The extension dissolved into `S`, which only happens if it was recognised as one.
    assert!(at(&index, "pkg::S::x").is_some());
    assert_eq!(at(&index, "pkg::C").map(|n| n.kind), Some(NodeKind::Type));
    assert_eq!(at(&index, "pkg::E").map(|n| n.kind), Some(NodeKind::Type));
}

// --- where members end up ---------------------------------------------------

#[test]
fn a_plain_extension_dissolves_into_the_type_it_extends() -> TestResult {
    let Some(index) = index(
        r"
struct Widget { var id: Int = 0 }
extension Widget {
    func render() {}
}
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
    assert_eq!(names, ["id", "render"]);
    Ok(())
}

#[test]
fn a_conformance_extension_keeps_its_level_under_the_type() -> TestResult {
    let Some(index) = index(
        r"
struct Widget {}
extension Widget: Codable {
    func encode() {}
}
",
    ) else {
        return Ok(());
    };
    let block = at(&index, "pkg::Widget::{extension Codable}").ok_or("no extension")?;
    assert_eq!(block.kind, NodeKind::Implementation);
    assert_eq!(block.name, "extension Codable");
    assert!(at(&index, "pkg::Widget::{extension Codable}::encode").is_some());
    Ok(())
}

#[test]
fn an_extension_of_a_foreign_type_sits_under_the_local_protocol() -> TestResult {
    let Some(index) = index(
        r"
protocol Renderable {}
extension Array: Renderable {
    func render() {}
}
",
    ) else {
        return Ok(());
    };
    // Nothing here declares `Array`, so the protocol is the only thing this is about.
    assert!(at(&index, "pkg::Renderable::{extension for Array}::render").is_some());
    Ok(())
}

#[test]
fn an_extension_of_something_nothing_here_declares_stays_where_it_was() {
    let Some(index) = index("extension Array { func render() {} }") else {
        return;
    };
    assert!(paths(&index).contains(&"pkg::{extension Array}".to_owned()));
}

#[test]
fn an_extension_gated_by_a_directive_keeps_its_level() -> TestResult {
    let Some(index) = index(
        r"
struct Widget {}
extension Widget {
    func always() {}
}
#if os(iOS)
extension Widget {
    func onlyOnIOS() {}
}
#endif
",
    ) else {
        return Ok(());
    };
    // `#if` is a flat sibling here, not a wrapper, so nothing in the tree relates the
    // two. Reading the directives still open above the extension is what makes the gate
    // answerable — and dissolving it would list a platform-only method as always present.
    assert!(at(&index, "pkg::Widget::always").is_some());
    let gated = at(&index, "pkg::Widget::{extension Widget}").ok_or("no gated block")?;
    assert_eq!(gated.kind, NodeKind::Implementation);
    assert!(at(&index, "pkg::Widget::{extension Widget}::onlyOnIOS").is_some());
    assert!(
        subtypes(&index, "pkg::Widget::{extension Widget}", Lens::Variation)
            .contains(&"compilation-condition".to_owned())
    );
    Ok(())
}

// --- api lens ----------------------------------------------------------------

#[test]
fn the_unmarked_default_is_module_wide_and_says_so() {
    let Some(index) = index("public struct Shown {}\nstruct Quiet {}\nprivate struct Hidden {}")
    else {
        return;
    };
    assert_eq!(
        at(&index, "pkg::Shown").and_then(|n| n.visibility),
        Some(Visibility::Public)
    );
    // Swift's default is `internal`, which is module-wide — not private.
    assert_eq!(
        at(&index, "pkg::Quiet").and_then(|n| n.visibility),
        Some(Visibility::Crate)
    );
    assert_eq!(
        at(&index, "pkg::Hidden").and_then(|n| n.visibility),
        Some(Visibility::Private)
    );
    assert_eq!(subtypes(&index, "pkg::Quiet", Lens::Api), ["internal"]);
    assert_eq!(subtypes(&index, "pkg::Shown", Lens::Api), ["public"]);
}

#[test]
fn open_is_told_apart_from_public() {
    let Some(index) = index("open class Base {}") else {
        return;
    };
    // They are equally reachable; what differs is whether they can be subclassed.
    assert_eq!(subtypes(&index, "pkg::Base", Lens::Api), ["open"]);
    assert_eq!(
        at(&index, "pkg::Base").and_then(|n| n.visibility),
        Some(Visibility::Public)
    );
}

// --- substitution lens ---------------------------------------------------------

#[test]
fn protocols_conformances_and_the_ways_of_hiding_a_type_are_substitution_points() {
    let Some(index) = index(
        r"
protocol Renderable { associatedtype Item
    func render() }
struct Widget: Renderable {
    func render() {}
    func opaque() -> some Sequence { [] }
    var boxed: any Renderable? = nil
}
",
    ) else {
        return;
    };
    assert!(
        subtypes(&index, "pkg::Renderable", Lens::Substitution).contains(&"protocol".to_owned())
    );
    assert!(
        subtypes(&index, "pkg::Renderable::Item", Lens::Substitution)
            .contains(&"assoc-type".to_owned())
    );
    // A protocol requirement has no body; the conformer supplies it.
    assert!(
        subtypes(&index, "pkg::Renderable::render", Lens::Substitution)
            .contains(&"requirement".to_owned())
    );
    assert!(
        subtypes(&index, "pkg::Widget", Lens::Substitution).contains(&"conformance".to_owned())
    );
    assert!(
        subtypes(&index, "pkg::Widget::opaque", Lens::Substitution)
            .contains(&"opaque-type".to_owned())
    );
    assert!(
        subtypes(&index, "pkg::Widget::boxed", Lens::Substitution)
            .contains(&"existential".to_owned())
    );
}

#[test]
fn a_method_in_a_conformance_extension_is_a_default_a_conformer_may_replace() {
    let Some(index) = index("struct W {}\nextension W: Codable { func encode() {} }") else {
        return;
    };
    assert!(
        subtypes(
            &index,
            "pkg::W::{extension Codable}::encode",
            Lens::Substitution
        )
        .contains(&"protocol-default".to_owned())
    );
}

// --- variation lens ------------------------------------------------------------

#[test]
fn attributes_that_rewrite_a_declaration_are_variation() {
    let Some(index) = index(
        r"
@propertyWrapper struct Wrap {}
@available(iOS 13, *) struct Newer {}
@objcMembers struct Other {}
",
    ) else {
        return;
    };
    assert!(
        subtypes(&index, "pkg::Wrap", Lens::Variation).contains(&"property-wrapper".to_owned())
    );
    assert!(subtypes(&index, "pkg::Newer", Lens::Variation).contains(&"availability".to_owned()));
    assert!(subtypes(&index, "pkg::Other", Lens::Variation).contains(&"attribute".to_owned()));
}

// --- boundary lens -------------------------------------------------------------

#[test]
fn the_ways_out_of_swift_are_boundary_crossings() {
    let Some(index) = index(
        r#"
import Foundation
@_cdecl("c_entry") func cEntry() {}
@objc class Bridged {}
@main struct App {}
"#,
    ) else {
        return;
    };
    assert!(subtypes(&index, "pkg::cEntry", Lens::Boundary).contains(&"c-entry".to_owned()));
    assert!(subtypes(&index, "pkg::Bridged", Lens::Boundary).contains(&"objc".to_owned()));
    assert!(subtypes(&index, "pkg::App", Lens::Boundary).contains(&"entry-point".to_owned()));
    // An import sits at module level, so it lands on the package root.
    assert!(subtypes(&index, "pkg", Lens::Boundary).contains(&"module-import".to_owned()));
}

#[test]
fn a_boundary_attribute_is_not_counted_twice_as_variation() {
    let Some(index) = index(r#"@_cdecl("c") func f() {}"#) else {
        return;
    };
    // One fact, one lens: counting it in both would double it in every rollup.
    assert!(subtypes(&index, "pkg::f", Lens::Variation).is_empty());
}

// --- hazard lens ----------------------------------------------------------------

#[test]
fn the_ways_of_trading_a_handled_failure_for_a_crash_are_hazards() {
    let Some(index) = index(
        r"
func risky() async throws {
    let a = try! parse()
    let b = thing as! Int
    await go()
}
",
    ) else {
        return;
    };
    let found = subtypes(&index, "pkg::risky", Lens::Hazard);
    for expected in ["async", "await", "forced", "throwing"] {
        assert!(
            found.contains(&expected.to_owned()),
            "{expected}: {found:?}"
        );
    }
}

#[test]
fn a_raw_pointer_and_an_asserted_guarantee_are_hazards() {
    let Some(index) = index(
        r"
struct Holder { var p: UnsafeMutablePointer<Int>? = nil }
struct Claimed: @unchecked Sendable {}
",
    ) else {
        return;
    };
    assert!(
        subtypes(&index, "pkg::Holder::p", Lens::Hazard).contains(&"unsafe-pointer".to_owned())
    );
    assert!(subtypes(&index, "pkg::Claimed", Lens::Hazard).contains(&"unchecked".to_owned()));
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
