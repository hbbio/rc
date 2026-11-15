use super::VfsPath;
use crate::{FileEntry, SelectionSizeReport, SortMode};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub(crate) struct Scan {
    pub entries: BTreeMap<PathBuf, FileEntry>,
    pub unreadable: u64,
}

pub(crate) fn scan(
    location: &VfsPath,
    paths: &[PathBuf],
    cancel: &AtomicBool,
    strict: bool,
) -> io::Result<Scan> {
    let mut result = Scan {
        entries: BTreeMap::new(),
        unreadable: 0,
    };
    let mut pending = Vec::new();
    let mut bytes = 0_usize;
    let mut charge = |path: &Path| -> io::Result<()> {
        bytes = bytes.saturating_add((path.as_os_str().len() + 128).saturating_mul(8));
        if bytes > 64 * 1024 * 1024 {
            return Err(io::Error::other("VFS traversal metadata exceeds 64 MiB"));
        }
        Ok(())
    };
    for path in paths {
        charge(path)?;
        pending.push((location.resolve_identity(path.clone())?, 0_usize));
    }
    while let Some((path, depth)) = pending.pop() {
        crate::archive::check_canceled(Some(cancel))?;
        let identity = path.identity();
        if result.entries.contains_key(&identity) {
            continue;
        }
        if depth > 256 || result.entries.len() >= 100_000 {
            return Err(io::Error::other(
                "VFS traversal exceeds its depth or entry limit",
            ));
        }
        let entry = match path.metadata(Some(cancel)) {
            Ok(entry) => entry,
            Err(error) if error.kind() == io::ErrorKind::Interrupted || strict => {
                return Err(error);
            }
            Err(_) => {
                result.unreadable += 1;
                continue;
            }
        };
        if entry.is_dir() && entry.metadata.symlink_target.is_none() {
            match path.read_directory(SortMode::default(), true, Some(cancel)) {
                Ok(entries) => {
                    for child in entries.into_iter().filter(|entry| !entry.is_parent()) {
                        charge(&child.path)?;
                        pending.push((location.resolve_identity(child.path)?, depth + 1));
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted || strict => {
                    return Err(error);
                }
                Err(_) => result.unreadable += 1,
            }
        }
        result.entries.insert(identity, entry);
    }
    Ok(result)
}

pub fn measure(
    location: &VfsPath,
    paths: &[PathBuf],
    cancel: &AtomicBool,
) -> io::Result<SelectionSizeReport> {
    let scan = scan(location, paths, cancel, false)?;
    Ok(SelectionSizeReport {
        apparent_bytes: scan
            .entries
            .values()
            .filter(|entry| !entry.is_dir())
            .fold(0, |sum, entry| sum.saturating_add(entry.size)),
        unreadable_entries: scan.unreadable,
    })
}
