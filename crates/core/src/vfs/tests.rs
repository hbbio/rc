use super::*;
use std::io::{Cursor, Write};
use std::sync::atomic::AtomicUsize;

struct MemoryBackend {
    entries: Vec<FileEntry>,
    closed: AtomicUsize,
    cancel_on_read: Option<Arc<AtomicBool>>,
}

fn entry(path: impl Into<PathBuf>, kind: FileEntryKind) -> FileEntry {
    let path = path.into();
    FileEntry {
        name: "untrusted display label".into(),
        path,
        kind,
        size: 7,
        modified: None,
        metadata: FileEntryMetadata::default(),
    }
}

impl Backend for MemoryBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Helper
    }
    fn read_directory(&self, _: &Path, cancel: &Cancellation<'_>) -> io::Result<Vec<FileEntry>> {
        cancel.check()?;
        Ok(self.entries.clone())
    }
    fn metadata(&self, path: &Path, cancel: &Cancellation<'_>) -> io::Result<FileEntry> {
        cancel.check()?;
        self.entries
            .iter()
            .find(|entry| entry.path == path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing member"))
    }
    fn with_file(
        &self,
        path: &Path,
        cancel: &Cancellation<'_>,
        consume: &mut dyn FnMut(&FileEntry, &mut dyn Read) -> io::Result<()>,
    ) -> io::Result<()> {
        let entry = self.metadata(path, cancel)?;
        struct Input<'a> {
            cursor: Cursor<&'static [u8]>,
            cancel: Option<&'a AtomicBool>,
        }
        impl Read for Input<'_> {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                if let Some(cancel) = self.cancel {
                    cancel.store(true, Ordering::Relaxed);
                }
                self.cursor.read(bytes)
            }
        }
        consume(
            &entry,
            &mut Input {
                cursor: Cursor::new(b"payload"),
                cancel: self.cancel_on_read.as_deref(),
            },
        )
    }
    fn close(&self) {
        self.closed.fetch_add(1, Ordering::Relaxed);
    }
}

fn mount(backend: Arc<MemoryBackend>) -> Arc<Mount> {
    Mount::new(
        "Memory".into(),
        "memory://mount".into(),
        VfsPath::Local("/origin".into()),
        backend,
    )
    .unwrap()
}

#[test]
fn mounted_paths_list_and_view_without_becoming_native_paths() {
    let backend = Arc::new(MemoryBackend {
        entries: vec![
            entry("file", FileEntryKind::File),
            entry("folder", FileEntryKind::Directory),
            entry(".hidden", FileEntryKind::File),
        ],
        closed: AtomicUsize::new(0),
        cancel_on_read: None,
    });
    let mount = mount(Arc::clone(&backend));
    let root = mount.root();
    assert!(root.native_path().is_none());
    assert!(root.capabilities().read_only);
    let entries = root
        .read_directory(SortMode::default(), false, None)
        .unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["..", "folder", "file"]
    );
    assert_eq!(entries[0].path, Path::new("/origin"));
    let file = root.resolve_identity("memory://mount/file".into()).unwrap();
    let viewer =
        crate::ViewerState::open_vfs_cancellable(file.clone(), &AtomicBool::new(false)).unwrap();
    assert_eq!(viewer.bytes, b"payload");
    assert_eq!(viewer.path(), file.identity());
    assert_eq!(file.parent().unwrap(), Some(root.clone()));
    assert_eq!(
        root.parent().unwrap(),
        Some(VfsPath::Local("/origin".into()))
    );
    assert!(root.resolve_identity("memory://other/file".into()).is_err());
    assert!(
        root.resolve_identity("memory://mount/../escape".into())
            .is_err()
    );
    mount.close();
    mount.close();
    assert_eq!(backend.closed.load(Ordering::Relaxed), 1);
    assert_eq!(
        file.metadata(None).unwrap_err().kind(),
        io::ErrorKind::Interrupted
    );
    assert!(
        file.with_file(None, &mut |_, _| panic!("closed backend invoked reader"))
            .is_err()
    );
}

#[test]
fn mounted_listing_rejects_unsafe_duplicate_and_foreign_children() {
    for entries in [
        vec![entry("../escape", FileEntryKind::File)],
        vec![entry("dir/file", FileEntryKind::File)],
        vec![
            entry("same", FileEntryKind::File),
            entry("same", FileEntryKind::File),
        ],
        vec![entry("parent", FileEntryKind::Parent)],
    ] {
        let root = mount(Arc::new(MemoryBackend {
            entries,
            closed: AtomicUsize::new(0),
            cancel_on_read: None,
        }))
        .root();
        assert!(
            root.read_directory(SortMode::default(), true, None)
                .is_err()
        );
    }
}

