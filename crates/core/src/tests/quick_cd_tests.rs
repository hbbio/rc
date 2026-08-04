use super::*;

fn quick_cd_dialog(app: &AppState) -> &QuickCdDialogState {
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("quick-cd dialog should be active");
    };
    let DialogKind::QuickCd(quick_cd) = &dialog.kind else {
        panic!("dialog should contain quick-cd search state");
    };
    quick_cd
}

#[test]
fn quick_cd_menu_entry_opens_the_directory_dialog() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-quick-cd-menu-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenMenuAt(1))
        .expect("file menu should open");
    move_menu_selection_to_label(&mut app, "Quick cd");
    app.apply(AppCommand::MenuAccept)
        .expect("quick cd menu action should succeed");

    assert_eq!(app.key_context(), KeyContext::Input);
    assert_eq!(quick_cd_dialog(&app).value, "");
    assert_eq!(
        quick_cd_dialog(&app).search_status,
        QuickCdSearchStatus::Idle
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn quick_cd_reopens_with_the_original_value_after_validation_failure() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-quick-cd-invalid-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenQuickCd)
        .expect("quick cd should open");
    app.finish_dialog(DialogResult::QuickCdSubmitted {
        input: String::from("missing path"),
        selected_path: None,
    });

    assert_eq!(app.key_context(), KeyContext::Input);
    assert_eq!(quick_cd_dialog(&app).value, "missing path");
    assert!(app.status_line.contains("quote paths containing spaces"));
    assert_eq!(app.active_panel().cwd, root);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn quick_cd_queues_refresh_and_dash_toggles_previous_directory() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-quick-cd-toggle-{stamp}"));
    let first = root.join("first");
    let second = root.join("second");
    fs::create_dir_all(&first).expect("must create first directory");
    fs::create_dir_all(&second).expect("must create second directory");

    let mut app = AppState::new(first.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenQuickCd)
        .expect("quick cd should open");
    app.finish_dialog(DialogResult::QuickCdSubmitted {
        input: String::from("../second"),
        selected_path: None,
    });

    assert_eq!(app.active_panel().cwd, second);
    assert_eq!(
        app.previous_panel_directories[ActivePanel::Left.index()],
        Some(first.clone())
    );
    let commands = app.take_pending_worker_commands();
    assert_eq!(commands.len(), 1);
    assert!(matches!(
        &commands[0],
        WorkerCommand::Run(job)
            if matches!(
                &job.request,
                JobRequest::RefreshPanel {
                    panel: ActivePanel::Left,
                    cwd,
                    ..
                } if cwd == &second
            )
    ));

    app.apply(AppCommand::OpenQuickCd)
        .expect("second quick cd should open");
    app.finish_dialog(DialogResult::QuickCdSubmitted {
        input: String::from("-"),
        selected_path: None,
    });
    assert_eq!(app.active_panel().cwd, first);
    assert_eq!(
        app.previous_panel_directories[ActivePanel::Left.index()],
        Some(second.clone())
    );

    app.take_pending_worker_commands();
    app.apply(AppCommand::OpenQuickCd)
        .expect("third quick cd should open");
    app.finish_dialog(DialogResult::QuickCdSubmitted {
        input: String::from("-"),
        selected_path: None,
    });
    assert_eq!(app.active_panel().cwd, second);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn normal_panel_navigation_updates_quick_cd_history_per_panel() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-quick-cd-history-{stamp}"));
    let child = root.join("child");
    fs::create_dir_all(&child).expect("must create child directory");

    let mut app = app_with_loaded_panels(root.clone());
    let child_index = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == child)
        .expect("child should be listed");
    app.active_panel_mut().cursor = child_index;
    assert!(app.open_selected_directory());
    assert_eq!(
        app.previous_panel_directories[ActivePanel::Left.index()],
        Some(root.clone())
    );
    assert_eq!(
        app.previous_panel_directories[ActivePanel::Right.index()],
        None,
        "the passive panel must keep independent history"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn quick_cd_coalesces_queued_searches_as_the_query_changes() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-quick-cd-coalesce-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenQuickCd)
        .expect("quick cd should open");
    app.apply(AppCommand::DialogInputChar('n'))
        .expect("first query character should be accepted");
    app.apply(AppCommand::DialogInputChar('e'))
        .expect("second query character should be accepted");

    assert!(
        app.take_pending_worker_commands().is_empty(),
        "search should remain debounced while the user is typing"
    );
    app.poll_deferred_work_at(
        Instant::now()
            .checked_add(Duration::from_secs(1))
            .expect("test deadline should fit"),
    );
    let commands = app.take_pending_worker_commands();
    assert_eq!(commands.len(), 1, "only the latest query should be queued");
    assert!(matches!(
        &commands[0],
        WorkerCommand::Run(job)
            if matches!(
                &job.request,
                JobRequest::QuickCdSearch { spec, .. } if spec.query == "ne"
            )
    ));
    assert!(matches!(
        quick_cd_dialog(&app).search_status,
        QuickCdSearchStatus::Searching { .. }
    ));

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn quick_cd_ignores_stale_results_and_opens_the_arrow_selected_path() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-quick-cd-select-{stamp}"));
    let first = root.join("alpha");
    let selected = root.join("beta with spaces");
    fs::create_dir_all(&first).expect("first match should be creatable");
    fs::create_dir_all(&selected).expect("selected match should be creatable");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenQuickCd)
        .expect("quick cd should open");
    app.apply(AppCommand::DialogInputChar('a'))
        .expect("query should be accepted");
    app.poll_deferred_work_at(
        Instant::now()
            .checked_add(Duration::from_secs(1))
            .expect("test deadline should fit"),
    );
    let first_commands = app.take_pending_worker_commands();
    let (first_job_id, first_request_id) = first_commands
        .iter()
        .find_map(|command| match command {
            WorkerCommand::Run(job) => match &job.request {
                JobRequest::QuickCdSearch { request_id, .. } => Some((job.id, *request_id)),
                _ => None,
            },
            _ => None,
        })
        .expect("first search should be queued");

    app.apply(AppCommand::DialogInputChar('l'))
        .expect("newer query should be accepted");
    app.poll_deferred_work_at(
        Instant::now()
            .checked_add(Duration::from_secs(1))
            .expect("test deadline should fit"),
    );
    let second_commands = app.take_pending_worker_commands();
    assert!(
        second_commands
            .iter()
            .any(|command| matches!(command, WorkerCommand::Cancel(id) if *id == first_job_id))
    );
    let second_request_id = second_commands
        .iter()
        .find_map(|command| match command {
            WorkerCommand::Run(job) => match &job.request {
                JobRequest::QuickCdSearch { request_id, .. } => Some(*request_id),
                _ => None,
            },
            _ => None,
        })
        .expect("replacement search should be queued");

    app.handle_background_event(BackgroundEvent::QuickCdSearchUpdated {
        request_id: first_request_id,
        snapshot: QuickCdSearchSnapshot {
            suggestions: vec![QuickCdSuggestion {
                path: root.join("stale"),
                display: String::from("./stale"),
            }],
            complete: true,
            ..QuickCdSearchSnapshot::default()
        },
    });
    assert!(quick_cd_dialog(&app).suggestions.is_empty());

    app.handle_background_event(BackgroundEvent::QuickCdSearchUpdated {
        request_id: second_request_id,
        snapshot: QuickCdSearchSnapshot {
            suggestions: vec![
                QuickCdSuggestion {
                    path: first,
                    display: String::from("./alpha"),
                },
                QuickCdSuggestion {
                    path: selected.clone(),
                    display: String::from("./beta with spaces"),
                },
            ],
            visited_directories: 12,
            complete: true,
            ..QuickCdSearchSnapshot::default()
        },
    });
    app.apply(AppCommand::DialogListboxDown)
        .expect("down should select the second suggestion");
    assert_eq!(quick_cd_dialog(&app).selected, 1);
    app.apply(AppCommand::DialogAccept)
        .expect("selected suggestion should be accepted");

    assert_eq!(app.active_panel().cwd, selected);
    assert!(matches!(
        app.last_dialog_result,
        Some(DialogResult::QuickCdSubmitted {
            selected_path: Some(_),
            ..
        })
    ));

    fs::remove_dir_all(&root).expect("must remove temp root");
}
