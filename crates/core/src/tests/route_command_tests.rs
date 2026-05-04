use super::*;

#[test]
fn tree_screen_selects_directory_for_active_panel() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-tree-screen-{stamp}"));
    let branch = root.join("branch");
    fs::create_dir_all(&branch).expect("must create temp tree");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenTree)
        .expect("tree screen should open");
    drain_background(&mut app);
    assert_eq!(app.key_context(), KeyContext::Tree);

    let branch_index = match app.top_route() {
        Route::Tree(tree) => tree
            .entries
            .iter()
            .position(|entry| entry.path == branch)
            .expect("branch should appear in tree"),
        _ => panic!("top route should be tree"),
    };
    let Some(Route::Tree(tree)) = app.routes.last_mut() else {
        panic!("top route should be tree");
    };
    tree.cursor = branch_index;

    app.apply(AppCommand::TreeOpenEntry)
        .expect("tree open should succeed");
    assert_eq!(app.key_context(), KeyContext::FileManager);
    assert_eq!(app.active_panel().cwd, branch);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn hotlist_supports_add_remove_and_open() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-hotlist-{stamp}"));
    let branch = root.join("branch");
    fs::create_dir_all(&branch).expect("must create temp tree");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenHotlist)
        .expect("hotlist should open");
    app.apply(AppCommand::HotlistAddCurrentDirectory)
        .expect("hotlist add should succeed");
    assert_eq!(app.hotlist(), std::slice::from_ref(&root));

    {
        let panel = app.active_panel_mut();
        panel.cwd = branch.clone();
        panel.refresh().expect("panel should refresh");
    }
    app.apply(AppCommand::HotlistAddCurrentDirectory)
        .expect("hotlist add should succeed");
    assert_eq!(app.hotlist(), &[root.clone(), branch.clone()]);

    app.hotlist_cursor = 0;
    app.apply(AppCommand::HotlistRemoveSelected)
        .expect("hotlist remove should succeed");
    assert_eq!(app.hotlist(), std::slice::from_ref(&branch));

    app.hotlist_cursor = 0;
    app.apply(AppCommand::HotlistOpenEntry)
        .expect("hotlist open should succeed");
    assert_eq!(app.key_context(), KeyContext::FileManager);
    assert_eq!(app.active_panel().cwd, branch);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn xmap_mode_applies_to_next_file_manager_command_only() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-xmap-mode-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    assert_eq!(app.key_context(), KeyContext::FileManager);
    app.apply(AppCommand::EnterXMap)
        .expect("xmap mode should activate");
    assert_eq!(app.key_context(), KeyContext::FileManagerXMap);
    app.apply(AppCommand::MoveDown)
        .expect("next command should execute");
    assert_eq!(app.key_context(), KeyContext::FileManager);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn resolve_external_editor_command_prefers_editor_over_visual() {
    let editor = resolve_external_editor_command_with_lookup(
        Some("  hx --wait  "),
        |name| match name {
            "EDITOR" => Some(String::from("  nvim  ")),
            "VISUAL" => Some(String::from("vim")),
            _ => None,
        },
        |_| false,
    );
    assert_eq!(editor, Some(String::from("hx --wait")));
}

#[test]
fn resolve_external_editor_command_uses_env_then_path_probe() {
    let editor = resolve_external_editor_command_with_lookup(
        None,
        |name| match name {
            "EDITOR" => Some(String::from("  ")),
            "VISUAL" => Some(String::from(" code --wait ")),
            _ => None,
        },
        |_| false,
    );
    assert_eq!(editor, Some(String::from("code --wait")));

    let probed =
        resolve_external_editor_command_with_lookup(None, |_| None, |name| matches!(name, "vim"));
    assert_eq!(probed, Some(String::from("vim")));

    let missing = resolve_external_editor_command_with_lookup(None, |_| None, |_| false);
    assert_eq!(missing, None);
}

