use super::*;

#[test]
fn view_file_menu_accepts_literal_paths_and_records_only_successful_reads() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("file ' with spaces.txt");
    fs::write(&path, "hello viewer").unwrap();
    let mut app = app_with_loaded_panels(root.path().to_path_buf());
    app.apply(AppCommand::OpenMenuBarAt(1)).unwrap();
    move_menu_selection_to_label(&mut app, "View file...");
    app.apply(AppCommand::MenuAccept).unwrap();
    app.finish_dialog(DialogResult::InputSubmitted(
        "file ' with spaces.txt".into(),
    ));
    drain_background(&mut app);
    assert_eq!(app.active_viewer().unwrap().path(), path);
    assert_eq!(app.file_history.len(), 1);
    app.apply(AppCommand::CloseViewer).unwrap();
    app.apply(AppCommand::ViewFile).unwrap();
    app.finish_dialog(DialogResult::InputSubmitted("missing".into()));
    drain_background(&mut app);
    assert!(app.active_viewer().is_none());
    assert_eq!(app.file_history.len(), 1);
    app.apply(AppCommand::OpenFileHistory).unwrap();
    app.apply(AppCommand::DialogAccept).unwrap();
    drain_background(&mut app);
    assert_eq!(app.active_viewer().unwrap().path(), path);
    assert_eq!(app.file_history.len(), 1);
}

#[test]
fn file_history_reopens_edit_with_original_directory_and_current_editor() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app_with_loaded_panels(root.path().to_path_buf());
    let request = ExternalEditRequest {
        editor_command: "old-editor".into(),
        path: root.path().join("example.txt"),
        cwd: root.path().to_path_buf(),
    };
    app.record_edited_file(&request);
    app.settings.configuration.editor_command = Some("new-editor --wait".into());
    app.panels[0].cwd = root.path().join("elsewhere");
    app.apply(AppCommand::OpenFileHistory).unwrap();
    app.apply(AppCommand::DialogAccept).unwrap();
    let pending = app.take_pending_external_edit_requests();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].path, request.path);
    assert_eq!(pending[0].cwd, request.cwd);
    assert_eq!(pending[0].editor_command, "new-editor --wait");
}

#[test]
fn empty_and_canceled_history_do_not_schedule_work() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app_with_loaded_panels(root.path().to_path_buf());
    app.apply(AppCommand::OpenFileHistory).unwrap();
    assert_eq!(app.key_context(), KeyContext::FileManager);
    app.record_viewed_file(root.path().join("file"), None);
    app.apply(AppCommand::OpenFileHistory).unwrap();
    app.apply(AppCommand::DialogCancel).unwrap();
    assert!(app.take_pending_worker_commands().is_empty());
    app.apply(AppCommand::ViewFile).unwrap();
    app.apply(AppCommand::DialogCancel).unwrap();
    assert!(app.take_pending_worker_commands().is_empty());
}

#[test]
fn history_shortcut_and_custom_view_file_binding_dispatch() {
    let keymap =
        Keymap::parse("[filemanager]\nViewFile = f12\nEditorViewerHistory = alt-shift-e\n")
            .unwrap();
    for (chord, command) in [
        (KeyChord::new(KeyCode::F(12)), AppCommand::ViewFile),
        (
            KeyChord {
                code: KeyCode::Char('e'),
                modifiers: KeyModifiers {
                    alt: true,
                    shift: true,
                    ctrl: false,
                },
            },
            AppCommand::OpenFileHistory,
        ),
    ] {
        let key_command = keymap.resolve(KeyContext::FileManager, chord).unwrap();
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, key_command),
            Some(command)
        );
    }
}
