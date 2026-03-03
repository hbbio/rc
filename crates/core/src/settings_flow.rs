use crate::*;

impl AppState {
    pub(super) fn apply_settings_command(&mut self, command: AppCommand) -> CommandOutcome {
        match command {
            AppCommand::OpenOptionsConfiguration => {
                self.open_settings_screen(SettingsCategory::Configuration)
            }
            AppCommand::OpenOptionsLayout => self.open_settings_screen(SettingsCategory::Layout),
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
            _ => unreachable!("non-settings command dispatched to settings handler: {command:?}"),
        }

        CommandOutcome::Continue
    }

    pub(crate) fn open_settings_screen(&mut self, category: SettingsCategory) {
        self.pending_learn_keys_capture = false;
        let next = SettingsScreenState::new(category, self.settings_entries_for_category(category));
        if let Some(Route::Settings(current)) = self.routes.last_mut() {
            *current = next;
        } else {
            self.routes.push(Route::Settings(next));
        }
        self.set_status(format!("Options: {}", category.label()));
    }

    pub(crate) fn close_settings_screen(&mut self) {
        if matches!(self.top_route(), Route::Settings(_)) {
            self.pending_learn_keys_capture = false;
            self.routes.pop();
            self.set_status("Closed options");
        }
    }

    pub(crate) fn settings_state_mut(&mut self) -> Option<&mut SettingsScreenState> {
        let Some(Route::Settings(settings)) = self.routes.last_mut() else {
            return None;
        };
        Some(settings)
    }

