//! Where a Swift `extension`'s members really belong.
//!
//! An `extension` is `impl` under another name: it holds members of a type declared
//! somewhere else, and the language lets it be written in any file in the module. So the
//! same two candidates apply, in the same order and for the same reasons — the extended
//! type first, then the protocol it conforms to, for the case where the type is not this
//! package's to extend.
//!
//! The one thing Swift does differently is its gate. Rust writes `#[cfg]` *on* the block,
//! so the extractor hands it over as an attribute; Swift writes `#if` *around* it, as a
//! flat sibling that neither containment nor decoration pairing can associate. Reading the
//! open directives above a declaration is what makes the gated case answerable at all.

use karet_treesitter::WalkNode;

use super::Classified;
use super::FacetContext;
use super::Owner;
use crate::model::NodeKind;

/// The owner candidates for one node, most specific first.
pub(super) fn owners(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Vec<Owner> {
    if node.kind() != "class_declaration" || super::keyword(node, ctx) != "extension" {
        return Vec::new();
    }
    let extended = node.child_text("name", ctx.text).unwrap_or_default().trim();
    let conformance = conformances(node, ctx);
    let mut owners = Vec::new();
    if let Some(base) = base_name(extended) {
        owners.push(match conformance.first() {
            // `extension Widget: Codable` reads as `extension Codable` once it is under
            // `Widget`; repeating the type it is already inside says nothing.
            Some(protocol) => Owner::nested(base).renamed(
                format!("extension {protocol}"),
                format!("{{extension {protocol}}}"),
            ),
            // A plain extension dissolves: its members belong to the type outright.
            // Unless a `#if` gates it, which decides whether they exist at all.
            None if open_condition(ctx.text, node.span().start.0).is_none() => {
                Owner::dissolved(base)
            },
            None => Owner::nested(base),
        });
    }
    if let Some(protocol) = conformance.first()
        && let Some(base) = base_name(protocol)
    {
        owners.push(Owner::nested(base).renamed(
            format!("extension for {extended}"),
            format!("{{extension for {extended}}}"),
        ));
    }
    owners
}

/// Describe an `extension`, which has no name of its own.
pub(super) fn classify_extension(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Classified {
    let extended = node
        .child_text("name", ctx.text)
        .unwrap_or("?")
        .trim()
        .to_owned();
    let conformance = conformances(node, ctx);
    let (name, detail) = match conformance.first() {
        Some(protocol) => (
            format!("extension {protocol} for {extended}"),
            Some(protocol.clone()),
        ),
        None => (format!("extension {extended}"), None),
    };
    Classified {
        kind: NodeKind::Implementation,
        segment: format!("{{{name}}}"),
        name,
        detail,
        selection: node
            .child_span("name")
            .map_or_else(|| ctx.range(node.span()), |span| ctx.range(span)),
        visibility: Some(super::visibility_of(node, ctx)),
    }
}

/// The protocols a declaration says it conforms to, as written.
pub(super) fn conformances(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Vec<String> {
    node.children()
        .filter(|child| child.kind() == "inheritance_specifier")
        .filter_map(|child| child.text(ctx.text).map(|text| text.trim().to_owned()))
        .collect()
}

/// The bare type name a type expression is about, or `None` when it is not about one.
///
/// Deliberately shallow, exactly as the Rust mapping's is: generic arguments and
/// qualification are dropped, and anything with no single name at its head is refused
/// rather than guessed at.
#[must_use]
pub fn base_name(text: &str) -> Option<String> {
    let head = text.trim().split(['<', ' ']).next().unwrap_or_default();
    let head = head.rsplit('.').next().unwrap_or_default().trim();
    let mut chars = head.chars();
    let first = chars.next()?;
    if !(first.is_alphabetic() || first == '_') || !chars.all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    if head == "Self" {
        return None;
    }
    Some(head.to_owned())
}

/// The innermost compilation condition still open at byte `at`, if any.
///
/// `#if` and `#endif` are flat siblings in this grammar, so a declaration between them is
/// related to neither by the tree. Counting the directives above it is what makes the
/// relationship answerable — and it is exact for any file the compiler would accept, since
/// unbalanced directives do not compile.
#[must_use]
pub fn open_condition(text: &str, at: usize) -> Option<String> {
    let before = text.get(..at)?;
    let mut open: Vec<String> = Vec::new();
    for line in before.lines() {
        let trimmed = line.trim_start();
        if let Some(condition) = trimmed.strip_prefix("#if") {
            open.push(condition.trim().to_owned());
        } else if trimmed.starts_with("#endif") {
            open.pop();
        } else if let Some(condition) = trimmed.strip_prefix("#elseif") {
            open.pop();
            open.push(condition.trim().to_owned());
        } else if trimmed.starts_with("#else") {
            let previous = open.pop().unwrap_or_default();
            open.push(format!("!({previous})"));
        }
    }
    open.pop()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declaration_outside_every_directive_is_ungated() {
        assert_eq!(open_condition("struct S {}\n", 11), None);
    }

    #[test]
    fn the_innermost_open_directive_is_the_one_that_gates() {
        let text = "#if os(iOS)\n#if DEBUG\nstruct S {}\n#endif\n#endif\n";
        let at = text.find("struct").unwrap_or(0);
        assert_eq!(open_condition(text, at).as_deref(), Some("DEBUG"));
    }

    #[test]
    fn a_closed_directive_gates_nothing_after_it() {
        let text = "#if DEBUG\nstruct A {}\n#endif\nstruct B {}\n";
        let at = text.find("struct B").unwrap_or(0);
        assert_eq!(open_condition(text, at), None);
    }

    #[test]
    fn an_else_branch_reads_as_the_negation_of_its_condition() {
        let text = "#if DEBUG\n#else\nstruct S {}\n#endif\n";
        let at = text.find("struct").unwrap_or(0);
        assert_eq!(open_condition(text, at).as_deref(), Some("!(DEBUG)"));
    }

    #[test]
    fn a_base_name_looks_through_generics_and_qualification() {
        assert_eq!(base_name("Widget").as_deref(), Some("Widget"));
        assert_eq!(base_name("Array<Int>").as_deref(), Some("Array"));
        assert_eq!(base_name("Module.Widget").as_deref(), Some("Widget"));
        assert_eq!(base_name("Self"), None);
        assert_eq!(base_name("[Int]"), None);
        assert_eq!(base_name(""), None);
    }
}
