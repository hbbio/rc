use crate::*;

impl AppState {
    pub(crate) fn open_panelize_dialog(&mut self) {
        let initial_command = self
            .active_panel()
            .panelize_command()
            .unwrap_or("find . -type f")
            .to_string();
        let preset_commands = self.panelize_presets().to_vec();
        self.open_panelize_preset_selection_dialog(initial_command, preset_commands);
        self.set_status("External panelize");
    }

    pub(crate) fn open_panelize_preset_selection_dialog(
        &mut self,
        initial_command: String,
        preset_commands: Vec<String>,
    ) {
        let mut items = vec![String::from(PANELIZE_CUSTOM_COMMAND_LABEL)];
        items.extend(preset_commands.iter().cloned());
        let selected = panelize_preset_selected_index(&initial_command, &preset_commands);
        self.pending_dialog_action = Some(PendingDialogAction::PanelizePresetSelection {
            initial_command,
            preset_commands,
        });
        self.routes.push(Route::Dialog(DialogState::listbox(
            "External panelize",
            items,
            selected,
        )));
    }

    pub(crate) fn open_panelize_command_input_dialog(
        &mut self,
        initial_command: String,
        preset_commands: Vec<String>,
    ) {
        self.pending_dialog_action = Some(PendingDialogAction::PanelizeCommand { preset_commands });
        self.routes.push(Route::Dialog(DialogState::input(
            "External panelize",
            "Command (stdout paths):",
            initial_command,
        )));
    }

    pub(crate) fn toggle_panelize_dialog_focus(&mut self) -> bool {
        match self.pending_dialog_action.clone() {
            Some(PendingDialogAction::PanelizePresetSelection {
                initial_command,
                preset_commands,
            }) => {
                let is_listbox = matches!(
                    self.top_route(),
                    Route::Dialog(DialogState {
                        kind: DialogKind::Listbox(_),
                        ..
                    })
                );
                if !is_listbox {
                    return false;
                }
                self.routes.pop();
                self.open_panelize_command_input_dialog(initial_command, preset_commands);
                self.set_status("External panelize: enter command");
                true
            }
            Some(PendingDialogAction::PanelizeCommand { preset_commands }) => {
                let initial_command = match self.top_route() {
                    Route::Dialog(dialog) => match &dialog.kind {
                        DialogKind::Input(input) => input.value.clone(),
                        _ => return false,
                    },
                    _ => return false,
                };
                self.routes.pop();
                self.open_panelize_preset_selection_dialog(initial_command, preset_commands);
                self.set_status("External panelize");
                true
            }
            _ => false,
        }
    }

    pub(crate) fn start_panelize_preset_add(&mut self) {
        let Some((initial_command, preset_commands, _)) = self.active_panelize_preset_selection()
        else {
            return;
        };
        self.pending_dialog_action = Some(PendingDialogAction::PanelizePresetAdd {
            initial_command,
            preset_commands,
        });
        self.routes.push(Route::Dialog(DialogState::input(
            "Add panelize command",
            "Command:",
            "",
        )));
        self.set_status("Panelize preset: add command");
    }

    pub(crate) fn start_panelize_preset_edit(&mut self) {
        let Some((initial_command, preset_commands, selected_index)) =
            self.active_panelize_preset_selection()
        else {
            return;
        };
        if selected_index == 0 {
            self.set_status("Select a preset command to edit");
            return;
        }
        let preset_index = selected_index - 1;
        let Some(existing_command) = preset_commands.get(preset_index).cloned() else {
            self.set_status("Panelize preset selection is invalid");
            return;
        };
        self.pending_dialog_action = Some(PendingDialogAction::PanelizePresetEdit {
            initial_command,
            preset_commands,
            preset_index,
        });
        self.routes.push(Route::Dialog(DialogState::input(
            "Edit panelize command",
            "Command:",
            existing_command,
        )));
        self.set_status("Panelize preset: edit command");
    }

    pub(crate) fn remove_panelize_preset(&mut self) {
        let Some((initial_command, mut preset_commands, selected_index)) =
            self.active_panelize_preset_selection()
        else {
            return;
        };
        if selected_index == 0 {
            self.set_status("Select a preset command to remove");
            return;
        }
        let preset_index = selected_index - 1;
        let Some(removed_command) =
            (preset_index < preset_commands.len()).then(|| preset_commands.remove(preset_index))
        else {
            self.set_status("Panelize preset selection is invalid");
            return;
        };

        self.settings.configuration.panelize_presets = preset_commands.clone();
        self.settings.mark_dirty();
        self.routes.pop();
        let next_initial = if initial_command == removed_command {
            preset_commands
                .first()
                .cloned()
                .unwrap_or_else(|| String::from("find . -type f"))
        } else {
            initial_command
        };
        self.open_panelize_preset_selection_dialog(next_initial, preset_commands);
        self.set_status(format!("Removed panelize preset: {removed_command}"));
    }

    pub(crate) fn start_panelize_command(&mut self, command: String) {
        let active_panel = self.active_panel;
        let previous_source = self.active_panel().source.clone();
        {
            let panel = self.active_panel_mut();
            panel.source = PanelListingSource::Panelize { command };
            panel.cursor = 0;
            panel.tagged.clear();
            panel.loading = true;
        }
        self.schedule_panelize_revert_for_panel_refresh(active_panel, previous_source);
        self.queue_panel_refresh(active_panel);
        self.set_status("Panelize running...");
    }

    fn active_panelize_preset_selection(&self) -> Option<(String, Vec<String>, usize)> {
        let PendingDialogAction::PanelizePresetSelection {
            initial_command,
            preset_commands,
        } = self.pending_dialog_action.clone()?
        else {
            return None;
        };
        let Route::Dialog(dialog) = self.top_route() else {
            return None;
        };
        let DialogKind::Listbox(listbox) = &dialog.kind else {
            return None;
        };
        Some((initial_command, preset_commands, listbox.selected))
    }
}

fn panelize_preset_selected_index(initial_command: &str, preset_commands: &[String]) -> usize {
    preset_commands
        .iter()
        .position(|command| command == initial_command)
        .map_or(0, |index| index.saturating_add(1))
}
