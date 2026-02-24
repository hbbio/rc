#![forbid(unsafe_code)]

pub mod dialog;
pub mod jobs;
pub mod keymap;

use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub use dialog::{DialogButtonFocus, DialogKind, DialogResult, DialogState};
pub use jobs::{
    JOB_CANCELED_MESSAGE, JobEvent, JobId, JobKind, JobManager, JobProgress, JobRecord, JobRequest,
    JobStatus, JobStatusCounts, OverwritePolicy, WorkerCommand, WorkerJob, run_worker,
};

use crate::dialog::DialogEvent;
use crate::keymap::{KeyCommand, KeyContext};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppCommand {
    Quit,
    CloseViewer,
    OpenFindDialog,
    CloseFindResults,
    OpenTree,
    CloseTree,
    OpenHotlist,
    CloseHotlist,
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
}

impl AppCommand {
    pub fn from_key_command(context: KeyContext, key_command: &KeyCommand) -> Option<Self> {
        match (context, key_command) {
            (KeyContext::FileManager, KeyCommand::Quit) => Some(Self::Quit),
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
            (KeyContext::FileManager, KeyCommand::OpenTree) => Some(Self::OpenTree),
            (KeyContext::Tree, KeyCommand::CursorUp) => Some(Self::TreeMoveUp),
            (KeyContext::Tree, KeyCommand::CursorDown) => Some(Self::TreeMoveDown),
            (KeyContext::Tree, KeyCommand::PageUp) => Some(Self::TreePageUp),
            (KeyContext::Tree, KeyCommand::PageDown) => Some(Self::TreePageDown),
            (KeyContext::Tree, KeyCommand::Home) => Some(Self::TreeHome),
            (KeyContext::Tree, KeyCommand::End) => Some(Self::TreeEnd),
            (KeyContext::Tree, KeyCommand::OpenEntry) => Some(Self::TreeOpenEntry),
            (KeyContext::FileManager, KeyCommand::OpenHotlist) => Some(Self::OpenHotlist),
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
            (KeyContext::Viewer, KeyCommand::Search) => Some(Self::ViewerSearchForward),
            (KeyContext::Viewer, KeyCommand::SearchBackward) => Some(Self::ViewerSearchBackward),
            (KeyContext::Viewer, KeyCommand::SearchContinue) => Some(Self::ViewerSearchContinue),
            (KeyContext::Viewer, KeyCommand::SearchContinueBackward) => {
                Some(Self::ViewerSearchContinueBackward)
            }
            (KeyContext::Viewer, KeyCommand::Goto) => Some(Self::ViewerGoto),
            (KeyContext::Viewer, KeyCommand::ToggleWrap) => Some(Self::ViewerToggleWrap),
            (KeyContext::FileManager, KeyCommand::OpenConfirmDialog) => {
                Some(Self::OpenConfirmDialog)
            }
            (KeyContext::FileManager, KeyCommand::OpenInputDialog) => Some(Self::OpenInputDialog),
            (KeyContext::FileManager, KeyCommand::OpenListboxDialog) => {
                Some(Self::OpenListboxDialog)
            }
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
pub enum ApplyResult {
    Continue,
    Quit,
}

const DEFAULT_PAGE_STEP: usize = 10;
const DEFAULT_VIEWER_PAGE_STEP: usize = 20;
const MAX_FIND_RESULTS: usize = 2_000;
const TREE_MAX_DEPTH: usize = 6;
const TREE_MAX_ENTRIES: usize = 2_000;

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

#[derive(Clone, Debug)]
pub struct PanelState {
    pub cwd: PathBuf,
    pub entries: Vec<FileEntry>,
    pub cursor: usize,
    pub sort_mode: SortMode,
    tagged: HashSet<PathBuf>,
}

impl PanelState {
    pub fn new(cwd: PathBuf) -> io::Result<Self> {
        let mut panel = Self {
            cwd,
            entries: Vec::new(),
            cursor: 0,
            sort_mode: SortMode::default(),
            tagged: HashSet::new(),
        };
        panel.refresh()?;
        Ok(panel)
    }

    pub fn refresh(&mut self) -> io::Result<()> {
        self.entries = read_entries(&self.cwd, self.sort_mode)?;
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
        Ok(())
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

    pub fn cycle_sort_field(&mut self) -> io::Result<()> {
        self.sort_mode.field = self.sort_mode.field.next();
        self.refresh()
    }

    pub fn toggle_sort_direction(&mut self) -> io::Result<()> {
        self.sort_mode.reverse = !self.sort_mode.reverse;
        self.refresh()
    }

    pub fn open_selected_directory(&mut self) -> io::Result<bool> {
        let Some(entry) = self.selected_entry() else {
            return Ok(false);
        };
        if !entry.is_dir {
            return Ok(false);
        }

        self.cwd = entry.path.clone();
        self.cursor = 0;
        self.tagged.clear();
        self.refresh()?;
        Ok(true)
    }

    pub fn go_parent(&mut self) -> io::Result<bool> {
        let Some(parent) = self.cwd.parent() else {
            return Ok(false);
        };

        self.cwd = parent.to_path_buf();
        self.cursor = 0;
        self.tagged.clear();
        self.refresh()?;
        Ok(true)
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
    pub content: String,
    pub scroll: usize,
    pub wrap: bool,
    line_offsets: Vec<usize>,
    last_search_query: Option<String>,
    last_search_match_offset: Option<usize>,
    last_search_direction: ViewerSearchDirection,
}

impl ViewerState {
    pub fn open(path: PathBuf) -> io::Result<Self> {
        let bytes = fs::read(&path)?;
        let content = String::from_utf8_lossy(&bytes).into_owned();
        let line_offsets = compute_line_offsets(&content);

        Ok(Self {
            path,
            content,
            scroll: 0,
            wrap: false,
            line_offsets,
            last_search_query: None,
            last_search_match_offset: None,
            last_search_direction: ViewerSearchDirection::Forward,
        })
    }

    pub fn line_count(&self) -> usize {
        self.line_offsets.len()
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
                let bounded = offset.min(self.content.len());
                self.scroll = self.line_index_for_offset(bounded);
            }
        }
        Ok(self.current_line_number())
    }

    fn current_line_offset(&self) -> usize {
        let index = self.scroll.min(self.line_count().saturating_sub(1));
        self.line_offsets[index]
    }

    fn line_index_for_offset(&self, offset: usize) -> usize {
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FindResultEntry {
    pub path: PathBuf,
    pub is_dir: bool,
}

#[derive(Clone, Debug)]
pub struct FindResultsState {
    pub query: String,
    pub base_dir: PathBuf,
    pub entries: Vec<FindResultEntry>,
    pub cursor: usize,
}

impl FindResultsState {
    fn new(query: String, base_dir: PathBuf, entries: Vec<FindResultEntry>) -> Self {
        Self {
            query,
            base_dir,
            entries,
            cursor: 0,
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
}

impl TreeState {
    fn new(root: PathBuf, entries: Vec<TreeEntry>) -> Self {
        Self {
            root,
            entries,
            cursor: 0,
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

#[derive(Clone, Debug)]
pub enum Route {
    FileManager,
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
    ViewerSearch {
        direction: ViewerSearchDirection,
    },
    ViewerGoto,
    FindQuery {
        base_dir: PathBuf,
    },
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
    routes: Vec<Route>,
    pending_dialog_action: Option<PendingDialogAction>,
    pending_worker_commands: Vec<WorkerCommand>,
}

impl AppState {
    pub fn new(start_path: PathBuf) -> io::Result<Self> {
        let left = PanelState::new(start_path.clone())?;
        let right = PanelState::new(start_path)?;

        Ok(Self {
            panels: [left, right],
            active_panel: ActivePanel::Left,
            status_line: String::from(
                "F2 rename | F3/Enter view | Alt-F find | Alt-T tree | Alt-H hotlist | Ctrl-J jobs | F5 copy | F6 move | F7 mkdir | F8 delete | F9 policy | Alt-J cancel job | q quit",
            ),
            last_dialog_result: None,
            jobs: JobManager::new(),
            overwrite_policy: OverwritePolicy::Skip,
            jobs_cursor: 0,
            hotlist: Vec::new(),
            hotlist_cursor: 0,
            routes: vec![Route::FileManager],
            pending_dialog_action: None,
            pending_worker_commands: Vec::new(),
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

    pub fn refresh_active_panel(&mut self) -> io::Result<()> {
        self.active_panel_mut().refresh()
    }

    pub fn refresh_panels(&mut self) -> io::Result<()> {
        for panel in &mut self.panels {
            panel.refresh()?;
        }
        Ok(())
    }

    pub fn move_cursor(&mut self, delta: isize) {
        self.active_panel_mut().move_cursor(delta);
    }

    pub fn open_selected_directory(&mut self) -> io::Result<bool> {
        self.active_panel_mut().open_selected_directory()
    }

    pub fn go_parent_directory(&mut self) -> io::Result<bool> {
        self.active_panel_mut().go_parent()
    }

    fn open_selected_file_in_viewer(&mut self) -> io::Result<bool> {
        let Some(entry) = self.selected_non_parent_entry() else {
            return Ok(false);
        };
        if entry.is_dir {
            return Ok(false);
        }

        let viewer = ViewerState::open(entry.path.clone())?;
        self.routes.push(Route::Viewer(viewer));
        Ok(true)
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

    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status_line = message.into();
    }

    pub fn take_pending_worker_commands(&mut self) -> Vec<WorkerCommand> {
        std::mem::take(&mut self.pending_worker_commands)
    }

    pub fn handle_job_event(&mut self, event: JobEvent) {
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
                    if let Err(error) = self.refresh_panels() {
                        self.set_status(format!("Job #{id} finished, refresh failed: {error}"));
                        return;
                    }
                    if let Some(job) = self.jobs.jobs().iter().find(|job| job.id == id) {
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

    pub fn handle_job_dispatch_failure(&mut self, id: JobId, error: String) {
        self.handle_job_event(JobEvent::Finished {
            id,
            result: Err(error),
        });
    }

    pub fn jobs_status_counts(&self) -> JobStatusCounts {
        self.jobs.status_counts()
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
        let base_dir = self.active_panel().cwd.clone();
        self.pending_dialog_action = Some(PendingDialogAction::FindQuery { base_dir });
        self.routes.push(Route::Dialog(DialogState::input(
            "Find file",
            "Name contains:",
            "",
        )));
        self.set_status("Find file");
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

        if selected.is_dir {
            if self.set_active_panel_directory(selected.path.clone())? {
                self.routes.pop();
                self.set_status(format!(
                    "Opened directory {}",
                    selected.path.to_string_lossy()
                ));
            } else {
                self.set_status("Selected result is not an accessible directory");
            }
            return Ok(());
        }

        match ViewerState::open(selected.path.clone()) {
            Ok(viewer) => {
                self.routes.pop();
                self.routes.push(Route::Viewer(viewer));
                self.set_status(format!("Opened viewer {}", selected.path.to_string_lossy()));
            }
            Err(error) => {
                self.set_status(format!("Viewer open failed: {error}"));
            }
        }
        Ok(())
    }

    fn open_tree_screen(&mut self) {
        if matches!(self.top_route(), Route::Tree(_)) {
            return;
        }
        let root = self.active_panel().cwd.clone();
        let entries = build_tree_entries(&root, TREE_MAX_DEPTH, TREE_MAX_ENTRIES);
        self.routes.push(Route::Tree(TreeState::new(root, entries)));
        self.set_status("Opened directory tree");
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
        panel.tagged.clear();
        panel.refresh()?;
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
        match command {
            AppCommand::Quit => return Ok(ApplyResult::Quit),
            AppCommand::CloseViewer => self.close_viewer(),
            AppCommand::OpenFindDialog => self.open_find_dialog(),
            AppCommand::CloseFindResults => self.close_find_results(),
            AppCommand::OpenTree => self.open_tree_screen(),
            AppCommand::CloseTree => self.close_tree_screen(),
            AppCommand::OpenHotlist => self.open_hotlist_screen(),
            AppCommand::CloseHotlist => self.close_hotlist_screen(),
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
                self.active_panel_mut().cycle_sort_field()?;
                let label = self.active_panel().sort_label();
                self.set_status(format!("Sort: {label}"));
            }
            AppCommand::SortReverse => {
                self.active_panel_mut().toggle_sort_direction()?;
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
                if self.open_selected_directory()? {
                    self.set_status("Opened selected directory");
                } else if self.open_selected_file_in_viewer()? {
                    self.set_status("Opened viewer");
                } else {
                    self.set_status("No entry selected");
                }
            }
            AppCommand::CdUp => {
                if self.go_parent_directory()? {
                    self.set_status("Moved to parent directory");
                } else {
                    self.set_status("Already at filesystem root");
                }
            }
            AppCommand::Reread => {
                self.refresh_active_panel()?;
                self.set_status("Refreshed active panel");
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
            Route::FileManager => KeyContext::FileManager,
            Route::Jobs => KeyContext::Jobs,
            Route::Viewer(_) => KeyContext::Viewer,
            Route::FindResults(_) => KeyContext::FindResults,
            Route::Tree(_) => KeyContext::Tree,
            Route::Hotlist => KeyContext::Hotlist,
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

        if self.jobs.request_cancel(job_id) {
            self.pending_worker_commands
                .push(WorkerCommand::Cancel(job_id));
            self.set_status(format!("Cancellation requested for job #{job_id}"));
        } else {
            self.set_status(format!("Job #{job_id} cannot be canceled"));
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
                        if let Err(error) = self.refresh_active_panel() {
                            self.set_status(format!(
                                "Directory created, but refresh failed: {error}"
                            ));
                        } else {
                            self.set_status(format!(
                                "Created directory {}",
                                destination.to_string_lossy()
                            ));
                        }
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
                        if let Err(error) = self.refresh_panels() {
                            self.set_status(format!("Renamed entry, but refresh failed: {error}"));
                        } else {
                            self.set_status(format!(
                                "Renamed to {}",
                                destination.to_string_lossy()
                            ));
                        }
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
                Some(PendingDialogAction::FindQuery { base_dir }),
                DialogResult::InputSubmitted(value),
            ) => {
                let query = value.trim();
                if query.is_empty() {
                    self.set_status("Find canceled: empty query");
                    return;
                }

                let results = find_entries(&base_dir, query, MAX_FIND_RESULTS);
                let result_count = results.len();
                self.routes.push(Route::FindResults(FindResultsState::new(
                    query.to_string(),
                    base_dir,
                    results,
                )));
                self.set_status(format!("Find '{query}': {result_count} result(s)"));
            }
            (Some(PendingDialogAction::FindQuery { .. }), DialogResult::Canceled) => {
                self.set_status("Find canceled");
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
        let Some(Route::Dialog(dialog)) = self.routes.last_mut() else {
            return;
        };
        let transition = dialog.handle_event(event);
        if let dialog::DialogTransition::Close(result) = transition {
            self.routes.pop();
            self.last_dialog_result = Some(result.clone());
            self.finish_dialog(result);
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

fn find_entries(base_dir: &Path, query: &str, max_results: usize) -> Vec<FindResultEntry> {
    if max_results == 0 {
        return Vec::new();
    }

    let normalized_query = query.to_lowercase();
    if normalized_query.is_empty() {
        return Vec::new();
    }

    let mut results = Vec::new();
    let mut stack = vec![base_dir.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let read_dir = match fs::read_dir(&dir) {
            Ok(read_dir) => read_dir,
            Err(_) => continue,
        };
        let mut child_dirs = Vec::new();

        for entry in read_dir.flatten() {
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = file_type.is_dir();

            if name.to_lowercase().contains(&normalized_query) {
                results.push(FindResultEntry {
                    path: path.clone(),
                    is_dir,
                });
                if results.len() >= max_results {
                    return results;
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

    results
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
    let mut entries = Vec::new();
    let include_metadata = !matches!(sort_mode.field, SortField::Name);
    for entry_result in fs::read_dir(dir)? {
        let entry = entry_result?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let file_type = entry.file_type()?;
        let (size, modified) = if include_metadata {
            let metadata = entry.metadata().ok();
            (
                metadata.as_ref().map_or(0, std::fs::Metadata::len),
                metadata.as_ref().and_then(|meta| meta.modified().ok()),
            )
        } else {
            (0, None)
        };
        if file_type.is_dir() {
            entries.push(FileEntry::directory(name, path, size, modified));
        } else {
            entries.push(FileEntry::file(name, path, size, modified));
        }
    }

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

    if let Some(parent) = dir.parent() {
        entries.insert(0, FileEntry::parent(parent.to_path_buf()));
    }
    Ok(entries)
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
            tagged: HashSet::new(),
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
    fn name_sort_listing_omits_metadata_population() {
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
        assert_eq!(file_entry.size, 0, "name sort should skip size metadata");
        assert_eq!(
            file_entry.modified, None,
            "name sort should skip mtime metadata"
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
            tagged: HashSet::new(),
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
            tagged: HashSet::new(),
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
            tagged: HashSet::new(),
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
    fn find_dialog_builds_results_and_opens_selected_entry() {
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
        assert_eq!(app.key_context(), KeyContext::FindResults);

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
        let Route::Viewer(viewer) = app.top_route() else {
            panic!("top route should be viewer");
        };
        assert_eq!(viewer.path, target);

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

    #[test]
    fn app_command_mapping_is_context_aware() {
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
    }
}
