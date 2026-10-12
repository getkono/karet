//! Go to Symbol in Editor: quick-open's `@` mode.
//!
//! Quick-open is one surface with modes, chosen by the query's leading sigil:
//! plain text finds files, and `@` lists the active document's symbols. The
//! symbol rows come from the session's [`Event::Symbols`] answer — the
//! language server's when one serves the document, the tree-sitter outline
//! otherwise — and may arrive after the picker opened, so the picker fills in
//! through [`Overlay::set_document_symbols`] without losing what was typed.
//!
//! [`Event::Symbols`]: karet_session::Event::Symbols

use std::path::Path;

use karet_core::Symbol;
use karet_core::SymbolKind;

use super::Overlay;
use super::OverlayEvent;
use super::Picker;
use super::QUICK_OPEN_TITLE;

/// The sigil that turns quick-open into Go to Symbol in Editor.
pub const DOCUMENT_SYMBOL_PREFIX: &str = "@";

/// The document-symbol picker's title, which is also how it is told apart.
const DOCUMENT_SYMBOLS_TITLE: &str = "Go to Symbol in Editor";

/// Which list quick-open shows, chosen by the query's leading sigil.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickOpenMode {
    /// Files in the workspace (no sigil).
    Files,
    /// The active document's symbols (`@`).
    DocumentSymbols,
}

impl QuickOpenMode {
    /// The mode a quick-open query asks for.
    #[must_use]
    pub fn of_query(query: &str) -> Self {
        if query.starts_with(DOCUMENT_SYMBOL_PREFIX) {
            Self::DocumentSymbols
        } else {
            Self::Files
        }
    }
}

impl Overlay {
    /// Build Go to Symbol in Editor over `symbols` of the document at `path`,
    /// its query seeded with the `@` sigil. With no document (`path` `None`)
    /// it lists nothing.
    #[must_use]
    pub fn document_symbols(path: Option<&Path>, symbols: &[Symbol]) -> Self {
        let rows = symbol_rows(path, symbols);
        Self::Picker(Picker::new(DOCUMENT_SYMBOLS_TITLE, rows).with_prefix(DOCUMENT_SYMBOL_PREFIX))
    }

    /// Which quick-open mode this overlay is, or `None` for any other overlay.
    #[must_use]
    pub fn quick_open_mode(&self) -> Option<QuickOpenMode> {
        match self {
            Self::Picker(picker) if picker.title() == QUICK_OPEN_TITLE => {
                Some(QuickOpenMode::Files)
            },
            Self::Picker(picker) if picker.title() == DOCUMENT_SYMBOLS_TITLE => {
                Some(QuickOpenMode::DocumentSymbols)
            },
            _ => None,
        }
    }

    /// Replace a document-symbol picker's rows with a fresh answer, keeping the
    /// query. Any other overlay is left alone.
    pub fn set_document_symbols(&mut self, path: Option<&Path>, symbols: &[Symbol]) {
        if self.quick_open_mode() != Some(QuickOpenMode::DocumentSymbols) {
            return;
        }
        if let Self::Picker(picker) = self {
            picker.set_items(symbol_rows(path, symbols));
        }
    }

    /// What a quick-open's delayed loading note says is still being gathered.
    #[must_use]
    pub fn loading_note(&self) -> &'static str {
        match self.quick_open_mode() {
            Some(QuickOpenMode::DocumentSymbols) => "loading symbols…",
            _ => "indexing files…",
        }
    }
}

