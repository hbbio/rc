use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

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
}

impl JobKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Move => "move",
            Self::Delete => "delete",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobRequest {
    Copy {
        sources: Vec<PathBuf>,
        destination_dir: PathBuf,
    },
    Move {
        sources: Vec<PathBuf>,
        destination_dir: PathBuf,
    },
    Delete {
        targets: Vec<PathBuf>,
    },
}

impl JobRequest {
    pub fn kind(&self) -> JobKind {
        match self {
            Self::Copy { .. } => JobKind::Copy,
            Self::Move { .. } => JobKind::Move,
            Self::Delete { .. } => JobKind::Delete,
        }
    }

    pub fn item_count(&self) -> usize {
        match self {
            Self::Copy { sources, .. } => sources.len(),
            Self::Move { sources, .. } => sources.len(),
            Self::Delete { targets } => targets.len(),
        }
    }

    pub fn summary(&self) -> String {
        match self {
            Self::Copy {
                sources,
                destination_dir,
            } => format!(
                "copy {} item(s) -> {}",
                sources.len(),
                destination_dir.to_string_lossy()
            ),
            Self::Move {
                sources,
                destination_dir,
            } => format!(
                "move {} item(s) -> {}",
                sources.len(),
                destination_dir.to_string_lossy()
            ),
            Self::Delete { targets } => format!("delete {} item(s)", targets.len()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobRecord {
    pub id: JobId,
    pub kind: JobKind,
    pub summary: String,
    pub status: JobStatus,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerJob {
    pub id: JobId,
    pub request: JobRequest,
}

#[derive(Debug)]
pub enum WorkerCommand {
    Run(WorkerJob),
    Shutdown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobEvent {
    Started {
        id: JobId,
    },
    Finished {
        id: JobId,
        result: Result<(), String>,
    },
}

#[derive(Debug)]
pub struct JobManager {
    next_id: u64,
    jobs: Vec<JobRecord>,
    index_by_id: HashMap<JobId, usize>,
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
            last_error: None,
        };
        self.index_by_id.insert(id, self.jobs.len());
        self.jobs.push(record);

        WorkerJob { id, request }
    }

    pub fn handle_event(&mut self, event: &JobEvent) {
        match event {
            JobEvent::Started { id } => {
                if let Some(job) = self.job_mut(*id) {
                    job.status = JobStatus::Running;
                    job.last_error = None;
                }
            }
            JobEvent::Finished { id, result } => {
                if let Some(job) = self.job_mut(*id) {
                    match result {
                        Ok(()) => {
                            job.status = JobStatus::Succeeded;
                            job.last_error = None;
                        }
                        Err(message) => {
                            job.status = JobStatus::Failed;
                            job.last_error = Some(message.clone());
                        }
                    }
                }
            }
        }
    }

    pub fn status_counts(&self) -> JobStatusCounts {
        let mut counts = JobStatusCounts::default();
        for job in &self.jobs {
            match job.status {
                JobStatus::Queued => counts.queued += 1,
                JobStatus::Running => counts.running += 1,
                JobStatus::Succeeded => counts.succeeded += 1,
                JobStatus::Failed => counts.failed += 1,
            }
        }
        counts
    }

    pub fn jobs(&self) -> &[JobRecord] {
        &self.jobs
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
    pub failed: usize,
}

pub fn run_worker(command_rx: Receiver<WorkerCommand>, event_tx: Sender<JobEvent>) {
    while let Ok(command) = command_rx.recv() {
        match command {
            WorkerCommand::Run(job) => {
                let _ = event_tx.send(JobEvent::Started { id: job.id });
                let result = execute_job(job.request).map_err(|error| error.to_string());
                let _ = event_tx.send(JobEvent::Finished { id: job.id, result });
            }
            WorkerCommand::Shutdown => break,
        }
    }
}

fn execute_job(request: JobRequest) -> io::Result<()> {
    match request {
        JobRequest::Copy {
            sources,
            destination_dir,
        } => copy_paths(&sources, &destination_dir),
        JobRequest::Move {
            sources,
            destination_dir,
        } => move_paths(&sources, &destination_dir),
        JobRequest::Delete { targets } => delete_paths(&targets),
    }
}

fn copy_paths(sources: &[PathBuf], destination_dir: &Path) -> io::Result<()> {
    for source in sources {
        let destination = destination_path(source, destination_dir)?;
        copy_path(source, &destination)?;
    }
    Ok(())
}

fn move_paths(sources: &[PathBuf], destination_dir: &Path) -> io::Result<()> {
    for source in sources {
        let destination = destination_path(source, destination_dir)?;
        if destination.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "destination already exists: {}",
                    destination.to_string_lossy()
                ),
            ));
        }

        match fs::rename(source, &destination) {
            Ok(()) => {}
            Err(error) if is_cross_device_error(&error) => {
                copy_path(source, &destination)?;
                remove_path(source)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn delete_paths(targets: &[PathBuf]) -> io::Result<()> {
    for target in targets {
        remove_path(target)?;
    }
    Ok(())
}

fn copy_path(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "symlink copy is not implemented yet: {}",
                source.to_string_lossy()
            ),
        ));
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
            copy_path(&child_source, &child_destination)?;
        }
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
    fs::copy(source, destination)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;
    use std::sync::mpsc;
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

    #[test]
    fn job_manager_tracks_status_transitions() {
        let mut manager = JobManager::new();
        let job = manager.enqueue(JobRequest::Delete {
            targets: vec![PathBuf::from("/tmp/demo")],
        });

        assert_eq!(job.id, JobId(1));
        assert_eq!(manager.status_counts().queued, 1);

        manager.handle_event(&JobEvent::Started { id: job.id });
        assert_eq!(manager.status_counts().running, 1);

        manager.handle_event(&JobEvent::Finished {
            id: job.id,
            result: Ok(()),
        });
        assert_eq!(manager.status_counts().succeeded, 1);
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
        });
        command_tx
            .send(WorkerCommand::Run(copy_job))
            .expect("copy command should send");
        event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("copy should emit started event");
        let copy_done = event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("copy should emit finished event");
        manager.handle_event(&copy_done);
        assert!(
            copy_dest.join("demo.txt").exists(),
            "copy should create file"
        );

        let move_job = manager.enqueue(JobRequest::Move {
            sources: vec![source_file.clone()],
            destination_dir: move_dest.clone(),
        });
        command_tx
            .send(WorkerCommand::Run(move_job))
            .expect("move command should send");
        event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("move should emit started event");
        let move_done = event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("move should emit finished event");
        manager.handle_event(&move_done);
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
            .send(WorkerCommand::Run(delete_job))
            .expect("delete command should send");
        event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("delete should emit started event");
        let delete_done = event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("delete should emit finished event");
        manager.handle_event(&delete_done);
        assert!(!moved_file.exists(), "delete should remove target file");

        command_tx
            .send(WorkerCommand::Shutdown)
            .expect("shutdown should send");
        worker
            .join()
            .expect("worker thread should terminate cleanly");
        fs::remove_dir_all(&root).expect("temp tree should be removable");
    }
}
