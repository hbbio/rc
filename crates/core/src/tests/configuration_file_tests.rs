use super::*;

fn files(root: &Path) -> ConfigurationFiles {
    ConfigurationFiles {
        user_directory: Some(root.join("user")),
        system_directories: vec![root.join("system")],
    }
}

#[test]
fn configuration_edit_copies_system_default_without_changing_it() {
    let root = tempfile::tempdir().unwrap();
    let files = files(root.path());
    fs::create_dir(root.path().join("system")).unwrap();
    let source = root.path().join("system/mc.ext.ini");
    fs::write(&source, "[mc.ext.ini]\nVersion=4.0\n").unwrap();
    let copied = files
        .prepare_for_edit(ConfigurationFile::Extensions, None)
        .unwrap();
    assert_eq!(copied, root.path().join("user/mc.ext.ini"));
    assert_eq!(fs::read(&copied).unwrap(), fs::read(&source).unwrap());
    fs::write(&copied, "user edit").unwrap();
    assert_eq!(
        files.read(ConfigurationFile::Extensions, None).unwrap(),
        "user edit"
    );
    assert_eq!(
        files
            .prepare_for_edit(ConfigurationFile::Extensions, None)
            .unwrap(),
        copied
    );
    assert_eq!(fs::read_to_string(&copied).unwrap(), "user edit");
    assert_eq!(
        fs::read_to_string(source).unwrap(),
        "[mc.ext.ini]\nVersion=4.0\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(copied).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn all_configuration_commands_enqueue_editor_with_existing_file_and_valid_cwd() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app_with_loaded_panels(root.path().to_path_buf());
    app.set_configuration_files(files(root.path()));
    app.settings_mut().configuration.editor_command = Some("my-editor --wait".into());
    for file in [
        ConfigurationFile::Extensions,
        ConfigurationFile::Menu,
        ConfigurationFile::Highlighting,
    ] {
        app.apply(AppCommand::EditConfigurationFile(file)).unwrap();
        let requests = app.take_pending_external_edit_requests();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.editor_command, "my-editor --wait");
        assert_eq!(
            request.path,
            root.path().join("user").join(file.user_filename())
        );
        assert_eq!(request.cwd, root.path().join("user"));
        assert_eq!(fs::read_to_string(&request.path).unwrap(), file.bundled());
        assert!(
            top_menus()
                .iter()
                .flat_map(|menu| menu.entries)
                .any(|entry| entry.command == AppCommand::EditConfigurationFile(file))
        );
    }
}

#[cfg(unix)]
#[test]
fn local_menu_requires_trusted_regular_file_and_overrides_user_menu() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = tempfile::tempdir().unwrap();
    let files = files(root.path());
    let user = files
        .prepare_for_edit(ConfigurationFile::Menu, None)
        .unwrap();
    let local = root.path().join(".mc.menu");
    fs::write(&local, "local").unwrap();
    fs::set_permissions(&local, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        files
            .read(ConfigurationFile::Menu, Some(root.path()))
            .unwrap(),
        "local"
    );
    assert_eq!(
        files
            .prepare_for_edit(ConfigurationFile::Menu, Some(root.path()))
            .unwrap(),
        local
    );
    fs::set_permissions(&local, fs::Permissions::from_mode(0o666)).unwrap();
    assert_eq!(
        files
            .prepare_for_edit(ConfigurationFile::Menu, Some(root.path()))
            .unwrap(),
        user
    );
    fs::remove_file(&local).unwrap();
    symlink(&user, &local).unwrap();
    assert_eq!(
        files
            .prepare_for_edit(ConfigurationFile::Menu, Some(root.path()))
            .unwrap(),
        user
    );
}

#[test]
fn invalid_configuration_override_is_an_error_and_not_replaced() {
    let root = tempfile::tempdir().unwrap();
    let files = files(root.path());
    let path = root.path().join("user/mc.ext.ini");
    fs::create_dir_all(&path).unwrap();
    assert!(files.read(ConfigurationFile::Extensions, None).is_err());
    assert!(
        files
            .prepare_for_edit(ConfigurationFile::Extensions, None)
            .is_err()
    );
    assert!(path.is_dir());
    fs::remove_dir(&path).unwrap();
    fs::write(&path, vec![b'a'; 1024 * 1024 + 1]).unwrap();
    assert!(files.read(ConfigurationFile::Extensions, None).is_err());
    fs::write(&path, [0xff]).unwrap();
    assert!(files.read(ConfigurationFile::Extensions, None).is_err());
}

#[test]
fn missing_user_directory_reports_edit_error_without_launching_editor() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app_with_loaded_panels(root.path().to_path_buf());
    app.set_configuration_files(ConfigurationFiles {
        user_directory: None,
        system_directories: vec![],
    });
    app.apply(AppCommand::EditConfigurationFile(
        ConfigurationFile::Extensions,
    ))
    .unwrap();
    assert!(app.take_pending_external_edit_requests().is_empty());
    assert!(
        app.status_line
            .contains("configuration directory is unavailable")
    );
}

#[test]
fn standard_user_menu_precedes_legacy_alias_and_system_menu() {
    let root = tempfile::tempdir().unwrap();
    let files = files(root.path());
    fs::create_dir(root.path().join("user")).unwrap();
    fs::create_dir(root.path().join("system")).unwrap();
    fs::write(root.path().join("system/mc.menu"), "system").unwrap();
    assert_eq!(files.read(ConfigurationFile::Menu, None).unwrap(), "system");
    fs::write(root.path().join("user/mc.menu"), "legacy").unwrap();
    assert_eq!(files.read(ConfigurationFile::Menu, None).unwrap(), "legacy");
    fs::write(root.path().join("user/menu"), "standard").unwrap();
    assert_eq!(
        files.read(ConfigurationFile::Menu, None).unwrap(),
        "standard"
    );
    assert_eq!(
        files
            .prepare_for_edit(ConfigurationFile::Menu, None)
            .unwrap(),
        root.path().join("user/menu")
    );
}
