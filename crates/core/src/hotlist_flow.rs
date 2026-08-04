use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::*;

#[derive(Debug)]
enum HotlistDirectoryError {
    Missing(PathBuf),
    Inaccessible(PathBuf),
    NotDirectory(PathBuf),
    Unavailable { path: PathBuf, source: io::Error },
}

impl fmt::Display for HotlistDirectoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(path) => {
                write!(formatter, "Hotlist path does not exist: {}", path.display())
            }
            Self::Inaccessible(path) => {
                write!(
                    formatter,
                    "Hotlist path is inaccessible: {}",
                    path.display()
                )
            }
            Self::NotDirectory(path) => {
                write!(
                    formatter,
                    "Hotlist path is not a directory: {}",
                    path.display()
                )
            }
            Self::Unavailable { path, source } => write!(
                formatter,
                "Hotlist path cannot be opened: {} ({source})",
                path.display()
            ),
        }
    }
}

fn map_directory_error(path: &Path, source: io::Error) -> HotlistDirectoryError {
    match source.kind() {
        io::ErrorKind::NotFound => HotlistDirectoryError::Missing(path.to_path_buf()),
        io::ErrorKind::PermissionDenied => HotlistDirectoryError::Inaccessible(path.to_path_buf()),
        _ => HotlistDirectoryError::Unavailable {
            path: path.to_path_buf(),
            source,
        },
    }
}

fn validate_directory(path: &Path) -> Result<PathBuf, HotlistDirectoryError> {
    let metadata = fs::metadata(path).map_err(|error| map_directory_error(path, error))?;
    if !metadata.is_dir() {
        return Err(HotlistDirectoryError::NotDirectory(path.to_path_buf()));
    }

    let canonical = fs::canonicalize(path).map_err(|error| map_directory_error(path, error))?;
    fs::read_dir(&canonical).map_err(|error| map_directory_error(path, error))?;
    Ok(canonical)
}

