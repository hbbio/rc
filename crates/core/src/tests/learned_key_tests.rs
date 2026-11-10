use super::*;
use crate::keymap::LearnedBinding;

#[test]
fn learned_binding_overrides_dispatch_and_hints_then_removal_restores_defaults() {
    let root = tempfile::tempdir().unwrap();
    let mut app = AppState::new(root.path().to_path_buf()).unwrap();
    let base = Keymap::bundled_mc_default().unwrap();
    app.set_keybinding_hints_from_keymap(&base);
    let chord = KeyChord::new(KeyCode::F(3));
    app.settings.learn_keys.bindings.push(LearnedBinding {
        context: "filemanager".into(),
        action: "View".into(),
        chord: "F3".into(),
    });
    app.open_learned_binding(chord, None);
    app.finish_dialog(DialogResult::PairInputSubmitted {
        first: "filemanager".into(),
        second: "Swap".into(),
    });
    assert_eq!(
        app.resolve_key_command(&base, KeyContext::FileManager, chord),
        Some(&KeyCommand::SwapPanels)
    );
    assert_eq!(
        app.settings.learn_keys.bindings.len(),
        1,
        "aliases of the same chord must be replaced"
    );
    assert!(
        app.keybinding_joined_label(KeyContext::FileManager, AppCommand::SwapPanels, ",", 10)
            .unwrap()
            .contains("F3")
    );
    app.edit_learned_binding(0);
    app.finish_dialog(DialogResult::PairInputSubmitted {
        first: "filemanager".into(),
        second: String::new(),
    });
    assert_eq!(
        app.resolve_key_command(&base, KeyContext::FileManager, chord),
        Some(&KeyCommand::ViewEntry)
    );
    assert!(app.settings.learn_keys.bindings.is_empty());
}

#[test]
fn learned_bindings_round_trip_through_settings_and_reject_invalid_actions() {
    let root = tempfile::tempdir().unwrap();
    let paths = settings_io::SettingsPaths {
        mc_ini_path: None,
        rc_ini_path: Some(root.path().join("settings.ini")),
    };
    let mut app = AppState::new(root.path().to_path_buf()).unwrap();
    let chord = KeyChord {
        code: KeyCode::Char(';'),
        modifiers: KeyModifiers {
            ctrl: true,
            alt: false,
            shift: false,
        },
    };
    app.open_learned_binding(chord, None);
    app.finish_dialog(DialogResult::PairInputSubmitted {
        first: "viewer".into(),
        second: "Copy".into(),
    });
    assert!(app.settings.learn_keys.bindings.is_empty());
    assert!(matches!(app.top_route(), Route::Dialog(_)));
    app.finish_dialog(DialogResult::PairInputSubmitted {
        first: "viewer".into(),
        second: "Search".into(),
    });
    settings_io::save_settings(&paths, &app.persisted_settings_snapshot()).unwrap();
    let settings = settings_io::load_settings(&paths).unwrap();
    assert_eq!(
        settings.learn_keys.bindings,
        app.settings.learn_keys.bindings
    );
    let mut restored = AppState::new(root.path().to_path_buf()).unwrap();
    restored.replace_settings(settings);
    let base = Keymap::bundled_mc_default().unwrap();
    assert_eq!(
        restored.resolve_key_command(&base, KeyContext::Viewer, chord),
        Some(&KeyCommand::Search)
    );
}

#[test]
fn learned_binding_tokens_round_trip_punctuation_unicode_and_modifiers() {
    for code in [
        KeyCode::Char(' '),
        KeyCode::Char(';'),
        KeyCode::Char('-'),
        KeyCode::Char('='),
        KeyCode::Char('é'),
        KeyCode::F(12),
        KeyCode::PageDown,
    ] {
        let chord = KeyChord {
            code,
            modifiers: KeyModifiers {
                ctrl: true,
                alt: true,
                shift: true,
            },
        };
        let binding = LearnedBinding {
            context: "filemanager".into(),
            action: "View".into(),
            chord: chord.config_token(),
        };
        assert_eq!(binding.parse().unwrap().1, chord);
    }
    for (context, action) in [
        ("filemanager\n[viewer]", "View"),
        ("filemanager", "View\nQuit=q"),
        ("filemanager", "ImaginaryCommand"),
    ] {
        assert!(
            LearnedBinding {
                context: context.into(),
                action: action.into(),
                chord: "f3".into()
            }
            .parse()
            .is_err()
        );
    }
    assert!(
        LearnedBinding {
            context: "filemanager:xmap".into(),
            action: "View".into(),
            chord: "v".into()
        }
        .parse()
        .is_ok()
    );
}

#[test]
fn canceling_binding_editor_keeps_existing_mapping() {
    let root = tempfile::tempdir().unwrap();
    let mut app = AppState::new(root.path().to_path_buf()).unwrap();
    app.open_learned_binding(KeyChord::new(KeyCode::F(3)), None);
    app.apply(AppCommand::DialogCancel).unwrap();
    assert!(app.settings.learn_keys.bindings.is_empty());
}

#[test]
fn learned_command_line_bindings_match_the_dispatcher_allowlist() {
    for action in ["Ok", "Cancel", "FocusNext", "Backspace"] {
        assert!(
            LearnedBinding {
                context: "commandline".into(),
                action: action.into(),
                chord: "f3".into()
            }
            .parse()
            .is_err()
        );
    }
    for action in [
        "Help",
        "CommandHistory",
        "PutCurrentSelected",
        "ExtendedKeyMap",
    ] {
        assert!(
            LearnedBinding {
                context: "commandline".into(),
                action: action.into(),
                chord: "f3".into()
            }
            .parse()
            .is_ok()
        );
    }
}
