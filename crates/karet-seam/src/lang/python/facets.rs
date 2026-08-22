//! Python constructs mapped onto the five lenses.
//!
//! The lens *questions* transfer unchanged; only the constructs answering them differ.
//! "What can be swapped" is a trait impl in Rust and a Protocol subclass in Python; "what
//! varies before compiling" is `cfg` there and a `TYPE_CHECKING` branch here.

use karet_treesitter::WalkNode;

use crate::lang::FacetContext;
use crate::model::Facet;
use crate::model::FacetSubtype;
use crate::model::Lens;
use crate::model::NodeKind;

// --- api -------------------------------------------------------------------
/// A name with no leading underscore.
pub const PUBLIC: FacetSubtype = FacetSubtype("public");
/// A name with a leading underscore — private by agreement, not enforcement.
pub const PRIVATE: FacetSubtype = FacetSubtype("private");
/// A dunder, which is public protocol despite the underscores.
pub const DUNDER: FacetSubtype = FacetSubtype("dunder");
/// An `__all__` declaration, the one place Python states its public surface outright.
pub const ALL_EXPORT: FacetSubtype = FacetSubtype("all-export");
/// A re-export: a name imported at module level for others to reach through here.
pub const REEXPORT: FacetSubtype = FacetSubtype("reexport");

// --- substitution ----------------------------------------------------------
/// A `Protocol` subclass — a structural contract.
pub const PROTOCOL: FacetSubtype = FacetSubtype("protocol");
/// An `ABC` subclass, or a class carrying `@abstractmethod`.
pub const ABSTRACT: FacetSubtype = FacetSubtype("abstract");
/// A class deriving from another, which may replace its behaviour.
pub const SUBCLASS: FacetSubtype = FacetSubtype("subclass");
/// A method with a body in a contract class — a default an implementor may replace.
pub const DEFAULT_METHOD: FacetSubtype = FacetSubtype("default-method");
/// A parameter or attribute typed `Callable[…]` — a behaviour slot.
pub const CALLABLE: FacetSubtype = FacetSubtype("callable");
/// An `@overload` declaration.
pub const OVERLOAD: FacetSubtype = FacetSubtype("overload");

// --- variation -------------------------------------------------------------
/// A `TYPE_CHECKING` branch, which exists only for the type checker.
pub const TYPE_CHECKING: FacetSubtype = FacetSubtype("type-checking");
/// A branch on `sys.platform` or `os.name`.
pub const PLATFORM_BRANCH: FacetSubtype = FacetSubtype("platform-branch");
/// A decorator applied to a definition.
pub const DECORATOR: FacetSubtype = FacetSubtype("decorator");
/// An import inside a conditional, so the module may or may not be present.
pub const CONDITIONAL_IMPORT: FacetSubtype = FacetSubtype("conditional-import");

// --- boundary --------------------------------------------------------------
/// A `ctypes` or `cffi` foreign-library binding.
pub const FOREIGN_BINDING: FacetSubtype = FacetSubtype("foreign-binding");
/// An import of a module from outside this package.
pub const EXTERNAL_IMPORT: FacetSubtype = FacetSubtype("external-import");
/// A `__main__` entry point.
pub const ENTRY_POINT: FacetSubtype = FacetSubtype("entry-point");

// --- hazard ----------------------------------------------------------------
/// An `async def`.
pub const ASYNC: FacetSubtype = FacetSubtype("async");
/// An await point.
pub const AWAIT: FacetSubtype = FacetSubtype("await");
/// A `global` statement, which reaches outside the function's own scope.
pub const GLOBAL: FacetSubtype = FacetSubtype("global");
/// A `nonlocal` statement.
pub const NONLOCAL: FacetSubtype = FacetSubtype("nonlocal");

