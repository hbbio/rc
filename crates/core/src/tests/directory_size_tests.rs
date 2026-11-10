use super::*;

fn fixture() -> (tempfile::TempDir, AppState) {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("large/nested")).unwrap();
    fs::create_dir(root.path().join("small")).unwrap();
    fs::write(root.path().join("large/nested/data"), vec![0; 200]).unwrap();
    fs::write(root.path().join("small/data"), vec![0; 10]).unwrap();
    let app = app_with_loaded_panels(root.path().to_path_buf());
    (root, app)
}

#[test]
fn directory_sizes_update_listing_and_sort_without_changing_selection() {
    let (root, mut app) = fixture();
    app.active_panel_mut().cursor = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.name == "large")
        .unwrap();
    app.set_panel_sort_mode(
        ActivePanel::Left,
        SortMode {
            field: SortField::Size,
            reverse: false,
        },
    );
    app.apply(AppCommand::ShowDirectorySizes).unwrap();
    drain_background(&mut app);
    let entries = &app.active_panel().entries;
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.name == "large")
            .unwrap()
            .size,
        200
    );
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.name == "small")
            .unwrap()
            .size,
        10
    );
    assert_eq!(
        app.active_panel().selected_entry().unwrap().path,
        root.path().join("large")
    );
    assert_eq!(
        entries
            .iter()
            .filter(|entry| !entry.is_parent())
            .map(|entry| entry.size)
            .collect::<Vec<_>>(),
        [10, 200]
    );
}

#[test]
fn directory_sizes_respect_tags_and_follow_panels_when_swapped() {
    let (_root, mut app) = fixture();
    app.active_panel_mut().cursor = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.name == "large")
        .unwrap();
    app.active_panel_mut().toggle_tag_on_cursor();
    let small_before = app
        .active_panel()
        .entries
        .iter()
        .find(|entry| entry.name == "small")
        .unwrap()
        .size;
    app.apply(AppCommand::ShowDirectorySizes).unwrap();
    app.apply(AppCommand::SwapPanels).unwrap();
    drain_background(&mut app);
    assert_eq!(app.active_panel, ActivePanel::Right);
    assert_eq!(
        app.active_panel()
            .entries
            .iter()
            .find(|entry| entry.name == "large")
            .unwrap()
            .size,
        200
    );
    assert_eq!(
        app.active_panel()
            .entries
            .iter()
            .find(|entry| entry.name == "small")
            .unwrap()
            .size,
        small_before
    );
}

#[test]
fn directory_sizes_reject_late_results_after_cancellation_or_refresh() {
    for refresh in [false, true] {
        let (root, mut app) = fixture();
        let before = app
            .active_panel()
            .entries
            .iter()
            .find(|entry| entry.name == "large")
            .unwrap()
            .size;
        app.apply(AppCommand::ShowDirectorySizes).unwrap();
        let id = app.jobs.jobs().last().unwrap().id;
        if refresh {
            app.queue_panel_refresh(ActivePanel::Left);
        } else {
            assert!(app.request_cancel_for_job(id));
        }
        app.handle_background_event(BackgroundEvent::DirectorySizesMeasured {
            job_id: id,
            sizes: vec![(
                root.path().join("large"),
                SelectionSizeReport {
                    apparent_bytes: 9999,
                    unreadable_entries: 0,
                },
            )],
        });
        assert_eq!(
            app.active_panel()
                .entries
                .iter()
                .find(|entry| entry.name == "large")
                .unwrap()
                .size,
            before
        );
    }
}

#[test]
fn directory_sizes_report_removed_entries_as_partial() {
    let (root, mut app) = fixture();
    app.apply(AppCommand::ShowDirectorySizes).unwrap();
    fs::remove_dir_all(root.path().join("large")).unwrap();
    drain_background(&mut app);
    assert!(app.status_line.contains("partial results"));
    assert!(app.status_line.contains("1 unreadable"));
}

#[test]
fn directory_sizes_terminal_summary_survives_buffered_progress() {
    let (root, mut app) = fixture();
    app.apply(AppCommand::ShowDirectorySizes).unwrap();
    let id = app.jobs.jobs().last().unwrap().id;
    app.handle_background_event(BackgroundEvent::DirectorySizesMeasured {
        job_id: id,
        sizes: vec![(
            root.path().join("large"),
            SelectionSizeReport {
                apparent_bytes: 100,
                unreadable_entries: 2,
            },
        )],
    });
    let summary = app.status_line.clone();
    assert!(summary.contains("partial results"));
    app.handle_job_event(JobEvent::Progress {
        id,
        progress: JobProgress {
            current_path: None,
            items_total: 1,
            items_done: 1,
            bytes_total: 0,
            bytes_done: 100,
        },
    });
    app.handle_job_event(JobEvent::Finished { id, result: Ok(()) });
    assert_eq!(app.status_line, summary);
}

#[test]
fn directory_sizes_keep_parent_first_and_preserve_unsorted_order() {
    for field in [SortField::Size, SortField::Unsorted] {
        let (_root, mut app) = fixture();
        app.set_panel_sort_mode(
            ActivePanel::Left,
            SortMode {
                field,
                reverse: true,
            },
        );
        app.active_panel_mut().refresh().unwrap();
        let before: Vec<_> = app
            .active_panel()
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        app.apply(AppCommand::ShowDirectorySizes).unwrap();
        drain_background(&mut app);
        assert!(app.active_panel().entries[0].is_parent());
        if field == SortField::Unsorted {
            assert_eq!(
                app.active_panel()
                    .entries
                    .iter()
                    .map(|entry| entry.path.clone())
                    .collect::<Vec<_>>(),
                before
            );
        } else {
            assert_eq!(
                app.active_panel().entries[1..]
                    .iter()
                    .map(|entry| entry.size)
                    .collect::<Vec<_>>(),
                [200, 10]
            );
        }
    }
}
