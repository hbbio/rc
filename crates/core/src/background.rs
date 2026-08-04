use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::{
    ActivePanel, DiskUsageSummary, FileEntry, FindResultEntry, JobId, PanelListingSource, SortMode,
    TreeBuildResult, ViewerState, build_tree_entries, ensure_panel_refresh_not_canceled,
    read_entries_with_visibility_cancel, read_panelized_entries_with_cancel, read_panelized_paths,
    sort_file_entries,
};

#[cfg(unix)]
use nix::sys::statvfs::statvfs;

const PANEL_EVENT_CHUNK_SIZE: usize = 96;

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
        _ => refresh_panel_entries(
            &request.cwd,
            &request.source,
            request.sort_mode,
            request.show_hidden_files,
            cancel_flag,
        ),
    }
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
