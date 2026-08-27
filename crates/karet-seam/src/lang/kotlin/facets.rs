//! The five lenses for Kotlin.

use karet_treesitter::WalkNode;

use super::FacetContext;
use crate::model::Facet;
use crate::model::FacetSubtype;
use crate::model::Lens;

// --- api ---------------------------------------------------------------------

/// Visible everywhere — Kotlin's unmarked default.
pub const PUBLIC: FacetSubtype = FacetSubtype("public");
/// Visible within the module.
pub const INTERNAL: FacetSubtype = FacetSubtype("internal");
/// Visible within its own class and subclasses.
pub const PROTECTED: FacetSubtype = FacetSubtype("protected");
/// Visible within its own file or class.
pub const PRIVATE: FacetSubtype = FacetSubtype("private");

// --- substitution --------------------------------------------------------------

/// An interface — the contract itself.
pub const INTERFACE: FacetSubtype = FacetSubtype("interface");
/// A declaration that cannot be instantiated on its own.
pub const ABSTRACT: FacetSubtype = FacetSubtype("abstract");
/// A class or member that may be subclassed or overridden — Kotlin's opt-in.
pub const OPEN: FacetSubtype = FacetSubtype("open");
/// A hierarchy closed to this file, which a compiler can check exhaustively.
pub const SEALED: FacetSubtype = FacetSubtype("sealed");
/// A member replacing one it inherited.
pub const OVERRIDE: FacetSubtype = FacetSubtype("override");
/// An implementation handed to another object — `by`.
pub const DELEGATION: FacetSubtype = FacetSubtype("delegation");
/// A type with exactly one instance.
pub const OBJECT: FacetSubtype = FacetSubtype("object");
/// An extension: a member of a type it was not declared inside.
pub const EXTENSION: FacetSubtype = FacetSubtype("extension");

// --- variation -----------------------------------------------------------------

/// An annotation rewriting or marking what it is attached to.
pub const ANNOTATION: FacetSubtype = FacetSubtype("annotation");
/// A multiplatform declaration whose body is supplied per target.
pub const EXPECT_ACTUAL: FacetSubtype = FacetSubtype("expect-actual");
/// Inlined at every call site, so its body is copied rather than called.
pub const INLINE: FacetSubtype = FacetSubtype("inline");
/// A generic parameter whose type survives to run time.
pub const REIFIED: FacetSubtype = FacetSubtype("reified");

// --- boundary ------------------------------------------------------------------

/// Implemented outside the language — JNI.
pub const EXTERNAL_FN: FacetSubtype = FacetSubtype("external-fn");
/// Shaped for the JVM's calling conventions rather than Kotlin's.
pub const JVM_SURFACE: FacetSubtype = FacetSubtype("jvm-surface");
/// The program's entry point.
pub const ENTRY_POINT: FacetSubtype = FacetSubtype("entry-point");
/// An import of another package.
pub const EXTERNAL_IMPORT: FacetSubtype = FacetSubtype("external-import");

// --- hazard --------------------------------------------------------------------

/// A `suspend` function: its caller cannot see when it finishes.
pub const SUSPEND: FacetSubtype = FacetSubtype("suspend");
/// A `!!`, which trades a null check for a crash.
pub const FORCED: FacetSubtype = FacetSubtype("forced");
/// A property promised to be initialized before use, with nothing checking it.
pub const LATEINIT: FacetSubtype = FacetSubtype("lateinit");
/// Shared mutable state.
pub const VOLATILE: FacetSubtype = FacetSubtype("volatile");

/// Every facet subtype this mapping can emit.
pub const SUBTYPES: &[(Lens, FacetSubtype)] = &[
    (Lens::Api, PUBLIC),
    (Lens::Api, INTERNAL),
    (Lens::Api, PROTECTED),
    (Lens::Api, PRIVATE),
    (Lens::Substitution, INTERFACE),
    (Lens::Substitution, ABSTRACT),
    (Lens::Substitution, OPEN),
    (Lens::Substitution, SEALED),
    (Lens::Substitution, OVERRIDE),
    (Lens::Substitution, DELEGATION),
    (Lens::Substitution, OBJECT),
    (Lens::Substitution, EXTENSION),
    (Lens::Variation, ANNOTATION),
    (Lens::Variation, EXPECT_ACTUAL),
    (Lens::Variation, INLINE),
    (Lens::Variation, REIFIED),
    (Lens::Boundary, EXTERNAL_FN),
    (Lens::Boundary, JVM_SURFACE),
    (Lens::Boundary, ENTRY_POINT),
    (Lens::Boundary, EXTERNAL_IMPORT),
    (Lens::Hazard, SUSPEND),
    (Lens::Hazard, FORCED),
    (Lens::Hazard, LATEINIT),
    (Lens::Hazard, VOLATILE),
];

/// Annotations the boundary lens claims, so they are not also counted as variation.
const BOUNDARY_ANNOTATIONS: &[&str] = &["JvmStatic", "JvmField", "JvmName", "JvmOverloads"];

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
    let (subtype, detail) = match super::modifier(node, ctx, "visibility_modifier").as_deref() {
        Some("private") => (PRIVATE, "private"),
        Some("protected") => (PROTECTED, "protected"),
        Some("internal") => (INTERNAL, "internal"),
        // Kotlin's default is the opposite of Rust's, and worth saying rather than
        // leaving a reader to remember which language they are in.
        _ => (PUBLIC, "public (default)"),
    };
    out.push(Facet::new(Lens::Api, subtype).with_detail(detail));
}

