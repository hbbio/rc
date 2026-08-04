use crate::*;

const SELECTION_SIZE_CANCELED_LABEL: &str = "Size calculation canceled";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum SelectionSizeState {
    #[default]
    Empty,
    Calculating {
        selected_items: usize,
    },
    Ready {
        selected_items: usize,
        apparent_bytes: u64,
        unreadable_entries: u64,
    },
    Failed {
        selected_items: usize,
        error: String,
    },
}

impl SelectionSizeState {
    fn selected_items(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Calculating { selected_items }
            | Self::Ready { selected_items, .. }
            | Self::Failed { selected_items, .. } => *selected_items,
        }
    }
}

#[derive(Debug)]
pub(crate) struct SelectionSizeWorkflow {
    job_ids: [Option<JobId>; 2],
    request_ids: [u64; 2],
    paths: [Vec<PathBuf>; 2],
    next_request_id: u64,
}

impl Default for SelectionSizeWorkflow {
    fn default() -> Self {
        Self {
            job_ids: [None; 2],
            request_ids: [0; 2],
            paths: std::array::from_fn(|_| Vec::new()),
            next_request_id: 1,
        }
    }
}

impl SelectionSizeWorkflow {
    fn begin_request(&mut self, panel: ActivePanel, paths: Vec<PathBuf>) -> (u64, Option<JobId>) {
        let panel_index = panel.index();
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.request_ids[panel_index] = request_id;
        self.paths[panel_index] = paths;
        (request_id, self.job_ids[panel_index].take())
    }

    fn invalidate(&mut self, panel: ActivePanel) -> Option<JobId> {
        self.begin_request(panel, Vec::new()).1
    }

    fn is_current(&self, panel: ActivePanel, request_id: u64) -> bool {
        self.request_ids[panel.index()] == request_id
    }

    fn paths_match(&self, panel: ActivePanel, paths: &[PathBuf]) -> bool {
        self.paths[panel.index()] == paths
    }

    fn set_job_id(&mut self, panel: ActivePanel, job_id: JobId) {
        self.job_ids[panel.index()] = Some(job_id);
    }

    fn clear(&mut self, panel: ActivePanel) {
        self.job_ids[panel.index()] = None;
    }

    fn panel_for_job_id(&self, job_id: JobId) -> Option<ActivePanel> {
        [ActivePanel::Left, ActivePanel::Right]
            .into_iter()
            .find(|panel| self.job_ids[panel.index()] == Some(job_id))
    }
}

impl AppState {
    pub fn selection_size_state(&self, panel: ActivePanel) -> &SelectionSizeState {
        &self.selection_sizes[panel.index()]
    }

    pub(crate) fn sync_selection_size(&mut self, panel: ActivePanel, force: bool) {
        let paths = if self.show_panel_totals() {
            self.panels[panel.index()].tagged_paths()
        } else {
            Vec::new()
        };
        if paths.is_empty() {
            if let Some(job_id) = self.selection_size.invalidate(panel) {
                let _ = self.request_cancel_for_job(job_id);
            }
            self.selection_sizes[panel.index()] = SelectionSizeState::Empty;
            return;
        }

        if !force && self.selection_size.paths_match(panel, &paths) {
            return;
        }

        let selected_items = paths.len();
        let (request_id, previous_job_id) = self.selection_size.begin_request(panel, paths.clone());
        self.selection_sizes[panel.index()] = SelectionSizeState::Calculating { selected_items };
        let request = JobRequest::MeasureSelection {
            panel,
            paths,
            request_id,
        };

        if let Some(previous_job_id) = previous_job_id {
            if self.replace_pending_queued_job_request(previous_job_id, &request) {
                self.selection_size.set_job_id(panel, previous_job_id);
                tracing::debug!(
                    job_event = "coalesced",
                    job_kind = JobKind::MeasureSelection.label(),
                    job_id = %previous_job_id,
                    panel_index = panel.index(),
                    request_id,
                    "coalesced pending selection-size request"
                );
                return;
            }
            let _ = self.request_cancel_for_job(previous_job_id);
        }

        let job_id = self.queue_transient_worker_job_request(request);
        self.selection_size.set_job_id(panel, job_id);
    }

    pub(crate) fn handle_selection_size_measured(
        &mut self,
        panel: ActivePanel,
        request_id: u64,
        report: SelectionSizeReport,
    ) {
        if !self.selection_size.is_current(panel, request_id) {
            return;
        }

        let selected_items = self.selection_sizes[panel.index()].selected_items();
        if selected_items == 0 {
            return;
        }
        self.selection_size.clear(panel);
        self.selection_sizes[panel.index()] = SelectionSizeState::Ready {
            selected_items,
            apparent_bytes: report.apparent_bytes,
            unreadable_entries: report.unreadable_entries,
        };
    }

    pub(crate) fn handle_selection_size_job_failure(&mut self, job_id: JobId, error: &JobError) {
        let Some(panel) = self.selection_size.panel_for_job_id(job_id) else {
            return;
        };
        let selected_items = self.selection_sizes[panel.index()].selected_items();
        self.selection_size.clear(panel);
        self.selection_sizes[panel.index()] = SelectionSizeState::Failed {
            selected_items,
            error: if error.is_canceled() {
                String::from(SELECTION_SIZE_CANCELED_LABEL)
            } else {
                error.user_message()
            },
        };
    }

    pub(crate) fn handle_selection_size_cancel_requested(&mut self, job_id: JobId) {
        let Some(panel) = self.selection_size.panel_for_job_id(job_id) else {
            return;
        };
        let selected_items = self.selection_sizes[panel.index()].selected_items();
        let _ = self.selection_size.invalidate(panel);
        self.selection_sizes[panel.index()] = SelectionSizeState::Failed {
            selected_items,
            error: String::from(SELECTION_SIZE_CANCELED_LABEL),
        };
    }
}
