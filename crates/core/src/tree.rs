use std::collections::BinaryHeap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

use crate::{JOB_CANCELED_MESSAGE, JobId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeEntry {
    pub path: PathBuf,
    pub depth: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeScanIssue {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TreeScanSummary {
    pub depth_limit_reached: bool,
    pub entry_limit_reached: bool,
    pub skipped_items: usize,
    pub first_issue: Option<TreeScanIssue>,
}

impl TreeScanSummary {
    pub fn is_truncated(&self) -> bool {
        self.depth_limit_reached || self.entry_limit_reached
    }

    fn record_issue(&mut self, path: PathBuf, error: &io::Error) {
        self.skipped_items = self.skipped_items.saturating_add(1);
        if self.first_issue.is_none() {
            self.first_issue = Some(TreeScanIssue {
                path,
                message: error.to_string(),
            });
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeBuildResult {
    pub entries: Vec<TreeEntry>,
    pub summary: TreeScanSummary,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TreeLoadState {
    Loading,
    Ready(TreeScanSummary),
    Canceled,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct TreeState {
    pub job_id: JobId,
    pub root: PathBuf,
    pub entries: Vec<TreeEntry>,
    pub cursor: usize,
    pub load_state: TreeLoadState,
}

impl TreeState {
    pub(crate) fn loading(job_id: JobId, root: PathBuf) -> Self {
        let entries = vec![TreeEntry {
            path: root.clone(),
            depth: 0,
        }];
        Self {
            job_id,
            root,
            entries,
            cursor: 0,
            load_state: TreeLoadState::Loading,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.load_state, TreeLoadState::Loading)
    }

    pub(crate) fn apply_build_result(&mut self, result: TreeBuildResult) {
        let selected_path = self.selected_entry().map(|entry| entry.path.clone());
        self.entries = result.entries;
        self.cursor = selected_path
            .and_then(|path| self.entries.iter().position(|entry| entry.path == path))
            .unwrap_or(0);
        self.load_state = TreeLoadState::Ready(result.summary);
    }

    pub(crate) fn mark_canceled(&mut self) {
        if self.is_loading() {
            self.load_state = TreeLoadState::Canceled;
        }
    }

    pub(crate) fn mark_failed(&mut self, message: String) {
        if self.is_loading() {
            self.load_state = TreeLoadState::Failed(message);
        }
    }

    pub(crate) fn move_cursor(&mut self, delta: isize) {
        if self.entries.is_empty() {
            self.cursor = 0;
            return;
        }

        let last = self.entries.len() - 1;
        let next = if delta.is_negative() {
            self.cursor.saturating_sub(delta.unsigned_abs())
        } else {
            self.cursor.saturating_add(delta as usize).min(last)
        };
        self.cursor = next;
    }

    pub(crate) fn move_page(&mut self, pages: isize, page_step: usize) {
        self.move_cursor(pages.saturating_mul(page_step as isize));
    }

    pub(crate) fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub(crate) fn move_end(&mut self) {
        self.cursor = self.entries.len().saturating_sub(1);
    }

    pub(crate) fn selected_entry(&self) -> Option<&TreeEntry> {
        self.entries.get(self.cursor)
    }
}

pub(crate) fn build_tree_entries(
    root: &Path,
    max_depth: usize,
    max_entries: usize,
    cancel_flag: Option<&AtomicBool>,
) -> io::Result<TreeBuildResult> {
    build_tree_entries_with_cancel_check(root, max_depth, max_entries, || {
        cancel_flag.is_some_and(|flag| flag.load(AtomicOrdering::Relaxed))
    })
}

fn build_tree_entries_with_cancel_check(
    root: &Path,
    max_depth: usize,
    max_entries: usize,
    mut is_canceled: impl FnMut() -> bool,
) -> io::Result<TreeBuildResult> {
    if max_entries == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "tree entry limit must be greater than zero",
        ));
    }
    ensure_not_canceled(&mut is_canceled)?;

    let root_metadata = fs::metadata(root).map_err(|error| contextual_io_error(root, error))?;
    if !root_metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("tree root is not a directory: {}", root.to_string_lossy()),
        ));
    }

    let mut summary = TreeScanSummary::default();
    let mut entries = Vec::with_capacity(max_entries.min(256));
    let mut stack = vec![(root.to_path_buf(), 0_usize)];

    while let Some((directory, depth)) = stack.pop() {
        ensure_not_canceled(&mut is_canceled)?;
        if entries.len() >= max_entries {
            summary.entry_limit_reached = true;
            break;
        }

        entries.push(TreeEntry {
            path: directory.clone(),
            depth,
        });

        let child_limit = if depth >= max_depth {
            0
        } else {
            max_entries.saturating_sub(entries.len())
        };
        let children = match collect_child_directories(
            &directory,
            child_limit,
            &mut summary,
            &mut is_canceled,
        ) {
            Ok(children) => children,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
            Err(error) if depth == 0 => return Err(contextual_io_error(&directory, error)),
            Err(error) => {
                summary.record_issue(directory, &error);
                continue;
            }
        };

        if depth >= max_depth {
            summary.depth_limit_reached |= children.omitted;
            continue;
        }
        summary.entry_limit_reached |= children.omitted;

        for child in children.paths.into_iter().rev() {
            stack.push((child, depth.saturating_add(1)));
        }
    }

    Ok(TreeBuildResult { entries, summary })
}

struct ChildDirectories {
    paths: Vec<PathBuf>,
    omitted: bool,
}

fn collect_child_directories(
    directory: &Path,
    limit: usize,
    summary: &mut TreeScanSummary,
    is_canceled: &mut impl FnMut() -> bool,
) -> io::Result<ChildDirectories> {
    ensure_not_canceled(is_canceled)?;
    let read_dir = fs::read_dir(directory)?;
    let mut smallest = BinaryHeap::<(TreeSortKey, PathBuf)>::new();
    let mut omitted = false;

    for entry_result in read_dir {
        ensure_not_canceled(is_canceled)?;
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(error) => {
                summary.record_issue(directory.to_path_buf(), &error);
                continue;
            }
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                summary.record_issue(entry.path(), &error);
                continue;
            }
        };
        if !file_type.is_dir() {
            continue;
        }

        if limit == 0 {
            return Ok(ChildDirectories {
                paths: Vec::new(),
                omitted: true,
            });
        }

        let path = entry.path();
        smallest.push((tree_sort_key(&path), path));
        if smallest.len() > limit {
            smallest.pop();
            omitted = true;
        }
    }

    ensure_not_canceled(is_canceled)?;
    let mut paths: Vec<PathBuf> = smallest.into_iter().map(|(_key, path)| path).collect();
    paths.sort_by_cached_key(|path| tree_sort_key(path));
    ensure_not_canceled(is_canceled)?;
    Ok(ChildDirectories { paths, omitted })
}

