use super::*;

#[test]
fn reread_coalesces_previous_refresh_for_same_panel() {
    use std::sync::atomic::Ordering as AtomicOrdering;

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-reread-cancel-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    fs::write(root.join("a.txt"), "a").expect("must create file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.refresh_active_panel();
    assert_eq!(app.pending_worker_commands.len(), 1);

    let (first_job_id, first_request_id, first_cancel_flag) = match &app.pending_worker_commands[0]
    {
        WorkerCommand::Run(job) => match &job.request {
            JobRequest::RefreshPanel { request_id, .. } => (job.id, *request_id, job.cancel_flag()),
            _ => panic!("expected refresh-panel job request"),
        },
        _ => panic!("expected worker run command"),
    };
    assert!(
        !first_cancel_flag.load(AtomicOrdering::Relaxed),
        "initial refresh should not be canceled"
    );

    app.refresh_active_panel();
    assert!(
        !first_cancel_flag.load(AtomicOrdering::Relaxed),
        "coalesced refresh should keep the existing queued request active"
    );
    assert!(
        !app.pending_worker_commands.iter().any(
            |command| matches!(command, WorkerCommand::Cancel(job_id) if *job_id == first_job_id)
        ),
        "coalesced refresh should not enqueue an explicit cancellation"
    );

    let (coalesced_job_id, second_request_id, second_cancel_flag) = app
        .pending_worker_commands
        .iter()
        .rev()
        .find_map(|command| {
            let WorkerCommand::Run(job) = command else {
                return None;
            };
            let JobRequest::RefreshPanel { request_id, .. } = &job.request else {
                return None;
            };
            Some((job.id, *request_id, job.cancel_flag()))
        })
        .expect("second refresh command should be queued");
    assert_eq!(
        coalesced_job_id, first_job_id,
        "coalescing should reuse the existing queued refresh job id"
    );
    assert!(
        second_request_id > first_request_id,
        "request ids should advance when a refresh request supersedes the queued one"
    );
    assert!(
        !second_cancel_flag.load(AtomicOrdering::Relaxed),
        "coalesced refresh should remain active"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn stale_panel_refresh_event_is_ignored() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-refresh-stale-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.refresh_active_panel();
    app.refresh_active_panel();
    let commands = app.take_pending_worker_commands();
    let refresh_requests: Vec<_> = commands
        .into_iter()
        .filter_map(|command| {
            let WorkerCommand::Run(job) = command else {
                return None;
            };
            match job.request {
                JobRequest::RefreshPanel {
                    panel,
                    cwd,
                    source,
                    sort_mode,
                    request_id,
                    ..
                } => Some((panel, cwd, source, sort_mode, request_id)),
                _ => None,
            }
        })
        .collect();
    assert_eq!(
        refresh_requests.len(),
        1,
        "superseded refreshes should coalesce while still queued"
    );

    let (panel, cwd, source, sort_mode, latest_request_id) = refresh_requests[0].clone();
    let stale_request_id = latest_request_id.saturating_sub(1);
    assert!(
        stale_request_id < latest_request_id,
        "stale request id should be older than the latest one"
    );

    app.handle_background_event(BackgroundEvent::PanelRefreshed {
        panel,
        cwd: cwd.clone(),
        source: source.clone(),
        sort_mode,
        request_id: stale_request_id,
        result: Ok(Vec::new()),
    });
    assert!(
        app.panels[panel.index()].loading,
        "stale refresh result should not clear loading state"
    );

    app.handle_background_event(BackgroundEvent::PanelRefreshed {
        panel,
        cwd,
        source,
        sort_mode,
        request_id: latest_request_id,
        result: Ok(Vec::new()),
    });
    assert!(
        !app.panels[panel.index()].loading,
        "latest refresh result should clear loading state"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn panel_refresh_chunks_preserve_existing_tags_until_final_result() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-refresh-chunk-tags-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let alpha_path = root.join("alpha.txt");
    let beta_path = root.join("beta.txt");
    fs::write(&alpha_path, "alpha").expect("alpha fixture should be writable");
    fs::write(&beta_path, "beta").expect("beta fixture should be writable");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let alpha_index = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == alpha_path)
        .expect("alpha entry should be visible");
    app.active_panel_mut().cursor = alpha_index;
    app.apply(AppCommand::ToggleTag)
        .expect("toggle tag should succeed");
    assert!(
        app.active_panel().is_tagged(&alpha_path),
        "precondition: alpha entry should start tagged"
    );

    app.refresh_active_panel();
    let (panel, cwd, source, sort_mode, request_id) = app
        .take_pending_worker_commands()
        .into_iter()
        .find_map(|command| {
            let WorkerCommand::Run(job) = command else {
                return None;
            };
            let JobRequest::RefreshPanel {
                panel,
                cwd,
                source,
                sort_mode,
                request_id,
                ..
            } = job.request
            else {
                return None;
            };
            Some((panel, cwd, source, sort_mode, request_id))
        })
        .expect("refresh command should be queued");

    app.handle_background_event(BackgroundEvent::PanelEntriesChunk {
        panel,
        cwd: cwd.clone(),
        source: source.clone(),
        sort_mode,
        request_id,
        entries: vec![FileEntry::file(
            String::from("beta.txt"),
            beta_path.clone(),
            4,
            None,
        )],
    });
    assert!(
        app.active_panel().is_tagged(&alpha_path),
        "chunk updates should not prune existing tags before final listing"
    );

    let final_entries =
        read_entries_with_visibility(&cwd, sort_mode, true).expect("listing should build");
    app.handle_background_event(BackgroundEvent::PanelRefreshed {
        panel,
        cwd,
        source,
        sort_mode,
        request_id,
        result: Ok(final_entries),
    });
    assert!(
        app.active_panel().is_tagged(&alpha_path),
        "tag should survive final listing when target is still present"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn panel_refresh_chunks_preserve_cursor_until_final_listing() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-refresh-chunk-cursor-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    for name in ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt", "f.txt"] {
        fs::write(root.join(name), name).expect("fixture should be writable");
    }

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let target_path = root.join("f.txt");
    let target_cursor = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == target_path)
        .expect("target entry should be visible");
    app.active_panel_mut().cursor = target_cursor;

    app.refresh_active_panel();
    let (panel, cwd, source, sort_mode, request_id) = app
        .take_pending_worker_commands()
        .into_iter()
        .find_map(|command| {
            let WorkerCommand::Run(job) = command else {
                return None;
            };
            let JobRequest::RefreshPanel {
                panel,
                cwd,
                source,
                sort_mode,
                request_id,
                ..
            } = job.request
            else {
                return None;
            };
            Some((panel, cwd, source, sort_mode, request_id))
        })
        .expect("refresh command should be queued");

    app.handle_background_event(BackgroundEvent::PanelEntriesChunk {
        panel,
        cwd: cwd.clone(),
        source: source.clone(),
        sort_mode,
        request_id,
        entries: vec![FileEntry::file(
            String::from("a.txt"),
            root.join("a.txt"),
            1,
            None,
        )],
    });
    assert_eq!(
        app.active_panel().cursor,
        target_cursor,
        "chunk updates should not clamp cursor before final listing arrives"
    );

    let final_entries =
        read_entries_with_visibility(&cwd, sort_mode, true).expect("listing should build");
    app.handle_background_event(BackgroundEvent::PanelRefreshed {
        panel,
        cwd,
        source,
        sort_mode,
        request_id,
        result: Ok(final_entries),
    });
    assert_eq!(
        app.active_panel().cursor,
        target_cursor,
        "final listing should preserve cursor when target index still exists"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn refresh_dispatch_failure_clears_loading_state() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-refresh-dispatch-failure-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let panel_index = app.active_panel.index();
    app.refresh_active_panel();
    assert!(
        app.panels[panel_index].loading,
        "refresh should set panel loading before dispatch"
    );

    let refresh_job_id = app
        .take_pending_worker_commands()
        .into_iter()
        .find_map(|command| {
            let WorkerCommand::Run(job) = command else {
                return None;
            };
            matches!(job.request, JobRequest::RefreshPanel { .. }).then_some(job.id)
        })
        .expect("refresh command should be queued");

    app.handle_job_dispatch_failure(refresh_job_id, JobError::dispatch("runtime queue is full"));
    assert!(
        !app.panels[panel_index].loading,
        "failed refresh dispatch should clear loading state"
    );
    assert_eq!(
        app.panel_refresh_job_ids[panel_index], None,
        "failed refresh dispatch should clear tracked refresh job id"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn refresh_cancel_before_start_clears_loading_state() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-refresh-cancel-before-start-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let panel_index = app.active_panel.index();
    app.refresh_active_panel();
    assert!(
        app.panels[panel_index].loading,
        "refresh should set panel loading before dispatch"
    );

    let refresh_job_id = app
        .take_pending_worker_commands()
        .into_iter()
        .find_map(|command| {
            let WorkerCommand::Run(job) = command else {
                return None;
            };
            matches!(job.request, JobRequest::RefreshPanel { .. }).then_some(job.id)
        })
        .expect("refresh command should be queued");

    app.handle_job_event(JobEvent::Finished {
        id: refresh_job_id,
        result: Err(JobError::canceled()),
    });
    assert!(
        !app.panels[panel_index].loading,
        "canceled refresh without background event should clear loading state"
    );
    assert_eq!(
        app.panel_refresh_job_ids[panel_index], None,
        "canceled refresh should clear tracked refresh job id"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}
