use std::collections::HashMap;

use crate::*;

impl AppState {
    pub fn keybinding_labels(&self, context: KeyContext, command: AppCommand) -> Option<&[String]> {
        self.keybinding_hints.labels_for(context, command)
    }

    pub fn keybinding_primary_label(
        &self,
        context: KeyContext,
        command: AppCommand,
    ) -> Option<&str> {
        self.keybinding_labels(context, command)
            .and_then(|labels| labels.first().map(String::as_str))
    }

    pub fn keybinding_joined_label(
        &self,
        context: KeyContext,
        command: AppCommand,
        separator: &str,
        limit: usize,
    ) -> Option<String> {
        let labels = self.keybinding_labels(context, command)?;
        let clipped = if limit == 0 {
            labels
        } else {
            &labels[..labels.len().min(limit)]
        };
        Some(clipped.join(separator))
    }

    pub fn menu_entry_shortcut_label(&self, entry: &MenuEntry) -> String {
        if entry.literal_shortcut && !entry.shortcut.is_empty() {
            return entry.shortcut.to_string();
        }
        if let Some(dynamic) = self.keybinding_primary_label(KeyContext::FileManager, entry.command)
        {
            return dynamic.to_string();
        }
        entry.shortcut.to_string()
    }

    pub fn menu_popup_width(&self, menu: &MenuState) -> u16 {
        let inner = menu
            .active_entries()
            .iter()
            .map(|entry| {
                let label_width = entry.label.chars().count() as u16;
                let shortcut = self.menu_entry_shortcut_label(entry);
                let shortcut_width = shortcut.chars().count() as u16;
                if shortcut_width == 0 {
                    label_width
                } else {
                    label_width.saturating_add(1).saturating_add(shortcut_width)
                }
            })
            .max()
            .unwrap_or(1)
            .saturating_add(2);
        inner.saturating_add(2)
    }

    fn keybinding_primary_or_fallback(
        &self,
        context: KeyContext,
        command: AppCommand,
        fallback: &str,
    ) -> String {
        self.keybinding_primary_label(context, command)
            .map_or_else(|| fallback.to_string(), ToString::to_string)
    }

    fn keybinding_joined_or_fallback(
        &self,
        context: KeyContext,
        command: AppCommand,
        fallback: &str,
        limit: usize,
    ) -> String {
        self.keybinding_joined_label(context, command, " / ", limit)
            .unwrap_or_else(|| fallback.to_string())
    }

    fn xmap_sequence_or_fallback(&self, command: AppCommand, fallback: &str) -> String {
        let prefix = self.keybinding_primary_label(KeyContext::FileManager, AppCommand::EnterXMap);
        let suffix = self.keybinding_primary_label(KeyContext::FileManagerXMap, command);
        match (prefix, suffix) {
            (Some(prefix), Some(suffix)) => format!("{prefix} {suffix}"),
            _ => fallback.to_string(),
        }
    }

    pub(crate) fn help_replacements(&self) -> HashMap<&'static str, String> {
        let mut replacements = HashMap::new();

        replacements.insert(
            "help_link_cycle",
            format!(
                "{} / {}",
                self.keybinding_primary_or_fallback(
                    KeyContext::Help,
                    AppCommand::HelpLinkNext,
                    "Tab"
                ),
                self.keybinding_primary_or_fallback(
                    KeyContext::Help,
                    AppCommand::HelpLinkPrev,
                    "Shift-Tab",
                ),
            ),
        );
        replacements.insert(
            "help_follow",
            self.keybinding_joined_or_fallback(
                KeyContext::Help,
                AppCommand::HelpFollowLink,
                "Enter / Right",
                2,
            ),
        );
        replacements.insert(
            "help_back",
            self.keybinding_joined_or_fallback(
                KeyContext::Help,
                AppCommand::HelpBack,
                "Left / F3 / l",
                3,
            ),
        );
        replacements.insert(
            "help_index",
            self.keybinding_joined_or_fallback(
                KeyContext::Help,
                AppCommand::HelpIndex,
                "F2 / c",
                2,
            ),
        );
        replacements.insert(
            "help_node_cycle",
            format!(
                "{} / {}",
                self.keybinding_primary_or_fallback(
                    KeyContext::Help,
                    AppCommand::HelpNodeNext,
                    "n"
                ),
                self.keybinding_primary_or_fallback(
                    KeyContext::Help,
                    AppCommand::HelpNodePrev,
                    "p"
                ),
            ),
        );
        replacements.insert(
            "help_close",
            self.keybinding_joined_or_fallback(
                KeyContext::Help,
                AppCommand::CloseHelp,
                "F10 / Esc",
                2,
            ),
        );

