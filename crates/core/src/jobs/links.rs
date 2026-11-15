use super::*;
use crate::LinkKind;

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

/// Resolve parent aliases while preserving the final component (which may
/// itself be a symlink, including a dangling one).
#[cfg(unix)]
fn absolute_entry(path: &Path) -> io::Result<PathBuf> {
    let Some(name) = path.file_name() else {
        return fs::canonicalize(path);
    };
    Ok(fs::canonicalize(path.parent().unwrap_or(Path::new(".")))?.join(name))
}

#[cfg(unix)]
fn relative_target(source: &Path, destination_parent: &Path) -> io::Result<PathBuf> {
    let source = absolute_entry(source)?;
    let parent = fs::canonicalize(destination_parent)?;
    let left: Vec<_> = source.components().collect();
    let right: Vec<_> = parent.components().collect();
    let common = left.iter().zip(&right).take_while(|(a, b)| a == b).count();
    let mut result = PathBuf::new();
    for _ in common..right.len() {
        result.push("..");
    }
    for component in &left[common..] {
        result.push(component.as_os_str());
    }
    if result.as_os_str().is_empty() {
        result.push(".");
    }
    Ok(result)
}

pub(super) fn create_links(
    kind: LinkKind,
    sources: &[PathBuf],
    destination: &Path,
    progress: &mut ProgressTracker<'_>,
) -> io::Result<()> {
    #[cfg(unix)]
    {
        let is_directory = destination.is_dir();
        if sources.len() > 1 && !is_directory {
            return Err(invalid(
                "Tagged links require an existing destination directory",
            ));
        }
        let mut plan = Vec::with_capacity(sources.len());
        let mut destinations = HashSet::new();
        for source in sources {
            progress.ensure_not_canceled()?;
            let metadata = fs::symlink_metadata(source)?;
            if kind == LinkKind::Hard && !metadata.file_type().is_file() {
                return Err(invalid(format!(
                    "Hard links require regular files: {}",
                    source.display()
                )));
            }
            let destination = if is_directory {
                destination_path(source, destination)?
            } else {
                destination.to_path_buf()
            };
            let identity = absolute_entry(&destination)?;
            if !destinations.insert(identity) {
                return Err(invalid(
                    "Two selected entries have the same destination name",
                ));
            }
            if path_entry_exists(&destination)? {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("Destination exists: {}", destination.display()),
                ));
            }
            let target = match kind {
                LinkKind::Hard => source.to_path_buf(),
                LinkKind::Symbolic => absolute_entry(source)?,
                LinkKind::RelativeSymbolic => {
                    relative_target(source, destination.parent().unwrap_or(Path::new(".")))?
                }
            };
            plan.push((target, destination));
        }
        for (target, destination) in plan {
            progress.ensure_not_canceled()?;
            progress.set_current_path(&destination);
            let result = match kind {
                LinkKind::Hard => nix::unistd::linkat(
                    None,
                    &target,
                    None,
                    &destination,
                    nix::fcntl::AtFlags::empty(),
                )
                .map_err(io::Error::from),
                _ => std::os::unix::fs::symlink(target, &destination),
            };
            result.map_err(|error| {
                io::Error::new(error.kind(), format!("{}: {error}", destination.display()))
            })?;
            progress.complete_item(&destination);
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (kind, sources, destination, progress);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Link operations require Unix",
        ))
    }
}

pub(super) fn edit_symlinks(
    paths: &[PathBuf],
    target: &Path,
    progress: &mut ProgressTracker<'_>,
) -> io::Result<()> {
    if target.as_os_str().is_empty() {
        return Err(invalid("A symlink target cannot be empty"));
    }
    for path in paths {
        progress.ensure_not_canceled()?;
        if !fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Err(invalid(format!("Not a symlink: {}", path.display())));
        }
    }
    for path in paths {
        progress.ensure_not_canceled()?;
        progress.set_current_path(path);
        replace_symlink(path, target, || progress.ensure_not_canceled())?;
        progress.complete_item(path);
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn replace_symlink(
    path: &Path,
    target: &Path,
    before_exchange: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    use std::os::unix::fs::MetadataExt;
    let before = fs::symlink_metadata(path)?;
    if !before.file_type().is_symlink() {
        return Err(invalid(format!("Not a symlink: {}", path.display())));
    }
    let stage = tempfile::Builder::new()
        .prefix(".rc-link-")
        .tempdir_in(path.parent().unwrap_or(Path::new(".")))?;
    let staged = stage.path().join("entry");
    std::os::unix::fs::symlink(target, &staged)?;
    before_exchange()?;
    // Exchange keeps the displaced entry recoverable, even if another process
    // replaces the link with a regular file just before this syscall.
    renameat_with(CWD, &staged, CWD, path, RenameFlags::EXCHANGE)?;
    let displaced = fs::symlink_metadata(&staged);
    if displaced.as_ref().is_ok_and(|m| {
        m.file_type().is_symlink() && m.dev() == before.dev() && m.ino() == before.ino()
    }) {
        return Ok(());
    }
    // Roll back by exchanging again. Keep the private directory even if another
    // concurrent replacement was displaced during rollback: never delete it.
    let rollback = renameat_with(CWD, &staged, CWD, path, RenameFlags::EXCHANGE);
    let recovery = stage.keep();
    Err(io::Error::other(format!(
        "{} changed during symlink editing; rollback {}; preserved entry: {}",
        path.display(),
        if rollback.is_ok() {
            "completed"
        } else {
            "failed"
        },
        recovery.join("entry").display()
    )))
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
fn replace_symlink(
    _path: &Path,
    _target: &Path,
    _before_exchange: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Atomic symlink editing requires Linux or macOS",
    ))
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn edit_symlink_handles_cancellation_and_concurrent_replacement_without_data_loss() {
        let root = tempfile::tempdir().unwrap();
        let link = root.path().join("link");
        symlink("old-target", &link).unwrap();
        let error = replace_symlink(&link, Path::new("new-target"), || {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                JOB_CANCELED_MESSAGE,
            ))
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(fs::read_link(&link).unwrap(), PathBuf::from("old-target"));
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        let error = replace_symlink(&link, Path::new("new-target"), || {
            fs::remove_file(&link)?;
            fs::write(&link, "concurrent file")
        })
        .unwrap_err();
        assert!(error.to_string().contains("rollback completed"));
        assert_eq!(fs::read(&link).unwrap(), b"concurrent file");
        let backup = fs::read_dir(root.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".rc-link-")
            })
            .unwrap();
        assert_eq!(
            fs::read_link(backup.join("entry")).unwrap(),
            PathBuf::from("new-target")
        );
    }
}
