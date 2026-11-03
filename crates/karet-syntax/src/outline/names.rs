//! Normalization and neutral kinds for declaration-query captures.

use karet_core::SymbolKind;

pub(super) fn clean_name(raw: &str) -> String {
    let trimmed = raw.split_once('{').map_or(raw, |(head, _)| head).trim();
    let unquoted_identifier = trimmed
        .strip_prefix("@\"")
        .and_then(|name| name.strip_suffix('"'))
        .unwrap_or(trimmed);
    let unbracketed = unquoted_identifier.trim_matches(['[', ']']).trim();
    unbracketed
        .strip_prefix('"')
        .and_then(|name| name.strip_suffix('"'))
        .or_else(|| {
            unbracketed
                .strip_prefix('\'')
                .and_then(|name| name.strip_suffix('\''))
        })
        .unwrap_or(unbracketed)
        .to_owned()
}

pub(super) fn clean_subroutine_name(raw: &str) -> String {
    raw.trim()
        .trim_start_matches(':')
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned()
}

pub(super) fn symbol_kind(name: &str) -> SymbolKind {
    match name {
        "class" => SymbolKind::Class,
        "method" => SymbolKind::Method,
        "function" | "macro" | "subroutine" => SymbolKind::Function,
        "interface" => SymbolKind::Interface,
        "module" | "namespace" | "package" => SymbolKind::Module,
        "constant" => SymbolKind::Constant,
        "field" => SymbolKind::Field,
        "type" => SymbolKind::Struct,
        "property" => SymbolKind::Property,
        "constructor" => SymbolKind::Constructor,
        "enum" => SymbolKind::Enum,
        "enum_variant" => SymbolKind::EnumMember,
        "operator" => SymbolKind::Operator,
        "array" => SymbolKind::Array,
        "object" => SymbolKind::Object,
        name if name == "heading" || name.starts_with("heading.") => SymbolKind::Namespace,
        _ => SymbolKind::Variable,
    }
}

pub(super) fn kind_rank(kind: SymbolKind) -> u8 {
    match kind {
        SymbolKind::Method => 0,
        SymbolKind::Function => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use karet_core::Symbol;
    use karet_treesitter::ParserPool;
    use karet_treesitter::SyntaxTree;
    use karet_treesitter::language_id_from_path;

    use super::*;
    use crate::OutlineExtractor;

    /// Every symbol extracted from `source`, pre-order, as `(name, kind)`.
    fn kinds(
        path: &str,
        source: &str,
    ) -> Result<Vec<(String, SymbolKind)>, Box<dyn std::error::Error>> {
        fn walk(symbols: &[Symbol], output: &mut Vec<(String, SymbolKind)>) {
            for symbol in symbols {
                output.push((symbol.name.clone(), symbol.kind));
                walk(&symbol.children, output);
            }
        }
        let language = language_id_from_path(Path::new(path)).ok_or("missing test grammar")?;
        let tree = SyntaxTree::parse(&mut ParserPool::new(), language, source)?;
        let mut output = Vec::new();
        walk(&OutlineExtractor::new().analyze(&tree, source), &mut output);
        Ok(output)
    }

    fn owned(expected: &[(&str, SymbolKind)]) -> Vec<(String, SymbolKind)> {
        expected
            .iter()
            .map(|&(name, kind)| (name.to_owned(), kind))
            .collect()
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn rust_symbols_carry_the_kind_of_their_declaration() -> Result<(), Box<dyn std::error::Error>>
    {
        let source =
            "mod m { struct S; }\nenum E { A }\ntrait T {}\nimpl S { fn new() {} }\nfn f() {}\n";
        // The grammar's tags query names structs and enums `class`, and any function
        // inside a declaration list a `method`.
        let expected = owned(&[
            ("m", SymbolKind::Module),
            ("S", SymbolKind::Class),
            ("E", SymbolKind::Class),
            ("T", SymbolKind::Interface),
            ("new", SymbolKind::Method),
            ("f", SymbolKind::Function),
        ]);
        assert_eq!(kinds("lib.rs", source)?, expected);
        Ok(())
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn python_symbols_carry_the_kind_of_their_declaration() -> Result<(), Box<dyn std::error::Error>>
    {
        let source = "class Cafe:\n  def brew(self): pass\ndef serve(): pass\n";
        // Python's tags query has no method capture: every `def` is a function.
        let expected = owned(&[
            ("Cafe", SymbolKind::Class),
            ("brew", SymbolKind::Function),
            ("serve", SymbolKind::Function),
        ]);
        assert_eq!(kinds("app.py", source)?, expected);
        Ok(())
    }

    #[test]
    fn every_definition_capture_maps_to_its_kind() {
        for (capture, kind) in [
            ("class", SymbolKind::Class),
            ("method", SymbolKind::Method),
            ("function", SymbolKind::Function),
            ("macro", SymbolKind::Function),
            ("subroutine", SymbolKind::Function),
            ("interface", SymbolKind::Interface),
            ("module", SymbolKind::Module),
            ("namespace", SymbolKind::Module),
            ("package", SymbolKind::Module),
            ("constant", SymbolKind::Constant),
            ("field", SymbolKind::Field),
            ("type", SymbolKind::Struct),
            ("property", SymbolKind::Property),
            ("constructor", SymbolKind::Constructor),
            ("enum", SymbolKind::Enum),
            ("enum_variant", SymbolKind::EnumMember),
            ("operator", SymbolKind::Operator),
            ("array", SymbolKind::Array),
            ("object", SymbolKind::Object),
            ("heading", SymbolKind::Namespace),
            ("heading.2", SymbolKind::Namespace),
            ("headings", SymbolKind::Variable),
            ("variable", SymbolKind::Variable),
            ("", SymbolKind::Variable),
        ] {
            assert_eq!(symbol_kind(capture), kind, "{capture:?}");
        }
    }

    #[test]
    fn methods_rank_before_functions_before_everything_else() {
        assert_eq!(kind_rank(SymbolKind::Method), 0);
        assert_eq!(kind_rank(SymbolKind::Function), 1);
        assert_eq!(kind_rank(SymbolKind::Class), 2);
        assert_eq!(kind_rank(SymbolKind::Variable), 2);
    }
}
