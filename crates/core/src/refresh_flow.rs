use crate::*;

#[derive(Debug)]
pub(crate) struct PanelRefreshWorkflow {
    job_ids: [Option<JobId>; 2],
    request_ids: [u64; 2],
    partial_entry_count: [usize; 2],
    next_request_id: u64,
}

#[derive(Debug, Default)]
pub(crate) struct PanelRefreshPostWorkflow {
    focus_target: Option<(ActivePanel, PathBuf)>,
    reverts: [Option<PanelRefreshRevert>; 2],
}

#[derive(Debug)]
pub(crate) struct PanelRefreshRevert {
    panel: PanelState,
    previous_directory: Option<PathBuf>,
    panelized_result_history: Option<PanelizedResultSnapshot>,
}

pub(crate) struct PanelRefreshCompletion {
    pub(crate) panel: ActivePanel,
    pub(crate) cwd: PathBuf,
    pub(crate) source: PanelListingSource,
    pub(crate) sort_mode: SortMode,
    pub(crate) request_id: u64,
    pub(crate) disk_usage: Option<DiskUsageSummary>,
    pub(crate) result: Result<Vec<FileEntry>, String>,
}

impl Default for PanelRefreshWorkflow {
    fn default() -> Self {
        Self {
            job_ids: [None; 2],
            request_ids: [0; 2],
            partial_entry_count: [0; 2],
            next_request_id: 1,
        }
    }
}

impl PanelRefreshWorkflow {
    fn begin_request(&mut self, panel: ActivePanel) -> u64 {
        let panel_index = panel.index();
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.request_ids[panel_index] = request_id;
        self.partial_entry_count[panel_index] = 0;
        request_id
    }

    fn is_current_request(&self, panel: ActivePanel, request_id: u64) -> bool {
        self.request_ids[panel.index()] == request_id
    }

    fn is_first_chunk(&self, panel: ActivePanel) -> bool {
        self.partial_entry_count[panel.index()] == 0
    }

    fn add_partial_entries(&mut self, panel: ActivePanel, count: usize) -> usize {
        let panel_index = panel.index();
        self.partial_entry_count[panel_index] =
            self.partial_entry_count[panel_index].saturating_add(count);
        self.partial_entry_count[panel_index]
    }

    fn set_job_id(&mut self, panel: ActivePanel, job_id: JobId) {
        self.job_ids[panel.index()] = Some(job_id);
    }

    fn take_job_id(&mut self, panel: ActivePanel) -> Option<JobId> {
        self.job_ids[panel.index()].take()
    }

    fn job_id(&self, panel: ActivePanel) -> Option<JobId> {
        self.job_ids[panel.index()]
    }

    fn invalidate_request(&mut self, panel: ActivePanel) -> Option<JobId> {
        let job_id = self.take_job_id(panel);
        self.begin_request(panel);
        self.clear_panel(panel);
        job_id
    }

    fn clear_panel(&mut self, panel: ActivePanel) {
        let panel_index = panel.index();
        self.job_ids[panel_index] = None;
        self.partial_entry_count[panel_index] = 0;
    }

    fn panel_for_job_id(&self, id: JobId) -> Option<ActivePanel> {
        [ActivePanel::Left, ActivePanel::Right]
            .into_iter()
            .find(|panel| self.job_ids[panel.index()].is_some_and(|job_id| job_id == id))
    }

    #[cfg(test)]
    fn job_id_at(&self, panel_index: usize) -> Option<JobId> {
        self.job_ids[panel_index]
    }
}

impl PanelRefreshPostWorkflow {
    fn clear_focus_target(&mut self) {
        self.focus_target = None;
    }

    fn set_focus_target(&mut self, panel: ActivePanel, path: PathBuf) {
        self.focus_target = Some((panel, path));
    }

    fn focus_target_for_panel(&self, panel: ActivePanel) -> Option<PathBuf> {
        self.focus_target
            .as_ref()
            .and_then(|(pending_panel, path)| (*pending_panel == panel).then(|| path.clone()))
    }

    fn clear_focus_target_for_panel(&mut self, panel: ActivePanel) {
        if self
            .focus_target
            .as_ref()
            .is_some_and(|(pending_panel, _)| *pending_panel == panel)
        {
            self.focus_target = None;
        }
    }

    fn ensure_revert(&mut self, panel: ActivePanel, snapshot: PanelRefreshRevert) {
        let pending = &mut self.reverts[panel.index()];
        if pending.is_none() {
            *pending = Some(snapshot);
        }
    }

    fn clear_revert(&mut self, panel: ActivePanel) {
        self.reverts[panel.index()] = None;
    }

    fn has_revert(&self, panel: ActivePanel) -> bool {
        self.reverts[panel.index()].is_some()
    }

    fn take_revert(&mut self, panel: ActivePanel) -> Option<PanelRefreshRevert> {
        self.reverts[panel.index()].take()
    }
}

