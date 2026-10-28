use std::sync::mpsc::Sender;

use crate::*;

#[derive(Debug)]
pub(crate) struct DirectorySizePending {
    id: JobId,
    cwd: PathBuf,
    source: PanelListingSource,
}

/// Measure directories on a worker without following local symlinks.
pub fn execute_directory_sizes(
    job: WorkerJob,
    jobs: &Sender<JobEvent>,
    events: &Sender<BackgroundEvent>,
) {
    let JobRequest::MeasureDirectories { paths, archive } = &job.request else {
        return;
    };
    let id = job.id;
    let _ = jobs.send(JobEvent::Started { id });
    let cancel = job.cancel_flag();
    let result = (|| {
        let mut sizes = Vec::with_capacity(paths.len());
        let mut total = 0u64;
        for path in paths {
            let report = match archive {
                Some(location) => {
                    measure_archive_selection(location, std::slice::from_ref(path), &cancel)
                }
                None => measure_selection_size(std::slice::from_ref(path), &cancel),
            }
            .map_err(JobError::from_io)?;
            total = total.saturating_add(report.apparent_bytes);
            sizes.push((path.clone(), report));
            let _ = jobs.send(JobEvent::Progress {
                id,
                progress: JobProgress {
                    current_path: Some(path.clone()),
                    items_total: paths.len() as u64,
                    items_done: sizes.len() as u64,
                    bytes_total: 0,
                    bytes_done: total,
                },
            });
        }
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(JobError::canceled());
        }
        events
            .send(BackgroundEvent::DirectorySizesMeasured { job_id: id, sizes })
            .map_err(|_| JobError::from_message("background event channel disconnected"))
    })();
    let _ = jobs.send(JobEvent::Finished { id, result });
}

impl AppState {
    pub(crate) fn directory_sizes_job_is_current(&self, id: JobId) -> bool {
        self.directory_size_jobs
            .iter()
            .any(|pending| pending.as_ref().is_some_and(|pending| pending.id == id))
    }

    pub(crate) fn show_directory_sizes(&mut self) {
        let panel = self.active_panel;
        let state = &self.panels[panel.index()];
        if state.loading {
            self.set_status("Wait for the directory listing to finish before calculating sizes");
            return;
        }
        let tagged_directories: Vec<_> = state
            .entries
            .iter()
            .filter(|entry| entry.is_dir() && !entry.is_parent() && state.is_tagged(&entry.path))
            .map(|entry| entry.path.clone())
            .collect();
        let paths = if tagged_directories.is_empty() {
            state
                .entries
                .iter()
                .filter(|entry| entry.is_dir() && !entry.is_parent())
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>()
        } else {
            tagged_directories
        };
        if paths.is_empty() {
            self.set_status("No directories to measure in this listing");
            return;
        }
        let cwd = state.cwd.clone();
        let source = state.source.clone();
        let archive = state.archive_location().cloned();
        let count = paths.len();
        self.cancel_directory_sizes(panel);
        let id = self.queue_worker_job_request(JobRequest::MeasureDirectories { paths, archive });
        self.directory_size_jobs[panel.index()] = Some(DirectorySizePending { id, cwd, source });
        self.set_status(format!("Calculating sizes for {count} directories..."));
    }

    pub(crate) fn cancel_directory_sizes(&mut self, panel: ActivePanel) {
        if let Some(pending) = self.directory_size_jobs[panel.index()].take() {
            self.request_cancel_for_job(pending.id);
        }
    }

    pub(crate) fn invalidate_directory_sizes_for_job(&mut self, id: JobId) {
        for pending in &mut self.directory_size_jobs {
            if pending.as_ref().is_some_and(|pending| pending.id == id) {
                *pending = None;
            }
        }
    }

    pub(crate) fn fail_directory_sizes(&mut self, id: JobId, error: &JobError) {
        if let Some(index) = self
            .directory_size_jobs
            .iter()
            .position(|pending| pending.as_ref().is_some_and(|pending| pending.id == id))
        {
            self.directory_size_jobs[index] = None;
            self.set_status(format!("Directory sizes: {}", error.user_message()));
        }
    }

    pub(crate) fn apply_directory_sizes(
        &mut self,
        id: JobId,
        sizes: Vec<(PathBuf, SelectionSizeReport)>,
    ) {
        let Some(index) = self
            .directory_size_jobs
            .iter()
            .position(|pending| pending.as_ref().is_some_and(|pending| pending.id == id))
        else {
            return;
        };
        let pending = self.directory_size_jobs[index]
            .take()
            .expect("matched request");
        let state = &mut self.panels[index];
        if state.cwd != pending.cwd || state.source != pending.source || state.loading {
            return;
        }
        let selected = state.selected_entry().map(|entry| entry.path.clone());
        let count = sizes.len();
        let unreadable = sizes.iter().fold(0u64, |total, (_, report)| {
            total.saturating_add(report.unreadable_entries)
        });
        let sizes: HashMap<_, _> = sizes.into_iter().collect();
        let update = |entry: &mut FileEntry| {
            if let Some(report) = sizes.get(&entry.path) {
                entry.size = report.apparent_bytes;
            }
        };
        state.entries.iter_mut().for_each(update);
        if let Some(cached) = state.panelized_entries.as_mut() {
            Arc::make_mut(cached).iter_mut().for_each(update);
        }
        if state.sort_mode.field == SortField::Size {
            let first_entry = usize::from(state.entries.first().is_some_and(FileEntry::is_parent));
            sort_file_entries(&mut state.entries[first_entry..], state.sort_mode);
        }
        if let Some(cursor) =
            selected.and_then(|path| state.entries.iter().position(|entry| entry.path == path))
        {
            state.cursor = cursor;
        }
        state.mouse_viewport = None;
        self.set_status(if unreadable == 0 {
            format!("Calculated sizes for {count} directories")
        } else { format!("Calculated sizes for {count} directories; partial results: {unreadable} unreadable entries") });
    }
}
