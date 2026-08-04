use crate::*;

impl AppState {
    pub(crate) fn cancel_active_panelized_refresh(&mut self) -> bool {
        let panel = self.active_panel;
        let panel_state = &self.panels[panel.index()];
        if !panel_state.source.is_panelized() || !panel_state.loading {
            return false;
        }
        let Some(job_id) = self.panel_refresh_job_id(panel) else {
            return false;
        };

        if self.request_cancel_for_job(job_id) {
            self.set_status(format!("Canceling panelize job #{job_id}..."));
        } else {
            self.set_status(format!("Panelize job #{job_id} cannot be canceled"));
        }
        true
    }

    pub(crate) fn restore_panelized_results(&mut self) {
        self.restore_panelized_results_for(self.active_panel);
    }

    pub(crate) fn restore_panelized_results_for(&mut self, panel_id: ActivePanel) {
        let panel_index = panel_id.index();
        self.deactivate_quick_view(panel_id);
        self.panel_views[panel_index] = PanelViewMode::Listing;
        if self.panels[panel_index].source.is_panelized() {
            self.set_status(format!(
                "{} panelized results are already active",
                panel_id.label()
            ));
            return;
        }
        let Some(snapshot) = self.panelized_result_history[panel_index].clone() else {
            self.set_status(format!(
                "No panelized results to restore for the {} panel",
                panel_id.label()
            ));
            return;
        };

        self.cancel_and_invalidate_panel_refresh(panel_id);
        let PanelizedResultSnapshot {
            cwd,
            source,
            entries,
            unfiltered_entries,
            cursor,
            tagged,
            disk_usage,
        } = snapshot;
        let source_label = match source {
            PanelListingSource::Panelize { .. } => "external",
            PanelListingSource::FindResults { .. } => "find",
            PanelListingSource::Directory => {
                self.set_status("Stored panelized results are invalid");
                return;
            }
        };
        let selected_path = entries.get(cursor).map(|entry| entry.path.clone());
        let raw_entries = unfiltered_entries
            .as_deref()
            .map(<[FileEntry]>::to_vec)
            .unwrap_or(entries);
        let filter = self.panels[panel_index].filter.clone();
        let panelized_entries = Arc::<[FileEntry]>::from(raw_entries);
        let Ok(mut entries) = apply_panel_filter(panelized_entries.to_vec(), &filter) else {
            self.set_status("Stored panelized results have an invalid filter");
            return;
        };
        let sort_mode = self.panels[panel_index].sort_mode;
        sort_file_entries(&mut entries, sort_mode);
        let restored_cursor = selected_path
            .as_ref()
            .and_then(|path| entries.iter().position(|entry| entry.path == *path))
            .unwrap_or_else(|| cursor.min(entries.len().saturating_sub(1)));
        let result_count = entries.len();

        let panel = &mut self.panels[panel_index];
        panel.cwd = cwd;
        panel.source = source;
        panel.panelized_entries = Some(panelized_entries);
        panel.entries = entries;
        panel.cursor = restored_cursor;
        panel.tagged = tagged;
        panel.loading = false;
        panel.disk_usage = disk_usage;
        self.sync_quick_view_from(panel_id, true);
        self.sync_selection_size(panel_id, true);
        self.set_status(format!(
            "Restored {result_count} {source_label} result(s) in the {} panel",
            panel_id.label()
        ));
    }

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
        let footer = self.panelize_preset_footer();
        self.push_dialog(
            DialogState::listbox_with_hint("External panelize", items, selected, footer),
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
        let Some((initial_command, presets, _)) = self.active_panelize_preset_selection() else {
            return;
        };
        self.push_dialog(
            DialogState::pair_input(
                "Add panelize preset",
                "Label:",
                "",
                "Command (stdout paths):",
                "",
            ),
            PendingDialogAction::PanelizePresetAdd {
                initial_command,
                presets,
            },
        );
        self.set_status("Panelize preset: add label and command");
    }