/// Every subtype the Python mapping can emit.
pub const SUBTYPES: &[(Lens, FacetSubtype)] = &[
    (Lens::Api, PUBLIC),
    (Lens::Api, PRIVATE),
    (Lens::Api, DUNDER),
    (Lens::Api, ALL_EXPORT),
    (Lens::Api, REEXPORT),
    (Lens::Substitution, PROTOCOL),
    (Lens::Substitution, ABSTRACT),
    (Lens::Substitution, SUBCLASS),
    (Lens::Substitution, DEFAULT_METHOD),
    (Lens::Substitution, CALLABLE),
    (Lens::Substitution, OVERLOAD),
    (Lens::Variation, TYPE_CHECKING),
    (Lens::Variation, PLATFORM_BRANCH),
    (Lens::Variation, DECORATOR),
    (Lens::Variation, CONDITIONAL_IMPORT),
    (Lens::Boundary, FOREIGN_BINDING),
    (Lens::Boundary, EXTERNAL_IMPORT),
    (Lens::Boundary, ENTRY_POINT),
    (Lens::Hazard, ASYNC),
    (Lens::Hazard, AWAIT),
    (Lens::Hazard, GLOBAL),
    (Lens::Hazard, NONLOCAL),
];

/// Base classes that make a class a contract rather than a concrete type.
const CONTRACT_BASES: &[&str] = &["Protocol", "ABC", "ABCMeta", "abc.ABC", "typing.Protocol"];

/// Modules whose use is a foreign-function boundary.
const FOREIGN_MODULES: &[&str] = &["ctypes", "cffi", "_ctypes"];

