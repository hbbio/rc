use super::*;

fn choose(app: &mut AppState, prefix: &str) {
    app.apply(AppCommand::OpenScreenList).unwrap();
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("screen list")
    };
    let DialogKind::Listbox(list) = &dialog.kind else {
        panic!("list")
    };
    let index = list
        .items
        .iter()
        .position(|s| s.starts_with(prefix))
        .unwrap();
    app.apply(AppCommand::DialogListboxSelectAt(index)).unwrap();
    app.apply(AppCommand::DialogAccept).unwrap();
}

#[test]
fn screen_switch_restores_viewer_state_and_cancel_preserves_stack() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("view.txt");
    fs::write(&path, "first\nsecond\nthird\n").unwrap();
    let mut app = app_with_loaded_panels(root.path().into());
    app.routes
        .push(Route::Viewer(ViewerState::open(path).unwrap()));
    app.apply(AppCommand::ViewerToggleHex).unwrap();
    choose(&mut app, "File manager");
    assert!(app.active_viewer().is_none());
    assert_eq!(app.suspended_screens.len(), 1);
    app.apply(AppCommand::OpenScreenList).unwrap();
    app.apply(AppCommand::DialogCancel).unwrap();
    assert!(matches!(app.top_route(), Route::FileManager));
    choose(&mut app, "View:");
    assert!(app.active_viewer().unwrap().hex_mode);
    assert_eq!(app.suspended_screens.len(), 0);
    app.apply(AppCommand::CloseViewer).unwrap();
    assert!(matches!(app.top_route(), Route::FileManager));
}

#[test]
fn suspended_find_and_tree_receive_background_completion() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("folder")).unwrap();
    fs::write(root.path().join("needle"), "content").unwrap();
    let mut app = app_with_loaded_panels(root.path().into());
    app.apply(AppCommand::OpenFindDialog).unwrap();
    for c in "needle".chars() {
        app.apply(AppCommand::DialogInputChar(c)).unwrap();
    }
    app.apply(AppCommand::DialogAccept).unwrap();
    let Route::FindResults(results) = app.top_route() else {
        panic!("find")
    };
    let id = results.job_id;
    choose(&mut app, "File manager");
    app.apply(AppCommand::OpenTree).unwrap();
    choose(&mut app, "File manager");
    drain_background(&mut app);
    let found = app.find_results_by_job_id(id).unwrap();
    assert_eq!(found.status, FindResultsStatus::Completed);
    assert!(
        found
            .entries
            .iter()
            .any(|e| e.path == root.path().join("needle"))
    );
    fs::create_dir(root.path().join("created-while-suspended")).unwrap();
    choose(&mut app, "Tree:");
    drain_background(&mut app);
    let Route::Tree(tree) = app.top_route() else {
        panic!("tree")
    };
    assert!(!tree.is_loading());
    assert!(
        tree.entries()
            .iter()
            .any(|e| e.path == root.path().join("folder"))
    );
    assert!(
        tree.entries()
            .iter()
            .any(|e| e.path == root.path().join("created-while-suspended"))
    );
    choose(&mut app, "Find:");
    assert!(matches!(app.top_route(), Route::FindResults(r) if r.job_id == id));
}

#[test]
fn opening_results_from_two_find_screens_cancels_the_superseded_paused_search() {
    let root = tempfile::tempdir().unwrap();
    let result_path = root.path().join("result.txt");
    fs::write(&result_path, b"result").unwrap();
    let mut app = app_with_loaded_panels(root.path().into());
    let start = |app: &mut AppState, pattern: &str| {
        let mut spec = FindSpec::new(root.path().into());
        spec.filename_pattern = pattern.into();
        app.start_find_search(spec);
        let Route::FindResults(results) = app.top_route() else {
            panic!("find results");
        };
        let id = results.job_id;
        let cancel = app
            .pending_worker_commands
            .iter()
            .find_map(|command| match command {
                WorkerCommand::Run(job) if job.id == id => Some(job.cancel_flag()),
                _ => None,
            })
            .unwrap();
        let pause = Arc::clone(&app.find_pause_flags[&id]);
        app.handle_job_event(JobEvent::Started { id });
        app.handle_background_event(BackgroundEvent::FindEntriesChunk {
            job_id: id,
            entries: vec![FindResultEntry {
                path: result_path.clone(),
                is_dir: false,
            }],
        });
        (id, pause, cancel)
    };
    let (a, pause_a, cancel_a) = start(&mut app, "A");
    choose(&mut app, "File manager");
    let (b, pause_b, cancel_b) = start(&mut app, "B");
    app.apply(AppCommand::FindResultsOpenEntry).unwrap();
    assert!(pause_b.load(AtomicOrdering::Relaxed));
    assert!(!cancel_b.load(AtomicOrdering::Relaxed));
    choose(&mut app, "Find: A");
    app.apply(AppCommand::FindResultsOpenEntry).unwrap();
    assert!(cancel_b.load(AtomicOrdering::Relaxed));
    assert!(
        app.pending_worker_commands
            .iter()
            .any(|command| matches!(command, WorkerCommand::Cancel(id) if *id == b))
    );
    assert_eq!(app.paused_find_results.as_ref().unwrap().job_id, a);
    assert!(pause_a.load(AtomicOrdering::Relaxed));
    assert!(!cancel_a.load(AtomicOrdering::Relaxed));
    app.apply(AppCommand::OpenFindDialog).unwrap();
    assert!(matches!(app.top_route(), Route::FindResults(results) if results.job_id == a));
    assert!(!pause_a.load(AtomicOrdering::Relaxed));
}

