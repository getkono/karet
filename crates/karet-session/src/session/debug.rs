//! `Debug*` command pre-dispatch: the session actor routes debugger commands
//! to the [`crate::dap::DebugManager`] without touching the main match.

use super::Session;
use crate::api::Command;
use crate::api::RequestId;
use crate::dap::RunControl;

impl Session {
    /// Consume a debugger command; `false` when `command` is not one.
    pub(super) fn handle_debug_command(&mut self, id: RequestId, command: &Command) -> bool {
        match command {
            Command::DebugStart { configuration } => {
                self.debug.start(configuration.as_deref());
            },
            Command::DebugStop => self.debug.stop(),
            Command::DebugContinue => self.debug.run_control(RunControl::Continue),
            Command::DebugStepOver => self.debug.run_control(RunControl::StepOver),
            Command::DebugStepIn => self.debug.run_control(RunControl::StepIn),
            Command::DebugStepOut => self.debug.run_control(RunControl::StepOut),
            Command::DebugPause => self.debug.run_control(RunControl::Pause),
            Command::DebugSetBreakpoints { path, lines } => {
                self.debug.set_breakpoints(path.clone(), lines.clone());
            },
            Command::DebugStackTrace => self.debug.stack_trace(id),
            Command::DebugScopes { frame } => self.debug.scopes(id, *frame),
            Command::DebugVariables { reference } => self.debug.variables(id, *reference),
            Command::DebugEvaluate { expression, frame } => {
                self.debug.evaluate(id, expression.clone(), *frame);
            },
            _ => return false,
        }
        true
    }
}