    fn settings_entries_for_category(&self, category: SettingsCategory) -> Vec<SettingsEntry> {
        match category {
            SettingsCategory::Configuration => vec![
                SettingsEntry::new(
                    "Use internal editor",
                    bool_label(self.settings.configuration.use_internal_editor),
                    SettingsEntryAction::ToggleUseInternalEditor,
                ),
                SettingsEntry::new(
                    "Default overwrite policy",
                    self.overwrite_policy.label(),
                    SettingsEntryAction::CycleDefaultOverwritePolicy,
                ),
                SettingsEntry::new(
                    "macOS Option-symbol compatibility",
                    bool_label(self.settings.configuration.macos_option_symbols),
                    SettingsEntryAction::ToggleMacosOptionSymbols,
                ),
                SettingsEntry::new(
                    "Keymap override",
                    self.settings
                        .configuration
                        .keymap_override
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_else(|| String::from("<none>")),
                    SettingsEntryAction::Info,
                ),
                SettingsEntry::new(
                    "Hotlist entries",
                    self.hotlist.len().to_string(),
                    SettingsEntryAction::Info,
                ),
                SettingsEntry::new(
                    "Panelize presets",
                    self.panelize_presets.len().to_string(),
                    SettingsEntryAction::Info,
                ),
            ],
            SettingsCategory::Layout => vec![
                SettingsEntry::new(
                    "Show menu bar",
                    bool_label(self.settings.layout.show_menu_bar),
                    SettingsEntryAction::ToggleLayoutShowMenuBar,
                ),
                SettingsEntry::new(
                    "Show button bar",
                    bool_label(self.settings.layout.show_button_bar),
                    SettingsEntryAction::ToggleLayoutShowButtonBar,
                ),
                SettingsEntry::new(
                    "Show debug status",
                    bool_label(self.settings.layout.show_debug_status),
                    SettingsEntryAction::ToggleLayoutShowDebugStatus,
                ),
                SettingsEntry::new(
                    "Show panel totals",
                    bool_label(self.settings.layout.show_panel_totals),
                    SettingsEntryAction::ToggleLayoutShowPanelTotals,
                ),
                SettingsEntry::new(
                    "Status message timeout",
                    status_message_timeout_label(
                        self.settings.layout.status_message_timeout_seconds,
                    ),
                    SettingsEntryAction::CycleLayoutStatusMessageTimeout,
                ),
            ],
            SettingsCategory::PanelOptions => vec![
                SettingsEntry::new(
                    "Show hidden files",
                    bool_label(self.settings.panel_options.show_hidden_files),
                    SettingsEntryAction::TogglePanelShowHiddenFiles,
                ),
                SettingsEntry::new(
                    "Default sort field",
                    match self.settings.panel_options.sort_field {
                        SettingsSortField::Name => "name",
                        SettingsSortField::Size => "size",
                        SettingsSortField::Modified => "mtime",
                    },
                    SettingsEntryAction::CyclePanelSortField,
                ),
                SettingsEntry::new(
                    "Default sort reverse",
                    bool_label(self.settings.panel_options.sort_reverse),
                    SettingsEntryAction::TogglePanelSortReverse,
                ),
            ],
            SettingsCategory::Confirmation => vec![
                SettingsEntry::new(
                    "Confirm delete",
                    bool_label(self.settings.confirmation.confirm_delete),
                    SettingsEntryAction::ToggleConfirmDelete,
                ),
                SettingsEntry::new(
                    "Confirm overwrite",
                    bool_label(self.settings.confirmation.confirm_overwrite),
                    SettingsEntryAction::ToggleConfirmOverwrite,
                ),
                SettingsEntry::new(
                    "Confirm quit",
                    bool_label(self.settings.confirmation.confirm_quit),
                    SettingsEntryAction::ToggleConfirmQuit,
                ),
            ],
            SettingsCategory::Appearance => vec![
                SettingsEntry::new(
                    "Skin...",
                    self.active_skin_name.clone(),
                    SettingsEntryAction::OpenSkinDialog,
                ),
                SettingsEntry::new(
                    "Available skins",
                    self.available_skins.len().to_string(),
                    SettingsEntryAction::Info,
                ),
                SettingsEntry::new(
                    "Custom skin directories",
                    self.settings.appearance.skin_dirs.len().to_string(),
                    SettingsEntryAction::Info,
                ),
            ],
            SettingsCategory::DisplayBits => vec![
                SettingsEntry::new(
                    "UTF-8 output",
                    bool_label(self.settings.display_bits.utf8_output),
                    SettingsEntryAction::ToggleUtf8Output,
                ),
                SettingsEntry::new(
                    "8-bit input",
                    bool_label(self.settings.display_bits.eight_bit_input),
                    SettingsEntryAction::ToggleEightBitInput,
                ),
            ],
            SettingsCategory::LearnKeys => vec![
                SettingsEntry::new(
                    "Last learned binding",
                    self.settings
                        .learn_keys
                        .last_learned_binding
                        .clone()
                        .unwrap_or_else(|| String::from("<none>")),
                    SettingsEntryAction::Info,
                ),
                SettingsEntry::new(
                    "Override target",
                    self.settings
                        .configuration
                        .keymap_override
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_else(|| String::from("<none>")),
                    SettingsEntryAction::Info,
                ),
                SettingsEntry::new(
                    "Unknown keymap actions",
                    self.keymap_unknown_actions.to_string(),
                    SettingsEntryAction::Info,
                ),
                SettingsEntry::new(
                    "Invalid key bindings",
                    self.keymap_invalid_bindings.to_string(),
                    SettingsEntryAction::Info,
                ),
                SettingsEntry::new(
                    "Capture binding (scaffold)",
                    "",
                    SettingsEntryAction::LearnKeysCapture,
                ),
            ],
            SettingsCategory::VirtualFs => vec![
                SettingsEntry::new(
                    "Enable virtual FS",
                    bool_label(self.settings.virtual_fs.vfs_enabled),
                    SettingsEntryAction::ToggleVfsEnabled,
                ),
                SettingsEntry::new(
                    "Enable FTP links",
                    bool_label(self.settings.virtual_fs.ftp_enabled),
                    SettingsEntryAction::ToggleVfsFtpEnabled,
                ),
                SettingsEntry::new(
                    "Enable shell links",
                    bool_label(self.settings.virtual_fs.shell_link_enabled),
                    SettingsEntryAction::ToggleVfsShellLinkEnabled,
                ),
                SettingsEntry::new(
                    "Enable SFTP links",
                    bool_label(self.settings.virtual_fs.sftp_enabled),
                    SettingsEntryAction::ToggleVfsSftpEnabled,
                ),
            ],
        }
    }

    pub(crate) fn refresh_settings_entries(&mut self) {
        let Some((category, selected)) = self.routes.last().and_then(|route| match route {
            Route::Settings(current) => Some((current.category, current.selected_entry)),
            _ => None,
        }) else {
            return;
        };
        let entries = self.settings_entries_for_category(category);
        if let Some(Route::Settings(current)) = self.routes.last_mut() {
            current.entries = entries;
            if current.entries.is_empty() {
                current.selected_entry = 0;
            } else {
                current.selected_entry = selected.min(current.entries.len().saturating_sub(1));
            }
        }
    }

