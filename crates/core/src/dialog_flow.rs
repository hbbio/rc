use std::path::PathBuf;

use crate::dialog::DialogEvent;
use crate::*;

impl AppState {
    pub(crate) fn push_dialog(&mut self, state: DialogState, action: PendingDialogAction) {
        self.routes
            .push(Route::Dialog(DialogRoute::new(state, action)));
    }

    pub(super) fn apply_dialog_command(&mut self, command: AppCommand) -> CommandOutcome {
        match command {
            AppCommand::OpenConfirmDialog => self.start_rename_dialog(),
            AppCommand::OpenInputDialog => self.start_mkdir_dialog(),
            AppCommand::OpenListboxDialog => self.start_overwrite_policy_dialog(),
            AppCommand::OpenSkinDialog => self.start_skin_dialog(),
            AppCommand::FindDialogBrowse => self.open_find_tree_picker(),
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
            _ => unreachable!("non-dialog command dispatched to dialog handler: {command:?}"),
        }

        CommandOutcome::Continue
    }

    pub(crate) fn start_copy_dialog(&mut self) {
        self.start_transfer_dialog(TransferKind::Copy);
    }

    pub(crate) fn start_move_dialog(&mut self) {
        self.start_transfer_dialog(TransferKind::Move);
    }

    fn start_transfer_dialog(&mut self, kind: TransferKind) {
        let sources = self.selected_operation_paths();
        if sources.is_empty() {
            self.set_status("Copy/Move requires a selected or tagged entry");
            return;
        }

        let destination_dir = self.passive_panel().cwd.clone();
        let source_base_dir = self.active_panel().cwd.clone();
        self.start_transfer_dialog_for_paths(
            kind,
            sources,
            source_base_dir,
            destination_dir,
            OperationOrigin::Panel,
        );
    }

    pub(crate) fn start_transfer_dialog_for_paths(
        &mut self,
        kind: TransferKind,
        sources: Vec<PathBuf>,
        source_base_dir: PathBuf,
        destination_dir: PathBuf,
        origin: OperationOrigin,
    ) {
        let title = match kind {
            TransferKind::Copy => "Copy",
            TransferKind::Move => "Move",
        };
        self.push_dialog(
            DialogState::input(
                title,
                "Destination directory:",
                destination_dir.to_string_lossy(),
            ),
            PendingDialogAction::TransferDestination {
                kind,
                sources,
                source_base_dir,
                origin,
            },
        );
        self.set_status(format!("{title}: choose destination"));
    }

    pub(crate) fn start_delete_confirmation(&mut self) {
        let targets = self.selected_operation_paths();
        if targets.is_empty() {
            self.set_status("Delete requires a selected or tagged entry");
            return;
        }

        self.start_delete_confirmation_for_targets(targets, OperationOrigin::Panel);
    }

