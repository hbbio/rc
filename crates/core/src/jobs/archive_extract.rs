use super::{
    COPY_BUFFER_SIZE, JobTotals, OverwritePolicy, ProgressTracker,
    canonicalize_allowing_missing_tail, destination_path, destination_staging_path,
    ensure_not_canceled, existing_paths_refer_to_same_entry, renamed_destination,
    resolve_destination,
};
use crate::ArchiveLocation;
use crate::archive::{ArchiveIndex, ArchiveMember, ArchiveReader, normalize_archive_io_result};
use filetime::FileTime;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

pub(super) fn measure_archive_totals(
    location: &ArchiveLocation,
    sources: &[PathBuf],
    cancel: &AtomicBool,
) -> io::Result<JobTotals> {
    let reader = ArchiveIndex::open(location, Some(cancel))?;
    let selected = reader.selections(location, sources)?;
    let mut totals = JobTotals::default();
    for source in selected {
        for (_, member) in reader.subtree(&source) {
            ensure_not_canceled(cancel)?;
            totals.items = totals.items.saturating_add(1);
            if !member.is_dir {
                totals.bytes = totals.bytes.saturating_add(member.size);
            }
        }
    }
    Ok(totals)
}

pub(super) fn extract_archive(
    location: &ArchiveLocation,
    sources: &[PathBuf],
    destination_dir: &Path,
    overwrite: OverwritePolicy,
    progress: &mut ProgressTracker<'_>,
) -> io::Result<()> {
    let cancel = Arc::clone(&progress.cancel_flag);
    let mut reader = ArchiveReader::open(location, Some(cancel.as_ref()))?;
    let mut selected = reader.selections(location, sources)?;
    selected.sort_by_key(|path| {
        reader
            .subtree(path)
            .filter_map(|(_, member)| member.index)
            .min()
    });
    let archive_path = fs::canonicalize(&location.archive)?;
    for source in selected {
        progress.ensure_not_canceled()?;
        let members = reader
            .subtree(&source)
            .map(|(path, member)| (path.clone(), member.clone()))
            .collect::<Vec<_>>();
        let source_path = location.archive.join(&source);
        let totals = JobTotals {
            items: members.len() as u64,
            bytes: members
                .iter()
                .filter(|(_, member)| !member.is_dir)
                .fold(0_u64, |total, (_, member)| {
                    total.saturating_add(member.size)
                }),
        };
        let mut destination = destination_path(&source, destination_dir)?;
        // An extraction must never replace the open archive or a containing
        // directory, including through a symlink or hard-link alias.
        let destination_identity = canonicalize_allowing_missing_tail(&destination)?;
        if archive_path.starts_with(&destination_identity)
            || existing_paths_refer_to_same_entry(&location.archive, &destination)?
        {
            match overwrite {
                OverwritePolicy::Skip => {
                    progress.advance_totals(&source_path, totals);
                    continue;
                }
                OverwritePolicy::Rename => {
                    destination = renamed_destination(&destination)?;
                }
                OverwritePolicy::Overwrite => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "cannot extract over the source {} archive",
                            location.format()?.label()
                        ),
                    ));
                }
            }
        }
        let Some(destination) =
            resolve_destination(&source_path, destination, overwrite, totals, progress)?
        else {
            continue;
        };
        let result = (|| {
            for (_, member) in &members {
                progress.ensure_not_canceled()?;
                member.ensure_extractable()?;
            }
            extract_selection(
                &mut reader,
                location,
                &source,
                &members,
                &destination.path,
                progress,
            )
        })();
        destination.finish(result)?;
    }
    Ok(())
}

