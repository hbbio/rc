#![forbid(unsafe_code)]

pub mod dialog;
pub mod help;
pub mod jobs;
pub mod keymap;

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
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

use crate::dialog::DialogEvent;
use crate::keymap::{KeyCommand, KeyContext};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
    CdUp,
    Reread,
    FindResultsMoveUp,
    FindResultsMoveDown,
    FindResultsPageUp,
    FindResultsPageDown,
    FindResultsHome,
    FindResultsEnd,
    FindResultsOpenEntry,
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
    pub command: AppCommand,
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

const LEFT_MENU_ENTRIES: [MenuEntry; 6] = [
    MenuEntry {
        label: "Find file",
        command: AppCommand::OpenFindDialog,
    },
    MenuEntry {
        label: "Directory tree",
        command: AppCommand::OpenTree,
    },
    MenuEntry {
        label: "Directory hotlist",
        command: AppCommand::OpenHotlist,
    },
    MenuEntry {
        label: "External panelize",
        command: AppCommand::OpenPanelizeDialog,
    },
    MenuEntry {
        label: "Skin",
        command: AppCommand::OpenSkinDialog,
    },
    MenuEntry {
        label: "Help",
        command: AppCommand::OpenHelp,
    },
];

const FILE_MENU_ENTRIES: [MenuEntry; 7] = [
    MenuEntry {
        label: "View",
        command: AppCommand::OpenEntry,
    },
    MenuEntry {
        label: "Copy",
        command: AppCommand::Copy,
    },
    MenuEntry {
        label: "Move",
        command: AppCommand::Move,
    },
    MenuEntry {
        label: "Mkdir",
        command: AppCommand::OpenInputDialog,
    },
    MenuEntry {
        label: "Delete",
        command: AppCommand::Delete,
    },
    MenuEntry {
        label: "Rename",
        command: AppCommand::OpenConfirmDialog,
    },
    MenuEntry {
        label: "Quit",
        command: AppCommand::Quit,
    },
];

const COMMAND_MENU_ENTRIES: [MenuEntry; 6] = [
    MenuEntry {
        label: "Jobs",
        command: AppCommand::OpenJobsScreen,
    },
    MenuEntry {
        label: "Cancel job",
        command: AppCommand::CancelJob,
    },
    MenuEntry {
        label: "Find file",
        command: AppCommand::OpenFindDialog,
    },
    MenuEntry {
        label: "Directory tree",
        command: AppCommand::OpenTree,
    },
    MenuEntry {
        label: "Directory hotlist",
        command: AppCommand::OpenHotlist,
    },
    MenuEntry {
        label: "Help",
        command: AppCommand::OpenHelp,
    },
];

const OPTIONS_MENU_ENTRIES: [MenuEntry; 5] = [
    MenuEntry {
        label: "Sort next",
        command: AppCommand::SortNext,
    },
    MenuEntry {
        label: "Sort reverse",
        command: AppCommand::SortReverse,
    },
    MenuEntry {
        label: "Reread",
        command: AppCommand::Reread,
    },
    MenuEntry {
        label: "Skin",
        command: AppCommand::OpenSkinDialog,
    },
    MenuEntry {
        label: "Panelize",
        command: AppCommand::OpenPanelizeDialog,
    },
];

const RIGHT_MENU_ENTRIES: [MenuEntry; 3] = [
    MenuEntry {
        label: "Swap panels",
        command: AppCommand::SwitchPanel,
    },
    MenuEntry {
        label: "Directory tree",
        command: AppCommand::OpenTree,
    },
    MenuEntry {
        label: "Directory hotlist",
        command: AppCommand::OpenHotlist,
    },
];

