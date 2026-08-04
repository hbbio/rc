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
        let is_dir = file_type.is_dir() || metadata.as_ref().is_some_and(std::fs::Metadata::is_dir);
        if is_dir {
            entries.push(FileEntry::directory_from_metadata(
                name,
                path,
                metadata.as_ref(),
            ));
        } else {
            entries.push(FileEntry::file_from_metadata(name, path, metadata.as_ref()));
        }
    }

    sort_file_entries(&mut entries, sort_mode);

    if let Some(parent) = dir.parent() {
        entries.insert(0, FileEntry::parent(parent.to_path_buf()));
    }
    Ok(entries)
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

pub(crate) fn stream_panelized_entries_with_cancel(
    base_dir: &Path,
    command: &str,
    cancel_flag: Option<&AtomicBool>,
    emit_entry: &mut dyn FnMut(&FileEntry) -> io::Result<()>,
) -> io::Result<Vec<FileEntry>> {
    let process_backend = LocalProcessBackend;
    read_panelized_entries_with_process_backend_and_emit(
        base_dir,
        command,
        cancel_flag,
        &process_backend,
        emit_entry,
    )
}

pub(crate) fn read_panelized_entries_with_process_backend(
    base_dir: &Path,
    command: &str,
    sort_mode: SortMode,
    cancel_flag: Option<&AtomicBool>,
    process_backend: &dyn ProcessBackend,
) -> io::Result<Vec<FileEntry>> {
    let mut entries = read_panelized_entries_with_process_backend_and_emit(
        base_dir,
        command,
        cancel_flag,
        process_backend,
        &mut |_| Ok(()),
    )?;
    sort_file_entries(&mut entries, sort_mode);
    Ok(entries)
}

fn read_panelized_entries_with_process_backend_and_emit(
    base_dir: &Path,
    command: &str,
    cancel_flag: Option<&AtomicBool>,
    process_backend: &dyn ProcessBackend,
    emit_entry: &mut dyn FnMut(&FileEntry) -> io::Result<()>,
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
            let Some(entry) = panelized_stdout_entry(base_dir, raw_line, &mut seen, cancel_flag)?
            else {
                return Ok(());
            };
            entries.push(entry);
            if entries.len() > PANELIZE_MAX_ENTRIES {
                return Err(io::Error::other(format!(
                    "panelize produced more than {PANELIZE_MAX_ENTRIES} entries"
                )));
            }
            emit_entry(entries.last().expect("entry was just appended"))
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

    Ok(entries)
}

fn panelized_stdout_entry(
    base_dir: &Path,
    raw_line: &[u8],
    seen: &mut HashSet<PathBuf>,
    cancel_flag: Option<&AtomicBool>,
) -> io::Result<Option<FileEntry>> {
    ensure_panel_refresh_not_canceled(cancel_flag)?;
    let line = String::from_utf8_lossy(raw_line);
    let line = line.strip_suffix('\n').unwrap_or(line.as_ref());
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.is_empty() {
        return Ok(None);
    }

    panelized_path_entry(base_dir, PathBuf::from(line), seen, cancel_flag)
}

pub(crate) fn read_panelized_paths(
    base_dir: &Path,
    paths: &[PathBuf],
    sort_mode: SortMode,
    cancel_flag: Option<&AtomicBool>,
) -> io::Result<Vec<FileEntry>> {
    let mut entries =
        stream_panelized_paths_with_cancel(base_dir, paths, cancel_flag, &mut |_| Ok(()))?;
    sort_file_entries(&mut entries, sort_mode);
    Ok(entries)
}

pub(crate) fn stream_panelized_paths_with_cancel(
    base_dir: &Path,
    paths: &[PathBuf],
    cancel_flag: Option<&AtomicBool>,
    emit_entry: &mut dyn FnMut(&FileEntry) -> io::Result<()>,
) -> io::Result<Vec<FileEntry>> {
    let mut seen = HashSet::with_capacity(paths.len());
    let mut entries = Vec::with_capacity(paths.len());
    for path in paths {
        let Some(entry) = panelized_path_entry(base_dir, path.clone(), &mut seen, cancel_flag)?
        else {
            continue;
        };
        entries.push(entry);
        emit_entry(entries.last().expect("entry was just appended"))?;
    }
    Ok(entries)
}

