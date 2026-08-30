use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use crate::*;

impl AppState {
    pub fn new(start_path: PathBuf) -> io::Result<Self> {
        let start_path = std::path::absolute(start_path)?;
        let settings = Settings::default();
        let home_directory = current_user_home_directory();
        let mut left = PanelState::new(start_path.clone())?;
        left.set_home_directory(home_directory.clone());
        let mut right = PanelState::new(start_path)?;
        right.set_home_directory(home_directory);

        Ok(Self {
            settings: settings.clone(),
            panels: [left, right],
            active_panel: ActivePanel::Left,
            panel_views: [PanelViewMode::Listing; 2],
            panel_listing_formats: [PanelListingFormat::Full; 2],
            quick_views: std::array::from_fn(|_| QuickViewState::default()),
            selection_sizes: std::array::from_fn(|_| SelectionSizeState::default()),
            status_line: String::from("Press F1 for help"),
            status_expires_at: None,
            status_message_generation: 0,
            last_dialog_result: None,
            jobs: JobManager::new(),
            jobs_cursor: 0,
            hotlist_cursor: 0,
            available_skins: Vec::new(),
            preview_skin_name: None,
            pending_skin_change: None,
            pending_skin_preview: None,
            pending_skin_revert: None,
            routes: vec![Route::FileManager],
            command_line: CommandLineModel::new(settings.shell.history),
            next_command_line_activation_id: 1,
            #[cfg(unix)]
            next_shell_resolution_request_id: 1,
            pending_shell_resolution: None,
            pending_shell_resolution_request: None,
            next_completion_request_id: 1,
            pending_completion_requests: Vec::new(),
            pending_completion_cancellations: Vec::new(),
            pending_foreground_shell_requests: Vec::new(),
            paused_find_results: None,
            pending_find_tree_picker: None,
            pending_worker_commands: Vec::new(),
            pending_external_edit_requests: Vec::new(),
            pending_external_execute_requests: Vec::new(),
            pending_clipboard_copy_requests: Vec::new(),
            panelized_result_history: [None, None],
            previous_panel_directories: [None, None],
            quick_cd_search: QuickCdSearchWorkflow::default(),
            panel_refresh: PanelRefreshWorkflow::default(),
            panel_refresh_post: PanelRefreshPostWorkflow::default(),
            quick_view: QuickViewWorkflow::default(),
            selection_size: SelectionSizeWorkflow::default(),
            find_pause_flags: HashMap::new(),
            deferred_persist_settings_request: None,
            panel_mkdirs: PanelMkdirTracker::default(),
            tree_mutations: TreeMutationTracker::default(),
            keybinding_hints: KeybindingHints::default(),
            keymap_unknown_actions: 0,
            keymap_invalid_bindings: 0,
            pending_learn_keys_capture: false,
            xmap_pending: false,
            pending_save_setup: false,
            pending_quit: false,
        })
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn settings_mut(&mut self) -> &mut Settings {
        &mut self.settings
    }

    pub fn persisted_settings_snapshot(&self) -> Settings {
        self.settings.clone()
    }

    pub fn mark_settings_saved(&mut self, saved_at: SystemTime) {
        self.settings.mark_saved(saved_at);
    }

    pub fn mark_settings_dirty(&mut self) {
        self.settings.mark_dirty();
    }

    pub fn show_menu_bar(&self) -> bool {
        self.settings.layout.show_menu_bar
    }

    pub fn show_button_bar(&self) -> bool {
        self.settings.layout.show_button_bar
    }

    pub fn show_debug_status(&self) -> bool {
        self.settings.layout.show_debug_status
    }

    pub fn show_panel_totals(&self) -> bool {
        self.settings.layout.show_panel_totals
    }

    fn status_message_timeout(&self) -> Option<Duration> {
        let seconds = self.settings.layout.status_message_timeout_seconds;
        if seconds == 0 {
            None
        } else {
            Some(Duration::from_secs(seconds))
        }
    }

    pub fn jobs_dialog_size(&self) -> (u16, u16) {
        (
            self.settings.layout.jobs_dialog_width,
            self.settings.layout.jobs_dialog_height,
        )
    }

    pub fn help_dialog_size(&self) -> (u16, u16) {
        (
            self.settings.layout.help_dialog_width,
            self.settings.layout.help_dialog_height,
        )
    }

    pub fn replace_settings(&mut self, settings: Settings) {
        self.settings = settings;
        self.command_line
            .set_history_mode(self.settings.shell.history);
        self.hotlist_cursor = self
            .hotlist_cursor
            .min(self.settings.configuration.hotlist.len().saturating_sub(1));
        self.preview_skin_name = None;
        self.status_expires_at = self
            .status_message_timeout()
            .and_then(|timeout| Instant::now().checked_add(timeout))
            .filter(|_| !self.status_line.is_empty());

        let show_hidden_files = self.settings.panel_options.show_hidden_files;
        self.panel_listing_formats = self.settings.panel_options.listing_formats;
        for (index, panel) in self.panels.iter_mut().enumerate() {
            panel.sort_mode = self.settings.panel_options.sort_modes[index];
            panel.filter = self.settings.panel_options.filters[index].clone();
            panel.set_show_hidden_files(show_hidden_files);
        }
        self.sync_selection_size(ActivePanel::Left, true);
        self.sync_selection_size(ActivePanel::Right, true);
    }

    pub(crate) fn set_panel_sort_mode(&mut self, panel: ActivePanel, sort_mode: SortMode) {
        self.panels[panel.index()].sort_mode = sort_mode;
        self.settings.panel_options.sort_modes[panel.index()] = sort_mode;
        self.settings.mark_dirty();
    }

    pub(crate) fn set_panel_filter(&mut self, panel: ActivePanel, filter: PanelFilter) {
        let panel_index = panel.index();
        let panel_state = &mut self.panels[panel_index];
        if panel_state.source.is_panelized()
            && !panel_state.loading
            && panel_state.panelized_entries.is_none()
            && !panel_state.filter.is_active()
            && filter.is_active()
        {
            panel_state.panelized_entries =
                Some(Arc::<[FileEntry]>::from(panel_state.entries.clone()));
        }
        panel_state.filter = filter.clone();
        self.settings.panel_options.filters[panel_index] = filter;
        self.settings.mark_dirty();
    }

    pub fn active_panel(&self) -> &PanelState {
        &self.panels[self.active_panel.index()]
    }

    pub fn panel_view_mode(&self, panel: ActivePanel) -> PanelViewMode {
        self.panel_views[panel.index()]
    }

    pub fn panel_listing_format(&self, panel: ActivePanel) -> PanelListingFormat {
        self.panel_listing_formats[panel.index()]
    }

    pub fn quick_view_state(&self, panel: ActivePanel) -> &QuickViewState {
        &self.quick_views[panel.index()]
    }

    pub(crate) fn set_panel_listing_format(
        &mut self,
        panel: ActivePanel,
        format: PanelListingFormat,
    ) {
        self.deactivate_quick_view(panel);
        self.panel_views[panel.index()] = PanelViewMode::Listing;
        self.panel_listing_formats[panel.index()] = format;
        self.settings.panel_options.listing_formats[panel.index()] = format;
        self.settings.mark_dirty();
        self.active_panel = panel;
        self.set_status(format!(
            "{} panel listing format: {}",
            panel.label(),
            format.label()
        ));
    }

    pub(crate) fn set_panel_view_mode(&mut self, panel: ActivePanel, mode: PanelViewMode) {
        match mode {
            PanelViewMode::Listing => {
                self.deactivate_quick_view(panel);
                self.panel_views[panel.index()] = PanelViewMode::Listing;
                if self.exit_panelize_mode_for(panel) {
                    self.queue_panel_refresh(panel);
                    self.set_status(format!("{} panel: loading file listing...", panel.label()));
                } else {
                    self.set_status(format!("{} panel: file listing", panel.label()));
                }
            }
            PanelViewMode::QuickView => self.activate_quick_view(panel),
            PanelViewMode::Info => {
                let source_panel = panel.other();
                self.deactivate_quick_view(panel);
                self.deactivate_quick_view(source_panel);
                self.panel_views[source_panel.index()] = PanelViewMode::Listing;
                self.panel_views[panel.index()] = PanelViewMode::Info;
                self.active_panel = source_panel;
                self.set_status(format!(
                    "{} panel: info for {} panel selection",
                    panel.label(),
                    source_panel.label()
                ));
            }
        }
    }

    pub fn active_panel_mut(&mut self) -> &mut PanelState {
        let index = self.active_panel.index();
        &mut self.panels[index]
    }

    pub fn passive_panel(&self) -> &PanelState {
        let index = self.passive_panel_index();
        &self.panels[index]
    }

    fn passive_panel_index(&self) -> usize {
        match self.active_panel {
            ActivePanel::Left => ActivePanel::Right.index(),
            ActivePanel::Right => ActivePanel::Left.index(),
        }
    }

    pub fn toggle_active_panel(&mut self) -> bool {
        let next = self.active_panel.other();
        if self.panel_view_mode(next) != PanelViewMode::Listing {
            return false;
        }
        self.active_panel = next;
        true
    }

    pub fn refresh_active_panel(&mut self) {
        self.queue_panel_refresh(self.active_panel);
    }

    pub fn refresh_panels(&mut self) {
        self.queue_panel_refresh(ActivePanel::Left);
        self.queue_panel_refresh(ActivePanel::Right);
    }

    pub fn move_cursor(&mut self, delta: isize) {
        self.active_panel_mut().move_cursor(delta);
    }

    pub fn open_selected_directory(&mut self) -> bool {
        let panel = self.active_panel;
        let previous_directory = self.active_panel().cwd.clone();
        let revert = self.panel_refresh_revert_snapshot(panel);
        let snapshot = self.completed_panelized_result_snapshot(panel);
        let opened = self.active_panel_mut().open_selected_directory();
        if opened {
            self.schedule_panel_refresh_revert(panel, revert);
            if let Some(snapshot) = snapshot {
                self.panelized_result_history[panel.index()] = Some(snapshot);
            }
            self.remember_previous_directory(panel, previous_directory);
            self.sync_quick_view_from(panel, false);
            self.sync_selection_size(panel, false);
        }
        opened
    }

    pub fn go_parent_directory(&mut self) -> bool {
        let panel = self.active_panel;
        let previous_directory = self.active_panel().cwd.clone();
        let revert = self.panel_refresh_revert_snapshot(panel);
        let snapshot = self.completed_panelized_result_snapshot(panel);
        let opened = self.active_panel_mut().go_parent();
        if opened {
            self.schedule_panel_refresh_revert(panel, revert);
            if let Some(snapshot) = snapshot {
                self.panelized_result_history[panel.index()] = Some(snapshot);
            }
            self.remember_previous_directory(panel, previous_directory);
            self.sync_quick_view_from(panel, false);
            self.sync_selection_size(panel, false);
        }
        opened
    }

    pub fn exit_panelize_mode(&mut self) -> bool {
        self.exit_panelize_mode_for(self.active_panel)
    }

    fn exit_panelize_mode_for(&mut self, panel: ActivePanel) -> bool {
        let revert = self.panel_refresh_revert_snapshot(panel);
        let snapshot = self.completed_panelized_result_snapshot(panel);
        let exited = self.panels[panel.index()].exit_panelize();
        if exited {
            self.schedule_panel_refresh_revert(panel, revert);
            if let Some(snapshot) = snapshot {
                self.panelized_result_history[panel.index()] = Some(snapshot);
            }
            self.sync_quick_view_from(panel, false);
            self.sync_selection_size(panel, false);
        }
        exited
    }

    pub(crate) fn open_selected_file_in_editor(&mut self) -> EditSelectionResult {
        let configured_editor = self.settings.configuration.editor_command.clone();
        self.open_selected_file_in_editor_with_resolver(|| {
            resolve_external_editor_command(configured_editor.as_deref())
        })
    }

    pub(crate) fn open_selected_file_in_editor_with_resolver(
        &mut self,
        mut resolve_external_editor: impl FnMut() -> Option<String>,
    ) -> EditSelectionResult {
        let Some((path, is_dir)) = self
            .selected_non_parent_entry()
            .map(|entry| (entry.path.clone(), entry.is_dir()))
        else {
            return EditSelectionResult::NoEntrySelected;
        };

        if is_dir {
            return EditSelectionResult::SelectedEntryIsDirectory;
        }

        if let Some(editor_command) = resolve_external_editor() {
            self.pending_external_edit_requests
                .push(ExternalEditRequest {
                    editor_command,
                    path,
                    cwd: self.active_panel().cwd.clone(),
                });
            return EditSelectionResult::OpenedExternal;
        }

        EditSelectionResult::NoEditorResolved
    }

    pub(crate) fn execute_selected_file(&mut self) -> ExecuteSelectionResult {
        let Some((path, is_dir, is_runnable)) = self
            .selected_non_parent_entry()
            .map(|entry| (entry.path.clone(), entry.is_dir(), entry.is_runnable()))
        else {
            return ExecuteSelectionResult::NoEntrySelected;
        };

        if is_dir {
            return ExecuteSelectionResult::SelectedEntryIsDirectory;
        }

        if is_runnable {
            self.pending_external_execute_requests
                .push(ExternalExecuteRequest {
                    path,
                    cwd: self.active_panel().cwd.clone(),
                });
            ExecuteSelectionResult::OpenedExternal
        } else {
            self.queue_worker_job_request(JobRequest::OpenDesktop { path });
            ExecuteSelectionResult::QueuedDesktopOpen
        }
    }

    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status_line = normalize_status_message(message.into());
        self.status_expires_at = self
            .status_message_timeout()
            .and_then(|timeout| Instant::now().checked_add(timeout))
            .filter(|_| !self.status_line.is_empty());
        self.status_message_generation = self.status_message_generation.wrapping_add(1);
    }

