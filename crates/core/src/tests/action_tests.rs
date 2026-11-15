use super::*;
use crate::action_macros::ViewOptions;

fn configured_app(root: &Path, body: &str) -> AppState {
    fs::write(root.join("one.txt"), "first\n").unwrap();
    fs::write(root.join("two.txt"), "second\n").unwrap();
    let config = root.join("config");
    fs::create_dir_all(&config).unwrap();
    fs::write(
        config.join("mc.ext.ini"),
        format!("[mc.ext.ini]\nVersion=4.0\n{body}"),
    )
    .unwrap();
    let mut app = app_with_loaded_panels(root.to_path_buf());
    app.set_configuration_files(ConfigurationFiles {
        user_directory: Some(config),
        system_directories: vec![],
    });
    select(&mut app, "one.txt");
    app
}

fn select(app: &mut AppState, name: &str) {
    app.panels[0].cursor = app.panels[0]
        .entries
        .iter()
        .position(|entry| entry.name == name)
        .unwrap();
}

#[cfg(unix)]
#[test]
fn extension_open_and_edit_dispatch_native_arguments_and_helper_environment() {
    let root = tempfile::tempdir().unwrap();
    let mut app = configured_app(
        root.path(),
        "[text]\nShell=.txt\nOpen=printf '%%s' %f\nEdit=printf '%%s' %p",
    );
    app.apply(AppCommand::OpenEntry).unwrap();
    let request = app.take_pending_external_action_requests().remove(0);
    assert_eq!(request.cwd, root.path());
    let output = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(&request.script)
        .envs(request.environment.iter().map(|(key, value)| (key, value)))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"one.txt");
    assert_eq!(
        request
            .environment
            .iter()
            .find(|(key, _)| key == "MC_EXT_FILENAME")
            .unwrap()
            .1,
        root.path().join("one.txt")
    );
    assert_eq!(
        request
            .environment
            .iter()
            .find(|(key, _)| key == "MC_EXT_SELECTED")
            .unwrap()
            .1,
        "'one.txt'"
    );
    app.apply(AppCommand::EditEntry).unwrap();
    let request = app.take_pending_external_action_requests().remove(0);
    assert_eq!(request.edited_path, Some(root.path().join("one.txt")));
    assert!(app.take_pending_external_edit_requests().is_empty());
}

#[cfg(unix)]
#[test]
fn extension_view_streams_output_and_untags_only_after_scheduling() {
    let root = tempfile::tempdir().unwrap();
    let mut app = configured_app(
        root.path(),
        r"[text]
Shell=.txt
View=%view{ascii} printf '%%s\\n' %u",
    );
    app.panels[0].tagged.insert(root.path().join("one.txt"));
    app.panels[0].tagged.insert(root.path().join("two.txt"));
    // Tags hidden by a listing filter still belong to file-operation macros.
    app.panels[0]
        .entries
        .retain(|entry| entry.name != "two.txt");
    app.apply(AppCommand::ViewEntry).unwrap();
    assert!(app.active_panel().tagged_paths().is_empty());
    drain_background(&mut app);
    assert_eq!(app.active_viewer().unwrap().content(), "one.txt\ntwo.txt\n");
    assert!(!app.active_viewer().unwrap().hex_mode);
}

#[test]
fn bare_view_honors_hex_and_nroff_without_altering_raw_bytes() {
    let root = tempfile::tempdir().unwrap();
    let mut app = configured_app(root.path(), "[text]\nShell=.txt\nView=%view{hex,nroff}");
    let bytes = b"_\x08H\x1b[1mi\x1b[0m\n";
    fs::write(root.path().join("one.txt"), bytes).unwrap();
    app.apply(AppCommand::ViewEntry).unwrap();
    drain_background(&mut app);
    let viewer = app.active_viewer().unwrap();
    assert_eq!(viewer.bytes, bytes);
    assert_eq!(viewer.content(), "Hi\n");
    assert!(viewer.hex_mode);
    assert_eq!(viewer.path(), root.path().join("one.txt"));
    assert_eq!(app.file_history.len(), 1);
}

