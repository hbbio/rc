use super::*;
use crate::tests::drain_background;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

fn fixture() -> (tempfile::TempDir, AppState, vfs::extfs::Helper, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.pkg");
    fs::write(&source, b"data").unwrap();
    let helper_path = dir.path().join("fake");
    fs::write(&helper_path, b"#!/bin/sh\nset -eu\ncase \"$1\" in\nlist) printf '%s\\n' 'dr-xr-xr-x 1 0 0 0 Jan 1 2025 tree' '-rw-r--r-- 1 0 0 4 Jan 1 2025 tree/data' '-rw------- 1 0 0 0 Jan 1 2025 tree/generated' '-rw------- 1 0 0 4 Jan 1 2025 source.pkg';;\ncopyout) printf data > \"$4\";;\nesac\n").unwrap();
    fs::set_permissions(&helper_path, fs::Permissions::from_mode(0o700)).unwrap();
    let helper = vfs::extfs::Helper::from_path(&helper_path).unwrap();
    let mut app = AppState::new(dir.path().into()).unwrap();
    drain_background(&mut app);
    (dir, app, helper, source)
}

fn mounted(app: &mut AppState, helper: vfs::extfs::Helper, source: PathBuf) -> Arc<Mount> {
    app.queue_helper_mount(
        ActivePanel::Left,
        helper,
        Some(VfsPath::Local(source)),
        PathBuf::new(),
    );
    assert!(
        !app.active_panel().is_virtual(),
        "opening must remain asynchronous"
    );
    drain_background(app);
    assert!(app.active_panel().is_virtual(), "{}", app.status_line);
    Arc::clone(app.active_panel().mounted_location().unwrap().mount())
}

fn select(app: &mut AppState, name: &str) {
    app.active_panel_mut().cursor = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.name == name)
        .unwrap();
}

