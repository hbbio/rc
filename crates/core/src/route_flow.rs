use crate::*;

impl AppState {
    pub(super) fn apply_route_command(&mut self, command: AppCommand) -> CommandOutcome {
        match command {
            AppCommand::MenuNoop => {}
            AppCommand::MenuNotImplemented(label) => {
                self.set_status(format!("{label} is not implemented yet"));
            }
            AppCommand::OpenMenu => self.open_menu(0),
            AppCommand::OpenMenuAt(index) => self.open_menu(index),
            AppCommand::CloseMenu => self.close_menu(),
            AppCommand::OpenHelp => self.open_help_screen(),
            AppCommand::CloseHelp => self.close_help_screen(),
            AppCommand::Quit => {
                if self.settings.confirmation.confirm_quit {
                    self.start_quit_confirmation();
                } else {
                    self.request_cancel_for_all_jobs();
                    return CommandOutcome::Quit;
                }
            }
            AppCommand::CloseViewer => self.close_viewer(),
            AppCommand::OpenFindDialog => self.open_find_dialog(),
            AppCommand::CloseFindResults => self.close_find_results(),
            AppCommand::OpenTree => self.open_tree_screen(),
            AppCommand::CloseTree => self.close_tree_screen(),
            AppCommand::OpenHotlist => self.open_hotlist_screen(),
            AppCommand::CloseHotlist => self.close_hotlist_screen(),
            AppCommand::OpenPanelizeDialog => self.open_panelize_dialog(),
            AppCommand::PanelizePresetAdd => self.start_panelize_preset_add(),
            AppCommand::PanelizePresetEdit => self.start_panelize_preset_edit(),
            AppCommand::PanelizePresetRemove => self.remove_panelize_preset(),
            AppCommand::EnterXMap => {
                self.xmap_pending = true;
                self.set_status("Extended keymap mode");
            }
            AppCommand::SwitchPanel => {
                self.toggle_active_panel();
                self.set_status(format!(
                    "Active panel: {}",
                    match self.active_panel {
                        ActivePanel::Left => "left",
                        ActivePanel::Right => "right",
                    }
                ));
            }
            AppCommand::OpenJobsScreen => self.open_jobs_screen(),
            AppCommand::CloseJobsScreen => self.close_jobs_screen(),
            AppCommand::JobsMoveUp => self.move_jobs_cursor(-1),
            AppCommand::JobsMoveDown => self.move_jobs_cursor(1),
            AppCommand::MenuMoveUp => {
                if let Some(menu) = self.menu_state_mut() {
                    menu.move_up();
                }
            }
            AppCommand::MenuMoveDown => {
                if let Some(menu) = self.menu_state_mut() {
                    menu.move_down();
                }
            }
            AppCommand::MenuMoveLeft => {
                if let Some(menu) = self.menu_state_mut() {
                    menu.move_left();
                }
            }
            AppCommand::MenuMoveRight => {
                if let Some(menu) = self.menu_state_mut() {
                    menu.move_right();
                }
            }
            AppCommand::MenuHome => {
                if let Some(menu) = self.menu_state_mut() {
                    menu.move_home();
                }
            }
            AppCommand::MenuEnd => {
                if let Some(menu) = self.menu_state_mut() {
                    menu.move_end();
                }
            }
            AppCommand::MenuAccept => {
                if let Some(next_command) = self.accept_menu_selection() {
                    return CommandOutcome::FollowUp(next_command);
                }
            }
            AppCommand::MenuSelectAt(index) => {
                if let Some(next_command) = self.accept_menu_selection_at(index) {
                    return CommandOutcome::FollowUp(next_command);
                }
            }
            AppCommand::HelpMoveUp
            | AppCommand::HelpMoveDown
            | AppCommand::HelpPageUp
            | AppCommand::HelpPageDown
            | AppCommand::HelpHalfPageUp
            | AppCommand::HelpHalfPageDown
            | AppCommand::HelpHome
            | AppCommand::HelpEnd
            | AppCommand::HelpFollowLink
            | AppCommand::HelpBack
            | AppCommand::HelpIndex
            | AppCommand::HelpLinkNext
            | AppCommand::HelpLinkPrev
            | AppCommand::HelpNodeNext
            | AppCommand::HelpNodePrev => self.apply_help_route_command(command),
            _ => unreachable!("non-route command dispatched to route domain: {command:?}"),
        }

        CommandOutcome::Continue
    }

