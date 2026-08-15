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

pub mod rust;

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

    /// The edge kinds this language's semantic tier can resolve. May be empty.
    fn semantic_capabilities(&self) -> &'static [EdgeKind];

    /// Every facet subtype this language can emit, for query-term suggestions.
    fn subtypes(&self) -> &'static [(Lens, FacetSubtype)];
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
    fn an_unmapped_language_resolves_to_nothing() {
        // Language id 0 is never a mapped grammar; the caller degrades rather than failing.
        assert!(for_language(LanguageId(u16::MAX)).is_none());
    }
}
