use super::*;

fn fixture() -> (tempfile::TempDir, AppState) {
    let root = tempfile::tempdir().unwrap();
    let left = root.path().join("left");
    let right = root.path().join("right");
    fs::create_dir(&left).unwrap();
    fs::create_dir(&right).unwrap();
    fs::write(left.join("left.txt"), "left").unwrap();
    fs::write(right.join("right.txt"), "right").unwrap();
    let mut app = app_with_loaded_panels(left);
    app.panels[1].cwd = right;
    app.panels[1].refresh().unwrap();
    for panel in &mut app.panels {
        panel.move_cursor_end();
    }
    (root, app)
}

#[test]
fn swap_preserves_panel_contents_tags_settings_focus_and_history() {
    let (_root, mut app) = fixture();
    app.panels[0].toggle_tag_on_cursor();
    app.set_panel_listing_format(ActivePanel::Left, PanelListingFormat::Brief);
    app.set_panel_sort_mode(
        ActivePanel::Right,
        SortMode {
            field: SortField::Size,
            reverse: true,
        },
    );
    app.previous_panel_directories = [
        Some(PathBuf::from("previous-left")),
        Some(PathBuf::from("previous-right")),
    ];
    let panels = app.panels.clone();
    app.apply(AppCommand::SwapPanels).unwrap();
    assert_eq!(app.active_panel, ActivePanel::Right);
    assert_eq!(app.panels[0], panels[1]);
    assert_eq!(app.panels[1], panels[0]);
    assert_eq!(app.panel_listing_formats[1], PanelListingFormat::Brief);
    assert_eq!(
        app.settings.panel_options.listing_formats,
        app.panel_listing_formats
    );
    assert_eq!(
        app.settings.panel_options.sort_modes[0],
        panels[1].sort_mode
    );
    assert_eq!(
        app.previous_panel_directories[1],
        Some(PathBuf::from("previous-left"))
    );
    app.apply(AppCommand::SwapPanels).unwrap();
    assert_eq!(app.panels, panels);
    assert_eq!(app.active_panel, ActivePanel::Left);
}

#[test]
fn swap_restarts_pending_refresh_and_ignores_old_side_results() {
    let (_root, mut app) = fixture();
    app.queue_panel_refresh(ActivePanel::Left);
    let pending = app.take_pending_worker_commands();
    let old = pending
        .iter()
        .find_map(|command| match command {
            WorkerCommand::Run(job) => match &job.request {
                JobRequest::RefreshPanel {
                    panel,
                    cwd,
                    source,
                    sort_mode,
                    filter,
                    request_id,
                    ..
                } => Some((
                    *panel,
                    cwd.clone(),
                    source.clone(),
                    *sort_mode,
                    filter.clone(),
                    *request_id,
                )),
                _ => None,
            },
            _ => None,
        })
        .unwrap();
    app.restore_pending_worker_commands(pending);
    let left = app.panels[0].cwd.clone();
    let right = app.panels[1].cwd.clone();
    app.apply(AppCommand::SwapPanels).unwrap();
    app.handle_background_event(BackgroundEvent::PanelRefreshed {
        panel: old.0,
        cwd: old.1,
        source: old.2,
        sort_mode: old.3,
        filter: old.4,
        request_id: old.5,
        disk_usage: None,
        result: Ok(panel_refresh_result(vec![file_entry("stale")])),
    });
    assert_eq!(app.panels[0].cwd, right);
    assert!(
        !app.panels[0]
            .entries
            .iter()
            .any(|entry| entry.name == "stale")
    );
    drain_background(&mut app);
    assert_eq!(app.panels[1].cwd, left);
    assert!(
        app.panels[1]
            .entries
            .iter()
            .any(|entry| entry.name == "left.txt")
    );
    assert!(!app.panels[1].loading);
}

#[test]
fn swap_moves_navigation_rollback_to_the_new_side() {
    let (_root, mut app) = fixture();
    let original = app.active_panel().cwd.clone();
    let child = original.join("vanishing");
    fs::create_dir(&child).unwrap();
    app.active_panel_mut().refresh().unwrap();
    app.active_panel_mut().cursor = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == child)
        .unwrap();
    app.apply(AppCommand::OpenEntry).unwrap();
    fs::remove_dir(&child).unwrap();
    app.apply(AppCommand::SwapPanels).unwrap();
    drain_background(&mut app);
    assert_eq!(app.active_panel, ActivePanel::Right);
    assert_eq!(app.active_panel().cwd, original);
    assert!(!app.active_panel().loading);
}