        replacements.insert(
            "fm_switch_panel",
            self.keybinding_joined_or_fallback(
                KeyContext::FileManager,
                AppCommand::SwitchPanel,
                "Tab",
                1,
            ),
        );
        replacements.insert(
            "fm_open_entry",
            self.keybinding_joined_or_fallback(
                KeyContext::FileManager,
                AppCommand::OpenEntry,
                "Enter/F3",
                2,
            ),
        );
        replacements.insert(
            "fm_parent",
            self.keybinding_joined_or_fallback(
                KeyContext::FileManager,
                AppCommand::CdUp,
                "Backspace",
                1,
            ),
        );
        replacements.insert(
            "fm_find",
            self.keybinding_joined_or_fallback(
                KeyContext::FileManager,
                AppCommand::OpenFindDialog,
                "Alt-F",
                2,
            ),
        );
        replacements.insert(
            "fm_tree",
            self.keybinding_joined_or_fallback(
                KeyContext::FileManager,
                AppCommand::OpenTree,
                "Alt-T",
                1,
            ),
        );
        replacements.insert(
            "fm_hotlist",
            self.keybinding_joined_or_fallback(
                KeyContext::FileManager,
                AppCommand::OpenHotlist,
                "Alt-H",
                1,
            ),
        );
        replacements.insert(
            "fm_external_panelize",
            format!(
                "{} (or {})",
                self.xmap_sequence_or_fallback(AppCommand::OpenPanelizeDialog, "Ctrl-X !"),
                self.keybinding_joined_or_fallback(
                    KeyContext::FileManager,
                    AppCommand::OpenPanelizeDialog,
                    "Alt/Ctrl-P",
                    2,
                )
            ),
        );
        replacements.insert(
            "fm_external_panelize_menu",
            self.keybinding_primary_or_fallback(
                KeyContext::FileManager,
                AppCommand::OpenMenu,
                "F9",
            ),
        );
        replacements.insert(
            "fm_open_jobs",
            self.keybinding_joined_or_fallback(
                KeyContext::FileManager,
                AppCommand::OpenJobsScreen,
                "Ctrl-J",
                1,
            ),
        );
        replacements.insert(
            "fm_cancel_job",
            self.keybinding_joined_or_fallback(
                KeyContext::FileManager,
                AppCommand::CancelJob,
                "Alt-J",
                1,
            ),
        );
        replacements.insert(
            "fm_skin",
            self.keybinding_joined_or_fallback(
                KeyContext::FileManager,
                AppCommand::OpenSkinDialog,
                "Alt-S/Ctrl-K",
                2,
            ),
        );
        replacements.insert(
            "fm_quit",
            self.keybinding_joined_or_fallback(
                KeyContext::FileManager,
                AppCommand::Quit,
                "q/F10",
                2,
            ),
        );
        replacements.insert("fm_move", "Up/Down".to_string());
        replacements.insert(
            "fm_toggle_tag",
            self.keybinding_joined_or_fallback(
                KeyContext::FileManager,
                AppCommand::ToggleTag,
                "Insert/Ctrl-T",
                2,
            ),
        );
        replacements.insert(
            "fm_file_ops",
            format!(
                "{}/{}/{}",
                self.keybinding_primary_or_fallback(
                    KeyContext::FileManager,
                    AppCommand::Copy,
                    "F5"
                ),
                self.keybinding_primary_or_fallback(
                    KeyContext::FileManager,
                    AppCommand::Move,
                    "F6"
                ),
                self.keybinding_primary_or_fallback(
                    KeyContext::FileManager,
                    AppCommand::Delete,
                    "F8"
                ),
            ),
        );

        replacements.insert("viewer_scroll", "Up/Down and PgUp/PgDn".to_string());
        replacements.insert(
            "viewer_search",
            self.keybinding_primary_or_fallback(
                KeyContext::Viewer,
                AppCommand::ViewerSearchForward,
                "F7",
            ),
        );
        replacements.insert(
            "viewer_search_back",
            self.keybinding_primary_or_fallback(
                KeyContext::Viewer,
                AppCommand::ViewerSearchBackward,
                "Shift-F7",
            ),
        );
        replacements.insert(
            "viewer_search_continue",
            format!(
                "{} / {}",
                self.keybinding_primary_or_fallback(
                    KeyContext::Viewer,
                    AppCommand::ViewerSearchContinue,
                    "n"
                ),
                self.keybinding_primary_or_fallback(
                    KeyContext::Viewer,
                    AppCommand::ViewerSearchContinueBackward,
                    "Shift-n",
                ),
            ),
        );
        replacements.insert(
            "viewer_goto",
            self.keybinding_primary_or_fallback(KeyContext::Viewer, AppCommand::ViewerGoto, "g"),
        );
        replacements.insert(
            "viewer_wrap",
            self.keybinding_primary_or_fallback(
                KeyContext::Viewer,
                AppCommand::ViewerToggleWrap,
                "w",
            ),
        );
        replacements.insert(
            "viewer_hex",
            self.keybinding_primary_or_fallback(
                KeyContext::Viewer,
                AppCommand::ViewerToggleHex,
                "h",
            ),
        );

        replacements.insert("jobs_move", "Up/Down".to_string());
        replacements.insert(
            "jobs_cancel",
            self.keybinding_joined_or_fallback(KeyContext::Jobs, AppCommand::CancelJob, "Alt-J", 1),
        );
        replacements.insert(
            "jobs_close",
            self.keybinding_joined_or_fallback(
                KeyContext::Jobs,
                AppCommand::CloseJobsScreen,
                "Esc/q",
                2,
            ),
        );

