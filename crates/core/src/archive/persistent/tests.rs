use super::*;
use crate::archive::ArchiveIndex;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

fn tar_bytes(content: &[u8]) -> Vec<u8> {
    let mut tar = ::tar::Builder::new(Vec::new());
    let mut header = ::tar::Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o640);
    header.set_mtime(123);
    header.set_cksum();
    tar.append_data(&mut header, "folder/message", content)
        .unwrap();
    tar.into_inner().unwrap()
}

fn fixtures(root: &Path, content: &[u8]) {
    let tar = tar_bytes(content);
    fs::write(root.join("outer.tar"), &tar).unwrap();
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gzip.write_all(&tar).unwrap();
    let gzip = gzip.finish().unwrap();
    fs::write(root.join("outer.tgz"), &gzip).unwrap();
    let mut bzip = ::bzip2::write::BzEncoder::new(Vec::new(), ::bzip2::Compression::fast());
    bzip.write_all(&tar).unwrap();
    fs::write(root.join("outer.tbz2"), bzip.finish().unwrap()).unwrap();
    let mut zip = ::zip::ZipWriter::new(File::create(root.join("outer.zip")).unwrap());
    zip.start_file("folder/message", ::zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(content).unwrap();
    zip.start_file("inner.tgz", ::zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(&gzip).unwrap();
    zip.finish().unwrap();
}

fn child(root: &Path, cache: &Path, phase: &str) {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "archive::persistent::tests::indexes_survive_restart_and_recover_from_corruption_and_source_changes", "--nocapture"])
        .env("RC_TEST_INDEX_PHASE", phase).env("RC_TEST_INDEX_ROOT", root).env("RC_TEST_INDEX_CACHE", cache)
        .output().unwrap();
    assert!(
        output.status.success(),
        "{phase}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn indexes_survive_restart_and_recover_from_corruption_and_source_changes() {
    if let Ok(phase) = std::env::var("RC_TEST_INDEX_PHASE") {
        let root = PathBuf::from(std::env::var_os("RC_TEST_INDEX_ROOT").unwrap());
        configure_archive_index_cache(std::env::var_os("RC_TEST_INDEX_CACHE").unwrap().into())
            .unwrap();
        let expected = if phase == "changed" {
            b"replacement contents".as_slice()
        } else {
            b"original contents"
        };
        for suffix in ["zip", "tar", "tgz", "tbz2"] {
            let location = ArchiveLocation::root(root.join(format!("outer.{suffix}")));
            let index = ArchiveIndex::open(&location, None).unwrap();
            if phase == "reuse" && matches!(suffix, "tgz" | "tbz2") {
                assert_eq!(
                    index.backend.estimated_bytes(),
                    0,
                    "cached metadata should defer seek-index construction"
                );
            }
            assert_eq!(
                index.members[Path::new("folder")].size,
                expected.len() as u64
            );
            let viewer = crate::ViewerState::open_archive_cancellable(
                &location,
                location.archive.join("folder/message"),
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(viewer.bytes, expected);
            if matches!(suffix, "tgz" | "tbz2") {
                assert!(index.backend.estimated_bytes() > 0);
            }
        }
        let outer = ArchiveLocation::root(root.join("outer.zip"));
        let nested =
            ArchiveLocation::nested(outer.clone(), outer.archive.join("inner.tgz")).unwrap();
        let index = ArchiveIndex::open(&nested, None).unwrap();
        if phase == "reuse" {
            assert_eq!(
                index.backend.estimated_bytes(),
                0,
                "nested keys must survive rematerialization"
            );
        }
        let viewer = crate::ViewerState::open_archive_cancellable(
            &nested,
            nested.archive.join("folder/message"),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(viewer.bytes, expected);
        return;
    }
    let files = tempfile::tempdir().unwrap();
    let root = files.path().join("sources");
    let cache = files.path().join("indexes");
    fs::create_dir(&root).unwrap();
    fixtures(&root, b"original contents");
    child(&root, &cache, "build");
    let entries = fs::read_dir(&cache)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 5);
    assert_eq!(fs::metadata(&cache).unwrap().mode() & 0o777, 0o700);
    let old_time = filetime::FileTime::from_unix_time(100, 0);
    for entry in &entries {
        assert_eq!(fs::metadata(entry).unwrap().mode() & 0o777, 0o600);
        filetime::set_file_mtime(entry, old_time).unwrap();
    }
    child(&root, &cache, "reuse");
    for entry in &entries {
        assert_eq!(
            filetime::FileTime::from_last_modification_time(&fs::metadata(entry).unwrap()),
            old_time,
            "cache hit must not rewrite the index"
        );
        fs::write(entry, b"corrupt cache").unwrap();
    }
    child(&root, &cache, "repair");
    assert!(
        entries
            .iter()
            .all(|path| fs::read(path).unwrap().starts_with(MAGIC))
    );
    fixtures(&root, b"replacement contents");
    child(&root, &cache, "changed");
}

#[test]
fn codec_preserves_native_names_and_rejects_corrupt_or_unsafe_metadata() {
    use std::os::unix::ffi::OsStrExt;
    let mut members = BTreeMap::new();
    let path = PathBuf::from(std::ffi::OsStr::from_bytes(b"native\xff"));
    members.insert(
        path.clone(),
        ArchiveMember {
            index: Some(512),
            order: 2,
            is_dir: false,
            size: 5,
            modified: Some(UNIX_EPOCH - Duration::new(10, 23)),
            mode: Some(0o100640),
            encrypted: false,
            unsupported: false,
        },
    );
    let mut encoded = encode(&members, b"key", None).unwrap();
    let restored = decode(&encoded, b"key", None).unwrap();
    assert_eq!(restored[&path].modified, members[&path].modified);
    assert_eq!(restored[&path].mode, Some(0o100640));
    assert_eq!(restored[&path].index, Some(512));
    assert!(decode(&encoded, b"different identity", None).is_err());
    assert!(decode(&encoded, b"key", Some(&AtomicBool::new(true))).is_err());
    encoded[12] ^= 1;
    assert!(decode(&encoded, b"key", None).is_err());
    let member = members.remove(&path).unwrap();
    members.insert("../escape".into(), member);
    assert!(decode(&encode(&members, b"key", None).unwrap(), b"key", None).is_err());
}

#[test]
fn cache_directory_permissions_and_symlinks_are_checked_before_use() {
    let files = tempfile::tempdir().unwrap();
    let private = files.path().join("private");
    fs::create_dir(&private).unwrap();
    fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(validate_directory(&private).is_ok());
    let link = files.path().join("link");
    std::os::unix::fs::symlink(&private, &link).unwrap();
    assert!(validate_directory(&link).is_err());
    fs::set_permissions(&private, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(validate_directory(&private).is_err());
}

#[test]
fn pruning_keeps_cache_file_count_bounded_without_following_symlinks() {
    let files = tempfile::tempdir().unwrap();
    let store = Store {
        directory: files.path().to_path_buf(),
        maintenance: Mutex::new(()),
    };
    for number in 0..70 {
        let path = files.path().join(format!("{number:016x}.idx"));
        fs::write(&path, b"cached").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::os::unix::fs::symlink(outside.path(), files.path().join("ffffffffffffffff.idx")).unwrap();
    store.prune(None).unwrap();
    assert!(outside.path().exists());
    assert_eq!(fs::read_dir(files.path()).unwrap().count(), CACHE_FILES + 1);
}

#[test]
fn pruning_removes_abandoned_temporary_indexes_but_keeps_active_writers() {
    let files = tempfile::tempdir().unwrap();
    let store = Store {
        directory: files.path().to_path_buf(),
        maintenance: Mutex::new(()),
    };
    let abandoned = tempfile::Builder::new()
        .prefix(".index-")
        .tempfile_in(files.path())
        .unwrap();
    let (file, abandoned) = abandoned.keep().unwrap();
    drop(file);
    let active = tempfile::Builder::new()
        .prefix(".index-")
        .tempfile_in(files.path())
        .unwrap();
    rustix::fs::flock(
        active.as_file(),
        rustix::fs::FlockOperation::NonBlockingLockExclusive,
    )
    .unwrap();
    store.prune(None).unwrap();
    assert!(!abandoned.exists());
    assert!(active.path().exists());
    rustix::fs::flock(active.as_file(), rustix::fs::FlockOperation::Unlock).unwrap();
    store.prune(None).unwrap();
    assert!(!active.path().exists());
}
