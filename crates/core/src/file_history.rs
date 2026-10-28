use crate::*;

const FILE_HISTORY_LIMIT: usize = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FileHistoryEntry {
    path: PathBuf,
    action: FileHistoryAction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum FileHistoryAction {
    View(Option<ArchiveLocation>),
    Edit { cwd: PathBuf },
}

impl AppState {
    pub(crate) fn open_view_file_dialog(&mut self) {
        self.push_dialog(
            DialogState::input(
                "View file",
                "File path (relative to the panel directory):",
                "",
            ),
            PendingDialogAction::ViewFile {
                base_dir: self.active_panel().local_directory().to_path_buf(),
            },
        );
    }

    pub(crate) fn view_file_input(&mut self, base_dir: &Path, value: &str) {
        if value.is_empty() {
            self.set_status("No file path entered");
            return;
        }
        let path = if let Some(suffix) = value.strip_prefix("~/") {
            let Some(home) = current_user_home_directory() else {
                self.set_status("Home directory is unavailable");
                return;
            };
            home.join(rc_shell::decode_path_text(suffix))
        } else {
            base_dir.join(rc_shell::decode_path_text(value))
        };
        self.queue_worker_job_request(JobRequest::LoadViewer { path });
        self.set_status("Opening viewer...");
    }

    pub(crate) fn record_viewed_file(&mut self, path: PathBuf, archive: Option<ArchiveLocation>) {
        self.remember_file(FileHistoryEntry {
            path,
            action: FileHistoryAction::View(archive),
        });
    }

    /// Record a successfully completed external editor invocation in session history.
    pub fn record_edited_file(&mut self, request: &ExternalEditRequest) {
        self.remember_file(FileHistoryEntry {
            path: request.path.clone(),
            action: FileHistoryAction::Edit {
                cwd: request.cwd.clone(),
            },
        });
    }

    fn remember_file(&mut self, entry: FileHistoryEntry) {
        self.file_history.retain(|existing| existing != &entry);
        self.file_history.insert(0, entry);
        self.file_history.truncate(FILE_HISTORY_LIMIT);
    }

    pub(crate) fn open_file_history(&mut self) {
        if self.file_history.is_empty() {
            self.set_status("No viewed or edited files in this session");
            return;
        }
        let entries = self.file_history.clone();
        let labels = entries
            .iter()
            .map(|entry| {
                let action = match entry.action {
                    FileHistoryAction::View(_) => "View",
                    FileHistoryAction::Edit { .. } => "Edit",
                };
                sanitize_display_line(&format!("{action}: {}", entry.path.display()))
            })
            .collect();
        self.push_dialog(
            DialogState::listbox("Viewed/edited files history", labels, 0),
            PendingDialogAction::FileHistory { entries },
        );
    }

    pub(crate) fn reopen_history_entry(&mut self, entry: &FileHistoryEntry) {
        match &entry.action {
            FileHistoryAction::View(archive) => {
                let request = match archive {
                    Some(location) => JobRequest::LoadArchiveViewer {
                        location: location.clone(),
                        path: entry.path.clone(),
                        quick_view: None,
                    },
                    None => JobRequest::LoadViewer {
                        path: entry.path.clone(),
                    },
                };
                self.queue_worker_job_request(request);
                self.set_status("Opening viewer...");
            }
            FileHistoryAction::Edit { cwd } => {
                let Some(editor_command) = resolve_external_editor_command(
                    self.settings.configuration.editor_command.as_deref(),
                ) else {
                    self.set_status("No external editor configured or found");
                    return;
                };
                self.pending_external_edit_requests
                    .push(ExternalEditRequest {
                        editor_command,
                        path: entry.path.clone(),
                        cwd: cwd.clone(),
                    });
                self.set_status("Opening editor...");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_files_are_deduplicated_bounded_and_keep_distinct_actions() {
        let root = tempfile::tempdir().unwrap();
        let mut app = AppState::new(root.path().to_path_buf()).unwrap();
        for index in 0..105 {
            app.record_viewed_file(root.path().join(index.to_string()), None);
        }
        assert_eq!(app.file_history.len(), FILE_HISTORY_LIMIT);
        let path = root.path().join("100");
        app.record_viewed_file(path.clone(), None);
        assert_eq!(app.file_history[0].path, path);
        assert_eq!(app.file_history.len(), FILE_HISTORY_LIMIT);
        app.record_edited_file(&ExternalEditRequest {
            editor_command: "editor".into(),
            path: path.clone(),
            cwd: root.path().to_path_buf(),
        });
        assert!(matches!(
            app.file_history[0].action,
            FileHistoryAction::Edit { .. }
        ));
        assert_eq!(app.file_history[1].path, path);
        assert!(matches!(
            app.file_history[1].action,
            FileHistoryAction::View(None)
        ));
    }
}
