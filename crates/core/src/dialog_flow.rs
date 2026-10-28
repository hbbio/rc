use rc_shell::{decode_path_text, encode_path_text};
use std::path::{Path, PathBuf};

use crate::dialog::DialogEvent;
use crate::*;

impl AppState {
    pub(crate) fn push_dialog(&mut self, state: DialogState, action: PendingDialogAction) {
        self.routes
            .push(Route::Dialog(DialogRoute::new(state, action)));
    }

    pub(super) fn apply_dialog_command(&mut self, command: AppCommand) -> CommandOutcome {
        match command {
            AppCommand::Panel(panel, PanelCommand::OpenSftp) => self.open_sftp_dialog(panel),
            AppCommand::OpenEncoding => self.open_encoding_dialog(self.active_panel),
            AppCommand::Panel(panel, PanelCommand::OpenEncoding) => {
                self.open_encoding_dialog(panel)
            }
            AppCommand::CreateLink(kind) => self.open_link_dialog(kind),
            AppCommand::EditSymlink => self.open_edit_symlink_dialog(),
            AppCommand::Chmod => {
                self.open_metadata_dialog(crate::metadata_change::MetadataAction::Chmod)
            }
            AppCommand::Chown => {
                self.open_metadata_dialog(crate::metadata_change::MetadataAction::Chown)
            }
            AppCommand::AdvancedChown => {
                self.open_metadata_dialog(crate::metadata_change::MetadataAction::Advanced)
            }
            AppCommand::OpenConfirmDialog => self.start_rename_dialog(),
            AppCommand::OpenInputDialog => self.start_mkdir_dialog(),
            AppCommand::OpenQuickCd => self.start_quick_cd_dialog(),
            AppCommand::OpenListboxDialog => self.start_overwrite_policy_dialog(),
            AppCommand::OpenSkinDialog => self.start_skin_dialog(),
            AppCommand::OpenListingFormat => {
                self.open_panel_listing_format_dialog(self.active_panel)
            }
            AppCommand::OpenSortOrder => self.open_panel_sort_order_dialog(self.active_panel),
            AppCommand::OpenPanelFilter => self.open_panel_filter_dialog(self.active_panel),
            AppCommand::SelectGroup | AppCommand::UnselectGroup => {
                self.open_group_selection_dialog(
                    self.active_panel,
                    command == AppCommand::SelectGroup,
                    &PanelFilter {
                        pattern: String::from("*"),
                        ..PanelFilter::default()
                    },
                );
            }
            AppCommand::Panel(panel, PanelCommand::OpenListingFormat) => {
                self.open_panel_listing_format_dialog(panel)
            }
            AppCommand::Panel(panel, PanelCommand::OpenSortOrder) => {
                self.open_panel_sort_order_dialog(panel)
            }
            AppCommand::Panel(panel, PanelCommand::OpenFilter) => {
                self.open_panel_filter_dialog(panel)
            }
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
                if !self.toggle_panel_sort_dialog_reverse() && !self.toggle_panelize_dialog_focus()
                {
                    self.handle_dialog_event(DialogEvent::FocusNext);
                }
            }
            AppCommand::DialogBackspace => {
                let quick_cd_changed = self.quick_cd_dialog_is_active();
                self.handle_dialog_event(DialogEvent::Backspace);
                if quick_cd_changed {
                    self.sync_quick_cd_search();
                }
            }
            AppCommand::DialogInputChar(ch) => {
                if !self.accept_user_menu_hotkey(ch)
                    && (ch != ' ' || !self.toggle_panel_sort_dialog_reverse())
                {
                    let quick_cd_changed = self.quick_cd_dialog_is_active();
                    self.handle_dialog_event(DialogEvent::InsertChar(ch));
                    if quick_cd_changed {
                        self.sync_quick_cd_search();
                    }
                }
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
            AppCommand::DialogListboxSelectAt(index) => {
                if let Some(Route::Dialog(dialog)) = self.routes.last_mut()
                    && let DialogKind::Listbox(listbox) = &mut dialog.kind
                    && index < listbox.items.len()
                {
                    listbox.selected = index;
                }
            }
            _ => unreachable!("non-dialog command dispatched to dialog handler: {command:?}"),
        }

        CommandOutcome::Continue
    }

