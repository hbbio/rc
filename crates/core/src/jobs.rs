use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};

use filetime::FileTime;
#[cfg(unix)]
use nix::errno::Errno;
#[cfg(unix)]
use nix::unistd::{Gid, Uid, chown};

use crate::settings::Settings;
use crate::settings_io::{SettingsPaths, save_settings};
use crate::{ActivePanel, PanelListingSource, SortMode};

const COPY_BUFFER_SIZE: usize = 64 * 1024;
pub const JOB_CANCELED_MESSAGE: &str = "job canceled";

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct JobId(pub u64);

impl fmt::Display for JobId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobKind {
    Copy,
    Move,
    Delete,
    Mkdir,
    Rename,
    PersistSettings,
    RefreshPanel,
    Find,
    LoadViewer,
    BuildTree,
}

impl JobKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Move => "move",
            Self::Delete => "delete",
            Self::Mkdir => "mkdir",
            Self::Rename => "rename",
            Self::PersistSettings => "persist-settings",
            Self::RefreshPanel => "refresh-panel",
            Self::Find => "find",
            Self::LoadViewer => "load-viewer",
            Self::BuildTree => "build-tree",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OverwritePolicy {
    Overwrite,
    #[default]
    Skip,
    Rename,
}

impl OverwritePolicy {
    pub fn label(self) -> &'static str {
        match self {
            Self::Overwrite => "overwrite",
            Self::Skip => "skip",
            Self::Rename => "rename",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobRequest {
    Copy {
        sources: Vec<PathBuf>,
        destination_dir: PathBuf,
        overwrite: OverwritePolicy,
    },
    Move {
        sources: Vec<PathBuf>,
        destination_dir: PathBuf,
        overwrite: OverwritePolicy,
    },
    Delete {
        targets: Vec<PathBuf>,
    },
    Mkdir {
        path: PathBuf,
    },
    Rename {
        source: PathBuf,
        destination: PathBuf,
    },
    PersistSettings {
        paths: SettingsPaths,
        snapshot: Box<Settings>,
    },
    RefreshPanel {
        panel: ActivePanel,
        cwd: PathBuf,
        source: PanelListingSource,
        sort_mode: SortMode,
        show_hidden_files: bool,
        request_id: u64,
    },
    Find {
        query: String,
        base_dir: PathBuf,
        max_results: usize,
    },
    LoadViewer {
        path: PathBuf,
    },
    BuildTree {
        root: PathBuf,
        max_depth: usize,
        max_entries: usize,
    },
}

impl JobRequest {
    pub fn kind(&self) -> JobKind {
        match self {
            Self::Copy { .. } => JobKind::Copy,
            Self::Move { .. } => JobKind::Move,
            Self::Delete { .. } => JobKind::Delete,
            Self::Mkdir { .. } => JobKind::Mkdir,
            Self::Rename { .. } => JobKind::Rename,
            Self::PersistSettings { .. } => JobKind::PersistSettings,
            Self::RefreshPanel { .. } => JobKind::RefreshPanel,
            Self::Find { .. } => JobKind::Find,
            Self::LoadViewer { .. } => JobKind::LoadViewer,
            Self::BuildTree { .. } => JobKind::BuildTree,
        }
    }

    pub fn item_count(&self) -> usize {
        match self {
            Self::Copy { sources, .. } => sources.len(),
            Self::Move { sources, .. } => sources.len(),
            Self::Delete { targets } => targets.len(),
            Self::Mkdir { .. } => 1,
            Self::Rename { .. } => 1,
            Self::PersistSettings { .. } => 1,
            Self::RefreshPanel { .. } => 1,
            Self::Find { .. } => 1,
            Self::LoadViewer { .. } => 1,
            Self::BuildTree { .. } => 1,
        }
    }

    pub fn summary(&self) -> String {
        match self {
            Self::Copy {
                sources,
                destination_dir,
                overwrite,
            } => format!(
                "copy {} item(s) -> {} [{}]",
                sources.len(),
                destination_dir.to_string_lossy(),
                overwrite.label(),
            ),
            Self::Move {
                sources,
                destination_dir,
                overwrite,
            } => format!(
                "move {} item(s) -> {} [{}]",
                sources.len(),
                destination_dir.to_string_lossy(),
                overwrite.label(),
            ),
            Self::Delete { targets } => format!("delete {} item(s)", targets.len()),
            Self::Mkdir { path } => format!("mkdir {}", path.to_string_lossy()),
            Self::Rename {
                source,
                destination,
            } => format!(
                "rename {} -> {}",
                source.to_string_lossy(),
                destination.to_string_lossy()
            ),
            Self::PersistSettings { paths, .. } => {
                let target = paths
                    .rc_ini_path
                    .as_ref()
                    .or(paths.mc_ini_path.as_ref())
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|| String::from("<none>"));
                format!("save setup -> {target}")
            }
            Self::RefreshPanel {
                panel,
                cwd,
                source,
                request_id,
                ..
            } => {
                let source_label = match source {
                    PanelListingSource::Directory => "directory",
                    PanelListingSource::Panelize { .. } => "panelize",
                    PanelListingSource::FindResults { .. } => "find-results",
                };
                format!(
                    "refresh {:?} panel at {} [{}] (request #{request_id})",
                    panel,
                    cwd.to_string_lossy(),
                    source_label
                )
            }
            Self::Find {
                query, base_dir, ..
            } => {
                format!("find '{}' under {}", query, base_dir.to_string_lossy())
            }
            Self::LoadViewer { path } => format!("open viewer {}", path.to_string_lossy()),
            Self::BuildTree {
                root,
                max_depth,
                max_entries,
            } => {
                format!(
                    "build tree for {} [depth={}, entries={}]",
                    root.to_string_lossy(),
                    max_depth,
                    max_entries
                )
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobProgress {
    pub current_path: Option<PathBuf>,
    pub items_total: u64,
    pub items_done: u64,
    pub bytes_total: u64,
    pub bytes_done: u64,
}

impl JobProgress {
    pub fn percent(&self) -> u8 {
        let bytes_pct = if self.bytes_total > 0 {
            self.bytes_done
                .saturating_mul(100)
                .saturating_div(self.bytes_total)
        } else {
            0
        };
        let items_pct = if self.items_total > 0 {
            self.items_done
                .saturating_mul(100)
                .saturating_div(self.items_total)
        } else {
            0
        };
        let overall = bytes_pct.max(items_pct).min(100);
        overall as u8
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Canceled,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobRetryHint {
    None,
    Retry,
    Elevated,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobErrorCode {
    Canceled,
    PermissionDenied,
    AlreadyExists,
    NotFound,
    InvalidInput,
    Interrupted,
    Unsupported,
    Dispatch,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobError {
    pub code: JobErrorCode,
    pub message: String,
    pub retry_hint: JobRetryHint,
}

pub type JobResult = Result<(), JobError>;

impl JobError {
    pub fn canceled() -> Self {
        Self {
            code: JobErrorCode::Canceled,
            message: String::from(JOB_CANCELED_MESSAGE),
            retry_hint: JobRetryHint::None,
        }
    }

    pub fn dispatch(message: impl Into<String>) -> Self {
        Self {
            code: JobErrorCode::Dispatch,
            message: message.into(),
            retry_hint: JobRetryHint::Retry,
        }
    }

    pub fn from_io(error: io::Error) -> Self {
        let kind = error.kind();
        let message = error.to_string();
        match kind {
            io::ErrorKind::PermissionDenied => Self {
                code: JobErrorCode::PermissionDenied,
                message,
                retry_hint: JobRetryHint::Elevated,
            },
            io::ErrorKind::AlreadyExists => Self {
                code: JobErrorCode::AlreadyExists,
                message,
                retry_hint: JobRetryHint::None,
            },
            io::ErrorKind::NotFound => Self {
                code: JobErrorCode::NotFound,
                message,
                retry_hint: JobRetryHint::None,
            },
            io::ErrorKind::InvalidInput => Self {
                code: JobErrorCode::InvalidInput,
                message,
                retry_hint: JobRetryHint::None,
            },
            io::ErrorKind::Unsupported => Self {
                code: JobErrorCode::Unsupported,
                message,
                retry_hint: JobRetryHint::None,
            },
            io::ErrorKind::Interrupted => {
                if is_canceled_message(&message) {
                    Self::canceled()
                } else {
                    Self {
                        code: JobErrorCode::Interrupted,
                        message,
                        retry_hint: JobRetryHint::Retry,
                    }
                }
            }
            _ => Self {
                code: JobErrorCode::Other,
                message,
                retry_hint: JobRetryHint::Retry,
            },
        }
    }

    pub fn from_message(message: impl Into<String>) -> Self {
        let message = message.into();
        if is_canceled_message(&message) {
            Self::canceled()
        } else {
            Self {
                code: JobErrorCode::Other,
                message,
                retry_hint: JobRetryHint::Retry,
            }
        }
    }

    pub fn is_canceled(&self) -> bool {
        matches!(self.code, JobErrorCode::Canceled)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobRecord {
    pub id: JobId,
    pub kind: JobKind,
    pub summary: String,
    pub status: JobStatus,
    pub progress: Option<JobProgress>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct WorkerJob {
    pub id: JobId,
    pub request: JobRequest,
    cancel_flag: Arc<AtomicBool>,
    find_pause_flag: Option<Arc<AtomicBool>>,
}

impl WorkerJob {
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel_flag)
    }

    pub fn set_find_pause_flag(&mut self, pause_flag: Arc<AtomicBool>) {
        self.find_pause_flag = Some(pause_flag);
    }

    pub fn find_pause_flag(&self) -> Option<Arc<AtomicBool>> {
        self.find_pause_flag.as_ref().map(Arc::clone)
    }
}

#[derive(Debug)]
pub enum WorkerCommand {
    Run(Box<WorkerJob>),
    Cancel(JobId),
    Shutdown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobEvent {
    Started { id: JobId },
    Progress { id: JobId, progress: JobProgress },
    Finished { id: JobId, result: JobResult },
}

#[derive(Debug)]
pub struct JobManager {
    next_id: u64,
    jobs: Vec<JobRecord>,
    index_by_id: HashMap<JobId, usize>,
    cancel_flags: HashMap<JobId, Arc<AtomicBool>>,
}

impl Default for JobManager {
    fn default() -> Self {
        Self::new()
    }
}

impl JobManager {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            jobs: Vec::new(),
            index_by_id: HashMap::new(),
            cancel_flags: HashMap::new(),
        }
    }

    pub fn enqueue(&mut self, request: JobRequest) -> WorkerJob {
        let id = JobId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);

        let record = JobRecord {
            id,
            kind: request.kind(),
            summary: request.summary(),
            status: JobStatus::Queued,
            progress: None,
            last_error: None,
        };
        self.index_by_id.insert(id, self.jobs.len());
        self.jobs.push(record);
        self.cancel_flags
            .insert(id, Arc::new(AtomicBool::new(false)));

        WorkerJob {
            id,
            request,
            cancel_flag: self
                .cancel_flags
                .get(&id)
                .expect("job cancellation flag should exist")
                .clone(),
            find_pause_flag: None,
        }
    }

    pub fn handle_event(&mut self, event: &JobEvent) {
        match event {
            JobEvent::Started { id } => {
                if let Some(job) = self.job_mut(*id) {
                    job.status = JobStatus::Running;
                    job.progress = None;
                    job.last_error = None;
                }
            }
            JobEvent::Progress { id, progress } => {
                if let Some(job) = self.job_mut(*id) {
                    job.progress = Some(progress.clone());
                }
            }
            JobEvent::Finished { id, result } => {
                if let Some(job) = self.job_mut(*id) {
                    match result {
                        Ok(()) => {
                            job.status = JobStatus::Succeeded;
                            if let Some(progress) = &mut job.progress {
                                progress.current_path = None;
                                progress.items_done = progress.items_total;
                                progress.bytes_done = progress.bytes_total;
                            }
                            job.last_error = None;
                        }
                        Err(error) => {
                            if error.is_canceled() {
                                job.status = JobStatus::Canceled;
                                job.last_error = None;
                            } else {
                                job.status = JobStatus::Failed;
                                job.last_error = Some(error.message.clone());
                            }
                        }
                    }
                }
                self.cancel_flags.remove(id);
            }
        }
    }

    pub fn request_cancel(&mut self, id: JobId) -> bool {
        let Some(job) = self.jobs.iter().find(|job| job.id == id) else {
            return false;
        };
        if !matches!(job.status, JobStatus::Queued | JobStatus::Running) {
            return false;
        }

        let Some(flag) = self.cancel_flags.get(&id) else {
            return false;
        };
        flag.store(true, Ordering::Relaxed);
        true
    }

    pub fn newest_cancelable_job_id(&self) -> Option<JobId> {
        self.jobs
            .iter()
            .rev()
            .find(|job| job.status == JobStatus::Running)
            .or_else(|| {
                self.jobs
                    .iter()
                    .rev()
                    .find(|job| job.status == JobStatus::Queued)
            })
            .map(|job| job.id)
    }

    pub fn status_counts(&self) -> JobStatusCounts {
        let mut counts = JobStatusCounts::default();
        for job in &self.jobs {
            match job.status {
                JobStatus::Queued => counts.queued += 1,
                JobStatus::Running => counts.running += 1,
                JobStatus::Succeeded => counts.succeeded += 1,
                JobStatus::Canceled => counts.canceled += 1,
                JobStatus::Failed => counts.failed += 1,
            }
        }
        counts
    }

    pub fn jobs(&self) -> &[JobRecord] {
        &self.jobs
    }

    pub fn job(&self, id: JobId) -> Option<&JobRecord> {
        let index = *self.index_by_id.get(&id)?;
        self.jobs.get(index)
    }

    pub fn last_job(&self) -> Option<&JobRecord> {
        self.jobs.last()
    }

    fn job_mut(&mut self, id: JobId) -> Option<&mut JobRecord> {
        let index = *self.index_by_id.get(&id)?;
        self.jobs.get_mut(index)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct JobStatusCounts {
    pub queued: usize,
    pub running: usize,
    pub succeeded: usize,
    pub canceled: usize,
    pub failed: usize,
}

pub trait FsBackend {
    fn create_dir(&self, path: &Path) -> io::Result<()>;
    fn rename(&self, source: &Path, destination: &Path) -> io::Result<()>;
    fn persist_settings(&self, paths: &SettingsPaths, snapshot: &Settings) -> io::Result<()>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LocalFsBackend;

impl FsBackend for LocalFsBackend {
    fn create_dir(&self, path: &Path) -> io::Result<()> {
        fs::create_dir(path)
    }

    fn rename(&self, source: &Path, destination: &Path) -> io::Result<()> {
        fs::rename(source, destination)
    }

    fn persist_settings(&self, paths: &SettingsPaths, snapshot: &Settings) -> io::Result<()> {
        save_settings(paths, snapshot)
    }
}

pub fn run_worker(command_rx: Receiver<WorkerCommand>, event_tx: Sender<JobEvent>) {
    let fs_backend = LocalFsBackend;
    let mut queued_cancellations = HashSet::new();
    while let Ok(command) = command_rx.recv() {
        match command {
            WorkerCommand::Run(job) => {
                let job = *job;
                if queued_cancellations.remove(&job.id) {
                    job.cancel_flag.store(true, Ordering::Relaxed);
                }
                run_single_job(job, &event_tx, &fs_backend);
            }
            WorkerCommand::Cancel(id) => {
                queued_cancellations.insert(id);
            }
            WorkerCommand::Shutdown => break,
        }
    }
}

pub fn execute_worker_job(job: WorkerJob, event_tx: &Sender<JobEvent>) {
    let fs_backend = LocalFsBackend;
    run_single_job(job, event_tx, &fs_backend);
}

#[cfg(test)]
fn execute_worker_job_with_backend(
    job: WorkerJob,
    event_tx: &Sender<JobEvent>,
    fs_backend: &dyn FsBackend,
) {
    run_single_job(job, event_tx, fs_backend);
}

fn run_single_job(job: WorkerJob, event_tx: &Sender<JobEvent>, fs_backend: &dyn FsBackend) {
    let WorkerJob {
        id,
        request,
        cancel_flag,
        ..
    } = job;
    let _ = event_tx.send(JobEvent::Started { id });

    if let Err(error) = ensure_not_canceled(cancel_flag.as_ref()) {
        let _ = event_tx.send(JobEvent::Finished {
            id,
            result: Err(JobError::from_io(error)),
        });
        return;
    }

    let totals = match measure_request_totals(&request, cancel_flag.as_ref()) {
        Ok(totals) => totals,
        Err(error) => {
            let _ = event_tx.send(JobEvent::Finished {
                id,
                result: Err(JobError::from_io(error)),
            });
            return;
        }
    };

    let mut progress = ProgressTracker::new(id, totals, event_tx, cancel_flag);
    progress.emit();
    if let Err(error) = progress.ensure_not_canceled() {
        let _ = event_tx.send(JobEvent::Finished {
            id,
            result: Err(JobError::from_io(error)),
        });
        return;
    }
    let result = execute_job(request, &mut progress, fs_backend).map_err(JobError::from_io);
    if result.is_ok() {
        progress.mark_done();
    }
    let _ = event_tx.send(JobEvent::Finished { id, result });
}

fn execute_job(
    request: JobRequest,
    progress: &mut ProgressTracker<'_>,
    fs_backend: &dyn FsBackend,
) -> io::Result<()> {
    match request {
        JobRequest::Copy {
            sources,
            destination_dir,
            overwrite,
        } => copy_paths(&sources, &destination_dir, overwrite, progress),
        JobRequest::Move {
            sources,
            destination_dir,
            overwrite,
        } => move_paths(&sources, &destination_dir, overwrite, progress),
        JobRequest::Delete { targets } => delete_paths(&targets, progress),
        JobRequest::Mkdir { path } => {
            progress.set_current_path(&path);
            fs_backend.create_dir(&path)?;
            progress.complete_item(&path);
            Ok(())
        }
        JobRequest::Rename {
            source,
            destination,
        } => {
            progress.set_current_path(&source);
            fs_backend.rename(&source, &destination)?;
            progress.complete_item(&destination);
            Ok(())
        }
        JobRequest::PersistSettings { paths, snapshot } => {
            let marker = paths
                .rc_ini_path
                .as_deref()
                .or(paths.mc_ini_path.as_deref())
                .unwrap_or_else(|| Path::new("."));
            progress.set_current_path(marker);
            fs_backend.persist_settings(&paths, snapshot.as_ref())?;
            progress.complete_item(marker);
            Ok(())
        }
        JobRequest::RefreshPanel { .. } => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "panel refresh jobs are executed by the runtime adapter",
        )),
        JobRequest::Find { .. } => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "find jobs are executed by the runtime adapter",
        )),
        JobRequest::LoadViewer { .. } => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "viewer jobs are executed by the runtime adapter",
        )),
        JobRequest::BuildTree { .. } => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "tree jobs are executed by the runtime adapter",
        )),
    }
}

fn copy_paths(
    sources: &[PathBuf],
    destination_dir: &Path,
    overwrite: OverwritePolicy,
    progress: &mut ProgressTracker<'_>,
) -> io::Result<()> {
    for source in sources {
        progress.ensure_not_canceled()?;
        let destination = destination_path(source, destination_dir)?;
        let source_totals = measure_path_totals(source, progress.cancel_flag.as_ref())?;
        let Some(destination) =
            resolve_destination(source, destination, overwrite, source_totals, progress)?
        else {
            continue;
        };
        copy_path(source, &destination, progress)?;
    }
    Ok(())
}

fn move_paths(
    sources: &[PathBuf],
    destination_dir: &Path,
    overwrite: OverwritePolicy,
    progress: &mut ProgressTracker<'_>,
) -> io::Result<()> {
    for source in sources {
        progress.ensure_not_canceled()?;
        let source_totals = measure_path_totals(source, progress.cancel_flag.as_ref())?;
        let destination = destination_path(source, destination_dir)?;
        let Some(destination) =
            resolve_destination(source, destination, overwrite, source_totals, progress)?
        else {
            continue;
        };
        validate_move_destination(source, &destination)?;
        progress.set_current_path(source);
        match fs::rename(source, &destination) {
            Ok(()) => {
                progress.advance_totals(source, source_totals);
            }
            Err(error) if is_cross_device_error(&error) => {
                copy_path(source, &destination, progress)?;
                remove_path(source)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn delete_paths(targets: &[PathBuf], progress: &mut ProgressTracker<'_>) -> io::Result<()> {
    for target in targets {
        progress.ensure_not_canceled()?;
        delete_path(target, progress)?;
    }
    Ok(())
}

fn copy_path(
    source: &Path,
    destination: &Path,
    progress: &mut ProgressTracker<'_>,
) -> io::Result<()> {
    progress.ensure_not_canceled()?;
    let metadata = fs::symlink_metadata(source)?;
    progress.set_current_path(source);
    if metadata.file_type().is_symlink() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        copy_symlink(source, destination)?;
        preserve_copied_metadata(destination, &metadata)?;
        progress.complete_item(source);
        return Ok(());
    }

    if metadata.is_dir() {
        if destination.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "destination already exists: {}",
                    destination.to_string_lossy()
                ),
            ));
        }

        if destination.starts_with(source) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "cannot copy directory into itself: {} -> {}",
                    source.to_string_lossy(),
                    destination.to_string_lossy()
                ),
            ));
        }

        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let child_source = entry.path();
            let child_destination = destination.join(entry.file_name());
            copy_path(&child_source, &child_destination, progress)?;
        }
        fs::set_permissions(destination, metadata.permissions())?;
        preserve_copied_metadata(destination, &metadata)?;
        progress.complete_item(source);
        return Ok(());
    }

    if destination.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "destination already exists: {}",
                destination.to_string_lossy()
            ),
        ));
    }

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    copy_file(source, destination, progress)?;
    fs::set_permissions(destination, metadata.permissions())?;
    preserve_copied_metadata(destination, &metadata)?;
    progress.complete_item(source);
    Ok(())
}