    fn apply_help_route_command(&mut self, command: AppCommand) {
        match command {
            AppCommand::HelpMoveUp => {
                if let Some(help) = self.help_state_mut() {
                    help.move_lines(-1);
                }
            }
            AppCommand::HelpMoveDown => {
                if let Some(help) = self.help_state_mut() {
                    help.move_lines(1);
                }
            }
            AppCommand::HelpPageUp => {
                if let Some(help) = self.help_state_mut() {
                    help.move_pages(-1);
                }
            }
            AppCommand::HelpPageDown => {
                if let Some(help) = self.help_state_mut() {
                    help.move_pages(1);
                }
            }
            AppCommand::HelpHalfPageUp => {
                if let Some(help) = self.help_state_mut() {
                    help.move_half_pages(-1);
                }
            }
            AppCommand::HelpHalfPageDown => {
                if let Some(help) = self.help_state_mut() {
                    help.move_half_pages(1);
                }
            }
            AppCommand::HelpHome => {
                if let Some(help) = self.help_state_mut() {
                    help.move_home();
                }
            }
            AppCommand::HelpEnd => {
                if let Some(help) = self.help_state_mut() {
                    help.move_end();
                }
            }
            AppCommand::HelpFollowLink => {
                if let Some(help) = self.help_state_mut()
                    && !help.follow_selected_link()
                {
                    self.set_status("No help link selected");
                }
            }
            AppCommand::HelpBack => {
                if let Some(help) = self.help_state_mut()
                    && !help.back()
                {
                    self.set_status("Help history is empty");
                }
            }
            AppCommand::HelpIndex => {
                if let Some(help) = self.help_state_mut() {
                    help.open_index();
                }
            }
            AppCommand::HelpLinkNext => {
                if let Some(help) = self.help_state_mut() {
                    help.select_next_link();
                }
            }
            AppCommand::HelpLinkPrev => {
                if let Some(help) = self.help_state_mut() {
                    help.select_prev_link();
                }
            }
            AppCommand::HelpNodeNext => {
                if let Some(help) = self.help_state_mut() {
                    help.open_next_node();
                }
            }
            AppCommand::HelpNodePrev => {
                if let Some(help) = self.help_state_mut() {
                    help.open_prev_node();
                }
            }
            _ => unreachable!("non-help command dispatched to help route handler: {command:?}"),
        }
    }

    pub(crate) fn open_help_screen(&mut self) {
        let context = self.key_context();
        if let Some(Route::Help(help)) = self.routes.last_mut() {
            help.open_for_context(KeyContext::Help);
            self.set_status("Help: help viewer");
            return;
        }

        let replacements = self.help_replacements();
        self.routes
            .push(Route::Help(HelpState::for_context_with_replacements(
                context,
                &replacements,
            )));
        self.set_status("Opened help");
    }

    pub(crate) fn close_help_screen(&mut self) {
        if matches!(self.top_route(), Route::Help(_)) {
            self.routes.pop();
            self.set_status("Closed help");
        }
    }

    pub(crate) fn help_state_mut(&mut self) -> Option<&mut HelpState> {
        let Some(Route::Help(help)) = self.routes.last_mut() else {
            return None;
        };
        Some(help)
    }

    pub(crate) fn open_menu(&mut self, menu_index: usize) {
        if let Some(Route::Menu(menu)) = self.routes.last_mut() {
            menu.set_active_menu(menu_index);
            let title = menu.active_menu_title();
            self.set_status(format!("Menu: {title}"));
            return;
        }

        let menu = MenuState::new(menu_index);
        self.set_status(format!("Menu: {}", menu.active_menu_title()));
        self.routes.push(Route::Menu(menu));
    }

