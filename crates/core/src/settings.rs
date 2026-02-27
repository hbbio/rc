use std::path::PathBuf;
use std::time::SystemTime;

use crate::OverwritePolicy;

pub const DEFAULT_PANELIZE_PRESETS: &[&str] = &[
    "find . -type f",
    "find . -name '*.orig'",
    "find . -name '*.rej'",
    "find . -name core",
    "find . -type f -perm -4000",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsCategory {
    Configuration,
    Layout,
    PanelOptions,
    Confirmation,
    Appearance,
    DisplayBits,
    LearnKeys,
    VirtualFs,
}

impl SettingsCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Configuration => "Configuration",
            Self::Layout => "Layout",
            Self::PanelOptions => "Panel options",
            Self::Confirmation => "Confirmation",
            Self::Appearance => "Appearance",
            Self::DisplayBits => "Display bits",
            Self::LearnKeys => "Learn keys",
            Self::VirtualFs => "Virtual FS",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsSortField {
    Name,
    Size,
    Modified,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Settings {
    pub configuration: ConfigurationSettings,
    pub layout: LayoutSettings,
    pub panel_options: PanelOptionsSettings,
    pub confirmation: ConfirmationSettings,
    pub appearance: AppearanceSettings,
    pub display_bits: DisplayBitsSettings,
    pub learn_keys: LearnKeysSettings,
    pub virtual_fs: VirtualFsSettings,
    pub advanced: AdvancedSettings,
    pub save_setup: SaveSetupMetadata,
}

impl Settings {
    pub fn mark_dirty(&mut self) {
        self.save_setup.dirty = true;
    }

    pub fn mark_saved(&mut self, saved_at: SystemTime) {
        self.save_setup.dirty = false;
        self.save_setup.last_saved_at = Some(saved_at);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigurationSettings {
    pub default_overwrite_policy: OverwritePolicy,
    pub macos_option_symbols: bool,
    pub use_internal_editor: bool,
    pub hotlist: Vec<PathBuf>,
    pub panelize_presets: Vec<String>,
    pub keymap_override: Option<PathBuf>,
}

impl Default for ConfigurationSettings {
    fn default() -> Self {
        Self {
            default_overwrite_policy: OverwritePolicy::Skip,
            macos_option_symbols: cfg!(target_os = "macos"),
            use_internal_editor: false,
            hotlist: Vec::new(),
            panelize_presets: DEFAULT_PANELIZE_PRESETS
                .iter()
                .map(ToString::to_string)
                .collect(),
            keymap_override: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayoutSettings {
    pub show_menu_bar: bool,
    pub show_button_bar: bool,
    pub show_debug_status: bool,
    pub show_panel_totals: bool,
    pub jobs_dialog_width: u16,
    pub jobs_dialog_height: u16,
    pub help_dialog_width: u16,
    pub help_dialog_height: u16,
}

impl Default for LayoutSettings {
    fn default() -> Self {
        Self {
            show_menu_bar: true,
            show_button_bar: true,
            show_debug_status: true,
            show_panel_totals: true,
            jobs_dialog_width: 92,
            jobs_dialog_height: 24,
            help_dialog_width: 116,
            help_dialog_height: 36,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PanelOptionsSettings {
    pub show_hidden_files: bool,
    pub sort_field: SettingsSortField,
    pub sort_reverse: bool,
}

impl Default for PanelOptionsSettings {
    fn default() -> Self {
        Self {
            show_hidden_files: true,
            sort_field: SettingsSortField::Name,
            sort_reverse: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfirmationSettings {
    pub confirm_delete: bool,
    pub confirm_overwrite: bool,
    pub confirm_quit: bool,
}

impl Default for ConfirmationSettings {
    fn default() -> Self {
        Self {
            confirm_delete: true,
            confirm_overwrite: true,
            confirm_quit: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppearanceSettings {
    pub skin: String,
    pub skin_dirs: Vec<PathBuf>,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            skin: String::from("default"),
            skin_dirs: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisplayBitsSettings {
    pub utf8_output: bool,
    pub eight_bit_input: bool,
}

impl Default for DisplayBitsSettings {
    fn default() -> Self {
        Self {
            utf8_output: true,
            eight_bit_input: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct LearnKeysSettings {
    pub last_learned_binding: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VirtualFsSettings {
    pub vfs_enabled: bool,
    pub ftp_enabled: bool,
    pub shell_link_enabled: bool,
    pub sftp_enabled: bool,
}

impl Default for VirtualFsSettings {
    fn default() -> Self {
        Self {
            vfs_enabled: true,
            ftp_enabled: true,
            shell_link_enabled: true,
            sftp_enabled: true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdvancedSettings {
    pub page_step: usize,
    pub viewer_page_step: usize,
    pub max_find_results: usize,
    pub tree_max_depth: usize,
    pub tree_max_entries: usize,
    pub disk_usage_cache_ttl_ms: u64,
    pub disk_usage_cache_max_entries: usize,
}

impl Default for AdvancedSettings {
    fn default() -> Self {
        Self {
            page_step: 10,
            viewer_page_step: 20,
            max_find_results: 2_000,
            tree_max_depth: 6,
            tree_max_entries: 2_000,
            disk_usage_cache_ttl_ms: 750,
            disk_usage_cache_max_entries: 16,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct SaveSetupMetadata {
    pub dirty: bool,
    pub last_saved_at: Option<SystemTime>,
}
