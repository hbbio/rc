use crate::*;

impl AppState {
    pub(crate) fn open_panelize_dialog(&mut self) {
        let initial_command = self
            .active_panel()
            .panelize_command()
            .unwrap_or("find . -type f")
            .to_string();
        let presets = self.panelize_presets().to_vec();
        self.open_panelize_preset_selection_dialog(initial_command, presets);
        self.set_status("External panelize");
    }

    pub(crate) fn open_panelize_preset_selection_dialog(
        &mut self,
        initial_command: String,
        presets: Vec<PanelizePreset>,
    ) {
        let mut items = vec![String::from(PANELIZE_CUSTOM_COMMAND_LABEL)];
        items.extend(presets.iter().map(|preset| preset.label.clone()));
        let selected = panelize_preset_selected_index(&initial_command, &presets);
        self.push_dialog(
            DialogState::listbox("External panelize", items, selected),
            PendingDialogAction::PanelizePresetSelection {
                initial_command,
                presets,
            },
        );
    }

    pub(crate) fn open_panelize_command_input_dialog(
        &mut self,
        initial_command: String,
        presets: Vec<PanelizePreset>,
    ) {
        self.push_dialog(
            DialogState::input(
                "External panelize",
                "Command (stdout paths):",
                initial_command,
            ),
            PendingDialogAction::PanelizeCommand { presets },
        );
    }

    pub(crate) fn toggle_panelize_dialog_focus(&mut self) -> bool {
        let Some(Route::Dialog(dialog)) = self.routes.last() else {
            return false;
        };
        match dialog.action().cloned() {
            Some(PendingDialogAction::PanelizePresetSelection {
                initial_command,
                presets,
            }) => {
                let is_listbox = matches!(&dialog.kind, DialogKind::Listbox(_));
                if !is_listbox {
                    return false;
                }
                self.routes.pop();
                self.open_panelize_command_input_dialog(initial_command, presets);
                self.set_status("External panelize: enter command");
                true
            }
            Some(PendingDialogAction::PanelizeCommand { presets }) => {
                let initial_command = match &dialog.kind {
                    DialogKind::Input(input) => input.value.clone(),
                    _ => return false,
                };
                self.routes.pop();
                self.open_panelize_preset_selection_dialog(initial_command, presets);
                self.set_status("External panelize");
                true
            }
            _ => false,
        }
    }

    pub(crate) fn start_panelize_preset_add(&mut self) {
        let Some((_, presets, _)) = self.active_panelize_preset_selection() else {
            return;
        };
        self.push_dialog(
            DialogState::input("Add panelize command", "Command:", ""),
            PendingDialogAction::PanelizePresetAdd { presets },
        );
        self.set_status("Panelize preset: add command");
    }

    pub(crate) fn start_panelize_preset_edit(&mut self) {
        let Some((_, presets, selected_index)) = self.active_panelize_preset_selection() else {
            return;
        };
        if selected_index == 0 {
            self.set_status("Select a preset command to edit");
            return;
        }
        let preset_index = selected_index - 1;
        let Some(existing_command) = presets
            .get(preset_index)
            .map(|preset| preset.command.clone())
        else {
            self.set_status("Panelize preset selection is invalid");
            return;
        };
        self.push_dialog(
            DialogState::input("Edit panelize command", "Command:", existing_command),
            PendingDialogAction::PanelizePresetEdit {
                presets,
                preset_index,
            },
        );
        self.set_status("Panelize preset: edit command");
    }

    pub(crate) fn remove_panelize_preset(&mut self) {
        let Some((initial_command, mut presets, selected_index)) =
            self.active_panelize_preset_selection()
        else {
            return;
        };
        if selected_index == 0 {
            self.set_status("Select a preset command to remove");
            return;
        }
        let preset_index = selected_index - 1;
        let Some(removed) = (preset_index < presets.len()).then(|| presets.remove(preset_index))
        else {
            self.set_status("Panelize preset selection is invalid");
            return;
        };

        self.settings.configuration.panelize_presets = presets.clone();
        self.settings.mark_dirty();
        self.routes.pop();
        let next_initial = if initial_command == removed.command {
            presets
                .first()
                .map(|preset| preset.command.clone())
                .unwrap_or_else(|| String::from("find . -type f"))
        } else {
            initial_command
        };
        self.open_panelize_preset_selection_dialog(next_initial, presets);
        self.set_status(format!("Removed panelize preset: {}", removed.label));
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

    fn active_panelize_preset_selection(&self) -> Option<(String, Vec<PanelizePreset>, usize)> {
        let Route::Dialog(dialog) = self.top_route() else {
            return None;
        };
        let Some(PendingDialogAction::PanelizePresetSelection {
            initial_command,
            presets,
        }) = dialog.action().cloned()
        else {
            return None;
        };
        let DialogKind::Listbox(listbox) = &dialog.kind else {
            return None;
        };
        Some((initial_command, presets, listbox.selected))
    }
}

fn panelize_preset_selected_index(initial_command: &str, presets: &[PanelizePreset]) -> usize {
    presets
        .iter()
        .position(|preset| preset.command == initial_command)
        .map_or(0, |index| index.saturating_add(1))
}