#[test]
fn action_prompts_are_sequential_and_cancel_preserves_tags() {
    let root = tempfile::tempdir().unwrap();
    let mut app = configured_app(
        root.path(),
        "[text]\nShell=.txt\nOpen=printf '%%s' %{First} %{Second} %u",
    );
    app.panels[0].tagged.insert(root.path().join("one.txt"));
    app.apply(AppCommand::OpenEntry).unwrap();
    assert!(matches!(app.top_route(), Route::Dialog(_)));
    app.finish_dialog(DialogResult::InputSubmitted("answer".into()));
    assert!(matches!(app.top_route(), Route::Dialog(_)));
    assert!(app.take_pending_external_action_requests().is_empty());
    app.finish_dialog(DialogResult::Canceled);
    assert_eq!(app.active_panel().tagged_count(), 1);
    assert!(app.take_pending_external_action_requests().is_empty());
    app.apply(AppCommand::OpenEntry).unwrap();
    app.finish_dialog(DialogResult::InputSubmitted("first".into()));
    app.finish_dialog(DialogResult::InputSubmitted("second".into()));
    assert_eq!(app.take_pending_external_action_requests().len(), 1);
    assert_eq!(app.active_panel().tagged_count(), 0);
}

#[test]
fn extension_probe_ignores_changed_selection_cancellation_and_stale_results() {
    let root = tempfile::tempdir().unwrap();
    let mut app = configured_app(root.path(), "[text]\nType=text\nOpen=printf '%%s' %f");
    app.apply(AppCommand::OpenEntry).unwrap();
    let first = app.pending_extension.as_ref().unwrap().0;
    select(&mut app, "two.txt");
    app.handle_background_event(BackgroundEvent::FileTypeProbed {
        job_id: first,
        result: Ok(Some(b"ASCII text".to_vec())),
    });
    assert!(app.take_pending_external_action_requests().is_empty());
    app.apply(AppCommand::OpenEntry).unwrap();
    let canceled = app.pending_extension.as_ref().unwrap().0;
    app.request_cancel_for_job(canceled);
    app.handle_background_event(BackgroundEvent::FileTypeProbed {
        job_id: canceled,
        result: Ok(Some(b"ASCII text".to_vec())),
    });
    assert!(app.take_pending_external_action_requests().is_empty());
    app.apply(AppCommand::OpenEntry).unwrap();
    let newest = app.pending_extension.as_ref().unwrap().0;
    app.handle_background_event(BackgroundEvent::FileTypeProbed {
        job_id: first,
        result: Ok(Some(b"ASCII text".to_vec())),
    });
    assert_eq!(app.pending_extension.as_ref().unwrap().0, newest);
    app.handle_job_event(JobEvent::Finished {
        id: newest,
        result: Ok(()),
    });
    app.handle_background_event(BackgroundEvent::FileTypeProbed {
        job_id: newest,
        result: Ok(Some(b"ASCII text".to_vec())),
    });
    assert_eq!(app.take_pending_external_action_requests().len(), 1);
}

#[test]
fn unavailable_file_type_uses_name_rules_and_missing_action_uses_builtin_viewer() {
    let root = tempfile::tempdir().unwrap();
    let mut app = configured_app(root.path(), "[text]\nType=text\nShell=.txt\nOpen=echo %f");
    app.apply(AppCommand::ViewEntry).unwrap();
    let id = app.pending_extension.as_ref().unwrap().0;
    app.handle_background_event(BackgroundEvent::FileTypeProbed {
        job_id: id,
        result: Ok(None),
    });
    assert!(app.take_pending_worker_commands().iter().any(|command| matches!(command, WorkerCommand::Run(job) if matches!(job.request, JobRequest::LoadViewer { .. }))));
}

#[cfg(unix)]
#[test]
fn real_file_type_probe_matches_an_end_anchored_pattern() {
    let root = tempfile::tempdir().unwrap();
    if rc_shell::find_program_on_path(std::ffi::OsStr::new("file"), root.path()).is_none() {
        return;
    }
    let mut app = configured_app(
        root.path(),
        "[text]\nType=text$\nOpen=echo matched\n[Default]\nOpen=echo fallback",
    );
    app.apply(AppCommand::OpenEntry).unwrap();
    drain_background(&mut app);
    assert_eq!(
        app.take_pending_external_action_requests()[0].script,
        "echo matched"
    );
}

