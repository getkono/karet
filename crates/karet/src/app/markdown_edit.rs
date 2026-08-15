//! Markdown editing commands: emphasis toggles, task checkboxes, and
//! list-aware Enter (continuation, renumbering, end-on-empty).
//!
//! The pure logic lives in `karet_markdown::edit`; this module gates it to
//! Markdown tabs, builds the buffer edits, and repositions the caret.

use karet_core::LineCol;
use karet_core::Range;
use karet_editor::editing;

use super::*;

impl App {
    /// Whether the active tab edits a Markdown file.
    fn active_is_markdown(&self) -> bool {
        self.tabs.get(self.active).is_some_and(|tab| {
            matches!(
                &tab.kind,
                TabKind::Code { path, .. }
                    if karet_filetype::file_type_for_path(path).name() == "Markdown"
            )
        })
    }

    /// Toggle `marker` (e.g. `**`) around the primary selection or the word
    /// under the caret. `fallback` runs instead on a non-Markdown tab, so a
    /// chord like Ctrl+B keeps its global meaning outside Markdown.
    pub(super) fn toggle_markdown_surround(&mut self, marker: &str, fallback: Option<Command>) {
        if !self.active_is_markdown() {
            match fallback {
                Some(command) => self.dispatch(command),
                None => {
                    self.status = Some("markdown formatting applies to Markdown files".to_owned());
                },
            }
            return;
        }
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let TabKind::Code { buffer, .. } = &tab.kind else {
            return;
        };
        let caret = tab.editor.cursor();
        let selection = tab.editor.selection_range().filter(|r| !r.is_empty());
        if selection.is_some_and(|r| r.start.line != r.end.line) {
            self.status = Some("select within one line to toggle formatting".to_owned());
            return;
        }
        let line = caret.line;
        let Some(text) = buffer.line(line as usize) else {
            return;
        };
        let (start, end) = selection.map_or((caret.col as usize, caret.col as usize), |r| {
            (r.start.col as usize, r.end.col as usize)
        });
        let Some(toggle) = karet_markdown::edit::toggle_surround(&text, start, end, marker) else {
            return;
        };
        let old_len = text.chars().count();
        let line_range = line_span(line, old_len);
        let (new_start, new_end) = (toggle.start, toggle.end);
        let new_text = toggle.text;
        self.submit_edit(move |c, _sel, _buf, base| {
            (c == caret)
                .then(|| editing::insert(line_range.start, Some(line_range), base, &new_text))
        });
        self.select_cols(line, new_start, new_end);
    }

    /// Toggle the `[ ]`/`[x]` checkbox on the caret's line (Alt+C).
    pub(super) fn toggle_task_checkbox(&mut self) {
        if !self.active_is_markdown() {
            self.status = Some("task checkboxes apply to Markdown files".to_owned());
            return;
        }
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let TabKind::Code { buffer, .. } = &tab.kind else {
            return;
        };
        let caret = tab.editor.cursor();
        let line = caret.line;
        let Some(text) = buffer.line(line as usize) else {
            return;
        };
        let Some(new_text) = karet_markdown::edit::toggle_task(&text) else {
            self.status = Some("no task checkbox on this line".to_owned());
            return;
        };
        let line_range = line_span(line, text.chars().count());
        self.submit_edit(move |c, _sel, _buf, base| {
            (c.line == line).then(|| {
                let mut edit = editing::insert(line_range.start, Some(line_range), base, &new_text);
                edit.caret = c; // the checkbox flip never moves the caret
                edit
            })
        });
    }

