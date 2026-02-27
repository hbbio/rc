#![forbid(unsafe_code)]

pub mod dialog;
pub mod help;
pub mod jobs;
pub mod keymap;
pub mod settings;
pub mod settings_io;

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering as AtomicOrdering},
};
use std::thread;
use std::time::Duration;
use std::time::SystemTime;

pub use dialog::{DialogButtonFocus, DialogKind, DialogResult, DialogState};
pub use help::{HelpLine, HelpSpan, HelpState};
pub use jobs::{
    JOB_CANCELED_MESSAGE, JobEvent, JobId, JobKind, JobManager, JobProgress, JobRecord, JobRequest,
    JobStatus, JobStatusCounts, OverwritePolicy, WorkerCommand, WorkerJob, run_worker,
};
pub use settings::{
    AdvancedSettings, AppearanceSettings, ConfigurationSettings, ConfirmationSettings,
    DEFAULT_PANELIZE_PRESETS, DisplayBitsSettings, LayoutSettings, LearnKeysSettings,
    PanelOptionsSettings, SaveSetupMetadata, Settings, SettingsCategory, SettingsSortField,
    VirtualFsSettings,
};

use crate::dialog::DialogEvent;
use crate::keymap::{KeyChord, KeyCode, KeyCommand, KeyContext, Keymap, KeymapParseReport};

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

