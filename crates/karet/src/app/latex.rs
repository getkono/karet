//! The LaTeX preview: reserving its tab, asking the backend to run the
//! configured external build, and filling the tab (and the document's
//! diagnostics) when the build answers.

use std::path::PathBuf;

use karet_core::Diagnostic;
use karet_core::NotificationKind;
use karet_session::DocumentCommand;
use karet_session::DocumentId;
use karet_session::RequestId;

use crate::app::App;
use crate::app::notifications::Report;
use crate::tab::Tab;
use crate::tab::TabKind;

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
        if let Some(request) = self.send(DocumentCommand::BuildLatex { doc }) {
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
                let mut tab = Tab::open(&pdf);
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
}
