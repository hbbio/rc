use super::*;

#[cfg(unix)]
fn fixture() -> (tempfile::TempDir, AppState) {
    let root = tempfile::tempdir().unwrap();
    let left = root.path().join("left");
    let right = root.path().join("right");
    fs::create_dir(&left).unwrap();
    fs::create_dir(&right).unwrap();
    fs::write(left.join("file.txt"), "before\n").unwrap();
    fs::write(right.join("file.txt"), "after\n").unwrap();
    let mut app = app_with_loaded_panels(left);
    app.panels[1].cwd = right;
    app.panels[1].refresh().unwrap();
    for panel in &mut app.panels {
        panel.cursor = panel
            .entries
            .iter()
            .position(|entry| entry.name == "file.txt")
            .unwrap();
    }
    (root, app)
}

#[test]
#[cfg(unix)]
fn file_comparison_reports_differences_as_success_and_handles_missing_files() {
    let (_root, mut app) = fixture();
    app.apply(AppCommand::CompareFiles).unwrap();
    let Route::CommandOutput(id) = *app.top_route() else {
        panic!("output screen")
    };
    drain_background(&mut app);
    assert!(
        app.active_viewer()
            .unwrap()
            .title()
            .contains("Differences found")
    );
    assert!(!app.active_viewer().unwrap().bytes.is_empty());
    assert_eq!(app.jobs.job(id).unwrap().status, JobStatus::Succeeded);
    app.apply(AppCommand::CloseViewer).unwrap();
    fs::remove_file(app.passive_panel().selected_entry().unwrap().path.clone()).unwrap();
    app.apply(AppCommand::CompareFiles).unwrap();
    let Route::CommandOutput(id) = *app.top_route() else {
        panic!("output screen")
    };
    drain_background(&mut app);
    assert_eq!(app.jobs.job(id).unwrap().status, JobStatus::Failed);
}

#[test]
#[cfg(unix)]
fn directory_comparison_finds_nested_changes_and_equal_directories() {
    let (_root, mut app) = fixture();
    for panel in &app.panels {
        fs::write(panel.cwd.join("file.txt"), "same\n").unwrap();
        fs::create_dir(panel.cwd.join("nested")).unwrap();
    }
    fs::write(app.panels[0].cwd.join("nested/unique.txt"), "unique").unwrap();
    app.apply(AppCommand::CompareDirectories).unwrap();
    drain_background(&mut app);
    assert!(
        app.active_viewer()
            .unwrap()
            .content()
            .contains("unique.txt")
    );
    assert!(
        app.active_viewer()
            .unwrap()
            .title()
            .contains("Differences found")
    );
    app.apply(AppCommand::CloseViewer).unwrap();
    fs::remove_file(app.panels[0].cwd.join("nested/unique.txt")).unwrap();
    app.apply(AppCommand::CompareDirectories).unwrap();
    drain_background(&mut app);
    assert!(
        app.active_viewer()
            .unwrap()
            .title()
            .contains("No differences")
    );
}

#[test]
fn comparison_shortcuts_use_the_extended_keymap() {
    let keymap = Keymap::bundled_mc_default().unwrap();
    for (ctrl, command) in [
        (false, AppCommand::CompareDirectories),
        (true, AppCommand::CompareFiles),
    ] {
        let chord = KeyChord {
            code: KeyCode::Char('d'),
            modifiers: KeyModifiers {
                ctrl,
                alt: false,
                shift: false,
            },
        };
        let key = keymap.resolve(KeyContext::FileManagerXMap, chord).unwrap();
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManagerXMap, key),
            Some(command)
        );
    }
}
