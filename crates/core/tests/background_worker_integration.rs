#![forbid(unsafe_code)]

use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rc_core::{BackgroundCommand, BackgroundEvent, JobId, run_background_worker};

fn make_temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-bg-worker-{label}-{stamp}"));
    fs::create_dir_all(&root).expect("temp root should be creatable");
    root
}

#[test]
fn shutdown_cancels_and_joins_spawned_find_tasks() {
    let root = make_temp_dir("shutdown");
    fs::write(root.join("needle.txt"), "needle").expect("fixture file should be writable");

    let (command_tx, command_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let worker = thread::spawn(move || run_background_worker(command_rx, event_tx));

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let pause_flag = Arc::new(AtomicBool::new(true));
    command_tx
        .send(BackgroundCommand::FindEntries {
            job_id: JobId(1),
            query: String::from("needle"),
            base_dir: root.clone(),
            max_results: 32,
            cancel_flag: Arc::clone(&cancel_flag),
            pause_flag: Arc::clone(&pause_flag),
        })
        .expect("find command should send");

    let started = event_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("find task should emit started event");
    assert!(
        matches!(started, BackgroundEvent::FindEntriesStarted { job_id } if job_id == JobId(1)),
        "find task should announce startup before shutdown",
    );

    command_tx
        .send(BackgroundCommand::Shutdown)
        .expect("shutdown command should send");
    worker
        .join()
        .expect("background worker should join cleanly");

    assert!(
        cancel_flag.load(Ordering::Relaxed),
        "shutdown should request cancellation for running find tasks",
    );
    assert_eq!(
        Arc::strong_count(&cancel_flag),
        1,
        "find task handles should be joined on shutdown",
    );
    assert_eq!(
        Arc::strong_count(&pause_flag),
        1,
        "pause flag clone should be released after join",
    );

    fs::remove_dir_all(&root).expect("temp tree should be removable");
}
