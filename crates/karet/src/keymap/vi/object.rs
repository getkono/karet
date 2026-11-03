//! Text objects (`iw`, `a(`, `i"`, `ip`, `it`, …): the span around the cursor
//! an operator or a selection acts on, shared by vim (`diw`) and helix
//! (`miw`).

use karet_core::LineCol;
use karet_core::Range;

use super::motion::scan_for;
use super::text::Class;
use super::text::Doc;
use super::text::class;

/// A span a text object covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Span {
    /// Characters from `start` to `end` (exclusive).
    Chars(Range),
    /// Whole lines `first..=last`.
    Lines(u32, u32),
}

/// Which text object a key names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Object {
    /// A word (`w`) or `WORD` (`W`).
    Word { big: bool },
    /// A paragraph (`p`).
    Paragraph,
    /// A quoted string delimited by this quote.
    Quote(char),
    /// A bracket pair, by its opener.
    Pair(char, char),
    /// The innermost pair of any bracket kind around the cursor (helix `m`).
    AnyPair,
    /// An XML/HTML element (`t`).
    Tag,
}

impl Object {
    /// The object a key names after `i`/`a` (vim) or `mi`/`ma` (helix).
    pub(super) fn from_char(c: char) -> Option<Self> {
        Some(match c {
            'w' => Self::Word { big: false },
            'W' => Self::Word { big: true },
            'p' => Self::Paragraph,
            '"' | '\'' | '`' => Self::Quote(c),
            '(' | ')' | 'b' => Self::Pair('(', ')'),
            '[' | ']' => Self::Pair('[', ']'),
            '{' | '}' | 'B' => Self::Pair('{', '}'),
            '<' | '>' => Self::Pair('<', '>'),
            'm' => Self::AnyPair,
            't' => Self::Tag,
            _ => return None,
        })
    }
}

/// The span of `object` around `p`, `inner` (`i`) or around (`a`), widened
/// `count` times where the object nests. `None` when there is none.
pub(super) fn select(
    doc: &Doc,
    p: LineCol,
    object: Object,
    inner: bool,
    count: u32,
) -> Option<Span> {
    match object {
        Object::Word { big } => word(doc, p, big, inner, count),
        Object::Paragraph => Some(paragraph(doc, p.line, inner, count)),
        Object::Quote(q) => quote(doc, p, q, inner),
        Object::Pair(open, close) => pair(doc, p, open, close, inner, count),
        Object::AnyPair => PAIRS
            .iter()
            .filter_map(|&(open, close)| pair(doc, p, open, close, inner, count))
            .min_by_key(|span| span_len(doc, *span)),
        Object::Tag => tag(doc, p, inner, count),
    }
}

/// Every bracket pair the any-pair object considers.
const PAIRS: [(char, char); 4] = [('(', ')'), ('[', ']'), ('{', '}'), ('<', '>')];

/// A rough size used to pick the innermost of several candidate spans.
fn span_len(doc: &Doc, span: Span) -> u64 {
    match span {
        Span::Chars(range) => doc.text(range).chars().count() as u64,
        Span::Lines(first, last) => doc.lines_text(first, last).chars().count() as u64,
    }
}

