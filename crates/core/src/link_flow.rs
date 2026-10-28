use crate::*;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LinkKind {
    Hard,
    Symbolic,
    RelativeSymbolic,
}

impl LinkKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Hard => "Hard link",
            Self::Symbolic => "Symlink",
            Self::RelativeSymbolic => "Relative symlink",
        }
    }
}

impl AppState {
    fn link_operation_paths(&mut self) -> Option<Vec<PathBuf>> {
        if !cfg!(unix) {
            self.set_status("Link operations require Unix");
            return None;
        }
        if self.active_panel().is_virtual() {
            self.set_status("Archives are read-only");
            return None;
        }
        if self.active_panel().loading {
            self.set_status("Wait for the panel to finish loading");
            return None;
        }
        let paths = self.selected_operation_paths();
        if paths.is_empty() {
            self.set_status("Select an entry or tag entries first");
            None
        } else {
            Some(paths)
        }
    }

    pub(crate) fn open_link_dialog(&mut self, kind: LinkKind) {
        let Some(sources) = self.link_operation_paths() else {
            return;
        };
        if self.passive_panel().is_virtual() {
            self.set_status("Links need a local destination; leave the passive archive first");
            return;
        }
        let base = self.passive_panel().cwd.clone();
        self.push_link_dialog(kind, sources, base, self.active_panel, ".".into());
    }

    pub(crate) fn push_link_dialog(
        &mut self,
        kind: LinkKind,
        sources: Vec<PathBuf>,
        base: PathBuf,
        panel: ActivePanel,
        value: String,
    ) {
        let prompt = if sources.len() == 1 {
            "New name/path or existing directory, relative to passive panel; no overwrite:"
        } else {
            "Destination directory, relative to passive panel; original names, no overwrite:"
        };
        self.push_dialog(
            DialogState::input(
                format!("{} ({} entries)", kind.label(), sources.len()),
                prompt,
                value,
            ),
            PendingDialogAction::CreateLinks {
                kind,
                sources,
                base,
                panel,
            },
        );
    }

    pub(crate) fn submit_link_dialog(
        &mut self,
        kind: LinkKind,
        sources: Vec<PathBuf>,
        base: PathBuf,
        panel: ActivePanel,
        value: String,
    ) {
        if value.is_empty() || value.contains('\0') {
            self.push_link_dialog(kind, sources, base, panel, value);
            self.set_status("Enter a destination path");
            return;
        }
        let path = PathBuf::from(rc_shell::decode_path_text(&value));
        let destination = if path.is_absolute() {
            path
        } else {
            base.join(path)
        };
        let id = self.queue_filesystem_job(
            JobRequest::CreateLinks {
                kind,
                sources,
                destination,
            },
            OperationOrigin::Panel(panel),
        );
        self.set_status(format!("Queued {} job #{id}", kind.label()));
    }

    pub(crate) fn open_edit_symlink_dialog(&mut self) {
        let Some(paths) = self.link_operation_paths() else {
            return;
        };
        // Escape bytes that cannot be represented directly in the input field.
        let value = if paths.len() == 1 {
            match fs::read_link(&paths[0]) {
                Ok(target) => rc_shell::encode_path_text(target.as_os_str()),
                Err(error) => {
                    self.set_status(format!("Cannot edit symlink: {error}"));
                    return;
                }
            }
        } else {
            String::new()
        };
        self.push_edit_symlink_dialog(paths, self.active_panel, value);
    }

    pub(crate) fn push_edit_symlink_dialog(
        &mut self,
        paths: Vec<PathBuf>,
        panel: ActivePanel,
        value: String,
    ) {
        self.push_dialog(
            DialogState::input(
                format!("Edit symlink ({} entries)", paths.len()),
                "Literal target for every link; relative paths resolve from each link's directory:",
                value,
            ),
            PendingDialogAction::EditSymlinks { paths, panel },
        );
    }

    pub(crate) fn submit_edit_symlink_dialog(
        &mut self,
        paths: Vec<PathBuf>,
        panel: ActivePanel,
        value: String,
    ) {
        if value.is_empty() || value.contains('\0') {
            self.push_edit_symlink_dialog(paths, panel, value);
            self.set_status("Enter a nonempty literal link target");
            return;
        }
        let id = self.queue_filesystem_job(
            JobRequest::EditSymlinks {
                paths,
                target: PathBuf::from(rc_shell::decode_path_text(&value)),
            },
            OperationOrigin::Panel(panel),
        );
        self.set_status(format!("Queued symlink edit job #{id}"));
    }
}
