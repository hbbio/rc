#![cfg(unix)]
use super::*;
use rc_shell::{decode_path_text, encode_path_text};
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

fn native_app(root: &Path) -> (AppState, PathBuf) {
    let mut app = app_with_loaded_panels(root.to_owned());
    let path = root.join(OsString::from_vec(b"raw\xff\\xFF.rs".to_vec()));
    app.panels[0].entries = vec![FileEntry::file(
        encode_path_text(path.file_name().unwrap()),
        path.clone(),
        0,
        None,
    )];
    app.panels[0].cursor = 0;
    app.settings.confirmation.confirm_overwrite = false;
    (app, path)
}

#[test]
fn rename_dialog_defaults_and_submitted_names_preserve_native_paths() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, path) = native_app(root.path());
    app.apply(AppCommand::OpenConfirmDialog).unwrap();
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("rename dialog");
    };
    let DialogKind::Input(input) = &dialog.kind else {
        panic!("rename input");
    };
    assert_eq!(decode_path_text(&input.value), path.file_name().unwrap());
    let value = input.value.clone();
    app.finish_dialog(DialogResult::InputSubmitted(value));
    assert!(app.status_line.contains("unchanged"));
    app.apply(AppCommand::OpenConfirmDialog).unwrap();
    app.finish_dialog(DialogResult::InputSubmitted(r"renamed\xFE.rs".into()));
    let requests = app.take_pending_worker_commands();
    assert!(requests.iter().any(|request| matches!(request, WorkerCommand::Run(job)
        if matches!(&job.request, JobRequest::Rename { source, destination }
            if source == &path && destination.file_name() == Some(OsString::from_vec(b"renamed\xfe.rs".to_vec()).as_os_str())))));
}

#[test]
fn move_dialog_passes_native_destination_names_to_jobs() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, path) = native_app(root.path());
    let destination = root.path().join(OsString::from_vec(b"folder\xfe".to_vec()));
    app.panels[1].cwd = destination.clone();
    app.apply(AppCommand::Move).unwrap();
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("move dialog");
    };
    let DialogKind::PairInput(input) = &dialog.kind else {
        panic!("move input");
    };
    assert_eq!(
        PathBuf::from(decode_path_text(&input.first_value)),
        destination
    );
    assert_eq!(
        decode_path_text(&input.second_value),
        path.file_name().unwrap()
    );
    app.finish_dialog(DialogResult::PairInputSubmitted {
        first: input.first_value.clone(),
        second: r"renamed\xFD.rs".into(),
    });
    assert!(app.take_pending_worker_commands().iter().any(|request| matches!(request, WorkerCommand::Run(job)
        if matches!(&job.request, JobRequest::Move { sources, destination_dir, destination_names: Some(names), .. }
            if sources == &vec![path.clone()] && destination_dir == &destination
                && names == &vec![OsString::from_vec(b"renamed\xfd.rs".to_vec())]))));
}

#[test]
fn utf8_panel_spelling_distinguishes_invalid_bytes_from_literal_escape_text() {
    let raw = OsString::from_vec(b"raw\xff".to_vec());
    let literal = OsString::from(r"raw\xFF");
    assert_eq!(FilenameEncoding::Utf8.decode(&raw), r"raw\xFF");
    assert_eq!(FilenameEncoding::Utf8.decode(&literal), r"raw\\xFF");
}

#[test]
fn view_file_and_find_dialog_decode_native_paths() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, path) = native_app(root.path());
    app.apply(AppCommand::ViewFile).unwrap();
    app.finish_dialog(DialogResult::InputSubmitted(encode_path_text(
        path.as_os_str(),
    )));
    assert!(
        app.take_pending_worker_commands()
            .iter()
            .any(|request| matches!(request, WorkerCommand::Run(job)
        if matches!(&job.request, JobRequest::LoadViewer { path: actual } if actual == &path)))
    );
    let spec = FindSpec::new(path.clone());
    let form = crate::dialog::FindDialogState::from_spec(&spec);
    assert_eq!(form.to_spec().start_dir, path);
}