fn extract_selection(
    reader: &mut ArchiveReader<'_>,
    location: &ArchiveLocation,
    source: &Path,
    members: &[(PathBuf, ArchiveMember)],
    destination: &Path,
    progress: &mut ProgressTracker<'_>,
) -> io::Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let stage = destination_staging_path(destination)?;
    let root = &reader.members[source];
    // Staging paths live beside the destination, where other users may be able
    // to search the parent. Restrict access at creation, before writing payloads.
    let mut directories = fs::DirBuilder::new();
    let mut files = fs::OpenOptions::new();
    files.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        directories.mode(0o700);
        files.mode(0o600);
    }
    if root.is_dir {
        directories.create(&stage)?;
    } else {
        // Reserve this path before cleanup can ever own it.
        files.create_new(true).open(&stage)?;
    }
    directories.recursive(true);
    let result = (|| {
        let mut ordered = members.iter().collect::<Vec<_>>();
        // Create parents first, then read payloads in physical archive order.
        // Adjacent TAR files share a decoder instead of replaying checkpoints.
        ordered.sort_by_key(|(_, member)| {
            (
                !member.is_dir,
                if member.is_dir { None } else { member.index },
            )
        });
        for (member_path, member) in ordered {
            progress.ensure_not_canceled()?;
            let path = if member_path == source {
                stage.clone()
            } else {
                stage.join(member_path.strip_prefix(source).map_err(io::Error::other)?)
            };
            let display_path = location.archive.join(member_path);
            progress.set_current_path(&display_path);
            if member.is_dir {
                if member_path != source {
                    directories.create(&path)?;
                }
            } else {
                if let Some(parent) = path.parent() {
                    directories.create(parent)?;
                }
                let mut output = files.create_new(member_path != source).open(&path)?;
                let mut input = normalize_archive_io_result(
                    reader.open_member(member),
                    Some(progress.cancel_flag.as_ref()),
                )?;
                let mut buffer = [0_u8; COPY_BUFFER_SIZE];
                let mut written = 0_u64;
                loop {
                    progress.ensure_not_canceled()?;
                    let length = normalize_archive_io_result(
                        input.read(&mut buffer),
                        Some(progress.cancel_flag.as_ref()),
                    )?;
                    if length == 0 {
                        break;
                    }
                    written = written
                        .checked_add(length as u64)
                        .ok_or_else(|| io::Error::other("archive size overflow"))?;
                    if written > member.size {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "archive member exceeds its declared size",
                        ));
                    }
                    output.write_all(&buffer[..length])?;
                    progress.advance_bytes(length as u64);
                }
                if written != member.size {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "archive member is shorter than its declared size",
                    ));
                }
                output.flush()?;
            }
            progress.complete_item(&display_path);
        }
        // Apply directory permissions after their children have been written,
        // keeping the staging root private until all descendants have their
        // final permissions. Entries without archived modes stay private.
        for (member_path, member) in members.iter().rev() {
            progress.ensure_not_canceled()?;
            let path = if member_path == source {
                stage.clone()
            } else {
                stage.join(member_path.strip_prefix(source).map_err(io::Error::other)?)
            };
            if let Some(modified) = member.modified {
                filetime::set_file_mtime(&path, FileTime::from_system_time(modified))?;
            }
            #[cfg(unix)]
            if let Some(mode) = member.mode {
                use std::os::unix::fs::PermissionsExt;
                // Do not restore setuid, setgid or sticky bits from an archive.
                fs::set_permissions(&path, fs::Permissions::from_mode(mode & 0o777))?;
            }
        }
        progress.ensure_not_canceled()?;
        reader.verify_source()?;
        fs::rename(&stage, destination)
    })();
    if result.is_err() {
        cleanup_stage(&stage);
    }
    result
}

