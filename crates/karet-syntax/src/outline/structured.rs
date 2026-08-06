//! Lightweight container outlines for data syntaxes whose compatible parser
//! bindings do not expose pair nodes with useful definition ranges.

use karet_core::BytePos;
use karet_core::Span;
use karet_core::Symbol;
use karet_core::SymbolKind;
use karet_treesitter::LanguageId;
use karet_treesitter::language_id_from_injection_name;

use super::Candidate;
use super::finish;
use super::line_starts;
use super::to_range;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TokenKind {
    Open(char),
    Close(char),
    Colon,
    Comma,
    Atom,
}

#[derive(Clone, Copy, Debug)]
struct Token {
    kind: TokenKind,
    start: usize,
    end: usize,
}

#[derive(Clone, Copy)]
enum Syntax {
    Json,
    Edn,
}

#[derive(Clone, Copy)]
struct Container {
    end: usize,
    kind: SymbolKind,
}

struct Parsed {
    next: usize,
    container: Option<Container>,
}

pub(super) fn analyze(lang: LanguageId, text: &str) -> Option<Vec<Symbol>> {
    if is(lang, "lockfile") {
        return Some(analyze_lockfile(text));
    }
    let syntax = if is(lang, "json5") || is(lang, "cbor") {
        Syntax::Json
    } else if is(lang, "edn") {
        Syntax::Edn
    } else {
        return None;
    };
    let tokens = lex(text, syntax);
    let starts = line_starts(text);
    let mut candidates = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let parsed = parse_value(&tokens, index, text, syntax, &starts, &mut candidates);
        index = parsed.next.max(index + 1);
    }
    Some(finish(candidates))
}

fn analyze_lockfile(text: &str) -> Vec<Symbol> {
    let starts = line_starts(text);
    let mut groups = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if !trimmed.starts_with(char::is_whitespace)
            && trimmed.ends_with(':')
            && !trimmed.starts_with("__metadata:")
        {
            groups.push((start, start + trimmed.len() - 1));
        }
    }
    let mut candidates = Vec::new();
    for (index, (start, name_end)) in groups.iter().copied().enumerate() {
        let end = groups.get(index + 1).map_or(text.len(), |(next, _)| *next);
        push_candidate(
            &mut candidates,
            Token {
                kind: TokenKind::Atom,
                start,
                end: name_end,
            },
            Container {
                end,
                kind: SymbolKind::Package,
            },
            text,
            &starts,
        );
    }
    finish(candidates)
}

fn is(lang: LanguageId, name: &str) -> bool {
    language_id_from_injection_name(name) == Some(lang)
}

fn lex(text: &str, syntax: Syntax) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < text.len() {
        let Some(ch) = text[index..].chars().next() else {
            break;
        };
        if ch.is_whitespace() {
            index += ch.len_utf8();
            continue;
        }
        if matches!(syntax, Syntax::Edn) && ch == ';' {
            index = line_end(text, index);
            continue;
        }
        if matches!(syntax, Syntax::Json) && text[index..].starts_with("//") {
            index = line_end(text, index);
            continue;
        }
        if matches!(syntax, Syntax::Json) && text[index..].starts_with("/*") {
            index = text[index + 2..]
                .find("*/")
                .map_or(text.len(), |offset| index + offset + 4);
            continue;
        }
        if matches!(syntax, Syntax::Edn) && text[index..].starts_with("#{") {
            tokens.push(Token {
                kind: TokenKind::Open('#'),
                start: index,
                end: index + 2,
            });
            index += 2;
            continue;
        }
        if ch == '"' || (matches!(syntax, Syntax::Json) && ch == '\'') {
            let end = quoted_end(text, index, ch);
            tokens.push(Token {
                kind: TokenKind::Atom,
                start: index,
                end,
            });
            index = end;
            continue;
        }
        let kind = match ch {
            '{' | '[' | '(' => Some(TokenKind::Open(ch)),
            '}' | ']' | ')' => Some(TokenKind::Close(ch)),
            ':' if matches!(syntax, Syntax::Json) => Some(TokenKind::Colon),
            ',' => Some(TokenKind::Comma),
            _ => None,
        };
        if let Some(kind) = kind {
            let end = index + ch.len_utf8();
            tokens.push(Token {
                kind,
                start: index,
                end,
            });
            index = end;
            continue;
        }
        let start = index;
        index += ch.len_utf8();
        while index < text.len() {
            let Some(next) = text[index..].chars().next() else {
                break;
            };
            if is_delimiter(next, syntax) {
                break;
            }
            index += next.len_utf8();
        }
        tokens.push(Token {
            kind: TokenKind::Atom,
            start,
            end: index,
        });
    }
    tokens
}

