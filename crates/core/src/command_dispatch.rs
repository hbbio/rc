use std::io;

use crate::*;

impl AppState {
    pub fn apply(&mut self, command: AppCommand) -> io::Result<ApplyResult> {
        if self.xmap_pending && !matches!(self.top_route(), Route::FileManager) {
            self.xmap_pending = false;
        }
        let clear_xmap_after_command = self.xmap_pending
            && matches!(self.top_route(), Route::FileManager)
            && !matches!(command, AppCommand::EnterXMap);

        let outcome = match command.domain() {
            CommandDomain::Route => self.apply_route_command(command),
            CommandDomain::Navigation => self.apply_navigation_command(command)?,
            CommandDomain::Viewer => self.apply_viewer_command(command),
            CommandDomain::Dialog => self.apply_dialog_command(command),
            CommandDomain::Settings => self.apply_settings_command(command),
        };

        if clear_xmap_after_command {
            self.xmap_pending = false;
        }

        if matches!(outcome, CommandOutcome::Quit) {
            return Ok(ApplyResult::Quit);
        }

        if self.pending_quit {
            self.pending_quit = false;
            return Ok(ApplyResult::Quit);
        }

        if let CommandOutcome::FollowUp(next_command) = outcome {
            return self.apply(next_command);
        }

        Ok(ApplyResult::Continue)
    }
}
