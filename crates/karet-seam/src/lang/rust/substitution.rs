//! The `substitution` lens for Rust: what behavior can be swapped.

use karet_treesitter::WalkNode;

use crate::lang::FacetContext;
use crate::model::Facet;
use crate::model::FacetSubtype;
use crate::model::Lens;
use crate::model::NodeKind;

/// A trait definition — the contract itself.
pub const TRAIT: FacetSubtype = FacetSubtype("trait");
/// An implementation binding a contract to a type.
pub const IMPL: FacetSubtype = FacetSubtype("impl");
/// An implementation covering every type satisfying a bound.
pub const BLANKET_IMPL: FacetSubtype = FacetSubtype("blanket-impl");
/// A trait method with a body, which an implementor may replace.
pub const DEFAULT_METHOD: FacetSubtype = FacetSubtype("default-method");
/// A `dyn Trait` — dispatch chosen at run time.
pub const DYN: FacetSubtype = FacetSubtype("dyn");
/// An `impl Trait` — an opaque type the caller cannot name.
pub const IMPL_TRAIT: FacetSubtype = FacetSubtype("impl-trait");
/// A generic bound constraining a type parameter.
pub const GENERIC_BOUND: FacetSubtype = FacetSubtype("generic-bound");
/// An associated type an implementor chooses.
pub const ASSOC_TYPE: FacetSubtype = FacetSubtype("assoc-type");
/// A function-pointer field or parameter.
pub const FN_PTR: FacetSubtype = FacetSubtype("fn-ptr");
/// A boxed closure field or parameter.
pub const BOXED_CLOSURE: FacetSubtype = FacetSubtype("boxed-closure");

/// Facets for an addressable entity.
pub fn facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    match node.kind() {
        "trait_item" => {
            let mut facet = Facet::new(Lens::Substitution, TRAIT);
            if let Some(bounds) = node.child_text("bounds", ctx.text) {
                facet = facet.with_detail(format!("supertraits{bounds}"));
            }
            out.push(facet);
        },
        "impl_item" => out.push(impl_facet(node, ctx)),
        "function_item" if ctx.container == Some(NodeKind::Interface) => {
            // A trait method *with a body* is a default an implementor may replace; the
            // bodiless `function_signature_item` is a requirement, not a substitution point.
            out.push(Facet::new(Lens::Substitution, DEFAULT_METHOD));
        },
        "associated_type" => out.push(Facet::new(Lens::Substitution, ASSOC_TYPE)),
        _ => {},
    }
    bound_facets(node, ctx, out);
}

/// Classify an `impl` block, distinguishing a blanket implementation.
///
/// A blanket impl is one whose self type *is* one of its own type parameters —
/// `impl<T: Display> MyTrait for T`. That is only decidable with the block's parameters
/// in hand, which is why the check lives here rather than in a query.
fn impl_facet(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Facet {
    let parameters = super::type_parameter_names(node, ctx.text);
    let self_type = node.child_text("type", ctx.text).unwrap_or_default();
    let blanket = parameters.iter().any(|param| param == self_type);
    let subtype = if blanket { BLANKET_IMPL } else { IMPL };
    let facet = Facet::new(Lens::Substitution, subtype);
    match node.child_text("trait", ctx.text) {
        Some(bound) => facet.with_detail(bound.to_owned()),
        None => facet.with_detail("inherent"),
    }
}

/// A generic bound anywhere on the declaration is a substitution point: it names the
/// behaviour a caller may supply.
fn bound_facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    if !matches!(
        node.kind(),
        "function_item"
            | "function_signature_item"
            | "struct_item"
            | "enum_item"
            | "union_item"
            | "trait_item"
            | "impl_item"
            | "type_item"
    ) {
        return;
    }
    let mut bounds = Vec::new();
    if let Some(parameters) = node
        .children()
        .find(|child| child.kind() == "type_parameters")
    {
        collect_bounds(&parameters, ctx, &mut bounds);
    }
    if let Some(clause) = node.children().find(|child| child.kind() == "where_clause") {
        collect_bounds(&clause, ctx, &mut bounds);
    }
    if bounds.is_empty() {
        return;
    }
    let sites = bounds.iter().map(|(_, range)| *range).collect();
    let detail = bounds
        .iter()
        .map(|(text, _)| text.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    out.push(
        Facet::new(Lens::Substitution, GENERIC_BOUND)
            .with_detail(detail)
            .with_sites(sites),
    );
}

/// Gather every `trait_bounds` node beneath `node`.
fn collect_bounds(
    node: &WalkNode<'_>,
    ctx: &FacetContext<'_>,
    out: &mut Vec<(String, karet_core::Range)>,
) {
    for child in node.children() {
        if child.kind() == "trait_bounds" {
            let text = child
                .text(ctx.text)
                .unwrap_or_default()
                .trim_start_matches(':')
                .trim()
                .to_owned();
            if !text.is_empty() {
                out.push((text, ctx.range(child.span())));
            }
        }
        collect_bounds(&child, ctx, out);
    }
}

/// Facets contributed by non-entity nodes, attributed to the enclosing entity.
///
/// A `dyn Trait` in a signature is a substitution point but not a declaration, so it is a
/// site on the function rather than a row of its own.
pub fn interior_facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    let site = || vec![ctx.range(node.span())];
    match node.kind() {
        "dynamic_type" => {
            let detail = node.text(ctx.text).unwrap_or("dyn").to_owned();
            // `Box<dyn Fn(..)>` and friends are a swappable behaviour slot, not merely a
            // trait object, so they get their own subtype.
            let subtype = if is_closure_trait(node, ctx) {
                BOXED_CLOSURE
            } else {
                DYN
            };
            out.push(
                Facet::new(Lens::Substitution, subtype)
                    .with_detail(detail)
                    .with_sites(site()),
            );
        },
        "abstract_type" => out.push(
            Facet::new(Lens::Substitution, IMPL_TRAIT)
                .with_detail(node.text(ctx.text).unwrap_or("impl Trait").to_owned())
                .with_sites(site()),
        ),
        "function_type" => out.push(
            Facet::new(Lens::Substitution, FN_PTR)
                .with_detail(node.text(ctx.text).unwrap_or("fn").to_owned())
                .with_sites(site()),
        ),
        _ => {},
    }
}

/// Whether a `dyn` type names one of the closure traits.
fn is_closure_trait(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> bool {
    node.child_text("trait", ctx.text)
        .is_some_and(|trait_name| {
            let head = trait_name
                .split(['<', '('])
                .next()
                .unwrap_or_default()
                .rsplit("::")
                .next()
                .unwrap_or_default()
                .trim();
            matches!(head, "Fn" | "FnMut" | "FnOnce")
        })
}
