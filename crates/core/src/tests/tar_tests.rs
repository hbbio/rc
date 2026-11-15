use super::*;
use std::io::{Cursor, Write};

const FORMATS: [&str; 3] = ["tar", "tar.gz", "tar.bz2"];

struct Fixture {
    root: tempfile::TempDir,
    archive: PathBuf,
    output: PathBuf,
}

fn tar_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (number, (name, data)) in entries.iter().enumerate() {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mtime(1_700_000_000 + number as u64);
        header.set_mode(if name.ends_with('/') { 0o755 } else { 0o644 });
        if name.ends_with('/') {
            header.set_entry_type(tar::EntryType::Directory);
        }
        header.set_cksum();
        builder
            .append_data(&mut header, name, Cursor::new(data))
            .unwrap();
    }
    builder.into_inner().unwrap()
}

fn encode(format: &str, bytes: &[u8]) -> Vec<u8> {
    match ArchiveFormat::for_path(Path::new(&format!("input.{format}"))).unwrap() {
        ArchiveFormat::Tar => bytes.to_vec(),
        ArchiveFormat::TarGzip => {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(bytes).unwrap();
            encoder.finish().unwrap()
        }
        ArchiveFormat::TarBzip2 => {
            let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
            encoder.write_all(bytes).unwrap();
            encoder.finish().unwrap()
        }
        ArchiveFormat::Zip => unreachable!(),
    }
}

impl Fixture {
    fn new(format: &str, entries: &[(&str, &[u8])]) -> Self {
        Self::raw(format, &tar_bytes(entries))
    }

    fn raw(format: &str, bytes: &[u8]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join(format!("example.{format}"));
        fs::write(&archive, encode(format, bytes)).unwrap();
        let output = root.path().join("output");
        fs::create_dir(&output).unwrap();
        Self {
            root,
            archive,
            output,
        }
    }

    fn app(&self) -> AppState {
        let mut app = app_with_loaded_panels(self.root.path().to_path_buf());
        app.settings.confirmation.confirm_overwrite = false;
        app.panels[1].cwd = self.output.clone();
        app.panels[1].refresh().unwrap();
        enter(&mut app, &self.archive);
        assert!(
            app.active_panel().is_archive(),
            "{}: {}",
            self.archive.display(),
            app.status_line
        );
        assert_eq!(
            app.active_panel().archive_location().unwrap().archive,
            self.archive
        );
        app
    }

    fn copy(&self, sources: &[&str], policy: OverwritePolicy) -> Result<(), JobError> {
        self.copy_paths(
            &sources.iter().map(PathBuf::from).collect::<Vec<_>>(),
            policy,
        )
    }

