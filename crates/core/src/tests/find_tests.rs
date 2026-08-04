use super::*;

#[test]
fn find_entries_chunk_updates_results_through_background_handler() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-find-chunk-handler-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenFindDialog)
        .expect("find dialog should open");
    for ch in "needle".chars() {
        app.apply(AppCommand::DialogInputChar(ch))
            .expect("typing find query should succeed");
    }
    app.apply(AppCommand::DialogAccept)
        .expect("find dialog should submit");

    let find_job = app.jobs.last_job().expect("find job should be recorded");
    let entry = FindResultEntry {
        path: root.join("needle.txt"),
        is_dir: false,
    };
    app.handle_background_event(BackgroundEvent::FindEntriesChunk {
        job_id: find_job.id,
        entries: vec![entry.clone()],
    });

    let Route::FindResults(results) = app.top_route() else {
        panic!("top route should be find results");
    };
    assert_eq!(results.entries, vec![entry]);
    assert_eq!(results.cursor, 0);
    assert_eq!(app.status_line, "Finding 'needle': 1 result(s)...");

    fs::remove_dir_all(&root).expect("must remove temp root");
}

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
    let mut spec = FindSpec::new(root.clone());
    spec.filename_pattern = String::from("*.jpg");
    let find_job_id = app.enqueue_worker_job_request(JobRequest::Find {
        spec,
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
    let mut spec = FindSpec::new(root.clone());
    spec.filename_pattern = String::from("*.jpg");
    let result = stream_find_entries(&spec, 32, &cancel_flag, &pause_flag, 1, |entries| {
        chunks.push(entries);
        true
    });
    let report = result.expect("glob search should succeed");
    assert_eq!(report.matched_entries, 2);
    assert!(!report.truncated);
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
    let mut spec = FindSpec::new(root.clone());
    spec.filename_pattern = String::from("*.jpg");
    let result = stream_find_entries(&spec, 32, &cancel_flag, &pause_flag, 1, |_entries| {
        chunks_seen = chunks_seen.saturating_add(1);
        cancel_flag.store(true, AtomicOrdering::Relaxed);
        true
    });
    assert_eq!(result, Err(FindSearchError::Canceled));
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
    let mut spec = FindSpec::new(root.clone());
    spec.filename_pattern = String::from("*.jpg");
    let result = stream_find_entries(
        &spec,
        32,
        &cancel_flag,
        pause_flag.as_ref(),
        1,
        |_entries| true,
    );
    let elapsed = started.elapsed();
    resumer.join().expect("resume thread should complete");

    assert_eq!(
        result.expect("paused search should resume").matched_entries,
        1
    );
    assert!(
        elapsed >= Duration::from_millis(25),
        "search should wait for resume while paused"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn find_engine_supports_regex_and_case_modes() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-find-regex-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let lowercase = root.join("report-42.rs");
    let uppercase = root.join("REPORT-7.RS");
    let unrelated = root.join("report.rs.bak");
    fs::write(&lowercase, "lowercase").expect("must create lowercase match");
    fs::write(&uppercase, "uppercase").expect("must create uppercase match");
    fs::write(&unrelated, "unrelated").expect("must create unrelated file");

    let cancel = AtomicBool::new(false);
    let pause = AtomicBool::new(false);
    let mut spec = FindSpec::new(root.clone());
    spec.filename_pattern = String::from(r"^report-[0-9]+[.]rs$");
    spec.name_mode = FindNameMode::Regex;

    let mut insensitive_matches = Vec::new();
    let report = run_find_entries(&spec, 16, &cancel, &pause, |entries| {
        insensitive_matches.extend(entries);
        true
    })
    .expect("case-insensitive regex search should succeed");
    assert_eq!(report.matched_entries, 2);
    assert_eq!(
        insensitive_matches
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>(),
        vec![uppercase.clone(), lowercase.clone()],
        "results should use deterministic filename order"
    );

    spec.case_sensitive = true;
    let mut sensitive_matches = Vec::new();
    let report = run_find_entries(&spec, 16, &cancel, &pause, |entries| {
        sensitive_matches.extend(entries);
        true
    })
    .expect("case-sensitive regex search should succeed");
    assert_eq!(report.matched_entries, 1);
    assert_eq!(sensitive_matches[0].path, lowercase);

    spec.filename_pattern = String::from("[");
    assert!(matches!(
        run_find_entries(&spec, 16, &cancel, &pause, |_| true),
        Err(FindSearchError::InvalidPattern {
            field: "filename",
            ..
        })
    ));

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn empty_filename_pattern_matches_all_nonignored_entries() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-find-empty-pattern-{stamp}"));
    let nested = root.join("nested");
    let ignored = root.join(".cache");
    fs::create_dir_all(&nested).expect("must create nested directory");
    fs::create_dir_all(&ignored).expect("must create ignored directory");
    let top_file = root.join("top.txt");
    let nested_file = nested.join("child.bin");
    let ignored_file = ignored.join("hidden.txt");
    fs::write(&top_file, "top").expect("must create top-level file");
    fs::write(&nested_file, "nested").expect("must create nested file");
    fs::write(&ignored_file, "ignored").expect("must create ignored file");

    let cancel = AtomicBool::new(false);
    let pause = AtomicBool::new(false);
    let mut spec = FindSpec::new(root.clone());
    spec.ignored_directories.push(String::from(".cache"));
    let mut matches = Vec::new();
    let report = run_find_entries(&spec, 16, &cancel, &pause, |entries| {
        matches.extend(entries);
        true
    })
    .expect("empty filename pattern should be valid");

    let paths = matches
        .into_iter()
        .map(|entry| entry.path)
        .collect::<Vec<_>>();
    assert_eq!(report.matched_entries, 3);
    assert_eq!(report.ignored_directories, 1);
    assert!(paths.contains(&top_file));
    assert!(paths.contains(&nested));
    assert!(paths.contains(&nested_file));
    assert!(!paths.contains(&ignored));
    assert!(!paths.contains(&ignored_file));

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn find_content_search_streams_files_and_honors_whole_words() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-find-content-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let exact = root.join("exact.txt");
    let embedded = root.join("embedded.txt");
    let punctuation = root.join("punctuation.txt");
    let boundary = root.join("stream-boundary.txt");
    fs::write(&exact, "A NEEDLE appears here").expect("must create exact content file");
    fs::write(&embedded, "needlessly close").expect("must create embedded content file");
    fs::write(&punctuation, "C++ is present").expect("must create punctuation content file");
    let mut boundary_content = vec![b'x'; 64 * 1024 - 3];
    boundary_content.extend_from_slice(b" needle after boundary");
    fs::write(&boundary, boundary_content).expect("must create boundary content file");

    let cancel = AtomicBool::new(false);
    let pause = AtomicBool::new(false);
    let mut spec = FindSpec::new(root.clone());
    spec.filename_pattern = String::from("*.txt");
    spec.content_pattern = Some(String::from("needle"));
    spec.whole_word = true;
    let mut matches = Vec::new();
    let report = run_find_entries(&spec, 16, &cancel, &pause, |entries| {
        matches.extend(entries);
        true
    })
    .expect("content search should succeed");

    let paths = matches
        .into_iter()
        .map(|entry| entry.path)
        .collect::<Vec<_>>();
    assert_eq!(report.matched_entries, 2);
    assert!(paths.contains(&exact));
    assert!(paths.contains(&boundary));
    assert!(!paths.contains(&embedded));

    spec.content_pattern = Some(String::from("C++"));
    let mut punctuation_matches = Vec::new();
    let punctuation_report = run_find_entries(&spec, 16, &cancel, &pause, |entries| {
        punctuation_matches.extend(entries);
        true
    })
    .expect("whole-word search should support punctuation at pattern edges");
    assert_eq!(punctuation_report.matched_entries, 1);
    assert_eq!(punctuation_matches[0].path, punctuation);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn find_report_distinguishes_exact_limit_from_truncation() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-find-truncation-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    fs::write(root.join("a.txt"), "a").expect("must create first file");
    fs::write(root.join("b.txt"), "b").expect("must create second file");

    let cancel = AtomicBool::new(false);
    let pause = AtomicBool::new(false);
    let mut spec = FindSpec::new(root.clone());
    spec.filename_pattern = String::from("*.txt");
    let exact_report = run_find_entries(&spec, 2, &cancel, &pause, |_| true)
        .expect("exact-limit search should succeed");
    assert_eq!(exact_report.matched_entries, 2);
    assert!(!exact_report.truncated);

    fs::write(root.join("c.txt"), "c").expect("must create overflow file");
    let mut emitted = Vec::new();
    let truncated_report = run_find_entries(&spec, 2, &cancel, &pause, |entries| {
        emitted.extend(entries);
        true
    })
    .expect("truncated search should succeed");
    assert_eq!(truncated_report.matched_entries, 2);
    assert!(truncated_report.truncated);
    assert_eq!(emitted.len(), 2);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn find_report_retains_subdirectory_read_errors_as_partial_results() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-find-partial-{stamp}"));
    let disappearing = root.join("0-disappearing");
    fs::create_dir_all(&disappearing).expect("must create disappearing directory");
    fs::write(root.join("a-result.txt"), "result").expect("must create result file");
    fs::write(disappearing.join("lost.txt"), "lost").expect("must create disappearing file");

    let cancel = AtomicBool::new(false);
    let pause = AtomicBool::new(false);
    let mut spec = FindSpec::new(root.clone());
    spec.filename_pattern = String::from("*.txt");
    let report = stream_find_entries(&spec, 16, &cancel, &pause, 1, |_| {
        fs::remove_dir_all(&disappearing).expect("callback should remove queued directory");
        true
    })
    .expect("search should preserve partial results");

    assert_eq!(report.matched_entries, 1);
    assert_eq!(report.skipped_directories, 1);
    assert_eq!(report.issue_count, 1);
    assert!(report.is_partial());
    assert_eq!(report.issues[0].kind, FindSearchIssueKind::ReadDirectory);
    assert_eq!(report.issues[0].path, disappearing);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn find_fails_when_starting_directory_cannot_be_read() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let missing = env::temp_dir().join(format!("rc-find-missing-root-{stamp}"));
    let cancel = AtomicBool::new(false);
    let pause = AtomicBool::new(false);
    let spec = FindSpec::new(missing.clone());

    assert!(matches!(
        run_find_entries(&spec, 16, &cancel, &pause, |_| true),
        Err(FindSearchError::StartDirectory { path, .. }) if path == missing
    ));
}

#[cfg(unix)]
#[test]
fn find_reports_permission_denied_subdirectories_without_losing_results() {
    use std::os::unix::fs::PermissionsExt;

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-find-permission-{stamp}"));
    let denied = root.join("denied");
    fs::create_dir_all(&denied).expect("must create denied directory");
    fs::write(root.join("visible.txt"), "visible").expect("must create visible result");
    fs::write(denied.join("hidden.txt"), "hidden").expect("must create hidden file");

    let original_permissions = fs::metadata(&denied)
        .expect("denied directory metadata should exist")
        .permissions();
    let mut denied_permissions = original_permissions.clone();
    denied_permissions.set_mode(0o000);
    fs::set_permissions(&denied, denied_permissions).expect("must remove directory permissions");

    if fs::read_dir(&denied).is_ok() {
        fs::set_permissions(&denied, original_permissions)
            .expect("must restore directory permissions");
        fs::remove_dir_all(&root).expect("must remove temp root");
        return;
    }

    let cancel = AtomicBool::new(false);
    let pause = AtomicBool::new(false);
    let mut spec = FindSpec::new(root.clone());
    spec.filename_pattern = String::from("*.txt");
    let mut matches = Vec::new();
    let result = run_find_entries(&spec, 16, &cancel, &pause, |entries| {
        matches.extend(entries);
        true
    });

    fs::set_permissions(&denied, original_permissions).expect("must restore directory permissions");
    let report = result.expect("permission failure should produce partial results");
    assert_eq!(report.matched_entries, 1);
    assert_eq!(matches[0].path, root.join("visible.txt"));
    assert_eq!(report.skipped_directories, 1);
    assert!(report.is_partial());
    assert!(report.issues.iter().any(|issue| {
        issue
            .message
            .to_ascii_lowercase()
            .contains("permission denied")
    }));

    fs::remove_dir_all(&root).expect("must remove temp root");
}