fn copy_file(
    source: &Path,
    destination: &Path,
    progress: &mut ProgressTracker<'_>,
) -> io::Result<()> {
    let mut source_file = fs::File::open(source)?;
    let mut destination_file = fs::File::create(destination)?;
    let mut buffer = [0_u8; COPY_BUFFER_SIZE];

    loop {
        progress.ensure_not_canceled()?;
        let bytes_read = source_file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        destination_file.write_all(&buffer[..bytes_read])?;
        progress.advance_bytes(bytes_read as u64);
    }
    destination_file.flush()?;
    Ok(())
}

fn delete_path(path: &Path, progress: &mut ProgressTracker<'_>) -> io::Result<()> {
    progress.ensure_not_canceled()?;
    let metadata = fs::symlink_metadata(path)?;
    progress.set_current_path(path);

    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            delete_path(&entry.path(), progress)?;
        }
        fs::remove_dir(path)?;
        progress.complete_item(path);
        return Ok(());
    }

    let bytes = if metadata.is_file() {
        metadata.len()
    } else {
        0
    };
    fs::remove_file(path)?;
    if bytes > 0 {
        progress.advance_bytes(bytes);
    }
    progress.complete_item(path);
    Ok(())
}

fn destination_path(source: &Path, destination_dir: &Path) -> io::Result<PathBuf> {
    let Some(name) = source.file_name() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("source has no file name: {}", source.to_string_lossy()),
        ));
    };
    Ok(destination_dir.join(name))
}