    pub(crate) fn start_panelize_preset_edit(&mut self) {
        let Some((initial_command, presets, selected_index)) =
            self.active_panelize_preset_selection()
        else {
            return;
        };
        if selected_index == 0 {
            self.set_status("Select a preset command to edit");
            return;
        }
        let preset_index = selected_index - 1;
        let Some(existing) = presets.get(preset_index).cloned() else {
            self.set_status("Panelize preset selection is invalid");
            return;
        };
        self.push_dialog(
            DialogState::pair_input(
                "Edit panelize preset",
                "Label:",
                existing.label,
                "Command (stdout paths):",
                existing.command,
            ),
            PendingDialogAction::PanelizePresetEdit {
                initial_command,
                presets,
                preset_index,
            },
        );
        self.set_status("Panelize preset: edit label or command");
    }

    pub(crate) fn remove_panelize_preset(&mut self) {
        let Some((initial_command, presets, selected_index)) =
            self.active_panelize_preset_selection()
        else {
            return;
        };
        if selected_index == 0 {
            self.set_status("Select a preset command to remove");
            return;
        }
        let preset_index = selected_index - 1;
        let Some(preset) = presets.get(preset_index).cloned() else {
            self.set_status("Panelize preset selection is invalid");
            return;
        };
        self.push_dialog(
            DialogState::confirm(
                "Remove panelize preset",
                format!("Remove '{}' ({})?", preset.label, preset.command),
            ),
            PendingDialogAction::PanelizePresetRemove {
                initial_command,
                presets,
                preset_index,
            },
        );
        self.set_status("Confirm panelize preset removal");
    }

    pub(crate) fn submit_panelize_preset_add(
        &mut self,
        initial_command: String,
        presets: Vec<PanelizePreset>,
        label: String,
        command: String,
    ) {
        if !self.panelize_presets_are_current(&presets) {
            self.restore_current_panelize_presets(initial_command);
            self.set_status("Panelize preset add canceled: presets changed");
            return;
        }
        let preset = match validate_panelize_preset(&label, &command, &presets, None) {
            Ok(preset) => preset,
            Err(error) => {
                self.reopen_panelize_preset_editor(
                    "Add panelize preset",
                    label,
                    command,
                    PendingDialogAction::PanelizePresetAdd {
                        initial_command,
                        presets,
                    },
                    error,
                );
                return;
            }
        };

        let label = preset.label.clone();
        let command = preset.command.clone();
        let mut updated = presets;
        updated.push(preset);
        self.commit_panelize_presets(command, updated);
        self.set_status(format!("Added panelize preset: {label}"));
    }

    pub(crate) fn submit_panelize_preset_edit(
        &mut self,
        initial_command: String,
        presets: Vec<PanelizePreset>,
        preset_index: usize,
        label: String,
        command: String,
    ) {
        if !self.panelize_presets_are_current(&presets) {
            self.restore_current_panelize_presets(initial_command);
            self.set_status("Panelize preset edit canceled: presets changed");
            return;
        }
        if preset_index >= presets.len() {
            self.restore_current_panelize_presets(initial_command);
            self.set_status("Panelize preset edit failed: invalid selection");
            return;
        }
        let preset = match validate_panelize_preset(&label, &command, &presets, Some(preset_index))
        {
            Ok(preset) => preset,
            Err(error) => {
                self.reopen_panelize_preset_editor(
                    "Edit panelize preset",
                    label,
                    command,
                    PendingDialogAction::PanelizePresetEdit {
                        initial_command,
                        presets,
                        preset_index,
                    },
                    error,
                );
                return;
            }
        };

        let label = preset.label.clone();
        let command = preset.command.clone();
        let mut updated = presets;
        updated[preset_index] = preset;
        self.commit_panelize_presets(command, updated);
        self.set_status(format!("Updated panelize preset: {label}"));
    }

