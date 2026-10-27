//! The Java mapping.
//!
//! Every Java declaration lives in a type, so containment is the type tree: classes,
//! records and enums are types, interfaces and annotation types are contracts, and
//! methods, constructors and fields are their members. A file is a module, as it is for
//! Kotlin, which shares Gradle's source sets with it.
//!
//! This mapping reads **containment and visibility only**; the other four lenses are
//! declared unread through [`SeamLanguage::unread_lenses`], so the view says `?` rather
//! than claiming Java has no substitution seams.

use karet_treesitter::LanguageId;
use karet_treesitter::WalkNode;

use super::Classified;
use super::FacetContext;
use super::SeamLanguage;
use crate::edge::EdgeKind;
use crate::model::Facet;
use crate::model::FacetSubtype;
use crate::model::Lens;
use crate::model::NodeKind;
use crate::model::Visibility;

#[cfg(test)]
mod tests;

/// The Java mapping.
#[derive(Debug, Default, Clone, Copy)]
pub struct Java;

/// The shared instance, registered by [`super::for_language`].
#[must_use]
pub fn mapping() -> &'static dyn SeamLanguage {
    &Java
}

/// The grammar this mapping serves, when compiled in.
#[must_use]
pub fn language_id() -> Option<LanguageId> {
    karet_treesitter::languages::JAVA
}

/// Declared `public`.
pub const PUBLIC: FacetSubtype = FacetSubtype("public");
/// Declared `protected`.
pub const PROTECTED: FacetSubtype = FacetSubtype("protected");
/// Declared `private`.
pub const PRIVATE: FacetSubtype = FacetSubtype("private");
/// No modifier: visible within the package only.
pub const PACKAGE_PRIVATE: FacetSubtype = FacetSubtype("package-private");

const SUBTYPES: &[(Lens, FacetSubtype)] = &[
    (Lens::Api, PUBLIC),
    (Lens::Api, PROTECTED),
    (Lens::Api, PRIVATE),
    (Lens::Api, PACKAGE_PRIVATE),
];

const UNREAD: &[Lens] = &[
    Lens::Substitution,
    Lens::Variation,
    Lens::Boundary,
    Lens::Hazard,
];

impl SeamLanguage for Java {
    fn language(&self) -> Option<LanguageId> {
        language_id()
    }

    fn classify(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Option<Classified> {
        let kind = match node.kind() {
            "class_declaration" | "record_declaration" | "enum_declaration" => NodeKind::Type,
            "interface_declaration" | "annotation_type_declaration" => NodeKind::Interface,
            "method_declaration" | "constructor_declaration" | "enum_constant" => NodeKind::Member,
            "field_declaration" | "constant_declaration" => {
                let declarator = node
                    .children()
                    .find(|child| child.field_name() == Some("declarator"))?;
                let name = declarator.child_text("name", ctx.text)?.trim().to_owned();
                return Some(classified(node, ctx, NodeKind::Member, name));
            },
            _ => return None,
        };
        let name = node.child_text("name", ctx.text)?.trim().to_owned();
        let mut out = classified(node, ctx, kind, name);
        // What a type extends and implements, so a call through a contract can tell its
        // implementations from same-named methods on unrelated types.
        if matches!(kind, NodeKind::Type | NodeKind::Interface) {
            let heritage: Vec<&str> = node
                .children()
                .filter(|child| {
                    matches!(
                        child.kind(),
                        "superclass" | "super_interfaces" | "extends_interfaces"
                    )
                })
                .filter_map(|child| child.text(ctx.text))
                .map(str::trim)
                .collect();
            out.detail = (!heritage.is_empty()).then(|| heritage.join(" "));
        }
        Some(out)
    }

    fn facets_of(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Vec<Facet> {
        let subtype = match visibility(node, ctx) {
            Visibility::Public => PUBLIC,
            Visibility::Super => PROTECTED,
            Visibility::Private => PRIVATE,
            _ => PACKAGE_PRIVATE,
        };
        vec![Facet::new(Lens::Api, subtype)]
    }

    fn interior_facets(&self, _: &WalkNode<'_>, _: &FacetContext<'_>) -> Vec<Facet> {
        Vec::new()
    }

    fn is_container(&self, node: &WalkNode<'_>) -> bool {
        matches!(
            node.kind(),
            "program"
                | "class_declaration"
                | "record_declaration"
                | "enum_declaration"
                | "interface_declaration"
        )
    }

    fn call_query(&self, _: LanguageId) -> Option<&'static str> {
        Some(include_str!("../../../queries/calls/java.scm"))
    }

    fn implicit_receiver(&self) -> bool {
        true
    }

    fn unread_lenses(&self) -> &'static [Lens] {
        UNREAD
    }

    fn semantic_capabilities(&self) -> &'static [EdgeKind] {
        &[]
    }

    fn subtypes(&self) -> &'static [(Lens, FacetSubtype)] {
        SUBTYPES
    }
}

fn classified(
    node: &WalkNode<'_>,
    ctx: &FacetContext<'_>,
    kind: NodeKind,
    name: String,
) -> Classified {
    let selection = node
        .child_span("name")
        .map_or_else(|| ctx.range(node.span()), |span| ctx.range(span));
    Classified {
        kind,
        segment: name.clone(),
        name,
        detail: None,
        selection,
        visibility: Some(visibility(node, ctx)),
    }
}

/// The declared visibility: a modifier, or the default where none is written.
///
/// An interface's members are public without saying so; everything else without a
/// modifier is visible to its package alone.
fn visibility(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Visibility {
    let modifiers = node
        .children()
        .find(|child| child.kind() == "modifiers")
        .and_then(|modifiers| modifiers.text(ctx.text))
        .unwrap_or_default();
    let has = |word: &str| modifiers.split_whitespace().any(|token| token == word);
    if has("public") {
        Visibility::Public
    } else if has("protected") {
        Visibility::Super
    } else if has("private") {
        Visibility::Private
    } else if ctx.container == Some(NodeKind::Interface) || node.kind() == "enum_constant" {
        Visibility::Public
    } else {
        Visibility::Crate
    }
}
