use super::*;
use std::io::Write;
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

static NEXT_FIXTURE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    zip: PathBuf,
    output: PathBuf,
}

impl Fixture {
    fn new(entries: &[(&str, &[u8])]) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = NEXT_FIXTURE_ID.fetch_add(1, AtomicOrdering::Relaxed);
        let root =
            env::temp_dir().join(format!("rc-zip-{}-{sequence}-{stamp}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let zip = root.join("example.ZIP");
        let output = root.join("output");
        fs::create_dir(&output).unwrap();
        let mut writer = ZipWriter::new(fs::File::create(&zip).unwrap());
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, content) in entries {
            if name.ends_with('/') {
                writer.add_directory(*name, options).unwrap();
            } else {
                writer.start_file(*name, options).unwrap();
                writer.write_all(content).unwrap();
            }
        }
        writer.finish().unwrap();
        Self { root, zip, output }
    }

    fn app(&self) -> AppState {
        let mut app = app_with_loaded_panels(self.root.clone());
        app.settings.confirmation.confirm_overwrite = false;
        app.panels[1].cwd = self.output.clone();
        app.panels[1].refresh().unwrap();
        select(&mut app, &self.zip);
        app.apply(AppCommand::OpenEntry).unwrap();
        drain_background(&mut app);
        assert!(app.active_panel().is_archive(), "{}", app.status_line);
        app
    }

    fn copy(&self, sources: &[&str], policy: OverwritePolicy) -> Result<(), JobError> {
        let mut manager = JobManager::new();
        let job = manager.enqueue(JobRequest::ExtractArchive {
            location: ArchiveLocation::root(self.zip.clone()),
            sources: sources.iter().map(|path| self.zip.join(path)).collect(),
            destination_dir: self.output.clone(),
            overwrite: policy,
        });
        let (tx, rx) = std::sync::mpsc::channel();
        execute_worker_job(job, &tx);
        drop(tx);
        rx.into_iter()
            .find_map(|event| match event {
                JobEvent::Finished { result, .. } => Some(result),
                _ => None,
            })
            .unwrap()
    }

