use super::*;
use crate::MetadataChange;

pub(super) fn change_metadata(
    paths: &[PathBuf],
    change: &MetadataChange,
    progress: &mut ProgressTracker<'_>,
) -> io::Result<()> {
    #[cfg(unix)]
    {
        use nix::fcntl::AtFlags;
        use nix::sys::stat::{FchmodatFlags, Mode, fchmodat};
        use nix::unistd::{Group, User, fchownat};
        use std::os::unix::fs::MetadataExt;

        let invalid =
            |message: &str| io::Error::new(io::ErrorKind::InvalidInput, message.to_string());
        // Resolve account names before modifying the first entry. NSS lookups
        // can block, which is why they run on this filesystem worker.
        let owner = change
            .owner
            .as_deref()
            .map(|name| {
                if name.bytes().all(|b| b.is_ascii_digit()) {
                    name.parse::<u32>()
                        .ok()
                        .filter(|v| *v != u32::MAX)
                        .map(Uid::from_raw)
                        .ok_or_else(|| invalid("Invalid owner ID"))
                } else {
                    User::from_name(name)
                        .map_err(io::Error::from)?
                        .map(|user| user.uid)
                        .ok_or_else(|| invalid(&format!("Unknown owner: {name}")))
                }
            })
            .transpose()?;
        let group = change
            .group
            .as_deref()
            .map(|name| {
                if name.bytes().all(|b| b.is_ascii_digit()) {
                    name.parse::<u32>()
                        .ok()
                        .filter(|v| *v != u32::MAX)
                        .map(Gid::from_raw)
                        .ok_or_else(|| invalid("Invalid group ID"))
                } else {
                    Group::from_name(name)
                        .map_err(io::Error::from)?
                        .map(|group| group.gid)
                        .ok_or_else(|| invalid(&format!("Unknown group: {name}")))
                }
            })
            .transpose()?;
        if change.permissions.is_none() && owner.is_none() && group.is_none() {
            return Err(invalid("No metadata changes specified"));
        }
        for path in paths {
            progress.ensure_not_canceled()?;
            let metadata = fs::symlink_metadata(path)?;
            if change.permissions.is_some() && metadata.file_type().is_symlink() {
                return Err(invalid(&format!(
                    "Chmod does not follow symlinks: {}",
                    path.display()
                )));
            }
        }
        for path in paths {
            progress.ensure_not_canceled()?;
            progress.set_current_path(path);
            let result = (|| -> io::Result<()> {
                let metadata = fs::symlink_metadata(path)?;
                if change.permissions.is_some() && metadata.file_type().is_symlink() {
                    return Err(invalid("Entry changed to a symlink"));
                }
                if owner.is_some() || group.is_some() {
                    fchownat(None, path, owner, group, AtFlags::AT_SYMLINK_NOFOLLOW)?;
                }
                if let Some(permissions) = &change.permissions {
                    let mode = permissions.apply(metadata.mode(), metadata.is_dir());
                    fchmodat(
                        None,
                        path,
                        Mode::from_bits_truncate(mode as _),
                        FchmodatFlags::NoFollowSymlink,
                    )?;
                }
                Ok(())
            })();
            result.map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "{}: {error} ({} of {} entries completed)",
                        path.display(),
                        progress.progress.items_done,
                        paths.len()
                    ),
                )
            })?;
            progress.complete_item(path);
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (paths, change, progress);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Permissions and ownership operations require Unix",
        ))
    }
}
