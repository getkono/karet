//! [`DebugCommand`] dispatch: the session actor routes debugger commands to
//! the [`crate::dap::DebugManager`].

use super::Session;
use crate::api::DebugCommand;
use crate::api::RequestId;
use crate::dap::RunControl;

impl Session {
    /// Serve one [`DebugCommand`].
    pub(super) fn handle_debug(&mut self, id: RequestId, command: DebugCommand) {
        match command {
            DebugCommand::Start { configuration } => {
                self.debug.start(configuration.as_deref());
            },
            DebugCommand::Stop => self.debug.stop(),
            DebugCommand::Continue => self.debug.run_control(RunControl::Continue),
            DebugCommand::StepOver => self.debug.run_control(RunControl::StepOver),
            DebugCommand::StepIn => self.debug.run_control(RunControl::StepIn),
            DebugCommand::StepOut => self.debug.run_control(RunControl::StepOut),
            DebugCommand::Pause => self.debug.run_control(RunControl::Pause),
            DebugCommand::SetBreakpoints { path, lines } => {
                self.debug.set_breakpoints(path, lines);
            },
            DebugCommand::StackTrace => self.debug.stack_trace(id),
            DebugCommand::Scopes { frame } => self.debug.scopes(id, frame),
            DebugCommand::Variables { reference } => self.debug.variables(id, reference),
            DebugCommand::Evaluate { expression, frame } => {
                self.debug.evaluate(id, expression, frame);
            },
        }
    }
}
