#![forbid(unsafe_code)]

pub mod dialog;
pub mod keymap;

use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub use dialog::{DialogButtonFocus, DialogKind, DialogResult, DialogState};

use crate::dialog::DialogEvent;
use crate::keymap::{KeyCommand, KeyContext};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppCommand {
    Quit,
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
    OpenEntry,
    CdUp,
    Reread,
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
}

impl AppCommand {
    pub fn from_key_command(context: KeyContext, key_command: &KeyCommand) -> Option<Self> {
        match (context, key_command) {
            (_, KeyCommand::Quit) => Some(Self::Quit),
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
            (KeyContext::Listbox, KeyCommand::CursorUp) => Some(Self::DialogListboxUp),
            (KeyContext::Listbox, KeyCommand::CursorDown) => Some(Self::DialogListboxDown),
            (KeyContext::FileManager, KeyCommand::OpenEntry) => Some(Self::OpenEntry),
            (KeyContext::FileManager, KeyCommand::CdUp) => Some(Self::CdUp),
            (KeyContext::FileManager, KeyCommand::Reread) => Some(Self::Reread),
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

#[derive(Clone, Debug)]
pub enum Route {
    FileManager,
    Dialog(DialogState),
}

#[derive(Debug)]
pub struct AppState {
    pub panels: [PanelState; 2],
    pub active_panel: ActivePanel,
    pub status_line: String,
    pub last_dialog_result: Option<DialogResult>,
    routes: Vec<Route>,
}

impl AppState {
    pub fn new(start_path: PathBuf) -> io::Result<Self> {
        let left = PanelState::new(start_path.clone())?;
        let right = PanelState::new(start_path)?;

        Ok(Self {
            panels: [left, right],
            active_panel: ActivePanel::Left,
            status_line: String::from(
                "Ins tag | Alt-* invert | PgUp/PgDn/Home/End nav | F6/F8 sort | F2/F7/F9 dialogs | q quit",
            ),
            last_dialog_result: None,
            routes: vec![Route::FileManager],
        })
    }

    pub fn active_panel(&self) -> &PanelState {
        &self.panels[self.active_panel.index()]
    }

    pub fn active_panel_mut(&mut self) -> &mut PanelState {
        let index = self.active_panel.index();
        &mut self.panels[index]
    }

    pub fn toggle_active_panel(&mut self) {
        self.active_panel.toggle();
    }

    pub fn refresh_active_panel(&mut self) -> io::Result<()> {
        self.active_panel_mut().refresh()
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

    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status_line = message.into();
    }

    pub fn apply(&mut self, command: AppCommand) -> io::Result<ApplyResult> {
        match command {
            AppCommand::Quit => return Ok(ApplyResult::Quit),
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
            AppCommand::OpenEntry => {
                if self.open_selected_directory()? {
                    self.set_status("Opened selected directory");
                } else {
                    self.set_status("Selected entry is not a directory");
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
            AppCommand::OpenConfirmDialog => {
                self.routes.push(Route::Dialog(DialogState::demo_confirm()));
                self.set_status("Opened confirm dialog");
            }
            AppCommand::OpenInputDialog => {
                self.routes.push(Route::Dialog(DialogState::demo_input()));
                self.set_status("Opened input dialog");
            }
            AppCommand::OpenListboxDialog => {
                self.routes.push(Route::Dialog(DialogState::demo_listbox()));
                self.set_status("Opened listbox dialog");
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
            Route::Dialog(dialog) => dialog.key_context(),
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
            self.set_status(result.status_line());
        }
    }
}

fn read_entries(dir: &Path, sort_mode: SortMode) -> io::Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    for entry_result in fs::read_dir(dir)? {
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
    }
}
