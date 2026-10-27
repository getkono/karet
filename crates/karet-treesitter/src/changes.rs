//! What an incremental reparse changed: the byte ranges of the new text whose
//! syntax may differ from the tree before the edits, so a consumer can redo
//! its query work over those ranges alone.

use std::collections::HashMap;

use karet_core::AppliedEdit;
use karet_core::BytePos;
use karet_core::Span;
use karet_core::shift_span;

use crate::LanguageId;
use crate::SyntaxTree;

/// Past this many edits in one reparse, tracking each edited range costs more
/// than it saves (every edit shifts every range before it), so the whole
/// document is reported changed instead.
const MAX_TRACKED_EDITS: usize = 256;

/// The ranges `edits`, applied in order, wrote, in the frame after the last.
///
/// `None` when there are too many to track (see [`MAX_TRACKED_EDITS`]).
pub(crate) fn edited_ranges(edits: &[AppliedEdit]) -> Option<Vec<Span>> {
    if edits.len() > MAX_TRACKED_EDITS {
        return None;
    }
    let mut out: Vec<Span> = Vec::with_capacity(edits.len());
    for edit in edits {
        for span in &mut out {
            *span = shift_span(*span, edit);
        }
        out.push(Span {
            start: BytePos(edit.start_byte),
            end: BytePos(edit.new_end_byte),
        });
    }
    Some(out)
}

/// Whether `a` and `b` share a byte, or touch: an insertion at a range's edge
/// may have changed what the range parses as.
fn touches(a: Span, b: Span) -> bool {
    a.start.0 <= b.end.0 && b.start.0 <= a.end.0
}

/// The identity of an injected layer: its language and the byte ranges it parsed.
type LayerKey = (LanguageId, Vec<(usize, usize)>);

fn key_of(layer: &SyntaxTree) -> LayerKey {
    let ranges = layer
        .tree
        .included_ranges()
        .iter()
        .map(|r| (r.start_byte, r.end_byte))
        .collect();
    (layer.language(), ranges)
}