fn resolve_destination(
    source: &Path,
    mut destination: PathBuf,
    overwrite: OverwritePolicy,
    source_totals: JobTotals,
    progress: &mut ProgressTracker<'_>,
) -> io::Result<Option<PathBuf>> {
    if source == destination {
        match overwrite {
            OverwritePolicy::Rename => {
                destination = renamed_destination(&destination);
            }
            OverwritePolicy::Overwrite | OverwritePolicy::Skip => {
                progress.advance_totals(source, source_totals);
                return Ok(None);
            }
        }
    }

    if destination.exists() {
        match overwrite {
            OverwritePolicy::Overwrite => {
                remove_path(&destination)?;
            }
            OverwritePolicy::Skip => {
                progress.advance_totals(source, source_totals);
                return Ok(None);
            }
            OverwritePolicy::Rename => {
                destination = renamed_destination(&destination);
            }
        }
    }

    Ok(Some(destination))
}

fn renamed_destination(destination: &Path) -> PathBuf {
    let parent = destination.parent().unwrap_or(Path::new("."));
    let file_name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("item"));

    for index in 1_usize.. {
        let suffix = if index == 1 {
            String::from("copy")
        } else {
            format!("copy{index}")
        };
        let candidate = parent.join(format!("{file_name}.{suffix}"));
        if !candidate.exists() {
            return candidate;
        }
    }

    unreachable!("rename candidate generator should always return");
}

