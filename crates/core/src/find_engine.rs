use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use globset::{GlobBuilder, GlobMatcher};
use regex::RegexBuilder;
use regex::bytes::{Regex as BytesRegex, RegexBuilder as BytesRegexBuilder};

use crate::{FindResultEntry, JOB_CANCELED_MESSAGE};

const DEFAULT_FIND_CHUNK_SIZE: usize = 64;
const CONTENT_READ_BUFFER_SIZE: usize = 64 * 1024;
const MAX_REPORTED_ISSUES: usize = 8;
const PAUSE_POLL_INTERVAL: Duration = Duration::from_millis(25);
const UTF8_BOUNDARY_LOOKAHEAD: usize = 4;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FindNameMode {
    #[default]
    Glob,
    Regex,
}

impl FindNameMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Glob => "glob",
            Self::Regex => "regex",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FindSpec {
    pub start_dir: PathBuf,
    pub filename_pattern: String,
    pub name_mode: FindNameMode,
    pub case_sensitive: bool,
    pub content_pattern: Option<String>,
    pub whole_word: bool,
    pub ignored_directories: Vec<String>,
}

impl FindSpec {
    pub fn new(start_dir: PathBuf) -> Self {
        Self {
            start_dir,
            filename_pattern: String::new(),
            name_mode: FindNameMode::Glob,
            case_sensitive: false,
            content_pattern: None,
            whole_word: false,
            ignored_directories: Vec::new(),
        }
    }

    pub fn display_pattern(&self) -> &str {
        if self.filename_pattern.is_empty() {
            "*"
        } else {
            self.filename_pattern.as_str()
        }
    }

    pub fn validate(&self) -> Result<(), FindSearchError> {
        CompiledFindSpec::compile(self).map(|_| ())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FindSearchIssueKind {
    ReadDirectory,
    ReadDirectoryEntry,
    ReadFileType,
    ReadFile,
}

impl FindSearchIssueKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::ReadDirectory => "read directory",
            Self::ReadDirectoryEntry => "read directory entry",
            Self::ReadFileType => "read file type",
            Self::ReadFile => "read file",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FindSearchIssue {
    pub kind: FindSearchIssueKind,
    pub path: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FindSearchReport {
    pub matched_entries: usize,
    pub visited_entries: usize,
    pub visited_directories: usize,
    pub ignored_directories: usize,
    pub skipped_directories: usize,
    pub skipped_files: usize,
    pub issue_count: usize,
    pub issues: Vec<FindSearchIssue>,
    pub truncated: bool,
}

impl FindSearchReport {
    pub const fn is_partial(&self) -> bool {
        self.issue_count > 0
    }

    fn record_issue(&mut self, kind: FindSearchIssueKind, path: PathBuf, error: &io::Error) {
        self.issue_count = self.issue_count.saturating_add(1);
        if self.issues.len() < MAX_REPORTED_ISSUES {
            self.issues.push(FindSearchIssue {
                kind,
                path,
                message: error.to_string(),
            });
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FindSearchError {
    InvalidPattern {
        field: &'static str,
        message: String,
    },
    StartDirectory {
        path: PathBuf,
        message: String,
    },
    Canceled,
    ResultSinkDisconnected,
}

impl fmt::Display for FindSearchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPattern { field, message } => {
                write!(formatter, "invalid {field} pattern: {message}")
            }
            Self::StartDirectory { path, message } => write!(
                formatter,
                "cannot search starting directory {}: {message}",
                path.to_string_lossy()
            ),
            Self::Canceled => formatter.write_str(JOB_CANCELED_MESSAGE),
            Self::ResultSinkDisconnected => {
                formatter.write_str("background event channel disconnected")
            }
        }
    }
}

impl std::error::Error for FindSearchError {}

pub fn run_find_entries<F>(
    spec: &FindSpec,
    max_results: usize,
    cancel_flag: &AtomicBool,
    pause_flag: &AtomicBool,
    emit_chunk: F,
) -> Result<FindSearchReport, FindSearchError>
where
    F: FnMut(Vec<FindResultEntry>) -> bool,
{
    stream_find_entries(
        spec,
        max_results,
        cancel_flag,
        pause_flag,
        DEFAULT_FIND_CHUNK_SIZE,
        emit_chunk,
    )
}

pub fn stream_find_entries<F>(
    spec: &FindSpec,
    max_results: usize,
    cancel_flag: &AtomicBool,
    pause_flag: &AtomicBool,
    chunk_size: usize,
    mut emit_chunk: F,
) -> Result<FindSearchReport, FindSearchError>
where
    F: FnMut(Vec<FindResultEntry>) -> bool,
{
    let compiled = CompiledFindSpec::compile(spec)?;
    let chunk_size = chunk_size.max(1);
    let mut report = FindSearchReport::default();
    let mut pending = Vec::with_capacity(chunk_size.min(max_results));
    let mut stack = vec![spec.start_dir.clone()];
    let mut is_root = true;

    'search: while let Some(directory) = stack.pop() {
        wait_for_resume(cancel_flag, pause_flag)?;
        let read_dir = match fs::read_dir(&directory) {
            Ok(read_dir) => read_dir,
            Err(error) if is_root => {
                return Err(FindSearchError::StartDirectory {
                    path: directory,
                    message: error.to_string(),
                });
            }
            Err(error) => {
                report.skipped_directories = report.skipped_directories.saturating_add(1);
                report.record_issue(FindSearchIssueKind::ReadDirectory, directory, &error);
                continue;
            }
        };
        is_root = false;
        report.visited_directories = report.visited_directories.saturating_add(1);

        let mut entries = Vec::new();
        for entry_result in read_dir {
            wait_for_resume(cancel_flag, pause_flag)?;
            match entry_result {
                Ok(entry) => entries.push((entry.file_name(), entry)),
                Err(error) => report.record_issue(
                    FindSearchIssueKind::ReadDirectoryEntry,
                    directory.clone(),
                    &error,
                ),
            }
        }
        entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));

        let mut child_directories = Vec::new();
        for (name, entry) in entries {
            wait_for_resume(cancel_flag, pause_flag)?;
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    report.record_issue(FindSearchIssueKind::ReadFileType, path, &error);
                    continue;
                }
            };
            report.visited_entries = report.visited_entries.saturating_add(1);

