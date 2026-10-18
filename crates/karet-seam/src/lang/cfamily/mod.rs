//! The C and C++ mapping — one mapping for both grammars.
//!
//! One rather than two because the languages share their declarations, and a header is
//! read with whichever grammar its extension picks: a C++ class in a `.h` is parsed as C.
//! Keeping them one family means a call from a `.cpp` file still reaches a function
//! declared in that header.
//!
//! This mapping reads **containment and visibility only**. The other four lenses — what
//! `virtual`, templates, the preprocessor and `extern "C"` would say — are not read, and
//! the mapping says so through [`SeamLanguage::unread_lenses`] so the view shows `?`
//! rather than claiming it looked and found nothing.
//!
//! A C++ method defined outside its class — `void Widget::draw() {…}` — is written away
//! from what it belongs to, the same way a Rust `impl` is, and is regrouped under its
//! class through [`SeamLanguage::ownership`].

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

#[cfg(test)]
mod tests;

/// The C/C++ mapping.
#[derive(Debug, Default, Clone, Copy)]
pub struct CFamily;

/// The shared instance, registered by [`super::for_language`].
#[must_use]
pub fn mapping() -> &'static dyn SeamLanguage {
    &CFamily
}

/// Every grammar this mapping serves, when compiled in: C first, then C++.
#[must_use]
pub fn language_ids() -> Vec<LanguageId> {
    ["c", "cpp"]
        .into_iter()
        .filter_map(language_id_from_injection_name)
        .collect()
}

/// A function with internal linkage: visible only in its translation unit.
pub const STATIC: FacetSubtype = FacetSubtype("static");
/// A function or object with external linkage.
pub const EXTERN: FacetSubtype = FacetSubtype("extern");
/// A C++ member under `public:`, or of a `struct` before any access label.
pub const PUBLIC: FacetSubtype = FacetSubtype("public");
/// A C++ member under `protected:`.
pub const PROTECTED: FacetSubtype = FacetSubtype("protected");
/// A C++ member under `private:`, or of a `class` before any access label.
pub const PRIVATE: FacetSubtype = FacetSubtype("private");

/// Every subtype this mapping can emit.
const SUBTYPES: &[(Lens, FacetSubtype)] = &[
    (Lens::Api, STATIC),
    (Lens::Api, EXTERN),
    (Lens::Api, PUBLIC),
    (Lens::Api, PROTECTED),
    (Lens::Api, PRIVATE),
];

/// The lenses this mapping does not read.
const UNREAD: &[Lens] = &[
    Lens::Substitution,
    Lens::Variation,
    Lens::Boundary,
    Lens::Hazard,
];

impl SeamLanguage for CFamily {
    fn language(&self) -> LanguageId {
        language_ids()
            .first()
            .copied()
            .unwrap_or(LanguageId(u16::MAX))
    }

    fn language_ids(&self) -> Vec<LanguageId> {
        language_ids()
    }

