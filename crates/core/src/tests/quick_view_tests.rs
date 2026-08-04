use super::*;

fn temp_root(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-quick-view-{label}-{stamp}"));
    fs::create_dir_all(&root).expect("quick-view root should be creatable");
    root
}

fn select_path(app: &mut AppState, panel: ActivePanel, path: &Path) {
    app.panels[panel.index()].cursor = app.panels[panel.index()]
        .entries
        .iter()
        .position(|entry| entry.path == path)
        .expect("fixture path should be listed");
}

#[test]
fn quick_view_targets_its_named_panel_and_loads_the_other_selection() {
    let root = temp_root("target");
    let preview_path = root.join("preview.txt");
    fs::write(&preview_path, "alpha\nbeta\n").expect("preview fixture should be writable");

    let mut app = app_with_loaded_panels(root.clone());
    select_path(&mut app, ActivePanel::Right, &preview_path);
    app.apply(AppCommand::Panel(
        ActivePanel::Left,
        PanelCommand::SetView(PanelViewMode::QuickView),
    ))
    .expect("left quick view should open");

    assert_eq!(
        app.panel_view_mode(ActivePanel::Left),
        PanelViewMode::QuickView
    );
    assert_eq!(
        app.panel_view_mode(ActivePanel::Right),
        PanelViewMode::Listing
    );
    assert_eq!(app.active_panel, ActivePanel::Right);
    assert!(
        !app.toggle_active_panel(),
        "quick-view panels cannot take focus"
    );
    assert!(matches!(
        app.quick_view_state(ActivePanel::Left),
        QuickViewState::Loading { path } if path == &preview_path
    ));

    drain_background(&mut app);
    let QuickViewState::Ready(viewer) = app.quick_view_state(ActivePanel::Left) else {
        panic!("quick view should contain the loaded viewer state");
    };
    assert_eq!(viewer.path(), preview_path);
    assert_eq!(viewer.content(), "alpha\nbeta\n");
    assert!(viewer.wrap, "panel previews should wrap text by default");

    fs::remove_dir_all(root).expect("quick-view root should be removable");
}

#[test]
fn pending_quick_view_requests_coalesce_to_the_latest_cursor_selection() {
    let root = temp_root("coalesce");
    let alpha = root.join("alpha.txt");
    let beta = root.join("beta.txt");
    fs::write(&alpha, "alpha").expect("alpha fixture should be writable");
    fs::write(&beta, "beta").expect("beta fixture should be writable");

    let mut app = app_with_loaded_panels(root.clone());
    select_path(&mut app, ActivePanel::Right, &alpha);
    app.apply(AppCommand::Panel(
        ActivePanel::Left,
        PanelCommand::SetView(PanelViewMode::QuickView),
    ))
    .expect("quick view should open");
    app.apply(AppCommand::Navigate(
        NavigationTarget::FileManager,
        NavigationMotion::Down,
    ))
    .expect("cursor should move to beta");

    let commands = app.take_pending_worker_commands();
    assert_eq!(
        commands.len(),
        1,
        "the queued preview should be replaced in place"
    );
    let WorkerCommand::Run(job) = &commands[0] else {
        panic!("the coalesced command should remain a worker run");
    };
    assert!(matches!(
        &job.request,
        JobRequest::LoadQuickView {
            panel: ActivePanel::Left,
            path,
            ..
        } if path == &beta
    ));
    assert!(
        app.jobs
            .job(job.id)
            .is_some_and(|record| record.summary.contains("beta.txt")),
        "the visible job record should describe the replacement request"
    );

    fs::remove_dir_all(root).expect("quick-view root should be removable");
}

#[test]
fn stale_quick_view_completion_cannot_replace_a_newer_preview() {
    let root = temp_root("stale");
    let alpha = root.join("alpha.txt");
    let beta = root.join("beta.txt");
    fs::write(&alpha, "old").expect("alpha fixture should be writable");
    fs::write(&beta, "new").expect("beta fixture should be writable");

    let mut app = app_with_loaded_panels(root.clone());
    select_path(&mut app, ActivePanel::Right, &alpha);
    app.apply(AppCommand::Panel(
        ActivePanel::Left,
        PanelCommand::SetView(PanelViewMode::QuickView),
    ))
    .expect("quick view should open");
    let first = app.take_pending_worker_commands();
    let WorkerCommand::Run(first_job) = &first[0] else {
        panic!("first preview should be queued");
    };
    let JobRequest::LoadQuickView {
        request_id: first_request_id,
        ..
    } = &first_job.request
    else {
        panic!("first job should load quick view");
    };
    let first_request_id = *first_request_id;

    app.apply(AppCommand::Navigate(
        NavigationTarget::FileManager,
        NavigationMotion::Down,
    ))
    .expect("cursor should move to beta");
    let second = app.take_pending_worker_commands();
    let second_request_id = second
        .iter()
        .find_map(|command| match command {
            WorkerCommand::Run(job) => match &job.request {
                JobRequest::LoadQuickView {
                    path, request_id, ..
                } if path == &beta => Some(*request_id),
                _ => None,
            },
            _ => None,
        })
        .expect("the beta preview should be queued");

    app.handle_background_event(BackgroundEvent::QuickViewLoaded {
        panel: ActivePanel::Left,
        path: alpha.clone(),
        request_id: first_request_id,
        result: Ok(ViewerState::open(alpha).expect("alpha viewer should load")),
    });
    assert!(matches!(
        app.quick_view_state(ActivePanel::Left),
        QuickViewState::Loading { path } if path == &beta
    ));

    app.handle_background_event(BackgroundEvent::QuickViewLoaded {
        panel: ActivePanel::Left,
        path: beta.clone(),
        request_id: second_request_id,
        result: Ok(ViewerState::open(beta.clone()).expect("beta viewer should load")),
    });
    assert!(matches!(
        app.quick_view_state(ActivePanel::Left),
        QuickViewState::Ready(viewer) if viewer.path() == beta
    ));

    fs::remove_dir_all(root).expect("quick-view root should be removable");
}