const FIND_EVENT_CHUNK_SIZE: usize = 64;
const PANEL_REFRESH_CANCELED_MESSAGE: &str = "panel refresh canceled";
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
        let Some(entry) = self.selected_entry() else {
            return false;
        };
        if !entry.is_dir {
            return false;
        }

        self.cwd = entry.path.clone();
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ViewerSearchDirection {
    Forward,
    Backward,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ViewerGotoTarget {
    Line(usize),
    Offset(usize),
}

#[derive(Clone, Debug)]
pub struct ViewerState {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub content: String,
    pub scroll: usize,
    pub wrap: bool,
    pub hex_mode: bool,
    line_offsets: Vec<usize>,
    last_search_query: Option<String>,
    last_search_match_offset: Option<usize>,
    last_search_direction: ViewerSearchDirection,
}

impl ViewerState {
    pub fn open(path: PathBuf) -> io::Result<Self> {
        let bytes = fs::read(&path)?;
        let hex_mode = should_default_to_hex_mode(&bytes);
        let content = String::from_utf8_lossy(&bytes).into_owned();
        let line_offsets = compute_line_offsets(&content);

        Ok(Self {
            path,
            bytes,
            content,
            scroll: 0,
            wrap: false,
            hex_mode,
            line_offsets,
            last_search_query: None,
            last_search_match_offset: None,
            last_search_direction: ViewerSearchDirection::Forward,
        })
    }

    pub fn line_count(&self) -> usize {
        if self.hex_mode {
            self.hex_line_count()
        } else {
            self.line_offsets.len()
        }
    }

    pub fn current_line_number(&self) -> usize {
        self.scroll.saturating_add(1)
    }

    pub fn last_search_query(&self) -> Option<&str> {
        self.last_search_query.as_deref()
    }

    pub fn move_lines(&mut self, delta: isize) {
        let max = self.line_count().saturating_sub(1);
        if delta.is_negative() {
            self.scroll = self.scroll.saturating_sub(delta.unsigned_abs());
        } else {
            self.scroll = self.scroll.saturating_add(delta as usize).min(max);
        }
    }

    pub fn move_pages(&mut self, pages: isize, viewer_page_step: usize) {
        self.move_lines(pages.saturating_mul(viewer_page_step as isize));
    }

    pub fn move_home(&mut self) {
        self.scroll = 0;
    }

    pub fn move_end(&mut self) {
        self.scroll = self.line_count().saturating_sub(1);
    }

    pub fn toggle_wrap(&mut self) {
        self.wrap = !self.wrap;
    }

    pub fn toggle_hex_mode(&mut self) {
        self.hex_mode = !self.hex_mode;
        self.scroll = self.scroll.min(self.line_count().saturating_sub(1));
    }

    fn start_search(&mut self, query: String, direction: ViewerSearchDirection) -> Option<usize> {
        self.last_search_query = Some(query);
        self.last_search_direction = direction;
        self.last_search_match_offset = None;
        self.continue_search(Some(direction))
    }

    fn continue_search(&mut self, direction: Option<ViewerSearchDirection>) -> Option<usize> {
        let query = self.last_search_query.as_deref()?;
        if query.is_empty() {
            return None;
        }
        let direction = direction.unwrap_or(self.last_search_direction);
        let start = match direction {
            ViewerSearchDirection::Forward => self
                .last_search_match_offset
                .map(|offset| offset.saturating_add(query.len()))
                .unwrap_or_else(|| self.current_line_offset()),
            ViewerSearchDirection::Backward => self
                .last_search_match_offset
                .unwrap_or_else(|| self.current_line_offset()),
        };
        let found = match direction {
            ViewerSearchDirection::Forward => find_forward_wrap(&self.content, query, start),
            ViewerSearchDirection::Backward => find_backward_wrap(&self.content, query, start),
        }?;

        self.last_search_match_offset = Some(found);
        self.last_search_direction = direction;
        self.scroll = self.line_index_for_offset(found);
        Some(self.scroll)
    }

    fn goto_input(&mut self, input: &str) -> Result<usize, String> {
        let target = parse_viewer_goto_target(input)?;
        match target {
            ViewerGotoTarget::Line(line) => {
                if line == 0 {
                    return Err(String::from("line numbers start at 1"));
                }
                self.scroll = line
                    .saturating_sub(1)
                    .min(self.line_count().saturating_sub(1));
            }
            ViewerGotoTarget::Offset(offset) => {
                let max_offset = if self.hex_mode {
                    self.bytes.len()
                } else {
                    self.content.len()
                };
                let bounded = offset.min(max_offset);
                self.scroll = self.line_index_for_offset(bounded);
            }
        }
        Ok(self.current_line_number())
    }

    fn current_line_offset(&self) -> usize {
        if self.hex_mode {
            return self
                .scroll
                .saturating_mul(16)
                .min(self.bytes.len().saturating_sub(1));
        }
        let index = self.scroll.min(self.line_count().saturating_sub(1));
        self.line_offsets[index]
    }

    fn line_index_for_offset(&self, offset: usize) -> usize {
        if self.hex_mode {
            return offset
                .saturating_div(16)
                .min(self.hex_line_count().saturating_sub(1));
        }
        if self.line_offsets.is_empty() {
            return 0;
        }
        let bounded = offset.min(self.content.len());
        match self.line_offsets.binary_search(&bounded) {
            Ok(index) => index,
            Err(0) => 0,
            Err(index) => index.saturating_sub(1),
        }
    }

    fn hex_line_count(&self) -> usize {
        let lines = (self.bytes.len().saturating_add(15)).saturating_div(16);
        lines.max(1)
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

#[derive(Clone, Debug)]
pub enum BackgroundCommand {
    RefreshPanel {
        panel: ActivePanel,
        cwd: PathBuf,
        source: PanelListingSource,
        sort_mode: SortMode,
        show_hidden_files: bool,
        request_id: u64,
        cancel_flag: Arc<AtomicBool>,
    },
    LoadViewer {
        path: PathBuf,
    },
    FindEntries {
        job_id: JobId,
        query: String,
        base_dir: PathBuf,
        max_results: usize,
        cancel_flag: Arc<AtomicBool>,
        pause_flag: Arc<AtomicBool>,
    },
    BuildTree {
        root: PathBuf,
        max_depth: usize,
        max_entries: usize,
    },
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum BackgroundEvent {
    PanelRefreshed {
        panel: ActivePanel,
        cwd: PathBuf,
        source: PanelListingSource,
        sort_mode: SortMode,
        request_id: u64,
        result: Result<Vec<FileEntry>, String>,
    },
    ViewerLoaded {
        path: PathBuf,
        result: Result<ViewerState, String>,
    },
    FindEntriesStarted {
        job_id: JobId,
    },
    FindEntriesChunk {
        job_id: JobId,
        entries: Vec<FindResultEntry>,
    },
    FindEntriesFinished {
        job_id: JobId,
        result: Result<(), String>,
    },
    TreeReady {
        root: PathBuf,
        entries: Vec<TreeEntry>,
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

pub fn run_background_worker(
    command_rx: Receiver<BackgroundCommand>,
    event_tx: Sender<BackgroundEvent>,
) {
    let mut running_find_tasks = Vec::new();
    #[cfg(not(test))]
    let mut running_panel_refresh_tasks = Vec::new();
    #[cfg(not(test))]
    let mut running_tree_tasks = Vec::new();
    while let Ok(command) = command_rx.recv() {
        reap_finished_find_tasks(&mut running_find_tasks);
        #[cfg(not(test))]
        reap_finished_panel_refresh_tasks(&mut running_panel_refresh_tasks);
        #[cfg(not(test))]
        reap_finished_tree_tasks(&mut running_tree_tasks);
        match execute_background_command(command, &event_tx) {
            BackgroundExecution::Continue => {}
            #[cfg(not(test))]
            BackgroundExecution::SpawnFind(task) => running_find_tasks.push(task),
            #[cfg(not(test))]
            BackgroundExecution::SpawnPanelRefresh(task) => running_panel_refresh_tasks.push(task),
            #[cfg(not(test))]
            BackgroundExecution::SpawnTree(task) => running_tree_tasks.push(task),
            BackgroundExecution::Stop => break,
        }
    }

    for task in &running_find_tasks {
        task.cancel_flag.store(true, AtomicOrdering::Relaxed);
    }
    #[cfg(not(test))]
    for task in &running_panel_refresh_tasks {
        task.cancel_flag.store(true, AtomicOrdering::Relaxed);
    }
    for task in running_find_tasks {
        let _ = task.handle.join();
    }
    #[cfg(not(test))]
    for task in running_panel_refresh_tasks {
        let _ = task.handle.join();
    }
    #[cfg(not(test))]
    for task in running_tree_tasks {
        let _ = task.handle.join();
    }
}

#[derive(Debug)]
struct RunningFindTask {
    handle: thread::JoinHandle<()>,
    cancel_flag: Arc<AtomicBool>,
}

#[cfg(not(test))]
#[derive(Debug)]
struct RunningPanelRefreshTask {
    handle: thread::JoinHandle<()>,
    cancel_flag: Arc<AtomicBool>,
}

#[cfg(not(test))]
#[derive(Debug)]
struct RunningTreeTask {
    handle: thread::JoinHandle<()>,
}

#[derive(Debug)]
enum BackgroundExecution {
    Continue,
    #[cfg(not(test))]
    SpawnFind(RunningFindTask),
    #[cfg(not(test))]
    SpawnPanelRefresh(RunningPanelRefreshTask),
    #[cfg(not(test))]
    SpawnTree(RunningTreeTask),
    Stop,
}

fn reap_finished_find_tasks(tasks: &mut Vec<RunningFindTask>) {
    let mut index = 0usize;
    while index < tasks.len() {
        if tasks[index].handle.is_finished() {
            let task = tasks.swap_remove(index);
            let _ = task.handle.join();
        } else {
            index += 1;
        }
    }
}

#[cfg(not(test))]
fn reap_finished_panel_refresh_tasks(tasks: &mut Vec<RunningPanelRefreshTask>) {
    let mut index = 0usize;
    while index < tasks.len() {
        if tasks[index].handle.is_finished() {
            let task = tasks.swap_remove(index);
            let _ = task.handle.join();
        } else {
            index += 1;
        }
    }
}

#[cfg(not(test))]
fn reap_finished_tree_tasks(tasks: &mut Vec<RunningTreeTask>) {
    let mut index = 0usize;
    while index < tasks.len() {
        if tasks[index].handle.is_finished() {
            let task = tasks.swap_remove(index);
            let _ = task.handle.join();
        } else {
            index += 1;
        }
    }
}

fn refresh_panel_entries(
    cwd: &Path,
    source: &PanelListingSource,
    sort_mode: SortMode,
    show_hidden_files: bool,
    cancel_flag: &AtomicBool,
) -> Result<Vec<FileEntry>, String> {
    match source {
        PanelListingSource::Directory => read_entries_with_visibility_cancel(
            cwd,
            sort_mode,
            show_hidden_files,
            Some(cancel_flag),
        )
        .map_err(|error| error.to_string()),
        PanelListingSource::Panelize { command } => {
            read_panelized_entries_with_cancel(cwd, command, sort_mode, Some(cancel_flag))
                .map_err(|error| error.to_string())
        }
        PanelListingSource::FindResults {
            base_dir, paths, ..
        } => read_panelized_paths(base_dir, paths, sort_mode, Some(cancel_flag))
            .map_err(|error| error.to_string()),
    }
}

fn execute_background_command(
    command: BackgroundCommand,
    event_tx: &Sender<BackgroundEvent>,
) -> BackgroundExecution {
    match command {
        BackgroundCommand::RefreshPanel {
            panel,
            cwd,
            source,
            sort_mode,
            show_hidden_files,
            request_id,
            cancel_flag,
        } => {
            #[cfg(test)]
            {
                let result = refresh_panel_entries(
                    &cwd,
                    &source,
                    sort_mode,
                    show_hidden_files,
                    cancel_flag.as_ref(),
                );
                if event_tx
                    .send(BackgroundEvent::PanelRefreshed {
                        panel,
                        cwd,
                        source,
                        sort_mode,
                        request_id,
                        result,
                    })
                    .is_ok()
                {
                    BackgroundExecution::Continue
                } else {
                    BackgroundExecution::Stop
                }
            }
            #[cfg(not(test))]
            {
                let worker_event_tx = event_tx.clone();
                let worker_cancel_flag = cancel_flag.clone();
                let worker_cwd = cwd.clone();
                let worker_source = source.clone();
                match thread::Builder::new()
                    .name(format!("rc-refresh-{}-{request_id}", panel.index()))
                    .spawn(move || {
                        let result = refresh_panel_entries(
                            &worker_cwd,
                            &worker_source,
                            sort_mode,
                            show_hidden_files,
                            worker_cancel_flag.as_ref(),
                        );
                        let _ = worker_event_tx.send(BackgroundEvent::PanelRefreshed {
                            panel,
                            cwd: worker_cwd,
                            source: worker_source,
                            sort_mode,
                            request_id,
                            result,
                        });
                    }) {
                    Ok(handle) => BackgroundExecution::SpawnPanelRefresh(RunningPanelRefreshTask {
                        handle,
                        cancel_flag,
                    }),
                    Err(error) => {
                        let _ = event_tx.send(BackgroundEvent::PanelRefreshed {
                            panel,
                            cwd,
                            source,
                            sort_mode,
                            request_id,
                            result: Err(format!("failed to spawn panel refresh worker: {error}")),
                        });
                        BackgroundExecution::Continue
                    }
                }
            }
        }
        BackgroundCommand::LoadViewer { path } => {
            if event_tx
                .send(BackgroundEvent::ViewerLoaded {
                    path: path.clone(),
                    result: ViewerState::open(path).map_err(|error| error.to_string()),
                })
                .is_ok()
            {
                BackgroundExecution::Continue
            } else {
                BackgroundExecution::Stop
            }
        }
        BackgroundCommand::FindEntries {
            job_id,
            query,
            base_dir,
            max_results,
            cancel_flag,
            pause_flag,
        } => {
            #[cfg(test)]
            {
                if run_find_search(
                    event_tx,
                    job_id,
                    query,
                    base_dir,
                    max_results,
                    cancel_flag.as_ref(),
                    pause_flag.as_ref(),
                ) {
                    BackgroundExecution::Continue
                } else {
                    BackgroundExecution::Stop
                }
            }
            #[cfg(not(test))]
            {
                let worker_event_tx = event_tx.clone();
                let worker_cancel_flag = cancel_flag.clone();
                let worker_pause_flag = pause_flag.clone();
                match thread::Builder::new()
                    .name(format!("rc-find-{job_id}"))
                    .spawn(move || {
                        let _ = run_find_search(
                            &worker_event_tx,
                            job_id,
                            query,
                            base_dir,
                            max_results,
                            worker_cancel_flag.as_ref(),
                            worker_pause_flag.as_ref(),
                        );
                    }) {
                    Ok(handle) => BackgroundExecution::SpawnFind(RunningFindTask {
                        handle,
                        cancel_flag,
                    }),
                    Err(error) => {
                        let _ = event_tx.send(BackgroundEvent::FindEntriesFinished {
                            job_id,
                            result: Err(format!("failed to spawn find worker: {error}")),
                        });
                        BackgroundExecution::Continue
                    }
                }
            }
        }
        BackgroundCommand::BuildTree {
            root,
            max_depth,
            max_entries,
        } => {
            #[cfg(test)]
            {
                let entries = build_tree_entries(&root, max_depth, max_entries);
                if event_tx
                    .send(BackgroundEvent::TreeReady { root, entries })
                    .is_ok()
                {
                    BackgroundExecution::Continue
                } else {
                    BackgroundExecution::Stop
                }
            }
            #[cfg(not(test))]
            {
                let worker_event_tx = event_tx.clone();
                let worker_root = root.clone();
                match thread::Builder::new()
                    .name(String::from("rc-tree"))
                    .spawn(move || {
                        let entries = build_tree_entries(&worker_root, max_depth, max_entries);
                        let _ = worker_event_tx.send(BackgroundEvent::TreeReady {
                            root: worker_root,
                            entries,
                        });
                    }) {
                    Ok(handle) => BackgroundExecution::SpawnTree(RunningTreeTask { handle }),
                    Err(_error) => {
                        let entries = build_tree_entries(&root, max_depth, max_entries);
                        if event_tx
                            .send(BackgroundEvent::TreeReady { root, entries })
                            .is_ok()
                        {
                            BackgroundExecution::Continue
                        } else {
                            BackgroundExecution::Stop
                        }
                    }
                }
            }
        }
        BackgroundCommand::Shutdown => BackgroundExecution::Stop,
    }
}

fn run_find_search(
    event_tx: &Sender<BackgroundEvent>,
    job_id: JobId,
    query: String,
    base_dir: PathBuf,
    max_results: usize,
    cancel_flag: &AtomicBool,
    pause_flag: &AtomicBool,
) -> bool {
    if event_tx
        .send(BackgroundEvent::FindEntriesStarted { job_id })
        .is_err()
    {
        return false;
    }

    let result = stream_find_entries(
        &base_dir,
        &query,
        max_results,
        cancel_flag,
        pause_flag,
        FIND_EVENT_CHUNK_SIZE,
        |entries| {
            event_tx
                .send(BackgroundEvent::FindEntriesChunk { job_id, entries })
                .is_ok()
        },
    );

    event_tx
        .send(BackgroundEvent::FindEntriesFinished { job_id, result })
        .is_ok()
}

fn stream_find_entries<F>(
    base_dir: &Path,
    query: &str,
    max_results: usize,
    cancel_flag: &AtomicBool,
    pause_flag: &AtomicBool,
    chunk_size: usize,
    mut emit_chunk: F,
) -> Result<(), String>
where
    F: FnMut(Vec<FindResultEntry>) -> bool,
{
    if max_results == 0 {
        return Ok(());
    }

    let normalized_query = query.trim().to_lowercase();
    if normalized_query.is_empty() {
        return Ok(());
    }

    let wildcard_query = normalized_query.contains('*') || normalized_query.contains('?');
    let chunk_size = chunk_size.max(1);
    let mut emitted = Vec::new();
    let mut matched = 0usize;
    let mut stack = vec![base_dir.to_path_buf()];

    while let Some(dir) = stack.pop() {
        wait_for_find_resume(cancel_flag, pause_flag)?;

        let read_dir = match fs::read_dir(&dir) {
            Ok(read_dir) => read_dir,
            Err(_) => continue,
        };
        let mut child_dirs = Vec::new();

        for entry in read_dir.flatten() {
            wait_for_find_resume(cancel_flag, pause_flag)?;

            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = file_type.is_dir();

            if query_matches_entry(&name, &normalized_query, wildcard_query) {
                emitted.push(FindResultEntry {
                    path: path.clone(),
                    is_dir,
                });
                matched = matched.saturating_add(1);

                if emitted.len() >= chunk_size && !emit_chunk(std::mem::take(&mut emitted)) {
                    return Err(String::from("background event channel disconnected"));
                }

                if matched >= max_results {
                    if !emitted.is_empty() && !emit_chunk(std::mem::take(&mut emitted)) {
                        return Err(String::from("background event channel disconnected"));
                    }
                    return Ok(());
                }
            }

            if is_dir {
                child_dirs.push(path);
            }
        }

        child_dirs.sort_by_cached_key(|left| path_sort_key(left));
        for child_dir in child_dirs.into_iter().rev() {
            stack.push(child_dir);
        }
    }

    if !emitted.is_empty() && !emit_chunk(emitted) {
        return Err(String::from("background event channel disconnected"));
    }
    Ok(())
}

fn wait_for_find_resume(cancel_flag: &AtomicBool, pause_flag: &AtomicBool) -> Result<(), String> {
    loop {
        if cancel_flag.load(AtomicOrdering::Relaxed) {
            return Err(String::from(JOB_CANCELED_MESSAGE));
        }
        if !pause_flag.load(AtomicOrdering::Relaxed) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn query_matches_entry(name: &str, normalized_query: &str, wildcard_query: bool) -> bool {
    let normalized_name = name.to_lowercase();
    if wildcard_query {
        wildcard_match(&normalized_name, normalized_query)
    } else {
        normalized_name.contains(normalized_query)
    }
}

fn wildcard_match(text: &str, pattern: &str) -> bool {
    let text: Vec<char> = text.chars().collect();
    let pattern: Vec<char> = pattern.chars().collect();
    let mut text_index = 0usize;
    let mut pattern_index = 0usize;
    let mut star_index: Option<usize> = None;
    let mut match_index = 0usize;

    while text_index < text.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == '?' || pattern[pattern_index] == text[text_index])
        {
            text_index += 1;
            pattern_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == '*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            match_index = text_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            match_index += 1;
            text_index = match_index;
        } else {
            return false;
        }
    }

    while pattern_index < pattern.len() && pattern[pattern_index] == '*' {
        pattern_index += 1;
    }

    pattern_index == pattern.len()
}

#[derive(Debug)]
pub struct AppState {
    settings: Settings,
    pub panels: [PanelState; 2],
    pub active_panel: ActivePanel,
    pub status_line: String,
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
    pending_background_commands: Vec<BackgroundCommand>,
    pending_external_edit_requests: Vec<ExternalEditRequest>,
    panel_refresh_cancel_flags: [Option<Arc<AtomicBool>>; 2],
    panel_refresh_request_ids: [u64; 2],
    next_panel_refresh_request_id: u64,
    pending_panel_focus: Option<(ActivePanel, PathBuf)>,
    find_pause_flags: HashMap<JobId, Arc<AtomicBool>>,
    pending_panelize_revert: Option<(ActivePanel, PanelListingSource)>,
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
            pending_background_commands: Vec::new(),
            pending_external_edit_requests: Vec::new(),
            panel_refresh_cancel_flags: std::array::from_fn(|_| None),
            panel_refresh_request_ids: [0; 2],
            next_panel_refresh_request_id: 1,
            pending_panel_focus: None,
            find_pause_flags: HashMap::new(),
            pending_panelize_revert: None,
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

        self.pending_background_commands
            .push(BackgroundCommand::LoadViewer { path });
        EditSelectionResult::OpenedInternal
    }

    fn open_selected_file_in_viewer(&mut self) -> bool {
        let Some(entry) = self.selected_non_parent_entry() else {
            return false;
        };
        if entry.is_dir {
            return false;
        }

        self.pending_background_commands
            .push(BackgroundCommand::LoadViewer {
                path: entry.path.clone(),
            });
        true
    }

    pub fn active_viewer(&self) -> Option<&ViewerState> {
        self.routes.iter().rev().find_map(|route| match route {
            Route::Viewer(viewer) => Some(viewer),
            _ => None,
        })
    }

    fn active_viewer_mut(&mut self) -> Option<&mut ViewerState> {
        self.routes.iter_mut().rev().find_map(|route| match route {
            Route::Viewer(viewer) => Some(viewer),
            _ => None,
        })
    }

    fn find_results_by_job_id(&self, job_id: JobId) -> Option<&FindResultsState> {
        self.routes
            .iter()
            .rev()
            .find_map(|route| match route {
                Route::FindResults(results) if results.job_id == job_id => Some(results),
                _ => None,
            })
            .or_else(|| {
                self.paused_find_results
                    .as_ref()
                    .filter(|results| results.job_id == job_id)
            })
    }

    fn find_results_by_job_id_mut(&mut self, job_id: JobId) -> Option<&mut FindResultsState> {
        if let Some(results) = self.routes.iter_mut().rev().find_map(|route| match route {
            Route::FindResults(results) if results.job_id == job_id => Some(results),
            _ => None,
        }) {
            return Some(results);
        }

        self.paused_find_results
            .as_mut()
            .filter(|results| results.job_id == job_id)
    }

    fn set_find_job_paused(&self, job_id: JobId, paused: bool) {
        if let Some(flag) = self.find_pause_flags.get(&job_id) {
            flag.store(paused, AtomicOrdering::Relaxed);
        }
    }

    fn pause_active_find_results(&mut self) -> bool {
        let Some(Route::FindResults(results)) = self.routes.pop() else {
            return false;
        };
        self.set_find_job_paused(results.job_id, true);
        self.paused_find_results = Some(results);
        true
    }

    fn resume_paused_find_results(&mut self) -> bool {
        if matches!(self.top_route(), Route::FindResults(_)) {
            return true;
        }
        let Some(results) = self.paused_find_results.take() else {
            return false;
        };
        self.set_find_job_paused(results.job_id, false);
        self.routes.push(Route::FindResults(results));
        true
    }

    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status_line = message.into();
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

    fn queue_panel_refresh(&mut self, panel: ActivePanel) {
        let panel_index = panel.index();
        if let Some(cancel_flag) = self.panel_refresh_cancel_flags[panel_index].as_ref() {
            cancel_flag.store(true, AtomicOrdering::Relaxed);
        }
        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.panel_refresh_cancel_flags[panel_index] = Some(cancel_flag.clone());
        let request_id = self.next_panel_refresh_request_id;
        self.next_panel_refresh_request_id = self.next_panel_refresh_request_id.saturating_add(1);
        self.panel_refresh_request_ids[panel_index] = request_id;

        let panel_state = &mut self.panels[panel.index()];
        panel_state.loading = true;
        self.pending_background_commands
            .push(BackgroundCommand::RefreshPanel {
                panel,
                cwd: panel_state.cwd.clone(),
                source: panel_state.source.clone(),
                sort_mode: panel_state.sort_mode,
                show_hidden_files: panel_state.show_hidden_files,
                request_id,
                cancel_flag,
            });
    }

    pub fn take_pending_worker_commands(&mut self) -> Vec<WorkerCommand> {
        std::mem::take(&mut self.pending_worker_commands)
    }

    pub fn take_pending_background_commands(&mut self) -> Vec<BackgroundCommand> {
        std::mem::take(&mut self.pending_background_commands)
    }

    pub fn take_pending_external_edit_requests(&mut self) -> Vec<ExternalEditRequest> {
        std::mem::take(&mut self.pending_external_edit_requests)
    }

    pub fn handle_job_event(&mut self, event: JobEvent) {
        if let JobEvent::Finished { id, .. } = &event {
            self.find_pause_flags.remove(id);
        }
        self.jobs.handle_event(&event);
        self.clamp_jobs_cursor();
        match event {
            JobEvent::Started { id } => {
                if let Some(job) = self.jobs.jobs().iter().find(|job| job.id == id) {
                    self.set_status(format!("Job #{id} started: {}", job.summary));
                } else {
                    self.set_status(format!("Job #{id} started"));
                }
            }
            JobEvent::Progress { id, progress } => {
                let percent = progress.percent();
                let path_label = progress
                    .current_path
                    .as_deref()
                    .and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| String::from("-"));
                self.set_status(format!(
                    "Job #{id} {percent}% | items {}/{} | bytes {}/{} | {path_label}",
                    progress.items_done,
                    progress.items_total,
                    progress.bytes_done,
                    progress.bytes_total
                ));
            }
            JobEvent::Finished { id, result } => match result {
                Ok(()) => {
                    let is_persist_settings = self
                        .jobs
                        .job(id)
                        .is_some_and(|job| matches!(job.kind, JobKind::PersistSettings));
                    if is_persist_settings {
                        self.mark_settings_saved(SystemTime::now());
                    }
                    let should_refresh = self.jobs.job(id).is_some_and(|job| {
                        matches!(
                            job.kind,
                            JobKind::Copy
                                | JobKind::Move
                                | JobKind::Delete
                                | JobKind::Mkdir
                                | JobKind::Rename
                        )
                    });
                    if should_refresh {
                        self.refresh_panels();
                    }
                    if let Some(job) = self.jobs.job(id) {
                        self.set_status(format!("Job #{id} finished: {}", job.summary));
                    } else {
                        self.set_status(format!("Job #{id} finished"));
                    }
                }
                Err(error) => {
                    if error == JOB_CANCELED_MESSAGE {
                        self.set_status(format!("Job #{id} canceled"));
                    } else {
                        self.set_status(format!("Job #{id} failed: {error}"));
                    }
                }
            },
        }
    }

    pub fn handle_background_event(&mut self, event: BackgroundEvent) {
        match event {
            BackgroundEvent::PanelRefreshed {
                panel,
                cwd,
                source,
                sort_mode,
                request_id,
                result,
            } => {
                if self.panel_refresh_request_ids[panel.index()] != request_id {
                    return;
                }
                let panel_state = &self.panels[panel.index()];
                let still_current = panel_state.cwd == cwd
                    && panel_state.source == source
                    && panel_state.sort_mode == sort_mode;
                if !still_current {
                    return;
                }

                let focus_target =
                    self.pending_panel_focus
                        .as_ref()
                        .and_then(|(pending_panel, path)| {
                            (*pending_panel == panel).then(|| path.clone())
                        });
                let mut clear_focus_target = false;
                let mut focus_status = None;
                {
                    let panel_state = &mut self.panels[panel.index()];
                    panel_state.loading = false;
                    match result {
                        Ok(entries) => {
                            panel_state.apply_entries(entries);
                            if self
                                .pending_panelize_revert
                                .as_ref()
                                .is_some_and(|(pending_panel, _)| *pending_panel == panel)
                            {
                                self.pending_panelize_revert = None;
                            }
                            if let Some(target_path) = focus_target {
                                clear_focus_target = true;
                                if let Some(index) = panel_state
                                    .entries
                                    .iter()
                                    .position(|entry| entry.path == target_path)
                                {
                                    panel_state.cursor = index;
                                    focus_status =
                                        Some(format!("Located {}", target_path.to_string_lossy()));
                                } else {
                                    focus_status = Some(format!(
                                        "Opened {} (target not found in listing)",
                                        panel_state.cwd.to_string_lossy()
                                    ));
                                }
                            }
                        }
                        Err(error) => {
                            let is_panelize = source.is_panelized();
                            if let Some((pending_panel, revert_source)) =
                                self.pending_panelize_revert.take()
                            {
                                if pending_panel == panel {
                                    panel_state.source = revert_source;
                                } else {
                                    self.pending_panelize_revert =
                                        Some((pending_panel, revert_source));
                                }
                            }
                            if error != PANEL_REFRESH_CANCELED_MESSAGE {
                                if is_panelize {
                                    self.set_status(format!("Panelize failed: {error}"));
                                } else {
                                    self.set_status(format!("Panel refresh failed: {error}"));
                                }
                            }
                        }
                    }
                }
                self.panel_refresh_cancel_flags[panel.index()] = None;
                if clear_focus_target {
                    self.pending_panel_focus = None;
                }
                if let Some(focus_status) = focus_status {
                    self.set_status(focus_status);
                }
            }
            BackgroundEvent::ViewerLoaded { path, result } => match result {
                Ok(viewer) => {
                    self.routes.push(Route::Viewer(viewer));
                    self.set_status(format!("Opened viewer {}", path.to_string_lossy()));
                }
                Err(error) => {
                    self.set_status(format!("Viewer open failed: {error}"));
                }
            },
            BackgroundEvent::FindEntriesStarted { job_id } => {
                self.handle_job_event(JobEvent::Started { id: job_id });
                if let Some(results) = self.find_results_by_job_id_mut(job_id) {
                    results.loading = true;
                }
            }
            BackgroundEvent::FindEntriesChunk { job_id, entries } => {
                let status_message = if let Some(results) = self.find_results_by_job_id_mut(job_id)
                {
                    let was_empty = results.entries.is_empty();
                    results.entries.extend(entries);
                    if was_empty && !results.entries.is_empty() {
                        results.cursor = 0;
                    }
                    Some(format!(
                        "Finding '{}': {} result(s)...",
                        results.query,
                        results.entries.len()
                    ))
                } else {
                    None
                };
                if let Some(status_message) = status_message {
                    self.set_status(status_message);
                }
            }
            BackgroundEvent::FindEntriesFinished { job_id, result } => {
                if let Some(results) = self.find_results_by_job_id_mut(job_id) {
                    results.loading = false;
                }
                let completed_successfully = result.is_ok();
                self.handle_job_event(JobEvent::Finished { id: job_id, result });
                let status_message = if completed_successfully {
                    self.find_results_by_job_id(job_id).map(|results| {
                        format!(
                            "Find '{}': {} result(s)",
                            results.query,
                            results.entries.len()
                        )
                    })
                } else {
                    None
                };
                if let Some(status_message) = status_message {
                    self.set_status(status_message);
                }
            }
            BackgroundEvent::TreeReady { root, entries } => {
                let mut replaced = false;
                for route in self.routes.iter_mut().rev() {
                    if let Route::Tree(tree) = route
                        && tree.root == root
                    {
                        tree.entries = entries.clone();
                        tree.cursor = 0;
                        tree.loading = false;
                        replaced = true;
                        break;
                    }
                }
                if replaced {
                    self.set_status(format!("Opened directory tree ({})", entries.len()));
                }
            }
        }
    }

    pub fn handle_job_dispatch_failure(&mut self, id: JobId, error: String) {
        self.handle_job_event(JobEvent::Finished {
            id,
            result: Err(error),
        });
    }

    pub fn jobs_status_counts(&self) -> JobStatusCounts {
        self.jobs.status_counts()
    }

    fn open_help_screen(&mut self) {
        let context = self.key_context();
        if let Some(Route::Help(help)) = self.routes.last_mut() {
            help.open_for_context(KeyContext::Help);
            self.set_status("Help: help viewer");
            return;
        }

        let replacements = self.help_replacements();
        self.routes
            .push(Route::Help(HelpState::for_context_with_replacements(
                context,
                &replacements,
            )));
        self.set_status("Opened help");
    }

    fn close_help_screen(&mut self) {
        if matches!(self.top_route(), Route::Help(_)) {
            self.routes.pop();
            self.set_status("Closed help");
        }
    }

    fn help_state_mut(&mut self) -> Option<&mut HelpState> {
        let Some(Route::Help(help)) = self.routes.last_mut() else {
            return None;
        };
        Some(help)
    }

    fn open_menu(&mut self, menu_index: usize) {
        if let Some(Route::Menu(menu)) = self.routes.last_mut() {
            menu.set_active_menu(menu_index);
            let title = menu.active_menu_title();
            self.set_status(format!("Menu: {title}"));
            return;
        }

        let menu = MenuState::new(menu_index);
        self.set_status(format!("Menu: {}", menu.active_menu_title()));
        self.routes.push(Route::Menu(menu));
    }

    fn close_menu(&mut self) {
        if matches!(self.top_route(), Route::Menu(_)) {
            self.routes.pop();
            self.set_status("Closed menu");
        }
    }

    fn open_settings_screen(&mut self, category: SettingsCategory) {
        self.pending_learn_keys_capture = false;
        let next = SettingsScreenState::new(category, self.settings_entries_for_category(category));
        if let Some(Route::Settings(current)) = self.routes.last_mut() {
            *current = next;
        } else {
            self.routes.push(Route::Settings(next));
        }
        self.set_status(format!("Options: {}", category.label()));
    }

    fn close_settings_screen(&mut self) {
        if matches!(self.top_route(), Route::Settings(_)) {
            self.pending_learn_keys_capture = false;
            self.routes.pop();
            self.set_status("Closed options");
        }
    }

    fn settings_state_mut(&mut self) -> Option<&mut SettingsScreenState> {
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

    fn refresh_settings_entries(&mut self) {
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

    fn apply_settings_entry(&mut self) {
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

    fn menu_state_mut(&mut self) -> Option<&mut MenuState> {
        let Some(Route::Menu(menu)) = self.routes.last_mut() else {
            return None;
        };
        Some(menu)
    }

    fn accept_menu_selection(&mut self) -> Option<AppCommand> {
        let selected = self
            .menu_state_mut()
            .and_then(|menu| menu.selected_command());
        self.close_menu();
        selected
    }

    fn accept_menu_selection_at(&mut self, index: usize) -> Option<AppCommand> {
        if let Some(menu) = self.menu_state_mut() {
            menu.select_entry(index);
        }
        self.accept_menu_selection()
    }

    pub fn command_for_left_click(&self, column: u16, row: u16) -> Option<AppCommand> {
        if !matches!(self.top_route(), Route::FileManager | Route::Menu(_)) {
            return None;
        }

        if row == 0
            && let Some(menu_index) = top_menu_hit_test(column)
        {
            return Some(AppCommand::OpenMenuAt(menu_index));
        }

        let Route::Menu(menu) = self.top_route() else {
            return None;
        };

        if let Some(entry_index) = self.menu_hit_test_entry(menu, column, row) {
            return Some(AppCommand::MenuSelectAt(entry_index));
        }

        Some(AppCommand::CloseMenu)
    }

    fn menu_hit_test_entry(&self, menu: &MenuState, column: u16, row: u16) -> Option<usize> {
        let x = menu.popup_origin_x();
        let y = 1u16;
        let width = self.menu_popup_width(menu);
        let items = menu.active_entries().len() as u16;
        if items == 0 {
            return None;
        }

        if row < y + 1 || row >= y + 1 + items {
            return None;
        }
        if column < x + 1 || column >= x + width.saturating_sub(1) {
            return None;
        }

        let index = (row - (y + 1)) as usize;
        menu.active_entries()
            .get(index)
            .filter(|entry| entry.selectable)
            .map(|_| index)
    }

    fn open_jobs_screen(&mut self) {
        if !matches!(self.top_route(), Route::Jobs) {
            self.routes.push(Route::Jobs);
        }
        self.clamp_jobs_cursor();
        self.set_status("Opened jobs screen");
    }

    fn close_jobs_screen(&mut self) {
        if matches!(self.top_route(), Route::Jobs) {
            self.routes.pop();
            self.set_status("Closed jobs screen");
        }
    }

    fn close_viewer(&mut self) {
        if matches!(self.top_route(), Route::Viewer(_)) {
            self.routes.pop();
            self.set_status("Closed viewer");
        }
    }

    fn open_find_dialog(&mut self) {
        if self.resume_paused_find_results() {
            self.set_status("Resumed find results");
            return;
        }

        let base_dir = self.active_panel().cwd.clone();
        self.pending_dialog_action = Some(PendingDialogAction::FindQuery { base_dir });
        self.routes.push(Route::Dialog(DialogState::input(
            "Find file",
            "Name contains:",
            "",
        )));
        self.set_status("Find file");
    }

    fn open_panelize_dialog(&mut self) {
        let initial_command = self
            .active_panel()
            .panelize_command()
            .unwrap_or("find . -type f")
            .to_string();
        let preset_commands = self.panelize_presets.clone();
        self.open_panelize_preset_selection_dialog(initial_command, preset_commands);
        self.set_status("External panelize");
    }

    fn open_panelize_preset_selection_dialog(
        &mut self,
        initial_command: String,
        preset_commands: Vec<String>,
    ) {
        let mut items = vec![String::from(PANELIZE_CUSTOM_COMMAND_LABEL)];
        items.extend(preset_commands.iter().cloned());
        let selected = panelize_preset_selected_index(&initial_command, &preset_commands);
        self.pending_dialog_action = Some(PendingDialogAction::PanelizePresetSelection {
            initial_command,
            preset_commands,
        });
        self.routes.push(Route::Dialog(DialogState::listbox(
            "External panelize",
            items,
            selected,
        )));
    }

    fn open_panelize_command_input_dialog(
        &mut self,
        initial_command: String,
        preset_commands: Vec<String>,
    ) {
        self.pending_dialog_action = Some(PendingDialogAction::PanelizeCommand { preset_commands });
        self.routes.push(Route::Dialog(DialogState::input(
            "External panelize",
            "Command (stdout paths):",
            initial_command,
        )));
    }

    fn toggle_panelize_dialog_focus(&mut self) -> bool {
        match self.pending_dialog_action.clone() {
            Some(PendingDialogAction::PanelizePresetSelection {
                initial_command,
                preset_commands,
            }) => {
                let is_listbox = matches!(
                    self.top_route(),
                    Route::Dialog(DialogState {
                        kind: DialogKind::Listbox(_),
                        ..
                    })
                );
                if !is_listbox {
                    return false;
                }
                self.routes.pop();
                self.open_panelize_command_input_dialog(initial_command, preset_commands);
                self.set_status("External panelize: enter command");
                true
            }
            Some(PendingDialogAction::PanelizeCommand { preset_commands }) => {
                let initial_command = match self.top_route() {
                    Route::Dialog(dialog) => match &dialog.kind {
                        DialogKind::Input(input) => input.value.clone(),
                        _ => return false,
                    },
                    _ => return false,
                };
                self.routes.pop();
                self.open_panelize_preset_selection_dialog(initial_command, preset_commands);
                self.set_status("External panelize");
                true
            }
            _ => false,
        }
    }

    fn active_panelize_preset_selection(&self) -> Option<(String, Vec<String>, usize)> {
        let PendingDialogAction::PanelizePresetSelection {
            initial_command,
            preset_commands,
        } = self.pending_dialog_action.clone()?
        else {
            return None;
        };
        let Route::Dialog(dialog) = self.top_route() else {
            return None;
        };
        let DialogKind::Listbox(listbox) = &dialog.kind else {
            return None;
        };
        Some((initial_command, preset_commands, listbox.selected))
    }

    fn start_panelize_preset_add(&mut self) {
        let Some((initial_command, preset_commands, _)) = self.active_panelize_preset_selection()
        else {
            return;
        };
        self.pending_dialog_action = Some(PendingDialogAction::PanelizePresetAdd {
            initial_command,
            preset_commands,
        });
        self.routes.push(Route::Dialog(DialogState::input(
            "Add panelize command",
            "Command:",
            "",
        )));
        self.set_status("Panelize preset: add command");
    }

    fn start_panelize_preset_edit(&mut self) {
        let Some((initial_command, preset_commands, selected_index)) =
            self.active_panelize_preset_selection()
        else {
            return;
        };
        if selected_index == 0 {
            self.set_status("Select a preset command to edit");
            return;
        }
        let preset_index = selected_index - 1;
        let Some(existing_command) = preset_commands.get(preset_index).cloned() else {
            self.set_status("Panelize preset selection is invalid");
            return;
        };
        self.pending_dialog_action = Some(PendingDialogAction::PanelizePresetEdit {
            initial_command,
            preset_commands,
            preset_index,
        });
        self.routes.push(Route::Dialog(DialogState::input(
            "Edit panelize command",
            "Command:",
            existing_command,
        )));
        self.set_status("Panelize preset: edit command");
    }

    fn remove_panelize_preset(&mut self) {
        let Some((initial_command, mut preset_commands, selected_index)) =
            self.active_panelize_preset_selection()
        else {
            return;
        };
        if selected_index == 0 {
            self.set_status("Select a preset command to remove");
            return;
        }
        let preset_index = selected_index - 1;
        let Some(removed_command) =
            (preset_index < preset_commands.len()).then(|| preset_commands.remove(preset_index))
        else {
            self.set_status("Panelize preset selection is invalid");
            return;
        };

        self.panelize_presets = preset_commands.clone();
        self.settings.configuration.panelize_presets = self.panelize_presets.clone();
        self.settings.mark_dirty();
        self.routes.pop();
        let next_initial = if initial_command == removed_command {
            preset_commands
                .first()
                .cloned()
                .unwrap_or_else(|| String::from("find . -type f"))
        } else {
            initial_command
        };
        self.open_panelize_preset_selection_dialog(next_initial, preset_commands);
        self.set_status(format!("Removed panelize preset: {removed_command}"));
    }

    fn start_panelize_command(&mut self, command: String) {
        let active_panel = self.active_panel;
        let previous_source = self.active_panel().source.clone();
        {
            let panel = self.active_panel_mut();
            panel.source = PanelListingSource::Panelize { command };
            panel.cursor = 0;
            panel.tagged.clear();
            panel.loading = true;
        }
        self.pending_panelize_revert = Some((active_panel, previous_source));
        self.queue_panel_refresh(active_panel);
        self.set_status("Panelize running...");
    }

    fn close_find_results(&mut self) {
        if matches!(self.top_route(), Route::FindResults(_)) {
            self.routes.pop();
            self.set_status("Closed find results");
        }
    }

    fn move_find_results_cursor(&mut self, delta: isize) {
        let Some(Route::FindResults(results)) = self.routes.last_mut() else {
            return;
        };
        results.move_cursor(delta);
    }

    fn move_find_results_page(&mut self, pages: isize) {
        let Some(Route::FindResults(results)) = self.routes.last_mut() else {
            return;
        };
        results.move_page(pages, self.settings.advanced.page_step);
    }

    fn move_find_results_home(&mut self) {
        let Some(Route::FindResults(results)) = self.routes.last_mut() else {
            return;
        };
        results.move_home();
    }

    fn move_find_results_end(&mut self) {
        let Some(Route::FindResults(results)) = self.routes.last_mut() else {
            return;
        };
        results.move_end();
    }

    fn open_selected_find_result(&mut self) -> io::Result<()> {
        let selected = match self.top_route() {
            Route::FindResults(results) => results.selected_entry().cloned(),
            _ => None,
        };
        let Some(selected) = selected else {
            self.set_status("No find result selected");
            return Ok(());
        };
        self.pending_panel_focus = None;

        if selected.is_dir {
            if self.set_active_panel_directory(selected.path.clone())? {
                self.pause_active_find_results();
                self.set_status(format!(
                    "Opened directory {} (Alt-F back to find)",
                    selected.path.to_string_lossy()
                ));
            } else {
                self.set_status("Selected result is not an accessible directory");
            }
            return Ok(());
        }

        let Some(parent_dir) = selected.path.parent().map(Path::to_path_buf) else {
            self.set_status("Selected result has no parent directory");
            return Ok(());
        };
        if self.set_active_panel_directory(parent_dir.clone())? {
            self.pending_panel_focus = Some((self.active_panel, selected.path.clone()));
            self.pause_active_find_results();
            self.set_status(format!(
                "Locating {} in {} (Alt-F back to find)",
                selected
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| selected.path.to_string_lossy().into_owned()),
                parent_dir.to_string_lossy()
            ));
        } else {
            self.set_status("Selected result parent directory is not accessible");
        }
        Ok(())
    }

    fn panelize_find_results(&mut self) {
        let Some((query, base_dir, paths)) = (match self.top_route() {
            Route::FindResults(results) => Some((
                results.query.clone(),
                results.base_dir.clone(),
                results
                    .entries
                    .iter()
                    .map(|entry| entry.path.clone())
                    .collect::<Vec<_>>(),
            )),
            _ => None,
        }) else {
            self.set_status("Find results are not active");
            return;
        };

        if paths.is_empty() {
            self.set_status("No find results to panelize");
            return;
        }

        let result_count = paths.len();
        let active_panel = self.active_panel;
        let previous_source = self.active_panel().source.clone();
        {
            let panel = self.active_panel_mut();
            panel.source = PanelListingSource::FindResults {
                query,
                base_dir,
                paths,
            };
            panel.cursor = 0;
            panel.tagged.clear();
            panel.loading = true;
        }
        self.pending_panelize_revert = Some((active_panel, previous_source));
        self.pause_active_find_results();
        self.queue_panel_refresh(active_panel);
        self.set_status(format!("Panelizing {result_count} find result(s)..."));
    }

    fn open_tree_screen(&mut self) {
        if matches!(self.top_route(), Route::Tree(_)) {
            return;
        }
        let root = self.active_panel().cwd.clone();
        self.routes
            .push(Route::Tree(TreeState::loading(root.clone())));
        self.pending_background_commands
            .push(BackgroundCommand::BuildTree {
                root,
                max_depth: self.settings.advanced.tree_max_depth,
                max_entries: self.settings.advanced.tree_max_entries,
            });
        self.set_status("Loading directory tree...");
    }

    fn close_tree_screen(&mut self) {
        if matches!(self.top_route(), Route::Tree(_)) {
            self.routes.pop();
            self.set_status("Closed directory tree");
        }
    }

    fn move_tree_cursor(&mut self, delta: isize) {
        let Some(Route::Tree(tree)) = self.routes.last_mut() else {
            return;
        };
        tree.move_cursor(delta);
    }

    fn move_tree_page(&mut self, pages: isize) {
        let Some(Route::Tree(tree)) = self.routes.last_mut() else {
            return;
        };
        tree.move_page(pages, self.settings.advanced.page_step);
    }

    fn move_tree_home(&mut self) {
        let Some(Route::Tree(tree)) = self.routes.last_mut() else {
            return;
        };
        tree.move_home();
    }

    fn move_tree_end(&mut self) {
        let Some(Route::Tree(tree)) = self.routes.last_mut() else {
            return;
        };
        tree.move_end();
    }

    fn open_selected_tree_entry(&mut self) -> io::Result<()> {
        let selected = match self.top_route() {
            Route::Tree(tree) => tree.selected_entry().cloned(),
            _ => None,
        };
        let Some(selected) = selected else {
            self.set_status("No tree entry selected");
            return Ok(());
        };

        if self.set_active_panel_directory(selected.path.clone())? {
            self.routes.pop();
            self.set_status(format!(
                "Opened directory {}",
                selected.path.to_string_lossy()
            ));
        } else {
            self.set_status("Selected tree entry is not an accessible directory");
        }
        Ok(())
    }

    fn open_hotlist_screen(&mut self) {
        if !matches!(self.top_route(), Route::Hotlist) {
            self.routes.push(Route::Hotlist);
        }
        self.clamp_hotlist_cursor();
        self.set_status("Opened directory hotlist");
    }

    fn close_hotlist_screen(&mut self) {
        if matches!(self.top_route(), Route::Hotlist) {
            self.routes.pop();
            self.set_status("Closed directory hotlist");
        }
    }

    fn clamp_hotlist_cursor(&mut self) {
        if self.hotlist.is_empty() {
            self.hotlist_cursor = 0;
        } else if self.hotlist_cursor >= self.hotlist.len() {
            self.hotlist_cursor = self.hotlist.len() - 1;
        }
    }

    fn move_hotlist_cursor(&mut self, delta: isize) {
        if self.hotlist.is_empty() {
            self.hotlist_cursor = 0;
            return;
        }
        let last = self.hotlist.len() - 1;
        let next = if delta.is_negative() {
            self.hotlist_cursor.saturating_sub(delta.unsigned_abs())
        } else {
            self.hotlist_cursor.saturating_add(delta as usize).min(last)
        };
        self.hotlist_cursor = next;
    }

    fn move_hotlist_page(&mut self, pages: isize) {
        self.move_hotlist_cursor(pages.saturating_mul(self.settings.advanced.page_step as isize));
    }

    fn move_hotlist_home(&mut self) {
        self.hotlist_cursor = 0;
    }

    fn move_hotlist_end(&mut self) {
        if self.hotlist.is_empty() {
            self.hotlist_cursor = 0;
        } else {
            self.hotlist_cursor = self.hotlist.len() - 1;
        }
    }

    fn add_current_directory_to_hotlist(&mut self) {
        let cwd = self.active_panel().cwd.clone();
        if self.hotlist.iter().any(|entry| entry == &cwd) {
            self.hotlist_cursor = self
                .hotlist
                .iter()
                .position(|entry| entry == &cwd)
                .unwrap_or(self.hotlist_cursor);
            self.set_status("Directory already exists in hotlist");
            return;
        }
        self.hotlist.push(cwd.clone());
        self.hotlist_cursor = self.hotlist.len() - 1;
        self.settings.configuration.hotlist = self.hotlist.clone();
        self.settings.mark_dirty();
        self.set_status(format!("Added {} to hotlist", cwd.to_string_lossy()));
    }

    fn remove_selected_hotlist_entry(&mut self) {
        if self.hotlist.is_empty() {
            self.set_status("Hotlist is empty");
            return;
        }
        let removed = self.hotlist.remove(self.hotlist_cursor);
        self.clamp_hotlist_cursor();
        self.settings.configuration.hotlist = self.hotlist.clone();
        self.settings.mark_dirty();
        self.set_status(format!(
            "Removed {} from hotlist",
            removed.to_string_lossy()
        ));
    }

    fn open_selected_hotlist_entry(&mut self) -> io::Result<()> {
        let Some(path) = self.hotlist.get(self.hotlist_cursor).cloned() else {
            self.set_status("No hotlist entry selected");
            return Ok(());
        };

        if self.set_active_panel_directory(path.clone())? {
            self.routes.pop();
            self.set_status(format!("Opened directory {}", path.to_string_lossy()));
        } else {
            self.set_status("Selected hotlist path is not an accessible directory");
        }
        Ok(())
    }

    fn set_active_panel_directory(&mut self, destination: PathBuf) -> io::Result<bool> {
        let metadata = match fs::metadata(&destination) {
            Ok(metadata) => metadata,
            Err(_) => return Ok(false),
        };
        if !metadata.is_dir() {
            return Ok(false);
        }

        let panel = self.active_panel_mut();
        panel.cwd = destination;
        panel.cursor = 0;
        panel.source = PanelListingSource::Directory;
        panel.tagged.clear();
        panel.entries.clear();
        panel.loading = true;
        self.queue_panel_refresh(self.active_panel);
        Ok(true)
    }

    fn clamp_jobs_cursor(&mut self) {
        let len = self.jobs.jobs().len();
        if len == 0 {
            self.jobs_cursor = 0;
        } else if self.jobs_cursor >= len {
            self.jobs_cursor = len - 1;
        }
    }

    fn move_jobs_cursor(&mut self, delta: isize) {
        let len = self.jobs.jobs().len();
        if len == 0 {
            self.jobs_cursor = 0;
            return;
        }
        let last = len - 1;
        let next = if delta.is_negative() {
            self.jobs_cursor.saturating_sub(delta.unsigned_abs())
        } else {
            self.jobs_cursor.saturating_add(delta as usize).min(last)
        };
        self.jobs_cursor = next;
    }

    fn open_viewer_search_dialog(&mut self, direction: ViewerSearchDirection) {
        let Some(viewer) = self.active_viewer() else {
            self.set_status("Viewer is not active");
            return;
        };
        let initial_query = viewer.last_search_query().unwrap_or("").to_string();

        let title = match direction {
            ViewerSearchDirection::Forward => "Search",
            ViewerSearchDirection::Backward => "Search Backward",
        };
        let prompt = match direction {
            ViewerSearchDirection::Forward => "Search text:",
            ViewerSearchDirection::Backward => "Search text (backward):",
        };

        self.pending_dialog_action = Some(PendingDialogAction::ViewerSearch { direction });
        self.routes.push(Route::Dialog(DialogState::input(
            title,
            prompt,
            initial_query,
        )));
        self.set_status(title);
    }

    fn open_viewer_goto_dialog(&mut self) {
        let Some(viewer) = self.active_viewer() else {
            self.set_status("Viewer is not active");
            return;
        };
        let current_line = viewer.current_line_number().to_string();

        self.pending_dialog_action = Some(PendingDialogAction::ViewerGoto);
        self.routes.push(Route::Dialog(DialogState::input(
            "Goto",
            "Line number, @offset, or 0xHEX offset:",
            current_line,
        )));
        self.set_status("Goto");
    }

    fn continue_viewer_search(&mut self, direction: Option<ViewerSearchDirection>) {
        let Some(viewer) = self.active_viewer_mut() else {
            self.set_status("Viewer is not active");
            return;
        };
        let Some(_) = viewer.last_search_query() else {
            self.set_status("No previous search query");
            return;
        };

        if let Some(line) = viewer.continue_search(direction) {
            self.set_status(format!("Search hit at line {}", line.saturating_add(1)));
        } else {
            self.set_status("Search text not found");
        }
    }

    pub fn selected_job_record(&self) -> Option<&JobRecord> {
        self.jobs.jobs().get(self.jobs_cursor)
    }

    pub fn apply(&mut self, command: AppCommand) -> io::Result<ApplyResult> {
        if self.xmap_pending && !matches!(self.top_route(), Route::FileManager) {
            self.xmap_pending = false;
        }
        let clear_xmap_after_command = self.xmap_pending
            && matches!(self.top_route(), Route::FileManager)
            && !matches!(command, AppCommand::EnterXMap);
        let mut follow_up_command = None;

        match command {
            AppCommand::MenuNoop => {}
            AppCommand::MenuNotImplemented(label) => {
                self.set_status(format!("{label} is not implemented yet"));
            }
            AppCommand::OpenMenu => self.open_menu(0),
            AppCommand::OpenMenuAt(index) => self.open_menu(index),
            AppCommand::CloseMenu => self.close_menu(),
            AppCommand::OpenHelp => self.open_help_screen(),
            AppCommand::CloseHelp => self.close_help_screen(),
            AppCommand::Quit => {
                if self.settings.confirmation.confirm_quit {
                    self.start_quit_confirmation();
                } else {
                    self.request_cancel_for_all_jobs();
                    return Ok(ApplyResult::Quit);
                }
            }
            AppCommand::CloseViewer => self.close_viewer(),
            AppCommand::OpenFindDialog => self.open_find_dialog(),
            AppCommand::CloseFindResults => self.close_find_results(),
            AppCommand::OpenTree => self.open_tree_screen(),
            AppCommand::CloseTree => self.close_tree_screen(),
            AppCommand::OpenHotlist => self.open_hotlist_screen(),
            AppCommand::CloseHotlist => self.close_hotlist_screen(),
            AppCommand::OpenPanelizeDialog => self.open_panelize_dialog(),
            AppCommand::PanelizePresetAdd => self.start_panelize_preset_add(),
            AppCommand::PanelizePresetEdit => self.start_panelize_preset_edit(),
            AppCommand::PanelizePresetRemove => self.remove_panelize_preset(),
            AppCommand::EnterXMap => {
                self.xmap_pending = true;
                self.set_status("Extended keymap mode");
            }
            AppCommand::SwitchPanel => {
                self.toggle_active_panel();
                self.set_status(format!(
                    "Active panel: {}",
                    match self.active_panel {
                        ActivePanel::Left => "left",
                        ActivePanel::Right => "right",
                    }
                ));
            }
            AppCommand::MoveUp => self.move_cursor(-1),
            AppCommand::MoveDown => self.move_cursor(1),
            AppCommand::PageUp => {
                let page_step = self.settings.advanced.page_step;
                self.active_panel_mut().move_cursor_page(-1, page_step);
            }
            AppCommand::PageDown => {
                let page_step = self.settings.advanced.page_step;
                self.active_panel_mut().move_cursor_page(1, page_step);
            }
            AppCommand::MoveHome => self.active_panel_mut().move_cursor_home(),
            AppCommand::MoveEnd => self.active_panel_mut().move_cursor_end(),
            AppCommand::ToggleTag => {
                let selected = self.active_panel().selected_entry();
                if selected.is_none() {
                    self.set_status("No entry selected");
                } else if selected.is_some_and(|entry| entry.is_parent) {
                    self.set_status("Parent entry cannot be tagged");
                } else {
                    let added = self.active_panel_mut().toggle_tag_on_cursor();
                    self.active_panel_mut().move_cursor(1);
                    let count = self.active_panel().tagged_count();
                    self.set_status(if added {
                        format!("Tagged entry ({count} total)")
                    } else {
                        format!("Untagged entry ({count} total)")
                    });
                }
            }
            AppCommand::InvertTags => {
                self.active_panel_mut().invert_tags();
                let count = self.active_panel().tagged_count();
                self.set_status(format!("Inverted tags ({count} selected)"));
            }
            AppCommand::SortNext => {
                self.active_panel_mut().cycle_sort_field();
                self.refresh_active_panel();
                let label = self.active_panel().sort_label();
                self.set_status(format!("Sort: {label}"));
            }
            AppCommand::SortReverse => {
                self.active_panel_mut().toggle_sort_direction();
                self.refresh_active_panel();
                let label = self.active_panel().sort_label();
                self.set_status(format!("Sort: {label}"));
            }
            AppCommand::Copy => self.start_copy_dialog(),
            AppCommand::Move => self.start_move_dialog(),
            AppCommand::Delete => {
                if self.settings.confirmation.confirm_delete {
                    self.start_delete_confirmation();
                } else {
                    let targets = self.selected_operation_paths();
                    if targets.is_empty() {
                        self.set_status("Delete requires a selected or tagged entry");
                    } else {
                        self.queue_delete_job(targets);
                    }
                }
            }
            AppCommand::CancelJob => self.cancel_latest_job(),
            AppCommand::OpenJobsScreen => self.open_jobs_screen(),
            AppCommand::CloseJobsScreen => self.close_jobs_screen(),
            AppCommand::JobsMoveUp => self.move_jobs_cursor(-1),
            AppCommand::JobsMoveDown => self.move_jobs_cursor(1),
            AppCommand::OpenEntry => {
                if self.open_selected_directory() {
                    self.queue_panel_refresh(self.active_panel);
                    self.set_status("Loading selected directory...");
                } else if self.open_selected_file_in_viewer() {
                    self.set_status("Opening viewer...");
                } else {
                    self.set_status("No entry selected");
                }
            }
            AppCommand::EditEntry => match self.open_selected_file_in_editor() {
                EditSelectionResult::OpenedExternal => {
                    self.set_status("Opening external editor...")
                }
                EditSelectionResult::OpenedInternal => {
                    self.set_status("Opening internal editor...")
                }
                EditSelectionResult::NoEntrySelected => self.set_status("No entry selected"),
                EditSelectionResult::SelectedEntryIsDirectory => {
                    self.set_status("Directory cannot be edited");
                }
            },
            AppCommand::CdUp => {
                if self.exit_panelize_mode() {
                    self.queue_panel_refresh(self.active_panel);
                    self.set_status("Leaving panelize mode...");
                } else if self.go_parent_directory() {
                    self.queue_panel_refresh(self.active_panel);
                    self.set_status("Loading parent directory...");
                } else {
                    self.set_status("Already at filesystem root");
                }
            }
            AppCommand::Reread => {
                self.refresh_active_panel();
                self.set_status("Refreshing active panel...");
            }
            AppCommand::FindResultsMoveUp => self.move_find_results_cursor(-1),
            AppCommand::FindResultsMoveDown => self.move_find_results_cursor(1),
            AppCommand::FindResultsPageUp => self.move_find_results_page(-1),
            AppCommand::FindResultsPageDown => self.move_find_results_page(1),
            AppCommand::FindResultsHome => self.move_find_results_home(),
            AppCommand::FindResultsEnd => self.move_find_results_end(),
            AppCommand::FindResultsOpenEntry => {
                self.open_selected_find_result()?;
            }
            AppCommand::FindResultsPanelize => self.panelize_find_results(),
            AppCommand::TreeMoveUp => self.move_tree_cursor(-1),
            AppCommand::TreeMoveDown => self.move_tree_cursor(1),
            AppCommand::TreePageUp => self.move_tree_page(-1),
            AppCommand::TreePageDown => self.move_tree_page(1),
            AppCommand::TreeHome => self.move_tree_home(),
            AppCommand::TreeEnd => self.move_tree_end(),
            AppCommand::TreeOpenEntry => {
                self.open_selected_tree_entry()?;
            }
            AppCommand::HotlistMoveUp => self.move_hotlist_cursor(-1),
            AppCommand::HotlistMoveDown => self.move_hotlist_cursor(1),
            AppCommand::HotlistPageUp => self.move_hotlist_page(-1),
            AppCommand::HotlistPageDown => self.move_hotlist_page(1),
            AppCommand::HotlistHome => self.move_hotlist_home(),
            AppCommand::HotlistEnd => self.move_hotlist_end(),
            AppCommand::HotlistOpenEntry => {
                self.open_selected_hotlist_entry()?;
            }
            AppCommand::HotlistAddCurrentDirectory => self.add_current_directory_to_hotlist(),
            AppCommand::HotlistRemoveSelected => self.remove_selected_hotlist_entry(),
            AppCommand::OpenConfirmDialog => self.start_rename_dialog(),
            AppCommand::OpenInputDialog => self.start_mkdir_dialog(),
            AppCommand::OpenListboxDialog => self.start_overwrite_policy_dialog(),
            AppCommand::OpenSkinDialog => self.start_skin_dialog(),
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
            AppCommand::MenuMoveUp => {
                if let Some(menu) = self.menu_state_mut() {
                    menu.move_up();
                }
            }
            AppCommand::MenuMoveDown => {
                if let Some(menu) = self.menu_state_mut() {
                    menu.move_down();
                }
            }
            AppCommand::MenuMoveLeft => {
                if let Some(menu) = self.menu_state_mut() {
                    menu.move_left();
                }
            }
            AppCommand::MenuMoveRight => {
                if let Some(menu) = self.menu_state_mut() {
                    menu.move_right();
                }
            }
            AppCommand::MenuHome => {
                if let Some(menu) = self.menu_state_mut() {
                    menu.move_home();
                }
            }
            AppCommand::MenuEnd => {
                if let Some(menu) = self.menu_state_mut() {
                    menu.move_end();
                }
            }
            AppCommand::MenuAccept => {
                follow_up_command = self.accept_menu_selection();
            }
            AppCommand::MenuSelectAt(index) => {
                follow_up_command = self.accept_menu_selection_at(index);
            }
            AppCommand::HelpMoveUp => {
                if let Some(help) = self.help_state_mut() {
                    help.move_lines(-1);
                }
            }
            AppCommand::HelpMoveDown => {
                if let Some(help) = self.help_state_mut() {
                    help.move_lines(1);
                }
            }
            AppCommand::HelpPageUp => {
                if let Some(help) = self.help_state_mut() {
                    help.move_pages(-1);
                }
            }
            AppCommand::HelpPageDown => {
                if let Some(help) = self.help_state_mut() {
                    help.move_pages(1);
                }
            }
            AppCommand::HelpHalfPageUp => {
                if let Some(help) = self.help_state_mut() {
                    help.move_half_pages(-1);
                }
            }
            AppCommand::HelpHalfPageDown => {
                if let Some(help) = self.help_state_mut() {
                    help.move_half_pages(1);
                }
            }
            AppCommand::HelpHome => {
                if let Some(help) = self.help_state_mut() {
                    help.move_home();
                }
            }
            AppCommand::HelpEnd => {
                if let Some(help) = self.help_state_mut() {
                    help.move_end();
                }
            }
            AppCommand::HelpFollowLink => {
                if let Some(help) = self.help_state_mut()
                    && !help.follow_selected_link()
                {
                    self.set_status("No help link selected");
                }
            }
            AppCommand::HelpBack => {
                if let Some(help) = self.help_state_mut()
                    && !help.back()
                {
                    self.set_status("Help history is empty");
                }
            }
            AppCommand::HelpIndex => {
                if let Some(help) = self.help_state_mut() {
                    help.open_index();
                }
            }
            AppCommand::HelpLinkNext => {
                if let Some(help) = self.help_state_mut() {
                    help.select_next_link();
                }
            }
            AppCommand::HelpLinkPrev => {
                if let Some(help) = self.help_state_mut() {
                    help.select_prev_link();
                }
            }
            AppCommand::HelpNodeNext => {
                if let Some(help) = self.help_state_mut() {
                    help.open_next_node();
                }
            }
            AppCommand::HelpNodePrev => {
                if let Some(help) = self.help_state_mut() {
                    help.open_prev_node();
                }
            }
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
            AppCommand::ViewerMoveUp => {
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.move_lines(-1);
                }
            }
            AppCommand::ViewerMoveDown => {
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.move_lines(1);
                }
            }
            AppCommand::ViewerPageUp => {
                let viewer_page_step = self.settings.advanced.viewer_page_step;
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.move_pages(-1, viewer_page_step);
                }
            }
            AppCommand::ViewerPageDown => {
                let viewer_page_step = self.settings.advanced.viewer_page_step;
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.move_pages(1, viewer_page_step);
                }
            }
            AppCommand::ViewerHome => {
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.move_home();
                }
            }
            AppCommand::ViewerEnd => {
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.move_end();
                }
            }
            AppCommand::ViewerSearchForward => {
                self.open_viewer_search_dialog(ViewerSearchDirection::Forward);
            }
            AppCommand::ViewerSearchBackward => {
                self.open_viewer_search_dialog(ViewerSearchDirection::Backward);
            }
            AppCommand::ViewerSearchContinue => {
                self.continue_viewer_search(None);
            }
            AppCommand::ViewerSearchContinueBackward => {
                self.continue_viewer_search(Some(ViewerSearchDirection::Backward));
            }
            AppCommand::ViewerGoto => {
                self.open_viewer_goto_dialog();
            }
            AppCommand::ViewerToggleWrap => {
                let mut next = None;
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.toggle_wrap();
                    next = Some(viewer.wrap);
                }
                if let Some(wrap) = next {
                    self.set_status(format!(
                        "Viewer wrap {}",
                        if wrap { "enabled" } else { "disabled" }
                    ));
                }
            }
            AppCommand::ViewerToggleHex => {
                let mut next = None;
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.toggle_hex_mode();
                    next = Some(viewer.hex_mode);
                }
                if let Some(hex_mode) = next {
                    self.set_status(format!(
                        "Viewer mode {}",
                        if hex_mode { "hex" } else { "text" }
                    ));
                }
            }
        }

        if clear_xmap_after_command {
            self.xmap_pending = false;
        }

        if self.pending_quit {
            self.pending_quit = false;
            return Ok(ApplyResult::Quit);
        }

        if let Some(next_command) = follow_up_command {
            return self.apply(next_command);
        }

        Ok(ApplyResult::Continue)
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

    fn start_copy_dialog(&mut self) {
        self.start_transfer_dialog(TransferKind::Copy);
    }

    fn start_move_dialog(&mut self) {
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
        let title = match kind {
            TransferKind::Copy => "Copy",
            TransferKind::Move => "Move",
        };
        self.pending_dialog_action = Some(PendingDialogAction::TransferDestination {
            kind,
            sources,
            source_base_dir,
        });
        self.routes.push(Route::Dialog(DialogState::input(
            title,
            "Destination directory:",
            destination_dir.to_string_lossy(),
        )));
        self.set_status(format!("{title}: choose destination"));
    }

    fn start_delete_confirmation(&mut self) {
        let targets = self.selected_operation_paths();
        if targets.is_empty() {
            self.set_status("Delete requires a selected or tagged entry");
            return;
        }

        let message = if targets.len() == 1 {
            let name = targets[0]
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| targets[0].to_string_lossy().into_owned());
            format!("Delete '{name}'?")
        } else {
            format!("Delete {} selected items?", targets.len())
        };
        self.pending_dialog_action = Some(PendingDialogAction::ConfirmDelete { targets });
        self.routes
            .push(Route::Dialog(DialogState::confirm("Delete", message)));
        self.set_status("Confirm delete");
    }

    fn start_quit_confirmation(&mut self) {
        self.pending_dialog_action = Some(PendingDialogAction::ConfirmQuit);
        self.routes
            .push(Route::Dialog(DialogState::confirm("Quit", "Exit rc?")));
        self.set_status("Confirm quit");
    }

    fn start_rename_dialog(&mut self) {
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
        self.pending_dialog_action = Some(PendingDialogAction::RenameEntry { source });
        self.routes.push(Route::Dialog(DialogState::input(
            "Rename/Move",
            "New name:",
            current_name,
        )));
        self.set_status("Rename/Move: enter new name");
    }

    fn start_mkdir_dialog(&mut self) {
        let base_dir = self.active_panel().cwd.clone();
        self.pending_dialog_action = Some(PendingDialogAction::Mkdir { base_dir });
        self.routes.push(Route::Dialog(DialogState::input(
            "Mkdir",
            "Directory name:",
            "",
        )));
        self.set_status("Mkdir: enter directory name");
    }

    fn start_overwrite_policy_dialog(&mut self) {
        let selected = overwrite_policy_index(self.overwrite_policy);
        self.pending_dialog_action = Some(PendingDialogAction::SetDefaultOverwritePolicy);
        self.routes.push(Route::Dialog(DialogState::listbox(
            "Overwrite Policy",
            overwrite_policy_items(),
            selected,
        )));
        self.set_status("Choose default overwrite policy");
    }

    fn start_skin_dialog(&mut self) {
        if self.available_skins.is_empty() {
            self.set_status("No skins available");
            return;
        }

        let selected = self
            .available_skins
            .iter()
            .position(|name| name.eq_ignore_ascii_case(&self.active_skin_name))
            .unwrap_or(0);
        self.pending_dialog_action = Some(PendingDialogAction::SetSkin {
            original_skin: self.active_skin_name.clone(),
        });
        self.routes.push(Route::Dialog(DialogState::listbox(
            "Skin",
            self.available_skins.clone(),
            selected,
        )));
        self.set_status("Choose skin");
    }

    fn queue_copy_or_move_job(
        &mut self,
        kind: TransferKind,
        sources: Vec<PathBuf>,
        destination_dir: PathBuf,
        overwrite: OverwritePolicy,
    ) {
        let request = match kind {
            TransferKind::Copy => JobRequest::Copy {
                sources,
                destination_dir,
                overwrite,
            },
            TransferKind::Move => JobRequest::Move {
                sources,
                destination_dir,
                overwrite,
            },
        };
        self.queue_worker_job_request(request);
    }

    pub fn enqueue_worker_job_request(&mut self, request: JobRequest) -> JobId {
        self.queue_worker_job_request(request)
    }

    fn queue_worker_job_request(&mut self, request: JobRequest) -> JobId {
        let summary = request.summary();
        let worker_job = self.jobs.enqueue(request);
        let job_id = worker_job.id;
        self.pending_worker_commands
            .push(WorkerCommand::Run(Box::new(worker_job)));
        self.set_status(format!("Queued job #{job_id}: {summary}"));
        job_id
    }

    fn cancel_latest_job(&mut self) {
        let selected_id = if matches!(self.top_route(), Route::Jobs) {
            self.selected_job_record().map(|job| job.id)
        } else {
            None
        };
        let Some(job_id) = selected_id.or_else(|| self.jobs.newest_cancelable_job_id()) else {
            self.set_status("No active job to cancel");
            return;
        };

        if self.request_cancel_for_job(job_id) {
            self.set_status(format!("Cancellation requested for job #{job_id}"));
        } else {
            self.set_status(format!("Job #{job_id} cannot be canceled"));
        }
    }

    fn request_cancel_for_job(&mut self, job_id: JobId) -> bool {
        if !self.jobs.request_cancel(job_id) {
            return false;
        }
        let is_worker_job = self
            .jobs
            .job(job_id)
            .is_some_and(|job| !matches!(job.kind, JobKind::Find));
        if is_worker_job {
            self.pending_worker_commands
                .push(WorkerCommand::Cancel(job_id));
        }
        true
    }

    fn request_cancel_for_all_jobs(&mut self) {
        let cancelable_job_ids: Vec<JobId> = self
            .jobs
            .jobs()
            .iter()
            .filter(|job| matches!(job.status, JobStatus::Queued | JobStatus::Running))
            .map(|job| job.id)
            .collect();
        for job_id in cancelable_job_ids {
            let _ = self.request_cancel_for_job(job_id);
        }
    }

    fn queue_delete_job(&mut self, targets: Vec<PathBuf>) {
        self.queue_worker_job_request(JobRequest::Delete { targets });
    }

    fn finish_dialog(&mut self, result: DialogResult) {
        let pending = self.pending_dialog_action.take();
        match (pending, result) {
            (None, result) => self.set_status(result.status_line()),
            (
                Some(PendingDialogAction::ConfirmDelete { targets }),
                DialogResult::ConfirmAccepted,
            ) => {
                self.queue_delete_job(targets);
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
                Some(PendingDialogAction::Mkdir { base_dir }),
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
                self.queue_worker_job_request(JobRequest::Mkdir { path: destination });
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
                    let selected = overwrite_policy_index(self.overwrite_policy);
                    self.pending_dialog_action = Some(PendingDialogAction::TransferOverwrite {
                        kind,
                        sources,
                        destination_dir,
                    });
                    self.routes.push(Route::Dialog(DialogState::listbox(
                        "Overwrite Policy",
                        overwrite_policy_items(),
                        selected,
                    )));
                    self.set_status("Choose overwrite policy");
                } else {
                    self.queue_copy_or_move_job(
                        kind,
                        sources,
                        destination_dir,
                        self.overwrite_policy,
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
                }),
                DialogResult::ListboxSubmitted { index, .. },
            ) => {
                let overwrite = index
                    .map(overwrite_policy_from_index)
                    .unwrap_or(self.overwrite_policy);
                self.queue_copy_or_move_job(kind, sources, destination_dir, overwrite);
            }
            (Some(PendingDialogAction::TransferOverwrite { .. }), DialogResult::Canceled) => {
                self.set_status("Copy/Move canceled");
            }
            (
                Some(PendingDialogAction::SetDefaultOverwritePolicy),
                DialogResult::ListboxSubmitted { index, .. },
            ) => {
                if let Some(index) = index {
                    self.overwrite_policy = overwrite_policy_from_index(index);
                    self.settings.configuration.default_overwrite_policy = self.overwrite_policy;
                    self.settings.mark_dirty();
                    self.set_status(format!(
                        "Default overwrite policy: {}",
                        self.overwrite_policy.label()
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
                Some(PendingDialogAction::FindQuery { base_dir }),
                DialogResult::InputSubmitted(value),
            ) => {
                let query = value.trim();
                if query.is_empty() {
                    self.set_status("Find canceled: empty query");
                    return;
                }

                let query = query.to_string();
                let request = JobRequest::Find {
                    query: query.clone(),
                    base_dir: base_dir.clone(),
                };
                let summary = request.summary();
                let worker_job = self.jobs.enqueue(request);
                let job_id = worker_job.id;
                let pause_flag = Arc::new(AtomicBool::new(false));
                self.find_pause_flags.insert(job_id, pause_flag.clone());
                self.routes
                    .push(Route::FindResults(FindResultsState::loading(
                        job_id,
                        query.clone(),
                        base_dir.clone(),
                    )));
                self.pending_background_commands
                    .push(BackgroundCommand::FindEntries {
                        job_id,
                        query: query.clone(),
                        base_dir,
                        max_results: self.settings.advanced.max_find_results,
                        cancel_flag: worker_job.cancel_flag(),
                        pause_flag,
                    });
                self.set_status(format!("Queued job #{job_id}: {summary}"));
            }
            (Some(PendingDialogAction::FindQuery { .. }), DialogResult::Canceled) => {
                self.set_status("Find canceled");
            }
            (
                Some(PendingDialogAction::PanelizePresetSelection {
                    initial_command,
                    preset_commands,
                }),
                DialogResult::ListboxSubmitted { index, .. },
            ) => {
                let Some(index) = index else {
                    self.set_status("Panelize canceled");
                    return;
                };
                if index == 0 {
                    self.open_panelize_command_input_dialog(initial_command, preset_commands);
                    self.set_status("External panelize: enter command");
                    return;
                }
                let Some(command) = preset_commands.get(index.saturating_sub(1)).cloned() else {
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
                    mut preset_commands,
                }),
                DialogResult::InputSubmitted(value),
            ) => {
                let command = value.trim();
                if command.is_empty() {
                    self.pending_dialog_action =
                        Some(PendingDialogAction::PanelizePresetSelection {
                            initial_command,
                            preset_commands,
                        });
                    self.set_status("Panelize preset add canceled: empty command");
                    return;
                }
                let command = command.to_string();
                if preset_commands.iter().any(|preset| preset == &command) {
                    self.pending_dialog_action =
                        Some(PendingDialogAction::PanelizePresetSelection {
                            initial_command,
                            preset_commands,
                        });
                    self.set_status("Panelize preset already exists");
                    return;
                }

                preset_commands.push(command.clone());
                self.panelize_presets = preset_commands.clone();
                self.settings.configuration.panelize_presets = self.panelize_presets.clone();
                self.settings.mark_dirty();
                self.routes.pop();
                self.open_panelize_preset_selection_dialog(command.clone(), preset_commands);
                self.set_status(format!("Added panelize preset: {command}"));
            }
            (
                Some(PendingDialogAction::PanelizePresetAdd {
                    initial_command,
                    preset_commands,
                }),
                DialogResult::Canceled,
            ) => {
                self.pending_dialog_action = Some(PendingDialogAction::PanelizePresetSelection {
                    initial_command,
                    preset_commands,
                });
                self.set_status("Panelize preset add canceled");
            }
            (
                Some(PendingDialogAction::PanelizePresetEdit {
                    initial_command,
                    mut preset_commands,
                    preset_index,
                }),
                DialogResult::InputSubmitted(value),
            ) => {
                let command = value.trim();
                if command.is_empty() {
                    self.pending_dialog_action =
                        Some(PendingDialogAction::PanelizePresetSelection {
                            initial_command,
                            preset_commands,
                        });
                    self.set_status("Panelize preset edit canceled: empty command");
                    return;
                }
                let command = command.to_string();
                let Some(entry) = preset_commands.get_mut(preset_index) else {
                    self.pending_dialog_action =
                        Some(PendingDialogAction::PanelizePresetSelection {
                            initial_command,
                            preset_commands,
                        });
                    self.set_status("Panelize preset edit failed: invalid selection");
                    return;
                };
                *entry = command.clone();

                self.panelize_presets = preset_commands.clone();
                self.settings.configuration.panelize_presets = self.panelize_presets.clone();
                self.settings.mark_dirty();
                self.routes.pop();
                self.open_panelize_preset_selection_dialog(command.clone(), preset_commands);
                self.set_status(format!("Updated panelize preset: {command}"));
            }
            (
                Some(PendingDialogAction::PanelizePresetEdit {
                    initial_command,
                    preset_commands,
                    ..
                }),
                DialogResult::Canceled,
            ) => {
                self.pending_dialog_action = Some(PendingDialogAction::PanelizePresetSelection {
                    initial_command,
                    preset_commands,
                });
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

    fn handle_dialog_event(&mut self, event: DialogEvent) {
        let preview_skin = matches!(
            self.pending_dialog_action,
            Some(PendingDialogAction::SetSkin { .. })
        ) && matches!(event, DialogEvent::MoveUp | DialogEvent::MoveDown);
        let Some(Route::Dialog(dialog)) = self.routes.last_mut() else {
            return;
        };
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
                self.routes.pop();
                self.last_dialog_result = Some(result.clone());
                self.finish_dialog(result);
            }
        }
    }
}

fn compute_line_offsets(content: &str) -> Vec<usize> {
    let mut offsets = vec![0];
    for (index, byte) in content.bytes().enumerate() {
        if byte == b'\n' && index.saturating_add(1) < content.len() {
            offsets.push(index + 1);
        }
    }
    offsets
}

fn should_default_to_hex_mode(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }

    let sample = &bytes[..bytes.len().min(4096)];
    if sample.contains(&0) {
        return true;
    }

    let suspicious = sample
        .iter()
        .filter(|byte| {
            let byte = **byte;
            !(byte.is_ascii_graphic() || matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
        })
        .count();
    suspicious.saturating_mul(100) / sample.len() > 30
}

fn find_forward_wrap(content: &str, query: &str, start: usize) -> Option<usize> {
    let start = start.min(content.len());
    if let Some(relative) = content[start..].find(query) {
        return Some(start + relative);
    }
    if start == 0 {
        return None;
    }
    content[..start].find(query)
}

fn find_backward_wrap(content: &str, query: &str, start: usize) -> Option<usize> {
    let start = start.min(content.len());
    if let Some(index) = content[..start].rfind(query) {
        return Some(index);
    }
    if start >= content.len() {
        return None;
    }
    content[start..]
        .rfind(query)
        .map(|relative| start + relative)
}

fn build_tree_entries(root: &Path, max_depth: usize, max_entries: usize) -> Vec<TreeEntry> {
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

fn parse_viewer_goto_target(input: &str) -> Result<ViewerGotoTarget, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(String::from("target is empty"));
    }

    if let Some(rest) = trimmed.strip_prefix('@') {
        let value = rest
            .trim()
            .parse::<usize>()
            .map_err(|_| String::from("invalid decimal offset"))?;
        return Ok(ViewerGotoTarget::Offset(value));
    }

    if let Some(rest) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        let value = usize::from_str_radix(rest.trim(), 16)
            .map_err(|_| String::from("invalid hex offset"))?;
        return Ok(ViewerGotoTarget::Offset(value));
    }

    let lowered = trimmed.to_ascii_lowercase();
    if let Some(rest) = lowered.strip_prefix("line:") {
        let value = rest
            .trim()
            .parse::<usize>()
            .map_err(|_| String::from("invalid line number"))?;
        return Ok(ViewerGotoTarget::Line(value));
    }
    if let Some(rest) = lowered.strip_prefix("offset:") {
        let value = rest
            .trim()
            .parse::<usize>()
            .map_err(|_| String::from("invalid decimal offset"))?;
        return Ok(ViewerGotoTarget::Offset(value));
    }

    let value = trimmed
        .parse::<usize>()
        .map_err(|_| String::from("invalid line number"))?;
    Ok(ViewerGotoTarget::Line(value))
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

