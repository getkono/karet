//! The five lenses for TypeScript and JavaScript.
//!
//! Where the language states a fact, this reports the fact. Where it states a convention,
//! it says so in the facet's detail rather than pretending the compiler enforces it — the
//! same line Python's mapping draws around a leading underscore.

use karet_treesitter::WalkNode;

use super::FacetContext;
use crate::model::Facet;
use crate::model::FacetSubtype;
use crate::model::Lens;
use crate::model::NodeKind;

// --- api ---------------------------------------------------------------------

/// Reachable outside its own file.
pub const EXPORT: FacetSubtype = FacetSubtype("export");
/// The module's default export — reachable under whatever name the importer chooses.
pub const DEFAULT_EXPORT: FacetSubtype = FacetSubtype("default-export");
/// Declared but not exported: reachable only from its own file.
pub const MODULE_LOCAL: FacetSubtype = FacetSubtype("module-local");
/// A `private` member, or one named with the `#` sigil.
pub const PRIVATE: FacetSubtype = FacetSubtype("private");
/// A `protected` member.
pub const PROTECTED: FacetSubtype = FacetSubtype("protected");
/// A type declared for consumers without a definition — `declare`.
pub const AMBIENT: FacetSubtype = FacetSubtype("ambient");

// --- substitution ------------------------------------------------------------

/// An `interface` — a contract with no implementation.
pub const INTERFACE: FacetSubtype = FacetSubtype("interface");
/// An `abstract` class or member: a contract with some of it filled in.
pub const ABSTRACT: FacetSubtype = FacetSubtype("abstract");
/// A class declaring it satisfies an interface.
pub const IMPLEMENTS: FacetSubtype = FacetSubtype("implements");
/// A class or interface extending another.
pub const EXTENDS: FacetSubtype = FacetSubtype("extends");
/// A signature with no body, which an implementation supplies.
pub const SIGNATURE: FacetSubtype = FacetSubtype("signature");
/// A member that may be absent — `render?(): void`.
pub const OPTIONAL: FacetSubtype = FacetSubtype("optional");
/// A value whose type is a function: the swap point of the language.
pub const CALLABLE: FacetSubtype = FacetSubtype("callable");
/// A generic parameter with a bound.
pub const GENERIC_BOUND: FacetSubtype = FacetSubtype("generic-bound");

// --- variation ---------------------------------------------------------------

/// A decorator rewriting what it is attached to.
pub const DECORATOR: FacetSubtype = FacetSubtype("decorator");
/// A type computed from a condition — `T extends U ? A : B`.
pub const CONDITIONAL_TYPE: FacetSubtype = FacetSubtype("conditional-type");
/// A module loaded at run time rather than linked.
pub const DYNAMIC_IMPORT: FacetSubtype = FacetSubtype("dynamic-import");
/// A branch on an environment variable, which changes shape per build.
pub const ENVIRONMENT_BRANCH: FacetSubtype = FacetSubtype("environment-branch");

// --- boundary ----------------------------------------------------------------

/// An import of a package rather than a file in this one.
pub const EXTERNAL_IMPORT: FacetSubtype = FacetSubtype("external-import");
/// A declaration describing a module this package does not contain.
pub const AMBIENT_MODULE: FacetSubtype = FacetSubtype("ambient-module");
/// A reach into the runtime's global object.
pub const GLOBAL: FacetSubtype = FacetSubtype("global");

// --- hazard ------------------------------------------------------------------

/// An `async` function: its caller cannot see when it finishes.
pub const ASYNC: FacetSubtype = FacetSubtype("async");
/// An await point.
pub const AWAIT: FacetSubtype = FacetSubtype("await");
/// A cast to `any`, which switches the type system off for that expression.
pub const ANY_CAST: FacetSubtype = FacetSubtype("any-cast");
/// A `!` assertion that a value is not null, which the compiler takes on trust.
pub const NON_NULL: FacetSubtype = FacetSubtype("non-null");
/// A comment silencing the type checker.
pub const SUPPRESSED: FacetSubtype = FacetSubtype("suppressed");
/// Code compiled at run time.
pub const RUNTIME_EVAL: FacetSubtype = FacetSubtype("runtime-eval");