fn cleanup_stage(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_dir() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(
                path,
                fs::Permissions::from_mode(metadata.permissions().mode() | 0o700),
            );
        }
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                cleanup_stage(&entry.path());
            }
        }
        let _ = fs::remove_dir(path);
    } else {
        #[cfg(windows)]
        if !metadata.file_type().is_symlink() && metadata.permissions().readonly() {
            let mut permissions = metadata.permissions();
            permissions.set_readonly(false);
            let _ = fs::set_permissions(path, permissions);
        }
        let _ = fs::remove_file(path);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::jobs::JobId;
    use std::cell::Cell;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;

    fn fixture(entries: &[(&str, u32, &[u8])]) -> (tempfile::TempDir, ArchiveLocation) {
        let root = tempfile::tempdir().expect("fixture directory should be created");
        let archive = root.path().join("example.tar");
        let mut builder = tar::Builder::new(fs::File::create(&archive).unwrap());
        for (name, mode, contents) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_mode(*mode);
            header.set_size(contents.len() as u64);
            if name.ends_with('/') {
                header.set_entry_type(tar::EntryType::Directory);
            }
            header.set_cksum();
            builder.append_data(&mut header, name, *contents).unwrap();
        }
        builder.finish().unwrap();
        (root, ArchiveLocation::root(archive))
    }

    fn extract_observing(
        location: &ArchiveLocation,
        source: &Path,
        destination: &Path,
        cancel: &Arc<AtomicBool>,
        inspect: impl FnMut(&ArchiveMember),
    ) -> io::Result<()> {
        let mut reader = ArchiveReader::open(location, Some(cancel))?.inspect_member_opens(inspect);
        let members = reader
            .subtree(source)
            .map(|(path, member)| (path.clone(), member.clone()))
            .collect::<Vec<_>>();
        let (tx, _rx) = mpsc::channel();
        let mut progress =
            ProgressTracker::new(JobId(1), JobTotals::default(), &tx, Arc::clone(cancel));
        extract_selection(
            &mut reader,
            location,
            source,
            &members,
            destination,
            &mut progress,
        )
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path)
            .expect("staged entry should exist")
            .permissions()
            .mode()
            & 0o7777
    }

    #[test]
    fn archive_directory_staging_is_private_until_all_payloads_are_written() {
        for (directory_mode, file_mode) in [(0o700, 0o600), (0o755, 0o644), (0o2700, 0o4600)] {
            let secret = b"private credential material";
            let (root, location) = fixture(&[
                ("tree/", directory_mode, b""),
                ("tree/nested/", directory_mode, b""),
                ("tree/nested/credentials", file_mode, secret),
                ("tree/later", file_mode, b"later payload"),
            ]);
            let destination = root.path().join("tree");
            let stage = destination_staging_path(&destination).unwrap();
            let opened = Cell::new(0);
            extract_observing(
                &location,
                Path::new("tree"),
                &destination,
                &Arc::default(),
                |_| {
                    assert!(
                        !destination.exists(),
                        "the selection must not be published yet"
                    );
                    assert_eq!(mode(&stage), 0o700);
                    assert_eq!(mode(&stage.join("nested")), 0o700);
                    assert_eq!(mode(&stage.join("nested/credentials")), 0o600);
                    if opened.get() == 1 {
                        assert_eq!(fs::read(stage.join("nested/credentials")).unwrap(), secret);
                        assert_eq!(mode(&stage.join("later")), 0o600);
                    }
                    opened.set(opened.get() + 1);
                },
            )
            .expect("directory extraction should succeed");
            assert_eq!(opened.get(), 2);
            assert!(!stage.exists());
            assert_eq!(mode(&destination), directory_mode & 0o777);
            assert_eq!(mode(&destination.join("nested")), directory_mode & 0o777);
            assert_eq!(
                mode(&destination.join("nested/credentials")),
                file_mode & 0o777
            );
            assert_eq!(
                fs::read(destination.join("nested/credentials")).unwrap(),
                secret
            );
        }
    }

    #[test]
    fn archive_file_staging_is_private_and_cancellation_removes_it() {
        for canceled in [false, true] {
            for file_mode in [0o600, 0o644] {
                let (root, location) =
                    fixture(&[("credentials", file_mode, b"private credential material")]);
                let destination = root.path().join("credentials");
                let stage = destination_staging_path(&destination).unwrap();
                let cancel = Arc::new(AtomicBool::new(false));
                let inspected = Cell::new(false);
                let result = extract_observing(
                    &location,
                    Path::new("credentials"),
                    &destination,
                    &cancel,
                    |_| {
                        assert!(!destination.exists());
                        assert_eq!(mode(&stage), 0o600);
                        inspected.set(true);
                        cancel.store(canceled, Ordering::Relaxed);
                    },
                );
                assert!(inspected.get());
                assert!(!stage.exists(), "staging must be moved or removed");
                if canceled {
                    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
                    assert!(!destination.exists());
                } else {
                    result.expect("file extraction should succeed");
                    assert_eq!(mode(&destination), file_mode);
                    assert_eq!(
                        fs::read(&destination).unwrap(),
                        b"private credential material"
                    );
                }
            }
        }
    }

    #[test]
    fn archive_implicit_directories_keep_private_permissions() {
        let (root, location) = fixture(&[(
            "tree/nested/credentials",
            0o600,
            b"private credential material",
        )]);
        let destination = root.path().join("tree");
        extract_observing(
            &location,
            Path::new("tree"),
            &destination,
            &Arc::default(),
            |_| {},
        )
        .expect("implicit directories should be extracted");
        assert_eq!(mode(&destination), 0o700);
        assert_eq!(mode(&destination.join("nested")), 0o700);
    }
}
