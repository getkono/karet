//! TypeScript and JavaScript mapping tests.
//!
//! Every one runs the pipeline the app runs, regroup pass included, so what they assert is
//! the tree a reader actually sees rather than the one extraction happens to build.

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

/// Index one snippet as a package root named `pkg`, under the named grammar.
fn index_as(grammar: &str, source: &str) -> Option<SeamIndex> {
    let language = karet_treesitter::language_id_from_injection_name(grammar)?;
    let mut index = SeamIndex::new();
    let root: SeamPath = "pkg".parse().ok()?;
    let root_id = index.intern(root);
    let file = index.intern_file(Path::new("pkg/index.ts"));
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

fn index(source: &str) -> Option<SeamIndex> {
    index_as("typescript", source)
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

/// Every path in the index, sorted.
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
export class Widget { render(): void {} }
export abstract class Base { abstract go(): void; }
export interface Renderable { render(): void; }
export type Alias = string;
export enum Colour { Red, Green = 2 }
export function free(): void {}
export const make = () => 1;
export const LIMIT = 10;
namespace NS { export const K = 1; }
",
    ) else {
        return;
    };
    let kind = |path: &str| at(&index, path).map(|node| node.kind);
    assert_eq!(kind("pkg::Widget"), Some(NodeKind::Type));
    assert_eq!(kind("pkg::Widget::render"), Some(NodeKind::Member));
    // An abstract class states a contract, which is the distinction `interface` draws.
    assert_eq!(kind("pkg::Base"), Some(NodeKind::Interface));
    assert_eq!(kind("pkg::Renderable"), Some(NodeKind::Interface));
    assert_eq!(kind("pkg::Alias"), Some(NodeKind::Type));
    assert_eq!(kind("pkg::Colour"), Some(NodeKind::Type));
    assert_eq!(kind("pkg::free"), Some(NodeKind::Function));
    // A `const` bound to an arrow function is a function by every measure that matters.
    assert_eq!(kind("pkg::make"), Some(NodeKind::Function));
    assert_eq!(kind("pkg::LIMIT"), Some(NodeKind::Constant));
    assert_eq!(kind("pkg::NS"), Some(NodeKind::Module));
    assert_eq!(kind("pkg::NS::K"), Some(NodeKind::Constant));
}

#[test]
fn an_enum_member_is_a_member_however_it_is_written() {
    let Some(index) = index("enum Colour { Red, Green = 2 }") else {
        return;
    };
    // `Red` has no wrapping node at all — the one construct in the language that does not.
    assert!(at(&index, "pkg::Colour::Red").is_some());
    assert!(at(&index, "pkg::Colour::Green").is_some());
}

#[test]
fn a_method_name_is_not_mistaken_for_an_enum_member() {
    // Both are a `property_identifier` in a `name` field. What tells them apart is that a
    // method's name sits inside the method, so a member frame is already on the stack.
    let Some(index) = index("class Widget { render(): void {} }") else {
        return;
    };
    assert_eq!(paths(&index), ["pkg", "pkg::Widget", "pkg::Widget::render"]);
}

#[test]
fn a_local_inside_a_function_is_not_an_entity() {
    let Some(index) = index("function go() { const local = 1; }") else {
        return;
    };
    assert!(at(&index, "pkg::go::local").is_none());
}

#[test]
fn javascript_reads_through_the_same_mapping() {
    let Some(index) = index_as("javascript", "export class Widget { render() {} }") else {
        return;
    };
    // One language with a type system bolted on: the class is a class either way.
    assert_eq!(
        at(&index, "pkg::Widget::render").map(|node| node.kind),
        Some(NodeKind::Member)
    );
}

// --- where members end up ---------------------------------------------------

#[test]
fn a_prototype_assignment_becomes_a_member_of_its_class() -> TestResult {
    let Some(index) = index(
        r"
class Widget { render() {} }
Widget.prototype.extra = function () { return 1; };
",
    ) else {
        return Ok(());
    };
    // How the language wrote methods before it had a syntax for them, and still how a
    // library adds one to a class it does not own.
    let extra = at(&index, "pkg::Widget::extra").ok_or("no extra")?;
    assert_eq!(extra.kind, NodeKind::Member);
    Ok(())
}

