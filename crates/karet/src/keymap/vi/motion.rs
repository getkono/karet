//! Cursor motions shared by the vim and helix grammars. Each is a pure
//! function of the document and a position, so the grammars differ only in
//! what they do with the target (move, select, or operate up to it).

use karet_core::LineCol;

use super::text::Class;
use super::text::Doc;
use super::text::class;

/// The start of the next word (`w`, or `W` when `big`). An empty line counts
/// as a word, as in vim. Stops at the end of the document.
pub(super) fn word_start_forward(doc: &Doc, p: LineCol, big: bool) -> LineCol {
    let mut it = p;
    let start = class(doc.ch(it), big);
    if matches!(start, Class::Word | Class::Punct) {
        loop {
            let Some(next) = doc.next(it) else {
                return it;
            };
            it = next;
            if class(doc.ch(it), big) != start {
                break;
            }
        }
    }
    loop {
        match class(doc.ch(it), big) {
            Class::Eol => {
                let Some(next) = doc.next(it) else {
                    return it;
                };
                it = next;
                if doc.len(it.line) == 0 {
                    return it;
                }
            },
            Class::Blank => {
                let Some(next) = doc.next(it) else {
                    return it;
                };
                it = next;
            },
            Class::Word | Class::Punct => return it,
        }
    }
}

/// The end of the word after `p` (`e`/`E`): at least one character on.
pub(super) fn word_end_forward(doc: &Doc, p: LineCol, big: bool) -> LineCol {
    let Some(mut it) = doc.next(p) else {
        return p;
    };
    while matches!(class(doc.ch(it), big), Class::Blank | Class::Eol) {
        let Some(next) = doc.next(it) else {
            return it;
        };
        it = next;
    }
    let run = class(doc.ch(it), big);
    while let Some(next) = doc.next(it) {
        if class(doc.ch(next), big) != run {
            break;
        }
        it = next;
    }
    it
}

/// The start of the word before `p` (`b`/`B`). Stops on an empty line.
pub(super) fn word_start_backward(doc: &Doc, p: LineCol, big: bool) -> LineCol {
    let Some(mut it) = doc.prev(p) else {
        return p;
    };
    loop {
        match class(doc.ch(it), big) {
            Class::Eol if doc.len(it.line) == 0 => return it,
            Class::Eol | Class::Blank => {},
            Class::Word | Class::Punct => break,
        }
        let Some(prev) = doc.prev(it) else {
            return it;
        };
        it = prev;
    }
    let run = class(doc.ch(it), big);
    while let Some(prev) = doc.prev(it) {
        if class(doc.ch(prev), big) != run {
            break;
        }
        it = prev;
    }
    it
}

/// The end of the word before `p` (`ge`/`gE`). Stops on an empty line.
pub(super) fn word_end_backward(doc: &Doc, p: LineCol, big: bool) -> LineCol {
    let start = class(doc.ch(p), big);
    let mut it = p;
    if matches!(start, Class::Word | Class::Punct) {
        loop {
            let Some(prev) = doc.prev(it) else {
                return LineCol::new(0, 0);
            };
            it = prev;
            if class(doc.ch(it), big) != start {
                break;
            }
        }
    } else {
        let Some(prev) = doc.prev(it) else {
            return it;
        };
        it = prev;
    }
    loop {
        match class(doc.ch(it), big) {
            Class::Eol if doc.len(it.line) == 0 => return it,
            Class::Eol | Class::Blank => {},
            Class::Word | Class::Punct => return it,
        }
        let Some(prev) = doc.prev(it) else {
            return it;
        };
        it = prev;
    }
}

/// Apply `step` `count` times (at least once), stopping early once it no
/// longer moves.
pub(super) fn repeat(
    doc: &Doc,
    p: LineCol,
    count: u32,
    step: impl Fn(&Doc, LineCol) -> LineCol,
) -> LineCol {
    let mut it = p;
    for _ in 0..count.max(1) {
        let next = step(doc, it);
        if next == it {
            break;
        }
        it = next;
    }
    it
}

