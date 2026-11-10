use super::*;

#[cfg(unix)]
fn start_capture(app: &mut AppState, command: &str) -> JobId {
    app.apply(AppCommand::FilteredView).unwrap();
    app.finish_dialog(DialogResult::InputSubmitted(command.into()));
    let Route::CommandOutput(id) = app.top_route() else {
        panic!("output screen expected")
    };
    *id
}

#[test]
#[cfg(unix)]
fn filtered_view_runs_in_panel_directory_and_retains_output_after_close() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("fixture"), "from file\n").unwrap();
    let mut app = app_with_loaded_panels(root.path().to_path_buf());
    app.settings.shell = ShellSettings::custom(
        "/bin/sh",
        ShellDialect::Posix,
        None,
        ShellHistoryMode::Session,
    )
    .unwrap();
    let id = start_capture(&mut app, "cat fixture; printf diagnostic >&2; exit 7");
    app.apply(AppCommand::CloseViewer).unwrap();
    assert!(matches!(app.top_route(), Route::FileManager));
    drain_background(&mut app);
    assert!(
        app.command_outputs[&id]
            .viewer
            .content()
            .contains("from file")
    );
    assert!(
        app.command_outputs[&id]
            .viewer
            .content()
            .contains("diagnostic")
    );
    assert!(app.command_outputs[&id].viewer.title().contains('7'));
    assert!(app.file_history.is_empty());
    app.apply(AppCommand::OpenJobsScreen).unwrap();
    app.jobs_cursor = app.jobs.jobs().iter().position(|job| job.id == id).unwrap();
    app.apply(AppCommand::OpenJobOutput).unwrap();
    assert_eq!(
        app.active_viewer().unwrap().bytes,
        app.command_outputs[&id].viewer.bytes
    );
    app.apply(AppCommand::ViewerSearchForward).unwrap();
    app.finish_dialog(DialogResult::InputSubmitted("diagnostic".into()));
    assert!(app.status_line.contains("Search hit"));
}

#[test]
#[cfg(unix)]
fn output_cancel_targets_its_command_and_dispatch_failure_completes_screen() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app_with_loaded_panels(root.path().to_path_buf());
    let id = start_capture(&mut app, "sleep 30");
    let later = app.queue_worker_job_request(JobRequest::LoadViewer {
        path: root.path().join("missing"),
    });
    app.apply(AppCommand::CancelJob).unwrap();
    let commands = app.take_pending_worker_commands();
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, WorkerCommand::Cancel(job) if *job == id))
    );
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, WorkerCommand::Cancel(job) if *job == later))
    );
    app.handle_job_dispatch_failure(id, JobError::dispatch("runtime unavailable"));
    assert!(
        app.command_outputs[&id]
            .viewer
            .title()
            .contains("runtime unavailable")
    );
}

#[test]
#[cfg(unix)]
fn job_failure_before_buffered_output_does_not_split_utf8() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app_with_loaded_panels(root.path().to_path_buf());
    let id = start_capture(&mut app, "command");
    app.append_command_output(id, &[0xe2, 0x82]);
    app.handle_job_event(JobEvent::Finished {
        id,
        result: Err(JobError::from_message("exit 1")),
    });
    app.append_command_output(id, &[0xac]);
    app.complete_command_output(id, "exit 1".into());
    assert_eq!(app.active_viewer().unwrap().content(), "€");
}

#[test]
fn streamed_viewer_preserves_split_utf8_line_indexes_and_search() {
    let mut viewer = ViewerState::command_output("test".into());
    viewer.append_output(b"first\n");
    viewer.append_output(&[0xe2, 0x82]);
    viewer.append_output(&[0xac, b'\n', b'x', 0xff]);
    assert_eq!(viewer.content(), "first\n€\nx�");
    assert_eq!(viewer.text_line(0), Some("first"));
    assert_eq!(viewer.text_line(1), Some("€"));
    assert_eq!(viewer.text_line(2), Some("x�"));
    viewer.append_output(&[0xe2]);
    viewer.finish_output("exit status: 0".into());
    assert_eq!(viewer.content(), "first\n€\nx��");
    assert_eq!(viewer.line_count(), 3);
    viewer.toggle_hex_mode();
    assert_eq!(viewer.bytes, b"first\n\xe2\x82\xac\nx\xff\xe2");
}

#[test]
fn repeated_commands_with_different_equal_length_output_have_distinct_cache_keys() {
    let mut first = ViewerState::command_output("cat fixture".into());
    let mut second = ViewerState::command_output("cat fixture".into());
    first.append_output(b"first");
    second.append_output(b"other");
    assert_eq!(first.path_fingerprint(), second.path_fingerprint());
    assert_eq!(first.content().len(), second.content().len());
    assert_ne!(first.content_fingerprint(), second.content_fingerprint());
    let mut fragmented = ViewerState::command_output("cat fixture".into());
    fragmented.append_output(b"fi");
    fragmented.append_output(b"rst");
    assert_eq!(
        first.content_fingerprint(),
        fragmented.content_fingerprint()
    );
}

#[test]
#[cfg(unix)]
fn output_history_evicts_finished_entries_but_keeps_running_commands() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app_with_loaded_panels(root.path().to_path_buf());
    let oldest = start_capture(&mut app, "first");
    app.apply(AppCommand::CloseViewer).unwrap();
    for _ in 0..7 {
        start_capture(&mut app, "other");
        app.apply(AppCommand::CloseViewer).unwrap();
    }
    app.apply(AppCommand::FilteredView).unwrap();
    app.finish_dialog(DialogResult::InputSubmitted("too many".into()));
    assert!(matches!(app.top_route(), Route::FileManager));
    assert_eq!(app.command_outputs.len(), 8);
    app.complete_command_output(oldest, "finished".into());
    start_capture(&mut app, "replacement");
    assert!(!app.command_outputs.contains_key(&oldest));
    assert_eq!(app.command_outputs.len(), 8);
}
