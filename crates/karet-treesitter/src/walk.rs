//! A neutral pre-order walk over a parsed tree.
//!
//! Queries answer "find me these nodes". Some consumers instead need to *traverse*,
//! carrying context down as they go — a visibility modifier that governs everything
//! nested under it, a `cfg` attribute that gates a whole item, an `unsafe` block whose
//! enclosing function is what matters. Captures cannot express that, because a capture
//! arrives with no memory of what contained it.
//!
//! [`SyntaxTree::walk`] supplies the traversal without breaching this crate's rule that
//! no tree-sitter handle escapes: the visitor borrows a [`WalkNode`] for the duration of
//! one call and cannot retain it or reach the underlying node through it. What it can
//! read is neutral — kind, field name, byte span, depth, and the node's children.
//!
//! ```no_run
//! # use karet_treesitter::{ParserPool, SyntaxTree, WalkControl, language_id_from_path};
//! # use std::path::Path;
//! # fn demo(pool: &mut ParserPool, text: &str) -> Result<(), karet_treesitter::TsError> {
//! # let lang = language_id_from_path(Path::new("x.rs"));
//! let tree = SyntaxTree::parse(pool, lang, text)?;
//! let mut functions = Vec::new();
//! tree.walk(|node| {
//!     if node.kind() == "function_item" {
//!         // A nested function is not interesting here, so do not descend.
//!         functions.push(node.span());
//!         return WalkControl::SkipSubtree;
//!     }
//!     WalkControl::Descend
//! });
//! # Ok(())
//! # }
//! ```

use karet_core::BytePos;
use karet_core::Span;

use crate::SyntaxTree;

/// What the walk should do after visiting a node.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WalkControl {
    /// Visit this node's children next.
    #[default]
    Descend,
    /// Skip this node's children and continue with its next sibling.
    SkipSubtree,
    /// End the walk entirely.
    Stop,
}

/// One node being visited by [`SyntaxTree::walk`].
///
/// Borrowed for the duration of a single visitor call. Everything it exposes is neutral
/// data — no tree-sitter type is reachable through it, so the walk cannot be used to
/// smuggle a node handle out of this crate.
pub struct WalkNode<'tree> {
    node: tree_sitter::Node<'tree>,
    field: Option<&'static str>,
    depth: u16,
}

impl<'tree> WalkNode<'tree> {
    /// The grammar-defined node kind, for example `function_item` or `visibility_modifier`.
    #[must_use]
    pub fn kind(&self) -> &'tree str {
        self.node.kind()
    }

    /// The field this node fills in its parent (`name`, `body`, `type`, …), when the
    /// grammar names one.
    ///
    /// Field names disambiguate same-kinded children: an `impl_item`'s `trait` and
    /// `type` fields are both type nodes, and only the field tells them apart.
    #[must_use]
    pub fn field_name(&self) -> Option<&'static str> {
        self.field
    }

    /// The byte range this node occupies in the document.
    #[must_use]
    pub fn span(&self) -> Span {
        Span {
            start: BytePos(self.node.start_byte()),
            end: BytePos(self.node.end_byte()),
        }
    }

    /// Depth below the tree root, which is depth `0`.
    ///
    /// A visitor that maintains its own containment stack pops every entry at a depth
    /// greater than or equal to this one before pushing, which is what makes a pre-order
    /// walk sufficient to reconstruct nesting.
    #[must_use]
    pub fn depth(&self) -> u16 {
        self.depth
    }

    /// Whether this is a *named* node rather than an anonymous token.
    ///
    /// [`SyntaxTree::walk`] only visits named nodes; this is `true` there. It is
    /// informative on children yielded by [`children`](Self::children), which include
    /// anonymous tokens such as the `unsafe` keyword.
    #[must_use]
    pub fn is_named(&self) -> bool {
        self.node.is_named()
    }

    /// Whether this node or anything under it failed to parse.
    ///
    /// A seam or outline extractor uses this to mark a subtree's facts as provisional
    /// instead of discarding what the grammar still recovered.
    #[must_use]
    pub fn has_error(&self) -> bool {
        self.node.has_error()
    }

    /// The span of the child filling `field`, if the grammar names one.
    #[must_use]
    pub fn child_span(&self, field: &str) -> Option<Span> {
        self.node.child_by_field_name(field).map(|child| Span {
            start: BytePos(child.start_byte()),
            end: BytePos(child.end_byte()),
        })
    }

    /// The text of the child filling `field`, sliced out of `text`.
    ///
    /// `text` must be the same source the tree was parsed from; a span that is not on a
    /// character boundary (only possible with mismatched text) yields `None` rather than
    /// panicking.
    #[must_use]
    pub fn child_text<'src>(&self, field: &str, text: &'src str) -> Option<&'src str> {
        let span = self.child_span(field)?;
        text.get(span.start.0..span.end.0)
    }

    /// This node's own text, sliced out of `text`.
    ///
    /// Returns `None` when the span is not on a character boundary of `text`, which
    /// means `text` is not the source this tree was parsed from.
    #[must_use]
    pub fn text<'src>(&self, text: &'src str) -> Option<&'src str> {
        let span = self.span();
        text.get(span.start.0..span.end.0)
    }

    /// Every direct child, named and anonymous alike, in source order.
    ///
    /// Anonymous tokens are included because they carry meaning a facet extractor
    /// needs: `unsafe` and `async` in a Rust `function_modifiers` node are anonymous.
    pub fn children(&self) -> impl Iterator<Item = WalkNode<'tree>> + '_ {
        let mut cursor = self.node.walk();
        let depth = self.depth.saturating_add(1);
        let mut has_child = cursor.goto_first_child();
        std::iter::from_fn(move || {
            if !has_child {
                return None;
            }
            let child = WalkNode {
                node: cursor.node(),
                field: cursor.field_name(),
                depth,
            };
            has_child = cursor.goto_next_sibling();
            Some(child)
        })
    }

    /// Whether any direct child has the given kind.
    ///
    /// The cheap shape of the common test: does this function carry the `unsafe`
    /// token, does this impl carry a `where` clause.
    #[must_use]
    pub fn has_child_kind(&self, kind: &str) -> bool {
        self.children().any(|child| child.kind() == kind)
    }
}

