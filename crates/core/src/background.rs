use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::thread;
use std::time::Duration;

use crate::{
    ActivePanel, FileEntry, FindResultEntry, JOB_CANCELED_MESSAGE, JobId, PanelListingSource,
    SortMode, TreeEntry, ViewerState, build_tree_entries, ensure_panel_refresh_not_canceled,
    read_entries_with_visibility_cancel, read_panelized_entries_with_cancel, read_panelized_paths,
    sort_file_entries,
};

const FIND_EVENT_CHUNK_SIZE: usize = 64;
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
        root: PathBuf,
        entries: Vec<TreeEntry>,
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
    BackgroundEvent::PanelRefreshed {
        panel,
        cwd,
        source,
        sort_mode,
        request_id,
        result,
    }
}

pub fn build_tree_ready_event(
    root: PathBuf,
    max_depth: usize,
    max_entries: usize,
) -> BackgroundEvent {
    let entries = build_tree_entries(&root, max_depth, max_entries);
    BackgroundEvent::TreeReady { root, entries }
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

pub fn run_find_entries<F>(
    base_dir: &Path,
    query: &str,
    max_results: usize,
    cancel_flag: &AtomicBool,
    pause_flag: &AtomicBool,
    emit_chunk: F,
) -> Result<(), String>
where
    F: FnMut(Vec<FindResultEntry>) -> bool,
{
    stream_find_entries(
        base_dir,
        query,
        max_results,
        cancel_flag,
        pause_flag,
        FIND_EVENT_CHUNK_SIZE,
        emit_chunk,
    )
}

pub fn stream_find_entries<F>(
    base_dir: &Path,
    query: &str,
    max_results: usize,
    cancel_flag: &AtomicBool,
    pause_flag: &AtomicBool,
    chunk_size: usize,
    mut emit_chunk: F,
) -> Result<(), String>
where
    F: FnMut(Vec<FindResultEntry>) -> bool,
{
    if max_results == 0 {
        return Ok(());
    }

    let normalized_query = query.trim().to_lowercase();
    if normalized_query.is_empty() {
        return Ok(());
    }

    let wildcard_query = normalized_query.contains('*') || normalized_query.contains('?');
    let chunk_size = chunk_size.max(1);
    let mut emitted = Vec::new();
    let mut matched = 0usize;
    let mut stack = vec![base_dir.to_path_buf()];

    while let Some(dir) = stack.pop() {
        wait_for_find_resume(cancel_flag, pause_flag)?;

        let read_dir = match fs::read_dir(&dir) {
            Ok(read_dir) => read_dir,
            Err(_) => continue,
        };
        let mut child_dirs = Vec::new();

        for entry in read_dir.flatten() {
            wait_for_find_resume(cancel_flag, pause_flag)?;

            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let metadata = fs::metadata(&path).ok().or_else(|| entry.metadata().ok());
            let is_dir =
                file_type.is_dir() || metadata.as_ref().is_some_and(std::fs::Metadata::is_dir);

            if query_matches_entry(&name, &normalized_query, wildcard_query) {
                emitted.push(FindResultEntry {
                    path: path.clone(),
                    is_dir,
                });
                matched = matched.saturating_add(1);

                if emitted.len() >= chunk_size && !emit_chunk(std::mem::take(&mut emitted)) {
                    return Err(String::from("background event channel disconnected"));
                }

                if matched >= max_results {
                    if !emitted.is_empty() && !emit_chunk(std::mem::take(&mut emitted)) {
                        return Err(String::from("background event channel disconnected"));
                    }
                    return Ok(());
                }
            }

            if file_type.is_dir() {
                child_dirs.push(path);
            }
        }

        child_dirs.sort_by_cached_key(|left| path_sort_key(left));
        for child_dir in child_dirs.into_iter().rev() {
            stack.push(child_dir);
        }
    }

    if !emitted.is_empty() && !emit_chunk(emitted) {
        return Err(String::from("background event channel disconnected"));
    }
    Ok(())
}

fn wait_for_find_resume(cancel_flag: &AtomicBool, pause_flag: &AtomicBool) -> Result<(), String> {
    loop {
        if cancel_flag.load(AtomicOrdering::Relaxed) {
            return Err(String::from(JOB_CANCELED_MESSAGE));
        }
        if !pause_flag.load(AtomicOrdering::Relaxed) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn query_matches_entry(name: &str, normalized_query: &str, wildcard_query: bool) -> bool {
    let normalized_name = name.to_lowercase();
    if wildcard_query {
        wildcard_match(&normalized_name, normalized_query)
    } else {
        normalized_name.contains(normalized_query)
    }
}

fn wildcard_match(text: &str, pattern: &str) -> bool {
    let text: Vec<char> = text.chars().collect();
    let pattern: Vec<char> = pattern.chars().collect();
    let mut text_index = 0usize;
    let mut pattern_index = 0usize;
    let mut star_index: Option<usize> = None;
    let mut match_index = 0usize;

    while text_index < text.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == '?' || pattern[pattern_index] == text[text_index])
        {
            text_index += 1;
            pattern_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == '*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            match_index = text_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            match_index += 1;
            text_index = match_index;
        } else {
            return false;
        }
    }

    while pattern_index < pattern.len() && pattern[pattern_index] == '*' {
        pattern_index += 1;
    }

    pattern_index == pattern.len()
}

fn path_sort_key(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| path.to_string_lossy().to_lowercase())
}
