//! The neutral outline model behind the right-side outline panel.
//!
//! A format-agnostic navigation tree: each producer converts its own model into an
//! [`OutlineEntry`], so one panel renders and one activation path drives them all.
//! PDF bookmarks, Markdown headings, and language-server symbols all convert into
//! this model without coupling the panel to their producer.

use std::cell::Cell;
use std::cell::RefCell;

use karet_core::LineCol;
use karet_core::Symbol;
#[cfg(test)]
use karet_core::SymbolKind;
use karet_markdown::Block;
use karet_markdown::Inline;
use karet_session::ViewId;

/// Where activating an outline entry navigates to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlineTarget {
    /// A page in a paginated document (0-based), e.g. a PDF bookmark.
    Page(usize),
    /// A text position, e.g. a code symbol or markdown heading.
    Text(LineCol),
}

/// One node in a document's navigation outline: a label, an optional navigation
/// target, and nested children.
#[derive(Clone, Debug)]
pub struct OutlineEntry {
    /// The row label.
    pub label: String,
    /// Where activating this entry navigates, or `None` for a pure grouping node.
    pub target: Option<OutlineTarget>,
    /// Nested child entries, in document order.
    pub children: Vec<OutlineEntry>,
}

/// One visible row of a flattened outline: its depth (for indentation), label, and
/// navigation target. Produced by [`flatten`] for list rendering and hit-testing.
#[derive(Clone, Debug)]
pub struct OutlineRow {
    /// Nesting depth (0 = top level), used to indent the label.
    pub depth: usize,
    /// The row's display label.
    pub label: String,
    /// Where activating this row navigates, if anywhere.
    pub target: Option<OutlineTarget>,
}

/// Flatten an outline tree into a depth-annotated row list (pre-order) — the form the
/// panel renders and selects over.
#[must_use]
pub fn flatten(entries: &[OutlineEntry]) -> Vec<OutlineRow> {
    let mut rows = Vec::new();
    push_rows(entries, 0, &mut rows);
    rows
}

/// Build a nested outline from the headings in Markdown source.
#[must_use]
pub fn from_markdown(source: &str) -> Vec<OutlineEntry> {
    let document = karet_markdown::parse(source);
    let headings: Vec<(u8, OutlineEntry)> = document
        .blocks()
        .iter()
        .enumerate()
        .filter_map(|(index, block)| {
            // An HTML heading aligned by its container (`<h1 align="center">`) is still
            // a heading; the container holds exactly the one block.
            let block = match block {
                Block::Aligned { blocks, .. } => blocks.first()?,
                block => block,
            };
            let Block::Heading { level, content } = block else {
                return None;
            };
            let label = inline_text(content);
            let line = u32::try_from(document.block_line(index)?).ok()?;
            Some((
                *level,
                OutlineEntry {
                    label,
                    target: Some(OutlineTarget::Text(LineCol::new(line, 0))),
                    children: Vec::new(),
                },
            ))
        })
        .collect();
    let mut index = 0;
    nest_headings(&headings, &mut index, 0)
}

/// The Markdown outline of the last view the outline panel asked about, so a
/// frame that redraws an unchanged document reuses its rows instead of parsing
/// the whole source again.
///
/// The panel asks for the active tab's rows several times per frame (the draw,
/// the selection sync, a click), and for Markdown each ask used to be a full
/// [`from_markdown`] parse. An entry is keyed by the view and its buffer
/// version and also holds the source it was built from: a version alone can
/// repeat over different text (a reserved tab's placeholder and the loaded file
/// share one, and a regenerated preview restarts at version 0), so a hit also
/// requires the text to match. That check is a byte comparison, far cheaper
/// than the parse it saves.
#[derive(Default)]
pub struct MarkdownOutlineCache {
    entry: RefCell<Option<CachedOutline>>,
    builds: Cell<u64>,
}

struct CachedOutline {
    view: ViewId,
    version: u64,
    source: String,
    rows: Vec<OutlineRow>,
}

impl MarkdownOutlineCache {
    /// The flattened Markdown outline of `view` at `version`, whose text is the
    /// concatenation of the pieces `chunks` yields (it is called once to compare
    /// and once more to copy on a miss): the cached rows when the view, version,
    /// and text all match the last build, otherwise a fresh parse that replaces it.
    #[must_use]
    pub fn rows<'a, I>(&self, view: ViewId, version: u64, chunks: impl Fn() -> I) -> Vec<OutlineRow>
    where
        I: Iterator<Item = &'a str>,
    {
        let mut entry = self.entry.borrow_mut();
        if let Some(cached) = entry.as_ref()
            && cached.view == view
            && cached.version == version
            && same_text(&cached.source, chunks())
        {
            return cached.rows.clone();
        }
        let source: String = chunks().collect();
        let rows = flatten(&from_markdown(&source));
        self.builds.set(self.builds.get() + 1);
        *entry = Some(CachedOutline {
            view,
            version,
            source,
            rows: rows.clone(),
        });
        rows
    }