/// Every facet subtype this mapping can emit.
pub const SUBTYPES: &[(Lens, FacetSubtype)] = &[
    (Lens::Api, EXPORT),
    (Lens::Api, DEFAULT_EXPORT),
    (Lens::Api, MODULE_LOCAL),
    (Lens::Api, PRIVATE),
    (Lens::Api, PROTECTED),
    (Lens::Api, AMBIENT),
    (Lens::Substitution, INTERFACE),
    (Lens::Substitution, ABSTRACT),
    (Lens::Substitution, IMPLEMENTS),
    (Lens::Substitution, EXTENDS),
    (Lens::Substitution, SIGNATURE),
    (Lens::Substitution, OPTIONAL),
    (Lens::Substitution, CALLABLE),
    (Lens::Substitution, GENERIC_BOUND),
    (Lens::Variation, DECORATOR),
    (Lens::Variation, CONDITIONAL_TYPE),
    (Lens::Variation, DYNAMIC_IMPORT),
    (Lens::Variation, ENVIRONMENT_BRANCH),
    (Lens::Boundary, EXTERNAL_IMPORT),
    (Lens::Boundary, AMBIENT_MODULE),
    (Lens::Boundary, GLOBAL),
    (Lens::Hazard, ASYNC),
    (Lens::Hazard, AWAIT),
    (Lens::Hazard, ANY_CAST),
    (Lens::Hazard, NON_NULL),
    (Lens::Hazard, SUPPRESSED),
    (Lens::Hazard, RUNTIME_EVAL),
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
    let member = matches!(
        ctx.container,
        Some(NodeKind::Type | NodeKind::Interface | NodeKind::Implementation)
    );
    if let Some(modifier) = super::accessibility(node, ctx) {
        match modifier {
            "private" => out.push(Facet::new(Lens::Api, PRIVATE).with_detail("private")),
            "protected" => out.push(Facet::new(Lens::Api, PROTECTED).with_detail("protected")),
            _ => {},
        }
    } else if node
        .child_text("name", ctx.text)
        .is_some_and(|name| name.starts_with('#'))
    {
        // The one privacy this language actually enforces at run time.
        out.push(Facet::new(Lens::Api, PRIVATE).with_detail("# sigil — enforced at run time"));
    }
    if member {
        return;
    }
    if super::preceded_by(node, ctx, &["default"]) {
        out.push(Facet::new(Lens::Api, DEFAULT_EXPORT).with_detail("export default"));
    } else if super::exported(node, ctx) {
        out.push(Facet::new(Lens::Api, EXPORT).with_detail("export"));
    } else {
        out.push(Facet::new(Lens::Api, MODULE_LOCAL).with_detail("not exported"));
    }
    if super::preceded_by(node, ctx, &["declare"]) {
        out.push(Facet::new(Lens::Api, AMBIENT).with_detail("declare"));
    }
}

/// What behavior can be swapped.
fn substitution(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    match node.kind() {
        "interface_declaration" => out.push(Facet::new(Lens::Substitution, INTERFACE)),
        "abstract_class_declaration" => {
            out.push(Facet::new(Lens::Substitution, ABSTRACT).with_detail("abstract class"));
        },
        "abstract_method_signature" => {
            out.push(Facet::new(Lens::Substitution, ABSTRACT).with_detail("abstract member"));
        },
        // A signature with no body: the implementation is supplied elsewhere.
        "method_signature" | "function_signature" | "property_signature" => {
            out.push(Facet::new(Lens::Substitution, SIGNATURE));
        },
        _ => {},
    }
    if let Some(heritage) = node
        .children()
        .find(|child| child.kind() == "class_heritage")
    {
        for clause in heritage.children() {
            let detail = clause.text(ctx.text).unwrap_or_default().to_owned();
            match clause.kind() {
                "implements_clause" => {
                    out.push(Facet::new(Lens::Substitution, IMPLEMENTS).with_detail(detail));
                },
                "extends_clause" => {
                    out.push(Facet::new(Lens::Substitution, EXTENDS).with_detail(detail));
                },
                _ => {},
            }
        }
    }
    if let Some(bounds) = node.children().find(|c| c.kind() == "type_parameters")
        && bounds
            .text(ctx.text)
            .is_some_and(|text| text.contains("extends"))
    {
        let detail = bounds.text(ctx.text).unwrap_or_default().to_owned();
        out.push(Facet::new(Lens::Substitution, GENERIC_BOUND).with_detail(detail));
    }
    // `render?(): void` — a member the implementor may simply not supply.
    if node
        .text(ctx.text)
        .zip(node.child_text("name", ctx.text))
        .is_some_and(|(all, name)| all.starts_with(&format!("{name}?")))
    {
        out.push(Facet::new(Lens::Substitution, OPTIONAL));
    }
    if annotation(node, ctx).is_some_and(|text| text.contains("=>") || text.contains("Function")) {
        out.push(Facet::new(Lens::Substitution, CALLABLE));
    }
}

