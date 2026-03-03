#![forbid(unsafe_code)]

mod background;
mod command_dispatch;
mod command_map;
pub mod dialog;
mod dialog_flow;
pub mod help;
pub mod jobs;
mod keybinding_help;
pub mod keymap;
mod navigation_flow;
mod orchestration;
mod panel;
mod panelize_flow;
mod refresh_flow;
mod route_flow;
pub mod settings;
mod settings_flow;
pub mod settings_io;
pub mod slo;
mod state_flow;
mod tree_builder;
mod viewer;
mod viewer_flow;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};
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
pub(crate) use tree_builder::build_tree_entries;
pub use viewer::ViewerState;

use crate::keymap::{KeyChord, KeyCode, KeyContext, Keymap, KeymapParseReport};
use crate::panel::{read_entries_with_visibility, read_panelized_entries};
use crate::refresh_flow::PanelRefreshWorkflow;
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
            | Self::OpenJobsScreen
            | Self::CloseJobsScreen
            | Self::JobsMoveUp
            | Self::JobsMoveDown
            | Self::MenuMoveUp
            | Self::MenuMoveDown
            | Self::MenuMoveLeft
            | Self::MenuMoveRight
            | Self::MenuHome
            | Self::MenuEnd
            | Self::MenuAccept
            | Self::MenuSelectAt(_)
            | Self::HelpMoveUp
            | Self::HelpMoveDown
            | Self::HelpPageUp
            | Self::HelpPageDown
            | Self::HelpHalfPageUp
            | Self::HelpHalfPageDown
            | Self::HelpHome
            | Self::HelpEnd
            | Self::HelpFollowLink
            | Self::HelpBack
            | Self::HelpIndex
            | Self::HelpLinkNext
            | Self::HelpLinkPrev
            | Self::HelpNodeNext
            | Self::HelpNodePrev
            | Self::MenuNoop
            | Self::MenuNotImplemented(_) => CommandDomain::Route,
            Self::MoveUp
            | Self::MoveDown
            | Self::PageUp
            | Self::PageDown
            | Self::MoveHome
            | Self::MoveEnd
            | Self::ToggleTag
            | Self::InvertTags
            | Self::SortNext
            | Self::SortReverse
            | Self::Copy
            | Self::Move
            | Self::Delete
            | Self::CancelJob
            | Self::OpenEntry
            | Self::EditEntry
            | Self::CdUp
            | Self::Reread
            | Self::FindResultsMoveUp
            | Self::FindResultsMoveDown
            | Self::FindResultsPageUp
            | Self::FindResultsPageDown
            | Self::FindResultsHome
            | Self::FindResultsEnd
            | Self::FindResultsOpenEntry
            | Self::FindResultsPanelize
            | Self::TreeMoveUp
            | Self::TreeMoveDown
            | Self::TreePageUp
            | Self::TreePageDown
            | Self::TreeHome
            | Self::TreeEnd
            | Self::TreeOpenEntry
            | Self::HotlistMoveUp
            | Self::HotlistMoveDown
            | Self::HotlistPageUp
            | Self::HotlistPageDown
            | Self::HotlistHome
            | Self::HotlistEnd
            | Self::HotlistOpenEntry
            | Self::HotlistAddCurrentDirectory
            | Self::HotlistRemoveSelected => CommandDomain::Navigation,
            Self::ViewerMoveUp
            | Self::ViewerMoveDown
            | Self::ViewerPageUp
            | Self::ViewerPageDown
            | Self::ViewerHome
            | Self::ViewerEnd
            | Self::ViewerSearchForward
            | Self::ViewerSearchBackward
            | Self::ViewerSearchContinue
            | Self::ViewerSearchContinueBackward
            | Self::ViewerGoto
            | Self::ViewerToggleWrap
            | Self::ViewerToggleHex => CommandDomain::Viewer,
            Self::OpenConfirmDialog
            | Self::OpenInputDialog
            | Self::OpenListboxDialog
            | Self::OpenSkinDialog
            | Self::DialogAccept
            | Self::DialogCancel
            | Self::DialogFocusNext
            | Self::DialogBackspace
            | Self::DialogInputChar(_)
            | Self::DialogListboxUp
            | Self::DialogListboxDown => CommandDomain::Dialog,
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
    panel_refresh: PanelRefreshWorkflow,
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
mod tests;
