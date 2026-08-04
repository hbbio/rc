use super::*;

fn input_dialog(app: &AppState) -> &crate::dialog::InputDialogState {
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("input dialog should be active");
    };
    let DialogKind::Input(input) = &dialog.kind else {
        panic!("dialog should contain a single input");
    };
    input
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
    assert_eq!(input_dialog(&app).prompt, "Directory:");
    assert_eq!(input_dialog(&app).value, "");

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
    app.finish_dialog(DialogResult::InputSubmitted(String::from("missing path")));

    assert_eq!(app.key_context(), KeyContext::Input);
    assert_eq!(input_dialog(&app).value, "missing path");
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
    app.finish_dialog(DialogResult::InputSubmitted(String::from("../second")));

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
    app.finish_dialog(DialogResult::InputSubmitted(String::from("-")));
    assert_eq!(app.active_panel().cwd, first);
    assert_eq!(
        app.previous_panel_directories[ActivePanel::Left.index()],
        Some(second.clone())
    );

    app.take_pending_worker_commands();
    app.apply(AppCommand::OpenQuickCd)
        .expect("third quick cd should open");
    app.finish_dialog(DialogResult::InputSubmitted(String::from("-")));
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
