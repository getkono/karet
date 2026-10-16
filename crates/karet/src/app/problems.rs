//! The Problems panel and the problem walk: every diagnostic the backend has
//! published (`DiagnosticsPublished`) for an open document, grouped by file and
//! filterable by severity and text; F8 / Shift+F8 step the caret through the
//! active file's problems; the status bar shows the error and warning counts.
//!
//! The panel is a derived view of [`DocState::diagnostics`]: it is rebuilt
//! whenever a document's diagnostic set is replaced or the document closes, so
//! it can never show a diagnostic the editor has already dropped.

use super::*;

/// The `source` the spell checker stamps on its diagnostics (mirrors
/// `karet_session`'s private `SPELL_SOURCE`). Misspellings have a panel of
/// their own, and counting them here would bury real warnings under prose.
const SPELL_SOURCE: &str = "karet-spell";

/// Whether a diagnostic belongs in the Problems panel, the status-bar counts,
/// and the F8 walk — one predicate, so the three always agree.
pub(crate) fn is_problem(diagnostic: &Diagnostic) -> bool {
    diagnostic.source.as_deref() != Some(SPELL_SOURCE)
}

/// Rank a severity most-severe first, for sorting and threshold filtering.
fn severity_rank(severity: Severity) -> u8 {
    match severity {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Information => 2,
        Severity::Hint => 3,
        // `Unknown`, and anything a newer peer adds, sorts last.
        _ => 4,
    }
}

/// Which severities the Problems panel lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SeverityFilter {
    /// Every problem.
    #[default]
    All,
    /// Errors and warnings.
    Warnings,
    /// Errors only.
    Errors,
}

impl SeverityFilter {
    /// Whether a diagnostic of `severity` passes this filter.
    pub(crate) fn admits(self, severity: Severity) -> bool {
        let rank = severity_rank(severity);
        match self {
            Self::All => true,
            Self::Warnings => rank <= 1,
            Self::Errors => rank == 0,
        }
    }

    /// The next filter in the cycle, narrowing then wrapping back to all.
    pub(crate) fn next(self) -> Self {
        match self {
            Self::All => Self::Warnings,
            Self::Warnings => Self::Errors,
            Self::Errors => Self::All,
        }
    }

    /// The toolbar label naming what is shown.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Warnings => "warnings+",
            Self::Errors => "errors",
        }
    }
}

/// One problem: a diagnostic and the file it was published for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProblemEntry {
    /// The document's path, as its tab holds it.
    pub(crate) path: PathBuf,
    /// The diagnostic itself.
    pub(crate) diagnostic: Diagnostic,
}

/// One row of the Problems list: a file heading, or one problem under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProblemRow {
    /// A file heading; `entry` is its first shown problem, `count` how many show.
    File {
        /// Index into [`ProblemsPanel::entries`] of the file's first shown problem.
        entry: usize,
        /// How many of the file's problems pass the filters.
        count: usize,
    },
    /// One problem; `entry` indexes [`ProblemsPanel::entries`].
    Item {
        /// Index into [`ProblemsPanel::entries`].
        entry: usize,
    },
}

impl ProblemRow {
    /// The problem this row jumps to when activated (a heading: its first).
    pub(crate) fn entry(self) -> usize {
        match self {
            Self::File { entry, .. } | Self::Item { entry } => entry,
        }
    }
}

/// The Problems panel state.
#[derive(Default)]
pub(crate) struct ProblemsPanel {
    /// Every problem, sorted by path, then position, then severity.
    pub(crate) entries: Vec<ProblemEntry>,
    /// The rendered rows: the entries passing both filters, grouped by file.
    pub(crate) rows: Vec<ProblemRow>,
    /// The cursor over `rows`.
    pub(crate) selection: ListSelection,
    /// The severity threshold.
    pub(crate) severity: SeverityFilter,
    /// Case-insensitive text every shown problem's message, source, code, or
    /// path contains.
    pub(crate) filter: String,
    /// Whether keys are being typed into `filter`.
    pub(crate) filter_input: bool,
}