fn panelize_preset_selected_index(initial_command: &str, preset_commands: &[String]) -> usize {
    preset_commands
        .iter()
        .position(|command| command == initial_command)
        .map_or(0, |index| index.saturating_add(1))
}

#[cfg(test)]
fn read_entries(dir: &Path, sort_mode: SortMode) -> io::Result<Vec<FileEntry>> {
    read_entries_with_visibility_cancel(dir, sort_mode, true, None)
}

fn read_entries_with_visibility(
    dir: &Path,
    sort_mode: SortMode,
    show_hidden_files: bool,
) -> io::Result<Vec<FileEntry>> {
    read_entries_with_visibility_cancel(dir, sort_mode, show_hidden_files, None)
}

fn read_entries_with_visibility_cancel(
    dir: &Path,
    sort_mode: SortMode,
    show_hidden_files: bool,
    cancel_flag: Option<&AtomicBool>,
) -> io::Result<Vec<FileEntry>> {
    ensure_panel_refresh_not_canceled(cancel_flag)?;
    let mut entries = Vec::new();
    for entry_result in fs::read_dir(dir)? {
        ensure_panel_refresh_not_canceled(cancel_flag)?;
        let entry = entry_result?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !show_hidden_files && name.starts_with('.') {
            continue;
        }
        let file_type = entry.file_type()?;
        let metadata = entry.metadata().ok();
        let size = metadata.as_ref().map_or(0, std::fs::Metadata::len);
        let modified = metadata.as_ref().and_then(|meta| meta.modified().ok());
        if file_type.is_dir() {
            entries.push(FileEntry::directory(name, path, size, modified));
        } else {
            entries.push(FileEntry::file(name, path, size, modified));
        }
    }

    sort_file_entries(&mut entries, sort_mode);

    if let Some(parent) = dir.parent() {
        entries.insert(0, FileEntry::parent(parent.to_path_buf()));
    }
    Ok(entries)
}