    fn classify(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Option<Classified> {
        classify_node(node, ctx)
    }

    fn facets_of(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Vec<Facet> {
        // An out-of-line definition repeats no access of its own; its declaration has it.
        if node.kind() == "function_definition"
            && declared_name(node, ctx).is_some_and(|(_, owner)| owner.is_some())
        {
            return Vec::new();
        }
        let subtype = match access(node, ctx) {
            Visibility::Public if in_class(ctx) => PUBLIC,
            Visibility::Super => PROTECTED,
            Visibility::Private if in_class(ctx) => PRIVATE,
            Visibility::Private => STATIC,
            _ => EXTERN,
        };
        vec![Facet::new(Lens::Api, subtype)]
    }

    fn interior_facets(&self, _: &WalkNode<'_>, _: &FacetContext<'_>) -> Vec<Facet> {
        Vec::new()
    }

    fn is_container(&self, node: &WalkNode<'_>) -> bool {
        matches!(
            node.kind(),
            "translation_unit"
                | "namespace_definition"
                | "class_specifier"
                | "struct_specifier"
                | "union_specifier"
        )
    }

    fn ownership(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Vec<Owner> {
        if node.kind() != "function_definition" {
            return Vec::new();
        }
        declared_name(node, ctx)
            .and_then(|(_, owner)| owner)
            .map(|owner| vec![Owner::nested(owner)])
            .unwrap_or_default()
    }

    fn call_query(&self, language: LanguageId) -> Option<&'static str> {
        if language_id_from_injection_name("cpp") == Some(language) {
            Some(include_str!("../../../queries/calls/cpp.scm"))
        } else {
            Some(include_str!("../../../queries/calls/c.scm"))
        }
    }

    fn implicit_receiver(&self) -> bool {
        // A C++ method's bare `size()` is `this->size()`. C has no members to reach.
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

/// Whether the construct sits directly in a class, struct or union.
fn in_class(ctx: &FacetContext<'_>) -> bool {
    matches!(ctx.container, Some(NodeKind::Type))
}

fn classify_node(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Option<Classified> {
    match node.kind() {
        "function_definition" => {
            let (name, owner) = declared_name(node, ctx)?;
            if owner.is_some() {
                // Defined outside its class: the access was declared in the class, which
                // this definition does not repeat, so it claims none rather than a guess.
                let mut out = classified(node, ctx, NodeKind::Member, name);
                out.visibility = None;
                return Some(out);
            }
            let kind = if in_class(ctx) {
                NodeKind::Member
            } else {
                NodeKind::Function
            };
            Some(classified(node, ctx, kind, name))
        },
        // A prototype at file or namespace scope, and a method declared in a class body.
        "declaration" | "field_declaration" if declares_function(node) => {
            let (name, _) = declared_name(node, ctx)?;
            let kind = if in_class(ctx) {
                NodeKind::Member
            } else {
                NodeKind::Function
            };
            Some(classified(node, ctx, kind, name))
        },
        "field_declaration" if in_class(ctx) => {
            let (name, _) = declared_name(node, ctx)?;
            Some(classified(node, ctx, NodeKind::Member, name))
        },
        // Only a definition — `struct S { … }` — is an entity; `struct S *p` names one.
        "struct_specifier" | "union_specifier" | "class_specifier" | "enum_specifier"
            if node.child_span("body").is_some() =>
        {
            let name = node.child_text("name", ctx.text)?.trim().to_owned();
            let mut out = classified(node, ctx, NodeKind::Type, name);
            // What it derives from, so a call through a base can tell an override from a
            // same-named method on an unrelated class.
            out.detail = node
                .children()
                .find(|child| child.kind() == "base_class_clause")
                .and_then(|clause| clause.text(ctx.text))
                .map(|clause| clause.trim().to_owned());
            Some(out)
        },
        // `typedef struct point {…} point;` names one type twice; the struct is the entity.
        "type_definition" if !names_a_defined_type(node, ctx) => {
            let name = node.child_text("declarator", ctx.text)?.trim().to_owned();
            plain_identifier(&name).then(|| classified(node, ctx, NodeKind::Type, name))
        },
        "namespace_definition" => {
            // `namespace a::b {` is one construct with a two-part name; `::` would read
            // back as two segments of a path, so the name keeps its parts with dots.
            let name = node.child_text("name", ctx.text).map_or_else(
                || "{anonymous}".to_owned(),
                |name| name.trim().replace("::", "."),
            );
            Some(classified(node, ctx, NodeKind::Module, name))
        },
        _ => None,
    }
}

/// Whether a declaration declares a function, rather than a variable of function type.
///
/// `int (*handler)(int);` has a function declarator too, but its name sits behind a
/// parenthesized pointer: it is a variable holding a function, and declaring it as one
/// would invent a function nobody wrote.
fn declares_function(node: &WalkNode<'_>) -> bool {
    let mut at = declarator(node);
    while let Some(current) = at {
        if current.kind() == "function_declarator" {
            return declarator(&current)
                .is_none_or(|inner| inner.kind() != "parenthesized_declarator");
        }
        at = declarator(&current);
    }
    false
}

/// Whether a typedef's type is itself a named definition, which is the entity already.
fn names_a_defined_type(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> bool {
    node.children()
        .find(|child| child.field_name() == Some("type"))
        .is_some_and(|ty| {
            ty.child_span("body").is_some() && ty.child_text("name", ctx.text).is_some()
        })
}

/// The next declarator down the chain from a node.
///
/// Usually its `declarator` field. A C++ reference, parenthesized or attributed
/// declarator holds its inner declarator as a plain child instead, and skipping it would
/// lose every function returning a reference.
fn declarator<'t>(node: &WalkNode<'t>) -> Option<WalkNode<'t>> {
    if let Some(field) = node
        .children()
        .find(|child| child.field_name() == Some("declarator"))
    {
        return Some(field);
    }
    if !matches!(
        node.kind(),
        "reference_declarator" | "parenthesized_declarator" | "attributed_declarator"
    ) {
        return None;
    }
    node.children().find(|child| {
        child.is_named()
            && !matches!(
                child.kind(),
                "attribute_declaration" | "ms_based_modifier" | "ms_pointer_modifier"
            )
    })
}

/// The name a declaration declares, and the class it names as its owner when it is
/// written `Owner::name`.
///
/// Follows the declarator chain — pointers, references, function declarators — down to
/// the identifier at its end, which is where C hides the name.
fn declared_name(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Option<(String, Option<String>)> {
    let mut at = declarator(node)?;
    loop {
        match at.kind() {
            "identifier" | "field_identifier" | "type_identifier" | "destructor_name"
            | "operator_name" => {
                return Some((at.text(ctx.text)?.trim().to_owned(), None));
            },
            "qualified_identifier" => return qualified(&at, ctx),
            _ => at = declarator(&at)?,
        }
    }
}

/// The last name of a `a::b::name` path, with the segment before it as its owner.
fn qualified(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Option<(String, Option<String>)> {
    let mut scope = node.child_text("scope", ctx.text).map(str::trim);
    let mut name = node.children().find(|c| c.field_name() == Some("name"))?;
    while name.kind() == "qualified_identifier" {
        scope = name.child_text("scope", ctx.text).map(str::trim).or(scope);
        let inner = name.children().find(|c| c.field_name() == Some("name"))?;
        name = inner;
    }
    let owner = scope
        .map(|scope| scope.split('<').next().unwrap_or(scope).trim().to_owned())
        .filter(|scope| plain_identifier(scope));
    Some((name.text(ctx.text)?.trim().to_owned(), owner))
}

/// Whether `text` is one plain identifier.
fn plain_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|first| first.is_alphabetic() || first == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
}

fn classified(
    node: &WalkNode<'_>,
    ctx: &FacetContext<'_>,
    kind: NodeKind,
    name: String,
) -> Classified {
    Classified {
        kind,
        segment: name.clone(),
        name,
        detail: None,
        selection: ctx.range(node.span()),
        visibility: Some(access(node, ctx)),
    }
}

/// The declared access: a C++ member's label, or a C declaration's linkage.
fn access(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Visibility {
    if in_class(ctx) {
        return member_access(ctx.text, node.span().start.0);
    }
    let is_static = node.children().any(|child| {
        child.kind() == "storage_class_specifier" && child.text(ctx.text) == Some("static")
    });
    if is_static {
        Visibility::Private
    } else {
        Visibility::Public
    }
}

/// The access label in force at `at` inside a class body.
///
/// Read off the text rather than the tree, because a label is a sibling that governs every
/// member after it, and a mapping sees one node at a time. Walks back to the body's
/// opening brace, skipping nested braces; the nearest label wins, and with none the
/// default is the one the keyword before the brace implies.
fn member_access(text: &str, at: usize) -> Visibility {
    let before = text.get(..at).unwrap_or_default();
    let mut depth = 0usize;
    for (offset, ch) in before.char_indices().rev() {
        match ch {
            '}' => depth += 1,
            '{' if depth == 0 => {
                // The last of `class`/`struct`/`union` as a whole word is the one this
                // body opens: `template <class T> struct Box` is a struct, and a comment
                // or file name merely containing "class" is neither.
                let head = before.get(..offset).unwrap_or_default();
                let keyword = head
                    .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .rev()
                    .find(|word| matches!(*word, "class" | "struct" | "union"));
                return if keyword == Some("class") {
                    Visibility::Private
                } else {
                    Visibility::Public
                };
            },
            '{' => depth = depth.saturating_sub(1),
            ':' if depth == 0 => {
                let label = before.get(..offset).unwrap_or_default().trim_end();
                for (word, visibility) in [
                    ("public", Visibility::Public),
                    ("protected", Visibility::Super),
                    ("private", Visibility::Private),
                ] {
                    if label.ends_with(word)
                        && !label
                            .get(..label.len() - word.len())
                            .unwrap_or_default()
                            .ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == ':')
                    {
                        return visibility;
                    }
                }
            },
            _ => {},
        }
    }
    Visibility::Public
}
