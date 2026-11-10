use super::{
    JobTotals, OverwritePolicy, ProgressTracker, canonicalize_allowing_missing_tail,
    destination_path, existing_paths_refer_to_same_entry, renamed_destination, resolve_destination,
};
use crate::{
    FileEntry,
    archive::SourceIdentity,
    vfs::{VfsPath, walk},
};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) fn totals<'a>(entries: impl Iterator<Item = &'a FileEntry>) -> JobTotals {
    entries.fold(JobTotals::default(), |mut totals, entry| {
        totals.items += 1;
        if !entry.is_dir() {
            totals.bytes = totals.bytes.saturating_add(entry.size);
        }
        totals
    })
}

fn extractable(entry: &FileEntry) -> io::Result<()> {
    let regular_mode = entry
        .metadata
        .mode
        .is_none_or(|mode| mode & 0o170000 == if entry.is_dir() { 0o040000 } else { 0o100000 });
    if entry.is_parent() || entry.metadata.symlink_target.is_some() || !regular_mode {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "VFS links and special files cannot be copied",
        ));
    }
    Ok(())
}

struct SourceGuard(Vec<(PathBuf, SourceIdentity)>);
impl SourceGuard {
    fn new(location: &VfsPath) -> io::Result<Self> {
        location
            .protected_local_paths()
            .into_iter()
            .map(|path| {
                let identity = SourceIdentity::new(&fs::metadata(&path)?);
                Ok((path, identity))
            })
            .collect::<io::Result<Vec<_>>>()
            .map(Self)
    }
    fn verify(&self) -> io::Result<()> {
        for (path, identity) in &self.0 {
            identity.verify(&fs::metadata(path)?)?;
        }
        Ok(())
    }
    fn contains_destination(&self, destination: &Path) -> io::Result<bool> {
        let resolved = canonicalize_allowing_missing_tail(destination)?;
        for (path, _) in &self.0 {
            if fs::canonicalize(path)?.starts_with(&resolved)
                || existing_paths_refer_to_same_entry(path, destination)?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

pub(super) fn extract(
    location: &VfsPath,
    sources: &[PathBuf],
    destination_dir: &Path,
    overwrite: OverwritePolicy,
    progress: &mut ProgressTracker<'_>,
) -> io::Result<()> {
    let cancel = Arc::clone(&progress.cancel_flag);
    let guard = SourceGuard::new(location)?;
    let scan = walk::scan(location, sources, &cancel, true)?;
    guard.verify()?;
    let mut selected = sources.to_vec();
    selected.sort();
    selected.dedup();
    let mut roots: Vec<PathBuf> = Vec::new();
    for path in selected {
        if !roots.iter().any(|parent| path.starts_with(parent)) {
            roots.push(path);
        }
    }
    for root in roots {
        progress.ensure_not_canceled()?;
        let members: Vec<_> = scan
            .entries
            .values()
            .filter(|entry| entry.path.starts_with(&root))
            .collect();
        if members.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "VFS selection disappeared",
            ));
        }
        let totals = totals(members.iter().copied());
        let mut destination = destination_path(&root, destination_dir)?;
        if guard.contains_destination(&destination)? {
            match overwrite {
                OverwritePolicy::Skip => {
                    progress.advance_totals(&root, totals);
                    continue;
                }
                OverwritePolicy::Rename => destination = renamed_destination(&destination)?,
                OverwritePolicy::Overwrite => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "cannot copy over a VFS source container",
                    ));
                }
            }
        }
        let Some(destination) =
            resolve_destination(&root, destination, overwrite, totals, progress)?
        else {
            continue;
        };
        let result = (|| {
            for member in &members {
                extractable(member)?;
            }
            stage_selection(
                location,
                &root,
                &members,
                &destination.path,
                &guard,
                progress,
            )
        })();
        destination.finish(result)?;
    }
    Ok(())
}

