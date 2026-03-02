#![forbid(unsafe_code)]

mod background;
mod command_dispatch;
pub mod dialog;
mod dialog_flow;
pub mod help;
pub mod jobs;
pub mod keymap;
mod navigation_flow;
mod orchestration;
mod panel;
mod panelize_flow;
mod route_flow;
pub mod settings;
mod settings_flow;
pub mod settings_io;
pub mod slo;
mod viewer;
mod viewer_flow;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;
use std::time::{Instant, SystemTime};

#[cfg(test)]
use background::stream_find_entries;
pub use background::{
    BackgroundEvent, PanelRefreshStreamRequest, build_tree_ready_event, refresh_panel_entries,
    refresh_panel_event, run_find_entries, stream_refresh_panel_entries,
};
pub use dialog::{DialogButtonFocus, DialogKind, DialogResult, DialogState};
pub use help::{HelpLine, HelpSpan, HelpState};
pub use jobs::{
    JOB_CANCELED_MESSAGE, JobError, JobErrorCode, JobEvent, JobId, JobKind, JobManager,
    JobProgress, JobRecord, JobRequest, JobRetryHint, JobStatus, JobStatusCounts, OverwritePolicy,
    WorkerCommand, WorkerJob, execute_worker_job, run_worker,
};
#[cfg(test)]
use panel::read_entries;
#[cfg(test)]
pub(crate) use panel::read_panelized_entries_with_process_backend;
pub(crate) use panel::{
    PANEL_REFRESH_CANCELED_MESSAGE, ensure_panel_refresh_not_canceled,
    read_entries_with_visibility_cancel, read_panelized_entries_with_cancel, read_panelized_paths,
    sort_file_entries,
};
pub use rc_shell::{LocalProcessBackend, ProcessBackend};
pub use settings::{
    AdvancedSettings, AppearanceSettings, ConfigurationSettings, ConfirmationSettings,
    DEFAULT_PANELIZE_PRESETS, DisplayBitsSettings, LayoutSettings, LearnKeysSettings,
    PanelOptionsSettings, SaveSetupMetadata, Settings, SettingsCategory, SettingsSortField,
    VirtualFsSettings,
};
pub use slo::{FOUNDATION_SLO, SloBudgets};
#[cfg(test)]
use std::sync::atomic::Ordering as AtomicOrdering;
pub use viewer::ViewerState;

use crate::keymap::{KeyChord, KeyCode, KeyCommand, KeyContext, Keymap, KeymapParseReport};
use crate::panel::{read_entries_with_visibility, read_panelized_entries};
use crate::viewer::ViewerSearchDirection;

