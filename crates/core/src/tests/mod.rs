use super::*;
use crate::keymap::{KeyCommand, KeyModifiers};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::Duration;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use std::{env, fs};

mod find_tests;
mod mouse_tests;
mod panelize_tests;
mod quick_cd_tests;
mod quick_view_tests;
mod refresh_tests;
mod route_command_tests;
mod selection_size_tests;
mod viewer_tests;

fn file_entry(name: &str) -> FileEntry {
    FileEntry {
        name: name.to_string(),
        path: PathBuf::from(name),
        kind: FileEntryKind::File,
        size: 0,
        modified: None,
        metadata: FileEntryMetadata::default(),
    }
}

fn panel_refresh_result(entries: Vec<FileEntry>) -> PanelRefreshResult {
    PanelRefreshResult {
        entries,
        panelized_entries: None,
    }
}

struct PermissionDeniedProcessBackend;

impl ProcessBackend for PermissionDeniedProcessBackend {
    fn run_shell_command_streaming(
        &self,
        _cwd: &Path,
        _command: &str,
        _cancel_flag: Option<&AtomicBool>,
        _canceled_message: &str,
        _limits: ProcessOutputLimits,
        _stdout_line: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> io::Result<ProcessExit> {
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
                            filter,
                            show_hidden_files,
                            cached_panelized_entries,
                            request_id,
                        } => {
                            let _ = event_tx.send(JobEvent::Started { id: job_id });
                            let cancel_flag = job.cancel_flag();
                            app.handle_background_event(refresh_panel_event(
                                PanelRefreshStreamRequest {
                                    panel: *panel,
                                    cwd: cwd.clone(),
                                    source: source.clone(),
                                    sort_mode: *sort_mode,
                                    filter: filter.clone(),
                                    show_hidden_files: *show_hidden_files,
                                    cached_panelized_entries: cached_panelized_entries.clone(),
                                    request_id: *request_id,
                                },
                                cancel_flag.as_ref(),
                            ));
                            let _ = event_tx.send(JobEvent::Finished {
                                id: job_id,
                                result: Ok(()),
                            });
                        }
                        JobRequest::Find { spec, max_results } => {
                            let spec = spec.clone();
                            let max_results = *max_results;
                            let cancel_flag = job.cancel_flag();
                            let pause_flag = job
                                .find_pause_flag()
                                .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
                            let _ = event_tx.send(JobEvent::Started { id: job_id });
                            let (chunk_tx, chunk_rx) = std::sync::mpsc::channel();
                            let result = run_find_entries(
                                &spec,
                                max_results,
                                cancel_flag.as_ref(),
                                pause_flag.as_ref(),
                                |entries| {
                                    chunk_tx
                                        .send(BackgroundEvent::FindEntriesChunk { job_id, entries })
                                        .is_ok()
                                },
                            )
                            .map_err(|error| JobError::from_message(error.to_string()))
                            .map(|report| {
                                let _ = chunk_tx
                                    .send(BackgroundEvent::FindCompleted { job_id, report });
                            });
                            for event in chunk_rx.try_iter() {
                                app.handle_background_event(event);
                            }
                            let _ = event_tx.send(JobEvent::Finished { id: job_id, result });
                        }
                        JobRequest::QuickCdSearch { spec, request_id } => {
                            let _ = event_tx.send(JobEvent::Started { id: job_id });
                            let cancel_flag = job.cancel_flag();
                            let result =
                                run_quick_cd_search(spec, cancel_flag.as_ref(), |snapshot| {
                                    app.handle_background_event(
                                        BackgroundEvent::QuickCdSearchUpdated {
                                            request_id: *request_id,
                                            snapshot,
                                        },
                                    );
                                    true
                                })
                                .map(|_| ())
                                .map_err(|error| JobError::from_message(error.to_string()));
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
                        JobRequest::LoadQuickView {
                            panel,
                            path,
                            request_id,
                        } => {
                            let _ = event_tx.send(JobEvent::Started { id: job_id });
                            let viewer_result =
                                ViewerState::open(path.clone()).map_err(|error| error.to_string());
                            app.handle_background_event(BackgroundEvent::QuickViewLoaded {
                                panel: *panel,
                                path: path.clone(),
                                request_id: *request_id,
                                result: viewer_result.clone(),
                            });
                            let result = viewer_result.map(|_| ()).map_err(JobError::from_message);
                            let _ = event_tx.send(JobEvent::Finished { id: job_id, result });
                        }
                        JobRequest::MeasureSelection {
                            panel,
                            paths,
                            request_id,
                        } => {
                            let _ = event_tx.send(JobEvent::Started { id: job_id });
                            let cancel_flag = job.cancel_flag();
                            let result = measure_selection_size(paths, cancel_flag.as_ref())
                                .map(|report| {
                                    app.handle_background_event(
                                        BackgroundEvent::SelectionSizeMeasured {
                                            panel: *panel,
                                            request_id: *request_id,
                                            report,
                                        },
                                    );
                                })
                                .map_err(|error| {
                                    if error.kind() == io::ErrorKind::Interrupted {
                                        JobError::canceled()
                                    } else {
                                        JobError::from_io(error)
                                    }
                                });
                            let _ = event_tx.send(JobEvent::Finished { id: job_id, result });
                        }
                        JobRequest::BuildTree {
                            root,
                            max_depth,
                            max_entries,
                        } => {
                            let _ = event_tx.send(JobEvent::Started { id: job_id });
                            let cancel_flag = job.cancel_flag();
                            let result = build_tree_ready_event(
                                job_id,
                                root.clone(),
                                *max_depth,
                                *max_entries,
                                cancel_flag.as_ref(),
                            )
                            .map(|event| app.handle_background_event(event))
                            .map_err(JobError::from_io);
                            let _ = event_tx.send(JobEvent::Finished { id: job_id, result });
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

fn app_with_loaded_panels(root: PathBuf) -> AppState {
    let mut app = AppState::new(root).expect("app should initialize");
    app.refresh_panels();
    drain_background(&mut app);
    app
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
        app.apply(AppCommand::Navigate(
            NavigationTarget::Menu,
            NavigationMotion::Down,
        ))
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
        filter: PanelFilter::default(),
        show_hidden_files: true,
        source: PanelListingSource::Directory,
        panelized_entries: None,
        tagged: HashSet::new(),
        loading: false,
        disk_usage: None,
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

    let mut panel = PanelState::new(child.clone()).expect("panel should initialize");
    panel.refresh().expect("panel listing should load");
    let first = panel.entries.first().expect("entries should not be empty");
    assert_eq!(first.name, "..");
    assert!(first.is_parent());
    assert!(first.is_dir());
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
        symlink_entry.is_dir(),
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
    #[cfg(unix)]
    {
        assert!(file_entry.metadata.mode.is_some());
        assert!(file_entry.metadata.hard_links.is_some());
        assert!(file_entry.metadata.user_id.is_some());
        assert!(file_entry.metadata.group_id.is_some());
        assert!(file_entry.metadata.inode.is_some());
    }

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
        filter: PanelFilter::default(),
        show_hidden_files: true,
        source: PanelListingSource::Directory,
        panelized_entries: None,
        tagged: HashSet::new(),
        loading: false,
        disk_usage: None,
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
        filter: PanelFilter::default(),
        show_hidden_files: true,
        source: PanelListingSource::Directory,
        panelized_entries: None,
        tagged: HashSet::new(),
        loading: false,
        disk_usage: None,
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
        filter: PanelFilter::default(),
        show_hidden_files: true,
        source: PanelListingSource::Directory,
        panelized_entries: None,
        tagged: HashSet::new(),
        loading: false,
        disk_usage: None,
    };

    panel.sort_mode.field = SortField::Name;
    panel.sort_mode.reverse = false;
    assert_eq!(panel.sort_label(), "name asc");

    panel.sort_mode.field = panel.sort_mode.field.next();
    assert_eq!(panel.sort_mode.field, SortField::Version);

    panel.sort_mode.reverse = true;
    assert_eq!(panel.sort_label(), "version desc");
}

#[test]
fn version_sort_compares_arbitrarily_large_numeric_runs_without_parsing() {
    let mut entries = vec![
        file_entry("release10"),
        file_entry("release0002"),
        file_entry("release2"),
        file_entry("release999999999999999999999999999999"),
        file_entry("release11"),
    ];

    sort_file_entries(
        &mut entries,
        SortMode {
            field: SortField::Version,
            reverse: false,
        },
    );

    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        [
            "release2",
            "release0002",
            "release10",
            "release11",
            "release999999999999999999999999999999",
        ]
    );
}

#[test]
fn unsorted_mode_stably_partitions_directories_and_reverses_within_each_partition() {
    let file_a = file_entry("file-a");
    let mut directory_a = file_entry("directory-a");
    directory_a.kind = FileEntryKind::Directory;
    let file_b = file_entry("file-b");
    let mut directory_b = file_entry("directory-b");
    directory_b.kind = FileEntryKind::Directory;
    let mut entries = vec![file_a.clone(), directory_a, file_b.clone(), directory_b];

    sort_file_entries(
        &mut entries,
        SortMode {
            field: SortField::Unsorted,
            reverse: false,
        },
    );
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["directory-a", "directory-b", "file-a", "file-b"]
    );

    sort_file_entries(
        &mut entries,
        SortMode {
            field: SortField::Unsorted,
            reverse: true,
        },
    );
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["directory-b", "directory-a", "file-b", "file-a"]
    );
}