/// What behavior can be swapped.
fn substitution(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    if node.kind() == "class_declaration" && super::keyword(node, ctx) == "interface" {
        out.push(Facet::new(Lens::Substitution, INTERFACE));
    }
    if node.kind() == "object_declaration" {
        out.push(Facet::new(Lens::Substitution, OBJECT).with_detail("single instance"));
    }
    for inheritance in super::modifiers(node, ctx, "inheritance_modifier") {
        match inheritance {
            "abstract" => out.push(Facet::new(Lens::Substitution, ABSTRACT)),
            // Everything is final here unless it says otherwise, so `open` is a decision.
            "open" => out.push(Facet::new(Lens::Substitution, OPEN)),
            _ => {},
        }
    }
    if super::modifiers(node, ctx, "class_modifier").contains(&"sealed") {
        out.push(Facet::new(Lens::Substitution, SEALED));
    }
    if super::modifiers(node, ctx, "member_modifier").contains(&"override") {
        out.push(Facet::new(Lens::Substitution, OVERRIDE));
    }
    if let Some(receiver) = super::receiver(node, ctx) {
        out.push(Facet::new(Lens::Substitution, EXTENSION).with_detail(receiver));
    }
    if let Some(specifiers) = node
        .children()
        .find(|child| child.kind() == "delegation_specifiers")
        && let Some(delegation) = specifiers
            .children()
            .find_map(|child| child.children().find(|c| c.kind() == "explicit_delegation"))
    {
        let detail = delegation.text(ctx.text).unwrap_or_default().to_owned();
        out.push(Facet::new(Lens::Substitution, DELEGATION).with_detail(detail));
    }
}

/// What changes shape before compiling.
fn variation(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    for (name, range) in annotations(node, ctx) {
        if BOUNDARY_ANNOTATIONS.contains(&name.as_str()) {
            continue;
        }
        out.push(
            Facet::new(Lens::Variation, ANNOTATION)
                .with_detail(format!("@{name}"))
                .with_sites(vec![range]),
        );
    }
    if let Some(platform) = super::modifier(node, ctx, "platform_modifier") {
        out.push(Facet::new(Lens::Variation, EXPECT_ACTUAL).with_detail(platform));
    }
    let functions = super::modifiers(node, ctx, "function_modifier");
    if functions.contains(&"inline") {
        out.push(Facet::new(Lens::Variation, INLINE));
    }
    if node
        .children()
        .find(|child| child.kind() == "type_parameters")
        .and_then(|child| child.text(ctx.text))
        .is_some_and(|text| text.contains("reified"))
    {
        out.push(Facet::new(Lens::Variation, REIFIED));
    }
}

/// What crosses the package line.
fn boundary(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    if super::modifiers(node, ctx, "function_modifier").contains(&"external") {
        out.push(Facet::new(Lens::Boundary, EXTERNAL_FN).with_detail("implemented in native code"));
    }
    for (name, range) in annotations(node, ctx) {
        if BOUNDARY_ANNOTATIONS.contains(&name.as_str()) {
            out.push(
                Facet::new(Lens::Boundary, JVM_SURFACE)
                    .with_detail(format!("@{name}"))
                    .with_sites(vec![range]),
            );
        }
    }
    // Only a top-level `main` is an entry point; a method named `main` is a member.
    if node.kind() == "function_declaration"
        && node.child_text("name", ctx.text) == Some("main")
        && matches!(
            ctx.container,
            Some(crate::model::NodeKind::Package | crate::model::NodeKind::Module)
        )
    {
        out.push(Facet::new(Lens::Boundary, ENTRY_POINT));
    }
}

/// Where substitution is dangerous.
fn hazard(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    if super::modifiers(node, ctx, "function_modifier").contains(&"suspend") {
        out.push(Facet::new(Lens::Hazard, SUSPEND));
    }
    if super::modifiers(node, ctx, "member_modifier").contains(&"lateinit") {
        out.push(Facet::new(Lens::Hazard, LATEINIT).with_detail("initialization is not checked"));
    }
    if annotations(node, ctx)
        .iter()
        .any(|(name, _)| name == "Volatile")
    {
        out.push(Facet::new(Lens::Hazard, VOLATILE));
    }
}

/// Facets contributed by nodes that are not entities, attributed to the enclosing one.
pub(super) fn interior_facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    let text = node.text(ctx.text).unwrap_or_default();
    match node.kind() {
        // `value!!` — a null check traded for a crash.
        "unary_expression" if text.ends_with("!!") => out.push(
            Facet::new(Lens::Hazard, FORCED)
                .with_detail("!!")
                .with_sites(vec![ctx.range(node.span())]),
        ),
        "import" => {
            let package = text.trim_start_matches("import").trim();
            out.push(
                Facet::new(Lens::Boundary, EXTERNAL_IMPORT)
                    .with_detail(package.to_owned())
                    .with_sites(vec![ctx.range(node.span())]),
            );
        },
        _ => {},
    }
}

/// Every annotation on a declaration, without its `@` or its arguments.
fn annotations(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Vec<(String, karet_core::Range)> {
    let Some(modifiers) = node.children().find(|child| child.kind() == "modifiers") else {
        return Vec::new();
    };
    modifiers
        .children()
        .filter(|child| child.kind() == "annotation")
        .filter_map(|child| {
            let text = child.text(ctx.text)?;
            let name = text
                .trim_start_matches('@')
                .split(['(', ' ', ':'])
                .next()
                .unwrap_or_default()
                .to_owned();
            Some((name, ctx.range(child.span())))
        })
        .collect()
}