#[test]
fn invalid_extension_edits_stop_actions_until_corrected() {
    let root = tempfile::tempdir().unwrap();
    let mut app = configured_app(root.path(), "[text]\nShell=.txt\nOpen=echo old");
    let config = root.path().join("config/mc.ext.ini");
    fs::write(&config, "[bad]\nOpen=echo bad").unwrap();
    app.reload_extension_rules();
    app.apply(AppCommand::OpenEntry).unwrap();
    assert!(app.status_line.contains("Cannot use mc.ext.ini"));
    assert!(app.take_pending_external_action_requests().is_empty());
    assert!(app.take_pending_worker_commands().is_empty());
    fs::write(
        config,
        "[mc.ext.ini]\nVersion=4.0\n[Default]\nOpen=echo new",
    )
    .unwrap();
    app.reload_extension_rules();
    app.apply(AppCommand::OpenEntry).unwrap();
    assert_eq!(
        app.take_pending_external_action_requests()[0].script,
        "echo new"
    );
}

#[test]
fn reloading_rules_invalidates_even_a_completed_probe_with_queued_results() {
    let root = tempfile::tempdir().unwrap();
    let mut app = configured_app(root.path(), "[text]\nType=text\nOpen=echo old");
    app.apply(AppCommand::OpenEntry).unwrap();
    let id = app.pending_extension.as_ref().unwrap().0;
    app.handle_job_event(JobEvent::Finished { id, result: Ok(()) });
    fs::write(
        root.path().join("config/mc.ext.ini"),
        "[mc.ext.ini]\nVersion=4.0\n[Default]\nOpen=echo new",
    )
    .unwrap();
    app.reload_extension_rules();
    app.handle_background_event(BackgroundEvent::FileTypeProbed {
        job_id: id,
        result: Ok(Some(b"ASCII text".to_vec())),
    });
    assert!(app.take_pending_external_action_requests().is_empty());
    app.apply(AppCommand::OpenEntry).unwrap();
    assert_eq!(
        app.take_pending_external_action_requests()[0].script,
        "echo new"
    );
}

#[test]
fn directory_rules_change_literal_paths_without_invoking_a_shell() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("source")).unwrap();
    fs::create_dir(root.path().join("destination dir")).unwrap();
    let mut app = configured_app(
        root.path(),
        "[directory]\nDirectory=/source$\nOpen=%cd destination dir\n[Default]\nOpen=echo should-not-run",
    );
    select(&mut app, "source");
    app.apply(AppCommand::OpenEntry).unwrap();
    drain_background(&mut app);
    assert_eq!(app.active_panel().cwd, root.path().join("destination dir"));
    assert!(app.take_pending_external_action_requests().is_empty());
}

#[test]
fn output_capacity_failure_preserves_untag_macro_targets() {
    let root = tempfile::tempdir().unwrap();
    let mut app = configured_app(root.path(), "[text]\nShell=.txt\nView=%view echo %u");
    for _ in 0..8 {
        app.start_command_capture(CommandCaptureSpec::shell(
            root.path().into(),
            "true".into(),
            app.settings.shell.clone(),
        ));
        app.apply(AppCommand::CloseViewer).unwrap();
    }
    app.panels[0].tagged.insert(root.path().join("one.txt"));
    app.apply(AppCommand::ViewEntry).unwrap();
    assert_eq!(app.active_panel().tagged_count(), 1);
    assert!(app.status_line.contains("Close or finish"));
}

#[test]
fn nroff_output_handles_overstrikes_and_escapes_across_chunks() {
    let mut viewer = ViewerState::command_output("nroff".into());
    viewer.apply_action_view_options(ViewOptions {
        hex: false,
        nroff: true,
    });
    viewer.append_output(b"__");
    let before = viewer.content_fingerprint();
    viewer.append_output(b"\x08\x08");
    assert_ne!(viewer.content_fingerprint(), before);
    viewer.append_output(b"H\x1b[");
    viewer.append_output(b"1mi\x1b[0m\nB\n");
    viewer.append_output(b"C");
    assert_eq!(viewer.content(), "Hi\nB\nC");
    assert_eq!(viewer.text_line(1), Some("B"));
    assert_eq!(viewer.text_line(2), Some("C"));
    assert!(viewer.bytes.contains(&8));
}
