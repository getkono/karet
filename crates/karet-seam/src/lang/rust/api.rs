//! The `api` lens for Rust: what is visible from outside.

use karet_treesitter::WalkNode;

use crate::lang::FacetContext;
use crate::model::Facet;
use crate::model::FacetSubtype;
use crate::model::Lens;
use crate::model::Visibility;

/// `pub` — visible outside the package.
pub const PUB: FacetSubtype = FacetSubtype("pub");
/// `pub(crate)` — visible throughout the package only.
pub const CRATE: FacetSubtype = FacetSubtype("crate");
/// `pub(super)` — visible to the parent module.
pub const SUPER: FacetSubtype = FacetSubtype("super");
/// `pub(in path)` — visible within a named subtree.
pub const IN: FacetSubtype = FacetSubtype("in");
/// No modifier — visible only within its own module.
pub const PRIVATE: FacetSubtype = FacetSubtype("private");
/// `pub use` — a name republished under another path.
pub const REEXPORT: FacetSubtype = FacetSubtype("reexport");

/// The subtype naming a declared visibility level.
#[must_use]
pub fn subtype_for(visibility: Visibility) -> FacetSubtype {
    match visibility {
        Visibility::Public => PUB,
        Visibility::Crate => CRATE,
        Visibility::Super => SUPER,
        Visibility::Restricted => IN,
        Visibility::Private => PRIVATE,
    }
}

/// Facets for an addressable entity.
pub fn facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    let Some(visibility) = super::declared_visibility(node) else {
        return;
    };
    // A `pub use` is a re-export, reported on the module that performs it rather than as
    // a declaration of its own — see `interior_facets`.
    if node.kind() == "use_declaration" {
        return;
    }
    let detail = node
        .children()
        .find(|child| child.kind() == "visibility_modifier")
        .and_then(|child| child.text(ctx.text))
        .map(str::to_owned);
    let mut facet = Facet::new(Lens::Api, subtype_for(visibility));
    if let Some(detail) = detail {
        facet = facet.with_detail(detail);
    }
    out.push(facet);
}

/// Facets contributed by nodes that are not entities: re-exports.
///
/// A `pub use` republishes someone else's name, so it is not a declaration and must not
/// become a row in the tree. It attaches to the module performing the re-export, where
/// "this module widens the reach of three things" is the fact worth seeing.
pub fn interior_facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    if node.kind() != "use_declaration" {
        return;
    }
    // A private `use` is an import, not a re-export — it changes nothing about reach.
    if super::declared_visibility(node) == Some(Visibility::Private) {
        return;
    }
    let Some(argument) = node.child_text("argument", ctx.text) else {
        return;
    };
    out.push(
        Facet::new(Lens::Api, REEXPORT)
            .with_detail(argument.split_whitespace().collect::<Vec<_>>().join(" "))
            .with_sites(vec![ctx.range(node.span())]),
    );
}
