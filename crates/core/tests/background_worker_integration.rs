#![forbid(unsafe_code)]

use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rc_core::{BackgroundEvent, build_tree_ready_event};

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
fn build_tree_event_includes_root_entry() {
    let root = make_temp_dir("shutdown");
    fs::write(root.join("needle.txt"), "needle").expect("fixture file should be writable");

    let event = build_tree_ready_event(root.clone(), 2, 64);
    match event {
        BackgroundEvent::TreeReady {
            root: event_root,
            entries,
        } => {
            assert_eq!(event_root, root);
            assert!(
                entries.iter().any(|entry| entry.path == root),
                "tree event should include the root directory"
            );
        }
        other => panic!("expected tree-ready event, got {other:?}"),
    }

    fs::remove_dir_all(&root).expect("temp tree should be removable");
}
