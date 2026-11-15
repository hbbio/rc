use super::*;
use std::io::{Cursor, Write};

fn zip_bytes(name: &str, bytes: &[u8]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(name, zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(bytes).unwrap();
    zip.finish().unwrap().into_inner()
}

fn tar_bytes(name: &str, bytes: &[u8]) -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append_data(&mut header, name, bytes).unwrap();
    tar.into_inner().unwrap()
}

fn select(app: &mut AppState, name: &str) {
    app.active_panel_mut().cursor = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("missing {name}: {}", app.status_line));
}

fn extract(
    location: ArchiveLocation,
    sources: Vec<PathBuf>,
    destination_dir: PathBuf,
) -> Result<(), JobError> {
    let mut manager = JobManager::new();
    let job = manager.enqueue(JobRequest::ExtractArchive {
        location,
        sources,
        destination_dir,
        overwrite: OverwritePolicy::Overwrite,
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

#[test]
fn nested_zip_tar_navigation_view_copy_and_history_preserve_enclosing_location() {
    let files = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(files.path()).unwrap();
    let archive = root.join("outer.zip");
    let inner = tar_bytes("folder/message.txt", b"nested contents");
    fs::write(&archive, zip_bytes("packages/inner.tar", &inner)).unwrap();
    let mut app = app_with_loaded_panels(root.clone());
    for name in ["outer.zip", "packages", "inner.tar", "folder"] {
        select(&mut app, name);
        app.apply(AppCommand::OpenEntry).unwrap();
        drain_background(&mut app);
    }
    assert_eq!(
        app.active_panel().cwd,
        archive.join("packages/inner.tar/folder")
    );
    assert_eq!(app.active_panel().local_directory(), root);
    let location = app.active_panel().archive_location().unwrap().clone();
    let path = app.active_panel().cwd.join("message.txt");
    let size = measure_archive_selection(
        &location,
        std::slice::from_ref(&path),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(size.apparent_bytes, 15);
    let output = root.join("output");
    fs::create_dir(&output).unwrap();
    extract(location.clone(), vec![path], output.clone()).unwrap();
    assert_eq!(
        fs::read(output.join("message.txt")).unwrap(),
        b"nested contents"
    );
    select(&mut app, "message.txt");
    app.apply(AppCommand::ViewEntry).unwrap();
    drain_background(&mut app);
    assert_eq!(app.active_viewer().unwrap().bytes, b"nested contents");
    app.apply(AppCommand::CloseViewer).unwrap();
    app.apply(AppCommand::CdUp).unwrap();
    drain_background(&mut app);
    select(&mut app, "..");
    app.apply(AppCommand::OpenEntry).unwrap();
    drain_background(&mut app);
    assert_eq!(app.active_panel().cwd, archive.join("packages"));
    assert_eq!(
        app.active_panel().selected_entry().unwrap().name,
        "inner.tar"
    );
    for _ in 0..2 {
        app.apply(AppCommand::CdUp).unwrap();
        drain_background(&mut app);
    }
    assert!(!app.active_panel().is_archive());
    app.apply(AppCommand::OpenFileHistory).unwrap();
    app.apply(AppCommand::DialogAccept).unwrap();
    drain_background(&mut app);
    assert_eq!(app.active_viewer().unwrap().bytes, b"nested contents");
}

#[test]
fn nested_sources_detect_outer_changes_and_protect_outer_file() {
    use crate::archive::{ArchiveIndex, ArchiveReader};
    let files = tempfile::tempdir().unwrap();
    let archive = files.path().join("outer.zip");
    fs::write(
        &archive,
        zip_bytes("inner.tar", &tar_bytes("outer.zip", b"first")),
    )
    .unwrap();
    let location = ArchiveLocation::nested(
        ArchiveLocation::root(archive.clone()),
        archive.join("inner.tar"),
    )
    .unwrap();
    let first = ArchiveIndex::open(&location, None).unwrap();
    let reader = ArchiveReader::open(&location, None).unwrap();
    let before = fs::read(&archive).unwrap();
    assert!(
        extract(
            location.clone(),
            vec![location.archive.join("outer.zip")],
            files.path().to_path_buf()
        )
        .is_err()
    );
    assert_eq!(fs::read(&archive).unwrap(), before);
    fs::write(
        &archive,
        zip_bytes("inner.tar", &tar_bytes("replacement", b"new contents")),
    )
    .unwrap();
    assert!(reader.verify_source().is_err());
    let second = ArchiveIndex::open(&location, None).unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
    assert!(second.members.contains_key(Path::new("replacement")));
    assert!(!second.members.contains_key(Path::new("outer.zip")));
    assert!(ArchiveIndex::open(&location, Some(&AtomicBool::new(true))).is_err());
}

#[test]
fn nested_paths_reject_traversal_depth_and_invalid_containers() {
    let mut location = ArchiveLocation::root("/outer.zip".into());
    assert!(
        ArchiveLocation::nested(location.clone(), "/outer.zip/../elsewhere.zip".into()).is_err()
    );
    for _ in 1..8 {
        let path = location.archive.join("inner.zip");
        location = ArchiveLocation::nested(location, path).unwrap();
    }
    assert!(
        ArchiveLocation::nested(location.clone(), location.archive.join("too-deep.zip")).is_err()
    );
    let files = tempfile::tempdir().unwrap();
    let archive = files.path().join("outer.tar");
    fs::write(&archive, tar_bytes("bad.zip", b"not an archive")).unwrap();
    let location = ArchiveLocation::nested(
        ArchiveLocation::root(archive.clone()),
        archive.join("bad.zip"),
    )
    .unwrap();
    assert!(
        location
            .read_entries(SortMode::default(), true, None)
            .is_err()
    );
}

#[test]
fn nesting_depth_error_remains_visible_without_changing_the_panel() {
    let files = tempfile::tempdir().unwrap();
    let mut app = app_with_loaded_panels(files.path().to_path_buf());
    let mut location = ArchiveLocation::root(files.path().join("outer.zip"));
    for _ in 1..8 {
        let path = location.archive.join("inner.zip");
        location = ArchiveLocation::nested(location, path).unwrap();
    }
    let cwd = location.path();
    let mut entry = file_entry("too-deep.zip");
    entry.path = cwd.join("too-deep.zip");
    let panel = app.active_panel_mut();
    panel.source = PanelListingSource::Archive(location);
    panel.cwd = cwd.clone();
    panel.entries = vec![entry];
    panel.cursor = 0;
    app.apply_builtin_file_action(crate::extension_rules::ExtensionAction::Open);
    assert!(
        app.status_line.contains("eight levels"),
        "{}",
        app.status_line
    );
    assert_eq!(app.active_panel().cwd, cwd);
    assert!(app.active_viewer().is_none());
}

#[test]
fn compressed_outer_archives_can_open_inner_zip_members() {
    let files = tempfile::tempdir().unwrap();
    let inner = zip_bytes("message.txt", b"compressed nesting");
    let outer = tar_bytes("inner.zip", &inner);
    for extension in ["tgz", "tbz2"] {
        let path = files.path().join(format!("outer.{extension}"));
        let file = fs::File::create(&path).unwrap();
        if extension == "tgz" {
            let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
            encoder.write_all(&outer).unwrap();
            encoder.finish().unwrap();
        } else {
            let mut encoder = bzip2::write::BzEncoder::new(file, bzip2::Compression::fast());
            encoder.write_all(&outer).unwrap();
            encoder.finish().unwrap();
        }
        let location =
            ArchiveLocation::nested(ArchiveLocation::root(path.clone()), path.join("inner.zip"))
                .unwrap();
        let viewer = ViewerState::open_archive_cancellable(
            &location,
            location.archive.join("message.txt"),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(viewer.bytes, b"compressed nesting");
    }
}
