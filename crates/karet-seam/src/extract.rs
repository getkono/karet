//! Driving a parse into index nodes.
//!
//! The walk is pre-order, so this maintains its own containment stack and pairs
//! attributes with the sibling they decorate. Three things are resolved here rather than
//! in a language mapping, because none of them are decidable from a single node:
//!
//! - **Paths and ordinals.** A node's identity is its position in the hierarchy, which
//!   only the stack knows, and a disambiguating ordinal needs its siblings.
//! - **Attribute pairing.** Attributes are siblings of the item they decorate, so
//!   something tracking sibling order has to hand them over.
//! - **Interior attribution.** An `unsafe` block belongs to the function around it, and
//!   only the stack knows which function that is.

use std::collections::HashMap;

use karet_treesitter::LanguageId;
use karet_treesitter::ParserPool;
use karet_treesitter::SyntaxTree;
use karet_treesitter::WalkControl;
use karet_treesitter::WalkNode;

use crate::id::SeamId;
use crate::id::SeamPath;
use crate::id::SeamSegment;
use crate::index::SeamIndex;
use crate::lang::Attribute;
use crate::lang::FacetContext;
use crate::lang::SeamLanguage;
use crate::lang::for_language;
use crate::model::ConfigMembership;
use crate::model::Facet;
use crate::model::FileId;
use crate::model::Node;
use crate::model::NodeKind;
use crate::model::SeamLocation;
use crate::rollup::Rollups;
use crate::text::LineIndex;

/// Errors extracting a file into the index.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ExtractError {
    /// The grammar for this language is not compiled in.
    #[error("no grammar compiled in for this language")]
    NoGrammar,
    /// The language has no seam mapping, so its files are not indexed.
    #[error("no seam mapping for this language")]
    NoMapping,
    /// The parser could not produce a tree at all.
    #[error("the file could not be parsed")]
    ParseFailed,
}

/// One entry on the containment stack.
struct Frame {
    depth: u16,
    id: SeamId,
    kind: NodeKind,
    path: SeamPath,
    type_parameters: Vec<String>,
}

/// Extract every entity in `text` into `index`, nested under `parent`.
///
/// A file that fails to parse cleanly is still extracted: tree-sitter's recovery leaves
/// the intact declarations addressable, and each node from a damaged subtree is marked
/// [`Node::provisional`] so the view can say the facts there are incomplete rather than
/// silently showing fewer of them.
///
/// # Errors
/// [`ExtractError::NoGrammar`] when the grammar is not compiled in,
/// [`ExtractError::NoMapping`] when the language has no seam mapping, and
/// [`ExtractError::ParseFailed`] when no tree could be produced at all.
pub fn extract_file(
    index: &mut SeamIndex,
    pool: &mut ParserPool,
    parent: SeamId,
    file: FileId,
    language: LanguageId,
    text: &str,
) -> Result<Vec<SeamId>, ExtractError> {
    let mapping = for_language(language).ok_or(ExtractError::NoMapping)?;
    let tree = SyntaxTree::parse(pool, language, text).map_err(|_| ExtractError::ParseFailed)?;
    let parent_path = index.path(parent).cloned().unwrap_or_default();
    // The root frame takes the real parent's kind. Assuming a module here would make a
    // crate-root `fn main` look like an ordinary nested function rather than an entry point.
    let parent_kind = index
        .node(parent)
        .map_or(NodeKind::Module, |node| node.kind);
    let lines = LineIndex::new(text);
    let mut extractor = Extractor {
        index,
        mapping,
        text,
        lines: &lines,
        file,
        stack: vec![Frame {
            depth: 0,
            id: parent,
            kind: parent_kind,
            path: parent_path,
            type_parameters: Vec::new(),
        }],
        pending: Vec::new(),
        ordinals: HashMap::new(),
        added: Vec::new(),
    };
    tree.walk(|node| extractor.visit(node));
    Ok(extractor.added)
}

/// Walk state: the containment stack, pending attributes, and sibling ordinals.
struct Extractor<'a> {
    index: &'a mut SeamIndex,
    mapping: &'static dyn SeamLanguage,
    text: &'a str,
    lines: &'a LineIndex,
    file: FileId,
    stack: Vec<Frame>,
    pending: Vec<(u16, Attribute)>,
    ordinals: HashMap<(SeamId, String), u32>,
    added: Vec<SeamId>,
}

