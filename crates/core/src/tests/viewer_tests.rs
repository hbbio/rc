use super::*;

#[test]
fn open_entry_on_file_opens_viewer_route() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-viewer-open-file-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let file_path = root.join("notes.txt");
    fs::write(&file_path, "alpha\nbeta\ngamma\n").expect("must create viewer file");

    let mut app = app_with_loaded_panels(root.clone());
    let file_index = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == file_path)
        .expect("viewer file should be visible");
    app.active_panel_mut().cursor = file_index;

    app.apply(AppCommand::OpenEntry)
        .expect("open entry should open viewer");
    drain_background(&mut app);
    assert_eq!(app.key_context(), KeyContext::Viewer);

    let Route::Viewer(viewer) = app.top_route() else {
        panic!("top route should be viewer");
    };
    assert_eq!(viewer.path(), &file_path);
    assert_eq!(viewer.line_count(), 3);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[cfg(unix)]
#[test]
fn open_entry_on_directory_symlink_descends_into_directory() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-open-dir-symlink-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let target_dir = root.join("target");
    fs::create_dir_all(&target_dir).expect("must create target directory");
    fs::write(target_dir.join("entry.txt"), "payload").expect("must create target file");
    let symlink_path = root.join("tmp-like");
    std::os::unix::fs::symlink(&target_dir, &symlink_path)
        .expect("directory symlink should be creatable");

    let mut app = app_with_loaded_panels(root.clone());
    let symlink_index = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == symlink_path)
        .expect("directory symlink should be visible");
    assert!(
        app.active_panel().entries[symlink_index].is_dir,
        "directory symlink should be treated as a directory entry"
    );
    app.active_panel_mut().cursor = symlink_index;

    app.apply(AppCommand::OpenEntry)
        .expect("open entry should descend into directory symlink");
    assert_eq!(
        app.active_panel().cwd,
        symlink_path,
        "open entry should switch into the symlink path"
    );
    assert!(
        app.active_panel().loading,
        "opening a directory symlink should queue a panel refresh"
    );

    let pending = app.take_pending_worker_commands();
    assert_eq!(
        pending.len(),
        1,
        "directory open should queue one refresh request"
    );
    match &pending[0] {
        WorkerCommand::Run(job) => match &job.request {
            JobRequest::RefreshPanel {
                cwd,
                source: PanelListingSource::Directory,
                ..
            } => assert_eq!(
                cwd,
                &app.active_panel().cwd,
                "refresh request should target the opened symlink directory"
            ),
            other => panic!("expected refresh panel request, got {other:?}"),
        },
        other => panic!("expected queued worker run command, got {other:?}"),
    }

    fs::remove_dir_all(&root).expect("must remove temp root");
}
#[test]
fn edit_entry_on_file_queues_external_editor_request() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-edit-open-file-external-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let file_path = root.join("notes.txt");
    fs::write(&file_path, "alpha\nbeta\ngamma\n").expect("must create edit target");

    let mut app = app_with_loaded_panels(root.clone());
    let file_index = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == file_path)
        .expect("edit target should be visible");
    app.active_panel_mut().cursor = file_index;

    assert_eq!(
        app.open_selected_file_in_editor_with_resolver(|| Some(String::from("nvim"))),
        EditSelectionResult::OpenedExternal
    );

    let requests = app.take_pending_external_edit_requests();
    assert_eq!(requests.len(), 1, "one editor request should be queued");
    let request = &requests[0];
    assert_eq!(request.editor_command, "nvim");
    assert_eq!(request.path, file_path);
    assert_eq!(request.cwd, root);
    assert!(app.take_pending_worker_commands().is_empty());

    fs::remove_dir_all(&root).expect("must remove temp root");
}
#[test]
fn edit_entry_requires_external_editor() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-edit-open-file-internal-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let file_path = root.join("notes.txt");
    fs::write(&file_path, "alpha\nbeta\ngamma\n").expect("must create edit target");

    let mut app = app_with_loaded_panels(root.clone());
    let file_index = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == file_path)
        .expect("edit target should be visible");
    app.active_panel_mut().cursor = file_index;

    assert_eq!(
        app.open_selected_file_in_editor_with_resolver(|| None),
        EditSelectionResult::NoEditorResolved
    );
    assert!(
        app.take_pending_external_edit_requests().is_empty(),
        "no external editor request should be queued"
    );
    assert!(
        app.take_pending_worker_commands().is_empty(),
        "edit without an external editor should not queue a viewer fallback"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}