    pub(crate) fn start_copy_dialog(&mut self) {
        self.start_transfer_dialog(TransferKind::Copy);
    }

    fn open_panel_listing_format_dialog(&mut self, panel: ActivePanel) {
        let selected = self.panel_listing_format(panel).index();
        let items = PanelListingFormat::ALL
            .into_iter()
            .map(|format| format.label().to_string())
            .collect();
        self.push_dialog(
            DialogState::listbox("Listing format", items, selected),
            PendingDialogAction::SetPanelListingFormat { panel },
        );
        self.set_status(format!("{} panel: choose listing format", panel.label()));
    }

    fn push_user_listing_format_dialog(&mut self, panel: ActivePanel, value: Option<String>) {
        let value = value.unwrap_or_else(|| {
            self.settings.panel_options.user_listing_formats[panel.index()].clone()
        });
        self.push_dialog(
            DialogState::input(
                "User listing format",
                "Columns: name size mtime perm nlink uid gid inode; optional :width",
                value,
            ),
            PendingDialogAction::SetUserListingFormat { panel },
        );
    }

    fn open_panel_sort_order_dialog(&mut self, panel: ActivePanel) {
        let sort_mode = self.panels[panel.index()].sort_mode;
        let is_archive = self.panels[panel.index()].is_virtual();
        let items = SortField::ALL
            .into_iter()
            .map(|field| {
                if is_archive && field == SortField::Unsorted {
                    String::from("Unsorted (archive order)")
                } else {
                    field.dialog_label().to_string()
                }
            })
            .collect();
        self.push_dialog(
            DialogState::listbox_with_hint(
                "Sort order",
                items,
                sort_mode.field.index(),
                panel_sort_dialog_footer(sort_mode.reverse),
            ),
            PendingDialogAction::SetPanelSortOrder {
                panel,
                reverse: sort_mode.reverse,
            },
        );
        self.set_status(format!("{} panel: choose sort order", panel.label()));
    }

    fn open_panel_filter_dialog(&mut self, panel: ActivePanel) {
        let filter = self.panels[panel.index()].filter().clone();
        self.push_dialog(
            DialogState::filter(&filter),
            PendingDialogAction::SetPanelFilter { panel },
        );
        self.set_status(format!("{} panel: edit listing filter", panel.label()));
    }

    fn toggle_panel_sort_dialog_reverse(&mut self) -> bool {
        let Some(Route::Dialog(dialog)) = self.routes.last_mut() else {
            return false;
        };
        let reverse = match dialog.action_mut() {
            Some(PendingDialogAction::SetPanelSortOrder { reverse, .. }) => {
                *reverse = !*reverse;
                *reverse
            }
            _ => return false,
        };
        if let DialogKind::Listbox(listbox) = &mut dialog.kind {
            listbox.footer_hint = Some(panel_sort_dialog_footer(reverse));
        }
        true
    }

    pub(crate) fn start_move_dialog(&mut self) {
        let sources = self.selected_operation_paths();
        if sources.is_empty() {
            self.set_status("Copy/Move requires a selected or tagged entry");
            return;
        }

        let destination_dir = self.passive_panel().cwd.clone();
        let source_base_dir = self.active_panel().cwd.clone();
        self.start_move_dialog_for_paths(
            sources,
            source_base_dir,
            destination_dir,
            OperationOrigin::Panel(self.active_panel),
        );
    }

    fn start_move_dialog_for_paths(
        &mut self,
        sources: Vec<PathBuf>,
        source_base_dir: PathBuf,
        destination_dir: PathBuf,
        origin: OperationOrigin,
    ) {
        let new_name_uses_shell_pattern = sources.len() > 1;
        let default_name = if new_name_uses_shell_pattern {
            most_common_file_extension_pattern(&sources)
        } else {
            sources
                .first()
                .and_then(|source| source.file_name())
                .map(encode_path_text)
                .unwrap_or_else(|| String::from("*"))
        };
        let dialog = DialogState::pair_input_with_default(
            "Move",
            "Destination directory:",
            encode_path_text(destination_dir.as_os_str()),
            "New name:",
            &default_name,
            &default_name,
        );
        self.push_dialog(
            dialog,
            PendingDialogAction::TransferDestination {
                kind: TransferKind::Move,
                sources,
                source_base_dir,
                new_name_default: Some(default_name),
                new_name_uses_shell_pattern,
                origin,
            },
        );
        self.set_status("Move: choose destination and new name");
    }