fn validate_move_destination(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_dir() && destination.starts_with(source) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "cannot move directory into itself: {} -> {}",
                source.to_string_lossy(),
                destination.to_string_lossy()
            ),
        ));
    }
    Ok(())
}

fn copy_symlink(source: &Path, destination: &Path) -> io::Result<()> {
    let target = fs::read_link(source)?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&target, destination)
    }
    #[cfg(windows)]
    {
        let parent = source.parent().unwrap_or(Path::new("."));
        let resolved_target = if target.is_absolute() {
            target.clone()
        } else {
            parent.join(&target)
        };
        let is_dir_target = fs::metadata(&resolved_target)
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false);
        if is_dir_target {
            std::os::windows::fs::symlink_dir(&target, destination)
        } else {
            std::os::windows::fs::symlink_file(&target, destination)
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = target;
        let _ = destination;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "symlink copy is not supported on this platform",
        ))
    }
}

fn preserve_copied_metadata(destination: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    let atime = FileTime::from_last_access_time(metadata);
    let mtime = FileTime::from_last_modification_time(metadata);

    if metadata.file_type().is_symlink() {
        filetime::set_symlink_file_times(destination, atime, mtime)?;
        return Ok(());
    }

    filetime::set_file_times(destination, atime, mtime)?;
    preserve_owner_best_effort(destination, metadata)?;
    Ok(())
}

#[cfg(unix)]
fn preserve_owner_best_effort(destination: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let uid = Uid::from_raw(metadata.uid());
    let gid = Gid::from_raw(metadata.gid());
    match chown(destination, Some(uid), Some(gid)) {
        Ok(()) => Ok(()),
        Err(Errno::EPERM) | Err(Errno::EACCES) => Ok(()),
        Err(error) => Err(io::Error::other(format!(
            "failed to preserve owner/group for {}: {error}",
            destination.to_string_lossy()
        ))),
    }
}

#[cfg(not(unix))]
fn preserve_owner_best_effort(_destination: &Path, _metadata: &fs::Metadata) -> io::Result<()> {
    Ok(())
}

fn remove_path(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn is_cross_device_error(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::CrossesDevices || error.raw_os_error() == Some(18)
}

fn canceled_error() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, JOB_CANCELED_MESSAGE)
}

fn is_canceled_message(message: &str) -> bool {
    message == JOB_CANCELED_MESSAGE
}

fn ensure_not_canceled(cancel_flag: &AtomicBool) -> io::Result<()> {
    if cancel_flag.load(Ordering::Relaxed) {
        return Err(canceled_error());
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default)]
struct JobTotals {
    items: u64,
    bytes: u64,
}

fn measure_request_totals(request: &JobRequest, cancel_flag: &AtomicBool) -> io::Result<JobTotals> {
    match request {
        JobRequest::Copy { sources, .. } => measure_paths_totals(sources, cancel_flag),
        JobRequest::Move { sources, .. } => measure_paths_totals(sources, cancel_flag),
        JobRequest::Delete { targets } => measure_paths_totals(targets, cancel_flag),
        JobRequest::Mkdir { .. }
        | JobRequest::Rename { .. }
        | JobRequest::PersistSettings { .. }
        | JobRequest::RefreshPanel { .. }
        | JobRequest::LoadViewer { .. }
        | JobRequest::BuildTree { .. } => Ok(JobTotals { items: 1, bytes: 0 }),
        JobRequest::Find { .. } => Ok(JobTotals { items: 0, bytes: 0 }),
    }
}

fn measure_paths_totals(paths: &[PathBuf], cancel_flag: &AtomicBool) -> io::Result<JobTotals> {
    let mut totals = JobTotals::default();
    for path in paths {
        ensure_not_canceled(cancel_flag)?;
        let path_totals = measure_path_totals(path, cancel_flag)?;
        totals.items = totals.items.saturating_add(path_totals.items);
        totals.bytes = totals.bytes.saturating_add(path_totals.bytes);
    }
    Ok(totals)
}

