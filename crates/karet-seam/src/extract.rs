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
use crate::lang::Owner;
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
    name: String,
    path: SeamPath,
    type_parameters: Vec<String>,
}

/// A module declaration whose body lives in another file.
///
/// The package walk resolves these; a single-file extraction cannot, because the file it
/// would have to read is exactly what it does not have.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalModule {
    /// The module node already added to the index, awaiting its contents.
    pub id: SeamId,
    /// The declared module name.
    pub name: String,
    /// The inline modules enclosing the declaration, outermost first.
    pub inline_path: Vec<String>,
    /// The `#[path = "…"]` override, when one is written.
    pub path_attribute: Option<String>,
}

/// What one file's extraction produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtractOutcome {
    /// Every node added, in source order.
    pub added: Vec<SeamId>,
    /// Module declarations whose bodies live elsewhere.
    pub external_modules: Vec<ExternalModule>,
    /// Nodes whose semantic owner is named elsewhere, with the candidates to try.
    ///
    /// Recorded rather than acted on, because the owner may be declared in a file this
    /// extraction has not read yet — a Rust `impl` and its type routinely live apart.
    /// The package layer resolves these once every file is in.
    pub ownership: Vec<(SeamId, Vec<Owner>)>,
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
) -> Result<ExtractOutcome, ExtractError> {
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
            name: String::new(),
            path: parent_path,
            type_parameters: Vec::new(),
        }],
        pending: Vec::new(),
        ordinals: HashMap::new(),
        outcome: ExtractOutcome::default(),
    };
    tree.walk(|node| extractor.visit(node));
    Ok(extractor.outcome)
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
    outcome: ExtractOutcome,
}

impl Extractor<'_> {
    /// Visit one node: maintain the stack, then classify it or attribute its facets.
    fn visit(&mut self, node: &WalkNode<'_>) -> WalkControl {
        let depth = node.depth();
        // Everything deeper than this node is finished, so unwind to its level.
        while self.stack.len() > 1 && self.stack.last().is_some_and(|f| f.depth >= depth) {
            self.stack.pop();
        }

        // A decoration is read by the language, since only it knows what one looks
        // like, and queued for the sibling it decorates.
        let bare = self.context(&[]);
        if let Some(attribute) = self.mapping.decoration(node, &bare) {
            self.pending.push((depth, attribute));
            // Parsed whole; its internals are not entities.
            return WalkControl::SkipSubtree;
        }

        // Decorations bind to the immediately following sibling. Take them for this node,
        // whatever it turns out to be — leaving them queued would let a `#[cfg]` on a
        // discarded construct leak onto the next real declaration.
        let attributes = self.take_pending(depth);
        let ctx = self.context(&attributes);

        match self.mapping.classify(node, &ctx) {
            Some(classified) => {
                let facets = self.mapping.facets_of(node, &ctx);
                let external = self.mapping.external_module(node, &ctx);
                let owners = self.mapping.ownership(node, &ctx);
                let inline_path = self.inline_module_path();
                let id = self.push_entity(node, classified, facets, depth);
                if let (Some(id), false) = (id, owners.is_empty()) {
                    self.outcome.ownership.push((id, owners));
                }
                if let (Some(name), Some(id)) = (external, id) {
                    self.outcome.external_modules.push(ExternalModule {
                        id,
                        name,
                        inline_path,
                        path_attribute: attributes
                            .iter()
                            .find(|attr| attr.name == "path")
                            .and_then(|attr| attr.arguments.clone()),
                    });
                }
            },
            None => {
                let facets = self.mapping.interior_facets(node, &ctx);
                self.attach_interior(facets);
            },
        }
        WalkControl::Descend
    }

    /// Build the context a language sees, over the given decorations.
    fn context<'a>(&'a self, attributes: &'a [Attribute]) -> FacetContext<'a> {
        FacetContext {
            text: self.text,
            lines: self.lines,
            attributes,
            container: self.stack.last().map(|f| f.kind),
            type_parameters: self.stack.last().map_or(&[][..], |f| &f.type_parameters),
        }
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
    ) -> Option<SeamId> {
        let (parent_id, parent_path) = self
            .stack
            .last()
            .map(|frame| (frame.id, frame.path.clone()))?;
        let header = self.mapping.header(node, &self.context(&[]));
        let segment = self.next_segment(parent_id, classified.segment.clone());
        let path = parent_path.child(segment);
        let id = self.index.intern(path.clone());
        let type_parameters = self.mapping.type_parameters(node, self.text);

        let name = classified.name.clone();
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
                header,
            },
            parent: Some(parent_id),
            children: Vec::new(),
            facets,
            visibility: classified.visibility,
            rollups: Rollups::new(),
            membership: ConfigMembership::Active,
            provisional: node.has_error(),
        });
        self.outcome.added.push(id);
        self.stack.push(Frame {
            depth,
            id,
            kind: classified.kind,
            name,
            path,
            type_parameters,
        });
        Some(id)
    }

    /// The inline modules enclosing the current position, outermost first.
    ///
    /// Frame 0 is the file's own module, which is not inline — everything above it was
    /// opened by a `mod … { … }` in this file, and each one deepens where a nested
    /// `mod x;` looks for its file.
    fn inline_module_path(&self) -> Vec<String> {
        self.stack
            .iter()
            .skip(1)
            .filter(|frame| frame.kind == NodeKind::Module)
            .map(|frame| frame.name.clone())
            .collect()
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
}
