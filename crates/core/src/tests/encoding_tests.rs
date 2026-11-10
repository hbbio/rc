#![cfg(unix)]
use super::*;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

#[test]
fn encoding_decodes_legacy_bytes_without_bom_stripping_or_path_changes() {
    let name = OsString::from_vec(b"caf\xe9.txt".to_vec());
    assert_eq!(FilenameEncoding::Windows1252.decode(&name), "café.txt");
    assert_eq!(
        FilenameEncoding::Cp437.decode(&OsString::from_vec(b"caf\x82.txt".to_vec())),
        "café.txt"
    );
    let japanese = OsString::from_vec(vec![0x93, 0xfa, 0x96, 0x7b]);
    assert_eq!(FilenameEncoding::ShiftJis.decode(&japanese), "日本");
    assert_eq!(
        FilenameEncoding::Utf8.decode(std::ffi::OsStr::new("\u{feff}name")),
        "\u{feff}name"
    );
    for encoding in FilenameEncoding::ALL {
        assert_eq!(FilenameEncoding::parse(encoding.label()), Some(encoding));
    }
    assert!(FilenameEncoding::parse("unknown").is_none());
}

#[test]
fn encoding_menu_targets_named_panel_preserves_selection_tags_and_swaps_with_content() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("é.txt");
    fs::write(&path, "original bytes").unwrap();
    let mut app = app_with_loaded_panels(root.path().into());
    app.active_panel_mut().cursor = app
        .active_panel()
        .entries
        .iter()
        .position(|e| e.path == path)
        .unwrap();
    app.active_panel_mut().tagged.insert(path.clone());
    app.apply(AppCommand::Panel(
        ActivePanel::Left,
        PanelCommand::OpenEncoding,
    ))
    .unwrap();
    app.apply(AppCommand::DialogListboxSelectAt(1)).unwrap();
    app.apply(AppCommand::DialogAccept).unwrap();
    drain_background(&mut app);
    assert_eq!(app.active_panel().selected_entry().unwrap().path, path);
    assert_eq!(app.active_panel().selected_entry().unwrap().name, "Ã©.txt");
    assert!(app.active_panel().is_tagged(&path));
    assert_eq!(
        app.passive_panel().filename_encoding,
        FilenameEncoding::Utf8
    );
    app.apply(AppCommand::ViewEntry).unwrap();
    drain_background(&mut app);
    assert_eq!(app.active_viewer().unwrap().content(), "original bytes");
    app.apply(AppCommand::CloseViewer).unwrap();
    app.apply(AppCommand::SwapPanels).unwrap();
    assert_eq!(
        app.panels[1].filename_encoding,
        FilenameEncoding::Windows1252
    );
    assert_eq!(
        app.settings.panel_options.filename_encodings[1],
        FilenameEncoding::Windows1252
    );
}

#[test]
fn panelized_encoding_uses_original_bytes_and_filters_without_rerunning_command() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app_with_loaded_panels(root.path().into());
    app.set_filename_encoding(ActivePanel::Left, FilenameEncoding::Windows1252);
    drain_background(&mut app);
    app.set_panel_filter(
        ActivePanel::Left,
        PanelFilter {
            pattern: "café.txt".into(),
            ..Default::default()
        },
    );
    app.start_panelize_command("printf x >> count; printf 'caf\\351.txt\\n'".into());
    app.set_filename_encoding(ActivePanel::Left, FilenameEncoding::Utf8);
    assert_eq!(
        app.active_panel().filename_encoding,
        FilenameEncoding::Windows1252
    );
    drain_background(&mut app);
    let entry = &app.active_panel().entries[0];
    assert_eq!(entry.name, "café.txt");
    assert_eq!(
        entry.path,
        root.path()
            .join(OsString::from_vec(b"caf\xe9.txt".to_vec()))
    );
    let path = entry.path.clone();
    for (command, expected_tag) in [
        (AppCommand::SelectGroup, true),
        (AppCommand::UnselectGroup, false),
    ] {
        app.apply(command).unwrap();
        app.finish_dialog(DialogResult::FilterSubmitted(PanelFilter {
            pattern: "café.txt".into(),
            ..Default::default()
        }));
        assert_eq!(app.active_panel().is_tagged(&path), expected_tag);
    }
    app.set_panel_filter(ActivePanel::Left, PanelFilter::default());
    app.set_filename_encoding(ActivePanel::Left, FilenameEncoding::Utf8);
    drain_background(&mut app);
    assert_eq!(app.active_panel().entries[0].name, r"caf\xE9.txt");
    assert_eq!(fs::read(root.path().join("count")).unwrap(), b"x");
}