/// What changes shape before running.
fn variation(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    // Read from the node rather than from `ctx.attributes`: this grammar hangs a
    // decorator inside the declaration it decorates, not beside it.
    for decorator in node.children().filter(|child| child.kind() == "decorator") {
        let text = decorator.text(ctx.text).unwrap_or("@?");
        let name = text.split_once('(').map_or(text, |(head, _)| head);
        out.push(
            Facet::new(Lens::Variation, DECORATOR)
                .with_detail(name.trim().to_owned())
                .with_sites(vec![ctx.range(decorator.span())]),
        );
    }
    if annotation(node, ctx).is_some_and(|text| text.contains('?') && text.contains("extends")) {
        out.push(Facet::new(Lens::Variation, CONDITIONAL_TYPE));
    }
}

/// What crosses the package line.
fn boundary(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    // `declare module "ext" { … }` describes something this package does not contain.
    if node.kind() == "module"
        && node
            .child_text("name", ctx.text)
            .is_some_and(|name| name.starts_with('"') || name.starts_with('\''))
    {
        out.push(Facet::new(Lens::Boundary, AMBIENT_MODULE));
    }
}

/// Where substitution is dangerous.
fn hazard(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    if super::preceded_by(node, ctx, &["async"])
        || node
            .text(ctx.text)
            .is_some_and(|text| text.starts_with("async "))
    {
        out.push(Facet::new(Lens::Hazard, ASYNC));
    }
}

/// Facets contributed by nodes that are not entities, attributed to the enclosing one.
pub(super) fn interior_facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    let site = || vec![ctx.range(node.span())];
    let text = node.text(ctx.text).unwrap_or_default();
    match node.kind() {
        "await_expression" => out.push(Facet::new(Lens::Hazard, AWAIT).with_sites(site())),
        "non_null_expression" => {
            out.push(Facet::new(Lens::Hazard, NON_NULL).with_sites(site()));
        },
        "as_expression" | "type_assertion" if text.ends_with("any") => {
            out.push(
                Facet::new(Lens::Hazard, ANY_CAST)
                    .with_detail("as any")
                    .with_sites(site()),
            );
        },
        "comment" if text.contains("@ts-ignore") || text.contains("@ts-expect-error") => {
            out.push(
                Facet::new(Lens::Hazard, SUPPRESSED)
                    .with_detail(text.trim().to_owned())
                    .with_sites(site()),
            );
        },
        "call_expression" => call_facets(node, ctx, out),
        "import_statement" | "export_statement" => import_facets(node, ctx, out),
        "member_expression" if text.starts_with("globalThis") => {
            out.push(Facet::new(Lens::Boundary, GLOBAL).with_sites(site()));
        },
        "identifier" if text == "process" => {
            // `process.env.X` decides shape at build time as surely as a `cfg` does.
            out.push(
                Facet::new(Lens::Variation, ENVIRONMENT_BRANCH)
                    .with_detail("process.env")
                    .with_sites(site()),
            );
        },
        _ => {},
    }
}

/// Facets for a call: dynamic import, and code compiled at run time.
fn call_facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    let Some(callee) = node
        .children()
        .find(|child| child.field_name() == Some("function"))
        .and_then(|child| child.text(ctx.text))
    else {
        return;
    };
    let site = vec![ctx.range(node.span())];
    match callee {
        "import" => out.push(Facet::new(Lens::Variation, DYNAMIC_IMPORT).with_sites(site)),
        "eval" | "Function" => out.push(
            Facet::new(Lens::Hazard, RUNTIME_EVAL)
                .with_detail(callee.to_owned())
                .with_sites(site),
        ),
        _ => {},
    }
}

/// An import of a package rather than a file in this one.
///
/// A specifier starting with `.` or `/` names something inside the tree; anything else is
/// a package, and the package line is exactly what the boundary lens is about.
fn import_facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    let Some(source) = node
        .children()
        .find(|child| child.field_name() == Some("source"))
        .and_then(|child| child.text(ctx.text))
    else {
        return;
    };
    let specifier = source.trim_matches(['"', '\'', '`']);
    if specifier.starts_with('.') || specifier.starts_with('/') {
        return;
    }
    out.push(
        Facet::new(Lens::Boundary, EXTERNAL_IMPORT)
            .with_detail(specifier.to_owned())
            .with_sites(vec![ctx.range(node.span())]),
    );
}

/// A declaration's type annotation, as written.
fn annotation<'a>(node: &WalkNode<'_>, ctx: &FacetContext<'a>) -> Option<&'a str> {
    node.children()
        .find(|child| matches!(child.kind(), "type_annotation"))
        .and_then(|child| child.text(ctx.text))
}
