//! The language extension contract.
//!
//! Adding a language must not require changing the view, the query language, the lens
//! set, or the model. A language contributes exactly two things — a mapping from its
//! constructs to the universal [`NodeKind`]s, and a mapping from its constructs to lens
//! [`Facet`]s — plus a declaration of what its semantic tier can resolve, which may be
//! empty. Everything downstream degrades rather than failing when it is.
//!
//! The contract is deliberately narrow. A language implementation sees one node at a
//! time, plus the context the walk has accumulated around it; it never sees the index,
//! so it cannot invent structure that the containment tree does not already express.

use karet_core::Range;
use karet_treesitter::LanguageId;
use karet_treesitter::WalkNode;

use crate::edge::EdgeKind;
use crate::model::Facet;
use crate::model::FacetSubtype;
use crate::model::Lens;
use crate::model::NodeKind;
use crate::model::Visibility;
use crate::text::LineIndex;

pub mod python;
pub mod rust;
#[cfg(feature = "lang-swift")]
pub mod swift;
#[cfg(feature = "lang-javascript")]
pub mod typescript;

/// What a language made of one syntax node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Classified {
    /// Which universal kind the construct maps onto.
    pub kind: NodeKind,
    /// The display name shown in the spine.
    pub name: String,
    /// The path segment naming it, braced when the construct is anonymous.
    ///
    /// Usually identical to `name`; they diverge for constructs that have no name of
    /// their own, where the segment describes the construct instead.
    pub segment: String,
    /// A short signature or descriptor for the facet pane.
    pub detail: Option<String>,
    /// The range to reveal when navigating here — the name, not the whole body.
    pub selection: Range,
    /// The declared visibility, when the language expresses one.
    pub visibility: Option<Visibility>,
}

/// One attribute attached to the construct being classified.
///
/// Attributes are collected by the extractor rather than the language because in most
/// grammars they are *siblings* of the item they decorate, not children — so only
/// something tracking sibling order can pair them up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute {
    /// The attribute's name — `cfg`, `derive`, `no_mangle`.
    pub name: String,
    /// The raw argument text, without the enclosing delimiters.
    pub arguments: Option<String>,
    /// Where the attribute is written.
    pub range: Range,
}

impl Attribute {
    /// The arguments, or an empty string when there are none.
    #[must_use]
    pub fn args(&self) -> &str {
        self.arguments.as_deref().unwrap_or("")
    }
}

/// One candidate semantic owner for a construct written away from what it belongs to.
///
/// A candidate is a *name*, not a node: a language mapping sees one syntax node at a time
/// and never the index, so it cannot know whether `Widget` exists, let alone where. It
/// says what it is looking for and what to become if that is found; resolving the name is
/// [`crate::regroup`]'s job, and a name that resolves to nothing leaves the node exactly
/// where it was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    /// The name to look for — a bare name, or a path the language's own syntax allows.
    pub name: String,
    /// Whether the construct dissolves into this owner rather than nesting under it.
    ///
    /// Dissolving lifts the construct's children into the owner and removes the
    /// construct: a Rust inherent `impl` is not a thing anyone navigates to, it is where
    /// the methods happen to be written. A trait binding *is* a thing, and nests.
    pub dissolve: bool,
    /// The display name and path segment to take on if this candidate is the one that
    /// resolves, or `None` to keep what the construct already has.
    ///
    /// Which candidate wins changes what the construct should be called: `impl Display
    /// for Widget` reads as `impl Display` under `Widget`, and as `impl for Widget` under
    /// `Display`. Naming is therefore per candidate rather than per construct.
    pub rename: Option<(String, String)>,
}

impl Owner {
    /// A candidate that nests under `name`, keeping the construct's own naming.
    #[must_use]
    pub fn nested(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            dissolve: false,
            rename: None,
        }
    }

    /// A candidate that lifts the construct's children into `name` and removes it.
    #[must_use]
    pub fn dissolved(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            dissolve: true,
            rename: None,
        }
    }

    /// Rename the construct to `name`, addressed by `segment`, if this candidate wins.
    #[must_use]
    pub fn renamed(mut self, name: impl Into<String>, segment: impl Into<String>) -> Self {
        self.rename = Some((name.into(), segment.into()));
        self
    }
}