/// One row per symbol, nested ones included, grouped by kind (in the LSP's
/// kind order, as VS Code groups them) and in document order within a group.
/// A row's label is the symbol's name alone, so typing matches names; its kind
/// and container ride along as the row's hint. Accepting it jumps to the name.
fn symbol_rows(path: Option<&Path>, symbols: &[Symbol]) -> Vec<(String, OverlayEvent)> {
    let Some(path) = path else {
        return Vec::new();
    };
    let mut flat = Vec::new();
    flatten(symbols, None, &mut flat);
    flat.sort_by_key(|(symbol, _)| (kind_rank(symbol.kind), symbol.selection_range.start));
    flat.into_iter()
        .map(|(symbol, parent)| {
            let (_, kind) = kind_info(symbol.kind);
            let mut detail = kind.to_string();
            if let Some(container) = parent.or(symbol.container_name.as_deref()) {
                detail.push_str(" · ");
                detail.push_str(container);
            }
            let accept = OverlayEvent::AcceptSymbol {
                path: path.to_path_buf(),
                position: symbol.selection_range.start,
                detail,
            };
            (symbol.name.clone(), accept)
        })
        .collect()
}

/// Every symbol in the tree, depth first, each with its parent's name.
fn flatten<'a>(
    symbols: &'a [Symbol],
    parent: Option<&'a str>,
    out: &mut Vec<(&'a Symbol, Option<&'a str>)>,
) {
    for symbol in symbols {
        out.push((symbol, parent));
        flatten(&symbol.children, Some(&symbol.name), out);
    }
}

fn kind_rank(kind: SymbolKind) -> u8 {
    kind_info(kind).0
}