fn resolve_path(base_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

fn normalized_label(label: &str) -> String {
    label.chars().flat_map(char::to_lowercase).collect()
}

impl AppState {
    pub(crate) fn open_hotlist_screen(&mut self) {
        if !matches!(self.top_route(), Route::Hotlist) {
            self.routes.push(Route::Hotlist);
        }
        self.clamp_hotlist_cursor();
        self.set_status("Opened directory hotlist");
    }

    pub(crate) fn close_hotlist_screen(&mut self) {
        if matches!(self.top_route(), Route::Hotlist) {
            self.routes.pop();
            self.set_status("Closed directory hotlist");
        }
    }

    fn clamp_hotlist_cursor(&mut self) {
        let len = self.settings.configuration.hotlist.len();
        if len == 0 {
            self.hotlist_cursor = 0;
        } else if self.hotlist_cursor >= len {
            self.hotlist_cursor = len - 1;
        }
    }

    pub(crate) fn move_hotlist_cursor(&mut self, delta: isize) {
        let len = self.settings.configuration.hotlist.len();
        if len == 0 {
            self.hotlist_cursor = 0;
            return;
        }
        let last = len - 1;
        let next = if delta.is_negative() {
            self.hotlist_cursor.saturating_sub(delta.unsigned_abs())
        } else {
            self.hotlist_cursor.saturating_add(delta as usize).min(last)
        };
        self.hotlist_cursor = next;
    }

    pub(crate) fn move_hotlist_page(&mut self, pages: isize) {
        self.move_hotlist_cursor(pages.saturating_mul(self.settings.advanced.page_step as isize));
    }

    pub(crate) fn move_hotlist_home(&mut self) {
        self.hotlist_cursor = 0;
    }

    pub(crate) fn move_hotlist_end(&mut self) {
        let len = self.settings.configuration.hotlist.len();
        self.hotlist_cursor = len.saturating_sub(1);
    }

    pub(crate) fn start_hotlist_add_dialog(&mut self) {
        let cwd = self.active_panel().cwd.clone();
        self.push_dialog(
            DialogState::pair_input(
                "Add hotlist entry",
                "Label:",
                HotlistEntry::suggested_label(&cwd),
                "Directory:",
                cwd.to_string_lossy(),
            ),
            PendingDialogAction::HotlistAdd { base_dir: cwd },
        );
        self.set_status("Add hotlist entry: edit label and directory");
    }

    pub(crate) fn start_hotlist_edit_dialog(&mut self) {
        let Some(entry) = self
            .settings
            .configuration
            .hotlist
            .get(self.hotlist_cursor)
            .cloned()
        else {
            self.set_status("No hotlist entry selected");
            return;
        };
        let base_dir = self.active_panel().cwd.clone();
        let label = entry.label.clone();
        let path = entry.path.to_string_lossy().into_owned();
        self.push_dialog(
            DialogState::pair_input("Edit hotlist entry", "Label:", label, "Directory:", path),
            PendingDialogAction::HotlistEdit {
                base_dir,
                index: self.hotlist_cursor,
                original: entry,
            },
        );
        self.set_status("Edit hotlist entry: update label or directory");
    }

    pub(crate) fn submit_hotlist_add(&mut self, base_dir: PathBuf, label: String, path: String) {
        let entry = match self.validate_hotlist_entry(&base_dir, &label, &path, None) {
            Ok(entry) => entry,
            Err(error) => {
                self.reopen_hotlist_editor(
                    "Add hotlist entry",
                    label,
                    path,
                    PendingDialogAction::HotlistAdd { base_dir },
                    error,
                );
                return;
            }
        };

        let label = entry.label.clone();
        self.settings.configuration.hotlist.push(entry);
        self.hotlist_cursor = self.settings.configuration.hotlist.len() - 1;
        self.settings.mark_dirty();
        self.set_status(format!("Added hotlist entry: {label}"));
    }

    pub(crate) fn submit_hotlist_edit(
        &mut self,
        base_dir: PathBuf,
        index: usize,
        original: HotlistEntry,
        label: String,
        path: String,
    ) {
        if self.settings.configuration.hotlist.get(index) != Some(&original) {
            self.clamp_hotlist_cursor();
            self.set_status("Hotlist edit canceled: selection changed");
            return;
        }
        let entry = match self.validate_hotlist_entry(&base_dir, &label, &path, Some(index)) {
            Ok(entry) => entry,
            Err(error) => {
                self.reopen_hotlist_editor(
                    "Edit hotlist entry",
                    label,
                    path,
                    PendingDialogAction::HotlistEdit {
                        base_dir,
                        index,
                        original,
                    },
                    error,
                );
                return;
            }
        };

        let label = entry.label.clone();
        self.settings.configuration.hotlist[index] = entry;
        self.hotlist_cursor = index;
        self.settings.mark_dirty();
        self.set_status(format!("Updated hotlist entry: {label}"));
    }

    fn validate_hotlist_entry(
        &self,
        base_dir: &Path,
        label: &str,
        path: &str,
        edited_index: Option<usize>,
    ) -> Result<HotlistEntry, String> {
        let label = label.trim();
        if label.is_empty() {
            return Err(String::from("Hotlist label cannot be empty"));
        }
        let path = path.trim();
        if path.is_empty() {
            return Err(String::from("Hotlist directory cannot be empty"));
        }

        let resolved = resolve_path(base_dir, Path::new(path));
        let canonical = validate_directory(&resolved).map_err(|error| error.to_string())?;
        let normalized_candidate = normalized_label(label);
        for (index, existing) in self.settings.configuration.hotlist.iter().enumerate() {
            if Some(index) == edited_index {
                continue;
            }
            if normalized_label(existing.label.trim()) == normalized_candidate {
                return Err(format!("Hotlist label already exists: {}", existing.label));
            }
            let existing_path = resolve_path(base_dir, &existing.path);
            if fs::canonicalize(existing_path).is_ok_and(|path| path == canonical) {
                return Err(format!(
                    "Hotlist directory already exists as '{}'",
                    existing.label
                ));
            }
        }

        Ok(HotlistEntry::new(label, canonical))
    }

    fn reopen_hotlist_editor(
        &mut self,
        title: &str,
        label: String,
        path: String,
        action: PendingDialogAction,
        error: String,
    ) {
        self.push_dialog(
            DialogState::pair_input(title, "Label:", label, "Directory:", path),
            action,
        );
        self.set_status(error);
    }

    pub(crate) fn remove_selected_hotlist_entry(&mut self) {
        let Some(entry) = self
            .settings
            .configuration
            .hotlist
            .get(self.hotlist_cursor)
            .cloned()
        else {
            self.set_status("No hotlist entry selected");
            return;
        };
        if self.settings.confirmation.confirm_hotlist_delete {
            self.push_dialog(
                DialogState::confirm(
                    "Remove hotlist entry",
                    format!("Remove '{}' ({})?", entry.label, entry.path.display()),
                ),
                PendingDialogAction::HotlistRemove {
                    index: self.hotlist_cursor,
                    entry,
                },
            );
            self.set_status("Confirm hotlist entry removal");
        } else {
            self.remove_hotlist_entry(self.hotlist_cursor, &entry);
        }
    }

    pub(crate) fn remove_hotlist_entry(&mut self, index: usize, expected: &HotlistEntry) {
        if self.settings.configuration.hotlist.get(index) != Some(expected) {
            self.clamp_hotlist_cursor();
            self.set_status("Hotlist removal canceled: selection changed");
            return;
        }
        let removed = self.settings.configuration.hotlist.remove(index);
        self.clamp_hotlist_cursor();
        self.settings.mark_dirty();
        self.set_status(format!("Removed hotlist entry: {}", removed.label));
    }

    pub(crate) fn open_selected_hotlist_entry(&mut self) -> io::Result<()> {
        let Some(entry) = self
            .settings
            .configuration
            .hotlist
            .get(self.hotlist_cursor)
            .cloned()
        else {
            self.set_status("No hotlist entry selected");
            return Ok(());
        };

        let resolved = resolve_path(&self.active_panel().cwd, &entry.path);
        let destination = match validate_directory(&resolved) {
            Ok(path) => path,
            Err(error) => {
                self.set_status(error.to_string());
                return Ok(());
            }
        };

        if self.set_active_panel_directory(destination)? {
            self.routes.pop();
            self.set_status(format!("Opened {} ({})", entry.label, entry.path.display()));
        } else {
            self.set_status("Hotlist path became unavailable while opening it");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_denied_is_reported_as_inaccessible() {
        let path = Path::new("/restricted");
        let error = map_directory_error(
            path,
            io::Error::new(io::ErrorKind::PermissionDenied, "permission denied"),
        );

        assert!(matches!(error, HotlistDirectoryError::Inaccessible(_)));
        assert_eq!(
            error.to_string(),
            "Hotlist path is inaccessible: /restricted"
        );
    }
}
