use super::*;
use crate::archive::{ZipMember, ZipReader, normalize_zip_io_result, zip_error};

pub(super) fn measure_zip_totals(
    location: &ZipLocation,
    sources: &[PathBuf],
    cancel: &AtomicBool,
) -> io::Result<JobTotals> {
    let reader = ZipReader::open(location, Some(cancel))?;
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

pub(super) fn extract_zip(
    location: &ZipLocation,
    sources: &[PathBuf],
    destination_dir: &Path,
    overwrite: OverwritePolicy,
    progress: &mut ProgressTracker<'_>,
) -> io::Result<()> {
    let cancel = Arc::clone(&progress.cancel_flag);
    let mut reader = ZipReader::open(location, Some(cancel.as_ref()))?;
    let selected = reader.selections(location, sources)?;
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
                        "cannot extract over the source ZIP archive",
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
    reader: &mut ZipReader<'_>,
    location: &ZipLocation,
    source: &Path,
    members: &[(PathBuf, ZipMember)],
    destination: &Path,
    progress: &mut ProgressTracker<'_>,
) -> io::Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let stage = destination_staging_path(destination)?;
    let root = &reader.members[source];
    if root.is_dir {
        fs::create_dir(&stage)?;
    } else {
        // Reserve this path before cleanup can ever own it.
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stage)?;
    }
    let result = (|| {
        for (member_path, member) in members {
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
                    fs::create_dir_all(&path)?;
                }
            } else {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut options = fs::OpenOptions::new();
                options.write(true);
                if member_path != source {
                    options.create_new(true);
                }
                let mut output = options.open(&path)?;
                let mut input = normalize_zip_io_result(
                    reader
                        .archive
                        .by_index(member.index.expect("file has an archive index"))
                        .map_err(zip_error),
                    Some(progress.cancel_flag.as_ref()),
                )?;
                let mut buffer = [0_u8; COPY_BUFFER_SIZE];
                let mut written = 0_u64;
                loop {
                    progress.ensure_not_canceled()?;
                    let length = normalize_zip_io_result(
                        input.read(&mut buffer),
                        Some(progress.cancel_flag.as_ref()),
                    )?;
                    if length == 0 {
                        break;
                    }
                    written = written
                        .checked_add(length as u64)
                        .ok_or_else(|| io::Error::other("ZIP size overflow"))?;
                    if written > member.size {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "ZIP member exceeds its declared size",
                        ));
                    }
                    output.write_all(&buffer[..length])?;
                    progress.advance_bytes(length as u64);
                }
                if written != member.size {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "ZIP member is shorter than its declared size",
                    ));
                }
                output.flush()?;
            }
            progress.complete_item(&display_path);
        }
        // Apply directory permissions after their children have been written.
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
