use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crate::{
    ActivePanel, DiskUsageSummary, FileEntry, FindResultEntry, JobId, PanelListingSource, SortMode,
    TreeBuildResult, ViewerState, build_tree_entries, ensure_panel_refresh_not_canceled,
    read_entries_with_visibility_cancel, read_panelized_entries_with_cancel, read_panelized_paths,
    sort_file_entries, stream_panelized_entries_with_cancel, stream_panelized_paths_with_cancel,
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
    pub show_hidden_files: bool,
    pub request_id: u64,
}

#[derive(Clone, Debug)]
pub enum BackgroundEvent {
    PanelEntriesChunk {
        panel: ActivePanel,
        cwd: PathBuf,
        source: PanelListingSource,
        sort_mode: SortMode,
        request_id: u64,
        entries: Vec<FileEntry>,
    },
    PanelRefreshed {
        panel: ActivePanel,
        cwd: PathBuf,
        source: PanelListingSource,
        sort_mode: SortMode,
        request_id: u64,
        disk_usage: Option<DiskUsageSummary>,
        result: Result<Vec<FileEntry>, String>,
    },
    ViewerLoaded {
        path: PathBuf,
        result: Result<ViewerState, String>,
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
    panel: ActivePanel,
    cwd: PathBuf,
    source: PanelListingSource,
    sort_mode: SortMode,
    show_hidden_files: bool,
    request_id: u64,
    cancel_flag: &AtomicBool,
) -> BackgroundEvent {
    let result = refresh_panel_entries(&cwd, &source, sort_mode, show_hidden_files, cancel_flag)
        .map_err(|error| error.to_string());
    let disk_usage = result
        .as_ref()
        .ok()
        .and_then(|_| read_disk_usage(cwd.as_path()));
    BackgroundEvent::PanelRefreshed {
        panel,
        cwd,
        source,
        sort_mode,
        request_id,
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

pub fn stream_refresh_panel_entries<F>(
    request: &PanelRefreshStreamRequest,
    cancel_flag: &AtomicBool,
    mut emit_chunk: F,
) -> io::Result<Vec<FileEntry>>
where
    F: FnMut(BackgroundEvent) -> bool,
{
    match &request.source {
        PanelListingSource::Directory => {
            stream_directory_entries(request, cancel_flag, &mut emit_chunk)
        }
        PanelListingSource::Panelize { .. } | PanelListingSource::FindResults { .. } => {
            stream_panelized_source_entries(request, cancel_flag, &mut emit_chunk)
        }
    }
}

fn stream_panelized_source_entries<F>(
    request: &PanelRefreshStreamRequest,
    cancel_flag: &AtomicBool,
    emit_chunk: &mut F,
) -> io::Result<Vec<FileEntry>>
where
    F: FnMut(BackgroundEvent) -> bool,
{
    let mut pending = Vec::with_capacity(1);
    let mut next_chunk_size = 1_usize;
    let mut last_emit = None::<Instant>;
    let mut emit_entry = |entry: &FileEntry| {
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

    let entries = match &request.source {
        PanelListingSource::Panelize { command } => stream_panelized_entries_with_cancel(
            &request.cwd,
            command,
            request.sort_mode,
            Some(cancel_flag),
            &mut emit_entry,
        ),
        PanelListingSource::FindResults {
            base_dir, paths, ..
        } => stream_panelized_paths_with_cancel(
            base_dir,
            paths,
            request.sort_mode,
            Some(cancel_flag),
            &mut emit_entry,
        ),
        PanelListingSource::Directory => unreachable!("directory sources use directory streaming"),
    }?;
    emit_panel_entries_chunk(request, &mut pending, emit_chunk)?;
    Ok(entries)
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
) -> io::Result<Vec<FileEntry>>
where
    F: FnMut(BackgroundEvent) -> bool,
{
    let cwd = request.cwd.as_path();
    ensure_panel_refresh_not_canceled(Some(cancel_flag))?;
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
        let size = metadata.as_ref().map_or(0, std::fs::Metadata::len);
        let modified = metadata.as_ref().and_then(|meta| meta.modified().ok());
        let is_dir = file_type.is_dir() || metadata.as_ref().is_some_and(std::fs::Metadata::is_dir);
        let panel_entry = if is_dir {
            FileEntry::directory(name, path, size, modified)
        } else {
            FileEntry::file(name, path, size, modified)
        };
        entries.push(panel_entry.clone());
        emitted.push(panel_entry);

        if emitted.len() >= PANEL_EVENT_CHUNK_SIZE {
            let delivered = emit_chunk(BackgroundEvent::PanelEntriesChunk {
                panel: request.panel,
                cwd: request.cwd.clone(),
                source: request.source.clone(),
                sort_mode: request.sort_mode,
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
    Ok(entries)
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
            show_hidden_files: true,
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
        assert_eq!(final_entries.len(), 300);
    }
}
