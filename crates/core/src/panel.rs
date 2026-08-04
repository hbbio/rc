use std::cmp::Reverse;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

use crate::{
    FileEntry, LocalProcessBackend, ProcessBackend, ProcessOutputLimits, SortField, SortMode,
};

pub(crate) const PANEL_REFRESH_CANCELED_MESSAGE: &str = "panel refresh canceled";
const PANELIZE_STDOUT_LIMIT_BYTES: usize = 16 * 1024 * 1024;
const PANELIZE_STDERR_LIMIT_BYTES: usize = 64 * 1024;
const PANELIZE_MAX_ENTRIES: usize = 100_000;

#[cfg(test)]
pub(crate) fn read_entries(dir: &Path, sort_mode: SortMode) -> io::Result<Vec<FileEntry>> {
    read_entries_with_visibility_cancel(dir, sort_mode, true, None)
}

pub(super) fn read_entries_with_visibility(
    dir: &Path,
    sort_mode: SortMode,
    show_hidden_files: bool,
) -> io::Result<Vec<FileEntry>> {
    read_entries_with_visibility_cancel(dir, sort_mode, show_hidden_files, None)
}

pub(crate) fn read_entries_with_visibility_cancel(
    dir: &Path,
    sort_mode: SortMode,
    show_hidden_files: bool,
    cancel_flag: Option<&AtomicBool>,
) -> io::Result<Vec<FileEntry>> {
    ensure_panel_refresh_not_canceled(cancel_flag)?;
    let mut entries = Vec::new();
    for entry_result in fs::read_dir(dir)? {
        ensure_panel_refresh_not_canceled(cancel_flag)?;
        let entry = entry_result?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !show_hidden_files && name.starts_with('.') {
            continue;
        }
        let file_type = entry.file_type()?;
        let metadata = fs::metadata(&path).ok().or_else(|| entry.metadata().ok());
        let size = metadata.as_ref().map_or(0, std::fs::Metadata::len);
        let modified = metadata.as_ref().and_then(|meta| meta.modified().ok());
        let is_dir = file_type.is_dir() || metadata.as_ref().is_some_and(std::fs::Metadata::is_dir);
        if is_dir {
            entries.push(FileEntry::directory(name, path, size, modified));
        } else {
            entries.push(FileEntry::file(name, path, size, modified));
        }
    }

    sort_file_entries(&mut entries, sort_mode);

    if let Some(parent) = dir.parent() {
        entries.insert(0, FileEntry::parent(parent.to_path_buf()));
    }
    Ok(entries)
}

pub(super) fn read_panelized_entries(
    base_dir: &Path,
    command: &str,
    sort_mode: SortMode,
) -> io::Result<Vec<FileEntry>> {
    read_panelized_entries_with_cancel(base_dir, command, sort_mode, None)
}

pub(crate) fn read_panelized_entries_with_cancel(
    base_dir: &Path,
    command: &str,
    sort_mode: SortMode,
    cancel_flag: Option<&AtomicBool>,
) -> io::Result<Vec<FileEntry>> {
    let process_backend = LocalProcessBackend;
    read_panelized_entries_with_process_backend(
        base_dir,
        command,
        sort_mode,
        cancel_flag,
        &process_backend,
    )
}

pub(crate) fn read_panelized_entries_with_process_backend(
    base_dir: &Path,
    command: &str,
    sort_mode: SortMode,
    cancel_flag: Option<&AtomicBool>,
    process_backend: &dyn ProcessBackend,
) -> io::Result<Vec<FileEntry>> {
    ensure_panel_refresh_not_canceled(cancel_flag)?;
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    let output = process_backend.run_shell_command_streaming(
        base_dir,
        command,
        cancel_flag,
        PANEL_REFRESH_CANCELED_MESSAGE,
        ProcessOutputLimits {
            stdout_bytes: PANELIZE_STDOUT_LIMIT_BYTES,
            stderr_bytes: PANELIZE_STDERR_LIMIT_BYTES,
        },
        &mut |raw_line| {
            append_panelized_stdout_line(base_dir, raw_line, &mut seen, &mut entries, cancel_flag)
        },
    )?;
    if !output.success {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        let detail = if stderr.is_empty() {
            output.status_label
        } else {
            stderr.to_string()
        };
        return Err(io::Error::other(format!("command failed: {detail}")));
    }

    sort_file_entries(&mut entries, sort_mode);
    Ok(entries)
}

fn append_panelized_stdout_line(
    base_dir: &Path,
    raw_line: &[u8],
    seen: &mut HashSet<PathBuf>,
    entries: &mut Vec<FileEntry>,
    cancel_flag: Option<&AtomicBool>,
) -> io::Result<()> {
    ensure_panel_refresh_not_canceled(cancel_flag)?;
    let line = String::from_utf8_lossy(raw_line);
    let line = line.strip_suffix('\n').unwrap_or(line.as_ref());
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.is_empty() {
        return Ok(());
    }

    append_panelized_path_entry(base_dir, PathBuf::from(line), seen, entries, cancel_flag)?;
    if entries.len() > PANELIZE_MAX_ENTRIES {
        return Err(io::Error::other(format!(
            "panelize produced more than {PANELIZE_MAX_ENTRIES} entries"
        )));
    }
    Ok(())
}

