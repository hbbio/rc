use crate::find_flow::find_results_status_message;
use crate::*;

impl PanelMkdirTracker {
    fn track(&mut self, job_id: JobId, pending: PendingPanelMkdir) {
        self.latest_job_ids[pending.panel.index()] = Some(job_id);
        self.pending.insert(job_id, pending);
    }

    fn finish(&mut self, job_id: JobId, succeeded: bool) -> Option<PendingPanelMkdir> {
        let pending = self.pending.remove(&job_id)?;
        let latest_job_id = &mut self.latest_job_ids[pending.panel.index()];
        if *latest_job_id != Some(job_id) {
            return None;
        }
        *latest_job_id = None;
        succeeded.then_some(pending)
    }
}

impl AppState {
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

    pub fn take_pending_external_execute_requests(&mut self) -> Vec<ExternalExecuteRequest> {
        std::mem::take(&mut self.pending_external_execute_requests)
    }

    pub fn handle_job_event(&mut self, event: JobEvent) {
        if let JobEvent::Finished { id, .. } = &event {
            self.find_pause_flags.remove(id);
        }
        self.jobs.handle_event(&event);
        self.clamp_jobs_cursor();
        match event {
            JobEvent::Started { id } => {
                let kind = self.jobs.job(id).map(|job| job.kind);
                let job_kind = kind.map(JobKind::label).unwrap_or("unknown");
                let suppress_status = suppress_transient_job_status(kind);
                tracing::debug!(job_event = "started", job_kind, job_id = %id, "job started");
                if !suppress_status {
                    if let Some(summary) = self.jobs.job(id).map(|job| job.summary.clone()) {
                        self.set_status(format!("Job #{id} started: {summary}"));
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
                    let panel_mkdir = self.panel_mkdirs.finish(id, true);
                    let tree_impacts = self.tree_mutations.finish(id, true);
                    let kind = self.jobs.job(id).map(|job| job.kind);
                    let job_kind = kind.map(JobKind::label).unwrap_or("unknown");
                    tracing::info!(
                        job_event = "finished",
                        outcome = "succeeded",
                        job_kind,
                        job_id = %id,
                        "job finished successfully"
                    );
                    let is_persist_settings = kind == Some(JobKind::PersistSettings);
                    let is_find = kind == Some(JobKind::Find);
                    let is_quick_cd_search = kind == Some(JobKind::QuickCdSearch);
                    let suppress_status = suppress_transient_job_status(kind);
                    if is_quick_cd_search {
                        self.handle_quick_cd_search_job_finished(id);
                    }
                    if is_persist_settings {
                        self.mark_settings_saved(SystemTime::now());
                    }
                    if is_find
                        && let Some(results) = self.find_results_by_job_id_mut(id)
                        && results.report.is_none()
                    {
                        results.status = FindResultsStatus::Completed;
                    }
                    let should_refresh = matches!(
                        kind,
                        Some(
                            JobKind::Copy
                                | JobKind::Move
                                | JobKind::Delete
                                | JobKind::Mkdir
                                | JobKind::Rename
                        )
                    );
                    let panel_mkdir_status =
                        panel_mkdir.map(|pending| self.complete_panel_mkdir(pending));
                    if should_refresh && panel_mkdir_status.is_none() {
                        self.refresh_panels();
                    }
                    if is_find {
                        if let Some(results) = self.find_results_by_job_id(id) {
                            self.set_status(find_results_status_message(results));
                        }
                    } else if let Some(status) = panel_mkdir_status {
                        self.set_status(status);
                    } else if !suppress_status {
                        if let Some(summary) = self.jobs.job(id).map(|job| job.summary.clone()) {
                            self.set_status(format!("Job #{id} finished: {summary}"));
                        } else {
                            self.set_status(format!("Job #{id} finished"));
                        }
                    }
                    if is_persist_settings
                        && let Some(request) = self.deferred_persist_settings_request.take()
                    {
                        self.queue_worker_job_request(request);
                    }
                    if let Some(impacts) = tree_impacts {
                        self.rescan_tree_for_impacts(&impacts);
                    }
                }
                Err(error) => {
                    self.panel_mkdirs.finish(id, false);
                    let tree_impacts = self.tree_mutations.finish(id, false);
                    let kind = self.jobs.job(id).map(|job| job.kind);
                    let job_kind = kind.map(JobKind::label).unwrap_or("unknown");
                    let is_persist_settings = kind == Some(JobKind::PersistSettings);
                    let is_find = kind == Some(JobKind::Find);
                    let is_refresh = kind == Some(JobKind::RefreshPanel);
                    let is_tree = kind == Some(JobKind::BuildTree);
                    let is_quick_cd_search = kind == Some(JobKind::QuickCdSearch);
                    let is_quick_view = kind == Some(JobKind::LoadQuickView);
                    let is_selection_size = kind == Some(JobKind::MeasureSelection);
                    let suppress_status = suppress_transient_job_status(kind);
                    if is_refresh {
                        self.clear_panel_refresh_state_for_job(id);
                    }
                    if is_quick_view {
                        self.handle_quick_view_job_failure(id, &error);
                    }
                    if is_quick_cd_search {
                        self.handle_quick_cd_search_job_failure(id, &error);
                    }
                    if is_selection_size {
                        self.handle_selection_size_job_failure(id, &error);
                    }
                    if is_find && let Some(results) = self.find_results_by_job_id_mut(id) {
                        results.status = if error.is_canceled() {
                            FindResultsStatus::Canceled
                        } else {
                            FindResultsStatus::Failed(error.user_message())
                        };
                    }
                    let find_status = is_find
                        .then(|| {
                            self.find_results_by_job_id(id)
                                .map(find_results_status_message)
                        })
                        .flatten();
                    if is_tree {
                        let canceled = error.is_canceled();
                        let failure = (!canceled).then(|| error.user_message());
                        let tree_is_active = if let Some(tree) = self.tree_by_job_id_mut(id) {
                            if canceled {
                                tree.mark_canceled(id);
                            } else if let Some(failure) = failure.as_ref() {
                                tree.mark_failed(id, failure.clone());
                            }
                            true
                        } else {
                            false
                        };
                        if tree_is_active {
                            if canceled {
                                self.set_status("Directory tree canceled");
                            } else if let Some(failure) = failure {
                                self.set_status(format!("Directory tree failed: {failure}"));
                            }
                        }
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
                        if !suppress_status && (!is_find || find_status.is_some()) {
                            self.set_status(
                                find_status
                                    .clone()
                                    .unwrap_or_else(|| format!("Job #{id} canceled")),
                            );
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
                        if !suppress_status && (!is_find || find_status.is_some()) {
                            self.set_status(find_status.unwrap_or_else(|| {
                                format!("Job #{id} failed: {}", error.user_message())
                            }));
                        }
                    }
                    if is_persist_settings
                        && let Some(request) = self.deferred_persist_settings_request.take()
                    {
                        self.queue_worker_job_request(request);
                    }
                    if let Some(impacts) = tree_impacts {
                        self.rescan_tree_for_impacts(&impacts);
                    }
                }
            },
        }
    }

    pub fn handle_background_event(&mut self, event: BackgroundEvent) {
        match event {
            BackgroundEvent::PanelEntriesChunk {
                panel,
                cwd,
                source,
                sort_mode,
                filter,
                request_id,
                entries,
            } => self.handle_panel_entries_chunk(PanelEntriesChunk {
                panel,
                cwd,
                source,
                sort_mode,
                filter,
                request_id,
                entries,
            }),
            BackgroundEvent::PanelRefreshed {
                panel,
                cwd,
                source,
                sort_mode,
                filter,
                request_id,
                disk_usage,
                result,
            } => self.handle_panel_refreshed(PanelRefreshCompletion {
                panel,
                cwd,
                source,
                sort_mode,
                filter,
                request_id,
                disk_usage,
                result,
            }),
            BackgroundEvent::PanelIdentityResolved {
                panel,
                cwd,
                request_id,
                result,
            } => self.handle_panel_identity_resolved(panel, cwd, request_id, result),
            BackgroundEvent::ViewerLoaded { path, result } => match result {
                Ok(viewer) => {
                    let is_preview = viewer.text_is_preview();
                    self.routes.push(Route::Viewer(viewer));
                    if is_preview {
                        self.set_status(format!(
                            "Opened viewer {} (text preview mode)",
                            path.to_string_lossy()
                        ));
                    } else {
                        self.set_status(format!("Opened viewer {}", path.to_string_lossy()));
                    }
                }
                Err(error) => {
                    self.set_status(format!("Viewer open failed: {error}"));
                }
            },
            BackgroundEvent::DesktopOpenFinished { path, result } => match result {
                Ok(()) => self.set_status(format!(
                    "Opened {} with the default application",
                    path.to_string_lossy()
                )),
                Err(error) => {
                    self.queue_worker_job_request(JobRequest::LoadViewer { path });
                    self.set_status(format!(
                        "Default application unavailable ({error}); opening viewer..."
                    ));
                }
            },
            BackgroundEvent::QuickViewLoaded {
                panel,
                path,
                request_id,
                result,
            } => self.handle_quick_view_loaded(panel, path, request_id, result),
            BackgroundEvent::SelectionSizeMeasured {
                panel,
                request_id,
                report,
            } => self.handle_selection_size_measured(panel, request_id, report),
            BackgroundEvent::QuickCdSearchUpdated {
                request_id,
                snapshot,
            } => self.handle_quick_cd_search_snapshot(request_id, snapshot),
            BackgroundEvent::FindEntriesChunk { job_id, entries } => {
                self.handle_find_entries_chunk(job_id, entries)
            }
            BackgroundEvent::FindCompleted { job_id, report } => {
                self.handle_find_completed(job_id, report)
            }
            BackgroundEvent::TreeReady {
                job_id,
                root,
                result,
            } => {
                let completion = self
                    .tree_by_job_id_mut(job_id)
                    .and_then(|tree| tree.apply_build_result(job_id, &root, result));
                if let Some(completion) = completion {
                    self.set_status(tree_ready_status(&completion));
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
        self.rollback_panel_refresh_for_job(id);
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
        origin: OperationOrigin,
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
                destination_names: None,
                overwrite,
            },
        };
        self.queue_filesystem_job(request, origin);
    }

    pub(crate) fn queue_copy_or_move_job_with_names(
        &mut self,
        kind: TransferKind,
        sources: Vec<PathBuf>,
        destination_dir: PathBuf,
        destination_names: Vec<String>,
        overwrite: OverwritePolicy,
        origin: OperationOrigin,
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
                destination_names: Some(destination_names),
                overwrite,
            },
        };
        self.queue_filesystem_job(request, origin);
    }

    pub(crate) fn queue_filesystem_job(
        &mut self,
        request: JobRequest,
        origin: OperationOrigin,
    ) -> JobId {
        let pending_panel_mkdir = match (&request, origin) {
            (JobRequest::Mkdir { path }, OperationOrigin::Panel(panel)) => {
                let panel_state = &self.panels[panel.index()];
                Some(PendingPanelMkdir {
                    panel,
                    path: path.clone(),
                    origin_cwd: panel_state.cwd.clone(),
                    origin_source: panel_state.source.clone(),
                })
            }
            _ => None,
        };
        let impacts = if origin == OperationOrigin::Tree {
            tree_mutation_impacts(&request)
        } else {
            Vec::new()
        };
        let job_id = self.queue_worker_job_request(request);
        if let Some(pending) = pending_panel_mkdir {
            self.panel_mkdirs.track(job_id, pending);
        }
        if origin == OperationOrigin::Tree {
            self.tree_mutations.track(job_id, impacts);
        }
        job_id
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

    fn complete_panel_mkdir(&mut self, pending: PendingPanelMkdir) -> String {
        let PendingPanelMkdir {
            panel,
            path,
            origin_cwd,
            origin_source,
        } = pending;
        let path_label = path.to_string_lossy().into_owned();
        let panel_state = &self.panels[panel.index()];
        let context_is_current = self.panel_view_mode(panel) == PanelViewMode::Listing
            && panel_state.cwd == origin_cwd
            && panel_state.source == origin_source;
        if !context_is_current {
            self.refresh_panels();
            return format!(
                "Created {path_label}; kept the newer {} panel state",
                panel.label()
            );
        }

        match self.set_panel_directory(panel, path) {
            Ok(true) => {
                // The target panel already queued its new listing. Refresh the other panel so a
                // sibling view of the parent directory also observes the new entry.
                self.queue_panel_refresh(panel.other());
                format!(
                    "Created {path_label} and entered it in the {} panel",
                    panel.label()
                )
            }
            Ok(false) => {
                self.refresh_panels();
                format!("Created {path_label}, but it is no longer accessible")
            }
            Err(error) => {
                self.refresh_panels();
                format!("Created {path_label}, but opening it failed: {error}")
            }
        }
    }

    pub(crate) fn queue_transient_worker_job_request(&mut self, request: JobRequest) -> JobId {
        let worker_job = self.jobs.enqueue(request);
        self.queue_worker_job_with_status(worker_job, false)
    }

    pub fn promote_deferred_persist_settings_request(&mut self) -> Option<JobId> {
        let request = self.deferred_persist_settings_request.take()?;
        let worker_job = self.jobs.enqueue(request);
        Some(self.queue_worker_job(worker_job))
    }

    pub(crate) fn queue_worker_job(&mut self, worker_job: WorkerJob) -> JobId {
        self.queue_worker_job_with_status(worker_job, true)
    }

    fn queue_worker_job_with_status(
        &mut self,
        worker_job: WorkerJob,
        report_status: bool,
    ) -> JobId {
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
        if report_status {
            self.set_status(format!("Queued job #{job_id}: {summary}"));
        }
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

    pub(crate) fn replace_pending_queued_job_request(
        &mut self,
        job_id: JobId,
        request: &JobRequest,
    ) -> bool {
        let request_kind = request.kind();
        if !self
            .jobs
            .job(job_id)
            .is_some_and(|job| job.kind == request_kind && matches!(job.status, JobStatus::Queued))
        {
            return false;
        }

        let run_index = self.pending_worker_commands.iter().rposition(|command| {
            matches!(
                command,
                WorkerCommand::Run(job)
                    if job.id == job_id
                        && job.request.kind() == request_kind
            )
        });
        let Some(run_index) = run_index else {
            return false;
        };

        if let WorkerCommand::Run(job) = &mut self.pending_worker_commands[run_index] {
            job.request = request.clone();
        }
        let metadata_replaced = self.jobs.replace_queued_request_metadata(job_id, request);
        debug_assert!(metadata_replaced);

        self.remove_pending_cancel_for_job(job_id);
        let _ = self.jobs.clear_cancel_request(job_id);
        true
    }

    fn remove_pending_cancel_for_job(&mut self, job_id: JobId) {
        self.pending_worker_commands
            .retain(|command| !matches!(command, WorkerCommand::Cancel(id) if *id == job_id));
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
        self.handle_quick_view_cancel_requested(job_id);
        self.handle_quick_cd_search_cancel_requested(job_id);
        self.handle_selection_size_cancel_requested(job_id);
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
                    // Persisted settings must finish, while an active desktop-open job owns an
                    // application launcher that the runtime releases without terminating it.
                    && !matches!(job.kind, JobKind::PersistSettings | JobKind::OpenDesktop)
            })
            .map(|job| job.id)
            .collect();
        for job_id in cancelable_job_ids {
            let _ = self.request_cancel_for_job(job_id);
        }
    }

    pub(crate) fn queue_delete_job(&mut self, targets: Vec<PathBuf>) {
        self.queue_delete_job_from(targets, OperationOrigin::Panel(self.active_panel));
    }

    pub(crate) fn queue_delete_job_from(&mut self, targets: Vec<PathBuf>, origin: OperationOrigin) {
        self.queue_filesystem_job(JobRequest::Delete { targets }, origin);
    }
}

fn suppress_transient_job_status(kind: Option<JobKind>) -> bool {
    matches!(
        kind,
        Some(
            JobKind::RefreshPanel
                | JobKind::OpenDesktop
                | JobKind::LoadViewer
                | JobKind::LoadQuickView
                | JobKind::QuickCdSearch
                | JobKind::MeasureSelection
                | JobKind::BuildTree
        )
    )
}

fn tree_mutation_impacts(request: &JobRequest) -> Vec<PathBuf> {
    let mut impacts = Vec::new();
    match request {
        JobRequest::Copy {
            destination_dir, ..
        } => impacts.push(destination_dir.clone()),
        JobRequest::Move {
            sources,
            destination_dir,
            ..
        } => {
            impacts.extend(
                sources
                    .iter()
                    .filter_map(|source| source.parent().map(Path::to_path_buf)),
            );
            impacts.push(destination_dir.clone());
        }
        JobRequest::Delete { targets } => impacts.extend(
            targets
                .iter()
                .filter_map(|target| target.parent().map(Path::to_path_buf)),
        ),
        JobRequest::Mkdir { path } => {
            if let Some(parent) = path.parent() {
                impacts.push(parent.to_path_buf());
            }
        }
        _ => {}
    }
    impacts.sort();
    impacts.dedup();
    impacts
}

fn tree_ready_status(completion: &TreeScanCompletion) -> String {
    let mut status = if completion.full_scan {
        format!("Opened directory tree ({})", completion.known_entries)
    } else {
        format!(
            "Rescanned {} ({} scanned, {} known)",
            completion.scan_root.to_string_lossy(),
            completion.scanned_entries,
            completion.known_entries
        )
    };
    let summary = &completion.summary;
    if summary.depth_limit_reached && summary.entry_limit_reached {
        status.push_str(" | truncated by depth and entry limits");
    } else if summary.depth_limit_reached {
        status.push_str(" | truncated by depth limit");
    } else if summary.entry_limit_reached {
        status.push_str(" | truncated by entry limit");
    }
    if summary.skipped_items > 0 {
        status.push_str(&format!(
            " | skipped {} unreadable item(s)",
            summary.skipped_items
        ));
        if let Some(issue) = &summary.first_issue {
            status.push_str(&format!(
                ": {}: {}",
                issue.path.to_string_lossy(),
                issue.message
            ));
        }
    }
    status
}