#[test]
fn metadata_sort_orders_use_the_cached_entry_snapshot() {
    let earlier = UNIX_EPOCH + Duration::from_secs(10);
    let later = UNIX_EPOCH + Duration::from_secs(20);
    let mut first = file_entry("zeta.rs");
    first.size = 20;
    first.modified = Some(later);
    first.metadata.accessed = Some(earlier);
    first.metadata.changed = Some(later);
    first.metadata.inode = Some(2);
    let mut second = file_entry("alpha.txt");
    second.size = 10;
    second.modified = Some(earlier);
    second.metadata.accessed = Some(later);
    second.metadata.changed = Some(earlier);
    second.metadata.inode = Some(1);

    for (field, expected_first) in [
        (SortField::Extension, "zeta.rs"),
        (SortField::Modified, "alpha.txt"),
        (SortField::Accessed, "zeta.rs"),
        (SortField::Changed, "alpha.txt"),
        (SortField::Size, "alpha.txt"),
        (SortField::Inode, "alpha.txt"),
    ] {
        let mut entries = vec![first.clone(), second.clone()];
        sort_file_entries(
            &mut entries,
            SortMode {
                field,
                reverse: false,
            },
        );
        assert_eq!(entries[0].name, expected_first, "unexpected {field:?} sort");
    }
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

    let mut app = app_with_loaded_panels(root.clone());
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

fn submit_mkdir_dialog(app: &mut AppState, name: &str) {
    app.apply(AppCommand::OpenInputDialog)
        .expect("mkdir dialog should open");
    for ch in name.chars() {
        app.apply(AppCommand::DialogInputChar(ch))
            .expect("typing should be accepted");
    }
    app.apply(AppCommand::DialogAccept)
        .expect("mkdir dialog should submit");
}

#[test]
fn mkdir_dialog_queues_mkdir_job() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-mkdir-dialog-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = app_with_loaded_panels(root.clone());
    submit_mkdir_dialog(&mut app, "newdir");

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
fn completed_panel_mkdir_enters_the_directory_in_its_originating_panel() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-mkdir-enter-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = app_with_loaded_panels(root.clone());
    submit_mkdir_dialog(&mut app, "newdir");

    app.apply(AppCommand::SwitchPanel)
        .expect("the user may switch panels while mkdir runs");
    drain_background(&mut app);

    let created = root.join("newdir");
    assert!(created.is_dir(), "mkdir job should create the directory");
    assert_eq!(
        app.panels[ActivePanel::Left.index()].cwd,
        created,
        "the panel that started mkdir should enter the created directory"
    );
    assert_eq!(
        app.active_panel,
        ActivePanel::Right,
        "job completion should not steal focus after the user switches panels"
    );
    assert_eq!(app.active_panel().cwd, root);
    assert!(app.status_line.contains("entered it in the left panel"));

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn failed_panel_mkdir_does_not_change_directories() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-mkdir-failure-{stamp}"));
    fs::create_dir_all(root.join("existing")).expect("must create existing directory");

    let mut app = app_with_loaded_panels(root.clone());
    submit_mkdir_dialog(&mut app, "existing");
    drain_background(&mut app);

    assert_eq!(app.active_panel().cwd, root);
    assert!(app.status_line.contains("failed"));
    assert!(app.panel_mkdirs.pending.is_empty());

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn superseded_panel_mkdir_completion_cannot_override_the_latest_destination() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-mkdir-order-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = app_with_loaded_panels(root.clone());
    submit_mkdir_dialog(&mut app, "first");
    submit_mkdir_dialog(&mut app, "latest");

    let jobs = app
        .take_pending_worker_commands()
        .into_iter()
        .filter_map(|command| match command {
            WorkerCommand::Run(job) => Some(*job),
            WorkerCommand::Cancel(_) | WorkerCommand::Shutdown => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(jobs.len(), 2);
    for job in &jobs {
        let JobRequest::Mkdir { path } = &job.request else {
            panic!("expected mkdir request");
        };
        fs::create_dir(path).expect("must create requested directory");
    }

    for job in jobs.iter().rev() {
        app.handle_job_event(JobEvent::Started { id: job.id });
        app.handle_job_event(JobEvent::Finished {
            id: job.id,
            result: Ok(()),
        });
    }

    assert_eq!(app.active_panel().cwd, root.join("latest"));
    assert!(app.panel_mkdirs.pending.is_empty());

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn completed_panel_mkdir_cannot_override_newer_navigation_in_the_same_panel() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-mkdir-navigation-{stamp}"));
    let newer_destination = root.join("newer-destination");
    fs::create_dir_all(&newer_destination).expect("must create navigation fixture");

    let mut app = app_with_loaded_panels(root.clone());
    submit_mkdir_dialog(&mut app, "created");
    assert!(
        app.set_active_panel_directory(newer_destination.clone())
            .expect("newer navigation should be valid")
    );

    drain_background(&mut app);

    assert!(root.join("created").is_dir());
    assert_eq!(app.active_panel().cwd, newer_destination);
    assert!(app.status_line.contains("kept the newer left panel state"));

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

    let mut app = app_with_loaded_panels(root.clone());
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

    let mut app = app_with_loaded_panels(root.clone());
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

    let mut app = app_with_loaded_panels(root.clone());
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

    let mut app = app_with_loaded_panels(root.clone());
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

    let mut app = app_with_loaded_panels(root.clone());
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
fn quick_cd_menu_prefers_the_portable_slash_shortcut() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-quick-cd-menu-shortcut-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = app_with_loaded_panels(root.clone());
    let keymap = Keymap::bundled_mc_default().expect("bundled keymap should parse");
    app.set_keybinding_hints_from_keymap(&keymap);
    let quick_cd = FILE_MENU_ENTRIES
        .iter()
        .find(|entry| entry.command == AppCommand::OpenQuickCd)
        .expect("Quick cd entry should exist");

    assert_eq!(app.menu_entry_shortcut_label(quick_cd), "/");

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

    let mut app = app_with_loaded_panels(root.clone());
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

    let mut app = app_with_loaded_panels(root.clone());
    app.apply(AppCommand::OpenMenuBarAt(2))
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
    app.apply(AppCommand::OpenMenuBarAt(0))
        .expect("left menu should open");
    move_menu_selection_to_label(&mut app, "Encoding...");
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
fn user_menu_command_is_reserved_for_milestone_five() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-user-menu-placeholder-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenUserMenu)
        .expect("user menu placeholder should be handled");

    assert_eq!(app.key_context(), KeyContext::FileManager);
    assert!(app.status_line.contains("planned for Milestone 5"));

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn side_menu_info_mode_targets_its_named_panel() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-side-info-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = app_with_loaded_panels(root.clone());
    app.active_panel = ActivePanel::Right;
    app.apply(AppCommand::OpenMenuBarAt(0))
        .expect("left menu should open");
    move_menu_selection_to_label(&mut app, "Info");
    app.apply(AppCommand::MenuAccept)
        .expect("left info mode should open");

    assert_eq!(app.panel_view_mode(ActivePanel::Left), PanelViewMode::Info);
    assert_eq!(
        app.panel_view_mode(ActivePanel::Right),
        PanelViewMode::Listing
    );
    assert_eq!(
        app.active_panel,
        ActivePanel::Right,
        "the listing that drives info should remain active"
    );
    assert!(
        !app.toggle_active_panel(),
        "file-manager focus must not enter an info-only panel"
    );
    assert_eq!(app.active_panel, ActivePanel::Right);

    app.apply(AppCommand::OpenMenuBarAt(0))
        .expect("left menu should reopen");
    move_menu_selection_to_label(&mut app, "File listing");
    app.apply(AppCommand::MenuAccept)
        .expect("left file listing should be restored");
    assert_eq!(
        app.panel_view_mode(ActivePanel::Left),
        PanelViewMode::Listing
    );

    fs::remove_dir_all(root).expect("must remove temp root");
}

#[test]
fn side_menu_rescan_targets_its_named_panel() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-side-rescan-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.active_panel = ActivePanel::Left;
    app.apply(AppCommand::OpenMenuBarAt(4))
        .expect("right menu should open");
    move_menu_selection_to_label(&mut app, "Rescan");
    app.apply(AppCommand::MenuAccept)
        .expect("right rescan should be accepted");

    let requests = app.take_pending_worker_commands();
    assert!(requests.iter().any(|command| matches!(
        command,
        WorkerCommand::Run(job)
            if matches!(job.request, JobRequest::RefreshPanel {
                panel: ActivePanel::Right,
                ..
            })
    )));
    assert_eq!(
        app.active_panel,
        ActivePanel::Left,
        "rescan should not steal focus from the active panel"
    );

    fs::remove_dir_all(root).expect("must remove temp root");
}

