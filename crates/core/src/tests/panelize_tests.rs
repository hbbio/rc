use super::*;

#[cfg(unix)]
#[test]
fn panelize_command_populates_active_panel_from_stdout_paths() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-populate-{stamp}"));
    fs::create_dir_all(root.join("sub")).expect("must create subdirectory");
    fs::write(root.join("a.txt"), "a").expect("must create file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    submit_panelize_custom_command(&mut app, "printf 'a.txt\\nsub\\nmissing\\n'");
    drain_background(&mut app);

    let panel = app.active_panel();
    assert_eq!(
        panel.panelize_command(),
        Some("printf 'a.txt\\nsub\\nmissing\\n'"),
        "panelize mode should retain command for reread"
    );
    assert!(
        panel
            .entries
            .iter()
            .any(|entry| entry.path == root.join("a.txt")),
        "panelized entries should include file output path"
    );
    assert!(
        panel
            .entries
            .iter()
            .any(|entry| entry.path == root.join("sub")),
        "panelized entries should include directory output path"
    );
    assert!(
        panel
            .entries
            .iter()
            .any(|entry| entry.path == root.join("missing")),
        "panelized entries should preserve command output even when path metadata is missing"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[cfg(unix)]
#[test]
fn panelize_empty_output_keeps_empty_panel_entries() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-empty-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    fs::write(root.join("a.txt"), "a").expect("must create file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    submit_panelize_custom_command(&mut app, "printf ''");
    drain_background(&mut app);

    assert_eq!(
        app.active_panel().entries.len(),
        0,
        "empty panelize output should produce empty panel entries"
    );
    assert_eq!(app.active_panel().panelize_command(), Some("printf ''"));

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[cfg(unix)]
#[test]
fn panelize_preserves_leading_and_trailing_spaces_in_paths() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-spaces-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let spaced_name = "  spaced file  ";
    let spaced_file = root.join(spaced_name);
    fs::write(&spaced_file, "a").expect("must create spaced filename");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    submit_panelize_custom_command(&mut app, "printf '  spaced file  \\n'");
    drain_background(&mut app);

    assert!(
        app.active_panel()
            .entries
            .iter()
            .any(|entry| entry.path == spaced_file),
        "panelize should preserve leading/trailing spaces in path lines"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[cfg(unix)]
#[test]
fn cdup_leaves_panelize_mode_without_changing_directory() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-cdup-{stamp}"));
    let sub = root.join("sub");
    fs::create_dir_all(&sub).expect("must create subdirectory");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    submit_panelize_custom_command(&mut app, "printf 'sub\\n'");
    drain_background(&mut app);

    assert_eq!(
        app.active_panel().panelize_command(),
        Some("printf 'sub\\n'"),
        "precondition: panel should be in panelize mode"
    );
    assert_eq!(app.active_panel().cwd, root);

    app.apply(AppCommand::CdUp)
        .expect("CdUp should leave panelize mode");
    drain_background(&mut app);

    assert_eq!(
        app.active_panel().panelize_command(),
        None,
        "CdUp should restore normal directory mode from panelize"
    );
    assert_eq!(
        app.active_panel().cwd,
        root,
        "CdUp in panelize mode should not change to parent directory"
    );
    assert!(
        app.active_panel()
            .entries
            .iter()
            .any(|entry| entry.path == sub),
        "restored listing should include entries from the current directory"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[cfg(unix)]
#[test]
fn side_panel_menu_restores_external_panelize_results_and_operation_targets() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-restore-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let target = root.join("remembered.txt");
    fs::write(&target, "remember me").expect("must create panelized file");

    let mut app = app_with_loaded_panels(root.clone());
    submit_panelize_custom_command(&mut app, "printf 'remembered.txt\\n'");
    drain_background(&mut app);
    app.apply(AppCommand::ToggleTag)
        .expect("panelized result should be taggable");

    app.apply(AppCommand::CdUp)
        .expect("CdUp should leave panelize mode");
    drain_background(&mut app);
    assert!(matches!(
        app.active_panel().source,
        PanelListingSource::Directory
    ));

    app.apply(AppCommand::OpenMenuBarAt(0))
        .expect("left panel menu should open");
    move_menu_selection_to_label(&mut app, "Panelize");
    app.apply(AppCommand::MenuAccept)
        .expect("side-panel Panelize should restore results");

    assert_eq!(
        app.active_panel().panelize_command(),
        Some("printf 'remembered.txt\\n'")
    );
    assert_eq!(
        app.active_panel().selected_entry().map(|entry| &entry.path),
        Some(&target),
        "restoring should preserve the selected result"
    );
    assert!(
        app.active_panel().is_tagged(&target),
        "restoring should preserve tagged results"
    );
    let pending = app.take_pending_worker_commands();
    assert!(
        pending.iter().all(|command| matches!(
            command,
            WorkerCommand::Run(job)
                if matches!(
                    &job.request,
                    JobRequest::MeasureSelection { paths, .. }
                        if paths.as_slice() == std::slice::from_ref(&target)
                )
        )),
        "history restoration may remeasure restored tags but must not rerun the external command"
    );

    app.apply(AppCommand::Copy)
        .expect("copy should open for a restored result");
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("copy should open a destination dialog");
    };
    match dialog.action() {
        Some(PendingDialogAction::TransferDestination { sources, .. }) => {
            assert_eq!(sources, &vec![target]);
        }
        other => panic!("expected copy destination action, got {other:?}"),
    }

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[cfg(unix)]
#[test]
fn restoring_panelize_history_invalidates_an_older_directory_refresh() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-restore-stale-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let target = root.join("remembered.txt");
    fs::write(&target, "remember me").expect("must create panelized file");

    let mut app = app_with_loaded_panels(root.clone());
    submit_panelize_custom_command(&mut app, "printf 'remembered.txt\\n'");
    drain_background(&mut app);
    app.apply(AppCommand::CdUp)
        .expect("CdUp should queue a directory refresh");

    let stale_request = app
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
        .expect("leaving panelize should queue a directory refresh");

    app.apply(AppCommand::RestorePanelizedResults)
        .expect("history should restore synchronously");
    let (panel, cwd, source, sort_mode, request_id) = stale_request;
    app.handle_background_event(BackgroundEvent::PanelRefreshed {
        panel,
        cwd,
        source,
        sort_mode,
        filter: PanelFilter::default(),
        request_id,
        disk_usage: None,
        result: Ok(panel_refresh_result(vec![FileEntry::file(
            String::from("stale.txt"),
            root.join("stale.txt"),
            0,
            None,
        )])),
    });

    assert!(app.active_panel().is_panelized());
    assert_eq!(
        app.active_panel()
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>(),
        vec![target],
        "a stale refresh must not replace restored history"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[cfg(unix)]
#[test]
fn panelize_history_is_independent_for_each_panel() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-per-panel-history-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    fs::write(root.join("left.txt"), "left").expect("must create left fixture");
    fs::write(root.join("right.txt"), "right").expect("must create right fixture");

    let mut app = app_with_loaded_panels(root.clone());
    submit_panelize_custom_command(&mut app, "printf 'left.txt\\n'");
    drain_background(&mut app);
    app.apply(AppCommand::CdUp)
        .expect("left panel should leave panelize mode");
    drain_background(&mut app);

    app.toggle_active_panel();
    submit_panelize_custom_command(&mut app, "printf 'right.txt\\n'");
    drain_background(&mut app);
    app.apply(AppCommand::CdUp)
        .expect("right panel should leave panelize mode");
    drain_background(&mut app);

    app.apply(AppCommand::RestorePanelizedResults)
        .expect("right history should restore");
    assert_eq!(
        app.active_panel()
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>(),
        vec![root.join("right.txt")]
    );

    app.toggle_active_panel();
    app.apply(AppCommand::RestorePanelizedResults)
        .expect("left history should restore");
    assert_eq!(
        app.active_panel()
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>(),
        vec![root.join("left.txt")]
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn cancel_job_targets_the_active_panelize_refresh_not_a_newer_job() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-targeted-cancel-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.start_panelize_command(String::from("long-running command"));
    let panelize_job = app
        .panel_refresh_job_id(ActivePanel::Left)
        .expect("left panelize refresh should have a job");
    app.toggle_active_panel();
    app.refresh_active_panel();
    let newer_job = app
        .panel_refresh_job_id(ActivePanel::Right)
        .expect("right directory refresh should have a job");
    assert!(newer_job.0 > panelize_job.0);
    app.toggle_active_panel();

    app.apply(AppCommand::CancelJob)
        .expect("cancel should target the visible panelize job");
    app.apply(AppCommand::CancelJob)
        .expect("a repeated cancel must remain scoped to the panelize job");
    let canceled_jobs: Vec<JobId> = app
        .take_pending_worker_commands()
        .into_iter()
        .filter_map(|command| match command {
            WorkerCommand::Cancel(job_id) => Some(job_id),
            WorkerCommand::Run(_) | WorkerCommand::Shutdown => None,
        })
        .collect();
    assert_eq!(canceled_jobs, vec![panelize_job]);
    assert!(app.status_line.contains(&format!("#{panelize_job}")));

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[cfg(unix)]
#[test]
fn panelize_failure_preserves_previous_directory_listing() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-failure-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    fs::write(root.join("a.txt"), "a").expect("must create file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let before = app.active_panel().entries.clone();

    submit_panelize_custom_command(&mut app, "exit 42");
    drain_background(&mut app);

    assert!(
        app.status_line.contains("Panelize failed:"),
        "status line should indicate panelize failure"
    );
    assert_eq!(
        app.active_panel().entries,
        before,
        "failed panelize should keep previous listing"
    );
    assert_eq!(
        app.active_panel().panelize_command(),
        None,
        "failed panelize should not switch source mode"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[cfg(unix)]
#[test]
fn rename_dialog_uses_basename_for_panelized_entry() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-rename-basename-{stamp}"));
    let sub = root.join("sub");
    fs::create_dir_all(&sub).expect("must create subdirectory");
    fs::write(sub.join("a.txt"), "a").expect("must create file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    submit_panelize_custom_command(&mut app, "printf 'sub/a.txt\\n'");
    drain_background(&mut app);

    app.apply(AppCommand::OpenConfirmDialog)
        .expect("rename dialog should open");
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("rename action should open a dialog route");
    };
    let DialogKind::Input(input) = &dialog.kind else {
        panic!("rename action should open an input dialog");
    };
    assert_eq!(
        input.value, "a.txt",
        "rename input should default to basename, not panelized display label"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn panelize_dialog_lists_predefined_commands() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-presets-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.open_panelize_dialog();
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("panelize should open a dialog");
    };
    let DialogKind::Listbox(listbox) = &dialog.kind else {
        panic!("panelize should open a listbox dialog");
    };
    assert_eq!(
        listbox.items.first(),
        Some(&String::from(PANELIZE_CUSTOM_COMMAND_LABEL))
    );
    assert!(
        listbox.items.iter().any(|item| item == "Backup files"),
        "panelize list should include descriptive preset labels"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn panelize_dialog_tab_switches_from_presets_to_input() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-tab-to-input-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.open_panelize_dialog();
    app.apply(AppCommand::DialogFocusNext)
        .expect("tab should switch to command input");

    let Route::Dialog(dialog) = app.top_route() else {
        panic!("panelize should remain in dialog route");
    };
    let DialogKind::Input(input) = &dialog.kind else {
        panic!("tab should open panelize input dialog");
    };
    assert_eq!(input.value, "find . -type f");

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn panelize_dialog_tab_switches_from_input_back_to_presets() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-tab-to-presets-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.open_panelize_dialog();
    app.apply(AppCommand::DialogFocusNext)
        .expect("tab should switch to command input");
    app.apply(AppCommand::DialogInputChar('x'))
        .expect("typing command suffix should succeed");
    app.apply(AppCommand::DialogFocusNext)
        .expect("tab should switch back to preset list");

    let Route::Dialog(dialog) = app.top_route() else {
        panic!("panelize should remain in dialog route");
    };
    let DialogKind::Listbox(listbox) = &dialog.kind else {
        panic!("tab should return to preset list");
    };
    assert_eq!(listbox.selected, 0);
    assert_eq!(
        listbox.items.first(),
        Some(&String::from(PANELIZE_CUSTOM_COMMAND_LABEL))
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn panelize_preset_management_add_edit_remove_works() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-preset-manage-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.open_panelize_dialog();
    app.apply(AppCommand::PanelizePresetAdd)
        .expect("F2 add should open preset input");
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("panelize preset editor should open");
    };
    assert!(matches!(dialog.kind, DialogKind::PairInput(_)));
    app.finish_dialog(DialogResult::PairInputSubmitted {
        first: String::from("Tracked files"),
        second: String::from("git ls-files"),
    });

    let Route::Dialog(dialog) = app.top_route() else {
        panic!("panelize should remain in preset list dialog");
    };
    let DialogKind::Listbox(listbox) = &dialog.kind else {
        panic!("panelize should return to preset list dialog");
    };
    assert!(
        listbox.items.iter().any(|item| item == "Tracked files"),
        "added preset should appear in list"
    );

    app.apply(AppCommand::PanelizePresetEdit)
        .expect("F4 edit should open preset input");
    app.finish_dialog(DialogResult::PairInputSubmitted {
        first: String::from("Repository files"),
        second: String::from("git ls-files --cached"),
    });

    let Route::Dialog(dialog) = app.top_route() else {
        panic!("panelize should remain in preset list dialog");
    };
    let DialogKind::Listbox(listbox) = &dialog.kind else {
        panic!("panelize should return to preset list dialog");
    };
    assert!(listbox.items.iter().any(|item| item == "Repository files"));
    assert!(
        app.settings()
            .configuration
            .panelize_presets
            .iter()
            .any(|preset| {
                preset.label == "Repository files" && preset.command == "git ls-files --cached"
            })
    );

    app.apply(AppCommand::PanelizePresetRemove)
        .expect("F8 remove should request confirmation");
    assert!(
        matches!(app.top_route(), Route::Dialog(dialog) if matches!(dialog.kind, DialogKind::Confirm(_)))
    );
    app.finish_dialog(DialogResult::ConfirmDeclined);
    assert!(
        app.settings()
            .configuration
            .panelize_presets
            .iter()
            .any(|preset| preset.label == "Repository files")
    );
    app.apply(AppCommand::PanelizePresetRemove)
        .expect("F8 remove should request confirmation again");
    app.finish_dialog(DialogResult::ConfirmAccepted);
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("panelize should remain in preset list dialog");
    };
    let DialogKind::Listbox(listbox) = &dialog.kind else {
        panic!("panelize should return to preset list dialog");
    };
    assert!(
        !listbox.items.iter().any(|item| item == "Repository files"),
        "removed preset should no longer be listed"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn panelize_preset_editor_rejects_empty_and_duplicate_fields() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-preset-validate-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.open_panelize_dialog();
    app.apply(AppCommand::PanelizePresetAdd)
        .expect("preset add should open");

    for (label, command, expected) in [
        ("  ", "echo unique", "label cannot be empty"),
        ("Unique", "  ", "command cannot be empty"),
        (" all FILES ", "echo unique", "label already exists"),
        ("Unique", "find . -type f", "command already exists"),
    ] {
        app.finish_dialog(DialogResult::PairInputSubmitted {
            first: label.to_string(),
            second: command.to_string(),
        });
        assert!(app.status_line.contains(expected), "{}", app.status_line);
        assert!(
            matches!(app.top_route(), Route::Dialog(dialog) if matches!(dialog.kind, DialogKind::PairInput(_)))
        );
    }

    app.finish_dialog(DialogResult::PairInputSubmitted {
        first: String::from("Unique"),
        second: String::from("echo unique"),
    });
    assert!(
        app.settings()
            .configuration
            .panelize_presets
            .iter()
            .any(|preset| preset.label == "Unique" && preset.command == "echo unique")
    );

    app.apply(AppCommand::PanelizePresetEdit)
        .expect("new preset should be editable");
    app.finish_dialog(DialogResult::PairInputSubmitted {
        first: String::from("BACKUP FILES"),
        second: String::from("echo changed"),
    });
    assert!(app.status_line.contains("label already exists"));
    app.finish_dialog(DialogResult::PairInputSubmitted {
        first: String::from("Changed"),
        second: String::from("find . -name '*.orig'"),
    });
    assert!(app.status_line.contains("command already exists"));
    app.finish_dialog(DialogResult::Canceled);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn stale_panelize_preset_editor_cannot_overwrite_changed_settings() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-preset-stale-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.open_panelize_dialog();
    app.apply(AppCommand::PanelizePresetAdd)
        .expect("preset add should open");
    app.settings_mut()
        .configuration
        .panelize_presets
        .push(PanelizePreset::new("External", "echo external"));
    app.finish_dialog(DialogResult::PairInputSubmitted {
        first: String::from("Submitted"),
        second: String::from("echo submitted"),
    });

    assert!(app.status_line.contains("presets changed"));
    assert!(
        app.settings()
            .configuration
            .panelize_presets
            .iter()
            .any(|preset| preset.label == "External")
    );
    assert!(
        !app.settings()
            .configuration
            .panelize_presets
            .iter()
            .any(|preset| preset.label == "Submitted")
    );
    assert!(
        matches!(app.top_route(), Route::Dialog(dialog) if matches!(dialog.kind, DialogKind::Listbox(_)))
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[cfg(unix)]
#[test]
fn panelize_preset_selection_runs_without_custom_input() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-preset-select-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    fs::write(root.join("a.txt"), "a").expect("must create file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.open_panelize_dialog();
    app.finish_dialog(DialogResult::ListboxSubmitted {
        index: Some(1),
        value: Some(String::from("find . -type f")),
    });
    drain_background(&mut app);

    assert_eq!(
        app.active_panel().panelize_command(),
        Some("find . -type f")
    );
    assert!(
        app.active_panel()
            .entries
            .iter()
            .any(|entry| entry.path == root.join("a.txt")),
        "preset command should populate panel entries"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[cfg(unix)]
#[test]
fn panelize_command_can_be_canceled_while_shell_process_runs() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-cancel-running-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let cancel_clone = Arc::clone(&cancel_flag);
    let cancel_task = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        cancel_clone.store(true, AtomicOrdering::Relaxed);
    });

    let started_at = Instant::now();
    let result = read_panelized_entries_with_cancel(
        &root,
        "sleep 3; printf 'a.txt\\n'",
        SortMode::default(),
        Some(cancel_flag.as_ref()),
    );

    cancel_task
        .join()
        .expect("cancel request thread should finish");
    let error = result.expect_err("panelize command should be canceled");
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    assert_eq!(error.to_string(), PANEL_REFRESH_CANCELED_MESSAGE);
    let elapsed = started_at.elapsed();
    assert!(
        elapsed < Duration::from_secs(1),
        "canceled panelize command should stop quickly, took {elapsed:?}"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}
