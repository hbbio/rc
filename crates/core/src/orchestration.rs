use crate::*;

impl AppState {
    pub(crate) fn queue_panel_refresh(&mut self, panel: ActivePanel) {
        let panel_index = panel.index();
        if let Some(previous_job_id) = self.panel_refresh_job_ids[panel_index].take() {
            let _ = self.request_cancel_for_job(previous_job_id);
        }
        let request_id = self.next_panel_refresh_request_id;
        self.next_panel_refresh_request_id = self.next_panel_refresh_request_id.saturating_add(1);
        self.panel_refresh_request_ids[panel_index] = request_id;

        let (cwd, source, sort_mode, show_hidden_files) = {
            let panel_state = &mut self.panels[panel.index()];
            panel_state.loading = true;
            (
                panel_state.cwd.clone(),
                panel_state.source.clone(),
                panel_state.sort_mode,
                panel_state.show_hidden_files,
            )
        };
        let job_id = self.queue_worker_job_request(JobRequest::RefreshPanel {
            panel,
            cwd,
            source,
            sort_mode,
            show_hidden_files,
            request_id,
        });
        self.panel_refresh_job_ids[panel_index] = Some(job_id);
    }

    pub(crate) fn clear_panel_refresh_state_for_job(&mut self, id: JobId) {
        for panel_index in 0..self.panel_refresh_job_ids.len() {
            if self.panel_refresh_job_ids[panel_index].is_some_and(|job_id| job_id == id) {
                self.panel_refresh_job_ids[panel_index] = None;
                self.panels[panel_index].loading = false;
                tracing::debug!(
                    job_event = "panel_refresh_state_cleared",
                    job_id = %id,
                    panel_index,
                    "cleared panel refresh loading state"
                );
            }
        }
    }

    pub fn take_pending_worker_commands(&mut self) -> Vec<WorkerCommand> {
        std::mem::take(&mut self.pending_worker_commands)
    }

    pub fn restore_pending_worker_commands(&mut self, mut commands: Vec<WorkerCommand>) {
        if commands.is_empty() {
            return;
        }
        let restored_count = commands.len();
        commands.append(&mut self.pending_worker_commands);
        self.pending_worker_commands = commands;
        tracing::debug!(
            job_event = "queue_restored",
            restored_count,
            queue_depth = self.pending_worker_commands.len(),
            "restored pending worker commands after dispatch interruption"
        );
    }

    pub fn take_pending_external_edit_requests(&mut self) -> Vec<ExternalEditRequest> {
        std::mem::take(&mut self.pending_external_edit_requests)
    }

