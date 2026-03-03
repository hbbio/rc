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
        listbox
            .items
            .iter()
            .any(|item| item == "find . -name '*.orig'"),
        "panelize list should include predefined commands"
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
    for ch in "echo added".chars() {
        app.apply(AppCommand::DialogInputChar(ch))
            .expect("typing preset command should succeed");
    }
    app.apply(AppCommand::DialogAccept)
        .expect("submitting new preset should succeed");

    let Route::Dialog(dialog) = app.top_route() else {
        panic!("panelize should remain in preset list dialog");
    };
    let DialogKind::Listbox(listbox) = &dialog.kind else {
        panic!("panelize should return to preset list dialog");
    };
    assert!(
        listbox.items.iter().any(|item| item == "echo added"),
        "added preset should appear in list"
    );

    app.apply(AppCommand::PanelizePresetEdit)
        .expect("F4 edit should open preset input");
    for ch in " updated".chars() {
        app.apply(AppCommand::DialogInputChar(ch))
            .expect("typing edit suffix should succeed");
    }
    app.apply(AppCommand::DialogAccept)
        .expect("submitting edited preset should succeed");

    let edited = String::from("echo added updated");
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("panelize should remain in preset list dialog");
    };
    let DialogKind::Listbox(listbox) = &dialog.kind else {
        panic!("panelize should return to preset list dialog");
    };
    assert!(
        listbox.items.iter().any(|item| item == &edited),
        "edited preset should replace previous value"
    );

    app.apply(AppCommand::PanelizePresetRemove)
        .expect("F8 remove should delete selected preset");
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("panelize should remain in preset list dialog");
    };
    let DialogKind::Listbox(listbox) = &dialog.kind else {
        panic!("panelize should return to preset list dialog");
    };
    assert!(
        !listbox.items.iter().any(|item| item == &edited),
        "removed preset should no longer be listed"
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
