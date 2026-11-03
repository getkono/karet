//! Where a Rust `impl` block's members really belong.
//!
//! An `impl` is not a thing anyone navigates to. It is where methods happen to be
//! written, and the language lets it be written anywhere in the crate — which is why a
//! containment tree built from syntax alone lists `struct Widget` and `impl Widget` as
//! siblings and puts every method under the second of them.
//!
//! Two candidates, in order. **The self type**, because an `impl` binds behaviour *to* a
//! type and that is what a reader is looking under. **The trait**, when the self type is
//! not something this package declares: `impl MyTrait for String` and the blanket
//! `impl<T: Bound> MyTrait for T` have no local type to sit beneath, and the trait is the
//! only thing in the package they are about. If neither resolves the block stays where it
//! was written, which is the honest answer for `impl Display for Vec<u8>`.

use karet_treesitter::WalkNode;

use crate::lang::FacetContext;
use crate::lang::Owner;

/// The owner candidates for one `impl_item`, most specific first.
pub(super) fn owners(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Vec<Owner> {
    if node.kind() != "impl_item" {
        return Vec::new();
    }
    let self_type = super::joined(node.child_text("type", ctx.text).unwrap_or_default());
    let bound = node
        .child_text("trait", ctx.text)
        .map(super::joined)
        .filter(|bound| !bound.is_empty());
    let parameters = super::type_parameter_names(node, ctx.text);

    let mut owners = Vec::new();
    // A blanket implementation's self type *is* a type parameter, so there is no type to
    // sit beneath — only the trait it is about.
    if let Some(base) = base_name(&self_type)
        && !parameters.contains(&base)
    {
        owners.push(match &bound {
            // `impl Display for Widget` reads as `impl Display` once it is under
            // `Widget`; repeating the type it is already inside says nothing.
            Some(bound) => {
                Owner::nested(base).renamed(format!("impl {bound}"), format!("{{impl {bound}}}"))
            },
            // An inherent block dissolves: its members belong to the type outright, and
            // keeping the level would spend a whole column of the spine to say "these
            // were written together". Unless a `cfg` gates it, which changes whether
            // those members exist at all — a fact the type itself cannot carry.
            None if !gated(ctx) => Owner::dissolved(base),
            None => Owner::nested(base),
        });
    }
    if let Some(bound) = bound
        && let Some(base) = base_name(&bound)
    {
        owners.push(Owner::nested(base).renamed(
            format!("impl for {self_type}"),
            format!("{{impl for {self_type}}}"),
        ));
    }
    owners
}

/// Whether a `cfg` decides that this block exists.
fn gated(ctx: &FacetContext<'_>) -> bool {
    ctx.attribute("cfg").is_some() || ctx.attribute("cfg_attr").is_some()
}

/// The bare type name a type expression is *about*, or `None` when it is not about one.
///
/// Deliberately shallow. It looks through references and pointers, drops generic
/// arguments and path qualification, and refuses anything with no single name at its
/// head — a tuple, an array, a `dyn Trait`, a function type. Looking deeper would mean
/// resolving types, and an index that guessed wrong about which `Widget` a method belongs
/// to would be worse than one that left it where it was written.
fn base_name(text: &str) -> Option<String> {
    let mut rest = text.trim();
    loop {
        let trimmed = rest
            .strip_prefix('&')
            .or_else(|| rest.strip_prefix("*const"))
            .or_else(|| rest.strip_prefix("*mut"))
            .map(str::trim_start)
            .unwrap_or(rest);
        // A lifetime binds tighter than `mut`: `&'a mut Widget`.
        let trimmed = match trimmed.strip_prefix('\'') {
            Some(after) => after
                .split_once(char::is_whitespace)
                .map_or("", |(_, tail)| tail)
                .trim_start(),
            None => trimmed,
        };
        let trimmed = trimmed
            .strip_prefix("mut ")
            .map_or(trimmed, str::trim_start);
        if trimmed == rest {
            break;
        }
        rest = trimmed;
    }
    let head = rest.split('<').next().unwrap_or_default().trim();
    let head = head.rsplit("::").next().unwrap_or_default().trim();
    let mut chars = head.chars();
    let first = chars.next()?;
    if !(first.is_alphabetic() || first == '_') || !chars.all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    // `Self` in an impl's own self type would be circular, and `_` names nothing.
    if head == "Self" || head == "_" {
        return None;
    }
    Some(head.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_name_is_its_own_base() {
        assert_eq!(base_name("Widget").as_deref(), Some("Widget"));
    }

    #[test]
    fn references_pointers_and_lifetimes_are_looked_through() {
        for text in [
            "&Widget",
            "&mut Widget",
            "&'a Widget",
            "&'a mut Widget",
            "*const Widget",
            "*mut Widget",
            "& & Widget",
        ] {
            assert_eq!(base_name(text).as_deref(), Some("Widget"), "{text}");
        }
    }

    #[test]
    fn generics_and_qualification_are_dropped() {
        assert_eq!(base_name("Widget<T>").as_deref(), Some("Widget"));
        assert_eq!(base_name("crate::a::Widget").as_deref(), Some("Widget"));
        assert_eq!(
            base_name("crate::a::Widget<T, U>").as_deref(),
            Some("Widget")
        );
        assert_eq!(base_name("self::Widget").as_deref(), Some("Widget"));
    }

    #[test]
    fn a_type_with_no_single_name_at_its_head_has_no_owner() {
        // Refused rather than guessed: none of these is *about* one nameable type.
        for text in [
            "(A, B)",
            "[Widget; 4]",
            "dyn Render",
            "impl Render",
            "fn(u8) -> u8",
            "Self",
            "_",
            "",
        ] {
            assert_eq!(base_name(text), None, "{text}");
        }
    }
}
