use crate::*;

#[derive(Debug)]
pub(crate) struct PanelRefreshWorkflow {
    job_ids: [Option<JobId>; 2],
    request_ids: [u64; 2],
    partial_entry_count: [usize; 2],
    next_request_id: u64,
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

    #[cfg(test)]
    pub(crate) fn panel_refresh_job_id_at(&self, panel_index: usize) -> Option<JobId> {
        self.panel_refresh.job_id_at(panel_index)
    }
}
