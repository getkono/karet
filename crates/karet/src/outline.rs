//! The neutral outline model behind the right-side outline panel.
//!
//! A format-agnostic navigation tree: each producer converts its own model into an
//! [`OutlineEntry`], so one panel renders and one activation path drives them all.
//! PDF bookmarks, Markdown headings, and language-server symbols all convert into
//! this model without coupling the panel to their producer.

use karet_core::LineCol;
use karet_core::Symbol;
use karet_core::SymbolKind;
use karet_markdown::Block;
use karet_markdown::Inline;

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
    /// A secondary detail shown after the label (e.g. a symbol's type), if any.
    pub detail: Option<String>,
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
    /// The row's detail suffix, if any.
    pub detail: Option<String>,
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
        .blocks
        .iter()
        .enumerate()
        .filter_map(|(index, block)| {
            let Block::Heading { level, content } = block else {
                return None;
            };
            let label = inline_text(content);
            let line = u32::try_from(document.block_line(index)?).ok()?;
            Some((
                *level,
                OutlineEntry {
                    label,
                    detail: Some(format!("H{level}")),
                    target: Some(OutlineTarget::Text(LineCol::new(line, 0))),
                    children: Vec::new(),
                },
            ))
        })
        .collect();
    let mut index = 0;
    nest_headings(&headings, &mut index, 0)
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
            detail: symbol
                .detail
                .clone()
                .or_else(|| Some(symbol_kind_label(symbol.kind).to_string())),
            target: Some(OutlineTarget::Text(symbol.selection_range.start)),
            children: symbol.children.iter().map(Self::from).collect(),
        }
    }
}

fn symbol_kind_label(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::File => "file",
        SymbolKind::Module => "module",
        SymbolKind::Namespace => "namespace",
        SymbolKind::Package => "package",
        SymbolKind::Class => "class",
        SymbolKind::Method => "method",
        SymbolKind::Property => "property",
        SymbolKind::Field => "field",
        SymbolKind::Constructor => "constructor",
        SymbolKind::Enum => "enum",
        SymbolKind::Interface => "interface",
        SymbolKind::Function => "function",
        SymbolKind::Variable => "variable",
        SymbolKind::Constant => "constant",
        SymbolKind::String => "string",
        SymbolKind::Number => "number",
        SymbolKind::Boolean => "boolean",
        SymbolKind::Array => "array",
        SymbolKind::Object => "object",
        SymbolKind::Key => "key",
        SymbolKind::Null => "null",
        SymbolKind::EnumMember => "enum member",
        SymbolKind::Struct => "struct",
        SymbolKind::Event => "event",
        SymbolKind::Operator => "operator",
        SymbolKind::TypeParameter => "type parameter",
        _ => "symbol",
    }
}

fn push_rows(entries: &[OutlineEntry], depth: usize, rows: &mut Vec<OutlineRow>) {
    for e in entries {
        rows.push(OutlineRow {
            depth,
            label: e.label.clone(),
            detail: e.detail.clone(),
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
            detail: None,
            target: item.page.map(OutlineTarget::Page),
            children: item.children.iter().map(OutlineEntry::from).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_walks_pre_order_with_depth() {
        let entries = vec![OutlineEntry {
            label: "A".into(),
            detail: None,
            target: Some(OutlineTarget::Page(0)),
            children: vec![OutlineEntry {
                label: "A.1".into(),
                detail: None,
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

    #[cfg(feature = "pdf")]
    #[test]
    fn pdf_bookmarks_convert_to_page_targets() {
        let items = vec![karet_pdf::OutlineItem {
            title: "Chapter 1".into(),
            page: Some(2),
            children: Vec::new(),
        }];
        let entries = from_pdf(&items);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].label, "Chapter 1");
        assert_eq!(entries[0].target, Some(OutlineTarget::Page(2)));
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

    #[test]
    fn symbols_keep_children_details_and_selection_targets() {
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
        assert_eq!(entries[0].detail.as_deref(), Some("struct"));
        assert_eq!(
            entries[0].target,
            Some(OutlineTarget::Text(LineCol::new(3, 7)))
        );
    }
}
