use std::path::{Path, PathBuf};

use crate::*;

#[derive(Clone, Debug, Default)]
pub enum QuickViewState {
    #[default]
    Empty,
    Directory {
        path: PathBuf,
    },
    Loading {
        path: PathBuf,
    },
    Ready(ViewerState),
    Failed {
        path: PathBuf,
        error: String,
    },
}

impl QuickViewState {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Empty => None,
            Self::Directory { path } | Self::Loading { path } | Self::Failed { path, .. } => {
                Some(path)
            }
            Self::Ready(viewer) => Some(viewer.path()),
        }
    }

    pub fn viewer(&self) -> Option<&ViewerState> {
        match self {
            Self::Ready(viewer) => Some(viewer),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub(crate) struct QuickViewWorkflow {
    job_ids: [Option<JobId>; 2],
    request_ids: [u64; 2],
    next_request_id: u64,
}

impl Default for QuickViewWorkflow {
    fn default() -> Self {
        Self {
            job_ids: [None; 2],
            request_ids: [0; 2],
            next_request_id: 1,
        }
    }
}

impl QuickViewWorkflow {
    fn begin_request(&mut self, panel: ActivePanel) -> (u64, Option<JobId>) {
        let panel_index = panel.index();
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.request_ids[panel_index] = request_id;
        (request_id, self.job_ids[panel_index].take())
    }

    fn invalidate(&mut self, panel: ActivePanel) -> Option<JobId> {
        self.begin_request(panel).1
    }

    fn is_current(&self, panel: ActivePanel, request_id: u64) -> bool {
        self.request_ids[panel.index()] == request_id
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
    pub(crate) fn activate_quick_view(&mut self, panel: ActivePanel) {
        let source_panel = panel.other();
        self.deactivate_quick_view(source_panel);
        self.deactivate_quick_view(panel);
        self.panel_views[source_panel.index()] = PanelViewMode::Listing;
        self.panel_views[panel.index()] = PanelViewMode::QuickView;
        self.active_panel = source_panel;
        self.sync_quick_view_from(source_panel, true);
        self.set_status(format!(
            "{} panel: quick view of {} panel selection",
            panel.label(),
            source_panel.label()
        ));
    }

    pub(crate) fn deactivate_quick_view(&mut self, panel: ActivePanel) {
        if let Some(job_id) = self.quick_view.invalidate(panel) {
            let _ = self.request_cancel_for_job(job_id);
        }
        self.quick_views[panel.index()] = QuickViewState::Empty;
    }

    pub(crate) fn sync_quick_view_from(&mut self, source_panel: ActivePanel, force: bool) {
        let target_panel = source_panel.other();
        if self.panel_view_mode(target_panel) != PanelViewMode::QuickView {
            return;
        }

        let selected = self.panels[source_panel.index()]
            .selected_entry()
            .map(|entry| (entry.path.clone(), entry.is_dir()));
        let Some((path, is_dir)) = selected else {
            self.cancel_quick_view_load(target_panel);
            self.quick_views[target_panel.index()] = QuickViewState::Empty;
            return;
        };

        if is_dir {
            self.cancel_quick_view_load(target_panel);
            self.quick_views[target_panel.index()] = QuickViewState::Directory { path };
            return;
        }

        if !force && self.quick_views[target_panel.index()].path() == Some(path.as_path()) {
            return;
        }

        let (request_id, previous_job_id) = self.quick_view.begin_request(target_panel);
        self.quick_views[target_panel.index()] = QuickViewState::Loading { path: path.clone() };
        let request = JobRequest::LoadQuickView {
            panel: target_panel,
            path,
            request_id,
        };

        if let Some(previous_job_id) = previous_job_id {
            if self.replace_pending_queued_job_request(previous_job_id, &request) {
                self.quick_view.set_job_id(target_panel, previous_job_id);
                tracing::debug!(
                    job_event = "coalesced",
                    job_kind = JobKind::LoadQuickView.label(),
                    job_id = %previous_job_id,
                    panel_index = target_panel.index(),
                    request_id,
                    "coalesced pending quick-view request"
                );
                return;
            }
            let _ = self.request_cancel_for_job(previous_job_id);
        }

        let job_id = self.queue_transient_worker_job_request(request);
        self.quick_view.set_job_id(target_panel, job_id);
    }

    fn cancel_quick_view_load(&mut self, panel: ActivePanel) {
        if let Some(job_id) = self.quick_view.invalidate(panel) {
            let _ = self.request_cancel_for_job(job_id);
        }
    }

    pub(crate) fn handle_quick_view_loaded(
        &mut self,
        panel: ActivePanel,
        path: PathBuf,
        request_id: u64,
        result: Result<ViewerState, String>,
    ) {
        let is_current = self.panel_view_mode(panel) == PanelViewMode::QuickView
            && self.quick_view.is_current(panel, request_id)
            && self.quick_views[panel.index()].path() == Some(path.as_path());
        if !is_current {
            return;
        }

        self.quick_view.clear(panel);
        match result {
            Ok(mut viewer) => {
                viewer.wrap = true;
                self.quick_views[panel.index()] = QuickViewState::Ready(viewer);
            }
            Err(error) => {
                self.quick_views[panel.index()] = QuickViewState::Failed {
                    path,
                    error: error.clone(),
                };
                self.set_status(format!("Quick view failed: {error}"));
            }
        }
    }

    pub(crate) fn handle_quick_view_job_failure(&mut self, job_id: JobId, error: &JobError) {
        let Some(panel) = self.quick_view.panel_for_job_id(job_id) else {
            return;
        };
        let Some(path) = self.quick_views[panel.index()]
            .path()
            .map(Path::to_path_buf)
        else {
            self.quick_view.clear(panel);
            return;
        };
        self.quick_view.clear(panel);
        self.quick_views[panel.index()] = QuickViewState::Failed {
            path,
            error: if error.is_canceled() {
                String::from("Preview canceled")
            } else {
                error.user_message()
            },
        };
    }

    pub(crate) fn handle_quick_view_cancel_requested(&mut self, job_id: JobId) {
        let Some(panel) = self.quick_view.panel_for_job_id(job_id) else {
            return;
        };
        let path = self.quick_views[panel.index()]
            .path()
            .map(Path::to_path_buf);
        let _ = self.quick_view.invalidate(panel);
        self.quick_views[panel.index()] =
            path.map_or(QuickViewState::Empty, |path| QuickViewState::Failed {
                path,
                error: String::from("Preview canceled"),
            });
    }
}