    fn corrupt(&self, member: &str) {
        let mut archive = ZipArchive::new(fs::File::open(&self.zip).unwrap()).unwrap();
        let start = archive.by_name(member).unwrap().data_start().unwrap();
        drop(archive);
        let mut bytes = fs::read(&self.zip).unwrap();
        bytes[start as usize] ^= 0xff;
        fs::write(&self.zip, bytes).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn select(app: &mut AppState, path: &Path) {
    app.active_panel_mut().cursor = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("{} is not listed", path.display()));
}

#[test]
fn file_history_reopens_archive_member_after_leaving_archive() {
    let fixture = Fixture::new(&[("member.txt", b"archive history")]);
    let mut app = fixture.app();
    select(&mut app, &fixture.zip.join("member.txt"));
    app.apply(AppCommand::ViewEntry).unwrap();
    drain_background(&mut app);
    assert_eq!(app.active_viewer().unwrap().bytes, b"archive history");
    app.apply(AppCommand::CloseViewer).unwrap();
    app.apply(AppCommand::CdUp).unwrap();
    drain_background(&mut app);
    assert!(!app.active_panel().is_archive());
    app.apply(AppCommand::OpenFileHistory).unwrap();
    app.apply(AppCommand::DialogAccept).unwrap();
    drain_background(&mut app);
    assert_eq!(app.active_viewer().unwrap().bytes, b"archive history");
}

fn enter(app: &mut AppState, path: &Path) {
    select(app, path);
    app.apply(AppCommand::OpenEntry).unwrap();
    drain_background(app);
}

#[test]
fn zip_navigation_preserves_quick_cd_local_history() {
    let fixture = Fixture::new(&[("nested/deep/file", b"contents")]);
    super::quick_cd_tests::assert_archive_navigation_preserves_local_history(
        &fixture.root,
        &fixture.zip,
    );
}

#[test]
fn zip_group_selection_tags_synthetic_member_paths() {
    let fixture = Fixture::new(&[("first.txt", b"one"), ("second.md", b"two")]);
    let mut app = fixture.app();
    app.apply(AppCommand::SelectGroup).unwrap();
    app.finish_dialog(DialogResult::FilterSubmitted(PanelFilter {
        pattern: String::from("*.txt"),
        ..PanelFilter::default()
    }));
    assert_eq!(
        app.active_panel().tagged_paths(),
        vec![fixture.zip.join("first.txt")]
    );
    assert!(app.active_panel().is_archive());
    app.apply(AppCommand::UnselectGroup).unwrap();
    app.finish_dialog(DialogResult::FilterSubmitted(PanelFilter {
        pattern: String::from("*"),
        ..PanelFilter::default()
    }));
    assert!(app.active_panel().tagged_paths().is_empty());
}

#[test]
fn zip_sort_dialog_orders_members_at_the_root_and_in_subdirectories() {
    let fixture = Fixture::new(&[]);
    let mut writer = ZipWriter::new(fs::File::create(&fixture.zip).unwrap());
    for prefix in ["", "nested/"] {
        for (name, size, month) in [
            ("file10.bin", 3, 3),
            ("file2.txt", 1, 1),
            ("a.md", 2, 2),
            ("z.txt", 0, 4),
        ] {
            let options = SimpleFileOptions::default().last_modified_time(
                zip::DateTime::from_date_and_time(2024, month, 1, 0, 0, 0).unwrap(),
            );
            writer
                .start_file(format!("{prefix}{name}"), options)
                .unwrap();
            writer.write_all(&vec![b'x'; size]).unwrap();
        }
    }
    writer.finish().unwrap();
    let mut app = fixture.app();
    for directory in ["", "nested"] {
        if !directory.is_empty() {
            enter(&mut app, &fixture.zip.join(directory));
        }
        for (field, expected) in [
            (
                SortField::Name,
                ["a.md", "file10.bin", "file2.txt", "z.txt"],
            ),
            (
                SortField::Version,
                ["a.md", "file2.txt", "file10.bin", "z.txt"],
            ),
            (
                SortField::Extension,
                ["file10.bin", "a.md", "file2.txt", "z.txt"],
            ),
            (
                SortField::Modified,
                ["file2.txt", "a.md", "file10.bin", "z.txt"],
            ),
            (
                SortField::Size,
                ["z.txt", "file2.txt", "a.md", "file10.bin"],
            ),
            (
                SortField::Unsorted,
                ["file10.bin", "file2.txt", "a.md", "z.txt"],
            ),
        ] {
            for reverse in [false, true] {
                app.set_panel_sort_mode(
                    ActivePanel::Left,
                    SortMode {
                        field: SortField::Name,
                        reverse: false,
                    },
                );
                app.apply(AppCommand::OpenSortOrder).unwrap();
                if reverse {
                    app.apply(AppCommand::DialogFocusNext).unwrap();
                }
                app.finish_dialog(DialogResult::ListboxSubmitted {
                    index: Some(field.index()),
                    value: None,
                });
                drain_background(&mut app);
                let mut expected = expected.to_vec();
                if reverse {
                    expected.reverse();
                }
                let actual = app
                    .active_panel()
                    .entries
                    .iter()
                    .filter(|entry| !entry.is_dir())
                    .map(|entry| entry.name.as_str())
                    .collect::<Vec<_>>();
                assert_eq!(
                    actual, expected,
                    "{directory}: {field:?}, reverse={reverse}"
                );
                assert!(app.active_panel().is_archive());
                assert!(app.active_panel().entries[0].is_parent());
            }
        }
    }
}

#[test]
fn zip_folder_time_sort_uses_descendants_only_when_the_folder_has_no_record() {
    let fixture = Fixture::new(&[]);
    let mut writer = ZipWriter::new(fs::File::create(&fixture.zip).unwrap());
    for (name, year) in [
        ("alpha/new", 2024),
        ("zeta/old", 2020),
        ("explicit/", 2019),
        ("explicit/newer", 2025),
        ("nested/deep/newest", 2026),
    ] {
        let options = SimpleFileOptions::default()
            .last_modified_time(zip::DateTime::from_date_and_time(year, 1, 1, 0, 0, 0).unwrap());
        if name.ends_with('/') {
            writer.add_directory(name, options).unwrap();
        } else {
            writer.start_file(name, options).unwrap();
            writer.write_all(b"payload").unwrap();
        }
    }
    writer.finish().unwrap();
    let mut app = fixture.app();
    for (reverse, expected) in [
        (false, ["explicit", "zeta", "alpha", "nested"]),
        (true, ["nested", "alpha", "zeta", "explicit"]),
    ] {
        app.set_panel_sort_mode(
            ActivePanel::Left,
            SortMode {
                field: SortField::Modified,
                reverse,
            },
        );
        app.refresh_active_panel();
        drain_background(&mut app);
        let actual = app
            .active_panel()
            .entries
            .iter()
            .skip(1)
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
        assert!(
            app.active_panel()
                .entries
                .iter()
                .skip(1)
                .all(|entry| entry.modified.is_some())
        );
    }
}

#[test]
fn zip_unsorted_folders_keep_their_first_appearance_in_the_archive() {
    let fixture = Fixture::new(&[
        ("zeta/first", b"1"),
        ("alpha/later", b"2"),
        ("zeta/", b""),
        ("empty/", b""),
        ("root", b"3"),
    ]);
    let mut app = fixture.app();
    for (reverse, expected) in [
        (false, ["zeta", "alpha", "empty", "root"]),
        (true, ["empty", "alpha", "zeta", "root"]),
    ] {
        app.set_panel_sort_mode(
            ActivePanel::Left,
            SortMode {
                field: SortField::Unsorted,
                reverse,
            },
        );
        app.refresh_active_panel();
        drain_background(&mut app);
        let actual = app
            .active_panel()
            .entries
            .iter()
            .skip(1)
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }
}

#[test]
fn zip_enter_browses_implicit_and_empty_directories_and_parent_exits() {
    let fixture = Fixture::new(&[
        ("docs/nested/readme.txt", b"hello"),
        ("empty/", b""),
        ("root.txt", b"root"),
    ]);
    let original = fs::read(&fixture.zip).unwrap();
    let mut app = fixture.app();
    assert!(!app.active_panel().is_panelized());
    assert_eq!(
        app.active_panel()
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["..", "docs", "empty", "root.txt"]
    );
    assert_eq!(app.panels[1].cwd, fixture.output);
    enter(&mut app, &fixture.zip.join("docs"));
    enter(&mut app, &fixture.zip.join("docs/nested"));
    assert_eq!(app.active_panel().cwd, fixture.zip.join("docs/nested"));
    assert_eq!(app.active_panel().entries[1].size, 5);
    app.apply(AppCommand::CdUp).unwrap();
    drain_background(&mut app);
    assert_eq!(app.active_panel().cwd, fixture.zip.join("docs"));
    enter(&mut app, &fixture.zip);
    enter(&mut app, &fixture.zip.join("empty"));
    assert_eq!(app.active_panel().entries.len(), 1);
    app.apply(AppCommand::CdUp).unwrap();
    drain_background(&mut app);
    enter(&mut app, &fixture.root);
    assert!(!app.active_panel().is_archive());
    assert_eq!(app.active_panel().cwd, fixture.root);
    assert_eq!(fs::read(&fixture.zip).unwrap(), original);
    assert!(!fixture.root.join("docs").exists());
}

#[test]
fn zip_f5_extracts_tagged_files_and_directories_only_and_refreshes_destination() {
    let fixture = Fixture::new(&[
        ("folder/a.txt", b"alpha"),
        ("folder/deep/b.txt", b"beta"),
        ("empty/", b""),
        ("selected.txt", b"chosen"),
        ("untouched.txt", b"leave"),
    ]);
    let original = fs::read(&fixture.zip).unwrap();
    let mut app = fixture.app();
    for name in ["folder", "empty", "selected.txt"] {
        select(&mut app, &fixture.zip.join(name));
        app.apply(AppCommand::ToggleTag).unwrap();
    }
    drain_background(&mut app);
    assert!(matches!(
        app.selection_size_state(ActivePanel::Left),
        SelectionSizeState::Ready {
            selected_items: 3,
            apparent_bytes: 15,
            unreadable_entries: 0
        }
    ));
    app.apply(AppCommand::Copy).unwrap();
    app.finish_dialog(DialogResult::InputSubmitted(
        fixture.output.to_string_lossy().into_owned(),
    ));
    drain_background(&mut app);
    assert_eq!(
        fs::read(fixture.output.join("folder/a.txt")).unwrap(),
        b"alpha"
    );
    assert_eq!(
        fs::read(fixture.output.join("folder/deep/b.txt")).unwrap(),
        b"beta"
    );
    assert!(fixture.output.join("empty").is_dir());
    assert_eq!(
        fs::read(fixture.output.join("selected.txt")).unwrap(),
        b"chosen"
    );
    assert!(!fixture.output.join("untouched.txt").exists());
    assert!(
        app.panels[1]
            .entries
            .iter()
            .any(|entry| entry.name == "folder")
    );
    assert!(app.active_panel().is_archive());
    assert_eq!(fs::read(&fixture.zip).unwrap(), original);
    assert!(
        app.jobs
            .jobs()
            .iter()
            .any(|job| job.kind == JobKind::Copy && job.status == JobStatus::Succeeded)
    );
}

#[test]
fn zip_f5_from_subdirectory_copies_file_without_ancestor_directories() {
    let fixture = Fixture::new(&[("a/b/file.txt", b"payload"), ("unselected", b"other")]);
    let mut app = fixture.app();
    enter(&mut app, &fixture.zip.join("a"));
    enter(&mut app, &fixture.zip.join("a/b"));
    select(&mut app, &fixture.zip.join("a/b/file.txt"));
    app.settings.confirmation.confirm_overwrite = true;
    app.apply(AppCommand::Copy).unwrap();
    app.finish_dialog(DialogResult::InputSubmitted(
        fixture.output.to_string_lossy().into_owned(),
    ));
    app.finish_dialog(DialogResult::ListboxSubmitted {
        index: Some(1),
        value: None,
    });
    drain_background(&mut app);
    assert_eq!(
        fs::read(fixture.output.join("file.txt")).unwrap(),
        b"payload"
    );
    assert!(!fixture.output.join("a").exists());
}

#[test]
fn zip_enter_and_f3_view_files_and_quick_view_reads_selected_member() {
    let fixture = Fixture::new(&[("file.txt", b"alpha\nbeta\n")]);
    let mut app = fixture.app();
    select(&mut app, &fixture.zip.join("file.txt"));
    app.apply(AppCommand::SetOtherPanelView(PanelViewMode::QuickView))
        .unwrap();
    drain_background(&mut app);
    assert_eq!(
        app.quick_view_state(ActivePanel::Right)
            .viewer()
            .unwrap()
            .content(),
        "alpha\nbeta\n"
    );
    for action in [AppCommand::OpenEntry, AppCommand::ViewEntry] {
        app.apply(action).unwrap();
        drain_background(&mut app);
        assert_eq!(app.active_viewer().unwrap().content(), "alpha\nbeta\n");
        assert_eq!(
            app.active_viewer().unwrap().path(),
            fixture.zip.join("file.txt")
        );
        app.apply(AppCommand::CloseViewer).unwrap();
    }
    assert!(!fixture.root.join("file.txt").exists());
    assert!(app.take_pending_external_edit_requests().is_empty());
    assert!(app.take_pending_external_execute_requests().is_empty());
}

#[test]
fn zip_large_member_preview_is_bounded_and_cancelable() {
    let data = vec![b'x'; VIEWER_TEXT_PREVIEW_LIMIT_BYTES + 1024];
    let fixture = Fixture::new(&[("large", &data)]);
    let location = ArchiveLocation::root(fixture.zip.clone());
    let viewer = ViewerState::open_archive_cancellable(
        &location,
        fixture.zip.join("large"),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        viewer.bytes.len(),
        FOUNDATION_SLO
            .viewer_memory_soft_limit_bytes
            .min(VIEWER_TEXT_PREVIEW_LIMIT_BYTES)
    );
    assert!(viewer.text_is_preview());
    let error = ViewerState::open_archive_cancellable(
        &location,
        fixture.zip.join("large"),
        &AtomicBool::new(true),
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
}

#[test]
fn zip_encrypted_members_can_be_listed_but_require_supported_decryption_to_read() {
    let fixture = Fixture::new(&[("secret", b"secret"), ("plain", b"plain")]);
    // Mark this payload encrypted in the central directory. Browsing must not
    // inspect or try to decode it, regardless of its actual payload bytes.
    let archive = ZipArchive::new(fs::File::open(&fixture.zip).unwrap()).unwrap();
    let index = archive.index_for_name("secret").unwrap();
    let header = archive.by_index_data(index).unwrap().central_header_start();
    drop(archive);
    let mut bytes = fs::read(&fixture.zip).unwrap();
    bytes[header as usize + 8] |= 1;
    fs::write(&fixture.zip, bytes).unwrap();
    let app = fixture.app();
    assert!(
        app.active_panel()
            .entries
            .iter()
            .any(|entry| entry.name == "secret")
    );
    fixture.copy(&["plain"], OverwritePolicy::Skip).unwrap();
    assert!(
        fixture
            .copy(&["secret"], OverwritePolicy::Skip)
            .unwrap_err()
            .message
            .contains("password-protected")
    );
    let error = ViewerState::open_archive_cancellable(
        &ArchiveLocation::root(fixture.zip.clone()),
        fixture.zip.join("secret"),
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    assert!(!fixture.output.join("secret").exists());
}

#[test]
fn zip_copy_cannot_overwrite_the_source_archive() {
    for policy in [
        OverwritePolicy::Skip,
        OverwritePolicy::Rename,
        OverwritePolicy::Overwrite,
    ] {
        let mut fixture = Fixture::new(&[("example.ZIP", b"extracted payload")]);
        fixture.output = fixture.root.clone();
        let original = fs::read(&fixture.zip).unwrap();
        let result = fixture.copy(&["example.ZIP"], policy);
        if policy == OverwritePolicy::Overwrite {
            assert!(result.unwrap_err().message.contains("source ZIP archive"));
        } else {
            result.unwrap();
        }
        assert_eq!(fs::read(&fixture.zip).unwrap(), original);
        if policy == OverwritePolicy::Rename {
            assert_eq!(
                fs::read(fixture.root.join("example.ZIP.copy")).unwrap(),
                b"extracted payload"
            );
        }
    }
}

#[test]
fn zip_backspace_restores_focus_to_the_archive_file() {
    let fixture = Fixture::new(&[("file", b"data")]);
    let mut app = fixture.app();
    app.apply(AppCommand::CdUp).unwrap();
    drain_background(&mut app);
    assert!(!app.active_panel().is_archive());
    assert_eq!(
        app.active_panel().selected_entry().unwrap().path,
        fixture.zip
    );
}

#[test]
fn zip_mutating_commands_are_blocked_with_and_without_delete_confirmation() {
    let fixture = Fixture::new(&[("file", b"unchanged")]);
    let original = fs::read(&fixture.zip).unwrap();
    let mut app = fixture.app();
    select(&mut app, &fixture.zip.join("file"));
    for confirm in [true, false] {
        app.settings.confirmation.confirm_delete = confirm;
        for command in [
            AppCommand::Move,
            AppCommand::Delete,
            AppCommand::EditEntry,
            AppCommand::OpenConfirmDialog,
            AppCommand::OpenInputDialog,
        ] {
            app.apply(command).unwrap();
            assert!(app.status_line.contains("read-only"));
            assert_eq!(app.route_depth(), 1);
            assert!(app.take_pending_worker_commands().is_empty());
            assert!(app.take_pending_external_edit_requests().is_empty());
        }
    }
    assert_eq!(fs::read(&fixture.zip).unwrap(), original);
}

#[test]
fn zip_cannot_be_a_copy_or_move_destination() {
    let fixture = Fixture::new(&[("file", b"archived")]);
    let mut app = fixture.app();
    let local = fixture.output.join("local");
    fs::write(&local, b"local").unwrap();
    app.apply(AppCommand::SwitchPanel).unwrap();
    app.active_panel_mut().refresh().unwrap();
    select(&mut app, &local);
    for command in [AppCommand::Copy, AppCommand::Move] {
        app.apply(command).unwrap();
        if command == AppCommand::Copy {
            app.finish_dialog(DialogResult::InputSubmitted(
                fixture.zip.to_string_lossy().into_owned(),
            ));
        } else {
            app.finish_dialog(DialogResult::PairInputSubmitted {
                first: fixture.zip.to_string_lossy().into_owned(),
                second: "local".into(),
            });
        }
        assert!(app.status_line.contains("read-only"));
        assert!(app.take_pending_worker_commands().is_empty());
    }
    assert!(local.exists());
}

#[test]
fn zip_corrupt_unselected_payload_is_never_decompressed() {
    let fixture = Fixture::new(&[
        ("valid", b"valid"),
        ("broken", b"this must not be decompressed"),
    ]);
    fixture.corrupt("broken");
    let mut app = fixture.app();
    select(&mut app, &fixture.zip.join("valid"));
    app.apply(AppCommand::OpenEntry).unwrap();
    drain_background(&mut app);
    assert_eq!(app.active_viewer().unwrap().content(), "valid");
    fixture.copy(&["valid"], OverwritePolicy::Skip).unwrap();
    assert_eq!(fs::read(fixture.output.join("valid")).unwrap(), b"valid");
    assert!(!fixture.output.join("broken").exists());
}

#[test]
fn zip_copy_preserves_overwrite_policies_for_files_and_directories() {
    for name in ["file", "directory"] {
        for policy in [
            OverwritePolicy::Skip,
            OverwritePolicy::Rename,
            OverwritePolicy::Overwrite,
        ] {
            let fixture = Fixture::new(&[("file", b"new"), ("directory/nested", b"new")]);
            let path = fixture.output.join(name);
            if name == "file" {
                fs::write(&path, b"old").unwrap();
            } else {
                fs::create_dir(&path).unwrap();
                fs::write(path.join("old"), b"old").unwrap();
            }
            fixture.copy(&[name], policy).unwrap();
            let target = if policy == OverwritePolicy::Rename {
                fixture.output.join(format!("{name}.copy"))
            } else {
                path.clone()
            };
            if policy == OverwritePolicy::Skip {
                assert_eq!(
                    fs::read(if name == "file" {
                        path
                    } else {
                        path.join("old")
                    })
                    .unwrap(),
                    b"old"
                );
            } else {
                assert_eq!(
                    fs::read(if name == "file" {
                        target
                    } else {
                        target.join("nested")
                    })
                    .unwrap(),
                    b"new"
                );
                if policy == OverwritePolicy::Overwrite && name == "directory" {
                    assert!(!path.join("old").exists());
                }
            }
        }
    }
}

#[test]
fn zip_crc_failure_restores_overwritten_file_and_directory_and_cleans_staging() {
    for name in ["file", "directory"] {
        let fixture = Fixture::new(&[
            ("file", b"bad"),
            ("directory/a", b"good"),
            ("directory/b", b"bad"),
        ]);
        fixture.corrupt(if name == "file" {
            "file"
        } else {
            "directory/b"
        });
        let target = fixture.output.join(name);
        if name == "file" {
            fs::write(&target, b"original").unwrap();
        } else {
            fs::create_dir(&target).unwrap();
            fs::write(target.join("original"), b"original").unwrap();
        }
        assert!(fixture.copy(&[name], OverwritePolicy::Overwrite).is_err());
        assert_eq!(
            fs::read(if name == "file" {
                target
            } else {
                target.join("original")
            })
            .unwrap(),
            b"original"
        );
        assert_eq!(fs::read_dir(&fixture.output).unwrap().count(), 1);
    }
}

#[test]
fn zip_unsafe_paths_are_omitted_and_cannot_escape_extraction() {
    let fixture = Fixture::new(&[
        ("safe/file", b"safe"),
        ("../escape", b"bad"),
        ("safe/../../escape", b"bad"),
        ("/absolute", b"bad"),
        ("C:/drive", b"bad"),
        ("safe\\..\\escape", b"bad"),
    ]);
    let app = fixture.app();
    assert_eq!(
        app.active_panel()
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["..", "safe"]
    );
    fixture.copy(&["safe"], OverwritePolicy::Skip).unwrap();
    assert_eq!(fs::read(fixture.output.join("safe/file")).unwrap(), b"safe");
    assert!(!fixture.root.join("escape").exists());
    assert!(fixture.copy(&["../escape"], OverwritePolicy::Skip).is_err());
}

#[test]
fn zip_symlink_members_cannot_escape_or_replace_existing_data() {
    let fixture = Fixture::new(&[("folder/file", b"safe")]);
    let mut writer = ZipWriter::new_append(
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&fixture.zip)
            .unwrap(),
    )
    .unwrap();
    writer
        .add_symlink("folder/link", "../../outside", SimpleFileOptions::default())
        .unwrap();
    writer.finish().unwrap();
    let old = fixture.output.join("folder");
    fs::create_dir(&old).unwrap();
    fs::write(old.join("original"), b"preserved").unwrap();
    assert!(
        fixture
            .copy(&["folder"], OverwritePolicy::Overwrite)
            .unwrap_err()
            .message
            .contains("symlinks")
    );
    assert_eq!(fs::read(old.join("original")).unwrap(), b"preserved");
    assert_eq!(fs::read_dir(&fixture.output).unwrap().count(), 1);
}

#[test]
fn zip_invalid_archive_and_failed_refresh_restore_previous_pane() {
    let fixture = Fixture::new(&[("file", b"content")]);
    let mut app = app_with_loaded_panels(fixture.root.clone());
    select(&mut app, &fixture.zip);
    let previous = app.active_panel().clone();
    fs::write(&fixture.zip, b"not a zip").unwrap();
    app.apply(AppCommand::OpenEntry).unwrap();
    drain_background(&mut app);
    assert_eq!(app.active_panel(), &previous);
    assert!(app.status_line.contains("failed"));
}

#[test]
fn zip_canceled_extraction_does_not_create_output() {
    let fixture = Fixture::new(&[("folder/file", b"payload")]);
    let mut manager = JobManager::new();
    let job = manager.enqueue(JobRequest::ExtractArchive {
        location: ArchiveLocation::root(fixture.zip.clone()),
        sources: vec![fixture.zip.join("folder")],
        destination_dir: fixture.output.clone(),
        overwrite: OverwritePolicy::Overwrite,
    });
    job.cancel_flag().store(true, AtomicOrdering::Relaxed);
    let (tx, rx) = std::sync::mpsc::channel();
    execute_worker_job(job, &tx);
    drop(tx);
    assert!(rx.into_iter().any(|event| matches!(event, JobEvent::Finished { result: Err(error), .. } if error.is_canceled())));
    assert_eq!(fs::read_dir(&fixture.output).unwrap().count(), 0);
}

#[test]
fn zip_filters_sorting_and_hidden_tags_preserve_archive_source() {
    let fixture = Fixture::new(&[
        ("folder/a", b"123"),
        (".hidden", b"12"),
        ("visible", b"12345"),
    ]);
    let mut app = fixture.app();
    select(&mut app, &fixture.zip.join("folder"));
    app.apply(AppCommand::ToggleTag).unwrap();
    app.set_panel_filter(
        ActivePanel::Left,
        PanelFilter {
            pattern: "visible".into(),
            files_only: false,
            ..PanelFilter::default()
        },
    );
    app.refresh_active_panel();
    drain_background(&mut app);
    assert!(app.active_panel().is_archive());
    assert_eq!(app.active_panel().tagged_count(), 1);
    assert!(matches!(
        app.selection_size_state(ActivePanel::Left),
        SelectionSizeState::Ready {
            apparent_bytes: 3,
            ..
        }
    ));
    assert_eq!(
        app.active_panel()
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["..", "visible"]
    );
    app.apply(AppCommand::Copy).unwrap();
    app.finish_dialog(DialogResult::InputSubmitted(
        fixture.output.to_string_lossy().into_owned(),
    ));
    drain_background(&mut app);
    assert_eq!(fs::read(fixture.output.join("folder/a")).unwrap(), b"123");
    app.set_panel_filter(ActivePanel::Left, PanelFilter::default());
    app.active_panel_mut().set_show_hidden_files(false);
    app.apply(AppCommand::SortReverse).unwrap();
    drain_background(&mut app);
    assert!(
        app.active_panel()
            .entries
            .iter()
            .all(|entry| entry.name != ".hidden")
    );
}