fn stage_selection(
    location: &VfsPath,
    root: &Path,
    members: &[&FileEntry],
    destination: &Path,
    guard: &SourceGuard,
    progress: &mut ProgressTracker<'_>,
) -> io::Result<()> {
    let parent = destination.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = tempfile::Builder::new()
        .prefix(".rc-vfs-stage-")
        .tempdir_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700))?;
    }
    // Directory roots are renamed within the same parent. macOS requires
    // write permission on a directory moved to a different parent, which would
    // otherwise prevent publishing a legitimately read-only source directory.
    let root_is_directory = members.first().is_some_and(|entry| entry.is_dir());
    let stage = if root_is_directory {
        temporary.path().to_path_buf()
    } else {
        temporary.path().join("payload")
    };
    let path_in_stage = |path: &Path| -> io::Result<PathBuf> {
        let relative = path.strip_prefix(root).map_err(io::Error::other)?;
        crate::vfs::validate_relative(relative)?;
        Ok(if relative.as_os_str().is_empty() {
            stage.clone()
        } else {
            stage.join(relative)
        })
    };
    let mut directories = fs::DirBuilder::new();
    let mut files = fs::OpenOptions::new();
    files.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        directories.mode(0o700);
        files.mode(0o600);
    }
    let cancel = Arc::clone(&progress.cancel_flag);
    let result = (|| {
        // BTreeMap order places every directory before its descendants.
        for member in members {
            progress.ensure_not_canceled()?;
            let target = path_in_stage(&member.path)?;
            progress.set_current_path(&member.path);
            if member.is_dir() {
                if member.path != root {
                    directories.create(&target)?;
                }
            } else {
                let mut output = files.open(&target)?;
                let source = location.resolve_identity(member.path.clone())?;
                source.with_file(Some(&cancel), &mut |actual, input| {
                    extractable(actual)?;
                    if actual.is_dir()
                        || (member.size != 0 && actual.size != member.size)
                        || member.modified != actual.modified
                    {
                        return Err(io::Error::other(
                            "VFS file changed after listing; refresh and retry",
                        ));
                    }
                    if member.size == 0 {
                        progress.progress.bytes_total =
                            progress.progress.bytes_total.saturating_add(actual.size);
                    }
                    let mut bytes = 0_u64;
                    let mut buffer = [0_u8; super::COPY_BUFFER_SIZE];
                    loop {
                        progress.ensure_not_canceled()?;
                        let count = input.read(&mut buffer)?;
                        if count == 0 {
                            break;
                        }
                        bytes = bytes
                            .checked_add(count as u64)
                            .ok_or_else(|| io::Error::other("VFS size overflow"))?;
                        if bytes > actual.size {
                            return Err(io::Error::other("VFS file exceeds its opened size"));
                        }
                        output.write_all(&buffer[..count])?;
                        progress.advance_bytes(count as u64);
                    }
                    if bytes != actual.size {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "VFS file was truncated",
                        ));
                    }
                    output.flush()
                })?;
            }
            progress.complete_item(&member.path);
        }
        for member in members.iter().rev() {
            progress.ensure_not_canceled()?;
            let target = path_in_stage(&member.path)?;
            if let Some(time) = member.modified {
                filetime::set_file_mtime(&target, filetime::FileTime::from_system_time(time))?;
            }
            #[cfg(unix)]
            if let Some(mode) = member.metadata.mode {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&target, fs::Permissions::from_mode(mode & 0o777))?;
            }
        }
        progress.ensure_not_canceled()?;
        guard.verify()?;
        // Closed mounts must not publish a selection even if the last read
        // finished just before the close request.
        location.metadata(Some(&cancel))?;
        fs::rename(&stage, destination)
    })();
    if result.is_err() {
        super::archive_extract::cleanup_stage(&stage);
    }
    if result.is_ok() && root_is_directory {
        let _ = temporary.keep();
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{
        FileEntryKind, FileEntryMetadata,
        vfs::{Backend, BackendKind, Cancellation, Mount},
    };
    use std::io::{Cursor, Read};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[derive(Clone, Copy, Debug)]
    enum Fault {
        Read,
        Truncated,
        Changed,
        Canceled,
    }
    struct Faulty {
        source: PathBuf,
        destination: PathBuf,
        fault: Fault,
        cancel: Arc<AtomicBool>,
    }
    fn entry(path: &Path) -> FileEntry {
        let directory = path.as_os_str().is_empty() || path == Path::new("tree");
        FileEntry {
            path: path.into(),
            name: String::new(),
            kind: if directory {
                FileEntryKind::Directory
            } else {
                FileEntryKind::File
            },
            size: if directory { 8 } else { 4 },
            modified: None,
            metadata: FileEntryMetadata {
                mode: Some(if directory { 0o040555 } else { 0o100644 }),
                ..FileEntryMetadata::default()
            },
        }
    }
    impl Backend for Faulty {
        fn kind(&self) -> BackendKind {
            BackendKind::Helper
        }
        fn protected_local_paths(&self) -> Vec<PathBuf> {
            vec![self.source.clone()]
        }
        fn metadata(&self, path: &Path, _: &Cancellation<'_>) -> io::Result<FileEntry> {
            Ok(entry(path))
        }
        fn read_directory(&self, path: &Path, _: &Cancellation<'_>) -> io::Result<Vec<FileEntry>> {
            Ok(if path.as_os_str().is_empty() {
                vec![entry(Path::new("tree"))]
            } else {
                vec![entry(Path::new("tree/a")), entry(Path::new("tree/b"))]
            })
        }
        fn with_file(
            &self,
            path: &Path,
            _: &Cancellation<'_>,
            consume: &mut dyn FnMut(&FileEntry, &mut dyn Read) -> io::Result<()>,
        ) -> io::Result<()> {
            let stage = fs::read_dir(&self.destination)?
                .filter_map(Result::ok)
                .find(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".rc-vfs-stage-")
                })
                .unwrap()
                .path();
            assert_eq!(fs::metadata(&stage)?.permissions().mode() & 0o777, 0o700);
            assert!(
                !self.destination.join("tree").exists(),
                "destination remains uncommitted"
            );
            if path == Path::new("tree/b") {
                assert_eq!(fs::read(stage.join("a"))?, b"data");
                match self.fault {
                    Fault::Read => return Err(io::Error::other("injected failure")),
                    Fault::Truncated => return consume(&entry(path), &mut Cursor::new(b"da")),
                    Fault::Changed => fs::write(&self.source, b"changed source")?,
                    Fault::Canceled => self.cancel.store(true, Ordering::Relaxed),
                }
            }
            consume(&entry(path), &mut Cursor::new(b"data"))
        }
    }

    #[test]
    fn failed_copies_restore_overwrites_remove_private_stages_and_detect_changed_sources() {
        for fault in [
            Fault::Read,
            Fault::Truncated,
            Fault::Changed,
            Fault::Canceled,
        ] {
            let root = tempfile::tempdir().unwrap();
            let source = root.path().join("source");
            fs::write(&source, b"original source").unwrap();
            let destination = root.path().join("output");
            fs::create_dir_all(destination.join("tree")).unwrap();
            fs::write(destination.join("tree/original"), b"keep this").unwrap();
            let cancel = Arc::new(AtomicBool::new(false));
            let mount = Mount::new(
                "test".into(),
                root.path().join("mounted"),
                VfsPath::Local(root.path().into()),
                Arc::new(Faulty {
                    source,
                    destination: destination.clone(),
                    fault,
                    cancel: Arc::clone(&cancel),
                }),
            )
            .unwrap();
            let location = mount.root();
            let (tx, _rx) = std::sync::mpsc::channel();
            let mut progress = ProgressTracker::new(
                super::super::JobId(1),
                JobTotals { items: 3, bytes: 8 },
                &tx,
                cancel,
            );
            assert!(
                extract(
                    &location,
                    &[location.identity().join("tree")],
                    &destination,
                    OverwritePolicy::Overwrite,
                    &mut progress
                )
                .is_err(),
                "{fault:?}"
            );
            assert_eq!(
                fs::read(destination.join("tree/original")).unwrap(),
                b"keep this"
            );
            assert_eq!(
                fs::read_dir(&destination).unwrap().count(),
                1,
                "{fault:?} left staging or backups"
            );
        }
    }
}