impl AppState {
    pub(crate) fn queue_panel_refresh(&mut self, panel: ActivePanel) {
        let panel_index = panel.index();
        if self.panels[panel_index].source.is_panelized() {
            let snapshot = self.panel_refresh_revert_snapshot(panel);
            self.panel_refresh_post.ensure_revert(panel, snapshot);
        }
        let request_id = self.panel_refresh.begin_request(panel);

        let (cwd, source, sort_mode, show_hidden_files) = {
            let panel_state = &mut self.panels[panel_index];
            panel_state.loading = true;
            panel_state.disk_usage = None;
            (
                panel_state.cwd.clone(),
                panel_state.source.clone(),
                panel_state.sort_mode,
                panel_state.show_hidden_files,
            )
        };
        let request = JobRequest::RefreshPanel {
            panel,
            cwd,
            source,
            sort_mode,
            show_hidden_files,
            request_id,
        };
        if let Some(previous_job_id) = self.panel_refresh.take_job_id(panel) {
            if self.replace_pending_panel_refresh_request(previous_job_id, &request) {
                self.panel_refresh.set_job_id(panel, previous_job_id);
                tracing::debug!(
                    job_event = "coalesced",
                    job_kind = JobKind::RefreshPanel.label(),
                    job_id = %previous_job_id,
                    panel_index,
                    request_id,
                    "coalesced pending panel refresh request"
                );
                return;
            }
            let _ = self.request_cancel_for_job(previous_job_id);
        }

        let job_id = self.queue_worker_job_request(request);
        self.panel_refresh.set_job_id(panel, job_id);
    }

    pub(crate) fn clear_panel_refresh_state_for_job(&mut self, id: JobId) {
        if let Some(panel) = self.panel_refresh.panel_for_job_id(id) {
            let panel_index = panel.index();
            self.panel_refresh.clear_panel(panel);
            self.panels[panel_index].loading = false;
            tracing::debug!(
                job_event = "panel_refresh_state_cleared",
                job_id = %id,
                panel_index,
                "cleared panel refresh loading state"
            );
        }
    }

    pub(crate) fn panel_refresh_is_current_request(
        &self,
        panel: ActivePanel,
        request_id: u64,
    ) -> bool {
        self.panel_refresh.is_current_request(panel, request_id)
    }

    pub(crate) fn panel_refresh_is_first_chunk(&self, panel: ActivePanel) -> bool {
        self.panel_refresh.is_first_chunk(panel)
    }

    pub(crate) fn panel_refresh_add_partial_entries(
        &mut self,
        panel: ActivePanel,
        count: usize,
    ) -> usize {
        self.panel_refresh.add_partial_entries(panel, count)
    }

    pub(crate) fn panel_refresh_clear_panel(&mut self, panel: ActivePanel) {
        self.panel_refresh.clear_panel(panel);
    }

    pub(crate) fn clear_pending_panel_focus_target(&mut self) {
        self.panel_refresh_post.clear_focus_target();
    }

    pub(crate) fn set_pending_panel_focus_target(&mut self, panel: ActivePanel, path: PathBuf) {
        self.panel_refresh_post.set_focus_target(panel, path);
    }

    pub(crate) fn panel_refresh_revert_snapshot(&self, panel: ActivePanel) -> PanelRefreshRevert {
        let panel_index = panel.index();
        PanelRefreshRevert {
            panel: self.panels[panel_index].clone(),
            previous_directory: self.previous_panel_directories[panel_index].clone(),
            panelized_result_history: self.panelized_result_history[panel_index].clone(),
        }
    }

    pub(crate) fn schedule_panel_refresh_revert(
        &mut self,
        panel: ActivePanel,
        snapshot: PanelRefreshRevert,
    ) {
        self.panel_refresh_post.ensure_revert(panel, snapshot);
    }

    fn restore_panel_refresh_revert(&mut self, panel: ActivePanel) -> bool {
        let Some(mut revert) = self.panel_refresh_post.take_revert(panel) else {
            return false;
        };
        let panel_index = panel.index();
        revert.panel.loading = false;
        self.panels[panel_index] = revert.panel;
        self.previous_panel_directories[panel_index] = revert.previous_directory;
        self.panelized_result_history[panel_index] = revert.panelized_result_history;
        true
    }

    pub(crate) fn rollback_panel_refresh_for_job(&mut self, id: JobId) {
        let Some(panel) = self.panel_refresh.panel_for_job_id(id) else {
            return;
        };
        if self.restore_panel_refresh_revert(panel) {
            self.panel_refresh_post.clear_focus_target_for_panel(panel);
        }
    }

    pub(crate) fn completed_panelized_result_snapshot(
        &self,
        panel: ActivePanel,
    ) -> Option<PanelizedResultSnapshot> {
        if self.panel_refresh_post.has_revert(panel) {
            return None;
        }
        PanelizedResultSnapshot::from_panel(&self.panels[panel.index()])
    }

