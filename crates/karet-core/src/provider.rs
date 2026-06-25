//! Interop traits that let renderers consume producer output without depending on
//! the producers.
//!
//! A producer (or the backend) resolves data asynchronously and stores it in
//! something implementing these traits; a widget then borrows it synchronously.
//! Blanket impls on `Vec<T>` and `[T]` let callers pass slices directly.

use crate::coord::LineCol;
use crate::model::{Decoration, Diagnostic, Symbol};

/// A snapshot source of document/workspace symbols.
pub trait SymbolProvider {
    /// The current, resolved symbols (a flat or nested list).
    fn symbols(&self) -> &[Symbol];

    /// The deepest symbol whose range contains `pos`, if any.
    fn symbol_at(&self, pos: LineCol) -> Option<&Symbol> {
        fn deepest(syms: &[Symbol], pos: LineCol) -> Option<&Symbol> {
            for s in syms {
                if s.range.contains(pos) {
                    return Some(deepest(&s.children, pos).unwrap_or(s));
                }
            }
            None
        }
        deepest(self.symbols(), pos)
    }
}

/// A snapshot source of diagnostics.
pub trait DiagnosticSource {
    /// The current diagnostics.
    fn diagnostics(&self) -> &[Diagnostic];
}

/// A snapshot source of decorations.
pub trait DecorationSource {
    /// The current decorations.
    fn decorations(&self) -> &[Decoration];
}

impl SymbolProvider for [Symbol] {
    fn symbols(&self) -> &[Symbol] {
        self
    }
}

impl SymbolProvider for Vec<Symbol> {
    fn symbols(&self) -> &[Symbol] {
        self
    }
}

impl DiagnosticSource for [Diagnostic] {
    fn diagnostics(&self) -> &[Diagnostic] {
        self
    }
}

impl DiagnosticSource for Vec<Diagnostic> {
    fn diagnostics(&self) -> &[Diagnostic] {
        self
    }
}

impl DecorationSource for [Decoration] {
    fn decorations(&self) -> &[Decoration] {
        self
    }
}

impl DecorationSource for Vec<Decoration> {
    fn decorations(&self) -> &[Decoration] {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::{LineCol, Range};
    use crate::model::SymbolKind;

    fn sym(name: &str, start: (u32, u32), end: (u32, u32), children: Vec<Symbol>) -> Symbol {
        Symbol {
            name: name.to_owned(),
            kind: SymbolKind::Function,
            detail: None,
            range: Range {
                start: LineCol::new(start.0, start.1),
                end: LineCol::new(end.0, end.1),
            },
            selection_range: Range::default(),
            container_name: None,
            children,
        }
    }

    #[test]
    fn symbol_at_finds_deepest() {
        let inner = sym("inner", (1, 0), (5, 0), Vec::new());
        let outer = sym("outer", (0, 0), (9, 0), vec![inner]);
        let syms = vec![outer];
        assert_eq!(syms.symbols().len(), 1);
        assert_eq!(
            syms.symbol_at(LineCol::new(2, 0)).map(|s| s.name.as_str()),
            Some("inner")
        );
        assert_eq!(
            syms.symbol_at(LineCol::new(8, 0)).map(|s| s.name.as_str()),
            Some("outer")
        );
        assert_eq!(
            syms.symbol_at(LineCol::new(20, 0)).map(|s| s.name.as_str()),
            None
        );
    }
}