            let traversable_directory = file_type.is_dir();
            if traversable_directory && compiled.is_ignored_directory(&path, &spec.start_dir) {
                report.ignored_directories = report.ignored_directories.saturating_add(1);
                continue;
            }

            let target_is_directory = traversable_directory
                || (file_type.is_symlink()
                    && fs::metadata(&path).is_ok_and(|metadata| metadata.is_dir()));
            let name_matches = compiled.name_matches(&name);
            let content_matches = if name_matches {
                match compiled.content_matches(
                    &path,
                    &file_type,
                    target_is_directory,
                    cancel_flag,
                    pause_flag,
                ) {
                    Ok(matches) => matches,
                    Err(ContentSearchError::Control(error)) => return Err(error),
                    Err(ContentSearchError::Io(error)) => {
                        report.skipped_files = report.skipped_files.saturating_add(1);
                        report.record_issue(FindSearchIssueKind::ReadFile, path.clone(), &error);
                        false
                    }
                }
            } else {
                false
            };

            if name_matches && content_matches {
                if report.matched_entries >= max_results {
                    report.truncated = true;
                    break 'search;
                }
                pending.push(FindResultEntry {
                    path: path.clone(),
                    is_dir: target_is_directory,
                });
                report.matched_entries = report.matched_entries.saturating_add(1);
                if pending.len() >= chunk_size && !emit_chunk(std::mem::take(&mut pending)) {
                    return Err(FindSearchError::ResultSinkDisconnected);
                }
            }

            if traversable_directory {
                child_directories.push(path);
            }
        }

        for child in child_directories.into_iter().rev() {
            stack.push(child);
        }
    }

    if !pending.is_empty() && !emit_chunk(pending) {
        return Err(FindSearchError::ResultSinkDisconnected);
    }
    Ok(report)
}