    pub(crate) fn confirm_panelize_preset_remove(
        &mut self,
        initial_command: String,
        mut presets: Vec<PanelizePreset>,
        preset_index: usize,
    ) {
        if !self.panelize_presets_are_current(&presets) {
            self.restore_current_panelize_presets(initial_command);
            self.set_status("Panelize preset removal canceled: presets changed");
            return;
        }
        if preset_index >= presets.len() {
            self.restore_current_panelize_presets(initial_command);
            self.set_status("Panelize preset removal failed: invalid selection");
            return;
        }
        let removed = presets.remove(preset_index);
        let next_initial = if initial_command == removed.command {
            presets
                .first()
                .map(|preset| preset.command.clone())
                .unwrap_or_else(|| String::from("find . -type f"))
        } else {
            initial_command
        };
        self.commit_panelize_presets(next_initial, presets);
        self.set_status(format!("Removed panelize preset: {}", removed.label));
    }

    fn panelize_presets_are_current(&self, expected: &[PanelizePreset]) -> bool {
        self.settings.configuration.panelize_presets == expected
    }

    fn panelize_preset_footer(&self) -> String {
        let binding = |command, fallback: &str| {
            self.keybinding_primary_label(KeyContext::Listbox, command)
                .unwrap_or(fallback)
                .to_string()
        };
        let tab = binding(AppCommand::DialogFocusNext, "Tab");
        let add = binding(AppCommand::PanelizePresetAdd, "F2");
        let edit = binding(AppCommand::PanelizePresetEdit, "F4");
        let remove = binding(AppCommand::PanelizePresetRemove, "F8");
        let run = binding(AppCommand::DialogAccept, "Enter");
        let cancel = binding(AppCommand::DialogCancel, "Esc");
        format!(
            "Up/Down select | {tab} command | {add} add | {edit} edit\n\
             {remove} remove | {run} run | {cancel} cancel"
        )
    }

    fn commit_panelize_presets(&mut self, selected_command: String, presets: Vec<PanelizePreset>) {
        self.settings.configuration.panelize_presets = presets.clone();
        self.settings.mark_dirty();
        self.routes.pop();
        self.open_panelize_preset_selection_dialog(selected_command, presets);
    }

    fn restore_current_panelize_presets(&mut self, selected_command: String) {
        let presets = self.settings.configuration.panelize_presets.clone();
        self.routes.pop();
        self.open_panelize_preset_selection_dialog(selected_command, presets);
    }

    fn reopen_panelize_preset_editor(
        &mut self,
        title: &str,
        label: String,
        command: String,
        action: PendingDialogAction,
        error: String,
    ) {
        self.push_dialog(
            DialogState::pair_input(title, "Label:", label, "Command (stdout paths):", command),
            action,
        );
        self.set_status(error);
    }

    pub(crate) fn start_panelize_command(&mut self, command: String) {
        let active_panel = self.active_panel;
        let revert = self.panel_refresh_revert_snapshot(active_panel);
        {
            let panel = self.active_panel_mut();
            panel.source = PanelListingSource::Panelize { command };
            panel.panelized_entries = None;
            panel.cursor = 0;
            panel.tagged.clear();
            panel.loading = true;
        }
        self.schedule_panel_refresh_revert(active_panel, revert);
        self.sync_quick_view_from(active_panel, false);
        self.sync_selection_size(active_panel, false);
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

fn validate_panelize_preset(
    label: &str,
    command: &str,
    presets: &[PanelizePreset],
    edited_index: Option<usize>,
) -> Result<PanelizePreset, String> {
    let label = label.trim();
    if label.is_empty() {
        return Err(String::from("Panelize preset label cannot be empty"));
    }
    let command = command.trim();
    if command.is_empty() {
        return Err(String::from("Panelize preset command cannot be empty"));
    }

    let normalized_candidate: String = label.chars().flat_map(char::to_lowercase).collect();
    for (index, existing) in presets.iter().enumerate() {
        if Some(index) == edited_index {
            continue;
        }
        let normalized_existing: String = existing
            .label
            .trim()
            .chars()
            .flat_map(char::to_lowercase)
            .collect();
        if normalized_existing == normalized_candidate {
            return Err(format!(
                "Panelize preset label already exists: {}",
                existing.label
            ));
        }
        if existing.command.trim() == command {
            return Err(format!(
                "Panelize preset command already exists as '{}'",
                existing.label
            ));
        }
    }

    Ok(PanelizePreset::new(label, command))
}