fn measure_path_totals(path: &Path, cancel_flag: &AtomicBool) -> io::Result<JobTotals> {
    let mut totals = JobTotals::default();
    measure_path(path, &mut totals, cancel_flag)?;
    Ok(totals)
}

fn measure_path(path: &Path, totals: &mut JobTotals, cancel_flag: &AtomicBool) -> io::Result<()> {
    ensure_not_canceled(cancel_flag)?;
    let metadata = fs::symlink_metadata(path)?;
    totals.items = totals.items.saturating_add(1);

    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            ensure_not_canceled(cancel_flag)?;
            let entry = entry?;
            measure_path(&entry.path(), totals, cancel_flag)?;
        }
        return Ok(());
    }

    totals.bytes = totals.bytes.saturating_add(metadata.len());
    Ok(())
}

struct ProgressTracker<'a> {
    job_id: JobId,
    progress: JobProgress,
    event_tx: &'a Sender<JobEvent>,
    cancel_flag: Arc<AtomicBool>,
}

impl<'a> ProgressTracker<'a> {
    fn new(
        job_id: JobId,
        totals: JobTotals,
        event_tx: &'a Sender<JobEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Self {
        Self {
            job_id,
            progress: JobProgress {
                current_path: None,
                items_total: totals.items,
                items_done: 0,
                bytes_total: totals.bytes,
                bytes_done: 0,
            },
            event_tx,
            cancel_flag,
        }
    }

    fn emit(&self) {
        let _ = self.event_tx.send(JobEvent::Progress {
            id: self.job_id,
            progress: self.progress.clone(),
        });
    }

    fn set_current_path(&mut self, path: &Path) {
        self.progress.current_path = Some(path.to_path_buf());
        self.emit();
    }

    fn advance_bytes(&mut self, bytes: u64) {
        self.progress.bytes_done = self
            .progress
            .bytes_done
            .saturating_add(bytes)
            .min(self.progress.bytes_total);
        self.emit();
    }

    fn complete_item(&mut self, path: &Path) {
        self.progress.current_path = Some(path.to_path_buf());
        self.progress.items_done = self
            .progress
            .items_done
            .saturating_add(1)
            .min(self.progress.items_total);
        self.emit();
    }

    fn advance_totals(&mut self, path: &Path, totals: JobTotals) {
        self.progress.current_path = Some(path.to_path_buf());
        self.progress.items_done = self
            .progress
            .items_done
            .saturating_add(totals.items)
            .min(self.progress.items_total);
        self.progress.bytes_done = self
            .progress
            .bytes_done
            .saturating_add(totals.bytes)
            .min(self.progress.bytes_total);
        self.emit();
    }

    fn mark_done(&mut self) {
        self.progress.current_path = None;
        self.progress.items_done = self.progress.items_total;
        self.progress.bytes_done = self.progress.bytes_total;
        self.emit();
    }

    fn ensure_not_canceled(&self) -> io::Result<()> {
        ensure_not_canceled(self.cancel_flag.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use filetime::FileTime;
    use std::env;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc::{self, Receiver};
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn make_temp_dir(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-jobs-{label}-{stamp}"));
        fs::create_dir_all(&root).expect("temp dir should be creatable");
        root
    }

    #[cfg(unix)]
    fn reset_file_permissions_for_cleanup(path: &Path) {
        let metadata = fs::metadata(path).expect("metadata should be readable");
        let mode = metadata.permissions().mode();
        let mut permissions = metadata.permissions();
        permissions.set_mode(mode | 0o200);
        let _ = fs::set_permissions(path, permissions);
    }

    #[cfg(not(unix))]
    fn reset_file_permissions_for_cleanup(path: &Path) {
        let mut permissions = fs::metadata(path)
            .expect("metadata should be readable")
            .permissions();
        permissions.set_readonly(false);
        let _ = fs::set_permissions(path, permissions);
    }

    fn recv_until_finished(event_rx: &Receiver<JobEvent>, manager: &mut JobManager) -> JobEvent {
        loop {
            let event = event_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("worker should emit job events");
            manager.handle_event(&event);
            if matches!(event, JobEvent::Finished { .. }) {
                return event;
            }
        }
    }

    #[derive(Default)]
    struct RecordingFsBackend {
        calls: Mutex<Vec<String>>,
    }

    impl RecordingFsBackend {
        fn push_call(&self, call: &str) {
            self.calls
                .lock()
                .expect("fs backend call log should not be poisoned")
                .push(String::from(call));
        }

        fn calls(&self) -> Vec<String> {
            self.calls
                .lock()
                .expect("fs backend call log should not be poisoned")
                .clone()
        }
    }

    impl FsBackend for RecordingFsBackend {
        fn create_dir(&self, _path: &Path) -> io::Result<()> {
            self.push_call("mkdir");
            Ok(())
        }

        fn rename(&self, _source: &Path, _destination: &Path) -> io::Result<()> {
            self.push_call("rename");
            Ok(())
        }

        fn persist_settings(&self, _paths: &SettingsPaths, _snapshot: &Settings) -> io::Result<()> {
            self.push_call("persist-settings");
            Ok(())
        }
    }

    fn execute_request_with_backend(
        request: JobRequest,
        fs_backend: &dyn FsBackend,
    ) -> (JobEvent, JobStatusCounts) {
        let mut manager = JobManager::new();
        let worker_job = manager.enqueue(request);
        let (event_tx, event_rx) = mpsc::channel();
        execute_worker_job_with_backend(worker_job, &event_tx, fs_backend);
        let finished = recv_until_finished(&event_rx, &mut manager);
        (finished, manager.status_counts())
    }

    #[test]
    fn job_manager_tracks_status_and_progress() {
        let mut manager = JobManager::new();
        let job = manager.enqueue(JobRequest::Delete {
            targets: vec![PathBuf::from("/tmp/demo")],
        });

        assert_eq!(job.id, JobId(1));
        assert_eq!(manager.status_counts().queued, 1);

        manager.handle_event(&JobEvent::Started { id: job.id });
        manager.handle_event(&JobEvent::Progress {
            id: job.id,
            progress: JobProgress {
                current_path: Some(PathBuf::from("/tmp/demo")),
                items_total: 2,
                items_done: 1,
                bytes_total: 128,
                bytes_done: 64,
            },
        });

        let progress = manager
            .jobs()
            .first()
            .and_then(|record| record.progress.as_ref())
            .expect("progress should be tracked");
        assert_eq!(progress.percent(), 50);

        manager.handle_event(&JobEvent::Finished {
            id: job.id,
            result: Ok(()),
        });
        assert_eq!(manager.status_counts().succeeded, 1);
    }

    #[test]
    fn mkdir_request_uses_fs_backend() {
        let root = make_temp_dir("mkdir-backend");
        let target = root.join("created");
        let fs_backend = RecordingFsBackend::default();
        let (finished, counts) = execute_request_with_backend(
            JobRequest::Mkdir {
                path: target.clone(),
            },
            &fs_backend,
        );

        assert!(
            matches!(finished, JobEvent::Finished { result: Ok(()), .. }),
            "mkdir request should succeed through backend"
        );
        assert_eq!(counts.succeeded, 1);
        assert!(
            !target.exists(),
            "test backend should avoid touching the filesystem directly"
        );
        assert_eq!(fs_backend.calls(), vec![String::from("mkdir")]);

        fs::remove_dir_all(&root).expect("temp tree should be removable");
    }

    #[test]
    fn rename_request_uses_fs_backend() {
        let root = make_temp_dir("rename-backend");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "source").expect("source payload should be writable");
        let fs_backend = RecordingFsBackend::default();
        let (finished, counts) = execute_request_with_backend(
            JobRequest::Rename {
                source: source.clone(),
                destination: destination.clone(),
            },
            &fs_backend,
        );

        assert!(
            matches!(finished, JobEvent::Finished { result: Ok(()), .. }),
            "rename request should succeed through backend"
        );
        assert_eq!(counts.succeeded, 1);
        assert!(
            source.exists() && !destination.exists(),
            "test backend should keep source in place without filesystem rename"
        );
        assert_eq!(fs_backend.calls(), vec![String::from("rename")]);

        fs::remove_dir_all(&root).expect("temp tree should be removable");
    }

    #[test]
    fn persist_settings_request_uses_fs_backend() {
        let root = make_temp_dir("persist-backend");
        let rc_ini = root.join("rc.ini");
        let mc_ini = root.join("mc.ini");
        let fs_backend = RecordingFsBackend::default();
        let (finished, counts) = execute_request_with_backend(
            JobRequest::PersistSettings {
                paths: SettingsPaths {
                    mc_ini_path: Some(mc_ini.clone()),
                    rc_ini_path: Some(rc_ini.clone()),
                },
                snapshot: Box::new(Settings::default()),
            },
            &fs_backend,
        );

        assert!(
            matches!(finished, JobEvent::Finished { result: Ok(()), .. }),
            "persist-settings request should succeed through backend"
        );
        assert_eq!(counts.succeeded, 1);
        assert!(
            !rc_ini.exists() && !mc_ini.exists(),
            "test backend should avoid inline settings writes"
        );
        assert_eq!(fs_backend.calls(), vec![String::from("persist-settings")]);

        fs::remove_dir_all(&root).expect("temp tree should be removable");
    }

    #[test]
    fn worker_honors_cancel_flag() {
        let root = make_temp_dir("cancel");
        let source_dir = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source_dir).expect("source dir should exist");
        fs::create_dir_all(&destination).expect("destination dir should exist");

        let payload = vec![7_u8; 2 * 1024 * 1024];
        let source_file = source_dir.join("blob.bin");
        fs::write(&source_file, payload).expect("source payload should be writable");

        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = thread::spawn(move || run_worker(command_rx, event_tx));

        let mut manager = JobManager::new();
        let copy_job = manager.enqueue(JobRequest::Copy {
            sources: vec![source_file],
            destination_dir: destination,
            overwrite: OverwritePolicy::Skip,
        });
        let copy_id = copy_job.id;
        assert!(
            manager.request_cancel(copy_id),
            "cancel request should succeed for queued job"
        );
        command_tx
            .send(WorkerCommand::Run(Box::new(copy_job)))
            .expect("copy command should send");
        command_tx
            .send(WorkerCommand::Cancel(copy_id))
            .expect("cancel command should send");

        let finished = recv_until_finished(&event_rx, &mut manager);
        match finished {
            JobEvent::Finished {
                result: Err(error), ..
            } => assert!(error.is_canceled(), "finished error should be cancellation"),
            _ => panic!("job should finish with a cancellation error"),
        }
        assert_eq!(manager.status_counts().canceled, 1);

        command_tx
            .send(WorkerCommand::Shutdown)
            .expect("shutdown should send");
        worker
            .join()
            .expect("worker thread should terminate cleanly");
        fs::remove_dir_all(&root).expect("temp tree should be removable");
    }

