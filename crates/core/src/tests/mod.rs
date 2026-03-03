use super::*;
use crate::keymap::{KeyCommand, KeyModifiers};
use std::path::Path;
use std::process::Output;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::Duration;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use std::{env, fs};

mod find_tests;
mod viewer_tests;

fn file_entry(name: &str) -> FileEntry {
    FileEntry {
        name: name.to_string(),
        path: PathBuf::from(name),
        is_dir: false,
        is_parent: false,
        size: 0,
        modified: None,
    }
}

struct PermissionDeniedProcessBackend;

impl ProcessBackend for PermissionDeniedProcessBackend {
    fn run_shell_command(
        &self,
        _cwd: &Path,
        _command: &str,
        _cancel_flag: Option<&AtomicBool>,
        _canceled_message: &str,
    ) -> io::Result<Output> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "permission denied",
        ))
    }
}

fn drain_background(app: &mut AppState) {
    loop {
        let mut progressed = false;

        let worker_commands = app.take_pending_worker_commands();
        if !worker_commands.is_empty() {
            progressed = true;
        }
        for command in worker_commands {
            match command {
                WorkerCommand::Run(job) => {
                    let job = *job;
                    let job_id = job.id;
                    let (event_tx, event_rx) = std::sync::mpsc::channel();
                    match &job.request {
                        JobRequest::RefreshPanel {
                            panel,
                            cwd,
                            source,
                            sort_mode,
                            show_hidden_files,
                            request_id,
                        } => {
                            let _ = event_tx.send(JobEvent::Started { id: job_id });
                            let cancel_flag = job.cancel_flag();
                            app.handle_background_event(refresh_panel_event(
                                *panel,
                                cwd.clone(),
                                source.clone(),
                                *sort_mode,
                                *show_hidden_files,
                                *request_id,
                                cancel_flag.as_ref(),
                            ));
                            let _ = event_tx.send(JobEvent::Finished {
                                id: job_id,
                                result: Ok(()),
                            });
                        }
                        JobRequest::Find {
                            query,
                            base_dir,
                            max_results,
                        } => {
                            let query = query.clone();
                            let base_dir = base_dir.clone();
                            let max_results = *max_results;
                            let cancel_flag = job.cancel_flag();
                            let pause_flag = job
                                .find_pause_flag()
                                .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
                            let _ = event_tx.send(JobEvent::Started { id: job_id });
                            let (chunk_tx, chunk_rx) = std::sync::mpsc::channel();
                            let result = run_find_entries(
                                &base_dir,
                                &query,
                                max_results,
                                cancel_flag.as_ref(),
                                pause_flag.as_ref(),
                                |entries| {
                                    chunk_tx
                                        .send(BackgroundEvent::FindEntriesChunk { job_id, entries })
                                        .is_ok()
                                },
                            )
                            .map_err(JobError::from_message);
                            for event in chunk_rx.try_iter() {
                                app.handle_background_event(event);
                            }
                            let _ = event_tx.send(JobEvent::Finished { id: job_id, result });
                        }
                        JobRequest::LoadViewer { path } => {
                            let _ = event_tx.send(JobEvent::Started { id: job_id });
                            let viewer_result =
                                ViewerState::open(path.clone()).map_err(|error| error.to_string());
                            app.handle_background_event(BackgroundEvent::ViewerLoaded {
                                path: path.clone(),
                                result: viewer_result.clone(),
                            });
                            let result = viewer_result.map(|_| ()).map_err(JobError::from_message);
                            let _ = event_tx.send(JobEvent::Finished { id: job_id, result });
                        }
                        JobRequest::BuildTree {
                            root,
                            max_depth,
                            max_entries,
                        } => {
                            let _ = event_tx.send(JobEvent::Started { id: job_id });
                            app.handle_background_event(build_tree_ready_event(
                                root.clone(),
                                *max_depth,
                                *max_entries,
                            ));
                            let _ = event_tx.send(JobEvent::Finished {
                                id: job_id,
                                result: Ok(()),
                            });
                        }
                        _ => {
                            execute_worker_job(job, &event_tx);
                        }
                    }
                    for event in event_rx.try_iter() {
                        app.handle_job_event(event);
                    }
                }
                WorkerCommand::Cancel(_) | WorkerCommand::Shutdown => {}
            }
        }

        if !progressed {
            break;
        }
    }
}

#[test]
fn panelized_entries_allow_process_backend_injection() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-backend-injection-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let backend = PermissionDeniedProcessBackend;
    let error = read_panelized_entries_with_process_backend(
        &root,
        "ignored",
        SortMode::default(),
        None,
        &backend,
    )
    .expect_err("injected process backend should drive panelize execution");
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

fn move_menu_selection_to_label(app: &mut AppState, label: &str) {
    let len = match app.top_route() {
        Route::Menu(menu) => menu.active_entries().len(),
        _ => panic!("menu route should be active"),
    };
    for _ in 0..len {
        let matches_target = match app.top_route() {
            Route::Menu(menu) => menu
                .active_entries()
                .get(menu.selected_entry)
                .is_some_and(|entry| entry.label == label),
            _ => false,
        };
        if matches_target {
            return;
        }
        app.apply(AppCommand::MenuMoveDown)
            .expect("menu movement should succeed");
    }
    panic!("menu entry '{label}' should exist");
}

fn submit_panelize_custom_command(app: &mut AppState, command: &str) {
    app.open_panelize_dialog();
    app.finish_dialog(DialogResult::ListboxSubmitted {
        index: Some(0),
        value: Some(String::from(PANELIZE_CUSTOM_COMMAND_LABEL)),
    });
    app.finish_dialog(DialogResult::InputSubmitted(command.to_string()));
}

#[test]
fn toggle_panel_flips_between_left_and_right() {
    let mut panel = ActivePanel::Left;
    panel.toggle();
    assert_eq!(panel, ActivePanel::Right);
    panel.toggle();
    assert_eq!(panel, ActivePanel::Left);
}

#[test]
fn move_cursor_stays_in_bounds() {
    let mut panel = PanelState {
        cwd: PathBuf::from("/tmp"),
        entries: vec![file_entry("a"), file_entry("b")],
        cursor: 0,
        sort_mode: SortMode::default(),
        show_hidden_files: true,
        source: PanelListingSource::Directory,
        tagged: HashSet::new(),
        loading: false,
    };

    panel.move_cursor(-1);
    assert_eq!(panel.cursor, 0);

    panel.move_cursor(99);
    assert_eq!(panel.cursor, 1);
}

#[test]
fn panel_listing_prepends_parent_entry() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-parent-entry-{stamp}"));
    let child = root.join("child");

    fs::create_dir_all(&child).expect("must create child directory");
    fs::write(child.join("a.txt"), "x").expect("must create child file");

    let panel = PanelState::new(child.clone()).expect("panel should initialize");
    let first = panel.entries.first().expect("entries should not be empty");
    assert_eq!(first.name, "..");
    assert!(first.is_parent);
    assert!(first.is_dir);
    assert_eq!(first.path, root);

    fs::remove_dir_all(&root).expect("must remove temp tree");
}

