//! Turning a document into something to look at: the LaTeX preview build and
//! the markdown table formatter.
//!
//! Split from `editor`, which owns carets, scrolling and clicks. These two
//! reshape or render a whole document rather than editing at a caret, and the
//! preview is the only thing in that module that drives an external build.

use super::super::*;

impl App {
    /// Reserve a preview immediately, then compile the active editable TeX document
    /// through the backend's configured external recipe.
    pub(in crate::app) fn build_latex_preview(&mut self) {
        let (doc, source) = match self.tabs.get(self.active).map(|tab| &tab.kind) {
            Some(TabKind::Code {
                path,
                doc: Some(doc),
                ..
            }) if path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("tex")) =>
            {
                (*doc, path.clone())
            },
            _ => {
                self.notify(
                    Report::Refusal,
                    NotificationKind::System,
                    "LaTeX preview requires an open editable .tex file",
                );
                return;
            },
        };

        self.push_tab(Tab::latex_preview(source));
        let view = self.tabs[self.active].view;
        if let Some(request) = self.send(SessionCommand::BuildLatex { doc }) {
            self.latex_previews.insert(request, view);
        } else if let TabKind::LatexPreview { error, .. } = &mut self.tabs[self.active].kind {
            *error = Some("LaTeX backend is unavailable".to_owned());
        }
    }

    /// Merge compiler diagnostics into the document and fill the reserved preview.
    pub(in crate::app) fn finish_latex_build(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        pdf: Option<PathBuf>,
        diagnostics: Vec<Diagnostic>,
        error: Option<String>,
    ) {
        let mut combined = self
            .docs
            .diagnostics
            .get(&doc)
            .into_iter()
            .flatten()
            .filter(|diagnostic| diagnostic.source.as_deref() != Some("latex"))
            .cloned()
            .collect::<Vec<_>>();
        combined.extend(diagnostics);
        self.replace_document_diagnostics(doc, combined);
        let destination = id.and_then(|request| self.latex_previews.remove(&request));
        if let Some(view) = destination
            && let Some(index) = self.tabs.iter().position(|tab| tab.view == view)
        {
            if let Some(pdf) = pdf {
                let mut tab = workspace::open_file(&pdf);
                tab.view = view;
                self.tabs[index] = tab;
                self.set_active(index);
                self.notify(
                    Report::Outcome,
                    NotificationKind::System,
                    "LaTeX preview built",
                );
            } else if let TabKind::LatexPreview {
                error: preview_error,
                ..
            } = &mut self.tabs[index].kind
            {
                *preview_error = Some(
                    error
                        .clone()
                        .unwrap_or_else(|| "LaTeX build produced no PDF".to_owned()),
                );
            }
        }
        if let Some(error) = error {
            self.notify(Report::Failure, NotificationKind::System, error);
        }
        self.maybe_auto_complete_spelling(doc);
    }

    /// Format every GFM table in the active Markdown document as one undoable edit.
    pub(in crate::app) fn format_markdown_tables(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        if !matches!(
            &tab.kind,
            TabKind::Code { path, .. }
                if karet_filetype::file_type_for_path(path).name() == "Markdown"
        ) {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "Table formatting is available for Markdown files",
            );
            return;
        }
        let TabKind::Code { buffer, .. } = &tab.kind else {
            return;
        };
        let original = buffer.text();
        let ranges = karet_markdown::table_line_ranges(&original);
        if ranges.is_empty() {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "No Markdown tables found",
            );
            return;
        }
        let formatted = karet_markdown::format_tables(&original);
        if formatted == original {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "Markdown tables are already formatted",
            );
            return;
        }
        let primary = tab.editor.cursor();
        let cursors = tab.editor.cursors().clone();
        self.submit_edit(move |caret, _selection, buffer, base| {
            (caret == primary).then(|| editing::replace_document(buffer, formatted.clone(), base))
        });
        if let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            editor.set_cursor_state(buffer, cursors);
        }
        self.notify(
            Report::Outcome,
            NotificationKind::System,
            format!(
                "Formatted {} Markdown table{}",
                ranges.len(),
                if ranges.len() == 1 { "" } else { "s" }
            ),
        );
    }
}