impl ProblemsPanel {
    /// Replace every entry (sorted here), then rebuild the rows.
    pub(crate) fn set_entries(&mut self, mut entries: Vec<ProblemEntry>) {
        entries.sort_by(|a, b| {
            a.path
                .cmp(&b.path)
                .then(a.diagnostic.range.start.cmp(&b.diagnostic.range.start))
                .then(
                    severity_rank(a.diagnostic.severity).cmp(&severity_rank(b.diagnostic.severity)),
                )
        });
        self.entries = entries;
        self.rebuild_rows();
    }

    /// Whether `entry` passes the severity and text filters.
    fn shows(&self, entry: &ProblemEntry, needle: &str) -> bool {
        if !self.severity.admits(entry.diagnostic.severity) {
            return false;
        }
        if needle.is_empty() {
            return true;
        }
        let diagnostic = &entry.diagnostic;
        diagnostic.message.to_lowercase().contains(needle)
            || diagnostic
                .source
                .as_deref()
                .is_some_and(|source| source.to_lowercase().contains(needle))
            || diagnostic
                .code
                .as_deref()
                .is_some_and(|code| code.to_lowercase().contains(needle))
            || entry.path.to_string_lossy().to_lowercase().contains(needle)
    }

    /// Rebuild [`rows`](Self::rows) from the entries and filters, clamping the
    /// cursor so it stays on (or near) the row it was on.
    pub(crate) fn rebuild_rows(&mut self) {
        let needle = self.filter.to_lowercase();
        let shown: Vec<usize> = (0..self.entries.len())
            .filter(|&index| self.shows(&self.entries[index], &needle))
            .collect();
        self.rows.clear();
        let mut start = 0;
        while start < shown.len() {
            let path = &self.entries[shown[start]].path;
            let count = shown[start..]
                .iter()
                .take_while(|&&index| self.entries[index].path == *path)
                .count();
            self.rows.push(ProblemRow::File {
                entry: shown[start],
                count,
            });
            self.rows.extend(
                shown[start..start + count]
                    .iter()
                    .map(|&entry| ProblemRow::Item { entry }),
            );
            start += count;
        }
        let cursor = self.selection.cursor();
        self.selection = ListSelection::new(self.rows.len());
        self.selection
            .move_to(cursor.min(self.rows.len().saturating_sub(1)));
    }

    /// The problem the cursor row stands for.
    pub(crate) fn selected(&self) -> Option<&ProblemEntry> {
        self.rows
            .get(self.selection.cursor())
            .and_then(|row| self.entries.get(row.entry()))
    }

    /// How many errors and warnings there are, whatever the filters show.
    pub(crate) fn counts(&self) -> (usize, usize) {
        self.entries
            .iter()
            .fold((0, 0), |(errors, warnings), entry| {
                match entry.diagnostic.severity {
                    Severity::Error => (errors + 1, warnings),
                    Severity::Warning => (errors, warnings + 1),
                    _ => (errors, warnings),
                }
            })
    }
}

/// The problem `forward` of `caret` among `starts` (sorted ascending), wrapping
/// past either end; `None` only when there is none at all.
pub(crate) fn step_problem(starts: &[LineCol], caret: LineCol, forward: bool) -> Option<LineCol> {
    if forward {
        starts
            .iter()
            .find(|&&start| start > caret)
            .or_else(|| starts.first())
            .copied()
    } else {
        starts
            .iter()
            .rev()
            .find(|&&start| start < caret)
            .or_else(|| starts.last())
            .copied()
    }
}

