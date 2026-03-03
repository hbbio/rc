use std::io;

use crate::dialog::DialogEvent;
use crate::*;

impl AppState {
    pub fn apply(&mut self, command: AppCommand) -> io::Result<ApplyResult> {
        if self.xmap_pending && !matches!(self.top_route(), Route::FileManager) {
            self.xmap_pending = false;
        }
        let clear_xmap_after_command = self.xmap_pending
            && matches!(self.top_route(), Route::FileManager)
            && !matches!(command, AppCommand::EnterXMap);
        let mut follow_up_command = None;
        if let Some(result) = self.apply_shell_command(command) {
            if clear_xmap_after_command {
                self.xmap_pending = false;
            }
            return Ok(result);
        }

        if !self.apply_viewer_command(command) {
            match command {
                AppCommand::MenuNoop
                | AppCommand::MenuNotImplemented(_)
                | AppCommand::OpenMenu
                | AppCommand::OpenMenuAt(_)
                | AppCommand::CloseMenu
                | AppCommand::OpenHelp
                | AppCommand::CloseHelp
                | AppCommand::Quit
                | AppCommand::CloseViewer
                | AppCommand::OpenFindDialog
                | AppCommand::CloseFindResults
                | AppCommand::OpenTree
                | AppCommand::CloseTree
                | AppCommand::OpenHotlist
                | AppCommand::CloseHotlist
                | AppCommand::OpenPanelizeDialog
                | AppCommand::PanelizePresetAdd
                | AppCommand::PanelizePresetEdit
                | AppCommand::PanelizePresetRemove
                | AppCommand::EnterXMap
                | AppCommand::SwitchPanel
                | AppCommand::ViewerMoveUp
                | AppCommand::ViewerMoveDown
                | AppCommand::ViewerPageUp
                | AppCommand::ViewerPageDown
                | AppCommand::ViewerHome
                | AppCommand::ViewerEnd
                | AppCommand::ViewerSearchForward
                | AppCommand::ViewerSearchBackward
                | AppCommand::ViewerSearchContinue
                | AppCommand::ViewerSearchContinueBackward
                | AppCommand::ViewerGoto
                | AppCommand::ViewerToggleWrap
                | AppCommand::ViewerToggleHex => {
                    unreachable!("shell command should be handled before main apply dispatch")
                }
                AppCommand::MoveUp => self.move_cursor(-1),
                AppCommand::MoveDown => self.move_cursor(1),
                AppCommand::PageUp => {
                    let page_step = self.settings.advanced.page_step;
                    self.active_panel_mut().move_cursor_page(-1, page_step);
                }
                AppCommand::PageDown => {
                    let page_step = self.settings.advanced.page_step;
                    self.active_panel_mut().move_cursor_page(1, page_step);
                }
                AppCommand::MoveHome => self.active_panel_mut().move_cursor_home(),
                AppCommand::MoveEnd => self.active_panel_mut().move_cursor_end(),
                AppCommand::ToggleTag => {
                    let selected = self.active_panel().selected_entry();
                    if selected.is_none() {
                        self.set_status("No entry selected");
                    } else if selected.is_some_and(|entry| entry.is_parent) {
                        self.set_status("Parent entry cannot be tagged");
                    } else {
                        let added = self.active_panel_mut().toggle_tag_on_cursor();
                        self.active_panel_mut().move_cursor(1);
                        let count = self.active_panel().tagged_count();
                        self.set_status(if added {
                            format!("Tagged entry ({count} total)")
                        } else {
                            format!("Untagged entry ({count} total)")
                        });
                    }
                }
                AppCommand::InvertTags => {
                    self.active_panel_mut().invert_tags();
                    let count = self.active_panel().tagged_count();
                    self.set_status(format!("Inverted tags ({count} selected)"));
                }
                AppCommand::SortNext => {
                    self.active_panel_mut().cycle_sort_field();
                    self.refresh_active_panel();
                    let label = self.active_panel().sort_label();
                    self.set_status(format!("Sort: {label}"));
                }
                AppCommand::SortReverse => {
                    self.active_panel_mut().toggle_sort_direction();
                    self.refresh_active_panel();
                    let label = self.active_panel().sort_label();
                    self.set_status(format!("Sort: {label}"));
                }
                AppCommand::Copy => self.start_copy_dialog(),
                AppCommand::Move => self.start_move_dialog(),
                AppCommand::Delete => {
                    if self.settings.confirmation.confirm_delete {
                        self.start_delete_confirmation();
                    } else {
                        let targets = self.selected_operation_paths();
                        if targets.is_empty() {
                            self.set_status("Delete requires a selected or tagged entry");
                        } else {
                            self.queue_delete_job(targets);
                        }
                    }
                }
                AppCommand::CancelJob => self.cancel_latest_job(),
                AppCommand::OpenJobsScreen => self.open_jobs_screen(),
                AppCommand::CloseJobsScreen => self.close_jobs_screen(),
                AppCommand::JobsMoveUp => self.move_jobs_cursor(-1),
                AppCommand::JobsMoveDown => self.move_jobs_cursor(1),
                AppCommand::OpenEntry => {
                    if self.open_selected_directory() {
                        self.queue_panel_refresh(self.active_panel);
                        self.set_status("Loading selected directory...");
                    } else if self.open_selected_file_in_viewer() {
                        self.set_status("Opening viewer...");
                    } else {
                        self.set_status("No entry selected");
                    }
                }
                AppCommand::EditEntry => match self.open_selected_file_in_editor() {
                    EditSelectionResult::OpenedExternal => {
                        self.set_status("Opening external editor...")
                    }
                    EditSelectionResult::OpenedInternal => {
                        self.set_status("Opening internal editor...")
                    }
                    EditSelectionResult::NoEntrySelected => self.set_status("No entry selected"),
                    EditSelectionResult::SelectedEntryIsDirectory => {
                        self.set_status("Directory cannot be edited");
                    }
                },
                AppCommand::CdUp => {
                    if self.exit_panelize_mode() {
                        self.queue_panel_refresh(self.active_panel);
                        self.set_status("Leaving panelize mode...");
                    } else if self.go_parent_directory() {
                        self.queue_panel_refresh(self.active_panel);
                        self.set_status("Loading parent directory...");
                    } else {
                        self.set_status("Already at filesystem root");
                    }
                }
                AppCommand::Reread => {
                    self.refresh_active_panel();
                    self.set_status("Refreshing active panel...");
                }
                AppCommand::FindResultsMoveUp => self.move_find_results_cursor(-1),
                AppCommand::FindResultsMoveDown => self.move_find_results_cursor(1),
                AppCommand::FindResultsPageUp => self.move_find_results_page(-1),
                AppCommand::FindResultsPageDown => self.move_find_results_page(1),
                AppCommand::FindResultsHome => self.move_find_results_home(),
                AppCommand::FindResultsEnd => self.move_find_results_end(),
                AppCommand::FindResultsOpenEntry => {
                    self.open_selected_find_result()?;
                }
                AppCommand::FindResultsPanelize => self.panelize_find_results(),
                AppCommand::TreeMoveUp => self.move_tree_cursor(-1),
                AppCommand::TreeMoveDown => self.move_tree_cursor(1),
                AppCommand::TreePageUp => self.move_tree_page(-1),
                AppCommand::TreePageDown => self.move_tree_page(1),
                AppCommand::TreeHome => self.move_tree_home(),
                AppCommand::TreeEnd => self.move_tree_end(),
                AppCommand::TreeOpenEntry => {
                    self.open_selected_tree_entry()?;
                }
                AppCommand::HotlistMoveUp => self.move_hotlist_cursor(-1),
                AppCommand::HotlistMoveDown => self.move_hotlist_cursor(1),
                AppCommand::HotlistPageUp => self.move_hotlist_page(-1),
                AppCommand::HotlistPageDown => self.move_hotlist_page(1),
                AppCommand::HotlistHome => self.move_hotlist_home(),
                AppCommand::HotlistEnd => self.move_hotlist_end(),
                AppCommand::HotlistOpenEntry => {
                    self.open_selected_hotlist_entry()?;
                }
                AppCommand::HotlistAddCurrentDirectory => self.add_current_directory_to_hotlist(),
                AppCommand::HotlistRemoveSelected => self.remove_selected_hotlist_entry(),
                AppCommand::OpenConfirmDialog => self.start_rename_dialog(),
                AppCommand::OpenInputDialog => self.start_mkdir_dialog(),
                AppCommand::OpenListboxDialog => self.start_overwrite_policy_dialog(),
                AppCommand::OpenSkinDialog => self.start_skin_dialog(),
                AppCommand::OpenOptionsConfiguration => {
                    self.open_settings_screen(SettingsCategory::Configuration)
                }
                AppCommand::OpenOptionsLayout => {
                    self.open_settings_screen(SettingsCategory::Layout)
                }
                AppCommand::OpenOptionsPanelOptions => {
                    self.open_settings_screen(SettingsCategory::PanelOptions)
                }
                AppCommand::OpenOptionsConfirmation => {
                    self.open_settings_screen(SettingsCategory::Confirmation)
                }
                AppCommand::OpenOptionsAppearance => {
                    self.open_settings_screen(SettingsCategory::Appearance)
                }
                AppCommand::OpenOptionsDisplayBits => {
                    self.open_settings_screen(SettingsCategory::DisplayBits)
                }
                AppCommand::OpenOptionsLearnKeys => {
                    self.open_settings_screen(SettingsCategory::LearnKeys)
                }
                AppCommand::OpenOptionsVirtualFs => {
                    self.open_settings_screen(SettingsCategory::VirtualFs)
                }
                AppCommand::SaveSetup => {
                    self.pending_save_setup = true;
                    self.set_status("Save setup requested");
                }
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
                    follow_up_command = self.accept_menu_selection();
                }
                AppCommand::MenuSelectAt(index) => {
                    follow_up_command = self.accept_menu_selection_at(index);
                }
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
                AppCommand::DialogAccept => {
                    if matches!(self.top_route(), Route::Settings(_)) {
                        self.apply_settings_entry();
                    } else {
                        self.handle_dialog_event(DialogEvent::Accept);
                    }
                }
                AppCommand::DialogCancel => {
                    if matches!(self.top_route(), Route::Settings(_)) {
                        self.close_settings_screen();
                    } else {
                        self.handle_dialog_event(DialogEvent::Cancel);
                    }
                }
                AppCommand::DialogFocusNext => {
                    if !self.toggle_panelize_dialog_focus() {
                        self.handle_dialog_event(DialogEvent::FocusNext);
                    }
                }
                AppCommand::DialogBackspace => self.handle_dialog_event(DialogEvent::Backspace),
                AppCommand::DialogInputChar(ch) => {
                    self.handle_dialog_event(DialogEvent::InsertChar(ch))
                }
                AppCommand::DialogListboxUp => {
                    if let Some(settings) = self.settings_state_mut() {
                        settings.move_up();
                    } else {
                        self.handle_dialog_event(DialogEvent::MoveUp);
                    }
                }
                AppCommand::DialogListboxDown => {
                    if let Some(settings) = self.settings_state_mut() {
                        settings.move_down();
                    } else {
                        self.handle_dialog_event(DialogEvent::MoveDown);
                    }
                }
            }
        }

        if clear_xmap_after_command {
            self.xmap_pending = false;
        }

        if self.pending_quit {
            self.pending_quit = false;
            return Ok(ApplyResult::Quit);
        }

        if let Some(next_command) = follow_up_command {
            return self.apply(next_command);
        }

        Ok(ApplyResult::Continue)
    }
}
