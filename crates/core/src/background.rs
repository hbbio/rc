use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crate::{
    ActivePanel, DiskUsageSummary, FileEntry, FindResultEntry, JobId, PanelFilter,
    PanelListingSource, SortMode, TreeBuildResult, ViewerState, build_tree_entries,
    canonical_panel_paths, ensure_panel_refresh_not_canceled, read_entries_with_visibility_cancel,
    read_panelized_entries_with_cancel, read_panelized_paths, sort_file_entries,
    stream_panelized_entries_with_cancel, stream_panelized_paths_with_cancel,
};

#[cfg(unix)]
use nix::sys::statvfs::statvfs;

const PANEL_EVENT_CHUNK_SIZE: usize = 96;
const PANEL_EVENT_FLUSH_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug)]
pub struct PanelRefreshStreamRequest {
    pub panel: ActivePanel,
    pub cwd: PathBuf,
    pub source: PanelListingSource,
    pub sort_mode: SortMode,
    pub filter: PanelFilter,
    pub show_hidden_files: bool,
    pub cached_panelized_entries: Option<Arc<[FileEntry]>>,
    pub home_directory: Option<PathBuf>,
    pub request_id: u64,
}

#[derive(Clone, Debug)]
pub struct PanelRefreshResult {
    pub entries: Vec<FileEntry>,
    pub panelized_entries: Option<Arc<[FileEntry]>>,
    pub canonical_cwd: Option<PathBuf>,
    pub canonical_home_directory: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PanelPathIdentity {
    pub canonical_cwd: Option<PathBuf>,
    pub canonical_home_directory: Option<PathBuf>,
}

impl PanelRefreshStreamRequest {
    pub fn canceled_event(&self) -> BackgroundEvent {
        BackgroundEvent::PanelRefreshed {
            panel: self.panel,
            cwd: self.cwd.clone(),
            source: self.source.clone(),
            sort_mode: self.sort_mode,
            filter: self.filter.clone(),
            request_id: self.request_id,
            disk_usage: None,
            result: Err(String::from(crate::PANEL_REFRESH_CANCELED_MESSAGE)),
        }
    }
}

#[derive(Clone, Debug)]
pub enum BackgroundEvent {
    PanelEntriesChunk {
        panel: ActivePanel,
        cwd: PathBuf,
        source: PanelListingSource,
        sort_mode: SortMode,
        filter: PanelFilter,
        request_id: u64,
        entries: Vec<FileEntry>,
    },
    PanelRefreshed {
        panel: ActivePanel,
        cwd: PathBuf,
        source: PanelListingSource,
        sort_mode: SortMode,
        filter: PanelFilter,
        request_id: u64,
        disk_usage: Option<DiskUsageSummary>,
        result: Result<PanelRefreshResult, String>,
    },
    PanelIdentityResolved {
        panel: ActivePanel,
        cwd: PathBuf,
        request_id: u64,
        result: Result<PanelPathIdentity, String>,
    },
    ViewerLoaded {
        path: PathBuf,
        result: Result<ViewerState, String>,
    },
    DesktopOpenFinished {
        path: PathBuf,
        result: Result<(), String>,
    },
    QuickViewLoaded {
        panel: ActivePanel,
        path: PathBuf,
        request_id: u64,
        result: Result<ViewerState, String>,
    },
    SelectionSizeMeasured {
        panel: ActivePanel,
        request_id: u64,
        report: crate::SelectionSizeReport,
    },
    QuickCdSearchUpdated {
        request_id: u64,
        snapshot: crate::QuickCdSearchSnapshot,
    },
    FindEntriesChunk {
        job_id: JobId,
        entries: Vec<FindResultEntry>,
    },
    FindCompleted {
        job_id: JobId,
        report: crate::FindSearchReport,
    },
    TreeReady {
        job_id: JobId,
        root: PathBuf,
        result: TreeBuildResult,
    },
}

pub fn refresh_panel_event(
    request: PanelRefreshStreamRequest,
    cancel_flag: &AtomicBool,
) -> BackgroundEvent {
    let result = stream_refresh_panel_entries(&request, cancel_flag, |_| true)
        .map_err(|error| error.to_string());
    let disk_usage = result
        .as_ref()
        .ok()
        .and_then(|_| read_disk_usage(request.cwd.as_path()));
    BackgroundEvent::PanelRefreshed {
        panel: request.panel,
        cwd: request.cwd,
        source: request.source,
        sort_mode: request.sort_mode,
        filter: request.filter,
        request_id: request.request_id,
        disk_usage,
        result,
    }
}

pub fn build_tree_ready_event(
    job_id: JobId,
    root: PathBuf,
    max_depth: usize,
    max_entries: usize,
    cancel_flag: &AtomicBool,
) -> io::Result<BackgroundEvent> {
    let result = build_tree_entries(&root, max_depth, max_entries, Some(cancel_flag))?;
    Ok(BackgroundEvent::TreeReady {
        job_id,
        root,
        result,
    })
}

pub fn refresh_panel_entries(
    cwd: &Path,
    source: &PanelListingSource,
    sort_mode: SortMode,
    show_hidden_files: bool,
    cancel_flag: &AtomicBool,
) -> io::Result<Vec<FileEntry>> {
    match source {
        PanelListingSource::Directory => read_entries_with_visibility_cancel(
            cwd,
            sort_mode,
            show_hidden_files,
            Some(cancel_flag),
        ),
        PanelListingSource::Panelize { command } => {
            read_panelized_entries_with_cancel(cwd, command, sort_mode, Some(cancel_flag))
        }
        PanelListingSource::FindResults {
            base_dir, paths, ..
        } => read_panelized_paths(base_dir, paths, sort_mode, Some(cancel_flag)),
    }
}

pub fn resolve_panel_path_identity(
    cwd: &Path,
    home_directory: Option<&Path>,
    cancel_flag: &AtomicBool,
) -> io::Result<PanelPathIdentity> {
    ensure_panel_refresh_not_canceled(Some(cancel_flag))?;
    let (canonical_cwd, canonical_home_directory) = canonical_panel_paths(cwd, home_directory);
    ensure_panel_refresh_not_canceled(Some(cancel_flag))?;
    Ok(PanelPathIdentity {
        canonical_cwd,
        canonical_home_directory,
    })
}

pub fn stream_refresh_panel_entries<F>(
    request: &PanelRefreshStreamRequest,
    cancel_flag: &AtomicBool,
    mut emit_chunk: F,
) -> io::Result<PanelRefreshResult>
where
    F: FnMut(BackgroundEvent) -> bool,
{
    let mut result = match &request.source {
        PanelListingSource::Directory => {
            stream_directory_entries(request, cancel_flag, &mut emit_chunk)
        }
        PanelListingSource::Panelize { .. } | PanelListingSource::FindResults { .. } => {
            stream_panelized_source_entries(request, cancel_flag, &mut emit_chunk)
        }
    }?;
    let identity =
        resolve_panel_path_identity(&request.cwd, request.home_directory.as_deref(), cancel_flag)?;
    result.canonical_cwd = identity.canonical_cwd;
    result.canonical_home_directory = identity.canonical_home_directory;
    Ok(result)
}

fn stream_panelized_source_entries<F>(
    request: &PanelRefreshStreamRequest,
    cancel_flag: &AtomicBool,
    emit_chunk: &mut F,
) -> io::Result<PanelRefreshResult>
where
    F: FnMut(BackgroundEvent) -> bool,
{
    let matcher = request
        .filter
        .compile()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    if let Some(cached_entries) = request.cached_panelized_entries.clone() {
        let mut entries = Vec::new();
        for entry in cached_entries.iter() {
            ensure_panel_refresh_not_canceled(Some(cancel_flag))?;
            if matcher.matches(entry) {
                entries.push(entry.clone());
            }
        }
        sort_file_entries(&mut entries, request.sort_mode);
        for chunk in entries.chunks(PANEL_EVENT_CHUNK_SIZE) {
            let mut chunk = chunk.to_vec();
            emit_panel_entries_chunk(request, &mut chunk, emit_chunk)?;
        }
        return Ok(PanelRefreshResult {
            entries,
            panelized_entries: Some(cached_entries),
            canonical_cwd: None,
            canonical_home_directory: None,
        });
    }

    let mut pending = Vec::with_capacity(1);
    let mut next_chunk_size = 1_usize;
    let mut last_emit = None::<Instant>;
    let mut emit_entry = |entry: &FileEntry| {
        if !matcher.matches(entry) {
            return Ok(());
        }
        pending.push(entry.clone());
        let flush_interval_elapsed =
            last_emit.is_some_and(|emitted_at| emitted_at.elapsed() >= PANEL_EVENT_FLUSH_INTERVAL);
        if pending.len() < next_chunk_size && !flush_interval_elapsed {
            return Ok(());
        }
        emit_panel_entries_chunk(request, &mut pending, emit_chunk)?;
        last_emit = Some(Instant::now());
        next_chunk_size = next_chunk_size
            .saturating_mul(2)
            .min(PANEL_EVENT_CHUNK_SIZE);
        pending.reserve(next_chunk_size);
        Ok(())
    };

    let discovered_entries = match &request.source {
        PanelListingSource::Panelize { command } => stream_panelized_entries_with_cancel(
            &request.cwd,
            command,
            Some(cancel_flag),
            &mut emit_entry,
        ),
        PanelListingSource::FindResults {
            base_dir, paths, ..
        } => {
            stream_panelized_paths_with_cancel(base_dir, paths, Some(cancel_flag), &mut emit_entry)
        }
        PanelListingSource::Directory => unreachable!("directory sources use directory streaming"),
    }?;
    emit_panel_entries_chunk(request, &mut pending, emit_chunk)?;
    let panelized_entries = Arc::<[FileEntry]>::from(discovered_entries);
    let mut visible_entries = panelized_entries
        .iter()
        .filter(|entry| matcher.matches(entry))
        .cloned()
        .collect::<Vec<_>>();
    sort_file_entries(&mut visible_entries, request.sort_mode);
    Ok(PanelRefreshResult {
        entries: visible_entries,
        panelized_entries: Some(panelized_entries),
        canonical_cwd: None,
        canonical_home_directory: None,
    })
}

fn emit_panel_entries_chunk<F>(
    request: &PanelRefreshStreamRequest,
    entries: &mut Vec<FileEntry>,
    emit_chunk: &mut F,
) -> io::Result<()>
where
    F: FnMut(BackgroundEvent) -> bool,
{
    if entries.is_empty() {
        return Ok(());
    }
    let delivered = emit_chunk(BackgroundEvent::PanelEntriesChunk {
        panel: request.panel,
        cwd: request.cwd.clone(),
        source: request.source.clone(),
        sort_mode: request.sort_mode,
        filter: request.filter.clone(),
        request_id: request.request_id,
        entries: std::mem::take(entries),
    });
    if !delivered {
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "background event channel disconnected",
        ));
    }
    Ok(())
}