/// `iw`/`aw` (and the `WORD` forms): the run under the cursor, with `a`
/// taking the blanks after it (or, with none, before it).
fn word(doc: &Doc, p: LineCol, big: bool, inner: bool, count: u32) -> Option<Span> {
    let chars = doc.chars(p.line);
    if chars.is_empty() {
        return None;
    }
    let len = chars.len();
    let kind = |i: usize| chars.get(i).map(|c| class(*c, big));
    let run_end = |from: usize| {
        let k = kind(from);
        let mut end = from;
        while end < len && kind(end) == k {
            end += 1;
        }
        end
    };
    let at = (p.col as usize).min(len - 1);
    let mut start = at;
    while start > 0 && kind(start - 1) == kind(at) {
        start -= 1;
    }
    // The next run after `end`, or `end` at the line's end.
    let step = |end: usize| if end < len { run_end(end) } else { end };
    let mut end = run_end(at);
    let count = count.max(1);
    if inner {
        for _ in 1..count {
            end = step(end);
        }
    } else if kind(at) == Some(Class::Blank) {
        // On blanks: each count takes the blanks and the word after them.
        end = step(end);
        for _ in 1..count {
            end = step(step(end));
        }
    } else {
        // On a word: each count takes the word and the blanks after it.
        let trailing = end < len && kind(end) == Some(Class::Blank);
        if trailing {
            end = step(end);
        }
        for _ in 1..count {
            end = step(step(end));
        }
        if !trailing {
            while start > 0 && kind(start - 1) == Some(Class::Blank) {
                start -= 1;
            }
        }
    }
    let col = |i: usize| u32::try_from(i).unwrap_or(u32::MAX);
    Some(Span::Chars(Range {
        start: LineCol::new(p.line, col(start)),
        end: LineCol::new(p.line, col(end)),
    }))
}

/// `ip`/`ap`: the run of blank or non-blank lines holding `line`, with `a`
/// taking the blank lines after it (or, with none, before it).
fn paragraph(doc: &Doc, line: u32, inner: bool, count: u32) -> Span {
    let last = doc.last_line();
    let blank = |l: u32| doc.is_blank_line(l);
    let run_end = |from: u32| {
        let kind = blank(from);
        let mut end = from;
        while end < last && blank(end + 1) == kind {
            end += 1;
        }
        end
    };
    let kind = blank(line);
    let mut first = line;
    while first > 0 && blank(first - 1) == kind {
        first -= 1;
    }
    let mut end = run_end(line);
    for _ in 1..count.max(1) {
        if end >= last {
            break;
        }
        end = run_end(end + 1);
    }
    if !inner {
        if end < last {
            end = run_end(end + 1);
        } else {
            while first > 0 && blank(first - 1) != kind {
                first -= 1;
            }
        }
    }
    Span::Lines(first, end)
}

/// `i"`/`a"`: the quoted string on the line around the cursor, or the first
/// one after it. `a` takes the quotes and the blanks after them.
fn quote(doc: &Doc, p: LineCol, q: char, inner: bool) -> Option<Span> {
    let chars = doc.chars(p.line);
    let mut quotes = Vec::new();
    let mut escaped = false;
    for (i, c) in chars.iter().enumerate() {
        if escaped {
            escaped = false;
        } else if *c == '\\' {
            escaped = true;
        } else if *c == q {
            quotes.push(i);
        }
    }
    let at = p.col as usize;
    let pairs: Vec<(usize, usize)> = quotes
        .chunks_exact(2)
        .filter_map(|pair| Some((*pair.first()?, *pair.get(1)?)))
        .collect();
    let (open, close) = pairs
        .iter()
        .copied()
        .find(|&(open, close)| open <= at && at <= close)
        .or_else(|| pairs.iter().copied().find(|&(open, _)| open > at))?;
    let (mut start, mut end) = if inner {
        (open + 1, close)
    } else {
        (open, close + 1)
    };
    if !inner {
        let blank = |i: usize| chars.get(i).is_some_and(|c| *c == ' ' || *c == '\t');
        if blank(end) {
            while blank(end) {
                end += 1;
            }
        } else {
            while start > 0 && blank(start - 1) {
                start -= 1;
            }
        }
    }
    let col = |i: usize| u32::try_from(i).unwrap_or(u32::MAX);
    Some(Span::Chars(Range {
        start: LineCol::new(p.line, col(start)),
        end: LineCol::new(p.line, col(end)),
    }))
}

