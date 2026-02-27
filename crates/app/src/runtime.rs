use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Instant;

use anyhow::{Result, anyhow};
use rc_core::{
    AppState, BackgroundCommand, BackgroundEvent, JobError, JobEvent, JobId, WorkerCommand,
    execute_worker_job, run_background_command_sync,
};
use tokio::sync::{Semaphore, mpsc as tokio_mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

const RUNTIME_COMMAND_QUEUE_CAPACITY: usize = 256;
const WORKER_CONCURRENCY_LIMIT: usize = 2;
const BACKGROUND_SCAN_CONCURRENCY_LIMIT: usize = 4;
const BACKGROUND_VIEWER_CONCURRENCY_LIMIT: usize = 2;

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
    Background {
        command: BackgroundCommand,
        queued_at: Instant,
    },
    Shutdown,
}

enum TaskCompletion {
    Worker { job_id: JobId },
    Background,
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

        for command in state.take_pending_background_commands() {
            let command_name = background_command_name(&command);
            let queued_at = Instant::now();
            match self
                .command_tx
                .try_send(RuntimeCommand::Background { command, queued_at })
            {
                Ok(()) => {
                    tracing::debug!(
                        runtime_event = "enqueued",
                        command_class = "background",
                        command = command_name,
                        queue_depth = runtime_queue_depth(&self.command_tx),
                        queue_capacity = self.command_tx.max_capacity(),
                        "runtime command enqueued"
                    );
                }
                Err(tokio_mpsc::error::TrySendError::Full(_)) => {
                    state.set_status("runtime queue is full");
                    break;
                }
                Err(tokio_mpsc::error::TrySendError::Closed(_)) => {
                    state.set_status("runtime is unavailable");
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
    let worker_limit = Arc::new(Semaphore::new(WORKER_CONCURRENCY_LIMIT));
    let background_scan_limit = Arc::new(Semaphore::new(BACKGROUND_SCAN_CONCURRENCY_LIMIT));
    let background_viewer_limit = Arc::new(Semaphore::new(BACKGROUND_VIEWER_CONCURRENCY_LIMIT));
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
                    Ok(TaskCompletion::Background) => {}
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
                        let job_cancel = shutdown.child_token();
                        worker_cancellations.insert(job_id, job_cancel.clone());
                        spawn_worker_task(
                            &mut tasks,
                            Arc::clone(&worker_limit),
                            shutdown.child_token(),
                            job_cancel,
                            worker_job,
                            worker_event_tx.clone(),
                            queued_at,
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
                    | RuntimeCommand::Background {
                        command: BackgroundCommand::Shutdown,
                        ..
                    }
                    | RuntimeCommand::Shutdown => {
                        tracing::debug!(runtime_event = "shutdown", "runtime shutdown requested");
                        break;
                    }
                    RuntimeCommand::Background { command, queued_at } => {
                        let limit = match &command {
                            BackgroundCommand::LoadViewer { .. } => {
                                Arc::clone(&background_viewer_limit)
                            }
                            _ => Arc::clone(&background_scan_limit),
                        };
                        spawn_background_task(
                            &mut tasks,
                            limit,
                            shutdown.child_token(),
                            command,
                            background_event_tx.clone(),
                            queued_at,
                        );
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

fn spawn_worker_task(
    tasks: &mut JoinSet<TaskCompletion>,
    limit: Arc<Semaphore>,
    shutdown: CancellationToken,
    job_cancel: CancellationToken,
    worker_job: rc_core::WorkerJob,
    worker_event_tx: Sender<JobEvent>,
    queued_at: Instant,
) {
    let job_id = worker_job.id;
    let job_kind = worker_job.request.kind().label();
    tasks.spawn(async move {
        let Ok(permit) = limit.acquire_owned().await else {
            return TaskCompletion::Worker { job_id };
        };
        let queue_wait_ms = queued_at.elapsed().as_millis();
        if shutdown.is_cancelled() || job_cancel.is_cancelled() {
            tracing::debug!(
                runtime_event = "canceled",
                command_class = "worker",
                job_id = %job_id,
                job_kind,
                queue_wait_ms,
                reason = if shutdown.is_cancelled() {
                    "runtime shutdown"
                } else {
                    "job cancellation token"
                },
                "runtime worker task canceled before start"
            );
            return TaskCompletion::Worker { job_id };
        }
        let blocking = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let run_started = Instant::now();
            tracing::debug!(
                runtime_event = "started",
                command_class = "worker",
                job_id = %job_id,
                job_kind,
                queue_wait_ms,
                "runtime worker task started"
            );
            execute_worker_job(worker_job, &worker_event_tx);
            tracing::debug!(
                runtime_event = "finished",
                command_class = "worker",
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

fn spawn_background_task(
    tasks: &mut JoinSet<TaskCompletion>,
    limit: Arc<Semaphore>,
    shutdown: CancellationToken,
    command: BackgroundCommand,
    background_event_tx: Sender<BackgroundEvent>,
    queued_at: Instant,
) {
    let command_name = background_command_name(&command);
    tasks.spawn(async move {
        let Ok(permit) = limit.acquire_owned().await else {
            return TaskCompletion::Background;
        };
        let queue_wait_ms = queued_at.elapsed().as_millis();
        if shutdown.is_cancelled() {
            tracing::debug!(
                runtime_event = "canceled",
                command_class = "background",
                command = command_name,
                queue_wait_ms,
                reason = "runtime shutdown",
                "runtime background task canceled before start"
            );
            return TaskCompletion::Background;
        }
        let blocking = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let run_started = Instant::now();
            tracing::debug!(
                runtime_event = "started",
                command_class = "background",
                command = command_name,
                queue_wait_ms,
                "runtime background task started"
            );
            let _ = run_background_command_sync(command, &background_event_tx);
            tracing::debug!(
                runtime_event = "finished",
                command_class = "background",
                command = command_name,
                queue_wait_ms,
                run_time_ms = run_started.elapsed().as_millis(),
                "runtime background task finished"
            );
        });
        if let Err(error) = blocking.await {
            tracing::warn!(
                runtime_event = "failed",
                command_class = "background",
                command = command_name,
                error_class = "join_error",
                queue_wait_ms,
                "background task panicked: {error}"
            );
        }
        TaskCompletion::Background
    });
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

fn background_command_name(command: &BackgroundCommand) -> &'static str {
    match command {
        BackgroundCommand::RefreshPanel { .. } => "refresh-panel",
        BackgroundCommand::LoadViewer { .. } => "load-viewer",
        BackgroundCommand::FindEntries { .. } => "find-entries",
        BackgroundCommand::BuildTree { .. } => "build-tree",
        BackgroundCommand::Shutdown => "shutdown",
    }
}
