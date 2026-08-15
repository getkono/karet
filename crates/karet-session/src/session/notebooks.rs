//! `Notebook*` command pre-dispatch (feature `notebook-kernel`): the session
//! actor routes kernel commands to [`crate::notebook_kernel::NotebookKernels`],
//! and warms a kernel at preview-open time when `notebook.kernel.autoStart`
//! asks for it.

use super::Session;
use crate::api::Command;

impl Session {
    /// Consume a notebook-kernel command; `false` when `command` is not one.
    /// `ConvertDocument` is *observed* (for the auto-start hook) but never
    /// consumed — the conversion path still runs.
    pub(super) fn handle_notebook_command(&mut self, command: &Command) -> bool {
        match command {
            Command::NotebookRunAll { path } => self.notebooks.run(path, None),
            Command::NotebookRunCell { path, cell } => self.notebooks.run(path, Some(*cell)),
            Command::NotebookInterrupt => self.notebooks.interrupt(),
            Command::NotebookRestart => self.notebooks.restart(),
            Command::ConvertDocument { path } => {
                if self.config.settings.notebook.kernel.auto_start
                    && path
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("ipynb"))
                {
                    self.notebooks.warm(path);
                }
                return false;
            },
            _ => return false,
        }
        true
    }
}