const MAX_STATUS_LINE_CHARS: usize = 1024;
const VIEWER_TEXT_PREVIEW_LIMIT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AppCommand {
    OpenHelp,
    CloseHelp,
    OpenMenu,
    OpenMenuAt(usize),
    CloseMenu,
    Quit,
    CloseViewer,
    OpenFindDialog,
    CloseFindResults,
    OpenTree,
    CloseTree,
    OpenHotlist,
    CloseHotlist,
    OpenPanelizeDialog,
    PanelizePresetAdd,
    PanelizePresetEdit,
    PanelizePresetRemove,
    EnterXMap,
    SwitchPanel,
    MoveUp,
    MoveDown,
    PageUp,
    PageDown,
    MoveHome,
    MoveEnd,
    ToggleTag,
    InvertTags,
    SortNext,
    SortReverse,
    Copy,
    Move,
    Delete,
    CancelJob,
    OpenJobsScreen,
    CloseJobsScreen,
    JobsMoveUp,
    JobsMoveDown,
    OpenEntry,
    EditEntry,
    CdUp,
    Reread,
    FindResultsMoveUp,
    FindResultsMoveDown,
    FindResultsPageUp,
    FindResultsPageDown,
    FindResultsHome,
    FindResultsEnd,
    FindResultsOpenEntry,
    FindResultsPanelize,
    TreeMoveUp,
    TreeMoveDown,
    TreePageUp,
    TreePageDown,
    TreeHome,
    TreeEnd,
    TreeOpenEntry,
    HotlistMoveUp,
    HotlistMoveDown,
    HotlistPageUp,
    HotlistPageDown,
    HotlistHome,
    HotlistEnd,
    HotlistOpenEntry,
    HotlistAddCurrentDirectory,
    HotlistRemoveSelected,
    OpenConfirmDialog,
    OpenInputDialog,
    OpenListboxDialog,
    OpenSkinDialog,
    OpenOptionsConfiguration,
    OpenOptionsLayout,
    OpenOptionsPanelOptions,
    OpenOptionsConfirmation,
    OpenOptionsAppearance,
    OpenOptionsDisplayBits,
    OpenOptionsLearnKeys,
    OpenOptionsVirtualFs,
    SaveSetup,
    MenuMoveUp,
    MenuMoveDown,
    MenuMoveLeft,
    MenuMoveRight,
    MenuHome,
    MenuEnd,
    MenuAccept,
    MenuSelectAt(usize),
    HelpMoveUp,
    HelpMoveDown,
    HelpPageUp,
    HelpPageDown,
    HelpHalfPageUp,
    HelpHalfPageDown,
    HelpHome,
    HelpEnd,
    HelpFollowLink,
    HelpBack,
    HelpIndex,
    HelpLinkNext,
    HelpLinkPrev,
    HelpNodeNext,
    HelpNodePrev,
    DialogAccept,
    DialogCancel,
    DialogFocusNext,
    DialogBackspace,
    DialogInputChar(char),
    DialogListboxUp,
    DialogListboxDown,
    ViewerMoveUp,
    ViewerMoveDown,
    ViewerPageUp,
    ViewerPageDown,
    ViewerHome,
    ViewerEnd,
    ViewerSearchForward,
    ViewerSearchBackward,
    ViewerSearchContinue,
    ViewerSearchContinueBackward,
    ViewerGoto,
    ViewerToggleWrap,
    ViewerToggleHex,
    MenuNoop,
    MenuNotImplemented(&'static str),
}

impl AppCommand {
    pub fn from_key_command(context: KeyContext, key_command: &KeyCommand) -> Option<Self> {
        match (context, key_command) {
            (_, KeyCommand::OpenHelp) => Some(Self::OpenHelp),
            (KeyContext::FileManager, KeyCommand::OpenMenu) => Some(Self::OpenMenu),
            (KeyContext::Menu, KeyCommand::Quit) => Some(Self::CloseMenu),
            (KeyContext::Menu, KeyCommand::DialogCancel) => Some(Self::CloseMenu),
            (KeyContext::Menu, KeyCommand::DialogAccept)
            | (KeyContext::Menu, KeyCommand::OpenEntry) => Some(Self::MenuAccept),
            (KeyContext::Menu, KeyCommand::CursorUp) => Some(Self::MenuMoveUp),
            (KeyContext::Menu, KeyCommand::CursorDown) => Some(Self::MenuMoveDown),
            (KeyContext::Menu, KeyCommand::CursorLeft) => Some(Self::MenuMoveLeft),
            (KeyContext::Menu, KeyCommand::CursorRight) => Some(Self::MenuMoveRight),
            (KeyContext::Menu, KeyCommand::Home) => Some(Self::MenuHome),
            (KeyContext::Menu, KeyCommand::End) => Some(Self::MenuEnd),
            (KeyContext::FileManager, KeyCommand::Quit) => Some(Self::Quit),
            (KeyContext::Help, KeyCommand::Quit) => Some(Self::CloseHelp),
            (KeyContext::Viewer, KeyCommand::Quit) => Some(Self::CloseViewer),
            (KeyContext::FindResults, KeyCommand::Quit) => Some(Self::CloseFindResults),
            (KeyContext::Tree, KeyCommand::Quit) => Some(Self::CloseTree),
            (KeyContext::Hotlist, KeyCommand::Quit) => Some(Self::CloseHotlist),
            (KeyContext::FileManager, KeyCommand::PanelOther) => Some(Self::SwitchPanel),
            (KeyContext::FileManager, KeyCommand::CursorUp) => Some(Self::MoveUp),
            (KeyContext::FileManager, KeyCommand::CursorDown) => Some(Self::MoveDown),
            (KeyContext::FileManager, KeyCommand::PageUp) => Some(Self::PageUp),
            (KeyContext::FileManager, KeyCommand::PageDown) => Some(Self::PageDown),
            (KeyContext::FileManager, KeyCommand::Home) => Some(Self::MoveHome),
            (KeyContext::FileManager, KeyCommand::End) => Some(Self::MoveEnd),
            (KeyContext::FileManager, KeyCommand::ToggleTag) => Some(Self::ToggleTag),
            (KeyContext::FileManager, KeyCommand::InvertTags) => Some(Self::InvertTags),
            (KeyContext::FileManager, KeyCommand::SortNext) => Some(Self::SortNext),
            (KeyContext::FileManager, KeyCommand::SortReverse) => Some(Self::SortReverse),
            (KeyContext::FileManager, KeyCommand::Copy) => Some(Self::Copy),
            (KeyContext::FileManager, KeyCommand::Move) => Some(Self::Move),
            (KeyContext::FileManager, KeyCommand::Delete) => Some(Self::Delete),
            (KeyContext::FileManager, KeyCommand::CancelJob) => Some(Self::CancelJob),
            (KeyContext::FileManager, KeyCommand::OpenJobs) => Some(Self::OpenJobsScreen),
            (KeyContext::Jobs, KeyCommand::CloseJobs) => Some(Self::CloseJobsScreen),
            (KeyContext::Jobs, KeyCommand::CursorUp) => Some(Self::JobsMoveUp),
            (KeyContext::Jobs, KeyCommand::CursorDown) => Some(Self::JobsMoveDown),
            (KeyContext::Jobs, KeyCommand::CancelJob) => Some(Self::CancelJob),
            (KeyContext::Listbox, KeyCommand::CursorUp) => Some(Self::DialogListboxUp),
            (KeyContext::Listbox, KeyCommand::CursorDown) => Some(Self::DialogListboxDown),
            (KeyContext::Listbox, KeyCommand::OpenInputDialog) => Some(Self::PanelizePresetAdd),
            (KeyContext::Listbox, KeyCommand::OpenConfirmDialog) => Some(Self::PanelizePresetEdit),
            (KeyContext::Listbox, KeyCommand::Delete) => Some(Self::PanelizePresetRemove),
            (KeyContext::Help, KeyCommand::CursorUp) => Some(Self::HelpMoveUp),
            (KeyContext::Help, KeyCommand::CursorDown) => Some(Self::HelpMoveDown),
            (KeyContext::Help, KeyCommand::PageUp) => Some(Self::HelpPageUp),
            (KeyContext::Help, KeyCommand::PageDown) => Some(Self::HelpPageDown),
            (KeyContext::Help, KeyCommand::HelpHalfPageUp) => Some(Self::HelpHalfPageUp),
            (KeyContext::Help, KeyCommand::HelpHalfPageDown) => Some(Self::HelpHalfPageDown),
            (KeyContext::Help, KeyCommand::Home) => Some(Self::HelpHome),
            (KeyContext::Help, KeyCommand::End) => Some(Self::HelpEnd),
            (KeyContext::Help, KeyCommand::OpenEntry) => Some(Self::HelpFollowLink),
            (KeyContext::Help, KeyCommand::HelpBack) => Some(Self::HelpBack),
            (KeyContext::Help, KeyCommand::HelpIndex) => Some(Self::HelpIndex),
            (KeyContext::Help, KeyCommand::HelpLinkNext) => Some(Self::HelpLinkNext),
            (KeyContext::Help, KeyCommand::HelpLinkPrev) => Some(Self::HelpLinkPrev),
            (KeyContext::Help, KeyCommand::HelpNodeNext) => Some(Self::HelpNodeNext),
            (KeyContext::Help, KeyCommand::HelpNodePrev) => Some(Self::HelpNodePrev),
            (KeyContext::FileManager, KeyCommand::OpenEntry) => Some(Self::OpenEntry),
            (KeyContext::FileManager, KeyCommand::EditEntry) => Some(Self::EditEntry),
            (KeyContext::FileManager, KeyCommand::CdUp) => Some(Self::CdUp),
            (KeyContext::FileManager, KeyCommand::Reread) => Some(Self::Reread),
            (KeyContext::FileManager, KeyCommand::OpenFindDialog) => Some(Self::OpenFindDialog),
            (KeyContext::FindResults, KeyCommand::CursorUp) => Some(Self::FindResultsMoveUp),
            (KeyContext::FindResults, KeyCommand::CursorDown) => Some(Self::FindResultsMoveDown),
            (KeyContext::FindResults, KeyCommand::PageUp) => Some(Self::FindResultsPageUp),
            (KeyContext::FindResults, KeyCommand::PageDown) => Some(Self::FindResultsPageDown),
            (KeyContext::FindResults, KeyCommand::Home) => Some(Self::FindResultsHome),
            (KeyContext::FindResults, KeyCommand::End) => Some(Self::FindResultsEnd),
            (KeyContext::FindResults, KeyCommand::OpenEntry) => Some(Self::FindResultsOpenEntry),
            (KeyContext::FindResults, KeyCommand::OpenPanelizeDialog) => {
                Some(Self::FindResultsPanelize)
            }
            (KeyContext::FindResults, KeyCommand::CancelJob) => Some(Self::CancelJob),
            (KeyContext::FileManager, KeyCommand::OpenTree) => Some(Self::OpenTree),
            (KeyContext::Tree, KeyCommand::CursorUp) => Some(Self::TreeMoveUp),
            (KeyContext::Tree, KeyCommand::CursorDown) => Some(Self::TreeMoveDown),
            (KeyContext::Tree, KeyCommand::PageUp) => Some(Self::TreePageUp),
            (KeyContext::Tree, KeyCommand::PageDown) => Some(Self::TreePageDown),
            (KeyContext::Tree, KeyCommand::Home) => Some(Self::TreeHome),
            (KeyContext::Tree, KeyCommand::End) => Some(Self::TreeEnd),
            (KeyContext::Tree, KeyCommand::OpenEntry) => Some(Self::TreeOpenEntry),
            (KeyContext::FileManager, KeyCommand::OpenHotlist) => Some(Self::OpenHotlist),
            (KeyContext::FileManager, KeyCommand::OpenPanelizeDialog) => {
                Some(Self::OpenPanelizeDialog)
            }
            (KeyContext::FileManager, KeyCommand::EnterXMap) => Some(Self::EnterXMap),
            (KeyContext::Hotlist, KeyCommand::CursorUp) => Some(Self::HotlistMoveUp),
            (KeyContext::Hotlist, KeyCommand::CursorDown) => Some(Self::HotlistMoveDown),
            (KeyContext::Hotlist, KeyCommand::PageUp) => Some(Self::HotlistPageUp),
            (KeyContext::Hotlist, KeyCommand::PageDown) => Some(Self::HotlistPageDown),
            (KeyContext::Hotlist, KeyCommand::Home) => Some(Self::HotlistHome),
            (KeyContext::Hotlist, KeyCommand::End) => Some(Self::HotlistEnd),
            (KeyContext::Hotlist, KeyCommand::OpenEntry) => Some(Self::HotlistOpenEntry),
            (KeyContext::Hotlist, KeyCommand::OpenHotlist) => Some(Self::OpenHotlist),
            (KeyContext::Hotlist, KeyCommand::AddHotlist) => Some(Self::HotlistAddCurrentDirectory),
            (KeyContext::Hotlist, KeyCommand::RemoveHotlist) => Some(Self::HotlistRemoveSelected),
            (KeyContext::Viewer, KeyCommand::CursorUp) => Some(Self::ViewerMoveUp),
            (KeyContext::Viewer, KeyCommand::CursorDown) => Some(Self::ViewerMoveDown),
            (KeyContext::Viewer, KeyCommand::PageUp) => Some(Self::ViewerPageUp),
            (KeyContext::Viewer, KeyCommand::PageDown) => Some(Self::ViewerPageDown),
            (KeyContext::Viewer, KeyCommand::Home) => Some(Self::ViewerHome),
            (KeyContext::Viewer, KeyCommand::End) => Some(Self::ViewerEnd),
            (KeyContext::ViewerHex, KeyCommand::Quit) => Some(Self::CloseViewer),
            (KeyContext::ViewerHex, KeyCommand::CursorUp) => Some(Self::ViewerMoveUp),
            (KeyContext::ViewerHex, KeyCommand::CursorDown) => Some(Self::ViewerMoveDown),
            (KeyContext::ViewerHex, KeyCommand::PageUp) => Some(Self::ViewerPageUp),
            (KeyContext::ViewerHex, KeyCommand::PageDown) => Some(Self::ViewerPageDown),
            (KeyContext::ViewerHex, KeyCommand::Home) => Some(Self::ViewerHome),
            (KeyContext::ViewerHex, KeyCommand::End) => Some(Self::ViewerEnd),
            (KeyContext::Viewer, KeyCommand::Search) => Some(Self::ViewerSearchForward),
            (KeyContext::Viewer, KeyCommand::SearchBackward) => Some(Self::ViewerSearchBackward),
            (KeyContext::Viewer, KeyCommand::SearchContinue) => Some(Self::ViewerSearchContinue),
            (KeyContext::Viewer, KeyCommand::SearchContinueBackward) => {
                Some(Self::ViewerSearchContinueBackward)
            }
            (KeyContext::Viewer, KeyCommand::Goto) => Some(Self::ViewerGoto),
            (KeyContext::Viewer, KeyCommand::ToggleWrap) => Some(Self::ViewerToggleWrap),
            (KeyContext::ViewerHex, KeyCommand::Search) => Some(Self::ViewerSearchForward),
            (KeyContext::ViewerHex, KeyCommand::SearchBackward) => Some(Self::ViewerSearchBackward),
            (KeyContext::ViewerHex, KeyCommand::SearchContinue) => Some(Self::ViewerSearchContinue),
            (KeyContext::ViewerHex, KeyCommand::SearchContinueBackward) => {
                Some(Self::ViewerSearchContinueBackward)
            }
            (KeyContext::ViewerHex, KeyCommand::Goto) => Some(Self::ViewerGoto),
            (KeyContext::ViewerHex, KeyCommand::ToggleWrap) => Some(Self::ViewerToggleWrap),
            (KeyContext::Viewer, KeyCommand::ToggleHex)
            | (KeyContext::ViewerHex, KeyCommand::ToggleHex) => Some(Self::ViewerToggleHex),
            (KeyContext::FileManager, KeyCommand::OpenConfirmDialog) => {
                Some(Self::OpenConfirmDialog)
            }
            (KeyContext::FileManager, KeyCommand::OpenInputDialog) => Some(Self::OpenInputDialog),
            (KeyContext::FileManager, KeyCommand::OpenListboxDialog) => {
                Some(Self::OpenListboxDialog)
            }
            (KeyContext::FileManager, KeyCommand::OpenSkinDialog) => Some(Self::OpenSkinDialog),
            (KeyContext::FileManager, KeyCommand::OpenOptionsConfiguration) => {
                Some(Self::OpenOptionsConfiguration)
            }
            (KeyContext::FileManager, KeyCommand::OpenOptionsLayout) => {
                Some(Self::OpenOptionsLayout)
            }
            (KeyContext::FileManager, KeyCommand::OpenOptionsPanelOptions) => {
                Some(Self::OpenOptionsPanelOptions)
            }
            (KeyContext::FileManager, KeyCommand::OpenOptionsConfirmation) => {
                Some(Self::OpenOptionsConfirmation)
            }
            (KeyContext::FileManager, KeyCommand::OpenOptionsAppearance) => {
                Some(Self::OpenOptionsAppearance)
            }
            (KeyContext::FileManager, KeyCommand::OpenOptionsDisplayBits) => {
                Some(Self::OpenOptionsDisplayBits)
            }
            (KeyContext::FileManager, KeyCommand::OpenOptionsLearnKeys) => {
                Some(Self::OpenOptionsLearnKeys)
            }
            (KeyContext::FileManager, KeyCommand::OpenOptionsVirtualFs) => {
                Some(Self::OpenOptionsVirtualFs)
            }
            (KeyContext::FileManager, KeyCommand::SaveSetup) => Some(Self::SaveSetup),
            (_, KeyCommand::DialogAccept) => Some(Self::DialogAccept),
            (_, KeyCommand::DialogCancel) => Some(Self::DialogCancel),
            (_, KeyCommand::DialogFocusNext) => Some(Self::DialogFocusNext),
            (_, KeyCommand::DialogBackspace) => Some(Self::DialogBackspace),
            (_, KeyCommand::Unknown(_)) => None,
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MenuEntry {
    pub label: &'static str,
    pub shortcut: &'static str,
    pub literal_shortcut: bool,
    pub command: AppCommand,
    pub selectable: bool,
}

impl MenuEntry {
    const fn action(label: &'static str, command: AppCommand) -> Self {
        Self {
            label,
            shortcut: "",
            literal_shortcut: false,
            command,
            selectable: true,
        }
    }

    const fn action_with_shortcut(
        label: &'static str,
        shortcut: &'static str,
        command: AppCommand,
    ) -> Self {
        Self {
            label,
            shortcut,
            literal_shortcut: false,
            command,
            selectable: true,
        }
    }

    const fn action_with_literal_shortcut(
        label: &'static str,
        shortcut: &'static str,
        command: AppCommand,
    ) -> Self {
        Self {
            label,
            shortcut,
            literal_shortcut: true,
            command,
            selectable: true,
        }
    }

    const fn stub(label: &'static str, shortcut: &'static str) -> Self {
        Self {
            label,
            shortcut,
            literal_shortcut: true,
            command: AppCommand::MenuNotImplemented(label),
            selectable: true,
        }
    }

    const fn separator() -> Self {
        Self {
            label: "",
            shortcut: "",
            literal_shortcut: true,
            command: AppCommand::MenuNoop,
            selectable: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TopMenu {
    pub title: &'static str,
    pub entries: &'static [MenuEntry],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MenuBarItem {
    pub index: usize,
    pub title: &'static str,
    pub start_x: u16,
    pub end_x: u16,
}

const SIDE_MENU_ENTRIES: [MenuEntry; 16] = [
    MenuEntry::stub("File listing", ""),
    MenuEntry::stub("Quick view", "C-x q"),
    MenuEntry::stub("Info", "C-x i"),
    MenuEntry::action("Tree", AppCommand::OpenTree),
    MenuEntry::separator(),
    MenuEntry::stub("Listing format...", ""),
    MenuEntry::stub("Sort order...", ""),
    MenuEntry::stub("Filter...", ""),
    MenuEntry::stub("Encoding...", "M-e"),
    MenuEntry::separator(),
    MenuEntry::stub("FTP link...", ""),
    MenuEntry::stub("Shell link...", ""),
    MenuEntry::stub("SFTP link...", ""),
    MenuEntry::action("Panelize", AppCommand::OpenPanelizeDialog),
    MenuEntry::separator(),
    MenuEntry::action_with_shortcut("Rescan", "C-r", AppCommand::Reread),
];

const FILE_MENU_ENTRIES: [MenuEntry; 22] = [
    MenuEntry::action_with_shortcut("View", "F3", AppCommand::OpenEntry),
    MenuEntry::stub("View file...", ""),
    MenuEntry::stub("Filtered view", "M-!"),
    MenuEntry::action_with_shortcut("Edit", "F4", AppCommand::EditEntry),
    MenuEntry::action_with_shortcut("Copy", "F5", AppCommand::Copy),
    MenuEntry::stub("Chmod", "C-x c"),
    MenuEntry::stub("Link", "C-x l"),
    MenuEntry::stub("Symlink", "C-x s"),
    MenuEntry::stub("Relative symlink", "C-x v"),
    MenuEntry::stub("Edit symlink", "C-x C-s"),
    MenuEntry::stub("Chown", "C-x o"),
    MenuEntry::stub("Advanced chown", ""),
    MenuEntry::action_with_shortcut("Rename/Move", "F6", AppCommand::Move),
    MenuEntry::action_with_shortcut("Mkdir", "F7", AppCommand::OpenInputDialog),
    MenuEntry::action_with_shortcut("Delete", "F8", AppCommand::Delete),
    MenuEntry::stub("Quick cd", "M-c"),
    MenuEntry::separator(),
    MenuEntry::stub("Select group", "+"),
    MenuEntry::stub("Unselect group", "-"),
    MenuEntry::action_with_shortcut("Invert selection", "*", AppCommand::InvertTags),
    MenuEntry::separator(),
    MenuEntry::action_with_shortcut("Exit", "F10", AppCommand::Quit),
];

const COMMAND_MENU_ENTRIES: [MenuEntry; 20] = [
    MenuEntry::stub("User menu", "F2"),
    MenuEntry::action("Directory tree", AppCommand::OpenTree),
    MenuEntry::action_with_literal_shortcut("Find file", "M-?", AppCommand::OpenFindDialog),
    MenuEntry::stub("Swap panels", "C-u"),
    MenuEntry::stub("Switch panels on/off", "C-o"),
    MenuEntry::stub("Compare directories", "C-x d"),
    MenuEntry::stub("Compare files", "C-x C-d"),
    MenuEntry::action_with_literal_shortcut(
        "External panelize",
        "C-x !",
        AppCommand::OpenPanelizeDialog,
    ),
    MenuEntry::stub("Show directory sizes", "C-Space"),
    MenuEntry::separator(),
    MenuEntry::stub("Command history", "M-h"),
    MenuEntry::stub("Viewed/edited files history", "M-E"),
    MenuEntry::action_with_literal_shortcut("Directory hotlist", "C-\\", AppCommand::OpenHotlist),
    MenuEntry::stub("Active VFS list", "C-x a"),
    MenuEntry::action_with_literal_shortcut("Background jobs", "C-x j", AppCommand::OpenJobsScreen),
    MenuEntry::stub("Screen list", "M-`"),
    MenuEntry::separator(),
    MenuEntry::stub("Edit extension file", ""),
    MenuEntry::stub("Edit menu file", ""),
    MenuEntry::stub("Edit highlighting group file", ""),
];

const OPTIONS_MENU_ENTRIES: [MenuEntry; 9] = [
    MenuEntry::action("Configuration...", AppCommand::OpenOptionsConfiguration),
    MenuEntry::action("Layout...", AppCommand::OpenOptionsLayout),
    MenuEntry::action("Panel options...", AppCommand::OpenOptionsPanelOptions),
    MenuEntry::action("Confirmation...", AppCommand::OpenOptionsConfirmation),
    MenuEntry::action("Appearance...", AppCommand::OpenOptionsAppearance),
    MenuEntry::action("Display bits...", AppCommand::OpenOptionsDisplayBits),
    MenuEntry::action("Learn keys...", AppCommand::OpenOptionsLearnKeys),
    MenuEntry::action("Virtual FS...", AppCommand::OpenOptionsVirtualFs),
    MenuEntry::action("Save setup", AppCommand::SaveSetup),
];

const TOP_MENUS: [TopMenu; 5] = [
    TopMenu {
        title: "Left",
        entries: &SIDE_MENU_ENTRIES,
    },
    TopMenu {
        title: "File",
        entries: &FILE_MENU_ENTRIES,
    },
    TopMenu {
        title: "Command",
        entries: &COMMAND_MENU_ENTRIES,
    },
    TopMenu {
        title: "Options",
        entries: &OPTIONS_MENU_ENTRIES,
    },
    TopMenu {
        title: "Right",
        entries: &SIDE_MENU_ENTRIES,
    },
];

pub fn top_menus() -> &'static [TopMenu] {
    &TOP_MENUS
}

pub fn top_menu_bar_items() -> Vec<MenuBarItem> {
    let mut items = Vec::with_capacity(TOP_MENUS.len());
    let mut cursor_x = 1u16;
    for (index, menu) in TOP_MENUS.iter().enumerate() {
        let title_width = menu.title.chars().count() as u16;
        let start_x = cursor_x;
        let end_x = start_x.saturating_add(title_width.saturating_sub(1));
        items.push(MenuBarItem {
            index,
            title: menu.title,
            start_x,
            end_x,
        });
        cursor_x = end_x.saturating_add(3);
    }
    items
}

pub fn top_menu_hit_test(column: u16) -> Option<usize> {
    top_menu_bar_items()
        .into_iter()
        .find(|item| column >= item.start_x && column <= item.end_x)
        .map(|item| item.index)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyResult {
    Continue,
    Quit,
}

const PANELIZE_CUSTOM_COMMAND_LABEL: &str = "<Custom command>";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SortField {
    Name,
    Size,
    Modified,
}

impl SortField {
    fn next(self) -> Self {
        match self {
            Self::Name => Self::Size,
            Self::Size => Self::Modified,
            Self::Modified => Self::Name,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Size => "size",
            Self::Modified => "mtime",
        }
    }

    fn from_settings(field: SettingsSortField) -> Self {
        match field {
            SettingsSortField::Name => Self::Name,
            SettingsSortField::Size => Self::Size,
            SettingsSortField::Modified => Self::Modified,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SortMode {
    pub field: SortField,
    pub reverse: bool,
}

impl Default for SortMode {
    fn default() -> Self {
        Self {
            field: SortField::Name,
            reverse: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivePanel {
    Left,
    Right,
}

impl ActivePanel {
    pub const fn index(self) -> usize {
        match self {
            Self::Left => 0,
            Self::Right => 1,
        }
    }

    pub fn toggle(&mut self) {
        *self = match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        };
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_parent: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

impl FileEntry {
    fn directory(name: String, path: PathBuf, size: u64, modified: Option<SystemTime>) -> Self {
        Self {
            name,
            path,
            is_dir: true,
            is_parent: false,
            size,
            modified,
        }
    }

    fn file(name: String, path: PathBuf, size: u64, modified: Option<SystemTime>) -> Self {
        Self {
            name,
            path,
            is_dir: false,
            is_parent: false,
            size,
            modified,
        }
    }

    fn parent(path: PathBuf) -> Self {
        Self {
            name: String::from(".."),
            path,
            is_dir: true,
            is_parent: true,
            size: 0,
            modified: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PanelListingSource {
    Directory,
    Panelize {
        command: String,
    },
    FindResults {
        query: String,
        base_dir: PathBuf,
        paths: Vec<PathBuf>,
    },
}

impl PanelListingSource {
    fn is_panelized(&self) -> bool {
        !matches!(self, Self::Directory)
    }
}

#[derive(Clone, Debug)]
pub struct PanelState {
    pub cwd: PathBuf,
    pub entries: Vec<FileEntry>,
    pub cursor: usize,
    pub sort_mode: SortMode,
    show_hidden_files: bool,
    source: PanelListingSource,
    tagged: HashSet<PathBuf>,
    pub loading: bool,
}

impl PanelState {
    pub fn new(cwd: PathBuf) -> io::Result<Self> {
        let mut panel = Self {
            cwd,
            entries: Vec::new(),
            cursor: 0,
            sort_mode: SortMode::default(),
            show_hidden_files: true,
            source: PanelListingSource::Directory,
            tagged: HashSet::new(),
            loading: false,
        };
        panel.refresh()?;
        Ok(panel)
    }

    pub fn refresh(&mut self) -> io::Result<()> {
        let entries = match &self.source {
            PanelListingSource::Directory => {
                read_entries_with_visibility(&self.cwd, self.sort_mode, self.show_hidden_files)?
            }
            PanelListingSource::Panelize { command } => {
                read_panelized_entries(&self.cwd, command, self.sort_mode)?
            }
            PanelListingSource::FindResults {
                base_dir, paths, ..
            } => read_panelized_paths(base_dir, paths, self.sort_mode, None)?,
        };
        self.apply_entries(entries);
        self.loading = false;
        Ok(())
    }

    fn apply_entries(&mut self, entries: Vec<FileEntry>) {
        self.entries = entries;
        self.tagged.retain(|tag| {
            self.entries
                .iter()
                .any(|entry| !entry.is_parent && entry.path == *tag)
        });
        if self.entries.is_empty() {
            self.cursor = 0;
        } else if self.cursor >= self.entries.len() {
            self.cursor = self.entries.len() - 1;
        }
    }

    pub fn move_cursor(&mut self, delta: isize) {
        if self.entries.is_empty() {
            self.cursor = 0;
            return;
        }

        let last = self.entries.len() - 1;
        let next = if delta.is_negative() {
            self.cursor.saturating_sub(delta.unsigned_abs())
        } else {
            self.cursor.saturating_add(delta as usize).min(last)
        };
        self.cursor = next;
    }

    pub fn move_cursor_page(&mut self, pages: isize, page_step: usize) {
        let delta = pages.saturating_mul(page_step as isize);
        self.move_cursor(delta);
    }

    pub fn set_show_hidden_files(&mut self, show_hidden_files: bool) {
        self.show_hidden_files = show_hidden_files;
    }

    pub fn move_cursor_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_cursor_end(&mut self) {
        if self.entries.is_empty() {
            self.cursor = 0;
        } else {
            self.cursor = self.entries.len() - 1;
        }
    }

    pub fn selected_entry(&self) -> Option<&FileEntry> {
        self.entries.get(self.cursor)
    }

    pub fn tagged_count(&self) -> usize {
        self.tagged.len()
    }

    pub fn is_tagged(&self, path: &Path) -> bool {
        self.tagged.contains(path)
    }

    pub fn toggle_tag_on_cursor(&mut self) -> bool {
        let Some(entry) = self.selected_entry() else {
            return false;
        };
        if entry.is_parent {
            return false;
        }
        let path = entry.path.clone();

        if self.tagged.contains(&path) {
            self.tagged.remove(&path);
            false
        } else {
            self.tagged.insert(path);
            true
        }
    }

    pub fn invert_tags(&mut self) {
        let mut next_tags = HashSet::new();
        for entry in &self.entries {
            if entry.is_parent {
                continue;
            }
            if !self.tagged.contains(&entry.path) {
                next_tags.insert(entry.path.clone());
            }
        }
        self.tagged = next_tags;
    }

    pub fn tagged_paths_in_display_order(&self) -> Vec<PathBuf> {
        self.entries
            .iter()
            .filter(|entry| !entry.is_parent && self.tagged.contains(&entry.path))
            .map(|entry| entry.path.clone())
            .collect()
    }

    pub fn sort_label(&self) -> String {
        format!(
            "{} {}",
            self.sort_mode.field.label(),
            if self.sort_mode.reverse {
                "desc"
            } else {
                "asc"
            }
        )
    }

    pub fn cycle_sort_field(&mut self) {
        self.sort_mode.field = self.sort_mode.field.next();
    }

    pub fn toggle_sort_direction(&mut self) {
        self.sort_mode.reverse = !self.sort_mode.reverse;
    }

    pub fn open_selected_directory(&mut self) -> bool {
        let Some((path, is_dir_hint)) = self
            .selected_entry()
            .map(|entry| (entry.path.clone(), entry.is_dir))
        else {
            return false;
        };
        let is_dir = is_dir_hint || fs::metadata(&path).is_ok_and(|metadata| metadata.is_dir());
        if !is_dir {
            return false;
        }

        self.cwd = path;
        self.cursor = 0;
        self.source = PanelListingSource::Directory;
        self.tagged.clear();
        self.entries.clear();
        self.loading = true;
        true
    }

    pub fn go_parent(&mut self) -> bool {
        let Some(parent) = self.cwd.parent() else {
            return false;
        };

        self.cwd = parent.to_path_buf();
        self.cursor = 0;
        self.source = PanelListingSource::Directory;
        self.tagged.clear();
        self.entries.clear();
        self.loading = true;
        true
    }

    pub fn exit_panelize(&mut self) -> bool {
        if !self.source.is_panelized() {
            return false;
        }

        self.cursor = 0;
        self.source = PanelListingSource::Directory;
        self.tagged.clear();
        self.entries.clear();
        self.loading = true;
        true
    }

    pub fn panelize_with_command(&mut self, command: String) -> io::Result<usize> {
        let previous_source = self.source.clone();
        self.source = PanelListingSource::Panelize { command };
        self.cursor = 0;
        self.tagged.clear();

        if let Err(error) = self.refresh() {
            self.source = previous_source;
            return Err(error);
        }

        Ok(self.entries.len())
    }

    pub fn panelize_command(&self) -> Option<&str> {
        match &self.source {
            PanelListingSource::Panelize { command } => Some(command.as_str()),
            _ => None,
        }
    }

    pub fn is_panelized(&self) -> bool {
        self.source.is_panelized()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FindResultEntry {
    pub path: PathBuf,
    pub is_dir: bool,
}

#[derive(Clone, Debug)]
pub struct FindResultsState {
    pub job_id: JobId,
    pub query: String,
    pub base_dir: PathBuf,
    pub entries: Vec<FindResultEntry>,
    pub cursor: usize,
    pub loading: bool,
}

impl FindResultsState {
    fn loading(job_id: JobId, query: String, base_dir: PathBuf) -> Self {
        Self {
            job_id,
            query,
            base_dir,
            entries: Vec::new(),
            cursor: 0,
            loading: true,
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        if self.entries.is_empty() {
            self.cursor = 0;
            return;
        }

        let last = self.entries.len() - 1;
        let next = if delta.is_negative() {
            self.cursor.saturating_sub(delta.unsigned_abs())
        } else {
            self.cursor.saturating_add(delta as usize).min(last)
        };
        self.cursor = next;
    }

    fn move_page(&mut self, pages: isize, page_step: usize) {
        self.move_cursor(pages.saturating_mul(page_step as isize));
    }

    fn move_home(&mut self) {
        self.cursor = 0;
    }

    fn move_end(&mut self) {
        if self.entries.is_empty() {
            self.cursor = 0;
        } else {
            self.cursor = self.entries.len() - 1;
        }
    }

    fn selected_entry(&self) -> Option<&FindResultEntry> {
        self.entries.get(self.cursor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeEntry {
    pub path: PathBuf,
    pub depth: usize,
}

#[derive(Clone, Debug)]
pub struct TreeState {
    pub root: PathBuf,
    pub entries: Vec<TreeEntry>,
    pub cursor: usize,
    pub loading: bool,
}

impl TreeState {
    fn loading(root: PathBuf) -> Self {
        let entries = vec![TreeEntry {
            path: root.clone(),
            depth: 0,
        }];
        Self {
            root,
            entries,
            cursor: 0,
            loading: true,
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        if self.entries.is_empty() {
            self.cursor = 0;
            return;
        }

        let last = self.entries.len() - 1;
        let next = if delta.is_negative() {
            self.cursor.saturating_sub(delta.unsigned_abs())
        } else {
            self.cursor.saturating_add(delta as usize).min(last)
        };
        self.cursor = next;
    }

    fn move_page(&mut self, pages: isize, page_step: usize) {
        self.move_cursor(pages.saturating_mul(page_step as isize));
    }

    fn move_home(&mut self) {
        self.cursor = 0;
    }

    fn move_end(&mut self) {
        if self.entries.is_empty() {
            self.cursor = 0;
        } else {
            self.cursor = self.entries.len() - 1;
        }
    }

    fn selected_entry(&self) -> Option<&TreeEntry> {
        self.entries.get(self.cursor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MenuState {
    pub active_menu: usize,
    pub selected_entry: usize,
}

impl MenuState {
    fn new(active_menu: usize) -> Self {
        let mut state = Self {
            active_menu: 0,
            selected_entry: 0,
        };
        state.set_active_menu(active_menu);
        state
    }

    pub fn active_menu_title(&self) -> &'static str {
        self.active_menu().title
    }

    pub fn active_entries(&self) -> &'static [MenuEntry] {
        self.active_menu().entries
    }

    pub fn popup_origin_x(&self) -> u16 {
        top_menu_bar_items()
            .into_iter()
            .find(|item| item.index == self.active_menu)
            .map(|item| item.start_x.saturating_sub(1))
            .unwrap_or(0)
    }

    pub fn popup_height(&self) -> u16 {
        self.active_entries().len() as u16 + 2
    }

    fn set_active_menu(&mut self, active_menu: usize) {
        self.active_menu = active_menu.min(TOP_MENUS.len().saturating_sub(1));
        self.selected_entry = self.first_selectable_entry().unwrap_or(0);
        self.clamp_selected_entry();
    }

    fn move_up(&mut self) {
        self.move_to_adjacent_selectable(-1);
    }

    fn move_down(&mut self) {
        self.move_to_adjacent_selectable(1);
    }

    fn move_left(&mut self) {
        let next = if self.active_menu == 0 {
            TOP_MENUS.len() - 1
        } else {
            self.active_menu - 1
        };
        self.set_active_menu(next);
    }

    fn move_right(&mut self) {
        self.set_active_menu((self.active_menu + 1) % TOP_MENUS.len());
    }

    fn move_home(&mut self) {
        self.selected_entry = self.first_selectable_entry().unwrap_or(0);
    }

    fn move_end(&mut self) {
        self.selected_entry = self.last_selectable_entry().unwrap_or(0);
    }

    fn select_entry(&mut self, index: usize) {
        self.selected_entry = index;
        self.clamp_selected_entry();
    }

    fn selected_command(&self) -> Option<AppCommand> {
        self.active_entries()
            .get(self.selected_entry)
            .filter(|entry| entry.selectable)
            .map(|entry| entry.command)
    }

    fn active_menu(&self) -> &'static TopMenu {
        TOP_MENUS.get(self.active_menu).unwrap_or(&TOP_MENUS[0])
    }

    fn clamp_selected_entry(&mut self) {
        if self.active_entries().is_empty() {
            self.selected_entry = 0;
        } else if self.selected_entry >= self.active_entries().len() {
            self.selected_entry = self.active_entries().len() - 1;
        }

        if self
            .active_entries()
            .get(self.selected_entry)
            .is_none_or(|entry| !entry.selectable)
        {
            self.selected_entry = self.first_selectable_entry().unwrap_or(0);
        }
    }

    fn first_selectable_entry(&self) -> Option<usize> {
        self.active_entries()
            .iter()
            .position(|entry| entry.selectable)
    }

    fn last_selectable_entry(&self) -> Option<usize> {
        self.active_entries()
            .iter()
            .rposition(|entry| entry.selectable)
    }

    fn move_to_adjacent_selectable(&mut self, direction: isize) {
        let entries = self.active_entries();
        if entries.is_empty() || direction == 0 {
            self.selected_entry = 0;
            return;
        }

        let mut index = self.selected_entry as isize;
        loop {
            let next = index + direction;
            if next < 0 || next >= entries.len() as isize {
                break;
            }
            index = next;
            if entries[index as usize].selectable {
                self.selected_entry = index as usize;
                return;
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingsScreenState {
    pub category: SettingsCategory,
    pub title: String,
    pub entries: Vec<SettingsEntry>,
    pub selected_entry: usize,
}

impl SettingsScreenState {
    fn new(category: SettingsCategory, entries: Vec<SettingsEntry>) -> Self {
        Self {
            category,
            title: format!("{} options", category.label()),
            entries,
            selected_entry: 0,
        }
    }

    fn move_up(&mut self) {
        self.selected_entry = self.selected_entry.saturating_sub(1);
    }

    fn move_down(&mut self) {
        if self.entries.is_empty() {
            self.selected_entry = 0;
            return;
        }
        self.selected_entry = self
            .selected_entry
            .saturating_add(1)
            .min(self.entries.len().saturating_sub(1));
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingsEntry {
    pub label: String,
    pub value: String,
    action: SettingsEntryAction,
}

impl SettingsEntry {
    fn new(
        label: impl Into<String>,
        value: impl Into<String>,
        action: SettingsEntryAction,
    ) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            action,
        }
    }

    pub fn text(&self) -> String {
        if self.value.is_empty() {
            return self.label.clone();
        }
        format!("{}: {}", self.label, self.value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SettingsEntryAction {
    ToggleUseInternalEditor,
    CycleDefaultOverwritePolicy,
    ToggleMacosOptionSymbols,
    ToggleLayoutShowMenuBar,
    ToggleLayoutShowButtonBar,
    ToggleLayoutShowDebugStatus,
    ToggleLayoutShowPanelTotals,
    CycleLayoutStatusMessageTimeout,
    TogglePanelShowHiddenFiles,
    CyclePanelSortField,
    TogglePanelSortReverse,
    ToggleConfirmDelete,
    ToggleConfirmOverwrite,
    ToggleConfirmQuit,
    OpenSkinDialog,
    ToggleUtf8Output,
    ToggleEightBitInput,
    LearnKeysCapture,
    ToggleVfsEnabled,
    ToggleVfsFtpEnabled,
    ToggleVfsShellLinkEnabled,
    ToggleVfsSftpEnabled,
    Info,
}

#[derive(Clone, Debug)]
pub enum Route {
    FileManager,
    Help(HelpState),
    Menu(MenuState),
    Settings(SettingsScreenState),
    Jobs,
    Viewer(ViewerState),
    FindResults(FindResultsState),
    Tree(TreeState),
    Hotlist,
    Dialog(DialogState),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransferKind {
    Copy,
    Move,
}

#[derive(Clone, Debug)]
enum PendingDialogAction {
    ConfirmDelete {
        targets: Vec<PathBuf>,
    },
    ConfirmQuit,
    Mkdir {
        base_dir: PathBuf,
    },
    RenameEntry {
        source: PathBuf,
    },
    TransferDestination {
        kind: TransferKind,
        sources: Vec<PathBuf>,
        source_base_dir: PathBuf,
    },
    TransferOverwrite {
        kind: TransferKind,
        sources: Vec<PathBuf>,
        destination_dir: PathBuf,
    },
    SetDefaultOverwritePolicy,
    SetSkin {
        original_skin: String,
    },
    ViewerSearch {
        direction: ViewerSearchDirection,
    },
    ViewerGoto,
    FindQuery {
        base_dir: PathBuf,
    },
    PanelizePresetSelection {
        initial_command: String,
        preset_commands: Vec<String>,
    },
    PanelizeCommand {
        preset_commands: Vec<String>,
    },
    PanelizePresetAdd {
        initial_command: String,
        preset_commands: Vec<String>,
    },
    PanelizePresetEdit {
        initial_command: String,
        preset_commands: Vec<String>,
        preset_index: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalEditRequest {
    pub editor_command: String,
    pub path: PathBuf,
    pub cwd: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EditSelectionResult {
    OpenedExternal,
    OpenedInternal,
    NoEntrySelected,
    SelectedEntryIsDirectory,
}

#[derive(Clone, Debug, Default)]
struct KeybindingHints {
    labels_by_context_and_command: HashMap<(KeyContext, AppCommand), Vec<String>>,
}

impl KeybindingHints {
    fn from_keymap(keymap: &Keymap) -> Self {
        let mut chords_by_context_and_command: HashMap<(KeyContext, AppCommand), Vec<KeyChord>> =
            HashMap::new();
        let contexts = [
            KeyContext::FileManager,
            KeyContext::FileManagerXMap,
            KeyContext::Help,
            KeyContext::Jobs,
            KeyContext::FindResults,
            KeyContext::Tree,
            KeyContext::Hotlist,
            KeyContext::Dialog,
            KeyContext::Input,
            KeyContext::Listbox,
            KeyContext::Menu,
            KeyContext::Editor,
            KeyContext::Viewer,
            KeyContext::ViewerHex,
            KeyContext::DiffViewer,
        ];

        for context in contexts {
            for (chord, key_command) in keymap.bindings_for_context(context) {
                let app_command =
                    AppCommand::from_key_command(context, &key_command).or_else(|| {
                        (context == KeyContext::FileManagerXMap)
                            .then(|| {
                                AppCommand::from_key_command(KeyContext::FileManager, &key_command)
                            })
                            .flatten()
                    });
                let Some(app_command) = app_command else {
                    continue;
                };
                chords_by_context_and_command
                    .entry((context, app_command))
                    .or_default()
                    .push(chord);
            }
        }

        let mut labels_by_context_and_command = HashMap::new();
        for ((context, app_command), mut chords) in chords_by_context_and_command {
            chords.sort_by_key(key_chord_sort_key);
            let mut labels = Vec::new();
            for chord in chords {
                let label = format_key_chord(chord);
                if !labels.iter().any(|existing| existing == &label) {
                    labels.push(label);
                }
            }
            if !labels.is_empty() {
                labels_by_context_and_command.insert((context, app_command), labels);
            }
        }

        Self {
            labels_by_context_and_command,
        }
    }

    fn labels_for(&self, context: KeyContext, command: AppCommand) -> Option<&[String]> {
        self.labels_by_context_and_command
            .get(&(context, command))
            .map(Vec::as_slice)
    }
}

#[derive(Debug)]
pub struct AppState {
    settings: Settings,
    pub panels: [PanelState; 2],
    pub active_panel: ActivePanel,
    pub status_line: String,
    status_expires_at: Option<Instant>,
    pub last_dialog_result: Option<DialogResult>,
    pub jobs: JobManager,
    pub overwrite_policy: OverwritePolicy,
    pub jobs_cursor: usize,
    pub hotlist: Vec<PathBuf>,
    pub hotlist_cursor: usize,
    available_skins: Vec<String>,
    active_skin_name: String,
    pending_skin_change: Option<String>,
    pending_skin_preview: Option<String>,
    pending_skin_revert: Option<String>,
    routes: Vec<Route>,
    paused_find_results: Option<FindResultsState>,
    pending_dialog_action: Option<PendingDialogAction>,
    pending_worker_commands: Vec<WorkerCommand>,
    pending_external_edit_requests: Vec<ExternalEditRequest>,
    panel_refresh_job_ids: [Option<JobId>; 2],
    panel_refresh_request_ids: [u64; 2],
    panel_refresh_partial_entry_count: [usize; 2],
    next_panel_refresh_request_id: u64,
    pending_panel_focus: Option<(ActivePanel, PathBuf)>,
    find_pause_flags: HashMap<JobId, Arc<AtomicBool>>,
    pending_panelize_revert: Option<(ActivePanel, PanelListingSource)>,
    deferred_persist_settings_request: Option<JobRequest>,
    panelize_presets: Vec<String>,
    keybinding_hints: KeybindingHints,
    keymap_unknown_actions: usize,
    keymap_invalid_bindings: usize,
    pending_learn_keys_capture: bool,
    xmap_pending: bool,
    pending_save_setup: bool,
    pending_quit: bool,
}

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
            overwrite_policy: settings.configuration.default_overwrite_policy,
            jobs_cursor: 0,
            hotlist: settings.configuration.hotlist.clone(),
            hotlist_cursor: 0,
            available_skins: Vec::new(),
            active_skin_name: settings.appearance.skin.clone(),
            pending_skin_change: None,
            pending_skin_preview: None,
            pending_skin_revert: None,
            routes: vec![Route::FileManager],
            paused_find_results: None,
            pending_dialog_action: None,
            pending_worker_commands: Vec::new(),
            pending_external_edit_requests: Vec::new(),
            panel_refresh_job_ids: [None; 2],
            panel_refresh_request_ids: [0; 2],
            panel_refresh_partial_entry_count: [0; 2],
            next_panel_refresh_request_id: 1,
            pending_panel_focus: None,
            find_pause_flags: HashMap::new(),
            pending_panelize_revert: None,
            deferred_persist_settings_request: None,
            panelize_presets: settings.configuration.panelize_presets.clone(),
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
        let mut settings = self.settings.clone();
        settings.configuration.default_overwrite_policy = self.overwrite_policy;
        settings.configuration.hotlist = self.hotlist.clone();
        settings.configuration.panelize_presets = self.panelize_presets.clone();
        settings.appearance.skin = self.active_skin_name.clone();
        settings
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

    pub fn disk_usage_cache_ttl(&self) -> Duration {
        Duration::from_millis(self.settings.advanced.disk_usage_cache_ttl_ms)
    }

    pub fn disk_usage_cache_max_entries(&self) -> usize {
        self.settings.advanced.disk_usage_cache_max_entries
    }

    pub fn replace_settings(&mut self, settings: Settings) {
        self.settings = settings;
        self.overwrite_policy = self.settings.configuration.default_overwrite_policy;
        self.hotlist = self.settings.configuration.hotlist.clone();
        self.hotlist_cursor = self
            .hotlist_cursor
            .min(self.hotlist.len().saturating_sub(1));
        self.panelize_presets = self.settings.configuration.panelize_presets.clone();
        self.active_skin_name = self.settings.appearance.skin.clone();
        self.status_expires_at = self
            .status_message_timeout()
            .and_then(|timeout| Instant::now().checked_add(timeout))
            .filter(|_| !self.status_line.is_empty());

        let sort_mode = self.default_panel_sort_mode();
        let show_hidden_files = self.settings.panel_options.show_hidden_files;
        for panel in &mut self.panels {
            panel.sort_mode = sort_mode;
            panel.set_show_hidden_files(show_hidden_files);
            let _ = panel.refresh();
        }
    }

    fn default_panel_sort_mode(&self) -> SortMode {
        SortMode {
            field: SortField::from_settings(self.settings.panel_options.sort_field),
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

    fn open_selected_file_in_editor(&mut self) -> EditSelectionResult {
        self.open_selected_file_in_editor_with_resolver(resolve_external_editor_command)
    }

    fn open_selected_file_in_editor_with_resolver(
        &mut self,
        mut resolve_external_editor: impl FnMut() -> Option<String>,
    ) -> EditSelectionResult {
        let Some((path, is_dir)) = self
            .selected_non_parent_entry()
            .map(|entry| (entry.path.clone(), entry.is_dir))
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

        self.queue_worker_job_request(JobRequest::LoadViewer { path });
        EditSelectionResult::OpenedInternal
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

    fn expire_status_line_at(&mut self, now: Instant) {
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
        self.active_skin_name = skin_name.into();
        self.refresh_settings_entries();
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

    fn help_replacements(&self) -> HashMap<&'static str, String> {
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

    fn selected_operation_paths(&self) -> Vec<PathBuf> {
        let tagged = self.active_panel().tagged_paths_in_display_order();
        if !tagged.is_empty() {
            return tagged;
        }

        self.active_panel()
            .selected_entry()
            .filter(|entry| !entry.is_parent)
            .map(|entry| vec![entry.path.clone()])
            .unwrap_or_default()
    }

    fn selected_non_parent_entry(&self) -> Option<&FileEntry> {
        self.active_panel()
            .selected_entry()
            .filter(|entry| !entry.is_parent)
    }
}

fn normalize_status_message(message: String) -> String {
    let mut normalized = String::new();
    let mut count = 0_usize;
    let mut truncated = false;

    for ch in message.chars() {
        if count >= MAX_STATUS_LINE_CHARS {
            truncated = true;
            break;
        }
        let normalized_ch = if ch == '\n' || ch == '\r' || ch == '\t' || ch.is_control() {
            ' '
        } else {
            ch
        };
        normalized.push(normalized_ch);
        count = count.saturating_add(1);
    }

    if truncated {
        normalized.push_str("...");
    }
    normalized
}

pub(crate) fn build_tree_entries(
    root: &Path,
    max_depth: usize,
    max_entries: usize,
) -> Vec<TreeEntry> {
    if max_entries == 0 {
        return Vec::new();
    }

    let root = root.to_path_buf();
    let mut entries = vec![TreeEntry {
        path: root.clone(),
        depth: 0,
    }];

    let mut stack = vec![(root, 0usize)];
    while let Some((directory, depth)) = stack.pop() {
        if depth >= max_depth || entries.len() >= max_entries {
            continue;
        }

        let read_dir = match fs::read_dir(&directory) {
            Ok(read_dir) => read_dir,
            Err(_) => continue,
        };
        let mut child_dirs = Vec::new();

        for entry in read_dir.flatten() {
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                child_dirs.push(entry.path());
            }
        }

        child_dirs.sort_by_key(|left| path_sort_key(left));

        for child_dir in child_dirs.into_iter().rev() {
            if entries.len() >= max_entries {
                return entries;
            }

            entries.push(TreeEntry {
                path: child_dir.clone(),
                depth: depth + 1,
            });
            stack.push((child_dir, depth + 1));
        }
    }

    entries
}

fn path_sort_key(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| path.to_string_lossy().to_lowercase())
}

fn key_chord_sort_key(chord: &KeyChord) -> (u8, u16, String) {
    let has_ctrl_or_alt = chord.modifiers.ctrl || chord.modifiers.alt;
    let has_any_modifiers = chord.modifiers.ctrl || chord.modifiers.alt || chord.modifiers.shift;
    let rank = match chord.code {
        KeyCode::F(_) if !has_any_modifiers => 0,
        KeyCode::F(_) => 1,
        _ if has_ctrl_or_alt => 2,
        KeyCode::Enter
        | KeyCode::Esc
        | KeyCode::Tab
        | KeyCode::Backspace
        | KeyCode::Up
        | KeyCode::Down
        | KeyCode::Left
        | KeyCode::Right
        | KeyCode::Home
        | KeyCode::End
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::Insert
        | KeyCode::Delete => 3,
        KeyCode::Char(ch) if ch.is_ascii_alphabetic() && !has_any_modifiers => 4,
        KeyCode::Char(_) if !has_any_modifiers => 5,
        KeyCode::Char(_) => 6,
    };

    let number = match chord.code {
        KeyCode::F(value) => value as u16,
        _ => 0,
    };
    (rank, number, format_key_chord(*chord))
}

fn format_key_chord(chord: KeyChord) -> String {
    let key = match chord.code {
        KeyCode::Char(ch) => ch.to_string(),
        KeyCode::F(number) => format!("F{number}"),
        KeyCode::Enter => String::from("Enter"),
        KeyCode::Esc => String::from("Esc"),
        KeyCode::Tab => String::from("Tab"),
        KeyCode::Backspace => String::from("Backspace"),
        KeyCode::Up => String::from("Up"),
        KeyCode::Down => String::from("Down"),
        KeyCode::Left => String::from("Left"),
        KeyCode::Right => String::from("Right"),
        KeyCode::Home => String::from("Home"),
        KeyCode::End => String::from("End"),
        KeyCode::PageUp => String::from("PgUp"),
        KeyCode::PageDown => String::from("PgDn"),
        KeyCode::Insert => String::from("Insert"),
        KeyCode::Delete => String::from("Delete"),
    };

    let mut modifiers = Vec::new();
    if chord.modifiers.ctrl {
        modifiers.push("Ctrl");
    }
    if chord.modifiers.alt {
        modifiers.push("Alt");
    }
    if chord.modifiers.shift {
        modifiers.push("Shift");
    }

    if modifiers.is_empty() {
        key
    } else {
        format!("{}-{key}", modifiers.join("-"))
    }
}

fn resolve_external_editor_command() -> Option<String> {
    resolve_external_editor_command_with_lookup(|name| std::env::var(name).ok())
}

fn resolve_external_editor_command_with_lookup(
    mut lookup_env: impl FnMut(&str) -> Option<String>,
) -> Option<String> {
    for variable in ["EDITOR", "VISUAL"] {
        if let Some(value) = lookup_env(variable)
            && let Some(trimmed) = non_empty_env_value(&value)
        {
            return Some(trimmed.to_string());
        }
    }
    None
}

fn non_empty_env_value(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::KeyModifiers;
    use std::path::Path;
    use std::process::Output;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::thread;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};
    use std::{env, fs};

    fn file_entry(name: &str) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: PathBuf::from(name),
            is_dir: false,
            is_parent: false,
            size: 0,
            modified: None,
        }
    }

    struct PermissionDeniedProcessBackend;

    impl ProcessBackend for PermissionDeniedProcessBackend {
        fn run_shell_command(
            &self,
            _cwd: &Path,
            _command: &str,
            _cancel_flag: Option<&AtomicBool>,
            _canceled_message: &str,
        ) -> io::Result<Output> {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "permission denied",
            ))
        }
    }

    fn drain_background(app: &mut AppState) {
        loop {
            let mut progressed = false;

            let worker_commands = app.take_pending_worker_commands();
            if !worker_commands.is_empty() {
                progressed = true;
            }
            for command in worker_commands {
                match command {
                    WorkerCommand::Run(job) => {
                        let job = *job;
                        let job_id = job.id;
                        let (event_tx, event_rx) = std::sync::mpsc::channel();
                        match &job.request {
                            JobRequest::RefreshPanel {
                                panel,
                                cwd,
                                source,
                                sort_mode,
                                show_hidden_files,
                                request_id,
                            } => {
                                let _ = event_tx.send(JobEvent::Started { id: job_id });
                                let cancel_flag = job.cancel_flag();
                                app.handle_background_event(refresh_panel_event(
                                    *panel,
                                    cwd.clone(),
                                    source.clone(),
                                    *sort_mode,
                                    *show_hidden_files,
                                    *request_id,
                                    cancel_flag.as_ref(),
                                ));
                                let _ = event_tx.send(JobEvent::Finished {
                                    id: job_id,
                                    result: Ok(()),
                                });
                            }
                            JobRequest::Find {
                                query,
                                base_dir,
                                max_results,
                            } => {
                                let query = query.clone();
                                let base_dir = base_dir.clone();
                                let max_results = *max_results;
                                let cancel_flag = job.cancel_flag();
                                let pause_flag = job
                                    .find_pause_flag()
                                    .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
                                let _ = event_tx.send(JobEvent::Started { id: job_id });
                                let (chunk_tx, chunk_rx) = std::sync::mpsc::channel();
                                let result = run_find_entries(
                                    &base_dir,
                                    &query,
                                    max_results,
                                    cancel_flag.as_ref(),
                                    pause_flag.as_ref(),
                                    |entries| {
                                        chunk_tx
                                            .send(BackgroundEvent::FindEntriesChunk {
                                                job_id,
                                                entries,
                                            })
                                            .is_ok()
                                    },
                                )
                                .map_err(JobError::from_message);
                                for event in chunk_rx.try_iter() {
                                    app.handle_background_event(event);
                                }
                                let _ = event_tx.send(JobEvent::Finished { id: job_id, result });
                            }
                            JobRequest::LoadViewer { path } => {
                                let _ = event_tx.send(JobEvent::Started { id: job_id });
                                let viewer_result = ViewerState::open(path.clone())
                                    .map_err(|error| error.to_string());
                                app.handle_background_event(BackgroundEvent::ViewerLoaded {
                                    path: path.clone(),
                                    result: viewer_result.clone(),
                                });
                                let result =
                                    viewer_result.map(|_| ()).map_err(JobError::from_message);
                                let _ = event_tx.send(JobEvent::Finished { id: job_id, result });
                            }
                            JobRequest::BuildTree {
                                root,
                                max_depth,
                                max_entries,
                            } => {
                                let _ = event_tx.send(JobEvent::Started { id: job_id });
                                app.handle_background_event(build_tree_ready_event(
                                    root.clone(),
                                    *max_depth,
                                    *max_entries,
                                ));
                                let _ = event_tx.send(JobEvent::Finished {
                                    id: job_id,
                                    result: Ok(()),
                                });
                            }
                            _ => {
                                execute_worker_job(job, &event_tx);
                            }
                        }
                        for event in event_rx.try_iter() {
                            app.handle_job_event(event);
                        }
                    }
                    WorkerCommand::Cancel(_) | WorkerCommand::Shutdown => {}
                }
            }

            if !progressed {
                break;
            }
        }
    }

    #[test]
    fn panelized_entries_allow_process_backend_injection() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-panelize-backend-injection-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let backend = PermissionDeniedProcessBackend;
        let error = read_panelized_entries_with_process_backend(
            &root,
            "ignored",
            SortMode::default(),
            None,
            &backend,
        )
        .expect_err("injected process backend should drive panelize execution");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    fn move_menu_selection_to_label(app: &mut AppState, label: &str) {
        let len = match app.top_route() {
            Route::Menu(menu) => menu.active_entries().len(),
            _ => panic!("menu route should be active"),
        };
        for _ in 0..len {
            let matches_target = match app.top_route() {
                Route::Menu(menu) => menu
                    .active_entries()
                    .get(menu.selected_entry)
                    .is_some_and(|entry| entry.label == label),
                _ => false,
            };
            if matches_target {
                return;
            }
            app.apply(AppCommand::MenuMoveDown)
                .expect("menu movement should succeed");
        }
        panic!("menu entry '{label}' should exist");
    }

    fn submit_panelize_custom_command(app: &mut AppState, command: &str) {
        app.open_panelize_dialog();
        app.finish_dialog(DialogResult::ListboxSubmitted {
            index: Some(0),
            value: Some(String::from(PANELIZE_CUSTOM_COMMAND_LABEL)),
        });
        app.finish_dialog(DialogResult::InputSubmitted(command.to_string()));
    }

    #[test]
    fn toggle_panel_flips_between_left_and_right() {
        let mut panel = ActivePanel::Left;
        panel.toggle();
        assert_eq!(panel, ActivePanel::Right);
        panel.toggle();
        assert_eq!(panel, ActivePanel::Left);
    }

    #[test]
    fn move_cursor_stays_in_bounds() {
        let mut panel = PanelState {
            cwd: PathBuf::from("/tmp"),
            entries: vec![file_entry("a"), file_entry("b")],
            cursor: 0,
            sort_mode: SortMode::default(),
            show_hidden_files: true,
            source: PanelListingSource::Directory,
            tagged: HashSet::new(),
            loading: false,
        };

        panel.move_cursor(-1);
        assert_eq!(panel.cursor, 0);

        panel.move_cursor(99);
        assert_eq!(panel.cursor, 1);
    }

    #[test]
    fn panel_listing_prepends_parent_entry() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-parent-entry-{stamp}"));
        let child = root.join("child");

        fs::create_dir_all(&child).expect("must create child directory");
        fs::write(child.join("a.txt"), "x").expect("must create child file");

        let panel = PanelState::new(child.clone()).expect("panel should initialize");
        let first = panel.entries.first().expect("entries should not be empty");
        assert_eq!(first.name, "..");
        assert!(first.is_parent);
        assert!(first.is_dir);
        assert_eq!(first.path, root);

        fs::remove_dir_all(&root).expect("must remove temp tree");
    }

    #[cfg(unix)]
    #[test]
    fn listing_marks_directory_symlinks_as_directories() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-dir-symlink-listing-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let target_dir = root.join("target-dir");
        fs::create_dir_all(&target_dir).expect("must create target directory");
        let symlink_path = root.join("tmp-like");
        std::os::unix::fs::symlink(&target_dir, &symlink_path)
            .expect("directory symlink should be creatable");

        let entries = read_entries(&root, SortMode::default()).expect("listing should load");
        let symlink_entry = entries
            .iter()
            .find(|entry| entry.path == symlink_path)
            .expect("directory symlink should be listed");
        assert!(
            symlink_entry.is_dir,
            "directory symlink should be classified as a directory"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn name_sort_listing_populates_metadata_fields() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-name-sort-metadata-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let file_path = root.join("entry.txt");
        fs::write(&file_path, "payload").expect("must create source file");

        let entries = read_entries(
            &root,
            SortMode {
                field: SortField::Name,
                reverse: false,
            },
        )
        .expect("listing should load");
        let file_entry = entries
            .iter()
            .find(|entry| entry.path == file_path)
            .expect("file entry should be present");
        assert!(
            file_entry.size >= 7,
            "name sort should include file metadata size"
        );
        assert!(
            file_entry.modified.is_some(),
            "name sort should include file metadata mtime"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn size_sort_listing_populates_metadata_fields() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-size-sort-metadata-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let file_path = root.join("entry.txt");
        fs::write(&file_path, "payload").expect("must create source file");

        let entries = read_entries(
            &root,
            SortMode {
                field: SortField::Size,
                reverse: false,
            },
        )
        .expect("listing should load");
        let file_entry = entries
            .iter()
            .find(|entry| entry.path == file_path)
            .expect("file entry should be present");
        assert!(
            file_entry.size >= 7,
            "size sort should include file metadata size"
        );
        assert!(
            file_entry.modified.is_some(),
            "size sort should include file metadata mtime"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn toggle_and_invert_tags_work_for_non_parent_entries() {
        let mut panel = PanelState {
            cwd: PathBuf::from("/tmp"),
            entries: vec![
                FileEntry::parent(PathBuf::from("/")),
                file_entry("a"),
                file_entry("b"),
            ],
            cursor: 0,
            sort_mode: SortMode::default(),
            show_hidden_files: true,
            source: PanelListingSource::Directory,
            tagged: HashSet::new(),
            loading: false,
        };

        assert!(
            !panel.toggle_tag_on_cursor(),
            "parent entry should not be taggable"
        );
        assert_eq!(panel.tagged_count(), 0);

        panel.cursor = 1;
        assert!(panel.toggle_tag_on_cursor());
        assert_eq!(panel.tagged_count(), 1);
        assert!(panel.is_tagged(Path::new("a")));

        panel.invert_tags();
        assert_eq!(panel.tagged_count(), 1);
        assert!(panel.is_tagged(Path::new("b")));
        assert!(!panel.is_tagged(Path::new("a")));
    }

    #[test]
    fn page_home_end_navigation_stays_bounded() {
        let entries = vec![
            FileEntry::parent(PathBuf::from("/tmp")),
            file_entry("a"),
            file_entry("b"),
            file_entry("c"),
        ];
        let mut panel = PanelState {
            cwd: PathBuf::from("/tmp"),
            entries,
            cursor: 1,
            sort_mode: SortMode::default(),
            show_hidden_files: true,
            source: PanelListingSource::Directory,
            tagged: HashSet::new(),
            loading: false,
        };

        panel.move_cursor_home();
        assert_eq!(panel.cursor, 0);

        panel.move_cursor_end();
        assert_eq!(panel.cursor, 3);

        panel.move_cursor_page(1, 10);
        assert_eq!(panel.cursor, 3);

        panel.move_cursor_page(-1, 10);
        assert_eq!(panel.cursor, 0);
    }

    #[test]
    fn sort_mode_cycles_and_toggles_direction() {
        let mut panel = PanelState {
            cwd: PathBuf::from("/tmp"),
            entries: Vec::new(),
            cursor: 0,
            sort_mode: SortMode::default(),
            show_hidden_files: true,
            source: PanelListingSource::Directory,
            tagged: HashSet::new(),
            loading: false,
        };

        panel.sort_mode.field = SortField::Name;
        panel.sort_mode.reverse = false;
        assert_eq!(panel.sort_label(), "name asc");

        panel.sort_mode.field = panel.sort_mode.field.next();
        assert_eq!(panel.sort_mode.field, SortField::Size);

        panel.sort_mode.reverse = true;
        assert_eq!(panel.sort_label(), "size desc");
    }

    #[test]
    fn toggle_tag_advances_cursor_to_next_entry() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-toggle-tag-cursor-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let alpha = root.join("alpha.txt");
        let bravo = root.join("bravo.txt");
        fs::write(&alpha, "a").expect("must create alpha file");
        fs::write(&bravo, "b").expect("must create bravo file");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let alpha_index = app
            .active_panel()
            .entries
            .iter()
            .position(|entry| entry.path == alpha)
            .expect("alpha entry should be visible");
        app.active_panel_mut().cursor = alpha_index;

        app.apply(AppCommand::ToggleTag)
            .expect("toggle tag should succeed");

        assert!(
            app.active_panel().is_tagged(&alpha),
            "alpha should be tagged after toggle"
        );
        assert_eq!(
            app.active_panel().cursor,
            alpha_index + 1,
            "cursor should advance to the next entry"
        );
        let selected = app
            .active_panel()
            .selected_entry()
            .expect("next entry should be selected");
        assert_eq!(
            selected.path, bravo,
            "cursor should land on the next file entry"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn mkdir_dialog_queues_mkdir_job() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-mkdir-dialog-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.apply(AppCommand::OpenInputDialog)
            .expect("mkdir dialog should open");
        for ch in "newdir".chars() {
            app.apply(AppCommand::DialogInputChar(ch))
                .expect("typing should be accepted");
        }
        app.apply(AppCommand::DialogAccept)
            .expect("mkdir dialog should submit");

        let pending = app.take_pending_worker_commands();
        assert_eq!(pending.len(), 1, "mkdir should enqueue one worker command");
        match &pending[0] {
            WorkerCommand::Run(job) => match &job.request {
                JobRequest::Mkdir { path } => {
                    assert_eq!(path, &root.join("newdir"));
                }
                _ => panic!("expected mkdir request"),
            },
            _ => panic!("expected worker run command"),
        }
        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn rename_dialog_queues_rename_job() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-rename-dialog-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let source = root.join("before.txt");
        fs::write(&source, "before").expect("must create source file");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let source_index = app
            .active_panel()
            .entries
            .iter()
            .position(|entry| entry.path == source)
            .expect("source entry should be visible");
        app.active_panel_mut().cursor = source_index;

        app.apply(AppCommand::OpenConfirmDialog)
            .expect("rename dialog should open");
        for _ in 0.."before.txt".len() {
            app.apply(AppCommand::DialogBackspace)
                .expect("rename input should accept backspace");
        }
        for ch in "after.txt".chars() {
            app.apply(AppCommand::DialogInputChar(ch))
                .expect("rename input should accept typing");
        }
        app.apply(AppCommand::DialogAccept)
            .expect("rename dialog should submit");

        let pending = app.take_pending_worker_commands();
        assert_eq!(pending.len(), 1, "rename should enqueue one worker command");
        match &pending[0] {
            WorkerCommand::Run(job) => match &job.request {
                JobRequest::Rename {
                    source,
                    destination,
                } => {
                    assert_eq!(source, &root.join("before.txt"));
                    assert_eq!(destination, &root.join("after.txt"));
                }
                _ => panic!("expected rename request"),
            },
            _ => panic!("expected worker run command"),
        }

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn skin_dialog_emits_selected_skin() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-skin-dialog-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.set_available_skins(vec![String::from("default"), String::from("dark")]);
        app.set_active_skin_name("default");

        app.apply(AppCommand::OpenSkinDialog)
            .expect("skin dialog should open");
        assert_eq!(app.key_context(), KeyContext::Listbox);

        app.apply(AppCommand::DialogListboxUp)
            .expect("listbox up should move selection");
        app.apply(AppCommand::DialogAccept)
            .expect("skin dialog should submit");

        assert_eq!(app.take_pending_skin_change(), Some(String::from("dark")));
        assert_eq!(app.status_line, "Skin selected: dark");

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn skin_dialog_emits_preview_and_revert_on_cancel() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-skin-preview-cancel-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.set_available_skins(vec![String::from("default"), String::from("dark")]);
        app.set_active_skin_name("default");

        app.apply(AppCommand::OpenSkinDialog)
            .expect("skin dialog should open");
        app.apply(AppCommand::DialogListboxUp)
            .expect("listbox up should move selection");
        assert_eq!(app.take_pending_skin_preview(), Some(String::from("dark")));
        assert_eq!(app.take_pending_skin_change(), None);
        assert_eq!(app.take_pending_skin_revert(), None);

        app.apply(AppCommand::DialogCancel)
            .expect("skin dialog cancel should close");
        assert_eq!(app.take_pending_skin_preview(), None);
        assert_eq!(app.take_pending_skin_change(), None);
        assert_eq!(
            app.take_pending_skin_revert(),
            Some(String::from("default"))
        );
        assert_eq!(app.status_line, "Skin unchanged");

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn help_route_supports_topic_links_and_back_navigation() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-help-route-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.apply(AppCommand::OpenHelp)
            .expect("help route should open");
        assert_eq!(app.key_context(), KeyContext::Help);
        let Route::Help(help) = app.top_route() else {
            panic!("top route should be help");
        };
        assert_eq!(help.current_id(), "file-manager");

        app.apply(AppCommand::HelpIndex)
            .expect("help index should open");
        let Route::Help(help) = app.top_route() else {
            panic!("top route should remain help");
        };
        assert_eq!(help.current_id(), "index");

        app.apply(AppCommand::HelpLinkNext)
            .expect("next help link should select");
        app.apply(AppCommand::HelpFollowLink)
            .expect("following selected link should succeed");
        let Route::Help(help) = app.top_route() else {
            panic!("top route should remain help");
        };
        assert_ne!(help.current_id(), "index");

        app.apply(AppCommand::HelpBack)
            .expect("help back should return to previous node");
        let Route::Help(help) = app.top_route() else {
            panic!("top route should remain help");
        };
        assert_eq!(help.current_id(), "index");

        app.apply(AppCommand::CloseHelp)
            .expect("help route should close");
        assert_eq!(app.key_context(), KeyContext::FileManager);

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn menu_shortcuts_follow_loaded_keymap_bindings() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-menu-shortcuts-keymap-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let keymap = Keymap::parse(
            r#"
[filemanager]
View = f11
Edit = f12
Copy = ctrl-y
"#,
        )
        .expect("keymap should parse");
        app.set_keybinding_hints_from_keymap(&keymap);

        let view_entry = FILE_MENU_ENTRIES
            .iter()
            .find(|entry| entry.label == "View")
            .expect("View entry should exist");
        let edit_entry = FILE_MENU_ENTRIES
            .iter()
            .find(|entry| entry.label == "Edit")
            .expect("Edit entry should exist");
        let copy_entry = FILE_MENU_ENTRIES
            .iter()
            .find(|entry| entry.label == "Copy")
            .expect("Copy entry should exist");

        assert_eq!(app.menu_entry_shortcut_label(view_entry), "F11");
        assert_eq!(app.menu_entry_shortcut_label(edit_entry), "F12");
        assert_eq!(app.menu_entry_shortcut_label(copy_entry), "Ctrl-y");

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn help_content_applies_keybinding_replacements() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-help-keybindings-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let keymap = Keymap::parse(
            r#"
[filemanager]
OpenJobs = f6
"#,
        )
        .expect("keymap should parse");
        app.set_keybinding_hints_from_keymap(&keymap);
        app.apply(AppCommand::OpenHelp)
            .expect("help route should open");

        let Route::Help(help) = app.top_route() else {
            panic!("top route should be help");
        };
        let mut content = String::new();
        for line in help.lines() {
            for span in &line.spans {
                match span {
                    HelpSpan::Text(text) => content.push_str(text),
                    HelpSpan::Link { label, .. } => content.push_str(label),
                }
            }
            content.push('\n');
        }

        assert!(
            !content.contains("{{"),
            "help content should not contain unresolved template tokens"
        );
        assert!(
            content.contains("F6 open jobs screen"),
            "help should reflect keymap-derived shortcuts"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn menu_route_supports_keyboard_navigation_and_selection() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-menu-route-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.apply(AppCommand::OpenMenuAt(2))
            .expect("menu route should open");
        assert_eq!(app.key_context(), KeyContext::Menu);

        move_menu_selection_to_label(&mut app, "Background jobs");
        app.apply(AppCommand::MenuAccept)
            .expect("menu accept should execute selected action");
        assert_eq!(app.key_context(), KeyContext::Jobs);

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn menu_stub_action_reports_not_implemented_status() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-menu-stub-action-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.apply(AppCommand::OpenMenuAt(0))
            .expect("left menu should open");
        app.apply(AppCommand::MenuAccept)
            .expect("accepting stub menu action should succeed");
        assert_eq!(app.key_context(), KeyContext::FileManager);
        assert!(
            app.status_line.contains("not implemented"),
            "stub actions should report a not-implemented status"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn side_menus_match_and_options_match_mc_shape() {
        let menus = top_menus();
        let left = menus
            .iter()
            .find(|menu| menu.title == "Left")
            .expect("left menu should exist");
        let right = menus
            .iter()
            .find(|menu| menu.title == "Right")
            .expect("right menu should exist");
        let file = menus
            .iter()
            .find(|menu| menu.title == "File")
            .expect("file menu should exist");
        let options = menus
            .iter()
            .find(|menu| menu.title == "Options")
            .expect("options menu should exist");
        let command = menus
            .iter()
            .find(|menu| menu.title == "Command")
            .expect("command menu should exist");

        let left_labels: Vec<&str> = left.entries.iter().map(|entry| entry.label).collect();
        let right_labels: Vec<&str> = right.entries.iter().map(|entry| entry.label).collect();
        assert_eq!(
            left_labels, right_labels,
            "left and right menu entries should remain identical"
        );
        assert!(
            left_labels.contains(&"File listing")
                && left_labels.contains(&"Panelize")
                && left_labels.contains(&"Rescan"),
            "side menus should include MC-style panel controls"
        );

        let file_labels: Vec<&str> = file.entries.iter().map(|entry| entry.label).collect();
        assert_eq!(file_labels.first(), Some(&"View"));
        assert!(file_labels.contains(&"Rename/Move"));
        assert!(file_labels.contains(&"Select group"));
        assert_eq!(file_labels.last(), Some(&"Exit"));

        let command_labels: Vec<&str> = command.entries.iter().map(|entry| entry.label).collect();
        assert_eq!(
            command_labels,
            vec![
                "User menu",
                "Directory tree",
                "Find file",
                "Swap panels",
                "Switch panels on/off",
                "Compare directories",
                "Compare files",
                "External panelize",
                "Show directory sizes",
                "",
                "Command history",
                "Viewed/edited files history",
                "Directory hotlist",
                "Active VFS list",
                "Background jobs",
                "Screen list",
                "",
                "Edit extension file",
                "Edit menu file",
                "Edit highlighting group file",
            ],
            "command menu should follow MC structure and ordering"
        );

        let command_shortcuts: Vec<&str> =
            command.entries.iter().map(|entry| entry.shortcut).collect();
        assert_eq!(command_shortcuts[0], "F2");
        assert_eq!(command_shortcuts[2], "M-?");
        assert_eq!(command_shortcuts[7], "C-x !");
        assert_eq!(command_shortcuts[12], "C-\\");
        assert_eq!(command_shortcuts[14], "C-x j");

        let option_labels: Vec<&str> = options.entries.iter().map(|entry| entry.label).collect();
        assert_eq!(
            option_labels,
            vec![
                "Configuration...",
                "Layout...",
                "Panel options...",
                "Confirmation...",
                "Appearance...",
                "Display bits...",
                "Learn keys...",
                "Virtual FS...",
                "Save setup",
            ],
            "options menu should follow mc ordering and labels"
        );
    }

    #[test]
    fn options_commands_open_settings_routes() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-options-route-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.apply(AppCommand::OpenOptionsLayout)
            .expect("layout options should open");
        let Route::Settings(settings) = app.top_route() else {
            panic!("settings route should open");
        };
        assert_eq!(settings.category, SettingsCategory::Layout);
        assert!(!settings.entries.is_empty());

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn settings_toggle_marks_dirty_and_save_setup_sets_pending_flag() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-options-dirty-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        assert!(!app.settings().save_setup.dirty);

        app.apply(AppCommand::OpenOptionsConfiguration)
            .expect("configuration options should open");
        app.apply(AppCommand::DialogAccept)
            .expect("toggle should apply");
        assert!(app.settings().save_setup.dirty);

        app.apply(AppCommand::SaveSetup)
            .expect("save setup command should succeed");
        assert!(app.take_pending_save_setup());

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn status_line_expires_after_configured_timeout() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-status-timeout-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.settings.layout.status_message_timeout_seconds = 10;
        app.set_status("Loading selected directory...");
        let expires_at = app
            .status_expires_at
            .expect("status timeout should schedule expiration");

        let before = expires_at
            .checked_sub(Duration::from_millis(1))
            .expect("status expiration should support sub-millisecond offset");
        app.expire_status_line_at(before);
        assert_eq!(
            app.status_line, "Loading selected directory...",
            "status should remain visible before configured timeout"
        );

        app.expire_status_line_at(expires_at);
        assert!(
            app.status_line.is_empty(),
            "status should clear once configured timeout elapses"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn status_line_timeout_zero_disables_auto_clear() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-status-timeout-off-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.settings.layout.status_message_timeout_seconds = 0;
        app.set_status("Loading selected directory...");
        assert!(
            app.status_expires_at.is_none(),
            "timeout value 0 should disable status auto-clear"
        );

        let much_later = Instant::now()
            .checked_add(Duration::from_secs(30))
            .expect("clock should support future offset");
        app.expire_status_line_at(much_later);
        assert_eq!(
            app.status_line, "Loading selected directory...",
            "status should remain until replaced when timeout is disabled"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn set_status_sanitizes_controls_and_truncates_very_long_messages() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-status-sanitize-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.set_status(format!(
            "line1\nline2\t{}\r{}",
            '\u{1b}',
            "x".repeat(MAX_STATUS_LINE_CHARS.saturating_add(128))
        ));
        assert!(
            !app.status_line.contains('\n')
                && !app.status_line.contains('\r')
                && !app.status_line.contains('\t')
                && !app.status_line.contains('\u{1b}'),
            "status text should strip control characters before render"
        );
        assert!(
            app.status_line.ends_with("..."),
            "very long status text should be truncated with an ellipsis"
        );
        assert!(
            app.status_line.chars().count() <= MAX_STATUS_LINE_CHARS.saturating_add(3),
            "status text should be bounded to avoid pathological render costs"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn persist_settings_job_coalesces_pending_request() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-persist-coalesce-pending-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let settings_paths = settings_io::SettingsPaths {
            mc_ini_path: Some(root.join("mc.ini")),
            rc_ini_path: Some(root.join("settings.ini")),
        };
        let snapshot_one = app.persisted_settings_snapshot();
        let mut snapshot_two = app.persisted_settings_snapshot();
        snapshot_two.appearance.skin = String::from("coalesced-skin");

        let first_id = app.enqueue_worker_job_request(JobRequest::PersistSettings {
            paths: settings_paths.clone(),
            snapshot: Box::new(snapshot_one),
        });
        let second_id = app.enqueue_worker_job_request(JobRequest::PersistSettings {
            paths: settings_paths.clone(),
            snapshot: Box::new(snapshot_two.clone()),
        });
        assert_eq!(first_id, second_id, "coalescing should reuse queued job id");

        let pending = app.take_pending_worker_commands();
        assert_eq!(
            pending.len(),
            1,
            "pending save setup should coalesce to one job"
        );
        match &pending[0] {
            WorkerCommand::Run(job) => match &job.request {
                JobRequest::PersistSettings { paths, snapshot } => {
                    assert_eq!(paths, &settings_paths);
                    assert_eq!(snapshot.appearance.skin, snapshot_two.appearance.skin);
                }
                _ => panic!("expected persist settings request"),
            },
            _ => panic!("expected queued worker command"),
        }

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn persist_settings_job_defers_latest_while_active() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-persist-coalesce-active-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let settings_paths = settings_io::SettingsPaths {
            mc_ini_path: Some(root.join("mc.ini")),
            rc_ini_path: Some(root.join("settings.ini")),
        };
        let first_snapshot = app.persisted_settings_snapshot();
        let mut second_snapshot = app.persisted_settings_snapshot();
        second_snapshot.appearance.skin = String::from("deferred-skin");

        let first_id = app.enqueue_worker_job_request(JobRequest::PersistSettings {
            paths: settings_paths.clone(),
            snapshot: Box::new(first_snapshot),
        });
        let pending = app.take_pending_worker_commands();
        assert_eq!(pending.len(), 1, "first save setup should be queued");

        let deferred_id = app.enqueue_worker_job_request(JobRequest::PersistSettings {
            paths: settings_paths,
            snapshot: Box::new(second_snapshot.clone()),
        });
        assert_eq!(
            deferred_id, first_id,
            "deferred save should attach to active job"
        );
        assert!(
            app.take_pending_worker_commands().is_empty(),
            "deferred save should not enqueue until active job finishes"
        );

        app.handle_job_event(JobEvent::Finished {
            id: first_id,
            result: Ok(()),
        });
        let pending = app.take_pending_worker_commands();
        assert_eq!(
            pending.len(),
            1,
            "latest deferred save should enqueue after finish"
        );
        match &pending[0] {
            WorkerCommand::Run(job) => match &job.request {
                JobRequest::PersistSettings { snapshot, .. } => {
                    assert_eq!(snapshot.appearance.skin, second_snapshot.appearance.skin);
                }
                _ => panic!("expected persist settings request"),
            },
            _ => panic!("expected queued worker command"),
        }

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn learn_keys_capture_stores_chord_and_marks_settings_dirty() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-learn-keys-capture-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.apply(AppCommand::OpenOptionsLearnKeys)
            .expect("learn keys options should open");
        for _ in 0..4 {
            app.apply(AppCommand::DialogListboxDown)
                .expect("selection should move down");
        }
        app.apply(AppCommand::DialogAccept)
            .expect("capture entry should activate");
        assert!(
            app.status_line.contains("Press a key chord"),
            "capture mode status should be shown"
        );

        assert!(app.capture_learn_keys_chord(KeyChord {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers {
                ctrl: true,
                alt: false,
                shift: false,
            },
        }));
        assert_eq!(
            app.settings().learn_keys.last_learned_binding.as_deref(),
            Some("Ctrl-x")
        );
        assert!(app.settings().save_setup.dirty);

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn learn_keys_capture_can_be_canceled_with_escape() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-learn-keys-cancel-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.settings_mut().learn_keys.last_learned_binding = Some(String::from("F5"));
        app.apply(AppCommand::OpenOptionsLearnKeys)
            .expect("learn keys options should open");
        for _ in 0..4 {
            app.apply(AppCommand::DialogListboxDown)
                .expect("selection should move down");
        }
        app.apply(AppCommand::DialogAccept)
            .expect("capture entry should activate");

        assert!(app.capture_learn_keys_chord(KeyChord {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::default(),
        }));
        assert_eq!(
            app.settings().learn_keys.last_learned_binding.as_deref(),
            Some("F5")
        );
        assert!(
            app.status_line.contains("canceled"),
            "cancel status should be shown"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn confirm_quit_setting_requires_dialog_before_quit() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-confirm-quit-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.apply(AppCommand::OpenOptionsConfirmation)
            .expect("confirmation options should open");
        app.apply(AppCommand::DialogListboxDown)
            .expect("move to confirm overwrite");
        app.apply(AppCommand::DialogListboxDown)
            .expect("move to confirm quit");
        app.apply(AppCommand::DialogAccept)
            .expect("toggle confirm quit");

        let result = app
            .apply(AppCommand::Quit)
            .expect("quit should open confirmation");
        assert_eq!(result, ApplyResult::Continue);
        assert!(matches!(app.top_route(), Route::Dialog(_)));

        let quit_result = app
            .apply(AppCommand::DialogAccept)
            .expect("confirm quit should return quit result");
        assert_eq!(quit_result, ApplyResult::Quit);

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn command_menu_external_panelize_opens_dialog() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-menu-command-panelize-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.apply(AppCommand::OpenMenuAt(2))
            .expect("command menu should open");
        move_menu_selection_to_label(&mut app, "External panelize");
        app.apply(AppCommand::MenuAccept)
            .expect("external panelize menu entry should open dialog");
        assert_eq!(app.key_context(), KeyContext::Listbox);
        assert!(app.status_line.contains("External panelize"));

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn menu_mouse_clicks_map_to_commands() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-menu-mouse-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let command = app.command_for_left_click(8, 0);
        assert_eq!(command, Some(AppCommand::OpenMenuAt(1)));

        app.apply(AppCommand::OpenMenuAt(1))
            .expect("menu route should open");
        assert_eq!(
            app.command_for_left_click(8, 3),
            Some(AppCommand::MenuSelectAt(1))
        );
        assert_eq!(
            app.command_for_left_click(100, 20),
            Some(AppCommand::CloseMenu)
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn delete_command_queues_job_only_after_confirmation() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-delete-dialog-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let victim = root.join("victim.txt");
        fs::write(&victim, "victim").expect("must create victim file");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let victim_index = app
            .active_panel()
            .entries
            .iter()
            .position(|entry| entry.path == victim)
            .expect("victim entry should be visible");
        app.active_panel_mut().cursor = victim_index;

        app.apply(AppCommand::Delete)
            .expect("delete should open confirm dialog");
        assert_eq!(app.route_depth(), 2);

        app.apply(AppCommand::DialogAccept)
            .expect("confirm dialog should submit");
        let pending = app.take_pending_worker_commands();
        assert_eq!(
            pending.len(),
            1,
            "delete should enqueue exactly one worker command"
        );
        match &pending[0] {
            WorkerCommand::Run(job) => match &job.request {
                JobRequest::Delete { targets } => {
                    assert_eq!(targets, &vec![victim.clone()]);
                }
                _ => panic!("expected delete job request"),
            },
            _ => panic!("expected queued worker run command"),
        }

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn copy_command_uses_destination_and_policy_dialogs() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-copy-dialog-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let source = root.join("a.txt");
        fs::write(&source, "a").expect("must create source file");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let source_index = app
            .active_panel()
            .entries
            .iter()
            .position(|entry| entry.path == source)
            .expect("source entry should be visible");
        app.active_panel_mut().cursor = source_index;

        app.apply(AppCommand::Copy)
            .expect("copy should open destination dialog");
        assert_eq!(app.route_depth(), 2);

        app.apply(AppCommand::DialogAccept)
            .expect("destination dialog should submit");
        assert_eq!(
            app.route_depth(),
            2,
            "policy dialog should replace destination dialog"
        );

        app.apply(AppCommand::DialogAccept)
            .expect("policy dialog should submit");
        let pending = app.take_pending_worker_commands();
        assert_eq!(pending.len(), 1, "copy should enqueue one worker command");
        match &pending[0] {
            WorkerCommand::Run(job) => match &job.request {
                JobRequest::Copy {
                    sources,
                    destination_dir,
                    overwrite,
                } => {
                    assert_eq!(sources, &vec![source.clone()]);
                    assert_eq!(destination_dir, &root);
                    assert_eq!(*overwrite, app.overwrite_policy);
                }
                _ => panic!("expected copy job request"),
            },
            _ => panic!("expected queued worker run command"),
        }

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn copy_relative_destination_is_resolved_from_active_panel() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-copy-relative-destination-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let source = root.join("a.txt");
        fs::write(&source, "a").expect("must create source file");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let source_index = app
            .active_panel()
            .entries
            .iter()
            .position(|entry| entry.path == source)
            .expect("source entry should be visible");
        app.active_panel_mut().cursor = source_index;

        app.start_copy_dialog();
        app.finish_dialog(DialogResult::InputSubmitted(String::from("dest")));

        match app.pending_dialog_action.as_ref() {
            Some(PendingDialogAction::TransferOverwrite {
                destination_dir, ..
            }) => {
                assert_eq!(destination_dir, &root.join("dest"));
            }
            other => panic!("expected transfer overwrite action, got {other:?}"),
        }

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn open_entry_on_file_opens_viewer_route() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-viewer-open-file-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let file_path = root.join("notes.txt");
        fs::write(&file_path, "alpha\nbeta\ngamma\n").expect("must create viewer file");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let file_index = app
            .active_panel()
            .entries
            .iter()
            .position(|entry| entry.path == file_path)
            .expect("viewer file should be visible");
        app.active_panel_mut().cursor = file_index;

        app.apply(AppCommand::OpenEntry)
            .expect("open entry should open viewer");
        drain_background(&mut app);
        assert_eq!(app.key_context(), KeyContext::Viewer);

        let Route::Viewer(viewer) = app.top_route() else {
            panic!("top route should be viewer");
        };
        assert_eq!(viewer.path(), &file_path);
        assert_eq!(viewer.line_count(), 3);

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[cfg(unix)]
    #[test]
    fn open_entry_on_directory_symlink_descends_into_directory() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-open-dir-symlink-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let target_dir = root.join("target");
        fs::create_dir_all(&target_dir).expect("must create target directory");
        fs::write(target_dir.join("entry.txt"), "payload").expect("must create target file");
        let symlink_path = root.join("tmp-like");
        std::os::unix::fs::symlink(&target_dir, &symlink_path)
            .expect("directory symlink should be creatable");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let symlink_index = app
            .active_panel()
            .entries
            .iter()
            .position(|entry| entry.path == symlink_path)
            .expect("directory symlink should be visible");
        assert!(
            app.active_panel().entries[symlink_index].is_dir,
            "directory symlink should be treated as a directory entry"
        );
        app.active_panel_mut().cursor = symlink_index;

        app.apply(AppCommand::OpenEntry)
            .expect("open entry should descend into directory symlink");
        assert_eq!(
            app.active_panel().cwd,
            symlink_path,
            "open entry should switch into the symlink path"
        );
        assert!(
            app.active_panel().loading,
            "opening a directory symlink should queue a panel refresh"
        );

        let pending = app.take_pending_worker_commands();
        assert_eq!(
            pending.len(),
            1,
            "directory open should queue one refresh request"
        );
        match &pending[0] {
            WorkerCommand::Run(job) => match &job.request {
                JobRequest::RefreshPanel {
                    cwd,
                    source: PanelListingSource::Directory,
                    ..
                } => assert_eq!(
                    cwd,
                    &app.active_panel().cwd,
                    "refresh request should target the opened symlink directory"
                ),
                other => panic!("expected refresh panel request, got {other:?}"),
            },
            other => panic!("expected queued worker run command, got {other:?}"),
        }

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn edit_entry_on_file_queues_external_editor_request() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-edit-open-file-external-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let file_path = root.join("notes.txt");
        fs::write(&file_path, "alpha\nbeta\ngamma\n").expect("must create edit target");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let file_index = app
            .active_panel()
            .entries
            .iter()
            .position(|entry| entry.path == file_path)
            .expect("edit target should be visible");
        app.active_panel_mut().cursor = file_index;

        assert_eq!(
            app.open_selected_file_in_editor_with_resolver(|| Some(String::from("nvim"))),
            EditSelectionResult::OpenedExternal
        );

        let requests = app.take_pending_external_edit_requests();
        assert_eq!(requests.len(), 1, "one editor request should be queued");
        let request = &requests[0];
        assert_eq!(request.editor_command, "nvim");
        assert_eq!(request.path, file_path);
        assert_eq!(request.cwd, root);
        assert!(app.take_pending_worker_commands().is_empty());

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn edit_entry_falls_back_to_internal_when_no_external_editor_is_set() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-edit-open-file-internal-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let file_path = root.join("notes.txt");
        fs::write(&file_path, "alpha\nbeta\ngamma\n").expect("must create edit target");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let file_index = app
            .active_panel()
            .entries
            .iter()
            .position(|entry| entry.path == file_path)
            .expect("edit target should be visible");
        app.active_panel_mut().cursor = file_index;

        assert_eq!(
            app.open_selected_file_in_editor_with_resolver(|| None),
            EditSelectionResult::OpenedInternal
        );
        assert!(
            app.take_pending_external_edit_requests().is_empty(),
            "no external editor request should be queued"
        );

        let pending_worker = app.take_pending_worker_commands();
        assert_eq!(pending_worker.len(), 1, "viewer load should be queued");
        match &pending_worker[0] {
            WorkerCommand::Run(job) => match &job.request {
                JobRequest::LoadViewer { path } => assert_eq!(path, &file_path),
                other => panic!("expected load-viewer request, got {other:?}"),
            },
            other => panic!("expected worker run command, got {other:?}"),
        }

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn viewer_supports_scroll_search_goto_and_wrap() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-viewer-actions-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let file_path = root.join("viewer.txt");
        fs::write(&file_path, "first\nsecond target\nthird\nfourth target\n")
            .expect("must create viewer content");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let file_index = app
            .active_panel()
            .entries
            .iter()
            .position(|entry| entry.path == file_path)
            .expect("viewer file should be visible");
        app.active_panel_mut().cursor = file_index;
        app.apply(AppCommand::OpenEntry)
            .expect("open entry should open viewer");
        drain_background(&mut app);

        app.apply(AppCommand::ViewerMoveDown)
            .expect("viewer should move down");
        let Route::Viewer(viewer) = app.top_route() else {
            panic!("top route should be viewer");
        };
        assert_eq!(viewer.current_line_number(), 2);

        app.apply(AppCommand::ViewerToggleWrap)
            .expect("viewer should toggle wrap");
        let Route::Viewer(viewer) = app.top_route() else {
            panic!("top route should be viewer");
        };
        assert!(viewer.wrap, "wrap should be enabled");

        app.apply(AppCommand::ViewerGoto)
            .expect("viewer goto should open dialog");
        app.apply(AppCommand::DialogBackspace)
            .expect("should edit goto target");
        for ch in "3".chars() {
            app.apply(AppCommand::DialogInputChar(ch))
                .expect("typing goto target should succeed");
        }
        app.apply(AppCommand::DialogAccept)
            .expect("goto dialog should submit");
        let Route::Viewer(viewer) = app.top_route() else {
            panic!("top route should be viewer");
        };
        assert_eq!(viewer.current_line_number(), 3);

        app.apply(AppCommand::ViewerSearchForward)
            .expect("search should open dialog");
        for ch in "target".chars() {
            app.apply(AppCommand::DialogInputChar(ch))
                .expect("typing search query should succeed");
        }
        app.apply(AppCommand::DialogAccept)
            .expect("search dialog should submit");
        let Route::Viewer(viewer) = app.top_route() else {
            panic!("top route should be viewer");
        };
        assert_eq!(viewer.current_line_number(), 4);

        app.apply(AppCommand::ViewerSearchContinueBackward)
            .expect("reverse continue search should run");
        let Route::Viewer(viewer) = app.top_route() else {
            panic!("top route should be viewer");
        };
        assert_eq!(viewer.current_line_number(), 2);

        app.apply(AppCommand::CloseViewer)
            .expect("viewer should close");
        assert_eq!(app.key_context(), KeyContext::FileManager);

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn viewer_hex_mode_switches_context_and_navigation_model() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-viewer-hex-mode-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let file_path = root.join("hex.bin");
        fs::write(
            &file_path,
            b"0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .expect("must create viewer content");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let file_index = app
            .active_panel()
            .entries
            .iter()
            .position(|entry| entry.path == file_path)
            .expect("viewer file should be visible");
        app.active_panel_mut().cursor = file_index;
        app.apply(AppCommand::OpenEntry)
            .expect("open entry should open viewer");
        drain_background(&mut app);
        assert_eq!(app.key_context(), KeyContext::Viewer);

        app.apply(AppCommand::ViewerToggleHex)
            .expect("viewer should toggle hex mode");
        assert_eq!(app.key_context(), KeyContext::ViewerHex);
        let Route::Viewer(viewer) = app.top_route() else {
            panic!("top route should be viewer");
        };
        assert_eq!(
            viewer.line_count(),
            3,
            "48 bytes should render as 3 hex rows"
        );

        app.apply(AppCommand::ViewerMoveDown)
            .expect("viewer should move by hex row");
        let Route::Viewer(viewer) = app.top_route() else {
            panic!("top route should be viewer");
        };
        assert_eq!(viewer.current_line_number(), 2);

        app.apply(AppCommand::ViewerToggleHex)
            .expect("viewer should toggle back to text mode");
        assert_eq!(app.key_context(), KeyContext::Viewer);

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn viewer_opens_binary_content_in_hex_mode_by_default() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-viewer-binary-default-hex-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let file_path = root.join("payload.bin");
        fs::write(&file_path, b"\x00\x1b\x7fBIN\x01\x02").expect("must create binary file");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let file_index = app
            .active_panel()
            .entries
            .iter()
            .position(|entry| entry.path == file_path)
            .expect("binary file should be visible");
        app.active_panel_mut().cursor = file_index;
        app.apply(AppCommand::OpenEntry)
            .expect("open entry should queue viewer");
        drain_background(&mut app);

        assert_eq!(
            app.key_context(),
            KeyContext::ViewerHex,
            "binary files should open in hex mode"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn viewer_state_fingerprints_track_path_and_content() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-viewer-fingerprints-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let first_path = root.join("first.txt");
        let second_path = root.join("second.txt");
        let third_path = root.join("third.txt");
        fs::write(&first_path, "abc").expect("first fixture should be writable");
        fs::write(&second_path, "abc").expect("second fixture should be writable");
        fs::write(&third_path, "xyz").expect("third fixture should be writable");

        let first = ViewerState::open(first_path).expect("first viewer fixture should open");
        let second = ViewerState::open(second_path).expect("second viewer fixture should open");
        let third = ViewerState::open(third_path).expect("third viewer fixture should open");

        assert_eq!(
            first.content_fingerprint(),
            second.content_fingerprint(),
            "matching content should reuse the same content fingerprint"
        );
        assert_ne!(
            first.path_fingerprint(),
            second.path_fingerprint(),
            "different file paths should produce distinct path fingerprints"
        );
        assert_ne!(
            first.content_fingerprint(),
            third.content_fingerprint(),
            "different content with the same length should produce distinct fingerprints"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn viewer_state_uses_preview_mode_for_large_text_files() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-viewer-preview-large-text-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let file_path = root.join("large.txt");
        let total_bytes = VIEWER_TEXT_PREVIEW_LIMIT_BYTES + 1024;
        fs::write(&file_path, vec![b'a'; total_bytes]).expect("large fixture should be writable");

        let viewer = ViewerState::open(file_path).expect("large viewer fixture should open");
        assert!(
            viewer.text_is_preview(),
            "large text file should be previewed"
        );
        assert_eq!(
            viewer.content().len(),
            VIEWER_TEXT_PREVIEW_LIMIT_BYTES,
            "viewer content should be capped at preview limit"
        );
        assert!(
            viewer.hex_mode,
            "preview mode should default to hex context"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn viewer_state_preview_fingerprint_includes_total_size() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-viewer-preview-fingerprint-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let first_path = root.join("first.txt");
        let second_path = root.join("second.txt");

        let mut first_bytes = vec![b'x'; VIEWER_TEXT_PREVIEW_LIMIT_BYTES + 1];
        let second_bytes = vec![b'x'; VIEWER_TEXT_PREVIEW_LIMIT_BYTES + 32];
        first_bytes[VIEWER_TEXT_PREVIEW_LIMIT_BYTES] = b'y';
        fs::write(&first_path, first_bytes).expect("first fixture should be writable");
        fs::write(&second_path, second_bytes).expect("second fixture should be writable");

        let first = ViewerState::open(first_path).expect("first preview fixture should open");
        let second = ViewerState::open(second_path).expect("second preview fixture should open");

        assert!(first.text_is_preview());
        assert!(second.text_is_preview());
        assert_eq!(
            first.content(),
            second.content(),
            "previewed text should match when prefixes are identical"
        );
        assert_ne!(
            first.content_fingerprint(),
            second.content_fingerprint(),
            "preview fingerprints should diverge when total file size differs"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn opening_large_text_file_reports_preview_mode_status() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-viewer-preview-status-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let file_path = root.join("large.txt");
        let total_bytes = VIEWER_TEXT_PREVIEW_LIMIT_BYTES + 1;
        fs::write(&file_path, vec![b'a'; total_bytes]).expect("large fixture should be writable");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let file_index = app
            .active_panel()
            .entries
            .iter()
            .position(|entry| entry.path == file_path)
            .expect("large file should be visible");
        app.active_panel_mut().cursor = file_index;
        app.apply(AppCommand::OpenEntry)
            .expect("open entry should queue viewer");
        drain_background(&mut app);

        assert_eq!(app.key_context(), KeyContext::ViewerHex);
        assert!(
            app.status_line.contains("(text preview mode)"),
            "status should communicate preview mode for large files"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn find_dialog_locates_selected_entry_in_panel_and_supports_resume() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-find-results-{stamp}"));
        let nested = root.join("nested");
        fs::create_dir_all(&nested).expect("must create temp tree");
        let target = nested.join("needle.txt");
        fs::write(&target, "needle").expect("must create target file");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.apply(AppCommand::OpenFindDialog)
            .expect("find dialog should open");
        for ch in "needle".chars() {
            app.apply(AppCommand::DialogInputChar(ch))
                .expect("typing find query should succeed");
        }
        app.apply(AppCommand::DialogAccept)
            .expect("find dialog should submit");
        drain_background(&mut app);
        assert_eq!(app.key_context(), KeyContext::FindResults);
        let find_job = app.jobs.last_job().expect("find job should be recorded");
        assert_eq!(find_job.kind, JobKind::Find);
        assert_eq!(find_job.status, JobStatus::Succeeded);

        let target_index = match app.top_route() {
            Route::FindResults(results) => results
                .entries
                .iter()
                .position(|entry| entry.path == target)
                .expect("target should be present in find results"),
            _ => panic!("top route should be find results"),
        };
        let Some(Route::FindResults(results)) = app.routes.last_mut() else {
            panic!("top route should be find results");
        };
        results.cursor = target_index;

        app.apply(AppCommand::FindResultsOpenEntry)
            .expect("opening find result should succeed");
        drain_background(&mut app);
        assert_eq!(app.key_context(), KeyContext::FileManager);
        assert_eq!(app.active_panel().cwd, nested);

        let focused_entry = app
            .active_panel()
            .selected_entry()
            .expect("selected panel entry should be present");
        assert_eq!(focused_entry.path, target);

        app.apply(AppCommand::OpenFindDialog)
            .expect("open find should resume results");
        assert_eq!(app.key_context(), KeyContext::FindResults);
        let Route::FindResults(results) = app.top_route() else {
            panic!("top route should be find results");
        };
        assert_eq!(
            results.entries.get(results.cursor).map(|entry| &entry.path),
            Some(&target)
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn find_results_panelize_creates_virtual_panel_and_preserves_resume() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-find-panelize-{stamp}"));
        let nested = root.join("nested");
        fs::create_dir_all(&nested).expect("must create temp tree");
        let target = nested.join("needle.txt");
        fs::write(&target, "needle").expect("must create target file");
        fs::write(root.join("other.log"), "other").expect("must create non-matching file");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.apply(AppCommand::OpenFindDialog)
            .expect("find dialog should open");
        for ch in "needle".chars() {
            app.apply(AppCommand::DialogInputChar(ch))
                .expect("typing find query should succeed");
        }
        app.apply(AppCommand::DialogAccept)
            .expect("find dialog should submit");
        drain_background(&mut app);
        assert_eq!(app.key_context(), KeyContext::FindResults);

        app.apply(AppCommand::FindResultsPanelize)
            .expect("panelizing find results should succeed");
        drain_background(&mut app);
        assert_eq!(app.key_context(), KeyContext::FileManager);
        assert!(matches!(
            app.active_panel().source,
            PanelListingSource::FindResults { .. }
        ));
        assert!(
            app.active_panel()
                .entries
                .iter()
                .any(|entry| entry.path == target),
            "panelized find results should include matching files"
        );
        assert_eq!(app.active_panel().cwd, root);

        app.apply(AppCommand::CdUp)
            .expect("CdUp should leave panelize mode");
        drain_background(&mut app);
        assert!(matches!(
            app.active_panel().source,
            PanelListingSource::Directory
        ));
        assert_eq!(
            app.active_panel().cwd,
            root,
            "leaving panelize mode should keep current directory unchanged"
        );

        app.apply(AppCommand::OpenFindDialog)
            .expect("find dialog should resume previous results");
        assert_eq!(app.key_context(), KeyContext::FindResults);
        let Route::FindResults(results) = app.top_route() else {
            panic!("top route should be find results");
        };
        assert!(
            results.entries.iter().any(|entry| entry.path == target),
            "resumed find results should still include prior matches"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn find_cancel_routes_through_worker_cancel_command() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-find-cancel-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        fs::write(root.join("a.jpg"), "a").expect("must create file");
        fs::write(root.join("b.jpg"), "b").expect("must create file");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.apply(AppCommand::OpenFindDialog)
            .expect("find dialog should open");
        for ch in "*.jpg".chars() {
            app.apply(AppCommand::DialogInputChar(ch))
                .expect("typing find query should succeed");
        }
        app.apply(AppCommand::DialogAccept)
            .expect("find dialog should submit");
        let queued_counts = app.jobs_status_counts();
        assert_eq!(queued_counts.queued, 1, "find should enqueue a worker job");

        app.apply(AppCommand::CancelJob)
            .expect("cancel job should succeed");
        let commands = app.take_pending_worker_commands();
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, WorkerCommand::Cancel(_))),
            "canceling find should enqueue worker cancel command"
        );
        for command in commands {
            if let WorkerCommand::Run(job) = command {
                app.pending_worker_commands.push(WorkerCommand::Run(job));
            }
        }

        drain_background(&mut app);
        let find_job = app.jobs.last_job().expect("find job should be present");
        assert_eq!(find_job.kind, JobKind::Find);
        assert_eq!(find_job.status, JobStatus::Canceled);

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn quit_requests_cancellation_for_pending_find_job() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-find-quit-cancel-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        fs::write(root.join("a.jpg"), "a").expect("must create file");
        fs::write(root.join("b.jpg"), "b").expect("must create file");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.apply(AppCommand::OpenFindDialog)
            .expect("find dialog should open");
        for ch in "*.jpg".chars() {
            app.apply(AppCommand::DialogInputChar(ch))
                .expect("typing find query should succeed");
        }
        app.apply(AppCommand::DialogAccept)
            .expect("find dialog should submit");

        assert_eq!(
            app.apply(AppCommand::Quit).expect("quit should succeed"),
            ApplyResult::Quit
        );

        drain_background(&mut app);
        let find_job = app.jobs.last_job().expect("find job should be present");
        assert_eq!(find_job.kind, JobKind::Find);
        assert_eq!(find_job.status, JobStatus::Canceled);

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn quit_cancels_find_but_keeps_persist_settings_job() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-quit-keep-persist-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let settings_paths = settings_io::SettingsPaths {
            mc_ini_path: Some(root.join("mc.ini")),
            rc_ini_path: Some(root.join("settings.ini")),
        };
        let persist_job_id = app.enqueue_worker_job_request(JobRequest::PersistSettings {
            paths: settings_paths,
            snapshot: Box::new(app.persisted_settings_snapshot()),
        });
        let find_job_id = app.enqueue_worker_job_request(JobRequest::Find {
            query: String::from("*.jpg"),
            base_dir: root.clone(),
            max_results: 64,
        });

        assert_eq!(
            app.apply(AppCommand::Quit).expect("quit should succeed"),
            ApplyResult::Quit
        );

        let pending_commands = app.take_pending_worker_commands();
        assert!(
            pending_commands.iter().any(|command| matches!(
                command,
                WorkerCommand::Cancel(job_id) if *job_id == find_job_id
            )),
            "quit should request cancellation for find jobs"
        );
        assert!(
            !pending_commands.iter().any(|command| matches!(
                command,
                WorkerCommand::Cancel(job_id) if *job_id == persist_job_id
            )),
            "quit should not request cancellation for persist-settings jobs"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn stream_find_entries_supports_glob_patterns_and_chunking() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-find-glob-{stamp}"));
        let nested = root.join("nested");
        fs::create_dir_all(&nested).expect("must create temp tree");
        let jpg_a = root.join("a.jpg");
        let jpg_b = nested.join("b.JPG");
        let png = root.join("c.png");
        fs::write(&jpg_a, "a").expect("must create jpg");
        fs::write(&jpg_b, "b").expect("must create jpg");
        fs::write(&png, "c").expect("must create png");

        let cancel_flag = AtomicBool::new(false);
        let pause_flag = AtomicBool::new(false);
        let mut chunks = Vec::new();
        let result = stream_find_entries(
            &root,
            "*.jpg",
            32,
            &cancel_flag,
            &pause_flag,
            1,
            |entries| {
                chunks.push(entries);
                true
            },
        );
        assert_eq!(result, Ok(()));
        assert!(
            chunks.len() >= 2,
            "chunk size 1 should emit multiple chunks for two matches"
        );

        let flattened: Vec<PathBuf> = chunks
            .iter()
            .flat_map(|chunk| chunk.iter().map(|entry| entry.path.clone()))
            .collect();
        assert!(
            flattened.contains(&jpg_a),
            "glob should match top-level jpg"
        );
        assert!(
            flattened.contains(&jpg_b),
            "glob should match nested uppercase extension"
        );
        assert!(
            !flattened.contains(&png),
            "glob should not match non-jpg file"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn stream_find_entries_stops_after_cancel_request() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-find-cancel-flag-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        fs::write(root.join("a.jpg"), "a").expect("must create file");
        fs::write(root.join("b.jpg"), "b").expect("must create file");
        fs::write(root.join("c.jpg"), "c").expect("must create file");

        let cancel_flag = AtomicBool::new(false);
        let pause_flag = AtomicBool::new(false);
        let mut chunks_seen = 0usize;
        let result = stream_find_entries(
            &root,
            "*.jpg",
            32,
            &cancel_flag,
            &pause_flag,
            1,
            |_entries| {
                chunks_seen = chunks_seen.saturating_add(1);
                cancel_flag.store(true, AtomicOrdering::Relaxed);
                true
            },
        );
        assert_eq!(result, Err(String::from(JOB_CANCELED_MESSAGE)));
        assert_eq!(chunks_seen, 1, "search should stop shortly after cancel");

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn stream_find_entries_waits_while_paused_and_resumes() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-find-paused-resume-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        fs::write(root.join("a.jpg"), "a").expect("must create file");

        let cancel_flag = AtomicBool::new(false);
        let pause_flag = Arc::new(AtomicBool::new(true));
        let pause_flag_for_thread = Arc::clone(&pause_flag);
        let resumer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(40));
            pause_flag_for_thread.store(false, AtomicOrdering::Relaxed);
        });

        let started = std::time::Instant::now();
        let result = stream_find_entries(
            &root,
            "*.jpg",
            32,
            &cancel_flag,
            pause_flag.as_ref(),
            1,
            |_entries| true,
        );
        let elapsed = started.elapsed();
        resumer.join().expect("resume thread should complete");

        assert_eq!(result, Ok(()));
        assert!(
            elapsed >= Duration::from_millis(25),
            "search should wait for resume while paused"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn tree_screen_selects_directory_for_active_panel() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-tree-screen-{stamp}"));
        let branch = root.join("branch");
        fs::create_dir_all(&branch).expect("must create temp tree");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.apply(AppCommand::OpenTree)
            .expect("tree screen should open");
        drain_background(&mut app);
        assert_eq!(app.key_context(), KeyContext::Tree);

        let branch_index = match app.top_route() {
            Route::Tree(tree) => tree
                .entries
                .iter()
                .position(|entry| entry.path == branch)
                .expect("branch should appear in tree"),
            _ => panic!("top route should be tree"),
        };
        let Some(Route::Tree(tree)) = app.routes.last_mut() else {
            panic!("top route should be tree");
        };
        tree.cursor = branch_index;

        app.apply(AppCommand::TreeOpenEntry)
            .expect("tree open should succeed");
        assert_eq!(app.key_context(), KeyContext::FileManager);
        assert_eq!(app.active_panel().cwd, branch);

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn hotlist_supports_add_remove_and_open() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-hotlist-{stamp}"));
        let branch = root.join("branch");
        fs::create_dir_all(&branch).expect("must create temp tree");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.apply(AppCommand::OpenHotlist)
            .expect("hotlist should open");
        app.apply(AppCommand::HotlistAddCurrentDirectory)
            .expect("hotlist add should succeed");
        assert_eq!(app.hotlist, vec![root.clone()]);

        {
            let panel = app.active_panel_mut();
            panel.cwd = branch.clone();
            panel.refresh().expect("panel should refresh");
        }
        app.apply(AppCommand::HotlistAddCurrentDirectory)
            .expect("hotlist add should succeed");
        assert_eq!(app.hotlist, vec![root.clone(), branch.clone()]);

        app.hotlist_cursor = 0;
        app.apply(AppCommand::HotlistRemoveSelected)
            .expect("hotlist remove should succeed");
        assert_eq!(app.hotlist, vec![branch.clone()]);

        app.hotlist_cursor = 0;
        app.apply(AppCommand::HotlistOpenEntry)
            .expect("hotlist open should succeed");
        assert_eq!(app.key_context(), KeyContext::FileManager);
        assert_eq!(app.active_panel().cwd, branch);

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[cfg(unix)]
    #[test]
    fn panelize_command_populates_active_panel_from_stdout_paths() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-panelize-populate-{stamp}"));
        fs::create_dir_all(root.join("sub")).expect("must create subdirectory");
        fs::write(root.join("a.txt"), "a").expect("must create file");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        submit_panelize_custom_command(&mut app, "printf 'a.txt\\nsub\\nmissing\\n'");
        drain_background(&mut app);

        let panel = app.active_panel();
        assert_eq!(
            panel.panelize_command(),
            Some("printf 'a.txt\\nsub\\nmissing\\n'"),
            "panelize mode should retain command for reread"
        );
        assert!(
            panel
                .entries
                .iter()
                .any(|entry| entry.path == root.join("a.txt")),
            "panelized entries should include file output path"
        );
        assert!(
            panel
                .entries
                .iter()
                .any(|entry| entry.path == root.join("sub")),
            "panelized entries should include directory output path"
        );
        assert!(
            panel
                .entries
                .iter()
                .any(|entry| entry.path == root.join("missing")),
            "panelized entries should preserve command output even when path metadata is missing"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[cfg(unix)]
    #[test]
    fn panelize_empty_output_keeps_empty_panel_entries() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-panelize-empty-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        fs::write(root.join("a.txt"), "a").expect("must create file");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        submit_panelize_custom_command(&mut app, "printf ''");
        drain_background(&mut app);

        assert_eq!(
            app.active_panel().entries.len(),
            0,
            "empty panelize output should produce empty panel entries"
        );
        assert_eq!(app.active_panel().panelize_command(), Some("printf ''"));

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[cfg(unix)]
    #[test]
    fn panelize_preserves_leading_and_trailing_spaces_in_paths() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-panelize-spaces-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let spaced_name = "  spaced file  ";
        let spaced_file = root.join(spaced_name);
        fs::write(&spaced_file, "a").expect("must create spaced filename");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        submit_panelize_custom_command(&mut app, "printf '  spaced file  \\n'");
        drain_background(&mut app);

        assert!(
            app.active_panel()
                .entries
                .iter()
                .any(|entry| entry.path == spaced_file),
            "panelize should preserve leading/trailing spaces in path lines"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[cfg(unix)]
    #[test]
    fn cdup_leaves_panelize_mode_without_changing_directory() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-panelize-cdup-{stamp}"));
        let sub = root.join("sub");
        fs::create_dir_all(&sub).expect("must create subdirectory");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        submit_panelize_custom_command(&mut app, "printf 'sub\\n'");
        drain_background(&mut app);

        assert_eq!(
            app.active_panel().panelize_command(),
            Some("printf 'sub\\n'"),
            "precondition: panel should be in panelize mode"
        );
        assert_eq!(app.active_panel().cwd, root);

        app.apply(AppCommand::CdUp)
            .expect("CdUp should leave panelize mode");
        drain_background(&mut app);

        assert_eq!(
            app.active_panel().panelize_command(),
            None,
            "CdUp should restore normal directory mode from panelize"
        );
        assert_eq!(
            app.active_panel().cwd,
            root,
            "CdUp in panelize mode should not change to parent directory"
        );
        assert!(
            app.active_panel()
                .entries
                .iter()
                .any(|entry| entry.path == sub),
            "restored listing should include entries from the current directory"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[cfg(unix)]
    #[test]
    fn panelize_failure_preserves_previous_directory_listing() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-panelize-failure-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        fs::write(root.join("a.txt"), "a").expect("must create file");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let before = app.active_panel().entries.clone();

        submit_panelize_custom_command(&mut app, "exit 42");
        drain_background(&mut app);

        assert!(
            app.status_line.contains("Panelize failed:"),
            "status line should indicate panelize failure"
        );
        assert_eq!(
            app.active_panel().entries,
            before,
            "failed panelize should keep previous listing"
        );
        assert_eq!(
            app.active_panel().panelize_command(),
            None,
            "failed panelize should not switch source mode"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[cfg(unix)]
    #[test]
    fn rename_dialog_uses_basename_for_panelized_entry() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-panelize-rename-basename-{stamp}"));
        let sub = root.join("sub");
        fs::create_dir_all(&sub).expect("must create subdirectory");
        fs::write(sub.join("a.txt"), "a").expect("must create file");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        submit_panelize_custom_command(&mut app, "printf 'sub/a.txt\\n'");
        drain_background(&mut app);

        app.apply(AppCommand::OpenConfirmDialog)
            .expect("rename dialog should open");
        let Route::Dialog(dialog) = app.top_route() else {
            panic!("rename action should open a dialog route");
        };
        let DialogKind::Input(input) = &dialog.kind else {
            panic!("rename action should open an input dialog");
        };
        assert_eq!(
            input.value, "a.txt",
            "rename input should default to basename, not panelized display label"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn panelize_dialog_lists_predefined_commands() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-panelize-presets-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.open_panelize_dialog();
        let Route::Dialog(dialog) = app.top_route() else {
            panic!("panelize should open a dialog");
        };
        let DialogKind::Listbox(listbox) = &dialog.kind else {
            panic!("panelize should open a listbox dialog");
        };
        assert_eq!(
            listbox.items.first(),
            Some(&String::from(PANELIZE_CUSTOM_COMMAND_LABEL))
        );
        assert!(
            listbox
                .items
                .iter()
                .any(|item| item == "find . -name '*.orig'"),
            "panelize list should include predefined commands"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn panelize_dialog_tab_switches_from_presets_to_input() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-panelize-tab-to-input-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.open_panelize_dialog();
        app.apply(AppCommand::DialogFocusNext)
            .expect("tab should switch to command input");

        let Route::Dialog(dialog) = app.top_route() else {
            panic!("panelize should remain in dialog route");
        };
        let DialogKind::Input(input) = &dialog.kind else {
            panic!("tab should open panelize input dialog");
        };
        assert_eq!(input.value, "find . -type f");

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn panelize_dialog_tab_switches_from_input_back_to_presets() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-panelize-tab-to-presets-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.open_panelize_dialog();
        app.apply(AppCommand::DialogFocusNext)
            .expect("tab should switch to command input");
        app.apply(AppCommand::DialogInputChar('x'))
            .expect("typing command suffix should succeed");
        app.apply(AppCommand::DialogFocusNext)
            .expect("tab should switch back to preset list");

        let Route::Dialog(dialog) = app.top_route() else {
            panic!("panelize should remain in dialog route");
        };
        let DialogKind::Listbox(listbox) = &dialog.kind else {
            panic!("tab should return to preset list");
        };
        assert_eq!(listbox.selected, 0);
        assert_eq!(
            listbox.items.first(),
            Some(&String::from(PANELIZE_CUSTOM_COMMAND_LABEL))
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn panelize_preset_management_add_edit_remove_works() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-panelize-preset-manage-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.open_panelize_dialog();
        app.apply(AppCommand::PanelizePresetAdd)
            .expect("F2 add should open preset input");
        for ch in "echo added".chars() {
            app.apply(AppCommand::DialogInputChar(ch))
                .expect("typing preset command should succeed");
        }
        app.apply(AppCommand::DialogAccept)
            .expect("submitting new preset should succeed");

        let Route::Dialog(dialog) = app.top_route() else {
            panic!("panelize should remain in preset list dialog");
        };
        let DialogKind::Listbox(listbox) = &dialog.kind else {
            panic!("panelize should return to preset list dialog");
        };
        assert!(
            listbox.items.iter().any(|item| item == "echo added"),
            "added preset should appear in list"
        );

        app.apply(AppCommand::PanelizePresetEdit)
            .expect("F4 edit should open preset input");
        for ch in " updated".chars() {
            app.apply(AppCommand::DialogInputChar(ch))
                .expect("typing edit suffix should succeed");
        }
        app.apply(AppCommand::DialogAccept)
            .expect("submitting edited preset should succeed");

        let edited = String::from("echo added updated");
        let Route::Dialog(dialog) = app.top_route() else {
            panic!("panelize should remain in preset list dialog");
        };
        let DialogKind::Listbox(listbox) = &dialog.kind else {
            panic!("panelize should return to preset list dialog");
        };
        assert!(
            listbox.items.iter().any(|item| item == &edited),
            "edited preset should replace previous value"
        );

        app.apply(AppCommand::PanelizePresetRemove)
            .expect("F8 remove should delete selected preset");
        let Route::Dialog(dialog) = app.top_route() else {
            panic!("panelize should remain in preset list dialog");
        };
        let DialogKind::Listbox(listbox) = &dialog.kind else {
            panic!("panelize should return to preset list dialog");
        };
        assert!(
            !listbox.items.iter().any(|item| item == &edited),
            "removed preset should no longer be listed"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[cfg(unix)]
    #[test]
    fn panelize_preset_selection_runs_without_custom_input() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-panelize-preset-select-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        fs::write(root.join("a.txt"), "a").expect("must create file");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.open_panelize_dialog();
        app.finish_dialog(DialogResult::ListboxSubmitted {
            index: Some(1),
            value: Some(String::from("find . -type f")),
        });
        drain_background(&mut app);

        assert_eq!(
            app.active_panel().panelize_command(),
            Some("find . -type f")
        );
        assert!(
            app.active_panel()
                .entries
                .iter()
                .any(|entry| entry.path == root.join("a.txt")),
            "preset command should populate panel entries"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[cfg(unix)]
    #[test]
    fn panelize_command_can_be_canceled_while_shell_process_runs() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-panelize-cancel-running-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let cancel_flag = Arc::new(AtomicBool::new(false));
        let cancel_clone = Arc::clone(&cancel_flag);
        let cancel_task = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            cancel_clone.store(true, AtomicOrdering::Relaxed);
        });

        let started_at = Instant::now();
        let result = read_panelized_entries_with_cancel(
            &root,
            "sleep 3; printf 'a.txt\\n'",
            SortMode::default(),
            Some(cancel_flag.as_ref()),
        );

        cancel_task
            .join()
            .expect("cancel request thread should finish");
        let error = result.expect_err("panelize command should be canceled");
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(error.to_string(), PANEL_REFRESH_CANCELED_MESSAGE);
        let elapsed = started_at.elapsed();
        assert!(
            elapsed < Duration::from_secs(1),
            "canceled panelize command should stop quickly, took {elapsed:?}"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn reread_coalesces_previous_refresh_for_same_panel() {
        use std::sync::atomic::Ordering as AtomicOrdering;

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-reread-cancel-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        fs::write(root.join("a.txt"), "a").expect("must create file");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.refresh_active_panel();
        assert_eq!(app.pending_worker_commands.len(), 1);

        let (first_job_id, first_request_id, first_cancel_flag) =
            match &app.pending_worker_commands[0] {
                WorkerCommand::Run(job) => match &job.request {
                    JobRequest::RefreshPanel { request_id, .. } => {
                        (job.id, *request_id, job.cancel_flag())
                    }
                    _ => panic!("expected refresh-panel job request"),
                },
                _ => panic!("expected worker run command"),
            };
        assert!(
            !first_cancel_flag.load(AtomicOrdering::Relaxed),
            "initial refresh should not be canceled"
        );

        app.refresh_active_panel();
        assert!(
            !first_cancel_flag.load(AtomicOrdering::Relaxed),
            "coalesced refresh should keep the existing queued request active"
        );
        assert!(
            !app.pending_worker_commands.iter().any(
                |command| matches!(command, WorkerCommand::Cancel(job_id) if *job_id == first_job_id)
            ),
            "coalesced refresh should not enqueue an explicit cancellation"
        );

        let (coalesced_job_id, second_request_id, second_cancel_flag) = app
            .pending_worker_commands
            .iter()
            .rev()
            .find_map(|command| {
                let WorkerCommand::Run(job) = command else {
                    return None;
                };
                let JobRequest::RefreshPanel { request_id, .. } = &job.request else {
                    return None;
                };
                Some((job.id, *request_id, job.cancel_flag()))
            })
            .expect("second refresh command should be queued");
        assert_eq!(
            coalesced_job_id, first_job_id,
            "coalescing should reuse the existing queued refresh job id"
        );
        assert!(
            second_request_id > first_request_id,
            "request ids should advance when a refresh request supersedes the queued one"
        );
        assert!(
            !second_cancel_flag.load(AtomicOrdering::Relaxed),
            "coalesced refresh should remain active"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn stale_panel_refresh_event_is_ignored() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-refresh-stale-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.refresh_active_panel();
        app.refresh_active_panel();
        let commands = app.take_pending_worker_commands();
        let refresh_requests: Vec<_> = commands
            .into_iter()
            .filter_map(|command| {
                let WorkerCommand::Run(job) = command else {
                    return None;
                };
                match job.request {
                    JobRequest::RefreshPanel {
                        panel,
                        cwd,
                        source,
                        sort_mode,
                        request_id,
                        ..
                    } => Some((panel, cwd, source, sort_mode, request_id)),
                    _ => None,
                }
            })
            .collect();
        assert_eq!(
            refresh_requests.len(),
            1,
            "superseded refreshes should coalesce while still queued"
        );

        let (panel, cwd, source, sort_mode, latest_request_id) = refresh_requests[0].clone();
        let stale_request_id = latest_request_id.saturating_sub(1);
        assert!(
            stale_request_id < latest_request_id,
            "stale request id should be older than the latest one"
        );

        app.handle_background_event(BackgroundEvent::PanelRefreshed {
            panel,
            cwd: cwd.clone(),
            source: source.clone(),
            sort_mode,
            request_id: stale_request_id,
            result: Ok(Vec::new()),
        });
        assert!(
            app.panels[panel.index()].loading,
            "stale refresh result should not clear loading state"
        );

        app.handle_background_event(BackgroundEvent::PanelRefreshed {
            panel,
            cwd,
            source,
            sort_mode,
            request_id: latest_request_id,
            result: Ok(Vec::new()),
        });
        assert!(
            !app.panels[panel.index()].loading,
            "latest refresh result should clear loading state"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn panel_refresh_chunks_preserve_existing_tags_until_final_result() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-refresh-chunk-tags-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let alpha_path = root.join("alpha.txt");
        let beta_path = root.join("beta.txt");
        fs::write(&alpha_path, "alpha").expect("alpha fixture should be writable");
        fs::write(&beta_path, "beta").expect("beta fixture should be writable");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let alpha_index = app
            .active_panel()
            .entries
            .iter()
            .position(|entry| entry.path == alpha_path)
            .expect("alpha entry should be visible");
        app.active_panel_mut().cursor = alpha_index;
        app.apply(AppCommand::ToggleTag)
            .expect("toggle tag should succeed");
        assert!(
            app.active_panel().is_tagged(&alpha_path),
            "precondition: alpha entry should start tagged"
        );

        app.refresh_active_panel();
        let (panel, cwd, source, sort_mode, request_id) = app
            .take_pending_worker_commands()
            .into_iter()
            .find_map(|command| {
                let WorkerCommand::Run(job) = command else {
                    return None;
                };
                let JobRequest::RefreshPanel {
                    panel,
                    cwd,
                    source,
                    sort_mode,
                    request_id,
                    ..
                } = job.request
                else {
                    return None;
                };
                Some((panel, cwd, source, sort_mode, request_id))
            })
            .expect("refresh command should be queued");

        app.handle_background_event(BackgroundEvent::PanelEntriesChunk {
            panel,
            cwd: cwd.clone(),
            source: source.clone(),
            sort_mode,
            request_id,
            entries: vec![FileEntry::file(
                String::from("beta.txt"),
                beta_path.clone(),
                4,
                None,
            )],
        });
        assert!(
            app.active_panel().is_tagged(&alpha_path),
            "chunk updates should not prune existing tags before final listing"
        );

        let final_entries =
            read_entries_with_visibility(&cwd, sort_mode, true).expect("listing should build");
        app.handle_background_event(BackgroundEvent::PanelRefreshed {
            panel,
            cwd,
            source,
            sort_mode,
            request_id,
            result: Ok(final_entries),
        });
        assert!(
            app.active_panel().is_tagged(&alpha_path),
            "tag should survive final listing when target is still present"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn refresh_dispatch_failure_clears_loading_state() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-refresh-dispatch-failure-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let panel_index = app.active_panel.index();
        app.refresh_active_panel();
        assert!(
            app.panels[panel_index].loading,
            "refresh should set panel loading before dispatch"
        );

        let refresh_job_id = app
            .take_pending_worker_commands()
            .into_iter()
            .find_map(|command| {
                let WorkerCommand::Run(job) = command else {
                    return None;
                };
                matches!(job.request, JobRequest::RefreshPanel { .. }).then_some(job.id)
            })
            .expect("refresh command should be queued");

        app.handle_job_dispatch_failure(
            refresh_job_id,
            JobError::dispatch("runtime queue is full"),
        );
        assert!(
            !app.panels[panel_index].loading,
            "failed refresh dispatch should clear loading state"
        );
        assert_eq!(
            app.panel_refresh_job_ids[panel_index], None,
            "failed refresh dispatch should clear tracked refresh job id"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn refresh_cancel_before_start_clears_loading_state() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-refresh-cancel-before-start-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        let panel_index = app.active_panel.index();
        app.refresh_active_panel();
        assert!(
            app.panels[panel_index].loading,
            "refresh should set panel loading before dispatch"
        );

        let refresh_job_id = app
            .take_pending_worker_commands()
            .into_iter()
            .find_map(|command| {
                let WorkerCommand::Run(job) = command else {
                    return None;
                };
                matches!(job.request, JobRequest::RefreshPanel { .. }).then_some(job.id)
            })
            .expect("refresh command should be queued");

        app.handle_job_event(JobEvent::Finished {
            id: refresh_job_id,
            result: Err(JobError::canceled()),
        });
        assert!(
            !app.panels[panel_index].loading,
            "canceled refresh without background event should clear loading state"
        );
        assert_eq!(
            app.panel_refresh_job_ids[panel_index], None,
            "canceled refresh should clear tracked refresh job id"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn xmap_mode_applies_to_next_file_manager_command_only() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-xmap-mode-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut app = AppState::new(root.clone()).expect("app should initialize");
        assert_eq!(app.key_context(), KeyContext::FileManager);
        app.apply(AppCommand::EnterXMap)
            .expect("xmap mode should activate");
        assert_eq!(app.key_context(), KeyContext::FileManagerXMap);
        app.apply(AppCommand::MoveDown)
            .expect("next command should execute");
        assert_eq!(app.key_context(), KeyContext::FileManager);

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn resolve_external_editor_command_prefers_editor_over_visual() {
        let editor = resolve_external_editor_command_with_lookup(|name| match name {
            "EDITOR" => Some(String::from("  nvim  ")),
            "VISUAL" => Some(String::from("vim")),
            _ => None,
        });
        assert_eq!(editor, Some(String::from("nvim")));
    }

    #[test]
    fn resolve_external_editor_command_uses_visual_then_none() {
        let editor = resolve_external_editor_command_with_lookup(|name| match name {
            "EDITOR" => Some(String::from("  ")),
            "VISUAL" => Some(String::from(" code --wait ")),
            _ => None,
        });
        assert_eq!(editor, Some(String::from("code --wait")));

        let missing = resolve_external_editor_command_with_lookup(|_| None);
        assert_eq!(missing, None);
    }

    #[test]
    fn app_command_mapping_is_context_aware() {
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenHelp),
            Some(AppCommand::OpenHelp)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Help, &KeyCommand::Quit),
            Some(AppCommand::CloseHelp)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Help, &KeyCommand::HelpBack),
            Some(AppCommand::HelpBack)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenMenu),
            Some(AppCommand::OpenMenu)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::CursorUp),
            Some(AppCommand::MenuMoveUp)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::CursorDown),
            Some(AppCommand::MenuMoveDown)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::CursorLeft),
            Some(AppCommand::MenuMoveLeft)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::CursorRight),
            Some(AppCommand::MenuMoveRight)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::DialogAccept),
            Some(AppCommand::MenuAccept)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::DialogCancel),
            Some(AppCommand::CloseMenu)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::CursorUp),
            Some(AppCommand::MoveUp)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenEntry),
            Some(AppCommand::OpenEntry)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::EditEntry),
            Some(AppCommand::EditEntry)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Listbox, &KeyCommand::CursorUp),
            Some(AppCommand::DialogListboxUp)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Listbox, &KeyCommand::OpenInputDialog),
            Some(AppCommand::PanelizePresetAdd)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Listbox, &KeyCommand::OpenConfirmDialog),
            Some(AppCommand::PanelizePresetEdit)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Listbox, &KeyCommand::Delete),
            Some(AppCommand::PanelizePresetRemove)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::DialogAccept),
            Some(AppCommand::DialogAccept)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::ToggleTag),
            Some(AppCommand::ToggleTag)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::SortNext),
            Some(AppCommand::SortNext)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::Copy),
            Some(AppCommand::Copy)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::Move),
            Some(AppCommand::Move)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::Delete),
            Some(AppCommand::Delete)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::CancelJob),
            Some(AppCommand::CancelJob)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenJobs),
            Some(AppCommand::OpenJobsScreen)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenFindDialog),
            Some(AppCommand::OpenFindDialog)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::CursorDown),
            Some(AppCommand::FindResultsMoveDown)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::OpenEntry),
            Some(AppCommand::FindResultsOpenEntry)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::OpenPanelizeDialog),
            Some(AppCommand::FindResultsPanelize)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::CancelJob),
            Some(AppCommand::CancelJob)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::Quit),
            Some(AppCommand::CloseFindResults)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenTree),
            Some(AppCommand::OpenTree)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::CursorUp),
            Some(AppCommand::TreeMoveUp)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::OpenEntry),
            Some(AppCommand::TreeOpenEntry)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::Quit),
            Some(AppCommand::CloseTree)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenHotlist),
            Some(AppCommand::OpenHotlist)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenPanelizeDialog),
            Some(AppCommand::OpenPanelizeDialog)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenSkinDialog),
            Some(AppCommand::OpenSkinDialog)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::EnterXMap),
            Some(AppCommand::EnterXMap)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Hotlist, &KeyCommand::AddHotlist),
            Some(AppCommand::HotlistAddCurrentDirectory)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Hotlist, &KeyCommand::RemoveHotlist),
            Some(AppCommand::HotlistRemoveSelected)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Hotlist, &KeyCommand::OpenEntry),
            Some(AppCommand::HotlistOpenEntry)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Hotlist, &KeyCommand::Quit),
            Some(AppCommand::CloseHotlist)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Jobs, &KeyCommand::CursorUp),
            Some(AppCommand::JobsMoveUp)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Jobs, &KeyCommand::CursorDown),
            Some(AppCommand::JobsMoveDown)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Jobs, &KeyCommand::CloseJobs),
            Some(AppCommand::CloseJobsScreen)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::Quit),
            Some(AppCommand::CloseViewer)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::Search),
            Some(AppCommand::ViewerSearchForward)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::SearchBackward),
            Some(AppCommand::ViewerSearchBackward)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::SearchContinue),
            Some(AppCommand::ViewerSearchContinue)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::SearchContinueBackward),
            Some(AppCommand::ViewerSearchContinueBackward)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::Goto),
            Some(AppCommand::ViewerGoto)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::ToggleWrap),
            Some(AppCommand::ViewerToggleWrap)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::ToggleHex),
            Some(AppCommand::ViewerToggleHex)
        );
        assert_eq!(
            AppCommand::from_key_command(KeyContext::ViewerHex, &KeyCommand::ToggleHex),
            Some(AppCommand::ViewerToggleHex)
        );
    }
}