type TreeSortKey = (String, OsString);

fn tree_sort_key(path: &Path) -> TreeSortKey {
    let name = path.file_name().unwrap_or(path.as_os_str());
    (name.to_string_lossy().to_lowercase(), name.to_os_string())
}

fn ensure_not_canceled(is_canceled: &mut impl FnMut() -> bool) -> io::Result<()> {
    if is_canceled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            JOB_CANCELED_MESSAGE,
        ));
    }
    Ok(())
}

fn contextual_io_error(path: &Path, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("cannot scan {}: {error}", path.to_string_lossy()),
    )
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-tree-{label}-{stamp}"));
        fs::create_dir_all(&root).expect("tree fixture should be creatable");
        root
    }

    #[test]
    fn tree_scan_is_sorted_preorder_with_contiguous_descendants() {
        let root = temp_root("preorder");
        fs::create_dir_all(root.join("beta").join("child-b"))
            .expect("beta fixture should be creatable");
        fs::create_dir_all(root.join("alpha").join("child-a"))
            .expect("alpha fixture should be creatable");

        let result = build_tree_entries(&root, 4, 64, None).expect("tree should scan");
        let relative: Vec<(PathBuf, usize)> = result
            .entries
            .iter()
            .map(|entry| {
                (
                    entry
                        .path
                        .strip_prefix(&root)
                        .expect("entry should be under root")
                        .to_path_buf(),
                    entry.depth,
                )
            })
            .collect();
        assert_eq!(
            relative,
            vec![
                (PathBuf::new(), 0),
                (PathBuf::from("alpha"), 1),
                (PathBuf::from("alpha/child-a"), 2),
                (PathBuf::from("beta"), 1),
                (PathBuf::from("beta/child-b"), 2),
            ]
        );
        assert_eq!(result.summary, TreeScanSummary::default());

        fs::remove_dir_all(root).expect("tree fixture should be removable");
    }

    #[test]
    fn tree_scan_reports_depth_and_entry_truncation() {
        let root = temp_root("limits");
        fs::create_dir_all(root.join("a").join("deep")).expect("deep fixture should be creatable");
        fs::create_dir_all(root.join("b")).expect("sibling fixture should be creatable");

        let depth_limited =
            build_tree_entries(&root, 1, 64, None).expect("depth-limited tree should scan");
        assert!(depth_limited.summary.depth_limit_reached);
        assert!(!depth_limited.summary.entry_limit_reached);

        let entry_limited =
            build_tree_entries(&root, 4, 2, None).expect("entry-limited tree should scan");
        assert!(entry_limited.summary.entry_limit_reached);
        assert_eq!(entry_limited.entries.len(), 2);
        assert_eq!(entry_limited.entries[1].path, root.join("a"));

        fs::remove_dir_all(root).expect("tree fixture should be removable");
    }

    #[test]
    fn tree_scan_checks_cancellation_during_traversal() {
        let root = temp_root("cancel");
        for index in 0..8 {
            fs::create_dir_all(root.join(format!("dir-{index}")))
                .expect("cancellation fixture should be creatable");
        }

        let mut checks = 0_usize;
        let error = build_tree_entries_with_cancel_check(&root, 4, 64, || {
            checks = checks.saturating_add(1);
            checks > 4
        })
        .expect_err("tree scan should observe cancellation");
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(error.to_string(), JOB_CANCELED_MESSAGE);
        assert!(
            checks > 4,
            "scan should perform repeated cancellation checks"
        );

        fs::remove_dir_all(root).expect("tree fixture should be removable");
    }

    #[test]
    fn tree_scan_rejects_invalid_roots_and_limits() {
        let root = temp_root("invalid");
        let file = root.join("file.txt");
        fs::write(&file, "file").expect("file fixture should be writable");

        let zero_limit =
            build_tree_entries(&root, 4, 0, None).expect_err("zero entry limit should be rejected");
        assert_eq!(zero_limit.kind(), io::ErrorKind::InvalidInput);

        let file_root =
            build_tree_entries(&file, 4, 64, None).expect_err("file root should be rejected");
        assert_eq!(file_root.kind(), io::ErrorKind::InvalidInput);

        fs::remove_dir_all(root).expect("tree fixture should be removable");
    }
}
