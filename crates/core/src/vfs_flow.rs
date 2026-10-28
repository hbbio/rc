use crate::vfs::{Mount, VfsPath, tasks::Task};
use crate::*;
use std::collections::BTreeMap;
#[cfg(unix)]
use std::ffi::OsStr;

const MOUNT_LIMIT: usize = 16;

#[derive(Debug)]
struct PendingMount {
    panel: ActivePanel,
    from: VfsPath,
    refresh_id: u64,
    directory: PathBuf,
}

#[derive(Debug, Default)]
pub(crate) struct VfsWorkflow {
    mounts: BTreeMap<u64, Arc<Mount>>,
    pending: HashMap<JobId, PendingMount>,
    jobs: HashMap<JobId, u64>,
    reconnects: HashMap<JobId, u64>,
}

impl AppState {
    fn ssh_link_enabled(&self, shell: bool) -> bool {
        self.settings.virtual_fs.vfs_enabled
            && if shell {
                self.settings.virtual_fs.shell_link_enabled
            } else {
                self.settings.virtual_fs.sftp_enabled
            }
    }
    pub(crate) fn open_ssh_dialog(&mut self, panel: ActivePanel, shell: bool) {
        let label = if shell { "Shell" } else { "SFTP" };
        if !self.ssh_link_enabled(shell) {
            self.set_status(format!(
                "Enable VFS and {label} links in Options → Virtual FS first"
            ));
            return;
        }
        self.push_dialog(
            DialogState::pair_input(
                format!("{label} link (SSH keys / agent)"),
                "Address [user@]host[:port]:",
                "",
                "Directory:",
                ".",
            ),
            PendingDialogAction::SshConnection { panel, shell },
        );
    }
    pub(crate) fn submit_ssh_connection(
        &mut self,
        panel: ActivePanel,
        shell: bool,
        address: &str,
        directory: &str,
    ) {
        #[cfg(unix)]
        {
            let label = if shell { "Shell" } else { "SFTP" };
            if !self.ssh_link_enabled(shell) {
                self.set_status(format!(
                    "{label} links are disabled in Options → Virtual FS"
                ));
                return;
            }
            let endpoint = match vfs::ssh::Endpoint::parse(address) {
                Ok(endpoint) => endpoint,
                Err(error) => {
                    self.push_dialog(
                        DialogState::pair_input(
                            format!("{label} link"),
                            "Address [user@]host[:port]:",
                            address,
                            "Directory:",
                            directory,
                        ),
                        PendingDialogAction::SshConnection { panel, shell },
                    );
                    self.set_status(error.to_string());
                    return;
                }
            };
            let directory = PathBuf::from(rc_shell::decode_path_text(directory));
            let return_to = self.panels[panel.index()].location();
            let connection = vfs::sftp::Connection {
                endpoint,
                directory,
            };
            self.queue_mount_task(
                panel,
                if shell {
                    Task::OpenShell {
                        connection,
                        return_to,
                    }
                } else {
                    Task::OpenSftp {
                        connection,
                        return_to,
                    }
                },
                PathBuf::new(),
            );
        }
        #[cfg(not(unix))]
        {
            let _ = (panel, shell, address, directory);
            self.set_status("SSH links require Unix OpenSSH");
        }
    }
    pub(crate) fn cancel_vfs_open(&mut self, job: JobId) {
        self.vfs_workflow.pending.remove(&job);
        self.vfs_workflow.reconnects.remove(&job);
    }
    pub(crate) fn track_vfs_job(&mut self, id: JobId, request: &JobRequest) {
        if let JobRequest::Vfs(Task::Reconnect { location }) = request {
            self.vfs_workflow
                .reconnects
                .insert(id, location.mount().id());
        }
        let mount = match request {
            JobRequest::Vfs(task) => task.mount_id(),
            JobRequest::RefreshPanel {
                source: PanelListingSource::Mounted(path),
                ..
            } => Some(path.mount().id()),
            _ => None,
        };
        self.vfs_workflow.jobs.remove(&id);
        if let Some(mount) = mount {
            self.vfs_workflow.jobs.insert(id, mount);
        }
    }
    pub(crate) fn finish_vfs_job(&mut self, id: JobId, result: &Result<(), JobError>) {
        self.vfs_workflow.jobs.remove(&id);
        if let Some(mount_id) = self.vfs_workflow.reconnects.remove(&id)
            && let Some(mount) = self
                .vfs_workflow
                .mounts
                .get(&mount_id)
                .filter(|mount| !mount.is_closed())
        {
            match result {
                Ok(()) => {
                    self.set_status(format!("Reconnected {}", mount.label()));
                    for panel in [ActivePanel::Left, ActivePanel::Right] {
                        if self.panels[panel.index()]
                            .mounted_location()
                            .is_some_and(|path| path.mount().id() == mount_id)
                        {
                            self.queue_panel_refresh(panel);
                        }
                    }
                }
                Err(error) => self.set_status(format!("Cannot reconnect VFS: {}", error.message)),
            }
        }
        if let Err(error) = result
            && self.vfs_workflow.pending.remove(&id).is_some()
        {
            self.set_status(format!("Cannot open VFS: {}", error.message));
        }
    }
    pub(crate) fn cancel_pending_mount(&mut self, panel: ActivePanel) {
        let ids: Vec<_> = self
            .vfs_workflow
            .pending
            .iter()
            .filter(|(_, pending)| pending.panel == panel)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.vfs_workflow.pending.remove(&id);
            self.request_cancel_for_job(id);
        }
    }
    #[cfg(unix)]
    fn queue_helper_mount(
        &mut self,
        panel: ActivePanel,
        helper: vfs::extfs::Helper,
        source: Option<VfsPath>,
        directory: PathBuf,
    ) {
        let from = self.panels[panel.index()].location();
        let return_to = source
            .as_ref()
            .and_then(|source| source.parent().ok().flatten())
            .unwrap_or(from);
        self.queue_mount_task(
            panel,
            Task::OpenHelper {
                helper,
                source,
                return_to,
            },
            directory,
        );
    }

    #[cfg(unix)]
    fn queue_mount_task(&mut self, panel: ActivePanel, task: Task, directory: PathBuf) {
        if !self.settings.virtual_fs.vfs_enabled {
            self.set_status("Enable VFS in Options → Virtual FS first");
            return;
        }
        if self.vfs_workflow.mounts.len() >= MOUNT_LIMIT {
            self.set_status("Close a mount in Active VFS list before opening another (limit 16)");
            return;
        }
        if let Err(error) = vfs::validate_relative(&directory) {
            self.set_status(error.to_string());
            return;
        }
        let from = self.panels[panel.index()].location();
        self.cancel_pending_mount(panel);
        let refresh_id = self.panel_refresh_generation(panel);
        let id = self.queue_worker_job_request(JobRequest::Vfs(task));
        self.vfs_workflow.pending.insert(
            id,
            PendingMount {
                panel,
                from,
                refresh_id,
                directory,
            },
        );
        self.set_status("Opening VFS connection...");
    }

    /// Returns true once a helper URI has been handled, including an error.
    #[cfg(unix)]
    pub(crate) fn open_helper_uri(&mut self, panel: ActivePanel, path: &Path) -> bool {
        if !path
            .as_os_str()
            .as_encoded_bytes()
            .windows(3)
            .any(|part| part == b"://")
        {
            return false;
        }
        self.open_helper_uri_using(panel, path, vfs::extfs::discover())
    }
    #[cfg(unix)]
    fn open_helper_uri_using(
        &mut self,
        panel: ActivePanel,
        path: &Path,
        helpers: Vec<vfs::extfs::Helper>,
    ) -> bool {
        use std::os::unix::ffi::OsStrExt;
        let bytes = path.as_os_str().as_bytes();
        let Some(separator) = bytes.windows(3).rposition(|part| part == b"://") else {
            return false;
        };
        let start = bytes[..separator]
            .iter()
            .rposition(|byte| *byte == b'/')
            .map_or(0, |index| index + 1);
        let Ok(prefix) = std::str::from_utf8(&bytes[start..separator]) else {
            return false;
        };
        let Some(helper) = helpers.into_iter().find(|helper| helper.name() == prefix) else {
            self.set_status(format!("No installed extfs helper for {prefix}"));
            return true;
        };
        let source = if helper.needs_archive() {
            if start == 0 {
                self.set_status("This helper requires a container path before its prefix");
                return true;
            }
            let source = PathBuf::from(OsStr::from_bytes(&bytes[..start - 1]));
            match self.panels[panel.index()].location().resolve_input(&source) {
                Ok(source) => Some(source),
                Err(error) => {
                    self.set_status(error.to_string());
                    return true;
                }
            }
        } else {
            None
        };
        let directory = PathBuf::from(OsStr::from_bytes(&bytes[separator + 3..]));
        self.queue_helper_mount(panel, helper, source, directory);
        true
    }
    #[cfg(not(unix))]
    pub(crate) fn open_helper_uri(&mut self, _: ActivePanel, _: &Path) -> bool {
        false
    }

    pub(crate) fn open_selected_helper(&mut self) -> bool {
        #[cfg(unix)]
        {
            let Some(entry) = self
                .selected_non_parent_entry()
                .filter(|entry| !entry.is_dir())
            else {
                return false;
            };
            let Some(extension) = entry.path.extension().and_then(OsStr::to_str) else {
                return false;
            };
            let prefix = match extension.to_ascii_lowercase().as_str() {
                "7z" => "u7z",
                "rar" => "urar",
                "rpm" => "rpm",
                "deb" => "deb",
                "ar" => "uar",
                "cpio" => "ucpio",
                _ => return false,
            };
            let Some(helper) = vfs::extfs::discover()
                .into_iter()
                .find(|helper| helper.name() == prefix && helper.needs_archive())
            else {
                return false;
            };
            let Ok(source) = self
                .active_panel()
                .location()
                .resolve_identity(entry.path.clone())
            else {
                return false;
            };
            self.queue_helper_mount(self.active_panel, helper, Some(source), PathBuf::new());
            true
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    pub(crate) fn handle_vfs_mounted(&mut self, job_id: JobId, mount: Arc<Mount>) {
        let Some(pending) = self.vfs_workflow.pending.remove(&job_id) else {
            mount.close();
            return;
        };
        if self.panels[pending.panel.index()].location() != pending.from
            || self.panel_refresh_generation(pending.panel) != pending.refresh_id
            || self.vfs_workflow.mounts.len() >= MOUNT_LIMIT
        {
            mount.close();
            return;
        }
        let VfsPath::Mounted(root) = mount.root() else {
            unreachable!()
        };
        let Ok(path) = root.at(pending.directory) else {
            mount.close();
            return;
        };
        self.vfs_workflow
            .mounts
            .insert(mount.id(), Arc::clone(&mount));
        self.enter_vfs_location(pending.panel, VfsPath::Mounted(path), true);
        self.set_status(format!("Opened {}", mount.label()));
    }

    pub(crate) fn enter_vfs_location(
        &mut self,
        panel: ActivePanel,
        location: VfsPath,
        allow_revert: bool,
    ) {
        let previous = self.panels[panel.index()].local_directory().to_path_buf();
        let revert = self.panel_refresh_revert_snapshot(panel);
        if let Some(snapshot) = self.completed_panelized_result_snapshot(panel) {
            self.panelized_result_history[panel.index()] = Some(snapshot);
        }
        if let Err(error) = self.panels[panel.index()].set_location(location) {
            self.set_status(error.to_string());
            return;
        }
        if allow_revert {
            self.schedule_panel_refresh_revert(panel, revert);
        }
        self.remember_previous_directory(panel, previous);
        self.set_panel_view_mode(panel, PanelViewMode::Listing);
        self.sync_quick_view_from(panel, true);
        self.sync_selection_size(panel, true);
        self.queue_panel_refresh(panel);
    }

    pub(crate) fn open_active_vfs(&mut self) {
        let mounts: Vec<_> = self
            .vfs_workflow
            .mounts
            .values()
            .filter(|mount| !mount.is_closed())
            .cloned()
            .collect();
        if mounts.is_empty() {
            self.set_status("No active VFS mounts");
            return;
        }
        let labels = mounts
            .iter()
            .map(|mount| {
                sanitize_display_line(&format!(
                    "{}  {}  (read-only)",
                    mount.label(),
                    mount.root().identity().display()
                ))
            })
            .collect();
        self.push_dialog(
            DialogState::listbox("Active VFS list", labels, 0),
            PendingDialogAction::VfsList { mounts },
        );
    }

    pub(crate) fn inspect_vfs_mount(&mut self, mount: Arc<Mount>) {
        if mount.is_closed() {
            self.set_status("This VFS mount has been closed");
            return;
        }
        let title = sanitize_display_line(&format!(
            "{}: {}",
            mount.label(),
            mount.root().identity().display()
        ));
        let mut labels = vec![
            "Open in left panel".into(),
            "Open in right panel".into(),
            "Close mount".into(),
        ];
        if matches!(
            mount.kind(),
            vfs::BackendKind::Sftp | vfs::BackendKind::Shell | vfs::BackendKind::Ftp
        ) {
            labels.push("Reconnect".into());
        }
        self.push_dialog(
            DialogState::listbox(title, labels, 0),
            PendingDialogAction::VfsMount { mount },
        );
    }

    pub(crate) fn act_on_vfs_mount(&mut self, mount: Arc<Mount>, index: usize) {
        if mount.is_closed() {
            self.set_status("This VFS mount has been closed");
            return;
        }
        match index {
            0 | 1 => self.enter_vfs_location(
                if index == 0 {
                    ActivePanel::Left
                } else {
                    ActivePanel::Right
                },
                mount.root(),
                true,
            ),
            2 => self.close_vfs_mount(mount.id()),
            3 if matches!(
                mount.kind(),
                vfs::BackendKind::Sftp | vfs::BackendKind::Shell | vfs::BackendKind::Ftp
            ) =>
            {
                let jobs: Vec<_> = self
                    .vfs_workflow
                    .jobs
                    .iter()
                    .filter(|(_, id)| **id == mount.id())
                    .map(|(job, _)| *job)
                    .collect();
                for job in jobs {
                    self.request_cancel_for_job(job);
                }
                mount.invalidate();
                let VfsPath::Mounted(location) = mount.root() else {
                    unreachable!()
                };
                self.queue_worker_job_request(JobRequest::Vfs(Task::Reconnect { location }));
                self.set_status(format!("Reconnecting {}...", mount.label()));
            }
            _ => {}
        }
    }

    pub(crate) fn close_vfs_mount(&mut self, id: u64) {
        let mut closing = vec![id];
        loop {
            let before = closing.len();
            for mount in self.vfs_workflow.mounts.values() {
                if let VfsPath::Mounted(parent) = mount.return_to()
                    && closing.contains(&parent.mount().id())
                    && !closing.contains(&mount.id())
                {
                    closing.push(mount.id());
                }
            }
            if closing.len() == before {
                break;
            }
        }
        for id in &closing {
            if let Some(mount) = self.vfs_workflow.mounts.remove(id) {
                mount.close();
            }
        }
        let jobs: Vec<_> = self
            .vfs_workflow
            .jobs
            .iter()
            .filter(|(_, mount)| closing.contains(mount))
            .map(|(job, _)| *job)
            .collect();
        for job in jobs {
            self.vfs_workflow.pending.remove(&job);
            self.request_cancel_for_job(job);
        }
        self.discard_closed_vfs_reverts();
        for panel in [ActivePanel::Left, ActivePanel::Right] {
            let mut location = self.panels[panel.index()].location();
            if !matches!(&location, VfsPath::Mounted(path) if path.mount().is_closed()) {
                continue;
            }
            while let VfsPath::Mounted(path) = &location {
                if !path.mount().is_closed() {
                    break;
                }
                location = path.mount().return_to().clone();
            }
            self.cancel_and_invalidate_panel_refresh(panel);
            self.enter_vfs_location(panel, location, false);
        }
        self.set_status("VFS mount closed");
    }
}

#[cfg(all(test, unix))]
mod tests;
