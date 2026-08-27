//! The TypeScript and JavaScript mapping.
//!
//! One mapping for three grammars — JavaScript, TypeScript, TSX — because they are one
//! language with a type system bolted on at increasing strength, and every construct the
//! weaker grammars have the stronger ones have too. A mapping per grammar would be three
//! copies of the same table drifting apart.
//!
//! Two shapes of this grammar drive the decisions here.
//!
//! **`export` is a parent, not a sibling.** `export class Widget {}` parses as an
//! `export_statement` *wrapping* the class, so the class node cannot see it — and the
//! extractor's decoration pairing, which matches siblings, does not reach it either.
//! Since visibility in this language is entirely a question of whether a declaration is
//! exported, that fact has to be recovered from the text immediately before the node,
//! which is exactly where the keyword is.
//!
//! **Decorators are children, not siblings.** Rust writes `#[cfg]` beside the item it
//! decorates and Python wraps both in one node, so the extractor's sibling pairing serves
//! them; TypeScript hangs the decorator *inside* the declaration, in a field of its own.
//! The shared rule would therefore pair it with the declaration's own name, so this
//! mapping declines the hook and reads its decorators from the node directly.
//!
//! **Enum members are bare identifiers.** `enum Colour { Red }` gives `Red` no wrapping
//! node at all, unlike every other member in the language. It is told apart from the name
//! of a method — the same node kind, in the same field — by what encloses it: a method's
//! name sits inside the method, so the walk has already pushed a member frame by then.

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

/// The TypeScript/JavaScript language mapping.
#[derive(Debug, Default, Clone, Copy)]
pub struct TypeScript;

/// The shared instance, registered by [`super::for_language`].
#[must_use]
pub fn mapping() -> &'static dyn SeamLanguage {
    &TypeScript
}

/// Every grammar id this mapping serves, when compiled in.
#[must_use]
pub fn language_ids() -> Vec<LanguageId> {
    ["javascript", "typescript", "tsx"]
        .into_iter()
        .filter_map(language_id_from_injection_name)
        .collect()
}

/// Nothing is resolved semantically here yet; edges stay unresolved and the view degrades.
const SEMANTIC: &[EdgeKind] = &[];