struct CompiledFindSpec {
    name: NameMatcher,
    content: Option<ContentMatcher>,
    ignored_directories: Vec<GlobMatcher>,
}

impl CompiledFindSpec {
    fn compile(spec: &FindSpec) -> Result<Self, FindSearchError> {
        let name = NameMatcher::compile(spec)?;
        let content = spec
            .content_pattern
            .as_deref()
            .filter(|pattern| !pattern.is_empty())
            .map(|pattern| ContentMatcher::compile(pattern, spec.case_sensitive, spec.whole_word))
            .transpose()?;
        let ignored_directories = spec
            .ignored_directories
            .iter()
            .filter_map(|pattern| {
                let trimmed = pattern.trim();
                (!trimmed.is_empty()).then_some(trimmed)
            })
            .map(|pattern| compile_glob(pattern, !spec.case_sensitive, "ignored-directory"))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            name,
            content,
            ignored_directories,
        })
    }

    fn name_matches(&self, name: &std::ffi::OsStr) -> bool {
        self.name.is_match(name)
    }

    fn is_ignored_directory(&self, path: &Path, root: &Path) -> bool {
        let basename = path.file_name().map(Path::new);
        let relative = path.strip_prefix(root).ok();
        self.ignored_directories.iter().any(|matcher| {
            basename.is_some_and(|name| matcher.is_match(name))
                || relative.is_some_and(|candidate| matcher.is_match(candidate))
        })
    }

    fn content_matches(
        &self,
        path: &Path,
        file_type: &fs::FileType,
        target_is_directory: bool,
        cancel_flag: &AtomicBool,
        pause_flag: &AtomicBool,
    ) -> Result<bool, ContentSearchError> {
        let Some(content) = &self.content else {
            return Ok(true);
        };
        if target_is_directory {
            return Ok(false);
        }
        if !(file_type.is_file()
            || file_type.is_symlink()
                && fs::metadata(path).is_ok_and(|metadata| metadata.is_file()))
        {
            return Ok(false);
        }
        content.is_match(path, cancel_flag, pause_flag)
    }
}

enum NameMatcher {
    Any,
    Glob(GlobMatcher),
    Regex(regex::Regex),
}

impl NameMatcher {
    fn compile(spec: &FindSpec) -> Result<Self, FindSearchError> {
        if spec.filename_pattern.is_empty() {
            return Ok(Self::Any);
        }
        match spec.name_mode {
            FindNameMode::Glob => {
                compile_glob(&spec.filename_pattern, !spec.case_sensitive, "filename")
                    .map(Self::Glob)
            }
            FindNameMode::Regex => RegexBuilder::new(&spec.filename_pattern)
                .case_insensitive(!spec.case_sensitive)
                .build()
                .map(Self::Regex)
                .map_err(|error| FindSearchError::InvalidPattern {
                    field: "filename",
                    message: error.to_string(),
                }),
        }
    }

    fn is_match(&self, name: &std::ffi::OsStr) -> bool {
        match self {
            Self::Any => true,
            Self::Glob(matcher) => matcher.is_match(Path::new(name)),
            Self::Regex(regex) => regex.is_match(&name.to_string_lossy()),
        }
    }
}

fn compile_glob(
    pattern: &str,
    case_insensitive: bool,
    field: &'static str,
) -> Result<GlobMatcher, FindSearchError> {
    GlobBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .literal_separator(false)
        .backslash_escape(true)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|error| FindSearchError::InvalidPattern {
            field,
            message: error.to_string(),
        })
}

struct ContentMatcher {
    regex: BytesRegex,
    continuation_regex: Option<BytesRegex>,
    overlap: usize,
    whole_word: bool,
}

