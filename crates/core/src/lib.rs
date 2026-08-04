#![forbid(unsafe_code)]

mod background;
mod command_dispatch;
mod command_map;
pub mod dialog;
mod dialog_flow;
mod find_engine;
mod find_flow;
pub mod help;
mod hotlist_flow;
pub mod jobs;
mod keybinding_help;
pub mod keymap;
pub mod layout;
mod navigation_flow;
mod orchestration;
mod panel;
mod panel_filter;
mod panelize_flow;
mod quick_cd;
mod quick_view_flow;
mod refresh_flow;
mod route_flow;
mod selection_size;
mod selection_size_flow;
pub mod settings;
mod settings_flow;
pub mod settings_io;
pub mod slo;
mod state_flow;
mod tree;
mod viewer;
mod viewer_flow;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};
use std::time::{Instant, SystemTime};

pub use background::{
    BackgroundEvent, PanelRefreshResult, PanelRefreshStreamRequest, build_tree_ready_event,
    read_disk_usage, refresh_panel_entries, refresh_panel_event, stream_refresh_panel_entries,
};
pub use dialog::{
    DialogButtonFocus, DialogKind, DialogResult, DialogState, FilterDialogField, FilterDialogState,
    FindDialogField, FindDialogState, PairInputDialogState, PairInputField,
};
pub use find_engine::{
    FindNameMode, FindSearchError, FindSearchIssue, FindSearchIssueKind, FindSearchReport,
    FindSpec, run_find_entries, stream_find_entries,
};
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
    sort_file_entries, stream_panelized_entries_with_cancel, stream_panelized_paths_with_cancel,
};
pub use panel_filter::{MAX_PANEL_FILTER_CHARS, PanelFilter, PanelFilterError};
pub use quick_view_flow::QuickViewState;
pub use rc_shell::{LocalProcessBackend, ProcessBackend, ProcessExit, ProcessOutputLimits};
pub use selection_size::{
    SELECTION_SIZE_CANCELED_MESSAGE, SelectionSizeReport, measure_selection_size,
};
pub use selection_size_flow::SelectionSizeState;
pub use settings::{
    AdvancedSettings, AppearanceSettings, ConfigurationSettings, ConfirmationSettings,
    DEFAULT_PANELIZE_PRESETS, DisplayBitsSettings, HotlistEntry, LayoutSettings, LearnKeysSettings,
    PanelOptionsSettings, PanelizePreset, SaveSetupMetadata, Settings, SettingsCategory,
    VirtualFsSettings,
};
pub use slo::{FOUNDATION_SLO, SloBudgets};
#[cfg(test)]
use std::sync::atomic::Ordering as AtomicOrdering;
pub use tree::{
    TreeBuildResult, TreeEntry, TreeLoadState, TreeNavigationMode, TreeScanIssue, TreeScanSummary,
    TreeState,
};
pub(crate) use tree::{
    TreeMutationTracker, TreeRescanPlan, TreeScanCompletion, build_tree_entries,
};
pub use viewer::ViewerState;