#[test]
fn xmap_info_and_quick_view_commands_target_the_other_panel() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-xmap-panel-view-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = app_with_loaded_panels(root.clone());
    assert_eq!(app.active_panel, ActivePanel::Left);
    app.apply(AppCommand::SetOtherPanelView(PanelViewMode::Info))
        .expect("xmap info should open");
    assert_eq!(app.panel_view_mode(ActivePanel::Right), PanelViewMode::Info);
    assert_eq!(app.active_panel, ActivePanel::Left);

    app.apply(AppCommand::Panel(
        ActivePanel::Right,
        PanelCommand::SetView(PanelViewMode::Listing),
    ))
    .expect("right listing should be restored");
    app.apply(AppCommand::SetOtherPanelView(PanelViewMode::QuickView))
        .expect("xmap quick view should open");
    assert_eq!(
        app.panel_view_mode(ActivePanel::Right),
        PanelViewMode::QuickView
    );
    assert_eq!(app.active_panel, ActivePanel::Left);

    fs::remove_dir_all(root).expect("must remove temp root");
}

#[test]
fn file_listing_mode_leaves_panelized_results_recoverably() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-side-file-listing-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    let result = file_entry("result.txt");
    let result_path = result.path.clone();
    {
        let panel = &mut app.panels[ActivePanel::Left.index()];
        panel.source = PanelListingSource::Panelize {
            command: String::from("find . -type f"),
        };
        panel.entries = vec![result];
        panel.cursor = 0;
        panel.loading = false;
    }

    app.apply(AppCommand::Panel(
        ActivePanel::Left,
        PanelCommand::SetView(PanelViewMode::Listing),
    ))
    .expect("file listing mode should be restored");

    assert_eq!(
        app.panels[ActivePanel::Left.index()].source,
        PanelListingSource::Directory
    );
    assert!(app.panels[ActivePanel::Left.index()].loading);
    let stored = app.panelized_result_history[ActivePanel::Left.index()]
        .as_ref()
        .expect("panelized results should remain recoverable");
    assert_eq!(stored.entries[0].path, result_path);
    assert!(
        app.take_pending_worker_commands()
            .iter()
            .any(|command| matches!(
                command,
                WorkerCommand::Run(job)
                    if matches!(job.request, JobRequest::RefreshPanel {
                        panel: ActivePanel::Left,
                        ..
                    })
            ))
    );

    fs::remove_dir_all(root).expect("must remove temp root");
}