impl App {
    /// Rebuild the Problems panel from the held diagnostic sets.
    pub(super) fn refresh_problems(&mut self) {
        let entries = self
            .docs
            .diagnostics
            .iter()
            .filter_map(|(&doc, diagnostics)| Some((self.doc_path(doc)?, diagnostics)))
            .flat_map(|(path, diagnostics)| {
                diagnostics
                    .iter()
                    .filter(|diagnostic| is_problem(diagnostic))
                    .map(move |diagnostic| ProblemEntry {
                        path: path.clone(),
                        diagnostic: diagnostic.clone(),
                    })
            })
            .collect();
        self.problems.set_entries(entries);
    }

    /// Show the Problems panel with the keyboard in it.
    pub(super) fn show_problems(&mut self) {
        self.sidebar_visible = true;
        self.sidebar_panel = SidebarPanel::Problems;
        self.focus = Focus::Sidebar;
        // Arriving at the panel lands on the list, never mid-way through a
        // filter edit the user walked away from.
        self.problems.filter_input = false;
        self.refresh_problems();
    }

    /// Move the Problems selection.
    pub(super) fn problems_select(&mut self, delta: i32) {
        self.problems.selection.move_by(delta);
    }

    /// Open the selected problem's file with the caret at its start. A file
    /// heading jumps to its first shown problem.
    pub(super) fn open_selected_problem(&mut self) {
        let Some(entry) = self.problems.selected() else {
            return;
        };
        let (path, at) = (entry.path.clone(), entry.diagnostic.range.start);
        // Focus leaves the panel, so any filter edit in progress ends here.
        self.problems.filter_input = false;
        self.focus_by_file_line(&path, at, true);
    }

    /// Narrow the panel to the next severity threshold.
    pub(super) fn problems_cycle_severity(&mut self) {
        self.problems.severity = self.problems.severity.next();
        self.problems.rebuild_rows();
    }

    /// Start typing into the text filter, showing the panel first.
    pub(super) fn problems_begin_filter(&mut self) {
        if self.sidebar_panel != SidebarPanel::Problems || !self.sidebar_visible {
            self.show_problems();
        }
        self.focus = Focus::Sidebar;
        self.problems.filter_input = true;
    }

    /// Stop typing into the filter; `clear` also drops what was typed.
    pub(super) fn problems_end_filter(&mut self, clear: bool) {
        self.problems.filter_input = false;
        if clear && !self.problems.filter.is_empty() {
            self.problems.filter.clear();
            self.problems.rebuild_rows();
        }
    }

