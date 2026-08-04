use super::*;

fn select_path(app: &mut AppState, path: &Path) {
    app.active_panel_mut().cursor = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == path)
        .expect("fixture path should be listed");
}

#[test]
fn tagged_directory_total_is_measured_in_the_background() {
    let root = env::temp_dir().join(format!(
        "rc-selection-size-flow-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos()
    ));
    let selected = root.join("selected");
    fs::create_dir_all(selected.join("nested")).expect("nested directory should be creatable");
    fs::write(selected.join("alpha"), vec![0_u8; 11]).expect("first file should be writable");
    fs::write(selected.join("nested/beta"), vec![0_u8; 23])
        .expect("nested file should be writable");

    let mut app = app_with_loaded_panels(root.clone());
    select_path(&mut app, &selected);
    app.apply(AppCommand::ToggleTag)
        .expect("directory should be taggable");

    assert_eq!(
        app.selection_size_state(ActivePanel::Left),
        &SelectionSizeState::Calculating { selected_items: 1 }
    );
    assert!(app.pending_worker_commands.iter().any(|command| matches!(
        command,
        WorkerCommand::Run(job)
            if matches!(
                &job.request,
                JobRequest::MeasureSelection { panel: ActivePanel::Left, paths, .. }
                    if paths.as_slice() == std::slice::from_ref(&selected)
            )
    )));

    drain_background(&mut app);

    assert_eq!(
        app.selection_size_state(ActivePanel::Left),
        &SelectionSizeState::Ready {
            selected_items: 1,
            apparent_bytes: 34,
            unreadable_entries: 0,
        }
    );
    fs::remove_dir_all(root).expect("temporary root should be removable");
}

#[test]
fn changing_tags_coalesces_a_queued_measurement_and_rejects_stale_results() {
    let root = env::temp_dir().join(format!(
        "rc-selection-size-stale-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("temporary root should be creatable");
    let alpha = root.join("alpha");
    let beta = root.join("beta");
    fs::write(&alpha, vec![0_u8; 3]).expect("first file should be writable");
    fs::write(&beta, vec![0_u8; 5]).expect("second file should be writable");

    let mut app = app_with_loaded_panels(root.clone());
    select_path(&mut app, &alpha);
    app.apply(AppCommand::ToggleTag)
        .expect("first file should be taggable");
    let (job_id, stale_request_id) = app
        .pending_worker_commands
        .iter()
        .find_map(|command| {
            let WorkerCommand::Run(job) = command else {
                return None;
            };
            let JobRequest::MeasureSelection { request_id, .. } = &job.request else {
                return None;
            };
            Some((job.id, *request_id))
        })
        .expect("first measurement should be queued");

    select_path(&mut app, &beta);
    app.apply(AppCommand::ToggleTag)
        .expect("second file should be taggable");
    let (coalesced_job_id, current_request_id) = app
        .pending_worker_commands
        .iter()
        .find_map(|command| {
            let WorkerCommand::Run(job) = command else {
                return None;
            };
            let JobRequest::MeasureSelection { request_id, .. } = &job.request else {
                return None;
            };
            Some((job.id, *request_id))
        })
        .expect("updated measurement should remain queued");

    assert_eq!(coalesced_job_id, job_id);
    assert!(current_request_id > stale_request_id);
    assert_eq!(
        app.selection_size_state(ActivePanel::Left),
        &SelectionSizeState::Calculating { selected_items: 2 }
    );

    app.handle_background_event(BackgroundEvent::SelectionSizeMeasured {
        panel: ActivePanel::Left,
        request_id: stale_request_id,
        report: SelectionSizeReport {
            apparent_bytes: 999,
            unreadable_entries: 0,
        },
    });
    assert_eq!(
        app.selection_size_state(ActivePanel::Left),
        &SelectionSizeState::Calculating { selected_items: 2 },
        "a superseded result must not replace the current calculation"
    );

    drain_background(&mut app);
    assert_eq!(
        app.selection_size_state(ActivePanel::Left),
        &SelectionSizeState::Ready {
            selected_items: 2,
            apparent_bytes: 8,
            unreadable_entries: 0,
        }
    );
    fs::remove_dir_all(root).expect("temporary root should be removable");
}

#[test]
fn clearing_the_selection_cancels_and_hides_the_pending_total() {
    let root = env::temp_dir().join(format!(
        "rc-selection-size-clear-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("temporary root should be creatable");
    let selected = root.join("selected");
    fs::write(&selected, "payload").expect("file should be writable");

    let mut app = app_with_loaded_panels(root.clone());
    select_path(&mut app, &selected);
    app.apply(AppCommand::ToggleTag)
        .expect("file should be taggable");
    let job_id = app
        .pending_worker_commands
        .iter()
        .find_map(|command| match command {
            WorkerCommand::Run(job)
                if matches!(job.request, JobRequest::MeasureSelection { .. }) =>
            {
                Some(job.id)
            }
            _ => None,
        })
        .expect("measurement should be queued");

    select_path(&mut app, &selected);
    app.apply(AppCommand::ToggleTag)
        .expect("file should be untaggable");

    assert_eq!(
        app.selection_size_state(ActivePanel::Left),
        &SelectionSizeState::Empty
    );
    assert!(
        app.pending_worker_commands
            .iter()
            .any(|command| matches!(command, WorkerCommand::Cancel(id) if *id == job_id))
    );

    drain_background(&mut app);
    assert_eq!(
        app.selection_size_state(ActivePanel::Left),
        &SelectionSizeState::Empty
    );
    fs::remove_dir_all(root).expect("temporary root should be removable");
}

#[test]
fn hidden_panel_totals_do_not_schedule_filesystem_traversal() {
    let root = env::temp_dir().join(format!(
        "rc-selection-size-disabled-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("temporary root should be creatable");
    let selected = root.join("selected");
    fs::write(&selected, "payload").expect("file should be writable");

    let mut app = app_with_loaded_panels(root.clone());
    app.settings_mut().layout.show_panel_totals = false;
    select_path(&mut app, &selected);
    app.apply(AppCommand::ToggleTag)
        .expect("file should remain taggable when totals are hidden");

    assert_eq!(
        app.selection_size_state(ActivePanel::Left),
        &SelectionSizeState::Empty
    );
    assert!(!app.pending_worker_commands.iter().any(|command| matches!(
        command,
        WorkerCommand::Run(job)
            if matches!(job.request, JobRequest::MeasureSelection { .. })
    )));
    fs::remove_dir_all(root).expect("temporary root should be removable");
}
