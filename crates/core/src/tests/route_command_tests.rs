use super::*;

fn select_tree_path(app: &mut AppState, path: &Path) {
    let Some(Route::Tree(tree)) = app.routes.last_mut() else {
        panic!("top route should be tree");
    };
    assert!(
        tree.select_path(path),
        "tree should contain {}",
        path.display()
    );
}

#[test]
fn tree_screen_selects_directory_for_active_panel() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-tree-screen-{stamp}"));
    let branch = root.join("branch");
    fs::create_dir_all(&branch).expect("must create temp tree");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenTree)
        .expect("tree screen should open");
    drain_background(&mut app);
    assert_eq!(app.key_context(), KeyContext::Tree);

    let Some(Route::Tree(tree)) = app.routes.last_mut() else {
        panic!("top route should be tree");
    };
    assert!(tree.select_path(&branch));

    app.apply(AppCommand::TreeOpenEntry)
        .expect("tree open should succeed");
    assert_eq!(app.key_context(), KeyContext::FileManager);
    assert_eq!(app.active_panel().cwd, branch);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn closing_tree_cancels_its_exact_build_job() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-tree-close-cancel-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp tree");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenTree)
        .expect("tree screen should open");
    let tree_job_id = match app.top_route() {
        Route::Tree(tree) => tree.scan_job_id().expect("tree scan should be pending"),
        _ => panic!("top route should be tree"),
    };

    app.apply(AppCommand::CloseTree)
        .expect("tree screen should close");
    assert_eq!(app.key_context(), KeyContext::FileManager);
    let commands = app.take_pending_worker_commands();
    assert!(commands.iter().any(|command| matches!(
        command,
        WorkerCommand::Cancel(job_id) if *job_id == tree_job_id
    )));

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn stale_tree_completion_cannot_replace_reopened_tree() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-tree-stale-{stamp}"));
    let branch = root.join("branch");
    fs::create_dir_all(&branch).expect("must create temp tree");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenTree)
        .expect("first tree screen should open");
    let first_job_id = match app.top_route() {
        Route::Tree(tree) => tree.scan_job_id().expect("tree scan should be pending"),
        _ => panic!("top route should be tree"),
    };
    app.apply(AppCommand::CloseTree)
        .expect("first tree screen should close");
    app.apply(AppCommand::OpenTree)
        .expect("second tree screen should open");
    let second_job_id = match app.top_route() {
        Route::Tree(tree) => tree.scan_job_id().expect("tree scan should be pending"),
        _ => panic!("top route should be tree"),
    };
    assert_ne!(first_job_id, second_job_id);

    let cancel_flag = AtomicBool::new(false);
    let stale_event = build_tree_ready_event(first_job_id, root.clone(), 4, 64, &cancel_flag)
        .expect("stale tree event should build");
    app.handle_background_event(stale_event);
    let Route::Tree(tree) = app.top_route() else {
        panic!("reopened tree should remain active");
    };
    assert!(tree.is_loading(), "stale result must not finish new tree");
    assert_eq!(tree.entries().len(), 1, "stale entries must be ignored");

    let current_event = build_tree_ready_event(second_job_id, root.clone(), 4, 64, &cancel_flag)
        .expect("current tree event should build");
    app.handle_background_event(current_event);
    let Route::Tree(tree) = app.top_route() else {
        panic!("reopened tree should remain active");
    };
    assert!(!tree.is_loading());
    assert!(tree.entries().iter().any(|entry| entry.path == branch));

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn tree_build_failure_is_retained_by_the_active_route() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-tree-failure-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp tree");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenTree)
        .expect("tree screen should open");
    let job_id = match app.top_route() {
        Route::Tree(tree) => tree.scan_job_id().expect("tree scan should be pending"),
        _ => panic!("top route should be tree"),
    };
    app.handle_job_event(JobEvent::Finished {
        id: job_id,
        result: Err(JobError::from_io(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "tree fixture denied",
        ))),
    });

    let Route::Tree(tree) = app.top_route() else {
        panic!("tree route should remain active after failure");
    };
    assert!(matches!(
        tree.load_state(),
        TreeLoadState::Failed(message) if message.contains("tree fixture denied")
    ));
    assert!(app.status_line.contains("Directory tree failed"));

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn tree_commands_navigate_search_rescan_and_forget_cached_subtrees() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-tree-actions-{stamp}"));
    let alpha = root.join("alpha");
    let alpha_child = alpha.join("child");
    let beta = root.join("beta");
    fs::create_dir_all(&alpha_child).expect("must create alpha subtree");
    fs::create_dir_all(&beta).expect("must create beta subtree");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenTree)
        .expect("tree screen should open");
    drain_background(&mut app);

    app.apply(AppCommand::Navigate(
        NavigationTarget::Tree,
        NavigationMotion::Right,
    ))
    .expect("right should select first child");
    assert_eq!(
        match app.top_route() {
            Route::Tree(tree) => tree.selected_entry().map(|entry| &entry.path),
            _ => None,
        },
        Some(&alpha)
    );
    app.apply(AppCommand::Navigate(
        NavigationTarget::Tree,
        NavigationMotion::Down,
    ))
    .expect("down should select next sibling");
    assert_eq!(
        match app.top_route() {
            Route::Tree(tree) => tree.selected_entry().map(|entry| &entry.path),
            _ => None,
        },
        Some(&beta)
    );
    app.apply(AppCommand::TreeSearchAppend('a'))
        .expect("incremental search should run");
    assert_eq!(
        match app.top_route() {
            Route::Tree(tree) => tree.selected_entry().map(|entry| &entry.path),
            _ => None,
        },
        Some(&alpha)
    );
    app.apply(AppCommand::TreeSearchBackspace)
        .expect("search should clear");
    app.apply(AppCommand::TreeToggleNavigation)
        .expect("navigation mode should toggle");
    assert!(matches!(
        app.top_route(),
        Route::Tree(tree) if tree.navigation_mode() == TreeNavigationMode::Static
    ));

    let added = alpha.join("added");
    fs::create_dir_all(&added).expect("must create rescan fixture");
    select_tree_path(&mut app, &alpha);
    app.apply(AppCommand::TreeRescan)
        .expect("first selected rescan should queue");
    let first_rescan = match app.top_route() {
        Route::Tree(tree) => tree.scan_job_id().expect("rescan should be pending"),
        _ => panic!("top route should be tree"),
    };
    app.apply(AppCommand::TreeRescan)
        .expect("second selected rescan should supersede first");
    let second_rescan = match app.top_route() {
        Route::Tree(tree) => tree.scan_job_id().expect("rescan should be pending"),
        _ => panic!("top route should be tree"),
    };
    assert_ne!(first_rescan, second_rescan);
    let commands = app.take_pending_worker_commands();
    assert!(commands.iter().any(|command| matches!(
        command,
        WorkerCommand::Cancel(job_id) if *job_id == first_rescan
    )));
    assert!(commands.iter().any(|command| matches!(
        command,
        WorkerCommand::Run(job)
            if job.id == second_rescan
                && matches!(&job.request, JobRequest::BuildTree { root, .. } if root == &alpha)
    )));
    app.restore_pending_worker_commands(commands);
    drain_background(&mut app);
    assert!(matches!(
        app.top_route(),
        Route::Tree(tree) if tree.entries().iter().any(|entry| entry.path == added)
    ));

    select_tree_path(&mut app, &alpha);
    app.apply(AppCommand::TreeForget)
        .expect("forget should remove cached subtree");
    assert!(matches!(
        app.top_route(),
        Route::Tree(tree)
            if tree.entries().iter().all(|entry| entry.path != alpha && entry.path != alpha_child)
    ));

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn tree_filesystem_actions_use_existing_jobs_and_refresh_the_cache() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let sandbox = env::temp_dir().join(format!("rc-tree-file-actions-{stamp}"));
    let root = sandbox.join("root");
    let destination = sandbox.join("destination");
    let copy_source = root.join("copy-source");
    let move_source = root.join("move-source");
    let mkdir_parent = root.join("mkdir-parent");
    let delete_target = root.join("delete-target");
    for path in [
        &copy_source,
        &move_source,
        &mkdir_parent,
        &delete_target,
        &destination,
    ] {
        fs::create_dir_all(path).expect("must create tree operation fixture");
    }

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.settings_mut().confirmation.confirm_overwrite = false;
    app.panels[1].cwd = destination.clone();
    app.panels[1]
        .refresh()
        .expect("passive panel destination should load");
    app.apply(AppCommand::OpenTree)
        .expect("tree screen should open");
    drain_background(&mut app);

    select_tree_path(&mut app, &copy_source);
    app.apply(AppCommand::TreeCopy)
        .expect("tree copy dialog should open");
    app.apply(AppCommand::DialogAccept)
        .expect("tree copy destination should submit");
    drain_background(&mut app);
    assert!(destination.join("copy-source").is_dir());
    assert!(copy_source.is_dir(), "copy must preserve the source");

    select_tree_path(&mut app, &move_source);
    app.apply(AppCommand::TreeMove)
        .expect("tree move dialog should open");
    app.apply(AppCommand::DialogAccept)
        .expect("tree move destination should submit");
    drain_background(&mut app);
    assert!(destination.join("move-source").is_dir());
    assert!(!move_source.exists());
    assert!(matches!(
        app.top_route(),
        Route::Tree(tree) if tree.entries().iter().all(|entry| entry.path != move_source)
    ));

    select_tree_path(&mut app, &mkdir_parent);
    app.apply(AppCommand::TreeMkdir)
        .expect("tree mkdir dialog should open");
    for ch in "child".chars() {
        app.apply(AppCommand::DialogInputChar(ch))
            .expect("mkdir name should be editable");
    }
    app.apply(AppCommand::DialogAccept)
        .expect("tree mkdir should submit");
    drain_background(&mut app);
    let created = mkdir_parent.join("child");
    assert!(created.is_dir());
    assert!(matches!(
        app.top_route(),
        Route::Tree(tree) if tree.entries().iter().any(|entry| entry.path == created)
    ));
    assert_eq!(
        app.active_panel().cwd,
        root,
        "tree mkdir should update the tree without navigating the underlying panel"
    );

    select_tree_path(&mut app, &delete_target);
    app.apply(AppCommand::TreeDelete)
        .expect("tree delete confirmation should open");
    app.apply(AppCommand::DialogAccept)
        .expect("tree delete should confirm");
    drain_background(&mut app);
    assert!(!delete_target.exists());
    assert!(matches!(
        app.top_route(),
        Route::Tree(tree) if tree.entries().iter().all(|entry| entry.path != delete_target)
    ));

    fs::remove_dir_all(&sandbox).expect("must remove temp sandbox");
}