    /// List-aware Enter. `true` when this handled the key (a list item was
    /// continued, renumbered, or ended); `false` falls back to the ordinary
    /// newline.
    pub(super) fn markdown_insert_newline(&mut self) -> bool {
        if !self.active_is_markdown() || !self.settings.markdown.list_continuation {
            return false;
        }
        let Some(tab) = self.tabs.get(self.active) else {
            return false;
        };
        let TabKind::Code { buffer, .. } = &tab.kind else {
            return false;
        };
        // Multi-caret Enter keeps its ordinary meaning: per-caret list edits
        // would need per-caret renumbering of overlapping runs.
        if tab.editor.has_multiple_cursors() || tab.editor.selection_range().is_some() {
            return false;
        }
        let caret = tab.editor.cursor();
        let text = buffer.text();
        let line = caret.line as usize;
        match karet_markdown::edit::continue_list(&text, line, caret.col as usize) {
            None => false,
            Some(karet_markdown::edit::ListContinuation::EndList { marker_end }) => {
                let range = line_span_cols(caret.line, 0, marker_end);
                self.submit_edit_with_cause(
                    karet_core::EditCause::Newline,
                    move |c, _s, _b, base| {
                        (c == caret).then(|| editing::insert(range.start, Some(range), base, ""))
                    },
                );
                true
            },
            Some(karet_markdown::edit::ListContinuation::Continue { insert }) => {
                // Renumber against the projected text so the new item and every
                // following sibling agree; rewrites map back to base lines
                // (the insert only splits the caret's own line).
                let mut projected = String::new();
                for (i, l) in text.lines().enumerate() {
                    if i == line {
                        let split: Vec<char> = l.chars().collect();
                        let at = (caret.col as usize).min(split.len());
                        projected.extend(&split[..at]);
                        projected.push('\n');
                        projected.push_str(&insert);
                        projected.extend(&split[at..]);
                    } else {
                        projected.push_str(l);
                    }
                    projected.push('\n');
                }
                let mut inserted = insert.clone();
                let mut rewrites = Vec::new();
                for rw in karet_markdown::edit::renumber_ordered(&projected, line + 1) {
                    match rw.line.cmp(&(line + 1)) {
                        std::cmp::Ordering::Equal => inserted = rw.text,
                        std::cmp::Ordering::Greater => rewrites.push((rw.line - 1, rw.text)),
                        // Misnumbered lines above the caret renumber too; the
                        // insert below them leaves their base coordinates alone.
                        std::cmp::Ordering::Less => rewrites.push((rw.line, rw.text)),
                    }
                }
                // For the inserted line the rewrite carries the split-off tail;
                // the insert text is everything before the caret's old tail.
                let tail: String = text
                    .lines()
                    .nth(line)
                    .map(|l| l.chars().skip(caret.col as usize).collect())
                    .unwrap_or_default();
                if let Some(stripped) = inserted.strip_suffix(&tail) {
                    inserted = stripped.to_owned();
                }
                let insert_text = format!("\n{inserted}");
                let caret_after = LineCol::new(
                    caret.line + 1,
                    u32::try_from(inserted.chars().count()).unwrap_or(u32::MAX),
                );
                let line_lengths: Vec<(u32, usize)> = rewrites
                    .iter()
                    .map(|(l, _)| {
                        let len = text.lines().nth(*l).map_or(0, |t| t.chars().count());
                        (u32::try_from(*l).unwrap_or(u32::MAX), len)
                    })
                    .collect();
                self.submit_edit_with_cause(
                    karet_core::EditCause::Newline,
                    move |c, _s, _b, base| {
                        if c != caret {
                            return None;
                        }
                        let mut edit = editing::insert(caret, None, base, &insert_text);
                        for ((l, old_len), (_, new_text)) in line_lengths.iter().zip(&rewrites) {
                            let range = line_span_cols(*l, 0, *old_len);
                            edit.change.edits.push(karet_core::TextEdit {
                                range,
                                new_text: new_text.clone(),
                            });
                        }
                        edit.caret = caret_after;
                        Some(edit)
                    },
                );
                true
            },
        }
    }

    /// Replace the primary cursor with a `start..end` selection on `line`.
    fn select_cols(&mut self, line: u32, start: usize, end: usize) {
        if let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            editor.set_cursor_state(
                buffer,
                karet_core::CursorState {
                    selections: vec![karet_core::Selection {
                        anchor: LineCol::new(line, u32::try_from(start).unwrap_or(u32::MAX)),
                        head: LineCol::new(line, u32::try_from(end).unwrap_or(u32::MAX)),
                    }],
                    primary: 0,
                },
            );
        }
    }
}

/// The whole-line range `(line, 0)..(line, chars)`.
fn line_span(line: u32, chars: usize) -> Range {
    line_span_cols(line, 0, chars)
}

/// The range `(line, start)..(line, end)` in character columns.
fn line_span_cols(line: u32, start: usize, end: usize) -> Range {
    Range {
        start: LineCol::new(line, u32::try_from(start).unwrap_or(u32::MAX)),
        end: LineCol::new(line, u32::try_from(end).unwrap_or(u32::MAX)),
    }
}