#[test]
fn restoring_panelized_results_uses_current_encoding_without_reexecuting_command() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app_with_loaded_panels(root.path().into());
    app.start_panelize_command("printf x >> count; printf 'caf\\351.txt\\n'".into());
    drain_background(&mut app);
    let path = app.active_panel().entries[0].path.clone();
    app.active_panel_mut().tagged.insert(path.clone());
    app.apply(AppCommand::Panel(
        ActivePanel::Left,
        PanelCommand::SetView(PanelViewMode::Listing),
    ))
    .unwrap();
    drain_background(&mut app);
    app.set_filename_encoding(ActivePanel::Left, FilenameEncoding::Windows1252);
    drain_background(&mut app);
    app.set_panel_filter(
        ActivePanel::Left,
        PanelFilter {
            pattern: "café.txt".into(),
            ..Default::default()
        },
    );
    app.apply(AppCommand::RestorePanelizedResults).unwrap();
    assert_eq!(app.active_panel().entries.len(), 1);
    assert_eq!(app.active_panel().entries[0].name, "café.txt");
    assert_eq!(app.active_panel().selected_entry().unwrap().path, path);
    assert!(app.active_panel().is_tagged(&path));
    assert_eq!(fs::read(root.path().join("count")).unwrap(), b"x");
}

#[test]
fn archive_encoding_filters_decoded_names_and_keeps_raw_member_identity() {
    let root = tempfile::tempdir().unwrap();
    let archive = root.path().join("legacy.tar");
    let name = OsString::from_vec(b"caf\xe9.txt".to_vec());
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(4);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, PathBuf::from(&name), &b"data"[..])
        .unwrap();
    fs::write(&archive, builder.into_inner().unwrap()).unwrap();
    let mut app = app_with_loaded_panels(root.path().into());
    app.active_panel_mut().cursor = app
        .active_panel()
        .entries
        .iter()
        .position(|e| e.path == archive)
        .unwrap();
    app.apply(AppCommand::OpenEntry).unwrap();
    drain_background(&mut app);
    app.set_panel_filter(
        ActivePanel::Left,
        PanelFilter {
            pattern: "café.txt".into(),
            ..Default::default()
        },
    );
    app.set_filename_encoding(ActivePanel::Left, FilenameEncoding::Windows1252);
    drain_background(&mut app);
    let entry = app
        .active_panel()
        .entries
        .iter()
        .find(|e| !e.is_parent())
        .unwrap();
    assert_eq!(entry.name, "café.txt");
    assert_eq!(entry.path, archive.join(name));
    let index = app
        .active_panel()
        .entries
        .iter()
        .position(|e| !e.is_parent())
        .unwrap();
    app.active_panel_mut().cursor = index;
    app.apply(AppCommand::ViewEntry).unwrap();
    drain_background(&mut app);
    assert_eq!(app.active_viewer().unwrap().content(), "data");
}

#[test]
fn zip_filename_encoding_preserves_raw_member_paths() {
    use std::io::{Cursor, Write};
    let root = tempfile::tempdir().unwrap();
    let archive = root.path().join("legacy.zip");
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file("cafX.txt", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"data").unwrap();
    let mut bytes = writer.finish().unwrap().into_inner();
    let offsets: Vec<_> = bytes
        .windows(8)
        .enumerate()
        .filter_map(|(index, word)| (word == b"cafX.txt").then_some(index))
        .collect();
    assert_eq!(offsets.len(), 2);
    for index in offsets {
        bytes[index + 3] = 0xe9;
    }
    fs::write(&archive, bytes).unwrap();
    let mut app = app_with_loaded_panels(root.path().into());
    app.active_panel_mut().cursor = app
        .active_panel()
        .entries
        .iter()
        .position(|e| e.path == archive)
        .unwrap();
    app.apply(AppCommand::OpenEntry).unwrap();
    drain_background(&mut app);
    let index = app
        .active_panel()
        .entries
        .iter()
        .position(|e| !e.is_parent())
        .unwrap();
    app.active_panel_mut().cursor = index;
    assert_eq!(
        app.active_panel().selected_entry().unwrap().name,
        "cafΘ.txt"
    );
    let path = app.active_panel().selected_entry().unwrap().path.clone();
    for (command, expected_tag) in [
        (AppCommand::SelectGroup, true),
        (AppCommand::UnselectGroup, false),
    ] {
        app.apply(command).unwrap();
        app.finish_dialog(DialogResult::FilterSubmitted(PanelFilter {
            pattern: "cafΘ.txt".into(),
            ..Default::default()
        }));
        assert_eq!(app.active_panel().is_tagged(&path), expected_tag);
    }
    app.set_filename_encoding(ActivePanel::Left, FilenameEncoding::Windows1252);
    drain_background(&mut app);
    assert_eq!(
        app.active_panel().selected_entry().unwrap().name,
        "café.txt"
    );
    assert_eq!(
        app.active_panel().selected_entry().unwrap().path,
        archive.join(OsString::from_vec(b"caf\xe9.txt".to_vec()))
    );
    app.apply(AppCommand::ViewEntry).unwrap();
    drain_background(&mut app);
    assert_eq!(app.active_viewer().unwrap().content(), "data");
}

#[test]
fn filename_encoding_shortcut_is_bound() {
    let map = keymap::Keymap::bundled_mc_default().unwrap();
    let chord = keymap::KeyChord {
        code: keymap::KeyCode::Char('e'),
        modifiers: keymap::KeyModifiers {
            alt: true,
            ..Default::default()
        },
    };
    assert_eq!(
        map.resolve(KeyContext::FileManager, chord)
            .and_then(|c| AppCommand::from_key_command(KeyContext::FileManager, c)),
        Some(AppCommand::OpenEncoding)
    );
}