#[test]
fn tree_destructive_actions_protect_the_scan_root() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-tree-root-protection-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenTree)
        .expect("tree screen should open");
    drain_background(&mut app);

    for (command, expected_status) in [
        (AppCommand::TreeForget, "cannot be forgotten"),
        (AppCommand::TreeMove, "cannot be moved"),
        (AppCommand::TreeDelete, "cannot be deleted"),
    ] {
        app.apply(command).expect("root protection should apply");
        assert!(app.status_line.contains(expected_status));
        assert!(matches!(app.top_route(), Route::Tree(_)));
    }
    assert!(app.take_pending_worker_commands().is_empty());

    fs::remove_dir_all(&root).expect("must remove temp root");
}

fn submit_hotlist_entry(app: &mut AppState, label: &str, path: &Path) {
    app.finish_dialog(DialogResult::PairInputSubmitted {
        first: label.to_string(),
        second: path.to_string_lossy().into_owned(),
    });
}

#[test]
fn hotlist_supports_add_edit_confirmed_remove_and_open() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-hotlist-{stamp}"));
    let branch = root.join("branch");
    fs::create_dir_all(&branch).expect("must create temp tree");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenHotlist)
        .expect("hotlist should open");
    app.apply(AppCommand::HotlistAddCurrentDirectory)
        .expect("hotlist add should succeed");
    assert_eq!(app.key_context(), KeyContext::Input);
    submit_hotlist_entry(&mut app, "Project root", &root);
    let canonical_root = fs::canonicalize(&root).expect("root should canonicalize");
    assert_eq!(
        app.hotlist(),
        std::slice::from_ref(&HotlistEntry::new("Project root", canonical_root.clone()))
    );

    {
        let panel = app.active_panel_mut();
        panel.cwd = branch.clone();
        panel.refresh().expect("panel should refresh");
    }
    app.apply(AppCommand::HotlistAddCurrentDirectory)
        .expect("hotlist add should succeed");
    submit_hotlist_entry(&mut app, "Branch", &branch);
    let canonical_branch = fs::canonicalize(&branch).expect("branch should canonicalize");
    assert_eq!(
        app.hotlist(),
        &[
            HotlistEntry::new("Project root", canonical_root.clone()),
            HotlistEntry::new("Branch", canonical_branch.clone()),
        ]
    );

    app.hotlist_cursor = 0;
    app.apply(AppCommand::HotlistEditSelected)
        .expect("hotlist edit should open");
    submit_hotlist_entry(&mut app, "Workspace", &root);
    assert_eq!(app.hotlist()[0].label, "Workspace");

    app.apply(AppCommand::HotlistRemoveSelected)
        .expect("hotlist removal confirmation should open");
    assert!(matches!(app.top_route(), Route::Dialog(_)));
    app.finish_dialog(DialogResult::ConfirmDeclined);
    assert_eq!(app.hotlist().len(), 2);
    app.apply(AppCommand::HotlistRemoveSelected)
        .expect("hotlist removal confirmation should reopen");
    app.finish_dialog(DialogResult::ConfirmAccepted);
    assert_eq!(
        app.hotlist(),
        std::slice::from_ref(&HotlistEntry::new("Branch", canonical_branch.clone()))
    );
    assert_eq!(app.hotlist_cursor, 0);

    app.hotlist_cursor = 0;
    app.apply(AppCommand::HotlistOpenEntry)
        .expect("hotlist open should succeed");
    assert_eq!(app.key_context(), KeyContext::FileManager);
    assert_eq!(app.active_panel().cwd, canonical_branch);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn hotlist_rejects_duplicate_labels_paths_and_invalid_directories() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-hotlist-validation-{stamp}"));
    let branch = root.join("branch");
    let file = root.join("file.txt");
    fs::create_dir_all(&branch).expect("must create temp tree");
    fs::write(&file, "not a directory").expect("must create file fixture");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenHotlist)
        .expect("hotlist should open");
    app.apply(AppCommand::HotlistAddCurrentDirectory)
        .expect("hotlist add should open");
    submit_hotlist_entry(&mut app, "Alpha", &root);
    assert_eq!(app.hotlist().len(), 1);

    app.apply(AppCommand::HotlistAddCurrentDirectory)
        .expect("second add should open");
    app.finish_dialog(DialogResult::PairInputSubmitted {
        first: String::from("   "),
        second: branch.to_string_lossy().into_owned(),
    });
    assert!(app.status_line.contains("label cannot be empty"));
    assert_eq!(app.key_context(), KeyContext::Input);

    app.finish_dialog(DialogResult::PairInputSubmitted {
        first: String::from("Beta"),
        second: String::from("  "),
    });
    assert!(app.status_line.contains("directory cannot be empty"));
    assert_eq!(app.key_context(), KeyContext::Input);

    submit_hotlist_entry(&mut app, " alpha ", &branch);
    assert!(app.status_line.contains("label already exists"));
    assert_eq!(app.key_context(), KeyContext::Input);
    assert_eq!(app.hotlist().len(), 1);

    submit_hotlist_entry(&mut app, "Beta", &root.join("."));
    assert!(app.status_line.contains("directory already exists"));
    assert_eq!(app.key_context(), KeyContext::Input);

    submit_hotlist_entry(&mut app, "Beta", &root.join("missing"));
    assert!(app.status_line.contains("does not exist"));
    assert_eq!(app.key_context(), KeyContext::Input);

    submit_hotlist_entry(&mut app, "Beta", &file);
    assert!(app.status_line.contains("not a directory"));
    assert_eq!(app.key_context(), KeyContext::Input);

    submit_hotlist_entry(&mut app, "Beta", &branch);
    assert_eq!(app.key_context(), KeyContext::Hotlist);
    assert_eq!(app.hotlist().len(), 2);

    app.hotlist_cursor = 1;
    app.apply(AppCommand::HotlistEditSelected)
        .expect("hotlist edit should open");
    submit_hotlist_entry(&mut app, "ALPHA", &branch);
    assert!(app.status_line.contains("label already exists"));
    assert_eq!(app.key_context(), KeyContext::Input);
    submit_hotlist_entry(&mut app, "Beta", &branch);
    assert_eq!(app.key_context(), KeyContext::Hotlist);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn hotlist_removal_can_skip_confirmation_and_clamps_cursor() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-hotlist-remove-{stamp}"));
    let first = root.join("first");
    let second = root.join("second");
    fs::create_dir_all(&first).expect("must create first directory");
    fs::create_dir_all(&second).expect("must create second directory");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.settings_mut().configuration.hotlist = vec![
        HotlistEntry::new("First", first),
        HotlistEntry::new("Second", second),
    ];
    app.settings_mut().confirmation.confirm_hotlist_delete = false;
    app.apply(AppCommand::OpenHotlist)
        .expect("hotlist should open");
    app.hotlist_cursor = 1;
    app.apply(AppCommand::HotlistRemoveSelected)
        .expect("hotlist removal should succeed");

    assert_eq!(app.hotlist().len(), 1);
    assert_eq!(app.hotlist_cursor, 0);
    assert_eq!(app.key_context(), KeyContext::Hotlist);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn opening_missing_hotlist_entry_keeps_route_and_explains_failure() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-hotlist-open-missing-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.settings_mut().configuration.hotlist =
        vec![HotlistEntry::new("Missing", root.join("missing"))];
    app.apply(AppCommand::OpenHotlist)
        .expect("hotlist should open");
    app.apply(AppCommand::HotlistOpenEntry)
        .expect("hotlist open should report failure");

    assert!(app.status_line.contains("does not exist"));
    assert_eq!(app.key_context(), KeyContext::Hotlist);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn stale_hotlist_edit_cannot_overwrite_a_changed_entry() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-hotlist-stale-edit-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.settings_mut().configuration.hotlist = vec![HotlistEntry::new("Original", root.clone())];
    app.apply(AppCommand::OpenHotlist)
        .expect("hotlist should open");
    app.apply(AppCommand::HotlistEditSelected)
        .expect("hotlist editor should open");
    app.settings_mut().configuration.hotlist[0].label = String::from("Externally changed");
    submit_hotlist_entry(&mut app, "Replacement", &root);

    assert_eq!(app.hotlist()[0].label, "Externally changed");
    assert!(app.status_line.contains("selection changed"));
    assert_eq!(app.key_context(), KeyContext::Hotlist);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn xmap_mode_applies_to_next_file_manager_command_only() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-xmap-mode-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    assert_eq!(app.key_context(), KeyContext::FileManager);
    app.apply(AppCommand::EnterXMap)
        .expect("xmap mode should activate");
    assert_eq!(app.key_context(), KeyContext::FileManagerXMap);
    app.apply(AppCommand::Navigate(
        NavigationTarget::FileManager,
        NavigationMotion::Down,
    ))
    .expect("next command should execute");
    assert_eq!(app.key_context(), KeyContext::FileManager);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn resolve_external_editor_command_prefers_editor_over_visual() {
    let editor = resolve_external_editor_command_with_lookup(
        Some("  hx --wait  "),
        |name| match name {
            "EDITOR" => Some(String::from("  nvim  ")),
            "VISUAL" => Some(String::from("vim")),
            _ => None,
        },
        |_| false,
    );
    assert_eq!(editor, Some(String::from("hx --wait")));
}

