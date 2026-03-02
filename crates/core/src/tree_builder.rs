use std::fs;
use std::path::Path;

use crate::TreeEntry;

pub(crate) fn build_tree_entries(
    root: &Path,
    max_depth: usize,
    max_entries: usize,
) -> Vec<TreeEntry> {
    if max_entries == 0 {
        return Vec::new();
    }

    let root = root.to_path_buf();
    let mut entries = vec![TreeEntry {
        path: root.clone(),
        depth: 0,
    }];

    let mut stack = vec![(root, 0usize)];
    while let Some((directory, depth)) = stack.pop() {
        if depth >= max_depth || entries.len() >= max_entries {
            continue;
        }

        let read_dir = match fs::read_dir(&directory) {
            Ok(read_dir) => read_dir,
            Err(_) => continue,
        };
        let mut child_dirs = Vec::new();

        for entry in read_dir.flatten() {
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                child_dirs.push(entry.path());
            }
        }

        child_dirs.sort_by_key(|left| path_sort_key(left));

        for child_dir in child_dirs.into_iter().rev() {
            if entries.len() >= max_entries {
                return entries;
            }

            entries.push(TreeEntry {
                path: child_dir.clone(),
                depth: depth + 1,
            });
            stack.push((child_dir, depth + 1));
        }
    }

    entries
}

fn path_sort_key(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| path.to_string_lossy().to_lowercase())
}