/// The spans of injected layers that differ between `old` and `new`.
///
/// `old` must already carry `edits` (see [`SyntaxTree::edit`]), so its ranges are
/// in the new text's frame. A layer is unchanged when one in `old` has its
/// language and its exact ranges and no edit touched those ranges: the parser
/// read only those bytes, so it built the same tree. Every other layer, new or
/// gone, reports its whole span.
pub(crate) fn layer_changes(old: &[SyntaxTree], new: &[SyntaxTree], edited: &[Span]) -> Vec<Span> {
    let mut unmatched: HashMap<LayerKey, Vec<usize>> = HashMap::new();
    for (index, layer) in old.iter().enumerate() {
        unmatched.entry(key_of(layer)).or_default().push(index);
    }
    let mut out = Vec::new();
    for layer in new {
        let key = key_of(layer);
        let untouched = key.1.iter().all(|&(start, end)| {
            let range = Span {
                start: BytePos(start),
                end: BytePos(end),
            };
            !edited.iter().any(|e| touches(*e, range))
        });
        let twin = untouched
            .then(|| unmatched.get_mut(&key).and_then(Vec::pop))
            .flatten();
        if twin.is_none() {
            out.push(layer.span());
        }
    }
    out.extend(
        unmatched
            .into_values()
            .flatten()
            .filter_map(|index| old.get(index).map(SyntaxTree::span)),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::point_of;

    fn span(start: usize, end: usize) -> Span {
        Span {
            start: BytePos(start),
            end: BytePos(end),
        }
    }

    fn edit(start: usize, old_end: usize, new_end: usize) -> AppliedEdit {
        AppliedEdit {
            start_byte: start,
            old_end_byte: old_end,
            new_end_byte: new_end,
            ..AppliedEdit::default()
        }
    }

    #[test]
    fn edited_ranges_follow_later_edits() {
        // Insert 3 bytes at 10, then 2 at 0: the first range moves right by 2.
        let ranges = edited_ranges(&[edit(10, 10, 13), edit(0, 0, 2)]);
        assert_eq!(ranges, Some(vec![span(12, 15), span(0, 2)]));
        // A later deletion through an earlier insertion keeps covering the seam.
        let ranges = edited_ranges(&[edit(10, 10, 13), edit(11, 20, 11)]);
        assert_eq!(ranges, Some(vec![span(10, 11), span(11, 11)]));
    }

    #[test]
    fn too_many_edits_are_not_tracked() {
        let edits = vec![edit(0, 0, 1); MAX_TRACKED_EDITS + 1];
        assert_eq!(edited_ranges(&edits), None);
        assert!(edited_ranges(&edits[..MAX_TRACKED_EDITS]).is_some());
    }

    /// Replace `old[start..end]` with `with`, returning the new text and its edit.
    fn replace(old: &str, start: usize, end: usize, with: &str) -> (String, AppliedEdit) {
        let new = format!(
            "{}{with}{}",
            old.get(..start).unwrap_or(""),
            old.get(end..).unwrap_or("")
        );
        let new_end = start + with.len();
        let edit = AppliedEdit {
            start_byte: start,
            old_end_byte: end,
            new_end_byte: new_end,
            start_point: point_of(old, start),
            old_end_point: point_of(old, end),
            new_end_point: point_of(&new, new_end),
            replaced: old.get(start..end).unwrap_or("").to_owned(),
        };
        (new, edit)
    }

    #[test]
    fn a_renamed_identifier_reports_the_range_it_wrote() -> Result<(), crate::TsError> {
        let Some(rust) = crate::language_id_from_injection_name("rust") else {
            return Ok(()); // grammar not compiled in
        };
        let old = "fn main() {\n    let value = 1;\n}\n\nfn other() {}\n";
        let at = old.find("value").unwrap_or_default();
        let (new, edit) = replace(old, at, at + 5, "total");
        let mut parser = crate::LayeredParser::new();
        let mut tree = parser.parse(rust, old)?;
        let changed = parser.reparse_changed(&mut tree, &[edit], &new)?;
        assert!(
            changed.iter().any(|s| s.start.0 <= at && at + 5 <= s.end.0),
            "the rename must be reported, got {changed:?}"
        );
        let other = new.find("fn other").unwrap_or_default();
        assert!(
            changed.iter().all(|s| s.end.0 <= other),
            "an untouched function must not be, got {changed:?}"
        );
        Ok(())
    }

    #[test]
    fn an_edit_in_one_fence_leaves_the_other_fence_unreported() -> Result<(), crate::TsError> {
        let (Some(md), Some(_)) = (
            crate::language_id_from_injection_name("markdown"),
            crate::language_id_from_injection_name("rust"),
        ) else {
            return Ok(()); // grammars not compiled in
        };
        let old = "```rust\nfn a() {}\n```\n\ntext\n\n```rust\nfn b() {}\n```\n";
        let at = old.find("a()").unwrap_or_default();
        let (new, edit) = replace(old, at, at + 1, "alpha");
        let mut parser = crate::LayeredParser::new();
        let mut tree = parser.parse(md, old)?;
        let changed = parser.reparse_changed(&mut tree, &[edit], &new)?;
        assert!(changed.iter().any(|s| s.start.0 <= at && at < s.end.0));
        let second = new.find("fn b").unwrap_or_default();
        assert!(
            changed.iter().all(|s| s.end.0 < second),
            "the second fence's layer is unchanged, got {changed:?}"
        );
        Ok(())
    }

    #[test]
    fn a_newly_typed_fence_reports_its_layer() -> Result<(), crate::TsError> {
        let (Some(md), Some(_)) = (
            crate::language_id_from_injection_name("markdown"),
            crate::language_id_from_injection_name("rust"),
        ) else {
            return Ok(()); // grammars not compiled in
        };
        let old = "text\n";
        let (new, edit) = replace(old, old.len(), old.len(), "\n```rust\nfn f() {}\n```\n");
        let mut parser = crate::LayeredParser::new();
        let mut tree = parser.parse(md, old)?;
        let changed = parser.reparse_changed(&mut tree, &[edit], &new)?;
        let body = new.find("fn f").unwrap_or_default();
        assert!(changed.iter().any(|s| s.start.0 <= body && body < s.end.0));
        Ok(())
    }

    #[test]
    fn untracked_edits_report_the_whole_document() -> Result<(), crate::TsError> {
        let Some(rust) = crate::language_id_from_injection_name("rust") else {
            return Ok(()); // grammar not compiled in
        };
        let mut text = "fn f() {}\n".to_owned();
        let mut parser = crate::LayeredParser::new();
        let mut tree = parser.parse(rust, &text)?;
        let mut edits = Vec::new();
        for _ in 0..=MAX_TRACKED_EDITS {
            let (next, edit) = replace(&text, 0, 0, " ");
            text = next;
            edits.push(edit);
        }
        let changed = parser.reparse_changed(&mut tree, &edits, &text)?;
        assert_eq!(changed, vec![span(0, text.len())]);
        Ok(())
    }

    #[test]
    fn captures_in_keeps_matches_that_reach_outside_the_range() -> Result<(), crate::TsError> {
        let Some(rust) = crate::language_id_from_injection_name("rust") else {
            return Ok(()); // grammar not compiled in
        };
        let text = "fn a() {}\nconst S: &str = \"one\ntwo\";\nfn b() {}\n";
        let tree = crate::SyntaxTree::parse(&mut crate::ParserPool::new(), rust, text)?;
        let source = crate::highlights_query(rust).ok_or(crate::TsError::UnknownLanguage)?;
        let query = crate::Query::compile(rust, source)?;
        let inside = text.find("two").unwrap_or_default();
        let range = span(inside, inside + 1);
        let found = tree.captures_in(&query, text, range);
        let string = text.find('"').unwrap_or_default();
        assert!(
            found.iter().any(|c| c.span.start.0 == string),
            "the string holding the range is reported whole"
        );
        assert!(found.iter().all(|c| touches(c.span, range)));
        assert!(found.len() < tree.captures(&query, text).len());
        Ok(())
    }
}
