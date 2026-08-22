//! The `variation` lens for Rust: what changes shape before compiling.

use karet_treesitter::WalkNode;

use crate::lang::FacetContext;
use crate::model::Facet;
use crate::model::FacetSubtype;
use crate::model::Lens;

/// A `#[cfg(…)]` gate.
pub const CFG: FacetSubtype = FacetSubtype("cfg");
/// A `#[cfg_attr(…)]` conditional attribute.
pub const CFG_ATTR: FacetSubtype = FacetSubtype("cfg-attr");
/// A gate that turns on a Cargo feature specifically.
pub const FEATURE: FacetSubtype = FacetSubtype("feature");
/// A macro definition of any flavour.
pub const MACRO_DEF: FacetSubtype = FacetSubtype("macro-def");
/// A `macro_rules!` definition.
pub const MACRO_RULES: FacetSubtype = FacetSubtype("macro-rules");
/// A procedural-macro entry point.
pub const PROC_MACRO: FacetSubtype = FacetSubtype("proc-macro");
/// An invocation of a macro defined in this workspace.
pub const MACRO_CALL: FacetSubtype = FacetSubtype("macro-call");
/// A `#[derive(…)]`.
pub const DERIVE: FacetSubtype = FacetSubtype("derive");
/// An attribute macro applied to an item.
pub const ATTR_MACRO: FacetSubtype = FacetSubtype("attr-macro");
/// A build script, which generates code before compiling.
pub const BUILD_SCRIPT: FacetSubtype = FacetSubtype("build-script");
/// An `include!`-family invocation pulling in generated source.
pub const INCLUDE: FacetSubtype = FacetSubtype("include");

/// Attributes that are neither built-in nor `derive`, hence attribute macros.
const BUILTIN_ATTRIBUTES: &[&str] = &[
    "cfg",
    "cfg_attr",
    "derive",
    "doc",
    "allow",
    "warn",
    "deny",
    "forbid",
    "must_use",
    "inline",
    "repr",
    "non_exhaustive",
    "deprecated",
    "test",
    "ignore",
    "should_panic",
    "no_mangle",
    "export_name",
    "link",
    "link_name",
    "used",
    "path",
    "macro_export",
    "macro_use",
    "track_caller",
    "cold",
    "unsafe",
];

/// Facets for an addressable entity.
pub fn facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    attribute_facets(ctx, out);
    if node.kind() == "macro_definition" {
        out.push(Facet::new(Lens::Variation, MACRO_DEF));
        out.push(Facet::new(Lens::Variation, MACRO_RULES));
    }
    // A proc-macro entry point is an ordinary function wearing one of three attributes.
    if matches!(node.kind(), "function_item")
        && let Some(attr) = ctx.attributes.iter().find(|attr| {
            matches!(
                attr.name.as_str(),
                "proc_macro" | "proc_macro_derive" | "proc_macro_attribute"
            )
        })
    {
        out.push(Facet::new(Lens::Variation, PROC_MACRO).with_detail(attr.name.clone()));
        out.push(Facet::new(Lens::Variation, MACRO_DEF));
    }
}

/// Turn the attributes decorating an item into variation facets.
fn attribute_facets(ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    for attribute in ctx.attributes {
        match attribute.name.as_str() {
            "cfg" => {
                let predicate = attribute.args().to_owned();
                // A feature gate is the variation a reader most often wants to isolate,
                // so it is reported as its own subtype *in addition to* the generic gate.
                if predicate.contains("feature") {
                    out.push(
                        Facet::new(Lens::Variation, FEATURE)
                            .with_detail(predicate.clone())
                            .with_sites(vec![attribute.range]),
                    );
                }
                out.push(
                    Facet::new(Lens::Variation, CFG)
                        .with_detail(predicate)
                        .with_sites(vec![attribute.range]),
                );
            },
            "cfg_attr" => out.push(
                Facet::new(Lens::Variation, CFG_ATTR)
                    .with_detail(attribute.args().to_owned())
                    .with_sites(vec![attribute.range]),
            ),
            "derive" => out.push(
                Facet::new(Lens::Variation, DERIVE)
                    .with_detail(attribute.args().to_owned())
                    .with_sites(vec![attribute.range]),
            ),
            name if !BUILTIN_ATTRIBUTES.contains(&name) => out.push(
                Facet::new(Lens::Variation, ATTR_MACRO)
                    .with_detail(name.to_owned())
                    .with_sites(vec![attribute.range]),
            ),
            _ => {},
        }
    }
}

/// Facets contributed by non-entity nodes: macro invocation sites.
///
/// Every invocation is emitted here with the macro name in `detail`; the index prunes
/// those naming macros defined outside the workspace once every file has been seen. A
/// single-file pass cannot know which names are local, so the filtering cannot happen yet.
pub fn interior_facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    if !matches!(node.kind(), "macro_invocation") {
        return;
    }
    let Some(name) = node.child_text("macro", ctx.text) else {
        return;
    };
    let subtype = if name.starts_with("include") {
        INCLUDE
    } else {
        MACRO_CALL
    };
    out.push(
        Facet::new(Lens::Variation, subtype)
            .with_detail(name.to_owned())
            .with_sites(vec![ctx.range(node.span())]),
    );
}