fn line_end(text: &str, start: usize) -> usize {
    text[start..]
        .find('\n')
        .map_or(text.len(), |offset| start + offset + 1)
}

fn quoted_end(text: &str, start: usize, quote: char) -> usize {
    let mut escaped = false;
    for (offset, ch) in text[start + quote.len_utf8()..].char_indices() {
        let end = start + quote.len_utf8() + offset + ch.len_utf8();
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == quote {
            return end;
        }
    }
    text.len()
}

fn is_delimiter(ch: char, syntax: Syntax) -> bool {
    ch.is_whitespace()
        || matches!(ch, '{' | '}' | '[' | ']' | '(' | ')' | ',' | '"' | '\'')
        || (matches!(syntax, Syntax::Json) && ch == ':')
        || (matches!(syntax, Syntax::Edn) && ch == ';')
}

fn parse_value(
    tokens: &[Token],
    index: usize,
    text: &str,
    syntax: Syntax,
    starts: &[usize],
    candidates: &mut Vec<Candidate>,
) -> Parsed {
    let Some(token) = tokens.get(index) else {
        return Parsed {
            next: index,
            container: None,
        };
    };
    let TokenKind::Open(open) = token.kind else {
        return Parsed {
            next: index + 1,
            container: None,
        };
    };
    if open == '{' {
        parse_map(tokens, index, text, syntax, starts, candidates)
    } else {
        parse_sequence(tokens, index, text, syntax, starts, candidates)
    }
}

fn parse_map(
    tokens: &[Token],
    index: usize,
    text: &str,
    syntax: Syntax,
    starts: &[usize],
    candidates: &mut Vec<Candidate>,
) -> Parsed {
    let mut cursor = index + 1;
    let close = matching_close(tokens.get(index).map(|token| token.kind));
    while cursor < tokens.len() {
        if is_close(tokens.get(cursor), close) {
            return closed(tokens, cursor, SymbolKind::Object);
        }
        if matches!(
            tokens.get(cursor).map(|token| token.kind),
            Some(TokenKind::Comma)
        ) {
            cursor += 1;
            continue;
        }
        let Some(key) = tokens.get(cursor).copied() else {
            break;
        };
        cursor += 1;
        if matches!(syntax, Syntax::Json)
            && matches!(
                tokens.get(cursor).map(|token| token.kind),
                Some(TokenKind::Colon)
            )
        {
            cursor += 1;
        }
        let parsed = parse_value(tokens, cursor, text, syntax, starts, candidates);
        if let Some(container) = parsed.container {
            push_candidate(candidates, key, container, text, starts);
        }
        cursor = parsed.next.max(cursor + 1);
    }
    unclosed(tokens, index, text.len(), SymbolKind::Object)
}

fn parse_sequence(
    tokens: &[Token],
    index: usize,
    text: &str,
    syntax: Syntax,
    starts: &[usize],
    candidates: &mut Vec<Candidate>,
) -> Parsed {
    let mut cursor = index + 1;
    let close = matching_close(tokens.get(index).map(|token| token.kind));
    while cursor < tokens.len() {
        if is_close(tokens.get(cursor), close) {
            return closed(tokens, cursor, SymbolKind::Array);
        }
        let parsed = parse_value(tokens, cursor, text, syntax, starts, candidates);
        cursor = parsed.next.max(cursor + 1);
    }
    unclosed(tokens, index, text.len(), SymbolKind::Array)
}