    pub fn expire_status_line(&mut self) {
        self.expire_status_line_at(Instant::now());
    }

    pub(crate) fn expire_status_line_at(&mut self, now: Instant) {
        let Some(expires_at) = self.status_expires_at else {
            return;
        };
        if now < expires_at {
            return;
        }
        self.status_line.clear();
        self.status_expires_at = None;
    }

    pub fn set_available_skins(&mut self, mut skins: Vec<String>) {
        skins.sort();
        skins.dedup();
        self.available_skins = skins;
    }

    pub fn set_active_skin_name(&mut self, skin_name: impl Into<String>) {
        self.settings.appearance.skin = skin_name.into();
        self.preview_skin_name = None;
        self.refresh_settings_entries();
    }

    pub fn set_preview_skin_name(&mut self, skin_name: impl Into<String>) {
        self.preview_skin_name = Some(skin_name.into());
        self.refresh_settings_entries();
    }

    pub fn clear_preview_skin_name(&mut self) {
        self.preview_skin_name = None;
        self.refresh_settings_entries();
    }

    pub fn active_skin_name(&self) -> &str {
        self.preview_skin_name
            .as_deref()
            .unwrap_or(self.settings.appearance.skin.as_str())
    }

    pub fn overwrite_policy(&self) -> OverwritePolicy {
        self.settings.configuration.default_overwrite_policy
    }