fn read_panelized_entries(
    base_dir: &Path,
    command: &str,
    sort_mode: SortMode,
) -> io::Result<Vec<FileEntry>> {
    read_panelized_entries_with_cancel(base_dir, command, sort_mode, None)
}

fn read_panelized_entries_with_cancel(
    base_dir: &Path,
    command: &str,
    sort_mode: SortMode,
    cancel_flag: Option<&AtomicBool>,
) -> io::Result<Vec<FileEntry>> {
    ensure_panel_refresh_not_canceled(cancel_flag)?;
    let output = run_shell_command(base_dir, command, cancel_flag)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        let detail = if stderr.is_empty() {
            format!("exit {}", output.status)
        } else {
            stderr.to_string()
        };
        return Err(io::Error::other(format!("command failed: {detail}")));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut seen = HashSet::new();
    let mut entries = Vec::new();

    for raw_line in stdout.lines() {
        ensure_panel_refresh_not_canceled(cancel_flag)?;
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.is_empty() {
            continue;
        }

        append_panelized_path_entry(
            base_dir,
            PathBuf::from(line),
            &mut seen,
            &mut entries,
            cancel_flag,
        )?;
    }

    sort_file_entries(&mut entries, sort_mode);
    Ok(entries)
}

fn read_panelized_paths(
    base_dir: &Path,
    paths: &[PathBuf],
    sort_mode: SortMode,
    cancel_flag: Option<&AtomicBool>,
) -> io::Result<Vec<FileEntry>> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for path in paths {
        append_panelized_path_entry(base_dir, path.clone(), &mut seen, &mut entries, cancel_flag)?;
    }
    sort_file_entries(&mut entries, sort_mode);
    Ok(entries)
}

