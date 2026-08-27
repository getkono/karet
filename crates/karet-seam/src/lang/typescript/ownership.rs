//! Members written onto a class rather than inside it.
//!
//! `Widget.prototype.render = function () { … }` is a method of `Widget` by every measure
//! except where it sits in the file, and it is how the language wrote methods before it
//! had a syntax for them. It is still how a library adds one to a class it does not own.
//!
//! Only the `prototype` form is read. `Widget.render = …` is indistinguishable from
//! setting a property on any object — a configuration bag, a namespace, a module's
//! exports — and treating every such assignment as a method would fill types with
//! properties that are nothing of the kind. `prototype` is unambiguous, so `prototype` is
//! the line.

use karet_treesitter::WalkNode;

use super::Classified;
use super::FacetContext;
use super::Owner;
use crate::lang::Owner as OwnerType;
use crate::model::NodeKind;

/// The owner candidates for one node, most specific first.
pub(super) fn owners(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Vec<Owner> {
    match prototype_target(node, ctx) {
        Some((owner, _)) => vec![OwnerType::nested(owner)],
        None => Vec::new(),
    }
}

/// The class and member names in `X.prototype.y = …`, when this node is one.
pub(super) fn prototype_target(
    node: &WalkNode<'_>,
    ctx: &FacetContext<'_>,
) -> Option<(String, String)> {
    if node.kind() != "assignment_expression" {
        return None;
    }
    let left = node.children().find(|c| c.field_name() == Some("left"))?;
    if left.kind() != "member_expression" {
        return None;
    }
    let member = left.child_text("property", ctx.text)?.to_owned();
    let object = left.children().find(|c| c.field_name() == Some("object"))?;
    if object.kind() != "member_expression"
        || object.child_text("property", ctx.text) != Some("prototype")
    {
        return None;
    }
    // The class itself must be a plain name. `a.b.prototype.c` names something this has
    // no way to resolve, and guessing at `b` would be inventing a type.
    let owner = object
        .children()
        .find(|c| c.field_name() == Some("object"))?;
    if owner.kind() != "identifier" {
        return None;
    }
    Some((owner.text(ctx.text)?.to_owned(), member))
}

/// Classify `X.prototype.y = …` as a member named `y`.
pub(super) fn prototype_member(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Option<Classified> {
    let (_, member) = prototype_target(node, ctx)?;
    let left = node.children().find(|c| c.field_name() == Some("left"))?;
    Some(Classified {
        kind: NodeKind::Member,
        segment: member.clone(),
        name: member,
        detail: None,
        selection: ctx.range(left.span()),
        // Assigned onto the prototype, which is the public face of the class.
        visibility: Some(crate::model::Visibility::Public),
    })
}