    fn copy_paths(&self, sources: &[PathBuf], policy: OverwritePolicy) -> Result<(), JobError> {
        let mut manager = JobManager::new();
        let job = manager.enqueue(JobRequest::ExtractArchive {
            location: ArchiveLocation::root(self.archive.clone()),
            sources: sources.iter().map(|path| self.archive.join(path)).collect(),
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
}

fn select(app: &mut AppState, path: &Path) {
    app.active_panel_mut().cursor = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("{} is not listed", path.display()));
}

fn enter(app: &mut AppState, path: &Path) {
    select(app, path);
    app.apply(AppCommand::OpenEntry).unwrap();
    drain_background(app);
}

#[test]
fn tar_formats_navigation_preserves_quick_cd_local_history() {
    for format in FORMATS {
        let fixture = Fixture::new(format, &[("nested/deep/file", b"contents")]);
        super::quick_cd_tests::assert_archive_navigation_preserves_local_history(
            fixture.root.path(),
            &fixture.archive,
        );
    }
}

#[test]
fn tar_formats_browse_aliases_nested_and_empty_directories_and_exit_with_focus() {
    for format in ["tar", "TAR.GZ", "tgz", "TAR.BZ2", "tbz2", "tbz"] {
        let fixture = Fixture::new(
            format,
            &[
                ("docs/deep/readme.txt", b"hello"),
                ("empty/", b""),
                ("root", b"root"),
            ],
        );
        let original = fs::read(&fixture.archive).unwrap();
        let mut app = fixture.app();
        assert!(!app.active_panel().is_panelized());
        assert_eq!(
            app.active_panel()
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["..", "docs", "empty", "root"]
        );
        enter(&mut app, &fixture.archive.join("docs"));
        enter(&mut app, &fixture.archive.join("docs/deep"));
        assert_eq!(app.active_panel().entries[1].size, 5);
        for _ in 0..2 {
            app.apply(AppCommand::CdUp).unwrap();
            drain_background(&mut app);
        }
        enter(&mut app, &fixture.archive.join("empty"));
        assert_eq!(app.active_panel().entries.len(), 1);
        app.apply(AppCommand::CdUp).unwrap();
        drain_background(&mut app);
        app.apply(AppCommand::CdUp).unwrap();
        drain_background(&mut app);
        assert!(!app.active_panel().is_archive());
        assert_eq!(
            app.active_panel().selected_entry().unwrap().path,
            fixture.archive
        );
        assert_eq!(fs::read(&fixture.archive).unwrap(), original);
        assert!(!fixture.root.path().join("docs").exists());
    }
}

#[test]
fn tar_formats_f5_extracts_only_tagged_subtrees_and_measures_metadata() {
    for format in FORMATS {
        let fixture = Fixture::new(
            format,
            &[
                ("folder/a", b"alpha"),
                ("folder/deep/b", b"beta"),
                ("empty/", b""),
                ("selected", b"chosen"),
                ("untouched", b"leave"),
            ],
        );
        let original = fs::read(&fixture.archive).unwrap();
        let mut app = fixture.app();
        for name in ["folder", "empty", "selected"] {
            select(&mut app, &fixture.archive.join(name));
            app.apply(AppCommand::ToggleTag).unwrap();
        }
        drain_background(&mut app);
        assert!(matches!(
            app.selection_size_state(ActivePanel::Left),
            SelectionSizeState::Ready {
                apparent_bytes: 15,
                unreadable_entries: 0,
                ..
            }
        ));
        app.apply(AppCommand::Copy).unwrap();
        app.finish_dialog(DialogResult::InputSubmitted(
            fixture.output.to_string_lossy().into_owned(),
        ));
        drain_background(&mut app);
        assert_eq!(fs::read(fixture.output.join("folder/a")).unwrap(), b"alpha");
        assert_eq!(
            fs::read(fixture.output.join("folder/deep/b")).unwrap(),
            b"beta"
        );
        assert!(fixture.output.join("empty").is_dir());
        assert_eq!(
            fs::read(fixture.output.join("selected")).unwrap(),
            b"chosen"
        );
        assert!(!fixture.output.join("untouched").exists());
        assert!(
            app.panels[1]
                .entries
                .iter()
                .any(|entry| entry.name == "folder")
        );
        assert_eq!(fs::read(&fixture.archive).unwrap(), original);
        // A nested copy uses its basename, without recreating ancestors.
        enter(&mut app, &fixture.archive.join("folder"));
        select(&mut app, &fixture.archive.join("folder/a"));
        app.apply(AppCommand::Copy).unwrap();
        app.finish_dialog(DialogResult::InputSubmitted(
            fixture.output.to_string_lossy().into_owned(),
        ));
        drain_background(&mut app);
        assert_eq!(fs::read(fixture.output.join("a")).unwrap(), b"alpha");
    }
}

#[test]
fn tar_formats_enter_f3_and_quick_view_use_bounded_member_readers() {
    for format in FORMATS {
        let fixture = Fixture::new(format, &[("file.txt", b"alpha\nbeta\n")]);
        let mut app = fixture.app();
        select(&mut app, &fixture.archive.join("file.txt"));
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
            app.apply(AppCommand::CloseViewer).unwrap();
        }
        assert!(!fixture.root.path().join("file.txt").exists());
        assert!(app.take_pending_external_execute_requests().is_empty());
    }
}

#[test]
fn tar_formats_sort_hidden_filter_and_read_only_rules_share_the_archive_flow() {
    for format in FORMATS {
        let fixture = Fixture::new(
            format,
            &[
                ("file10.txt", b"abc"),
                ("folder/z", b"zzzz"),
                ("file2.bin", b"a"),
                (".hidden", b"xx"),
            ],
        );
        let mut app = fixture.app();
        for (field, expected) in [
            (SortField::Name, vec![".hidden", "file10.txt", "file2.bin"]),
            (
                SortField::Version,
                vec![".hidden", "file2.bin", "file10.txt"],
            ),
            (SortField::Size, vec!["file2.bin", ".hidden", "file10.txt"]),
            (
                SortField::Unsorted,
                vec!["file10.txt", "file2.bin", ".hidden"],
            ),
        ] {
            app.active_panel_mut().sort_mode = SortMode {
                field,
                reverse: false,
            };
            app.active_panel_mut().refresh().unwrap();
            assert_eq!(
                app.active_panel()
                    .entries
                    .iter()
                    .filter(|entry| !entry.is_dir())
                    .map(|entry| entry.name.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
            app.active_panel_mut().sort_mode.reverse = true;
            app.active_panel_mut().refresh().unwrap();
            assert_eq!(
                app.active_panel()
                    .entries
                    .iter()
                    .filter(|entry| !entry.is_dir())
                    .map(|entry| entry.name.as_str())
                    .collect::<Vec<_>>(),
                expected.into_iter().rev().collect::<Vec<_>>()
            );
        }
        select(&mut app, &fixture.archive.join("file2.bin"));
        for command in [
            AppCommand::Move,
            AppCommand::Delete,
            AppCommand::EditEntry,
            AppCommand::OpenInputDialog,
        ] {
            app.apply(command).unwrap();
            assert!(app.status_line.contains("read-only"));
            assert_eq!(app.route_depth(), 1);
        }
        app.set_panel_filter(
            ActivePanel::Left,
            PanelFilter {
                pattern: "*.bin".into(),
                files_only: false,
                ..PanelFilter::default()
            },
        );
        app.refresh_active_panel();
        drain_background(&mut app);
        assert!(app.active_panel().is_archive());
        assert_eq!(app.active_panel().entries.len(), 2);
    }
}

#[test]
fn tar_formats_last_duplicate_record_wins_and_overwrite_policies_are_preserved() {
    for format in FORMATS {
        let fixture = Fixture::new(
            format,
            &[("file", b"old record"), ("file", b"latest record")],
        );
        fs::write(fixture.output.join("file"), b"existing").unwrap();
        fixture.copy(&["file"], OverwritePolicy::Skip).unwrap();
        assert_eq!(fs::read(fixture.output.join("file")).unwrap(), b"existing");
        fixture.copy(&["file"], OverwritePolicy::Rename).unwrap();
        assert_eq!(
            fs::read(fixture.output.join("file.copy")).unwrap(),
            b"latest record"
        );
        fixture.copy(&["file"], OverwritePolicy::Overwrite).unwrap();
        assert_eq!(
            fs::read(fixture.output.join("file")).unwrap(),
            b"latest record"
        );
    }
}

#[test]
fn tar_formats_reject_unsafe_paths_links_and_source_overwrites() {
    let mut raw = tar_bytes(&[("safe", b"allowed")]);
    raw.truncate(raw.len() - 1024);
    for (name, kind) in [
        ("../outside", tar::EntryType::Regular),
        ("/absolute", tar::EntryType::Regular),
        ("folder/link", tar::EntryType::Symlink),
        ("hard", tar::EntryType::Link),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o777);
        header.set_mtime(0);
        header.set_entry_type(kind);
        header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
        header.set_link_name("../../outside").unwrap();
        header.set_cksum();
        raw.extend_from_slice(header.as_bytes());
    }
    raw.resize(raw.len() + 1024, 0);
    for format in FORMATS {
        let fixture = Fixture::raw(format, &raw);
        let app = fixture.app();
        assert!(
            !app.active_panel()
                .entries
                .iter()
                .any(|entry| entry.name == "outside" || entry.name == "absolute")
        );
        fixture.copy(&["safe"], OverwritePolicy::Overwrite).unwrap();
        assert_eq!(fs::read(fixture.output.join("safe")).unwrap(), b"allowed");
        assert!(
            fixture
                .copy(&["folder", "hard"], OverwritePolicy::Overwrite)
                .unwrap_err()
                .message
                .contains("symlinks")
        );
        assert!(!fixture.output.join("folder").exists());
        assert!(!fixture.root.path().join("outside").exists());
        let name = fixture.archive.file_name().unwrap().to_str().unwrap();
        let overwrite = Fixture::new(format, &[(name, b"replacement")]);
        let mut manager = JobManager::new();
        let job = manager.enqueue(JobRequest::ExtractArchive {
            location: ArchiveLocation::root(overwrite.archive.clone()),
            sources: vec![overwrite.archive.join(name)],
            destination_dir: overwrite.root.path().to_path_buf(),
            overwrite: OverwritePolicy::Overwrite,
        });
        let original = fs::read(&overwrite.archive).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        execute_worker_job(job, &tx);
        drop(tx);
        assert!(
            rx.into_iter()
                .any(|event| matches!(event, JobEvent::Finished { result: Err(_), .. }))
        );
        assert_eq!(fs::read(&overwrite.archive).unwrap(), original);
    }
}

#[test]
fn tar_formats_support_gnu_long_names_pax_paths_sizes_and_fractional_times() {
    let long_name = format!("long/{}/file", "directory/".repeat(18));
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(4);
    header.set_mode(0o600);
    header.set_mtime(1);
    header.set_cksum();
    builder
        .append_data(&mut header, &long_name, Cursor::new(b"long"))
        .unwrap();
    builder
        .append_pax_extensions([
            ("path", b"pax/renamed.txt".as_slice()),
            ("size", b"7".as_slice()),
            ("mtime", b"1700000000.125".as_slice()),
        ])
        .unwrap();
    header.set_size(7);
    header.set_cksum();
    builder
        .append_data(&mut header, "ignored", Cursor::new(b"payload"))
        .unwrap();
    let raw = builder.into_inner().unwrap();
    for format in FORMATS {
        let fixture = Fixture::raw(format, &raw);
        fixture
            .copy(&[&long_name, "pax/renamed.txt"], OverwritePolicy::Overwrite)
            .unwrap();
        assert_eq!(fs::read(fixture.output.join("file")).unwrap(), b"long");
        assert_eq!(
            fs::read(fixture.output.join("renamed.txt")).unwrap(),
            b"payload"
        );
        let location = ArchiveLocation {
            archive: fixture.archive.clone(),
            directory: "pax".into(),
            parent: None,
        };
        let entries = location
            .read_entries(SortMode::default(), true, None)
            .unwrap();
        assert_eq!(
            entries[1].modified,
            Some(UNIX_EPOCH + Duration::new(1_700_000_000, 125_000_000))
        );
    }
}

#[test]
fn tar_formats_reject_truncated_payload_headers_and_oversized_metadata() {
    for format in FORMATS {
        let mut broken = tar_bytes(&[("file", b"payload")]);
        broken.truncate(515);
        let fixture = Fixture::raw(format, &broken);
        assert!(
            ArchiveLocation::root(fixture.archive.clone())
                .read_entries(SortMode::default(), true, None)
                .is_err()
        );
        let mut bad_header = tar_bytes(&[("file", b"payload")]);
        bad_header[12] ^= 0xff;
        let fixture = Fixture::raw(format, &bad_header);
        assert!(
            ArchiveLocation::root(fixture.archive.clone())
                .read_entries(SortMode::default(), true, None)
                .is_err()
        );
        let mut header = tar::Header::new_gnu();
        header.set_size(2 * 1024 * 1024);
        header.set_mode(0o600);
        header.set_mtime(1);
        header.set_entry_type(tar::EntryType::XHeader);
        header.set_cksum();
        let fixture = Fixture::raw(format, header.as_bytes());
        let error = ArchiveLocation::root(fixture.archive.clone())
            .read_entries(SortMode::default(), true, None)
            .unwrap_err();
        assert!(error.to_string().contains("1 MiB"));
    }
}

#[test]
fn compressed_tar_checksums_are_validated_after_the_tar_end_marker() {
    for format in ["tar.gz", "tar.bz2"] {
        let fixture = Fixture::new(format, &[("file", b"payload")]);
        let mut bytes = fs::read(&fixture.archive).unwrap();
        let index = bytes.len() - 2;
        bytes[index] ^= 0x80;
        fs::write(&fixture.archive, bytes).unwrap();
        assert!(
            ArchiveLocation::root(fixture.archive.clone())
                .read_entries(SortMode::default(), true, None)
                .is_err()
        );
    }
}

#[test]
fn compressed_tar_supports_concatenated_streams_split_inside_headers() {
    let raw = tar_bytes(&[("folder/file", b"payload"), ("other", b"second")]);
    for format in ["tar.gz", "tar.bz2"] {
        let fixture = Fixture::raw(format, &raw);
        let mut bytes = encode(format, &raw[..267]);
        bytes.extend(encode(format, &raw[267..]));
        fs::write(&fixture.archive, bytes).unwrap();
        fixture
            .copy(&["folder"], OverwritePolicy::Overwrite)
            .unwrap();
        assert_eq!(
            fs::read(fixture.output.join("folder/file")).unwrap(),
            b"payload"
        );
        assert!(!fixture.output.join("other").exists());
    }
}

// Construct raw name fields independently of the host platform. Non-Unix tests
// must also encounter valid TAR byte names that cannot be represented locally.
fn tar_raw_name_bytes(entries: &[(&[u8], &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, data) in entries {
        assert!(name.len() <= 100);
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(1_700_000_000);
        header.as_mut_bytes()[..name.len()].copy_from_slice(name);
        header.set_cksum();
        builder.append(&header, Cursor::new(data)).unwrap();
    }
    builder.into_inner().unwrap()
}

#[cfg(unix)]
fn byte_path(bytes: &[u8]) -> &Path {
    use std::os::unix::ffi::OsStrExt;
    Path::new(std::ffi::OsStr::from_bytes(bytes))
}

#[cfg(unix)]
fn filesystem_supports_byte_names(directory: &Path) -> bool {
    let probe = directory.join(byte_path(b".rc-byte-name-probe\xff"));
    match fs::write(&probe, b"") {
        Ok(()) => {
            fs::remove_file(probe).unwrap();
            true
        }
        Err(error) if error.raw_os_error() == Some(nix::errno::Errno::EILSEQ as i32) => false,
        Err(error) => panic!("byte-name filesystem probe failed: {error}"),
    }
}

#[cfg(unix)]
fn assert_copy_name_error(app: &AppState) {
    assert!(
        app.jobs.jobs().iter().any(|job| {
            job.kind == JobKind::Copy
                && job.status == JobStatus::Failed
                && job
                    .last_error
                    .as_deref()
                    .is_some_and(|error| error.contains("cannot represent an archive member name"))
        }),
        "jobs: {:?}; status: {}",
        app.jobs.jobs(),
        app.status_line
    );
}

#[cfg(unix)]
#[test]
fn tar_formats_f5_directory_copy_preserves_distinct_non_utf8_members() {
    // These three names have the same lossy display label but distinct native
    // identities. Indexing or copying via display text would merge/rename them.
    let members: &[(&[u8], &[u8])] = &[
        (b"folder/report\xff.txt", b"first"),
        (b"folder/report\xfe.txt", b"second"),
        (b"folder/report\xef\xbf\xbd.txt", b"replacement character"),
        (b"folder/readme.txt", b"readme"),
    ];
    let raw = tar_raw_name_bytes(members);
    for format in FORMATS {
        let fixture = Fixture::raw(format, &raw);
        let original = fs::read(&fixture.archive).unwrap();
        let byte_names = filesystem_supports_byte_names(&fixture.output);
        if !byte_names {
            fs::create_dir(fixture.output.join("folder")).unwrap();
            fs::write(
                fixture.output.join("folder/original.txt"),
                b"keep existing data",
            )
            .unwrap();
        }
        let mut app = fixture.app();
        app.settings.configuration.default_overwrite_policy = OverwritePolicy::Overwrite;
        select(&mut app, &fixture.archive.join("folder"));
        app.apply(AppCommand::ToggleTag).unwrap();
        drain_background(&mut app);
        let bytes = members
            .iter()
            .map(|(_, data)| data.len() as u64)
            .sum::<u64>();
        assert!(
            matches!(app.selection_size_state(ActivePanel::Left), SelectionSizeState::Ready { apparent_bytes, unreadable_entries: 0, .. } if *apparent_bytes == bytes)
        );
        app.apply(AppCommand::Copy).unwrap();
        app.finish_dialog(DialogResult::InputSubmitted(
            fixture.output.to_string_lossy().into_owned(),
        ));
        drain_background(&mut app);
        if byte_names {
            for (name, data) in members {
                assert_eq!(
                    fs::read(fixture.output.join(byte_path(name))).unwrap(),
                    *data
                );
            }
            assert_eq!(
                fs::read_dir(fixture.output.join("folder")).unwrap().count(),
                members.len()
            );
        } else {
            assert_copy_name_error(&app);
            assert_eq!(
                fs::read(fixture.output.join("folder/original.txt")).unwrap(),
                b"keep existing data"
            );
            assert_eq!(
                fs::read_dir(fixture.output.join("folder")).unwrap().count(),
                1
            );
            assert_eq!(fs::read_dir(&fixture.output).unwrap().count(), 1);
        }
        assert_eq!(fs::read(&fixture.archive).unwrap(), original);
        enter(&mut app, &fixture.archive.join("folder"));
        for (name, _) in members {
            assert!(
                app.active_panel()
                    .entries
                    .iter()
                    .any(|entry| entry.path == fixture.archive.join(byte_path(name)))
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn tar_formats_navigate_non_utf8_directories_and_view_and_copy_members() {
    let name = b"folder\xff/nested\xfe/report\xfd.txt";
    let raw = tar_raw_name_bytes(&[(name, b"raw-name payload")]);
    for format in FORMATS {
        let fixture = Fixture::raw(format, &raw);
        let byte_names = filesystem_supports_byte_names(&fixture.output);
        let mut app = fixture.app();
        enter(&mut app, &fixture.archive.join(byte_path(b"folder\xff")));
        enter(
            &mut app,
            &fixture.archive.join(byte_path(b"folder\xff/nested\xfe")),
        );
        select(&mut app, &fixture.archive.join(byte_path(name)));
        app.apply(AppCommand::SetOtherPanelView(PanelViewMode::QuickView))
            .unwrap();
        drain_background(&mut app);
        assert_eq!(
            app.quick_view_state(ActivePanel::Right)
                .viewer()
                .unwrap()
                .content(),
            "raw-name payload"
        );
        app.apply(AppCommand::ViewEntry).unwrap();
        drain_background(&mut app);
        assert_eq!(
            app.active_viewer().unwrap().path(),
            fixture.archive.join(byte_path(name))
        );
        assert_eq!(app.active_viewer().unwrap().content(), "raw-name payload");
        app.apply(AppCommand::CloseViewer).unwrap();
        app.apply(AppCommand::SetOtherPanelView(PanelViewMode::Listing))
            .unwrap();
        app.apply(AppCommand::Copy).unwrap();
        app.finish_dialog(DialogResult::InputSubmitted(
            fixture.output.to_string_lossy().into_owned(),
        ));
        drain_background(&mut app);
        if byte_names {
            assert_eq!(
                fs::read(fixture.output.join(byte_path(b"report\xfd.txt"))).unwrap(),
                b"raw-name payload"
            );
        } else {
            assert_copy_name_error(&app);
            assert_eq!(fs::read_dir(&fixture.output).unwrap().count(), 0);
        }
        app.apply(AppCommand::CdUp).unwrap();
        drain_background(&mut app);
        app.apply(AppCommand::CdUp).unwrap();
        drain_background(&mut app);
        assert_eq!(app.active_panel().cwd, fixture.archive);
        assert_eq!(
            app.active_panel().selected_entry().unwrap().path,
            fixture.archive.join(byte_path(b"folder\xff"))
        );
        let result = fixture.copy_paths(
            &[byte_path(b"folder\xff").to_path_buf()],
            OverwritePolicy::Overwrite,
        );
        if byte_names {
            result.unwrap();
            assert_eq!(
                fs::read(fixture.output.join(byte_path(name))).unwrap(),
                b"raw-name payload"
            );
        } else {
            assert_eq!(result.unwrap_err().code, JobErrorCode::Unsupported);
            assert_eq!(fs::read_dir(&fixture.output).unwrap().count(), 0);
        }
    }
}

#[cfg(unix)]
#[test]
fn tar_formats_preserve_gnu_long_names_ustar_prefixes_and_pax_binary_paths() {
    let mut long_name = b"folder/".to_vec();
    for _ in 0..18 {
        long_name.extend_from_slice(b"directory\xff/");
    }
    long_name.extend_from_slice(b"report\xfe.txt");
    let pax_name = b"folder/pax\xff.txt";
    let mut prefix_name = b"folder/".to_vec();
    for _ in 0..7 {
        prefix_name.extend_from_slice(b"prefix\xfd/");
    }
    prefix_name.extend_from_slice(b"record.txt");
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(4);
    header.set_mode(0o600);
    header.set_mtime(1);
    header.set_cksum();
    builder
        .append_data(&mut header, byte_path(&long_name), Cursor::new(b"long"))
        .unwrap();
    builder
        .append_pax_extensions([
            ("hdrcharset", b"BINARY".as_slice()),
            ("path", pax_name.as_slice()),
        ])
        .unwrap();
    header.set_size(3);
    header.set_cksum();
    builder
        .append_data(&mut header, "ignored", Cursor::new(b"pax"))
        .unwrap();
    let mut ustar = tar::Header::new_ustar();
    ustar.set_size(5);
    ustar.set_mode(0o600);
    ustar.set_mtime(1);
    let prefix_length = prefix_name.len() - b"/record.txt".len();
    ustar.as_ustar_mut().unwrap().prefix[..prefix_length]
        .copy_from_slice(&prefix_name[..prefix_length]);
    ustar.as_mut_bytes()[..b"record.txt".len()].copy_from_slice(b"record.txt");
    ustar.set_cksum();
    builder.append(&ustar, Cursor::new(b"ustar")).unwrap();
    let raw = builder.into_inner().unwrap();
    for format in FORMATS {
        let fixture = Fixture::raw(format, &raw);
        for (name, expected) in [
            (&long_name[..], "long"),
            (pax_name.as_slice(), "pax"),
            (&prefix_name[..], "ustar"),
        ] {
            let path = fixture.archive.join(byte_path(name));
            let viewer = ViewerState::open_archive_cancellable(
                &ArchiveLocation::root(fixture.archive.clone()),
                path.clone(),
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(viewer.path(), path);
            assert_eq!(viewer.content(), expected);
        }
        let result = fixture.copy(&["folder"], OverwritePolicy::Overwrite);
        if filesystem_supports_byte_names(&fixture.output) {
            result.unwrap();
            assert_eq!(
                fs::read(fixture.output.join(byte_path(&long_name))).unwrap(),
                b"long"
            );
            assert_eq!(
                fs::read(fixture.output.join(byte_path(pax_name))).unwrap(),
                b"pax"
            );
            assert_eq!(
                fs::read(fixture.output.join(byte_path(&prefix_name))).unwrap(),
                b"ustar"
            );
        } else {
            assert_eq!(result.unwrap_err().code, JobErrorCode::Unsupported);
            assert_eq!(fs::read_dir(&fixture.output).unwrap().count(), 0);
        }
    }
}

#[cfg(unix)]
#[test]
fn tar_formats_rename_preserves_non_utf8_member_names() {
    let name = byte_path(b"report\xff.txt");
    let raw = tar_raw_name_bytes(&[(b"report\xff.txt", b"archived")]);
    for format in FORMATS {
        let fixture = Fixture::raw(format, &raw);
        if !filesystem_supports_byte_names(&fixture.output) {
            let result = fixture.copy_paths(&[name.to_path_buf()], OverwritePolicy::Rename);
            assert_eq!(result.unwrap_err().code, JobErrorCode::Unsupported);
            assert_eq!(fs::read_dir(&fixture.output).unwrap().count(), 0);
            continue;
        }
        fs::write(fixture.output.join(name), b"existing").unwrap();
        for suffix in [".copy", ".copy2"] {
            fixture
                .copy_paths(&[name.to_path_buf()], OverwritePolicy::Rename)
                .unwrap();
            let mut renamed = name.as_os_str().to_os_string();
            renamed.push(suffix);
            assert_eq!(fs::read(fixture.output.join(renamed)).unwrap(), b"archived");
        }
        fixture
            .copy_paths(&[name.to_path_buf()], OverwritePolicy::Skip)
            .unwrap();
        assert_eq!(fs::read(fixture.output.join(name)).unwrap(), b"existing");
        fixture
            .copy_paths(&[name.to_path_buf()], OverwritePolicy::Overwrite)
            .unwrap();
        assert_eq!(fs::read(fixture.output.join(name)).unwrap(), b"archived");
        assert_eq!(fs::read_dir(&fixture.output).unwrap().count(), 3);
    }
}

#[test]
fn tar_formats_unsafe_byte_paths_are_omitted_before_native_conversion() {
    let raw = tar_raw_name_bytes(&[
        (b"folder/safe", b"allowed"),
        (b"../outside\xff", b"denied"),
        (b"/absolute\xff", b"denied"),
        (b"folder/../outside\xff", b"denied"),
        (b"folder/..\\outside\xff", b"denied"),
        (b"folder/drive:stream\xff", b"denied"),
    ]);
    for format in FORMATS {
        let fixture = Fixture::raw(format, &raw);
        fixture
            .copy(&["folder"], OverwritePolicy::Overwrite)
            .unwrap();
        assert_eq!(
            fs::read(fixture.output.join("folder/safe")).unwrap(),
            b"allowed"
        );
        assert_eq!(
            fs::read_dir(fixture.output.join("folder")).unwrap().count(),
            1
        );
        assert_eq!(fs::read_dir(&fixture.output).unwrap().count(), 1);
    }
}

#[cfg(not(unix))]
#[test]
fn tar_formats_unrepresentable_member_names_fail_without_incomplete_copies() {
    let raw = tar_raw_name_bytes(&[
        (b"folder/readme.txt", b"visible"),
        (b"folder/report\xff.txt", b"must not disappear"),
    ]);
    for format in FORMATS {
        let fixture = Fixture::raw(format, &raw);
        let location = ArchiveLocation::root(fixture.archive.clone());
        let error = location
            .read_entries(SortMode::default(), true, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("non-UTF-8"));
        assert!(
            fixture
                .copy(&["folder"], OverwritePolicy::Overwrite)
                .unwrap_err()
                .message
                .contains("non-UTF-8")
        );
        assert_eq!(fs::read_dir(&fixture.output).unwrap().count(), 0);
    }
}
