//! Go to Symbol in Editor and in Workspace: quick-open's `@` and `#` modes.
//!
//! Quick-open is one surface with modes, chosen by the query's leading sigil:
//! plain text finds files, `:` goes to a line (see the `goto_line` module),
//! `@` lists the active document's symbols, and `#` searches the workspace's.
//! The document's
//! symbol rows come from the session's [`Event::Symbols`] answer — the
//! language server's when one serves the document, the tree-sitter outline
//! otherwise — and may arrive after the picker opened, so the picker fills in
//! through [`Overlay::set_document_symbols`] without losing what was typed.
//! The workspace's come from [`Event::WorkspaceSymbols`], one answer per
//! settled query, and fill in the same way through
//! [`Overlay::set_workspace_symbols`].
//!
//! [`Event::Symbols`]: karet_session::Event::Symbols
//! [`Event::WorkspaceSymbols`]: karet_session::Event::WorkspaceSymbols

use std::path::Path;

use karet_core::Symbol;
use karet_core::SymbolKind;
use karet_core::WorkspaceSymbol;

use super::Overlay;
use super::OverlayEvent;
use super::Picker;
use super::QUICK_OPEN_TITLE;
use super::goto_line::FILE_AT_LINE_TITLE;
use super::goto_line::GO_TO_LINE_PREFIX;
use super::goto_line::GO_TO_LINE_TITLE;

/// The sigil that turns quick-open into Go to Symbol in Editor.
pub const DOCUMENT_SYMBOL_PREFIX: &str = "@";

/// The document-symbol picker's title, which is also how it is told apart.
const DOCUMENT_SYMBOLS_TITLE: &str = "Go to Symbol in Editor";

/// The breadcrumb's sibling picker's title.
const SIBLING_SYMBOLS_TITLE: &str = "Go to Sibling Symbol";

/// The sigil that turns quick-open into Go to Symbol in Workspace.
pub const WORKSPACE_SYMBOL_PREFIX: &str = "#";

/// The workspace-symbol picker's title, which is also how it is told apart.
const WORKSPACE_SYMBOLS_TITLE: &str = "Go to Symbol in Workspace";

/// Which list quick-open shows, chosen by the query's leading sigil.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickOpenMode {
    /// Files in the workspace (no sigil).
    Files,
    /// Files in the workspace, opened at the line the query ends with
    /// (`main.rs:42[:7]`).
    FileAtLine,
    /// A line in the active document (`:42[:7]`).
    Line,
    /// The active document's symbols (`@`).
    DocumentSymbols,
    /// Symbols anywhere in the workspace, searched by the language server
    /// (`#`).
    WorkspaceSymbols,
}

impl QuickOpenMode {
    /// The mode a quick-open query asks for.
    #[must_use]
    pub fn of_query(query: &str) -> Self {
        if query.starts_with(DOCUMENT_SYMBOL_PREFIX) {
            Self::DocumentSymbols
        } else if query.starts_with(WORKSPACE_SYMBOL_PREFIX) {
            Self::WorkspaceSymbols
        } else if query.starts_with(GO_TO_LINE_PREFIX) {
            Self::Line
        } else if crate::cli::split_goto_suffix(query).1.is_some() {
            Self::FileAtLine
        } else {
            Self::Files
        }
    }

