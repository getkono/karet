//! The snippet session an accepted snippet completion starts (issue #373).
//!
//! An accept inserts the snippet's text -- once per caret -- and the session
//! remembers where each of its stops landed, in document coordinates. Tab and
//! Shift+Tab select the next and previous stop at every copy and every mirror
//! at once, so typing edits them together through the ordinary multi-caret
//! path. Reaching the final stop (`$0`) ends the session; so does Esc, an
//! edit the session did not see (an undo, another view's), or Tab from a
//! caret that left the snippet.
//!
//! The session follows every edit the app applies: the stop being edited
//! grows with what is typed at either end of it -- and so does a stop
//! enclosing it, at an end the two share -- the others only move, and a
//! stop that an edit swallowed whole -- a nested placeholder typed over --
//! is gone.

use karet_core::CursorState;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::Selection;
use karet_core::Tabstop;
use karet_core::TextEdit;
use karet_editor::editing::caret_after_insert;
use karet_editor::editing::map_position;
use karet_editor::editing::reflow_caret;
use karet_session::ViewId;

/// A live snippet session in one view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SnippetSession {
    /// The view the snippet was accepted in.
    pub view: ViewId,
    /// The tab version the stops describe; an edit from any other base is
    /// one the session did not follow, and ends it.
    pub version: u64,
    /// Each stop's ranges, in visiting order, the final stop last.
    stops: Vec<Vec<Range>>,
    /// The stop the carets are on.
    current: usize,
}

/// Where the character at `offset` into `text`, inserted at `start`, lands.
fn offset_at(start: LineCol, text: &str, offset: u32) -> LineCol {
    let prefix: String = text.chars().take(offset as usize).collect();
    caret_after_insert(start, &prefix)
}

/// `pos` through `edits` (sorted, in the coordinates before them), staying
/// before text inserted exactly at it -- [`map_position`] moves past it.
fn map_before(pos: LineCol, edits: &[TextEdit]) -> LineCol {
    let before = edits
        .iter()
        .take_while(|e| e.range.end < pos || (e.range.end == pos && !e.range.is_empty()))
        .count();
    match edits.get(before) {
        Some(e) if e.range.start < pos => {
            let landed = caret_after_insert(e.range.start, &e.new_text);
            reflow_caret(landed, &edits[..before])
        },
        _ => reflow_caret(pos, &edits[..before]),
    }
}

/// Which ends of a stop's range take text inserted exactly at them.
#[derive(Clone, Copy)]
struct Grow {
    /// Text inserted at the range's start joins it.
    start: bool,
    /// Text inserted at the range's end joins it.
    end: bool,
}

impl Grow {
    /// How `range` grows while the carets are on the stop whose ranges are
    /// `active`: at both ends when it is one of them (`own`), else at each
    /// end it shares with an active range it encloses -- `${1:foo ${2:bar}}`
    /// typed at the end of `bar` extends stop 1 too -- else at neither.
    fn of(range: Range, own: bool, active: &[Range]) -> Self {
        if own {
            return Self {
                start: true,
                end: true,
            };
        }
        let enclosed = || {
            active
                .iter()
                .filter(move |a| range.start <= a.start && a.end <= range.end)
        };
        Self {
            start: enclosed().any(|a| a.start == range.start),
            end: enclosed().any(|a| a.end == range.end),
        }
    }
}

/// `range` through `edits`: grown by text inserted at the ends `grow` names,
/// else only moved. `None` when an edit replaced a larger span holding it
/// whole -- one merely touching its edge (a final stop right after a
/// placeholder) keeps it -- unless it is the stop being edited (`own`).
fn map_range(range: Range, edits: &[TextEdit], own: bool, grow: Grow) -> Option<Range> {
    let swallowed = edits.iter().any(|e| {
        e.range != range
            && e.range.start <= range.start
            && range.end <= e.range.end
            && e.range.start < range.end
            && range.start < e.range.end
    });
    if swallowed && !own {
        return None;
    }
    let start = if grow.start {
        map_before(range.start, edits)
    } else {
        map_position(range.start, edits)
    };
    let end = if grow.end {
        map_position(range.end, edits)
    } else {
        map_before(range.end, edits)
    };
    Some(Range {
        start,
        end: end.max(start),
    })
}