    #[test]
    fn worker_cancel_command_marks_queued_job_as_canceled() {
        let root = make_temp_dir("queued-cancel-command");
        let source_dir = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source_dir).expect("source dir should exist");
        fs::create_dir_all(&destination).expect("destination dir should exist");

        let payload = vec![8_u8; 512 * 1024];
        let source_file = source_dir.join("blob.bin");
        fs::write(&source_file, payload).expect("source payload should be writable");

        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = thread::spawn(move || run_worker(command_rx, event_tx));

        let mut manager = JobManager::new();
        let copy_job = manager.enqueue(JobRequest::Copy {
            sources: vec![source_file],
            destination_dir: destination,
            overwrite: OverwritePolicy::Skip,
        });
        let copy_id = copy_job.id;
        command_tx
            .send(WorkerCommand::Cancel(copy_id))
            .expect("cancel command should send");
        command_tx
            .send(WorkerCommand::Run(Box::new(copy_job)))
            .expect("copy command should send");

        let finished = recv_until_finished(&event_rx, &mut manager);
        match finished {
            JobEvent::Finished {
                result: Err(error), ..
            } => assert!(error.is_canceled(), "finished error should be cancellation"),
            _ => panic!("job should finish with a cancellation error"),
        }
        assert_eq!(manager.status_counts().canceled, 1);

