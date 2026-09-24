//! A lenient HTML tag lexer for the markup markdown embeds.
//!
//! This is not an HTML parser: it recognises tags, attributes, comments and character
//! references well enough for the render model to map a curated subset of elements
//! (see `parse/html.rs`), and it never fails — anything it cannot read as markup is text.
//! No tree is built and no content model is enforced; nesting is the caller's business.
//!
//! `pulldown-cmark` hands an HTML block over line by line, so a tag or comment may be
//! split across [`Tokenizer::feed`] calls. The unfinished tail is held back until the next
//! chunk completes it, bounded by [`PENDING_CAP`]; [`Tokenizer::flush`] releases whatever
//! is left once the block ends.

#[cfg(test)]
mod tests;

/// The most unfinished markup held back waiting for its end. Past it the tail is
/// released as literal text, so a stray `<` can never swallow a document.
const PENDING_CAP: usize = 16 * 1024;

/// Elements whose content is never rendered, and is skipped unread up to the matching
/// close tag (their content is not markup: `if (a<b)` inside a script is no tag).
pub(crate) const RAW_ELEMENTS: [&str; 4] = ["script", "style", "iframe", "object"];

/// A tag's attributes: lowercased names with their decoded values, in source order.
pub(crate) type Attrs = Vec<(String, String)>;

/// One lexical unit of HTML.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Token {
    /// An opening tag: its lowercased name and attributes, and whether it ended `/>`.
    Open {
        name: String,
        attrs: Attrs,
        self_closing: bool,
    },
    /// A closing tag's lowercased name.
    Close(String),
    /// Text, with character references decoded.
    Text(String),
}

/// The value of attribute `name` among `attrs`, if present.
pub(crate) fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// A stateful lexer fed an HTML block chunk by chunk.
#[derive(Debug, Default)]
pub(crate) struct Tokenizer {
    /// Unfinished markup from the previous chunk, awaiting its end.
    pending: String,
    /// The raw element being skipped, if inside one.
    raw: Option<String>,
}

impl Tokenizer {
    /// Lex `chunk`, appending complete tokens to `out`.
    pub(crate) fn feed(&mut self, chunk: &str, out: &mut Vec<Token>) {
        let mut input = std::mem::take(&mut self.pending);
        input.push_str(chunk);
        let mut rest = input.as_str();
        while !rest.is_empty() {
            if let Some(name) = self.raw.take() {
                match skip_raw(rest, &name) {
                    Some(after) => {
                        out.push(Token::Close(name));
                        rest = after;
                    },
                    None => {
                        self.raw = Some(name);
                        return;
                    },
                }
                continue;
            }
            let Some(lt) = rest.find('<') else {
                push_text(out, rest);
                return;
            };
            push_text(out, &rest[..lt]);
            rest = &rest[lt..];
            match lex_markup(rest) {
                Lexed::Token(token, after) => {
                    if let Token::Open {
                        name,
                        self_closing: false,
                        ..
                    } = &token
                        && RAW_ELEMENTS.contains(&name.as_str())
                    {
                        self.raw = Some(name.clone());
                    }
                    out.push(token);
                    rest = after;
                },
                Lexed::Skip(after) => rest = after,
                Lexed::Literal => {
                    push_text(out, "<");
                    rest = &rest[1..];
                },
                Lexed::Incomplete if rest.len() <= PENDING_CAP => {
                    self.pending = rest.to_owned();
                    return;
                },
                Lexed::Incomplete => {
                    push_text(out, "<");
                    rest = &rest[1..];
                },
            }
        }
    }

    /// Release everything held back: an unfinished tag becomes literal text, an
    /// unterminated comment is dropped, and an unclosed raw element is closed.
    pub(crate) fn flush(&mut self, out: &mut Vec<Token>) {
        self.flush_pending(out);
        if let Some(name) = self.raw.take() {
            out.push(Token::Close(name));
        }
    }

    /// Release the unfinished markup held back, leaving a raw element open.
    pub(crate) fn flush_pending(&mut self, out: &mut Vec<Token>) {
        let pending = std::mem::take(&mut self.pending);
        if !pending.starts_with("<!") && !pending.starts_with("<?") {
            push_text(out, &pending);
        }
    }
}

