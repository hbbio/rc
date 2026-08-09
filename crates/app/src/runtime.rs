use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
#[cfg(any(target_os = "linux", all(test, unix)))]
use rc_core::JOB_CANCELED_MESSAGE;
use rc_core::{
    AppState, BackgroundEvent, FOUNDATION_SLO, JobError, JobEvent, JobId, JobRequest,
    PanelListingSource, PanelRefreshResult, PanelRefreshStreamRequest, WorkerCommand,
    build_tree_ready_event, execute_worker_job, read_disk_usage, run_find_entries,
    stream_refresh_panel_entries,
};
use tokio::sync::{Semaphore, mpsc as tokio_mpsc, oneshot};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

const RUNTIME_COMMAND_QUEUE_CAPACITY: usize = 256;
const RUNTIME_EVENT_DRAIN_LIMIT_PER_TICK: usize = 256;
const FS_MUTATION_CONCURRENCY_LIMIT: usize = 2;
const SETTINGS_CONCURRENCY_LIMIT: usize = 1;
const SCAN_CONCURRENCY_LIMIT: usize = 4;
const PROCESS_CONCURRENCY_LIMIT: usize = 2;
const DESKTOP_OPEN_CONCURRENCY_LIMIT: usize = 4;
#[cfg(target_os = "linux")]
const DESKTOP_PORTAL_CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
#[cfg(target_os = "linux")]
const DESKTOP_PORTAL_DESTINATION: &str = "org.freedesktop.portal.Desktop";
#[cfg(target_os = "linux")]
const DESKTOP_PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
#[cfg(target_os = "linux")]
const DESKTOP_PORTAL_OPEN_URI_INTERFACE: &str = "org.freedesktop.portal.OpenURI";
#[cfg(target_os = "linux")]
const DESKTOP_PORTAL_REQUEST_INTERFACE: &str = "org.freedesktop.portal.Request";
#[cfg(target_os = "linux")]
const DESKTOP_LAUNCHER_STARTUP_OBSERVATION: Duration = Duration::from_secs(2);
#[cfg(any(target_os = "linux", all(test, unix)))]
const DESKTOP_LAUNCHER_POLL_INTERVAL: Duration = Duration::from_millis(20);
#[cfg(any(target_os = "linux", all(test, unix)))]
const DESKTOP_LAUNCHER_TERMINATION_GRACE: Duration = Duration::from_millis(100);

pub(crate) struct RuntimeBridge {
    command_tx: tokio_mpsc::Sender<RuntimeCommand>,
    worker_event_rx: Receiver<JobEvent>,
    background_event_rx: Receiver<BackgroundEvent>,
    runtime_handle: Option<thread::JoinHandle<Result<()>>>,
    worker_disconnected: bool,
    background_disconnected: bool,
    pending_first_seen: HashMap<PendingCommandKey, Instant>,
    consecutive_full_count: u64,
    stale_pending_warned: bool,
}

#[derive(Debug)]
pub(crate) enum RuntimeCommand {
    Worker {
        command: WorkerCommand,
        queued_at: Instant,
    },
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum PendingCommandKey {
    Run(JobId),
    Cancel(JobId),
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandPriority {
    High,
    Medium,
    Low,
}

enum TaskCompletion {
    Worker { job_id: JobId },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuntimeShutdownPolicy {
    FinishActiveAndQueued,
    CancelQueuedAndReleaseActive,
    CancelQueuedAndActive,
}

impl RuntimeShutdownPolicy {
    fn cancel_queued(self) -> bool {
        !matches!(self, Self::FinishActiveAndQueued)
    }

    fn cancel_active(self) -> bool {
        matches!(self, Self::CancelQueuedAndActive)
    }

    fn release_active(self) -> bool {
        matches!(self, Self::CancelQueuedAndReleaseActive)
    }
}

struct WorkerCancellation {
    token: CancellationToken,
    cancel_flag: Arc<AtomicBool>,
    release_flag: Arc<AtomicBool>,
    runtime_shutdown_policy: RuntimeShutdownPolicy,
}

struct WorkerTaskSpec {
    limit: Arc<Semaphore>,
    runtime_shutdown: CancellationToken,
    job_cancel: CancellationToken,
    release_flag: Arc<AtomicBool>,
    run_after: Option<oneshot::Receiver<()>>,
    notify_next: Option<oneshot::Sender<()>>,
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
            pending_first_seen: HashMap::new(),
            consecutive_full_count: 0,
            stale_pending_warned: false,
        })
    }

    pub(crate) fn dispatch_pending_commands(&mut self, state: &mut AppState) {
        let pending_commands = prioritize_worker_commands(state.take_pending_worker_commands());
        if pending_commands.is_empty() {
            self.clear_pending_dispatch_metrics();
            return;
        }

        let mut pending_commands = pending_commands.into_iter();
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
            let pending_key = Self::command_key(&command);
            let queued_at = self.record_pending_command(&command, Instant::now());
            match self
                .command_tx
                .try_send(RuntimeCommand::Worker { command, queued_at })
            {
                Ok(()) => {
                    self.mark_command_dispatched(pending_key);
                    self.consecutive_full_count = 0;
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
                    self.record_queue_full_metrics();
                    match runtime_command {
                        RuntimeCommand::Worker { command, .. } => {
                            let oldest_pending_age_ms =
                                self.oldest_pending_age_ms(Instant::now()).unwrap_or(0);
                            if should_drop_for_backpressure(
                                &command,
                                self.consecutive_full_count,
                                oldest_pending_age_ms,
                            ) {
                                self.mark_command_dispatched(pending_key);
                                finalize_backpressure_drop(state, command);
                                state.set_status(
                                    "runtime queue saturated; dropped low-priority background job",
                                );
                                tracing::warn!(
                                    runtime_event = "queue_drop",
                                    command = command_name,
                                    job_id = ?run_job_id,
                                    job_kind = ?run_job_kind,
                                    consecutive_full_count = self.consecutive_full_count,
                                    oldest_pending_age_ms,
                                    "dropped low-priority runtime command under backpressure"
                                );
                            } else {
                                state.set_status("runtime queue is full; retrying");
                                unsent.push(command);
                            }
                        }
                        RuntimeCommand::Shutdown => {
                            state.set_status("runtime queue is full");
                        }
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
                    self.clear_pending_dispatch_metrics();
                    break;
                }
            }
        }
    }

