//! The five lenses for Swift.

use karet_treesitter::WalkNode;

use super::FacetContext;
use crate::model::Facet;
use crate::model::FacetSubtype;
use crate::model::Lens;
use crate::model::NodeKind;

// --- api ---------------------------------------------------------------------

/// Visible outside the module, and subclassable there.
pub const OPEN: FacetSubtype = FacetSubtype("open");
/// Visible outside the module.
pub const PUBLIC: FacetSubtype = FacetSubtype("public");
/// Visible throughout the module — Swift's unmarked default.
pub const INTERNAL: FacetSubtype = FacetSubtype("internal");
/// Visible within its own file.
pub const FILEPRIVATE: FacetSubtype = FacetSubtype("fileprivate");
/// Visible within its own declaration.
pub const PRIVATE: FacetSubtype = FacetSubtype("private");

// --- substitution --------------------------------------------------------------

/// A protocol — the contract itself.
pub const PROTOCOL: FacetSubtype = FacetSubtype("protocol");
/// An extension adding members to a type declared elsewhere.
pub const EXTENSION: FacetSubtype = FacetSubtype("extension");
/// A declaration stating it satisfies a protocol.
pub const CONFORMANCE: FacetSubtype = FacetSubtype("conformance");
/// An associated type the conformer chooses.
pub const ASSOC_TYPE: FacetSubtype = FacetSubtype("assoc-type");
/// A `some T` — an opaque type the caller cannot name.
pub const OPAQUE: FacetSubtype = FacetSubtype("opaque-type");
/// An `any T` — a box dispatched at run time.
pub const EXISTENTIAL: FacetSubtype = FacetSubtype("existential");
/// A method in a protocol extension, which a conformer may replace.
pub const PROTOCOL_DEFAULT: FacetSubtype = FacetSubtype("protocol-default");
/// A requirement with no body, which a conformer must supply.
pub const REQUIREMENT: FacetSubtype = FacetSubtype("requirement");
/// A member replacing one it inherited.
pub const OVERRIDE: FacetSubtype = FacetSubtype("override");

// --- variation -----------------------------------------------------------------

/// Compiled only when a condition holds — `#if`.
pub const CONDITION: FacetSubtype = FacetSubtype("compilation-condition");
/// Present only on some platform versions — `@available`.
pub const AVAILABILITY: FacetSubtype = FacetSubtype("availability");
/// A property wrapper, which rewrites every property that uses it.
pub const PROPERTY_WRAPPER: FacetSubtype = FacetSubtype("property-wrapper");
/// An attribute rewriting what it is attached to.
pub const ATTRIBUTE: FacetSubtype = FacetSubtype("attribute");

// --- boundary ------------------------------------------------------------------

/// Callable from C under a chosen symbol — `@_cdecl`.
pub const C_ENTRY: FacetSubtype = FacetSubtype("c-entry");
/// Exposed to the Objective-C runtime.
pub const OBJC: FacetSubtype = FacetSubtype("objc");
/// Bound to a symbol the compiler did not emit — `@_silgen_name`.
pub const SILGEN_NAME: FacetSubtype = FacetSubtype("silgen-name");
/// The program's entry point.
pub const ENTRY_POINT: FacetSubtype = FacetSubtype("entry-point");
/// An import of another module.
pub const MODULE_IMPORT: FacetSubtype = FacetSubtype("module-import");

// --- hazard --------------------------------------------------------------------

/// An `async` function: its caller cannot see when it finishes.
pub const ASYNC: FacetSubtype = FacetSubtype("async");
/// An await point.
pub const AWAIT: FacetSubtype = FacetSubtype("await");
/// A function that can fail in a way the caller must handle.
pub const THROWING: FacetSubtype = FacetSubtype("throwing");
/// A `try!` or `as!`, which trades a handled failure for a crash.
pub const FORCED: FacetSubtype = FacetSubtype("forced");
/// A raw pointer, outside everything the language guarantees.
pub const UNSAFE_POINTER: FacetSubtype = FacetSubtype("unsafe-pointer");
/// A concurrency guarantee asserted rather than checked.
pub const UNCHECKED: FacetSubtype = FacetSubtype("unchecked");

/// Every facet subtype this mapping can emit.
pub const SUBTYPES: &[(Lens, FacetSubtype)] = &[
    (Lens::Api, OPEN),
    (Lens::Api, PUBLIC),
    (Lens::Api, INTERNAL),
    (Lens::Api, FILEPRIVATE),
    (Lens::Api, PRIVATE),
    (Lens::Substitution, PROTOCOL),
    (Lens::Substitution, EXTENSION),
    (Lens::Substitution, CONFORMANCE),
    (Lens::Substitution, ASSOC_TYPE),
    (Lens::Substitution, OPAQUE),
    (Lens::Substitution, EXISTENTIAL),
    (Lens::Substitution, PROTOCOL_DEFAULT),
    (Lens::Substitution, REQUIREMENT),
    (Lens::Substitution, OVERRIDE),
    (Lens::Variation, CONDITION),
    (Lens::Variation, AVAILABILITY),
    (Lens::Variation, PROPERTY_WRAPPER),
    (Lens::Variation, ATTRIBUTE),
    (Lens::Boundary, C_ENTRY),
    (Lens::Boundary, OBJC),
    (Lens::Boundary, SILGEN_NAME),
    (Lens::Boundary, ENTRY_POINT),
    (Lens::Boundary, MODULE_IMPORT),
    (Lens::Hazard, ASYNC),
    (Lens::Hazard, AWAIT),
    (Lens::Hazard, THROWING),
    (Lens::Hazard, FORCED),
    (Lens::Hazard, UNSAFE_POINTER),
    (Lens::Hazard, UNCHECKED),
];