        replacements.insert("find_move", "Up/Down".to_string());
        replacements.insert("find_nav", "PgUp/PgDn/Home/End".to_string());
        replacements.insert(
            "find_open",
            self.keybinding_primary_or_fallback(
                KeyContext::FindResults,
                AppCommand::FindResultsOpenEntry,
                "Enter",
            ),
        );
        replacements.insert(
            "find_panelize",
            self.keybinding_primary_or_fallback(
                KeyContext::FindResults,
                AppCommand::FindResultsPanelize,
                "F5",
            ),
        );
        replacements.insert(
            "find_cancel",
            self.keybinding_joined_or_fallback(
                KeyContext::FindResults,
                AppCommand::CancelJob,
                "Alt-J",
                1,
            ),
        );
        replacements.insert(
            "find_close",
            self.keybinding_joined_or_fallback(
                KeyContext::FindResults,
                AppCommand::CloseFindResults,
                "Esc/q",
                2,
            ),
        );

        replacements.insert(
            "panelize_find_results",
            self.keybinding_primary_or_fallback(
                KeyContext::FindResults,
                AppCommand::FindResultsPanelize,
                "F5",
            ),
        );
        replacements.insert(
            "panelize_find_entry",
            format!(
                "{} search, then {} in results",
                self.keybinding_joined_or_fallback(
                    KeyContext::FileManager,
                    AppCommand::OpenFindDialog,
                    "Alt-?",
                    1
                ),
                self.keybinding_primary_or_fallback(
                    KeyContext::FindResults,
                    AppCommand::FindResultsPanelize,
                    "F5",
                ),
            ),
        );
        replacements.insert(
            "panelize_external",
            self.xmap_sequence_or_fallback(AppCommand::OpenPanelizeDialog, "Ctrl-X !"),
        );
        replacements.insert(
            "panelize_external_entry",
            format!(
                "{} or {} -> Command -> External panelize",
                self.xmap_sequence_or_fallback(AppCommand::OpenPanelizeDialog, "Ctrl-X !"),
                self.keybinding_primary_or_fallback(
                    KeyContext::FileManager,
                    AppCommand::OpenMenu,
                    "F9"
                ),
            ),
        );
        replacements.insert(
            "panelize_dialog_keys",
            "Up/Down, Tab, Enter, Esc, F2/F4/F8".to_string(),
        );
        replacements.insert(
            "panelize_ops",
            format!(
                "{}/{}/{}/{}/{}",
                self.keybinding_primary_or_fallback(
                    KeyContext::FileManager,
                    AppCommand::OpenEntry,
                    "F3"
                ),
                self.keybinding_primary_or_fallback(
                    KeyContext::FileManager,
                    AppCommand::EditEntry,
                    "F4"
                ),
                self.keybinding_primary_or_fallback(
                    KeyContext::FileManager,
                    AppCommand::Copy,
                    "F5"
                ),
                self.keybinding_primary_or_fallback(
                    KeyContext::FileManager,
                    AppCommand::Move,
                    "F6"
                ),
                self.keybinding_primary_or_fallback(
                    KeyContext::FileManager,
                    AppCommand::Delete,
                    "F8"
                ),
            ),
        );
        replacements.insert(
            "panelize_refresh",
            self.keybinding_joined_or_fallback(
                KeyContext::FileManager,
                AppCommand::Reread,
                "Ctrl-R",
                1,
            ),
        );

        replacements.insert("tree_move", "Up/Down".to_string());
        replacements.insert("tree_nav", "PgUp/PgDn/Home/End".to_string());
        replacements.insert(
            "tree_open",
            self.keybinding_primary_or_fallback(
                KeyContext::Tree,
                AppCommand::TreeOpenEntry,
                "Enter",
            ),
        );
        replacements.insert(
            "tree_close",
            self.keybinding_joined_or_fallback(KeyContext::Tree, AppCommand::CloseTree, "Esc/q", 2),
        );

        replacements.insert(
            "hotlist_open",
            self.keybinding_primary_or_fallback(
                KeyContext::Hotlist,
                AppCommand::HotlistOpenEntry,
                "Enter",
            ),
        );
        replacements.insert(
            "hotlist_add",
            self.keybinding_primary_or_fallback(
                KeyContext::Hotlist,
                AppCommand::HotlistAddCurrentDirectory,
                "a",
            ),
        );
        replacements.insert(
            "hotlist_remove",
            self.keybinding_joined_or_fallback(
                KeyContext::Hotlist,
                AppCommand::HotlistRemoveSelected,
                "d/delete",
                2,
            ),
        );
        replacements.insert(
            "hotlist_close",
            self.keybinding_joined_or_fallback(
                KeyContext::Hotlist,
                AppCommand::CloseHotlist,
                "Esc/q",
                2,
            ),
        );

        replacements
    }
}
