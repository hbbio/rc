use crate::*;

impl AppState {
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