/// A kind's group rank (the LSP `SymbolKind` order) and its row label.
fn kind_info(kind: SymbolKind) -> (u8, &'static str) {
    match kind {
        SymbolKind::File => (1, "file"),
        SymbolKind::Module => (2, "module"),
        SymbolKind::Namespace => (3, "namespace"),
        SymbolKind::Package => (4, "package"),
        SymbolKind::Class => (5, "class"),
        SymbolKind::Method => (6, "method"),
        SymbolKind::Property => (7, "property"),
        SymbolKind::Field => (8, "field"),
        SymbolKind::Constructor => (9, "constructor"),
        SymbolKind::Enum => (10, "enum"),
        SymbolKind::Interface => (11, "interface"),
        SymbolKind::Function => (12, "function"),
        SymbolKind::Variable => (13, "variable"),
        SymbolKind::Constant => (14, "constant"),
        SymbolKind::String => (15, "string"),
        SymbolKind::Number => (16, "number"),
        SymbolKind::Boolean => (17, "boolean"),
        SymbolKind::Array => (18, "array"),
        SymbolKind::Object => (19, "object"),
        SymbolKind::Key => (20, "key"),
        SymbolKind::Null => (21, "null"),
        SymbolKind::EnumMember => (22, "enum member"),
        SymbolKind::Struct => (23, "struct"),
        SymbolKind::Event => (24, "event"),
        SymbolKind::Operator => (25, "operator"),
        SymbolKind::TypeParameter => (26, "type parameter"),
        // `Unknown`, and any kind a newer core adds.
        _ => (u8::MAX, "symbol"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use karet_core::LineCol;
    use karet_core::Range;

    use super::*;

    fn symbol(name: &str, kind: SymbolKind, line: u32, children: Vec<Symbol>) -> Symbol {
        let at = LineCol::new(line, 4);
        Symbol {
            name: name.to_string(),
            kind,
            detail: None,
            range: Range {
                start: LineCol::new(line, 0),
                end: LineCol::new(line + 1, 0),
            },
            selection_range: Range { start: at, end: at },
            container_name: None,
            children,
        }
    }

    fn tree() -> Vec<Symbol> {
        vec![
            symbol("helper", SymbolKind::Function, 1, Vec::new()),
            symbol(
                "Widget",
                SymbolKind::Struct,
                3,
                vec![symbol("size", SymbolKind::Field, 4, Vec::new())],
            ),
            symbol("main", SymbolKind::Function, 9, Vec::new()),
        ]
    }

    #[test]
    fn the_sigil_chooses_the_mode() {
        assert_eq!(QuickOpenMode::of_query(""), QuickOpenMode::Files);
        assert_eq!(QuickOpenMode::of_query("main.rs"), QuickOpenMode::Files);
        assert_eq!(QuickOpenMode::of_query("a@b"), QuickOpenMode::Files);
        assert_eq!(QuickOpenMode::of_query("@"), QuickOpenMode::DocumentSymbols);
        assert_eq!(
            QuickOpenMode::of_query("@ma"),
            QuickOpenMode::DocumentSymbols
        );
    }

    #[test]
    fn rows_are_grouped_by_kind_in_document_order_with_their_container() {
        let path = PathBuf::from("/w/src/lib.rs");
        let overlay = Overlay::document_symbols(Some(&path), &tree());
        assert_eq!(
            overlay.quick_open_mode(),
            Some(QuickOpenMode::DocumentSymbols)
        );
        assert!(
            !overlay.is_quick_open(),
            "the file mode is a different list"
        );
        assert_eq!(overlay.query(), "@");
        assert_eq!(overlay.rows(), vec!["size", "helper", "main", "Widget"]);
        assert_eq!(
            overlay.row_hints(),
            vec![
                Some("field · Widget".to_string()),
                Some("function".to_string()),
                Some("function".to_string()),
                Some("struct".to_string()),
            ]
        );
    }

    #[test]
    fn only_names_are_matched_not_kinds_or_containers() {
        let path = PathBuf::from("/w/src/lib.rs");
        let mut overlay = Overlay::document_symbols(Some(&path), &tree());
        overlay.push_str("func");
        assert!(
            overlay.rows().is_empty(),
            "the kind is not part of the name"
        );
        let mut overlay = Overlay::document_symbols(Some(&path), &tree());
        overlay.push_str("wid");
        assert_eq!(overlay.rows(), vec!["Widget"], "nor is the container");
    }

    #[test]
    fn typing_after_the_sigil_filters_and_accept_jumps_to_the_name() {
        let path = PathBuf::from("/w/src/lib.rs");
        let mut overlay = Overlay::document_symbols(Some(&path), &tree());
        overlay.push_str("main");
        assert_eq!(overlay.rows(), vec!["main"]);
        match overlay.accept() {
            OverlayEvent::AcceptSymbol {
                path: to, position, ..
            } => {
                assert_eq!(to, path);
                assert_eq!(position, LineCol::new(9, 4));
            },
            _ => unreachable!("a symbol row jumps to its location"),
        }
    }

    #[test]
    fn symbols_arriving_later_fill_the_open_picker_keeping_the_query() {
        let path = PathBuf::from("/w/src/lib.rs");
        let mut overlay = Overlay::document_symbols(Some(&path), &[]);
        assert!(overlay.rows().is_empty());
        overlay.push_str("wid");
        overlay.set_document_symbols(Some(&path), &tree());
        assert_eq!(overlay.query(), "@wid");
        assert_eq!(overlay.rows(), vec!["Widget"]);
        // The file picker is not a symbol list, and is left alone.
        let mut files = Overlay::quick_open(vec![("a.rs".to_string(), PathBuf::from("/w/a.rs"))]);
        files.set_document_symbols(Some(&path), &tree());
        assert_eq!(files.rows(), vec!["a.rs"]);
    }

    #[test]
    fn with_no_document_nothing_is_listed() {
        let overlay = Overlay::document_symbols(None, &tree());
        assert!(overlay.rows().is_empty());
        assert!(matches!(overlay.accept(), OverlayEvent::Close));
    }

    #[test]
    fn the_loading_note_names_what_is_being_gathered() {
        let symbols = Overlay::document_symbols(None, &[]);
        assert_eq!(symbols.loading_note(), "loading symbols…");
        assert_eq!(
            Overlay::quick_open(Vec::new()).loading_note(),
            "indexing files…"
        );
        assert_eq!(Overlay::command_palette().quick_open_mode(), None);
    }
}