fn append_panelized_path_entry(
    base_dir: &Path,
    input_path: PathBuf,
    seen: &mut HashSet<PathBuf>,
    entries: &mut Vec<FileEntry>,
    cancel_flag: Option<&AtomicBool>,
) -> io::Result<()> {
    ensure_panel_refresh_not_canceled(cancel_flag)?;
    let path = if input_path.is_absolute() {
        input_path
    } else {
        base_dir.join(input_path)
    };
    if !seen.insert(path.clone()) {
        return Ok(());
    }

    let metadata = fs::metadata(&path).ok();
    let size = metadata.as_ref().map_or(0, std::fs::Metadata::len);
    let modified = metadata.as_ref().and_then(|meta| meta.modified().ok());
    let name = panelized_entry_label(base_dir, &path);
    let is_dir = metadata.as_ref().is_some_and(std::fs::Metadata::is_dir);
    if is_dir {
        entries.push(FileEntry::directory(name, path, size, modified));
    } else {
        entries.push(FileEntry::file(name, path, size, modified));
    }
    Ok(())
}

fn ensure_panel_refresh_not_canceled(cancel_flag: Option<&AtomicBool>) -> io::Result<()> {
    if cancel_flag.is_some_and(|flag| flag.load(AtomicOrdering::Relaxed)) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            PANEL_REFRESH_CANCELED_MESSAGE,
        ));
    }
    Ok(())
}