    pub fn handle_job_event(&mut self, event: JobEvent) {
        if let JobEvent::Finished { id, .. } = &event {
            self.find_pause_flags.remove(id);
        }
        self.jobs.handle_event(&event);
        self.clamp_jobs_cursor();
        match event {
            JobEvent::Started { id } => {
                let job_kind = self
                    .jobs
                    .job(id)
                    .map(|job| job.kind.label())
                    .unwrap_or("unknown");
                let is_refresh = self
                    .jobs
                    .job(id)
                    .is_some_and(|job| matches!(job.kind, JobKind::RefreshPanel));
                tracing::debug!(job_event = "started", job_kind, job_id = %id, "job started");
                if !is_refresh {
                    if let Some(job) = self.jobs.jobs().iter().find(|job| job.id == id) {
                        self.set_status(format!("Job #{id} started: {}", job.summary));
                    } else {
                        self.set_status(format!("Job #{id} started"));
                    }
                }
            }
            JobEvent::Progress { id, progress } => {
                let percent = progress.percent();
                let job_kind = self
                    .jobs
                    .job(id)
                    .map(|job| job.kind.label())
                    .unwrap_or("unknown");
                tracing::debug!(
                    job_event = "progress",
                    job_kind,
                    job_id = %id,
                    percent,
                    items_done = progress.items_done,
                    items_total = progress.items_total,
                    bytes_done = progress.bytes_done,
                    bytes_total = progress.bytes_total,
                    "job progress update"
                );
                let path_label = progress
                    .current_path
                    .as_deref()
                    .and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| String::from("-"));
                self.set_status(format!(
                    "Job #{id} {percent}% | items {}/{} | bytes {}/{} | {path_label}",
                    progress.items_done,
                    progress.items_total,
                    progress.bytes_done,
                    progress.bytes_total
                ));
            }
            JobEvent::Finished { id, result } => match result {
                Ok(()) => {
                    let job_kind = self
                        .jobs
                        .job(id)
                        .map(|job| job.kind.label())
                        .unwrap_or("unknown");
                    tracing::info!(
                        job_event = "finished",
                        outcome = "succeeded",
                        job_kind,
                        job_id = %id,
                        "job finished successfully"
                    );
                    let is_persist_settings = self
                        .jobs
                        .job(id)
                        .is_some_and(|job| matches!(job.kind, JobKind::PersistSettings));
                    let is_find = self
                        .jobs
                        .job(id)
                        .is_some_and(|job| matches!(job.kind, JobKind::Find));
                    let is_refresh = self
                        .jobs
                        .job(id)
                        .is_some_and(|job| matches!(job.kind, JobKind::RefreshPanel));
                    if is_persist_settings {
                        self.mark_settings_saved(SystemTime::now());
                    }
                    if is_find && let Some(results) = self.find_results_by_job_id_mut(id) {
                        results.loading = false;
                    }
                    let should_refresh = self.jobs.job(id).is_some_and(|job| {
                        matches!(
                            job.kind,
                            JobKind::Copy
                                | JobKind::Move
                                | JobKind::Delete
                                | JobKind::Mkdir
                                | JobKind::Rename
                        )
                    });
                    if should_refresh {
                        self.refresh_panels();
                    }
                    if is_find {
                        if let Some(results) = self.find_results_by_job_id(id) {
                            self.set_status(format!(
                                "Find '{}': {} result(s)",
                                results.query,
                                results.entries.len()
                            ));
                        } else {
                            self.set_status(format!("Job #{id} finished"));
                        }
                    } else if let Some(job) = self.jobs.job(id) {
                        if !is_refresh {
                            self.set_status(format!("Job #{id} finished: {}", job.summary));
                        }
                    } else if !is_refresh {
                        self.set_status(format!("Job #{id} finished"));
                    }
                    if is_persist_settings
                        && let Some(request) = self.deferred_persist_settings_request.take()
                    {
                        self.queue_worker_job_request(request);
                    }
                }
                Err(error) => {
                    let job_kind = self
                        .jobs
                        .job(id)
                        .map(|job| job.kind.label())
                        .unwrap_or("unknown");
                    let is_persist_settings = self
                        .jobs
                        .job(id)
                        .is_some_and(|job| matches!(job.kind, JobKind::PersistSettings));
                    let is_find = self
                        .jobs
                        .job(id)
                        .is_some_and(|job| matches!(job.kind, JobKind::Find));
                    let is_refresh = self
                        .jobs
                        .job(id)
                        .is_some_and(|job| matches!(job.kind, JobKind::RefreshPanel));
                    if is_refresh {
                        self.clear_panel_refresh_state_for_job(id);
                    }
                    if is_find && let Some(results) = self.find_results_by_job_id_mut(id) {
                        results.loading = false;
                    }
                    if error.is_canceled() {
                        tracing::info!(
                            job_event = "finished",
                            outcome = "canceled",
                            job_kind,
                            job_id = %id,
                            error_code = ?error.code,
                            retry_hint = ?error.retry_hint,
                            "job canceled"
                        );
                        if !is_refresh {
                            self.set_status(format!("Job #{id} canceled"));
                        }
                    } else {
                        tracing::warn!(
                            job_event = "finished",
                            outcome = "failed",
                            job_kind,
                            job_id = %id,
                            error_code = ?error.code,
                            retry_hint = ?error.retry_hint,
                            error_message = %error.message,
                            "job failed"
                        );
                        if !is_refresh {
                            self.set_status(format!("Job #{id} failed: {}", error.message));
                        }
                    }
                    if is_persist_settings
                        && let Some(request) = self.deferred_persist_settings_request.take()
                    {
                        self.queue_worker_job_request(request);
                    }
                }
            },
        }
    }

    pub fn handle_background_event(&mut self, event: BackgroundEvent) {
        match event {
            BackgroundEvent::PanelRefreshed {
                panel,
                cwd,
                source,
                sort_mode,
                request_id,
                result,
            } => {
                if self.panel_refresh_request_ids[panel.index()] != request_id {
                    return;
                }
                let panel_state = &self.panels[panel.index()];
                let still_current = panel_state.cwd == cwd
                    && panel_state.source == source
                    && panel_state.sort_mode == sort_mode;
                if !still_current {
                    return;
                }

                let focus_target =
                    self.pending_panel_focus
                        .as_ref()
                        .and_then(|(pending_panel, path)| {
                            (*pending_panel == panel).then(|| path.clone())
                        });
                let mut clear_focus_target = false;
                let mut focus_status = None;
                {
                    let panel_state = &mut self.panels[panel.index()];
                    panel_state.loading = false;
                    match result {
                        Ok(entries) => {
                            panel_state.apply_entries(entries);
                            if self
                                .pending_panelize_revert
                                .as_ref()
                                .is_some_and(|(pending_panel, _)| *pending_panel == panel)
                            {
                                self.pending_panelize_revert = None;
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
                            if let Some((pending_panel, revert_source)) =
                                self.pending_panelize_revert.take()
                            {
                                if pending_panel == panel {
                                    panel_state.source = revert_source;
                                } else {
                                    self.pending_panelize_revert =
                                        Some((pending_panel, revert_source));
                                }
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
                self.panel_refresh_job_ids[panel.index()] = None;
                if clear_focus_target {
                    self.pending_panel_focus = None;
                }
                if let Some(focus_status) = focus_status {
                    self.set_status(focus_status);
                }
            }
            BackgroundEvent::ViewerLoaded { path, result } => match result {
                Ok(viewer) => {
                    self.routes.push(Route::Viewer(viewer));
                    self.set_status(format!("Opened viewer {}", path.to_string_lossy()));
                }
                Err(error) => {
                    self.set_status(format!("Viewer open failed: {error}"));
                }
            },
            BackgroundEvent::FindEntriesChunk { job_id, entries } => {
                let status_message = if let Some(results) = self.find_results_by_job_id_mut(job_id)
                {
                    let was_empty = results.entries.is_empty();
                    results.entries.extend(entries);
                    if was_empty && !results.entries.is_empty() {
                        results.cursor = 0;
                    }
                    Some(format!(
                        "Finding '{}': {} result(s)...",
                        results.query,
                        results.entries.len()
                    ))
                } else {
                    None
                };
                if let Some(status_message) = status_message {
                    self.set_status(status_message);
                }
            }
            BackgroundEvent::TreeReady { root, entries } => {
                let mut replaced = false;
                for route in self.routes.iter_mut().rev() {
                    if let Route::Tree(tree) = route
                        && tree.root == root
                    {
                        tree.entries = entries.clone();
                        tree.cursor = 0;
                        tree.loading = false;
                        replaced = true;
                        break;
                    }
                }
                if replaced {
                    self.set_status(format!("Opened directory tree ({})", entries.len()));
                }
            }
        }
    }

    pub fn handle_job_dispatch_failure(&mut self, id: JobId, error: JobError) {
        tracing::warn!(
            job_event = "dispatch_failed",
            job_id = %id,
            error_code = ?error.code,
            retry_hint = ?error.retry_hint,
            error_message = %error.message,
            "job dispatch failed"
        );
        self.handle_job_event(JobEvent::Finished {
            id,
            result: Err(error),
        });
    }

    pub fn jobs_status_counts(&self) -> JobStatusCounts {
        self.jobs.status_counts()
    }

    pub(crate) fn queue_copy_or_move_job(
        &mut self,
        kind: TransferKind,
        sources: Vec<PathBuf>,
        destination_dir: PathBuf,
        overwrite: OverwritePolicy,
    ) {
        let request = match kind {
            TransferKind::Copy => JobRequest::Copy {
                sources,
                destination_dir,
                overwrite,
            },
            TransferKind::Move => JobRequest::Move {
                sources,
                destination_dir,
                overwrite,
            },
        };
        self.queue_worker_job_request(request);
    }

    pub fn enqueue_worker_job_request(&mut self, request: JobRequest) -> JobId {
        self.queue_worker_job_request(request)
    }

    pub(crate) fn queue_worker_job_request(&mut self, request: JobRequest) -> JobId {
        if matches!(request, JobRequest::PersistSettings { .. }) {
            if let Some(existing_id) = self.replace_pending_persist_settings_request(&request) {
                tracing::debug!(
                    job_event = "coalesced",
                    job_kind = JobKind::PersistSettings.label(),
                    job_id = %existing_id,
                    queue_depth = self.pending_worker_commands.len(),
                    "coalesced pending persist-settings job request"
                );
                self.set_status(format!("Updated pending setup save for job #{existing_id}"));
                return existing_id;
            }
            if let Some(active_id) = self.active_persist_settings_job_id() {
                self.deferred_persist_settings_request = Some(request);
                tracing::debug!(
                    job_event = "deferred",
                    job_kind = JobKind::PersistSettings.label(),
                    job_id = %active_id,
                    "deferred persist-settings request behind active job"
                );
                self.set_status(format!("Queued latest setup save after job #{active_id}"));
                return active_id;
            }
        }
        let worker_job = self.jobs.enqueue(request);
        self.queue_worker_job(worker_job)
    }

    pub(crate) fn queue_worker_job(&mut self, worker_job: WorkerJob) -> JobId {
        let job_id = worker_job.id;
        let job_kind = worker_job.request.kind().label();
        let summary = worker_job.request.summary();
        self.pending_worker_commands
            .push(WorkerCommand::Run(Box::new(worker_job)));
        tracing::debug!(
            job_event = "queued",
            job_kind,
            job_id = %job_id,
            queue_depth = self.pending_worker_commands.len(),
            summary = %summary,
            "queued worker job"
        );
        self.set_status(format!("Queued job #{job_id}: {summary}"));
        job_id
    }

    pub(crate) fn active_persist_settings_job_id(&self) -> Option<JobId> {
        self.jobs
            .jobs()
            .iter()
            .rev()
            .find(|job| {
                matches!(job.kind, JobKind::PersistSettings)
                    && matches!(job.status, JobStatus::Queued | JobStatus::Running)
            })
            .map(|job| job.id)
    }

    pub(crate) fn replace_pending_persist_settings_request(
        &mut self,
        request: &JobRequest,
    ) -> Option<JobId> {
        for command in self.pending_worker_commands.iter_mut().rev() {
            let WorkerCommand::Run(job) = command else {
                continue;
            };
            if matches!(job.request, JobRequest::PersistSettings { .. }) {
                job.request = request.clone();
                return Some(job.id);
            }
        }
        None
    }

    pub(crate) fn cancel_latest_job(&mut self) {
        let selected_id = if matches!(self.top_route(), Route::Jobs) {
            self.selected_job_record().map(|job| job.id)
        } else {
            None
        };
        let Some(job_id) = selected_id.or_else(|| self.jobs.newest_cancelable_job_id()) else {
            self.set_status("No active job to cancel");
            return;
        };

        if self.request_cancel_for_job(job_id) {
            self.set_status(format!("Cancellation requested for job #{job_id}"));
        } else {
            self.set_status(format!("Job #{job_id} cannot be canceled"));
        }
    }

    pub(crate) fn request_cancel_for_job(&mut self, job_id: JobId) -> bool {
        if !self.jobs.request_cancel(job_id) {
            return false;
        }
        let job_kind = self
            .jobs
            .job(job_id)
            .map(|job| job.kind.label())
            .unwrap_or("unknown");
        tracing::debug!(
            job_event = "cancel_requested",
            job_kind,
            job_id = %job_id,
            queue_depth = self.pending_worker_commands.len().saturating_add(1),
            "requested job cancellation"
        );
        self.pending_worker_commands
            .push(WorkerCommand::Cancel(job_id));
        true
    }

    pub(crate) fn request_cancel_for_all_jobs(&mut self) {
        let cancelable_job_ids: Vec<JobId> = self
            .jobs
            .jobs()
            .iter()
            .filter(|job| {
                matches!(job.status, JobStatus::Queued | JobStatus::Running)
                    && !matches!(job.kind, JobKind::PersistSettings)
            })
            .map(|job| job.id)
            .collect();
        for job_id in cancelable_job_ids {
            let _ = self.request_cancel_for_job(job_id);
        }
    }

    pub(crate) fn queue_delete_job(&mut self, targets: Vec<PathBuf>) {
        self.queue_worker_job_request(JobRequest::Delete { targets });
    }
}