impl SeamLanguage for TypeScript {
    fn language(&self) -> LanguageId {
        language_ids()
            .first()
            .copied()
            .unwrap_or(LanguageId(u16::MAX))
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
            "program"
                | "class_declaration"
                | "abstract_class_declaration"
                | "interface_declaration"
                | "enum_declaration"
                | "internal_module"
                | "module"
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
    match node.kind() {
        // An abstract class states a contract its subclasses fill, which is the same
        // distinction `interface` draws — and the same one Python draws with `ABC`.
        "abstract_class_declaration" => named(node, ctx, NodeKind::Interface),
        "interface_declaration" => named(node, ctx, NodeKind::Interface),
        "class_declaration" | "type_alias_declaration" | "enum_declaration" => {
            named(node, ctx, NodeKind::Type)
        },
        // `namespace X {}` and the ambient `declare module "x" {}`.
        "internal_module" | "module" => named(node, ctx, NodeKind::Module),
        "function_declaration" | "generator_function_declaration" | "function_signature" => {
            named(node, ctx, NodeKind::Function)
        },
        "method_definition"
        | "method_signature"
        | "abstract_method_signature"
        | "public_field_definition"
        | "property_signature"
        | "enum_assignment" => named(node, ctx, NodeKind::Member),
        // A bare enum member: the one construct in the language with no wrapping node.
        "property_identifier"
            if node.field_name() == Some("name") && ctx.container == Some(NodeKind::Type) =>
        {
            named_here(node, ctx, NodeKind::Member)
        },
        // `const make = () => …` is a function by every measure that matters; `const K = 1`
        // is a constant. A declarator inside a function body is a local and not an entity.
        "variable_declarator" => match ctx.container {
            Some(NodeKind::Module | NodeKind::Package) => named(node, ctx, declarator_kind(node)),
            _ => None,
        },
        "assignment_expression" if ownership::prototype_target(node, ctx).is_some() => {
            ownership::prototype_member(node, ctx)
        },
        _ => None,
    }
}

/// Whether a `variable_declarator` binds a function or a value.
fn declarator_kind(node: &WalkNode<'_>) -> NodeKind {
    let bound = node
        .children()
        .find(|child| child.field_name() == Some("value"));
    match bound.as_ref().map(WalkNode::kind) {
        Some("arrow_function" | "function_expression" | "generator_function") => NodeKind::Function,
        _ => NodeKind::Constant,
    }
}

/// The path segment a name takes.
///
/// `#` marks an ordinal in the path model, so a `#private` member cannot wear its own name
/// as a segment without producing an identity that will not parse back. The sigil is
/// dropped from the segment and kept in the display name, which is what the reader sees;
/// a same-named public member is then disambiguated by the ordinal, as any collision is.
fn segment_for(name: &str) -> String {
    name.trim_start_matches('#').to_owned()
}

/// Build a [`Classified`] from the node's `name` field.
fn named(node: &WalkNode<'_>, ctx: &FacetContext<'_>, kind: NodeKind) -> Option<Classified> {
    let name = node.child_text("name", ctx.text)?.to_owned();
    let selection = node
        .child_span("name")
        .map_or_else(|| ctx.range(node.span()), |span| ctx.range(span));
    Some(build(node, ctx, kind, name, selection))
}

/// Build a [`Classified`] from the node's own text, for a construct that *is* its name.
fn named_here(node: &WalkNode<'_>, ctx: &FacetContext<'_>, kind: NodeKind) -> Option<Classified> {
    let name = node.text(ctx.text)?.to_owned();
    Some(build(node, ctx, kind, name, ctx.range(node.span())))
}

/// Assemble a classification, resolving the visibility this language expresses.
fn build(
    node: &WalkNode<'_>,
    ctx: &FacetContext<'_>,
    kind: NodeKind,
    name: String,
    selection: karet_core::Range,
) -> Classified {
    Classified {
        kind,
        segment: segment_for(&name),
        visibility: Some(visibility_of(node, ctx, &name)),
        name,
        detail: None,
        selection,
    }
}

/// How far a declaration is visible.
///
/// Three sources, in the order they override each other: an explicit member modifier, the
/// `#private` sigil, and — for anything at module level — whether it is exported. A
/// declaration nobody exported is reachable only from its own file, which is what
/// [`Visibility::Private`] means here.
#[must_use]
pub fn visibility_of(node: &WalkNode<'_>, ctx: &FacetContext<'_>, name: &str) -> Visibility {
    if name.starts_with('#') {
        return Visibility::Private;
    }
    if let Some(modifier) = accessibility(node, ctx) {
        return match modifier {
            "private" => Visibility::Private,
            "protected" => Visibility::Super,
            _ => Visibility::Public,
        };
    }
    // A class member with no modifier is public; a module-level declaration is only as
    // visible as its export makes it.
    if matches!(
        ctx.container,
        Some(NodeKind::Type | NodeKind::Interface | NodeKind::Implementation)
    ) {
        return Visibility::Public;
    }
    if exported(node, ctx) {
        Visibility::Public
    } else {
        Visibility::Private
    }
}

/// The `public`/`private`/`protected` modifier on a class member, if it carries one.
#[must_use]
pub fn accessibility<'a>(node: &WalkNode<'_>, ctx: &FacetContext<'a>) -> Option<&'a str> {
    node.children()
        .find(|child| child.kind() == "accessibility_modifier")
        .and_then(|child| child.text(ctx.text))
}

/// Whether `export` immediately precedes this declaration.
///
/// Read from the text rather than the tree because `export` wraps a declaration rather
/// than decorating it, and a node cannot see its own parent. The keyword is always the
/// token immediately before, so this is exact rather than a search.
#[must_use]
pub fn exported(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> bool {
    preceded_by(node, ctx, &["export", "default"])
}

/// Whether the tokens immediately before `node` are any of `keywords`, in any order.
#[must_use]
pub fn preceded_by(node: &WalkNode<'_>, ctx: &FacetContext<'_>, keywords: &[&str]) -> bool {
    let Some(before) = ctx.text.get(..node.span().start.0) else {
        return false;
    };
    let mut rest = before.trim_end();
    // `export default async function …`: skip back over whatever modifiers sit between.
    for _ in 0..4 {
        let Some(last) = rest.split_whitespace().next_back() else {
            return false;
        };
        if keywords.contains(&last) {
            return true;
        }
        if !matches!(
            last,
            "async" | "abstract" | "declare" | "static" | "readonly"
        ) {
            return false;
        }
        rest = rest[..rest.len() - last.len()].trim_end();
    }
    false
}