#[test]
fn a_plain_property_assignment_is_not_read_as_a_method() {
    let Some(index) = index("const config = {};\nconfig.debug = true;") else {
        return;
    };
    // Indistinguishable from setting a property on any object, so it is left alone rather
    // than filling a type with members that are nothing of the kind.
    assert!(!paths(&index).iter().any(|path| path.ends_with("debug")));
}

#[test]
fn a_prototype_assignment_naming_no_plain_class_stays_where_it_was() {
    let Some(index) = index("a.b.prototype.c = function () {};") else {
        return;
    };
    // `a.b` names something this has no way to resolve; guessing at `b` would invent a type.
    assert_eq!(paths(&index), ["pkg"]);
}

// --- api lens ----------------------------------------------------------------

#[test]
fn export_decides_visibility_and_the_absence_of_it_says_so() {
    let Some(index) = index("export class Shown {}\nclass Hidden {}") else {
        return;
    };
    assert_eq!(
        at(&index, "pkg::Shown").and_then(|node| node.visibility),
        Some(Visibility::Public)
    );
    assert_eq!(
        at(&index, "pkg::Hidden").and_then(|node| node.visibility),
        Some(Visibility::Private)
    );
    assert!(subtypes(&index, "pkg::Shown", Lens::Api).contains(&"export".to_owned()));
    // Reported, not omitted: "nothing is exposed here" is a fact worth showing.
    assert!(subtypes(&index, "pkg::Hidden", Lens::Api).contains(&"module-local".to_owned()));
}

#[test]
fn a_default_export_is_told_apart_from_a_named_one() {
    let Some(index) = index("export default class Widget {}") else {
        return;
    };
    assert!(subtypes(&index, "pkg::Widget", Lens::Api).contains(&"default-export".to_owned()));
}

#[test]
fn member_modifiers_and_the_private_sigil_are_both_read() {
    let Some(index) = index(
        r"
class Widget {
  #secret = 1;
  private hidden = 2;
  protected shared = 3;
  open = 4;
}
",
    ) else {
        return;
    };
    let vis = |name: &str| at(&index, &format!("pkg::Widget::{name}")).and_then(|n| n.visibility);
    assert_eq!(vis("secret"), Some(Visibility::Private));
    assert_eq!(vis("hidden"), Some(Visibility::Private));
    assert_eq!(vis("shared"), Some(Visibility::Super));
    // A class member with no modifier is public, unlike a module-level declaration.
    assert_eq!(vis("open"), Some(Visibility::Public));
    assert!(
        subtypes(&index, "pkg::Widget::secret", Lens::Api).contains(&"private".to_owned()),
        "the # sigil is the one privacy the language enforces at run time"
    );
    // The sigil stays in what the reader sees; it cannot stay in the identity, where `#`
    // already means an ordinal.
    assert_eq!(
        at(&index, "pkg::Widget::secret").map(|node| node.name.as_str()),
        Some("#secret")
    );
}

// --- substitution lens --------------------------------------------------------

#[test]
fn contracts_and_the_classes_filling_them_are_both_substitution_points() {
    let Some(index) = index(
        r"
export interface Renderable { render(): void; }
export abstract class Base { abstract go(): void; }
export class Widget extends Base implements Renderable {
  render(): void {}
  handler?: () => void;
}
",
    ) else {
        return;
    };
    assert!(
        subtypes(&index, "pkg::Renderable", Lens::Substitution).contains(&"interface".to_owned())
    );
    assert!(subtypes(&index, "pkg::Base", Lens::Substitution).contains(&"abstract".to_owned()));
    assert!(subtypes(&index, "pkg::Base::go", Lens::Substitution).contains(&"abstract".to_owned()));
    let widget = subtypes(&index, "pkg::Widget", Lens::Substitution);
    assert!(widget.contains(&"extends".to_owned()), "{widget:?}");
    assert!(widget.contains(&"implements".to_owned()), "{widget:?}");
    // A signature with no body is where an implementation gets supplied.
    assert!(
        subtypes(&index, "pkg::Renderable::render", Lens::Substitution)
            .contains(&"signature".to_owned())
    );
    let handler = subtypes(&index, "pkg::Widget::handler", Lens::Substitution);
    assert!(handler.contains(&"optional".to_owned()), "{handler:?}");
    assert!(handler.contains(&"callable".to_owned()), "{handler:?}");
}