    pub(crate) fn panel_refresh_job_id(&self, panel: ActivePanel) -> Option<JobId> {
        self.panel_refresh.job_id(panel)
    }

    pub(crate) fn cancel_and_invalidate_panel_refresh(&mut self, panel: ActivePanel) {
        if let Some(job_id) = self.panel_refresh.invalidate_request(panel) {
            let _ = self.request_cancel_for_job(job_id);
        }
        self.panel_refresh_post.clear_revert(panel);
        self.panel_refresh_post.clear_focus_target_for_panel(panel);
        self.panels[panel.index()].loading = false;
    }

    pub(crate) fn handle_panel_entries_chunk(
        &mut self,
        panel: ActivePanel,
        cwd: PathBuf,
        source: PanelListingSource,
        sort_mode: SortMode,
        request_id: u64,
        entries: Vec<FileEntry>,
    ) {
        if !self.panel_refresh_is_current_request(panel, request_id) {
            return;
        }
        let panel_state = &self.panels[panel.index()];
        let still_current = panel_state.cwd == cwd
            && panel_state.source == source
            && panel_state.sort_mode == sort_mode;
        if !still_current {
            return;
        }
        if entries.is_empty() {
            return;
        }

        let panel_index = panel.index();
        let is_first_chunk = self.panel_refresh_is_first_chunk(panel);
        let partial_count = self.panel_refresh_add_partial_entries(panel, entries.len());
        let panel_state = &mut self.panels[panel_index];
        if is_first_chunk {
            panel_state.entries.clear();
            if matches!(source, PanelListingSource::Directory)
                && let Some(parent) = cwd.parent()
            {
                panel_state
                    .entries
                    .push(FileEntry::parent(parent.to_path_buf()));
            }
        }
        panel_state.entries.extend(entries);
        if panel_state.entries.is_empty() {
            panel_state.cursor = 0;
        }
        panel_state.loading = true;
        if source.is_panelized() {
            self.set_status(format!("Panelize: {partial_count} result(s) received..."));
        } else {
            self.set_status(format!("Loading {partial_count} entries..."));
        }
    }

    pub(crate) fn handle_panel_refreshed(&mut self, completion: PanelRefreshCompletion) {
        let PanelRefreshCompletion {
            panel,
            cwd,
            source,
            sort_mode,
            request_id,
            disk_usage,
            result,
        } = completion;
        if !self.panel_refresh_is_current_request(panel, request_id) {
            return;
        }
        let panel_state = &self.panels[panel.index()];
        let still_current = panel_state.cwd == cwd
            && panel_state.source == source
            && panel_state.sort_mode == sort_mode;
        if !still_current {
            return;
        }

        let focus_target = self.panel_refresh_post.focus_target_for_panel(panel);
        let has_streamed_entries = !self.panel_refresh_is_first_chunk(panel);
        let refresh_failed = result.is_err();
        let mut clear_focus_target = false;
        let mut focus_status = None;
        let mut completion_status = None;
        {
            let panel_state = &mut self.panels[panel.index()];
            panel_state.loading = false;
            panel_state.disk_usage = disk_usage;
            match result {
                Ok(entries) => {
                    let entry_count = entries.len();
                    let streamed_selection = if source.is_panelized() && has_streamed_entries {
                        panel_state.selected_entry().map(|entry| entry.path.clone())
                    } else {
                        None
                    };
                    panel_state.apply_entries(entries);
                    if let Some(selected_path) = streamed_selection
                        && let Some(index) = panel_state
                            .entries
                            .iter()
                            .position(|entry| entry.path == selected_path)
                    {
                        panel_state.cursor = index;
                    }
                    self.panel_refresh_post.clear_revert(panel);
                    if source.is_panelized() {
                        completion_status =
                            Some(format!("Panelize complete: {entry_count} result(s)"));
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
                    let is_panelize = source.is_panelized();
                    completion_status = match (is_panelize, error.as_str()) {
                        (true, PANEL_REFRESH_CANCELED_MESSAGE) => {
                            Some(String::from("Panelize canceled"))
                        }
                        (true, _) => Some(format!("Panelize failed: {error}")),
                        (false, PANEL_REFRESH_CANCELED_MESSAGE) => None,
                        (false, _) => Some(format!("Panel refresh failed: {error}")),
                    };
                    clear_focus_target = focus_target.is_some();
                }
            }
        }
        if refresh_failed {
            self.restore_panel_refresh_revert(panel);
        }
        self.panel_refresh_clear_panel(panel);
        if clear_focus_target {
            self.panel_refresh_post.clear_focus_target_for_panel(panel);
        }
        if let Some(focus_status) = focus_status {
            self.set_status(focus_status);
        } else if let Some(completion_status) = completion_status {
            self.set_status(completion_status);
        }
    }

    #[cfg(test)]
    pub(crate) fn panel_refresh_job_id_at(&self, panel_index: usize) -> Option<JobId> {
        self.panel_refresh.job_id_at(panel_index)
    }
}