    /// Feed an unbound key to the filter: printable text extends it,
    /// Backspace trims it.
    pub(super) fn problems_filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Backspace => {
                if self.problems.filter.pop().is_none() {
                    return;
                }
            },
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.problems.filter.push(c);
            },
            _ => return,
        }
        self.problems.rebuild_rows();
    }

    /// Append pasted text to the filter (its first line: a filter is one line).
    pub(super) fn problems_filter_paste(&mut self, text: &str) {
        let line = text.lines().next().unwrap_or_default();
        if line.is_empty() {
            return;
        }
        self.problems.filter.push_str(line);
        self.problems.rebuild_rows();
    }

    /// Route a click inside the Problems panel: a toolbar action, or a row.
    pub(super) fn problems_click(&mut self, col: u16, row_y: u16) {
        if let Some(command) = self
            .problems_ui
            .action_hits
            .iter()
            .find(|&&(start, end, row, _)| row == row_y && (start..end).contains(&col))
            .map(|&(_, _, _, command)| command)
        {
            self.dispatch(command);
            return;
        }
        if !rect_contains(self.problems_ui.results_rect, (col, row_y)) {
            return;
        }
        let index = self.problems_ui.offset + (row_y - self.problems_ui.results_rect.y) as usize;
        if index < self.problems.rows.len() {
            self.problems.selection.move_to(index);
            self.open_selected_problem();
        }
    }

    /// Move the caret to the next (or previous) problem in the active file,
    /// wrapping at either end, and say what it is.
    pub(super) fn goto_problem(&mut self, forward: bool) {
        let target = self.tabs.get(self.active).and_then(|tab| match &tab.kind {
            TabKind::Code {
                doc: Some(doc),
                buffer,
                ..
            } => Some((*doc, buffer.clone(), tab.editor.cursor())),
            _ => None,
        });
        let Some((doc, buffer, caret)) = target else {
            return;
        };
        let mut problems: Vec<&Diagnostic> = self
            .docs
            .diagnostics
            .get(&doc)
            .map(|all| all.iter().filter(|d| is_problem(d)).collect())
            .unwrap_or_default();
        problems.sort_by(|a, b| {
            a.range
                .start
                .cmp(&b.range.start)
                .then(severity_rank(a.severity).cmp(&severity_rank(b.severity)))
        });
        let starts: Vec<LineCol> = problems.iter().map(|d| d.range.start).collect();
        let Some(at) = step_problem(&starts, caret, forward) else {
            self.notify(
                Report::Refusal,
                NotificationKind::Lsp,
                "no problems in this file",
            );
            return;
        };
        // Several problems may share a start; the most severe one is named.
        let (severity, message) = problems
            .iter()
            .find(|d| d.range.start == at)
            .map(|d| (d.severity, d.message.clone()))
            .unwrap_or((Severity::Unknown, String::new()));
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.editor.goto(&buffer, at);
        }
        self.view = View::Editor;
        self.focus = Focus::Editor;
        self.problems.filter_input = false;
        let label = match severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Information => "info",
            Severity::Hint => "hint",
            _ => "problem",
        };
        let first_line = message.lines().next().unwrap_or_default();
        self.notify_tagged(
            Report::Outcome,
            NotificationKind::Lsp,
            format!("{label}: {first_line}"),
            Some(PROBLEM_WALK_TAG.to_string()),
        );
    }
}

/// Tags the F8 walk's toast, so stepping replaces it rather than stacking.
const PROBLEM_WALK_TAG: &str = "problem-walk";

/// The Problems panel's per-frame chrome — same shape as the Spelling panel's.
pub(crate) type ProblemsChrome = SpellingChrome;

#[cfg(test)]
mod tests {
    use super::*;

    fn diagnostic(line: u32, col: u32, severity: Severity, message: &str) -> Diagnostic {
        Diagnostic {
            range: karet_core::Range {
                start: LineCol::new(line, col),
                end: LineCol::new(line, col + 1),
            },
            severity,
            message: message.to_owned(),
            source: Some("rustc".to_owned()),
            code: None,
            code_href: None,
            tags: Vec::new(),
            related: Vec::new(),
        }
    }

    fn entry(path: &str, line: u32, severity: Severity, message: &str) -> ProblemEntry {
        ProblemEntry {
            path: PathBuf::from(path),
            diagnostic: diagnostic(line, 0, severity, message),
        }
    }

    fn panel() -> ProblemsPanel {
        let mut panel = ProblemsPanel::default();
        panel.set_entries(vec![
            entry("b.rs", 3, Severity::Warning, "unused variable"),
            entry("a.rs", 9, Severity::Hint, "consider this"),
            entry("a.rs", 1, Severity::Error, "mismatched types"),
        ]);
        panel
    }

    #[test]
    fn entries_group_under_one_heading_per_file_in_path_then_position_order() {
        let panel = panel();
        let paths: Vec<_> = panel.entries.iter().map(|e| e.path.clone()).collect();
        assert_eq!(
            paths,
            ["a.rs", "a.rs", "b.rs"].map(PathBuf::from).to_vec(),
            "sorted by path"
        );
        assert_eq!(panel.entries[0].diagnostic.message, "mismatched types");
        assert_eq!(
            panel.rows,
            vec![
                ProblemRow::File { entry: 0, count: 2 },
                ProblemRow::Item { entry: 0 },
                ProblemRow::Item { entry: 1 },
                ProblemRow::File { entry: 2, count: 1 },
                ProblemRow::Item { entry: 2 },
            ]
        );
    }