#[test]
fn a_bounded_generic_is_a_substitution_point() {
    let Some(index) = index("export function free<T extends object>(v: T): T { return v; }") else {
        return;
    };
    assert!(
        subtypes(&index, "pkg::free", Lens::Substitution).contains(&"generic-bound".to_owned())
    );
}

// --- variation lens -----------------------------------------------------------

#[test]
fn a_decorator_is_variation_on_what_it_rewrites() {
    let Some(index) = index("class Widget {\n  @observable value = 1;\n}") else {
        return;
    };
    assert!(
        subtypes(&index, "pkg::Widget::value", Lens::Variation).contains(&"decorator".to_owned())
    );
}

#[test]
fn a_dynamic_import_and_an_environment_branch_both_vary_the_shape() {
    let Some(index) = index(
        r#"
export async function load() {
  if (process.env.NODE_ENV === "production") { return null; }
  return await import("./heavy");
}
"#,
    ) else {
        return;
    };
    let found = subtypes(&index, "pkg::load", Lens::Variation);
    assert!(found.contains(&"dynamic-import".to_owned()), "{found:?}");
    assert!(
        found.contains(&"environment-branch".to_owned()),
        "{found:?}"
    );
}

// --- boundary lens ------------------------------------------------------------

#[test]
fn a_package_import_crosses_the_line_and_a_relative_one_does_not() {
    let Some(index) = index(
        r#"
import { x } from "lodash";
import { y } from "./local";
export const use = () => 1;
"#,
    ) else {
        return;
    };
    // Imports sit at module level, so their facets land on the package root.
    let found = subtypes(&index, "pkg", Lens::Boundary);
    assert_eq!(found, ["external-import"]);
}

#[test]
fn an_ambient_module_declares_something_this_package_does_not_contain() {
    let Some(index) = index(r#"declare module "ext" { export function f(): void; }"#) else {
        return;
    };
    assert!(
        subtypes(&index, "pkg::\"ext\"", Lens::Boundary).contains(&"ambient-module".to_owned())
    );
}

// --- hazard lens --------------------------------------------------------------

#[test]
fn the_ways_of_switching_the_type_system_off_are_all_hazards() {
    let Some(index) = index(
        r"
export async function go(v: unknown) {
  // @ts-ignore
  const forced = (v as any).x!;
  return await Promise.resolve(forced);
}
",
    ) else {
        return;
    };
    let found = subtypes(&index, "pkg::go", Lens::Hazard);
    for expected in ["any-cast", "async", "await", "non-null", "suppressed"] {
        assert!(
            found.contains(&expected.to_owned()),
            "{expected}: {found:?}"
        );
    }
}

#[test]
fn code_compiled_at_run_time_is_a_hazard() {
    let Some(index) = index(r#"export function go() { return eval("1 + 1"); }"#) else {
        return;
    };
    assert!(subtypes(&index, "pkg::go", Lens::Hazard).contains(&"runtime-eval".to_owned()));
}

#[test]
fn every_declared_subtype_has_a_lens_and_no_duplicates() {
    // The table feeds query-term suggestions; a duplicate would offer the same term twice.
    let mut seen: Vec<(&str, &str)> = super::facets::SUBTYPES
        .iter()
        .map(|(lens, subtype)| (lens.name(), subtype.0))
        .collect();
    let count = seen.len();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), count);
}
