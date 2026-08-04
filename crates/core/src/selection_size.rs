use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const SELECTION_SIZE_CANCELED_MESSAGE: &str = "selection size calculation canceled";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SelectionSizeReport {
    pub apparent_bytes: u64,
    pub unreadable_entries: u64,
}

/// Measures the apparent size of selected filesystem entries without following symlinks.
///
/// Directories contribute the sizes of their descendants, not the platform-specific size of
/// the directory inode itself. Unreadable or concurrently removed entries are counted in the
/// report so callers can present the successfully measured bytes as a partial result.
pub fn measure_selection_size(
    paths: &[PathBuf],
    cancel_flag: &AtomicBool,
) -> io::Result<SelectionSizeReport> {
    ensure_not_canceled(cancel_flag)?;

    let selected_paths = paths.iter().cloned().collect::<HashSet<_>>();
    let mut pending = selected_paths.iter().cloned().collect::<Vec<_>>();
    pending.sort_unstable_by(|left, right| right.cmp(left));

    let mut measured_selected_files = HashSet::with_capacity(selected_paths.len());
    let mut visited_directories = HashSet::new();
    let mut report = SelectionSizeReport::default();

    while let Some(path) = pending.pop() {
        ensure_not_canceled(cancel_flag)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                report.unreadable_entries = report.unreadable_entries.saturating_add(1);
                continue;
            }
        };

        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            let identity = directory_identity(&path, &metadata);
            if !visited_directories.insert(identity) {
                continue;
            }

            let entries = match fs::read_dir(&path) {
                Ok(entries) => entries,
                Err(_) => {
                    report.unreadable_entries = report.unreadable_entries.saturating_add(1);
                    continue;
                }
            };
            for entry in entries {
                ensure_not_canceled(cancel_flag)?;
                let path = match entry {
                    Ok(entry) => entry.path(),
                    Err(_) => {
                        report.unreadable_entries = report.unreadable_entries.saturating_add(1);
                        continue;
                    }
                };
                let metadata = match fs::symlink_metadata(&path) {
                    Ok(metadata) => metadata,
                    Err(_) => {
                        report.unreadable_entries = report.unreadable_entries.saturating_add(1);
                        continue;
                    }
                };
                if metadata.is_dir() && !metadata.file_type().is_symlink() {
                    pending.push(path);
                } else {
                    measure_non_directory(
                        path,
                        &metadata,
                        &selected_paths,
                        &mut measured_selected_files,
                        &mut report,
                    );
                }
            }
            continue;
        }

        measure_non_directory(
            path,
            &metadata,
            &selected_paths,
            &mut measured_selected_files,
            &mut report,
        );
    }

    ensure_not_canceled(cancel_flag)?;
    Ok(report)
}

fn measure_non_directory(
    path: PathBuf,
    metadata: &fs::Metadata,
    selected_paths: &HashSet<PathBuf>,
    measured_selected_files: &mut HashSet<PathBuf>,
    report: &mut SelectionSizeReport,
) {
    // A selected file can also be discovered while walking a selected ancestor. Count the
    // directory entry once while retaining only the selected roots, not every descendant path.
    if selected_paths.contains(&path) && !measured_selected_files.insert(path) {
        return;
    }
    report.apparent_bytes = report.apparent_bytes.saturating_add(metadata.len());
}

fn ensure_not_canceled(cancel_flag: &AtomicBool) -> io::Result<()> {
    if cancel_flag.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            SELECTION_SIZE_CANCELED_MESSAGE,
        ));
    }
    Ok(())
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct DirectoryIdentity {
    device: u64,
    inode: u64,
}

#[cfg(unix)]
fn directory_identity(_path: &Path, metadata: &fs::Metadata) -> DirectoryIdentity {
    use std::os::unix::fs::MetadataExt as _;

    DirectoryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

#[cfg(not(unix))]
type DirectoryIdentity = PathBuf;

#[cfg(not(unix))]
fn directory_identity(path: &Path, _metadata: &fs::Metadata) -> DirectoryIdentity {
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-selection-size-{label}-{stamp}"));
        fs::create_dir_all(&root).expect("temporary root should be creatable");
        root
    }

    #[test]
    fn directory_size_is_the_sum_of_nested_file_bytes() {
        let root = temp_root("nested");
        let selected = root.join("selected");
        fs::create_dir_all(selected.join("nested")).expect("nested directory should be creatable");
        fs::write(selected.join("one"), vec![0_u8; 17]).expect("first file should be writable");
        fs::write(selected.join("nested/two"), vec![0_u8; 29])
            .expect("nested file should be writable");

        let report = measure_selection_size(&[selected], &AtomicBool::new(false))
            .expect("selection size should be measurable");

        assert_eq!(report.apparent_bytes, 46);
        assert_eq!(report.unreadable_entries, 0);
        fs::remove_dir_all(root).expect("temporary root should be removable");
    }

    #[test]
    fn overlapping_selected_trees_are_measured_once() {
        let root = temp_root("overlap");
        let selected = root.join("selected");
        let nested = selected.join("nested");
        let file = nested.join("payload");
        fs::create_dir_all(&nested).expect("nested directory should be creatable");
        fs::write(&file, vec![0_u8; 41]).expect("file should be writable");

        let report = measure_selection_size(&[selected, nested, file], &AtomicBool::new(false))
            .expect("overlapping selection should be measurable");

        assert_eq!(report.apparent_bytes, 41);
        assert_eq!(report.unreadable_entries, 0);
        fs::remove_dir_all(root).expect("temporary root should be removable");
    }

    #[test]
    fn missing_entries_produce_a_partial_report() {
        let root = temp_root("missing");
        let readable = root.join("readable");
        fs::write(&readable, vec![0_u8; 7]).expect("file should be writable");

        let report =
            measure_selection_size(&[readable, root.join("missing")], &AtomicBool::new(false))
                .expect("missing entries should not discard readable totals");

        assert_eq!(report.apparent_bytes, 7);
        assert_eq!(report.unreadable_entries, 1);
        fs::remove_dir_all(root).expect("temporary root should be removable");
    }

    #[test]
    fn cancellation_is_reported_before_scanning() {
        let cancel_flag = AtomicBool::new(true);
        let error = measure_selection_size(&[], &cancel_flag)
            .expect_err("canceled scan should fail immediately");

        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(error.to_string(), SELECTION_SIZE_CANCELED_MESSAGE);
    }

    #[cfg(unix)]
    #[test]
    fn directory_symlinks_are_not_followed() {
        use std::os::unix::fs::symlink;

        let root = temp_root("symlink");
        let target = root.join("target");
        fs::create_dir_all(&target).expect("target directory should be creatable");
        fs::write(target.join("payload"), vec![0_u8; 128]).expect("file should be writable");
        let link = root.join("link");
        symlink(&target, &link).expect("directory symlink should be creatable");
        let link_bytes = fs::symlink_metadata(&link)
            .expect("symlink metadata should be readable")
            .len();

        let report = measure_selection_size(&[link], &AtomicBool::new(false))
            .expect("symlink should be measurable");

        assert_eq!(report.apparent_bytes, link_bytes);
        assert_eq!(report.unreadable_entries, 0);
        fs::remove_dir_all(root).expect("temporary root should be removable");
    }
}