    fn start_transfer_dialog(&mut self, kind: TransferKind) {
        let sources = self.selected_operation_paths();
        if sources.is_empty() {
            self.set_status("Copy/Move requires a selected or tagged entry");
            return;
        }

        let destination_dir = self.passive_panel().cwd.clone();
        let source_base_dir = self.active_panel().local_directory().to_path_buf();
        self.start_transfer_dialog_for_paths(
            kind,
            sources,
            source_base_dir,
            destination_dir,
            OperationOrigin::Panel(self.active_panel),
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
                encode_path_text(destination_dir.as_os_str()),
            ),
            PendingDialogAction::TransferDestination {
                kind,
                sources,
                source_base_dir,
                new_name_default: None,
                new_name_uses_shell_pattern: false,
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

        self.start_delete_confirmation_for_targets(
            targets,
            OperationOrigin::Panel(self.active_panel),
        );
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
            .map(encode_path_text)
            .unwrap_or_else(|| entry.name.clone());
        self.push_dialog(
            DialogState::input("Rename/Move", "New name:", current_name),
            PendingDialogAction::RenameEntry { source },
        );
        self.set_status("Rename/Move: enter new name");
    }

    pub(crate) fn start_mkdir_dialog(&mut self) {
        let base_dir = self.active_panel().cwd.clone();
        self.start_mkdir_dialog_at(base_dir, OperationOrigin::Panel(self.active_panel));
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
        if matches!(pending.as_ref(), Some(PendingDialogAction::QuickCd)) {
            self.stop_quick_cd_search();
        }
        match (pending, result) {
            (
                Some(PendingDialogAction::UserMenu(menu)),
                DialogResult::ListboxSubmitted { index, .. },
            ) => {
                if let Some(entry) = index.and_then(|i| menu.entries.get(i)) {
                    self.begin_action(&entry.command, entry.label.clone(), None);
                }
            }
            (
                Some(PendingDialogAction::UserMenuDebug(menu)),
                DialogResult::ListboxSubmitted { .. },
            ) => {
                self.show_user_menu(menu);
            }
            (
                Some(PendingDialogAction::UserMenu(_) | PendingDialogAction::UserMenuDebug(_)),
                DialogResult::Canceled,
            ) => {
                self.set_status("User menu canceled");
            }
            (
                Some(PendingDialogAction::SetFilenameEncoding { panel }),
                DialogResult::ListboxSubmitted { index, .. },
            ) => {
                if let Some(encoding) = index.and_then(|i| FilenameEncoding::ALL.get(i)).copied() {
                    self.set_filename_encoding(panel, encoding);
                }
            }
            (Some(PendingDialogAction::SetFilenameEncoding { .. }), DialogResult::Canceled) => {
                self.set_status("Filename encoding unchanged")
            }
            (
                Some(PendingDialogAction::CreateLinks {
                    kind,
                    sources,
                    base,
                    panel,
                }),
                DialogResult::InputSubmitted(value),
            ) => self.submit_link_dialog(kind, sources, base, panel, value),
            (
                Some(PendingDialogAction::EditSymlinks { paths, panel }),
                DialogResult::InputSubmitted(value),
            ) => self.submit_edit_symlink_dialog(paths, panel, value),
            (
                Some(
                    PendingDialogAction::CreateLinks { .. }
                    | PendingDialogAction::EditSymlinks { .. },
                ),
                DialogResult::Canceled,
            ) => self.set_status("Links unchanged"),
            (
                Some(PendingDialogAction::MetadataChange {
                    action,
                    paths,
                    panel,
                }),
                DialogResult::InputSubmitted(value),
            ) => self.submit_metadata_dialog(action, paths, panel, value, String::new()),
            (
                Some(PendingDialogAction::MetadataChange {
                    action,
                    paths,
                    panel,
                }),
                DialogResult::PairInputSubmitted { first, second },
            ) => self.submit_metadata_dialog(action, paths, panel, first, second),
            (Some(PendingDialogAction::MetadataChange { .. }), DialogResult::Canceled) => {
                self.set_status("Permissions/ownership unchanged")
            }
            (
                Some(PendingDialogAction::SelectScreen { targets }),
                DialogResult::ListboxSubmitted { index, .. },
            ) => {
                if let Some(target) = index.and_then(|i| targets.get(i)).cloned() {
                    self.select_screen(target);
                }
            }
            (Some(PendingDialogAction::SelectScreen { .. }), DialogResult::Canceled) => {
                self.set_status("Screen unchanged");
            }
            (
                Some(PendingDialogAction::LearnKey { chord, original }),
                DialogResult::PairInputSubmitted { first, second },
            ) => self.save_learned_binding(chord, original, first, second),
            (
                Some(PendingDialogAction::FilteredView { cwd }),
                DialogResult::InputSubmitted(command),
            ) => {
                if !command.trim().is_empty() {
                    self.start_command_capture(CommandCaptureSpec::shell(
                        cwd,
                        command,
                        self.settings.shell.clone(),
                    ));
                }
            }
            (
                Some(PendingDialogAction::ActionPrompt(pending)),
                DialogResult::InputSubmitted(answer),
            ) => self.answer_action_prompt(*pending, answer),
            (Some(PendingDialogAction::ActionPrompt(_)), DialogResult::Canceled) => {
                self.set_status("Action canceled")
            }
            (
                Some(PendingDialogAction::CommandHistory { commands }),
                DialogResult::ListboxSubmitted { index, .. },
            ) => {
                if let Some(command) = index.and_then(|index| commands.get(index)) {
                    self.recall_command(command);
                }
            }
            (
                Some(PendingDialogAction::ViewFile { base_dir }),
                DialogResult::InputSubmitted(value),
            ) => {
                self.view_file_input(&base_dir, &value);
            }
            (
                Some(PendingDialogAction::FileHistory { entries }),
                DialogResult::ListboxSubmitted { index, .. },
            ) => {
                if let Some(entry) = index.and_then(|index| entries.get(index)) {
                    self.reopen_history_entry(entry);
                }
            }
            (
                Some(PendingDialogAction::VfsList { mounts }),
                DialogResult::ListboxSubmitted { index, .. },
            ) => {
                if let Some(mount) = index.and_then(|index| mounts.get(index)) {
                    self.inspect_vfs_mount(Arc::clone(mount));
                }
            }
            (
                Some(PendingDialogAction::VfsMount { mount }),
                DialogResult::ListboxSubmitted {
                    index: Some(index), ..
                },
            ) => self.act_on_vfs_mount(mount, index),
            (
                Some(PendingDialogAction::SftpConnection { panel }),
                DialogResult::PairInputSubmitted { first, second },
            ) => self.submit_sftp_connection(panel, &first, &second),
            (None, result) => self.set_status(result.status_line()),
            (
                Some(PendingDialogAction::ViewVfsFile { base }),
                DialogResult::InputSubmitted(value),
            ) => self.view_vfs_file_input(&base, &value),
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
                let input_path = PathBuf::from(decode_path_text(value));
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
                let destination = parent.join(decode_path_text(value));
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
                    new_name_default: _,
                    new_name_uses_shell_pattern: _,
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
            (
                Some(PendingDialogAction::TransferDestination {
                    kind,
                    sources,
                    source_base_dir,
                    new_name_default,
                    new_name_uses_shell_pattern,
                    origin,
                }),
                DialogResult::PairInputSubmitted { first, second },
            ) => {
                if kind != TransferKind::Move {
                    self.set_status("Copy does not support rename input");
                    return;
                }
                let destination_input = first.trim();
                if destination_input.is_empty() {
                    self.set_status("Copy/Move canceled: empty destination");
                    return;
                }
                let destination = parse_destination_path(&source_base_dir, destination_input);
                if destination.as_os_str().is_empty() {
                    self.set_status("Copy/Move canceled: empty destination");
                    return;
                }
                let default_new_name = new_name_default.as_deref().unwrap_or_default();
                let new_name = second.as_str();
                if new_name.is_empty() {
                    self.set_status("Rename canceled: empty name");
                    return;
                }
                if default_new_name == new_name {
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
                                destination_dir: destination,
                                origin,
                            },
                        );
                        self.set_status("Choose overwrite policy");
                    } else {
                        self.queue_copy_or_move_job(
                            kind,
                            sources,
                            destination,
                            self.overwrite_policy(),
                            origin,
                        );
                    }
                    return;
                }

                let rename_destinations = rename_sources(
                    &sources,
                    default_new_name,
                    new_name,
                    new_name_uses_shell_pattern,
                );
                if self.settings.confirmation.confirm_overwrite {
                    let selected = overwrite_policy_index(self.overwrite_policy());
                    self.push_dialog(
                        DialogState::listbox(
                            "Overwrite Policy",
                            overwrite_policy_items(),
                            selected,
                        ),
                        PendingDialogAction::TransferRenameOverwrite {
                            kind,
                            sources,
                            destination_dir: destination,
                            destination_names: rename_destinations,
                            origin,
                        },
                    );
                    self.set_status("Choose overwrite policy");
                } else {
                    self.queue_copy_or_move_job_with_names(
                        kind,
                        sources,
                        destination,
                        rename_destinations,
                        self.overwrite_policy(),
                        origin,
                    );
                }
            }
            (
                Some(PendingDialogAction::TransferRenameOverwrite {
                    kind,
                    sources,
                    destination_dir,
                    destination_names,
                    origin,
                }),
                DialogResult::ListboxSubmitted { index, .. },
            ) => {
                let overwrite = index
                    .map(overwrite_policy_from_index)
                    .unwrap_or(self.overwrite_policy());
                self.queue_copy_or_move_job_with_names(
                    kind,
                    sources,
                    destination_dir,
                    destination_names,
                    overwrite,
                    origin,
                );
            }
            (Some(PendingDialogAction::TransferRenameOverwrite { .. }), DialogResult::Canceled) => {
                self.set_status("Copy/Move canceled");
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
            (
                Some(PendingDialogAction::SetPanelListingFormat { panel }),
                DialogResult::ListboxSubmitted { index, .. },
            ) => {
                if let Some(format) = index.and_then(PanelListingFormat::from_index) {
                    if format == PanelListingFormat::User {
                        self.push_user_listing_format_dialog(panel, None);
                    } else {
                        self.set_panel_listing_format(panel, format);
                    }
                } else {
                    self.set_status("Listing format unchanged");
                }
            }
            (Some(PendingDialogAction::SetPanelListingFormat { .. }), DialogResult::Canceled) => {
                self.set_status("Listing format unchanged");
            }
            (
                Some(PendingDialogAction::SetUserListingFormat { panel }),
                DialogResult::InputSubmitted(value),
            ) => match UserListingFormat::parse(&value) {
                Ok(_) => {
                    self.settings.panel_options.user_listing_formats[panel.index()] = value;
                    self.set_panel_listing_format(panel, PanelListingFormat::User);
                }
                Err(error) => {
                    self.push_user_listing_format_dialog(panel, Some(value));
                    self.set_status(error);
                }
            },
            (Some(PendingDialogAction::SetUserListingFormat { .. }), DialogResult::Canceled) => {
                self.set_status("Listing format unchanged");
            }
            (
                Some(PendingDialogAction::SetPanelSortOrder { panel, reverse }),
                DialogResult::ListboxSubmitted { index, .. },
            ) => {
                if let Some(field) = index.and_then(SortField::from_index) {
                    let sort_mode = SortMode { field, reverse };
                    self.set_panel_sort_mode(panel, sort_mode);
                    self.queue_panel_refresh(panel);
                    self.set_status(format!(
                        "{} panel sort: {}",
                        panel.label(),
                        self.panels[panel.index()].sort_label()
                    ));
                } else {
                    self.set_status("Sort order unchanged");
                }
            }
            (Some(PendingDialogAction::SetPanelSortOrder { .. }), DialogResult::Canceled) => {
                self.set_status("Sort order unchanged");
            }
            (
                Some(PendingDialogAction::SetPanelFilter { panel }),
                DialogResult::FilterSubmitted(filter),
            ) => {
                if let Err(error) = filter.validate() {
                    self.push_dialog(
                        DialogState::filter(&filter),
                        PendingDialogAction::SetPanelFilter { panel },
                    );
                    self.set_status(format!("Filter not applied: {error}"));
                    return;
                }
                if self.panels[panel.index()].filter() == &filter {
                    self.set_status("Filter unchanged");
                    return;
                }

                let affects_listing =
                    self.panels[panel.index()].filter().is_active() || filter.is_active();
                let label = filter.display_pattern().to_string();
                self.set_panel_filter(panel, filter);
                if affects_listing {
                    self.queue_panel_refresh(panel);
                }
                self.set_status(format!("{} panel filter: {label}", panel.label()));
            }
            (Some(PendingDialogAction::SetPanelFilter { .. }), DialogResult::Canceled) => {
                self.set_status("Filter unchanged");
            }
            (
                Some(PendingDialogAction::SetGroupSelection { panel, select }),
                DialogResult::FilterSubmitted(filter),
            ) => self.apply_group_selection(panel, select, filter),
            (Some(PendingDialogAction::SetGroupSelection { .. }), DialogResult::Canceled) => {
                self.set_status("Selection unchanged");
            }
            (Some(PendingDialogAction::FindSearch), DialogResult::FindSubmitted(spec)) => {
                self.start_find_search(*spec);
            }
            (Some(PendingDialogAction::FindSearch), DialogResult::Canceled) => {
                self.set_status("Find canceled");
            }
            (
                Some(PendingDialogAction::QuickCd),
                DialogResult::QuickCdSubmitted {
                    input,
                    selected_path,
                },
            ) => {
                self.submit_quick_cd(input, selected_path);
            }
            (Some(PendingDialogAction::QuickCd), DialogResult::Canceled) => {
                self.set_status("Quick cd canceled");
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
                Some(PendingDialogAction::PanelizePresetAdd {
                    initial_command,
                    presets,
                }),
                DialogResult::PairInputSubmitted { first, second },
            ) => self.submit_panelize_preset_add(initial_command, presets, first, second),
            (Some(PendingDialogAction::PanelizePresetAdd { .. }), DialogResult::Canceled) => {
                self.set_status("Panelize preset add canceled");
            }
            (
                Some(PendingDialogAction::PanelizePresetEdit {
                    initial_command,
                    presets,
                    preset_index,
                }),
                DialogResult::PairInputSubmitted { first, second },
            ) => self.submit_panelize_preset_edit(
                initial_command,
                presets,
                preset_index,
                first,
                second,
            ),
            (Some(PendingDialogAction::PanelizePresetEdit { .. }), DialogResult::Canceled) => {
                self.set_status("Panelize preset edit canceled");
            }
            (
                Some(PendingDialogAction::PanelizePresetRemove {
                    initial_command,
                    presets,
                    preset_index,
                }),
                DialogResult::ConfirmAccepted,
            ) => self.confirm_panelize_preset_remove(initial_command, presets, preset_index),
            (
                Some(PendingDialogAction::PanelizePresetRemove { .. }),
                DialogResult::ConfirmDeclined,
            )
            | (Some(PendingDialogAction::PanelizePresetRemove { .. }), DialogResult::Canceled) => {
                self.set_status("Panelize preset removal canceled");
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

    fn quick_cd_dialog_is_active(&self) -> bool {
        matches!(
            self.routes.last(),
            Some(Route::Dialog(dialog))
                if matches!(dialog.action(), Some(PendingDialogAction::QuickCd))
                    && matches!(dialog.kind, DialogKind::QuickCd(_))
        )
    }
}

fn panel_sort_dialog_footer(reverse: bool) -> String {
    format!(
        "Reverse: {} | Space/Tab toggle | Enter apply | Esc cancel",
        if reverse { "on" } else { "off" }
    )
}

fn overwrite_policy_items() -> Vec<String> {
    vec![
        String::from("Overwrite existing"),
        String::from("Skip existing"),
        String::from("Rename destination"),
    ]
}

fn parse_destination_path(base_dir: &Path, value: &str) -> PathBuf {
    let input_path = PathBuf::from(decode_path_text(value));
    if input_path.is_absolute() {
        input_path
    } else {
        base_dir.join(input_path)
    }
}

fn most_common_file_extension_pattern(sources: &[PathBuf]) -> String {
    let mut common_extension: Option<String> = None;
    for source in sources {
        let Some(name) = source.file_name() else {
            return String::from("*");
        };
        let extension = Path::new(name)
            .extension()
            .map(encode_path_text)
            .unwrap_or_default();
        if extension.is_empty() || extension.contains('*') {
            return String::from("*");
        }
        if let Some(current) = common_extension.as_ref() {
            if current != &extension {
                return String::from("*");
            }
        } else {
            common_extension = Some(extension);
        }
    }

    let extension = common_extension.unwrap_or_default();
    if extension.is_empty() {
        String::from("*")
    } else {
        format!("*.{extension}")
    }
}

fn rename_sources(
    sources: &[PathBuf],
    old_pattern: &str,
    new_pattern: &str,
    use_shell_patterns: bool,
) -> Vec<std::ffi::OsString> {
    let old_pattern = decode_path_text(old_pattern);
    let new_pattern = decode_path_text(new_pattern);
    sources
        .iter()
        .map(|source| {
            let source_name = source.file_name().unwrap_or(source.as_os_str());
            rename_source_name(source_name, &old_pattern, &new_pattern, use_shell_patterns)
        })
        .collect()
}

fn rename_source_name(
    source_name: &std::ffi::OsStr,
    old_pattern: &std::ffi::OsStr,
    new_pattern: &std::ffi::OsStr,
    use_shell_patterns: bool,
) -> std::ffi::OsString {
    if use_shell_patterns {
        if let Some(capture) = wildcard_capture(
            source_name.as_encoded_bytes(),
            old_pattern.as_encoded_bytes(),
        ) {
            if new_pattern.as_encoded_bytes().contains(&b'*') {
                let mut result = Vec::new();
                for (index, part) in new_pattern
                    .as_encoded_bytes()
                    .split(|byte| *byte == b'*')
                    .enumerate()
                {
                    if index != 0 {
                        result.extend_from_slice(capture);
                    }
                    result.extend_from_slice(part);
                }
                return crate::action_macros::os_from_bytes(&result)
                    .unwrap_or_else(|_| source_name.to_owned());
            }
            return new_pattern.to_owned();
        }
        return source_name.to_owned();
    }
    if source_name == old_pattern {
        new_pattern.to_owned()
    } else {
        source_name.to_owned()
    }
}

fn wildcard_capture<'a>(haystack: &'a [u8], pattern: &[u8]) -> Option<&'a [u8]> {
    let parts: Vec<_> = pattern.split(|byte| *byte == b'*').collect();
    if parts.len() > 2 {
        return None;
    }
    if parts.len() == 1 {
        if haystack == pattern {
            return Some(&[]);
        }
        return None;
    }