impl std::fmt::Debug for WalkNode<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WalkNode")
            .field("kind", &self.kind())
            .field("field", &self.field)
            .field("span", &self.span())
            .field("depth", &self.depth)
            .finish()
    }
}

impl SyntaxTree {
    /// Walk every *named* node in pre-order (a parent before its children, siblings in
    /// source order), letting `visit` steer the descent.
    ///
    /// The visitor's [`WalkControl`] decides what happens next: descend into the node's
    /// children, skip them, or end the walk. Anonymous tokens are not visited — reach
    /// them through [`WalkNode::children`] on the node that contains them.
    ///
    /// A tree with parse errors is still walked; the nodes tree-sitter recovered are
    /// visited as usual and [`WalkNode::has_error`] marks the damaged subtrees.
    pub fn walk(&self, mut visit: impl FnMut(&WalkNode<'_>) -> WalkControl) {
        let mut cursor = self.tree.walk();
        let mut depth: u16 = 0;
        'walk: loop {
            let node = cursor.node();
            let mut descend = true;
            if node.is_named() {
                let visited = WalkNode {
                    node,
                    field: cursor.field_name(),
                    depth,
                };
                match visit(&visited) {
                    WalkControl::Descend => {},
                    WalkControl::SkipSubtree => descend = false,
                    WalkControl::Stop => break 'walk,
                }
            }
            // Iterative pre-order DFS: descend to the first child, else advance to the
            // next sibling, else climb until a sibling exists.
            if descend && cursor.goto_first_child() {
                depth = depth.saturating_add(1);
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    continue 'walk;
                }
                if !cursor.goto_parent() {
                    break 'walk;
                }
                depth = depth.saturating_sub(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LanguageId;
    use crate::ParserPool;
    use crate::language_id_from_injection_name;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn rust() -> Option<LanguageId> {
        language_id_from_injection_name("rust")
    }

    fn parse(text: &str) -> Option<SyntaxTree> {
        let lang = rust()?;
        let mut pool = ParserPool::new();
        SyntaxTree::parse(&mut pool, lang, text).ok()
    }

    #[test]
    fn visits_named_nodes_in_pre_order() -> TestResult {
        let Some(tree) = parse("fn outer() { fn inner() {} }") else {
            return Ok(());
        };
        let mut kinds = Vec::new();
        tree.walk(|node| {
            kinds.push(node.kind().to_owned());
            WalkControl::Descend
        });
        // The root comes first, and an outer function precedes the one nested in it.
        assert_eq!(kinds.first().map(String::as_str), Some("source_file"));
        let outer = kinds.iter().position(|k| k == "function_item");
        let inner = kinds.iter().rposition(|k| k == "function_item");
        assert!(
            outer < inner,
            "outer must be visited before inner: {kinds:?}"
        );
        Ok(())
    }

    #[test]
    fn skip_subtree_prunes_children_but_continues_with_siblings() -> TestResult {
        let Some(tree) = parse("fn first() { fn buried() {} }\nfn second() {}") else {
            return Ok(());
        };
        let mut names = Vec::new();
        tree.walk(|node| {
            if node.kind() == "function_item" {
                let text = node.child_text("name", "fn first() { fn buried() {} }\nfn second() {}");
                names.push(text.unwrap_or_default().to_owned());
                return WalkControl::SkipSubtree;
            }
            WalkControl::Descend
        });
        assert_eq!(names, ["first", "second"], "buried must be pruned");
        Ok(())
    }

    #[test]
    fn stop_ends_the_walk_immediately() -> TestResult {
        let Some(tree) = parse("fn a() {}\nfn b() {}\nfn c() {}") else {
            return Ok(());
        };
        let mut seen = 0usize;
        tree.walk(|node| {
            if node.kind() == "function_item" {
                seen += 1;
                return WalkControl::Stop;
            }
            WalkControl::Descend
        });
        assert_eq!(seen, 1);
        Ok(())
    }

    #[test]
    fn depth_tracks_nesting_and_returns_after_climbing() -> TestResult {
        let src = "mod m { fn f() {} }\nfn g() {}";
        let Some(tree) = parse(src) else {
            return Ok(());
        };
        let mut depths = Vec::new();
        tree.walk(|node| {
            if node.kind() == "mod_item" || node.kind() == "function_item" {
                depths.push((
                    node.child_text("name", src).unwrap_or_default().to_owned(),
                    node.depth(),
                ));
            }
            WalkControl::Descend
        });
        let depth_of = |name: &str| depths.iter().find(|(n, _)| n == name).map(|(_, d)| *d);
        assert_eq!(depth_of("m"), Some(1));
        // `f` is nested inside `m`'s declaration list, so it is deeper than `m`.
        assert!(depth_of("f") > depth_of("m"));
        // `g` is a sibling of `m`, so the walk climbed back to the same depth.
        assert_eq!(depth_of("g"), Some(1));
        Ok(())
    }

    #[test]
    fn field_names_disambiguate_same_kinded_children() -> TestResult {
        let src = "impl Display for Widget {}";
        let Some(tree) = parse(src) else {
            return Ok(());
        };
        let mut trait_text = None;
        let mut type_text = None;
        tree.walk(|node| {
            if node.kind() == "impl_item" {
                trait_text = node.child_text("trait", src).map(str::to_owned);
                type_text = node.child_text("type", src).map(str::to_owned);
            }
            WalkControl::Descend
        });
        assert_eq!(trait_text.as_deref(), Some("Display"));
        assert_eq!(type_text.as_deref(), Some("Widget"));
        Ok(())
    }

    #[test]
    fn children_include_anonymous_tokens() -> TestResult {
        let src = "unsafe fn danger() {}";
        let Some(tree) = parse(src) else {
            return Ok(());
        };
        let mut found_unsafe = false;
        tree.walk(|node| {
            // `unsafe` is an anonymous token, so it is never visited directly — it is
            // only reachable through the modifiers node's children.
            if node.kind() == "function_modifiers" {
                found_unsafe = node.has_child_kind("unsafe");
                assert!(node.children().any(|c| !c.is_named()));
            }
            WalkControl::Descend
        });
        assert!(found_unsafe, "the unsafe token must be reachable");
        Ok(())
    }

    #[test]
    fn child_span_and_text_agree() -> TestResult {
        let src = "fn hello() {}";
        let Some(tree) = parse(src) else {
            return Ok(());
        };
        tree.walk(|node| {
            if node.kind() == "function_item"
                && let Some(span) = node.child_span("name")
            {
                assert_eq!(src.get(span.start.0..span.end.0), Some("hello"));
                assert_eq!(node.child_text("name", src), Some("hello"));
            }
            WalkControl::Descend
        });
        Ok(())
    }

    #[test]
    fn node_text_returns_none_for_mismatched_source() -> TestResult {
        let Some(tree) = parse("fn hello() {}") else {
            return Ok(());
        };
        let mut checked = false;
        tree.walk(|node| {
            if node.kind() == "function_item" {
                // A shorter, unrelated buffer cannot contain the span.
                assert_eq!(node.text("x"), None);
                checked = true;
                return WalkControl::Stop;
            }
            WalkControl::Descend
        });
        assert!(checked);
        Ok(())
    }

    #[test]
    fn a_broken_buffer_still_walks_and_reports_the_damage() -> TestResult {
        // A usable tree from invalid input is the whole point for a structural consumer.
        let src = "fn good() {}\nfn broken( {";
        let Some(tree) = parse(src) else {
            return Ok(());
        };
        let mut names = Vec::new();
        tree.walk(|node| {
            if node.kind() == "function_item" {
                names.push(node.child_text("name", src).unwrap_or_default().to_owned());
            }
            WalkControl::Descend
        });
        assert!(
            names.contains(&"good".to_owned()),
            "the intact declaration must survive: {names:?}"
        );
        assert!(
            !tree.error_lines().is_empty(),
            "the damaged region must remain visible"
        );
        Ok(())
    }

    #[test]
    fn walking_an_empty_document_visits_only_the_root() -> TestResult {
        let Some(tree) = parse("") else {
            return Ok(());
        };
        let mut kinds = Vec::new();
        tree.walk(|node| {
            kinds.push(node.kind().to_owned());
            WalkControl::Descend
        });
        assert_eq!(kinds, ["source_file"]);
        Ok(())
    }
}
