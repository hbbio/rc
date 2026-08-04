use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use crate::*;

impl AppState {
    pub fn new(start_path: PathBuf) -> io::Result<Self> {
        let settings = Settings::default();
        let left = PanelState::new(start_path.clone())?;
        let right = PanelState::new(start_path)?;

        Ok(Self {
            settings: settings.clone(),
            panels: [left, right],
            active_panel: ActivePanel::Left,
            status_line: String::from("Press F1 for help"),
            status_expires_at: None,
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
            paused_find_results: None,
            pending_worker_commands: Vec::new(),
            pending_external_edit_requests: Vec::new(),
            panel_refresh: PanelRefreshWorkflow::default(),
            panel_refresh_post: PanelRefreshPostWorkflow::default(),
            find_pause_flags: HashMap::new(),
            deferred_persist_settings_request: None,
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
        self.hotlist_cursor = self
            .hotlist_cursor
            .min(self.settings.configuration.hotlist.len().saturating_sub(1));
        self.preview_skin_name = None;
        self.status_expires_at = self
            .status_message_timeout()
            .and_then(|timeout| Instant::now().checked_add(timeout))
            .filter(|_| !self.status_line.is_empty());

        let sort_mode = self.default_panel_sort_mode();
        let show_hidden_files = self.settings.panel_options.show_hidden_files;
        for panel in &mut self.panels {
            panel.sort_mode = sort_mode;
            panel.set_show_hidden_files(show_hidden_files);
        }
    }

    pub(crate) fn default_panel_sort_mode(&self) -> SortMode {
        SortMode {
            field: self.settings.panel_options.sort_field,
            reverse: self.settings.panel_options.sort_reverse,
        }
    }

    pub fn active_panel(&self) -> &PanelState {
        &self.panels[self.active_panel.index()]
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

    pub fn toggle_active_panel(&mut self) {
        self.active_panel.toggle();
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
        self.active_panel_mut().open_selected_directory()
    }

    pub fn go_parent_directory(&mut self) -> bool {
        self.active_panel_mut().go_parent()
    }

    pub fn exit_panelize_mode(&mut self) -> bool {
        self.active_panel_mut().exit_panelize()
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

    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status_line = normalize_status_message(message.into());
        self.status_expires_at = self
            .status_message_timeout()
            .and_then(|timeout| Instant::now().checked_add(timeout))
            .filter(|_| !self.status_line.is_empty());
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

    pub fn hotlist(&self) -> &[PathBuf] {
        &self.settings.configuration.hotlist
    }

    pub(crate) fn panelize_presets(&self) -> &[String] {
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
        let tagged = self.active_panel().tagged_paths_in_display_order();
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
}
