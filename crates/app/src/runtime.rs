use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Instant;

use anyhow::{Result, anyhow};
use rc_core::{
    AppState, BackgroundEvent, JobError, JobEvent, JobId, JobRequest, PanelListingSource,
    WorkerCommand, build_tree_ready_event, execute_worker_job, refresh_panel_entries,
    run_find_entries,
};
use tokio::sync::{Semaphore, mpsc as tokio_mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

const RUNTIME_COMMAND_QUEUE_CAPACITY: usize = 256;
const FS_MUTATION_CONCURRENCY_LIMIT: usize = 2;
const SETTINGS_CONCURRENCY_LIMIT: usize = 1;
const SCAN_CONCURRENCY_LIMIT: usize = 4;
const PROCESS_CONCURRENCY_LIMIT: usize = 2;

pub(crate) struct RuntimeBridge {
    command_tx: tokio_mpsc::Sender<RuntimeCommand>,
    worker_event_rx: Receiver<JobEvent>,
    background_event_rx: Receiver<BackgroundEvent>,
    runtime_handle: Option<thread::JoinHandle<Result<()>>>,
    worker_disconnected: bool,
    background_disconnected: bool,
}

#[derive(Debug)]
pub(crate) enum RuntimeCommand {
    Worker {
        command: WorkerCommand,
        queued_at: Instant,
    },
    Shutdown,
}

enum TaskCompletion {
    Worker { job_id: JobId },
}

struct WorkerCancellation {
    token: CancellationToken,
    cancel_flag: Arc<AtomicBool>,
    cancel_on_runtime_shutdown: bool,
}

struct WorkerTaskSpec {
    limit: Arc<Semaphore>,
    runtime_shutdown: CancellationToken,
    job_cancel: CancellationToken,
    worker_class: &'static str,
    worker_job: rc_core::WorkerJob,
    worker_event_tx: Sender<JobEvent>,
    background_event_tx: Sender<BackgroundEvent>,
    queued_at: Instant,
}

impl RuntimeBridge {
    pub(crate) fn spawn() -> Result<Self> {
        let (command_tx, command_rx) = tokio_mpsc::channel(RUNTIME_COMMAND_QUEUE_CAPACITY);
        let (worker_event_tx, worker_event_rx) = mpsc::channel();
        let (background_event_tx, background_event_rx) = mpsc::channel();
        let runtime_handle = thread::Builder::new()
            .name(String::from("rc-runtime"))
            .spawn(move || -> Result<()> {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .worker_threads(2)
                    .build()
                    .map_err(|error| anyhow!("failed to build runtime: {error}"))?;
                runtime.block_on(run_runtime_loop(
                    command_rx,
                    worker_event_tx,
                    background_event_tx,
                ));
                Ok(())
            })
            .map_err(|error| anyhow!("failed to spawn runtime thread: {error}"))?;

        Ok(Self {
            command_tx,
            worker_event_rx,
            background_event_rx,
            runtime_handle: Some(runtime_handle),
            worker_disconnected: false,
            background_disconnected: false,
        })
    }

    pub(crate) fn dispatch_pending_commands(&self, state: &mut AppState) {
        let mut pending_commands = state.take_pending_worker_commands().into_iter();
        while let Some(command) = pending_commands.next() {
            let command_name = worker_command_name(&command);
            let run_job_id = match &command {
                WorkerCommand::Run(job) => Some(job.id),
                _ => None,
            };
            let run_job_kind = match &command {
                WorkerCommand::Run(job) => Some(job.request.kind().label()),
                _ => None,
            };
            let queued_at = Instant::now();
            match self
                .command_tx
                .try_send(RuntimeCommand::Worker { command, queued_at })
            {
                Ok(()) => {
                    tracing::debug!(
                        runtime_event = "enqueued",
                        command_class = "worker",
                        command = command_name,
                        job_id = ?run_job_id,
                        job_kind = ?run_job_kind,
                        queue_depth = runtime_queue_depth(&self.command_tx),
                        queue_capacity = self.command_tx.max_capacity(),
                        "runtime command enqueued"
                    );
                }
                Err(tokio_mpsc::error::TrySendError::Full(runtime_command)) => {
                    let mut unsent = Vec::new();
                    if let Some(command) = handle_runtime_queue_full(state, runtime_command) {
                        unsent.push(command);
                    }
                    unsent.extend(pending_commands);
                    state.restore_pending_worker_commands(unsent);
                    break;
                }
                Err(tokio_mpsc::error::TrySendError::Closed(runtime_command)) => {
                    handle_runtime_unavailable(state, runtime_command);
                    for command in pending_commands {
                        handle_worker_unavailable(state, command);
                    }
                    break;
                }
            }
        }
    }

    pub(crate) fn drain_events(&mut self, state: &mut AppState) {
        loop {
            match self.worker_event_rx.try_recv() {
                Ok(event) => state.handle_job_event(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.worker_disconnected {
                        state.set_status("Worker channel disconnected");
                        self.worker_disconnected = true;
                    }
                    break;
                }
            }
        }

        loop {
            match self.background_event_rx.try_recv() {
                Ok(event) => state.handle_background_event(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.background_disconnected {
                        state.set_status("Background worker channel disconnected");
                        self.background_disconnected = true;
                    }
                    break;
                }
            }
        }

        self.dispatch_pending_commands(state);
    }

    pub(crate) fn shutdown(mut self) -> Result<()> {
        let _ = self.command_tx.blocking_send(RuntimeCommand::Shutdown);
        if let Some(handle) = self.runtime_handle.take() {
            handle
                .join()
                .map_err(|_| anyhow!("runtime thread panicked"))??;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn test_runtime_bridge_with_capacity(
    capacity: usize,
) -> (RuntimeBridge, tokio_mpsc::Receiver<RuntimeCommand>) {
    let (command_tx, command_rx) = tokio_mpsc::channel(capacity);
    let (_worker_event_tx, worker_event_rx) = mpsc::channel();
    let (_background_event_tx, background_event_rx) = mpsc::channel();
    (
        RuntimeBridge {
            command_tx,
            worker_event_rx,
            background_event_rx,
            runtime_handle: None,
            worker_disconnected: false,
            background_disconnected: false,
        },
        command_rx,
    )
}

#[cfg(test)]
pub(crate) fn test_runtime_bridge_with_channels(
    capacity: usize,
) -> (
    RuntimeBridge,
    tokio_mpsc::Receiver<RuntimeCommand>,
    Sender<JobEvent>,
    Sender<BackgroundEvent>,
) {
    let (command_tx, command_rx) = tokio_mpsc::channel(capacity);
    let (worker_event_tx, worker_event_rx) = mpsc::channel();
    let (background_event_tx, background_event_rx) = mpsc::channel();
    (
        RuntimeBridge {
            command_tx,
            worker_event_rx,
            background_event_rx,
            runtime_handle: None,
            worker_disconnected: false,
            background_disconnected: false,
        },
        command_rx,
        worker_event_tx,
        background_event_tx,
    )
}

async fn run_runtime_loop(
    mut command_rx: tokio_mpsc::Receiver<RuntimeCommand>,
    worker_event_tx: Sender<JobEvent>,
    background_event_tx: Sender<BackgroundEvent>,
) {
    let fs_mutation_limit = Arc::new(Semaphore::new(FS_MUTATION_CONCURRENCY_LIMIT));
    let settings_limit = Arc::new(Semaphore::new(SETTINGS_CONCURRENCY_LIMIT));
    let background_scan_limit = Arc::new(Semaphore::new(SCAN_CONCURRENCY_LIMIT));
    let background_process_limit = Arc::new(Semaphore::new(PROCESS_CONCURRENCY_LIMIT));
    let shutdown = CancellationToken::new();
    let mut worker_cancellations = HashMap::<JobId, WorkerCancellation>::new();
    let mut tasks = JoinSet::new();

    loop {
        tokio::select! {
            Some(join_result) = tasks.join_next(), if !tasks.is_empty() => {
                match join_result {
                    Ok(TaskCompletion::Worker { job_id }) => {
                        worker_cancellations.remove(&job_id);
                    }
                    Err(error) => {
                        tracing::warn!(
                            runtime_event = "task_failed",
                            error_class = "join_error",
                            "runtime task failed: {error}"
                        );
                    }
                }
            }
            command = command_rx.recv() => {
                let Some(command) = command else {
                    break;
                };
                match command {
                    RuntimeCommand::Worker {
                        command: WorkerCommand::Run(job),
                        queued_at,
                    } => {
                        let worker_job = *job;
                        let job_id = worker_job.id;
                        let cancel_flag = worker_job.cancel_flag();
                        let (limit, worker_class, cancel_on_runtime_shutdown) =
                            match &worker_job.request {
                            JobRequest::PersistSettings { .. } => {
                                (Arc::clone(&settings_limit), "settings", false)
                            }
                            JobRequest::Copy { .. }
                            | JobRequest::Move { .. }
                            | JobRequest::Delete { .. }
                            | JobRequest::Mkdir { .. }
                            | JobRequest::Rename { .. } => {
                                (Arc::clone(&fs_mutation_limit), "fs_mutation", true)
                            }
                            JobRequest::Find { .. } | JobRequest::BuildTree { .. } => {
                                (Arc::clone(&background_scan_limit), "scan", true)
                            }
                            JobRequest::LoadViewer { .. } => {
                                (Arc::clone(&background_process_limit), "process", true)
                            }
                            JobRequest::RefreshPanel {
                                source: PanelListingSource::Panelize { .. },
                                ..
                            } => (Arc::clone(&background_process_limit), "process", true),
                            JobRequest::RefreshPanel { .. } => {
                                (Arc::clone(&background_scan_limit), "scan", true)
                            }
                        };
                        let runtime_shutdown = if cancel_on_runtime_shutdown {
                            shutdown.child_token()
                        } else {
                            CancellationToken::new()
                        };
                        let job_cancel = if cancel_on_runtime_shutdown {
                            shutdown.child_token()
                        } else {
                            CancellationToken::new()
                        };
                        worker_cancellations.insert(
                            job_id,
                            WorkerCancellation {
                                token: job_cancel.clone(),
                                cancel_flag,
                                cancel_on_runtime_shutdown,
                            },
                        );
                        spawn_worker_task(
                            &mut tasks,
                            WorkerTaskSpec {
                                limit,
                                runtime_shutdown,
                                job_cancel,
                                worker_class,
                                worker_job,
                                worker_event_tx: worker_event_tx.clone(),
                                background_event_tx: background_event_tx.clone(),
                                queued_at,
                            },
                        );
                    }
                    RuntimeCommand::Worker {
                        command: WorkerCommand::Cancel(job_id),
                        queued_at,
                    } => {
                        if let Some(cancel) = worker_cancellations.get(&job_id) {
                            cancel.cancel_flag.store(true, AtomicOrdering::Relaxed);
                            cancel.token.cancel();
                            tracing::debug!(
                                runtime_event = "canceled",
                                command_class = "worker",
                                command = "cancel",
                                job_id = %job_id,
                                queue_wait_ms = queued_at.elapsed().as_millis(),
                                "runtime cancellation token triggered"
                            );
                        } else {
                            tracing::debug!(
                                runtime_event = "skipped",
                                command_class = "worker",
                                command = "cancel",
                                job_id = %job_id,
                                queue_wait_ms = queued_at.elapsed().as_millis(),
                                reason = "job already finished",
                                "runtime cancel command skipped"
                            );
                        }
                    }
                    RuntimeCommand::Worker {
                        command: WorkerCommand::Shutdown,
                        ..
                    }
                    | RuntimeCommand::Shutdown => {
                        tracing::debug!(runtime_event = "shutdown", "runtime shutdown requested");
                        break;
                    }
                }
            }
        }
    }

    shutdown.cancel();
    for cancel in worker_cancellations.values() {
        if !cancel.cancel_on_runtime_shutdown {
            continue;
        }
        cancel.cancel_flag.store(true, AtomicOrdering::Relaxed);
        cancel.token.cancel();
    }
    worker_cancellations.clear();
    while let Some(join_result) = tasks.join_next().await {
        if let Err(error) = join_result {
            tracing::warn!(
                runtime_event = "task_failed",
                error_class = "join_error",
                "runtime task failed during shutdown: {error}"
            );
        }
    }
}

fn spawn_worker_task(tasks: &mut JoinSet<TaskCompletion>, spec: WorkerTaskSpec) {
    let WorkerTaskSpec {
        limit,
        runtime_shutdown,
        job_cancel,
        worker_class,
        worker_job,
        worker_event_tx,
        background_event_tx,
        queued_at,
    } = spec;
    let job_id = worker_job.id;
    let job_kind = worker_job.request.kind().label();
    tasks.spawn(async move {
        let Ok(permit) = limit.acquire_owned().await else {
            return TaskCompletion::Worker { job_id };
        };
        let queue_wait_ms = queued_at.elapsed().as_millis();
        if runtime_shutdown.is_cancelled() || job_cancel.is_cancelled() {
            tracing::debug!(
                runtime_event = "canceled",
                command_class = "worker",
                scheduler_class = worker_class,
                job_id = %job_id,
                job_kind,
                queue_wait_ms,
                reason = if runtime_shutdown.is_cancelled() {
                    "runtime shutdown"
                } else {
                    "job cancellation token"
                },
                "runtime worker task canceled before start"
            );
            let _ = worker_event_tx.send(JobEvent::Finished {
                id: job_id,
                result: Err(JobError::canceled()),
            });
            return TaskCompletion::Worker { job_id };
        }
        let blocking = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let run_started = Instant::now();
            tracing::debug!(
                runtime_event = "started",
                command_class = "worker",
                scheduler_class = worker_class,
                job_id = %job_id,
                job_kind,
                queue_wait_ms,
                "runtime worker task started"
            );
            execute_runtime_worker_job(worker_job, &worker_event_tx, &background_event_tx);
            tracing::debug!(
                runtime_event = "finished",
                command_class = "worker",
                scheduler_class = worker_class,
                job_id = %job_id,
                job_kind,
                queue_wait_ms,
                run_time_ms = run_started.elapsed().as_millis(),
                "runtime worker task finished"
            );
        });
        if let Err(error) = blocking.await {
            tracing::warn!(
                runtime_event = "failed",
                command_class = "worker",
                scheduler_class = worker_class,
                error_class = "join_error",
                job_id = %job_id,
                job_kind,
                queue_wait_ms,
                "worker task panicked: {error}"
            );
        }
        TaskCompletion::Worker { job_id }
    });
}

fn execute_runtime_worker_job(
    worker_job: rc_core::WorkerJob,
    worker_event_tx: &Sender<JobEvent>,
    background_event_tx: &Sender<BackgroundEvent>,
) {
    let cancel_flag = worker_job.cancel_flag();
    match worker_job.request.clone() {
        JobRequest::RefreshPanel {
            panel,
            cwd,
            source,
            sort_mode,
            show_hidden_files,
            request_id,
        } => execute_refresh_worker_job(
            worker_job.id,
            panel,
            cwd,
            source,
            sort_mode,
            show_hidden_files,
            request_id,
            cancel_flag,
            worker_event_tx,
            background_event_tx,
        ),
        JobRequest::Find {
            query,
            base_dir,
            max_results,
        } => execute_find_worker_job(
            worker_job,
            worker_event_tx,
            background_event_tx,
            query,
            base_dir,
            max_results,
        ),
        JobRequest::LoadViewer { path } => execute_viewer_worker_job(
            worker_job.id,
            path,
            cancel_flag,
            worker_event_tx,
            background_event_tx,
        ),
        JobRequest::BuildTree {
            root,
            max_depth,
            max_entries,
        } => execute_tree_worker_job(
            worker_job.id,
            root,
            max_depth,
            max_entries,
            cancel_flag,
            worker_event_tx,
            background_event_tx,
        ),
        _ => execute_worker_job(worker_job, worker_event_tx),
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_refresh_worker_job(
    job_id: JobId,
    panel: rc_core::ActivePanel,
    cwd: std::path::PathBuf,
    source: PanelListingSource,
    sort_mode: rc_core::SortMode,
    show_hidden_files: bool,
    request_id: u64,
    cancel_flag: Arc<AtomicBool>,
    worker_event_tx: &Sender<JobEvent>,
    background_event_tx: &Sender<BackgroundEvent>,
) {
    let _ = worker_event_tx.send(JobEvent::Started { id: job_id });
    let refresh_result = refresh_panel_entries(
        &cwd,
        &source,
        sort_mode,
        show_hidden_files,
        cancel_flag.as_ref(),
    );
    let (event_result, result) = refresh_outcomes(refresh_result, cancel_flag.as_ref());
    let event = BackgroundEvent::PanelRefreshed {
        panel,
        cwd,
        source,
        sort_mode,
        request_id,
        result: event_result,
    };
    let delivered = background_event_tx.send(event).is_ok();
    let result = if delivered {
        result
    } else {
        Err(JobError::from_message(
            "background event channel disconnected",
        ))
    };
    let _ = worker_event_tx.send(JobEvent::Finished { id: job_id, result });
}

fn refresh_outcomes(
    refresh_result: std::io::Result<Vec<rc_core::FileEntry>>,
    cancel_flag: &AtomicBool,
) -> (
    Result<Vec<rc_core::FileEntry>, String>,
    Result<(), JobError>,
) {
    match refresh_result {
        Ok(entries) => {
            if is_canceled(cancel_flag) {
                (Ok(entries), Err(JobError::canceled()))
            } else {
                (Ok(entries), Ok(()))
            }
        }
        Err(error) => {
            let event_error = error.to_string();
            if is_canceled(cancel_flag) || error.kind() == std::io::ErrorKind::Interrupted {
                (Err(event_error), Err(JobError::canceled()))
            } else {
                (Err(event_error), Err(JobError::from_io(error)))
            }
        }
    }
}

fn execute_find_worker_job(
    worker_job: rc_core::WorkerJob,
    worker_event_tx: &Sender<JobEvent>,
    background_event_tx: &Sender<BackgroundEvent>,
    query: String,
    base_dir: std::path::PathBuf,
    max_results: usize,
) {
    let job_id = worker_job.id;
    let cancel_flag = worker_job.cancel_flag();
    let pause_flag = worker_job
        .find_pause_flag()
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
    let _ = worker_event_tx.send(JobEvent::Started { id: job_id });
    let result = run_find_entries(
        &base_dir,
        &query,
        max_results,
        cancel_flag.as_ref(),
        pause_flag.as_ref(),
        |entries| {
            background_event_tx
                .send(BackgroundEvent::FindEntriesChunk { job_id, entries })
                .is_ok()
        },
    )
    .map_err(JobError::from_message);
    let _ = worker_event_tx.send(JobEvent::Finished { id: job_id, result });
}

fn execute_viewer_worker_job(
    job_id: JobId,
    path: std::path::PathBuf,
    cancel_flag: Arc<AtomicBool>,
    worker_event_tx: &Sender<JobEvent>,
    background_event_tx: &Sender<BackgroundEvent>,
) {
    let _ = worker_event_tx.send(JobEvent::Started { id: job_id });
    if is_canceled(cancel_flag.as_ref()) {
        let _ = worker_event_tx.send(JobEvent::Finished {
            id: job_id,
            result: Err(JobError::canceled()),
        });
        return;
    }
    let viewer_result = rc_core::ViewerState::open(path.clone()).map_err(|error| error.to_string());
    if is_canceled(cancel_flag.as_ref()) {
        let _ = worker_event_tx.send(JobEvent::Finished {
            id: job_id,
            result: Err(JobError::canceled()),
        });
        return;
    }
    let _ = background_event_tx.send(BackgroundEvent::ViewerLoaded {
        path,
        result: viewer_result.clone(),
    });
    let result = viewer_result.map(|_| ()).map_err(JobError::from_message);
    let _ = worker_event_tx.send(JobEvent::Finished { id: job_id, result });
}

fn execute_tree_worker_job(
    job_id: JobId,
    root: std::path::PathBuf,
    max_depth: usize,
    max_entries: usize,
    cancel_flag: Arc<AtomicBool>,
    worker_event_tx: &Sender<JobEvent>,
    background_event_tx: &Sender<BackgroundEvent>,
) {
    let _ = worker_event_tx.send(JobEvent::Started { id: job_id });
    if is_canceled(cancel_flag.as_ref()) {
        let _ = worker_event_tx.send(JobEvent::Finished {
            id: job_id,
            result: Err(JobError::canceled()),
        });
        return;
    }
    let event = build_tree_ready_event(root, max_depth, max_entries);
    if is_canceled(cancel_flag.as_ref()) {
        let _ = worker_event_tx.send(JobEvent::Finished {
            id: job_id,
            result: Err(JobError::canceled()),
        });
        return;
    }
    let delivered = background_event_tx.send(event).is_ok();
    let result = if delivered {
        Ok(())
    } else {
        Err(JobError::from_message(
            "background event channel disconnected",
        ))
    };
    let _ = worker_event_tx.send(JobEvent::Finished { id: job_id, result });
}

fn runtime_queue_depth(command_tx: &tokio_mpsc::Sender<RuntimeCommand>) -> usize {
    command_tx
        .max_capacity()
        .saturating_sub(command_tx.capacity())
}

fn handle_runtime_queue_full(
    state: &mut AppState,
    command: RuntimeCommand,
) -> Option<WorkerCommand> {
    match command {
        RuntimeCommand::Worker { command, .. } => match command {
            WorkerCommand::Run(job) => {
                state.handle_job_dispatch_failure(
                    job.id,
                    JobError::dispatch("runtime queue is full"),
                );
                None
            }
            WorkerCommand::Cancel(_) | WorkerCommand::Shutdown => {
                state.set_status("runtime queue is full");
                Some(command)
            }
        },
        RuntimeCommand::Shutdown => {
            state.set_status("runtime queue is full");
            None
        }
    }
}

fn handle_runtime_unavailable(state: &mut AppState, command: RuntimeCommand) {
    match command {
        RuntimeCommand::Worker { command, .. } => handle_worker_unavailable(state, command),
        RuntimeCommand::Shutdown => {
            state.set_status("runtime is unavailable");
        }
    }
}

fn handle_worker_unavailable(state: &mut AppState, command: WorkerCommand) {
    match command {
        WorkerCommand::Run(job) => {
            state.handle_job_dispatch_failure(job.id, JobError::dispatch("runtime is unavailable"));
        }
        WorkerCommand::Cancel(_) | WorkerCommand::Shutdown => {
            state.set_status("runtime is unavailable");
        }
    }
}

fn is_canceled(cancel_flag: &AtomicBool) -> bool {
    cancel_flag.load(AtomicOrdering::Relaxed)
}

fn worker_command_name(command: &WorkerCommand) -> &'static str {
    match command {
        WorkerCommand::Run(_) => "run",
        WorkerCommand::Cancel(_) => "cancel",
        WorkerCommand::Shutdown => "shutdown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rc_core::{
        ActivePanel, AppState, JobErrorCode, JobManager, JobRequest, JobRetryHint,
        PanelListingSource, SortMode, settings_io,
    };
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const TEST_RUNTIME_COMMAND_QUEUE_CAPACITY: usize = 64;

    fn make_temp_dir(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-runtime-tests-{label}-{stamp}"));
        fs::create_dir_all(&root).expect("temp root should be creatable");
        root
    }

    fn spawn_runtime_loop_thread() -> (
        tokio_mpsc::Sender<RuntimeCommand>,
        Receiver<JobEvent>,
        Receiver<BackgroundEvent>,
        thread::JoinHandle<()>,
    ) {
        let (command_tx, command_rx) = tokio_mpsc::channel(TEST_RUNTIME_COMMAND_QUEUE_CAPACITY);
        let (worker_event_tx, worker_event_rx) = mpsc::channel();
        let (background_event_tx, background_event_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .worker_threads(2)
                .build()
                .expect("test runtime should build");
            runtime.block_on(run_runtime_loop(
                command_rx,
                worker_event_tx,
                background_event_tx,
            ));
        });
        (command_tx, worker_event_rx, background_event_rx, handle)
    }

    fn enqueue_paused_find_job(
        manager: &mut JobManager,
        root: &std::path::Path,
        pause_flag: Arc<AtomicBool>,
    ) -> rc_core::WorkerJob {
        let mut job = manager.enqueue(JobRequest::Find {
            query: String::from("entry"),
            base_dir: root.to_path_buf(),
            max_results: 1024,
        });
        job.set_find_pause_flag(pause_flag);
        job
    }

    fn send_run(command_tx: &tokio_mpsc::Sender<RuntimeCommand>, job: rc_core::WorkerJob) {
        command_tx
            .blocking_send(RuntimeCommand::Worker {
                command: WorkerCommand::Run(Box::new(job)),
                queued_at: Instant::now(),
            })
            .expect("worker run command should send");
    }

    fn send_cancel(command_tx: &tokio_mpsc::Sender<RuntimeCommand>, job_id: JobId) {
        command_tx
            .blocking_send(RuntimeCommand::Worker {
                command: WorkerCommand::Cancel(job_id),
                queued_at: Instant::now(),
            })
            .expect("worker cancel command should send");
    }

    fn recv_event(event_rx: &Receiver<JobEvent>, timeout: Duration) -> JobEvent {
        event_rx.recv_timeout(timeout).unwrap_or_else(|error| {
            panic!("worker event should arrive within {timeout:?}: {error}")
        })
    }

    #[test]
    fn drain_events_dispatches_deferred_persist_settings_without_input() {
        let root = make_temp_dir("deferred-save-dispatch");
        let (mut runtime, mut command_rx, worker_event_tx, _background_event_tx) =
            test_runtime_bridge_with_channels(4);
        let mut state = AppState::new(root.clone()).expect("app state should initialize");
        let settings_paths = settings_io::SettingsPaths {
            mc_ini_path: Some(root.join("mc.ini")),
            rc_ini_path: Some(root.join("settings.ini")),
        };
        let first_snapshot = state.persisted_settings_snapshot();
        let mut deferred_snapshot = state.persisted_settings_snapshot();
        deferred_snapshot.appearance.skin = String::from("deferred-save-dispatch-skin");

        let first_id = state.enqueue_worker_job_request(JobRequest::PersistSettings {
            paths: settings_paths.clone(),
            snapshot: Box::new(first_snapshot),
        });
        runtime.dispatch_pending_commands(&mut state);
        match command_rx.try_recv() {
            Ok(RuntimeCommand::Worker {
                command: WorkerCommand::Run(job),
                ..
            }) => {
                assert_eq!(job.id, first_id, "first persist request should dispatch");
            }
            Ok(other) => panic!("unexpected runtime command for first save: {other:?}"),
            Err(error) => panic!("first save request should dispatch: {error}"),
        }

        state.handle_job_event(JobEvent::Started { id: first_id });
        let deferred_id = state.enqueue_worker_job_request(JobRequest::PersistSettings {
            paths: settings_paths,
            snapshot: Box::new(deferred_snapshot.clone()),
        });
        assert_eq!(
            deferred_id, first_id,
            "deferred save should attach to active persist job id"
        );
        assert!(
            command_rx.try_recv().is_err(),
            "deferred save should stay pending until first save finishes"
        );

        worker_event_tx
            .send(JobEvent::Finished {
                id: first_id,
                result: Ok(()),
            })
            .expect("worker event injection should succeed");
        runtime.drain_events(&mut state);

        match command_rx.try_recv() {
            Ok(RuntimeCommand::Worker {
                command: WorkerCommand::Run(job),
                ..
            }) => match &job.request {
                JobRequest::PersistSettings { snapshot, .. } => {
                    assert_eq!(
                        snapshot.appearance.skin, deferred_snapshot.appearance.skin,
                        "deferred snapshot should dispatch after finish without extra input",
                    );
                }
                other => panic!("expected deferred persist request, got {other:?}"),
            },
            Ok(other) => panic!("unexpected runtime command for deferred save: {other:?}"),
            Err(error) => panic!("deferred save should dispatch from drain_events: {error}"),
        }

        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[test]
    fn shutdown_cancels_running_and_queued_find_jobs() {
        let root = make_temp_dir("shutdown-race");
        fs::write(root.join("entry.txt"), "entry").expect("fixture file should be writable");
        let pause_flag = Arc::new(AtomicBool::new(true));

        let (command_tx, worker_event_rx, _background_event_rx, runtime_handle) =
            spawn_runtime_loop_thread();
        let mut manager = JobManager::new();
        let mut job_ids = Vec::new();
        for _ in 0..5 {
            let job = enqueue_paused_find_job(&mut manager, &root, Arc::clone(&pause_flag));
            job_ids.push(job.id);
            send_run(&command_tx, job);
        }

        let mut started = Vec::new();
        while started.len() < 4 {
            let event = recv_event(&worker_event_rx, Duration::from_secs(2));
            if let JobEvent::Started { id } = event {
                started.push(id);
            }
        }

        command_tx
            .blocking_send(RuntimeCommand::Shutdown)
            .expect("runtime shutdown should send");

        let mut finished = HashMap::<JobId, JobErrorCode>::new();
        while finished.len() < job_ids.len() {
            match recv_event(&worker_event_rx, Duration::from_secs(3)) {
                JobEvent::Finished { id, result } => {
                    let error = result.expect_err("shutdown should cancel queued and running jobs");
                    finished.insert(id, error.code);
                }
                JobEvent::Started { .. } | JobEvent::Progress { .. } => {}
            }
        }
        for job_id in &job_ids {
            assert_eq!(
                finished.get(job_id),
                Some(&JobErrorCode::Canceled),
                "job {job_id} should finish as canceled during shutdown",
            );
        }

        runtime_handle
            .join()
            .expect("runtime loop thread should terminate cleanly");
        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[test]
    fn shutdown_allows_persist_settings_jobs_to_finish() {
        let root = make_temp_dir("shutdown-persist-settings");
        let rc_ini_path = root.join("settings.ini");
        let settings_paths = settings_io::SettingsPaths {
            mc_ini_path: None,
            rc_ini_path: Some(rc_ini_path.clone()),
        };

        let state = AppState::new(root.clone()).expect("app should initialize");
        let mut first_snapshot = state.persisted_settings_snapshot();
        first_snapshot.appearance.skin = "persist-shutdown-slow-".repeat(800_000);
        let mut second_snapshot = state.persisted_settings_snapshot();
        second_snapshot.appearance.skin = String::from("persist-shutdown-final");

        let (command_tx, worker_event_rx, _background_event_rx, runtime_handle) =
            spawn_runtime_loop_thread();
        let mut manager = JobManager::new();
        let first_job = manager.enqueue(JobRequest::PersistSettings {
            paths: settings_paths.clone(),
            snapshot: Box::new(first_snapshot),
        });
        let first_job_id = first_job.id;
        let second_job = manager.enqueue(JobRequest::PersistSettings {
            paths: settings_paths.clone(),
            snapshot: Box::new(second_snapshot.clone()),
        });
        let second_job_id = second_job.id;
        send_run(&command_tx, first_job);
        send_run(&command_tx, second_job);

        loop {
            let event = recv_event(&worker_event_rx, Duration::from_secs(2));
            if matches!(event, JobEvent::Started { id } if id == first_job_id) {
                break;
            }
        }

        command_tx
            .blocking_send(RuntimeCommand::Shutdown)
            .expect("runtime shutdown should send");

        let mut finished = HashMap::<JobId, Result<(), JobError>>::new();
        while finished.len() < 2 {
            match recv_event(&worker_event_rx, Duration::from_secs(20)) {
                JobEvent::Finished { id, result } => {
                    finished.insert(id, result);
                }
                JobEvent::Started { .. } | JobEvent::Progress { .. } => {}
            }
        }
        assert!(
            matches!(finished.get(&first_job_id), Some(Ok(()))),
            "first persist settings job should finish successfully during shutdown"
        );
        assert!(
            matches!(finished.get(&second_job_id), Some(Ok(()))),
            "queued persist settings job should finish successfully during shutdown"
        );

        runtime_handle
            .join()
            .expect("runtime loop thread should terminate cleanly");
        assert!(rc_ini_path.exists(), "persisted settings file should exist");
        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[test]
    fn cancel_before_start_finishes_job_as_canceled() {
        let root = make_temp_dir("cancel-before-start");
        fs::write(root.join("entry.txt"), "entry").expect("fixture file should be writable");
        let pause_flag = Arc::new(AtomicBool::new(true));

        let (command_tx, worker_event_rx, _background_event_rx, runtime_handle) =
            spawn_runtime_loop_thread();
        let mut manager = JobManager::new();
        let mut jobs = Vec::new();
        for _ in 0..4 {
            jobs.push(enqueue_paused_find_job(
                &mut manager,
                &root,
                Arc::clone(&pause_flag),
            ));
        }
        let canceled_job = enqueue_paused_find_job(&mut manager, &root, Arc::clone(&pause_flag));
        let canceled_job_id = canceled_job.id;
        let started_job_ids: Vec<JobId> = jobs.iter().map(|job| job.id).collect();
        for job in jobs {
            send_run(&command_tx, job);
        }

        let mut started = HashMap::<JobId, ()>::new();
        while started.len() < started_job_ids.len() {
            let event = recv_event(&worker_event_rx, Duration::from_secs(2));
            if let JobEvent::Started { id } = event {
                started.insert(id, ());
            }
        }

        send_run(&command_tx, canceled_job);
        send_cancel(&command_tx, canceled_job_id);

        command_tx
            .blocking_send(RuntimeCommand::Shutdown)
            .expect("runtime shutdown should send");

        let canceled_error = loop {
            match worker_event_rx.recv_timeout(Duration::from_secs(3)) {
                Ok(JobEvent::Finished { id, result }) if id == canceled_job_id => {
                    break result.expect_err("canceled queued job should fail");
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {
                    panic!("canceled queued job should finish before timeout");
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("worker event channel should remain connected until runtime stops");
                }
            }
        };
        assert_eq!(
            canceled_error.code,
            JobErrorCode::Canceled,
            "queued job canceled before start should use canceled error code"
        );
        assert!(
            !started.contains_key(&canceled_job_id),
            "queued job canceled before start should not emit a started event"
        );

        runtime_handle
            .join()
            .expect("runtime loop thread should terminate cleanly");
        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[test]
    fn cancel_during_run_finishes_job_as_canceled() {
        let root = make_temp_dir("cancel-during-run");
        fs::write(root.join("entry.txt"), "entry").expect("fixture file should be writable");
        let pause_flag = Arc::new(AtomicBool::new(true));

        let (command_tx, worker_event_rx, _background_event_rx, runtime_handle) =
            spawn_runtime_loop_thread();
        let mut manager = JobManager::new();
        let running_job = enqueue_paused_find_job(&mut manager, &root, Arc::clone(&pause_flag));
        let running_job_id = running_job.id;
        send_run(&command_tx, running_job);

        loop {
            let event = recv_event(&worker_event_rx, Duration::from_secs(2));
            if matches!(event, JobEvent::Started { id } if id == running_job_id) {
                break;
            }
        }
        send_cancel(&command_tx, running_job_id);

        let canceled_error = loop {
            let event = recv_event(&worker_event_rx, Duration::from_secs(3));
            if let JobEvent::Finished { id, result } = event
                && id == running_job_id
            {
                break result.expect_err("running canceled job should finish with an error");
            }
        };
        assert_eq!(
            canceled_error.code,
            JobErrorCode::Canceled,
            "running job canceled in-flight should use canceled error code"
        );

        command_tx
            .blocking_send(RuntimeCommand::Shutdown)
            .expect("runtime shutdown should send");
        runtime_handle
            .join()
            .expect("runtime loop thread should terminate cleanly");
        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[test]
    fn viewer_worker_reports_canceled_when_flag_is_set() {
        let root = make_temp_dir("viewer-canceled");
        let viewer_file = root.join("viewer.txt");
        fs::write(&viewer_file, "viewer").expect("viewer file should be writable");
        let cancel_flag = Arc::new(AtomicBool::new(true));
        let (worker_event_tx, worker_event_rx) = mpsc::channel();
        let (background_event_tx, background_event_rx) = mpsc::channel();

        execute_viewer_worker_job(
            JobId(1),
            viewer_file,
            cancel_flag,
            &worker_event_tx,
            &background_event_tx,
        );

        let started = recv_event(&worker_event_rx, Duration::from_secs(1));
        assert!(matches!(started, JobEvent::Started { id: JobId(1) }));
        let finished = recv_event(&worker_event_rx, Duration::from_secs(1));
        match finished {
            JobEvent::Finished {
                id: JobId(1),
                result: Err(error),
            } => {
                assert_eq!(error.code, JobErrorCode::Canceled);
            }
            other => panic!("expected canceled viewer finish event, got {other:?}"),
        }
        assert!(
            background_event_rx.try_recv().is_err(),
            "canceled viewer should not emit a background event"
        );

        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[test]
    fn tree_worker_reports_canceled_when_flag_is_set() {
        let root = make_temp_dir("tree-canceled");
        fs::write(root.join("entry.txt"), "entry").expect("tree fixture should be writable");
        let cancel_flag = Arc::new(AtomicBool::new(true));
        let (worker_event_tx, worker_event_rx) = mpsc::channel();
        let (background_event_tx, background_event_rx) = mpsc::channel();

        execute_tree_worker_job(
            JobId(1),
            root.clone(),
            2,
            64,
            cancel_flag,
            &worker_event_tx,
            &background_event_tx,
        );

        let started = recv_event(&worker_event_rx, Duration::from_secs(1));
        assert!(matches!(started, JobEvent::Started { id: JobId(1) }));
        let finished = recv_event(&worker_event_rx, Duration::from_secs(1));
        match finished {
            JobEvent::Finished {
                id: JobId(1),
                result: Err(error),
            } => {
                assert_eq!(error.code, JobErrorCode::Canceled);
            }
            other => panic!("expected canceled tree finish event, got {other:?}"),
        }
        assert!(
            background_event_rx.try_recv().is_err(),
            "canceled tree build should not emit a background event"
        );

        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[test]
    fn refresh_worker_reports_canceled_when_flag_is_set() {
        let root = make_temp_dir("refresh-canceled");
        fs::write(root.join("entry.txt"), "entry").expect("refresh fixture should be writable");
        let cancel_flag = Arc::new(AtomicBool::new(true));
        let (worker_event_tx, worker_event_rx) = mpsc::channel();
        let (background_event_tx, background_event_rx) = mpsc::channel();

        execute_refresh_worker_job(
            JobId(1),
            ActivePanel::Left,
            root.clone(),
            PanelListingSource::Directory,
            SortMode::default(),
            true,
            1,
            cancel_flag,
            &worker_event_tx,
            &background_event_tx,
        );

        let started = recv_event(&worker_event_rx, Duration::from_secs(1));
        assert!(matches!(started, JobEvent::Started { id: JobId(1) }));
        let finished = recv_event(&worker_event_rx, Duration::from_secs(1));
        match finished {
            JobEvent::Finished {
                id: JobId(1),
                result: Err(error),
            } => {
                assert_eq!(error.code, JobErrorCode::Canceled);
            }
            other => panic!("expected canceled refresh finish event, got {other:?}"),
        }
        assert!(
            matches!(
                background_event_rx.try_recv(),
                Ok(BackgroundEvent::PanelRefreshed { .. })
            ),
            "refresh path should still emit the background panel event"
        );

        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[test]
    fn refresh_outcomes_map_permission_denied_to_elevated_retry_hint() {
        let cancel_flag = AtomicBool::new(false);
        let (event_result, result) = refresh_outcomes(
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "permission denied",
            )),
            &cancel_flag,
        );
        assert!(
            matches!(event_result, Err(message) if message.contains("permission denied")),
            "background error payload should preserve process backend error context"
        );
        let error = result.expect_err("permission denied refresh should fail");
        assert_eq!(error.code, JobErrorCode::PermissionDenied);
        assert_eq!(error.retry_hint, JobRetryHint::Elevated);
    }
}
