//! The Python mapping — and the conformance test for the language contract.
//!
//! Python was chosen as the second language precisely because it shares almost nothing
//! with Rust structurally. It has no visibility keyword, no `cfg`, no monomorphized
//! generics, and no module-per-file rule. If the lens vocabulary survives that, it is
//! genuinely language-neutral rather than Rust's vocabulary with the serial numbers
//! filed off.
//!
//! It survives, and the mapping needed no change to the view, the query language, the
//! lens set, or the model. It did force one honest generalization: reading a decoration
//! used to be hardcoded to Rust's `attribute_item`, and is now something a language
//! supplies. The *pairing* rule turned out to be shared — Python's `@decorator` and the
//! definition it decorates are siblings inside a wrapping node, exactly as a Rust
//! attribute and its item are siblings in a block.
//!
//! Where Rust states a fact, Python states a convention, and the mapping says so rather
//! than pretending otherwise: a leading underscore is privacy by agreement, not by
//! enforcement, so it maps to [`Visibility::Private`] with the convention named in the
//! facet's detail.

use karet_treesitter::LanguageId;
use karet_treesitter::WalkNode;
use karet_treesitter::language_id_from_injection_name;

use super::Attribute;
use super::Classified;
use super::FacetContext;
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

/// The Python language mapping.
#[derive(Debug, Default, Clone, Copy)]
pub struct Python;

/// The shared instance, registered by [`super::for_language`].
#[must_use]
pub fn mapping() -> &'static dyn SeamLanguage {
    &Python
}

/// The grammar id for Python, when the grammar is compiled in.
#[must_use]
pub fn language_id() -> Option<LanguageId> {
    language_id_from_injection_name("python")
}

/// Python resolves nothing semantically here yet; edges stay unresolved and the view
/// degrades rather than failing, exactly as the contract allows.
const SEMANTIC: &[EdgeKind] = &[];

impl SeamLanguage for Python {
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

    fn is_container(&self, node: &WalkNode<'_>) -> bool {
        matches!(
            node.kind(),
            "module" | "class_definition" | "function_definition"
        )
    }

    fn decoration(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Option<Attribute> {
        if node.kind() != "decorator" {
            return None;
        }
        // `@app.route("/x")` — the name is everything up to the call, the arguments
        // whatever it was called with.
        let text = node.text(ctx.text)?.trim_start_matches('@').trim();
        let (name, arguments) = match text.split_once('(') {
            Some((head, rest)) => (
                head.trim().to_owned(),
                Some(rest.trim_end_matches(')').trim().to_owned()),
            ),
            None => (text.to_owned(), None),
        };
        Some(Attribute {
            name,
            arguments,
            range: ctx.range(node.span()),
        })
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
        "class_definition" => {
            let name = node.child_text("name", ctx.text)?.to_owned();
            // A class deriving Protocol or ABC is a contract, not a concrete type — the
            // same distinction Rust draws with `trait` versus `struct`.
            let kind = if facets::is_contract(node, ctx) {
                NodeKind::Interface
            } else {
                NodeKind::Type
            };
            Some(classified(node, ctx, kind, name))
        },
        "function_definition" => {
            let name = node.child_text("name", ctx.text)?.to_owned();
            let kind = match ctx.container {
                Some(NodeKind::Type | NodeKind::Interface) => NodeKind::Member,
                _ => NodeKind::Function,
            };
            Some(classified(node, ctx, kind, name))
        },
        // A module-level `NAME = value` in screaming case is a constant by convention;
        // inside a class it is an attribute, which is a member either way.
        "assignment" => {
            let name = node.child_text("left", ctx.text)?.to_owned();
            if !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                return None;
            }
            let kind = match ctx.container {
                Some(NodeKind::Type | NodeKind::Interface) => NodeKind::Member,
                // `__all__` is the one place Python states its public surface outright,
                // so it is worth a row even though it is not screaming-case.
                _ if name == "__all__" => NodeKind::Constant,
                _ if name.to_uppercase() == name && name.chars().any(char::is_alphabetic) => {
                    NodeKind::Constant
                },
                _ => return None,
            };
            Some(classified(node, ctx, kind, name))
        },
        _ => None,
    }
}

/// Build a [`Classified`] with the visibility Python's naming convention implies.
fn classified(
    node: &WalkNode<'_>,
    ctx: &FacetContext<'_>,
    kind: NodeKind,
    name: String,
) -> Classified {
    let selection = node
        .child_span("name")
        .or_else(|| node.child_span("left"))
        .map_or_else(|| ctx.range(node.span()), |span| ctx.range(span));
    Classified {
        kind,
        segment: name.clone(),
        visibility: Some(visibility_of(&name)),
        name,
        detail: None,
        selection,
    }
}

/// Python's visibility, which is a convention rather than a rule.
///
/// A single leading underscore asks callers to stay out; a dunder is a special method and
/// part of the public protocol despite its underscores. Nothing here is enforced by the
/// language, and the `api` facet's detail says so rather than implying otherwise.
#[must_use]
pub fn visibility_of(name: &str) -> Visibility {
    if name.starts_with("__") && name.ends_with("__") {
        return Visibility::Public;
    }
    if name.starts_with('_') {
        return Visibility::Private;
    }
    Visibility::Public
}
