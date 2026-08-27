//! The Kotlin mapping.
//!
//! Kotlin's version of "written away from what it belongs to" is not a block but a single
//! declaration: `fun Widget.render()` is a member of `Widget` written at the top level of
//! whatever file its author chose. So the ownership hook applies to the member itself
//! rather than to a container around it, which is the shape it was designed to allow and
//! the reason it takes a node rather than a block.
//!
//! Three shapes of this grammar drive the rest.
//!
//! **One node kind carries `class`, `interface`, `enum class` and `data class`,** with the
//! keyword an anonymous token the walk never visits — the same situation Swift is in, and
//! it is read the same way, from the text between the modifiers and the name.
//!
//! **A receiver and a return type are the same node kind.** `fun Widget.extra(): Int` has
//! two `user_type` children and neither is in a field of its own; what separates them is
//! that the receiver is written before the name and the return type after it.
//!
//! **A companion object is transparent.** Its members are members of the class in every
//! way that matters to a reader, and it is already written inside that class, so it is
//! simply not classified and its contents attach to the class directly. No regrouping is
//! needed for the one case that looks like it would need it.

use karet_treesitter::LanguageId;
use karet_treesitter::WalkNode;
use karet_treesitter::language_id_from_injection_name;

use super::Classified;
use super::FacetContext;
use super::Owner;
use super::SeamLanguage;
use crate::edge::EdgeKind;
use crate::model::Facet;
use crate::model::FacetSubtype;
use crate::model::Lens;
use crate::model::NodeKind;
use crate::model::Visibility;

mod facets;

#[cfg(test)]
mod tests;

/// The Kotlin language mapping.
#[derive(Debug, Default, Clone, Copy)]
pub struct Kotlin;

/// The shared instance, registered by [`super::for_language`].
#[must_use]
pub fn mapping() -> &'static dyn SeamLanguage {
    &Kotlin
}

/// The grammar id for Kotlin, when the grammar is compiled in.
#[must_use]
pub fn language_id() -> Option<LanguageId> {
    language_id_from_injection_name("kotlin")
}

/// Nothing is resolved semantically here yet; edges stay unresolved and the view degrades.
const SEMANTIC: &[EdgeKind] = &[];

impl SeamLanguage for Kotlin {
    fn language(&self) -> LanguageId {
        language_id().unwrap_or(LanguageId(u16::MAX))
    }

