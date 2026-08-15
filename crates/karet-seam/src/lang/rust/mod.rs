//! The Rust mapping: constructs to universal kinds, and constructs to lens facets.
//!
//! Written against the real `tree-sitter-rust` grammar, whose shape drives two decisions
//! worth stating up front.
//!
//! Attributes are **siblings** of the item they decorate, not children — `#[cfg(unix)]`
//! parses as an `attribute_item` followed by a `function_item`. So attribute pairing is
//! the extractor's job, and arrives here already resolved on [`FacetContext`].
//!
//! Sub-item constructs — an `unsafe` block, an await point, a `dyn` type in a signature —
//! are not entities and never become nodes. They are reported through
//! [`interior_facets`](SeamLanguage::interior_facets) and attributed to the nearest
//! enclosing entity as *sites*, which is what keeps containment a tree.

use karet_treesitter::LanguageId;
use karet_treesitter::WalkNode;
use karet_treesitter::language_id_from_injection_name;

use super::Classified;
use super::FacetContext;
use super::SeamLanguage;
use crate::edge::EdgeKind;
use crate::model::Facet;
use crate::model::FacetSubtype;
use crate::model::Lens;
use crate::model::NodeKind;
use crate::model::Visibility;

mod api;
mod boundary;
mod hazard;
mod substitution;
mod variation;

#[cfg(test)]
mod tests;

pub use api::REEXPORT;

/// The Rust language mapping.
#[derive(Debug, Default, Clone, Copy)]
pub struct Rust;

/// The shared instance, registered by [`super::for_language`].
#[must_use]
pub fn mapping() -> &'static dyn SeamLanguage {
    &Rust
}

/// The grammar id for Rust, when the grammar is compiled in.
#[must_use]
pub fn language_id() -> Option<LanguageId> {
    language_id_from_injection_name("rust")
}

/// Every facet subtype the Rust mapping can emit.
const SUBTYPES: &[(Lens, FacetSubtype)] = &[
    (Lens::Api, api::PUB),
    (Lens::Api, api::CRATE),
    (Lens::Api, api::SUPER),
    (Lens::Api, api::IN),
    (Lens::Api, api::PRIVATE),
    (Lens::Api, api::REEXPORT),
    (Lens::Substitution, substitution::TRAIT),
    (Lens::Substitution, substitution::IMPL),
    (Lens::Substitution, substitution::BLANKET_IMPL),
    (Lens::Substitution, substitution::DEFAULT_METHOD),
    (Lens::Substitution, substitution::DYN),
    (Lens::Substitution, substitution::IMPL_TRAIT),
    (Lens::Substitution, substitution::GENERIC_BOUND),
    (Lens::Substitution, substitution::ASSOC_TYPE),
    (Lens::Substitution, substitution::FN_PTR),
    (Lens::Substitution, substitution::BOXED_CLOSURE),
    (Lens::Variation, variation::CFG),
    (Lens::Variation, variation::CFG_ATTR),
    (Lens::Variation, variation::FEATURE),
    (Lens::Variation, variation::MACRO_DEF),
    (Lens::Variation, variation::MACRO_RULES),
    (Lens::Variation, variation::PROC_MACRO),
    (Lens::Variation, variation::MACRO_CALL),
    (Lens::Variation, variation::DERIVE),
    (Lens::Variation, variation::ATTR_MACRO),
    (Lens::Variation, variation::BUILD_SCRIPT),
    (Lens::Variation, variation::INCLUDE),
    (Lens::Boundary, boundary::EXTERN_BLOCK),
    (Lens::Boundary, boundary::EXTERN_FN),
    (Lens::Boundary, boundary::NO_MANGLE),
    (Lens::Boundary, boundary::EXPORT_NAME),
    (Lens::Boundary, boundary::LINK),
    (Lens::Boundary, boundary::LINK_NAME),
    (Lens::Boundary, boundary::ENTRY_POINT),
    (Lens::Boundary, boundary::EXTERNAL_CRATE_USE),
    (Lens::Hazard, hazard::UNSAFE),
    (Lens::Hazard, hazard::ASYNC),
    (Lens::Hazard, hazard::AWAIT),
    (Lens::Hazard, hazard::SEND_BOUND),
    (Lens::Hazard, hazard::SYNC_BOUND),
];