/// Apply a fallible `step` `count` times (at least once); `None` when the
/// first step finds nothing.
pub(super) fn repeat_opt(
    doc: &Doc,
    p: LineCol,
    count: u32,
    step: impl Fn(&Doc, LineCol) -> Option<LineCol>,
) -> Option<LineCol> {
    let mut it = step(doc, p)?;
    for _ in 1..count.max(1) {
        match step(doc, it) {
            Some(next) if next != it => it = next,
            _ => break,
        }
    }
    Some(it)
}

/// Which way, and how far, a character search (`f`, `t`, `F`, `T`) goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CharSearch {
    /// The character searched for.
    pub(super) ch: char,
    /// Search toward the end of the line.
    pub(super) forward: bool,
    /// Stop one short of the character (`t`/`T`).
    pub(super) till: bool,
}

impl CharSearch {
    /// The same search in the other direction (`,`).
    pub(super) fn reversed(self) -> Self {
        Self {
            forward: !self.forward,
            ..self
        }
    }
}

/// The target of `search` from `p`, `count` occurrences on, within the line.
/// `repeat` is a `;`/`,` repeat, which steps past an adjacent match of a
/// `t`/`T` search so the repeat is not stuck on it.
pub(super) fn find_char(
    doc: &Doc,
    p: LineCol,
    search: CharSearch,
    count: u32,
    repeat: bool,
) -> Option<LineCol> {
    let chars = doc.chars(p.line);
    let mut col = i64::from(p.col);
    let len = chars.len() as i64;
    let step: i64 = if search.forward { 1 } else { -1 };
    if repeat && search.till {
        col += step;
    }
    let mut found = 0;
    loop {
        col += step;
        if col < 0 || col >= len {
            return None;
        }
        if chars.get(col as usize) == Some(&search.ch) {
            found += 1;
            if found >= count.max(1) {
                break;
            }
        }
    }
    if search.till {
        col -= step;
    }
    Some(LineCol::new(p.line, u32::try_from(col).ok()?))
}

/// The bracket characters `%` and the bracket objects pair.
const PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

/// The partner of the bracket at `p`, or — when `p` is not on one — of the
/// first bracket after it on the line (`%`). Counts nesting; does not read
/// strings or comments.
pub(super) fn matching_bracket(doc: &Doc, p: LineCol) -> Option<LineCol> {
    let chars = doc.chars(p.line);
    let col = (p.col as usize..chars.len()).find(|&col| {
        chars
            .get(col)
            .is_some_and(|c| PAIRS.iter().any(|(o, e)| c == o || c == e))
    })?;
    let at = LineCol::new(p.line, u32::try_from(col).ok()?);
    let c = doc.ch(at);
    for (open, close) in PAIRS {
        if c == open {
            return scan_for(doc, at, open, close, true);
        }
        if c == close {
            return scan_for(doc, at, close, open, false);
        }
    }
    None
}

/// The most characters a bracket scan reads before giving up, so a stray
/// bracket in a huge file cannot stall a keystroke.
const SCAN_LIMIT: usize = 1 << 20;

/// From the bracket `same` at `p`, the matching `other`, scanning forward or
/// backward and counting nesting.
pub(super) fn scan_for(
    doc: &Doc,
    p: LineCol,
    same: char,
    other: char,
    forward: bool,
) -> Option<LineCol> {
    let mut depth = 0usize;
    let mut it = p;
    for _ in 0..SCAN_LIMIT {
        it = if forward {
            doc.next(it)?
        } else {
            doc.prev(it)?
        };
        let c = doc.ch(it);
        if c == same {
            depth += 1;
        } else if c == other {
            if depth == 0 {
                return Some(it);
            }
            depth -= 1;
        }
    }
    None
}

/// The next paragraph boundary after `p` (`}`): the next empty line past the
/// current paragraph, or the end of the document.
pub(super) fn paragraph_forward(doc: &Doc, p: LineCol) -> LineCol {
    let last = doc.last_line();
    let mut line = p.line;
    while line < last && doc.len(line) == 0 {
        line += 1;
    }
    while line < last {
        line += 1;
        if doc.len(line) == 0 {
            return LineCol::new(line, 0);
        }
    }
    doc.line_end(last)
}