    let (head, tail) = (parts[0], parts[1]);
    if !haystack.starts_with(head) || !haystack.ends_with(tail) {
        return None;
    }
    let middle_start = head.len();
    let middle_end = haystack.len().saturating_sub(tail.len());
    if middle_end < middle_start {
        return None;
    }
    Some(&haystack[middle_start..middle_end])
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

#[cfg(test)]
mod rename_tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn rename_masks_preserve_native_bytes_and_literal_backslashes() {
        use std::os::unix::ffi::OsStringExt;
        let source = PathBuf::from(std::ffi::OsString::from_vec(b"raw\xff\\xFF.rs".to_vec()));
        let names = rename_sources(std::slice::from_ref(&source), "*.rs", "*.txt", true);
        assert_eq!(
            names,
            vec![std::ffi::OsString::from_vec(b"raw\xff\\xFF.txt".to_vec())]
        );
        let old = encode_path_text(source.as_os_str());
        let renamed = rename_sources(&[source], &old, r"new\xFE.txt", false);
        assert_eq!(
            renamed,
            vec![std::ffi::OsString::from_vec(b"new\xfe.txt".to_vec())]
        );
        let sources = [PathBuf::from(r"a.\x41"), PathBuf::from(r"b.\x41")];
        let mask = most_common_file_extension_pattern(&sources);
        assert_eq!(mask, r"*.\\x41");
        assert_eq!(
            rename_sources(&sources, &mask, "*.bak", true),
            vec!["a.bak", "b.bak"]
        );
    }

    #[test]
    fn wildcard_extensions_use_a_valid_catch_all_pattern() {
        let sources = [PathBuf::from("first.*"), PathBuf::from("second.*")];
        let pattern = most_common_file_extension_pattern(&sources);

        assert_eq!(pattern, "*");
        assert_eq!(
            rename_sources(&sources, &pattern, "*.bak", true),
            vec!["first.*.bak", "second.*.bak"]
        );
    }
}