impl SnippetSession {
    /// The session for `text`, inserted at each of `starts` (in the
    /// coordinates after the insert), whose stops are `tabstops`, in `view`
    /// at `version`. `None` when there is no stop to visit beyond the first:
    /// [`Self::first_cursors`] is then all the accept needs.
    pub(crate) fn new(
        view: ViewId,
        version: u64,
        starts: &[LineCol],
        text: &str,
        tabstops: &[Tabstop],
    ) -> Option<Self> {
        let stops: Vec<Vec<Range>> = tabstops
            .iter()
            .map(|stop| {
                starts
                    .iter()
                    .flat_map(|&start| {
                        stop.spans.iter().map(move |&(from, to)| Range {
                            start: offset_at(start, text, from),
                            end: offset_at(start, text, to),
                        })
                    })
                    .collect()
            })
            .filter(|ranges: &Vec<Range>| !ranges.is_empty())
            .collect();
        (!stops.is_empty()).then_some(Self {
            view,
            version,
            stops,
            current: 0,
        })
    }

    /// Whether the carets are on the final stop, where the session is over.
    pub(crate) fn finished(&self) -> bool {
        self.current + 1 >= self.stops.len()
    }

    /// The carets for the current stop: each of its ranges selected, the
    /// primary the one nearest `primary` (fewest lines away, then fewest
    /// columns), so it stays in the copy the primary caret was in.
    pub(crate) fn cursors(&self, primary: LineCol) -> CursorState {
        let ranges = self.stops.get(self.current).cloned().unwrap_or_default();
        let primary = ranges
            .iter()
            .enumerate()
            .min_by_key(|(_, range)| {
                (
                    range.start.line.abs_diff(primary.line),
                    range.start.col.abs_diff(primary.col),
                )
            })
            .map_or(0, |(index, _)| index);
        CursorState {
            selections: ranges
                .into_iter()
                .map(|range| Selection {
                    anchor: range.start,
                    head: range.end,
                })
                .collect(),
            primary,
        }
    }

    /// Move to the next stop (`forward`) or the previous one; returns whether
    /// the carets moved. Past the final stop there is nowhere to go, and
    /// before the first neither.
    pub(crate) fn step(&mut self, forward: bool) -> bool {
        let next = if forward {
            self.current + 1
        } else {
            self.current.wrapping_sub(1)
        };
        if next >= self.stops.len() {
            return false;
        }
        self.current = next;
        true
    }

    /// Whether `pos` is within the snippet: from its first stop to its last,
    /// ends included.
    pub(crate) fn holds(&self, pos: LineCol) -> bool {
        let ranges = || self.stops.iter().flatten();
        let first = ranges().map(|range| range.start).min();
        let last = ranges().map(|range| range.end).max();
        first
            .zip(last)
            .is_some_and(|(first, last)| first <= pos && pos <= last)
    }

    /// Follow a change from `base` to `version` made of `edits` (sorted, in
    /// the coordinates before it). Returns `false` when the session cannot
    /// follow it -- it was made from a version the session never saw, or it
    /// left no stop to visit -- and is over.
    pub(crate) fn carry(&mut self, (base, version): (u64, u64), edits: &[TextEdit]) -> bool {
        if base != self.version {
            return false;
        }
        let current = self.current;
        let active = self.stops.get(current).cloned().unwrap_or_default();
        let mut kept = Vec::with_capacity(self.stops.len());
        for (index, ranges) in std::mem::take(&mut self.stops).into_iter().enumerate() {
            let own = index == current;
            let ranges: Vec<Range> = ranges
                .into_iter()
                .filter_map(|range| map_range(range, edits, own, Grow::of(range, own, &active)))
                .collect();
            if ranges.is_empty() {
                if index < current {
                    self.current -= 1;
                }
            } else {
                kept.push(ranges);
            }
        }
        self.stops = kept;
        self.version = version;
        self.current < self.stops.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: u32, col: u32) -> LineCol {
        LineCol::new(line, col)
    }

    fn span(start: LineCol, end: LineCol) -> Range {
        Range { start, end }
    }

    fn stop(index: u32, spans: &[(u32, u32)]) -> Tabstop {
        Tabstop {
            index,
            spans: spans.to_vec(),
            choices: Vec::new(),
        }
    }

    fn edit(range: Range, text: &str) -> TextEdit {
        TextEdit {
            range,
            new_text: text.to_owned(),
        }
    }

    fn selected(session: &SnippetSession) -> Vec<Range> {
        session
            .cursors(at(0, 0))
            .selections
            .iter()
            .map(|s| s.range())
            .collect()
    }

