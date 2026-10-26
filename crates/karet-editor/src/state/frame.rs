//! The geometry one render laid out and painted, kept for everything that
//! runs between renders.

use std::ops::RangeInclusive;

use ratatui::layout::Rect;

use super::Fold;
use crate::LineNumbers;
use crate::WrappingIndent;
use crate::hint::BlockIndex;
use crate::hint::HintIndex;

/// What the last render laid out and painted: the viewport it measured, the
/// options it was given, the virtual text and lines it indexed, and where it
/// painted each clickable affordance.
///
/// A mouse click, a reveal and a page motion all resolve between renders,
/// and each must land against the frame actually on screen. So the render
/// builds one of these and hands it over whole through
/// [`EditorState::record_frame`](super::EditorState::record_frame): every
/// field describes the same frame, rather than whichever fields the render
/// had written when something read them.
#[derive(Clone, Debug)]
pub(crate) struct FrameGeometry {
    /// The rows left for the document below the sticky headers.
    pub(crate) height: u16,
    /// The content width after the gutter.
    pub(crate) content_width: u16,
    /// The gutter numbering, which sets the gutter's width for hit-testing
    /// and caret placement.
    pub(crate) line_numbers: LineNumbers,
    /// Whether the gutter reserved its change lane, widening it by one
    /// column.
    pub(crate) change_lane: bool,
    /// The keyboard reveal's vertical margin.
    pub(crate) scroll_off: u16,
    /// The buffer's last line, where a margin stops.
    pub(crate) last_line: u32,
    /// Whether the frame was soft wrapped.
    pub(crate) word_wrap: bool,
    /// The wrap column soft wrapping was capped at.
    pub(crate) wrap_column: Option<u32>,
    /// How soft-wrapped continuation rows were indented.
    pub(crate) wrapping_indent: WrappingIndent,
    /// The hard-tab width.
    pub(crate) tab_width: u16,
    /// Logical-line ranges exempted from soft wrapping.
    pub(crate) unwrapped_lines: Vec<RangeInclusive<u32>>,
    /// The virtual text painted, indexed for lookup. A click resolved
    /// against hints the frame did not paint would select the wrong
    /// character.
    pub(crate) hints: HintIndex,
    /// The virtual lines painted, for the same reason as
    /// [`hints`](Self::hints): the rows a click counts are the rows that
    /// frame painted.
    pub(crate) blocks: BlockIndex,
    /// The collapsed folds lines were hidden behind, so a reveal made
    /// between renders counts rows, not buffer lines.
    pub(crate) folds: Vec<Fold>,
    /// Source lines on the sticky-scroll rows, outermost first.
    pub(crate) sticky_rows: Vec<u32>,
    /// Rows reserved above the live document viewport.
    pub(crate) sticky_height: u16,
    /// Distinct buffer lines painted — see
    /// [`EditorState::visible_lines`](super::EditorState::visible_lines).
    pub(crate) visible_lines: u32,
    /// Where each virtual text was painted, keyed by its index in the slice
    /// given to `Editor::virtual_text`.
    pub(crate) virtual_text: Vec<(usize, Rect)>,
    /// The row each virtual line was painted on, keyed by its index in the
    /// slice given to `Editor::virtual_lines`.
    pub(crate) virtual_lines: Vec<(usize, Rect)>,
    /// Where each collapsed fold header's ` ⋯` was painted, with that
    /// header's line.
    pub(crate) fold_ellipses: Vec<(Rect, u32)>,
}

impl FrameGeometry {
    /// Forget where the frame painted its clickable affordances, for a
    /// render that painted nothing: a click must not hit text no longer on
    /// screen.
    pub(crate) fn forget_painted(&mut self) {
        self.virtual_text.clear();
        self.virtual_lines.clear();
        self.fold_ellipses.clear();
    }
}

impl Default for FrameGeometry {
    /// The geometry before any render: no viewport measured, no margin
    /// stop, default tab width.
    fn default() -> Self {
        Self {
            height: 0,
            content_width: 0,
            line_numbers: LineNumbers::Absolute,
            change_lane: false,
            scroll_off: 0,
            last_line: u32::MAX,
            word_wrap: false,
            wrap_column: None,
            wrapping_indent: WrappingIndent::None,
            tab_width: 4,
            unwrapped_lines: Vec::new(),
            hints: HintIndex::default(),
            blocks: BlockIndex::default(),
            folds: Vec::new(),
            sticky_rows: Vec::new(),
            sticky_height: 0,
            visible_lines: 0,
            virtual_text: Vec::new(),
            virtual_lines: Vec::new(),
            fold_ellipses: Vec::new(),
        }
    }
}