/// `i(`/`a(` and the other brackets: the `count`-th pair enclosing the
/// cursor. An inner block whose brackets sit on their own lines is the lines
/// between them, as vim's `di{` leaves the braces.
fn pair(doc: &Doc, p: LineCol, open: char, close: char, inner: bool, count: u32) -> Option<Span> {
    let mut from = p;
    let mut found = None;
    for _ in 0..count.max(1) {
        // On an opener, that pair; anywhere else (a closer included, whose
        // own character the backward scan skips) the nearest unmatched
        // opener before it.
        let start = if doc.ch(from) == open && found.is_none() {
            from
        } else {
            scan_for(doc, from, close, open, false)?
        };
        let end = scan_for(doc, start, open, close, true)?;
        found = Some((start, end));
        from = start;
    }
    let (start, end) = found?;
    if !inner {
        return Some(Span::Chars(Range {
            start,
            end: doc.after(end),
        }));
    }
    let after_open = doc.after(start);
    let opens_line = doc.ch(after_open) == '\n' || after_open.line > start.line;
    let closes_line = doc
        .chars(end.line)
        .get(..end.col as usize)
        .unwrap_or_default()
        .iter()
        .all(|c| c.is_whitespace());
    if opens_line && closes_line && end.line > start.line + 1 {
        return Some(Span::Lines(start.line + 1, end.line - 1));
    }
    Some(Span::Chars(Range {
        start: after_open,
        end,
    }))
}

/// `it`/`at`: the `count`-th element enclosing the cursor, found by a light
/// scan of `<name …>` / `</name>` tags (no comments, no CDATA).
fn tag(doc: &Doc, p: LineCol, inner: bool, count: u32) -> Option<Span> {
    let last = doc.last_line();
    let mut text: Vec<char> = Vec::new();
    let mut starts = Vec::with_capacity(last as usize + 1);
    for line in 0..=last {
        starts.push(text.len());
        text.extend(doc.chars(line).iter());
        text.push('\n');
    }
    let offset =
        |pos: LineCol| starts.get(pos.line as usize).copied().unwrap_or(0) + pos.col as usize;
    let to_pos = |off: usize| {
        let line = starts.partition_point(|&s| s <= off).saturating_sub(1);
        let col = off - starts.get(line).copied().unwrap_or(0);
        LineCol::new(
            u32::try_from(line).unwrap_or(u32::MAX),
            u32::try_from(col).unwrap_or(u32::MAX),
        )
    };
    // (open_start, open_end, close_start, close_end) of every matched element.
    let mut elements = Vec::new();
    let mut stack: Vec<(String, usize, usize)> = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if text.get(i) != Some(&'<') {
            i += 1;
            continue;
        }
        let Some(end) = (i..text.len()).find(|&j| text.get(j) == Some(&'>')) else {
            break;
        };
        let body: String = text.get(i + 1..end).unwrap_or_default().iter().collect();
        if let Some(name) = body.strip_prefix('/') {
            let name = name.trim();
            if let Some(at) = stack.iter().rposition(|(open, _, _)| open == name) {
                let (_, open_start, open_end) = stack.get(at).cloned()?;
                stack.truncate(at);
                elements.push((open_start, open_end, i, end + 1));
            }
        } else if !body.starts_with(['!', '?']) && !body.ends_with('/') {
            let name: String = body
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '/')
                .collect();
            if !name.is_empty() {
                stack.push((name, i, end + 1));
            }
        }
        i = end + 1;
    }
    let at = offset(p);
    let mut enclosing: Vec<_> = elements
        .into_iter()
        .filter(|&(open_start, _, _, close_end)| open_start <= at && at < close_end)
        .collect();
    enclosing.sort_by_key(|&(open_start, _, _, close_end)| close_end - open_start);
    let (open_start, open_end, close_start, close_end) =
        *enclosing.get(count.max(1) as usize - 1)?;
    let (start, end) = if inner {
        (open_end, close_start)
    } else {
        (open_start, close_end)
    };
    Some(Span::Chars(Range {
        start: to_pos(start),
        end: to_pos(end),
    }))
}