#[test]
fn viewer_supports_scroll_search_goto_and_wrap() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-viewer-actions-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let file_path = root.join("viewer.txt");
    fs::write(&file_path, "first\nsecond target\nthird\nfourth target\n")
        .expect("must create viewer content");

    let mut app = app_with_loaded_panels(root.clone());
    let file_index = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == file_path)
        .expect("viewer file should be visible");
    app.active_panel_mut().cursor = file_index;
    app.apply(AppCommand::OpenEntry)
        .expect("open entry should open viewer");
    drain_background(&mut app);

    app.apply(AppCommand::ViewerMoveDown)
        .expect("viewer should move down");
    let Route::Viewer(viewer) = app.top_route() else {
        panic!("top route should be viewer");
    };
    assert_eq!(viewer.current_line_number(), 2);

    app.apply(AppCommand::ViewerToggleWrap)
        .expect("viewer should toggle wrap");
    let Route::Viewer(viewer) = app.top_route() else {
        panic!("top route should be viewer");
    };
    assert!(viewer.wrap, "wrap should be enabled");

    app.apply(AppCommand::ViewerGoto)
        .expect("viewer goto should open dialog");
    app.apply(AppCommand::DialogBackspace)
        .expect("should edit goto target");
    for ch in "3".chars() {
        app.apply(AppCommand::DialogInputChar(ch))
            .expect("typing goto target should succeed");
    }
    app.apply(AppCommand::DialogAccept)
        .expect("goto dialog should submit");
    let Route::Viewer(viewer) = app.top_route() else {
        panic!("top route should be viewer");
    };
    assert_eq!(viewer.current_line_number(), 3);

    app.apply(AppCommand::ViewerSearchForward)
        .expect("search should open dialog");
    for ch in "target".chars() {
        app.apply(AppCommand::DialogInputChar(ch))
            .expect("typing search query should succeed");
    }
    app.apply(AppCommand::DialogAccept)
        .expect("search dialog should submit");
    let Route::Viewer(viewer) = app.top_route() else {
        panic!("top route should be viewer");
    };
    assert_eq!(viewer.current_line_number(), 4);

    app.apply(AppCommand::ViewerSearchContinueBackward)
        .expect("reverse continue search should run");
    let Route::Viewer(viewer) = app.top_route() else {
        panic!("top route should be viewer");
    };
    assert_eq!(viewer.current_line_number(), 2);

    app.apply(AppCommand::CloseViewer)
        .expect("viewer should close");
    assert_eq!(app.key_context(), KeyContext::FileManager);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn viewer_hex_mode_switches_context_and_navigation_model() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-viewer-hex-mode-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let file_path = root.join("hex.bin");
    fs::write(
        &file_path,
        b"0123456789abcdef0123456789abcdef0123456789abcdef",
    )
    .expect("must create viewer content");

    let mut app = app_with_loaded_panels(root.clone());
    let file_index = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == file_path)
        .expect("viewer file should be visible");
    app.active_panel_mut().cursor = file_index;
    app.apply(AppCommand::OpenEntry)
        .expect("open entry should open viewer");
    drain_background(&mut app);
    assert_eq!(app.key_context(), KeyContext::Viewer);

    app.apply(AppCommand::ViewerToggleHex)
        .expect("viewer should toggle hex mode");
    assert_eq!(app.key_context(), KeyContext::ViewerHex);
    let Route::Viewer(viewer) = app.top_route() else {
        panic!("top route should be viewer");
    };
    assert_eq!(
        viewer.line_count(),
        3,
        "48 bytes should render as 3 hex rows"
    );

    app.apply(AppCommand::ViewerMoveDown)
        .expect("viewer should move by hex row");
    let Route::Viewer(viewer) = app.top_route() else {
        panic!("top route should be viewer");
    };
    assert_eq!(viewer.current_line_number(), 2);

    app.apply(AppCommand::ViewerToggleHex)
        .expect("viewer should toggle back to text mode");
    assert_eq!(app.key_context(), KeyContext::Viewer);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn viewer_opens_binary_content_in_hex_mode_by_default() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-viewer-binary-default-hex-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let file_path = root.join("payload.bin");
    fs::write(&file_path, b"\x00\x1b\x7fBIN\x01\x02").expect("must create binary file");

    let mut app = app_with_loaded_panels(root.clone());
    let file_index = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == file_path)
        .expect("binary file should be visible");
    app.active_panel_mut().cursor = file_index;
    app.apply(AppCommand::OpenEntry)
        .expect("open entry should queue viewer");
    drain_background(&mut app);

    assert_eq!(
        app.key_context(),
        KeyContext::ViewerHex,
        "binary files should open in hex mode"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn viewer_state_fingerprints_track_path_and_content() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-viewer-fingerprints-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let first_path = root.join("first.txt");
    let second_path = root.join("second.txt");
    let third_path = root.join("third.txt");
    fs::write(&first_path, "abc").expect("first fixture should be writable");
    fs::write(&second_path, "abc").expect("second fixture should be writable");
    fs::write(&third_path, "xyz").expect("third fixture should be writable");

    let first = ViewerState::open(first_path).expect("first viewer fixture should open");
    let second = ViewerState::open(second_path).expect("second viewer fixture should open");
    let third = ViewerState::open(third_path).expect("third viewer fixture should open");

    assert_eq!(
        first.content_fingerprint(),
        second.content_fingerprint(),
        "matching content should reuse the same content fingerprint"
    );
    assert_ne!(
        first.path_fingerprint(),
        second.path_fingerprint(),
        "different file paths should produce distinct path fingerprints"
    );
    assert_ne!(
        first.content_fingerprint(),
        third.content_fingerprint(),
        "different content with the same length should produce distinct fingerprints"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn viewer_state_uses_preview_mode_for_large_text_files() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-viewer-preview-large-text-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let file_path = root.join("large.txt");
    let total_bytes = VIEWER_TEXT_PREVIEW_LIMIT_BYTES + 1024;
    fs::write(&file_path, vec![b'a'; total_bytes]).expect("large fixture should be writable");

    let viewer = ViewerState::open(file_path).expect("large viewer fixture should open");
    assert!(
        viewer.text_is_preview(),
        "large text file should be previewed"
    );
    assert_eq!(
        viewer.content().len(),
        VIEWER_TEXT_PREVIEW_LIMIT_BYTES,
        "viewer content should be capped at preview limit"
    );
    assert!(
        viewer.hex_mode,
        "preview mode should default to hex context"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn viewer_state_reads_content_when_reported_size_is_zero() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-viewer-zero-reported-size-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let file_path = root.join("virtual-ish.txt");
    fs::write(&file_path, "virtual content\n").expect("fixture should be writable");

    let viewer = ViewerState::open_with_reported_size_for_test(file_path, 0)
        .expect("viewer fixture should open even when metadata under-reports size");
    assert_eq!(
        viewer.content(),
        "virtual content\n",
        "viewer should not derive the read cap solely from metadata length"
    );
    assert!(
        !viewer.text_is_preview(),
        "short content should not be marked as preview just because metadata under-reported"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn viewer_state_preview_fingerprint_includes_total_size() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-viewer-preview-fingerprint-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let first_path = root.join("first.txt");
    let second_path = root.join("second.txt");

    let mut first_bytes = vec![b'x'; VIEWER_TEXT_PREVIEW_LIMIT_BYTES + 1];
    let second_bytes = vec![b'x'; VIEWER_TEXT_PREVIEW_LIMIT_BYTES + 32];
    first_bytes[VIEWER_TEXT_PREVIEW_LIMIT_BYTES] = b'y';
    fs::write(&first_path, first_bytes).expect("first fixture should be writable");
    fs::write(&second_path, second_bytes).expect("second fixture should be writable");

    let first = ViewerState::open(first_path).expect("first preview fixture should open");
    let second = ViewerState::open(second_path).expect("second preview fixture should open");

    assert!(first.text_is_preview());
    assert!(second.text_is_preview());
    assert_eq!(
        first.content(),
        second.content(),
        "previewed text should match when prefixes are identical"
    );
    assert_ne!(
        first.content_fingerprint(),
        second.content_fingerprint(),
        "preview fingerprints should diverge when total file size differs"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn opening_large_text_file_reports_preview_mode_status() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-viewer-preview-status-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let file_path = root.join("large.txt");
    let total_bytes = VIEWER_TEXT_PREVIEW_LIMIT_BYTES + 1;
    fs::write(&file_path, vec![b'a'; total_bytes]).expect("large fixture should be writable");

    let mut app = app_with_loaded_panels(root.clone());
    let file_index = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == file_path)
        .expect("large file should be visible");
    app.active_panel_mut().cursor = file_index;
    app.apply(AppCommand::OpenEntry)
        .expect("open entry should queue viewer");
    drain_background(&mut app);

    assert_eq!(app.key_context(), KeyContext::ViewerHex);
    assert!(
        app.status_line.contains("(text preview mode)"),
        "status should communicate preview mode for large files"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}