    pub(crate) fn set_overwrite_policy(&mut self, policy: OverwritePolicy) {
        self.settings.configuration.default_overwrite_policy = policy;
    }

    pub fn hotlist(&self) -> &[HotlistEntry] {
        &self.settings.configuration.hotlist
    }

    pub(crate) fn panelize_presets(&self) -> &[PanelizePreset] {
        &self.settings.configuration.panelize_presets
    }

    pub fn take_pending_skin_change(&mut self) -> Option<String> {
        self.pending_skin_change.take()
    }

    pub fn take_pending_skin_preview(&mut self) -> Option<String> {
        self.pending_skin_preview.take()
    }

    pub fn take_pending_skin_revert(&mut self) -> Option<String> {
        self.pending_skin_revert.take()
    }

    pub fn take_pending_save_setup(&mut self) -> bool {
        std::mem::take(&mut self.pending_save_setup)
    }

    pub fn clear_xmap(&mut self) {
        self.xmap_pending = false;
    }

    pub fn is_xmap_pending(&self) -> bool {
        self.xmap_pending
    }

    pub fn set_keybinding_hints_from_keymap(&mut self, keymap: &Keymap) {
        self.keybinding_hints = KeybindingHints::from_keymap(keymap);
    }

    pub fn set_keymap_parse_report(&mut self, report: &KeymapParseReport) {
        self.keymap_unknown_actions = report.unknown_actions.len();
        self.keymap_invalid_bindings = report.skipped_bindings.len();
    }