    pub(crate) fn close_menu(&mut self) {
        if matches!(self.top_route(), Route::Menu(_)) {
            self.routes.pop();
            self.set_status("Closed menu");
        }
    }

    pub(crate) fn menu_state_mut(&mut self) -> Option<&mut MenuState> {
        let Some(Route::Menu(menu)) = self.routes.last_mut() else {
            return None;
        };
        Some(menu)
    }

    pub(crate) fn accept_menu_selection(&mut self) -> Option<AppCommand> {
        let selected = self
            .menu_state_mut()
            .and_then(|menu| menu.selected_command());
        self.close_menu();
        selected
    }

    pub(crate) fn accept_menu_selection_at(&mut self, index: usize) -> Option<AppCommand> {
        if let Some(menu) = self.menu_state_mut() {
            menu.select_entry(index);
        }
        self.accept_menu_selection()
    }

    pub fn command_for_left_click(&self, column: u16, row: u16) -> Option<AppCommand> {
        if !matches!(self.top_route(), Route::FileManager | Route::Menu(_)) {
            return None;
        }

        if row == 0
            && let Some(menu_index) = top_menu_hit_test(column)
        {
            return Some(AppCommand::OpenMenuAt(menu_index));
        }

        let Route::Menu(menu) = self.top_route() else {
            return None;
        };

        if let Some(entry_index) = self.menu_hit_test_entry(menu, column, row) {
            return Some(AppCommand::MenuSelectAt(entry_index));
        }

        Some(AppCommand::CloseMenu)
    }

    pub(crate) fn open_jobs_screen(&mut self) {
        if !matches!(self.top_route(), Route::Jobs) {
            self.routes.push(Route::Jobs);
        }
        self.clamp_jobs_cursor();
        self.set_status("Opened jobs screen");
    }

    pub(crate) fn close_jobs_screen(&mut self) {
        if matches!(self.top_route(), Route::Jobs) {
            self.routes.pop();
            self.set_status("Closed jobs screen");
        }
    }

    pub(crate) fn close_viewer(&mut self) {
        if matches!(self.top_route(), Route::Viewer(_)) {
            self.routes.pop();
            self.set_status("Closed viewer");
        }
    }

    pub(crate) fn clamp_jobs_cursor(&mut self) {
        let len = self.jobs.jobs().len();
        if len == 0 {
            self.jobs_cursor = 0;
        } else if self.jobs_cursor >= len {
            self.jobs_cursor = len - 1;
        }
    }

    pub(crate) fn move_jobs_cursor(&mut self, delta: isize) {
        let len = self.jobs.jobs().len();
        if len == 0 {
            self.jobs_cursor = 0;
            return;
        }
        let last = len - 1;
        let next = if delta.is_negative() {
            self.jobs_cursor.saturating_sub(delta.unsigned_abs())
        } else {
            self.jobs_cursor.saturating_add(delta as usize).min(last)
        };
        self.jobs_cursor = next;
    }

    pub fn selected_job_record(&self) -> Option<&JobRecord> {
        self.jobs.jobs().get(self.jobs_cursor)
    }

    fn menu_hit_test_entry(&self, menu: &MenuState, column: u16, row: u16) -> Option<usize> {
        let x = menu.popup_origin_x();
        let y = 1u16;
        let width = self.menu_popup_width(menu);
        let items = menu.active_entries().len() as u16;
        if items == 0 {
            return None;
        }

        if row < y + 1 || row >= y + 1 + items {
            return None;
        }
        if column < x + 1 || column >= x + width.saturating_sub(1) {
            return None;
        }

        let index = (row - (y + 1)) as usize;
        menu.active_entries()
            .get(index)
            .filter(|entry| entry.selectable)
            .map(|_| index)
    }
}
