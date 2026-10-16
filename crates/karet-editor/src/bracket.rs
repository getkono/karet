//! Bracket-pair matching at a caret: the pair the editor highlights, where Go to
//! Bracket lands, and what Select to Bracket covers.
//!
//! The brackets are `()`, `[]` and `{}`. One inside a string or comment is text,
//! not a bracket: the tree-sitter highlight spans the editor already paints say
//! which bytes those are, so a `"("` in a string literal neither matches nor
//! unbalances the code around it. Without highlights (plain text, or before the
//! first highlight lands) every bracket counts.
//!
//! A pair is matched by counting brackets of its own kind only, as Vim's `%`
//! does, so a stray bracket of another kind cannot hide an otherwise balanced
//! pair.

use karet_core::CursorState;
use karet_core::Decoration;
use karet_core::DecorationKind;
use karet_core::Highlights;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::StandardToken;
use karet_core::ThemeRole;
use karet_text::TextBuffer;

/// How many lines a match scans past the line it starts on, in either
/// direction, before giving up: it bounds the per-frame cost of the highlight
/// when the caret sits beside a bracket that is never closed.
pub(crate) const SCAN_LINES: usize = 10_000;

/// Opening brackets, indexed by kind; [`CLOSE`] holds their partners.
const OPEN: [char; 3] = ['(', '[', '{'];
/// Closing brackets, indexed by kind.
const CLOSE: [char; 3] = [')', ']', '}'];

/// A matched bracket pair: the positions of its opening and closing brackets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BracketPair {
    /// The opening bracket.
    pub open: LineCol,
    /// The closing bracket.
    pub close: LineCol,
}

impl BracketPair {
    /// The range covering both brackets and everything between them.
    #[must_use]
    pub fn outer(self) -> Range {
        Range {
            start: self.open,
            end: LineCol::new(self.close.line, self.close.col + 1),
        }
    }
}

/// A bracket character: its kind (an index into [`OPEN`]/[`CLOSE`]) and side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bracket {
    kind: usize,
    open: bool,
}

/// Whether `token` marks text whose brackets are not code.
fn is_opaque(token: karet_core::TokenId) -> bool {
    [
        StandardToken::String,
        StandardToken::StringEscape,
        StandardToken::Comment,
        StandardToken::CommentDoc,
        StandardToken::CommentMark,
    ]
    .iter()
    .any(|t| t.id() == token)
}

/// The brackets of line `idx`, one slot per `char` (`None` for anything that is
/// not a code bracket), or `None` past the last line.
fn brackets_on(
    buffer: &TextBuffer,
    highlights: Option<&Highlights>,
    idx: usize,
) -> Option<Vec<Option<Bracket>>> {
    let text = buffer.line(idx)?;
    let span = buffer.line_to_byte_range(idx)?;
    Some(classify(highlights, span, &text))
}

/// The brackets of the first `cap` chars of line `idx`, as [`brackets_on`]
/// gives them for the whole line: only that prefix is copied out of the
/// buffer, so a minified line costs the chars asked for, not its length.
/// Empty past the last line.
fn brackets_in_prefix(
    buffer: &TextBuffer,
    highlights: Option<&Highlights>,
    idx: usize,
    cap: u32,
) -> Vec<Option<Bracket>> {
    let Some(line) = buffer.line_to_byte_range(idx) else {
        return Vec::new();
    };
    let end = u32::try_from(idx)
        .ok()
        .and_then(|l| buffer.line_col_to_byte(LineCol::new(l, cap)).ok())
        .unwrap_or(line.end);
    let span = karet_core::Span {
        start: line.start,
        end,
    };
    let text = buffer.slice(span).unwrap_or_default();
    classify(highlights, span, &text)
}

/// One slot per `char` of `text`, the bytes `span` of the buffer: the code
/// bracket it is, or `None` (not a bracket, or one `highlights` marks as text).
fn classify(
    highlights: Option<&Highlights>,
    span: karet_core::Span,
    text: &str,
) -> Vec<Option<Bracket>> {
    let opaque: Vec<_> = highlights
        .map(|h| h.spans_in(span))
        .unwrap_or_default()
        .iter()
        .filter(|s| is_opaque(s.token))
        .map(|s| s.span)
        .collect();
    text.char_indices()
        .map(|(off, ch)| {
            let byte = span.start.0 + off;
            let bracket = OPEN
                .iter()
                .position(|&c| c == ch)
                .map(|kind| Bracket { kind, open: true })
                .or_else(|| {
                    CLOSE
                        .iter()
                        .position(|&c| c == ch)
                        .map(|kind| Bracket { kind, open: false })
                })?;
            let hidden = opaque.iter().any(|s| s.start.0 <= byte && byte < s.end.0);
            (!hidden).then_some(bracket)
        })
        .collect()
}

