use super::*;

fn fixture() -> (tempfile::TempDir, AppState) {
    let root = tempfile::tempdir().unwrap();
    for name in ["first.txt", "SECOND.TXT", "hidden.md"] {
        fs::write(root.path().join(name), b"contents").unwrap();
    }
    fs::create_dir(root.path().join("folder.txt")).unwrap();
    let app = app_with_loaded_panels(root.path().to_path_buf());
    (root, app)
}

fn submit(app: &mut AppState, command: AppCommand, filter: PanelFilter) {
    app.apply(command).unwrap();
    app.finish_dialog(DialogResult::FilterSubmitted(filter));
}

#[test]
fn group_selection_menu_and_keys_apply_patterns_without_changing_the_listing() {
    let (root, mut app) = fixture();
    let keymap = Keymap::bundled_mc_default().unwrap();
    for (key, expected) in [
        ('+', AppCommand::SelectGroup),
        ('-', AppCommand::UnselectGroup),
    ] {
        let command = keymap
            .resolve(KeyContext::FileManager, KeyChord::new(KeyCode::Char(key)))
            .unwrap();
        assert_eq!(
            AppCommand::from_key_command(KeyContext::FileManager, command),
            Some(expected)
        );
    }
    let before = app.active_panel().entries.clone();
    let cursor = app.active_panel().cursor;
    app.apply(AppCommand::OpenMenuBarAt(1)).unwrap();
    move_menu_selection_to_label(&mut app, "Select group");
    app.apply(AppCommand::MenuAccept).unwrap();
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("selection dialog should open")
    };
    assert_eq!(dialog.title, "Select group");
    let filter = PanelFilter {
        pattern: String::from("*.txt"),
        case_sensitive: false,
        ..PanelFilter::default()
    };
    app.finish_dialog(DialogResult::FilterSubmitted(filter));
    assert_eq!(
        app.active_panel().tagged_paths(),
        vec![
            root.path().join("SECOND.TXT"),
            root.path().join("first.txt")
        ]
    );
    assert_eq!(app.active_panel().entries, before);
    assert_eq!(app.active_panel().cursor, cursor);
    assert_eq!(app.active_panel().filter(), &PanelFilter::default());
    assert!(app.passive_panel().tagged_paths().is_empty());

    submit(
        &mut app,
        AppCommand::UnselectGroup,
        PanelFilter {
            pattern: String::from("*.txt"),
            ..PanelFilter::default()
        },
    );
    assert_eq!(
        app.active_panel().tagged_paths(),
        vec![root.path().join("SECOND.TXT")]
    );
    submit(
        &mut app,
        AppCommand::SelectGroup,
        PanelFilter {
            pattern: String::from("^folder\\.txt$"),
            files_only: false,
            name_mode: FindNameMode::Regex,
            case_sensitive: true,
        },
    );
    assert!(
        app.active_panel()
            .is_tagged(&root.path().join("folder.txt"))
    );
    assert!(!app.active_panel().is_tagged(root.path().parent().unwrap()));
    assert!(matches!(
        app.selection_size_state(ActivePanel::Left),
        SelectionSizeState::Calculating { .. }
    ));
}

#[test]
fn group_selection_preserves_hidden_tags_and_rejects_invalid_patterns_atomically() {
    let (root, mut app) = fixture();
    let hidden = root.path().join("hidden.md");
    app.active_panel_mut().tagged.insert(hidden.clone());
    let filter = PanelFilter {
        pattern: String::from("*.txt"),
        ..PanelFilter::default()
    };
    app.set_panel_filter(ActivePanel::Left, filter.clone());
    app.refresh_active_panel();
    drain_background(&mut app);
    let original = app.active_panel().tagged_paths();
    for pattern in ["[", ""] {
        submit(
            &mut app,
            AppCommand::SelectGroup,
            PanelFilter {
                pattern: pattern.to_string(),
                ..PanelFilter::default()
            },
        );
        assert!(matches!(app.top_route(), Route::Dialog(_)));
        assert_eq!(app.active_panel().tagged_paths(), original);
        app.apply(AppCommand::DialogCancel).unwrap();
        assert_eq!(app.active_panel().tagged_paths(), original);
    }
    submit(&mut app, AppCommand::SelectGroup, filter);
    assert!(app.active_panel().is_tagged(&hidden));
    assert!(app.active_panel().is_tagged(&root.path().join("first.txt")));
    submit(
        &mut app,
        AppCommand::UnselectGroup,
        PanelFilter {
            pattern: String::from("*"),
            files_only: false,
            ..PanelFilter::default()
        },
    );
    assert_eq!(app.active_panel().tagged_paths(), vec![hidden]);
}

#[cfg(unix)]
#[test]
fn group_selection_distinguishes_native_names_with_identical_display_labels() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let (root, mut app) = fixture();
    let native = root.path().join(OsStr::from_bytes(b"a\xff.txt"));
    let unicode = root.path().join("a\u{fffd}.txt");
    app.active_panel_mut().entries = vec![
        FileEntry::file(String::from("a\u{fffd}.txt"), native.clone(), 0, None),
        FileEntry::file(String::from("a\u{fffd}.txt"), unicode.clone(), 0, None),
    ];
    for (name_mode, pattern) in [
        (FindNameMode::Glob, "a\u{fffd}.txt"),
        (FindNameMode::Regex, "^a\u{fffd}\\.txt$"),
    ] {
        app.active_panel_mut().tagged.clear();
        submit(
            &mut app,
            AppCommand::SelectGroup,
            PanelFilter {
                pattern: pattern.to_string(),
                name_mode,
                ..PanelFilter::default()
            },
        );
        assert_eq!(app.active_panel().tagged_paths(), vec![unicode.clone()]);
    }
    app.active_panel_mut().tagged.clear();
    submit(
        &mut app,
        AppCommand::SelectGroup,
        PanelFilter {
            pattern: String::from(r"(?-u:^a\xFF\.txt$)"),
            name_mode: FindNameMode::Regex,
            ..PanelFilter::default()
        },
    );
    assert_eq!(app.active_panel().tagged_paths(), vec![native.clone()]);
    submit(
        &mut app,
        AppCommand::SelectGroup,
        PanelFilter {
            pattern: String::from("a*.txt"),
            ..PanelFilter::default()
        },
    );
    assert!(app.active_panel().is_tagged(&native));
    assert!(app.active_panel().is_tagged(&unicode));
}