fn panelized_path_entry(
    base_dir: &Path,
    input_path: PathBuf,
    seen: &mut HashSet<PathBuf>,
    cancel_flag: Option<&AtomicBool>,
) -> io::Result<Option<FileEntry>> {
    ensure_panel_refresh_not_canceled(cancel_flag)?;
    let path = if input_path.is_absolute() {
        input_path
    } else {
        base_dir.join(input_path)
    };
    if !seen.insert(path.clone()) {
        return Ok(None);
    }

    let metadata = fs::metadata(&path).ok();
    let name = panelized_entry_label(base_dir, &path);
    let is_dir = metadata.as_ref().is_some_and(std::fs::Metadata::is_dir);
    let entry = if is_dir {
        FileEntry::directory_from_metadata(name, path, metadata.as_ref())
    } else {
        FileEntry::file_from_metadata(name, path, metadata.as_ref())
    };
    Ok(Some(entry))
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

    if sort_mode.field == SortField::Unsorted {
        entries.sort_by_key(type_rank);
        if sort_mode.reverse {
            let first_file = entries.partition_point(FileEntry::is_dir);
            entries[..first_file].reverse();
            entries[first_file..].reverse();
        }
        return;
    }

    if sort_mode.reverse {
        entries.sort_by_cached_key(|entry| {
            (
                type_rank(entry),
                Reverse(entry_sort_key(entry, sort_mode.field)),
            )
        });
    } else {
        entries
            .sort_by_cached_key(|entry| (type_rank(entry), entry_sort_key(entry, sort_mode.field)));
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct EntrySortKey {
    primary: EntrySortValue,
    natural_name: VersionKey,
    folded_name: String,
    exact_name: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum EntrySortValue {
    Text(String),
    Version(VersionKey),
    Time(Option<std::time::SystemTime>),
    Number(Option<u64>),
}

fn entry_sort_key(entry: &FileEntry, field: SortField) -> EntrySortKey {
    let folded_name = entry.name.to_lowercase();
    let natural_name = VersionKey::new(&entry.name);
    let primary = match field {
        SortField::Name => EntrySortValue::Text(folded_name.clone()),
        SortField::Version => EntrySortValue::Version(natural_name.clone()),
        SortField::Extension => EntrySortValue::Text(
            Path::new(&entry.name)
                .extension()
                .map(|extension| extension.to_string_lossy().to_lowercase())
                .unwrap_or_default(),
        ),
        SortField::Modified => EntrySortValue::Time(entry.modified),
        SortField::Accessed => EntrySortValue::Time(entry.metadata.accessed),
        SortField::Changed => EntrySortValue::Time(entry.metadata.changed),
        SortField::Size => EntrySortValue::Number(Some(entry.size)),
        SortField::Inode => EntrySortValue::Number(entry.metadata.inode),
        SortField::Unsorted => unreachable!("unsorted entries do not build sort keys"),
    };
    EntrySortKey {
        primary,
        natural_name,
        folded_name,
        exact_name: entry.name.clone(),
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct VersionKey(Vec<VersionToken>);

impl VersionKey {
    fn new(value: &str) -> Self {
        let mut tokens = Vec::new();
        let mut characters = value.chars().peekable();
        while let Some(character) = characters.peek().copied() {
            if character.is_ascii_digit() {
                let mut digits = String::new();
                while let Some(digit) = characters.peek().copied().filter(char::is_ascii_digit) {
                    digits.push(digit);
                    characters.next();
                }
                tokens.push(VersionToken::Number(NumericVersionToken::new(digits)));
            } else {
                let mut text = String::new();
                while let Some(character) = characters
                    .peek()
                    .copied()
                    .filter(|character| !character.is_ascii_digit())
                {
                    text.extend(character.to_lowercase());
                    characters.next();
                }
                tokens.push(VersionToken::Text(text));
            }
        }
        Self(tokens)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum VersionToken {
    Number(NumericVersionToken),
    Text(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NumericVersionToken {
    significant_digits: String,
    leading_zeroes: usize,
}

impl NumericVersionToken {
    fn new(digits: String) -> Self {
        let significant = digits.trim_start_matches('0');
        let significant_digits = if significant.is_empty() {
            String::from("0")
        } else {
            significant.to_string()
        };
        let leading_zeroes = digits.len().saturating_sub(significant_digits.len());
        Self {
            significant_digits,
            leading_zeroes,
        }
    }
}

impl Ord for NumericVersionToken {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.significant_digits
            .len()
            .cmp(&other.significant_digits.len())
            .then_with(|| self.significant_digits.cmp(&other.significant_digits))
            .then_with(|| self.leading_zeroes.cmp(&other.leading_zeroes))
    }
}

impl PartialOrd for NumericVersionToken {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
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