/// Facets for an addressable entity.
pub(super) fn entity_facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    api(node, ctx, out);
    substitution(node, ctx, out);
    variation(node, ctx, out);
    boundary(node, ctx, out);
    hazard(node, ctx, out);
}

/// What is visible from outside.
fn api(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    let declared = super::modifier(node, ctx, "visibility_modifier");
    let (subtype, detail) = match declared.as_deref() {
        Some("open") => (OPEN, "open"),
        Some("public") => (PUBLIC, "public"),
        Some("fileprivate") => (FILEPRIVATE, "fileprivate"),
        Some("private") => (PRIVATE, "private"),
        // Reported rather than omitted: `internal` is Swift's default, and a reader
        // deciding what is exposed needs to see that it was never widened.
        _ => (INTERNAL, "internal (default)"),
    };
    out.push(Facet::new(Lens::Api, subtype).with_detail(detail));
}

/// What behavior can be swapped.
fn substitution(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    match node.kind() {
        "protocol_declaration" => out.push(Facet::new(Lens::Substitution, PROTOCOL)),
        "associatedtype_declaration" => out.push(Facet::new(Lens::Substitution, ASSOC_TYPE)),
        // A protocol requirement has no body; the conformer supplies it.
        "protocol_function_declaration" => out.push(Facet::new(Lens::Substitution, REQUIREMENT)),
        "class_declaration" if super::keyword(node, ctx) == "extension" => {
            let detail = node.child_text("name", ctx.text).unwrap_or_default().trim();
            out.push(Facet::new(Lens::Substitution, EXTENSION).with_detail(detail.to_owned()));
        },
        _ => {},
    }
    for protocol in super::ownership::conformances(node, ctx) {
        out.push(Facet::new(Lens::Substitution, CONFORMANCE).with_detail(protocol));
    }
    if super::modifier(node, ctx, "member_modifier").as_deref() == Some("override") {
        out.push(Facet::new(Lens::Substitution, OVERRIDE));
    }
    // A method written in an extension of a protocol is a default a conformer may replace.
    if node.kind() == "function_declaration" && ctx.container == Some(NodeKind::Implementation) {
        out.push(Facet::new(Lens::Substitution, PROTOCOL_DEFAULT).with_detail("in an extension"));
    }
    let text = node.text(ctx.text).unwrap_or_default();
    if let Some(header) = text.split('{').next() {
        if header.contains("some ") {
            out.push(Facet::new(Lens::Substitution, OPAQUE));
        }
        if header.contains("any ") {
            out.push(Facet::new(Lens::Substitution, EXISTENTIAL));
        }
    }
}

/// What changes shape before compiling.
fn variation(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    if let Some(condition) = super::open_condition(ctx.text, node.span().start.0) {
        out.push(Facet::new(Lens::Variation, CONDITION).with_detail(condition));
    }
    for (name, range) in super::attributes(node, ctx) {
        let subtype = match name.as_str() {
            "available" => AVAILABILITY,
            "propertyWrapper" => PROPERTY_WRAPPER,
            // The boundary lens claims these; reporting them here as well would count one
            // fact twice in two different rollups.
            "_cdecl" | "objc" | "_silgen_name" | "main" => continue,
            _ => ATTRIBUTE,
        };
        out.push(
            Facet::new(Lens::Variation, subtype)
                .with_detail(format!("@{name}"))
                .with_sites(vec![range]),
        );
    }
}

/// What crosses the package line.
fn boundary(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    for (name, range) in super::attributes(node, ctx) {
        let subtype = match name.as_str() {
            "_cdecl" => C_ENTRY,
            "objc" => OBJC,
            "_silgen_name" => SILGEN_NAME,
            "main" => ENTRY_POINT,
            _ => continue,
        };
        out.push(
            Facet::new(Lens::Boundary, subtype)
                .with_detail(format!("@{name}"))
                .with_sites(vec![range]),
        );
    }
}

/// Where substitution is dangerous.
fn hazard(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    let text = node.text(ctx.text).unwrap_or_default();
    let header = text.split('{').next().unwrap_or_default();
    if header.contains(" async") {
        out.push(Facet::new(Lens::Hazard, ASYNC));
    }
    if node.children().any(|child| child.kind() == "throws") {
        out.push(Facet::new(Lens::Hazard, THROWING));
    }
    if header.contains("Unsafe") {
        out.push(Facet::new(Lens::Hazard, UNSAFE_POINTER).with_detail("unsafe pointer type"));
    }
    if header.contains("@unchecked") {
        out.push(Facet::new(Lens::Hazard, UNCHECKED).with_detail("@unchecked Sendable"));
    }
}

/// Facets contributed by nodes that are not entities, attributed to the enclosing one.
pub(super) fn interior_facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    let site = || vec![ctx.range(node.span())];
    let text = node.text(ctx.text).unwrap_or_default();
    match node.kind() {
        "await_expression" => out.push(Facet::new(Lens::Hazard, AWAIT).with_sites(site())),
        "try_expression" if text.starts_with("try!") => out.push(
            Facet::new(Lens::Hazard, FORCED)
                .with_detail("try!")
                .with_sites(site()),
        ),
        "as_expression" if text.contains("as!") => out.push(
            Facet::new(Lens::Hazard, FORCED)
                .with_detail("as!")
                .with_sites(site()),
        ),
        "import_declaration" => {
            let module = text
                .trim_start_matches("import")
                .split_whitespace()
                .next_back()
                .unwrap_or_default();
            out.push(
                Facet::new(Lens::Boundary, MODULE_IMPORT)
                    .with_detail(module.to_owned())
                    .with_sites(site()),
            );
        },
        _ => {},
    }
}