    /// `x = $1; ${1:v}` then `$0`, inserted at (0, 4).
    fn mirrored() -> Option<SnippetSession> {
        SnippetSession::new(
            ViewId(1),
            5,
            &[at(0, 4)],
            "x = v; v",
            &[stop(1, &[(4, 5), (7, 8)]), stop(0, &[(8, 8)])],
        )
    }

    #[test]
    fn no_stops_is_no_session() {
        assert_eq!(
            SnippetSession::new(ViewId(1), 0, &[at(0, 0)], "a", &[]),
            None
        );
    }

    /// Falsified by a stop placed in buffer columns of the text rather than
    /// after the insert's start, or by a mirror left unselected.
    #[test]
    fn the_first_stop_selects_every_mirror() -> Result<(), String> {
        let session = mirrored().ok_or("a session")?;
        assert_eq!(
            selected(&session),
            vec![span(at(0, 8), at(0, 9)), span(at(0, 11), at(0, 12))]
        );
        assert!(!session.finished());
        Ok(())
    }

    /// Stops in a multi-line snippet land on the lines they were written on.
    #[test]
    fn offsets_cross_lines() -> Result<(), String> {
        let session = SnippetSession::new(
            ViewId(1),
            0,
            &[at(2, 4)],
            "if c {\n    b\n}",
            &[
                stop(1, &[(3, 4)]),
                stop(2, &[(11, 12)]),
                stop(0, &[(14, 14)]),
            ],
        )
        .ok_or("a session")?;
        let mut session = session;
        assert!(session.step(true));
        assert_eq!(selected(&session), vec![span(at(3, 4), at(3, 5))]);
        assert!(session.step(true));
        assert!(session.finished());
        assert_eq!(selected(&session), vec![span(at(4, 1), at(4, 1))]);
        assert!(!session.step(true), "nothing past the final stop");
        Ok(())
    }

    /// Every copy of a snippet accepted at several carets moves together.
    #[test]
    fn every_copy_is_one_more_set_of_ranges() -> Result<(), String> {
        let session = SnippetSession::new(
            ViewId(1),
            0,
            &[at(0, 0), at(1, 0)],
            "f(a)",
            &[stop(1, &[(2, 3)]), stop(0, &[(4, 4)])],
        )
        .ok_or("a session")?;
        let cursors = session.cursors(at(1, 4));
        assert_eq!(cursors.selections.len(), 2);
        assert_eq!(cursors.primary, 1, "the copy at the primary caret");
        Ok(())
    }

    /// Falsified by typing at the end of a placeholder falling outside it,
    /// or by the stops after it staying put.
    #[test]
    fn typing_grows_the_current_stop_and_moves_the_rest() -> Result<(), String> {
        let mut session = mirrored().ok_or("a session")?;
        // Both mirrors' `v` replaced by `val` at once.
        let edits = [
            edit(span(at(0, 8), at(0, 9)), "val"),
            edit(span(at(0, 11), at(0, 12)), "val"),
        ];
        assert!(session.carry((5, 6), &edits));
        assert_eq!(session.version, 6);
        assert_eq!(
            selected(&session),
            vec![span(at(0, 8), at(0, 11)), span(at(0, 13), at(0, 16))]
        );
        // Then `s` typed at the end of each.
        let edits = [
            edit(span(at(0, 11), at(0, 11)), "s"),
            edit(span(at(0, 16), at(0, 16)), "s"),
        ];
        assert!(session.carry((6, 7), &edits));
        assert_eq!(
            selected(&session),
            vec![span(at(0, 8), at(0, 12)), span(at(0, 14), at(0, 18))]
        );
        assert!(session.step(true));
        assert_eq!(selected(&session), vec![span(at(0, 18), at(0, 18))]);
        Ok(())
    }

    /// A stop the user is not on does not swallow what is typed next to it.
    #[test]
    fn an_adjacent_stop_does_not_grow() -> Result<(), String> {
        let mut session = SnippetSession::new(
            ViewId(1),
            0,
            &[at(0, 0)],
            "ab",
            &[stop(1, &[(0, 1)]), stop(2, &[(1, 2)]), stop(0, &[(2, 2)])],
        )
        .ok_or("a session")?;
        assert!(session.carry((0, 1), &[edit(span(at(0, 1), at(0, 1)), "x")]));
        assert_eq!(selected(&session), vec![span(at(0, 0), at(0, 2))]);
        assert!(session.step(true));
        assert_eq!(selected(&session), vec![span(at(0, 2), at(0, 3))]);
        Ok(())
    }

