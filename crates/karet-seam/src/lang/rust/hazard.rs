//! The `hazard` lens for Rust: where substitution is dangerous.
//!
//! This lens is deliberately narrower than the draft specification proposed. Lock
//! acquisitions and task spawns are *not* syntactically decidable — `.lock()` is a method
//! name anyone may use, and `spawn` could be any function. Reporting them from a
//! name-matching guess would mean the lens sometimes lies, and a lens that sometimes lies
//! destroys the one thing this view promises: that absence of evidence and evidence of
//! absence stay distinguishable.
//!
//! So the structural tier reports only what the grammar decides outright — `unsafe`,
//! `async`, await points, and the auto-trait bounds. Locks and spawns are left to the
//! semantic tier, which has the type information to be right.

use karet_treesitter::WalkNode;

use crate::lang::FacetContext;
use crate::model::Facet;
use crate::model::FacetSubtype;
use crate::model::Lens;

/// An `unsafe` item or block.
pub const UNSAFE: FacetSubtype = FacetSubtype("unsafe");
/// An `async` item.
pub const ASYNC: FacetSubtype = FacetSubtype("async");
/// An await point, where execution may suspend.
pub const AWAIT: FacetSubtype = FacetSubtype("await");
/// A `Send` bound, constraining what may cross a thread.
pub const SEND_BOUND: FacetSubtype = FacetSubtype("send-bound");
/// A `Sync` bound, constraining what may be shared between threads.
pub const SYNC_BOUND: FacetSubtype = FacetSubtype("sync-bound");

/// Facets for an addressable entity.
pub fn facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    if super::has_modifier(node, "unsafe") {
        out.push(Facet::new(Lens::Hazard, UNSAFE).with_detail("unsafe fn"));
    }
    if super::has_modifier(node, "async") {
        out.push(Facet::new(Lens::Hazard, ASYNC).with_detail("async fn"));
    }
    // An `unsafe trait` / `unsafe impl` carries the token directly rather than inside a
    // modifiers node, so the token scan is separate from `has_modifier`.
    if matches!(node.kind(), "trait_item" | "impl_item" | "foreign_mod_item")
        && node.has_child_kind("unsafe")
    {
        out.push(Facet::new(Lens::Hazard, UNSAFE).with_detail(node.kind().replace('_', " ")));
    }
    auto_trait_bounds(node, ctx, out);
}

/// Report `Send` and `Sync` bounds wherever they are written on this declaration.
fn auto_trait_bounds(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    let mut send = Vec::new();
    let mut sync = Vec::new();
    collect_bound_names(node, ctx, &mut send, &mut sync);
    if !send.is_empty() {
        out.push(Facet::new(Lens::Hazard, SEND_BOUND).with_sites(send));
    }
    if !sync.is_empty() {
        out.push(Facet::new(Lens::Hazard, SYNC_BOUND).with_sites(sync));
    }
}

/// Walk the declaration's bound nodes, recording where `Send` and `Sync` appear.
///
/// The body is deliberately not descended into: a bound written inside a nested closure
/// belongs to that closure, not to this declaration.
fn collect_bound_names(
    node: &WalkNode<'_>,
    ctx: &FacetContext<'_>,
    send: &mut Vec<karet_core::Range>,
    sync: &mut Vec<karet_core::Range>,
) {
    for child in node.children() {
        if child.field_name() == Some("body") {
            continue;
        }
        if child.kind() == "type_identifier"
            && let Some(text) = child.text(ctx.text)
        {
            match text {
                "Send" => send.push(ctx.range(child.span())),
                "Sync" => sync.push(ctx.range(child.span())),
                _ => {},
            }
        }
        collect_bound_names(&child, ctx, send, sync);
    }
}

/// Facets contributed by non-entity nodes, attributed to the enclosing entity.
pub fn interior_facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    match node.kind() {
        "unsafe_block" => out.push(
            Facet::new(Lens::Hazard, UNSAFE)
                .with_detail("unsafe block")
                .with_sites(vec![ctx.range(node.span())]),
        ),
        "await_expression" => out.push(
            Facet::new(Lens::Hazard, AWAIT)
                .with_detail(node.text(ctx.text).unwrap_or(".await").to_owned())
                .with_sites(vec![ctx.range(node.span())]),
        ),
        _ => {},
    }
}
