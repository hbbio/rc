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
                    "Editor command",
                    self.settings
                        .configuration
                        .editor_command
                        .clone()
                        .unwrap_or_else(|| String::from("<auto>")),
                    SettingsEntryAction::Info,
                ),
                SettingsEntry::new(
                    "Default overwrite policy",
                    self.overwrite_policy().label(),
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
                    self.hotlist().len().to_string(),
                    SettingsEntryAction::Info,
                ),
                SettingsEntry::new(
                    "Panelize presets",
                    self.panelize_presets().len().to_string(),
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
                    "Left listing format",
                    self.settings.panel_options.listing_formats[ActivePanel::Left.index()].label(),
                    SettingsEntryAction::Info,
                ),
                SettingsEntry::new(
                    "Right listing format",
                    self.settings.panel_options.listing_formats[ActivePanel::Right.index()].label(),
                    SettingsEntryAction::Info,
                ),
                SettingsEntry::new(
                    "Show hidden files",
                    bool_label(self.settings.panel_options.show_hidden_files),
                    SettingsEntryAction::TogglePanelShowHiddenFiles,
                ),
                SettingsEntry::new(
                    "Left sort field",
                    self.settings.panel_options.sort_modes[ActivePanel::Left.index()]
                        .field
                        .label(),
                    SettingsEntryAction::CyclePanelSortField(ActivePanel::Left),
                ),
                SettingsEntry::new(
                    "Left sort reverse",
                    bool_label(
                        self.settings.panel_options.sort_modes[ActivePanel::Left.index()].reverse,
                    ),
                    SettingsEntryAction::TogglePanelSortReverse(ActivePanel::Left),
                ),
                SettingsEntry::new(
                    "Right sort field",
                    self.settings.panel_options.sort_modes[ActivePanel::Right.index()]
                        .field
                        .label(),
                    SettingsEntryAction::CyclePanelSortField(ActivePanel::Right),
                ),
                SettingsEntry::new(
                    "Right sort reverse",
                    bool_label(
                        self.settings.panel_options.sort_modes[ActivePanel::Right.index()].reverse,
                    ),
                    SettingsEntryAction::TogglePanelSortReverse(ActivePanel::Right),
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
                SettingsEntry::new(
                    "Confirm hotlist deletion",
                    bool_label(self.settings.confirmation.confirm_hotlist_delete),
                    SettingsEntryAction::ToggleConfirmHotlistDelete,
                ),
            ],
            SettingsCategory::Appearance => vec![
                SettingsEntry::new(
                    "Skin...",
                    self.active_skin_name().to_string(),
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
            SettingsEntryAction::CycleDefaultOverwritePolicy => {
                let policy = next_overwrite_policy(self.overwrite_policy());
                self.set_overwrite_policy(policy);
                self.settings.mark_dirty();
                self.set_status(format!(
                    "Default overwrite policy: {}",
                    self.overwrite_policy().label()
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
            SettingsEntryAction::CyclePanelSortField(panel) => {
                let mut sort_mode = self.settings.panel_options.sort_modes[panel.index()];
                sort_mode.field = sort_mode.field.next();
                self.set_panel_sort_mode(panel, sort_mode);
                self.queue_panel_refresh(panel);
                self.set_status(format!(
                    "{} panel sort: {}",
                    panel.label(),
                    sort_mode.field.label()
                ));
            }
            SettingsEntryAction::TogglePanelSortReverse(panel) => {
                let mut sort_mode = self.settings.panel_options.sort_modes[panel.index()];
                sort_mode.reverse = !sort_mode.reverse;
                self.set_panel_sort_mode(panel, sort_mode);
                self.queue_panel_refresh(panel);
                self.set_status(format!(
                    "{} panel sort reverse: {}",
                    panel.label(),
                    bool_label(sort_mode.reverse)
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
            SettingsEntryAction::ToggleConfirmHotlistDelete => {
                self.settings.confirmation.confirm_hotlist_delete =
                    !self.settings.confirmation.confirm_hotlist_delete;
                self.settings.mark_dirty();
                self.set_status(format!(
                    "Confirm hotlist deletion: {}",
                    bool_label(self.settings.confirmation.confirm_hotlist_delete)
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