#[test]
fn swap_completes_navigation_when_terminal_failure_overtakes_scan_result() {
    let (_root, mut app) = fixture();
    let original = app.active_panel().cwd.clone();
    let child = original.join("vanishing");
    fs::create_dir(&child).unwrap();
    app.active_panel_mut().refresh().unwrap();
    app.active_panel_mut().cursor = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == child)
        .unwrap();
    app.apply(AppCommand::OpenEntry).unwrap();
    let commands = app.take_pending_worker_commands();
    let (id, stale_event) = commands
        .into_iter()
        .find_map(|command| match command {
            WorkerCommand::Run(job) => match job.request {
                JobRequest::RefreshPanel {
                    panel,
                    cwd,
                    source,
                    sort_mode,
                    filter,
                    request_id,
                    ..
                } => Some((
                    job.id,
                    BackgroundEvent::PanelRefreshed {
                        panel,
                        cwd,
                        source,
                        sort_mode,
                        filter,
                        request_id,
                        disk_usage: None,
                        result: Err("scan failed".into()),
                    },
                )),
                _ => None,
            },
            _ => None,
        })
        .unwrap();
    fs::remove_dir(&child).unwrap();
    app.handle_job_event(JobEvent::Finished {
        id,
        result: Err(JobError::from_message("scan failed")),
    });
    assert!(!app.active_panel().loading);
    app.apply(AppCommand::SwapPanels).unwrap();
    app.handle_background_event(stale_event);
    drain_background(&mut app);
    assert_eq!(app.active_panel, ActivePanel::Right);
    assert_eq!(app.active_panel().cwd, original);
}

#[test]
fn swap_preserves_quick_view_and_pending_mkdir_destination() {
    let (_root, mut app) = fixture();
    app.apply(AppCommand::SetOtherPanelView(PanelViewMode::QuickView))
        .unwrap();
    app.apply(AppCommand::SwapPanels).unwrap();
    drain_background(&mut app);
    assert_eq!(
        app.panel_view_mode(ActivePanel::Left),
        PanelViewMode::QuickView
    );
    assert_eq!(
        app.quick_view_state(ActivePanel::Left)
            .viewer()
            .unwrap()
            .bytes,
        b"left"
    );
    app.apply(AppCommand::OpenInputDialog).unwrap();
    let destination = app.active_panel().cwd.join("new-dir");
    app.finish_dialog(DialogResult::InputSubmitted("new-dir".into()));
    app.apply(AppCommand::SwapPanels).unwrap();
    drain_background(&mut app);
    assert_eq!(app.active_panel, ActivePanel::Left);
    assert_eq!(app.active_panel().cwd, destination);
}

#[test]
#[cfg(unix)]
fn swap_waits_for_initial_panelize_without_replaying_or_truncating_command() {
    for partial in [false, true] {
        let (_root, mut app) = fixture();
        let cwd = app.active_panel().cwd.clone();
        app.start_panelize_command("printf x >> invocation-count; printf 'left.txt\\n'".into());
        if partial {
            let job = app
                .pending_worker_commands
                .iter()
                .find_map(|command| match command {
                    WorkerCommand::Run(job) => match &job.request {
                        JobRequest::RefreshPanel {
                            panel,
                            cwd,
                            source,
                            sort_mode,
                            filter,
                            request_id,
                            ..
                        } => Some((
                            *panel,
                            cwd.clone(),
                            source.clone(),
                            *sort_mode,
                            filter.clone(),
                            *request_id,
                        )),
                        _ => None,
                    },
                    _ => None,
                })
                .unwrap();
            app.handle_background_event(BackgroundEvent::PanelEntriesChunk {
                panel: job.0,
                cwd: job.1,
                source: job.2,
                sort_mode: job.3,
                filter: job.4,
                request_id: job.5,
                entries: vec![FileEntry {
                    path: cwd.join("left.txt"),
                    ..file_entry("left.txt")
                }],
            });
        }
        let queued = app.pending_worker_commands.len();
        app.apply(AppCommand::SwapPanels).unwrap();
        assert_eq!(app.active_panel, ActivePanel::Left);
        assert!(app.status_line.contains("Wait"));
        assert_eq!(app.pending_worker_commands.len(), queued);
        drain_background(&mut app);
        app.apply(AppCommand::SwapPanels).unwrap();
        drain_background(&mut app);
        assert_eq!(app.active_panel, ActivePanel::Right);
        assert_eq!(app.active_panel().entries.len(), 1);
        assert_eq!(app.active_panel().entries[0].path, cwd.join("left.txt"));
        assert_eq!(fs::read(cwd.join("invocation-count")).unwrap(), b"x");
    }
}