    /// How many times [`rows`](Self::rows) has parsed a source rather than
    /// reusing the cached outline.
    #[cfg(test)]
    pub fn builds(&self) -> u64 {
        self.builds.get()
    }
}

/// Whether the concatenation of `chunks` is exactly `source`, compared piece by
/// piece without joining the pieces.
fn same_text<'a>(source: &str, chunks: impl Iterator<Item = &'a str>) -> bool {
    let mut rest = source.as_bytes();
    for chunk in chunks {
        let Some((head, tail)) = rest.split_at_checked(chunk.len()) else {
            return false;
        };
        if head != chunk.as_bytes() {
            return false;
        }
        rest = tail;
    }
    rest.is_empty()
}

fn nest_headings(
    headings: &[(u8, OutlineEntry)],
    index: &mut usize,
    parent_level: u8,
) -> Vec<OutlineEntry> {
    let mut entries = Vec::new();
    while let Some((level, source)) = headings.get(*index) {
        if *level <= parent_level {
            break;
        }
        let level = *level;
        let mut entry = source.clone();
        *index += 1;
        entry.children = nest_headings(headings, index, level);
        entries.push(entry);
    }
    entries
}

fn inline_text(content: &[Inline]) -> String {
    let mut text = String::new();
    for inline in content {
        match inline {
            Inline::Text(value) | Inline::Code(value) => text.push_str(value),
            Inline::Emphasis(children) | Inline::Strong(children) => {
                text.push_str(&inline_text(children));
            },
            Inline::Link { text: value, .. } => text.push_str(value),
            Inline::Image(image) => text.push_str(&image.alt),
            _ => {},
        }
    }
    text
}

/// Build a navigation outline from a language server's symbol tree.
#[must_use]
pub fn from_symbols(symbols: &[Symbol]) -> Vec<OutlineEntry> {
    symbols.iter().map(OutlineEntry::from).collect()
}

impl From<&Symbol> for OutlineEntry {
    fn from(symbol: &Symbol) -> Self {
        Self {
            label: symbol.name.clone(),
            target: Some(OutlineTarget::Text(symbol.selection_range.start)),
            children: symbol.children.iter().map(Self::from).collect(),
        }
    }
}

/// The chain of symbols enclosing `caret`, outermost first, as the index of each
/// within its parent's children (the first within `symbols` itself) — the path
/// the breadcrumb paints and its sibling picker walks. A symbol encloses the
/// caret when its full range holds it, the end included, so a caret just past a
/// closing brace still names the block. Where siblings overlap, the one that
/// starts last (the tighter fit) wins. Empty when no symbol encloses the caret.
#[must_use]
pub fn enclosing_path(symbols: &[Symbol], caret: LineCol) -> Vec<usize> {
    let mut path = Vec::new();
    let mut level = symbols;
    while let Some((index, symbol)) = level
        .iter()
        .enumerate()
        .filter(|(_, symbol)| symbol.range.start <= caret && caret <= symbol.range.end)
        .max_by_key(|(_, symbol)| symbol.range.start)
    {
        path.push(index);
        level = &symbol.children;
    }
    path
}

/// The symbols `path` (from [`enclosing_path`]) names, outermost first. A path
/// that leaves the tree stops at the last symbol it reached.
#[must_use]
pub fn resolve_path<'a>(symbols: &'a [Symbol], path: &[usize]) -> Vec<&'a Symbol> {
    let mut chain = Vec::with_capacity(path.len());
    let mut level = symbols;
    for &index in path {
        let Some(symbol) = level.get(index) else {
            break;
        };
        chain.push(symbol);
        level = &symbol.children;
    }
    chain
}

/// The symbol at `depth` along `path` and its siblings: the level it sits in and
/// its index there. `None` when `path` does not reach that deep.
#[must_use]
pub fn siblings_at<'a>(
    symbols: &'a [Symbol],
    path: &[usize],
    depth: usize,
) -> Option<(&'a [Symbol], usize)> {
    let index = *path.get(depth)?;
    let parents = resolve_path(symbols, path.get(..depth)?);
    if parents.len() != depth {
        return None;
    }
    let level = parents
        .last()
        .map_or(symbols, |parent| &parent.children[..]);
    (index < level.len()).then_some((level, index))
}

