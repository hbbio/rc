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
    panelize_revert: Option<(ActivePanel, PanelListingSource)>,
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

    fn schedule_panelize_revert(&mut self, panel: ActivePanel, source: PanelListingSource) {
        self.panelize_revert = Some((panel, source));
    }

    fn clear_panelize_revert_for_panel(&mut self, panel: ActivePanel) {
        if self
            .panelize_revert
            .as_ref()
            .is_some_and(|(pending_panel, _)| *pending_panel == panel)
        {
            self.panelize_revert = None;
        }
    }

    fn take_panelize_revert_source_for_panel(
        &mut self,
        panel: ActivePanel,
    ) -> Option<PanelListingSource> {
        let (pending_panel, revert_source) = self.panelize_revert.take()?;
        if pending_panel == panel {
            return Some(revert_source);
        }
        self.panelize_revert = Some((pending_panel, revert_source));
        None
    }
}

impl AppState {
    pub(crate) fn queue_panel_refresh(&mut self, panel: ActivePanel) {
        let panel_index = panel.index();
        let request_id = self.panel_refresh.begin_request(panel);

        let (cwd, source, sort_mode, show_hidden_files) = {
            let panel_state = &mut self.panels[panel_index];
            panel_state.loading = true;
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

    pub(crate) fn schedule_panelize_revert_for_panel_refresh(
        &mut self,
        panel: ActivePanel,
        source: PanelListingSource,
    ) {
        self.panel_refresh_post
            .schedule_panelize_revert(panel, source);
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
            if let Some(parent) = cwd.parent() {
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
        self.set_status(format!("Loading {} entries...", partial_count));
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
        let mut clear_focus_target = false;
        let mut focus_status = None;
        {
            let panel_state = &mut self.panels[panel.index()];
            panel_state.loading = false;
            match result {
                Ok(entries) => {
                    panel_state.apply_entries(entries);
                    panel_state.disk_usage = disk_usage;
                    self.panel_refresh_post
                        .clear_panelize_revert_for_panel(panel);
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
                    if let Some(revert_source) = self
                        .panel_refresh_post
                        .take_panelize_revert_source_for_panel(panel)
                    {
                        panel_state.source = revert_source;
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
        self.panel_refresh_clear_panel(panel);
        if clear_focus_target {
            self.panel_refresh_post.clear_focus_target_for_panel(panel);
        }
        if let Some(focus_status) = focus_status {
            self.set_status(focus_status);
        }
    }

    #[cfg(test)]
    pub(crate) fn panel_refresh_job_id_at(&self, panel_index: usize) -> Option<JobId> {
        self.panel_refresh.job_id_at(panel_index)
    }
}
