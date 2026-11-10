#![cfg(unix)]
use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

fn run_change(paths: Vec<PathBuf>, change: MetadataChange) -> Vec<JobEvent> {
    let mut jobs = JobManager::default();
    let job = jobs.enqueue(JobRequest::ChangeMetadata { paths, change });
    let (tx, rx) = std::sync::mpsc::channel();
    execute_worker_job(job, &tx);
    drop(tx);
    rx.into_iter().collect()
}

#[test]
fn chmod_tags_preserves_children_and_refreshes_panel_metadata() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file");
    let dir = root.path().join("dir");
    fs::write(&file, "data").unwrap();
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("child"), "data").unwrap();
    fs::set_permissions(dir.join("child"), fs::Permissions::from_mode(0o600)).unwrap();
    let mut app = app_with_loaded_panels(root.path().into());
    app.active_panel_mut()
        .tagged
        .extend([file.clone(), dir.clone()]);
    app.apply(AppCommand::Chmod).unwrap();
    app.finish_dialog(DialogResult::InputSubmitted("u=rwX,g=rX,o=".into()));
    drain_background(&mut app);
    assert_eq!(fs::metadata(&file).unwrap().mode() & 0o7777, 0o640);
    assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o7777, 0o750);
    assert_eq!(
        fs::metadata(dir.join("child")).unwrap().mode() & 0o7777,
        0o600
    );
    let entry = app
        .active_panel()
        .entries
        .iter()
        .find(|e| e.path == file)
        .unwrap();
    assert_eq!(entry.metadata.mode.unwrap() & 0o7777, 0o640);
}

#[test]
fn chmod_uses_tagged_target_defaults_and_refreshes_panelized_metadata_without_reexecution() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file");
    fs::write(&file, "data").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(root.path().join("other"), "data").unwrap();
    let mut app = app_with_loaded_panels(root.path().into());
    app.start_panelize_command("printf x >> count; printf 'file\\nother\\n'".into());
    drain_background(&mut app);
    app.active_panel_mut().tagged.insert(file.clone());
    app.active_panel_mut().cursor = app
        .active_panel()
        .entries
        .iter()
        .position(|e| e.path == root.path().join("other"))
        .unwrap();
    app.apply(AppCommand::Chmod).unwrap();
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("chmod dialog")
    };
    let DialogKind::Input(input) = &dialog.kind else {
        panic!("input")
    };
    assert_eq!(input.value, "0600");
    app.finish_dialog(DialogResult::InputSubmitted("0640".into()));
    drain_background(&mut app);
    let entry = app
        .active_panel()
        .entries
        .iter()
        .find(|e| e.path == file)
        .unwrap();
    assert_eq!(entry.metadata.mode.unwrap() & 0o7777, 0o640);
    assert_eq!(fs::read(root.path().join("count")).unwrap(), b"x");
    app.active_panel_mut()
        .tagged
        .insert(root.path().join("other"));
    app.apply(AppCommand::Chmod).unwrap();
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("chmod dialog")
    };
    let DialogKind::Input(input) = &dialog.kind else {
        panic!("input")
    };
    assert!(input.value.is_empty());
}

#[test]
fn chmod_rejects_symlink_batch_before_modifying_any_entry() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file");
    let link = root.path().join("link");
    fs::write(&file, "data").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&file, &link).unwrap();
    let events = run_change(
        vec![file.clone(), link],
        MetadataChange::new("777", "", "").unwrap(),
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, JobEvent::Finished { result: Err(_), .. }))
    );
    assert_eq!(fs::metadata(file).unwrap().mode() & 0o7777, 0o600);
}

#[test]
fn chown_and_advanced_chown_resolve_ids_and_names_without_following_links() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file");
    let link = root.path().join("dangling");
    fs::write(&file, "data").unwrap();
    symlink("absent", &link).unwrap();
    let uid = nix::unistd::getuid();
    let gid = fs::metadata(&file).unwrap().gid();
    let user = nix::unistd::User::from_uid(uid).unwrap().unwrap();
    let group = nix::unistd::Group::from_gid(nix::unistd::Gid::from_raw(gid))
        .unwrap()
        .unwrap();
    for (path, change) in [
        (
            file.clone(),
            MetadataChange::new("0600", &user.name, &group.name).unwrap(),
        ),
        (
            link.clone(),
            MetadataChange::new("", &uid.as_raw().to_string(), &gid.to_string()).unwrap(),
        ),
    ] {
        let events = run_change(vec![path], change);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, JobEvent::Finished { result: Ok(()), .. })),
            "{events:?}"
        );
    }
    assert_eq!(fs::metadata(&file).unwrap().mode() & 0o7777, 0o600);
    assert_eq!(fs::symlink_metadata(&link).unwrap().uid(), uid.as_raw());
    assert_eq!(fs::read_link(&link).unwrap(), PathBuf::from("absent"));
    let events = run_change(
        vec![file.clone()],
        MetadataChange::new("0777", "rc-no-such-account-987654", "").unwrap(),
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, JobEvent::Finished { result: Err(_), .. }))
    );
    assert_eq!(fs::metadata(&file).unwrap().mode() & 0o7777, 0o600);
}

#[test]
fn metadata_dialog_validation_cancel_and_shortcuts_do_not_modify_files() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file");
    fs::write(&file, "data").unwrap();
    let mut app = app_with_loaded_panels(root.path().into());
    app.active_panel_mut().cursor = app
        .active_panel()
        .entries
        .iter()
        .position(|e| e.path == file)
        .unwrap();
    app.apply(AppCommand::AdvancedChown).unwrap();
    app.finish_dialog(DialogResult::PairInputSubmitted {
        first: "-1:".into(),
        second: "0600".into(),
    });
    assert!(matches!(app.top_route(), Route::Dialog(_)));
    app.apply(AppCommand::DialogCancel).unwrap();
    assert!(
        !app.jobs
            .jobs()
            .iter()
            .any(|j| j.kind == JobKind::ChangeMetadata)
    );
    let map = keymap::Keymap::bundled_mc_default().unwrap();
    for (key, expected) in [('c', AppCommand::Chmod), ('o', AppCommand::Chown)] {
        assert_eq!(
            map.resolve(
                KeyContext::FileManagerXMap,
                keymap::KeyChord::new(keymap::KeyCode::Char(key))
            )
            .and_then(|c| AppCommand::from_key_command(KeyContext::FileManagerXMap, c)),
            Some(expected)
        );
    }
}

#[test]
fn canceled_metadata_job_leaves_entries_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("file");
    fs::write(&path, "data").unwrap();
    let original = fs::metadata(&path).unwrap().mode();
    let mut jobs = JobManager::default();
    let job = jobs.enqueue(JobRequest::ChangeMetadata {
        paths: vec![path.clone()],
        change: MetadataChange::new("777", "", "").unwrap(),
    });
    job.cancel_flag()
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let (tx, rx) = std::sync::mpsc::channel();
    execute_worker_job(job, &tx);
    assert!(rx.try_iter().any(
        |e| matches!(e, JobEvent::Finished { result: Err(error), .. } if error.is_canceled())
    ));
    assert_eq!(fs::metadata(path).unwrap().mode(), original);
}
