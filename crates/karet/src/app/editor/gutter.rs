//! Clicks in the editor gutter, whose lanes are controls rather than text.
//!
//! The marker lane toggles a breakpoint, the change lane's marker opens the
//! file's changes against `HEAD`, the fold lane's chevron and a collapsed
//! fold's `⋯` toggle the fold, and the line numbers select whole lines: a
//! click selects one, a drag extends by lines, Shift+click extends from the
//! selection's anchored line. A row below the last line shows no line, so a
//! click there is an ordinary click below the text.

use karet_core::Selection;
use karet_editor::GutterLane;

use super::super::*;

/// What a press on the gutter (or a fold's `⋯`) does, resolved before any of
/// it is acted on.
enum GutterAction {
    /// Toggle the breakpoint on this line of this file.
    Breakpoint(PathBuf, u32),
    /// Collapse or expand the fold headed on this line.
    Fold(u32),
    /// Select whole lines from the first line to the second.
    Lines(u32, u32),
    /// Open the file's changes against `HEAD`, the comparison its change
    /// markers draw, landed on the hunk of this clicked line.
    Changes(u32),
}

impl App {
    /// Act on a left press at `mouse` in the active code tab's gutter, or on
    /// a collapsed fold's `⋯`, given the editor `area` of `pane`; `true` when
    /// the press is spent, `false` when it is an ordinary click in the text.
    ///
    /// The fold lane of a line that heads no fold, the change lane of a line
    /// with no change marker, a modified line-number
    /// click (Ctrl keeps go-to-definition's "nothing to resolve", Alt its
    /// caret gestures) and any row below the last line fall through to the
    /// text click. Every row of a fold header's line, wrap continuations
    /// included, toggles its fold.
    pub(crate) fn gutter_click(&mut self, mouse: MouseEvent, area: Rect, pane: PaneId) -> bool {
        let Some(action) = self.gutter_action(mouse, area) else {
            return false;
        };
        self.editor_drag = None;
        match action {
            GutterAction::Breakpoint(path, line) => self.debug_toggle_breakpoint_at(path, line),
            GutterAction::Changes(line) => self.open_changes_landing("HEAD", "HEAD", Some(line)),
            GutterAction::Fold(line) => {
                // A fold toggle is not a first click: a quick press on what
                // the fold just revealed starts its own streak.
                self.last_click = None;
                self.toggle_fold_at(line);
            },
            GutterAction::Lines(from, to) => {
                let Some(Tab {
                    kind: TabKind::Code { buffer, .. },
                    editor,
                    view,
                    ..
                }) = self.tabs.get_mut(self.active)
                else {
                    return true;
                };
                editor.select_lines(buffer, from, to);
                // The press arms a line drag anchored on the `from` line, so
                // dragging grows the selection a whole line at a time.
                self.editor_drag = Some(EditorDrag {
                    anchor: line_span(buffer, from),
                    granularity: DragGranularity::Line,
                    pointer: (mouse.column, mouse.row),
                    area,
                    pane,
                    view: *view,
                    last_step: None,
                });
            },
        }
        true
    }

    /// What a press at `mouse` on the active code tab's gutter or `⋯` does,
    /// or `None` for a press the text should handle.
    fn gutter_action(&self, mouse: MouseEvent, area: Rect) -> Option<GutterAction> {
        let tab = self.tabs.get(self.active)?;
        let TabKind::Code {
            path,
            buffer,
            folds,
            folded,
            doc,
            ..
        } = &tab.kind
        else {
            return None;
        };
        if let Some(line) = tab.editor.fold_ellipsis_at(mouse.column, mouse.row) {
            return Some(GutterAction::Fold(line));
        }
        let fold_lines = resolve_folds(folds, folded);
        let hit = tab
            .editor
            .gutter_hit_at(area, buffer, &fold_lines, mouse.column, mouse.row)?;
        match hit.lane {
            GutterLane::Marker => Some(GutterAction::Breakpoint(path.clone(), hit.line)),
            GutterLane::Fold => fold_lines
                .iter()
                .any(|fold| fold.start == hit.line)
                .then_some(GutterAction::Fold(hit.line)),
            GutterLane::Change => self
                .change_marked(*doc, hit.line)
                .then_some(GutterAction::Changes(hit.line)),
            GutterLane::LineNumber => {
                let modifiers = mouse.modifiers;
                if modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER | KeyModifiers::ALT)
                {
                    return None;
                }
                let from = if modifiers.contains(KeyModifiers::SHIFT) {
                    anchored_line(tab.editor.cursors().primary())
                } else {
                    hit.line
                };
                Some(GutterAction::Lines(from, hit.line))
            },
            // A lane the widget adds later does nothing here until it is wired.
            _ => None,
        }
    }

    /// Whether document `doc` holds a change marker on `line`. Markers are
    /// only held while `git.decorations` is on.
    fn change_marked(&self, doc: Option<DocumentId>, line: u32) -> bool {
        doc.and_then(|doc| self.docs.line_changes.get(&doc))
            .is_some_and(|markers| {
                markers.iter().any(|marker| {
                    matches!(marker.kind, DecorationKind::ChangeMarker { .. })
                        && (marker.range.start.line..=marker.range.end.line).contains(&line)
                })
            })
    }

    /// Collapse or expand the fold whose header is line `start` of the active
    /// code tab. Collapsing it over the primary caret moves that caret onto
    /// the header, which stays visible, so the caret is never hidden.
    pub(crate) fn toggle_fold_at(&mut self, start: u32) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let TabKind::Code {
            buffer,
            folds,
            folded,
            ..
        } = &mut tab.kind
        else {
            return;
        };
        let Some(region) = folds.regions().iter().find(|r| r.start == start).copied() else {
            return;
        };
        // `remove` returns whether it was collapsed: toggle by remove-or-insert.
        if folded.remove(&start) {
            return;
        }
        folded.insert(start);
        let caret = tab.editor.cursor();
        if start < caret.line && caret.line <= region.end {
            tab.editor.set_caret(buffer, LineCol::new(start, caret.col));
        }
    }
}

/// The line a whole-line extension keeps from `selection`: its anchor's line,
/// except that a selection made upward over whole lines anchors just past the
/// break of the line it began on, at column 0 of the next.
fn anchored_line(selection: Selection) -> u32 {
    let anchor = selection.anchor;
    if selection.head < anchor && anchor.col == 0 && anchor.line > 0 {
        anchor.line - 1
    } else {
        anchor.line
    }
}
