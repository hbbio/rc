use super::*;

fn command(app: &mut AppState, text: &str) -> ForegroundShellRequest {
    if app.command_line_session().is_none() {
        app.routes
            .push(Route::CommandLine(command_line::CommandLineSession {
                activation_id: 1,
                shell: rc_shell::ResolvedShell {
                    program: "/bin/sh".into(),
                    argument_template: vec!["-c".into(), "{command}".into()],
                    dialect: ShellDialect::Posix,
                    identity: "sh".into(),
                },
                completion: command_line::CommandLineCompletionState::Closed,
                notice: None,
            }));
    }
    app.handle_command_line_input(CommandLineInput::Paste(text.into()))
        .unwrap();
    app.handle_command_line_input(CommandLineInput::Enter)
        .unwrap();
    app.take_pending_foreground_shell_requests().pop().unwrap()
}

#[test]
fn shell_completion_updates_under_help_and_preserves_a_new_draft() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app_with_loaded_panels(root.path().into());
    let request = command(&mut app, "false");
    app.apply(AppCommand::OpenHelp).unwrap();
    app.finish_persistent_shell_request(&request, Ok(7));
    app.apply(AppCommand::CloseHelp).unwrap();
    assert!(app.command_line_model().value().is_empty());
    assert_eq!(app.command_line_model().last_exit_status(), Some(7));
    let request = command(&mut app, "sleep 1");
    app.handle_command_line_input(CommandLineInput::Clear)
        .unwrap();
    app.handle_command_line_input(CommandLineInput::Paste("next draft".into()))
        .unwrap();
    app.finish_persistent_shell_request(&request, Ok(0));
    assert_eq!(app.command_line_model().value(), "next draft");
    app.handle_command_line_input(CommandLineInput::Clear)
        .unwrap();
    let request = command(&mut app, "true");
    app.handle_command_line_input(CommandLineInput::Escape)
        .unwrap();
    app.finish_persistent_shell_request(&request, Ok(0));
    assert!(
        app.command_line_model().value().is_empty(),
        "executed draft survived closing the prompt"
    );
}

#[test]
fn shell_cwd_following_respects_new_navigation_and_panel_switches() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    let mut app = app_with_loaded_panels(root.path().into());
    let original = app.active_panel().cwd.clone();
    app.panels[0].cwd = a.clone();
    assert!(
        !app.follow_shell_directory(ActivePanel::Left, &original, &b)
            .unwrap()
    );
    app.active_panel = ActivePanel::Right;
    assert!(
        !app.follow_shell_directory(ActivePanel::Left, &a, &b)
            .unwrap()
    );
    app.active_panel = ActivePanel::Left;
    assert!(
        app.follow_shell_directory(ActivePanel::Left, &a, &b)
            .unwrap()
    );
    assert_eq!(app.active_panel().cwd, b);
}

#[test]
fn screen_list_can_restore_the_shell_without_discarding_current_routes() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app_with_loaded_panels(root.path().into());
    app.set_shell_available(true);
    app.apply(AppCommand::OpenScreenList).unwrap();
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("screen list");
    };
    let DialogKind::Listbox(list) = &dialog.kind else {
        panic!("listbox");
    };
    let index = list.items.iter().position(|item| item == "Shell").unwrap();
    app.apply(AppCommand::DialogListboxSelectAt(index)).unwrap();
    app.apply(AppCommand::DialogAccept).unwrap();
    assert!(app.take_panel_toggle_request());
    assert!(matches!(app.top_route(), Route::FileManager));
}