    pub(crate) fn start_delete_confirmation_for_targets(
        &mut self,
        targets: Vec<PathBuf>,
        origin: OperationOrigin,
    ) {
        let message = if targets.len() == 1 {
            let name = targets[0]
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| targets[0].to_string_lossy().into_owned());
            format!("Delete '{name}'?")
        } else {
            format!("Delete {} selected items?", targets.len())
        };
        self.push_dialog(
            DialogState::confirm("Delete", message),
            PendingDialogAction::ConfirmDelete { targets, origin },
        );
        self.set_status("Confirm delete");
    }

    pub(crate) fn start_quit_confirmation(&mut self) {
        self.push_dialog(
            DialogState::confirm("Quit", "Exit rc?"),
            PendingDialogAction::ConfirmQuit,
        );
        self.set_status("Confirm quit");
    }

    pub(crate) fn start_rename_dialog(&mut self) {
        let Some(entry) = self.selected_non_parent_entry() else {
            self.set_status("Rename requires a selected entry");
            return;
        };
        let tagged_count = self.active_panel().tagged_count();
        if tagged_count > 1 {
            self.set_status("Rename supports a single selected entry");
            return;
        }

        let source = entry.path.clone();
        let current_name = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| entry.name.clone());
        self.push_dialog(
            DialogState::input("Rename/Move", "New name:", current_name),
            PendingDialogAction::RenameEntry { source },
        );
        self.set_status("Rename/Move: enter new name");
    }

    pub(crate) fn start_mkdir_dialog(&mut self) {
        let base_dir = self.active_panel().cwd.clone();
        self.start_mkdir_dialog_at(base_dir, OperationOrigin::Panel);
    }

    pub(crate) fn start_mkdir_dialog_at(&mut self, base_dir: PathBuf, origin: OperationOrigin) {
        self.push_dialog(
            DialogState::input("Mkdir", "Directory name:", ""),
            PendingDialogAction::Mkdir { base_dir, origin },
        );
        self.set_status("Mkdir: enter directory name");
    }

    pub(crate) fn start_overwrite_policy_dialog(&mut self) {
        let selected = overwrite_policy_index(self.overwrite_policy());
        self.push_dialog(
            DialogState::listbox("Overwrite Policy", overwrite_policy_items(), selected),
            PendingDialogAction::SetDefaultOverwritePolicy,
        );
        self.set_status("Choose default overwrite policy");
    }

    pub(crate) fn start_skin_dialog(&mut self) {
        if self.available_skins.is_empty() {
            self.set_status("No skins available");
            return;
        }

        let selected = self
            .available_skins
            .iter()
            .position(|name| name.eq_ignore_ascii_case(self.active_skin_name()))
            .unwrap_or(0);
        self.push_dialog(
            DialogState::listbox("Skin", self.available_skins.clone(), selected),
            PendingDialogAction::SetSkin {
                original_skin: self.active_skin_name().to_string(),
            },
        );
        self.set_status("Choose skin");
    }

    pub(crate) fn finish_dialog(&mut self, result: DialogResult) {
        let pending = match self.routes.last() {
            Some(Route::Dialog(_)) => match self.routes.pop() {
                Some(Route::Dialog(mut dialog)) => dialog.take_action(),
                _ => None,
            },
            _ => None,
        };
        match (pending, result) {
            (None, result) => self.set_status(result.status_line()),
            (
                Some(PendingDialogAction::ConfirmDelete { targets, origin }),
                DialogResult::ConfirmAccepted,
            ) => {
                self.queue_delete_job_from(targets, origin);
            }
            (Some(PendingDialogAction::ConfirmDelete { .. }), DialogResult::ConfirmDeclined)
            | (Some(PendingDialogAction::ConfirmDelete { .. }), DialogResult::Canceled) => {
                self.set_status("Delete canceled");
            }
            (Some(PendingDialogAction::ConfirmQuit), DialogResult::ConfirmAccepted) => {
                self.request_cancel_for_all_jobs();
                self.pending_quit = true;
                self.set_status("Quitting...");
            }
            (Some(PendingDialogAction::ConfirmQuit), DialogResult::ConfirmDeclined)
            | (Some(PendingDialogAction::ConfirmQuit), DialogResult::Canceled) => {
                self.set_status("Quit canceled");
            }
            (
                Some(PendingDialogAction::Mkdir { base_dir, origin }),
                DialogResult::InputSubmitted(value),
            ) => {
                let value = value.trim();
                if value.is_empty() {
                    self.set_status("Mkdir canceled: empty name");
                    return;
                }
                let input_path = PathBuf::from(value);
                let destination = if input_path.is_absolute() {
                    input_path
                } else {
                    base_dir.join(input_path)
                };
                self.queue_filesystem_job(JobRequest::Mkdir { path: destination }, origin);
            }
            (Some(PendingDialogAction::Mkdir { .. }), DialogResult::Canceled) => {
                self.set_status("Mkdir canceled");
            }
            (
                Some(PendingDialogAction::RenameEntry { source }),
                DialogResult::InputSubmitted(value),
            ) => {
                let value = value.trim();
                if value.is_empty() {
                    self.set_status("Rename canceled: empty name");
                    return;
                }
                let Some(parent) = source.parent() else {
                    self.set_status("Rename failed: source has no parent directory");
                    return;
                };
                let destination = parent.join(value);
                if destination == source {
                    self.set_status("Rename skipped: name unchanged");
                    return;
                }
                self.queue_worker_job_request(JobRequest::Rename {
                    source,
                    destination,
                });
            }
            (Some(PendingDialogAction::RenameEntry { .. }), DialogResult::Canceled) => {
                self.set_status("Rename canceled");
            }
            (
                Some(PendingDialogAction::TransferDestination {
                    kind,
                    sources,
                    source_base_dir,
                    origin,
                }),
                DialogResult::InputSubmitted(value),
            ) => {
                let value = value.trim();
                if value.is_empty() {
                    self.set_status("Copy/Move canceled: empty destination");
                    return;
                }
                let input_path = PathBuf::from(value);
                let destination_dir = if input_path.is_absolute() {
                    input_path
                } else {
                    source_base_dir.join(input_path)
                };
                if self.settings.confirmation.confirm_overwrite {
                    let selected = overwrite_policy_index(self.overwrite_policy());
                    self.push_dialog(
                        DialogState::listbox(
                            "Overwrite Policy",
                            overwrite_policy_items(),
                            selected,
                        ),
                        PendingDialogAction::TransferOverwrite {
                            kind,
                            sources,
                            destination_dir,
                            origin,
                        },
                    );
                    self.set_status("Choose overwrite policy");
                } else {
                    self.queue_copy_or_move_job(
                        kind,
                        sources,
                        destination_dir,
                        self.overwrite_policy(),
                        origin,
                    );
                }
            }
            (Some(PendingDialogAction::TransferDestination { .. }), DialogResult::Canceled) => {
                self.set_status("Copy/Move canceled");
            }
            (
                Some(PendingDialogAction::TransferOverwrite {
                    kind,
                    sources,
                    destination_dir,
                    origin,
                }),
                DialogResult::ListboxSubmitted { index, .. },
            ) => {
                let overwrite = index
                    .map(overwrite_policy_from_index)
                    .unwrap_or(self.overwrite_policy());
                self.queue_copy_or_move_job(kind, sources, destination_dir, overwrite, origin);
            }
            (Some(PendingDialogAction::TransferOverwrite { .. }), DialogResult::Canceled) => {
                self.set_status("Copy/Move canceled");
            }
            (
                Some(PendingDialogAction::SetDefaultOverwritePolicy),
                DialogResult::ListboxSubmitted { index, .. },
            ) => {
                if let Some(index) = index {
                    let policy = overwrite_policy_from_index(index);
                    self.set_overwrite_policy(policy);
                    self.settings.mark_dirty();
                    self.set_status(format!(
                        "Default overwrite policy: {}",
                        self.overwrite_policy().label()
                    ));
                } else {
                    self.set_status("Overwrite policy unchanged");
                }
            }
            (Some(PendingDialogAction::SetDefaultOverwritePolicy), DialogResult::Canceled) => {
                self.set_status("Overwrite policy unchanged");
            }
            (
                Some(PendingDialogAction::SetSkin { .. }),
                DialogResult::ListboxSubmitted {
                    value: Some(value), ..
                },
            ) => {
                self.pending_skin_preview = None;
                self.pending_skin_change = Some(value.clone());
                self.set_status(format!("Skin selected: {value}"));
            }
            (
                Some(PendingDialogAction::SetSkin { .. }),
                DialogResult::ListboxSubmitted { value: None, .. },
            ) => {
                self.pending_skin_preview = None;
                self.set_status("Skin unchanged");
            }
            (Some(PendingDialogAction::SetSkin { original_skin }), DialogResult::Canceled) => {
                self.pending_skin_preview = None;
                self.pending_skin_revert = Some(original_skin);
                self.set_status("Skin unchanged");
            }
            (Some(PendingDialogAction::FindSearch), DialogResult::FindSubmitted(spec)) => {
                self.start_find_search(*spec);
            }
            (Some(PendingDialogAction::FindSearch), DialogResult::Canceled) => {
                self.set_status("Find canceled");
            }
            (
                Some(PendingDialogAction::HotlistAdd { base_dir }),
                DialogResult::PairInputSubmitted { first, second },
            ) => self.submit_hotlist_add(base_dir, first, second),
            (
                Some(PendingDialogAction::HotlistEdit {
                    base_dir,
                    index,
                    original,
                }),
                DialogResult::PairInputSubmitted { first, second },
            ) => self.submit_hotlist_edit(base_dir, index, original, first, second),
            (
                Some(PendingDialogAction::HotlistRemove { index, entry }),
                DialogResult::ConfirmAccepted,
            ) => self.remove_hotlist_entry(index, &entry),
            (Some(PendingDialogAction::HotlistAdd { .. }), DialogResult::Canceled) => {
                self.set_status("Hotlist entry add canceled");
            }
            (Some(PendingDialogAction::HotlistEdit { .. }), DialogResult::Canceled) => {
                self.set_status("Hotlist entry edit canceled");
            }
            (Some(PendingDialogAction::HotlistRemove { .. }), DialogResult::ConfirmDeclined)
            | (Some(PendingDialogAction::HotlistRemove { .. }), DialogResult::Canceled) => {
                self.set_status("Hotlist entry removal canceled");
            }
            (
                Some(PendingDialogAction::PanelizePresetSelection {
                    initial_command,
                    presets,
                }),
                DialogResult::ListboxSubmitted { index, .. },
            ) => {
                let Some(index) = index else {
                    self.set_status("Panelize canceled");
                    return;
                };
                if index == 0 {
                    self.open_panelize_command_input_dialog(initial_command, presets);
                    self.set_status("External panelize: enter command");
                    return;
                }
                let Some(command) = presets
                    .get(index.saturating_sub(1))
                    .map(|preset| preset.command.clone())
                else {
                    self.set_status("Panelize canceled");
                    return;
                };
                self.start_panelize_command(command);
            }
            (Some(PendingDialogAction::PanelizePresetSelection { .. }), DialogResult::Canceled) => {
                self.set_status("Panelize canceled");
            }
            (
                Some(PendingDialogAction::PanelizeCommand { .. }),
                DialogResult::InputSubmitted(value),
            ) => {
                let command = value.trim();
                if command.is_empty() {
                    self.set_status("Panelize canceled: empty command");
                    return;
                }

                self.start_panelize_command(command.to_string());
            }
            (Some(PendingDialogAction::PanelizeCommand { .. }), DialogResult::Canceled) => {
                self.set_status("Panelize canceled");
            }
            (
                Some(PendingDialogAction::PanelizePresetAdd { mut presets }),
                DialogResult::InputSubmitted(value),
            ) => {
                let command = value.trim();
                if command.is_empty() {
                    self.set_status("Panelize preset add canceled: empty command");
                    return;
                }
                let command = command.to_string();
                if presets.iter().any(|preset| preset.command == command) {
                    self.set_status("Panelize preset already exists");
                    return;
                }

                let preset = PanelizePreset::from_legacy_command(command.clone());
                let label = preset.label.clone();
                presets.push(preset);
                self.settings.configuration.panelize_presets = presets.clone();
                self.settings.mark_dirty();
                self.routes.pop();
                self.open_panelize_preset_selection_dialog(command, presets);
                self.set_status(format!("Added panelize preset: {label}"));
            }
            (
                Some(PendingDialogAction::PanelizePresetAdd { presets: _ }),
                DialogResult::Canceled,
            ) => {
                self.set_status("Panelize preset add canceled");
            }
            (
                Some(PendingDialogAction::PanelizePresetEdit {
                    mut presets,
                    preset_index,
                }),
                DialogResult::InputSubmitted(value),
            ) => {
                let command = value.trim();
                if command.is_empty() {
                    self.set_status("Panelize preset edit canceled: empty command");
                    return;
                }
                let command = command.to_string();
                let Some(entry) = presets.get_mut(preset_index) else {
                    self.set_status("Panelize preset edit failed: invalid selection");
                    return;
                };
                entry.command = command.clone();
                let label = entry.label.clone();

                self.settings.configuration.panelize_presets = presets.clone();
                self.settings.mark_dirty();
                self.routes.pop();
                self.open_panelize_preset_selection_dialog(command, presets);
                self.set_status(format!("Updated panelize preset: {label}"));
            }
            (Some(PendingDialogAction::PanelizePresetEdit { .. }), DialogResult::Canceled) => {
                self.set_status("Panelize preset edit canceled");
            }
            (
                Some(PendingDialogAction::ViewerSearch { direction }),
                DialogResult::InputSubmitted(value),
            ) => {
                let query = value.trim();
                if query.is_empty() {
                    self.set_status("Search canceled: empty query");
                    return;
                }

                let Some(viewer) = self.active_viewer_mut() else {
                    self.set_status("Viewer is not active");
                    return;
                };

                if let Some(line) = viewer.start_search(query.to_string(), direction) {
                    self.set_status(format!("Search hit at line {}", line.saturating_add(1)));
                } else {
                    self.set_status("Search text not found");
                }
            }
            (Some(PendingDialogAction::ViewerSearch { .. }), DialogResult::Canceled) => {
                self.set_status("Search canceled");
            }
            (Some(PendingDialogAction::ViewerGoto), DialogResult::InputSubmitted(value)) => {
                let value = value.trim();
                if value.is_empty() {
                    self.set_status("Goto canceled: empty target");
                    return;
                }

                let Some(viewer) = self.active_viewer_mut() else {
                    self.set_status("Viewer is not active");
                    return;
                };

                match viewer.goto_input(value) {
                    Ok(line) => self.set_status(format!("Moved to line {line}")),
                    Err(error) => self.set_status(format!("Goto failed: {error}")),
                }
            }
            (Some(PendingDialogAction::ViewerGoto), DialogResult::Canceled) => {
                self.set_status("Goto canceled");
            }
            (_, result) => self.set_status(result.status_line()),
        }
    }

    pub(crate) fn handle_dialog_event(&mut self, event: DialogEvent) {
        let Some(Route::Dialog(dialog)) = self.routes.last_mut() else {
            return;
        };
        let preview_skin = matches!(dialog.action(), Some(PendingDialogAction::SetSkin { .. }))
            && matches!(event, DialogEvent::MoveUp | DialogEvent::MoveDown);
        let transition = dialog.handle_event(event);
        match transition {
            dialog::DialogTransition::Stay => {
                if preview_skin
                    && let DialogKind::Listbox(listbox) = &dialog.kind
                    && let Some(value) = listbox.items.get(listbox.selected)
                {
                    self.pending_skin_preview = Some(value.clone());
                }
            }
            dialog::DialogTransition::Close(result) => {
                self.last_dialog_result = Some(result.clone());
                self.finish_dialog(result);
            }
        }
    }
}

fn overwrite_policy_items() -> Vec<String> {
    vec![
        String::from("Overwrite existing"),
        String::from("Skip existing"),
        String::from("Rename destination"),
    ]
}

fn overwrite_policy_index(policy: OverwritePolicy) -> usize {
    match policy {
        OverwritePolicy::Overwrite => 0,
        OverwritePolicy::Skip => 1,
        OverwritePolicy::Rename => 2,
    }
}

fn overwrite_policy_from_index(index: usize) -> OverwritePolicy {
    match index {
        0 => OverwritePolicy::Overwrite,
        1 => OverwritePolicy::Skip,
        2 => OverwritePolicy::Rename,
        _ => OverwritePolicy::Skip,
    }
}