    /// Whether this mode lists the workspace's files.
    #[must_use]
    pub fn lists_files(self) -> bool {
        matches!(self, Self::Files | Self::FileAtLine)
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

    /// Build the breadcrumb's sibling picker: one row per symbol in `siblings`
    /// (one level of the document's tree, the clicked symbol among them), in
    /// document order, with `current` highlighted. Their own children are not
    /// listed — that is the next breadcrumb segment's picker. It is not a
    /// quick-open mode, so a fresh symbol answer does not refill it with the
    /// whole document and typing a sigil does not change what it lists.
    #[must_use]
    pub fn sibling_symbols(path: &Path, siblings: &[Symbol], current: usize) -> Self {
        let mut order: Vec<usize> = (0..siblings.len()).collect();
        order.sort_by_key(|&i| siblings.get(i).map(|s| s.selection_range.start));
        let rows = order
            .iter()
            .filter_map(|&i| siblings.get(i))
            .map(|symbol| {
                let accept = OverlayEvent::AcceptSymbol {
                    path: path.to_path_buf(),
                    position: symbol.selection_range.start,
                    detail: kind_info(symbol.kind).1.to_string(),
                };
                (symbol.name.clone(), accept)
            })
            .collect();
        let mut picker = Picker::new(SIBLING_SYMBOLS_TITLE, rows);
        let row = order.iter().position(|&i| i == current).unwrap_or_default();
        for _ in 0..row {
            picker.select_down();
        }
        Self::Picker(picker)
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
            Self::Picker(picker) if picker.title() == WORKSPACE_SYMBOLS_TITLE => {
                Some(QuickOpenMode::WorkspaceSymbols)
            },
            Self::Picker(picker) if picker.title() == FILE_AT_LINE_TITLE => {
                Some(QuickOpenMode::FileAtLine)
            },
            Self::Picker(picker) if picker.title() == GO_TO_LINE_TITLE => Some(QuickOpenMode::Line),
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

    /// Build Go to Symbol in Workspace over `symbols`, its query seeded with
    /// the `#` sigil. Each row's file is shown relative to `root` where it is
    /// under it. The rows are the server's answer as it gave it: the server
    /// already matched the query, possibly on more than the name (a container,
    /// a qualified path, a query modifier), so the picker does not filter or
    /// re-rank them.
    #[must_use]
    pub fn workspace_symbols(root: &Path, symbols: &[WorkspaceSymbol]) -> Self {
        let rows = workspace_symbol_rows(root, symbols);
        Self::Picker(
            Picker::new(WORKSPACE_SYMBOLS_TITLE, rows)
                .with_prefix(WORKSPACE_SYMBOL_PREFIX)
                .unranked(),
        )
    }

    /// Replace a workspace-symbol picker's rows with a fresh answer, keeping
    /// the query. Any other overlay is left alone.
    pub fn set_workspace_symbols(&mut self, root: &Path, symbols: &[WorkspaceSymbol]) {
        if self.quick_open_mode() != Some(QuickOpenMode::WorkspaceSymbols) {
            return;
        }
        if let Self::Picker(picker) = self {
            picker.set_items(workspace_symbol_rows(root, symbols));
        }
    }

    /// The text after a workspace-symbol picker's `#` sigil — what the
    /// language server is asked for — or `None` for any other overlay.
    #[must_use]
    pub fn workspace_symbol_query(&self) -> Option<&str> {
        if self.quick_open_mode() != Some(QuickOpenMode::WorkspaceSymbols) {
            return None;
        }
        let query = self.query();
        Some(query.strip_prefix(WORKSPACE_SYMBOL_PREFIX).unwrap_or(query))
    }

    /// What a quick-open's delayed loading note says is still being gathered.
    #[must_use]
    pub fn loading_note(&self) -> &'static str {
        match self.quick_open_mode() {
            Some(QuickOpenMode::DocumentSymbols | QuickOpenMode::WorkspaceSymbols) => {
                "loading symbols…"
            },
            _ => "indexing files…",
        }
    }
}

/// One row per workspace symbol, in the server's order (the picker shows them
/// unfiltered; the server did the matching). The label is the name; the kind, container, and file ride along as the row's hint. Accepting it
/// opens the file at the symbol, or at its top when the server named only the
/// file.
fn workspace_symbol_rows(root: &Path, symbols: &[WorkspaceSymbol]) -> Vec<(String, OverlayEvent)> {
    symbols
        .iter()
        .map(|symbol| {
            let (_, kind) = kind_info(symbol.kind);
            let mut detail = kind.to_string();
            if let Some(container) = symbol.container_name.as_deref().filter(|c| !c.is_empty()) {
                detail.push_str(" · ");
                detail.push_str(container);
            }
            let shown = symbol.path.strip_prefix(root).unwrap_or(&symbol.path);
            detail.push_str(" · ");
            detail.push_str(&shown.display().to_string());
            let accept = OverlayEvent::AcceptSymbol {
                path: symbol.path.clone(),
                position: symbol.range.map(|range| range.start).unwrap_or_default(),
                detail,
            };
            (symbol.name.clone(), accept)
        })
        .collect()
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
        assert_eq!(QuickOpenMode::of_query(":"), QuickOpenMode::Line);
        assert_eq!(QuickOpenMode::of_query(":12:3"), QuickOpenMode::Line);
        assert_eq!(QuickOpenMode::of_query("a:12"), QuickOpenMode::FileAtLine);
        assert_eq!(QuickOpenMode::of_query("a:"), QuickOpenMode::Files);
        assert_eq!(QuickOpenMode::of_query("a:b"), QuickOpenMode::Files);
        assert_eq!(
            QuickOpenMode::of_query("#"),
            QuickOpenMode::WorkspaceSymbols
        );
        assert_eq!(
            QuickOpenMode::of_query("#run"),
            QuickOpenMode::WorkspaceSymbols
        );
        assert_eq!(QuickOpenMode::of_query("a#b"), QuickOpenMode::Files);
        assert!(QuickOpenMode::FileAtLine.lists_files());
        assert!(!QuickOpenMode::Line.lists_files());
        assert!(!QuickOpenMode::WorkspaceSymbols.lists_files());
    }

    fn workspace_symbol(name: &str, path: &str, at: Option<LineCol>) -> WorkspaceSymbol {
        WorkspaceSymbol {
            name: name.to_string(),
            kind: SymbolKind::Function,
            container_name: (name == "run").then(|| "Runner".to_string()),
            path: PathBuf::from(path),
            range: at.map(|start| Range { start, end: start }),
        }
    }