fn sort_file_entries(entries: &mut [FileEntry], sort_mode: SortMode) {
    let type_rank = |entry: &FileEntry| if entry.is_dir { 0_u8 } else { 1_u8 };

    match (sort_mode.field, sort_mode.reverse) {
        (SortField::Name, false) => {
            entries.sort_by_cached_key(|entry| (type_rank(entry), entry.name.to_lowercase()));
        }
        (SortField::Name, true) => {
            entries
                .sort_by_cached_key(|entry| (type_rank(entry), Reverse(entry.name.to_lowercase())));
        }
        (SortField::Size, false) => {
            entries.sort_by_cached_key(|entry| {
                (type_rank(entry), (entry.size, entry.name.to_lowercase()))
            });
        }
        (SortField::Size, true) => {
            entries.sort_by_cached_key(|entry| {
                (
                    type_rank(entry),
                    Reverse((entry.size, entry.name.to_lowercase())),
                )
            });
        }
        (SortField::Modified, false) => {
            entries.sort_by_cached_key(|entry| {
                (
                    type_rank(entry),
                    (entry.modified, entry.name.to_lowercase()),
                )
            });
        }
        (SortField::Modified, true) => {
            entries.sort_by_cached_key(|entry| {
                (
                    type_rank(entry),
                    Reverse((entry.modified, entry.name.to_lowercase())),
                )
            });
        }
    }
}

