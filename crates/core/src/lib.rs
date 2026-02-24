#![forbid(unsafe_code)]

pub mod keymap;

use std::cmp::Ordering;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::keymap::{KeyCommand, KeyContext};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppCommand {
    Quit,
    SwitchPanel,
    MoveUp,
    MoveDown,
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
}

impl FileEntry {
    fn directory(name: String, path: PathBuf) -> Self {
        Self {
            name,
            path,
            is_dir: true,
            is_parent: false,
        }
    }

    fn file(name: String, path: PathBuf) -> Self {
        Self {
            name,
            path,
            is_dir: false,
            is_parent: false,
        }
    }

    fn parent(path: PathBuf) -> Self {
        Self {
            name: String::from(".."),
            path,
            is_dir: true,
            is_parent: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PanelState {
    pub cwd: PathBuf,
    pub entries: Vec<FileEntry>,
    pub cursor: usize,
}

impl PanelState {
    pub fn new(cwd: PathBuf) -> io::Result<Self> {
        let mut panel = Self {
            cwd,
            entries: Vec::new(),
            cursor: 0,
        };
        panel.refresh()?;
        Ok(panel)
    }

    pub fn refresh(&mut self) -> io::Result<()> {
        self.entries = read_entries(&self.cwd)?;
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

    pub fn selected_entry(&self) -> Option<&FileEntry> {
        self.entries.get(self.cursor)
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
        self.refresh()?;
        Ok(true)
    }

    pub fn go_parent(&mut self) -> io::Result<bool> {
        let Some(parent) = self.cwd.parent() else {
            return Ok(false);
        };

        self.cwd = parent.to_path_buf();
        self.cursor = 0;
        self.refresh()?;
        Ok(true)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DialogButtonFocus {
    Ok,
    Cancel,
}

impl DialogButtonFocus {
    pub fn toggle(&mut self) {
        *self = match self {
            Self::Ok => Self::Cancel,
            Self::Cancel => Self::Ok,
        };
    }
}

#[derive(Clone, Debug)]
pub struct ConfirmDialogState {
    pub message: String,
    pub focus: DialogButtonFocus,
}

#[derive(Clone, Debug)]
pub struct InputDialogState {
    pub prompt: String,
    pub value: String,
}

#[derive(Clone, Debug)]
pub struct ListboxDialogState {
    pub items: Vec<String>,
    pub selected: usize,
}

#[derive(Clone, Debug)]
pub enum DialogKind {
    Confirm(ConfirmDialogState),
    Input(InputDialogState),
    Listbox(ListboxDialogState),
}

#[derive(Clone, Debug)]
pub struct DialogState {
    pub title: String,
    pub kind: DialogKind,
}

impl DialogState {
    fn demo_confirm() -> Self {
        Self {
            title: String::from("Confirm"),
            kind: DialogKind::Confirm(ConfirmDialogState {
                message: String::from("Proceed with this action?"),
                focus: DialogButtonFocus::Ok,
            }),
        }
    }

    fn demo_input() -> Self {
        Self {
            title: String::from("Input"),
            kind: DialogKind::Input(InputDialogState {
                prompt: String::from("New name:"),
                value: String::new(),
            }),
        }
    }

    fn demo_listbox() -> Self {
        Self {
            title: String::from("Listbox"),
            kind: DialogKind::Listbox(ListboxDialogState {
                items: vec![
                    String::from("Sort by name"),
                    String::from("Sort by size"),
                    String::from("Sort by mtime"),
                ],
                selected: 0,
            }),
        }
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
                "Tab switch panel | Enter open dir | Backspace up | F2/F7/F9 dialogs | q quit",
            ),
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
                self.open_confirm_dialog();
                self.set_status("Opened confirm dialog");
            }
            AppCommand::OpenInputDialog => {
                self.open_input_dialog();
                self.set_status("Opened input dialog");
            }
            AppCommand::OpenListboxDialog => {
                self.open_listbox_dialog();
                self.set_status("Opened listbox dialog");
            }
            AppCommand::DialogAccept => {
                if let Some(status) = self.accept_dialog() {
                    self.set_status(status);
                }
            }
            AppCommand::DialogCancel => {
                if let Some(status) = self.cancel_dialog() {
                    self.set_status(status);
                }
            }
            AppCommand::DialogFocusNext => {
                self.dialog_focus_next();
            }
            AppCommand::DialogBackspace => {
                self.dialog_input_backspace();
            }
            AppCommand::DialogInputChar(ch) => {
                self.dialog_input_insert(ch);
            }
            AppCommand::DialogListboxUp => {
                self.dialog_listbox_move(-1);
            }
            AppCommand::DialogListboxDown => {
                self.dialog_listbox_move(1);
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
            Route::Dialog(dialog) => match dialog.kind {
                DialogKind::Confirm(_) => KeyContext::Dialog,
                DialogKind::Input(_) => KeyContext::Input,
                DialogKind::Listbox(_) => KeyContext::Listbox,
            },
        }
    }

    pub fn open_confirm_dialog(&mut self) {
        self.routes.push(Route::Dialog(DialogState::demo_confirm()));
    }

    pub fn open_input_dialog(&mut self) {
        self.routes.push(Route::Dialog(DialogState::demo_input()));
    }

    pub fn open_listbox_dialog(&mut self) {
        self.routes.push(Route::Dialog(DialogState::demo_listbox()));
    }

    pub fn dialog_focus_next(&mut self) -> bool {
        let Some(Route::Dialog(dialog)) = self.routes.last_mut() else {
            return false;
        };

        match &mut dialog.kind {
            DialogKind::Confirm(confirm) => {
                confirm.focus.toggle();
                true
            }
            _ => false,
        }
    }

    pub fn dialog_listbox_move(&mut self, delta: isize) -> bool {
        let Some(Route::Dialog(dialog)) = self.routes.last_mut() else {
            return false;
        };
        let DialogKind::Listbox(listbox) = &mut dialog.kind else {
            return false;
        };
        if listbox.items.is_empty() {
            listbox.selected = 0;
            return true;
        }

        let last = listbox.items.len() - 1;
        let next = if delta.is_negative() {
            listbox.selected.saturating_sub(delta.unsigned_abs())
        } else {
            listbox.selected.saturating_add(delta as usize).min(last)
        };
        listbox.selected = next;
        true
    }

    pub fn dialog_input_insert(&mut self, ch: char) -> bool {
        let Some(Route::Dialog(dialog)) = self.routes.last_mut() else {
            return false;
        };
        let DialogKind::Input(input) = &mut dialog.kind else {
            return false;
        };

        input.value.push(ch);
        true
    }

    pub fn dialog_input_backspace(&mut self) -> bool {
        let Some(Route::Dialog(dialog)) = self.routes.last_mut() else {
            return false;
        };
        let DialogKind::Input(input) = &mut dialog.kind else {
            return false;
        };

        input.value.pop();
        true
    }

    pub fn accept_dialog(&mut self) -> Option<String> {
        let dialog = self.pop_dialog()?;
        let status = match dialog.kind {
            DialogKind::Confirm(confirm) => match confirm.focus {
                DialogButtonFocus::Ok => String::from("Dialog accepted"),
                DialogButtonFocus::Cancel => String::from("Dialog canceled"),
            },
            DialogKind::Input(input) => format!("Input accepted: {}", input.value),
            DialogKind::Listbox(listbox) => {
                if listbox.items.is_empty() {
                    String::from("Listbox accepted: <empty>")
                } else {
                    format!("Listbox accepted: {}", listbox.items[listbox.selected])
                }
            }
        };
        Some(status)
    }

    pub fn cancel_dialog(&mut self) -> Option<String> {
        self.pop_dialog()?;
        Some(String::from("Dialog canceled"))
    }

    fn pop_dialog(&mut self) -> Option<DialogState> {
        let Some(Route::Dialog(_)) = self.routes.last() else {
            return None;
        };
        match self.routes.pop() {
            Some(Route::Dialog(dialog)) => Some(dialog),
            _ => None,
        }
    }
}

fn read_entries(dir: &Path) -> io::Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    for entry_result in fs::read_dir(dir)? {
        let entry = entry_result?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            entries.push(FileEntry::directory(name, path));
        } else {
            entries.push(FileEntry::file(name, path));
        }
    }

    entries.sort_by(|left, right| match (left.is_dir, right.is_dir) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => left.name.to_lowercase().cmp(&right.name.to_lowercase()),
    });

    if let Some(parent) = dir.parent() {
        entries.insert(0, FileEntry::parent(parent.to_path_buf()));
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use std::{env, fs};

    fn file_entry(name: &str) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: PathBuf::from(name),
            is_dir: false,
            is_parent: false,
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
    }
}