#[cfg(unix)]
#[test]
fn listing_marks_directory_symlinks_as_directories() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-dir-symlink-listing-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let target_dir = root.join("target-dir");
    fs::create_dir_all(&target_dir).expect("must create target directory");
    let symlink_path = root.join("tmp-like");
    std::os::unix::fs::symlink(&target_dir, &symlink_path)
        .expect("directory symlink should be creatable");

    let entries = read_entries(&root, SortMode::default()).expect("listing should load");
    let symlink_entry = entries
        .iter()
        .find(|entry| entry.path == symlink_path)
        .expect("directory symlink should be listed");
    assert!(
        symlink_entry.is_dir,
        "directory symlink should be classified as a directory"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn name_sort_listing_populates_metadata_fields() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-name-sort-metadata-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let file_path = root.join("entry.txt");
    fs::write(&file_path, "payload").expect("must create source file");

    let entries = read_entries(
        &root,
        SortMode {
            field: SortField::Name,
            reverse: false,
        },
    )
    .expect("listing should load");
    let file_entry = entries
        .iter()
        .find(|entry| entry.path == file_path)
        .expect("file entry should be present");
    assert!(
        file_entry.size >= 7,
        "name sort should include file metadata size"
    );
    assert!(
        file_entry.modified.is_some(),
        "name sort should include file metadata mtime"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn size_sort_listing_populates_metadata_fields() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-size-sort-metadata-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let file_path = root.join("entry.txt");
    fs::write(&file_path, "payload").expect("must create source file");

    let entries = read_entries(
        &root,
        SortMode {
            field: SortField::Size,
            reverse: false,
        },
    )
    .expect("listing should load");
    let file_entry = entries
        .iter()
        .find(|entry| entry.path == file_path)
        .expect("file entry should be present");
    assert!(
        file_entry.size >= 7,
        "size sort should include file metadata size"
    );
    assert!(
        file_entry.modified.is_some(),
        "size sort should include file metadata mtime"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn toggle_and_invert_tags_work_for_non_parent_entries() {
    let mut panel = PanelState {
        cwd: PathBuf::from("/tmp"),
        entries: vec![
            FileEntry::parent(PathBuf::from("/")),
            file_entry("a"),
            file_entry("b"),
        ],
        cursor: 0,
        sort_mode: SortMode::default(),
        show_hidden_files: true,
        source: PanelListingSource::Directory,
        tagged: HashSet::new(),
        loading: false,
    };

    assert!(
        !panel.toggle_tag_on_cursor(),
        "parent entry should not be taggable"
    );
    assert_eq!(panel.tagged_count(), 0);

    panel.cursor = 1;
    assert!(panel.toggle_tag_on_cursor());
    assert_eq!(panel.tagged_count(), 1);
    assert!(panel.is_tagged(Path::new("a")));

    panel.invert_tags();
    assert_eq!(panel.tagged_count(), 1);
    assert!(panel.is_tagged(Path::new("b")));
    assert!(!panel.is_tagged(Path::new("a")));
}

#[test]
fn page_home_end_navigation_stays_bounded() {
    let entries = vec![
        FileEntry::parent(PathBuf::from("/tmp")),
        file_entry("a"),
        file_entry("b"),
        file_entry("c"),
    ];
    let mut panel = PanelState {
        cwd: PathBuf::from("/tmp"),
        entries,
        cursor: 1,
        sort_mode: SortMode::default(),
        show_hidden_files: true,
        source: PanelListingSource::Directory,
        tagged: HashSet::new(),
        loading: false,
    };

    panel.move_cursor_home();
    assert_eq!(panel.cursor, 0);

    panel.move_cursor_end();
    assert_eq!(panel.cursor, 3);

    panel.move_cursor_page(1, 10);
    assert_eq!(panel.cursor, 3);

    panel.move_cursor_page(-1, 10);
    assert_eq!(panel.cursor, 0);
}

#[test]
fn sort_mode_cycles_and_toggles_direction() {
    let mut panel = PanelState {
        cwd: PathBuf::from("/tmp"),
        entries: Vec::new(),
        cursor: 0,
        sort_mode: SortMode::default(),
        show_hidden_files: true,
        source: PanelListingSource::Directory,
        tagged: HashSet::new(),
        loading: false,
    };

    panel.sort_mode.field = SortField::Name;
    panel.sort_mode.reverse = false;
    assert_eq!(panel.sort_label(), "name asc");

    panel.sort_mode.field = panel.sort_mode.field.next();
    assert_eq!(panel.sort_mode.field, SortField::Size);

    panel.sort_mode.reverse = true;
    assert_eq!(panel.sort_label(), "size desc");
}

#[test]
fn toggle_tag_advances_cursor_to_next_entry() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-toggle-tag-cursor-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let alpha = root.join("alpha.txt");
    let bravo = root.join("bravo.txt");
    fs::write(&alpha, "a").expect("must create alpha file");
    fs::write(&bravo, "b").expect("must create bravo file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let alpha_index = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == alpha)
        .expect("alpha entry should be visible");
    app.active_panel_mut().cursor = alpha_index;

    app.apply(AppCommand::ToggleTag)
        .expect("toggle tag should succeed");

    assert!(
        app.active_panel().is_tagged(&alpha),
        "alpha should be tagged after toggle"
    );
    assert_eq!(
        app.active_panel().cursor,
        alpha_index + 1,
        "cursor should advance to the next entry"
    );
    let selected = app
        .active_panel()
        .selected_entry()
        .expect("next entry should be selected");
    assert_eq!(
        selected.path, bravo,
        "cursor should land on the next file entry"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn mkdir_dialog_queues_mkdir_job() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-mkdir-dialog-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenInputDialog)
        .expect("mkdir dialog should open");
    for ch in "newdir".chars() {
        app.apply(AppCommand::DialogInputChar(ch))
            .expect("typing should be accepted");
    }
    app.apply(AppCommand::DialogAccept)
        .expect("mkdir dialog should submit");

    let pending = app.take_pending_worker_commands();
    assert_eq!(pending.len(), 1, "mkdir should enqueue one worker command");
    match &pending[0] {
        WorkerCommand::Run(job) => match &job.request {
            JobRequest::Mkdir { path } => {
                assert_eq!(path, &root.join("newdir"));
            }
            _ => panic!("expected mkdir request"),
        },
        _ => panic!("expected worker run command"),
    }
    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn rename_dialog_queues_rename_job() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-rename-dialog-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let source = root.join("before.txt");
    fs::write(&source, "before").expect("must create source file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let source_index = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == source)
        .expect("source entry should be visible");
    app.active_panel_mut().cursor = source_index;

    app.apply(AppCommand::OpenConfirmDialog)
        .expect("rename dialog should open");
    for _ in 0.."before.txt".len() {
        app.apply(AppCommand::DialogBackspace)
            .expect("rename input should accept backspace");
    }
    for ch in "after.txt".chars() {
        app.apply(AppCommand::DialogInputChar(ch))
            .expect("rename input should accept typing");
    }
    app.apply(AppCommand::DialogAccept)
        .expect("rename dialog should submit");

    let pending = app.take_pending_worker_commands();
    assert_eq!(pending.len(), 1, "rename should enqueue one worker command");
    match &pending[0] {
        WorkerCommand::Run(job) => match &job.request {
            JobRequest::Rename {
                source,
                destination,
            } => {
                assert_eq!(source, &root.join("before.txt"));
                assert_eq!(destination, &root.join("after.txt"));
            }
            _ => panic!("expected rename request"),
        },
        _ => panic!("expected worker run command"),
    }

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn skin_dialog_emits_selected_skin() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-skin-dialog-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.set_available_skins(vec![String::from("default"), String::from("dark")]);
    app.set_active_skin_name("default");

    app.apply(AppCommand::OpenSkinDialog)
        .expect("skin dialog should open");
    assert_eq!(app.key_context(), KeyContext::Listbox);

    app.apply(AppCommand::DialogListboxUp)
        .expect("listbox up should move selection");
    app.apply(AppCommand::DialogAccept)
        .expect("skin dialog should submit");

    assert_eq!(app.take_pending_skin_change(), Some(String::from("dark")));
    assert_eq!(app.status_line, "Skin selected: dark");

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn skin_dialog_emits_preview_and_revert_on_cancel() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-skin-preview-cancel-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.set_available_skins(vec![String::from("default"), String::from("dark")]);
    app.set_active_skin_name("default");

    app.apply(AppCommand::OpenSkinDialog)
        .expect("skin dialog should open");
    app.apply(AppCommand::DialogListboxUp)
        .expect("listbox up should move selection");
    assert_eq!(app.take_pending_skin_preview(), Some(String::from("dark")));
    assert_eq!(app.take_pending_skin_change(), None);
    assert_eq!(app.take_pending_skin_revert(), None);

    app.apply(AppCommand::DialogCancel)
        .expect("skin dialog cancel should close");
    assert_eq!(app.take_pending_skin_preview(), None);
    assert_eq!(app.take_pending_skin_change(), None);
    assert_eq!(
        app.take_pending_skin_revert(),
        Some(String::from("default"))
    );
    assert_eq!(app.status_line, "Skin unchanged");

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn help_route_supports_topic_links_and_back_navigation() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-help-route-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenHelp)
        .expect("help route should open");
    assert_eq!(app.key_context(), KeyContext::Help);
    let Route::Help(help) = app.top_route() else {
        panic!("top route should be help");
    };
    assert_eq!(help.current_id(), "file-manager");

    app.apply(AppCommand::HelpIndex)
        .expect("help index should open");
    let Route::Help(help) = app.top_route() else {
        panic!("top route should remain help");
    };
    assert_eq!(help.current_id(), "index");

    app.apply(AppCommand::HelpLinkNext)
        .expect("next help link should select");
    app.apply(AppCommand::HelpFollowLink)
        .expect("following selected link should succeed");
    let Route::Help(help) = app.top_route() else {
        panic!("top route should remain help");
    };
    assert_ne!(help.current_id(), "index");

    app.apply(AppCommand::HelpBack)
        .expect("help back should return to previous node");
    let Route::Help(help) = app.top_route() else {
        panic!("top route should remain help");
    };
    assert_eq!(help.current_id(), "index");

    app.apply(AppCommand::CloseHelp)
        .expect("help route should close");
    assert_eq!(app.key_context(), KeyContext::FileManager);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn menu_shortcuts_follow_loaded_keymap_bindings() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-menu-shortcuts-keymap-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let keymap = Keymap::parse(
        r#"
[filemanager]
View = f11
Edit = f12
Copy = ctrl-y
"#,
    )
    .expect("keymap should parse");
    app.set_keybinding_hints_from_keymap(&keymap);

    let view_entry = FILE_MENU_ENTRIES
        .iter()
        .find(|entry| entry.label == "View")
        .expect("View entry should exist");
    let edit_entry = FILE_MENU_ENTRIES
        .iter()
        .find(|entry| entry.label == "Edit")
        .expect("Edit entry should exist");
    let copy_entry = FILE_MENU_ENTRIES
        .iter()
        .find(|entry| entry.label == "Copy")
        .expect("Copy entry should exist");

    assert_eq!(app.menu_entry_shortcut_label(view_entry), "F11");
    assert_eq!(app.menu_entry_shortcut_label(edit_entry), "F12");
    assert_eq!(app.menu_entry_shortcut_label(copy_entry), "Ctrl-y");

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn help_content_applies_keybinding_replacements() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-help-keybindings-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let keymap = Keymap::parse(
        r#"
[filemanager]
OpenJobs = f6
"#,
    )
    .expect("keymap should parse");
    app.set_keybinding_hints_from_keymap(&keymap);
    app.apply(AppCommand::OpenHelp)
        .expect("help route should open");

    let Route::Help(help) = app.top_route() else {
        panic!("top route should be help");
    };
    let mut content = String::new();
    for line in help.lines() {
        for span in &line.spans {
            match span {
                HelpSpan::Text(text) => content.push_str(text),
                HelpSpan::Link { label, .. } => content.push_str(label),
            }
        }
        content.push('\n');
    }

    assert!(
        !content.contains("{{"),
        "help content should not contain unresolved template tokens"
    );
    assert!(
        content.contains("F6 open jobs screen"),
        "help should reflect keymap-derived shortcuts"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn menu_route_supports_keyboard_navigation_and_selection() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-menu-route-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenMenuAt(2))
        .expect("menu route should open");
    assert_eq!(app.key_context(), KeyContext::Menu);

    move_menu_selection_to_label(&mut app, "Background jobs");
    app.apply(AppCommand::MenuAccept)
        .expect("menu accept should execute selected action");
    assert_eq!(app.key_context(), KeyContext::Jobs);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn menu_stub_action_reports_not_implemented_status() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-menu-stub-action-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenMenuAt(0))
        .expect("left menu should open");
    app.apply(AppCommand::MenuAccept)
        .expect("accepting stub menu action should succeed");
    assert_eq!(app.key_context(), KeyContext::FileManager);
    assert!(
        app.status_line.contains("not implemented"),
        "stub actions should report a not-implemented status"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn side_menus_match_and_options_match_mc_shape() {
    let menus = top_menus();
    let left = menus
        .iter()
        .find(|menu| menu.title == "Left")
        .expect("left menu should exist");
    let right = menus
        .iter()
        .find(|menu| menu.title == "Right")
        .expect("right menu should exist");
    let file = menus
        .iter()
        .find(|menu| menu.title == "File")
        .expect("file menu should exist");
    let options = menus
        .iter()
        .find(|menu| menu.title == "Options")
        .expect("options menu should exist");
    let command = menus
        .iter()
        .find(|menu| menu.title == "Command")
        .expect("command menu should exist");

    let left_labels: Vec<&str> = left.entries.iter().map(|entry| entry.label).collect();
    let right_labels: Vec<&str> = right.entries.iter().map(|entry| entry.label).collect();
    assert_eq!(
        left_labels, right_labels,
        "left and right menu entries should remain identical"
    );
    assert!(
        left_labels.contains(&"File listing")
            && left_labels.contains(&"Panelize")
            && left_labels.contains(&"Rescan"),
        "side menus should include MC-style panel controls"
    );

    let file_labels: Vec<&str> = file.entries.iter().map(|entry| entry.label).collect();
    assert_eq!(file_labels.first(), Some(&"View"));
    assert!(file_labels.contains(&"Rename/Move"));
    assert!(file_labels.contains(&"Select group"));
    assert_eq!(file_labels.last(), Some(&"Exit"));

    let command_labels: Vec<&str> = command.entries.iter().map(|entry| entry.label).collect();
    assert_eq!(
        command_labels,
        vec![
            "User menu",
            "Directory tree",
            "Find file",
            "Swap panels",
            "Switch panels on/off",
            "Compare directories",
            "Compare files",
            "External panelize",
            "Show directory sizes",
            "",
            "Command history",
            "Viewed/edited files history",
            "Directory hotlist",
            "Active VFS list",
            "Background jobs",
            "Screen list",
            "",
            "Edit extension file",
            "Edit menu file",
            "Edit highlighting group file",
        ],
        "command menu should follow MC structure and ordering"
    );

    let command_shortcuts: Vec<&str> = command.entries.iter().map(|entry| entry.shortcut).collect();
    assert_eq!(command_shortcuts[0], "F2");
    assert_eq!(command_shortcuts[2], "M-?");
    assert_eq!(command_shortcuts[7], "C-x !");
    assert_eq!(command_shortcuts[12], "C-\\");
    assert_eq!(command_shortcuts[14], "C-x j");

    let option_labels: Vec<&str> = options.entries.iter().map(|entry| entry.label).collect();
    assert_eq!(
        option_labels,
        vec![
            "Configuration...",
            "Layout...",
            "Panel options...",
            "Confirmation...",
            "Appearance...",
            "Display bits...",
            "Learn keys...",
            "Virtual FS...",
            "Save setup",
        ],
        "options menu should follow mc ordering and labels"
    );
}

#[test]
fn options_commands_open_settings_routes() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-options-route-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenOptionsLayout)
        .expect("layout options should open");
    let Route::Settings(settings) = app.top_route() else {
        panic!("settings route should open");
    };
    assert_eq!(settings.category, SettingsCategory::Layout);
    assert!(!settings.entries.is_empty());

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn settings_toggle_marks_dirty_and_save_setup_sets_pending_flag() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-options-dirty-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    assert!(!app.settings().save_setup.dirty);

    app.apply(AppCommand::OpenOptionsConfiguration)
        .expect("configuration options should open");
    app.apply(AppCommand::DialogAccept)
        .expect("toggle should apply");
    assert!(app.settings().save_setup.dirty);

    app.apply(AppCommand::SaveSetup)
        .expect("save setup command should succeed");
    assert!(app.take_pending_save_setup());

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn status_line_expires_after_configured_timeout() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-status-timeout-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.settings.layout.status_message_timeout_seconds = 10;
    app.set_status("Loading selected directory...");
    let expires_at = app
        .status_expires_at
        .expect("status timeout should schedule expiration");

    let before = expires_at
        .checked_sub(Duration::from_millis(1))
        .expect("status expiration should support sub-millisecond offset");
    app.expire_status_line_at(before);
    assert_eq!(
        app.status_line, "Loading selected directory...",
        "status should remain visible before configured timeout"
    );

    app.expire_status_line_at(expires_at);
    assert!(
        app.status_line.is_empty(),
        "status should clear once configured timeout elapses"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn status_line_timeout_zero_disables_auto_clear() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-status-timeout-off-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.settings.layout.status_message_timeout_seconds = 0;
    app.set_status("Loading selected directory...");
    assert!(
        app.status_expires_at.is_none(),
        "timeout value 0 should disable status auto-clear"
    );

    let much_later = Instant::now()
        .checked_add(Duration::from_secs(30))
        .expect("clock should support future offset");
    app.expire_status_line_at(much_later);
    assert_eq!(
        app.status_line, "Loading selected directory...",
        "status should remain until replaced when timeout is disabled"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn set_status_sanitizes_controls_and_truncates_very_long_messages() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-status-sanitize-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.set_status(format!(
        "line1\nline2\t{}\r{}",
        '\u{1b}',
        "x".repeat(MAX_STATUS_LINE_CHARS.saturating_add(128))
    ));
    assert!(
        !app.status_line.contains('\n')
            && !app.status_line.contains('\r')
            && !app.status_line.contains('\t')
            && !app.status_line.contains('\u{1b}'),
        "status text should strip control characters before render"
    );
    assert!(
        app.status_line.ends_with("..."),
        "very long status text should be truncated with an ellipsis"
    );
    assert!(
        app.status_line.chars().count() <= MAX_STATUS_LINE_CHARS.saturating_add(3),
        "status text should be bounded to avoid pathological render costs"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn persist_settings_job_coalesces_pending_request() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-persist-coalesce-pending-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let settings_paths = settings_io::SettingsPaths {
        mc_ini_path: Some(root.join("mc.ini")),
        rc_ini_path: Some(root.join("settings.ini")),
    };
    let snapshot_one = app.persisted_settings_snapshot();
    let mut snapshot_two = app.persisted_settings_snapshot();
    snapshot_two.appearance.skin = String::from("coalesced-skin");

    let first_id = app.enqueue_worker_job_request(JobRequest::PersistSettings {
        paths: settings_paths.clone(),
        snapshot: Box::new(snapshot_one),
    });
    let second_id = app.enqueue_worker_job_request(JobRequest::PersistSettings {
        paths: settings_paths.clone(),
        snapshot: Box::new(snapshot_two.clone()),
    });
    assert_eq!(first_id, second_id, "coalescing should reuse queued job id");

    let pending = app.take_pending_worker_commands();
    assert_eq!(
        pending.len(),
        1,
        "pending save setup should coalesce to one job"
    );
    match &pending[0] {
        WorkerCommand::Run(job) => match &job.request {
            JobRequest::PersistSettings { paths, snapshot } => {
                assert_eq!(paths, &settings_paths);
                assert_eq!(snapshot.appearance.skin, snapshot_two.appearance.skin);
            }
            _ => panic!("expected persist settings request"),
        },
        _ => panic!("expected queued worker command"),
    }

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn persist_settings_job_defers_latest_while_active() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-persist-coalesce-active-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let settings_paths = settings_io::SettingsPaths {
        mc_ini_path: Some(root.join("mc.ini")),
        rc_ini_path: Some(root.join("settings.ini")),
    };
    let first_snapshot = app.persisted_settings_snapshot();
    let mut second_snapshot = app.persisted_settings_snapshot();
    second_snapshot.appearance.skin = String::from("deferred-skin");

    let first_id = app.enqueue_worker_job_request(JobRequest::PersistSettings {
        paths: settings_paths.clone(),
        snapshot: Box::new(first_snapshot),
    });
    let pending = app.take_pending_worker_commands();
    assert_eq!(pending.len(), 1, "first save setup should be queued");

    let deferred_id = app.enqueue_worker_job_request(JobRequest::PersistSettings {
        paths: settings_paths,
        snapshot: Box::new(second_snapshot.clone()),
    });
    assert_eq!(
        deferred_id, first_id,
        "deferred save should attach to active job"
    );
    assert!(
        app.take_pending_worker_commands().is_empty(),
        "deferred save should not enqueue until active job finishes"
    );

    app.handle_job_event(JobEvent::Finished {
        id: first_id,
        result: Ok(()),
    });
    let pending = app.take_pending_worker_commands();
    assert_eq!(
        pending.len(),
        1,
        "latest deferred save should enqueue after finish"
    );
    match &pending[0] {
        WorkerCommand::Run(job) => match &job.request {
            JobRequest::PersistSettings { snapshot, .. } => {
                assert_eq!(snapshot.appearance.skin, second_snapshot.appearance.skin);
            }
            _ => panic!("expected persist settings request"),
        },
        _ => panic!("expected queued worker command"),
    }

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn learn_keys_capture_stores_chord_and_marks_settings_dirty() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-learn-keys-capture-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenOptionsLearnKeys)
        .expect("learn keys options should open");
    for _ in 0..4 {
        app.apply(AppCommand::DialogListboxDown)
            .expect("selection should move down");
    }
    app.apply(AppCommand::DialogAccept)
        .expect("capture entry should activate");
    assert!(
        app.status_line.contains("Press a key chord"),
        "capture mode status should be shown"
    );

    assert!(app.capture_learn_keys_chord(KeyChord {
        code: KeyCode::Char('x'),
        modifiers: KeyModifiers {
            ctrl: true,
            alt: false,
            shift: false,
        },
    }));
    assert_eq!(
        app.settings().learn_keys.last_learned_binding.as_deref(),
        Some("Ctrl-x")
    );
    assert!(app.settings().save_setup.dirty);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn learn_keys_capture_can_be_canceled_with_escape() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-learn-keys-cancel-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.settings_mut().learn_keys.last_learned_binding = Some(String::from("F5"));
    app.apply(AppCommand::OpenOptionsLearnKeys)
        .expect("learn keys options should open");
    for _ in 0..4 {
        app.apply(AppCommand::DialogListboxDown)
            .expect("selection should move down");
    }
    app.apply(AppCommand::DialogAccept)
        .expect("capture entry should activate");

    assert!(app.capture_learn_keys_chord(KeyChord {
        code: KeyCode::Esc,
        modifiers: KeyModifiers::default(),
    }));
    assert_eq!(
        app.settings().learn_keys.last_learned_binding.as_deref(),
        Some("F5")
    );
    assert!(
        app.status_line.contains("canceled"),
        "cancel status should be shown"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn confirm_quit_setting_requires_dialog_before_quit() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-confirm-quit-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenOptionsConfirmation)
        .expect("confirmation options should open");
    app.apply(AppCommand::DialogListboxDown)
        .expect("move to confirm overwrite");
    app.apply(AppCommand::DialogListboxDown)
        .expect("move to confirm quit");
    app.apply(AppCommand::DialogAccept)
        .expect("toggle confirm quit");

    let result = app
        .apply(AppCommand::Quit)
        .expect("quit should open confirmation");
    assert_eq!(result, ApplyResult::Continue);
    assert!(matches!(app.top_route(), Route::Dialog(_)));

    let quit_result = app
        .apply(AppCommand::DialogAccept)
        .expect("confirm quit should return quit result");
    assert_eq!(quit_result, ApplyResult::Quit);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn command_menu_external_panelize_opens_dialog() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-menu-command-panelize-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenMenuAt(2))
        .expect("command menu should open");
    move_menu_selection_to_label(&mut app, "External panelize");
    app.apply(AppCommand::MenuAccept)
        .expect("external panelize menu entry should open dialog");
    assert_eq!(app.key_context(), KeyContext::Listbox);
    assert!(app.status_line.contains("External panelize"));

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn menu_mouse_clicks_map_to_commands() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-menu-mouse-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let command = app.command_for_left_click(8, 0);
    assert_eq!(command, Some(AppCommand::OpenMenuAt(1)));

    app.apply(AppCommand::OpenMenuAt(1))
        .expect("menu route should open");
    assert_eq!(
        app.command_for_left_click(8, 3),
        Some(AppCommand::MenuSelectAt(1))
    );
    assert_eq!(
        app.command_for_left_click(100, 20),
        Some(AppCommand::CloseMenu)
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn delete_command_queues_job_only_after_confirmation() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-delete-dialog-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let victim = root.join("victim.txt");
    fs::write(&victim, "victim").expect("must create victim file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let victim_index = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == victim)
        .expect("victim entry should be visible");
    app.active_panel_mut().cursor = victim_index;

    app.apply(AppCommand::Delete)
        .expect("delete should open confirm dialog");
    assert_eq!(app.route_depth(), 2);

    app.apply(AppCommand::DialogAccept)
        .expect("confirm dialog should submit");
    let pending = app.take_pending_worker_commands();
    assert_eq!(
        pending.len(),
        1,
        "delete should enqueue exactly one worker command"
    );
    match &pending[0] {
        WorkerCommand::Run(job) => match &job.request {
            JobRequest::Delete { targets } => {
                assert_eq!(targets, &vec![victim.clone()]);
            }
            _ => panic!("expected delete job request"),
        },
        _ => panic!("expected queued worker run command"),
    }

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn copy_command_uses_destination_and_policy_dialogs() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-copy-dialog-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let source = root.join("a.txt");
    fs::write(&source, "a").expect("must create source file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let source_index = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == source)
        .expect("source entry should be visible");
    app.active_panel_mut().cursor = source_index;

    app.apply(AppCommand::Copy)
        .expect("copy should open destination dialog");
    assert_eq!(app.route_depth(), 2);

    app.apply(AppCommand::DialogAccept)
        .expect("destination dialog should submit");
    assert_eq!(
        app.route_depth(),
        2,
        "policy dialog should replace destination dialog"
    );

    app.apply(AppCommand::DialogAccept)
        .expect("policy dialog should submit");
    let pending = app.take_pending_worker_commands();
    assert_eq!(pending.len(), 1, "copy should enqueue one worker command");
    match &pending[0] {
        WorkerCommand::Run(job) => match &job.request {
            JobRequest::Copy {
                sources,
                destination_dir,
                overwrite,
            } => {
                assert_eq!(sources, &vec![source.clone()]);
                assert_eq!(destination_dir, &root);
                assert_eq!(*overwrite, app.overwrite_policy);
            }
            _ => panic!("expected copy job request"),
        },
        _ => panic!("expected queued worker run command"),
    }

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn copy_relative_destination_is_resolved_from_active_panel() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-copy-relative-destination-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let source = root.join("a.txt");
    fs::write(&source, "a").expect("must create source file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let source_index = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == source)
        .expect("source entry should be visible");
    app.active_panel_mut().cursor = source_index;

    app.start_copy_dialog();
    app.finish_dialog(DialogResult::InputSubmitted(String::from("dest")));

    match app.pending_dialog_action.as_ref() {
        Some(PendingDialogAction::TransferOverwrite {
            destination_dir, ..
        }) => {
            assert_eq!(destination_dir, &root.join("dest"));
        }
        other => panic!("expected transfer overwrite action, got {other:?}"),
    }

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn tree_screen_selects_directory_for_active_panel() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-tree-screen-{stamp}"));
    let branch = root.join("branch");
    fs::create_dir_all(&branch).expect("must create temp tree");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenTree)
        .expect("tree screen should open");
    drain_background(&mut app);
    assert_eq!(app.key_context(), KeyContext::Tree);

    let branch_index = match app.top_route() {
        Route::Tree(tree) => tree
            .entries
            .iter()
            .position(|entry| entry.path == branch)
            .expect("branch should appear in tree"),
        _ => panic!("top route should be tree"),
    };
    let Some(Route::Tree(tree)) = app.routes.last_mut() else {
        panic!("top route should be tree");
    };
    tree.cursor = branch_index;

    app.apply(AppCommand::TreeOpenEntry)
        .expect("tree open should succeed");
    assert_eq!(app.key_context(), KeyContext::FileManager);
    assert_eq!(app.active_panel().cwd, branch);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn hotlist_supports_add_remove_and_open() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-hotlist-{stamp}"));
    let branch = root.join("branch");
    fs::create_dir_all(&branch).expect("must create temp tree");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenHotlist)
        .expect("hotlist should open");
    app.apply(AppCommand::HotlistAddCurrentDirectory)
        .expect("hotlist add should succeed");
    assert_eq!(app.hotlist, vec![root.clone()]);

    {
        let panel = app.active_panel_mut();
        panel.cwd = branch.clone();
        panel.refresh().expect("panel should refresh");
    }
    app.apply(AppCommand::HotlistAddCurrentDirectory)
        .expect("hotlist add should succeed");
    assert_eq!(app.hotlist, vec![root.clone(), branch.clone()]);

    app.hotlist_cursor = 0;
    app.apply(AppCommand::HotlistRemoveSelected)
        .expect("hotlist remove should succeed");
    assert_eq!(app.hotlist, vec![branch.clone()]);

    app.hotlist_cursor = 0;
    app.apply(AppCommand::HotlistOpenEntry)
        .expect("hotlist open should succeed");
    assert_eq!(app.key_context(), KeyContext::FileManager);
    assert_eq!(app.active_panel().cwd, branch);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[cfg(unix)]
#[test]
fn panelize_command_populates_active_panel_from_stdout_paths() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-populate-{stamp}"));
    fs::create_dir_all(root.join("sub")).expect("must create subdirectory");
    fs::write(root.join("a.txt"), "a").expect("must create file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    submit_panelize_custom_command(&mut app, "printf 'a.txt\\nsub\\nmissing\\n'");
    drain_background(&mut app);

    let panel = app.active_panel();
    assert_eq!(
        panel.panelize_command(),
        Some("printf 'a.txt\\nsub\\nmissing\\n'"),
        "panelize mode should retain command for reread"
    );
    assert!(
        panel
            .entries
            .iter()
            .any(|entry| entry.path == root.join("a.txt")),
        "panelized entries should include file output path"
    );
    assert!(
        panel
            .entries
            .iter()
            .any(|entry| entry.path == root.join("sub")),
        "panelized entries should include directory output path"
    );
    assert!(
        panel
            .entries
            .iter()
            .any(|entry| entry.path == root.join("missing")),
        "panelized entries should preserve command output even when path metadata is missing"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[cfg(unix)]
#[test]
fn panelize_empty_output_keeps_empty_panel_entries() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-empty-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    fs::write(root.join("a.txt"), "a").expect("must create file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    submit_panelize_custom_command(&mut app, "printf ''");
    drain_background(&mut app);

    assert_eq!(
        app.active_panel().entries.len(),
        0,
        "empty panelize output should produce empty panel entries"
    );
    assert_eq!(app.active_panel().panelize_command(), Some("printf ''"));

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[cfg(unix)]
#[test]
fn panelize_preserves_leading_and_trailing_spaces_in_paths() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-spaces-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let spaced_name = "  spaced file  ";
    let spaced_file = root.join(spaced_name);
    fs::write(&spaced_file, "a").expect("must create spaced filename");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    submit_panelize_custom_command(&mut app, "printf '  spaced file  \\n'");
    drain_background(&mut app);

    assert!(
        app.active_panel()
            .entries
            .iter()
            .any(|entry| entry.path == spaced_file),
        "panelize should preserve leading/trailing spaces in path lines"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[cfg(unix)]
#[test]
fn cdup_leaves_panelize_mode_without_changing_directory() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-cdup-{stamp}"));
    let sub = root.join("sub");
    fs::create_dir_all(&sub).expect("must create subdirectory");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    submit_panelize_custom_command(&mut app, "printf 'sub\\n'");
    drain_background(&mut app);

    assert_eq!(
        app.active_panel().panelize_command(),
        Some("printf 'sub\\n'"),
        "precondition: panel should be in panelize mode"
    );
    assert_eq!(app.active_panel().cwd, root);

    app.apply(AppCommand::CdUp)
        .expect("CdUp should leave panelize mode");
    drain_background(&mut app);

    assert_eq!(
        app.active_panel().panelize_command(),
        None,
        "CdUp should restore normal directory mode from panelize"
    );
    assert_eq!(
        app.active_panel().cwd,
        root,
        "CdUp in panelize mode should not change to parent directory"
    );
    assert!(
        app.active_panel()
            .entries
            .iter()
            .any(|entry| entry.path == sub),
        "restored listing should include entries from the current directory"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[cfg(unix)]
#[test]
fn panelize_failure_preserves_previous_directory_listing() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-failure-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    fs::write(root.join("a.txt"), "a").expect("must create file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let before = app.active_panel().entries.clone();

    submit_panelize_custom_command(&mut app, "exit 42");
    drain_background(&mut app);

    assert!(
        app.status_line.contains("Panelize failed:"),
        "status line should indicate panelize failure"
    );
    assert_eq!(
        app.active_panel().entries,
        before,
        "failed panelize should keep previous listing"
    );
    assert_eq!(
        app.active_panel().panelize_command(),
        None,
        "failed panelize should not switch source mode"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[cfg(unix)]
#[test]
fn rename_dialog_uses_basename_for_panelized_entry() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-rename-basename-{stamp}"));
    let sub = root.join("sub");
    fs::create_dir_all(&sub).expect("must create subdirectory");
    fs::write(sub.join("a.txt"), "a").expect("must create file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    submit_panelize_custom_command(&mut app, "printf 'sub/a.txt\\n'");
    drain_background(&mut app);

    app.apply(AppCommand::OpenConfirmDialog)
        .expect("rename dialog should open");
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("rename action should open a dialog route");
    };
    let DialogKind::Input(input) = &dialog.kind else {
        panic!("rename action should open an input dialog");
    };
    assert_eq!(
        input.value, "a.txt",
        "rename input should default to basename, not panelized display label"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn panelize_dialog_lists_predefined_commands() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-presets-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.open_panelize_dialog();
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("panelize should open a dialog");
    };
    let DialogKind::Listbox(listbox) = &dialog.kind else {
        panic!("panelize should open a listbox dialog");
    };
    assert_eq!(
        listbox.items.first(),
        Some(&String::from(PANELIZE_CUSTOM_COMMAND_LABEL))
    );
    assert!(
        listbox
            .items
            .iter()
            .any(|item| item == "find . -name '*.orig'"),
        "panelize list should include predefined commands"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn panelize_dialog_tab_switches_from_presets_to_input() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-tab-to-input-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.open_panelize_dialog();
    app.apply(AppCommand::DialogFocusNext)
        .expect("tab should switch to command input");

    let Route::Dialog(dialog) = app.top_route() else {
        panic!("panelize should remain in dialog route");
    };
    let DialogKind::Input(input) = &dialog.kind else {
        panic!("tab should open panelize input dialog");
    };
    assert_eq!(input.value, "find . -type f");

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn panelize_dialog_tab_switches_from_input_back_to_presets() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-tab-to-presets-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.open_panelize_dialog();
    app.apply(AppCommand::DialogFocusNext)
        .expect("tab should switch to command input");
    app.apply(AppCommand::DialogInputChar('x'))
        .expect("typing command suffix should succeed");
    app.apply(AppCommand::DialogFocusNext)
        .expect("tab should switch back to preset list");

    let Route::Dialog(dialog) = app.top_route() else {
        panic!("panelize should remain in dialog route");
    };
    let DialogKind::Listbox(listbox) = &dialog.kind else {
        panic!("tab should return to preset list");
    };
    assert_eq!(listbox.selected, 0);
    assert_eq!(
        listbox.items.first(),
        Some(&String::from(PANELIZE_CUSTOM_COMMAND_LABEL))
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn panelize_preset_management_add_edit_remove_works() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-preset-manage-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.open_panelize_dialog();
    app.apply(AppCommand::PanelizePresetAdd)
        .expect("F2 add should open preset input");
    for ch in "echo added".chars() {
        app.apply(AppCommand::DialogInputChar(ch))
            .expect("typing preset command should succeed");
    }
    app.apply(AppCommand::DialogAccept)
        .expect("submitting new preset should succeed");

    let Route::Dialog(dialog) = app.top_route() else {
        panic!("panelize should remain in preset list dialog");
    };
    let DialogKind::Listbox(listbox) = &dialog.kind else {
        panic!("panelize should return to preset list dialog");
    };
    assert!(
        listbox.items.iter().any(|item| item == "echo added"),
        "added preset should appear in list"
    );

    app.apply(AppCommand::PanelizePresetEdit)
        .expect("F4 edit should open preset input");
    for ch in " updated".chars() {
        app.apply(AppCommand::DialogInputChar(ch))
            .expect("typing edit suffix should succeed");
    }
    app.apply(AppCommand::DialogAccept)
        .expect("submitting edited preset should succeed");

    let edited = String::from("echo added updated");
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("panelize should remain in preset list dialog");
    };
    let DialogKind::Listbox(listbox) = &dialog.kind else {
        panic!("panelize should return to preset list dialog");
    };
    assert!(
        listbox.items.iter().any(|item| item == &edited),
        "edited preset should replace previous value"
    );

    app.apply(AppCommand::PanelizePresetRemove)
        .expect("F8 remove should delete selected preset");
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("panelize should remain in preset list dialog");
    };
    let DialogKind::Listbox(listbox) = &dialog.kind else {
        panic!("panelize should return to preset list dialog");
    };
    assert!(
        !listbox.items.iter().any(|item| item == &edited),
        "removed preset should no longer be listed"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[cfg(unix)]
#[test]
fn panelize_preset_selection_runs_without_custom_input() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-preset-select-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    fs::write(root.join("a.txt"), "a").expect("must create file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.open_panelize_dialog();
    app.finish_dialog(DialogResult::ListboxSubmitted {
        index: Some(1),
        value: Some(String::from("find . -type f")),
    });
    drain_background(&mut app);

    assert_eq!(
        app.active_panel().panelize_command(),
        Some("find . -type f")
    );
    assert!(
        app.active_panel()
            .entries
            .iter()
            .any(|entry| entry.path == root.join("a.txt")),
        "preset command should populate panel entries"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[cfg(unix)]
#[test]
fn panelize_command_can_be_canceled_while_shell_process_runs() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-cancel-running-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let cancel_clone = Arc::clone(&cancel_flag);
    let cancel_task = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        cancel_clone.store(true, AtomicOrdering::Relaxed);
    });

    let started_at = Instant::now();
    let result = read_panelized_entries_with_cancel(
        &root,
        "sleep 3; printf 'a.txt\\n'",
        SortMode::default(),
        Some(cancel_flag.as_ref()),
    );

    cancel_task
        .join()
        .expect("cancel request thread should finish");
    let error = result.expect_err("panelize command should be canceled");
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    assert_eq!(error.to_string(), PANEL_REFRESH_CANCELED_MESSAGE);
    let elapsed = started_at.elapsed();
    assert!(
        elapsed < Duration::from_secs(1),
        "canceled panelize command should stop quickly, took {elapsed:?}"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn reread_coalesces_previous_refresh_for_same_panel() {
    use std::sync::atomic::Ordering as AtomicOrdering;

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-reread-cancel-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    fs::write(root.join("a.txt"), "a").expect("must create file");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.refresh_active_panel();
    assert_eq!(app.pending_worker_commands.len(), 1);

    let (first_job_id, first_request_id, first_cancel_flag) = match &app.pending_worker_commands[0]
    {
        WorkerCommand::Run(job) => match &job.request {
            JobRequest::RefreshPanel { request_id, .. } => (job.id, *request_id, job.cancel_flag()),
            _ => panic!("expected refresh-panel job request"),
        },
        _ => panic!("expected worker run command"),
    };
    assert!(
        !first_cancel_flag.load(AtomicOrdering::Relaxed),
        "initial refresh should not be canceled"
    );

    app.refresh_active_panel();
    assert!(
        !first_cancel_flag.load(AtomicOrdering::Relaxed),
        "coalesced refresh should keep the existing queued request active"
    );
    assert!(
        !app.pending_worker_commands.iter().any(
            |command| matches!(command, WorkerCommand::Cancel(job_id) if *job_id == first_job_id)
        ),
        "coalesced refresh should not enqueue an explicit cancellation"
    );

    let (coalesced_job_id, second_request_id, second_cancel_flag) = app
        .pending_worker_commands
        .iter()
        .rev()
        .find_map(|command| {
            let WorkerCommand::Run(job) = command else {
                return None;
            };
            let JobRequest::RefreshPanel { request_id, .. } = &job.request else {
                return None;
            };
            Some((job.id, *request_id, job.cancel_flag()))
        })
        .expect("second refresh command should be queued");
    assert_eq!(
        coalesced_job_id, first_job_id,
        "coalescing should reuse the existing queued refresh job id"
    );
    assert!(
        second_request_id > first_request_id,
        "request ids should advance when a refresh request supersedes the queued one"
    );
    assert!(
        !second_cancel_flag.load(AtomicOrdering::Relaxed),
        "coalesced refresh should remain active"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn stale_panel_refresh_event_is_ignored() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-refresh-stale-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.refresh_active_panel();
    app.refresh_active_panel();
    let commands = app.take_pending_worker_commands();
    let refresh_requests: Vec<_> = commands
        .into_iter()
        .filter_map(|command| {
            let WorkerCommand::Run(job) = command else {
                return None;
            };
            match job.request {
                JobRequest::RefreshPanel {
                    panel,
                    cwd,
                    source,
                    sort_mode,
                    request_id,
                    ..
                } => Some((panel, cwd, source, sort_mode, request_id)),
                _ => None,
            }
        })
        .collect();
    assert_eq!(
        refresh_requests.len(),
        1,
        "superseded refreshes should coalesce while still queued"
    );

    let (panel, cwd, source, sort_mode, latest_request_id) = refresh_requests[0].clone();
    let stale_request_id = latest_request_id.saturating_sub(1);
    assert!(
        stale_request_id < latest_request_id,
        "stale request id should be older than the latest one"
    );

    app.handle_background_event(BackgroundEvent::PanelRefreshed {
        panel,
        cwd: cwd.clone(),
        source: source.clone(),
        sort_mode,
        request_id: stale_request_id,
        result: Ok(Vec::new()),
    });
    assert!(
        app.panels[panel.index()].loading,
        "stale refresh result should not clear loading state"
    );

    app.handle_background_event(BackgroundEvent::PanelRefreshed {
        panel,
        cwd,
        source,
        sort_mode,
        request_id: latest_request_id,
        result: Ok(Vec::new()),
    });
    assert!(
        !app.panels[panel.index()].loading,
        "latest refresh result should clear loading state"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn panel_refresh_chunks_preserve_existing_tags_until_final_result() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-refresh-chunk-tags-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let alpha_path = root.join("alpha.txt");
    let beta_path = root.join("beta.txt");
    fs::write(&alpha_path, "alpha").expect("alpha fixture should be writable");
    fs::write(&beta_path, "beta").expect("beta fixture should be writable");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let alpha_index = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == alpha_path)
        .expect("alpha entry should be visible");
    app.active_panel_mut().cursor = alpha_index;
    app.apply(AppCommand::ToggleTag)
        .expect("toggle tag should succeed");
    assert!(
        app.active_panel().is_tagged(&alpha_path),
        "precondition: alpha entry should start tagged"
    );

    app.refresh_active_panel();
    let (panel, cwd, source, sort_mode, request_id) = app
        .take_pending_worker_commands()
        .into_iter()
        .find_map(|command| {
            let WorkerCommand::Run(job) = command else {
                return None;
            };
            let JobRequest::RefreshPanel {
                panel,
                cwd,
                source,
                sort_mode,
                request_id,
                ..
            } = job.request
            else {
                return None;
            };
            Some((panel, cwd, source, sort_mode, request_id))
        })
        .expect("refresh command should be queued");

    app.handle_background_event(BackgroundEvent::PanelEntriesChunk {
        panel,
        cwd: cwd.clone(),
        source: source.clone(),
        sort_mode,
        request_id,
        entries: vec![FileEntry::file(
            String::from("beta.txt"),
            beta_path.clone(),
            4,
            None,
        )],
    });
    assert!(
        app.active_panel().is_tagged(&alpha_path),
        "chunk updates should not prune existing tags before final listing"
    );

    let final_entries =
        read_entries_with_visibility(&cwd, sort_mode, true).expect("listing should build");
    app.handle_background_event(BackgroundEvent::PanelRefreshed {
        panel,
        cwd,
        source,
        sort_mode,
        request_id,
        result: Ok(final_entries),
    });
    assert!(
        app.active_panel().is_tagged(&alpha_path),
        "tag should survive final listing when target is still present"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn refresh_dispatch_failure_clears_loading_state() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-refresh-dispatch-failure-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let panel_index = app.active_panel.index();
    app.refresh_active_panel();
    assert!(
        app.panels[panel_index].loading,
        "refresh should set panel loading before dispatch"
    );

    let refresh_job_id = app
        .take_pending_worker_commands()
        .into_iter()
        .find_map(|command| {
            let WorkerCommand::Run(job) = command else {
                return None;
            };
            matches!(job.request, JobRequest::RefreshPanel { .. }).then_some(job.id)
        })
        .expect("refresh command should be queued");

    app.handle_job_dispatch_failure(refresh_job_id, JobError::dispatch("runtime queue is full"));
    assert!(
        !app.panels[panel_index].loading,
        "failed refresh dispatch should clear loading state"
    );
    assert_eq!(
        app.panel_refresh_job_ids[panel_index], None,
        "failed refresh dispatch should clear tracked refresh job id"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn refresh_cancel_before_start_clears_loading_state() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-refresh-cancel-before-start-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let panel_index = app.active_panel.index();
    app.refresh_active_panel();
    assert!(
        app.panels[panel_index].loading,
        "refresh should set panel loading before dispatch"
    );

    let refresh_job_id = app
        .take_pending_worker_commands()
        .into_iter()
        .find_map(|command| {
            let WorkerCommand::Run(job) = command else {
                return None;
            };
            matches!(job.request, JobRequest::RefreshPanel { .. }).then_some(job.id)
        })
        .expect("refresh command should be queued");

    app.handle_job_event(JobEvent::Finished {
        id: refresh_job_id,
        result: Err(JobError::canceled()),
    });
    assert!(
        !app.panels[panel_index].loading,
        "canceled refresh without background event should clear loading state"
    );
    assert_eq!(
        app.panel_refresh_job_ids[panel_index], None,
        "canceled refresh should clear tracked refresh job id"
    );

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn xmap_mode_applies_to_next_file_manager_command_only() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-xmap-mode-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    assert_eq!(app.key_context(), KeyContext::FileManager);
    app.apply(AppCommand::EnterXMap)
        .expect("xmap mode should activate");
    assert_eq!(app.key_context(), KeyContext::FileManagerXMap);
    app.apply(AppCommand::MoveDown)
        .expect("next command should execute");
    assert_eq!(app.key_context(), KeyContext::FileManager);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn resolve_external_editor_command_prefers_editor_over_visual() {
    let editor = resolve_external_editor_command_with_lookup(|name| match name {
        "EDITOR" => Some(String::from("  nvim  ")),
        "VISUAL" => Some(String::from("vim")),
        _ => None,
    });
    assert_eq!(editor, Some(String::from("nvim")));
}

#[test]
fn resolve_external_editor_command_uses_visual_then_none() {
    let editor = resolve_external_editor_command_with_lookup(|name| match name {
        "EDITOR" => Some(String::from("  ")),
        "VISUAL" => Some(String::from(" code --wait ")),
        _ => None,
    });
    assert_eq!(editor, Some(String::from("code --wait")));

    let missing = resolve_external_editor_command_with_lookup(|_| None);
    assert_eq!(missing, None);
}

#[test]
fn app_command_mapping_is_context_aware() {
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenHelp),
        Some(AppCommand::OpenHelp)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Help, &KeyCommand::Quit),
        Some(AppCommand::CloseHelp)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Help, &KeyCommand::HelpBack),
        Some(AppCommand::HelpBack)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenMenu),
        Some(AppCommand::OpenMenu)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::CursorUp),
        Some(AppCommand::MenuMoveUp)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::CursorDown),
        Some(AppCommand::MenuMoveDown)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::CursorLeft),
        Some(AppCommand::MenuMoveLeft)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::CursorRight),
        Some(AppCommand::MenuMoveRight)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::DialogAccept),
        Some(AppCommand::MenuAccept)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Menu, &KeyCommand::DialogCancel),
        Some(AppCommand::CloseMenu)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::CursorUp),
        Some(AppCommand::MoveUp)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenEntry),
        Some(AppCommand::OpenEntry)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::EditEntry),
        Some(AppCommand::EditEntry)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Listbox, &KeyCommand::CursorUp),
        Some(AppCommand::DialogListboxUp)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Listbox, &KeyCommand::OpenInputDialog),
        Some(AppCommand::PanelizePresetAdd)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Listbox, &KeyCommand::OpenConfirmDialog),
        Some(AppCommand::PanelizePresetEdit)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Listbox, &KeyCommand::Delete),
        Some(AppCommand::PanelizePresetRemove)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::DialogAccept),
        Some(AppCommand::DialogAccept)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::ToggleTag),
        Some(AppCommand::ToggleTag)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::SortNext),
        Some(AppCommand::SortNext)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::Copy),
        Some(AppCommand::Copy)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::Move),
        Some(AppCommand::Move)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::Delete),
        Some(AppCommand::Delete)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::CancelJob),
        Some(AppCommand::CancelJob)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenJobs),
        Some(AppCommand::OpenJobsScreen)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenFindDialog),
        Some(AppCommand::OpenFindDialog)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::CursorDown),
        Some(AppCommand::FindResultsMoveDown)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::OpenEntry),
        Some(AppCommand::FindResultsOpenEntry)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::OpenPanelizeDialog),
        Some(AppCommand::FindResultsPanelize)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::CancelJob),
        Some(AppCommand::CancelJob)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FindResults, &KeyCommand::Quit),
        Some(AppCommand::CloseFindResults)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenTree),
        Some(AppCommand::OpenTree)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::CursorUp),
        Some(AppCommand::TreeMoveUp)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::OpenEntry),
        Some(AppCommand::TreeOpenEntry)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Tree, &KeyCommand::Quit),
        Some(AppCommand::CloseTree)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenHotlist),
        Some(AppCommand::OpenHotlist)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenPanelizeDialog),
        Some(AppCommand::OpenPanelizeDialog)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::OpenSkinDialog),
        Some(AppCommand::OpenSkinDialog)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::FileManager, &KeyCommand::EnterXMap),
        Some(AppCommand::EnterXMap)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Hotlist, &KeyCommand::AddHotlist),
        Some(AppCommand::HotlistAddCurrentDirectory)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Hotlist, &KeyCommand::RemoveHotlist),
        Some(AppCommand::HotlistRemoveSelected)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Hotlist, &KeyCommand::OpenEntry),
        Some(AppCommand::HotlistOpenEntry)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Hotlist, &KeyCommand::Quit),
        Some(AppCommand::CloseHotlist)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Jobs, &KeyCommand::CursorUp),
        Some(AppCommand::JobsMoveUp)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Jobs, &KeyCommand::CursorDown),
        Some(AppCommand::JobsMoveDown)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Jobs, &KeyCommand::CloseJobs),
        Some(AppCommand::CloseJobsScreen)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::Quit),
        Some(AppCommand::CloseViewer)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::Search),
        Some(AppCommand::ViewerSearchForward)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::SearchBackward),
        Some(AppCommand::ViewerSearchBackward)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::SearchContinue),
        Some(AppCommand::ViewerSearchContinue)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::SearchContinueBackward),
        Some(AppCommand::ViewerSearchContinueBackward)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::Goto),
        Some(AppCommand::ViewerGoto)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::ToggleWrap),
        Some(AppCommand::ViewerToggleWrap)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::Viewer, &KeyCommand::ToggleHex),
        Some(AppCommand::ViewerToggleHex)
    );
    assert_eq!(
        AppCommand::from_key_command(KeyContext::ViewerHex, &KeyCommand::ToggleHex),
        Some(AppCommand::ViewerToggleHex)
    );
}