fn panelized_entry_label(base_dir: &Path, path: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(base_dir) {
        let relative = relative.to_string_lossy();
        if relative.is_empty() {
            String::from(".")
        } else {
            relative.into_owned()
        }
    } else {
        path.to_string_lossy().into_owned()
    }
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

#[cfg(unix)]
fn spawn_shell_command(cwd: &Path, command: &str) -> io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;

    Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
}

#[cfg(windows)]
fn spawn_shell_command(cwd: &Path, command: &str) -> io::Result<std::process::Child> {
    Command::new("cmd")
        .arg("/C")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}

#[cfg(unix)]
fn terminate_shell_command(child: &mut std::process::Child) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    let Ok(pid) = i32::try_from(child.id()) else {
        let _ = child.kill();
        return;
    };

    let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
}

#[cfg(windows)]
fn terminate_shell_command(child: &mut std::process::Child) {
    let pid = child.id().to_string();
    let status = Command::new("taskkill")
        .arg("/PID")
        .arg(&pid)
        .arg("/T")
        .arg("/F")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    if !matches!(status, Ok(exit_status) if exit_status.success()) {
        let _ = child.kill();
    }
}

fn run_shell_command(
    cwd: &Path,
    command: &str,
    cancel_flag: Option<&AtomicBool>,
) -> io::Result<std::process::Output> {
    let mut child = spawn_shell_command(cwd, command)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("failed to capture command stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("failed to capture command stderr"))?;

    let stdout_handle = thread::spawn(move || {
        let mut reader = stdout;
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer)?;
        Ok(buffer)
    });
    let stderr_handle = thread::spawn(move || {
        let mut reader = stderr;
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer)?;
        Ok(buffer)
    });

    loop {
        if cancel_flag.is_some_and(|flag| flag.load(AtomicOrdering::Relaxed)) {
            terminate_shell_command(&mut child);
            let _ = child.wait();
            let _ = stdout_handle.join();
            let _ = stderr_handle.join();
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                PANEL_REFRESH_CANCELED_MESSAGE,
            ));
        }

        if let Some(status) = child.try_wait()? {
            let stdout = join_command_output_reader(stdout_handle, "stdout")?;
            let stderr = join_command_output_reader(stderr_handle, "stderr")?;
            return Ok(std::process::Output {
                status,
                stdout,
                stderr,
            });
        }

        thread::sleep(Duration::from_millis(20));
    }
}

