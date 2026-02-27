use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Instant;

use anyhow::{Result, anyhow};
use rc_core::{
    AppState, BackgroundCommand, BackgroundEvent, JobError, JobEvent, WorkerCommand,
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
    Worker(WorkerCommand),
    Background(BackgroundCommand),
    Shutdown,
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
            let run_job_id = match &command {
                WorkerCommand::Run(job) => Some(job.id),
                _ => None,
            };
            match self.command_tx.try_send(RuntimeCommand::Worker(command)) {
                Ok(()) => {}
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
            match self
                .command_tx
                .try_send(RuntimeCommand::Background(command))
            {
                Ok(()) => {}
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
    let mut tasks = JoinSet::new();

    loop {
        tokio::select! {
            Some(join_result) = tasks.join_next(), if !tasks.is_empty() => {
                if let Err(error) = join_result {
                    tracing::warn!("runtime task failed: {error}");
                }
            }
            command = command_rx.recv() => {
                let Some(command) = command else {
                    break;
                };
                match command {
                    RuntimeCommand::Worker(WorkerCommand::Run(job)) => {
                        spawn_worker_task(
                            &mut tasks,
                            Arc::clone(&worker_limit),
                            shutdown.child_token(),
                            *job,
                            worker_event_tx.clone(),
                        );
                    }
                    RuntimeCommand::Worker(WorkerCommand::Cancel(_)) => {}
                    RuntimeCommand::Worker(WorkerCommand::Shutdown)
                    | RuntimeCommand::Background(BackgroundCommand::Shutdown)
                    | RuntimeCommand::Shutdown => {
                        break;
                    }
                    RuntimeCommand::Background(command) => {
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
                        );
                    }
                }
            }
        }
    }

    shutdown.cancel();
    while let Some(join_result) = tasks.join_next().await {
        if let Err(error) = join_result {
            tracing::warn!("runtime task failed during shutdown: {error}");
        }
    }
}

fn spawn_worker_task(
    tasks: &mut JoinSet<()>,
    limit: Arc<Semaphore>,
    shutdown: CancellationToken,
    worker_job: rc_core::WorkerJob,
    worker_event_tx: Sender<JobEvent>,
) {
    let job_id = worker_job.id;
    let job_kind = worker_job.request.kind().label();
    tasks.spawn(async move {
        tracing::debug!(job_id = %job_id, job_kind, "runtime worker task queued");
        let Ok(permit) = limit.acquire_owned().await else {
            return;
        };
        if shutdown.is_cancelled() {
            return;
        }
        let blocking = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let started = Instant::now();
            execute_worker_job(worker_job, &worker_event_tx);
            tracing::debug!(
                job_id = %job_id,
                job_kind,
                elapsed_ms = started.elapsed().as_millis(),
                "runtime worker task finished"
            );
        });
        if let Err(error) = blocking.await {
            tracing::warn!(job_id = %job_id, job_kind, "worker task panicked: {error}");
        }
    });
}

fn spawn_background_task(
    tasks: &mut JoinSet<()>,
    limit: Arc<Semaphore>,
    shutdown: CancellationToken,
    command: BackgroundCommand,
    background_event_tx: Sender<BackgroundEvent>,
) {
    let command_name = background_command_name(&command);
    tasks.spawn(async move {
        tracing::debug!(command = command_name, "runtime background task queued");
        let Ok(permit) = limit.acquire_owned().await else {
            return;
        };
        if shutdown.is_cancelled() {
            return;
        }
        let blocking = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let started = Instant::now();
            let _ = run_background_command_sync(command, &background_event_tx);
            tracing::debug!(
                command = command_name,
                elapsed_ms = started.elapsed().as_millis(),
                "runtime background task finished"
            );
        });
        if let Err(error) = blocking.await {
            tracing::warn!(command = command_name, "background task panicked: {error}");
        }
    });
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