impl ContentMatcher {
    fn compile(
        pattern: &str,
        case_sensitive: bool,
        whole_word: bool,
    ) -> Result<Self, FindSearchError> {
        let escaped = regex::escape(pattern);
        let (expression, continuation_expression) = if whole_word {
            let trailing_boundary = format!(r"{escaped}(?:$|(?u:\W))");
            (
                format!(r"(?:^|(?u:\W)){trailing_boundary}"),
                Some(format!(r"(?u:\W){trailing_boundary}")),
            )
        } else {
            (escaped, None)
        };
        let compile_regex = |expression: &str| {
            BytesRegexBuilder::new(expression)
                .case_insensitive(!case_sensitive)
                .unicode(true)
                .build()
                .map_err(|error| FindSearchError::InvalidPattern {
                    field: "content",
                    message: error.to_string(),
                })
        };
        let regex = compile_regex(&expression)?;
        let continuation_regex = continuation_expression
            .as_deref()
            .map(compile_regex)
            .transpose()?;
        let overlap = pattern
            .len()
            .saturating_mul(UTF8_BOUNDARY_LOOKAHEAD)
            .saturating_add(UTF8_BOUNDARY_LOOKAHEAD * 2);
        Ok(Self {
            regex,
            continuation_regex,
            overlap,
            whole_word,
        })
    }

    fn is_match(
        &self,
        path: &Path,
        cancel_flag: &AtomicBool,
        pause_flag: &AtomicBool,
    ) -> Result<bool, ContentSearchError> {
        let mut file = File::open(path).map_err(ContentSearchError::Io)?;
        let mut read_buffer = [0_u8; CONTENT_READ_BUFFER_SIZE];
        let mut window = Vec::with_capacity(CONTENT_READ_BUFFER_SIZE + self.overlap);
        let mut window_includes_file_start = true;

        loop {
            wait_for_resume(cancel_flag, pause_flag).map_err(ContentSearchError::Control)?;
            let read = file
                .read(&mut read_buffer)
                .map_err(ContentSearchError::Io)?;
            let eof = read == 0;
            if read > 0 {
                window.extend_from_slice(&read_buffer[..read]);
            }

            let stable_end = if eof || !self.whole_word {
                window.len()
            } else {
                window.len().saturating_sub(UTF8_BOUNDARY_LOOKAHEAD)
            };
            let regex = if window_includes_file_start {
                &self.regex
            } else {
                self.continuation_regex.as_ref().unwrap_or(&self.regex)
            };
            if regex
                .find_iter(&window)
                .any(|matched| matched.end() <= stable_end)
            {
                return Ok(true);
            }
            if eof {
                return Ok(false);
            }

            let keep_from = window.len().saturating_sub(self.overlap);
            window.drain(..keep_from);
            if keep_from > 0 {
                // `^` denotes only the start of the file. The retained overlap supplies
                // real boundary context for candidates that span subsequent reads.
                window_includes_file_start = false;
            }
        }
    }
}

enum ContentSearchError {
    Control(FindSearchError),
    Io(io::Error),
}

fn wait_for_resume(
    cancel_flag: &AtomicBool,
    pause_flag: &AtomicBool,
) -> Result<(), FindSearchError> {
    loop {
        if cancel_flag.load(Ordering::Relaxed) {
            return Err(FindSearchError::Canceled);
        }
        if !pause_flag.load(Ordering::Relaxed) {
            return Ok(());
        }
        thread::sleep(PAUSE_POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn whole_word_anchor_does_not_match_at_a_retained_window_boundary() {
        let matcher = ContentMatcher::compile("needle", true, true)
            .expect("whole-word matcher should compile");
        let retained_start = CONTENT_READ_BUFFER_SIZE
            .checked_sub(matcher.overlap)
            .expect("overlap should fit inside the read buffer");
        let mut content = vec![b'x'; retained_start];
        content.extend_from_slice(b"needle ");
        content.resize(CONTENT_READ_BUFFER_SIZE + 1, b'x');

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after the Unix epoch")
            .as_nanos();
        let path = env::temp_dir().join(format!("rc-whole-word-window-{stamp}"));
        fs::write(&path, content).expect("content fixture should be written");
        let cancel = AtomicBool::new(false);
        let pause = AtomicBool::new(false);

        let matched = matcher
            .is_match(&path, &cancel, &pause)
            .unwrap_or_else(|_| panic!("content fixture should be searchable"));

        assert!(
            !matched,
            "the word character before the retained window must prevent a whole-word match"
        );
        fs::remove_file(path).expect("content fixture should be removable");
    }
}
