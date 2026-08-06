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