const TOP_MENUS: [TopMenu; 5] = [
    TopMenu {
        title: "Left",
        entries: &LEFT_MENU_ENTRIES,
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
        entries: &RIGHT_MENU_ENTRIES,
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

const DEFAULT_PAGE_STEP: usize = 10;
const DEFAULT_VIEWER_PAGE_STEP: usize = 20;
const MAX_FIND_RESULTS: usize = 2_000;
const FIND_EVENT_CHUNK_SIZE: usize = 64;
const TREE_MAX_DEPTH: usize = 6;
const TREE_MAX_ENTRIES: usize = 2_000;
const PANEL_REFRESH_CANCELED_MESSAGE: &str = "panel refresh canceled";

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
    Panelize { command: String },
}

#[derive(Clone, Debug)]
pub struct PanelState {
    pub cwd: PathBuf,
    pub entries: Vec<FileEntry>,
    pub cursor: usize,
    pub sort_mode: SortMode,
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
            source: PanelListingSource::Directory,
            tagged: HashSet::new(),
            loading: false,
        };
        panel.refresh()?;
        Ok(panel)
    }

    pub fn refresh(&mut self) -> io::Result<()> {
        let entries = match &self.source {
            PanelListingSource::Directory => read_entries(&self.cwd, self.sort_mode)?,
            PanelListingSource::Panelize { command } => {
                read_panelized_entries(&self.cwd, command, self.sort_mode)?
            }
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

    pub fn move_cursor_page(&mut self, pages: isize) {
        let delta = pages.saturating_mul(DEFAULT_PAGE_STEP as isize);
        self.move_cursor(delta);
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
            PanelListingSource::Directory => None,
        }
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

    pub fn move_pages(&mut self, pages: isize) {
        self.move_lines(pages.saturating_mul(DEFAULT_VIEWER_PAGE_STEP as isize));
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

    fn move_page(&mut self, pages: isize) {
        self.move_cursor(pages.saturating_mul(DEFAULT_PAGE_STEP as isize));
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

    fn move_page(&mut self, pages: isize) {
        self.move_cursor(pages.saturating_mul(DEFAULT_PAGE_STEP as isize));
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

    pub fn popup_width(&self) -> u16 {
        let inner = self
            .active_entries()
            .iter()
            .map(|entry| entry.label.chars().count() as u16)
            .max()
            .unwrap_or(1)
            .saturating_add(2);
        inner.saturating_add(2)
    }

    pub fn popup_height(&self) -> u16 {
        self.active_entries().len() as u16 + 2
    }

    fn set_active_menu(&mut self, active_menu: usize) {
        self.active_menu = active_menu.min(TOP_MENUS.len().saturating_sub(1));
        self.selected_entry = 0;
        self.clamp_selected_entry();
    }

    fn move_up(&mut self) {
        if self.active_entries().is_empty() {
            self.selected_entry = 0;
            return;
        }
        self.selected_entry = self.selected_entry.saturating_sub(1);
    }

    fn move_down(&mut self) {
        if self.active_entries().is_empty() {
            self.selected_entry = 0;
            return;
        }
        let last = self.active_entries().len() - 1;
        self.selected_entry = self.selected_entry.saturating_add(1).min(last);
    }

    fn move_left(&mut self) {
        if self.active_menu == 0 {
            self.active_menu = TOP_MENUS.len() - 1;
        } else {
            self.active_menu -= 1;
        }
        self.selected_entry = 0;
    }

    fn move_right(&mut self) {
        self.active_menu = (self.active_menu + 1) % TOP_MENUS.len();
        self.selected_entry = 0;
    }

    fn move_home(&mut self) {
        self.selected_entry = 0;
    }

    fn move_end(&mut self) {
        self.selected_entry = self.active_entries().len().saturating_sub(1);
    }

    fn select_entry(&mut self, index: usize) {
        self.selected_entry = index;
        self.clamp_selected_entry();
    }

    fn selected_command(&self) -> Option<AppCommand> {
        self.active_entries()
            .get(self.selected_entry)
            .map(|entry| entry.command)
    }

    fn hit_test_entry(&self, column: u16, row: u16) -> Option<usize> {
        let x = self.popup_origin_x();
        let y = 1u16;
        let width = self.popup_width();
        let items = self.active_entries().len() as u16;
        if items == 0 {
            return None;
        }

        if row < y + 1 || row >= y + 1 + items {
            return None;
        }
        if column < x + 1 || column >= x + width.saturating_sub(1) {
            return None;
        }

        Some((row - (y + 1)) as usize)
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
    }
}

#[derive(Clone, Debug)]
pub enum Route {
    FileManager,
    Help(HelpState),
    Menu(MenuState),
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
    PanelizeCommand,
}

#[derive(Clone, Debug)]
pub enum BackgroundCommand {
    RefreshPanel {
        panel: ActivePanel,
        cwd: PathBuf,
        source: PanelListingSource,
        sort_mode: SortMode,
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

pub fn run_background_worker(
    command_rx: Receiver<BackgroundCommand>,
    event_tx: Sender<BackgroundEvent>,
) {
    let mut running_find_tasks = Vec::new();
    while let Ok(command) = command_rx.recv() {
        reap_finished_find_tasks(&mut running_find_tasks);
        match execute_background_command(command, &event_tx) {
            BackgroundExecution::Continue => {}
            #[cfg(not(test))]
            BackgroundExecution::SpawnFind(task) => running_find_tasks.push(task),
            BackgroundExecution::Stop => break,
        }
    }

    for task in &running_find_tasks {
        task.cancel_flag.store(true, AtomicOrdering::Relaxed);
    }
    for task in running_find_tasks {
        let _ = task.handle.join();
    }
}

#[derive(Debug)]
struct RunningFindTask {
    handle: thread::JoinHandle<()>,
    cancel_flag: Arc<AtomicBool>,
}

#[derive(Debug)]
enum BackgroundExecution {
    Continue,
    #[cfg(not(test))]
    SpawnFind(RunningFindTask),
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
            request_id,
            cancel_flag,
        } => {
            let result = match &source {
                PanelListingSource::Directory =>
                    read_entries_with_cancel(&cwd, sort_mode, Some(cancel_flag.as_ref()))
                        .map_err(|error| error.to_string()),
                PanelListingSource::Panelize { command } =>
                    read_panelized_entries_with_cancel(
                        &cwd,
                        command,
                        sort_mode,
                        Some(cancel_flag.as_ref()),
                    )
                    .map_err(|error| error.to_string()),
            };
            event_tx
                .send(BackgroundEvent::PanelRefreshed {
                    panel,
                    cwd,
                    source,
                    sort_mode,
                    request_id,
                    result,
                })
                .is_ok()
                .then_some(BackgroundExecution::Continue)
                .unwrap_or(BackgroundExecution::Stop)
        }
        BackgroundCommand::LoadViewer { path } => event_tx
            .send(BackgroundEvent::ViewerLoaded {
                path: path.clone(),
                result: ViewerState::open(path).map_err(|error| error.to_string()),
            })
            .is_ok()
            .then_some(BackgroundExecution::Continue)
            .unwrap_or(BackgroundExecution::Stop),
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
                run_find_search(
                    event_tx,
                    job_id,
                    query,
                    base_dir,
                    max_results,
                    cancel_flag.as_ref(),
                    pause_flag.as_ref(),
                )
                .then_some(BackgroundExecution::Continue)
                .unwrap_or(BackgroundExecution::Stop)
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
            let entries = build_tree_entries(&root, max_depth, max_entries);
            event_tx
                .send(BackgroundEvent::TreeReady { root, entries })
                .is_ok()
                .then_some(BackgroundExecution::Continue)
                .unwrap_or(BackgroundExecution::Stop)
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

        child_dirs.sort_by_key(|left| path_sort_key(left));
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
    panel_refresh_cancel_flags: [Option<Arc<AtomicBool>>; 2],
    panel_refresh_request_ids: [u64; 2],
    next_panel_refresh_request_id: u64,
    pending_panel_focus: Option<(ActivePanel, PathBuf)>,
    find_pause_flags: HashMap<JobId, Arc<AtomicBool>>,
    pending_panelize_revert: Option<(ActivePanel, PanelListingSource)>,
    xmap_pending: bool,
}

impl AppState {
    pub fn new(start_path: PathBuf) -> io::Result<Self> {
        let left = PanelState::new(start_path.clone())?;
        let right = PanelState::new(start_path)?;

        Ok(Self {
            panels: [left, right],
            active_panel: ActivePanel::Left,
            status_line: String::from("Press F1 for help"),
            last_dialog_result: None,
            jobs: JobManager::new(),
            overwrite_policy: OverwritePolicy::Skip,
            jobs_cursor: 0,
            hotlist: Vec::new(),
            hotlist_cursor: 0,
            available_skins: Vec::new(),
            active_skin_name: String::from("default"),
            pending_skin_change: None,
            pending_skin_preview: None,
            pending_skin_revert: None,
            routes: vec![Route::FileManager],
            paused_find_results: None,
            pending_dialog_action: None,
            pending_worker_commands: Vec::new(),
            pending_background_commands: Vec::new(),
            panel_refresh_cancel_flags: std::array::from_fn(|_| None),
            panel_refresh_request_ids: [0; 2],
            next_panel_refresh_request_id: 1,
            pending_panel_focus: None,
            find_pause_flags: HashMap::new(),
            pending_panelize_revert: None,
            xmap_pending: false,
        })
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

    pub fn clear_xmap(&mut self) {
        self.xmap_pending = false;
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
                    let should_refresh = self.jobs.job(id).is_some_and(|job| {
                        matches!(job.kind, JobKind::Copy | JobKind::Move | JobKind::Delete)
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
                            let is_panelize = matches!(source, PanelListingSource::Panelize { .. });
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

        self.routes
            .push(Route::Help(HelpState::for_context(context)));
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

        if let Some(entry_index) = menu.hit_test_entry(column, row) {
            return Some(AppCommand::MenuSelectAt(entry_index));
        }

        Some(AppCommand::CloseMenu)
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
        let initial = self
            .active_panel()
            .panelize_command()
            .unwrap_or("find . -type f")
            .to_string();
        self.pending_dialog_action = Some(PendingDialogAction::PanelizeCommand);
        self.routes.push(Route::Dialog(DialogState::input(
            "External panelize",
            "Command (stdout paths):",
            initial,
        )));
        self.set_status("External panelize");
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
        results.move_page(pages);
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
                max_depth: TREE_MAX_DEPTH,
                max_entries: TREE_MAX_ENTRIES,
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
        tree.move_page(pages);
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
        self.move_hotlist_cursor(pages.saturating_mul(DEFAULT_PAGE_STEP as isize));
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
        self.set_status(format!("Added {} to hotlist", cwd.to_string_lossy()));
    }

    fn remove_selected_hotlist_entry(&mut self) {
        if self.hotlist.is_empty() {
            self.set_status("Hotlist is empty");
            return;
        }
        let removed = self.hotlist.remove(self.hotlist_cursor);
        self.clamp_hotlist_cursor();
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
            AppCommand::OpenMenu => self.open_menu(0),
            AppCommand::OpenMenuAt(index) => self.open_menu(index),
            AppCommand::CloseMenu => self.close_menu(),
            AppCommand::OpenHelp => self.open_help_screen(),
            AppCommand::CloseHelp => self.close_help_screen(),
            AppCommand::Quit => {
                self.request_cancel_for_all_jobs();
                return Ok(ApplyResult::Quit);
            }
            AppCommand::CloseViewer => self.close_viewer(),
            AppCommand::OpenFindDialog => self.open_find_dialog(),
            AppCommand::CloseFindResults => self.close_find_results(),
            AppCommand::OpenTree => self.open_tree_screen(),
            AppCommand::CloseTree => self.close_tree_screen(),
            AppCommand::OpenHotlist => self.open_hotlist_screen(),
            AppCommand::CloseHotlist => self.close_hotlist_screen(),
            AppCommand::OpenPanelizeDialog => self.open_panelize_dialog(),
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
            AppCommand::PageUp => self.active_panel_mut().move_cursor_page(-1),
            AppCommand::PageDown => self.active_panel_mut().move_cursor_page(1),
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
            AppCommand::Delete => self.start_delete_confirmation(),
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
            AppCommand::CdUp => {
                if self.go_parent_directory() {
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
            AppCommand::DialogAccept => self.handle_dialog_event(DialogEvent::Accept),
            AppCommand::DialogCancel => self.handle_dialog_event(DialogEvent::Cancel),
            AppCommand::DialogFocusNext => self.handle_dialog_event(DialogEvent::FocusNext),
            AppCommand::DialogBackspace => self.handle_dialog_event(DialogEvent::Backspace),
            AppCommand::DialogInputChar(ch) => {
                self.handle_dialog_event(DialogEvent::InsertChar(ch))
            }
            AppCommand::DialogListboxUp => self.handle_dialog_event(DialogEvent::MoveUp),
            AppCommand::DialogListboxDown => self.handle_dialog_event(DialogEvent::MoveDown),
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
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.move_pages(-1);
                }
            }
            AppCommand::ViewerPageDown => {
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.move_pages(1);
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
        let current_name = entry.name.clone();
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
        let summary = request.summary();
        let worker_job = self.jobs.enqueue(request);
        let job_id = worker_job.id;
        self.pending_worker_commands
            .push(WorkerCommand::Run(worker_job));
        self.set_status(format!("Queued job #{job_id}: {summary}"));
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

    fn finish_dialog(&mut self, result: DialogResult) {
        let pending = self.pending_dialog_action.take();
        match (pending, result) {
            (None, result) => self.set_status(result.status_line()),
            (
                Some(PendingDialogAction::ConfirmDelete { targets }),
                DialogResult::ConfirmAccepted,
            ) => {
                let request = JobRequest::Delete { targets };
                let summary = request.summary();
                let worker_job = self.jobs.enqueue(request);
                let job_id = worker_job.id;
                self.pending_worker_commands
                    .push(WorkerCommand::Run(worker_job));
                self.set_status(format!("Queued job #{job_id}: {summary}"));
            }
            (Some(PendingDialogAction::ConfirmDelete { .. }), DialogResult::ConfirmDeclined)
            | (Some(PendingDialogAction::ConfirmDelete { .. }), DialogResult::Canceled) => {
                self.set_status("Delete canceled");
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
                match fs::create_dir(&destination) {
                    Ok(()) => {
                        self.refresh_active_panel();
                        self.set_status(format!(
                            "Created directory {}",
                            destination.to_string_lossy()
                        ));
                    }
                    Err(error) => {
                        self.set_status(format!("Mkdir failed: {error}"));
                    }
                }
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
                match fs::rename(&source, &destination) {
                    Ok(()) => {
                        self.refresh_panels();
                        self.set_status(format!("Renamed to {}", destination.to_string_lossy()));
                    }
                    Err(error) => {
                        self.set_status(format!("Rename failed: {error}"));
                    }
                }
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
                        max_results: MAX_FIND_RESULTS,
                        cancel_flag: worker_job.cancel_flag(),
                        pause_flag,
                    });
                self.set_status(format!("Queued job #{job_id}: {summary}"));
            }
            (Some(PendingDialogAction::FindQuery { .. }), DialogResult::Canceled) => {
                self.set_status("Find canceled");
            }
            (Some(PendingDialogAction::PanelizeCommand), DialogResult::InputSubmitted(value)) => {
                let command = value.trim();
                if command.is_empty() {
                    self.set_status("Panelize canceled: empty command");
                    return;
                }

                let active_panel = self.active_panel;
                let previous_source = self.active_panel().source.clone();
                {
                    let panel = self.active_panel_mut();
                    panel.source = PanelListingSource::Panelize {
                        command: command.to_string(),
                    };
                    panel.cursor = 0;
                    panel.tagged.clear();
                    panel.loading = true;
                }
                self.pending_panelize_revert = Some((active_panel, previous_source));
                self.queue_panel_refresh(active_panel);
                self.set_status("Panelize running...");
            }
            (Some(PendingDialogAction::PanelizeCommand), DialogResult::Canceled) => {
                self.set_status("Panelize canceled");
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

fn read_entries(dir: &Path, sort_mode: SortMode) -> io::Result<Vec<FileEntry>> {
    read_entries_with_cancel(dir, sort_mode, None)
}

fn read_entries_with_cancel(
    dir: &Path,
    sort_mode: SortMode,
    cancel_flag: Option<&AtomicBool>,
) -> io::Result<Vec<FileEntry>> {
    ensure_panel_refresh_not_canceled(cancel_flag)?;
    let mut entries = Vec::new();
    for entry_result in fs::read_dir(dir)? {
        ensure_panel_refresh_not_canceled(cancel_flag)?;
        let entry = entry_result?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
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
    let output = run_shell_command(base_dir, command)?;
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
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let input_path = PathBuf::from(trimmed);
        let path = if input_path.is_absolute() {
            input_path
        } else {
            base_dir.join(input_path)
        };
        if !seen.insert(path.clone()) {
            continue;
        }

        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        let size = metadata.len();
        let modified = metadata.modified().ok();

        let name = panelized_entry_label(base_dir, &path);
        if metadata.is_dir() {
            entries.push(FileEntry::directory(name, path, size, modified));
        } else {
            entries.push(FileEntry::file(name, path, size, modified));
        }
    }

    sort_file_entries(&mut entries, sort_mode);
    Ok(entries)
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
    entries.sort_by(|left, right| {
        let dir_order = match (left.is_dir, right.is_dir) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => Ordering::Equal,
        };
        if dir_order != Ordering::Equal {
            return dir_order;
        }

        let mut order = match sort_mode.field {
            SortField::Name => left.name.to_lowercase().cmp(&right.name.to_lowercase()),
            SortField::Size => left
                .size
                .cmp(&right.size)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase())),
            SortField::Modified => left
                .modified
                .cmp(&right.modified)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase())),
        };
        if sort_mode.reverse {
            order = order.reverse();
        }
        order
    });
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

#[cfg(unix)]
fn run_shell_command(cwd: &Path, command: &str) -> io::Result<std::process::Output> {
    Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .output()
}

#[cfg(windows)]
fn run_shell_command(cwd: &Path, command: &str) -> io::Result<std::process::Output> {
    Command::new("cmd")
        .arg("/C")
        .arg(command)
        .current_dir(cwd)
        .output()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};
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
            source: PanelListingSource::Directory,
            tagged: HashSet::new(),
            loading: false,
        };

        panel.move_cursor_home();
        assert_eq!(panel.cursor, 0);

        panel.move_cursor_end();
        assert_eq!(panel.cursor, 3);

        panel.move_cursor_page(1);
        assert_eq!(panel.cursor, 3);

        panel.move_cursor_page(-1);
        assert_eq!(panel.cursor, 0);
    }

    #[test]
    fn sort_mode_cycles_and_toggles_direction() {
        let mut panel = PanelState {
            cwd: PathBuf::from("/tmp"),
            entries: Vec::new(),
            cursor: 0,
            sort_mode: SortMode::default(),
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
    fn mkdir_dialog_creates_directory() {
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

        assert!(
            root.join("newdir").exists(),
            "mkdir should create directory"
        );
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

        app.apply(AppCommand::MenuAccept)
            .expect("menu accept should execute selected action");
        assert_eq!(app.key_context(), KeyContext::Jobs);

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
        app.open_panelize_dialog();
        app.finish_dialog(DialogResult::InputSubmitted(String::from(
            "printf 'a.txt\\nsub\\nmissing\\n'",
        )));
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
                .all(|entry| entry.path != root.join("missing")),
            "missing output paths should be ignored"
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
        app.open_panelize_dialog();
        app.finish_dialog(DialogResult::InputSubmitted(String::from("printf ''")));
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

        app.open_panelize_dialog();
        app.finish_dialog(DialogResult::InputSubmitted(String::from("exit 42")));
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
            AppCommand::from_key_command(KeyContext::Listbox, &KeyCommand::CursorUp),
            Some(AppCommand::DialogListboxUp)
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
