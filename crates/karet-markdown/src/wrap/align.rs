//! Aligning the lines of a [`Block::Aligned`] within the width.

use super::WrappedLine;
use super::prefix_width;
use super::prefixed_line;
use super::space;
use super::wrap_block;
use crate::Alignment;
use crate::Block;
use crate::ImageSizer;
use crate::TextSpan;

/// Wrap `blocks` as [`super::wrap_blocks`] does, then shift each line right so its
/// content sits centered or right-aligned within the width left after `prefix`.
///
/// A code block and a table keep their own layout — their columns line up only as
/// written — and a nested aligned block has already placed its own lines.
pub(super) fn wrap_aligned(
    align: Alignment,
    blocks: &[Block],
    width: usize,
    prefix: &[TextSpan],
    sizer: &dyn ImageSizer,
    out: &mut Vec<WrappedLine>,
) {
    let indent = prefix_width(prefix);
    let inner = width.saturating_sub(indent).max(1);
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 && !matches!(block, Block::List { .. }) {
            out.push(prefixed_line(prefix, Vec::new()));
        }
        let first = out.len();
        wrap_block(block, width, prefix, sizer, out);
        if matches!(
            block,
            Block::CodeBlock { .. } | Block::Table { .. } | Block::Aligned { .. }
        ) {
            continue;
        }
        for line in out.iter_mut().skip(first) {
            align_line(line, align, prefix.len(), indent, inner);
        }
    }
}

/// Pad `line` after its first `prefix_spans` spans (its gutter or indent, `indent`
/// columns wide) so its content is aligned within `inner` columns. An empty line, or
/// one already as wide as the space, is left alone.
fn align_line(
    line: &mut WrappedLine,
    align: Alignment,
    prefix_spans: usize,
    indent: usize,
    inner: usize,
) {
    // An image row's content is the image: move its column, not its (empty) text.
    if let Some(image) = &mut line.image {
        let pad = offset(align, inner.saturating_sub(usize::from(image.cols)));
        image.col = image
            .col
            .saturating_add(u16::try_from(pad).unwrap_or(u16::MAX));
        return;
    }
    let content = line.width().saturating_sub(indent);
    if content == 0 {
        return;
    }
    let pad = offset(align, inner.saturating_sub(content));
    if pad == 0 {
        return;
    }
    let at = prefix_spans.min(line.spans.len());
    line.spans.insert(at, space(pad));
}

/// How far content `extra` columns narrower than its space moves right under `align`.
pub(super) fn offset(align: Alignment, extra: usize) -> usize {
    match align {
        Alignment::Center => extra / 2,
        Alignment::Right => extra,
        Alignment::None | Alignment::Left => 0,
    }
}