fn join_command_output_reader(
    handle: thread::JoinHandle<io::Result<Vec<u8>>>,
    stream: &str,
) -> io::Result<Vec<u8>> {
    handle
        .join()
        .map_err(|_| io::Error::other(format!("command {stream} reader thread panicked")))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::KeyModifiers;
    use std::path::Path;
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

    fn drain_background(app: &mut AppState) {
        loop {
            let commands = app.take_pending_background_commands();
            if commands.is_empty() {
                break;
            }
            for command in commands {
                let (event_tx, event_rx) = std::sync::mpsc::channel();
                match execute_background_command(command, &event_tx) {
                    BackgroundExecution::Continue => {}
                    #[cfg(not(test))]
                    BackgroundExecution::SpawnFind(task) => {
                        let _ = task.handle.join();
                    }
                    BackgroundExecution::Stop => return,
                }
                for event in event_rx.try_iter() {
                    app.handle_background_event(event);
                }
            }
        }
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
        assert_eq!(viewer.path, file_path);
        assert_eq!(viewer.line_count(), 3);

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
        assert!(
            app.take_pending_background_commands().is_empty(),
            "external edit should not queue viewer load"
        );

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

        let pending_background = app.take_pending_background_commands();
        assert_eq!(pending_background.len(), 1, "viewer load should be queued");
        match &pending_background[0] {
            BackgroundCommand::LoadViewer { path } => {
                assert_eq!(path, &file_path);
            }
            other => panic!("expected viewer load command, got {other:?}"),
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
    fn find_cancel_uses_job_flag_without_worker_cancel_command() {
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
        assert!(
            app.take_pending_worker_commands().is_empty(),
            "find should not queue worker commands"
        );

        app.apply(AppCommand::CancelJob)
            .expect("cancel job should succeed");
        assert!(
            app.take_pending_worker_commands().is_empty(),
            "canceling find should not send worker cancel command"
        );

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
    fn reread_cancels_previous_refresh_for_same_panel() {
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
        assert_eq!(app.pending_background_commands.len(), 1);

        let (first_request_id, first_cancel_flag) = match &app.pending_background_commands[0] {
            BackgroundCommand::RefreshPanel {
                request_id,
                cancel_flag,
                ..
            } => (*request_id, Arc::clone(cancel_flag)),
            _ => panic!("expected panel refresh command"),
        };
        assert!(
            !first_cancel_flag.load(AtomicOrdering::Relaxed),
            "initial refresh should not be canceled"
        );

        app.refresh_active_panel();
        assert!(
            first_cancel_flag.load(AtomicOrdering::Relaxed),
            "second refresh should cancel the previous in-flight request"
        );

        let (second_request_id, second_cancel_flag) = match app
            .pending_background_commands
            .last()
            .expect("second refresh command should be queued")
        {
            BackgroundCommand::RefreshPanel {
                request_id,
                cancel_flag,
                ..
            } => (*request_id, Arc::clone(cancel_flag)),
            _ => panic!("expected panel refresh command"),
        };
        assert!(
            second_request_id > first_request_id,
            "request ids should advance for newer refresh commands"
        );
        assert!(
            !second_cancel_flag.load(AtomicOrdering::Relaxed),
            "newest refresh should remain active"
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
        let commands = app.take_pending_background_commands();
        assert_eq!(commands.len(), 2);

        let first = commands[0].clone();
        let second = commands[1].clone();
        let (panel, cwd, source, sort_mode, first_request_id) = match first {
            BackgroundCommand::RefreshPanel {
                panel,
                cwd,
                source,
                sort_mode,
                request_id,
                ..
            } => (panel, cwd, source, sort_mode, request_id),
            _ => panic!("expected panel refresh command"),
        };
        let second_request_id = match second {
            BackgroundCommand::RefreshPanel { request_id, .. } => request_id,
            _ => panic!("expected panel refresh command"),
        };

        app.handle_background_event(BackgroundEvent::PanelRefreshed {
            panel,
            cwd: cwd.clone(),
            source: source.clone(),
            sort_mode,
            request_id: first_request_id,
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
            request_id: second_request_id,
            result: Ok(Vec::new()),
        });
        assert!(
            !app.panels[panel.index()].loading,
            "latest refresh result should clear loading state"
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
