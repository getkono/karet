//! The neutral outline model behind the right-side outline panel.
//!
//! A format-agnostic navigation tree: each producer converts its own model into an
//! [`OutlineEntry`], so one panel renders and one activation path drives them all.
//! Today PDF bookmarks are the only source (see the [`From`] impl); code symbols
//! (`karet_core::Symbol`, whose `selection_range.start` maps to
//! [`OutlineTarget::Text`]) and markdown headings are the intended next producers,
//! wired the same way without touching the panel.

use karet_core::LineCol;

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
#[must_use]
pub fn from_pdf(items: &[karet_pdf::OutlineItem]) -> Vec<OutlineEntry> {
    items.iter().map(OutlineEntry::from).collect()
}

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
}