/// The outcome of lexing markup at a `<`.
enum Lexed<'a> {
    /// A tag, and the input after it.
    Token(Token, &'a str),
    /// A comment, doctype, CDATA section or processing instruction to drop, and the
    /// input after it.
    Skip(&'a str),
    /// Not markup: the `<` is text.
    Literal,
    /// Markup whose end lies beyond the input.
    Incomplete,
}

/// Lex the markup `input` begins with (`input` starts with `<`).
fn lex_markup(input: &str) -> Lexed<'_> {
    let after_lt = &input[1..];
    if let Some(body) = after_lt.strip_prefix("!--") {
        return skip_past(body, "-->");
    }
    if let Some(body) = after_lt.strip_prefix("![CDATA[") {
        return skip_past(body, "]]>");
    }
    if let Some(body) = after_lt.strip_prefix(['!', '?']) {
        return skip_past(body, ">");
    }
    let (closing, body) = match after_lt.strip_prefix('/') {
        Some(body) => (true, body),
        None => (false, after_lt),
    };
    if !body.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return Lexed::Literal;
    }
    let name_len = body
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == ':'))
        .unwrap_or(body.len());
    let name = body[..name_len].to_ascii_lowercase();
    let Some((attrs, self_closing, after)) = lex_attrs(&body[name_len..]) else {
        return Lexed::Incomplete;
    };
    let token = if closing {
        Token::Close(name)
    } else {
        Token::Open {
            name,
            attrs,
            self_closing,
        }
    };
    Lexed::Token(token, after)
}

/// Drop everything up to and including `end`.
fn skip_past<'a>(input: &'a str, end: &str) -> Lexed<'a> {
    input.find(end).map_or(Lexed::Incomplete, |at| {
        Lexed::Skip(&input[at + end.len()..])
    })
}

/// A tag's attributes up to its closing `>`: the attributes, whether the tag ended
/// `/>`, and the input after it. `None` when the input ends first.
fn lex_attrs(mut input: &str) -> Option<(Attrs, bool, &str)> {
    let mut attrs = Vec::new();
    loop {
        input = input.trim_start();
        let first = input.chars().next()?;
        if first == '>' {
            return Some((attrs, false, &input[1..]));
        }
        if let Some(after) = input.strip_prefix("/>") {
            return Some((attrs, true, after));
        }
        if first == '/' {
            input = &input[1..];
            continue;
        }
        let name_len = input
            .find(|c: char| c.is_whitespace() || matches!(c, '=' | '>' | '/'))
            .unwrap_or(input.len());
        let name = input[..name_len].to_ascii_lowercase();
        input = input[name_len..].trim_start();
        if input.is_empty() {
            return None;
        }
        let value = match input.strip_prefix('=') {
            Some(after) => {
                let (value, after) = lex_value(after.trim_start())?;
                input = after;
                decode_entities(value)
            },
            None => String::new(),
        };
        if !name.is_empty() {
            attrs.push((name, value));
        }
    }
}

/// An attribute value — quoted or bare — and the input after it.
fn lex_value(input: &str) -> Option<(&str, &str)> {
    let quote = input.chars().next()?;
    if matches!(quote, '"' | '\'') {
        let body = &input[1..];
        let end = body.find(quote)?;
        return Some((&body[..end], &body[end + 1..]));
    }
    let end = input.find(|c: char| c.is_whitespace() || c == '>')?;
    Some((&input[..end], &input[end..]))
}

/// Skip a raw element's content up to and past `</name…>`, returning the input after
/// it, or `None` when the close tag is not in `input`.
fn skip_raw<'a>(input: &'a str, name: &str) -> Option<&'a str> {
    let lower = input.to_ascii_lowercase();
    let close = format!("</{name}");
    let mut from = 0;
    while let Some(at) = lower[from..].find(&close).map(|at| from + at) {
        let after = at + close.len();
        // `</scripts>` does not close `<script>`.
        if lower[after..].starts_with(|c: char| c.is_ascii_alphanumeric()) {
            from = after;
            continue;
        }
        let end = lower[after..]
            .find('>')
            .map_or(input.len(), |gt| after + gt + 1);
        return Some(&input[end..]);
    }
    None
}

/// Append `text`, decoded, to `out`, merging with a preceding text token.
fn push_text(out: &mut Vec<Token>, text: &str) {
    if text.is_empty() {
        return;
    }
    let text = decode_entities(text);
    if let Some(Token::Text(last)) = out.last_mut() {
        last.push_str(&text);
    } else {
        out.push(Token::Text(text));
    }
}

/// Decode the character references in `text`. An unknown or malformed reference is
/// kept as written.
pub(crate) fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let decoded = rest
            .find(';')
            .filter(|&semi| semi <= 12)
            .and_then(|semi| entity(&rest[1..semi]).map(|c| (c, semi)));
        match decoded {
            Some((c, semi)) => {
                out.push(c);
                rest = &rest[semi + 1..];
            },
            None => {
                out.push('&');
                rest = &rest[1..];
            },
        }
    }
    out.push_str(rest);
    out
}

/// The character a reference names (`amp`, `#39`, `#x27`), if known.
fn entity(name: &str) -> Option<char> {
    if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        // NUL and the surrogates are not characters; `from_u32` rejects the latter.
        return char::from_u32(code).filter(|&c| c != '\0');
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "middot" => '·',
        "bull" => '•',
        _ => return None,
    })
}