impl Extractor<'_> {
    /// Visit one node: maintain the stack, then classify it or attribute its facets.
    fn visit(&mut self, node: &WalkNode<'_>) -> WalkControl {
        let depth = node.depth();
        // Everything deeper than this node is finished, so unwind to its level.
        while self.stack.len() > 1 && self.stack.last().is_some_and(|f| f.depth >= depth) {
            self.stack.pop();
        }

        if node.kind() == "attribute_item" {
            if let Some(attribute) = self.parse_attribute(node) {
                self.pending.push((depth, attribute));
            }
            // Parsed whole; its internals are not entities.
            return WalkControl::SkipSubtree;
        }

        // Attributes bind to the immediately following sibling. Take them for this node,
        // whatever it turns out to be — leaving them queued would let a `#[cfg]` on a
        // discarded construct leak onto the next real declaration.
        let attributes = self.take_pending(depth);

        let ctx = FacetContext {
            text: self.text,
            lines: self.lines,
            attributes: &attributes,
            container: self.stack.last().map(|f| f.kind),
            type_parameters: self.stack.last().map_or(&[][..], |f| &f.type_parameters),
        };

        match self.mapping.classify(node, &ctx) {
            Some(classified) => {
                let facets = self.mapping.facets_of(node, &ctx);
                self.push_entity(node, classified, facets, depth);
            },
            None => {
                let facets = self.mapping.interior_facets(node, &ctx);
                self.attach_interior(facets);
            },
        }
        WalkControl::Descend
    }

    /// Take every pending attribute queued at `depth`.
    fn take_pending(&mut self, depth: u16) -> Vec<Attribute> {
        let mut taken = Vec::new();
        self.pending.retain(|(at, attribute)| {
            if *at == depth {
                taken.push(attribute.clone());
                false
            } else {
                true
            }
        });
        taken
    }

    /// Add a classified entity to the index and make it the current container.
    fn push_entity(
        &mut self,
        node: &WalkNode<'_>,
        classified: crate::lang::Classified,
        facets: Vec<Facet>,
        depth: u16,
    ) {
        let Some((parent_id, parent_path)) = self
            .stack
            .last()
            .map(|frame| (frame.id, frame.path.clone()))
        else {
            return;
        };
        let segment = self.next_segment(parent_id, classified.segment.clone());
        let path = parent_path.child(segment);
        let id = self.index.intern(path.clone());
        let type_parameters = super::lang::rust::type_parameter_names(node, self.text);

        self.index.insert(Node {
            id,
            kind: classified.kind,
            name: classified.name,
            detail: classified.detail,
            location: SeamLocation {
                file: self.file,
                range: self.lines.range(self.text, node.span()),
                span: node.span(),
                selection: classified.selection,
            },
            parent: Some(parent_id),
            children: Vec::new(),
            facets,
            visibility: classified.visibility,
            rollups: Rollups::new(),
            membership: ConfigMembership::Active,
            provisional: node.has_error(),
        });
        self.added.push(id);
        self.stack.push(Frame {
            depth,
            id,
            kind: classified.kind,
            path,
            type_parameters,
        });
    }

    /// Disambiguate a segment against its siblings.
    ///
    /// The first occurrence keeps its bare name; later ones take a 1-based ordinal. This
    /// is positional by necessity — two `impl Widget` blocks are genuinely
    /// indistinguishable otherwise — so deleting the first renumbers the rest, and that
    /// is the one edit shape identity cannot survive.
    fn next_segment(&mut self, parent: SeamId, name: String) -> SeamSegment {
        let count = self
            .ordinals
            .entry((parent, name.clone()))
            .and_modify(|n| *n = n.saturating_add(1))
            .or_insert(1);
        if *count == 1 {
            SeamSegment::new(name)
        } else {
            SeamSegment::numbered(name, *count)
        }
    }

    /// Attribute interior facets to the innermost enclosing entity.
    ///
    /// Facets of the same subtype merge, accumulating their sites, so a function with
    /// three `unsafe` blocks carries one facet with three sites rather than three facets.
    fn attach_interior(&mut self, facets: Vec<Facet>) {
        if facets.is_empty() {
            return;
        }
        let Some(frame) = self.stack.last() else {
            return;
        };
        let Some(node) = self.index.node_mut(frame.id) else {
            return;
        };
        for facet in facets {
            match node
                .facets
                .iter_mut()
                .find(|existing| existing.lens == facet.lens && existing.subtype == facet.subtype)
            {
                Some(existing) => existing.sites.extend(facet.sites),
                None => node.facets.push(facet),
            }
        }
    }

    /// Read one `#[…]` attribute into its name and raw arguments.
    ///
    /// The edition-2024 `#[unsafe(no_mangle)]` wrapper is unwrapped to the attribute it
    /// carries, so callers match on `no_mangle` regardless of which spelling was used.
    fn parse_attribute(&self, node: &WalkNode<'_>) -> Option<Attribute> {
        let attribute = node.children().find(|child| child.kind() == "attribute")?;
        let mut name = attribute
            .children()
            .find(|child| matches!(child.kind(), "identifier" | "scoped_identifier"))
            .and_then(|child| child.text(self.text))?
            .to_owned();
        let mut arguments = attribute
            .child_text("arguments", self.text)
            .or_else(|| attribute.child_text("value", self.text))
            .map(|raw| raw.trim_matches(['(', ')', ' ']).to_owned());

        if name == "unsafe"
            && let Some(inner) = arguments.clone()
        {
            let (head, rest) = split_attribute(&inner);
            name = head;
            arguments = rest;
        }

        Some(Attribute {
            name,
            arguments,
            range: self.lines.range(self.text, node.span()),
        })
    }
}

/// Split `no_mangle` / `link_name = "x"` / `link(name = "z")` into name and arguments.
fn split_attribute(text: &str) -> (String, Option<String>) {
    let trimmed = text.trim();
    if let Some((head, rest)) = trimmed.split_once('(') {
        return (
            head.trim().to_owned(),
            Some(rest.trim_end_matches(')').trim().to_owned()),
        );
    }
    if let Some((head, rest)) = trimmed.split_once('=') {
        return (head.trim().to_owned(), Some(rest.trim().to_owned()));
    }
    (trimmed.to_owned(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_the_three_attribute_shapes() {
        assert_eq!(split_attribute("no_mangle"), ("no_mangle".to_owned(), None));
        assert_eq!(
            split_attribute("link_name = \"c_fn\""),
            ("link_name".to_owned(), Some("\"c_fn\"".to_owned()))
        );
        assert_eq!(
            split_attribute("link(name = \"z\")"),
            ("link".to_owned(), Some("name = \"z\"".to_owned()))
        );
    }
}