#[test]
fn cancellation_inside_read_to_end_does_not_retry_forever() {
    let canceled = Arc::new(AtomicBool::new(false));
    let root = mount(Arc::new(MemoryBackend {
        entries: vec![entry("file", FileEntryKind::File)],
        closed: AtomicUsize::new(0),
        cancel_on_read: Some(Arc::clone(&canceled)),
    }))
    .root();
    let path = root.resolve_identity("memory://mount/file".into()).unwrap();
    let error = path
        .with_file(Some(&canceled), &mut |_, reader| {
            reader.read_to_end(&mut Vec::new())?;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
}

#[test]
fn backend_cannot_swallow_consumer_failures_or_repeat_the_callback() {
    struct FaultyBackend {
        calls: usize,
    }
    impl Backend for FaultyBackend {
        fn kind(&self) -> BackendKind {
            BackendKind::Helper
        }
        fn read_directory(&self, _: &Path, _: &Cancellation<'_>) -> io::Result<Vec<FileEntry>> {
            unreachable!()
        }
        fn metadata(&self, _: &Path, _: &Cancellation<'_>) -> io::Result<FileEntry> {
            unreachable!()
        }
        fn with_file(
            &self,
            path: &Path,
            _: &Cancellation<'_>,
            consume: &mut dyn FnMut(&FileEntry, &mut dyn Read) -> io::Result<()>,
        ) -> io::Result<()> {
            for _ in 0..self.calls {
                let _ = consume(
                    &entry(path, FileEntryKind::File),
                    &mut Cursor::new(b"payload"),
                );
            }
            Ok(())
        }
    }
    for calls in [0, 1, 2] {
        let mount = Mount::new(
            "Faulty".into(),
            "memory://faulty".into(),
            VfsPath::Local("/origin".into()),
            Arc::new(FaultyBackend { calls }),
        )
        .unwrap();
        let path = mount
            .root()
            .resolve_identity("memory://faulty/file".into())
            .unwrap();
        let error = path
            .with_file(None, &mut |_, _| {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "destination denied",
                ))
            })
            .unwrap_err();
        assert_eq!(
            error.kind(),
            if calls == 0 {
                io::ErrorKind::InvalidData
            } else {
                io::ErrorKind::PermissionDenied
            }
        );
        if calls == 2 {
            assert_eq!(
                path.with_file(None, &mut |_, _| Ok(())).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
    }
}

#[test]
fn local_and_archive_readers_preserve_snapshot_metadata_and_parent_boundaries() {
    let files = tempfile::tempdir().unwrap();
    let local = files.path().join("file");
    std::fs::write(&local, b"native contents").unwrap();
    let path = VfsPath::Local(local.clone());
    assert_eq!(path.native_path(), Some(local.as_path()));
    path.with_file(None, &mut |entry, input| {
        assert_eq!(entry.size, 15);
        let mut bytes = Vec::new();
        input.read_to_end(&mut bytes)?;
        assert_eq!(bytes, b"native contents");
        Ok(())
    })
    .unwrap();
    let archive = files.path().join("archive.zip");
    let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
    zip.start_file("folder/file", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"archive contents").unwrap();
    zip.finish().unwrap();
    let root = VfsPath::from(ArchiveLocation::root(archive.clone()));
    assert!(root.native_path().is_none());
    assert!(root.metadata(None).unwrap().is_dir());
    let member = root.resolve_identity(archive.join("folder/file")).unwrap();
    let viewer = crate::ViewerState::open_vfs_cancellable(member, &AtomicBool::new(false)).unwrap();
    assert_eq!(viewer.bytes, b"archive contents");
    assert_eq!(
        root.parent().unwrap(),
        Some(VfsPath::Local(files.path().to_path_buf()))
    );
    let outer = ArchiveLocation::root(archive.clone()).in_directory("folder".into());
    let nested = ArchiveLocation::nested(outer.clone(), archive.join("folder/inner.tar")).unwrap();
    assert_eq!(VfsPath::from(nested).parent().unwrap(), Some(outer.into()));
}

#[cfg(unix)]
#[test]
fn native_member_names_and_local_symlinks_remain_intact() {
    use std::os::unix::ffi::OsStrExt;
    let name = PathBuf::from(std::ffi::OsStr::from_bytes(b"raw\xff\n"));
    let root = mount(Arc::new(MemoryBackend {
        entries: vec![entry(name.clone(), FileEntryKind::File)],
        closed: AtomicUsize::new(0),
        cancel_on_read: None,
    }))
    .root();
    let entries = root
        .read_directory(SortMode::default(), true, None)
        .unwrap();
    assert_eq!(entries[1].path.file_name(), name.file_name());
    let files = tempfile::tempdir().unwrap();
    std::fs::write(files.path().join("target"), b"linked contents").unwrap();
    let link = files.path().join("link");
    std::os::unix::fs::symlink("target", &link).unwrap();
    let path = VfsPath::Local(link.clone());
    assert_eq!(
        path.metadata(None).unwrap().metadata.symlink_target,
        Some(true)
    );
    let viewer = crate::ViewerState::open_vfs_cancellable(path, &AtomicBool::new(false)).unwrap();
    assert_eq!(viewer.bytes, b"linked contents");
    assert_eq!(viewer.path(), link);
}
