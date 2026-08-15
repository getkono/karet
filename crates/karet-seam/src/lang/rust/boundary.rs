//! The `boundary` lens for Rust: what crosses the package line.

use karet_treesitter::WalkNode;

use crate::lang::FacetContext;
use crate::model::Facet;
use crate::model::FacetSubtype;
use crate::model::Lens;
use crate::model::NodeKind;

/// An `extern` block declaring foreign items.
pub const EXTERN_BLOCK: FacetSubtype = FacetSubtype("extern-block");
/// A function declared or defined with a foreign ABI.
pub const EXTERN_FN: FacetSubtype = FacetSubtype("extern-fn");
/// `#[no_mangle]` — the symbol keeps its written name.
pub const NO_MANGLE: FacetSubtype = FacetSubtype("no-mangle");
/// `#[export_name = "…"]` — the symbol is exported under a chosen name.
pub const EXPORT_NAME: FacetSubtype = FacetSubtype("export-name");
/// `#[link(…)]` — a native library this package links against.
pub const LINK: FacetSubtype = FacetSubtype("link");
/// `#[link_name = "…"]` — the foreign symbol this item binds to.
pub const LINK_NAME: FacetSubtype = FacetSubtype("link-name");
/// A build-target entry point.
pub const ENTRY_POINT: FacetSubtype = FacetSubtype("entry-point");
/// A module naming a crate from outside this package.
pub const EXTERNAL_CRATE_USE: FacetSubtype = FacetSubtype("external-crate-use");

/// Attribute names that describe a symbol crossing the boundary.
const BOUNDARY_ATTRIBUTES: &[(&str, FacetSubtype)] = &[
    ("no_mangle", NO_MANGLE),
    ("export_name", EXPORT_NAME),
    ("link", LINK),
    ("link_name", LINK_NAME),
];

/// Facets for an addressable entity.
pub fn facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    for (name, subtype) in BOUNDARY_ATTRIBUTES {
        for attribute in ctx.attributes_named(name) {
            let mut facet = Facet::new(Lens::Boundary, *subtype).with_sites(vec![attribute.range]);
            if let Some(arguments) = &attribute.arguments {
                facet = facet.with_detail(arguments.clone());
            }
            out.push(facet);
        }
    }

    match node.kind() {
        "foreign_mod_item" => {
            let abi = node
                .children()
                .find(|child| child.kind() == "extern_modifier")
                .and_then(|child| child.text(ctx.text))
                .unwrap_or("extern");
            out.push(Facet::new(Lens::Boundary, EXTERN_BLOCK).with_detail(abi.to_owned()));
        },
        "function_item" | "function_signature_item" => {
            // A foreign ABI on the function itself, or membership of an extern block:
            // either way this symbol is reachable from outside Rust.
            if let Some(abi) = foreign_abi(node, ctx) {
                out.push(Facet::new(Lens::Boundary, EXTERN_FN).with_detail(abi));
            } else if ctx.container == Some(NodeKind::ForeignBlock) {
                out.push(Facet::new(Lens::Boundary, EXTERN_FN).with_detail("foreign declaration"));
            }
            if is_entry_point(node, ctx) {
                out.push(Facet::new(Lens::Boundary, ENTRY_POINT).with_detail("main"));
            }
        },
        _ => {},
    }
}

/// The ABI string on a function's modifiers, when it declares one.
fn foreign_abi(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Option<String> {
    node.children()
        .filter(|child| child.kind() == "function_modifiers")
        .flat_map(|modifiers| modifiers.children().collect::<Vec<_>>())
        .find(|child| child.kind() == "extern_modifier")
        .and_then(|child| child.text(ctx.text))
        .map(str::to_owned)
}

/// Whether this is a binary target's entry point.
///
/// Only a top-level `fn main` qualifies: a `main` method on a type is an ordinary
/// member, and calling it an entry point would be wrong.
fn is_entry_point(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> bool {
    node.child_text("name", ctx.text) == Some("main")
        && matches!(ctx.container, None | Some(NodeKind::Package))
}