#[test]
fn listing_format_dialog_updates_only_its_named_panel() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-listing-format-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = app_with_loaded_panels(root.clone());
    app.active_panel = ActivePanel::Right;
    app.apply(AppCommand::OpenMenuBarAt(0))
        .expect("left menu should open");
    move_menu_selection_to_label(&mut app, "Listing format...");
    app.apply(AppCommand::MenuAccept)
        .expect("listing format dialog should open");
    assert_eq!(app.key_context(), KeyContext::Listbox);

    app.apply(AppCommand::DialogListboxSelectAt(1))
        .expect("brief format should be selected");
    app.apply(AppCommand::DialogAccept)
        .expect("brief format should be applied");

    assert_eq!(
        app.panel_listing_format(ActivePanel::Left),
        PanelListingFormat::Brief
    );
    assert_eq!(
        app.panel_listing_format(ActivePanel::Right),
        PanelListingFormat::Full,
        "the other panel format must remain unchanged"
    );
    assert_eq!(
        app.settings().panel_options.listing_formats,
        [PanelListingFormat::Brief, PanelListingFormat::Full]
    );
    assert!(app.settings().save_setup.dirty);
    assert_eq!(app.active_panel, ActivePanel::Left);

    fs::remove_dir_all(root).expect("must remove temp root");
}