#[test]
fn mounted_panels_navigate_preview_keep_history_and_copy_only_selected_trees() {
    let (dir, mut app, helper, source) = fixture();
    let mount = mounted(&mut app, helper, source);
    let destination = dir.path().join("output");
    fs::create_dir(&destination).unwrap();
    select(&mut app, "tree");
    let selected = app.active_panel().selected_entry().unwrap().path.clone();
    app.queue_copy_or_move_job(
        TransferKind::Copy,
        vec![selected.clone()],
        destination.clone(),
        OverwritePolicy::Overwrite,
        OperationOrigin::Panel(ActivePanel::Left),
    );
    drain_background(&mut app);
    assert!(
        destination.join("tree/data").exists(),
        "{}; {:?}",
        app.status_line,
        app.jobs.jobs()
    );
    assert_eq!(fs::read(destination.join("tree/data")).unwrap(), b"data");
    assert_eq!(
        fs::read(destination.join("tree/generated")).unwrap(),
        b"data"
    );
    assert!(!destination.join("source.pkg").exists());
    assert_eq!(
        fs::metadata(destination.join("tree"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o555
    );
    fs::set_permissions(destination.join("tree"), fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        fs::metadata(destination.join("tree/generated"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(!fs::read_dir(&destination).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("stage")
    }));
    select(&mut app, "tree");
    assert!(app.open_selected_directory());
    app.queue_panel_refresh(ActivePanel::Left);
    drain_background(&mut app);
    select(&mut app, "data");
    assert!(app.open_selected_file_in_viewer());
    drain_background(&mut app);
    assert!(app.active_viewer().unwrap().mounted_location.is_some());
    assert_eq!(app.file_history.len(), 1);
    app.apply(AppCommand::CloseViewer).unwrap();
    let history = app.file_history[0].clone();
    app.reopen_history_entry(&history);
    drain_background(&mut app);
    assert!(app.active_viewer().unwrap().mounted_location.is_some());
    app.apply(AppCommand::CloseViewer).unwrap();
    app.close_vfs_mount(mount.id());
    drain_background(&mut app);
    assert!(mount.is_closed());
    assert!(!app.active_panel().is_virtual());
    app.reopen_history_entry(&history);
    assert!(app.take_pending_worker_commands().is_empty());
    assert!(app.status_line.contains("closed"));
}

#[test]
fn quick_cd_and_view_file_resolve_inside_mount_and_typed_parents_leave_it() {
    let (dir, mut app, helper, source) = fixture();
    let mount = mounted(&mut app, helper, source);
    fs::create_dir(dir.path().join("tree")).unwrap();
    app.apply(AppCommand::OpenQuickCd).unwrap();
    for ch in "tree".chars() {
        app.apply(AppCommand::DialogInputChar(ch)).unwrap();
    }
    app.poll_deferred_work_at(Instant::now() + Duration::from_secs(1));
    assert!(!app.take_pending_worker_commands().iter().any(|command| matches!(command, WorkerCommand::Run(job) if matches!(job.request, JobRequest::QuickCdSearch { .. }))));
    app.apply(AppCommand::DialogAccept).unwrap();
    drain_background(&mut app);
    assert_eq!(
        app.active_panel().mounted_location().unwrap().relative(),
        Path::new("tree")
    );
    app.view_vfs_file_input(&app.active_panel().location(), "data");
    drain_background(&mut app);
    assert_eq!(
        app.active_viewer().unwrap().path(),
        mount.root().identity().join("tree/data")
    );
    app.apply(AppCommand::CloseViewer).unwrap();
    let target = mount.root().identity().join("tree/../tree");
    app.submit_quick_cd(rc_shell::encode_path_text(target.as_os_str()), None);
    drain_background(&mut app);
    assert_eq!(
        app.active_panel().mounted_location().unwrap().relative(),
        Path::new("tree")
    );
    app.submit_quick_cd("../..".into(), None);
    drain_background(&mut app);
    assert!(!app.active_panel().is_virtual());
    assert_eq!(app.active_panel().cwd, dir.path());
}

#[test]
fn active_vfs_menu_reopens_mounts_and_cancel_discards_an_already_completed_open() {
    let (_dir, mut app, helper, source) = fixture();
    app.queue_helper_mount(
        ActivePanel::Left,
        helper,
        Some(VfsPath::Local(source)),
        PathBuf::new(),
    );
    let job = app
        .take_pending_worker_commands()
        .into_iter()
        .find_map(|command| match command {
            WorkerCommand::Run(job) => Some(*job),
            _ => None,
        })
        .unwrap();
    let id = job.id;
    let (jobs, _job_events) = std::sync::mpsc::channel();
    let (events, background) = std::sync::mpsc::channel();
    vfs::tasks::execute(job, &jobs, &events);
    assert!(app.request_cancel_for_job(id));
    let BackgroundEvent::VfsMounted { mount, .. } = background.try_recv().unwrap() else {
        panic!("mount completion");
    };
    app.handle_vfs_mounted(id, Arc::clone(&mount));
    assert!(mount.is_closed());
    assert!(!app.active_panel().is_virtual());

    let (_dir2, mut app, helper, source) = fixture();
    let mount = mounted(&mut app, helper, source);
    app.apply(AppCommand::OpenActiveVfs).unwrap();
    assert!(matches!(app.top_route(), Route::Dialog(_)));
    app.apply(AppCommand::DialogAccept).unwrap();
    assert!(matches!(app.top_route(), Route::Dialog(_)));
    app.act_on_vfs_mount(Arc::clone(&mount), 1);
    drain_background(&mut app);
    assert_eq!(
        app.panels[1].mounted_location().unwrap().mount().id(),
        mount.id()
    );
    app.close_vfs_mount(mount.id());
    drain_background(&mut app);
    assert!(app.panels.iter().all(|panel| !panel.is_virtual()));
}

#[test]
fn mounted_locations_block_native_mutations_and_comparison() {
    let (_dir, mut app, helper, source) = fixture();
    mounted(&mut app, helper, source);
    select(&mut app, "source.pkg");
    for command in [
        AppCommand::Move,
        AppCommand::Delete,
        AppCommand::EditEntry,
        AppCommand::CompareFiles,
        AppCommand::CompareDirectories,
    ] {
        app.apply(command).unwrap();
        assert!(app.take_pending_worker_commands().is_empty(), "{command:?}");
        assert!(matches!(app.top_route(), Route::FileManager), "{command:?}");
    }
    app.active_panel = ActivePanel::Right;
    let context = app.macro_context();
    for letter in ['F', 'P', 'D', 'T', 'S', 'U'] {
        let action = action_macros::ActionTemplate::parse(&format!("echo %{letter}")).unwrap();
        assert!(action.expand(&context, &[], |_| None).is_err(), "{letter}");
    }
    assert!(
        action_macros::ActionTemplate::parse("echo %d")
            .unwrap()
            .expand(&context, &[], |_| None)
            .is_ok()
    );
}

#[test]
fn source_container_cannot_be_overwritten_but_rename_can_copy_beside_it() {
    let (dir, mut app, helper, source) = fixture();
    mounted(&mut app, helper, source.clone());
    select(&mut app, "source.pkg");
    let selected = app.active_panel().selected_entry().unwrap().path.clone();
    for overwrite in [OverwritePolicy::Overwrite, OverwritePolicy::Rename] {
        app.queue_copy_or_move_job(
            TransferKind::Copy,
            vec![selected.clone()],
            dir.path().into(),
            overwrite,
            OperationOrigin::Panel(ActivePanel::Left),
        );
        drain_background(&mut app);
    }
    assert_eq!(fs::read(&source).unwrap(), b"data");
    assert_eq!(
        fs::read(dir.path().join("source.pkg.copy")).unwrap(),
        b"data"
    );
}

#[test]
fn closing_a_mount_drops_reverts_from_an_inflight_exit_and_cancels_nested_opens() {
    let (dir, mut app, helper, source) = fixture();
    let mount = mounted(&mut app, helper.clone(), source);
    let member = mount.root().resolve_input(Path::new("source.pkg")).unwrap();
    app.queue_helper_mount(ActivePanel::Left, helper, Some(member), PathBuf::new());
    let requests = app.take_pending_worker_commands();
    let nested = requests
        .iter()
        .find_map(|command| match command {
            WorkerCommand::Run(job)
                if matches!(job.request, JobRequest::Vfs(Task::OpenHelper { .. })) =>
            {
                Some(job.cancel_flag())
            }
            _ => None,
        })
        .unwrap();
    let destination = dir.path().join("vanishing");
    fs::create_dir(&destination).unwrap();
    assert!(app.set_active_panel_directory(destination.clone()).unwrap());
    fs::remove_dir(&destination).unwrap();
    app.close_vfs_mount(mount.id());
    assert!(nested.load(std::sync::atomic::Ordering::Relaxed));
    drain_background(&mut app);
    assert!(
        !app.active_panel().is_virtual(),
        "failed exit must not restore the closed mount"
    );
    assert!(app.vfs_workflow.pending.is_empty());
}

#[test]
fn helper_uri_parsing_and_stale_completion_do_not_replace_newer_navigation() {
    let (dir, mut app, helper, source) = fixture();
    let uri = source.join("fake://tree");
    assert!(app.open_helper_uri_using(ActivePanel::Left, &uri, vec![helper.clone()]));
    drain_background(&mut app);
    assert_eq!(
        app.active_panel().mounted_location().unwrap().relative(),
        Path::new("tree")
    );
    let id = app.active_panel().mounted_location().unwrap().mount().id();
    app.close_vfs_mount(id);
    drain_background(&mut app);
    app.queue_helper_mount(
        ActivePanel::Left,
        helper,
        Some(VfsPath::Local(source)),
        PathBuf::new(),
    );
    let other = dir.path().join("other");
    fs::create_dir(&other).unwrap();
    app.set_active_panel_directory(other.clone()).unwrap();
    drain_background(&mut app);
    assert_eq!(app.active_panel().cwd, other);
    assert!(!app.active_panel().is_virtual());
    assert!(app.vfs_workflow.mounts.is_empty());
}