    fn classify(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Option<Classified> {
        classify_node(node, ctx)
    }

    fn facets_of(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Vec<Facet> {
        let mut out = Vec::new();
        facets::entity_facets(node, ctx, &mut out);
        out
    }

    fn interior_facets(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Vec<Facet> {
        let mut out = Vec::new();
        facets::interior_facets(node, ctx, &mut out);
        out
    }

    fn ownership(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Vec<Owner> {
        // The receiver of an extension declaration, which is the only thing in this
        // language written somewhere other than where it belongs.
        match receiver(node, ctx).as_deref().and_then(base_name) {
            Some(base) => vec![Owner::nested(base)],
            None => Vec::new(),
        }
    }

    fn is_container(&self, node: &WalkNode<'_>) -> bool {
        matches!(
            node.kind(),
            "source_file" | "class_declaration" | "object_declaration"
        )
    }

    fn semantic_capabilities(&self) -> &'static [EdgeKind] {
        SEMANTIC
    }

    fn subtypes(&self) -> &'static [(Lens, FacetSubtype)] {
        facets::SUBTYPES
    }
}

/// Map one node onto an addressable entity.
fn classify_node(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Option<Classified> {
    let member_of_type = matches!(
        ctx.container,
        Some(NodeKind::Type | NodeKind::Interface | NodeKind::Implementation)
    );
    match node.kind() {
        "class_declaration" if keyword(node, ctx) == "interface" => {
            named(node, ctx, NodeKind::Interface)
        },
        // An `object` is a type with exactly one instance, which is still a type.
        "class_declaration" | "object_declaration" => named(node, ctx, NodeKind::Type),
        "type_alias" => alias(node, ctx),
        "function_declaration" => named(
            node,
            ctx,
            // An extension function is a member of its receiver wherever it is written;
            // the regroup pass is what puts it there.
            if member_of_type || receiver(node, ctx).is_some() {
                NodeKind::Member
            } else {
                NodeKind::Function
            },
        ),
        // `val id: Int` inside a class, `val Widget.area` at the top level, and
        // `const val LIMIT` in a companion are all members of something. A `val` in a
        // function body is a local and not an entity.
        "property_declaration" => {
            let extension = receiver(node, ctx).is_some();
            let kind = if member_of_type || extension {
                NodeKind::Member
            } else if matches!(ctx.container, Some(NodeKind::Package | NodeKind::Module)) {
                NodeKind::Constant
            } else {
                return None;
            };
            binding(node, ctx, kind)
        },
        // A primary-constructor `val` declares a property as surely as a body one does.
        "class_parameter" if member_of_type => first_identifier(node, ctx, NodeKind::Member),
        "enum_entry" => first_identifier(node, ctx, NodeKind::Member),
        _ => None,
    }
}

/// The keyword introducing a declaration — `class`, `interface`, `object`, `enum`.
///
/// Read from the text between the modifiers and the name: the token itself is anonymous,
/// so the walk never offers it.
#[must_use]
pub fn keyword(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> String {
    let from = node
        .children()
        .find(|child| child.kind() == "modifiers")
        .map_or(node.span().start.0, |modifiers| modifiers.span().end.0);
    let to = node
        .child_span("name")
        .map_or(node.span().end.0, |name| name.start.0);
    ctx.text
        .get(from..to.max(from))
        .and_then(|between| between.split_whitespace().next_back())
        .unwrap_or_default()
        .to_owned()
}

/// The receiver type of an extension declaration, when this node is one.
///
/// A receiver and a return type are both bare `user_type` children; what separates them is
/// that the receiver is written *before* the declaration's name and the return type after.
#[must_use]
pub fn receiver(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Option<String> {
    if !matches!(node.kind(), "function_declaration" | "property_declaration") {
        return None;
    }
    let anchor = node
        .child_span("name")
        .map(|span| span.start.0)
        .or_else(|| {
            node.children()
                .find(|child| child.kind() == "variable_declaration")
                .map(|child| child.span().start.0)
        })?;
    node.children()
        .find(|child| child.kind() == "user_type" && child.span().start.0 < anchor)
        .and_then(|child| child.text(ctx.text))
        .map(str::to_owned)
}

/// The bare type name a type expression is about, or `None` when it is not about one.
///
/// Shallow by the same choice the Rust and Swift mappings make: generic arguments and
/// qualification are dropped, and anything with no single name at its head is refused.
#[must_use]
pub fn base_name(text: &str) -> Option<String> {
    let head = text
        .trim()
        .split(['<', ' ', '?'])
        .next()
        .unwrap_or_default();
    let head = head.rsplit('.').next().unwrap_or_default().trim();
    let mut chars = head.chars();
    let first = chars.next()?;
    if !(first.is_alphabetic() || first == '_') || !chars.all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    Some(head.to_owned())
}

/// Build a [`Classified`] from the node's `name` field.
fn named(node: &WalkNode<'_>, ctx: &FacetContext<'_>, kind: NodeKind) -> Option<Classified> {
    let name = node.child_text("name", ctx.text)?.trim().to_owned();
    let selection = node
        .child_span("name")
        .map_or_else(|| ctx.range(node.span()), |span| ctx.range(span));
    Some(build(node, ctx, kind, name, selection))
}

/// A `typealias`, whose name sits in a `type` field rather than a `name` one.
fn alias(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Option<Classified> {
    let name = node.child_text("type", ctx.text)?.trim().to_owned();
    let selection = node
        .child_span("type")
        .map_or_else(|| ctx.range(node.span()), |span| ctx.range(span));
    Some(build(node, ctx, NodeKind::Type, name, selection))
}

/// A property, whose name sits inside its `variable_declaration`.
fn binding(node: &WalkNode<'_>, ctx: &FacetContext<'_>, kind: NodeKind) -> Option<Classified> {
    let declaration = node
        .children()
        .find(|child| child.kind() == "variable_declaration")?;
    first_identifier(&declaration, ctx, kind).map(|classified| Classified {
        // The property carries the modifiers and the annotations, so visibility is read
        // from it rather than from the binding inside it.
        visibility: Some(visibility_of(node, ctx)),
        ..classified
    })
}

/// Build a [`Classified`] from the node's first bare `identifier` child.
fn first_identifier(
    node: &WalkNode<'_>,
    ctx: &FacetContext<'_>,
    kind: NodeKind,
) -> Option<Classified> {
    let identifier = node.children().find(|child| child.kind() == "identifier")?;
    let name = identifier.text(ctx.text)?.to_owned();
    Some(build(node, ctx, kind, name, ctx.range(identifier.span())))
}

/// Assemble a classification, resolving declared visibility.
fn build(
    node: &WalkNode<'_>,
    ctx: &FacetContext<'_>,
    kind: NodeKind,
    name: String,
    selection: karet_core::Range,
) -> Classified {
    Classified {
        kind,
        segment: name.clone(),
        visibility: Some(visibility_of(node, ctx)),
        name,
        detail: None,
        selection,
    }
}

/// Kotlin's declared visibility, mapped onto the neutral levels.
///
/// The default is `public`, which is the opposite of Rust's and worth stating: an unmarked
/// Kotlin declaration is part of the module's surface, not hidden from it.
#[must_use]
pub fn visibility_of(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Visibility {
    match modifier(node, ctx, "visibility_modifier").as_deref() {
        Some("private") => Visibility::Private,
        Some("protected") => Visibility::Super,
        Some("internal") => Visibility::Crate,
        _ => Visibility::Public,
    }
}

/// The text of a modifier of the given kind, if the declaration carries one.
#[must_use]
pub fn modifier(node: &WalkNode<'_>, ctx: &FacetContext<'_>, kind: &str) -> Option<String> {
    node.children()
        .find(|child| child.kind() == "modifiers")?
        .children()
        .find(|child| child.kind() == kind)
        .and_then(|child| child.text(ctx.text))
        .map(str::to_owned)
}

/// Every modifier of the given kind, for the kinds a declaration may repeat.
#[must_use]
pub fn modifiers<'a>(node: &WalkNode<'_>, ctx: &FacetContext<'a>, kind: &str) -> Vec<&'a str> {
    let Some(modifiers) = node.children().find(|child| child.kind() == "modifiers") else {
        return Vec::new();
    };
    modifiers
        .children()
        .filter(|child| child.kind() == kind)
        .filter_map(|child| child.text(ctx.text))
        .collect()
}
