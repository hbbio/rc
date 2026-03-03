use super::*;

#[test]
fn find_dialog_locates_selected_entry_in_panel_and_supports_resume() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-find-results-{stamp}"));
    let nested = root.join("nested");
    fs::create_dir_all(&nested).expect("must create temp tree");
    let target = nested.join("needle.txt");
    fs::write(&target, "needle").expect("must create target file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenFindDialog)
        .expect("find dialog should open");
    for ch in "needle".chars() {
        app.apply(AppCommand::DialogInputChar(ch))
            .expect("typing find query should succeed");
    }
    app.apply(AppCommand::DialogAccept)
        .expect("find dialog should submit");
    drain_background(&mut app);
    assert_eq!(app.key_context(), KeyContext::FindResults);
    let find_job = app.jobs.last_job().expect("find job should be recorded");
    assert_eq!(find_job.kind, JobKind::Find);
    assert_eq!(find_job.status, JobStatus::Succeeded);

    let target_index = match app.top_route() {
        Route::FindResults(results) => results
            .entries
            .iter()
            .position(|entry| entry.path == target)
            .expect("target should be present in find results"),
        _ => panic!("top route should be find results"),
    };
    let Some(Route::FindResults(results)) = app.routes.last_mut() else {
        panic!("top route should be find results");
    };
    results.cursor = target_index;

    app.apply(AppCommand::FindResultsOpenEntry)
        .expect("opening find result should succeed");
    drain_background(&mut app);
    assert_eq!(app.key_context(), KeyContext::FileManager);
    assert_eq!(app.active_panel().cwd, nested);

    let focused_entry = app
        .active_panel()
        .selected_entry()
        .expect("selected panel entry should be present");
    assert_eq!(focused_entry.path, target);

    app.apply(AppCommand::OpenFindDialog)
        .expect("open find should resume results");
    assert_eq!(app.key_context(), KeyContext::FindResults);
    let Route::FindResults(results) = app.top_route() else {
        panic!("top route should be find results");
    };
    assert_eq!(
        results.entries.get(results.cursor).map(|entry| &entry.path),
        Some(&target)
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn find_results_panelize_creates_virtual_panel_and_preserves_resume() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-find-panelize-{stamp}"));
    let nested = root.join("nested");
    fs::create_dir_all(&nested).expect("must create temp tree");
    let target = nested.join("needle.txt");
    fs::write(&target, "needle").expect("must create target file");
    fs::write(root.join("other.log"), "other").expect("must create non-matching file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenFindDialog)
        .expect("find dialog should open");
    for ch in "needle".chars() {
        app.apply(AppCommand::DialogInputChar(ch))
            .expect("typing find query should succeed");
    }
    app.apply(AppCommand::DialogAccept)
        .expect("find dialog should submit");
    drain_background(&mut app);
    assert_eq!(app.key_context(), KeyContext::FindResults);

    app.apply(AppCommand::FindResultsPanelize)
        .expect("panelizing find results should succeed");
    drain_background(&mut app);
    assert_eq!(app.key_context(), KeyContext::FileManager);
    assert!(matches!(
        app.active_panel().source,
        PanelListingSource::FindResults { .. }
    ));
    assert!(
        app.active_panel()
            .entries
            .iter()
            .any(|entry| entry.path == target),
        "panelized find results should include matching files"
    );
    assert_eq!(app.active_panel().cwd, root);

    app.apply(AppCommand::CdUp)
        .expect("CdUp should leave panelize mode");
    drain_background(&mut app);
    assert!(matches!(
        app.active_panel().source,
        PanelListingSource::Directory
    ));
    assert_eq!(
        app.active_panel().cwd,
        root,
        "leaving panelize mode should keep current directory unchanged"
    );

    app.apply(AppCommand::OpenFindDialog)
        .expect("find dialog should resume previous results");
    assert_eq!(app.key_context(), KeyContext::FindResults);
    let Route::FindResults(results) = app.top_route() else {
        panic!("top route should be find results");
    };
    assert!(
        results.entries.iter().any(|entry| entry.path == target),
        "resumed find results should still include prior matches"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn find_cancel_routes_through_worker_cancel_command() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-find-cancel-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    fs::write(root.join("a.jpg"), "a").expect("must create file");
    fs::write(root.join("b.jpg"), "b").expect("must create file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenFindDialog)
        .expect("find dialog should open");
    for ch in "*.jpg".chars() {
        app.apply(AppCommand::DialogInputChar(ch))
            .expect("typing find query should succeed");
    }
    app.apply(AppCommand::DialogAccept)
        .expect("find dialog should submit");
    let queued_counts = app.jobs_status_counts();
    assert_eq!(queued_counts.queued, 1, "find should enqueue a worker job");

    app.apply(AppCommand::CancelJob)
        .expect("cancel job should succeed");
    let commands = app.take_pending_worker_commands();
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, WorkerCommand::Cancel(_))),
        "canceling find should enqueue worker cancel command"
    );
    for command in commands {
        if let WorkerCommand::Run(job) = command {
            app.pending_worker_commands.push(WorkerCommand::Run(job));
        }
    }

    drain_background(&mut app);
    let find_job = app.jobs.last_job().expect("find job should be present");
    assert_eq!(find_job.kind, JobKind::Find);
    assert_eq!(find_job.status, JobStatus::Canceled);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn quit_requests_cancellation_for_pending_find_job() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-find-quit-cancel-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    fs::write(root.join("a.jpg"), "a").expect("must create file");
    fs::write(root.join("b.jpg"), "b").expect("must create file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenFindDialog)
        .expect("find dialog should open");
    for ch in "*.jpg".chars() {
        app.apply(AppCommand::DialogInputChar(ch))
            .expect("typing find query should succeed");
    }
    app.apply(AppCommand::DialogAccept)
        .expect("find dialog should submit");

    assert_eq!(
        app.apply(AppCommand::Quit).expect("quit should succeed"),
        ApplyResult::Quit
    );

    drain_background(&mut app);
    let find_job = app.jobs.last_job().expect("find job should be present");
    assert_eq!(find_job.kind, JobKind::Find);
    assert_eq!(find_job.status, JobStatus::Canceled);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn quit_cancels_find_but_keeps_persist_settings_job() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-quit-keep-persist-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let settings_paths = settings_io::SettingsPaths {
        mc_ini_path: Some(root.join("mc.ini")),
        rc_ini_path: Some(root.join("settings.ini")),
    };
    let persist_job_id = app.enqueue_worker_job_request(JobRequest::PersistSettings {
        paths: settings_paths,
        snapshot: Box::new(app.persisted_settings_snapshot()),
    });
    let find_job_id = app.enqueue_worker_job_request(JobRequest::Find {
        query: String::from("*.jpg"),
        base_dir: root.clone(),
        max_results: 64,
    });

    assert_eq!(
        app.apply(AppCommand::Quit).expect("quit should succeed"),
        ApplyResult::Quit
    );

    let pending_commands = app.take_pending_worker_commands();
    assert!(
        pending_commands.iter().any(|command| matches!(
            command,
            WorkerCommand::Cancel(job_id) if *job_id == find_job_id
        )),
        "quit should request cancellation for find jobs"
    );
    assert!(
        !pending_commands.iter().any(|command| matches!(
            command,
            WorkerCommand::Cancel(job_id) if *job_id == persist_job_id
        )),
        "quit should not request cancellation for persist-settings jobs"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn stream_find_entries_supports_glob_patterns_and_chunking() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-find-glob-{stamp}"));
    let nested = root.join("nested");
    fs::create_dir_all(&nested).expect("must create temp tree");
    let jpg_a = root.join("a.jpg");
    let jpg_b = nested.join("b.JPG");
    let png = root.join("c.png");
    fs::write(&jpg_a, "a").expect("must create jpg");
    fs::write(&jpg_b, "b").expect("must create jpg");
    fs::write(&png, "c").expect("must create png");

    let cancel_flag = AtomicBool::new(false);
    let pause_flag = AtomicBool::new(false);
    let mut chunks = Vec::new();
    let result = stream_find_entries(
        &root,
        "*.jpg",
        32,
        &cancel_flag,
        &pause_flag,
        1,
        |entries| {
            chunks.push(entries);
            true
        },
    );
    assert_eq!(result, Ok(()));
    assert!(
        chunks.len() >= 2,
        "chunk size 1 should emit multiple chunks for two matches"
    );

    let flattened: Vec<PathBuf> = chunks
        .iter()
        .flat_map(|chunk| chunk.iter().map(|entry| entry.path.clone()))
        .collect();
    assert!(
        flattened.contains(&jpg_a),
        "glob should match top-level jpg"
    );
    assert!(
        flattened.contains(&jpg_b),
        "glob should match nested uppercase extension"
    );
    assert!(
        !flattened.contains(&png),
        "glob should not match non-jpg file"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn stream_find_entries_stops_after_cancel_request() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-find-cancel-flag-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    fs::write(root.join("a.jpg"), "a").expect("must create file");
    fs::write(root.join("b.jpg"), "b").expect("must create file");
    fs::write(root.join("c.jpg"), "c").expect("must create file");

    let cancel_flag = AtomicBool::new(false);
    let pause_flag = AtomicBool::new(false);
    let mut chunks_seen = 0usize;
    let result = stream_find_entries(
        &root,
        "*.jpg",
        32,
        &cancel_flag,
        &pause_flag,
        1,
        |_entries| {
            chunks_seen = chunks_seen.saturating_add(1);
            cancel_flag.store(true, AtomicOrdering::Relaxed);
            true
        },
    );
    assert_eq!(result, Err(String::from(JOB_CANCELED_MESSAGE)));
    assert_eq!(chunks_seen, 1, "search should stop shortly after cancel");

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn stream_find_entries_waits_while_paused_and_resumes() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-find-paused-resume-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    fs::write(root.join("a.jpg"), "a").expect("must create file");

    let cancel_flag = AtomicBool::new(false);
    let pause_flag = Arc::new(AtomicBool::new(true));
    let pause_flag_for_thread = Arc::clone(&pause_flag);
    let resumer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(40));
        pause_flag_for_thread.store(false, AtomicOrdering::Relaxed);
    });

    let started = std::time::Instant::now();
    let result = stream_find_entries(
        &root,
        "*.jpg",
        32,
        &cancel_flag,
        pause_flag.as_ref(),
        1,
        |_entries| true,
    );
    let elapsed = started.elapsed();
    resumer.join().expect("resume thread should complete");

    assert_eq!(result, Ok(()));
    assert!(
        elapsed >= Duration::from_millis(25),
        "search should wait for resume while paused"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}