fn matching_close(kind: Option<TokenKind>) -> char {
    match kind {
        Some(TokenKind::Open('{')) => '}',
        Some(TokenKind::Open('[')) => ']',
        Some(TokenKind::Open('(')) => ')',
        Some(TokenKind::Open('#')) => '}',
        _ => '\0',
    }
}

fn is_close(token: Option<&Token>, expected: char) -> bool {
    matches!(token.map(|token| token.kind), Some(TokenKind::Close(ch)) if ch == expected)
}

fn closed(tokens: &[Token], close: usize, kind: SymbolKind) -> Parsed {
    let end = tokens.get(close).map_or(0, |token| token.end);
    Parsed {
        next: close + 1,
        container: Some(Container { end, kind }),
    }
}

fn unclosed(tokens: &[Token], index: usize, end: usize, kind: SymbolKind) -> Parsed {
    Parsed {
        next: tokens.len().max(index + 1),
        container: Some(Container { end, kind }),
    }
}

fn push_candidate(
    candidates: &mut Vec<Candidate>,
    key: Token,
    container: Container,
    text: &str,
    starts: &[usize],
) {
    let Some(raw_name) = text.get(key.start..key.end) else {
        return;
    };
    let name = raw_name
        .trim()
        .trim_start_matches(':')
        .trim_matches(['"', '\''])
        .to_owned();
    if name.is_empty() {
        return;
    }
    let span = Span {
        start: BytePos(key.start),
        end: BytePos(container.end),
    };
    let name_span = Span {
        start: BytePos(key.start),
        end: BytePos(key.end),
    };
    candidates.push(Candidate {
        span,
        heading_level: None,
        symbol: Symbol {
            name,
            kind: container.kind,
            detail: None,
            range: to_range(starts, text, span),
            selection_range: to_range(starts, text, name_span),
            container_name: None,
            children: Vec::new(),
        },
    });
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use karet_core::LineCol;
    use karet_treesitter::ParserPool;
    use karet_treesitter::SyntaxTree;
    use karet_treesitter::language_id_from_path;

    use super::super::OutlineExtractor;
    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    fn symbols(path: &str, source: &str) -> TestResult<Vec<Symbol>> {
        let language = language_id_from_path(Path::new(path)).ok_or("missing test grammar")?;
        let mut pool = ParserPool::new();
        let tree = SyntaxTree::parse(&mut pool, language, source)?;
        Ok(OutlineExtractor::new().analyze(&tree, source))
    }

    fn names(symbols: &[Symbol]) -> Vec<&str> {
        fn collect<'a>(symbols: &'a [Symbol], output: &mut Vec<&'a str>) {
            for symbol in symbols {
                output.push(&symbol.name);
                collect(&symbol.children, output);
            }
        }
        let mut output = Vec::new();
        collect(symbols, &mut output);
        output
    }

    fn assert_names(path: &str, source: &str, expected: &[&str]) -> TestResult<Vec<Symbol>> {
        let extracted = symbols(path, source)?;
        for name in expected {
            assert!(
                names(&extracted).contains(name),
                "{path}: missing {name:?}: {extracted:#?}"
            );
        }
        Ok(extracted)
    }

    #[test]
    fn json5_cbor_and_edn_keep_named_containers_only() -> TestResult {
        let json5 = assert_names(
            "data.json5",
            "{ // project\n workspace: { packages: [{name: 'app'}] }, café: { nested: [] }, scalar: 1, broken: {\n",
            &["workspace", "packages", "café", "nested", "broken"],
        )?;
        assert!(!names(&json5).contains(&"scalar"));
        assert_eq!(json5[1].selection_range.start, LineCol::new(1, 43));

        let cbor = assert_names(
            "data.cbor",
            "{\n  \"workspace\": {\n    \"packages\": [\n      {\"name\": \"app\"}\n    ]\n  },\n  \"scalar\": 1\n",
            &["workspace", "packages"],
        )?;
        assert!(!names(&cbor).contains(&"scalar"));

        let edn = assert_names(
            "data.edn",
            "{:workspace {:packages [{:name \"app\"}]} :café {:nested []} :scalar 1 :broken {",
            &["workspace", "packages", "café", "nested", "broken"],
        )?;
        assert!(!names(&edn).contains(&"scalar"));
        Ok(())
    }

    #[test]
    fn ini_properties_and_named_files_expose_sections_and_keys() -> TestResult {
        for (path, source, expected) in [
            (
                "settings.ini",
                "[workspace]\nroot=true\n[café]\nname=tea\n[broken\n",
                &["workspace", "café"][..],
            ),
            (
                ".editorconfig",
                "root = true\n[*.rs]\nindent_size = 4\n",
                &["*.rs"],
            ),
            (
                ".gitmodules",
                "[submodule \"café\"]\npath = crates/cafe\n",
                &["submodule \"café\""],
            ),
            (
                "messages.properties",
                "app.title=Café\napp.subtitle=Tea\nbroken\n",
                &["app.title", "app.subtitle"],
            ),
            (
                ".env",
                "API_URL=https://example.test\nCAFÉ=tea\n",
                &["API_URL", "CAFÉ"],
            ),
        ] {
            let _ = assert_names(path, source, expected)?;
        }
        Ok(())
    }

    #[test]
    fn xml_and_svg_prioritize_id_and_name_attributes() -> TestResult {
        let xml = assert_names(
            "catalog.xml",
            "<catalog id=\"café\"><group name=\"tea\"><anonymous><item id=\"leaf\"/></anonymous></group></catalog><broken id=\"recover\">",
            &["café", "tea", "leaf"],
        )?;
        assert!(!names(&xml).contains(&"anonymous"));

        let svg = assert_names(
            "icon.svg",
            "<svg id=\"logo\"><g id=\"foreground\"><path name=\"accent\" /></g><path /></svg>",
            &["logo", "foreground", "accent"],
        )?;
        assert_eq!(svg[0].children[0].container_name.as_deref(), Some("logo"));
        Ok(())
    }

    #[test]
    fn ecosystem_lockfiles_route_to_their_native_data_semantics() -> TestResult {
        let cargo = assert_names(
            "Cargo.lock",
            "[[package]]\nname = \"café\"\n[[package]]\nname = \"tea\"\n",
            &["package"],
        )?;
        assert_eq!(names(&cargo), vec!["package", "package"]);

        let npm = assert_names(
            "package-lock.json",
            "{\"packages\": {\"\": {}, \"node_modules/café\": {\"version\": \"1\"}}}",
            &["packages", "node_modules/café"],
        )?;
        assert!(!names(&npm).contains(&"version"));

        let pnpm = assert_names(
            "pnpm-lock.yaml",
            "lockfileVersion: '9.0'\npackages:\n  café@1.0.0:\n    resolution:\n      integrity: sha512-x\n",
            &["packages", "café@1.0.0", "resolution"],
        )?;
        assert!(!names(&pnpm).contains(&"integrity"));

        let yarn = assert_names(
            "yarn.lock",
            "# yarn lockfile v1\n\"café@^1.0.0\":\n  version \"1.2.0\"\n  dependencies:\n    tea \"^2.0.0\"\n\"tea@^2.0.0\":\n  version \"2.1.0\"\n",
            &["café@^1.0.0", "tea@^2.0.0"],
        )?;
        assert!(!names(&yarn).contains(&"version"));
        Ok(())
    }

    #[test]
    fn structured_formats_are_empty_safe_and_pkl_stays_deferred() -> TestResult {
        for path in [
            "data.json5",
            "settings.ini",
            "messages.properties",
            "data.edn",
            "data.cbor",
            "catalog.xml",
            "icon.svg",
            ".editorconfig",
            ".env",
            ".gitmodules",
            "Cargo.lock",
            "package-lock.json",
            "pnpm-lock.yaml",
        ] {
            assert!(symbols(path, "")?.is_empty(), "{path}");
        }
        assert!(language_id_from_path(Path::new("settings.pkl")).is_none());
        Ok(())
    }
}