/// Edge kinds a Rust language server can resolve for us.
const SEMANTIC: &[EdgeKind] = &[
    EdgeKind::Implements,
    EdgeKind::OverridesDefault,
    EdgeKind::ReExports,
];

impl SeamLanguage for Rust {
    fn language(&self) -> LanguageId {
        language_id().unwrap_or(LanguageId(u16::MAX))
    }

    fn classify(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Option<Classified> {
        classify_node(node, ctx)
    }

    fn facets_of(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Vec<Facet> {
        let mut facets = Vec::new();
        api::facets(node, ctx, &mut facets);
        substitution::facets(node, ctx, &mut facets);
        variation::facets(node, ctx, &mut facets);
        boundary::facets(node, ctx, &mut facets);
        hazard::facets(node, ctx, &mut facets);
        facets
    }

    fn interior_facets(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Vec<Facet> {
        let mut facets = Vec::new();
        substitution::interior_facets(node, ctx, &mut facets);
        variation::interior_facets(node, ctx, &mut facets);
        hazard::interior_facets(node, ctx, &mut facets);
        api::interior_facets(node, ctx, &mut facets);
        facets
    }

    fn is_container(&self, node: &WalkNode<'_>) -> bool {
        matches!(
            node.kind(),
            "source_file"
                | "mod_item"
                | "trait_item"
                | "impl_item"
                | "foreign_mod_item"
                | "struct_item"
                | "enum_item"
                | "union_item"
        )
    }

    fn external_module(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Option<String> {
        // `mod net { … }` carries its body; `mod net;` does not, and that missing body is
        // the whole signal that another file holds it.
        if node.kind() != "mod_item" || node.child_span("body").is_some() {
            return None;
        }
        node.child_text("name", ctx.text).map(str::to_owned)
    }

    fn semantic_capabilities(&self) -> &'static [EdgeKind] {
        SEMANTIC
    }

    fn subtypes(&self) -> &'static [(Lens, FacetSubtype)] {
        SUBTYPES
    }
}

/// Map one node onto an addressable entity, or `None` when it is not one.
fn classify_node(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Option<Classified> {
    let kind = node.kind();
    let visibility = declared_visibility(node);
    let named = |universal: NodeKind| -> Option<Classified> {
        let name = node.child_text("name", ctx.text)?.to_owned();
        let selection = node
            .child_span("name")
            .map_or_else(|| ctx.range(node.span()), |span| ctx.range(span));
        Some(Classified {
            kind: universal,
            segment: name.clone(),
            name,
            detail: None,
            selection,
            visibility,
        })
    };

    match kind {
        "mod_item" => named(NodeKind::Module),
        "struct_item" | "enum_item" | "union_item" => named(NodeKind::Type),
        "type_item" => named(if ctx.container == Some(NodeKind::Implementation) {
            NodeKind::Member
        } else {
            NodeKind::Type
        }),
        "trait_item" => named(NodeKind::Interface),
        "impl_item" => Some(classify_impl(node, ctx, visibility)),
        "function_item" | "function_signature_item" => named(function_kind(ctx)),
        "field_declaration" | "enum_variant" | "associated_type" => named(NodeKind::Member),
        "const_item" | "static_item" => named(NodeKind::Constant),
        "macro_definition" => named(NodeKind::MacroDef),
        "foreign_mod_item" => Some(classify_foreign(node, ctx, visibility)),
        _ => None,
    }
}

/// A function is a free function at module level and a member anywhere else.
fn function_kind(ctx: &FacetContext<'_>) -> NodeKind {
    match ctx.container {
        Some(NodeKind::Interface | NodeKind::Implementation | NodeKind::ForeignBlock) => {
            NodeKind::Member
        },
        _ => NodeKind::Function,
    }
}

/// Describe an `impl` block, which has no name of its own.
///
/// The segment names what the block *binds* — `{impl Display for Widget}` — because that
/// is what stays stable across edits. Generic parameters are excluded: adding a bound is
/// not a rename, and must not cost the reader their place.
fn classify_impl(
    node: &WalkNode<'_>,
    ctx: &FacetContext<'_>,
    visibility: Option<Visibility>,
) -> Classified {
    let self_type = node
        .child_text("type", ctx.text)
        .unwrap_or("?")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let trait_name = node
        .child_text("trait", ctx.text)
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "));
    let (segment, name) = match &trait_name {
        Some(bound) => (
            format!("{{impl {bound} for {self_type}}}"),
            format!("impl {bound} for {self_type}"),
        ),
        None => (format!("{{impl {self_type}}}"), format!("impl {self_type}")),
    };
    let selection = node
        .child_span("trait")
        .or_else(|| node.child_span("type"))
        .map_or_else(|| ctx.range(node.span()), |span| ctx.range(span));
    Classified {
        kind: NodeKind::Implementation,
        name,
        segment,
        detail: trait_name,
        selection,
        visibility,
    }
}

/// Describe an `extern` block, which also has no name of its own.
fn classify_foreign(
    node: &WalkNode<'_>,
    ctx: &FacetContext<'_>,
    visibility: Option<Visibility>,
) -> Classified {
    let abi = node
        .children()
        .find(|child| child.kind() == "extern_modifier")
        .and_then(|child| child.text(ctx.text))
        .unwrap_or("extern")
        .to_owned();
    Classified {
        kind: NodeKind::ForeignBlock,
        name: abi.clone(),
        segment: format!("{{{abi}}}"),
        detail: Some(abi),
        selection: ctx.range(node.span()),
        visibility,
    }
}

/// Read a node's `visibility_modifier`, mapping it onto the neutral levels.
///
/// An item with no modifier is private, which is a fact rather than an absence — the
/// `api` lens reports it so "nothing is exposed here" is visible at a glance.
pub(crate) fn declared_visibility(node: &WalkNode<'_>) -> Option<Visibility> {
    if !is_visibility_bearing(node.kind()) {
        return None;
    }
    let Some(modifier) = node
        .children()
        .find(|child| child.kind() == "visibility_modifier")
    else {
        return Some(Visibility::Private);
    };
    // `pub` alone has no child; the restricted forms carry one naming the scope.
    let restriction = modifier.children().find(|child| {
        matches!(
            child.kind(),
            "crate" | "super" | "self" | "scoped_identifier" | "identifier"
        )
    });
    Some(match restriction.as_ref().map(WalkNode::kind) {
        None => Visibility::Public,
        Some("crate") => Visibility::Crate,
        Some("super") => Visibility::Super,
        Some("self") => Visibility::Private,
        Some(_) => Visibility::Restricted,
    })
}

/// Whether a construct can carry a visibility modifier at all.
fn is_visibility_bearing(kind: &str) -> bool {
    matches!(
        kind,
        "mod_item"
            | "struct_item"
            | "enum_item"
            | "union_item"
            | "type_item"
            | "trait_item"
            | "function_item"
            | "function_signature_item"
            | "const_item"
            | "static_item"
            | "field_declaration"
            | "use_declaration"
            | "foreign_mod_item"
            | "macro_definition"
    )
}

/// The type parameter names declared on a node, for blanket-impl detection.
pub(crate) fn type_parameter_names(node: &WalkNode<'_>, text: &str) -> Vec<String> {
    let Some(parameters) = node
        .children()
        .find(|child| child.kind() == "type_parameters")
    else {
        return Vec::new();
    };
    parameters
        .children()
        .filter(|child| {
            child.kind() == "type_parameter" || child.kind() == "constrained_type_parameter"
        })
        .filter_map(|child| {
            child
                .child_text("name", text)
                .or_else(|| child.child_text("left", text))
                .map(str::to_owned)
        })
        .collect()
}

/// Whether a node carries a modifier token such as `unsafe`, `async`, or `const`.
pub(crate) fn has_modifier(node: &WalkNode<'_>, modifier: &str) -> bool {
    node.children()
        .filter(|child| child.kind() == "function_modifiers")
        .any(|modifiers| modifiers.has_child_kind(modifier))
}