/// Whether a class derives from a contract base.
#[must_use]
pub fn is_contract(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> bool {
    superclasses(node, ctx)
        .iter()
        .any(|base| CONTRACT_BASES.contains(&base.as_str()))
}

/// The superclass names a class declares.
fn superclasses(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Vec<String> {
    node.child_text("superclasses", ctx.text)
        .map(|text| {
            text.trim_matches(['(', ')'])
                .split(',')
                .map(|base| base.trim().to_owned())
                .filter(|base| !base.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Facets for an addressable entity.
pub fn entity_facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    let name = node
        .child_text("name", ctx.text)
        .or_else(|| node.child_text("left", ctx.text))
        .unwrap_or_default();

    api_facets(name, out);
    decorator_facets(ctx, out);

    match node.kind() {
        "class_definition" => class_facets(node, ctx, out),
        "function_definition" => function_facets(node, ctx, name, out),
        "assignment" if name == "__all__" => out.push(
            Facet::new(Lens::Api, ALL_EXPORT).with_detail(
                node.child_text("right", ctx.text)
                    .unwrap_or_default()
                    .to_owned(),
            ),
        ),
        _ => {},
    }
}

/// The `api` facet a name's shape implies.
fn api_facets(name: &str, out: &mut Vec<Facet>) {
    if name.is_empty() {
        return;
    }
    if name.starts_with("__") && name.ends_with("__") {
        out.push(Facet::new(Lens::Api, DUNDER).with_detail("special method — public protocol"));
    } else if name.starts_with('_') {
        // Say plainly that this is a convention. Rust states a fact here; Python asks.
        out.push(
            Facet::new(Lens::Api, PRIVATE)
                .with_detail("leading underscore — private by convention, not enforced"),
        );
    } else {
        out.push(Facet::new(Lens::Api, PUBLIC));
    }
}

/// Decorators become variation facets, and a few of them mean more than that.
fn decorator_facets(ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    for attribute in ctx.attributes {
        out.push(
            Facet::new(Lens::Variation, DECORATOR)
                .with_detail(attribute.name.clone())
                .with_sites(vec![attribute.range]),
        );
        match attribute.name.rsplit('.').next().unwrap_or_default() {
            "abstractmethod" | "abstractproperty" => {
                out.push(
                    Facet::new(Lens::Substitution, ABSTRACT).with_detail(attribute.name.clone()),
                );
            },
            "overload" => out.push(Facet::new(Lens::Substitution, OVERLOAD)),
            _ => {},
        }
    }
}

/// Facets specific to a class.
fn class_facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    let bases = superclasses(node, ctx);
    if bases.is_empty() {
        return;
    }
    let detail = bases.join(", ");
    if bases.iter().any(|b| b.ends_with("Protocol")) {
        out.push(Facet::new(Lens::Substitution, PROTOCOL).with_detail(detail.clone()));
    }
    if bases
        .iter()
        .any(|b| b.ends_with("ABC") || b.ends_with("ABCMeta"))
    {
        out.push(Facet::new(Lens::Substitution, ABSTRACT).with_detail(detail.clone()));
    }
    out.push(Facet::new(Lens::Substitution, SUBCLASS).with_detail(detail));
}

/// Facets specific to a function or method.
fn function_facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, name: &str, out: &mut Vec<Facet>) {
    if node.has_child_kind("async") {
        out.push(Facet::new(Lens::Hazard, ASYNC).with_detail("async def"));
    }
    // A method with a real body inside a contract class is a replaceable default; one
    // whose body is `...` or `pass` is a requirement, exactly as in Rust.
    if ctx.container == Some(NodeKind::Interface) && has_real_body(node, ctx) {
        out.push(Facet::new(Lens::Substitution, DEFAULT_METHOD));
    }
    if name == "main" && ctx.container.is_none() {
        out.push(Facet::new(Lens::Boundary, ENTRY_POINT).with_detail("main"));
    }
    callable_parameters(node, ctx, out);
}

/// Whether a function body is more than a placeholder.
fn has_real_body(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> bool {
    let Some(body) = node.child_text("body", ctx.text) else {
        return false;
    };
    let trimmed = body.trim();
    trimmed != "..." && trimmed != "pass" && !trimmed.is_empty()
}

/// Parameters typed `Callable[…]` are behaviour slots a caller fills.
fn callable_parameters(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    let Some(parameters) = node.children().find(|c| c.kind() == "parameters") else {
        return;
    };
    let mut sites = Vec::new();
    for parameter in parameters.children() {
        if parameter.kind() != "typed_parameter" {
            continue;
        }
        let is_callable = parameter
            .child_text("type", ctx.text)
            .is_some_and(|annotation| annotation.trim_start().starts_with("Callable"));
        if is_callable {
            sites.push(ctx.range(parameter.span()));
        }
    }
    if !sites.is_empty() {
        out.push(Facet::new(Lens::Substitution, CALLABLE).with_sites(sites));
    }
}

/// Facets contributed by non-entity nodes, attributed to the enclosing entity.
pub fn interior_facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    let site = || vec![ctx.range(node.span())];
    match node.kind() {
        "await" => out.push(
            Facet::new(Lens::Hazard, AWAIT)
                .with_detail(node.text(ctx.text).unwrap_or("await").to_owned())
                .with_sites(site()),
        ),
        "global_statement" => out.push(Facet::new(Lens::Hazard, GLOBAL).with_sites(site())),
        "nonlocal_statement" => out.push(Facet::new(Lens::Hazard, NONLOCAL).with_sites(site())),
        "if_statement" => branch_facets(node, ctx, out),
        "import_statement" | "import_from_statement" => import_facets(node, ctx, out),
        _ => {},
    }
}

/// A conditional whose predicate decides what the module even contains.
fn branch_facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    let Some(condition) = node.child_text("condition", ctx.text) else {
        return;
    };
    let site = vec![ctx.range(node.span())];
    // `if TYPE_CHECKING:` is Python's nearest equivalent to a `cfg`: the branch is real
    // to a type checker and absent at run time.
    if condition.contains("TYPE_CHECKING") {
        out.push(
            Facet::new(Lens::Variation, TYPE_CHECKING)
                .with_detail(condition.to_owned())
                .with_sites(site.clone()),
        );
    }
    if condition.contains("sys.platform") || condition.contains("os.name") {
        out.push(
            Facet::new(Lens::Variation, PLATFORM_BRANCH)
                .with_detail(condition.to_owned())
                .with_sites(site),
        );
    }
}

/// Imports that cross the package line, or bind a foreign library.
fn import_facets(node: &WalkNode<'_>, ctx: &FacetContext<'_>, out: &mut Vec<Facet>) {
    let module = node
        .child_text("module_name", ctx.text)
        .or_else(|| node.child_text("name", ctx.text))
        .unwrap_or_default();
    if module.is_empty() {
        return;
    }
    let root = module.split('.').next().unwrap_or(module);
    let site = vec![ctx.range(node.span())];
    if FOREIGN_MODULES.contains(&root) {
        out.push(
            Facet::new(Lens::Boundary, FOREIGN_BINDING)
                .with_detail(module.to_owned())
                .with_sites(site.clone()),
        );
    }
    // A relative import stays inside the package; anything else names something outside.
    if !module.starts_with('.') {
        out.push(
            Facet::new(Lens::Boundary, EXTERNAL_IMPORT)
                .with_detail(module.to_owned())
                .with_sites(site),
        );
    }
}