    pub(crate) fn drain_events(&mut self, state: &mut AppState) {
        let drain_started = Instant::now();
        let mut drained_events = 0_usize;
        while drained_events < RUNTIME_EVENT_DRAIN_LIMIT_PER_TICK
            && drain_started.elapsed() < FOUNDATION_SLO.ui_frame_budget
        {
            let mut progressed = false;

            if !self.worker_disconnected {
                match self.worker_event_rx.try_recv() {
                    Ok(event) => {
                        state.handle_job_event(event);
                        drained_events = drained_events.saturating_add(1);
                        progressed = true;
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        state.set_status("Worker channel disconnected");
                        self.worker_disconnected = true;
                    }
                }
            }

            if drained_events >= RUNTIME_EVENT_DRAIN_LIMIT_PER_TICK
                || drain_started.elapsed() >= FOUNDATION_SLO.ui_frame_budget
            {
                break;
            }

            if !self.background_disconnected {
                match self.background_event_rx.try_recv() {
                    Ok(event) => {
                        state.handle_background_event(event);
                        drained_events = drained_events.saturating_add(1);
                        progressed = true;
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        state.set_status("Background worker channel disconnected");
                        self.background_disconnected = true;
                    }
                }
            }

            if !progressed {
                break;
            }
        }

        let elapsed = drain_started.elapsed();
        if drained_events >= RUNTIME_EVENT_DRAIN_LIMIT_PER_TICK
            || elapsed >= FOUNDATION_SLO.ui_frame_budget
        {
            tracing::debug!(
                runtime_event = "drain_budget_exhausted",
                drained_events,
                elapsed_ms = elapsed.as_millis(),
                event_limit = RUNTIME_EVENT_DRAIN_LIMIT_PER_TICK,
                frame_budget_ms = FOUNDATION_SLO.ui_frame_budget.as_millis(),
                "runtime event draining stopped at per-tick budget"
            );
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

    fn command_key(command: &WorkerCommand) -> PendingCommandKey {
        match command {
            WorkerCommand::Run(job) => PendingCommandKey::Run(job.id),
            WorkerCommand::Cancel(id) => PendingCommandKey::Cancel(*id),
            WorkerCommand::Shutdown => PendingCommandKey::Shutdown,
        }
    }

    fn record_pending_command(&mut self, command: &WorkerCommand, now: Instant) -> Instant {
        let key = Self::command_key(command);
        *self.pending_first_seen.entry(key).or_insert(now)
    }

    fn mark_command_dispatched(&mut self, key: PendingCommandKey) {
        self.pending_first_seen.remove(&key);
        if self.pending_first_seen.is_empty() {
            self.stale_pending_warned = false;
        }
    }

    fn oldest_pending_age_ms(&self, now: Instant) -> Option<u128> {
        self.pending_first_seen
            .values()
            .map(|first_seen| now.saturating_duration_since(*first_seen).as_millis())
            .max()
    }

    fn record_queue_full_metrics(&mut self) {
        self.consecutive_full_count = self.consecutive_full_count.saturating_add(1);
        let now = Instant::now();
        let oldest_pending_age_ms = self.oldest_pending_age_ms(now).unwrap_or(0);
        let stale_threshold_ms = FOUNDATION_SLO.queue_stale_warn_after.as_millis();
        let is_stale = FOUNDATION_SLO.is_queue_stale(Duration::from_millis(
            oldest_pending_age_ms.min(u64::MAX as u128) as u64,
        ));
        tracing::debug!(
            runtime_event = "queue_full",
            consecutive_full_count = self.consecutive_full_count,
            oldest_pending_age_ms,
            stale_threshold_ms,
            "runtime command queue is full"
        );
        if is_stale {
            if !self.stale_pending_warned {
                tracing::warn!(
                    runtime_event = "queue_stale",
                    consecutive_full_count = self.consecutive_full_count,
                    oldest_pending_age_ms,
                    stale_threshold_ms,
                    "runtime pending queue has become stale"
                );
                self.stale_pending_warned = true;
            }
        } else {
            self.stale_pending_warned = false;
        }
    }

    fn clear_pending_dispatch_metrics(&mut self) {
        self.pending_first_seen.clear();
        self.consecutive_full_count = 0;
        self.stale_pending_warned = false;
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
            pending_first_seen: HashMap::new(),
            consecutive_full_count: 0,
            stale_pending_warned: false,
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
            pending_first_seen: HashMap::new(),
            consecutive_full_count: 0,
            stale_pending_warned: false,
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
    let desktop_open_limit = Arc::new(Semaphore::new(DESKTOP_OPEN_CONCURRENCY_LIMIT));
    let shutdown = CancellationToken::new();
    let mut settings_sequence_tail = None;
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
                        let runtime_shutdown_policy =
                            worker_runtime_shutdown_policy(&worker_job.request);
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
                            JobRequest::Find { .. }
                            | JobRequest::QuickCdSearch { .. }
                            | JobRequest::MeasureSelection { .. }
                            | JobRequest::BuildTree { .. } => {
                                (Arc::clone(&background_scan_limit), "scan")
                            }
                            JobRequest::OpenDesktop { .. } => {
                                (Arc::clone(&desktop_open_limit), "desktop_open")
                            }
                            JobRequest::LoadViewer { .. }
                            | JobRequest::LoadQuickView { .. } => {
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
                        let runtime_shutdown = if runtime_shutdown_policy.cancel_queued() {
                            shutdown.child_token()
                        } else {
                            CancellationToken::new()
                        };
                        let job_cancel = CancellationToken::new();
                        let release_flag = Arc::new(AtomicBool::new(false));
                        let (run_after, notify_next) = if matches!(
                            &worker_job.request,
                            JobRequest::PersistSettings { .. }
                        ) {
                            let run_after = settings_sequence_tail.take();
                            let (notify_next, next_tail) = oneshot::channel();
                            settings_sequence_tail = Some(next_tail);
                            (run_after, Some(notify_next))
                        } else {
                            (None, None)
                        };
                        worker_cancellations.insert(
                            job_id,
                            WorkerCancellation {
                                token: job_cancel.clone(),
                                cancel_flag,
                                release_flag: Arc::clone(&release_flag),
                                runtime_shutdown_policy,
                            },
                        );
                        spawn_worker_task(
                            &mut tasks,
                            WorkerTaskSpec {
                                limit,
                                runtime_shutdown,
                                job_cancel,
                                release_flag,
                                run_after,
                                notify_next,
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
        if cancel.runtime_shutdown_policy.release_active() {
            cancel.release_flag.store(true, AtomicOrdering::Relaxed);
            continue;
        }
        if !cancel.runtime_shutdown_policy.cancel_active() {
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

fn worker_runtime_shutdown_policy(request: &JobRequest) -> RuntimeShutdownPolicy {
    match request {
        JobRequest::PersistSettings { .. } => RuntimeShutdownPolicy::FinishActiveAndQueued,
        // A queued opener has not launched anything and can be canceled. An active portal request
        // is closed, while an active legacy launcher is handed to the independent reaper; runtime
        // shutdown must never turn into an application kill.
        JobRequest::OpenDesktop { .. } => RuntimeShutdownPolicy::CancelQueuedAndReleaseActive,
        _ => RuntimeShutdownPolicy::CancelQueuedAndActive,
    }
}

fn spawn_worker_task(tasks: &mut JoinSet<TaskCompletion>, spec: WorkerTaskSpec) {
    let WorkerTaskSpec {
        limit,
        runtime_shutdown,
        job_cancel,
        release_flag,
        run_after,
        notify_next,
        worker_class,
        worker_job,
        worker_event_tx,
        background_event_tx,
        queued_at,
    } = spec;
    let job_id = worker_job.id;
    let job_kind = worker_job.request.kind().label();
    tasks.spawn(async move {
        let _sequence_completion = SequenceCompletion::new(notify_next);
        if let Some(predecessor) = run_after {
            let _ = predecessor.await;
        }
        let permit = tokio::select! {
            _ = runtime_shutdown.cancelled() => {
                tracing::debug!(
                    runtime_event = "canceled",
                    command_class = "worker",
                    scheduler_class = worker_class,
                    job_id = %job_id,
                    job_kind,
                    queue_wait_ms = queued_at.elapsed().as_millis(),
                    reason = "runtime shutdown",
                    "runtime worker task canceled while waiting for scheduler permit"
                );
                finish_canceled_worker_before_start(
                    &worker_job,
                    &worker_event_tx,
                    &background_event_tx,
                );
                return TaskCompletion::Worker { job_id };
            }
            _ = job_cancel.cancelled() => {
                tracing::debug!(
                    runtime_event = "canceled",
                    command_class = "worker",
                    scheduler_class = worker_class,
                    job_id = %job_id,
                    job_kind,
                    queue_wait_ms = queued_at.elapsed().as_millis(),
                    reason = "job cancellation token",
                    "runtime worker task canceled while waiting for scheduler permit"
                );
                finish_canceled_worker_before_start(
                    &worker_job,
                    &worker_event_tx,
                    &background_event_tx,
                );
                return TaskCompletion::Worker { job_id };
            }
            permit = limit.acquire_owned() => {
                let Ok(permit) = permit else {
                    return TaskCompletion::Worker { job_id };
                };
                permit
            }
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
            finish_canceled_worker_before_start(
                &worker_job,
                &worker_event_tx,
                &background_event_tx,
            );
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
            execute_runtime_worker_job(
                worker_job,
                release_flag,
                &worker_event_tx,
                &background_event_tx,
            );
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

fn finish_canceled_worker_before_start(
    worker_job: &rc_core::WorkerJob,
    worker_event_tx: &Sender<JobEvent>,
    background_event_tx: &Sender<BackgroundEvent>,
) {
    if let JobRequest::RefreshPanel {
        panel,
        cwd,
        source,
        sort_mode,
        filter,
        show_hidden_files,
        cached_panelized_entries,
        request_id,
    } = &worker_job.request
    {
        let request = PanelRefreshStreamRequest {
            panel: *panel,
            cwd: cwd.clone(),
            source: source.clone(),
            sort_mode: *sort_mode,
            filter: filter.clone(),
            show_hidden_files: *show_hidden_files,
            cached_panelized_entries: cached_panelized_entries.clone(),
            request_id: *request_id,
        };
        let _ = background_event_tx.send(request.canceled_event());
    }
    let _ = worker_event_tx.send(JobEvent::Finished {
        id: worker_job.id,
        result: Err(JobError::canceled()),
    });
}

struct SequenceCompletion(Option<oneshot::Sender<()>>);

impl SequenceCompletion {
    fn new(sender: Option<oneshot::Sender<()>>) -> Self {
        Self(sender)
    }
}

impl Drop for SequenceCompletion {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

fn execute_runtime_worker_job(
    worker_job: rc_core::WorkerJob,
    release_flag: Arc<AtomicBool>,
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
            filter,
            show_hidden_files,
            cached_panelized_entries,
            request_id,
        } => execute_refresh_worker_job(
            worker_job.id,
            PanelRefreshStreamRequest {
                panel,
                cwd,
                source,
                sort_mode,
                filter,
                show_hidden_files,
                cached_panelized_entries,
                request_id,
            },
            cancel_flag,
            worker_event_tx,
            background_event_tx,
        ),
        JobRequest::Find { spec, max_results } => execute_find_worker_job(
            worker_job,
            worker_event_tx,
            background_event_tx,
            spec,
            max_results,
        ),
        JobRequest::QuickCdSearch { spec, request_id } => execute_quick_cd_search_worker_job(
            worker_job.id,
            spec,
            request_id,
            cancel_flag,
            worker_event_tx,
            background_event_tx,
        ),
        JobRequest::OpenDesktop { path } => execute_desktop_open_worker_job(
            worker_job.id,
            path,
            cancel_flag,
            release_flag,
            worker_event_tx,
            background_event_tx,
        ),
        JobRequest::LoadViewer { path } => execute_viewer_worker_job(
            worker_job.id,
            path,
            cancel_flag,
            worker_event_tx,
            background_event_tx,
        ),
        JobRequest::LoadQuickView {
            panel,
            path,
            request_id,
        } => execute_quick_view_worker_job(
            worker_job.id,
            panel,
            path,
            request_id,
            cancel_flag,
            worker_event_tx,
            background_event_tx,
        ),
        JobRequest::MeasureSelection {
            panel,
            paths,
            request_id,
        } => execute_selection_size_worker_job(
            worker_job.id,
            panel,
            paths,
            request_id,
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

fn execute_desktop_open_worker_job(
    job_id: JobId,
    path: std::path::PathBuf,
    cancel_flag: Arc<AtomicBool>,
    release_flag: Arc<AtomicBool>,
    worker_event_tx: &Sender<JobEvent>,
    background_event_tx: &Sender<BackgroundEvent>,
) {
    execute_desktop_open_worker(
        job_id,
        path,
        cancel_flag,
        release_flag,
        worker_event_tx,
        background_event_tx,
        open_with_default_application,
    );
}

// On macOS, `/usr/bin/open` is short-lived but reports Launch Services errors only when we wait
// for it. Linux prefers the response-bearing desktop portal, then observes the complete legacy
// opener chain for a bounded startup window before handing attached helpers to a reaper. Windows
// uses the native ShellExecuteExW implementation enabled for the `open` dependency.
#[cfg(target_os = "macos")]
fn open_with_default_application(
    path: &std::path::Path,
    _cancel_flag: &AtomicBool,
    _release_flag: &AtomicBool,
) -> std::io::Result<()> {
    open::that(path)
}

#[cfg(target_os = "linux")]
fn open_with_default_application(
    path: &std::path::Path,
    cancel_flag: &AtomicBool,
    release_flag: &AtomicBool,
) -> std::io::Result<()> {
    let portal_error = match open_with_desktop_portal(path, cancel_flag, release_flag) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => return Err(error),
        Err(error) => error,
    };

    run_status_aware_launcher_commands(
        open::commands(path),
        cancel_flag,
        release_flag,
        DESKTOP_LAUNCHER_STARTUP_OBSERVATION,
    )
    .map_err(|launcher_error| {
        std::io::Error::new(
            launcher_error.kind(),
            format!(
                "desktop portal failed: {portal_error}; launcher fallback failed: \
                 {launcher_error}"
            ),
        )
    })
}

#[cfg(target_os = "linux")]
fn open_with_desktop_portal(
    path: &std::path::Path,
    cancel_flag: &AtomicBool,
    release_flag: &AtomicBool,
) -> std::io::Result<()> {
    use std::os::fd::AsFd as _;

    use futures_util::StreamExt as _;
    use zbus::zvariant::{Fd, OwnedObjectPath, OwnedValue, Value};

    if cancel_flag.load(AtomicOrdering::Relaxed) || release_flag.load(AtomicOrdering::Relaxed) {
        return Err(desktop_launcher_canceled_error());
    }
    let (file, writeable) = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    {
        Ok(file) => (file, true),
        Err(_) => (std::fs::File::open(path)?, false),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let connection = zbus::Connection::session()
            .await
            .map_err(|error| desktop_portal_error("failed to connect to the session bus", error))?;
        let handle_token = desktop_portal_handle_token()?;
        let expected_path = desktop_portal_request_path(&connection, &handle_token)?;

        // Subscribe at the predictable request path before OpenFile to avoid missing a fast
        // Response signal. The portal specification permits a different returned path for older
        // implementations, which is handled below.
        let mut request_proxy =
            desktop_portal_request_proxy(&connection, expected_path.clone()).await?;
        let mut responses = request_proxy
            .receive_signal("Response")
            .await
            .map_err(|error| {
                desktop_portal_error("failed to subscribe to the portal response", error)
            })?;
        let open_proxy = zbus::Proxy::new_owned(
            connection.clone(),
            String::from(DESKTOP_PORTAL_DESTINATION),
            String::from(DESKTOP_PORTAL_PATH),
            String::from(DESKTOP_PORTAL_OPEN_URI_INTERFACE),
        )
        .await
        .map_err(|error| desktop_portal_error("failed to create the OpenURI proxy", error))?;
        let mut options = HashMap::<&str, Value<'_>>::new();
        options.insert("handle_token", Value::from(handle_token.as_str()));
        options.insert("writable", Value::from(writeable));
        let arguments = ("", Fd::from(file.as_fd()), options);
        let open_request = open_proxy.call::<_, _, OwnedObjectPath>("OpenFile", &arguments);
        tokio::pin!(open_request);
        let returned_path = tokio::select! {
            result = &mut open_request => match result {
                Ok(path) => path,
                Err(error) => {
                    close_desktop_portal_request(&request_proxy).await;
                    return Err(desktop_portal_error("OpenFile request failed", error));
                }
            },
            () = wait_for_desktop_portal_cancellation(cancel_flag, release_flag) => {
                close_desktop_portal_request(&request_proxy).await;
                return Err(desktop_launcher_canceled_error());
            }
        };

        if returned_path != expected_path {
            let replacement_proxy =
                desktop_portal_request_proxy(&connection, returned_path).await?;
            let replacement_responses =
                replacement_proxy
                    .receive_signal("Response")
                    .await
                    .map_err(|error| {
                        desktop_portal_error(
                            "failed to subscribe at the returned request path",
                            error,
                        )
                    });
            match replacement_responses {
                Ok(replacement_responses) => {
                    request_proxy = replacement_proxy;
                    responses = replacement_responses;
                }
                Err(error) => {
                    close_desktop_portal_request(&replacement_proxy).await;
                    return Err(error);
                }
            }
        }

        let response = tokio::select! {
            response = responses.next() => response.ok_or_else(|| {
                std::io::Error::other("desktop portal response stream ended unexpectedly")
            })?,
            () = wait_for_desktop_portal_cancellation(cancel_flag, release_flag) => {
                close_desktop_portal_request(&request_proxy).await;
                return Err(desktop_launcher_canceled_error());
            }
        };
        let (response_code, _results): (u32, HashMap<String, OwnedValue>) = response
            .body()
            .deserialize()
            .map_err(|error| desktop_portal_error("invalid portal response", error))?;
        match response_code {
            0 => Ok(()),
            1 => Err(desktop_launcher_canceled_error()),
            2 => Err(std::io::Error::other(
                "desktop portal interaction did not succeed",
            )),
            code => Err(std::io::Error::other(format!(
                "desktop portal returned unknown response code {code}"
            ))),
        }
    })
}

#[cfg(target_os = "linux")]
fn desktop_portal_handle_token() -> std::io::Result<String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(|error| {
        std::io::Error::other(format!(
            "failed to generate a portal request token: {error}"
        ))
    })?;
    let mut token = String::with_capacity(3 + random.len() * 2);
    token.push_str("rc_");
    for byte in random {
        token.push(char::from(HEX[usize::from(byte >> 4)]));
        token.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(token)
}

#[cfg(target_os = "linux")]
fn desktop_portal_request_path(
    connection: &zbus::Connection,
    handle_token: &str,
) -> std::io::Result<zbus::zvariant::OwnedObjectPath> {
    let sender = connection
        .unique_name()
        .ok_or_else(|| std::io::Error::other("session bus did not assign a unique name"))?
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    zbus::zvariant::OwnedObjectPath::try_from(format!(
        "/org/freedesktop/portal/desktop/request/{sender}/{handle_token}"
    ))
    .map_err(|error| desktop_portal_error("failed to construct the portal request path", error))
}

#[cfg(target_os = "linux")]
async fn desktop_portal_request_proxy(
    connection: &zbus::Connection,
    path: zbus::zvariant::OwnedObjectPath,
) -> std::io::Result<zbus::Proxy<'static>> {
    zbus::Proxy::new_owned(
        connection.clone(),
        String::from(DESKTOP_PORTAL_DESTINATION),
        path,
        String::from(DESKTOP_PORTAL_REQUEST_INTERFACE),
    )
    .await
    .map_err(|error| desktop_portal_error("failed to create the portal request proxy", error))
}

#[cfg(target_os = "linux")]
async fn close_desktop_portal_request(request: &zbus::Proxy<'_>) {
    match tokio::time::timeout(
        DESKTOP_PORTAL_CLOSE_TIMEOUT,
        request.call_method("Close", &()),
    )
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => tracing::debug!(
            runtime_event = "desktop_portal_close_failed",
            "failed to close desktop portal request: {error}"
        ),
        Err(_) => tracing::debug!(
            runtime_event = "desktop_portal_close_timed_out",
            "timed out while closing desktop portal request"
        ),
    }
}

#[cfg(target_os = "linux")]
fn desktop_portal_error(context: &str, error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(format!("{context}: {error}"))
}

#[cfg(target_os = "linux")]
async fn wait_for_desktop_portal_cancellation(cancel_flag: &AtomicBool, release_flag: &AtomicBool) {
    while !cancel_flag.load(AtomicOrdering::Relaxed) && !release_flag.load(AtomicOrdering::Relaxed)
    {
        tokio::time::sleep(DESKTOP_LAUNCHER_POLL_INTERVAL).await;
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn open_with_default_application(
    path: &std::path::Path,
    _cancel_flag: &AtomicBool,
    _release_flag: &AtomicBool,
) -> std::io::Result<()> {
    open::that_detached(path)
}

#[cfg(any(target_os = "linux", all(test, unix)))]
fn run_status_aware_launcher_commands(
    commands: Vec<std::process::Command>,
    cancel_flag: &AtomicBool,
    release_flag: &AtomicBool,
    startup_observation: Duration,
) -> std::io::Result<()> {
    let mut failures = Vec::new();
    for mut command in commands {
        if cancel_flag.load(AtomicOrdering::Relaxed) {
            return Err(desktop_launcher_canceled_error());
        }
        if release_flag.load(AtomicOrdering::Relaxed) {
            return Err(desktop_launcher_canceled_error());
        }

        let launcher = command.get_program().to_string_lossy().into_owned();
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;

            command.process_group(0);
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                failures.push(format!("{launcher}: {error}"));
                continue;
            }
        };
        let started = Instant::now();
        loop {
            if cancel_flag.load(AtomicOrdering::Relaxed) {
                terminate_desktop_launcher(&mut child);
                return Err(desktop_launcher_canceled_error());
            }
            if release_flag.load(AtomicOrdering::Relaxed) {
                handoff_desktop_launcher_to_reaper(child, launcher);
                return Err(desktop_launcher_canceled_error());
            }

            match child.try_wait() {
                Ok(Some(status)) if status.success() => return Ok(()),
                Ok(Some(status)) => {
                    failures.push(format!("{launcher}: exited with {status}"));
                    break;
                }
                Ok(None) if started.elapsed() < startup_observation => {
                    thread::sleep(
                        DESKTOP_LAUNCHER_POLL_INTERVAL
                            .min(startup_observation.saturating_sub(started.elapsed())),
                    );
                }
                // Legacy launchers have no acceptance handshake and may remain attached for the
                // application's complete lifetime. Immediate failures have now had a bounded
                // observation window; transfer ownership to a reaper that is independent of the
                // worker scheduler and runtime shutdown.
                Ok(None) => {
                    handoff_desktop_launcher_to_reaper(child, launcher);
                    return Ok(());
                }
                Err(error) => {
                    terminate_desktop_launcher(&mut child);
                    failures.push(format!("{launcher}: status check failed: {error}"));
                    break;
                }
            }
        }
    }

    let detail = if failures.is_empty() {
        String::from("no desktop launchers are available")
    } else {
        failures.join("; ")
    };
    Err(std::io::Error::other(format!(
        "no desktop launcher accepted the file ({detail})"
    )))
}

#[cfg(any(target_os = "linux", all(test, unix)))]
fn handoff_desktop_launcher_to_reaper(mut child: std::process::Child, launcher: String) {
    let launcher_for_error = launcher.clone();
    if let Err(error) = thread::Builder::new()
        .name(String::from("rc-desktop-reaper"))
        .spawn(move || {
            if let Err(error) = child.wait() {
                tracing::debug!(
                    runtime_event = "desktop_launcher_reap_failed",
                    launcher,
                    "failed to reap detached desktop launcher: {error}"
                );
            }
        })
    {
        // Dropping Child does not terminate it. In the exceptional case where the reaper thread
        // cannot be created, preserve the launched application even though rc cannot reap the
        // helper before its own process exits.
        tracing::warn!(
            runtime_event = "desktop_launcher_reaper_unavailable",
            launcher = launcher_for_error,
            "failed to start desktop launcher reaper: {error}"
        );
    }
}

#[cfg(any(target_os = "linux", all(test, unix)))]
fn desktop_launcher_canceled_error() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Interrupted, JOB_CANCELED_MESSAGE)
}

#[cfg(any(target_os = "linux", all(test, unix)))]
fn terminate_desktop_launcher(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        use nix::sys::signal::{Signal, killpg};
        use nix::unistd::Pid;

        if let Ok(process_group) = i32::try_from(child.id()).map(Pid::from_raw) {
            let _ = killpg(process_group, Signal::SIGTERM);
            let deadline = Instant::now() + DESKTOP_LAUNCHER_TERMINATION_GRACE;
            while Instant::now() < deadline {
                match child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) | Err(_) => thread::sleep(DESKTOP_LAUNCHER_POLL_INTERVAL),
                }
            }
            let _ = killpg(process_group, Signal::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn execute_desktop_open_worker(
    job_id: JobId,
    path: std::path::PathBuf,
    cancel_flag: Arc<AtomicBool>,
    release_flag: Arc<AtomicBool>,
    worker_event_tx: &Sender<JobEvent>,
    background_event_tx: &Sender<BackgroundEvent>,
    opener: impl FnOnce(&std::path::Path, &AtomicBool, &AtomicBool) -> std::io::Result<()>,
) {
    let _ = worker_event_tx.send(JobEvent::Started { id: job_id });
    if is_canceled(cancel_flag.as_ref()) || is_canceled(release_flag.as_ref()) {
        let _ = worker_event_tx.send(JobEvent::Finished {
            id: job_id,
            result: Err(JobError::canceled()),
        });
        return;
    }

    let opener_result = opener(&path, cancel_flag.as_ref(), release_flag.as_ref());
    if is_canceled(cancel_flag.as_ref())
        || is_canceled(release_flag.as_ref())
        || opener_result
            .as_ref()
            .is_err_and(|error| error.kind() == std::io::ErrorKind::Interrupted)
    {
        let _ = worker_event_tx.send(JobEvent::Finished {
            id: job_id,
            result: Err(JobError::canceled()),
        });
        return;
    }
    let (open_result, mut job_result) = match opener_result {
        Ok(()) => (Ok(()), Ok(())),
        Err(error) => {
            let error = JobError::from_io(error);
            (Err(error.message.clone()), Err(error))
        }
    };
    if background_event_tx
        .send(BackgroundEvent::DesktopOpenFinished {
            path,
            result: open_result,
        })
        .is_err()
        && job_result.is_ok()
    {
        job_result = Err(JobError::from_message(
            "background event channel disconnected",
        ));
    }
    let _ = worker_event_tx.send(JobEvent::Finished {
        id: job_id,
        result: job_result,
    });
}

fn execute_refresh_worker_job(
    job_id: JobId,
    request: PanelRefreshStreamRequest,
    cancel_flag: Arc<AtomicBool>,
    worker_event_tx: &Sender<JobEvent>,
    background_event_tx: &Sender<BackgroundEvent>,
) {
    let _ = worker_event_tx.send(JobEvent::Started { id: job_id });
    let refresh_result = stream_refresh_panel_entries(&request, cancel_flag.as_ref(), |event| {
        background_event_tx.send(event).is_ok()
    });
    let (event_result, result) = refresh_outcomes(refresh_result, cancel_flag.as_ref());
    let disk_usage = event_result
        .as_ref()
        .ok()
        .and_then(|_| read_disk_usage(request.cwd.as_path()));
    let event = BackgroundEvent::PanelRefreshed {
        panel: request.panel,
        cwd: request.cwd,
        source: request.source,
        sort_mode: request.sort_mode,
        filter: request.filter,
        request_id: request.request_id,
        disk_usage,
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
    refresh_result: std::io::Result<PanelRefreshResult>,
    cancel_flag: &AtomicBool,
) -> (Result<PanelRefreshResult, String>, Result<(), JobError>) {
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
    spec: rc_core::FindSpec,
    max_results: usize,
) {
    let job_id = worker_job.id;
    let cancel_flag = worker_job.cancel_flag();
    let pause_flag = worker_job
        .find_pause_flag()
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
    let _ = worker_event_tx.send(JobEvent::Started { id: job_id });
    let result = run_find_entries(
        &spec,
        max_results,
        cancel_flag.as_ref(),
        pause_flag.as_ref(),
        |entries| {
            background_event_tx
                .send(BackgroundEvent::FindEntriesChunk { job_id, entries })
                .is_ok()
        },
    )
    .map_err(|error| JobError::from_message(error.to_string()))
    .and_then(|report| {
        background_event_tx
            .send(BackgroundEvent::FindCompleted { job_id, report })
            .map_err(|_| JobError::from_message("background event channel disconnected"))
    });
    let _ = worker_event_tx.send(JobEvent::Finished { id: job_id, result });
}

fn execute_quick_cd_search_worker_job(
    job_id: JobId,
    spec: rc_core::QuickCdSearchSpec,
    request_id: u64,
    cancel_flag: Arc<AtomicBool>,
    worker_event_tx: &Sender<JobEvent>,
    background_event_tx: &Sender<BackgroundEvent>,
) {
    let _ = worker_event_tx.send(JobEvent::Started { id: job_id });
    let result = rc_core::run_quick_cd_search(&spec, cancel_flag.as_ref(), |snapshot| {
        background_event_tx
            .send(BackgroundEvent::QuickCdSearchUpdated {
                request_id,
                snapshot,
            })
            .is_ok()
    })
    .map(|_| ())
    .map_err(|error| JobError::from_message(error.to_string()));
    let _ = worker_event_tx.send(JobEvent::Finished { id: job_id, result });
}

fn execute_viewer_worker_job(
    job_id: JobId,
    path: std::path::PathBuf,
    cancel_flag: Arc<AtomicBool>,
    worker_event_tx: &Sender<JobEvent>,
    background_event_tx: &Sender<BackgroundEvent>,
) {
    execute_viewer_load_worker(
        job_id,
        path,
        cancel_flag,
        worker_event_tx,
        background_event_tx,
        |path, result| BackgroundEvent::ViewerLoaded { path, result },
    );
}

#[allow(clippy::too_many_arguments)]
fn execute_quick_view_worker_job(
    job_id: JobId,
    panel: rc_core::ActivePanel,
    path: std::path::PathBuf,
    request_id: u64,
    cancel_flag: Arc<AtomicBool>,
    worker_event_tx: &Sender<JobEvent>,
    background_event_tx: &Sender<BackgroundEvent>,
) {
    execute_viewer_load_worker(
        job_id,
        path,
        cancel_flag,
        worker_event_tx,
        background_event_tx,
        |path, result| BackgroundEvent::QuickViewLoaded {
            panel,
            path,
            request_id,
            result,
        },
    );
}

fn execute_viewer_load_worker(
    job_id: JobId,
    path: std::path::PathBuf,
    cancel_flag: Arc<AtomicBool>,
    worker_event_tx: &Sender<JobEvent>,
    background_event_tx: &Sender<BackgroundEvent>,
    event: impl FnOnce(std::path::PathBuf, Result<rc_core::ViewerState, String>) -> BackgroundEvent,
) {
    let _ = worker_event_tx.send(JobEvent::Started { id: job_id });
    if is_canceled(cancel_flag.as_ref()) {
        let _ = worker_event_tx.send(JobEvent::Finished {
            id: job_id,
            result: Err(JobError::canceled()),
        });
        return;
    }
    let viewer_result = rc_core::ViewerState::open_cancellable(path.clone(), cancel_flag.as_ref());
    if is_canceled(cancel_flag.as_ref()) {
        let _ = worker_event_tx.send(JobEvent::Finished {
            id: job_id,
            result: Err(JobError::canceled()),
        });
        return;
    }
    let (viewer_result, mut result) = match viewer_result {
        Ok(viewer) => (Ok(viewer), Ok(())),
        Err(error) => {
            let error = JobError::from_io(error);
            (Err(error.message.clone()), Err(error))
        }
    };
    if background_event_tx
        .send(event(path, viewer_result))
        .is_err()
        && result.is_ok()
    {
        result = Err(JobError::from_message(
            "background event channel disconnected",
        ));
    }
    let _ = worker_event_tx.send(JobEvent::Finished { id: job_id, result });
}

#[allow(clippy::too_many_arguments)]
fn execute_selection_size_worker_job(
    job_id: JobId,
    panel: rc_core::ActivePanel,
    paths: Vec<std::path::PathBuf>,
    request_id: u64,
    cancel_flag: Arc<AtomicBool>,
    worker_event_tx: &Sender<JobEvent>,
    background_event_tx: &Sender<BackgroundEvent>,
) {
    let _ = worker_event_tx.send(JobEvent::Started { id: job_id });
    let report = match rc_core::measure_selection_size(&paths, cancel_flag.as_ref()) {
        Ok(report) if !is_canceled(cancel_flag.as_ref()) => report,
        Ok(_) => {
            let _ = worker_event_tx.send(JobEvent::Finished {
                id: job_id,
                result: Err(JobError::canceled()),
            });
            return;
        }
        Err(error)
            if is_canceled(cancel_flag.as_ref())
                || error.kind() == std::io::ErrorKind::Interrupted =>
        {
            let _ = worker_event_tx.send(JobEvent::Finished {
                id: job_id,
                result: Err(JobError::canceled()),
            });
            return;
        }
        Err(error) => {
            let _ = worker_event_tx.send(JobEvent::Finished {
                id: job_id,
                result: Err(JobError::from_io(error)),
            });
            return;
        }
    };

    let result = background_event_tx
        .send(BackgroundEvent::SelectionSizeMeasured {
            panel,
            request_id,
            report,
        })
        .map_err(|_| JobError::from_message("background event channel disconnected"));
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
    let event =
        match build_tree_ready_event(job_id, root, max_depth, max_entries, cancel_flag.as_ref()) {
            Ok(event) if !is_canceled(cancel_flag.as_ref()) => event,
            Ok(_) => {
                let _ = worker_event_tx.send(JobEvent::Finished {
                    id: job_id,
                    result: Err(JobError::canceled()),
                });
                return;
            }
            Err(_error) if is_canceled(cancel_flag.as_ref()) => {
                let _ = worker_event_tx.send(JobEvent::Finished {
                    id: job_id,
                    result: Err(JobError::canceled()),
                });
                return;
            }
            Err(error) => {
                let _ = worker_event_tx.send(JobEvent::Finished {
                    id: job_id,
                    result: Err(JobError::from_io(error)),
                });
                return;
            }
        };
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

fn worker_command_priority(command: &WorkerCommand) -> CommandPriority {
    match command {
        WorkerCommand::Cancel(_) | WorkerCommand::Shutdown => CommandPriority::High,
        WorkerCommand::Run(job) => match job.request {
            JobRequest::OpenDesktop { .. }
            | JobRequest::LoadViewer { .. }
            | JobRequest::LoadQuickView { .. } => CommandPriority::High,
            JobRequest::RefreshPanel { .. }
            | JobRequest::QuickCdSearch { .. }
            | JobRequest::MeasureSelection { .. } => CommandPriority::Low,
            _ => CommandPriority::Medium,
        },
    }
}

fn prioritize_worker_commands(commands: Vec<WorkerCommand>) -> Vec<WorkerCommand> {
    let mut high = Vec::new();
    let mut medium = Vec::new();
    let mut low = Vec::new();

    for command in commands {
        match worker_command_priority(&command) {
            CommandPriority::High => high.push(command),
            CommandPriority::Medium => medium.push(command),
            CommandPriority::Low => low.push(command),
        }
    }

    let mut prioritized = Vec::with_capacity(high.len() + medium.len() + low.len());
    prioritized.extend(high);
    prioritized.extend(medium);
    prioritized.extend(low);
    prioritized
}

fn should_drop_for_backpressure(
    command: &WorkerCommand,
    consecutive_full_count: u64,
    oldest_pending_age_ms: u128,
) -> bool {
    if !matches!(worker_command_priority(command), CommandPriority::Low) {
        return false;
    }
    if consecutive_full_count >= 3 {
        return true;
    }
    FOUNDATION_SLO.is_queue_stale(Duration::from_millis(
        oldest_pending_age_ms.min(u64::MAX as u128) as u64,
    ))
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

fn finalize_backpressure_drop(state: &mut AppState, command: WorkerCommand) {
    match command {
        WorkerCommand::Run(job) => {
            state.handle_job_dispatch_failure(
                job.id,
                JobError::dispatch("runtime queue saturated; dropped low-priority background job"),
            );
        }
        WorkerCommand::Cancel(_) | WorkerCommand::Shutdown => {
            state.set_status("runtime queue saturated; dropped runtime command");
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
        ActivePanel, AppState, JobErrorCode, JobId, JobManager, JobRequest, JobRetryHint,
        JobStatus, PanelFilter, PanelListingSource, SortMode, settings_io,
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

    #[test]
    fn desktop_open_shutdown_policy_cancels_only_unstarted_jobs() {
        let request = JobRequest::OpenDesktop {
            path: PathBuf::from("document.png"),
        };

        let policy = worker_runtime_shutdown_policy(&request);

        assert_eq!(policy, RuntimeShutdownPolicy::CancelQueuedAndReleaseActive);
        assert!(policy.cancel_queued());
        assert!(!policy.cancel_active());
        assert!(policy.release_active());
    }

    fn refresh_request(
        panel: ActivePanel,
        cwd: PathBuf,
        source: PanelListingSource,
        request_id: u64,
    ) -> PanelRefreshStreamRequest {
        PanelRefreshStreamRequest {
            panel,
            cwd,
            source,
            sort_mode: SortMode::default(),
            filter: PanelFilter::default(),
            show_hidden_files: true,
            cached_panelized_entries: None,
            request_id,
        }
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
        let mut spec = rc_core::FindSpec::new(root.to_path_buf());
        spec.filename_pattern = String::from("*entry*");
        let mut job = manager.enqueue(JobRequest::Find {
            spec,
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

    #[test]
    fn runtime_bridge_streams_find_entries_and_terminal_report() {
        let root = make_temp_dir("find-terminal-report");
        fs::write(root.join("alpha.txt"), "alpha").expect("find fixture should be writable");
        fs::write(root.join("ignored.log"), "ignored")
            .expect("nonmatching fixture should be writable");
        let (command_tx, worker_event_rx, background_event_rx, runtime_handle) =
            spawn_runtime_loop_thread();
        let mut manager = JobManager::new();
        let mut spec = rc_core::FindSpec::new(root.clone());
        spec.filename_pattern = String::from("*.txt");
        let job = manager.enqueue(JobRequest::Find {
            spec,
            max_results: 16,
        });
        let job_id = job.id;
        send_run(&command_tx, job);

        assert!(matches!(
            recv_event(&worker_event_rx, Duration::from_secs(2)),
            JobEvent::Started { id } if id == job_id
        ));
        let finished = recv_event(&worker_event_rx, Duration::from_secs(2));
        assert!(matches!(
            finished,
            JobEvent::Finished { id, result: Ok(()) } if id == job_id
        ));

        let mut paths = Vec::new();
        let mut terminal_report = None;
        while terminal_report.is_none() {
            match background_event_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(BackgroundEvent::FindEntriesChunk {
                    job_id: id,
                    entries,
                }) => {
                    assert_eq!(id, job_id);
                    paths.extend(entries.into_iter().map(|entry| entry.path));
                }
                Ok(BackgroundEvent::FindCompleted { job_id: id, report }) => {
                    assert_eq!(id, job_id);
                    terminal_report = Some(report);
                }
                Ok(other) => panic!("unexpected find background event: {other:?}"),
                Err(error) => panic!("find background event should arrive: {error}"),
            }
        }
        assert_eq!(paths, [root.join("alpha.txt")]);
        let report = terminal_report.expect("terminal report should be emitted");
        assert_eq!(report.matched_entries, 1);
        assert!(!report.truncated);
        assert!(!report.is_partial());

        command_tx
            .blocking_send(RuntimeCommand::Shutdown)
            .expect("runtime shutdown should send");
        runtime_handle
            .join()
            .expect("runtime loop thread should terminate cleanly");
        fs::remove_dir_all(&root).expect("temp root should be removable");
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
    fn queue_full_tracks_pending_age_and_flags_stale_queue() {
        let root = make_temp_dir("queue-full-metrics");
        let (mut runtime, _command_rx) = test_runtime_bridge_with_capacity(1);
        let mut state = AppState::new(root.clone()).expect("app should initialize");
        let _first_id = state.enqueue_worker_job_request(JobRequest::Mkdir {
            path: root.join("first"),
        });
        let second_id = state.enqueue_worker_job_request(JobRequest::Mkdir {
            path: root.join("second"),
        });

        let stale_age = FOUNDATION_SLO.queue_stale_warn_after + Duration::from_secs(1);
        let stale_seen = Instant::now()
            .checked_sub(stale_age)
            .unwrap_or_else(Instant::now);
        runtime
            .pending_first_seen
            .insert(PendingCommandKey::Run(second_id), stale_seen);

        runtime.dispatch_pending_commands(&mut state);

        assert!(
            runtime.consecutive_full_count >= 1,
            "queue-full dispatch should increment consecutive-full metric"
        );
        let oldest_pending_age_ms = runtime
            .oldest_pending_age_ms(Instant::now())
            .expect("overflowed command should remain pending");
        assert!(
            oldest_pending_age_ms >= FOUNDATION_SLO.queue_stale_warn_after.as_millis(),
            "oldest pending age should reflect stale queue pressure"
        );
        assert!(
            runtime.stale_pending_warned,
            "stale queue pressure should trigger warning guardrail"
        );

        let pending = state.take_pending_worker_commands();
        assert_eq!(
            pending.len(),
            1,
            "overflowed command should remain pending for retry"
        );

        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[test]
    fn drain_events_limits_burst_to_per_tick_budget() {
        let root = make_temp_dir("drain-event-budget");
        let (mut runtime, _command_rx, worker_event_tx, _background_event_tx) =
            test_runtime_bridge_with_channels(4);
        let mut state = AppState::new(root.clone()).expect("app should initialize");
        let total_events = RUNTIME_EVENT_DRAIN_LIMIT_PER_TICK + 32;
        for id in 1..=total_events {
            worker_event_tx
                .send(JobEvent::Started {
                    id: JobId(id as u64),
                })
                .expect("worker event injection should succeed");
        }

        runtime.drain_events(&mut state);
        assert!(
            state.status_line.contains(&format!(
                "Job #{} started",
                RUNTIME_EVENT_DRAIN_LIMIT_PER_TICK
            )),
            "first drain should stop at configured per-tick event budget"
        );
        assert!(
            !state
                .status_line
                .contains(&format!("Job #{} started", total_events)),
            "first drain should not consume the full burst"
        );

        runtime.drain_events(&mut state);
        assert!(
            state
                .status_line
                .contains(&format!("Job #{} started", total_events)),
            "second drain should consume the remaining events"
        );

        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[test]
    fn drain_events_reports_worker_disconnect_once() {
        let root = make_temp_dir("drain-worker-disconnect");
        let (mut runtime, _command_rx, worker_event_tx, _background_event_tx) =
            test_runtime_bridge_with_channels(4);
        let mut state = AppState::new(root.clone()).expect("app should initialize");

        drop(worker_event_tx);
        runtime.drain_events(&mut state);
        assert_eq!(
            state.status_line, "Worker channel disconnected",
            "first disconnect should be surfaced to status line"
        );

        state.set_status("status should remain unchanged");
        runtime.drain_events(&mut state);
        assert_eq!(
            state.status_line, "status should remain unchanged",
            "disconnect warning should not be repeated every tick"
        );

        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[test]
    fn drain_events_reports_background_disconnect_once() {
        let root = make_temp_dir("drain-background-disconnect");
        let (mut runtime, _command_rx, _worker_event_tx, background_event_tx) =
            test_runtime_bridge_with_channels(4);
        let mut state = AppState::new(root.clone()).expect("app should initialize");

        drop(background_event_tx);
        runtime.drain_events(&mut state);
        assert_eq!(
            state.status_line, "Background worker channel disconnected",
            "first disconnect should be surfaced to status line"
        );

        state.set_status("status should remain unchanged");
        runtime.drain_events(&mut state);
        assert_eq!(
            state.status_line, "status should remain unchanged",
            "disconnect warning should not be repeated every tick"
        );

        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[test]
    fn priority_sorting_dispatches_high_before_medium_before_low() {
        let mut manager = JobManager::new();
        let medium_job = manager.enqueue(JobRequest::Mkdir {
            path: PathBuf::from("/tmp/medium"),
        });
        let low_job = manager.enqueue(JobRequest::RefreshPanel {
            panel: ActivePanel::Left,
            cwd: PathBuf::from("/tmp"),
            source: PanelListingSource::Directory,
            sort_mode: SortMode::default(),
            filter: PanelFilter::default(),
            show_hidden_files: true,
            cached_panelized_entries: None,
            request_id: 99,
        });
        let high_job = manager.enqueue(JobRequest::LoadViewer {
            path: PathBuf::from("/tmp/high.txt"),
        });
        let high_cancel = WorkerCommand::Cancel(medium_job.id);

        let prioritized = prioritize_worker_commands(vec![
            WorkerCommand::Run(Box::new(low_job)),
            WorkerCommand::Run(Box::new(medium_job)),
            high_cancel,
            WorkerCommand::Run(Box::new(high_job)),
        ]);

        assert!(
            matches!(prioritized[0], WorkerCommand::Cancel(_)),
            "cancel commands should have highest dispatch priority"
        );
        assert!(
            matches!(
                prioritized[1],
                WorkerCommand::Run(ref job) if matches!(job.request, JobRequest::LoadViewer { .. })
            ),
            "interactive viewer loads should dispatch before medium jobs"
        );
        assert!(
            matches!(
                prioritized[2],
                WorkerCommand::Run(ref job) if matches!(job.request, JobRequest::Mkdir { .. })
            ),
            "medium jobs should dispatch before refresh jobs"
        );
        assert!(
            matches!(
                prioritized[3],
                WorkerCommand::Run(ref job) if matches!(job.request, JobRequest::RefreshPanel { .. })
            ),
            "refresh jobs should be treated as low-priority traffic"
        );
    }

    #[test]
    fn saturated_queue_drops_low_priority_refresh_after_repeated_pressure() {
        let root = make_temp_dir("queue-drop-low-priority");
        let (mut runtime, mut command_rx) = test_runtime_bridge_with_capacity(1);
        let mut state = AppState::new(root.clone()).expect("app should initialize");
        let medium_id = state.enqueue_worker_job_request(JobRequest::Mkdir {
            path: root.join("first"),
        });
        let low_id = state.enqueue_worker_job_request(JobRequest::RefreshPanel {
            panel: ActivePanel::Left,
            cwd: root.clone(),
            source: PanelListingSource::Directory,
            sort_mode: SortMode::default(),
            filter: PanelFilter::default(),
            show_hidden_files: true,
            cached_panelized_entries: None,
            request_id: 7,
        });
        let stale_age = FOUNDATION_SLO.queue_stale_warn_after + Duration::from_secs(1);
        let stale_seen = Instant::now()
            .checked_sub(stale_age)
            .unwrap_or_else(Instant::now);
        runtime
            .pending_first_seen
            .insert(PendingCommandKey::Run(low_id), stale_seen);

        runtime.dispatch_pending_commands(&mut state);

        match command_rx.try_recv() {
            Ok(RuntimeCommand::Worker {
                command: WorkerCommand::Run(job),
                ..
            }) => {
                assert_eq!(job.id, medium_id, "medium command should stay queued");
            }
            other => panic!("expected medium run command in runtime queue, got {other:?}"),
        }
        assert!(
            state.take_pending_worker_commands().is_empty(),
            "low-priority refresh should be dropped instead of requeued"
        );
        assert!(
            state
                .status_line
                .contains("dropped low-priority background job"),
            "status should explain backpressure drop behavior"
        );
        assert!(
            !runtime
                .pending_first_seen
                .contains_key(&PendingCommandKey::Run(low_id)),
            "dropped command should not remain tracked as pending"
        );
        assert!(
            state
                .jobs
                .job(low_id)
                .is_some_and(|job| job.status == JobStatus::Failed),
            "dropped refresh should be finalized as a failed dispatch"
        );

        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[test]
    fn dropped_refresh_clears_panel_loading_state_under_backpressure() {
        let root = make_temp_dir("queue-drop-clears-refresh-state");
        let (mut runtime, mut command_rx) = test_runtime_bridge_with_capacity(1);
        let mut state = AppState::new(root.clone()).expect("app should initialize");
        state.refresh_active_panel();
        let refresh_job_id = {
            let pending = state.take_pending_worker_commands();
            let refresh_job_id = pending
                .iter()
                .find_map(|command| {
                    let WorkerCommand::Run(job) = command else {
                        return None;
                    };
                    matches!(job.request, JobRequest::RefreshPanel { .. }).then_some(job.id)
                })
                .expect("refresh command should be queued");
            state.restore_pending_worker_commands(pending);
            refresh_job_id
        };
        let medium_id = state.enqueue_worker_job_request(JobRequest::Mkdir {
            path: root.join("medium"),
        });
        assert!(
            state.active_panel().loading,
            "refresh should set panel loading before dispatch"
        );

        let stale_age = FOUNDATION_SLO.queue_stale_warn_after + Duration::from_secs(1);
        let stale_seen = Instant::now()
            .checked_sub(stale_age)
            .unwrap_or_else(Instant::now);
        runtime
            .pending_first_seen
            .insert(PendingCommandKey::Run(refresh_job_id), stale_seen);

        runtime.dispatch_pending_commands(&mut state);

        match command_rx.try_recv() {
            Ok(RuntimeCommand::Worker {
                command: WorkerCommand::Run(job),
                ..
            }) => assert_eq!(job.id, medium_id, "medium job should dispatch first"),
            other => panic!("expected medium job in runtime queue, got {other:?}"),
        }
        assert!(
            state.take_pending_worker_commands().is_empty(),
            "dropped refresh should not be requeued"
        );
        assert!(
            !state.active_panel().loading,
            "dropped refresh should clear panel loading state"
        );
        assert!(
            state
                .jobs
                .job(refresh_job_id)
                .is_some_and(|job| job.status == JobStatus::Failed),
            "dropped refresh should not remain queued"
        );

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
    fn shutdown_finishes_persist_settings_jobs_in_submission_order() {
        let root = make_temp_dir("shutdown-persist-settings");
        let settings_paths = settings_io::SettingsPaths {
            mc_ini_path: None,
            rc_ini_path: Some(root.join("settings.ini")),
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
                JobEvent::Started { id } => {
                    if id == second_job_id {
                        assert!(
                            finished.contains_key(&first_job_id),
                            "serialized settings jobs must start in submission order"
                        );
                    }
                }
                JobEvent::Progress { .. } => {}
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
        let saved_settings =
            settings_io::load_settings(&settings_paths).expect("persisted settings should load");
        assert_eq!(
            saved_settings.appearance.skin, second_snapshot.appearance.skin,
            "the latest submitted settings snapshot must win"
        );
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
    fn cancel_queued_job_waiting_for_permit_finishes_promptly() {
        let root = make_temp_dir("cancel-queued-permit-wait");
        fs::write(root.join("entry.txt"), "entry").expect("fixture file should be writable");
        let pause_flag = Arc::new(AtomicBool::new(true));

        let (command_tx, worker_event_rx, _background_event_rx, runtime_handle) =
            spawn_runtime_loop_thread();
        let mut manager = JobManager::new();
        let mut jobs = Vec::new();
        for _ in 0..5 {
            jobs.push(enqueue_paused_find_job(
                &mut manager,
                &root,
                Arc::clone(&pause_flag),
            ));
        }
        let all_job_ids: Vec<JobId> = jobs.iter().map(|job| job.id).collect();
        for job in jobs {
            send_run(&command_tx, job);
        }

        let mut started = HashMap::<JobId, ()>::new();
        while started.len() < 4 {
            let event = recv_event(&worker_event_rx, Duration::from_secs(2));
            if let JobEvent::Started { id } = event {
                started.insert(id, ());
            }
        }
        let queued_job_id = all_job_ids
            .into_iter()
            .find(|job_id| !started.contains_key(job_id))
            .expect("one job should still be queued while semaphore permits are saturated");

        send_cancel(&command_tx, queued_job_id);

        let canceled_error = loop {
            match worker_event_rx.recv_timeout(Duration::from_secs(1)) {
                Ok(JobEvent::Finished { id, result }) if id == queued_job_id => {
                    break result.expect_err("canceled queued job should finish with an error");
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {
                    panic!("canceled queued job should finish promptly while waiting for permit");
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("worker event channel should remain connected until runtime stops");
                }
            }
        };
        assert_eq!(
            canceled_error.code,
            JobErrorCode::Canceled,
            "queued job canceled during permit wait should use canceled error code"
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
    fn desktop_open_worker_reports_launcher_result_without_blocking_the_ui_adapter() {
        let root = make_temp_dir("desktop-open-worker");
        let document = root.join("image.png");
        fs::write(&document, "payload").expect("document should be writable");
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let release_flag = Arc::new(AtomicBool::new(false));
        let (worker_event_tx, worker_event_rx) = mpsc::channel();
        let (background_event_tx, background_event_rx) = mpsc::channel();

        execute_desktop_open_worker(
            JobId(1),
            document.clone(),
            cancel_flag,
            release_flag,
            &worker_event_tx,
            &background_event_tx,
            |path, cancel_flag, release_flag| {
                assert_eq!(path, document.as_path());
                assert!(!cancel_flag.load(AtomicOrdering::Relaxed));
                assert!(!release_flag.load(AtomicOrdering::Relaxed));
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no desktop handler",
                ))
            },
        );

        assert!(matches!(
            recv_event(&worker_event_rx, Duration::from_secs(1)),
            JobEvent::Started { id: JobId(1) }
        ));
        assert!(matches!(
            recv_event(&worker_event_rx, Duration::from_secs(1)),
            JobEvent::Finished {
                id: JobId(1),
                result: Err(error)
            } if error.code == JobErrorCode::NotFound
        ));
        match background_event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("desktop result should be emitted")
        {
            BackgroundEvent::DesktopOpenFinished { path, result } => {
                assert_eq!(path, document);
                assert!(
                    result
                        .expect_err("injected opener should fail")
                        .contains("no desktop handler")
                );
            }
            other => panic!("expected desktop-open result, got {other:?}"),
        }

        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[cfg(unix)]
    #[test]
    fn desktop_launcher_chain_skips_missing_and_failed_openers() {
        let root = make_temp_dir("desktop-launcher-chain");
        let commands = vec![
            std::process::Command::new(root.join("missing-launcher")),
            std::process::Command::new("false"),
            std::process::Command::new("true"),
        ];
        let cancel_flag = AtomicBool::new(false);
        let release_flag = AtomicBool::new(false);

        run_status_aware_launcher_commands(
            commands,
            &cancel_flag,
            &release_flag,
            Duration::from_secs(1),
        )
        .expect("a later working opener should be attempted");

        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[cfg(unix)]
    #[test]
    fn desktop_launcher_chain_reports_nonzero_statuses() {
        let commands = vec![std::process::Command::new("false")];
        let cancel_flag = AtomicBool::new(false);
        let release_flag = AtomicBool::new(false);

        let error = run_status_aware_launcher_commands(
            commands,
            &cancel_flag,
            &release_flag,
            Duration::from_secs(1),
        )
        .expect_err("a nonzero opener must not be reported as success");

        assert!(error.to_string().contains("exited with"));
    }

    #[cfg(unix)]
    #[test]
    fn desktop_launcher_chain_honors_in_flight_cancellation() {
        let commands = vec![{
            let mut command = std::process::Command::new("sleep");
            command.arg("30");
            command
        }];
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let release_flag = AtomicBool::new(false);
        let cancel_for_thread = Arc::clone(&cancel_flag);
        let cancel_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(40));
            cancel_for_thread.store(true, AtomicOrdering::Relaxed);
        });

        let started = Instant::now();
        let error = run_status_aware_launcher_commands(
            commands,
            cancel_flag.as_ref(),
            &release_flag,
            Duration::from_secs(5),
        )
        .expect_err("a canceled opener must stop promptly");
        cancel_thread.join().expect("cancel thread should finish");

        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(error.to_string(), JOB_CANCELED_MESSAGE);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn desktop_launcher_chain_releases_in_flight_opener_on_runtime_shutdown() {
        let root = make_temp_dir("desktop-launcher-shutdown-release");
        let continue_marker = root.join("continue-application");
        let survived_marker = root.join("application-survived");
        let commands = vec![{
            let mut command = std::process::Command::new("sh");
            command
                .args([
                    "-c",
                    "attempt=0; while [ ! -e \"$1\" ] && [ \"$attempt\" -lt 100 ]; do \
                     sleep 0.02; attempt=$((attempt + 1)); done; printf survived > \"$2\"",
                    "sh",
                ])
                .arg(&continue_marker)
                .arg(&survived_marker);
            command
        }];
        let cancel_flag = AtomicBool::new(false);
        let release_flag = Arc::new(AtomicBool::new(false));
        let release_for_thread = Arc::clone(&release_flag);
        let release_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(40));
            release_for_thread.store(true, AtomicOrdering::Relaxed);
        });

        let started = Instant::now();
        let error = run_status_aware_launcher_commands(
            commands,
            &cancel_flag,
            release_flag.as_ref(),
            Duration::from_secs(5),
        )
        .expect_err("runtime shutdown should release an in-flight desktop opener");
        release_thread.join().expect("release thread should finish");
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert!(started.elapsed() < Duration::from_secs(2));

        fs::write(&continue_marker, "continue")
            .expect("application continuation marker should be writable");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !survived_marker.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            fs::read_to_string(&survived_marker)
                .expect("the released application should keep running"),
            "survived"
        );

        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[cfg(unix)]
    #[test]
    fn desktop_launcher_chain_releases_attached_opener_to_independent_reaper() {
        let root = make_temp_dir("desktop-launcher-reaper");
        let release = root.join("release-application");
        let marker = root.join("application-survived");
        let commands = vec![{
            let mut command = std::process::Command::new("sh");
            command
                .args([
                    "-c",
                    "attempt=0; while [ ! -e \"$1\" ] && [ \"$attempt\" -lt 100 ]; do \
                     sleep 0.02; attempt=$((attempt + 1)); done; printf survived > \"$2\"",
                    "sh",
                ])
                .arg(&release)
                .arg(&marker);
            command
        }];
        let cancel_flag = AtomicBool::new(false);
        let release_flag = AtomicBool::new(false);

        run_status_aware_launcher_commands(
            commands,
            &cancel_flag,
            &release_flag,
            Duration::from_millis(20),
        )
        .expect("an attached opener should be accepted after bounded observation");
        assert!(
            !marker.exists(),
            "the scheduler worker must return before the opened application exits"
        );

        // Runtime shutdown and later cancellation no longer own an opener after handoff.
        cancel_flag.store(true, AtomicOrdering::Relaxed);
        fs::write(&release, "continue").expect("application release marker should be writable");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !marker.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            fs::read_to_string(&marker).expect("the accepted application should keep running"),
            "survived"
        );

        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[test]
    fn interrupted_desktop_open_does_not_enqueue_viewer_fallback() {
        let root = make_temp_dir("desktop-open-canceled");
        let document = root.join("image.png");
        fs::write(&document, "payload").expect("document should be writable");
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let release_flag = Arc::new(AtomicBool::new(false));
        let (worker_event_tx, worker_event_rx) = mpsc::channel();
        let (background_event_tx, background_event_rx) = mpsc::channel();

        execute_desktop_open_worker(
            JobId(1),
            document,
            cancel_flag,
            release_flag,
            &worker_event_tx,
            &background_event_tx,
            |_path, _cancel_flag, _release_flag| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "desktop open canceled",
                ))
            },
        );

        assert!(matches!(
            recv_event(&worker_event_rx, Duration::from_secs(1)),
            JobEvent::Started { id: JobId(1) }
        ));
        assert!(matches!(
            recv_event(&worker_event_rx, Duration::from_secs(1)),
            JobEvent::Finished {
                id: JobId(1),
                result: Err(error)
            } if error.code == JobErrorCode::Canceled
        ));
        assert!(
            matches!(
                background_event_rx.try_recv(),
                Err(TryRecvError::Empty | TryRecvError::Disconnected)
            ),
            "canceling a desktop opener must not trigger viewer fallback"
        );

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
    fn quick_view_worker_preserves_panel_and_request_identity() {
        let root = make_temp_dir("quick-view-worker");
        let viewer_file = root.join("preview.txt");
        fs::write(&viewer_file, "preview payload").expect("preview file should be writable");
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let (worker_event_tx, worker_event_rx) = mpsc::channel();
        let (background_event_tx, background_event_rx) = mpsc::channel();

        execute_quick_view_worker_job(
            JobId(1),
            ActivePanel::Right,
            viewer_file.clone(),
            42,
            cancel_flag,
            &worker_event_tx,
            &background_event_tx,
        );

        assert!(matches!(
            recv_event(&worker_event_rx, Duration::from_secs(1)),
            JobEvent::Started { id: JobId(1) }
        ));
        match background_event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("quick-view background event should arrive")
        {
            BackgroundEvent::QuickViewLoaded {
                panel: ActivePanel::Right,
                path,
                request_id: 42,
                result: Ok(viewer),
            } => {
                assert_eq!(path, viewer_file);
                assert_eq!(viewer.content(), "preview payload");
            }
            other => panic!("expected quick-view completion, got {other:?}"),
        }
        assert!(matches!(
            recv_event(&worker_event_rx, Duration::from_secs(1)),
            JobEvent::Finished {
                id: JobId(1),
                result: Ok(())
            }
        ));

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn selection_size_worker_reports_recursive_bytes_with_request_identity() {
        let root = make_temp_dir("selection-size-worker");
        let selected = root.join("selected");
        fs::create_dir_all(selected.join("nested")).expect("nested directory should be creatable");
        fs::write(selected.join("first"), vec![0_u8; 13]).expect("first file should be writable");
        fs::write(selected.join("nested/second"), vec![0_u8; 31])
            .expect("second file should be writable");
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let (worker_event_tx, worker_event_rx) = mpsc::channel();
        let (background_event_tx, background_event_rx) = mpsc::channel();

        execute_selection_size_worker_job(
            JobId(1),
            ActivePanel::Right,
            vec![selected],
            73,
            cancel_flag,
            &worker_event_tx,
            &background_event_tx,
        );

        assert!(matches!(
            recv_event(&worker_event_rx, Duration::from_secs(1)),
            JobEvent::Started { id: JobId(1) }
        ));
        assert!(matches!(
            background_event_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("selection-size background event should arrive"),
            BackgroundEvent::SelectionSizeMeasured {
                panel: ActivePanel::Right,
                request_id: 73,
                report: rc_core::SelectionSizeReport {
                    apparent_bytes: 44,
                    unreadable_entries: 0,
                },
            }
        ));
        assert!(matches!(
            recv_event(&worker_event_rx, Duration::from_secs(1)),
            JobEvent::Finished {
                id: JobId(1),
                result: Ok(())
            }
        ));

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn quick_cd_worker_streams_ranked_snapshots_with_request_identity() {
        let root = make_temp_dir("quick-cd-worker");
        let target = root.join("Project-Needle");
        fs::create_dir_all(&target).expect("matching directory should be creatable");
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let (worker_event_tx, worker_event_rx) = mpsc::channel();
        let (background_event_tx, background_event_rx) = mpsc::channel();

        execute_quick_cd_search_worker_job(
            JobId(1),
            rc_core::QuickCdSearchSpec {
                query: String::from("needle"),
                cwd: root.clone(),
                home: Some(root.clone()),
                root: root.clone(),
                previous_directory: None,
                max_results: 8,
                max_directories: 32,
            },
            91,
            cancel_flag,
            &worker_event_tx,
            &background_event_tx,
        );

        assert!(matches!(
            recv_event(&worker_event_rx, Duration::from_secs(1)),
            JobEvent::Started { id: JobId(1) }
        ));
        let mut final_snapshot = None;
        while let Ok(event) = background_event_rx.try_recv() {
            match event {
                BackgroundEvent::QuickCdSearchUpdated {
                    request_id: 91,
                    snapshot,
                } if snapshot.complete => final_snapshot = Some(snapshot),
                BackgroundEvent::QuickCdSearchUpdated { request_id: 91, .. } => {}
                other => panic!("unexpected quick-cd background event: {other:?}"),
            }
        }
        let final_snapshot = final_snapshot.expect("final quick-cd snapshot should be emitted");
        assert!(
            final_snapshot
                .suggestions
                .iter()
                .any(|suggestion| suggestion.path == target)
        );
        assert!(matches!(
            recv_event(&worker_event_rx, Duration::from_secs(1)),
            JobEvent::Finished {
                id: JobId(1),
                result: Ok(())
            }
        ));

        fs::remove_dir_all(root).expect("temp root should be removable");
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
            refresh_request(
                ActivePanel::Left,
                root.clone(),
                PanelListingSource::Directory,
                1,
            ),
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
    fn refresh_canceled_before_start_emits_terminal_background_event() {
        let root = make_temp_dir("refresh-canceled-before-start");
        let mut manager = JobManager::new();
        let job = manager.enqueue(JobRequest::RefreshPanel {
            panel: ActivePanel::Right,
            cwd: root.clone(),
            source: PanelListingSource::Directory,
            sort_mode: SortMode::default(),
            filter: PanelFilter::default(),
            show_hidden_files: true,
            cached_panelized_entries: None,
            request_id: 17,
        });
        let job_id = job.id;
        let (worker_event_tx, worker_event_rx) = mpsc::channel();
        let (background_event_tx, background_event_rx) = mpsc::channel();

        finish_canceled_worker_before_start(&job, &worker_event_tx, &background_event_tx);

        assert!(matches!(
            background_event_rx.recv_timeout(Duration::from_secs(1)),
            Ok(BackgroundEvent::PanelRefreshed {
                panel: ActivePanel::Right,
                request_id: 17,
                result: Err(error),
                ..
            }) if error == "panel refresh canceled"
        ));
        assert!(matches!(
            worker_event_rx.recv_timeout(Duration::from_secs(1)),
            Ok(JobEvent::Finished {
                id,
                result: Err(error),
            }) if id == job_id && error.code == JobErrorCode::Canceled
        ));

        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[test]
    fn refresh_worker_streams_directory_entries_before_final_event() {
        let root = make_temp_dir("refresh-streaming");
        fs::write(root.join("alpha.txt"), "a").expect("first fixture should be writable");
        fs::write(root.join("bravo.txt"), "b").expect("second fixture should be writable");
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let (worker_event_tx, _worker_event_rx) = mpsc::channel();
        let (background_event_tx, background_event_rx) = mpsc::channel();

        execute_refresh_worker_job(
            JobId(7),
            refresh_request(
                ActivePanel::Left,
                root.clone(),
                PanelListingSource::Directory,
                11,
            ),
            cancel_flag,
            &worker_event_tx,
            &background_event_tx,
        );

        let mut saw_chunk = false;
        let mut saw_final = false;
        for event in background_event_rx.try_iter() {
            match event {
                BackgroundEvent::PanelEntriesChunk {
                    request_id,
                    entries,
                    ..
                } => {
                    assert_eq!(request_id, 11);
                    assert!(
                        !entries.is_empty(),
                        "chunk event should carry at least one discovered entry"
                    );
                    saw_chunk = true;
                }
                BackgroundEvent::PanelRefreshed { request_id, .. } => {
                    assert_eq!(request_id, 11);
                    saw_final = true;
                }
                _ => {}
            }
        }

        assert!(
            saw_chunk,
            "streaming refresh should emit at least one chunk"
        );
        assert!(
            saw_final,
            "streaming refresh should emit final completion event"
        );
        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[cfg(unix)]
    #[test]
    fn refresh_worker_streams_panelize_entries_in_adaptive_chunks() {
        let root = make_temp_dir("refresh-panelize-streaming");
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let (worker_event_tx, worker_event_rx) = mpsc::channel();
        let (background_event_tx, background_event_rx) = mpsc::channel();

        execute_refresh_worker_job(
            JobId(8),
            refresh_request(
                ActivePanel::Left,
                root.clone(),
                PanelListingSource::Panelize {
                    command: String::from(
                        "printf 'delta.txt\\nalpha.txt\\ncharlie.txt\\nbravo.txt\\n'",
                    ),
                },
                12,
            ),
            cancel_flag,
            &worker_event_tx,
            &background_event_tx,
        );

        let events: Vec<BackgroundEvent> = background_event_rx.try_iter().collect();
        let chunk_sizes: Vec<usize> = events
            .iter()
            .filter_map(|event| match event {
                BackgroundEvent::PanelEntriesChunk { entries, .. } => Some(entries.len()),
                _ => None,
            })
            .collect();
        assert_eq!(
            chunk_sizes,
            vec![1, 2, 1],
            "small early chunks should minimize latency before growing toward the cap"
        );
        let final_entries = events.iter().find_map(|event| match event {
            BackgroundEvent::PanelRefreshed {
                request_id,
                result: Ok(entries),
                ..
            } => {
                assert_eq!(*request_id, 12);
                Some(entries)
            }
            _ => None,
        });
        let final_names: Vec<&str> = final_entries
            .expect("panelize refresh should emit a successful final event")
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(
            final_names,
            vec!["alpha.txt", "bravo.txt", "charlie.txt", "delta.txt"],
            "the authoritative final result should still be sorted"
        );
        assert!(
            matches!(
                worker_event_rx.try_iter().last(),
                Some(JobEvent::Finished {
                    id: JobId(8),
                    result: Ok(())
                })
            ),
            "streamed panelize refresh should finish successfully"
        );

        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[cfg(unix)]
    #[test]
    fn failed_panelize_worker_emits_partial_chunk_before_terminal_error() {
        let root = make_temp_dir("refresh-panelize-partial-failure");
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let (worker_event_tx, worker_event_rx) = mpsc::channel();
        let (background_event_tx, background_event_rx) = mpsc::channel();

        execute_refresh_worker_job(
            JobId(9),
            refresh_request(
                ActivePanel::Right,
                root.clone(),
                PanelListingSource::Panelize {
                    command: String::from("printf 'partial.txt\\n'; printf 'boom\\n' >&2; exit 7"),
                },
                13,
            ),
            cancel_flag,
            &worker_event_tx,
            &background_event_tx,
        );

        let events: Vec<BackgroundEvent> = background_event_rx.try_iter().collect();
        assert!(matches!(
            events.first(),
            Some(BackgroundEvent::PanelEntriesChunk { entries, .. })
                if entries.iter().any(|entry| entry.name == "partial.txt")
        ));
        assert!(matches!(
            events.last(),
            Some(BackgroundEvent::PanelRefreshed {
                request_id: 13,
                result: Err(error),
                ..
            }) if error.contains("boom")
        ));
        assert!(matches!(
            worker_event_rx.try_iter().last(),
            Some(JobEvent::Finished {
                id: JobId(9),
                result: Err(_)
            })
        ));

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