    #[test]
    fn the_severity_filter_narrows_then_wraps_back_to_everything() {
        let mut panel = panel();
        panel.severity = panel.severity.next();
        panel.rebuild_rows();
        assert_eq!(panel.severity, SeverityFilter::Warnings);
        assert_eq!(panel.rows.len(), 4, "the hint drops out: {:?}", panel.rows);

        panel.severity = panel.severity.next();
        panel.rebuild_rows();
        assert_eq!(
            panel.rows,
            vec![
                ProblemRow::File { entry: 0, count: 1 },
                ProblemRow::Item { entry: 0 },
            ],
            "errors only; a file with none loses its heading too"
        );

        assert_eq!(panel.severity.next(), SeverityFilter::All);
    }

    #[test]
    fn the_text_filter_matches_message_source_code_and_path_case_insensitively() {
        let mut panel = panel();
        panel.filter = "UNUSED".to_owned();
        panel.rebuild_rows();
        assert_eq!(panel.rows.len(), 2, "{:?}", panel.rows);

        panel.filter = "b.rs".to_owned();
        panel.rebuild_rows();
        assert_eq!(panel.rows.len(), 2, "the path matches: {:?}", panel.rows);

        panel.filter = "rustc".to_owned();
        panel.rebuild_rows();
        assert_eq!(panel.rows.len(), 5, "the source matches every entry");

        panel.filter = "nothing like it".to_owned();
        panel.rebuild_rows();
        assert!(panel.rows.is_empty());
        assert_eq!(panel.selected(), None);
    }

    #[test]
    fn a_heading_selects_its_first_shown_problem_and_the_cursor_survives_a_refresh() {
        let mut panel = panel();
        panel.selection.move_to(3);
        assert_eq!(
            panel.selected().map(|e| e.diagnostic.message.as_str()),
            Some("unused variable")
        );
        // A refresh that removes rows clamps rather than resetting to the top.
        panel.set_entries(vec![entry("a.rs", 1, Severity::Error, "mismatched types")]);
        assert_eq!(panel.selection.cursor(), 1);
    }

    #[test]
    fn counts_include_only_errors_and_warnings_and_ignore_the_filters() {
        let mut panel = panel();
        panel.severity = SeverityFilter::Errors;
        panel.filter = "zzz".to_owned();
        panel.rebuild_rows();
        assert_eq!(panel.counts(), (1, 1));
    }

    #[test]
    fn spelling_findings_are_not_problems() {
        let mut spelled = diagnostic(0, 0, Severity::Warning, "Unknown word");
        spelled.source = Some(SPELL_SOURCE.to_owned());
        assert!(!is_problem(&spelled));
        assert!(is_problem(&diagnostic(0, 0, Severity::Warning, "x")));
        let mut sourceless = diagnostic(0, 0, Severity::Warning, "x");
        sourceless.source = None;
        assert!(is_problem(&sourceless));
    }

    #[test]
    fn stepping_moves_strictly_past_the_caret_and_wraps_at_either_end() {
        let starts = [LineCol::new(1, 0), LineCol::new(4, 2), LineCol::new(9, 0)];
        let at = |line, col| LineCol::new(line, col);
        assert_eq!(step_problem(&starts, at(0, 0), true), Some(at(1, 0)));
        // Sitting on a problem moves to the next one, not the same one again.
        assert_eq!(step_problem(&starts, at(4, 2), true), Some(at(9, 0)));
        assert_eq!(
            step_problem(&starts, at(9, 0), true),
            Some(at(1, 0)),
            "wraps"
        );
        assert_eq!(step_problem(&starts, at(4, 2), false), Some(at(1, 0)));
        assert_eq!(
            step_problem(&starts, at(1, 0), false),
            Some(at(9, 0)),
            "wraps"
        );
        assert_eq!(step_problem(&[], at(0, 0), true), None);
    }
}