fn stream_directory_entries<F>(
    request: &PanelRefreshStreamRequest,
    cancel_flag: &AtomicBool,
    emit_chunk: &mut F,
) -> io::Result<PanelRefreshResult>
where
    F: FnMut(BackgroundEvent) -> bool,
{
    let cwd = request.cwd.as_path();
    ensure_panel_refresh_not_canceled(Some(cancel_flag))?;
    let matcher = request
        .filter
        .compile()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let mut entries = Vec::new();
    let mut emitted = Vec::new();

    for entry_result in fs::read_dir(cwd)? {
        ensure_panel_refresh_not_canceled(Some(cancel_flag))?;
        let entry = entry_result?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !request.show_hidden_files && name.starts_with('.') {
            continue;
        }
        let file_type = entry.file_type()?;
        let metadata = fs::metadata(&path).ok().or_else(|| entry.metadata().ok());
        let is_dir = file_type.is_dir() || metadata.as_ref().is_some_and(std::fs::Metadata::is_dir);
        let panel_entry = if is_dir {
            FileEntry::directory_from_metadata(name, path, metadata.as_ref())
        } else {
            FileEntry::file_from_metadata(name, path, metadata.as_ref())
        };
        if !matcher.matches(&panel_entry) {
            continue;
        }
        entries.push(panel_entry.clone());
        emitted.push(panel_entry);

        if emitted.len() >= PANEL_EVENT_CHUNK_SIZE {
            let delivered = emit_chunk(BackgroundEvent::PanelEntriesChunk {
                panel: request.panel,
                cwd: request.cwd.clone(),
                source: request.source.clone(),
                sort_mode: request.sort_mode,
                filter: request.filter.clone(),
                request_id: request.request_id,
                entries: std::mem::take(&mut emitted),
            });
            if !delivered {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "background event channel disconnected",
                ));
            }
        }
    }

    if !emitted.is_empty() {
        let delivered = emit_chunk(BackgroundEvent::PanelEntriesChunk {
            panel: request.panel,
            cwd: request.cwd.clone(),
            source: request.source.clone(),
            sort_mode: request.sort_mode,
            filter: request.filter.clone(),
            request_id: request.request_id,
            entries: emitted,
        });
        if !delivered {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "background event channel disconnected",
            ));
        }
    }

    sort_file_entries(&mut entries, request.sort_mode);
    if let Some(parent) = cwd.parent() {
        entries.insert(0, FileEntry::parent(parent.to_path_buf()));
    }
    Ok(PanelRefreshResult {
        entries,
        panelized_entries: None,
        canonical_cwd: None,
        canonical_home_directory: None,
    })
}