        command_tx
            .send(WorkerCommand::Shutdown)
            .expect("shutdown should send");
        worker
            .join()
            .expect("worker thread should terminate cleanly");
        fs::remove_dir_all(&root).expect("temp tree should be removable");
    }

    #[test]
    fn measure_request_totals_stops_when_cancel_is_requested() {
        let root = make_temp_dir("measure-cancel");
        let source_file = root.join("source.bin");
        fs::write(&source_file, vec![1_u8; 16 * 1024]).expect("source payload should be writable");

        let request = JobRequest::Delete {
            targets: vec![source_file],
        };
        let cancel_flag = Arc::new(AtomicBool::new(true));
        let error = measure_request_totals(&request, cancel_flag.as_ref())
            .expect_err("canceled preflight should fail");
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(error.to_string(), JOB_CANCELED_MESSAGE);

        fs::remove_dir_all(&root).expect("temp tree should be removable");
    }

    #[test]
    fn copy_skip_policy_preserves_existing_destination() {
        let root = make_temp_dir("skip-policy");
        let source_dir = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source_dir).expect("source dir should exist");
        fs::create_dir_all(&destination).expect("destination dir should exist");

        let source_file = source_dir.join("demo.txt");
        let destination_file = destination.join("demo.txt");
        fs::write(&source_file, "source").expect("source payload should be writable");
        fs::write(&destination_file, "destination")
            .expect("destination payload should be writable");

        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = thread::spawn(move || run_worker(command_rx, event_tx));

        let mut manager = JobManager::new();
        let copy_job = manager.enqueue(JobRequest::Copy {
            sources: vec![source_file],
            destination_dir: destination,
            overwrite: OverwritePolicy::Skip,
        });
        command_tx
            .send(WorkerCommand::Run(Box::new(copy_job)))
            .expect("copy command should send");
        let finished = recv_until_finished(&event_rx, &mut manager);
        assert!(
            matches!(finished, JobEvent::Finished { result: Ok(()), .. }),
            "copy should succeed with skip policy"
        );

        let content =
            fs::read_to_string(&destination_file).expect("destination should be readable");
        assert_eq!(
            content, "destination",
            "existing destination should be preserved"
        );

        command_tx
            .send(WorkerCommand::Shutdown)
            .expect("shutdown should send");
        worker
            .join()
            .expect("worker thread should terminate cleanly");
        fs::remove_dir_all(&root).expect("temp tree should be removable");
    }

    #[test]
    fn copy_overwrite_policy_replaces_existing_destination() {
        let root = make_temp_dir("overwrite-policy");
        let source_dir = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source_dir).expect("source dir should exist");
        fs::create_dir_all(&destination).expect("destination dir should exist");

        let source_file = source_dir.join("demo.txt");
        let destination_file = destination.join("demo.txt");
        fs::write(&source_file, "source").expect("source payload should be writable");
        fs::write(&destination_file, "destination")
            .expect("destination payload should be writable");

        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = thread::spawn(move || run_worker(command_rx, event_tx));

        let mut manager = JobManager::new();
        let copy_job = manager.enqueue(JobRequest::Copy {
            sources: vec![source_file],
            destination_dir: destination.clone(),
            overwrite: OverwritePolicy::Overwrite,
        });
        command_tx
            .send(WorkerCommand::Run(Box::new(copy_job)))
            .expect("copy command should send");
        let finished = recv_until_finished(&event_rx, &mut manager);
        assert!(
            matches!(finished, JobEvent::Finished { result: Ok(()), .. }),
            "copy should succeed with overwrite policy"
        );

        let content =
            fs::read_to_string(&destination_file).expect("destination should be readable");
        assert_eq!(content, "source", "existing destination should be replaced");

        command_tx
            .send(WorkerCommand::Shutdown)
            .expect("shutdown should send");
        worker
            .join()
            .expect("worker thread should terminate cleanly");
        fs::remove_dir_all(&root).expect("temp tree should be removable");
    }

    #[test]
    fn copy_rename_policy_creates_alternate_destination() {
        let root = make_temp_dir("rename-policy");
        let source_dir = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source_dir).expect("source dir should exist");
        fs::create_dir_all(&destination).expect("destination dir should exist");

        let source_file = source_dir.join("demo.txt");
        let destination_file = destination.join("demo.txt");
        fs::write(&source_file, "source").expect("source payload should be writable");
        fs::write(&destination_file, "destination")
            .expect("destination payload should be writable");

        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = thread::spawn(move || run_worker(command_rx, event_tx));

        let mut manager = JobManager::new();
        let copy_job = manager.enqueue(JobRequest::Copy {
            sources: vec![source_file],
            destination_dir: destination.clone(),
            overwrite: OverwritePolicy::Rename,
        });
        command_tx
            .send(WorkerCommand::Run(Box::new(copy_job)))
            .expect("copy command should send");
        let finished = recv_until_finished(&event_rx, &mut manager);
        assert!(
            matches!(finished, JobEvent::Finished { result: Ok(()), .. }),
            "copy should succeed with rename policy"
        );

        let content_original =
            fs::read_to_string(&destination_file).expect("original destination should be readable");
        assert_eq!(content_original, "destination");

        let renamed_file = destination.join("demo.txt.copy");
        let content_renamed =
            fs::read_to_string(&renamed_file).expect("renamed destination should be readable");
        assert_eq!(content_renamed, "source");

        command_tx
            .send(WorkerCommand::Shutdown)
            .expect("shutdown should send");
        worker
            .join()
            .expect("worker thread should terminate cleanly");
        fs::remove_dir_all(&root).expect("temp tree should be removable");
    }

    #[test]
    fn copy_preserves_readonly_permission_bit() {
        let root = make_temp_dir("permissions");
        let source_dir = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source_dir).expect("source dir should exist");
        fs::create_dir_all(&destination).expect("destination dir should exist");

        let source_file = source_dir.join("readonly.txt");
        fs::write(&source_file, "readonly").expect("source payload should be writable");
        let mut permissions = fs::metadata(&source_file)
            .expect("source metadata should be readable")
            .permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&source_file, permissions).expect("source should become readonly");

        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = thread::spawn(move || run_worker(command_rx, event_tx));

        let mut manager = JobManager::new();
        let copy_job = manager.enqueue(JobRequest::Copy {
            sources: vec![source_file],
            destination_dir: destination.clone(),
            overwrite: OverwritePolicy::Skip,
        });
        command_tx
            .send(WorkerCommand::Run(Box::new(copy_job)))
            .expect("copy command should send");
        let finished = recv_until_finished(&event_rx, &mut manager);
        assert!(
            matches!(finished, JobEvent::Finished { result: Ok(()), .. }),
            "copy should finish successfully"
        );

        let copied_metadata = fs::metadata(destination.join("readonly.txt"))
            .expect("copied metadata should be readable");
        assert!(
            copied_metadata.permissions().readonly(),
            "readonly bit should be preserved"
        );

        command_tx
            .send(WorkerCommand::Shutdown)
            .expect("shutdown should send");
        worker
            .join()
            .expect("worker thread should terminate cleanly");
        reset_file_permissions_for_cleanup(&root.join("source/readonly.txt"));
        fs::remove_dir_all(&root).expect("temp tree should be removable");
    }

    #[test]
    fn copy_preserves_file_modification_time() {
        let root = make_temp_dir("mtime");
        let source_dir = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source_dir).expect("source dir should exist");
        fs::create_dir_all(&destination).expect("destination dir should exist");

        let source_file = source_dir.join("mtime.txt");
        fs::write(&source_file, "mtime").expect("source payload should be writable");
        let expected_mtime = FileTime::from_unix_time(946_684_800, 0);
        filetime::set_file_mtime(&source_file, expected_mtime)
            .expect("source mtime should be settable");

        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = thread::spawn(move || run_worker(command_rx, event_tx));

        let mut manager = JobManager::new();
        let copy_job = manager.enqueue(JobRequest::Copy {
            sources: vec![source_file],
            destination_dir: destination.clone(),
            overwrite: OverwritePolicy::Skip,
        });
        command_tx
            .send(WorkerCommand::Run(Box::new(copy_job)))
            .expect("copy command should send");
        let finished = recv_until_finished(&event_rx, &mut manager);
        assert!(
            matches!(finished, JobEvent::Finished { result: Ok(()), .. }),
            "copy should finish successfully"
        );

        let copied_metadata = fs::metadata(destination.join("mtime.txt"))
            .expect("copied metadata should be readable");
        let copied_mtime = FileTime::from_last_modification_time(&copied_metadata);
        assert_eq!(
            copied_mtime.unix_seconds(),
            expected_mtime.unix_seconds(),
            "copy should preserve source mtime"
        );

        command_tx
            .send(WorkerCommand::Shutdown)
            .expect("shutdown should send");
        worker
            .join()
            .expect("worker thread should terminate cleanly");
        fs::remove_dir_all(&root).expect("temp tree should be removable");
    }

    #[cfg(unix)]
    #[test]
    fn copy_preserves_symlink_entries() {
        use std::os::unix::fs::symlink;

        let root = make_temp_dir("symlink");
        let source_dir = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source_dir).expect("source dir should exist");
        fs::create_dir_all(&destination).expect("destination dir should exist");

        fs::write(source_dir.join("target.txt"), "target").expect("target file should exist");
        symlink("target.txt", source_dir.join("link.txt")).expect("symlink should be creatable");

        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = thread::spawn(move || run_worker(command_rx, event_tx));

        let mut manager = JobManager::new();
        let copy_job = manager.enqueue(JobRequest::Copy {
            sources: vec![source_dir.join("link.txt")],
            destination_dir: destination.clone(),
            overwrite: OverwritePolicy::Skip,
        });
        command_tx
            .send(WorkerCommand::Run(Box::new(copy_job)))
            .expect("copy command should send");
        let finished = recv_until_finished(&event_rx, &mut manager);
        assert!(
            matches!(finished, JobEvent::Finished { result: Ok(()), .. }),
            "copy should finish successfully"
        );

        let copied_link = destination.join("link.txt");
        let copied_target = fs::read_link(&copied_link).expect("copied symlink should be readable");
        assert_eq!(
            copied_target,
            PathBuf::from("target.txt"),
            "symlink target should be preserved"
        );

        command_tx
            .send(WorkerCommand::Shutdown)
            .expect("shutdown should send");
        worker
            .join()
            .expect("worker thread should terminate cleanly");
        fs::remove_dir_all(&root).expect("temp tree should be removable");
    }

    #[test]
    fn move_rejects_destination_inside_source_tree() {
        let root = make_temp_dir("move-self");
        let source_root = root.join("source");
        fs::create_dir_all(source_root.join("child")).expect("source tree should exist");
        fs::write(source_root.join("child/data.txt"), "x").expect("source file should exist");

        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = thread::spawn(move || run_worker(command_rx, event_tx));

        let mut manager = JobManager::new();
        let move_job = manager.enqueue(JobRequest::Move {
            sources: vec![source_root.clone()],
            destination_dir: source_root.clone(),
            overwrite: OverwritePolicy::Skip,
        });
        command_tx
            .send(WorkerCommand::Run(Box::new(move_job)))
            .expect("move command should send");
        let finished = recv_until_finished(&event_rx, &mut manager);
        match finished {
            JobEvent::Finished {
                result: Err(error), ..
            } => assert!(
                error.message.contains("cannot move directory into itself"),
                "move should reject recursive destination"
            ),
            _ => panic!("move should fail for recursive destination"),
        }

        command_tx
            .send(WorkerCommand::Shutdown)
            .expect("shutdown should send");
        worker
            .join()
            .expect("worker thread should terminate cleanly");
        fs::remove_dir_all(&root).expect("temp tree should be removable");
    }

    #[test]
    fn worker_executes_copy_move_and_delete() {
        let root = make_temp_dir("ops");
        let source_dir = root.join("source");
        let copy_dest = root.join("copy-dest");
        let move_dest = root.join("move-dest");
        fs::create_dir_all(&source_dir).expect("source dir should exist");
        fs::create_dir_all(&copy_dest).expect("copy destination should exist");
        fs::create_dir_all(&move_dest).expect("move destination should exist");

        let source_file = source_dir.join("demo.txt");
        fs::write(&source_file, "demo").expect("source file should be writable");

        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = thread::spawn(move || run_worker(command_rx, event_tx));

        let mut manager = JobManager::new();
        let copy_job = manager.enqueue(JobRequest::Copy {
            sources: vec![source_file.clone()],
            destination_dir: copy_dest.clone(),
            overwrite: OverwritePolicy::Skip,
        });
        command_tx
            .send(WorkerCommand::Run(Box::new(copy_job)))
            .expect("copy command should send");
        let copy_done = recv_until_finished(&event_rx, &mut manager);
        assert!(
            matches!(copy_done, JobEvent::Finished { result: Ok(()), .. }),
            "copy should finish successfully"
        );
        assert!(
            copy_dest.join("demo.txt").exists(),
            "copy should create file"
        );

        let move_job = manager.enqueue(JobRequest::Move {
            sources: vec![source_file.clone()],
            destination_dir: move_dest.clone(),
            overwrite: OverwritePolicy::Skip,
        });
        command_tx
            .send(WorkerCommand::Run(Box::new(move_job)))
            .expect("move command should send");
        let move_done = recv_until_finished(&event_rx, &mut manager);
        assert!(
            matches!(move_done, JobEvent::Finished { result: Ok(()), .. }),
            "move should finish successfully"
        );
        assert!(
            !source_file.exists(),
            "move should remove source file after success"
        );
        let moved_file = move_dest.join("demo.txt");
        assert!(
            moved_file.exists(),
            "move should create file in destination"
        );

        let delete_job = manager.enqueue(JobRequest::Delete {
            targets: vec![moved_file.clone()],
        });
        command_tx
            .send(WorkerCommand::Run(Box::new(delete_job)))
            .expect("delete command should send");
        let delete_done = recv_until_finished(&event_rx, &mut manager);
        assert!(
            matches!(delete_done, JobEvent::Finished { result: Ok(()), .. }),
            "delete should finish successfully"
        );
        assert!(!moved_file.exists(), "delete should remove target file");

        command_tx
            .send(WorkerCommand::Shutdown)
            .expect("shutdown should send");
        worker
            .join()
            .expect("worker thread should terminate cleanly");
        fs::remove_dir_all(&root).expect("temp tree should be removable");
    }

    #[test]
    fn worker_emits_progress_updates_for_copy() {
        let root = make_temp_dir("progress");
        let source_dir = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source_dir).expect("source dir should exist");
        fs::create_dir_all(&destination).expect("destination dir should exist");

        let payload = vec![42_u8; 256 * 1024];
        let source_file = source_dir.join("blob.bin");
        fs::write(&source_file, payload).expect("source payload should be writable");

        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = thread::spawn(move || run_worker(command_rx, event_tx));

        let mut manager = JobManager::new();
        let copy_job = manager.enqueue(JobRequest::Copy {
            sources: vec![source_file],
            destination_dir: destination,
            overwrite: OverwritePolicy::Skip,
        });
        command_tx
            .send(WorkerCommand::Run(Box::new(copy_job)))
            .expect("copy command should send");

        let mut saw_progress = false;
        loop {
            let event = event_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("copy should emit events");
            if matches!(event, JobEvent::Progress { .. }) {
                saw_progress = true;
            }
            let finished = matches!(event, JobEvent::Finished { .. });
            manager.handle_event(&event);
            if finished {
                break;
            }
        }

        assert!(
            saw_progress,
            "copy should emit at least one progress update"
        );
        let progress = manager
            .last_job()
            .and_then(|job| job.progress.as_ref())
            .expect("job progress should be retained after completion");
        assert_eq!(progress.percent(), 100, "completed job should be at 100%");
        assert_eq!(progress.items_done, progress.items_total);
        assert_eq!(progress.bytes_done, progress.bytes_total);

        command_tx
            .send(WorkerCommand::Shutdown)
            .expect("shutdown should send");
        worker
            .join()
            .expect("worker thread should terminate cleanly");
        fs::remove_dir_all(&root).expect("temp tree should be removable");
    }

    #[test]
    fn permission_denied_maps_to_elevated_retry_hint() {
        let error = JobError::from_io(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "permission denied",
        ));
        assert_eq!(error.code, JobErrorCode::PermissionDenied);
        assert_eq!(error.retry_hint, JobRetryHint::Elevated);
    }
}
