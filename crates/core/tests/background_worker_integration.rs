#![forbid(unsafe_code)]

use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use rc_core::{BackgroundCommand, run_background_worker};

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
fn shutdown_stops_background_worker_loop() {
    let root = make_temp_dir("shutdown");
    fs::write(root.join("needle.txt"), "needle").expect("fixture file should be writable");

    let (command_tx, command_rx) = mpsc::channel();
    let (event_tx, _event_rx) = mpsc::channel();
    let worker = thread::spawn(move || run_background_worker(command_rx, event_tx));

    command_tx
        .send(BackgroundCommand::BuildTree {
            root: root.clone(),
            max_depth: 2,
            max_entries: 64,
        })
        .expect("tree command should send");

    command_tx
        .send(BackgroundCommand::Shutdown)
        .expect("shutdown command should send");
    worker
        .join()
        .expect("background worker should join cleanly");

    fs::remove_dir_all(&root).expect("temp tree should be removable");
}