    fn workspace_hits() -> Vec<WorkspaceSymbol> {
        vec![
            workspace_symbol("run", "/w/src/run.rs", Some(LineCol::new(7, 3))),
            workspace_symbol("rerun", "/elsewhere/lib.rs", Some(LineCol::new(1, 0))),
            workspace_symbol("lazy", "/w/src/lazy.rs", None),
        ]
    }

    #[test]
    fn workspace_rows_name_their_file_and_accept_opens_it_at_the_symbol() {
        let root = Path::new("/w");
        let mut overlay = Overlay::workspace_symbols(root, &workspace_hits());
        assert_eq!(
            overlay.quick_open_mode(),
            Some(QuickOpenMode::WorkspaceSymbols)
        );
        assert_eq!(overlay.query(), "#");
        assert_eq!(overlay.workspace_symbol_query(), Some(""));
        assert_eq!(overlay.loading_note(), "loading symbols…");
        assert_eq!(overlay.rows(), vec!["run", "rerun", "lazy"]);
        assert_eq!(
            overlay.row_hints(),
            vec![
                Some("function · Runner · src/run.rs".to_string()),
                Some("function · /elsewhere/lib.rs".to_string()),
                Some("function · src/lazy.rs".to_string()),
            ],
            "a file outside the workspace keeps its whole path"
        );
        // The server matched a qualified query on the container; the picker
        // keeps every hit it answered with, in its order.
        overlay.push_str("Runner::run#");
        assert_eq!(overlay.workspace_symbol_query(), Some("Runner::run#"));
        assert_eq!(overlay.rows(), vec!["run", "rerun", "lazy"]);
        match overlay.accept() {
            OverlayEvent::AcceptSymbol { path, position, .. } => {
                assert_eq!(path, PathBuf::from("/w/src/run.rs"));
                assert_eq!(position, LineCol::new(7, 3));
            },
            _ => unreachable!("a workspace symbol row jumps to its file"),
        }
        let mut overlay = Overlay::workspace_symbols(root, &workspace_hits());
        overlay.push_str("lazy");
        overlay.select_down();
        overlay.select_down();
        match overlay.accept() {
            OverlayEvent::AcceptSymbol { path, position, .. } => {
                assert_eq!(path, PathBuf::from("/w/src/lazy.rs"));
                assert_eq!(position, LineCol::new(0, 0), "no range: the file's top");
            },
            _ => unreachable!("a file-only symbol still opens its file"),
        }
    }

    #[test]
    fn workspace_symbols_arriving_later_fill_only_their_own_picker() {
        let root = Path::new("/w");
        let mut overlay = Overlay::workspace_symbols(root, &[]);
        overlay.push_str("re");
        overlay.set_workspace_symbols(root, &workspace_hits());
        assert_eq!(overlay.query(), "#re");
        assert_eq!(overlay.rows(), vec!["run", "rerun", "lazy"]);
        let path = PathBuf::from("/w/src/lib.rs");
        let mut editor = Overlay::document_symbols(Some(&path), &tree());
        editor.set_workspace_symbols(root, &workspace_hits());
        assert_eq!(editor.rows().len(), 4, "the editor's symbols are kept");
        assert_eq!(editor.workspace_symbol_query(), None);
        editor.set_document_symbols(Some(&path), &[]);
        overlay.set_document_symbols(Some(&path), &tree());
        assert_eq!(
            overlay.rows(),
            vec!["run", "rerun", "lazy"],
            "and the other way round"
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
    fn the_sibling_picker_lists_one_level_in_document_order_at_the_clicked_symbol() {
        let path = PathBuf::from("/w/src/lib.rs");
        // Out of document order on purpose: the picker sorts by position.
        let mut level = tree();
        level.reverse();
        let current = level.iter().position(|s| s.name == "Widget").unwrap_or(9);
        let mut overlay = Overlay::sibling_symbols(&path, &level, current);
        assert_eq!(
            overlay.rows(),
            vec!["helper", "Widget", "main"],
            "children are not listed"
        );
        assert_eq!(overlay.selected(), 1, "the clicked symbol is highlighted");
        assert_eq!(overlay.quick_open_mode(), None);
        match overlay.accept() {
            OverlayEvent::AcceptSymbol {
                path: at,
                position,
                detail,
            } => {
                assert_eq!(at, path);
                assert_eq!(position, LineCol::new(3, 4));
                assert_eq!(detail, "struct");
            },
            _ => unreachable!("a sibling row jumps to its symbol"),
        }
        // A fresh answer for the whole document leaves it as it is.
        overlay.set_document_symbols(Some(&path), &[]);
        assert_eq!(overlay.rows().len(), 3);
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