#[test]
fn restoring_pending_tree_invalidates_snapshot_from_before_suspended_mutation() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app_with_loaded_panels(root.path().into());
    app.apply(AppCommand::OpenTree).unwrap();
    let old_id = app.directory_tree().unwrap().scan_job_id().unwrap();
    let stale = build_tree_ready_event(old_id, root.path().into(), 8, 100, &AtomicBool::new(false))
        .unwrap();
    choose(&mut app, "File manager");
    fs::create_dir(root.path().join("new-dir")).unwrap();
    choose(&mut app, "Tree:");
    assert_ne!(app.directory_tree().unwrap().scan_job_id(), Some(old_id));
    app.handle_background_event(stale);
    assert!(app.directory_tree().unwrap().is_loading());
    drain_background(&mut app);
    assert!(
        app.directory_tree()
            .unwrap()
            .entries()
            .iter()
            .any(|entry| entry.path == root.path().join("new-dir"))
    );
}

#[test]
fn suspended_output_keeps_streaming_and_is_not_evicted_or_duplicated() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app_with_loaded_panels(root.path().into());
    let spec = CommandCaptureSpec::shell(
        root.path().into(),
        "echo kept".into(),
        app.settings.shell.clone(),
    );
    app.start_command_capture(spec.clone());
    let Route::CommandOutput(id) = *app.top_route() else {
        panic!("output")
    };
    choose(&mut app, "File manager");
    app.append_command_output(id, b"still running\n");
    app.complete_command_output(id, "done".into());
    for _ in 0..10 {
        app.start_command_capture(spec.clone());
        let Route::CommandOutput(next) = *app.top_route() else {
            panic!("output")
        };
        app.complete_command_output(next, "done".into());
        app.apply(AppCommand::CloseViewer).unwrap();
    }
    assert!(app.command_outputs.contains_key(&id));
    app.apply(AppCommand::OpenJobsScreen).unwrap();
    app.jobs_cursor = app.jobs.jobs().iter().position(|job| job.id == id).unwrap();
    app.apply(AppCommand::OpenJobOutput).unwrap();
    assert!(
        app.active_viewer()
            .unwrap()
            .content()
            .contains("still running")
    );
    let count = app
        .routes
        .iter()
        .chain(app.suspended_screens.iter().flatten())
        .filter(|r| matches!(r, Route::CommandOutput(open) if *open == id))
        .count();
    assert_eq!(count, 1);
}

#[test]
fn screen_limit_allows_restoring_existing_screens_and_dialogs_cannot_be_suspended() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app_with_loaded_panels(root.path().into());
    app.apply(AppCommand::OpenInputDialog).unwrap();
    let depth = app.route_depth();
    app.apply(AppCommand::OpenScreenList).unwrap();
    assert_eq!(app.route_depth(), depth);
    app.apply(AppCommand::DialogCancel).unwrap();
    for _ in 0..16 {
        app.apply(AppCommand::OpenHelp).unwrap();
        choose(&mut app, "File manager");
    }
    app.apply(AppCommand::OpenHelp).unwrap();
    choose(&mut app, "File manager");
    assert!(matches!(app.top_route(), Route::Help(_)));
    assert!(app.status_line.contains("limit"));
    app.select_screen(crate::screens::ScreenTarget::Suspended(0));
    assert_eq!(app.suspended_screens.len(), 16);
    app.apply(AppCommand::CloseHelp).unwrap();
    choose(&mut app, "Help");
    assert_eq!(app.suspended_screens.len(), 15);
}

#[test]
fn screen_list_shortcut_dispatches_from_every_supported_screen() {
    let map = keymap::Keymap::bundled_mc_default().unwrap();
    let chord = keymap::KeyChord {
        code: keymap::KeyCode::Char('`'),
        modifiers: keymap::KeyModifiers {
            alt: true,
            ..Default::default()
        },
    };
    for context in [
        KeyContext::FileManager,
        KeyContext::Viewer,
        KeyContext::ViewerHex,
        KeyContext::Jobs,
        KeyContext::FindResults,
        KeyContext::Tree,
        KeyContext::Hotlist,
        KeyContext::Help,
    ] {
        assert_eq!(
            map.resolve(context, chord)
                .and_then(|c| AppCommand::from_key_command(context, c)),
            Some(AppCommand::OpenScreenList),
            "{context:?}"
        );
    }
}