/// The code bracket at `pos`, if any.
fn bracket_at(
    buffer: &TextBuffer,
    highlights: Option<&Highlights>,
    pos: LineCol,
) -> Option<Bracket> {
    brackets_on(buffer, highlights, pos.line as usize)?
        .get(pos.col as usize)
        .copied()
        .flatten()
}

/// Visit the code brackets strictly after (`forward`) or strictly before `from`,
/// nearest first, until `visit` returns a value or the scan runs out.
fn scan<R>(
    buffer: &TextBuffer,
    highlights: Option<&Highlights>,
    from: LineCol,
    forward: bool,
    mut visit: impl FnMut(LineCol, Bracket) -> Option<R>,
) -> Option<R> {
    let first = from.line as usize;
    let lines: Box<dyn Iterator<Item = usize>> = if forward {
        Box::new(first..=first.saturating_add(SCAN_LINES))
    } else {
        Box::new((first.saturating_sub(SCAN_LINES)..=first).rev())
    };
    for idx in lines {
        let slots = brackets_on(buffer, highlights, idx)?;
        let line = u32::try_from(idx).ok()?;
        let cols: Box<dyn Iterator<Item = usize>> = match (forward, idx == first) {
            (true, true) => Box::new(from.col as usize + 1..slots.len()),
            (true, false) => Box::new(0..slots.len()),
            (false, true) => Box::new((0..(from.col as usize).min(slots.len())).rev()),
            (false, false) => Box::new((0..slots.len()).rev()),
        };
        for col in cols {
            let Some(bracket) = slots.get(col).copied().flatten() else {
                continue;
            };
            let col = u32::try_from(col).ok()?;
            if let Some(found) = visit(LineCol::new(line, col), bracket) {
                return Some(found);
            }
        }
    }
    None
}

/// The partner of the bracket `bracket` at `at`, counting only its own kind.
fn partner(
    buffer: &TextBuffer,
    highlights: Option<&Highlights>,
    at: LineCol,
    bracket: Bracket,
) -> Option<LineCol> {
    let mut depth = 0u32;
    scan(buffer, highlights, at, bracket.open, |pos, b| {
        if b.kind != bracket.kind {
            return None;
        }
        if b.open == bracket.open {
            depth += 1;
            return None;
        }
        if depth == 0 {
            return Some(pos);
        }
        depth -= 1;
        None
    })
}

/// The pair whose bracket touches `pos`: the bracket just after the caret first,
/// then the one just before it. `None` when neither is a code bracket with a
/// partner.
#[must_use]
pub fn matching_pair(
    buffer: &TextBuffer,
    highlights: Option<&Highlights>,
    pos: LineCol,
) -> Option<BracketPair> {
    let before = pos
        .col
        .checked_sub(1)
        .map(|col| LineCol::new(pos.line, col));
    [Some(pos), before].into_iter().flatten().find_map(|at| {
        let bracket = bracket_at(buffer, highlights, at)?;
        let other = partner(buffer, highlights, at, bracket)?;
        Some(if bracket.open {
            BracketPair {
                open: at,
                close: other,
            }
        } else {
            BracketPair {
                open: other,
                close: at,
            }
        })
    })
}

/// The innermost pair whose brackets enclose `pos` (the opening bracket strictly
/// before it), or `None` when no opening bracket before `pos` is closed after
/// it. An opening bracket that is never closed is skipped and the scan goes on
/// outward, so a stray `[` in `f(a[b|c)` does not hide the parens around it.
#[must_use]
pub fn enclosing_pair(
    buffer: &TextBuffer,
    highlights: Option<&Highlights>,
    pos: LineCol,
) -> Option<BracketPair> {
    let mut depth = [0u32; 3];
    scan(buffer, highlights, pos, false, |at, b| {
        let d = depth.get_mut(b.kind)?;
        if !b.open {
            *d += 1;
            return None;
        }
        if *d == 0 {
            let close = partner(buffer, highlights, at, b)?;
            return Some(BracketPair { open: at, close });
        }
        *d -= 1;
        None
    })
}

/// The opening bracket a `close` bracket typed at `pos` would pair with: the
/// nearest opening bracket of its kind strictly before `pos` that no closing
/// bracket of that kind between them already answers. `None` when `close` is
/// not a closing bracket or no such opener is in range.
pub(crate) fn unclosed_opener(
    buffer: &TextBuffer,
    highlights: Option<&Highlights>,
    pos: LineCol,
    close: char,
) -> Option<LineCol> {
    let kind = CLOSE.iter().position(|&c| c == close)?;
    let mut depth = 0u32;
    scan(buffer, highlights, pos, false, |at, b| {
        if b.kind != kind {
            return None;
        }
        if !b.open {
            depth += 1;
            return None;
        }
        if depth == 0 {
            return Some(at);
        }
        depth -= 1;
        None
    })
}