    pub fn capture_learn_keys_chord(&mut self, chord: KeyChord) -> bool {
        if !self.pending_learn_keys_capture {
            return false;
        }

        self.pending_learn_keys_capture = false;
        if chord.code == KeyCode::Esc
            && !chord.modifiers.ctrl
            && !chord.modifiers.alt
            && !chord.modifiers.shift
        {
            self.set_status("Learn keys capture canceled");
            return true;
        }

        let captured = format_key_chord(chord);
        self.settings.learn_keys.last_learned_binding = Some(captured.clone());
        self.settings.mark_dirty();
        let target = self
            .settings
            .configuration
            .keymap_override
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| String::from("<none>"));
        self.set_status(format!(
            "Captured key chord: {captured} (override target: {target})"
        ));
        self.refresh_settings_entries();
        true
    }

    pub fn top_route(&self) -> &Route {
        self.routes
            .last()
            .expect("route stack must always contain file manager route")
    }

    pub fn route_depth(&self) -> usize {
        self.routes.len()
    }

    pub fn key_context(&self) -> KeyContext {
        match self.top_route() {
            Route::FileManager => {
                if self.xmap_pending {
                    KeyContext::FileManagerXMap
                } else {
                    KeyContext::FileManager
                }
            }
            Route::CommandLine(_) => KeyContext::CommandLine,
            Route::Jobs => KeyContext::Jobs,
            Route::Viewer(viewer) => {
                if viewer.hex_mode {
                    KeyContext::ViewerHex
                } else {
                    KeyContext::Viewer
                }
            }
            Route::Menu(_) => KeyContext::Menu,
            Route::Settings(_) => KeyContext::Listbox,
            Route::FindResults(_) => KeyContext::FindResults,
            Route::Tree(_) => KeyContext::Tree,
            Route::Hotlist => KeyContext::Hotlist,
            Route::Help(_) => KeyContext::Help,
            Route::Dialog(dialog) => dialog.key_context(),
        }
    }

    pub(crate) fn selected_operation_paths(&self) -> Vec<PathBuf> {
        let tagged = self.active_panel().tagged_paths_in_operation_order();
        if !tagged.is_empty() {
            return tagged;
        }

        self.active_panel()
            .selected_entry()
            .filter(|entry| !entry.is_parent())
            .map(|entry| vec![entry.path.clone()])
            .unwrap_or_default()
    }

    pub(crate) fn selected_non_parent_entry(&self) -> Option<&FileEntry> {
        self.active_panel()
            .selected_entry()
            .filter(|entry| !entry.is_parent())
    }

    pub(crate) fn copy_selected_path_to_clipboard(&mut self) {
        let Some(path) = self
            .active_panel()
            .selected_entry()
            .map(|entry| entry.path.clone())
        else {
            self.set_command_feedback("No entry selected");
            return;
        };
        let absolute_path = match std::path::absolute(path) {
            Ok(path) => path,
            Err(error) => {
                self.set_command_feedback(format!("Selected path cannot be resolved: {error}"));
                return;
            }
        };
        let Some(text) = absolute_path.to_str().map(ToString::to_string) else {
            self.set_command_feedback("Selected path is not valid UTF-8 and cannot be copied");
            return;
        };

        self.pending_clipboard_copy_requests
            .push(ClipboardCopyRequest { text });
        self.set_command_feedback("Copying selected path to clipboard...");
    }
}
