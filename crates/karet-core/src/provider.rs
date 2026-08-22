//! Interop traits that let renderers consume producer output without depending on
//! the producers.
//!
//! A producer (or the backend) resolves data asynchronously and stores it in
//! something implementing these traits; a widget then borrows it synchronously.
//! Blanket impls on `Vec<T>` and `[T]` let callers pass slices directly.

use crate::coord::LineCol;
use crate::model::Diagnostic;
use crate::model::Symbol;

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

/// A diagnostic message re-rendered as markdown by a [`DiagnosticFormatter`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormattedDiagnostic {
    /// The markdown replacing the raw message text wherever richer rendering
    /// is available (hover popups, detail views). The raw
    /// [`message`](Diagnostic::message) remains the fallback everywhere else.
    pub markdown: String,
}

/// Re-renders a tool's raw diagnostic message as richer markdown.
///
/// Implementations are keyed on [`Diagnostic::source`] (a TypeScript formatter
/// ignores rustc output) and are pure: same diagnostic in, same markdown out.
/// Renderers stay decoupled — they receive the formatted markdown, never the
/// formatter.
pub trait DiagnosticFormatter {
    /// Format `diagnostic`'s message; `None` when this formatter does not
    /// apply, letting the raw message stand.
    fn format(&self, diagnostic: &Diagnostic) -> Option<FormattedDiagnostic>;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::LineCol;
    use crate::coord::Range;
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

#[cfg(test)]
mod formatter_tests {
    use super::*;
    use crate::coord::Range;
    use crate::model::Severity;

    struct Shout;
    impl DiagnosticFormatter for Shout {
        fn format(&self, diagnostic: &Diagnostic) -> Option<FormattedDiagnostic> {
            (diagnostic.source.as_deref() == Some("demo")).then(|| FormattedDiagnostic {
                markdown: diagnostic.message.to_uppercase(),
            })
        }
    }

    #[test]
    fn formatter_applies_only_to_its_source() {
        let mut diagnostic = Diagnostic {
            range: Range::default(),
            severity: Severity::Error,
            message: "boom".to_owned(),
            source: Some("demo".to_owned()),
            code: None,
            tags: Vec::new(),
            related: Vec::new(),
        };
        let formatter: &dyn DiagnosticFormatter = &Shout;
        assert_eq!(
            formatter.format(&diagnostic).map(|f| f.markdown),
            Some("BOOM".to_owned())
        );
        diagnostic.source = Some("other".to_owned());
        assert_eq!(formatter.format(&diagnostic), None);
    }
}
