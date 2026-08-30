use crate::layout::{
    ScreenRect, find_results_layout, hotlist_layout, listbox_dialog_layout, tree_layout,
    visible_window,
};
use crate::*;

impl AppState {
    pub(super) fn apply_route_command(&mut self, command: AppCommand) -> CommandOutcome {
        match command {
            AppCommand::MenuNoop => {}
            AppCommand::MenuNotImplemented(label) => {
                self.set_status(format!("{label} is not implemented yet"));
            }
            AppCommand::OpenUserMenu => {
                self.set_status("User menu is not implemented yet (planned for Milestone 5)");
            }
            AppCommand::OpenCommandLine => self.open_command_line(),
            AppCommand::OpenMenuBar => self.open_menu_bar(0),
            AppCommand::OpenMenuBarAt(index) => self.open_menu_bar(index),
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
            AppCommand::Panel(panel, PanelCommand::OpenTree) => self.open_tree_screen_for(panel),
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
                if self.toggle_active_panel() {
                    self.set_status(format!("Active panel: {}", self.active_panel.label()));
                } else {
                    self.set_status("The other panel is not a file listing");
                }
            }
            AppCommand::OpenJobsScreen => self.open_jobs_screen(),
            AppCommand::CloseJobsScreen => self.close_jobs_screen(),
            AppCommand::Navigate(NavigationTarget::Jobs, motion) => {
                self.apply_jobs_navigation(motion);
            }
            AppCommand::Navigate(NavigationTarget::Menu, motion) => {
                self.apply_menu_navigation(motion);
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
            AppCommand::Navigate(NavigationTarget::Help, motion) => {
                self.apply_help_navigation(motion);
            }
            AppCommand::HelpFollowLink
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

    fn apply_jobs_navigation(&mut self, motion: NavigationMotion) {
        match motion {
            NavigationMotion::Up => self.move_jobs_cursor(-1),
            NavigationMotion::Down => self.move_jobs_cursor(1),
            _ => {}
        }
    }

    fn apply_menu_navigation(&mut self, motion: NavigationMotion) {
        let Some(menu) = self.menu_state_mut() else {
            return;
        };
        match motion {
            NavigationMotion::Up => menu.move_up(),
            NavigationMotion::Down => menu.move_down(),
            NavigationMotion::Left => menu.move_left(),
            NavigationMotion::Right => menu.move_right(),
            NavigationMotion::Home => menu.move_home(),
            NavigationMotion::End => menu.move_end(),
            _ => {}
        }
    }

    fn apply_help_navigation(&mut self, motion: NavigationMotion) {
        let Some(help) = self.help_state_mut() else {
            return;
        };
        match motion {
            NavigationMotion::Up => help.move_lines(-1),
            NavigationMotion::Down => help.move_lines(1),
            NavigationMotion::PageUp => help.move_pages(-1),
            NavigationMotion::PageDown => help.move_pages(1),
            NavigationMotion::HalfPageUp => help.move_half_pages(-1),
            NavigationMotion::HalfPageDown => help.move_half_pages(1),
            NavigationMotion::Home => help.move_home(),
            NavigationMotion::End => help.move_end(),
            _ => {}
        }
    }

    fn apply_help_route_command(&mut self, command: AppCommand) {
        match command {
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

    pub(crate) fn open_menu_bar(&mut self, menu_index: usize) {
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

    pub fn commands_for_left_click(
        &self,
        column: u16,
        row: u16,
        viewport_width: u16,
        viewport_height: u16,
    ) -> Option<MouseClickCommands> {
        let viewport = ScreenRect::new(0, 0, viewport_width, viewport_height);
        match self.top_route() {
            Route::FileManager | Route::Menu(_) => self.menu_commands_for_left_click(column, row),
            Route::CommandLine(_) => None,
            Route::FindResults(results) => {
                let layout = find_results_layout(viewport);
                let index = visible_list_index_at(
                    layout.list,
                    column,
                    row,
                    results.entries.len(),
                    results.cursor,
                )?;
                Some(MouseClickCommands::list_selection(
                    AppCommand::FindResultsSelectAt(index),
                    AppCommand::FindResultsOpenEntry,
                ))
            }
            Route::Tree(tree) => {
                let layout = tree_layout(viewport);
                let index = visible_list_index_at(
                    layout.list,
                    column,
                    row,
                    tree.visible_entry_count(),
                    tree.visible_cursor(),
                )?;
                let target = MouseClickTarget::TreeEntry(tree.visible_entry(index)?.path.clone());
                Some(MouseClickCommands::list_selection_with_target(
                    AppCommand::TreeSelectVisibleAt(index),
                    AppCommand::TreeOpenEntry,
                    target,
                ))
            }
            Route::Hotlist => {
                let layout = hotlist_layout(viewport);
                let index = visible_list_index_at(
                    layout.list,
                    column,
                    row,
                    self.hotlist().len(),
                    self.hotlist_cursor,
                )?;
                Some(MouseClickCommands::list_selection(
                    AppCommand::HotlistSelectAt(index),
                    AppCommand::HotlistOpenEntry,
                ))
            }
            Route::Dialog(dialog)
                if matches!(
                    dialog.action(),
                    Some(PendingDialogAction::PanelizePresetSelection { .. })
                ) =>
            {
                let DialogKind::Listbox(listbox) = &dialog.kind else {
                    return None;
                };
                let footer_height = if listbox.footer_hint.is_some() { 2 } else { 1 };
                let layout = listbox_dialog_layout(viewport, footer_height);
                let index = visible_list_index_at(
                    layout.list,
                    column,
                    row,
                    listbox.items.len(),
                    listbox.selected,
                )?;
                Some(MouseClickCommands::list_selection(
                    AppCommand::DialogListboxSelectAt(index),
                    AppCommand::DialogAccept,
                ))
            }
            Route::Dialog(_)
            | Route::Jobs
            | Route::Viewer(_)
            | Route::Help(_)
            | Route::Settings(_) => None,
        }
    }

    fn menu_commands_for_left_click(&self, column: u16, row: u16) -> Option<MouseClickCommands> {
        if self.show_menu_bar()
            && row == 0
            && let Some(menu_index) = top_menu_hit_test(column)
        {
            return Some(MouseClickCommands::primary(AppCommand::OpenMenuBarAt(
                menu_index,
            )));
        }

        let Route::Menu(menu) = self.top_route() else {
            return None;
        };

        if let Some(entry_index) = self.menu_hit_test_entry(menu, column, row) {
            return Some(MouseClickCommands::primary(AppCommand::MenuSelectAt(
                entry_index,
            )));
        }

        Some(MouseClickCommands::primary(AppCommand::CloseMenu))
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

fn visible_list_index_at(
    list_area: ScreenRect,
    column: u16,
    row: u16,
    total: usize,
    cursor: usize,
) -> Option<usize> {
    if !list_area.contains(column, row) || total == 0 {
        return None;
    }
    let (window_start, window_end) =
        visible_window(total, cursor, list_area.height.max(1) as usize);
    let index = window_start.saturating_add(row.saturating_sub(list_area.y) as usize);
    (index < window_end).then_some(index)
}
