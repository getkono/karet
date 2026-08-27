//! The Swift mapping.
//!
//! Swift is the second proof of the ownership hook, and a strict one: an `extension` is
//! Rust's `impl` in a language that shares nothing else with it. `extension Widget { … }`
//! holds members of a type declared elsewhere, `extension Widget: Codable { … }` binds a
//! protocol to it, and both are written wherever their author found convenient. The same
//! two candidates serve both languages, which is the point of the hook being neutral.
//!
//! Three shapes of this grammar drive the rest.
//!
//! **One node kind carries `struct`, `class`, `enum` and `extension`.** They are all
//! `class_declaration`, and the keyword that tells them apart is an anonymous token the
//! walk never visits. So the keyword is read from the text between the modifiers and the
//! name — exact, since that is the only thing that can be there.
//!
//! **`#if` is a flat sibling, not a wrapper.** `#if os(iOS)` and `#endif` parse as
//! `directive` nodes beside the declarations they gate rather than around them, so
//! neither containment nor the extractor's decoration pairing can say what a declaration
//! is gated by. Reading the directives that are still open above it can, and does.
//!
//! **An enum case has no node of its own.** `case red, green` gives one `enum_entry` with
//! two names in it, so the names are classified individually — told apart from a method's
//! name, which is the same node kind in the same field, by a member frame already being
//! on the stack when the walk reaches it.

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
mod ownership;

#[cfg(test)]
mod tests;

pub use ownership::open_condition;

/// The Swift language mapping.
#[derive(Debug, Default, Clone, Copy)]
pub struct Swift;

/// The shared instance, registered by [`super::for_language`].
#[must_use]
pub fn mapping() -> &'static dyn SeamLanguage {
    &Swift
}

/// The grammar id for Swift, when the grammar is compiled in.
#[must_use]
pub fn language_id() -> Option<LanguageId> {
    language_id_from_injection_name("swift")
}

/// Nothing is resolved semantically here yet; edges stay unresolved and the view degrades.
const SEMANTIC: &[EdgeKind] = &[];

impl SeamLanguage for Swift {
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
        ownership::owners(node, ctx)
    }

    fn is_container(&self, node: &WalkNode<'_>) -> bool {
        matches!(
            node.kind(),
            "source_file" | "class_declaration" | "protocol_declaration"
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
        "protocol_declaration" => named(node, ctx, NodeKind::Interface),
        "class_declaration" if keyword(node, ctx) == "extension" => {
            Some(ownership::classify_extension(node, ctx))
        },
        "class_declaration" => named(node, ctx, NodeKind::Type),
        "typealias_declaration" => named(node, ctx, NodeKind::Type),
        "associatedtype_declaration" | "protocol_function_declaration" => {
            named(node, ctx, NodeKind::Member)
        },
        "function_declaration" => named(
            node,
            ctx,
            if member_of_type {
                NodeKind::Member
            } else {
                NodeKind::Function
            },
        ),
        // A local `let` inside a function body is not an entity; a stored property is.
        "property_declaration" if member_of_type => named(node, ctx, NodeKind::Member),
        "property_declaration"
            if matches!(ctx.container, Some(NodeKind::Package | NodeKind::Module)) =>
        {
            named(node, ctx, NodeKind::Constant)
        },
        "init_declaration" => here(node, ctx, NodeKind::Member, "init"),
        "deinit_declaration" => here(node, ctx, NodeKind::Member, "deinit"),
        "subscript_declaration" => here(node, ctx, NodeKind::Member, "subscript"),
        // One `case red, green` holds two members and wraps neither.
        "simple_identifier"
            if node.field_name() == Some("name") && ctx.container == Some(NodeKind::Type) =>
        {
            let name = node.text(ctx.text)?.to_owned();
            Some(build(
                node,
                ctx,
                NodeKind::Member,
                name,
                ctx.range(node.span()),
            ))
        },
        _ => None,
    }
}

/// The keyword introducing a declaration — `struct`, `class`, `enum`, `extension`, `actor`.
///
/// Read from the text between the modifiers and the name, which is the only place it can
/// be: the token itself is anonymous, so the walk never offers it.
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
        .and_then(|between| between.split_whitespace().next())
        .unwrap_or_default()
        .to_owned()
}

/// Build a [`Classified`] from the node's `name` field.
fn named(node: &WalkNode<'_>, ctx: &FacetContext<'_>, kind: NodeKind) -> Option<Classified> {
    let name = node.child_text("name", ctx.text)?.trim().to_owned();
    let selection = node
        .child_span("name")
        .map_or_else(|| ctx.range(node.span()), |span| ctx.range(span));
    Some(build(node, ctx, kind, name, selection))
}

/// Build a [`Classified`] for a construct whose name is its keyword.
fn here(
    node: &WalkNode<'_>,
    ctx: &FacetContext<'_>,
    kind: NodeKind,
    name: &str,
) -> Option<Classified> {
    Some(build(
        node,
        ctx,
        kind,
        name.to_owned(),
        ctx.range(node.span()),
    ))
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

/// Swift's declared visibility, mapped onto the neutral levels.
///
/// The default is `internal` — visible throughout the module and no further — so an
/// unmarked declaration is [`Visibility::Crate`] rather than private. `open` and `public`
/// differ only in whether subclassing is allowed, which the substitution lens reports;
/// both are equally reachable.
#[must_use]
pub fn visibility_of(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Visibility {
    match modifier(node, ctx, "visibility_modifier").as_deref() {
        Some("open" | "public") => Visibility::Public,
        Some("private") => Visibility::Private,
        Some("fileprivate") => Visibility::Restricted,
        _ => Visibility::Crate,
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

/// Every attribute name on a declaration, without its `@` or its arguments.
#[must_use]
pub fn attributes(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Vec<(String, karet_core::Range)> {
    let Some(modifiers) = node.children().find(|child| child.kind() == "modifiers") else {
        return Vec::new();
    };
    modifiers
        .children()
        .filter(|child| child.kind() == "attribute")
        .filter_map(|child| {
            let text = child.text(ctx.text)?;
            let name = text
                .trim_start_matches('@')
                .split(['(', ' '])
                .next()
                .unwrap_or_default()
                .to_owned();
            Some((name, ctx.range(child.span())))
        })
        .collect()
}