#[test]
fn listing_format_shortcut_cycles_the_active_panel_only() {
    let root = env::temp_dir().join(format!(
        "rc-listing-format-cycle-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("must create temp root");
    let mut app = app_with_loaded_panels(root.clone());
    app.active_panel = ActivePanel::Right;

    for expected in [
        PanelListingFormat::Brief,
        PanelListingFormat::Long,
        PanelListingFormat::Full,
    ] {
        app.apply(AppCommand::CycleListingFormat)
            .expect("listing format should cycle");
        assert_eq!(app.panel_listing_format(ActivePanel::Right), expected);
        assert_eq!(
            app.panel_listing_format(ActivePanel::Left),
            PanelListingFormat::Full
        );
    }

    assert!(app.settings().save_setup.dirty);
    fs::remove_dir_all(root).expect("must remove temp root");
}

#[test]
fn sort_order_dialog_applies_field_and_reverse_to_only_its_named_panel() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-sort-order-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = app_with_loaded_panels(root.clone());
    app.apply(AppCommand::Panel(
        ActivePanel::Right,
        PanelCommand::OpenSortOrder,
    ))
    .expect("sort order dialog should open");
    app.apply(AppCommand::DialogListboxSelectAt(
        SortField::Version.index(),
    ))
    .expect("version sort should be selected");
    app.apply(AppCommand::DialogInputChar(' '))
        .expect("reverse should toggle");
    let Route::Dialog(dialog) = app.top_route() else {
        panic!("sort order dialog should remain open");
    };
    let DialogKind::Listbox(listbox) = &dialog.kind else {
        panic!("sort order should use a listbox");
    };
    assert!(
        listbox
            .footer_hint
            .as_deref()
            .is_some_and(|footer| footer.contains("Reverse: on"))
    );

    app.apply(AppCommand::DialogAccept)
        .expect("sort order should be applied");
    let expected = SortMode {
        field: SortField::Version,
        reverse: true,
    };
    assert_eq!(app.panels[ActivePanel::Right.index()].sort_mode, expected);
    assert_eq!(
        app.panels[ActivePanel::Left.index()].sort_mode,
        SortMode::default()
    );
    assert_eq!(
        app.settings().panel_options.sort_modes,
        [SortMode::default(), expected]
    );
    assert!(
        app.take_pending_worker_commands()
            .iter()
            .any(|command| matches!(
                command,
                WorkerCommand::Run(job)
                    if matches!(job.request, JobRequest::RefreshPanel {
                        panel: ActivePanel::Right,
                        sort_mode,
                        ..
                    } if sort_mode == expected)
            ))
    );
    assert_eq!(
        app.active_panel,
        ActivePanel::Left,
        "sorting a named panel should not steal focus"
    );

    fs::remove_dir_all(root).expect("must remove temp root");
}

#[test]
fn filter_dialog_updates_only_its_named_panel_and_queues_that_filter() {
    let root = env::temp_dir().join(format!(
        "rc-filter-dialog-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("must create temp root");
    let mut app = app_with_loaded_panels(root.clone());
    let filter = PanelFilter {
        pattern: String::from("^README\\.md$"),
        files_only: false,
        name_mode: FindNameMode::Regex,
        case_sensitive: false,
    };

    app.apply(AppCommand::Panel(
        ActivePanel::Right,
        PanelCommand::OpenFilter,
    ))
    .expect("filter dialog should open");
    assert!(matches!(
        app.top_route(),
        Route::Dialog(dialog) if matches!(dialog.kind, DialogKind::Filter(_))
    ));
    app.finish_dialog(DialogResult::FilterSubmitted(filter.clone()));

    assert_eq!(app.panels[ActivePanel::Right.index()].filter(), &filter);
    assert_eq!(
        app.panels[ActivePanel::Left.index()].filter(),
        &PanelFilter::default()
    );
    assert_eq!(app.settings().panel_options.filters[1], filter);
    assert!(app.settings().save_setup.dirty);
    assert!(app.pending_worker_commands.iter().any(|command| matches!(
        command,
        WorkerCommand::Run(job)
            if matches!(&job.request, JobRequest::RefreshPanel {
                panel: ActivePanel::Right,
                filter: queued_filter,
                ..
            } if queued_filter == &filter)
    )));
    assert_eq!(app.active_panel, ActivePanel::Left);

    fs::remove_dir_all(root).expect("must remove temp root");
}

#[test]
fn invalid_filter_reopens_the_dialog_without_mutating_panel_settings() {
    let root = env::temp_dir().join(format!(
        "rc-invalid-filter-dialog-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("must create temp root");
    let mut app = app_with_loaded_panels(root.clone());
    app.apply(AppCommand::Panel(
        ActivePanel::Left,
        PanelCommand::OpenFilter,
    ))
    .expect("filter dialog should open");
    let invalid = PanelFilter {
        pattern: String::from("["),
        ..PanelFilter::default()
    };

    app.finish_dialog(DialogResult::FilterSubmitted(invalid.clone()));

    let Route::Dialog(dialog) = app.top_route() else {
        panic!("invalid filter should reopen its dialog");
    };
    let DialogKind::Filter(form) = &dialog.kind else {
        panic!("filter form should be restored");
    };
    assert_eq!(form.to_filter(), invalid);
    assert_eq!(
        app.active_panel().filter(),
        &PanelFilter::default(),
        "invalid input must not change the live filter"
    );
    assert!(app.status_line.contains("Filter not applied"));

    fs::remove_dir_all(root).expect("must remove temp root");
}

#[test]
fn filtering_preserves_the_selected_path_and_tags_hidden_entries_non_destructively() {
    let root = env::temp_dir().join(format!(
        "rc-filter-selection-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("must create temp root");
    for name in ["alpha.rs", "notes.txt", "zulu.rs"] {
        fs::write(root.join(name), name).expect("fixture should be written");
    }
    let mut app = app_with_loaded_panels(root.clone());
    let notes_path = root.join("notes.txt");
    let zulu_path = root.join("zulu.rs");
    app.active_panel_mut().cursor = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == notes_path)
        .expect("text fixture should be listed");
    assert!(app.active_panel_mut().toggle_tag_on_cursor());
    app.active_panel_mut().cursor = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == zulu_path)
        .expect("selected fixture should be listed");

    app.apply(AppCommand::Panel(
        ActivePanel::Left,
        PanelCommand::OpenFilter,
    ))
    .expect("filter dialog should open");
    app.finish_dialog(DialogResult::FilterSubmitted(PanelFilter {
        pattern: String::from("*.rs"),
        ..PanelFilter::default()
    }));
    drain_background(&mut app);

    assert_eq!(
        app.active_panel().selected_entry().map(|entry| &entry.path),
        Some(&zulu_path),
        "filtering should retain the selected path when it remains visible"
    );
    assert!(
        app.active_panel().is_tagged(&notes_path),
        "a view filter must not destructively clear hidden tags"
    );
    assert!(
        app.active_panel()
            .tagged_paths_in_display_order()
            .is_empty()
    );
    assert_eq!(
        app.selected_operation_paths(),
        std::slice::from_ref(&notes_path),
        "operations must target preserved tags even when every tag is filtered out"
    );
    app.active_panel_mut().invert_tags();
    assert!(
        app.active_panel().is_tagged(&notes_path),
        "inverting the filtered view must preserve tags on hidden entries"
    );
    assert_eq!(
        app.selected_operation_paths(),
        [root.join("alpha.rs"), zulu_path.clone(), notes_path.clone()],
        "visible tags should retain display order before deterministically ordered hidden tags"
    );

    app.apply(AppCommand::Panel(
        ActivePanel::Left,
        PanelCommand::OpenFilter,
    ))
    .expect("filter dialog should reopen");
    app.finish_dialog(DialogResult::FilterSubmitted(PanelFilter::default()));
    drain_background(&mut app);
    assert!(app.active_panel().is_tagged(&notes_path));
    assert_eq!(
        app.active_panel().tagged_paths_in_display_order(),
        [root.join("alpha.rs"), notes_path, zulu_path]
    );

    fs::remove_dir_all(root).expect("must remove temp root");
}

#[cfg(unix)]
#[test]
fn changing_a_panelized_filter_reuses_cached_results_without_rerunning_the_command() {
    let root = env::temp_dir().join(format!(
        "rc-cached-panel-filter-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("must create temp root");
    fs::write(root.join("alpha.rs"), "alpha").expect("Rust fixture should be written");
    fs::write(root.join("notes.txt"), "notes").expect("text fixture should be written");
    let marker = root.join("panelize-runs.log");
    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.start_panelize_command(String::from(
        "printf 'notes.txt\\nalpha.rs\\n'; printf x >> panelize-runs.log",
    ));
    drain_background(&mut app);
    assert_eq!(
        fs::read_to_string(&marker).expect("marker should exist"),
        "x"
    );

    app.apply(AppCommand::Panel(
        ActivePanel::Left,
        PanelCommand::OpenFilter,
    ))
    .expect("filter dialog should open");
    app.finish_dialog(DialogResult::FilterSubmitted(PanelFilter {
        pattern: String::from("*.rs"),
        files_only: false,
        ..PanelFilter::default()
    }));
    assert!(app.pending_worker_commands.iter().any(|command| matches!(
        command,
        WorkerCommand::Run(job)
            if matches!(&job.request, JobRequest::RefreshPanel {
                cached_panelized_entries: Some(_),
                ..
            })
    )));
    drain_background(&mut app);

    assert_eq!(
        fs::read_to_string(&marker).expect("marker should remain"),
        "x"
    );
    assert_eq!(
        app.active_panel()
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["alpha.rs"]
    );

    app.apply(AppCommand::Panel(
        ActivePanel::Left,
        PanelCommand::OpenFilter,
    ))
    .expect("filter dialog should reopen");
    app.finish_dialog(DialogResult::FilterSubmitted(PanelFilter::default()));
    drain_background(&mut app);
    assert_eq!(
        fs::read_to_string(&marker).expect("marker should remain"),
        "x"
    );
    assert_eq!(app.active_panel().entries.len(), 2);
    assert!(app.active_panel().panelized_entries.is_some());

    app.set_panel_sort_mode(
        ActivePanel::Left,
        SortMode {
            field: SortField::Unsorted,
            reverse: false,
        },
    );
    app.queue_panel_refresh(ActivePanel::Left);
    drain_background(&mut app);
    assert_eq!(
        app.active_panel()
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["notes.txt", "alpha.rs"],
        "cached panelize data should retain the source command's discovery order"
    );
    assert_eq!(
        fs::read_to_string(&marker).expect("marker should remain"),
        "x",
        "changing to unsorted order must not rerun the panelize command"
    );

    app.set_panel_sort_mode(
        ActivePanel::Left,
        SortMode {
            field: SortField::Name,
            reverse: false,
        },
    );
    app.queue_panel_refresh(ActivePanel::Left);
    drain_background(&mut app);
    app.set_active_panel_directory(root.join(".."))
        .expect("parent should be inspectable");
    drain_background(&mut app);
    app.restore_panelized_results_for(ActivePanel::Left);
    app.set_panel_sort_mode(
        ActivePanel::Left,
        SortMode {
            field: SortField::Unsorted,
            reverse: false,
        },
    );
    app.queue_panel_refresh(ActivePanel::Left);
    drain_background(&mut app);
    assert_eq!(
        app.active_panel()
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["notes.txt", "alpha.rs"],
        "restored panelize data should retain the source command's discovery order"
    );
    assert_eq!(
        fs::read_to_string(&marker).expect("marker should remain"),
        "x",
        "sorting restored results must not rerun the panelize command"
    );

    fs::remove_dir_all(root).expect("must remove temp root");
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
    assert_eq!(
        left.entries[1].command,
        AppCommand::Panel(
            ActivePanel::Left,
            PanelCommand::SetView(PanelViewMode::QuickView)
        )
    );
    assert_eq!(
        right.entries[1].command,
        AppCommand::Panel(
            ActivePanel::Right,
            PanelCommand::SetView(PanelViewMode::QuickView)
        )
    );
    assert_eq!(
        left.entries[2].command,
        AppCommand::Panel(
            ActivePanel::Left,
            PanelCommand::SetView(PanelViewMode::Info)
        )
    );
    assert_eq!(
        right.entries[2].command,
        AppCommand::Panel(
            ActivePanel::Right,
            PanelCommand::SetView(PanelViewMode::Info)
        )
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
    app.apply(AppCommand::DialogListboxDown)
        .expect("settings selection should move");
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

    let mut app = app_with_loaded_panels(root.clone());
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

    let mut app = app_with_loaded_panels(root.clone());
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

    let mut app = app_with_loaded_panels(root.clone());
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
fn confirmation_options_toggle_hotlist_deletion_prompt() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-confirm-hotlist-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");

    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenOptionsConfirmation)
        .expect("confirmation options should open");
    for _ in 0..3 {
        app.apply(AppCommand::DialogListboxDown)
            .expect("selection should move down");
    }
    app.apply(AppCommand::DialogAccept)
        .expect("hotlist confirmation should toggle");

    assert!(!app.settings().confirmation.confirm_hotlist_delete);
    assert!(app.status_line.contains("Confirm hotlist deletion: off"));

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
    app.apply(AppCommand::OpenMenuBarAt(2))
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
    let commands = app.commands_for_left_click(8, 0, 120, 40);
    assert_eq!(
        commands,
        Some(MouseClickCommands {
            primary: AppCommand::OpenMenuBarAt(1),
            activation: None,
            target: MouseClickTarget::Command(AppCommand::OpenMenuBarAt(1)),
        })
    );

    app.apply(AppCommand::OpenMenuBarAt(1))
        .expect("menu route should open");
    assert_eq!(
        app.commands_for_left_click(8, 3, 120, 40),
        Some(MouseClickCommands {
            primary: AppCommand::MenuSelectAt(1),
            activation: None,
            target: MouseClickTarget::Command(AppCommand::MenuSelectAt(1)),
        })
    );
    assert_eq!(
        app.commands_for_left_click(100, 20, 120, 40),
        Some(MouseClickCommands {
            primary: AppCommand::CloseMenu,
            activation: None,
            target: MouseClickTarget::Command(AppCommand::CloseMenu),
        })
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

    let mut app = app_with_loaded_panels(root.clone());
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

    let mut app = app_with_loaded_panels(root.clone());
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
                assert_eq!(*overwrite, app.overwrite_policy());
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

    let mut app = app_with_loaded_panels(root.clone());
    let source_index = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == source)
        .expect("source entry should be visible");
    app.active_panel_mut().cursor = source_index;

    app.start_copy_dialog();
    app.finish_dialog(DialogResult::InputSubmitted(String::from("dest")));

    match app.top_route() {
        Route::Dialog(dialog) => match dialog.action() {
            Some(PendingDialogAction::TransferOverwrite {
                destination_dir, ..
            }) => {
                assert_eq!(destination_dir, &root.join("dest"));
            }
            other => panic!("expected transfer overwrite action, got {other:?}"),
        },
        other => panic!("expected transfer overwrite dialog, got {other:?}"),
    }

    fs::remove_dir_all(&root).expect("must remove temp root");
}
