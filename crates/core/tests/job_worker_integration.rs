#![forbid(unsafe_code)]

use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rc_core::{
    JOB_CANCELED_MESSAGE, JobEvent, JobManager, JobRequest, JobStatus, OverwritePolicy,
    WorkerCommand, run_worker,
};

fn make_temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-job-it-{label}-{stamp}"));
    fs::create_dir_all(&root).expect("temp root should be creatable");
    root
}

fn spawn_worker() -> (
    Sender<WorkerCommand>,
    Receiver<JobEvent>,
    thread::JoinHandle<()>,
) {
    let (command_tx, command_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let handle = thread::spawn(move || run_worker(command_rx, event_tx));
    (command_tx, event_rx, handle)
}

fn run_job(event_rx: &Receiver<JobEvent>, manager: &mut JobManager) -> JobEvent {
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

fn shutdown_worker(command_tx: Sender<WorkerCommand>, handle: thread::JoinHandle<()>) {
    command_tx
        .send(WorkerCommand::Shutdown)
        .expect("shutdown should send");
    handle
        .join()
        .expect("worker thread should terminate cleanly");
}

#[test]
fn recursive_copy_move_delete_round_trip() {
    let root = make_temp_dir("round-trip");
    let source_root = root.join("source");
    fs::create_dir_all(source_root.join("nested")).expect("source tree should exist");
    fs::write(source_root.join("root.txt"), "root").expect("root file should exist");
    fs::write(source_root.join("nested/child.txt"), "child").expect("child file should exist");

    let copy_dest = root.join("copy-dest");
    let move_dest = root.join("move-dest");
    fs::create_dir_all(&copy_dest).expect("copy destination should exist");
    fs::create_dir_all(&move_dest).expect("move destination should exist");

    let (command_tx, event_rx, handle) = spawn_worker();
    let mut manager = JobManager::new();

    let copy_job = manager.enqueue(JobRequest::Copy {
        sources: vec![source_root.clone()],
        destination_dir: copy_dest.clone(),
        overwrite: OverwritePolicy::Skip,
    });
    command_tx
        .send(WorkerCommand::Run(Box::new(copy_job)))
        .expect("copy command should send");
    let copy_result = run_job(&event_rx, &mut manager);
    assert!(
        matches!(copy_result, JobEvent::Finished { result: Ok(()), .. }),
        "copy job should succeed"
    );
    let copied_root = copy_dest.join("source");
    assert!(copied_root.exists(), "copied tree should exist");
    assert_eq!(
        fs::read_to_string(copied_root.join("nested/child.txt"))
            .expect("copied child should be readable"),
        "child"
    );

    let move_job = manager.enqueue(JobRequest::Move {
        sources: vec![copied_root.clone()],
        destination_dir: move_dest.clone(),
        overwrite: OverwritePolicy::Skip,
    });
    command_tx
        .send(WorkerCommand::Run(Box::new(move_job)))
        .expect("move command should send");
    let move_result = run_job(&event_rx, &mut manager);
    assert!(
        matches!(move_result, JobEvent::Finished { result: Ok(()), .. }),
        "move job should succeed"
    );
    assert!(
        !copied_root.exists(),
        "source path should be removed after move"
    );
    let moved_root = move_dest.join("source");
    assert!(moved_root.exists(), "moved tree should exist");

    let delete_job = manager.enqueue(JobRequest::Delete {
        targets: vec![moved_root.clone()],
    });
    command_tx
        .send(WorkerCommand::Run(Box::new(delete_job)))
        .expect("delete command should send");
    let delete_result = run_job(&event_rx, &mut manager);
    assert!(
        matches!(delete_result, JobEvent::Finished { result: Ok(()), .. }),
        "delete job should succeed"
    );
    assert!(!moved_root.exists(), "target should be deleted");

    shutdown_worker(command_tx, handle);
    fs::remove_dir_all(&root).expect("temp tree should be removable");
}

#[test]
fn overwrite_policies_apply_end_to_end() {
    let root = make_temp_dir("policies");
    let source_dir = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(&source_dir).expect("source directory should exist");
    fs::create_dir_all(&destination).expect("destination directory should exist");

    let source_file = source_dir.join("demo.txt");
    let destination_file = destination.join("demo.txt");
    fs::write(&source_file, "source").expect("source file should exist");
    fs::write(&destination_file, "destination").expect("destination file should exist");

    let (command_tx, event_rx, handle) = spawn_worker();
    let mut manager = JobManager::new();

    let skip_job = manager.enqueue(JobRequest::Copy {
        sources: vec![source_file.clone()],
        destination_dir: destination.clone(),
        overwrite: OverwritePolicy::Skip,
    });
    command_tx
        .send(WorkerCommand::Run(Box::new(skip_job)))
        .expect("skip copy should send");
    let skip_result = run_job(&event_rx, &mut manager);
    assert!(
        matches!(skip_result, JobEvent::Finished { result: Ok(()), .. }),
        "skip policy should not fail"
    );
    assert_eq!(
        fs::read_to_string(&destination_file).expect("destination should be readable"),
        "destination",
        "skip policy should preserve destination"
    );

    let rename_job = manager.enqueue(JobRequest::Copy {
        sources: vec![source_file.clone()],
        destination_dir: destination.clone(),
        overwrite: OverwritePolicy::Rename,
    });
    command_tx
        .send(WorkerCommand::Run(Box::new(rename_job)))
        .expect("rename copy should send");
    let rename_result = run_job(&event_rx, &mut manager);
    assert!(
        matches!(rename_result, JobEvent::Finished { result: Ok(()), .. }),
        "rename policy should succeed"
    );
    assert_eq!(
        fs::read_to_string(destination.join("demo.txt.copy"))
            .expect("renamed destination should be readable"),
        "source",
        "rename policy should create alternate destination"
    );

    let overwrite_job = manager.enqueue(JobRequest::Copy {
        sources: vec![source_file.clone()],
        destination_dir: destination.clone(),
        overwrite: OverwritePolicy::Overwrite,
    });
    command_tx
        .send(WorkerCommand::Run(Box::new(overwrite_job)))
        .expect("overwrite copy should send");
    let overwrite_result = run_job(&event_rx, &mut manager);
    assert!(
        matches!(overwrite_result, JobEvent::Finished { result: Ok(()), .. }),
        "overwrite policy should succeed"
    );
    assert_eq!(
        fs::read_to_string(&destination_file).expect("destination should be readable"),
        "source",
        "overwrite policy should replace destination"
    );

    shutdown_worker(command_tx, handle);
    fs::remove_dir_all(&root).expect("temp tree should be removable");
}

#[test]
fn canceled_copy_job_reports_canceled_status() {
    let root = make_temp_dir("cancel-status");
    let source_dir = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(&source_dir).expect("source directory should exist");
    fs::create_dir_all(&destination).expect("destination directory should exist");

    let source_file = source_dir.join("big.bin");
    fs::write(&source_file, vec![9_u8; 4 * 1024 * 1024]).expect("source file should exist");

    let (command_tx, event_rx, handle) = spawn_worker();
    let mut manager = JobManager::new();
    let copy_job = manager.enqueue(JobRequest::Copy {
        sources: vec![source_file],
        destination_dir: destination,
        overwrite: OverwritePolicy::Skip,
    });
    assert!(
        manager.request_cancel(copy_job.id),
        "job should be cancelable while queued"
    );

    command_tx
        .send(WorkerCommand::Run(Box::new(copy_job)))
        .expect("copy command should send");
    let canceled_result = run_job(&event_rx, &mut manager);
    match canceled_result {
        JobEvent::Finished {
            result: Err(error), ..
        } => assert_eq!(error, JOB_CANCELED_MESSAGE),
        _ => panic!("job should finish with cancellation"),
    }
    assert_eq!(
        manager
            .last_job()
            .expect("job manager should keep record")
            .status,
        JobStatus::Canceled,
        "canceled job should have canceled status"
    );

    shutdown_worker(command_tx, handle);
    fs::remove_dir_all(&root).expect("temp tree should be removable");
}