use crate::keymap::{KeyChord, KeyCode, KeyContext, Keymap, KeymapParseReport};
use crate::panel::read_entries_with_visibility;
use crate::panel_filter::apply_panel_filter;
use crate::quick_view_flow::QuickViewWorkflow;
use crate::refresh_flow::{
    PanelEntriesChunk, PanelRefreshCompletion, PanelRefreshPostWorkflow, PanelRefreshWorkflow,
};
use crate::selection_size_flow::SelectionSizeWorkflow;
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
    FindResultsAgain,
    FindResultsTogglePause,
    FindDialogBrowse,
    OpenTree,
    CloseTree,
    OpenHotlist,
    CloseHotlist,
    OpenPanelizeDialog,
    RestorePanelizedResults,
    PanelizePresetAdd,
    PanelizePresetEdit,
    PanelizePresetRemove,
    EnterXMap,
    SwitchPanel,
    SetOtherPanelView(PanelViewMode),
    Panel(ActivePanel, PanelCommand),
    Navigate(NavigationTarget, NavigationMotion),
    ToggleTag,
    InvertTags,
    CycleListingFormat,
    OpenListingFormat,
    OpenSortOrder,
    OpenPanelFilter,
    SortNext,
    SortReverse,
    Copy,
    Move,
    Delete,
    CancelJob,
    OpenJobsScreen,
    CloseJobsScreen,
    OpenEntry,
    EditEntry,
    CdUp,
    OpenQuickCd,
    Reread,
    FindResultsOpenEntry,
    FindResultsPanelize,
    FindResultsSelectAt(usize),
    TreeOpenEntry,
    TreeRescan,
    TreeForget,
    TreeToggleNavigation,
    TreeCopy,
    TreeMove,
    TreeMkdir,
    TreeDelete,
    TreeSearchNext,
    TreeSearchBackspace,
    TreeSearchAppend(char),
    TreeSelectVisibleAt(usize),
    HotlistOpenEntry,
    HotlistAddCurrentDirectory,
    HotlistEditSelected,
    HotlistRemoveSelected,
    HotlistSelectAt(usize),
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
    MenuAccept,
    MenuSelectAt(usize),
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
    DialogListboxSelectAt(usize),
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PanelCommand {
    SetView(PanelViewMode),
    OpenTree,
    OpenListingFormat,
    OpenSortOrder,
    OpenFilter,
    RestorePanelizedResults,
    Reread,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum PanelViewMode {
    #[default]
    Listing,
    QuickView,
    Info,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum PanelListingFormat {
    #[default]
    Full,
    Brief,
    Long,
}

impl PanelListingFormat {
    pub const ALL: [Self; 3] = [Self::Full, Self::Brief, Self::Long];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Full => "Full",
            Self::Brief => "Brief",
            Self::Long => "Long",
        }
    }

    pub const fn title_label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Brief => "brief",
            Self::Long => "long",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Full => Self::Brief,
            Self::Brief => Self::Long,
            Self::Long => Self::Full,
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Full => 0,
            Self::Brief => 1,
            Self::Long => 2,
        }
    }

    fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NavigationTarget {
    FileManager,
    Jobs,
    Menu,
    Help,
    FindResults,
    Tree,
    Hotlist,
    Viewer,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NavigationMotion {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    HalfPageUp,
    HalfPageDown,
    Home,
    End,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommandDomain {
    Route,
    Navigation,
    Viewer,
    Dialog,
    Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommandOutcome {
    Continue,
    FollowUp(AppCommand),
    Quit,
}

impl AppCommand {
    pub(crate) const fn domain(self) -> CommandDomain {
        match self {
            Self::OpenHelp
            | Self::CloseHelp
            | Self::OpenMenu
            | Self::OpenMenuAt(_)
            | Self::CloseMenu
            | Self::Quit
            | Self::CloseViewer
            | Self::OpenFindDialog
            | Self::CloseFindResults
            | Self::OpenTree
            | Self::CloseTree
            | Self::OpenHotlist
            | Self::CloseHotlist
            | Self::OpenPanelizeDialog
            | Self::PanelizePresetAdd
            | Self::PanelizePresetEdit
            | Self::PanelizePresetRemove
            | Self::EnterXMap
            | Self::SwitchPanel
            | Self::Panel(_, PanelCommand::OpenTree)
            | Self::OpenJobsScreen
            | Self::CloseJobsScreen
            | Self::MenuAccept
            | Self::MenuSelectAt(_)
            | Self::HelpFollowLink
            | Self::HelpBack
            | Self::HelpIndex
            | Self::HelpLinkNext
            | Self::HelpLinkPrev
            | Self::HelpNodeNext
            | Self::HelpNodePrev
            | Self::MenuNoop
            | Self::MenuNotImplemented(_) => CommandDomain::Route,
            Self::Navigate(target, _) => match target {
                NavigationTarget::Jobs | NavigationTarget::Menu | NavigationTarget::Help => {
                    CommandDomain::Route
                }
                NavigationTarget::FileManager
                | NavigationTarget::FindResults
                | NavigationTarget::Tree
                | NavigationTarget::Hotlist => CommandDomain::Navigation,
                NavigationTarget::Viewer => CommandDomain::Viewer,
            },
            Self::ToggleTag
            | Self::SetOtherPanelView(_)
            | Self::Panel(
                _,
                PanelCommand::SetView(_)
                | PanelCommand::RestorePanelizedResults
                | PanelCommand::Reread,
            )
            | Self::InvertTags
            | Self::CycleListingFormat
            | Self::SortNext
            | Self::SortReverse
            | Self::Copy
            | Self::Move
            | Self::Delete
            | Self::CancelJob
            | Self::RestorePanelizedResults
            | Self::OpenEntry
            | Self::EditEntry
            | Self::CdUp
            | Self::Reread
            | Self::FindResultsOpenEntry
            | Self::FindResultsPanelize
            | Self::FindResultsSelectAt(_)
            | Self::FindResultsAgain
            | Self::FindResultsTogglePause
            | Self::TreeOpenEntry
            | Self::TreeRescan
            | Self::TreeForget
            | Self::TreeToggleNavigation
            | Self::TreeCopy
            | Self::TreeMove
            | Self::TreeMkdir
            | Self::TreeDelete
            | Self::TreeSearchNext
            | Self::TreeSearchBackspace
            | Self::TreeSearchAppend(_)
            | Self::TreeSelectVisibleAt(_)
            | Self::HotlistOpenEntry
            | Self::HotlistAddCurrentDirectory
            | Self::HotlistEditSelected
            | Self::HotlistRemoveSelected
            | Self::HotlistSelectAt(_) => CommandDomain::Navigation,
            Self::ViewerSearchForward
            | Self::ViewerSearchBackward
            | Self::ViewerSearchContinue
            | Self::ViewerSearchContinueBackward
            | Self::ViewerGoto
            | Self::ViewerToggleWrap
            | Self::ViewerToggleHex => CommandDomain::Viewer,
            Self::OpenConfirmDialog
            | Self::OpenInputDialog
            | Self::OpenQuickCd
            | Self::OpenListboxDialog
            | Self::OpenSkinDialog
            | Self::OpenListingFormat
            | Self::OpenSortOrder
            | Self::OpenPanelFilter
            | Self::Panel(
                _,
                PanelCommand::OpenListingFormat
                | PanelCommand::OpenSortOrder
                | PanelCommand::OpenFilter,
            )
            | Self::FindDialogBrowse
            | Self::DialogAccept
            | Self::DialogCancel
            | Self::DialogFocusNext
            | Self::DialogBackspace
            | Self::DialogInputChar(_)
            | Self::DialogListboxUp
            | Self::DialogListboxDown
            | Self::DialogListboxSelectAt(_) => CommandDomain::Dialog,
            Self::OpenOptionsConfiguration
            | Self::OpenOptionsLayout
            | Self::OpenOptionsPanelOptions
            | Self::OpenOptionsConfirmation
            | Self::OpenOptionsAppearance
            | Self::OpenOptionsDisplayBits
            | Self::OpenOptionsLearnKeys
            | Self::OpenOptionsVirtualFs
            | Self::SaveSetup => CommandDomain::Settings,
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
    pub fn is_implemented(&self) -> bool {
        !matches!(self.command, AppCommand::MenuNotImplemented(_))
    }

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

const fn side_menu_entries(panel: ActivePanel) -> [MenuEntry; 16] {
    [
        MenuEntry::action(
            "File listing",
            AppCommand::Panel(panel, PanelCommand::SetView(PanelViewMode::Listing)),
        ),
        MenuEntry::action_with_literal_shortcut(
            "Quick view",
            "C-x q",
            AppCommand::Panel(panel, PanelCommand::SetView(PanelViewMode::QuickView)),
        ),
        MenuEntry::action_with_literal_shortcut(
            "Info",
            "C-x i",
            AppCommand::Panel(panel, PanelCommand::SetView(PanelViewMode::Info)),
        ),
        MenuEntry::action("Tree", AppCommand::Panel(panel, PanelCommand::OpenTree)),
        MenuEntry::separator(),
        MenuEntry::action(
            "Listing format...",
            AppCommand::Panel(panel, PanelCommand::OpenListingFormat),
        ),
        MenuEntry::action(
            "Sort order...",
            AppCommand::Panel(panel, PanelCommand::OpenSortOrder),
        ),
        MenuEntry::action(
            "Filter...",
            AppCommand::Panel(panel, PanelCommand::OpenFilter),
        ),
        MenuEntry::stub("Encoding...", "M-e"),
        MenuEntry::separator(),
        MenuEntry::stub("FTP link...", ""),
        MenuEntry::stub("Shell link...", ""),
        MenuEntry::stub("SFTP link...", ""),
        MenuEntry::action(
            "Panelize",
            AppCommand::Panel(panel, PanelCommand::RestorePanelizedResults),
        ),
        MenuEntry::separator(),
        MenuEntry::action_with_shortcut(
            "Rescan",
            "C-r",
            AppCommand::Panel(panel, PanelCommand::Reread),
        ),
    ]
}

const LEFT_SIDE_MENU_ENTRIES: [MenuEntry; 16] = side_menu_entries(ActivePanel::Left);
const RIGHT_SIDE_MENU_ENTRIES: [MenuEntry; 16] = side_menu_entries(ActivePanel::Right);

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
    MenuEntry::action_with_shortcut("Quick cd", "M-c", AppCommand::OpenQuickCd),
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
        entries: &LEFT_SIDE_MENU_ENTRIES,
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
        entries: &RIGHT_SIDE_MENU_ENTRIES,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MouseClickCommands {
    pub primary: AppCommand,
    pub activation: Option<AppCommand>,
}

impl MouseClickCommands {
    const fn primary(primary: AppCommand) -> Self {
        Self {
            primary,
            activation: None,
        }
    }

    const fn list_selection(primary: AppCommand, activation: AppCommand) -> Self {
        Self {
            primary,
            activation: Some(activation),
        }
    }
}

const PANELIZE_CUSTOM_COMMAND_LABEL: &str = "<Custom command>";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SortField {
    Name,
    Version,
    Extension,
    Modified,
    Accessed,
    Changed,
    Size,
    Inode,
    Unsorted,
}

impl SortField {
    pub const ALL: [Self; 9] = [
        Self::Name,
        Self::Version,
        Self::Extension,
        Self::Modified,
        Self::Accessed,
        Self::Changed,
        Self::Size,
        Self::Inode,
        Self::Unsorted,
    ];

    fn next(self) -> Self {
        match self {
            Self::Name => Self::Version,
            Self::Version => Self::Extension,
            Self::Extension => Self::Modified,
            Self::Modified => Self::Accessed,
            Self::Accessed => Self::Changed,
            Self::Changed => Self::Size,
            Self::Size => Self::Inode,
            Self::Inode => Self::Unsorted,
            Self::Unsorted => Self::Name,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Version => "version",
            Self::Extension => "extension",
            Self::Modified => "mtime",
            Self::Accessed => "atime",
            Self::Changed => "ctime",
            Self::Size => "size",
            Self::Inode => "inode",
            Self::Unsorted => "unsorted",
        }
    }

    pub const fn dialog_label(self) -> &'static str {
        match self {
            Self::Name => "Name (alphabetical)",
            Self::Version => "Name (natural/version)",
            Self::Extension => "Extension",
            Self::Modified => "Modification time",
            Self::Accessed => "Access time",
            Self::Changed => "Inode change time",
            Self::Size => "Size",
            Self::Inode => "Inode",
            Self::Unsorted => "Unsorted (filesystem order)",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Name => 0,
            Self::Version => 1,
            Self::Extension => 2,
            Self::Modified => 3,
            Self::Accessed => 4,
            Self::Changed => 5,
            Self::Size => 6,
            Self::Inode => 7,
            Self::Unsorted => 8,
        }
    }

    fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
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
        *self = self.other();
    }

    pub const fn other(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileEntryKind {
    Parent,
    Directory,
    File,
}

impl FileEntryKind {
    pub const fn is_dir(self) -> bool {
        matches!(self, Self::Parent | Self::Directory)
    }

    pub const fn is_parent(self) -> bool {
        matches!(self, Self::Parent)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub kind: FileEntryKind,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub metadata: FileEntryMetadata,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FileEntryMetadata {
    pub accessed: Option<SystemTime>,
    pub changed: Option<SystemTime>,
    pub mode: Option<u32>,
    pub hard_links: Option<u64>,
    pub user_id: Option<u32>,
    pub group_id: Option<u32>,
    pub inode: Option<u64>,
}

impl FileEntryMetadata {
    fn from_metadata(metadata: Option<&fs::Metadata>) -> Self {
        let Some(metadata) = metadata else {
            return Self::default();
        };

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;

            Self {
                accessed: metadata.accessed().ok(),
                changed: system_time_from_unix_parts(metadata.ctime(), metadata.ctime_nsec()),
                mode: Some(metadata.mode()),
                hard_links: Some(metadata.nlink()),
                user_id: Some(metadata.uid()),
                group_id: Some(metadata.gid()),
                inode: Some(metadata.ino()),
            }
        }

        #[cfg(not(unix))]
        {
            Self {
                accessed: metadata.accessed().ok(),
                changed: metadata.created().ok(),
                ..Self::default()
            }
        }
    }
}

#[cfg(unix)]
fn system_time_from_unix_parts(seconds: i64, nanoseconds: i64) -> Option<SystemTime> {
    use std::time::{Duration, UNIX_EPOCH};

    const NANOS_PER_SECOND: i128 = 1_000_000_000;
    let total_nanoseconds = i128::from(seconds)
        .checked_mul(NANOS_PER_SECOND)?
        .checked_add(i128::from(nanoseconds))?;
    let magnitude = total_nanoseconds.unsigned_abs();
    let duration = Duration::new(
        u64::try_from(magnitude / NANOS_PER_SECOND as u128).ok()?,
        u32::try_from(magnitude % NANOS_PER_SECOND as u128).ok()?,
    );
    if total_nanoseconds.is_negative() {
        UNIX_EPOCH.checked_sub(duration)
    } else {
        UNIX_EPOCH.checked_add(duration)
    }
}

impl FileEntry {
    #[cfg(test)]
    fn file(name: String, path: PathBuf, size: u64, modified: Option<SystemTime>) -> Self {
        Self {
            name,
            path,
            kind: FileEntryKind::File,
            size,
            modified,
            metadata: FileEntryMetadata::default(),
        }
    }

    fn directory_from_metadata(
        name: String,
        path: PathBuf,
        metadata: Option<&fs::Metadata>,
    ) -> Self {
        Self {
            name,
            path,
            kind: FileEntryKind::Directory,
            size: metadata.map_or(0, fs::Metadata::len),
            modified: metadata.and_then(|metadata| metadata.modified().ok()),
            metadata: FileEntryMetadata::from_metadata(metadata),
        }
    }

    fn file_from_metadata(name: String, path: PathBuf, metadata: Option<&fs::Metadata>) -> Self {
        Self {
            name,
            path,
            kind: FileEntryKind::File,
            size: metadata.map_or(0, fs::Metadata::len),
            modified: metadata.and_then(|metadata| metadata.modified().ok()),
            metadata: FileEntryMetadata::from_metadata(metadata),
        }
    }

    fn parent(path: PathBuf) -> Self {
        Self {
            name: String::from(".."),
            path,
            kind: FileEntryKind::Parent,
            size: 0,
            modified: None,
            metadata: FileEntryMetadata::default(),
        }
    }

    pub const fn is_dir(&self) -> bool {
        self.kind.is_dir()
    }

    pub const fn is_parent(&self) -> bool {
        self.kind.is_parent()
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiskUsageSummary {
    pub free_bytes: u64,
    pub total_bytes: u64,
}

impl PanelListingSource {
    fn is_panelized(&self) -> bool {
        !matches!(self, Self::Directory)
    }
}

#[derive(Clone, Debug)]
struct PanelizedResultSnapshot {
    cwd: PathBuf,
    source: PanelListingSource,
    entries: Vec<FileEntry>,
    unfiltered_entries: Option<Arc<[FileEntry]>>,
    cursor: usize,
    tagged: HashSet<PathBuf>,
    disk_usage: Option<DiskUsageSummary>,
}

impl PanelizedResultSnapshot {
    fn from_panel(panel: &PanelState) -> Option<Self> {
        (panel.source.is_panelized() && !panel.loading).then(|| Self {
            cwd: panel.cwd.clone(),
            source: panel.source.clone(),
            entries: panel.entries.clone(),
            unfiltered_entries: panel.panelized_entries.clone(),
            cursor: panel.cursor,
            tagged: panel.tagged.clone(),
            disk_usage: panel.disk_usage,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PanelState {
    pub cwd: PathBuf,
    pub entries: Vec<FileEntry>,
    pub cursor: usize,
    pub sort_mode: SortMode,
    filter: PanelFilter,
    show_hidden_files: bool,
    source: PanelListingSource,
    panelized_entries: Option<Arc<[FileEntry]>>,
    tagged: HashSet<PathBuf>,
    pub loading: bool,
    pub disk_usage: Option<DiskUsageSummary>,
}

impl PanelState {
    pub fn new(cwd: PathBuf) -> io::Result<Self> {
        Ok(Self {
            cwd,
            entries: Vec::new(),
            cursor: 0,
            sort_mode: SortMode::default(),
            filter: PanelFilter::default(),
            show_hidden_files: true,
            source: PanelListingSource::Directory,
            panelized_entries: None,
            tagged: HashSet::new(),
            loading: false,
            disk_usage: None,
        })
    }

    pub fn refresh(&mut self) -> io::Result<()> {
        let (entries, panelized_entries) = match &self.source {
            PanelListingSource::Directory => {
                let entries = read_entries_with_visibility(
                    &self.cwd,
                    self.sort_mode,
                    self.show_hidden_files,
                )?;
                (entries, None)
            }
            PanelListingSource::Panelize { command } => {
                let discovered_entries = stream_panelized_entries_with_cancel(
                    &self.cwd,
                    command,
                    None,
                    &mut |_| Ok(()),
                )?;
                let mut entries = discovered_entries.clone();
                sort_file_entries(&mut entries, self.sort_mode);
                (entries, Some(Arc::<[FileEntry]>::from(discovered_entries)))
            }
            PanelListingSource::FindResults {
                base_dir, paths, ..
            } => {
                let discovered_entries =
                    stream_panelized_paths_with_cancel(base_dir, paths, None, &mut |_| Ok(()))?;
                let mut entries = discovered_entries.clone();
                sort_file_entries(&mut entries, self.sort_mode);
                (entries, Some(Arc::<[FileEntry]>::from(discovered_entries)))
            }
        };
        let entries = apply_panel_filter(entries, &self.filter)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        self.panelized_entries = panelized_entries;
        self.apply_entries(entries);
        self.loading = false;
        Ok(())
    }

    fn apply_entries(&mut self, entries: Vec<FileEntry>) {
        self.entries = entries;
        // Filters are views: tags hidden by a filter must reappear when it is cleared.
        if !self.filter.is_active() {
            self.tagged.retain(|tag| {
                self.entries
                    .iter()
                    .any(|entry| !entry.is_parent() && entry.path == *tag)
            });
        }
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

    pub fn filter(&self) -> &PanelFilter {
        &self.filter
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
        if entry.is_parent() {
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
        for entry in &self.entries {
            if entry.is_parent() {
                continue;
            }
            if !self.tagged.insert(entry.path.clone()) {
                self.tagged.remove(&entry.path);
            }
        }
    }

    pub fn tagged_paths_in_display_order(&self) -> Vec<PathBuf> {
        self.entries
            .iter()
            .filter(|entry| !entry.is_parent() && self.tagged.contains(&entry.path))
            .map(|entry| entry.path.clone())
            .collect()
    }

    pub fn tagged_paths(&self) -> Vec<PathBuf> {
        let mut paths = self.tagged.iter().cloned().collect::<Vec<_>>();
        paths.sort_unstable();
        paths
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

    pub fn open_selected_directory(&mut self) -> bool {
        let Some((path, is_dir_hint)) = self
            .selected_entry()
            .map(|entry| (entry.path.clone(), entry.is_dir()))
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
        self.panelized_entries = None;
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
        self.panelized_entries = None;
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
        self.panelized_entries = None;
        self.tagged.clear();
        self.entries.clear();
        self.loading = true;
        true
    }

    pub fn panelize_with_command(&mut self, command: String) -> io::Result<usize> {
        let previous_source = self.source.clone();
        let previous_panelized_entries = self.panelized_entries.clone();
        self.source = PanelListingSource::Panelize { command };
        self.panelized_entries = None;
        self.cursor = 0;
        self.tagged.clear();

        if let Err(error) = self.refresh() {
            self.source = previous_source;
            self.panelized_entries = previous_panelized_entries;
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FindResultsStatus {
    Running,
    Paused,
    Canceling,
    Completed,
    Partial,
    Canceled,
    Failed(String),
}

impl FindResultsStatus {
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Running => "searching",
            Self::Paused => "paused",
            Self::Canceling => "canceling",
            Self::Completed => "completed",
            Self::Partial => "partial",
            Self::Canceled => "canceled",
            Self::Failed(_) => "failed",
        }
    }

    pub const fn is_active(&self) -> bool {
        matches!(self, Self::Running | Self::Paused | Self::Canceling)
    }
}

#[derive(Clone, Debug)]
pub struct FindResultsState {
    pub job_id: JobId,
    pub spec: FindSpec,
    pub entries: Vec<FindResultEntry>,
    pub cursor: usize,
    pub status: FindResultsStatus,
    pub report: Option<FindSearchReport>,
}

impl FindResultsState {
    fn loading(job_id: JobId, spec: FindSpec) -> Self {
        Self {
            job_id,
            spec,
            entries: Vec::new(),
            cursor: 0,
            status: FindResultsStatus::Running,
            report: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.status.is_active()
    }

    fn apply_report(&mut self, report: FindSearchReport) {
        self.status = if report.is_partial() || report.truncated {
            FindResultsStatus::Partial
        } else {
            FindResultsStatus::Completed
        };
        self.report = Some(report);
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
    CycleDefaultOverwritePolicy,
    ToggleMacosOptionSymbols,
    ToggleLayoutShowMenuBar,
    ToggleLayoutShowButtonBar,
    ToggleLayoutShowDebugStatus,
    ToggleLayoutShowPanelTotals,
    CycleLayoutStatusMessageTimeout,
    TogglePanelShowHiddenFiles,
    CyclePanelSortField(ActivePanel),
    TogglePanelSortReverse(ActivePanel),
    ToggleConfirmDelete,
    ToggleConfirmOverwrite,
    ToggleConfirmQuit,
    ToggleConfirmHotlistDelete,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransferKind {
    Copy,
    Move,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OperationOrigin {
    Panel(ActivePanel),
    Tree,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingPanelMkdir {
    panel: ActivePanel,
    path: PathBuf,
    origin_cwd: PathBuf,
    origin_source: PanelListingSource,
}

#[derive(Debug, Default)]
struct PanelMkdirTracker {
    pending: HashMap<JobId, PendingPanelMkdir>,
    latest_job_ids: [Option<JobId>; 2],
}

#[derive(Clone, Debug)]
enum PendingDialogAction {
    ConfirmDelete {
        targets: Vec<PathBuf>,
        origin: OperationOrigin,
    },
    ConfirmQuit,
    Mkdir {
        base_dir: PathBuf,
        origin: OperationOrigin,
    },
    RenameEntry {
        source: PathBuf,
    },
    TransferDestination {
        kind: TransferKind,
        sources: Vec<PathBuf>,
        source_base_dir: PathBuf,
        origin: OperationOrigin,
    },
    TransferOverwrite {
        kind: TransferKind,
        sources: Vec<PathBuf>,
        destination_dir: PathBuf,
        origin: OperationOrigin,
    },
    SetDefaultOverwritePolicy,
    SetSkin {
        original_skin: String,
    },
    SetPanelListingFormat {
        panel: ActivePanel,
    },
    SetPanelSortOrder {
        panel: ActivePanel,
        reverse: bool,
    },
    SetPanelFilter {
        panel: ActivePanel,
    },
    ViewerSearch {
        direction: ViewerSearchDirection,
    },
    ViewerGoto,
    FindSearch,
    QuickCd,
    HotlistAdd {
        base_dir: PathBuf,
    },
    HotlistEdit {
        base_dir: PathBuf,
        index: usize,
        original: HotlistEntry,
    },
    HotlistRemove {
        index: usize,
        entry: HotlistEntry,
    },
    PanelizePresetSelection {
        initial_command: String,
        presets: Vec<PanelizePreset>,
    },
    PanelizeCommand {
        presets: Vec<PanelizePreset>,
    },
    PanelizePresetAdd {
        initial_command: String,
        presets: Vec<PanelizePreset>,
    },
    PanelizePresetEdit {
        initial_command: String,
        presets: Vec<PanelizePreset>,
        preset_index: usize,
    },
    PanelizePresetRemove {
        initial_command: String,
        presets: Vec<PanelizePreset>,
        preset_index: usize,
    },
}

#[derive(Clone, Debug)]
pub struct DialogRoute {
    pub state: DialogState,
    action: Option<PendingDialogAction>,
}

impl DialogRoute {
    fn new(state: DialogState, action: PendingDialogAction) -> Self {
        Self {
            state,
            action: Some(action),
        }
    }

    fn take_action(&mut self) -> Option<PendingDialogAction> {
        self.action.take()
    }

    fn action(&self) -> Option<&PendingDialogAction> {
        self.action.as_ref()
    }

    fn action_mut(&mut self) -> Option<&mut PendingDialogAction> {
        self.action.as_mut()
    }
}

impl Deref for DialogRoute {
    type Target = DialogState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DerefMut for DialogRoute {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
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
    Tree(Box<TreeState>),
    Hotlist,
    Dialog(DialogRoute),
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
    NoEditorResolved,
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
            KeyContext::FindDialog,
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
    panel_views: [PanelViewMode; 2],
    panel_listing_formats: [PanelListingFormat; 2],
    quick_views: [QuickViewState; 2],
    selection_sizes: [SelectionSizeState; 2],
    pub status_line: String,
    status_expires_at: Option<Instant>,
    pub last_dialog_result: Option<DialogResult>,
    pub jobs: JobManager,
    pub jobs_cursor: usize,
    pub hotlist_cursor: usize,
    available_skins: Vec<String>,
    preview_skin_name: Option<String>,
    pending_skin_change: Option<String>,
    pending_skin_preview: Option<String>,
    pending_skin_revert: Option<String>,
    routes: Vec<Route>,
    paused_find_results: Option<FindResultsState>,
    pending_find_tree_picker: Option<FindDialogState>,
    pending_worker_commands: Vec<WorkerCommand>,
    pending_external_edit_requests: Vec<ExternalEditRequest>,
    panelized_result_history: [Option<PanelizedResultSnapshot>; 2],
    previous_panel_directories: [Option<PathBuf>; 2],
    panel_refresh: PanelRefreshWorkflow,
    panel_refresh_post: PanelRefreshPostWorkflow,
    quick_view: QuickViewWorkflow,
    selection_size: SelectionSizeWorkflow,
    find_pause_flags: HashMap<JobId, Arc<AtomicBool>>,
    deferred_persist_settings_request: Option<JobRequest>,
    panel_mkdirs: PanelMkdirTracker,
    tree_mutations: TreeMutationTracker,
    keybinding_hints: KeybindingHints,
    keymap_unknown_actions: usize,
    keymap_invalid_bindings: usize,
    pending_learn_keys_capture: bool,
    xmap_pending: bool,
    pending_save_setup: bool,
    pending_quit: bool,
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

fn resolve_external_editor_command(configured_editor: Option<&str>) -> Option<String> {
    resolve_external_editor_command_with_lookup(
        configured_editor,
        |name| std::env::var(name).ok(),
        executable_on_path,
    )
}

fn resolve_external_editor_command_with_lookup(
    configured_editor: Option<&str>,
    mut lookup_env: impl FnMut(&str) -> Option<String>,
    mut executable_exists: impl FnMut(&str) -> bool,
) -> Option<String> {
    if let Some(editor) = configured_editor.and_then(non_empty_env_value) {
        return Some(editor.to_string());
    }
    for variable in ["EDITOR", "VISUAL"] {
        if let Some(value) = lookup_env(variable)
            && let Some(trimmed) = non_empty_env_value(&value)
        {
            return Some(trimmed.to_string());
        }
    }
    for executable in ["hx", "nvim", "vim", "vi", "emacs"] {
        if executable_exists(executable) {
            return Some(executable.to_string());
        }
    }
    None
}

fn non_empty_env_value(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn executable_on_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| executable_candidate_exists(&dir, name))
}

fn executable_candidate_exists(dir: &Path, name: &str) -> bool {
    #[cfg(windows)]
    {
        let candidate = dir.join(name);
        if executable_path_is_runnable(&candidate) {
            return true;
        }
        let extensions = std::env::var_os("PATHEXT")
            .map(|value| {
                value
                    .to_string_lossy()
                    .split(';')
                    .filter(|extension| !extension.trim().is_empty())
                    .map(|extension| extension.trim().trim_start_matches('.').to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                vec![
                    String::from("exe"),
                    String::from("cmd"),
                    String::from("bat"),
                ]
            });
        extensions
            .into_iter()
            .any(|extension| executable_path_is_runnable(&dir.join(format!("{name}.{extension}"))))
    }
    #[cfg(not(windows))]
    {
        executable_path_is_runnable(&dir.join(name))
    }
}

#[cfg(windows)]
fn executable_path_is_runnable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(unix)]
fn executable_path_is_runnable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(any(unix, windows)))]
fn executable_path_is_runnable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests;