#[cfg(unix)]
#[test]
fn executable_candidate_requires_execute_bit() {
    use std::os::unix::fs::PermissionsExt;

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-editor-path-probe-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let editor = root.join("vim");
    fs::write(&editor, "#!/bin/sh\n").expect("editor fixture should be writable");

    fs::set_permissions(&editor, fs::Permissions::from_mode(0o644))
        .expect("permissions should be writable");
    assert!(
        !executable_candidate_exists(&root, "vim"),
        "non-executable files should not satisfy PATH probing"
    );

    fs::set_permissions(&editor, fs::Permissions::from_mode(0o755))
        .expect("permissions should be writable");
    assert!(
        executable_candidate_exists(&root, "vim"),
        "executable files should satisfy PATH probing"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn app_command_mapping_is_context_aware() {
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenHelp),
        Some(AppCommand::OpenHelp)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Help, &KeyCommand::Quit),
        Some(AppCommand::CloseHelp)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Help, &KeyCommand::HelpBack),
        Some(AppCommand::HelpBack)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenMenu),
        Some(AppCommand::OpenMenu)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::CursorUp),
        Some(AppCommand::MenuMoveUp)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::CursorDown),
        Some(AppCommand::MenuMoveDown)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::CursorLeft),
        Some(AppCommand::MenuMoveLeft)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::CursorRight),
        Some(AppCommand::MenuMoveRight)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::DialogAccept),
        Some(AppCommand::MenuAccept)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::DialogCancel),
        Some(AppCommand::CloseMenu)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::CursorUp),
        Some(AppCommand::MoveUp)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenEntry),
        Some(AppCommand::OpenEntry)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::EditEntry),
        Some(AppCommand::EditEntry)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Listbox, &KeyCommand::CursorUp),
        Some(AppCommand::DialogListboxUp)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Listbox, &KeyCommand::OpenInputDialog),
        Some(AppCommand::PanelizePresetAdd)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Listbox, &KeyCommand::OpenConfirmDialog),
        Some(AppCommand::PanelizePresetEdit)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Listbox, &KeyCommand::Delete),
        Some(AppCommand::PanelizePresetRemove)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::DialogAccept),
        Some(AppCommand::DialogAccept)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::ToggleTag),
        Some(AppCommand::ToggleTag)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::SortNext),
        Some(AppCommand::SortNext)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::Copy),
        Some(AppCommand::Copy)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::Move),
        Some(AppCommand::Move)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::Delete),
        Some(AppCommand::Delete)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::CancelJob),
        Some(AppCommand::CancelJob)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenJobs),
        Some(AppCommand::OpenJobsScreen)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenFindDialog),
        Some(AppCommand::OpenFindDialog)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::CursorDown),
        Some(AppCommand::FindResultsMoveDown)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::OpenEntry),
        Some(AppCommand::FindResultsOpenEntry)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::OpenPanelizeDialog),
        Some(AppCommand::FindResultsPanelize)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::CancelJob),
        Some(AppCommand::CancelJob)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::Quit),
        Some(AppCommand::CloseFindResults)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenTree),
        Some(AppCommand::OpenTree)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::CursorUp),
        Some(AppCommand::TreeMoveUp)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::OpenEntry),
        Some(AppCommand::TreeOpenEntry)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::Quit),
        Some(AppCommand::CloseTree)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenHotlist),
        Some(AppCommand::OpenHotlist)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenPanelizeDialog),
        Some(AppCommand::OpenPanelizeDialog)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenSkinDialog),
        Some(AppCommand::OpenSkinDialog)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::EnterXMap),
        Some(AppCommand::EnterXMap)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Hotlist, &KeyCommand::AddHotlist),
        Some(AppCommand::HotlistAddCurrentDirectory)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Hotlist, &KeyCommand::RemoveHotlist),
        Some(AppCommand::HotlistRemoveSelected)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Hotlist, &KeyCommand::OpenEntry),
        Some(AppCommand::HotlistOpenEntry)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Hotlist, &KeyCommand::Quit),
        Some(AppCommand::CloseHotlist)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Jobs, &KeyCommand::CursorUp),
        Some(AppCommand::JobsMoveUp)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Jobs, &KeyCommand::CursorDown),
        Some(AppCommand::JobsMoveDown)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Jobs, &KeyCommand::CloseJobs),
        Some(AppCommand::CloseJobsScreen)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::Quit),
        Some(AppCommand::CloseViewer)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::Search),
        Some(AppCommand::ViewerSearchForward)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::SearchBackward),
        Some(AppCommand::ViewerSearchBackward)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::SearchContinue),
        Some(AppCommand::ViewerSearchContinue)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::SearchContinueBackward),
        Some(AppCommand::ViewerSearchContinueBackward)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::Goto),
        Some(AppCommand::ViewerGoto)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::ToggleWrap),
        Some(AppCommand::ViewerToggleWrap)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::ToggleHex),
        Some(AppCommand::ViewerToggleHex)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::ViewerHex, &KeyCommand::ToggleHex),
        Some(AppCommand::ViewerToggleHex)
    );
}