#[test]
fn resolve_external_editor_command_uses_env_then_path_probe() {
    let editor = resolve_external_editor_command_with_lookup(
        None,
        |name| match name {
            "EDITOR" => Some(String::from("  ")),
            "VISUAL" => Some(String::from(" code --wait ")),
            _ => None,
        },
        |_| false,
    );
    assert_eq!(editor, Some(String::from("code --wait")));

    let probed =
        resolve_external_editor_command_with_lookup(None, |_| None, |name| matches!(name, "vim"));
    assert_eq!(probed, Some(String::from("vim")));

    let missing = resolve_external_editor_command_with_lookup(None, |_| None, |_| false);
    assert_eq!(missing, None);
}

#[cfg(unix)]
#[test]
fn executable_candidate_requires_execute_bit() {
    use std::os::unix::fs::PermissionsExt;

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-editor-path-probe-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let editor = root.join("vim");
    fs::write(&editor, "#!/bin/sh\n").expect("editor fixture should be writable");

    fs::set_permissions(&editor, fs::Permissions::from_mode(0o644))
        .expect("permissions should be writable");
    assert!(
        !executable_candidate_exists(&root, "vim"),
        "non-executable files should not satisfy PATH probing"
    );

    fs::set_permissions(&editor, fs::Permissions::from_mode(0o755))
        .expect("permissions should be writable");
    assert!(
        executable_candidate_exists(&root, "vim"),
        "executable files should satisfy PATH probing"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn app_command_mapping_is_context_aware() {
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenHelp),
        Some(AppCommand::OpenHelp)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Help, &KeyCommand::Quit),
        Some(AppCommand::CloseHelp)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Help, &KeyCommand::HelpBack),
        Some(AppCommand::HelpBack)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenMenu),
        Some(AppCommand::OpenMenu)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManagerXMap, &KeyCommand::PanelInfo),
        Some(AppCommand::SetOtherPanelView(PanelViewMode::Info))
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManagerXMap, &KeyCommand::PanelQuickView),
        Some(AppCommand::SetOtherPanelView(PanelViewMode::QuickView))
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::CursorUp),
        Some(AppCommand::Navigate(
            NavigationTarget::Menu,
            NavigationMotion::Up,
        ))
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::CursorDown),
        Some(AppCommand::Navigate(
            NavigationTarget::Menu,
            NavigationMotion::Down,
        ))
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::CursorLeft),
        Some(AppCommand::Navigate(
            NavigationTarget::Menu,
            NavigationMotion::Left,
        ))
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::CursorRight),
        Some(AppCommand::Navigate(
            NavigationTarget::Menu,
            NavigationMotion::Right,
        ))
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::DialogAccept),
        Some(AppCommand::MenuAccept)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::DialogCancel),
        Some(AppCommand::CloseMenu)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::CursorUp),
        Some(AppCommand::Navigate(
            NavigationTarget::FileManager,
            NavigationMotion::Up,
        ))
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::CursorLeft),
        Some(AppCommand::Navigate(
            NavigationTarget::FileManager,
            NavigationMotion::Left,
        ))
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::CursorRight),
        Some(AppCommand::Navigate(
            NavigationTarget::FileManager,
            NavigationMotion::Right,
        ))
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenEntry),
        Some(AppCommand::OpenEntry)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::EditEntry),
        Some(AppCommand::EditEntry)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::QuickCd),
        Some(AppCommand::OpenQuickCd)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Listbox, &KeyCommand::CursorUp),
        Some(AppCommand::DialogListboxUp)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Input, &KeyCommand::CursorUp),
        Some(AppCommand::DialogListboxUp)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Input, &KeyCommand::CursorDown),
        Some(AppCommand::DialogListboxDown)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Listbox, &KeyCommand::OpenInputDialog),
        Some(AppCommand::PanelizePresetAdd)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Listbox, &KeyCommand::OpenConfirmDialog),
        Some(AppCommand::PanelizePresetEdit)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Listbox, &KeyCommand::Delete),
        Some(AppCommand::PanelizePresetRemove)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::DialogAccept),
        Some(AppCommand::DialogAccept)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::ToggleTag),
        Some(AppCommand::ToggleTag)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::SortNext),
        Some(AppCommand::SortNext)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::CycleListingFormat),
        Some(AppCommand::CycleListingFormat)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenListingFormat),
        Some(AppCommand::OpenListingFormat)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenSortOrder),
        Some(AppCommand::OpenSortOrder)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenPanelFilter),
        Some(AppCommand::OpenPanelFilter)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::Copy),
        Some(AppCommand::Copy)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::Move),
        Some(AppCommand::Move)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::Delete),
        Some(AppCommand::Delete)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::CancelJob),
        Some(AppCommand::CancelJob)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenJobs),
        Some(AppCommand::OpenJobsScreen)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenFindDialog),
        Some(AppCommand::OpenFindDialog)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::CursorDown),
        Some(AppCommand::Navigate(
            NavigationTarget::FindResults,
            NavigationMotion::Down,
        ))
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::OpenEntry),
        Some(AppCommand::FindResultsOpenEntry)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::OpenPanelizeDialog),
        Some(AppCommand::FindResultsPanelize)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::CancelJob),
        Some(AppCommand::CancelJob)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::FindAgain),
        Some(AppCommand::FindResultsAgain)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::FindTogglePause),
        Some(AppCommand::FindResultsTogglePause)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FindDialog, &KeyCommand::OpenTree),
        Some(AppCommand::FindDialogBrowse)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::Quit),
        Some(AppCommand::CloseFindResults)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenTree),
        Some(AppCommand::OpenTree)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::CursorUp),
        Some(AppCommand::Navigate(
            NavigationTarget::Tree,
            NavigationMotion::Up,
        ))
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::OpenEntry),
        Some(AppCommand::TreeOpenEntry)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::CursorLeft),
        Some(AppCommand::Navigate(
            NavigationTarget::Tree,
            NavigationMotion::Left,
        ))
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::Reread),
        Some(AppCommand::TreeRescan)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::Forget),
        Some(AppCommand::TreeForget)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::ToggleNavigation),
        Some(AppCommand::TreeToggleNavigation)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::Copy),
        Some(AppCommand::TreeCopy)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::Move),
        Some(AppCommand::TreeMove)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::OpenInputDialog),
        Some(AppCommand::TreeMkdir)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::Delete),
        Some(AppCommand::TreeDelete)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::Search),
        Some(AppCommand::TreeSearchNext)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::DialogBackspace),
        Some(AppCommand::TreeSearchBackspace)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::Quit),
        Some(AppCommand::CloseTree)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenHotlist),
        Some(AppCommand::OpenHotlist)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenPanelizeDialog),
        Some(AppCommand::OpenPanelizeDialog)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenSkinDialog),
        Some(AppCommand::OpenSkinDialog)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::EnterXMap),
        Some(AppCommand::EnterXMap)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Hotlist, &KeyCommand::AddHotlist),
        Some(AppCommand::HotlistAddCurrentDirectory)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManagerXMap, &KeyCommand::AddHotlist),
        Some(AppCommand::HotlistAddCurrentDirectory)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Hotlist, &KeyCommand::EditHotlist),
        Some(AppCommand::HotlistEditSelected)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Hotlist, &KeyCommand::RemoveHotlist),
        Some(AppCommand::HotlistRemoveSelected)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Hotlist, &KeyCommand::OpenEntry),
        Some(AppCommand::HotlistOpenEntry)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Hotlist, &KeyCommand::Quit),
        Some(AppCommand::CloseHotlist)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Jobs, &KeyCommand::CursorUp),
        Some(AppCommand::Navigate(
            NavigationTarget::Jobs,
            NavigationMotion::Up,
        ))
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Jobs, &KeyCommand::CursorDown),
        Some(AppCommand::Navigate(
            NavigationTarget::Jobs,
            NavigationMotion::Down,
        ))
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Jobs, &KeyCommand::CloseJobs),
        Some(AppCommand::CloseJobsScreen)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::Quit),
        Some(AppCommand::CloseViewer)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::Search),
        Some(AppCommand::ViewerSearchForward)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::SearchBackward),
        Some(AppCommand::ViewerSearchBackward)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::SearchContinue),
        Some(AppCommand::ViewerSearchContinue)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::SearchContinueBackward),
        Some(AppCommand::ViewerSearchContinueBackward)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::Goto),
        Some(AppCommand::ViewerGoto)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::ToggleWrap),
        Some(AppCommand::ViewerToggleWrap)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::ToggleHex),
        Some(AppCommand::ViewerToggleHex)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::ViewerHex, &KeyCommand::ToggleHex),
        Some(AppCommand::ViewerToggleHex)
    );
}
