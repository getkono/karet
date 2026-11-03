//! Who owns a byte, and how many lines of code each callable owns.
//!
//! Callables nest — a Rust `fn` inside a `fn`, a Python `def` inside a `def` — so every
//! position belongs to the *innermost* callable around it. That one rule gives both
//! answers this module exists for: which function a call site belongs to, and which
//! function a line of code counts towards. A nested function's lines are its own and
//! not its parent's, so summing every function's lines never counts one twice.
//!
//! A *code line* is a line with at least one byte that is neither whitespace nor inside
//! a comment, owned by the callable around that first byte. Blank lines, comment-only
//! lines and documentation strings count for nobody; a line with code and a trailing
//! comment counts once.

use karet_core::Span;

/// Attribute each position to the innermost span containing it.
///
/// `spans` must nest properly — any two either nest or are disjoint — which is what a
/// syntax tree's nodes do. Returns, per position and in the order given, the index into
/// `spans` of its innermost container, or `None` when no span holds it.
#[must_use]
pub fn innermost(spans: &[Span], positions: &[usize]) -> Vec<Option<usize>> {
    // Outer before inner where two start together, so the inner one ends up on top.
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|&i| {
        spans
            .get(i)
            .map_or((usize::MAX, 0), |s| (s.start.0, usize::MAX - s.end.0))
    });
    let mut queries: Vec<(usize, usize)> = positions
        .iter()
        .copied()
        .enumerate()
        .map(|(slot, at)| (at, slot))
        .collect();
    queries.sort_unstable();

    let mut out = vec![None; positions.len()];
    let mut stack: Vec<usize> = Vec::new();
    let mut next = 0usize;
    for (at, slot) in queries {
        while let Some(&i) = order.get(next)
            && spans.get(i).is_some_and(|s| s.start.0 <= at)
        {
            // Anything that ended before this one starts cannot hold it, or anything later.
            while stack
                .last()
                .and_then(|top| spans.get(*top))
                .is_some_and(|top| top.end.0 <= spans.get(i).map_or(0, |s| s.start.0))
            {
                stack.pop();
            }
            stack.push(i);
            next += 1;
        }
        while stack
            .last()
            .and_then(|top| spans.get(*top))
            .is_some_and(|top| top.end.0 <= at)
        {
            stack.pop();
        }
        if let Some(cell) = out.get_mut(slot) {
            *cell = stack.last().copied();
        }
    }
    out
}

/// The byte offset of the first code byte on every line that has one.
///
/// `comments` may overlap and come in any order.
#[must_use]
pub fn code_line_starts(text: &str, comments: &[Span]) -> Vec<usize> {
    let mut merged: Vec<(usize, usize)> = comments
        .iter()
        .map(|span| (span.start.0, span.end.0))
        .filter(|(start, end)| start < end)
        .collect();
    merged.sort_unstable();
    let mut joined: Vec<(usize, usize)> = Vec::with_capacity(merged.len());
    for (start, end) in merged {
        match joined.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => joined.push((start, end)),
        }
    }

    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut comment = joined.iter().peekable();
    let mut seen_code = false;
    let mut at = 0usize;
    while let Some(&byte) = bytes.get(at) {
        while comment.peek().is_some_and(|(_, end)| *end <= at) {
            comment.next();
        }
        if let Some(&&(start, end)) = comment.peek()
            && start <= at
        {
            // A comment can span lines; each newline inside it still ends a line.
            if bytes
                .get(at..end.min(bytes.len()))
                .is_some_and(|inside| inside.contains(&b'\n'))
            {
                seen_code = false;
            }
            at = end;
            continue;
        }
        if byte == b'\n' {
            seen_code = false;
        } else if !seen_code && !byte.is_ascii_whitespace() {
            seen_code = true;
            out.push(at);
        }
        at += 1;
    }
    out
}