pub fn read_disk_usage(path: &Path) -> Option<DiskUsageSummary> {
    disk_usage(path).map(|(free_bytes, total_bytes)| DiskUsageSummary {
        free_bytes,
        total_bytes,
    })
}

#[cfg(unix)]
fn disk_usage(path: &Path) -> Option<(u64, u64)> {
    let stats = statvfs(path).ok()?;
    let fragment_size = stats.fragment_size() as u64;
    if fragment_size == 0 {
        return None;
    }

    let total = bytes_from_blocks(stats.blocks() as u64, fragment_size);
    let free = bytes_from_blocks(stats.blocks_available() as u64, fragment_size);
    Some((free, total))
}

#[cfg(not(unix))]
fn disk_usage(_path: &Path) -> Option<(u64, u64)> {
    None
}

fn bytes_from_blocks(blocks: u64, block_size: u64) -> u64 {
    ((blocks as u128).saturating_mul(block_size as u128)).min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileEntryKind, FileEntryMetadata, FindNameMode};
    use std::env;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn cached_entry(name: &str, kind: FileEntryKind) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: PathBuf::from(name),
            kind,
            size: 0,
            modified: None,
            metadata: FileEntryMetadata::default(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn panel_refresh_resolves_canonical_paths_in_the_worker() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-panel-path-identity-{stamp}"));
        let home = root.join("home");
        fs::create_dir_all(home.join("projects")).expect("home fixture should be creatable");
        let alias = root.join("home-alias");
        std::os::unix::fs::symlink(&home, &alias).expect("home alias should be creatable");
        let request = PanelRefreshStreamRequest {
            panel: ActivePanel::Left,
            cwd: alias,
            source: PanelListingSource::Directory,
            sort_mode: SortMode::default(),
            filter: PanelFilter::default(),
            show_hidden_files: true,
            cached_panelized_entries: None,
            home_directory: Some(home.clone()),
            request_id: 40,
        };

        let result = stream_refresh_panel_entries(&request, &AtomicBool::new(false), |_| true)
            .expect("panel refresh should resolve path identity");

        let canonical_home = fs::canonicalize(&home).expect("home should canonicalize");
        assert_eq!(
            result.canonical_cwd.as_deref(),
            Some(canonical_home.as_path())
        );
        assert_eq!(
            result.canonical_home_directory.as_deref(),
            Some(canonical_home.as_path())
        );

        fs::remove_dir_all(root).expect("temporary directory should be removed");
    }

    #[test]
    fn find_panelized_stream_uses_bounded_chunks_without_losing_entries() {
        let paths: Vec<PathBuf> = (0..300)
            .map(|index| PathBuf::from(format!("entry-{index:03}.txt")))
            .collect();
        let request = PanelRefreshStreamRequest {
            panel: ActivePanel::Right,
            cwd: PathBuf::from("."),
            source: PanelListingSource::FindResults {
                query: String::from("*.txt"),
                base_dir: PathBuf::from("."),
                paths,
            },
            sort_mode: SortMode::default(),
            filter: PanelFilter::default(),
            show_hidden_files: true,
            cached_panelized_entries: None,
            home_directory: None,
            request_id: 41,
        };
        let cancel_flag = AtomicBool::new(false);
        let mut chunk_sizes = Vec::new();

        let final_entries = stream_refresh_panel_entries(&request, &cancel_flag, |event| {
            let BackgroundEvent::PanelEntriesChunk {
                request_id,
                entries,
                ..
            } = event
            else {
                return false;
            };
            assert_eq!(request_id, 41);
            chunk_sizes.push(entries.len());
            true
        })
        .expect("find-panelized paths should stream");

        assert_eq!(chunk_sizes.first(), Some(&1));
        assert!(
            chunk_sizes
                .iter()
                .all(|chunk_size| *chunk_size <= PANEL_EVENT_CHUNK_SIZE),
            "no streamed chunk may exceed the event bound"
        );
        assert_eq!(chunk_sizes.iter().sum::<usize>(), 300);
        assert_eq!(final_entries.entries.len(), 300);
    }

    #[test]
    fn directory_stream_filters_files_but_keeps_navigation_directories() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-panel-filter-stream-{stamp}"));
        fs::create_dir_all(root.join("docs")).expect("directory fixture should be created");
        fs::write(root.join("main.rs"), "fn main() {}").expect("Rust fixture should be written");
        fs::write(root.join("notes.txt"), "notes").expect("text fixture should be written");
        let request = PanelRefreshStreamRequest {
            panel: ActivePanel::Left,
            cwd: root.clone(),
            source: PanelListingSource::Directory,
            sort_mode: SortMode::default(),
            filter: PanelFilter {
                pattern: String::from("*.rs"),
                ..PanelFilter::default()
            },
            show_hidden_files: true,
            cached_panelized_entries: None,
            home_directory: None,
            request_id: 42,
        };

        let result = stream_refresh_panel_entries(&request, &AtomicBool::new(false), |_| true)
            .expect("filtered directory should stream");
        let names = result
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();

        assert!(names.contains(&".."));
        assert!(names.contains(&"docs"));
        assert!(names.contains(&"main.rs"));
        assert!(!names.contains(&"notes.txt"));
        fs::remove_dir_all(root).expect("temporary directory should be removed");
    }

    #[test]
    fn cached_panelized_filter_does_not_execute_the_source_command() {
        let cached = Arc::<[FileEntry]>::from(vec![
            cached_entry("main.rs", FileEntryKind::File),
            cached_entry("notes.txt", FileEntryKind::File),
            cached_entry("src", FileEntryKind::Directory),
        ]);
        let request = PanelRefreshStreamRequest {
            panel: ActivePanel::Right,
            cwd: PathBuf::from("."),
            source: PanelListingSource::Panelize {
                command: String::from("this-command-must-never-run"),
            },
            sort_mode: SortMode::default(),
            filter: PanelFilter {
                pattern: String::from("^main\\.rs$"),
                files_only: false,
                name_mode: FindNameMode::Regex,
                case_sensitive: true,
            },
            show_hidden_files: true,
            cached_panelized_entries: Some(Arc::clone(&cached)),
            home_directory: None,
            request_id: 43,
        };

        let result = stream_refresh_panel_entries(&request, &AtomicBool::new(false), |_| true)
            .expect("cached entries should bypass the panelize command");

        assert_eq!(
            result
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["main.rs"]
        );
        assert!(
            result
                .panelized_entries
                .as_ref()
                .is_some_and(|entries| Arc::ptr_eq(entries, &cached))
        );
    }
}
