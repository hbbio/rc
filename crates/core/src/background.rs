use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::Duration;

use crate::{
    ActivePanel, FileEntry, FindResultEntry, JOB_CANCELED_MESSAGE, JobId, PanelListingSource,
    SortMode, TreeEntry, ViewerState, build_tree_entries, read_entries_with_visibility_cancel,
    read_panelized_entries_with_cancel, read_panelized_paths,
};

const FIND_EVENT_CHUNK_SIZE: usize = 64;

#[derive(Clone, Debug)]
pub enum BackgroundCommand {
    RefreshPanel {
        panel: ActivePanel,
        cwd: PathBuf,
        source: PanelListingSource,
        sort_mode: SortMode,
        show_hidden_files: bool,
        request_id: u64,
        cancel_flag: Arc<AtomicBool>,
    },
    LoadViewer {
        path: PathBuf,
    },
    FindEntries {
        job_id: JobId,
        query: String,
        base_dir: PathBuf,
        max_results: usize,
        cancel_flag: Arc<AtomicBool>,
        pause_flag: Arc<AtomicBool>,
    },
    BuildTree {
        root: PathBuf,
        max_depth: usize,
        max_entries: usize,
    },
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum BackgroundEvent {
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
    FindEntriesStarted {
        job_id: JobId,
    },
    FindEntriesChunk {
        job_id: JobId,
        entries: Vec<FindResultEntry>,
    },
    FindEntriesFinished {
        job_id: JobId,
        result: Result<(), String>,
    },
    TreeReady {
        root: PathBuf,
        entries: Vec<TreeEntry>,
    },
}

pub fn run_background_worker(
    command_rx: Receiver<BackgroundCommand>,
    event_tx: Sender<BackgroundEvent>,
) {
    while let Ok(command) = command_rx.recv() {
        if !run_background_command_sync(command, &event_tx) {
            break;
        }
    }
}

pub fn run_background_command_sync(
    command: BackgroundCommand,
    event_tx: &Sender<BackgroundEvent>,
) -> bool {
    match command {
        BackgroundCommand::RefreshPanel {
            panel,
            cwd,
            source,
            sort_mode,
            show_hidden_files,
            request_id,
            cancel_flag,
        } => {
            let result = refresh_panel_entries(
                &cwd,
                &source,
                sort_mode,
                show_hidden_files,
                cancel_flag.as_ref(),
            );
            event_tx
                .send(BackgroundEvent::PanelRefreshed {
                    panel,
                    cwd,
                    source,
                    sort_mode,
                    request_id,
                    result,
                })
                .is_ok()
        }
        BackgroundCommand::LoadViewer { path } => event_tx
            .send(BackgroundEvent::ViewerLoaded {
                path: path.clone(),
                result: ViewerState::open(path).map_err(|error| error.to_string()),
            })
            .is_ok(),
        BackgroundCommand::FindEntries {
            job_id,
            query,
            base_dir,
            max_results,
            cancel_flag,
            pause_flag,
        } => run_find_search(
            event_tx,
            job_id,
            query,
            base_dir,
            max_results,
            cancel_flag.as_ref(),
            pause_flag.as_ref(),
        ),
        BackgroundCommand::BuildTree {
            root,
            max_depth,
            max_entries,
        } => {
            let entries = build_tree_entries(&root, max_depth, max_entries);
            event_tx
                .send(BackgroundEvent::TreeReady { root, entries })
                .is_ok()
        }
        BackgroundCommand::Shutdown => false,
    }
}

fn refresh_panel_entries(
    cwd: &Path,
    source: &PanelListingSource,
    sort_mode: SortMode,
    show_hidden_files: bool,
    cancel_flag: &AtomicBool,
) -> Result<Vec<FileEntry>, String> {
    match source {
        PanelListingSource::Directory => read_entries_with_visibility_cancel(
            cwd,
            sort_mode,
            show_hidden_files,
            Some(cancel_flag),
        )
        .map_err(|error| error.to_string()),
        PanelListingSource::Panelize { command } => {
            read_panelized_entries_with_cancel(cwd, command, sort_mode, Some(cancel_flag))
                .map_err(|error| error.to_string())
        }
        PanelListingSource::FindResults {
            base_dir, paths, ..
        } => read_panelized_paths(base_dir, paths, sort_mode, Some(cancel_flag))
            .map_err(|error| error.to_string()),
    }
}

fn run_find_search(
    event_tx: &Sender<BackgroundEvent>,
    job_id: JobId,
    query: String,
    base_dir: PathBuf,
    max_results: usize,
    cancel_flag: &AtomicBool,
    pause_flag: &AtomicBool,
) -> bool {
    if event_tx
        .send(BackgroundEvent::FindEntriesStarted { job_id })
        .is_err()
    {
        return false;
    }

    let result = stream_find_entries(
        &base_dir,
        &query,
        max_results,
        cancel_flag,
        pause_flag,
        FIND_EVENT_CHUNK_SIZE,
        |entries| {
            event_tx
                .send(BackgroundEvent::FindEntriesChunk { job_id, entries })
                .is_ok()
        },
    );

    event_tx
        .send(BackgroundEvent::FindEntriesFinished { job_id, result })
        .is_ok()
}

pub(crate) fn stream_find_entries<F>(
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
            let is_dir = file_type.is_dir();

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

            if is_dir {
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