/// The nesting depth of each code bracket on the lines a frame paints, for
/// bracket-pair colourisation.
///
/// Lines are asked for in ascending order, so one forward scan serves the
/// frame: it starts [`SCAN_LINES`] above the first line asked for — the
/// bound the pair matcher scans to as well — and keeps a stack of the open
/// brackets' kinds. A jump forward of more than [`SCAN_LINES`] (past a large
/// fold) restarts it the same distance above the line, so no frame steps
/// more lines than that per line it paints. Every kind nests together, so
/// `{ ( [ ] ) }` takes three depths. A closer pops back to the nearest
/// opener of its own kind, dropping any unclosed opener above it; a closer
/// with no opener has no depth and keeps its ordinary colour.
///
/// A painted line is read only as far as its row shows; the rest of it is
/// read once, when the scan moves past it to a later line.
pub(crate) struct BracketDepths<'a> {
    buffer: &'a TextBuffer,
    highlights: Option<&'a Highlights>,
    /// The first line the stack has not consumed, once the scan has started.
    next: Option<usize>,
    /// The kinds of the brackets still open at the start of line `next`.
    stack: Vec<usize>,
}

impl<'a> BracketDepths<'a> {
    /// A scan over `buffer`, skipping brackets `highlights` marks as text.
    pub(crate) fn new(buffer: &'a TextBuffer, highlights: Option<&'a Highlights>) -> Self {
        Self {
            buffer,
            highlights,
            next: None,
            stack: Vec::new(),
        }
    }

    /// The depth of each of the first `cap` chars of `line` (`None` for
    /// anything that is not a code bracket with a place in the nesting).
    /// Asking for a line above the one asked for last restarts the scan;
    /// asking for the same line again (the next row of a wrapped line) reads
    /// it again from its start.
    pub(crate) fn line(&mut self, line: u32, cap: u32) -> Vec<Option<usize>> {
        let idx = line as usize;
        if self
            .next
            .is_none_or(|next| next > idx || idx - next > SCAN_LINES)
        {
            self.next = Some(idx.saturating_sub(SCAN_LINES));
            self.stack.clear();
        }
        let mut next = self.next.unwrap_or(idx);
        while next < idx {
            let slots = brackets_in_prefix(self.buffer, self.highlights, next, u32::MAX);
            nest(&mut self.stack, slots);
            next += 1;
        }
        self.next = Some(idx);
        let slots = brackets_in_prefix(self.buffer, self.highlights, idx, cap);
        nest(&mut self.stack.clone(), slots)
    }
}

/// Advance `stack` over one line's bracket `slots`, returning each char's
/// depth.
fn nest(stack: &mut Vec<usize>, slots: Vec<Option<Bracket>>) -> Vec<Option<usize>> {
    slots
        .into_iter()
        .map(|slot| {
            let bracket = slot?;
            if bracket.open {
                stack.push(bracket.kind);
                return Some(stack.len() - 1);
            }
            let depth = stack.iter().rposition(|&kind| kind == bracket.kind)?;
            stack.truncate(depth);
            Some(depth)
        })
        .collect()
}

/// The pair a bracket command acts on at `pos`: the one touching it, else the
/// one enclosing it.
#[must_use]
pub fn bracket_pair_at(
    buffer: &TextBuffer,
    highlights: Option<&Highlights>,
    pos: LineCol,
) -> Option<BracketPair> {
    matching_pair(buffer, highlights, pos).or_else(|| enclosing_pair(buffer, highlights, pos))
}

/// [`ThemeRole::MatchingBracket`] text-background decorations over both
/// brackets of the pair touching each caret, for the editor to paint alongside
/// the application's own decorations.
#[must_use]
pub fn bracket_decorations(
    buffer: &TextBuffer,
    highlights: Option<&Highlights>,
    cursors: &CursorState,
) -> Vec<Decoration> {
    let mut out = Vec::new();
    for selection in &cursors.selections {
        let Some(pair) = matching_pair(buffer, highlights, selection.head) else {
            continue;
        };
        for at in [pair.open, pair.close] {
            let deco = Decoration {
                range: Range {
                    start: at,
                    end: LineCol::new(at.line, at.col + 1),
                },
                kind: DecorationKind::TextBackground,
                role: Some(ThemeRole::MatchingBracket),
            };
            if !out.contains(&deco) {
                out.push(deco);
            }
        }
    }
    out
}