#[test]
fn quick_view_handles_directories_without_starting_file_io() {
    let root = temp_root("directory");
    let directory = root.join("child");
    fs::create_dir_all(&directory).expect("child directory should be creatable");

    let mut app = app_with_loaded_panels(root.clone());
    select_path(&mut app, ActivePanel::Right, &directory);
    app.apply(AppCommand::Panel(
        ActivePanel::Left,
        PanelCommand::SetView(PanelViewMode::QuickView),
    ))
    .expect("quick view should open");

    assert!(matches!(
        app.quick_view_state(ActivePanel::Left),
        QuickViewState::Directory { path } if path == &directory
    ));
    assert!(
        app.take_pending_worker_commands().is_empty(),
        "directory quick view should use cached panel metadata only"
    );

    fs::remove_dir_all(root).expect("quick-view root should be removable");
}

#[test]
fn leaving_quick_view_cancels_the_load_and_invalidates_late_completion() {
    let root = temp_root("close");
    let preview_path = root.join("preview.txt");
    fs::write(&preview_path, "payload").expect("preview fixture should be writable");

    let mut app = app_with_loaded_panels(root.clone());
    select_path(&mut app, ActivePanel::Right, &preview_path);
    app.apply(AppCommand::Panel(
        ActivePanel::Left,
        PanelCommand::SetView(PanelViewMode::QuickView),
    ))
    .expect("quick view should open");
    let pending = app.take_pending_worker_commands();
    let WorkerCommand::Run(job) = &pending[0] else {
        panic!("preview load should be queued");
    };
    let job_id = job.id;
    let JobRequest::LoadQuickView { request_id, .. } = &job.request else {
        panic!("queued job should be a quick-view load");
    };
    let request_id = *request_id;

    app.apply(AppCommand::Panel(
        ActivePanel::Left,
        PanelCommand::SetView(PanelViewMode::Listing),
    ))
    .expect("file listing should replace quick view");
    assert_eq!(
        app.panel_view_mode(ActivePanel::Left),
        PanelViewMode::Listing
    );
    assert!(matches!(
        app.quick_view_state(ActivePanel::Left),
        QuickViewState::Empty
    ));
    assert!(
        app.take_pending_worker_commands()
            .iter()
            .any(|command| matches!(command, WorkerCommand::Cancel(id) if *id == job_id))
    );

    app.handle_background_event(BackgroundEvent::QuickViewLoaded {
        panel: ActivePanel::Left,
        path: preview_path.clone(),
        request_id,
        result: Ok(ViewerState::open(preview_path).expect("viewer fixture should open")),
    });
    assert!(matches!(
        app.quick_view_state(ActivePanel::Left),
        QuickViewState::Empty
    ));

    fs::remove_dir_all(root).expect("quick-view root should be removable");
}

#[test]
fn canceling_a_quick_view_job_rejects_a_racing_completion() {
    let root = temp_root("cancel");
    let preview_path = root.join("preview.txt");
    fs::write(&preview_path, "payload").expect("preview fixture should be writable");

    let mut app = app_with_loaded_panels(root.clone());
    select_path(&mut app, ActivePanel::Right, &preview_path);
    app.apply(AppCommand::Panel(
        ActivePanel::Left,
        PanelCommand::SetView(PanelViewMode::QuickView),
    ))
    .expect("quick view should open");
    let pending = app.take_pending_worker_commands();
    let WorkerCommand::Run(job) = &pending[0] else {
        panic!("preview load should be queued");
    };
    let JobRequest::LoadQuickView { request_id, .. } = &job.request else {
        panic!("queued job should be a quick-view load");
    };
    let request_id = *request_id;

    assert!(app.request_cancel_for_job(job.id));
    assert!(matches!(
        app.quick_view_state(ActivePanel::Left),
        QuickViewState::Failed { error, .. } if error == "Preview canceled"
    ));
    app.handle_background_event(BackgroundEvent::QuickViewLoaded {
        panel: ActivePanel::Left,
        path: preview_path.clone(),
        request_id,
        result: Ok(ViewerState::open(preview_path).expect("viewer fixture should open")),
    });
    assert!(matches!(
        app.quick_view_state(ActivePanel::Left),
        QuickViewState::Failed { error, .. } if error == "Preview canceled"
    ));

    fs::remove_dir_all(root).expect("quick-view root should be removable");
}
