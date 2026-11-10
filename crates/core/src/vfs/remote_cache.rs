use crate::FileEntry;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const LIMIT: usize = 16 * 1024 * 1024;
struct Directory {
    path: PathBuf,
    entries: Arc<[FileEntry]>,
    indexes: HashMap<PathBuf, usize>,
    created: Instant,
    bytes: usize,
}

#[derive(Default)]
struct State {
    directories: VecDeque<Directory>,
    generation: u64,
}
#[derive(Default)]
pub(super) struct MetadataCache(Mutex<State>);
impl MetadataCache {
    pub fn clear(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.generation = state.generation.wrapping_add(1);
            state.directories.clear();
        }
    }
    pub fn generation(&self) -> u64 {
        self.0.lock().map_or(0, |state| state.generation)
    }
    pub fn directory(&self, path: &Path) -> Option<Arc<[FileEntry]>> {
        let entries = self.0.lock().ok()?;
        entries
            .directories
            .iter()
            .find(|entry| entry.path == path && entry.created.elapsed() < Duration::from_secs(30))
            .map(|entry| Arc::clone(&entry.entries))
    }
    pub fn metadata(&self, path: &Path) -> Option<FileEntry> {
        let parent = path.parent()?;
        let entries = self.0.lock().ok()?;
        let directory = entries.directories.iter().find(|entry| {
            entry.path == parent && entry.created.elapsed() < Duration::from_secs(30)
        })?;
        directory
            .indexes
            .get(path)
            .map(|index| directory.entries[*index].clone())
    }
    pub fn insert(&self, path: &Path, entries: &[FileEntry], generation: u64) {
        let bytes = entries.iter().fold(path.as_os_str().len(), |sum, entry| {
            sum.saturating_add(
                (entry.path.as_os_str().len() + entry.name.len() + 128).saturating_mul(4),
            )
        });
        if bytes > LIMIT {
            return;
        }
        let Ok(mut state) = self.0.lock() else {
            return;
        };
        if state.generation != generation {
            return;
        }
        let directories = &mut state.directories;
        directories.retain(|entry| entry.path != path);
        while directories.len() >= 16
            || directories
                .iter()
                .map(|entry| entry.bytes)
                .sum::<usize>()
                .saturating_add(bytes)
                > LIMIT
        {
            directories.pop_front();
        }
        directories.push_back(Directory {
            path: path.into(),
            entries: Arc::from(entries),
            indexes: entries
                .iter()
                .enumerate()
                .map(|(index, entry)| (entry.path.clone(), index))
                .collect(),
            created: Instant::now(),
            bytes,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalidation_prevents_late_publication() {
        let cache = MetadataCache::default();
        let before = cache.generation();
        cache.clear();
        cache.insert(Path::new(""), &[], before);
        assert!(cache.directory(Path::new("")).is_none());
        cache.insert(Path::new(""), &[], cache.generation());
        assert!(cache.directory(Path::new("")).is_some());
    }
}