fn push_rows(entries: &[OutlineEntry], depth: usize, rows: &mut Vec<OutlineRow>) {
    for e in entries {
        rows.push(OutlineRow {
            depth,
            label: e.label.clone(),
            target: e.target,
        });
        push_rows(&e.children, depth + 1, rows);
    }
}

/// Build the neutral outline for a PDF document from its extracted bookmarks.
#[cfg(feature = "pdf")]
#[must_use]
pub fn from_pdf(items: &[karet_pdf::OutlineItem]) -> Vec<OutlineEntry> {
    items.iter().map(OutlineEntry::from).collect()
}

#[cfg(feature = "pdf")]
impl From<&karet_pdf::OutlineItem> for OutlineEntry {
    fn from(item: &karet_pdf::OutlineItem) -> Self {
        Self {
            label: item.title.clone(),
            target: item.dest.page().map(OutlineTarget::Page),
            children: item.children.iter().map(OutlineEntry::from).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_text_compares_across_chunk_boundaries() {
        assert!(same_text("# ab", ["# ", "a", "b"].into_iter()));
        assert!(same_text("", std::iter::empty()));
        assert!(!same_text("# ab", ["# ", "a"].into_iter()), "shorter");
        assert!(!same_text("# a", ["# ", "ab"].into_iter()), "longer");
        assert!(!same_text("# ab", ["# ", "ax"].into_iter()), "differs");
    }

    #[test]
    fn markdown_cache_rebuilds_for_another_view_or_version() {
        let cache = MarkdownOutlineCache::default();
        let rows = |view, version| cache.rows(ViewId(view), version, || ["# H\n"].into_iter());
        assert_eq!(rows(1, 0).len(), 1);
        assert_eq!(rows(1, 0).len(), 1);
        assert_eq!(cache.builds(), 1);
        let _ = rows(2, 0);
        assert_eq!(cache.builds(), 2, "another view");
        let _ = rows(2, 1);
        assert_eq!(cache.builds(), 3, "another version");
    }

    #[test]
    fn flatten_walks_pre_order_with_depth() {
        let entries = vec![OutlineEntry {
            label: "A".into(),
            target: Some(OutlineTarget::Page(0)),
            children: vec![OutlineEntry {
                label: "A.1".into(),
                target: Some(OutlineTarget::Page(1)),
                children: Vec::new(),
            }],
        }];
        let rows = flatten(&entries);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].depth, rows[0].label.as_str()), (0, "A"));
        assert_eq!((rows[1].depth, rows[1].label.as_str()), (1, "A.1"));
        assert_eq!(rows[1].target, Some(OutlineTarget::Page(1)));
    }

    #[test]
    fn an_aligned_html_heading_is_an_outline_entry_on_its_line() {
        let entries = from_markdown("intro\n\n<h1 align=\"center\">Title</h1>\n\n## Sub\n");
        let rows = flatten(&entries);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].depth, rows[0].label.as_str()), (0, "Title"));
        assert_eq!(
            rows[0].target,
            Some(OutlineTarget::Text(LineCol::new(2, 0)))
        );
        assert_eq!((rows[1].depth, rows[1].label.as_str()), (1, "Sub"));
    }

    #[test]
    fn an_image_in_a_heading_labels_it_by_its_alt_text() {
        let rows = flatten(&from_markdown("# ![logo](l.png) Project\n"));
        assert_eq!(
            rows.first().map(|row| row.label.as_str()),
            Some("logo Project")
        );
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn pdf_bookmarks_convert_to_page_targets() {
        let items = vec![
            karet_pdf::OutlineItem::new("Chapter 1", karet_pdf::OutlineDest::Page(2), Vec::new()),
            karet_pdf::OutlineItem::new("Links", karet_pdf::OutlineDest::Unresolved, Vec::new()),
        ];
        let entries = from_pdf(&items);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].label, "Chapter 1");
        assert_eq!(entries[0].target, Some(OutlineTarget::Page(2)));
        assert_eq!(
            entries[1].target, None,
            "an unresolved bookmark has no target"
        );
    }

    #[test]
    fn markdown_headings_form_a_hierarchy_and_strip_inline_markup() {
        let entries = from_markdown("# Root *title*\n\n### Deep `code`\n\n## Sibling [link](x)\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].label, "Root title");
        assert_eq!(entries[0].children.len(), 2);
        assert_eq!(entries[0].children[0].label, "Deep code");
        assert_eq!(entries[0].children[1].label, "Sibling link");
        assert_eq!(
            entries[0].children[1].target,
            Some(OutlineTarget::Text(LineCol::new(4, 0)))
        );
    }

    /// A symbol spanning `start..=end` lines (column 0 to 1), named at its start.
    fn spanning(name: &str, start: u32, end: u32, children: Vec<Symbol>) -> Symbol {
        Symbol {
            name: name.into(),
            kind: SymbolKind::Function,
            detail: None,
            range: karet_core::Range {
                start: LineCol::new(start, 0),
                end: LineCol::new(end, 1),
            },
            selection_range: karet_core::Range {
                start: LineCol::new(start, 3),
                end: LineCol::new(start, 4),
            },
            container_name: None,
            children,
        }
    }

    /// `mod a` (0..=10) holding `fn b` (1..=3) and `struct c` (5..=9, holding
    /// `d` at 6..=7), then a top-level `e` (12..=14).
    fn nested() -> Vec<Symbol> {
        vec![
            spanning(
                "a",
                0,
                10,
                vec![
                    spanning("b", 1, 3, Vec::new()),
                    spanning("c", 5, 9, vec![spanning("d", 6, 7, Vec::new())]),
                ],
            ),
            spanning("e", 12, 14, Vec::new()),
        ]
    }

    fn names(symbols: &[Symbol], caret: LineCol) -> Vec<String> {
        resolve_path(symbols, &enclosing_path(symbols, caret))
            .into_iter()
            .map(|symbol| symbol.name.clone())
            .collect()
    }

    #[test]
    fn the_enclosing_path_descends_to_the_innermost_symbol() {
        let tree = nested();
        assert_eq!(enclosing_path(&tree, LineCol::new(6, 4)), [0, 1, 0]);
        assert_eq!(names(&tree, LineCol::new(6, 4)), ["a", "c", "d"]);
        assert_eq!(names(&tree, LineCol::new(4, 0)), ["a"], "between b and c");
        assert_eq!(names(&tree, LineCol::new(13, 0)), ["e"]);
    }

    #[test]
    fn a_caret_outside_every_symbol_has_no_chain() {
        let tree = nested();
        assert!(enclosing_path(&tree, LineCol::new(11, 0)).is_empty());
        assert!(enclosing_path(&[], LineCol::new(0, 0)).is_empty());
    }

    #[test]
    fn a_range_holds_its_own_end() {
        let tree = nested();
        assert_eq!(names(&tree, LineCol::new(3, 1)), ["a", "b"]);
        assert_eq!(names(&tree, LineCol::new(3, 2)), ["a"]);
    }

    #[test]
    fn overlapping_siblings_resolve_to_the_one_starting_last() {
        let tree = vec![
            spanning("outer", 0, 9, Vec::new()),
            spanning("inner", 2, 4, Vec::new()),
        ];
        assert_eq!(names(&tree, LineCol::new(3, 0)), ["inner"]);
        assert_eq!(names(&tree, LineCol::new(6, 0)), ["outer"]);
    }

    #[test]
    fn siblings_at_a_depth_are_that_level_of_the_tree() {
        let tree = nested();
        let path = [0, 1, 0];
        let level = |depth| {
            siblings_at(&tree, &path, depth).map(|(level, index)| {
                let names: Vec<_> = level.iter().map(|s| s.name.as_str()).collect();
                (names, index)
            })
        };
        assert_eq!(level(0), Some((vec!["a", "e"], 0)));
        assert_eq!(level(1), Some((vec!["b", "c"], 1)));
        assert_eq!(level(2), Some((vec!["d"], 0)));
        assert_eq!(level(3), None, "deeper than the path");
    }

    #[test]
    fn a_path_from_a_different_tree_resolves_no_further_than_it_fits() {
        let tree = nested();
        // A stale path: `a` has two children, not three.
        assert_eq!(resolve_path(&tree, &[0, 5, 0]).len(), 1);
        assert_eq!(siblings_at(&tree, &[0, 5, 0], 1), None);
        assert_eq!(siblings_at(&tree, &[0, 5, 0], 2), None);
    }

    #[test]
    fn symbols_keep_selection_targets() {
        let symbols = vec![Symbol {
            name: "App".into(),
            kind: SymbolKind::Struct,
            detail: None,
            range: karet_core::Range::default(),
            selection_range: karet_core::Range {
                start: LineCol::new(3, 7),
                end: LineCol::new(3, 10),
            },
            container_name: None,
            children: Vec::new(),
        }];
        let entries = from_symbols(&symbols);
        assert_eq!(entries[0].label, "App");
        assert_eq!(
            entries[0].target,
            Some(OutlineTarget::Text(LineCol::new(3, 7)))
        );
    }
}