/// The previous paragraph boundary before `p` (`{`).
pub(super) fn paragraph_backward(doc: &Doc, p: LineCol) -> LineCol {
    let mut line = p.line;
    while line > 0 && doc.len(line) == 0 {
        line -= 1;
    }
    while line > 0 {
        line -= 1;
        if doc.len(line) == 0 {
            return LineCol::new(line, 0);
        }
    }
    LineCol::new(0, 0)
}

/// `p` moved `delta` lines, steering toward column `goal` (`u32::MAX` for
/// "end of line"), clamped onto a character unless `past_end`.
pub(super) fn vertical(doc: &Doc, p: LineCol, delta: i64, goal: u32, past_end: bool) -> LineCol {
    let line = (i64::from(p.line) + delta).clamp(0, i64::from(doc.last_line()));
    let line = u32::try_from(line).unwrap_or_default();
    let len = doc.len(line);
    let max = if past_end { len } else { len.saturating_sub(1) };
    LineCol::new(line, goal.min(max))
}

/// Find `needle` from `p` (exclusive) in the given direction, wrapping around
/// the document; `word` matches only whole words. Returns the match start.
pub(super) fn search(
    doc: &Doc,
    p: LineCol,
    needle: &str,
    forward: bool,
    word: bool,
) -> Option<LineCol> {
    let needle: Vec<char> = needle.chars().collect();
    if needle.is_empty() || needle.contains(&'\n') {
        return None;
    }
    let smart_case = !needle.iter().any(|c| c.is_uppercase());
    let eq = |a: char, b: char| {
        if smart_case {
            a.to_lowercase().eq(b.to_lowercase())
        } else {
            a == b
        }
    };
    let total = doc.last_line() + 1;
    let hits = |line: u32| -> Vec<u32> {
        let chars = doc.chars(line);
        let mut hits = Vec::new();
        if chars.len() < needle.len() {
            return hits;
        }
        for start in 0..=chars.len() - needle.len() {
            let window = chars.get(start..start + needle.len()).unwrap_or_default();
            if !window.iter().zip(&needle).all(|(a, b)| eq(*a, *b)) {
                continue;
            }
            if word {
                let is_word = |c: Option<&char>| c.is_some_and(|c| class(*c, false) == Class::Word);
                let before = start.checked_sub(1).and_then(|i| chars.get(i));
                if is_word(before) || is_word(chars.get(start + needle.len())) {
                    continue;
                }
            }
            hits.push(u32::try_from(start).unwrap_or(u32::MAX));
        }
        hits
    };
    for offset in 0..=total {
        let line = if forward {
            (p.line + offset) % total
        } else {
            (p.line + total * 2 - offset) % total
        };
        let found = hits(line);
        let pick = match (forward, offset) {
            (true, 0) => found.into_iter().find(|&col| col > p.col),
            (false, 0) => found.into_iter().rev().find(|&col| col < p.col),
            (true, _) if offset == total => found.into_iter().find(|&col| col <= p.col),
            (false, _) if offset == total => found.into_iter().rev().find(|&col| col >= p.col),
            (true, _) => found.into_iter().next(),
            (false, _) => found.into_iter().next_back(),
        };
        if let Some(col) = pick {
            return Some(LineCol::new(line, col));
        }
    }
    None
}

/// The word under (or after) `p` on its line, and where it starts, for `*`
/// and `#`.
pub(super) fn word_under(doc: &Doc, p: LineCol) -> Option<(LineCol, String)> {
    let chars = doc.chars(p.line);
    let start = (p.col as usize..chars.len()).find(|&i| {
        chars
            .get(i)
            .is_some_and(|c| class(*c, false) == Class::Word)
    })?;
    let mut from = start;
    while from > 0
        && chars
            .get(from - 1)
            .is_some_and(|c| class(*c, false) == Class::Word)
    {
        from -= 1;
    }
    let mut to = start;
    while chars
        .get(to)
        .is_some_and(|c| class(*c, false) == Class::Word)
    {
        to += 1;
    }
    let word = chars.get(from..to).unwrap_or_default().iter().collect();
    Some((LineCol::new(p.line, u32::try_from(from).ok()?), word))
}