    /// Falsified by an enclosing stop left short of what is typed at the
    /// end (or start) it shares with the nested stop being edited, or by the
    /// nested stop growing when the enclosing one is being edited.
    #[test]
    fn an_enclosing_stop_grows_with_its_nested_stop() -> Result<(), String> {
        // `${1:foo ${2:bar}}` then `$0`, with the carets on stop 2.
        let mut session = SnippetSession::new(
            ViewId(1),
            0,
            &[at(0, 0)],
            "foo bar",
            &[stop(1, &[(0, 7)]), stop(2, &[(4, 7)]), stop(0, &[(7, 7)])],
        )
        .ok_or("a session")?;
        assert!(session.step(true));
        assert!(session.carry((0, 1), &[edit(span(at(0, 7), at(0, 7)), "s")]));
        assert_eq!(selected(&session), vec![span(at(0, 4), at(0, 8))]);
        assert!(session.step(false));
        assert_eq!(selected(&session), vec![span(at(0, 0), at(0, 8))]);
        // `${1:${2:bar} foo}` typed at the start of `bar` grows stop 1 too.
        let mut session = SnippetSession::new(
            ViewId(1),
            0,
            &[at(0, 0)],
            "bar foo",
            &[stop(1, &[(0, 7)]), stop(2, &[(0, 3)]), stop(0, &[(7, 7)])],
        )
        .ok_or("a session")?;
        assert!(session.step(true));
        assert!(session.carry((0, 1), &[edit(span(at(0, 0), at(0, 0)), "x")]));
        assert_eq!(selected(&session), vec![span(at(0, 0), at(0, 4))]);
        assert!(session.step(false));
        assert_eq!(selected(&session), vec![span(at(0, 0), at(0, 8))]);
        // On stop 1, typing at its end leaves the nested stop 2 alone.
        let mut session = SnippetSession::new(
            ViewId(1),
            0,
            &[at(0, 0)],
            "foo bar",
            &[stop(1, &[(0, 7)]), stop(2, &[(4, 7)]), stop(0, &[(7, 7)])],
        )
        .ok_or("a session")?;
        assert!(session.carry((0, 1), &[edit(span(at(0, 7), at(0, 7)), "s")]));
        assert_eq!(selected(&session), vec![span(at(0, 0), at(0, 8))]);
        assert!(session.step(true));
        assert_eq!(selected(&session), vec![span(at(0, 4), at(0, 7))]);
        Ok(())
    }

    /// Typing over a placeholder removes the stops nested inside it.
    #[test]
    fn a_nested_stop_typed_over_is_gone() -> Result<(), String> {
        let mut session = SnippetSession::new(
            ViewId(1),
            0,
            &[at(0, 0)],
            "foo bar",
            &[stop(1, &[(0, 7)]), stop(2, &[(4, 7)]), stop(0, &[(7, 7)])],
        )
        .ok_or("a session")?;
        assert!(session.carry((0, 1), &[edit(span(at(0, 0), at(0, 7)), "x")]));
        assert!(session.step(true));
        assert!(session.finished(), "stop 2 went with the text it was in");
        assert_eq!(selected(&session), vec![span(at(0, 1), at(0, 1))]);
        Ok(())
    }

    /// An edit from a version the session never saw (an undo) ends it.
    #[test]
    fn an_unseen_edit_ends_the_session() -> Result<(), String> {
        let mut session = mirrored().ok_or("a session")?;
        assert!(!session.carry((9, 10), &[]));
        Ok(())
    }

    #[test]
    fn stepping_back_stops_at_the_first_stop() -> Result<(), String> {
        let mut session = mirrored().ok_or("a session")?;
        assert!(!session.step(false));
        assert!(session.step(true));
        assert!(session.step(false));
        assert_eq!(selected(&session).len(), 2);
        Ok(())
    }

    #[test]
    fn the_snippet_holds_positions_from_its_first_stop_to_its_last() -> Result<(), String> {
        let session = mirrored().ok_or("a session")?;
        assert!(session.holds(at(0, 8)));
        assert!(session.holds(at(0, 10)), "between two stops");
        assert!(session.holds(at(0, 12)));
        assert!(!session.holds(at(0, 7)));
        assert!(!session.holds(at(0, 13)));
        assert!(!session.holds(at(1, 0)));
        Ok(())
    }
}