pub(crate) fn read_panelized_paths(
    base_dir: &Path,
    paths: &[PathBuf],
    sort_mode: SortMode,
    cancel_flag: Option<&AtomicBool>,
) -> io::Result<Vec<FileEntry>> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for path in paths {
        append_panelized_path_entry(base_dir, path.clone(), &mut seen, &mut entries, cancel_flag)?;
    }
    sort_file_entries(&mut entries, sort_mode);
    Ok(entries)
}

fn append_panelized_path_entry(
    base_dir: &Path,
    input_path: PathBuf,
    seen: &mut HashSet<PathBuf>,
    entries: &mut Vec<FileEntry>,
    cancel_flag: Option<&AtomicBool>,
) -> io::Result<()> {
    ensure_panel_refresh_not_canceled(cancel_flag)?;
    let path = if input_path.is_absolute() {
        input_path
    } else {
        base_dir.join(input_path)
    };
    if !seen.insert(path.clone()) {
        return Ok(());
    }

    let metadata = fs::metadata(&path).ok();
    let size = metadata.as_ref().map_or(0, std::fs::Metadata::len);
    let modified = metadata.as_ref().and_then(|meta| meta.modified().ok());
    let name = panelized_entry_label(base_dir, &path);
    let is_dir = metadata.as_ref().is_some_and(std::fs::Metadata::is_dir);
    if is_dir {
        entries.push(FileEntry::directory(name, path, size, modified));
    } else {
        entries.push(FileEntry::file(name, path, size, modified));
    }
    Ok(())
}

pub(crate) fn ensure_panel_refresh_not_canceled(
    cancel_flag: Option<&AtomicBool>,
) -> io::Result<()> {
    if cancel_flag.is_some_and(|flag| flag.load(AtomicOrdering::Relaxed)) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            PANEL_REFRESH_CANCELED_MESSAGE,
        ));
    }
    Ok(())
}

pub(crate) fn sort_file_entries(entries: &mut [FileEntry], sort_mode: SortMode) {
    let type_rank = |entry: &FileEntry| if entry.is_dir() { 0_u8 } else { 1_u8 };

    match (sort_mode.field, sort_mode.reverse) {
        (SortField::Name, false) => {
            entries.sort_by_cached_key(|entry| (type_rank(entry), entry.name.to_lowercase()));
        }
        (SortField::Name, true) => {
            entries
                .sort_by_cached_key(|entry| (type_rank(entry), Reverse(entry.name.to_lowercase())));
        }
        (SortField::Size, false) => {
            entries.sort_by_cached_key(|entry| {
                (type_rank(entry), (entry.size, entry.name.to_lowercase()))
            });
        }
        (SortField::Size, true) => {
            entries.sort_by_cached_key(|entry| {
                (
                    type_rank(entry),
                    Reverse((entry.size, entry.name.to_lowercase())),
                )
            });
        }
        (SortField::Modified, false) => {
            entries.sort_by_cached_key(|entry| {
                (
                    type_rank(entry),
                    (entry.modified, entry.name.to_lowercase()),
                )
            });
        }
        (SortField::Modified, true) => {
            entries.sort_by_cached_key(|entry| {
                (
                    type_rank(entry),
                    Reverse((entry.modified, entry.name.to_lowercase())),
                )
            });
        }
    }
}

fn panelized_entry_label(base_dir: &Path, path: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(base_dir) {
        let relative = relative.to_string_lossy();
        if relative.is_empty() {
            String::from(".")
        } else {
            relative.into_owned()
        }
    } else {
        path.to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProcessExit;

    struct ManyLineProcessBackend {
        lines: usize,
    }

    impl ProcessBackend for ManyLineProcessBackend {
        fn run_shell_command_streaming(
            &self,
            _cwd: &Path,
            _command: &str,
            _cancel_flag: Option<&AtomicBool>,
            _canceled_message: &str,
            _limits: ProcessOutputLimits,
            stdout_line: &mut dyn FnMut(&[u8]) -> io::Result<()>,
        ) -> io::Result<ProcessExit> {
            for index in 0..self.lines {
                stdout_line(format!("entry-{index}\n").as_bytes())?;
            }
            Ok(ProcessExit {
                success: true,
                status_label: String::from("exit status: 0"),
                stderr: Vec::new(),
            })
        }
    }

    #[test]
    fn panelize_rejects_too_many_streamed_entries() {
        let backend = ManyLineProcessBackend {
            lines: PANELIZE_MAX_ENTRIES + 1,
        };
        let error = read_panelized_entries_with_process_backend(
            Path::new("."),
            "ignored",
            SortMode::default(),
            None,
            &backend,
        )
        .expect_err("panelize should reject excessive output");

        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(
            error.to_string().contains("panelize produced more than"),
            "panelize limit error should be explicit"
        );
    }
}