/// Everything a language needs to classify one node beyond the node itself.
pub struct FacetContext<'a> {
    /// The full source text the tree was parsed from.
    pub text: &'a str,
    /// Line table for the same text, for turning spans into navigable ranges.
    pub lines: &'a LineIndex,
    /// The attributes decorating this node.
    pub attributes: &'a [Attribute],
    /// The kind of the nearest enclosing node, when there is one.
    ///
    /// This is what separates a method in a trait (a substitutable default) from the
    /// same syntax in an inherent implementation (not one).
    pub container: Option<NodeKind>,
    /// Type parameter names in scope on the enclosing declaration.
    ///
    /// A blanket implementation is one whose self type *is* one of these, and that is
    /// only decidable with the enclosing declaration's parameters in hand.
    pub type_parameters: &'a [String],
}

impl FacetContext<'_> {
    /// The line/column range for a byte span in this file.
    #[must_use]
    pub fn range(&self, span: karet_core::Span) -> Range {
        self.lines.range(self.text, span)
    }

    /// The first attribute with this name.
    #[must_use]
    pub fn attribute(&self, name: &str) -> Option<&Attribute> {
        self.attributes.iter().find(|attr| attr.name == name)
    }

    /// Every attribute with this name.
    pub fn attributes_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Attribute> {
        self.attributes.iter().filter(move |attr| attr.name == name)
    }
}

/// A language's mapping into the seam vocabulary.
///
/// Object-safe, so languages are registered as trait objects and a new one is additive.
pub trait SeamLanguage {
    /// Which grammar this maps.
    fn language(&self) -> LanguageId;

