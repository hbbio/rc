use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Instant;

use anyhow::{Result, anyhow};
use rc_core::{
    AppState, BackgroundCommand, BackgroundEvent, JobError, JobEvent, JobId, JobRequest,
    PanelListingSource, WorkerCommand, execute_worker_job, run_background_command_sync,
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
        for command in state.take_pending_worker_commands() {
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
                Err(tokio_mpsc::error::TrySendError::Full(_)) => {
                    if let Some(job_id) = run_job_id {
                        state.handle_job_dispatch_failure(
                            job_id,
                            JobError::dispatch("runtime queue is full"),
                        );
                    } else {
                        state.set_status("runtime queue is full");
                    }
                    break;
                }
                Err(tokio_mpsc::error::TrySendError::Closed(_)) => {
                    if let Some(job_id) = run_job_id {
                        state.handle_job_dispatch_failure(
                            job_id,
                            JobError::dispatch("runtime is unavailable"),
                        );
                    } else {
                        state.set_status("runtime is unavailable");
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
    let mut worker_cancellations = HashMap::<JobId, CancellationToken>::new();
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
                        let (limit, worker_class) = match &worker_job.request {
                            JobRequest::PersistSettings { .. } => {
                                (Arc::clone(&settings_limit), "settings")
                            }
                            JobRequest::Copy { .. }
                            | JobRequest::Move { .. }
                            | JobRequest::Delete { .. }
                            | JobRequest::Mkdir { .. }
                            | JobRequest::Rename { .. } => {
                                (Arc::clone(&fs_mutation_limit), "fs_mutation")
                            }
                            JobRequest::Find { .. } | JobRequest::BuildTree { .. } => {
                                (Arc::clone(&background_scan_limit), "scan")
                            }
                            JobRequest::LoadViewer { .. } => {
                                (Arc::clone(&background_process_limit), "process")
                            }
                            JobRequest::RefreshPanel {
                                source: PanelListingSource::Panelize { .. },
                                ..
                            } => (Arc::clone(&background_process_limit), "process"),
                            JobRequest::RefreshPanel { .. } => {
                                (Arc::clone(&background_scan_limit), "scan")
                            }
                        };
                        let job_cancel = shutdown.child_token();
                        worker_cancellations.insert(job_id, job_cancel.clone());
                        spawn_worker_task(
                            &mut tasks,
                            WorkerTaskSpec {
                                limit,
                                runtime_shutdown: shutdown.child_token(),
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
                            cancel.cancel();
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
        JobRequest::LoadViewer { path } => {
            execute_viewer_worker_job(worker_job.id, path, worker_event_tx, background_event_tx)
        }
        JobRequest::BuildTree {
            root,
            max_depth,
            max_entries,
        } => execute_tree_worker_job(
            worker_job.id,
            root,
            max_depth,
            max_entries,
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
    let delivered = run_background_command_sync(
        BackgroundCommand::RefreshPanel {
            panel,
            cwd,
            source,
            sort_mode,
            show_hidden_files,
            request_id,
            cancel_flag,
        },
        background_event_tx,
    );
    let result = if delivered {
        Ok(())
    } else {
        Err(JobError::from_message(
            "background event channel disconnected",
        ))
    };
    let _ = worker_event_tx.send(JobEvent::Finished { id: job_id, result });
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
    worker_event_tx: &Sender<JobEvent>,
    background_event_tx: &Sender<BackgroundEvent>,
) {
    let _ = worker_event_tx.send(JobEvent::Started { id: job_id });
    let viewer_result = rc_core::ViewerState::open(path.clone()).map_err(|error| error.to_string());
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
    worker_event_tx: &Sender<JobEvent>,
    background_event_tx: &Sender<BackgroundEvent>,
) {
    let _ = worker_event_tx.send(JobEvent::Started { id: job_id });
    let delivered = run_background_command_sync(
        BackgroundCommand::BuildTree {
            root,
            max_depth,
            max_entries,
        },
        background_event_tx,
    );
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

fn worker_command_name(command: &WorkerCommand) -> &'static str {
    match command {
        WorkerCommand::Run(_) => "run",
        WorkerCommand::Cancel(_) => "cancel",
        WorkerCommand::Shutdown => "shutdown",
    }
}
