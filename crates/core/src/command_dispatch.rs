use std::io;

use crate::*;

impl AppState {
    fn archive_command_restriction(&self, command: AppCommand) -> Option<&'static str> {
        if self.active_panel().is_archive()
            && matches!(
                command,
                AppCommand::Move
                    | AppCommand::Delete
                    | AppCommand::EditEntry
                    | AppCommand::OpenConfirmDialog
                    | AppCommand::OpenInputDialog
            )
        {
            return Some("Archives are read-only; use F5 to copy entries out");
        }
        if self.active_panel().is_archive()
            && matches!(
                command,
                AppCommand::OpenCommandLine
                    | AppCommand::PutCurrentSelected
                    | AppCommand::PutCurrentFullSelected
                    | AppCommand::PutCurrentTagged
                    | AppCommand::PutOtherTagged
                    | AppCommand::OpenFindDialog
                    | AppCommand::OpenPanelizeDialog
                    | AppCommand::OpenTree
                    | AppCommand::HotlistAddCurrentDirectory
            )
            || (command == AppCommand::PutOtherTagged && self.passive_panel().is_archive())
        {
            return Some("This command requires a local directory; leave the archive first");
        }
        if let AppCommand::Panel(panel, PanelCommand::OpenTree) = command
            && self.panels[panel.index()].is_archive()
        {
            return Some("Directory tree requires a local directory");
        }
        None
    }

    pub fn apply(&mut self, command: AppCommand) -> io::Result<ApplyResult> {
        if self.xmap_pending && !matches!(self.top_route(), Route::FileManager) {
            self.xmap_pending = false;
        }
        let clear_xmap_after_command = self.xmap_pending
            && matches!(self.top_route(), Route::FileManager)
            && !matches!(command, AppCommand::EnterXMap);

        if let Some(message) = self.archive_command_restriction(command) {
            if clear_xmap_after_command {
                self.xmap_pending = false;
            }
            self.set_status(message);
            return Ok(ApplyResult::Continue);
        }

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