    pub(crate) fn apply_settings_entry(&mut self) {
        let Some(route) = self.routes.last() else {
            return;
        };
        let Route::Settings(settings) = route else {
            return;
        };
        let Some(entry) = settings.entries.get(settings.selected_entry).cloned() else {
            return;
        };

        match entry.action {
            SettingsEntryAction::ToggleUseInternalEditor => {
                self.settings.configuration.use_internal_editor =
                    !self.settings.configuration.use_internal_editor;
                self.settings.mark_dirty();
                self.set_status(format!(
                    "Use internal editor: {}",
                    bool_label(self.settings.configuration.use_internal_editor)
                ));
            }
            SettingsEntryAction::CycleDefaultOverwritePolicy => {
                self.overwrite_policy = next_overwrite_policy(self.overwrite_policy);
                self.settings.configuration.default_overwrite_policy = self.overwrite_policy;
                self.settings.mark_dirty();
                self.set_status(format!(
                    "Default overwrite policy: {}",
                    self.overwrite_policy.label()
                ));
            }
            SettingsEntryAction::ToggleMacosOptionSymbols => {
                self.settings.configuration.macos_option_symbols =
                    !self.settings.configuration.macos_option_symbols;
                self.settings.mark_dirty();
                self.set_status(format!(
                    "macOS Option-symbol compatibility: {}",
                    bool_label(self.settings.configuration.macos_option_symbols)
                ));
            }
            SettingsEntryAction::ToggleLayoutShowMenuBar => {
                self.settings.layout.show_menu_bar = !self.settings.layout.show_menu_bar;
                self.settings.mark_dirty();
                self.set_status(format!(
                    "Show menu bar: {}",
                    bool_label(self.settings.layout.show_menu_bar)
                ));
            }
            SettingsEntryAction::ToggleLayoutShowButtonBar => {
                self.settings.layout.show_button_bar = !self.settings.layout.show_button_bar;
                self.settings.mark_dirty();
                self.set_status(format!(
                    "Show button bar: {}",
                    bool_label(self.settings.layout.show_button_bar)
                ));
            }
            SettingsEntryAction::ToggleLayoutShowDebugStatus => {
                self.settings.layout.show_debug_status = !self.settings.layout.show_debug_status;
                self.settings.mark_dirty();
                self.set_status(format!(
                    "Show debug status: {}",
                    bool_label(self.settings.layout.show_debug_status)
                ));
            }
            SettingsEntryAction::ToggleLayoutShowPanelTotals => {
                self.settings.layout.show_panel_totals = !self.settings.layout.show_panel_totals;
                self.settings.mark_dirty();
                self.set_status(format!(
                    "Show panel totals: {}",
                    bool_label(self.settings.layout.show_panel_totals)
                ));
            }
            SettingsEntryAction::CycleLayoutStatusMessageTimeout => {
                let next = next_status_message_timeout_seconds(
                    self.settings.layout.status_message_timeout_seconds,
                );
                self.settings.layout.status_message_timeout_seconds = next;
                self.settings.mark_dirty();
                self.set_status(format!(
                    "Status message timeout: {}",
                    status_message_timeout_label(next)
                ));
            }
            SettingsEntryAction::TogglePanelShowHiddenFiles => {
                self.settings.panel_options.show_hidden_files =
                    !self.settings.panel_options.show_hidden_files;
                let show_hidden_files = self.settings.panel_options.show_hidden_files;
                for panel in &mut self.panels {
                    panel.set_show_hidden_files(show_hidden_files);
                }
                self.settings.mark_dirty();
                self.refresh_panels();
                self.set_status(format!(
                    "Show hidden files: {}",
                    bool_label(show_hidden_files)
                ));
            }
            SettingsEntryAction::CyclePanelSortField => {
                self.settings.panel_options.sort_field =
                    next_settings_sort_field(self.settings.panel_options.sort_field);
                let sort_mode = self.default_panel_sort_mode();
                for panel in &mut self.panels {
                    panel.sort_mode = sort_mode;
                }
                self.settings.mark_dirty();
                self.refresh_panels();
                self.set_status(format!("Default sort: {}", sort_mode.field.label()));
            }
            SettingsEntryAction::TogglePanelSortReverse => {
                self.settings.panel_options.sort_reverse =
                    !self.settings.panel_options.sort_reverse;
                let sort_mode = self.default_panel_sort_mode();
                for panel in &mut self.panels {
                    panel.sort_mode = sort_mode;
                }
                self.settings.mark_dirty();
                self.refresh_panels();
                self.set_status(format!(
                    "Default sort reverse: {}",
                    bool_label(self.settings.panel_options.sort_reverse)
                ));
            }
            SettingsEntryAction::ToggleConfirmDelete => {
                self.settings.confirmation.confirm_delete =
                    !self.settings.confirmation.confirm_delete;
                self.settings.mark_dirty();
                self.set_status(format!(
                    "Confirm delete: {}",
                    bool_label(self.settings.confirmation.confirm_delete)
                ));
            }
            SettingsEntryAction::ToggleConfirmOverwrite => {
                self.settings.confirmation.confirm_overwrite =
                    !self.settings.confirmation.confirm_overwrite;
                self.settings.mark_dirty();
                self.set_status(format!(
                    "Confirm overwrite: {}",
                    bool_label(self.settings.confirmation.confirm_overwrite)
                ));
            }
            SettingsEntryAction::ToggleConfirmQuit => {
                self.settings.confirmation.confirm_quit = !self.settings.confirmation.confirm_quit;
                self.settings.mark_dirty();
                self.set_status(format!(
                    "Confirm quit: {}",
                    bool_label(self.settings.confirmation.confirm_quit)
                ));
            }
            SettingsEntryAction::OpenSkinDialog => self.start_skin_dialog(),
            SettingsEntryAction::ToggleUtf8Output => {
                self.settings.display_bits.utf8_output = !self.settings.display_bits.utf8_output;
                self.settings.mark_dirty();
                self.set_status(format!(
                    "UTF-8 output: {}",
                    bool_label(self.settings.display_bits.utf8_output)
                ));
            }
            SettingsEntryAction::ToggleEightBitInput => {
                self.settings.display_bits.eight_bit_input =
                    !self.settings.display_bits.eight_bit_input;
                self.settings.mark_dirty();
                self.set_status(format!(
                    "8-bit input: {}",
                    bool_label(self.settings.display_bits.eight_bit_input)
                ));
            }
            SettingsEntryAction::LearnKeysCapture => {
                self.pending_learn_keys_capture = true;
                self.set_status("Press a key chord to capture (Esc to cancel)");
            }
            SettingsEntryAction::ToggleVfsEnabled => {
                self.settings.virtual_fs.vfs_enabled = !self.settings.virtual_fs.vfs_enabled;
                self.settings.mark_dirty();
                self.set_status(format!(
                    "Enable virtual FS: {}",
                    bool_label(self.settings.virtual_fs.vfs_enabled)
                ));
            }
            SettingsEntryAction::ToggleVfsFtpEnabled => {
                self.settings.virtual_fs.ftp_enabled = !self.settings.virtual_fs.ftp_enabled;
                self.settings.mark_dirty();
                self.set_status(format!(
                    "Enable FTP links: {}",
                    bool_label(self.settings.virtual_fs.ftp_enabled)
                ));
            }
            SettingsEntryAction::ToggleVfsShellLinkEnabled => {
                self.settings.virtual_fs.shell_link_enabled =
                    !self.settings.virtual_fs.shell_link_enabled;
                self.settings.mark_dirty();
                self.set_status(format!(
                    "Enable shell links: {}",
                    bool_label(self.settings.virtual_fs.shell_link_enabled)
                ));
            }
            SettingsEntryAction::ToggleVfsSftpEnabled => {
                self.settings.virtual_fs.sftp_enabled = !self.settings.virtual_fs.sftp_enabled;
                self.settings.mark_dirty();
                self.set_status(format!(
                    "Enable SFTP links: {}",
                    bool_label(self.settings.virtual_fs.sftp_enabled)
                ));
            }
            SettingsEntryAction::Info => {
                self.set_status(format!("{}: {}", entry.label, entry.value));
            }
        }

        self.refresh_settings_entries();
    }
}

fn next_overwrite_policy(policy: OverwritePolicy) -> OverwritePolicy {
    match policy {
        OverwritePolicy::Overwrite => OverwritePolicy::Skip,
        OverwritePolicy::Skip => OverwritePolicy::Rename,
        OverwritePolicy::Rename => OverwritePolicy::Overwrite,
    }
}

fn next_settings_sort_field(field: SettingsSortField) -> SettingsSortField {
    match field {
        SettingsSortField::Name => SettingsSortField::Size,
        SettingsSortField::Size => SettingsSortField::Modified,
        SettingsSortField::Modified => SettingsSortField::Name,
    }
}

fn bool_label(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

fn status_message_timeout_label(seconds: u64) -> String {
    if seconds == 0 {
        String::from("off")
    } else {
        format!("{seconds}s")
    }
}

fn next_status_message_timeout_seconds(current: u64) -> u64 {
    const PRESETS: [u64; 6] = [0, 5, 10, 15, 30, 60];
    for preset in PRESETS {
        if preset > current {
            return preset;
        }
    }
    PRESETS[0]
}
