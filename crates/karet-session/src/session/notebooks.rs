//! [`NotebookCommand`] dispatch (feature `notebook-kernel`): the session actor
//! routes kernel commands to [`crate::notebook_kernel::NotebookKernels`], and
//! warms a kernel at preview-open time when `notebook.kernel.autoStart` asks
//! for it.

use std::path::Path;

use super::Session;
use crate::api::NotebookCommand;

impl Session {
    /// Serve one [`NotebookCommand`].
    pub(super) fn handle_notebook(&mut self, command: NotebookCommand) {
        match command {
            NotebookCommand::RunAll { path } => self.notebooks.run(&path, None),
            NotebookCommand::RunCell { path, cell } => self.notebooks.run(&path, Some(cell)),
            NotebookCommand::Interrupt => self.notebooks.interrupt(),
            NotebookCommand::Restart => self.notebooks.restart(),
        }
    }

    /// Start the kernel of a notebook whose preview is being converted, when
    /// `notebook.kernel.autoStart` asks for it. Observes a
    /// [`DocumentCommand::Convert`](crate::api::DocumentCommand::Convert)
    /// without consuming it: the conversion still runs.
    pub(super) fn warm_notebook_kernel(&mut self, path: &Path) {
        if self.config.settings.notebook.kernel.auto_start
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("ipynb"))
        {
            self.notebooks.warm(path);
        }
    }
}