    /// Map one syntax node onto a universal kind, or `None` if it is not a node at all.
    ///
    /// Returning `None` is the common case — most syntax nodes are not addressable
    /// entities, and the walk simply descends past them.
    fn classify(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Option<Classified>;

    /// The seam properties this node carries.
    fn facets_of(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Vec<Facet>;

    /// Facets contributed by a node that is *not* itself an entity — an `unsafe` block,
    /// an await point, a `dyn` type — attributed to the nearest enclosing entity.
    ///
    /// This is what keeps containment a tree: three `unsafe` blocks in one function are
    /// three sites on the function, not three nodes.
    fn interior_facets(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Vec<Facet>;

    /// Whether a node of this kind introduces a new scope for path purposes.
    ///
    /// A Rust `declaration_list` is a body, not a scope; the `mod_item` around it is.
    fn is_container(&self, node: &WalkNode<'_>) -> bool;

    /// Read this node as a decoration applying to the sibling that follows it.
    ///
    /// Rust's `#[cfg(unix)]` and Python's `@decorator` are both decorations written
    /// before what they decorate, and in both grammars they parse as a *sibling* of it —
    /// Python's wrapping `decorated_definition` node holds the decorator and the
    /// definition side by side. So one pairing rule serves both, and the only
    /// language-specific part is reading the decoration itself, which is this.
    fn decoration(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Option<Attribute> {
        let _ = (node, ctx);
        None
    }

    /// The type parameter names this node declares, for blanket-implementation detection.
    ///
    /// A language without generics leaves this empty and nothing downstream notices.
    fn type_parameters(&self, node: &WalkNode<'_>, text: &str) -> Vec<String> {
        let _ = (node, text);
        Vec::new()
    }

    /// Where this construct's members really belong, when its syntactic parent is not
    /// its semantic owner — most specific candidate first.
    ///
    /// Some constructs are written away from the thing they belong to. A Rust `impl` and
    /// a Swift `extension` hold members of a type declared elsewhere; a Kotlin extension
    /// function is a member of its receiver; a JavaScript `X.prototype.y = …` is a member
    /// of `X`. Containment built from syntax alone puts all of them beside the type
    /// rather than inside it, and "show me everything about `Widget`" stops being a
    /// question the tree can answer.
    ///
    /// Returning candidates rather than a decision keeps the contract narrow: the
    /// language still sees one node and never the index, and it is the neutral
    /// [`crate::regroup`] pass that decides whether a name resolves. Empty — the default
    /// — means the construct is already written where it belongs, which is the common
    /// case and the whole of Python.
    fn ownership(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Vec<Owner> {
        let _ = (node, ctx);
        Vec::new()
    }

    /// The declaration head: everything from the construct's start up to the body it
    /// opens, or the whole construct when it opens none.
    ///
    /// The default reads the grammar's `body` field, which every grammar mapped here
    /// names the same way, so a language overrides this only where its grammar disagrees.
    fn header(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Range {
        header_before_body(node, ctx)
    }

    /// The name of a module whose body lives in *another file*, when this node declares one.
    ///
    /// Rust's `mod net;` is the case: a containment edge that crosses a file boundary and
    /// has to be followed to build a whole-package tree. A language whose modules never
    /// span files leaves this at its default and loses nothing.
    fn external_module(&self, node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Option<String> {
        let _ = (node, ctx);
        None
    }

    /// The edge kinds this language's semantic tier can resolve. May be empty.
    fn semantic_capabilities(&self) -> &'static [EdgeKind];

    /// Every facet subtype this language can emit, for query-term suggestions.
    fn subtypes(&self) -> &'static [(Lens, FacetSubtype)];
}

/// The head of a construct that names its body with a `body` field.
///
/// Shared rather than defaulted-in-place so a language that overrides
/// [`SeamLanguage::header`] for one construct can still fall back to it for the rest.
///
/// Where the cut lands is decided by the *line*, not by the grammar, because grammars
/// disagree about where a body begins and readers do not. Rust's `body` is the `{` that
/// closes the signature's line; Python's is the first statement, a line below the `:`.
/// Cutting at the body would therefore give Rust its whole signature and Python its
/// signature plus a stolen line of code. So a body that opens its own line — nothing but
/// whitespace before it — cuts at the end of the line above instead.
#[must_use]
pub fn header_before_body(node: &WalkNode<'_>, ctx: &FacetContext<'_>) -> Range {
    let span = node.span();
    // A body that starts at or before the construct does is not a body we can cut at;
    // the whole extent is the honest answer rather than an inverted range.
    let Some(body) = node
        .child_span("body")
        .filter(|body| body.start > span.start)
    else {
        return ctx.range(span);
    };
    let end = line_opening_body(ctx.text, span.start.0, body.start.0).unwrap_or(body.start.0);
    ctx.range(karet_core::Span {
        start: span.start,
        end: karet_core::BytePos(end),
    })
}

/// The end of the line above `body`, when `body` opens a line of its own.
///
/// `None` when the body shares its line with the signature, when it is the construct's
/// own first line, or when the line above would leave nothing of the head at all.
fn line_opening_body(text: &str, start: usize, body: usize) -> Option<usize> {
    let before = text.get(start..body)?;
    let newline = start.checked_add(before.rfind('\n')?)?;
    if !text.get(newline.checked_add(1)?..body)?.trim().is_empty() {
        return None;
    }
    (newline > start).then_some(newline)
}

/// Look up the mapping for a grammar, or `None` when the language has none.
///
/// A language with no mapping is not an error: its files are simply not indexed, and
/// the view says so rather than showing an empty tree that implies there is nothing there.
#[must_use]
pub fn for_language(language: LanguageId) -> Option<&'static dyn SeamLanguage> {
    #[cfg(feature = "lang-rust")]
    if let Some(rust) = rust::language_id()
        && rust == language
    {
        return Some(rust::mapping());
    }
    #[cfg(feature = "lang-python")]
    if let Some(python) = python::language_id()
        && python == language
    {
        return Some(python::mapping());
    }
    #[cfg(feature = "lang-javascript")]
    if typescript::language_ids().contains(&language) {
        return Some(typescript::mapping());
    }
    #[cfg(feature = "lang-swift")]
    if let Some(swift) = swift::language_id()
        && swift == language
    {
        return Some(swift::mapping());
    }
    let _ = language;
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::LineIndex;

    fn attribute(name: &str, arguments: Option<&str>) -> Attribute {
        Attribute {
            name: name.to_owned(),
            arguments: arguments.map(str::to_owned),
            range: Range::default(),
        }
    }

    #[test]
    fn attribute_args_default_to_empty() {
        assert_eq!(attribute("no_mangle", None).args(), "");
        assert_eq!(attribute("cfg", Some("unix")).args(), "unix");
    }

    #[test]
    fn context_finds_attributes_by_name() {
        let text = "";
        let lines = LineIndex::new(text);
        let attributes = vec![
            attribute("cfg", Some("unix")),
            attribute("derive", Some("Clone")),
            attribute("cfg", Some("test")),
        ];
        let ctx = FacetContext {
            text,
            lines: &lines,
            attributes: &attributes,
            container: None,
            type_parameters: &[],
        };
        assert_eq!(ctx.attribute("cfg").map(Attribute::args), Some("unix"));
        assert_eq!(ctx.attributes_named("cfg").count(), 2);
        assert_eq!(ctx.attributes_named("missing").count(), 0);
        assert_eq!(ctx.attribute("missing"), None);
    }

    #[test]
    fn an_owner_candidate_carries_how_it_wants_to_be_attached() {
        assert_eq!(
            Owner::nested("Widget"),
            Owner {
                name: "Widget".to_owned(),
                dissolve: false,
                rename: None,
            }
        );
        let dissolved = Owner::dissolved("Widget");
        assert!(dissolved.dissolve);
        // Naming is per candidate: which owner wins changes what the node should read as.
        let renamed = Owner::nested("Widget").renamed("impl Display", "{impl Display}");
        assert_eq!(
            renamed.rename,
            Some(("impl Display".to_owned(), "{impl Display}".to_owned()))
        );
    }

    #[test]
    fn a_language_owns_nothing_elsewhere_by_default() {
        // The common case, and the whole of Python: constructs are written where they
        // belong, so the regroup pass has nothing to do.
        struct Bare;
        impl SeamLanguage for Bare {
            fn language(&self) -> LanguageId {
                LanguageId(u16::MAX)
            }
            fn classify(&self, _: &WalkNode<'_>, _: &FacetContext<'_>) -> Option<Classified> {
                None
            }
            fn facets_of(&self, _: &WalkNode<'_>, _: &FacetContext<'_>) -> Vec<Facet> {
                Vec::new()
            }
            fn interior_facets(&self, _: &WalkNode<'_>, _: &FacetContext<'_>) -> Vec<Facet> {
                Vec::new()
            }
            fn is_container(&self, _: &WalkNode<'_>) -> bool {
                false
            }
            fn semantic_capabilities(&self) -> &'static [EdgeKind] {
                &[]
            }
            fn subtypes(&self) -> &'static [(Lens, FacetSubtype)] {
                &[]
            }
        }
        // Reached through the trait object, since that is how the extractor sees it.
        let language: &dyn SeamLanguage = &Bare;
        assert_eq!(language.subtypes().len(), 0);
    }

    #[test]
    fn a_body_on_its_own_line_leaves_that_line_out_of_the_head() {
        // `fn f()\n{` — the brace line carries nothing a reader wants.
        assert_eq!(line_opening_body("fn f()\n{}", 0, 7), Some(6));
        // `fn f() {` — the brace shares the signature's line, so there is nothing to cut.
        assert_eq!(line_opening_body("fn f() {}", 0, 7), None);
        // A body starting on the construct's own first line has no line above to end at.
        assert_eq!(line_opening_body("{}", 0, 0), None);
    }

    #[test]
    fn an_unmapped_language_resolves_to_nothing() {
        // Language id 0 is never a mapped grammar; the caller degrades rather than failing.
        assert!(for_language(LanguageId(u16::MAX)).is_none());
    }
}