/// How many code lines each of `callables` owns, by innermost containment.
///
/// A line whose first code byte sits outside every callable still counts for a callable
/// that *starts* on it — `mod a { fn f() { 1 } }` is one line of `f`, not a line of
/// nobody's. Of several starting there, the first wins.
#[must_use]
pub fn owned_code_lines(text: &str, comments: &[Span], callables: &[Span]) -> Vec<u32> {
    let starts = code_line_starts(text, comments);
    let mut owners = innermost(callables, &starts);

    let mut opening: Vec<usize> = callables.iter().map(|span| span.start.0).collect();
    opening.sort_unstable();
    let orphans: Vec<(usize, usize)> = owners
        .iter()
        .zip(&starts)
        .enumerate()
        .filter(|(_, (owner, _))| owner.is_none())
        .filter_map(|(slot, (_, start))| {
            let line_end = text
                .get(*start..)
                .and_then(|rest| rest.find('\n'))
                .map_or(text.len(), |at| start + at);
            let first = opening.partition_point(|open| open < start);
            let open = *opening.get(first)?;
            (open < line_end).then_some((slot, open))
        })
        .collect();
    let adopted = innermost(
        callables,
        &orphans.iter().map(|(_, open)| *open).collect::<Vec<_>>(),
    );
    for ((slot, _), owner) in orphans.iter().zip(adopted) {
        if let Some(cell) = owners.get_mut(*slot) {
            *cell = owner;
        }
    }

    let mut counts = vec![0u32; callables.len()];
    for owner in owners.into_iter().flatten() {
        if let Some(count) = counts.get_mut(owner) {
            *count = count.saturating_add(1);
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use karet_core::BytePos;

    use super::*;

    fn span(start: usize, end: usize) -> Span {
        Span {
            start: BytePos(start),
            end: BytePos(end),
        }
    }

    /// The span of the first occurrence of `needle` through the end of `until`.
    fn find(text: &str, needle: &str, until: &str) -> Span {
        let start = text.find(needle).unwrap_or(0);
        let end = text
            .get(start..)
            .and_then(|rest| rest.find(until))
            .map_or(text.len(), |at| start + at + until.len());
        span(start, end)
    }

    #[test]
    fn the_innermost_span_wins_and_outside_is_nobody() {
        let spans = [span(0, 100), span(10, 20), span(30, 90), span(40, 50)];
        let owners = innermost(&spans, &[5, 15, 25, 45, 60, 100, 0, 99]);
        assert_eq!(
            owners,
            [
                Some(0),
                Some(1),
                Some(0),
                Some(3),
                Some(2),
                None,
                Some(0),
                Some(0)
            ]
        );
    }

    #[test]
    fn positions_come_back_in_the_order_asked() {
        let spans = [span(0, 10), span(20, 30)];
        assert_eq!(innermost(&spans, &[25, 5, 15]), [Some(1), Some(0), None]);
    }

    #[test]
    fn blank_and_comment_only_lines_are_not_code() {
        let text = "fn f() {\n\n    // note\n    let x = 1; // trailing\n    /* a\n       b */\n    x\n}\n";
        let comments: Vec<Span> = ["// note", "// trailing", "/* a\n       b */"]
            .iter()
            .map(|c| find(text, c, c))
            .collect();
        let starts = code_line_starts(text, &comments);
        // `fn f() {`, `let x = 1;`, `x`, `}` — four lines.
        assert_eq!(starts.len(), 4);
    }

    #[test]
    fn code_after_a_closing_block_comment_counts_its_line() {
        let text = "/* a\n b */ x\ny";
        let comments = [find(text, "/*", "*/")];
        assert_eq!(code_line_starts(text, &comments).len(), 2);
    }

    #[test]
    fn a_nested_function_owns_its_own_lines() {
        let text = "fn outer() {\n    fn inner() {\n        1\n    }\n    inner()\n}\n";
        let outer = find(text, "fn outer", "\n}");
        let inner = find(text, "fn inner", "    }");
        let counts = owned_code_lines(text, &[], &[outer, inner]);
        // outer: `fn outer() {`, `inner()`, `}`; inner: its three lines.
        assert_eq!(counts, [3, 3]);
    }

    #[test]
    fn a_closure_stays_with_the_function_around_it() {
        let text = "fn f() {\n    let g = |x| {\n        x + 1\n    };\n}\n";
        let f = find(text, "fn f", "\n}");
        assert_eq!(owned_code_lines(text, &[], &[f]), [5]);
    }

    #[test]
    fn a_one_line_item_inside_a_container_still_owns_its_line() {
        let text = "mod a { fn f() { 1 } fn g() {} }\nfn h() {}\n";
        let f = find(text, "fn f", "1 }");
        let g = find(text, "fn g", "{}");
        let h = find(text, "fn h", "{}");
        assert_eq!(owned_code_lines(text, &[], &[f, g, h]), [1, 0, 1]);
    }

    #[test]
    fn overlapping_comment_spans_merge() {
        let text = "a\n/* x */\nb";
        let comment = find(text, "/*", "*/");
        assert_eq!(code_line_starts(text, &[comment, comment]).len(), 2);
    }
}
