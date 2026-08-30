use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::fs;
#[cfg(unix)]
use std::io::{self, Read};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(unix)]
use std::sync::{Arc, mpsc};
#[cfg(unix)]
use std::thread;
use std::time::{Duration, Instant};
#[cfg(unix)]
use std::time::{SystemTime, UNIX_EPOCH};

use crate::backend::is_executable_file;
#[cfg(unix)]
use crate::sanitize_display_lossy;
use crate::{ResolvedShell, ShellDialect};

pub const COMPLETION_STDOUT_LIMIT_BYTES: usize = 512 * 1024;
pub const COMPLETION_STDERR_LIMIT_BYTES: usize = 128 * 1024;
pub const COMPLETION_CANDIDATE_LIMIT: usize = 512;
pub const COMPLETION_FIELD_LIMIT_BYTES: usize = 16 * 1024;
pub const COMPLETION_RETAINED_LIMIT_BYTES: usize = 1024 * 1024;
pub const COMPLETION_DEADLINE: Duration = Duration::from_secs(5);
#[cfg(unix)]
const COMPLETION_POLL_INTERVAL: Duration = Duration::from_millis(20);
#[cfg(unix)]
const COMPLETION_CLEANUP_RESERVE: Duration = Duration::from_millis(250);
#[cfg(unix)]
const FISH_COMPLETION_SCRIPT: &str = r#"printf '%s\n' "$argv[1]"; for item in (complete --do-complete --escape "$argv[3]"); printf '%s%s\n' "$argv[2]" "$item"; end; printf '%s\n' "$argv[4]""#;
#[cfg(unix)]
static COMPLETION_NONCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionEdit {
    pub start_byte: usize,
    pub end_byte: usize,
    pub replacement: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionCandidate {
    pub edit: CompletionEdit,
    pub display: String,
    pub description: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionProvider {
    Fish,
    Generic,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TokenRole {
    Argument,
    CommandName,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionRequest {
    pub activation_id: u64,
    pub request_id: u64,
    pub buffer_revision: u64,
    pub buffer: String,
    pub cursor_byte: usize,
    pub cwd: PathBuf,
    pub shell: ResolvedShell,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionResponse {
    pub activation_id: u64,
    pub request_id: u64,
    pub buffer_revision: u64,
    pub original_buffer: String,
    pub outcome: CompletionOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompletionOutcome {
    Candidates {
        provider: CompletionProvider,
        candidates: Vec<CompletionCandidate>,
        notice: Option<String>,
    },
    /// A successful provider response with no candidates is authoritative.
    Empty(CompletionProvider),
    Unavailable(String),
    Canceled,
}

pub fn complete_request(request: &CompletionRequest, cancel: &AtomicBool) -> CompletionResponse {
    let outcome = match request.shell.dialect {
        ShellDialect::Fish => complete_with_fish_then_generic(request, cancel),
        ShellDialect::Posix => generic_completion(request, cancel),
    };
    CompletionResponse {
        activation_id: request.activation_id,
        request_id: request.request_id,
        buffer_revision: request.buffer_revision,
        original_buffer: request.buffer.clone(),
        outcome,
    }
}

fn complete_with_fish_then_generic(
    request: &CompletionRequest,
    cancel: &AtomicBool,
) -> CompletionOutcome {
    let Some((start_byte, end_byte)) =
        simple_token_range(&request.buffer, request.cursor_byte, true, false)
    else {
        return generic_completion(request, cancel);
    };
    let prefix = &request.buffer[start_byte..request.cursor_byte];
    let command_position = request.buffer[..start_byte].trim().is_empty();
    let local_candidates = if command_position && !prefix.contains('/') && !prefix.starts_with('~')
    {
        cwd_command_candidates(
            prefix,
            &request.cwd,
            start_byte,
            end_byte,
            request.shell.dialect,
            cancel,
            Instant::now(),
        )
    } else {
        Vec::new()
    };
    if cancel.load(Ordering::Relaxed) {
        return CompletionOutcome::Canceled;
    }
    match run_fish_completion(request, start_byte, end_byte, cancel) {
        Ok(candidates) => {
            let candidates = merge_prioritized_candidates(local_candidates, candidates);
            if candidates.is_empty() {
                CompletionOutcome::Empty(CompletionProvider::Fish)
            } else {
                CompletionOutcome::Candidates {
                    provider: CompletionProvider::Fish,
                    candidates,
                    notice: None,
                }
            }
        }
        Err(FishCompletionError::Canceled) => CompletionOutcome::Canceled,
        Err(FishCompletionError::Failed(error)) => {
            if cancel.load(Ordering::Relaxed) {
                CompletionOutcome::Canceled
            } else {
                match generic_completion(request, cancel) {
                    CompletionOutcome::Candidates {
                        provider,
                        candidates,
                        ..
                    } => CompletionOutcome::Candidates {
                        provider,
                        candidates,
                        notice: Some(format!("fish completion unavailable: {error}")),
                    },
                    CompletionOutcome::Empty(provider) => CompletionOutcome::Empty(provider),
                    CompletionOutcome::Canceled => CompletionOutcome::Canceled,
                    CompletionOutcome::Unavailable(_) => CompletionOutcome::Unavailable(format!(
                        "fish completion unavailable: {error}"
                    )),
                }
            }
        }
    }
}

#[cfg_attr(not(unix), allow(dead_code))]
enum FishCompletionError {
    Canceled,
    Failed(String),
}

fn run_fish_completion(
    request: &CompletionRequest,
    start_byte: usize,
    end_byte: usize,
    cancel: &AtomicBool,
) -> Result<Vec<CompletionCandidate>, FishCompletionError> {
    #[cfg(not(unix))]
    {
        let _ = (request, start_byte, end_byte, cancel);
        return Err(FishCompletionError::Failed(String::from(
            "fish completion is unavailable on this platform",
        )));
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;

        let started = Instant::now();
        let deadline = started + COMPLETION_DEADLINE;
        let execution_deadline = deadline - COMPLETION_CLEANUP_RESERVE;
        if cancel.load(Ordering::Relaxed) {
            return Err(FishCompletionError::Canceled);
        }
        let nonce = completion_nonce();
        let start_marker = format!("__RC_COMPLETE_START_{nonce}__");
        let record_marker = format!("__RC_COMPLETE_RECORD_{nonce}__");
        let end_marker = format!("__RC_COMPLETE_END_{nonce}__");
        let mut child = Command::new(&request.shell.program)
            .arg("--private")
            .arg("-c")
            .arg(FISH_COMPLETION_SCRIPT)
            .arg("--")
            .arg(&start_marker)
            .arg(&record_marker)
            .arg(&request.buffer)
            .arg(&end_marker)
            .current_dir(&request.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|error| FishCompletionError::Failed(error.to_string()))?;
        let process_group = match i32::try_from(child.id()) {
            Ok(process_group) => process_group,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(FishCompletionError::Failed(String::from(
                    "fish PID exceeds pid_t",
                )));
            }
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                let error = abort_fish_completion(&mut child, process_group, deadline).err();
                return Err(FishCompletionError::Failed(append_cleanup_error(
                    "fish stdout was not captured",
                    error,
                )));
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                let error = abort_fish_completion(&mut child, process_group, deadline).err();
                return Err(FishCompletionError::Failed(append_cleanup_error(
                    "fish stderr was not captured",
                    error,
                )));
            }
        };
        let overflow = Arc::new(AtomicBool::new(false));
        let stdout_overflow = Arc::clone(&overflow);
        let stdout_reader =
            LimitedReader::spawn(stdout, COMPLETION_STDOUT_LIMIT_BYTES, stdout_overflow);
        let stderr_overflow = Arc::clone(&overflow);
        let stderr_reader =
            LimitedReader::spawn(stderr, COMPLETION_STDERR_LIMIT_BYTES, stderr_overflow);

        let status = loop {
            if cancel.load(Ordering::Relaxed) {
                return match abort_fish_completion(&mut child, process_group, deadline) {
                    Ok(()) => Err(FishCompletionError::Canceled),
                    Err(error) => Err(FishCompletionError::Failed(format!(
                        "canceled fish completion cleanup failed: {error}"
                    ))),
                };
            }
            if overflow.load(Ordering::Relaxed) {
                let error = abort_fish_completion(&mut child, process_group, deadline).err();
                return Err(FishCompletionError::Failed(append_cleanup_error(
                    "completion helper output limit exceeded",
                    error,
                )));
            }
            if Instant::now() >= execution_deadline {
                let error = abort_fish_completion(&mut child, process_group, deadline).err();
                return Err(FishCompletionError::Failed(append_cleanup_error(
                    "completion helper timed out",
                    error,
                )));
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => thread::sleep(COMPLETION_POLL_INTERVAL),
                Err(error) => {
                    let cleanup = abort_fish_completion(&mut child, process_group, deadline).err();
                    return Err(FishCompletionError::Failed(append_cleanup_error(
                        &error.to_string(),
                        cleanup,
                    )));
                }
            }
        };

        // Completion definitions must not leave descendants holding the capture pipes.
        terminate_completion_group(process_group)
            .and_then(|()| wait_for_completion_group_exit(process_group, deadline))
            .map_err(|error| FishCompletionError::Failed(error.to_string()))?;
        let stdout = stdout_reader.finish(deadline, "stdout")?;
        let stderr = stderr_reader.finish(deadline, "stderr")?;
        if overflow.load(Ordering::Relaxed) {
            return Err(FishCompletionError::Failed(String::from(
                "completion helper output limit exceeded",
            )));
        }
        if !status.success() {
            let stderr = sanitize_display_lossy(&stderr);
            return Err(FishCompletionError::Failed(if stderr.is_empty() {
                format!("fish exited with {status}")
            } else {
                format!("fish exited with {status}: {stderr}")
            }));
        }
        parse_fish_output(
            &stdout,
            &start_marker,
            &record_marker,
            &end_marker,
            start_byte,
            end_byte,
        )
        .map_err(FishCompletionError::Failed)
    }
}

#[cfg(unix)]
struct LimitedReader {
    result_rx: mpsc::Receiver<io::Result<Vec<u8>>>,
    handle: Option<thread::JoinHandle<()>>,
}

#[cfg(unix)]
impl LimitedReader {
    fn spawn(reader: impl Read + Send + 'static, limit: usize, overflow: Arc<AtomicBool>) -> Self {
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        let handle = thread::spawn(move || {
            let _ = result_tx.send(read_limited(reader, limit, overflow));
        });
        Self {
            result_rx,
            handle: Some(handle),
        }
    }

    fn finish(mut self, deadline: Instant, stream: &str) -> Result<Vec<u8>, FishCompletionError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let result = self.result_rx.recv_timeout(remaining);
        // The coordinator enforces the user-visible deadline around this whole completion
        // worker. Keep ownership of the capture thread until its pipe actually closes: dropping
        // a JoinHandle here would let an escaped or uninterruptible helper accumulate detached
        // reader threads across later completion requests.
        self.join(stream)?;
        let result = result.map_err(|error| {
            let detail = match error {
                mpsc::RecvTimeoutError::Timeout => "did not close before the safety deadline",
                mpsc::RecvTimeoutError::Disconnected => "reader disconnected",
            };
            FishCompletionError::Failed(format!("fish {stream} {detail}"))
        })?;
        result.map_err(|error| FishCompletionError::Failed(error.to_string()))
    }

    fn join(&mut self, stream: &str) -> Result<(), FishCompletionError> {
        if let Some(handle) = self.handle.take() {
            handle.join().map_err(|_| {
                FishCompletionError::Failed(format!("fish {stream} reader panicked"))
            })?;
        }
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for LimitedReader {
    fn drop(&mut self) {
        // Cancellation and process-cleanup failures can bypass finish(). Joining here may keep
        // the outer completion worker alive, which is intentional: the coordinator retains that
        // one worker and disables the lane instead of allowing detached readers to multiply.
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(unix)]
fn read_limited(
    mut reader: impl Read,
    limit: usize,
    overflow: Arc<AtomicBool>,
) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            return Ok(output);
        }
        if count > limit.saturating_sub(output.len()) {
            overflow.store(true, Ordering::Relaxed);
            return Err(io::Error::other("completion output limit exceeded"));
        }
        output.extend_from_slice(&chunk[..count]);
    }
}

#[cfg(unix)]
fn terminate_completion_group(process_group: i32) -> io::Result<()> {
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    match killpg(Pid::from_raw(process_group), Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(io::Error::from(error)),
    }
}

#[cfg(unix)]
fn abort_fish_completion(
    child: &mut std::process::Child,
    process_group: i32,
    deadline: Instant,
) -> io::Result<()> {
    let mut errors = Vec::new();
    if let Err(error) = terminate_completion_group(process_group) {
        errors.push(format!("failed to signal process group: {error}"));
        if let Err(error) = child.kill() {
            errors.push(format!("failed to kill fish leader: {error}"));
        }
    }
    if let Err(error) = wait_for_completion_leader_exit(child, deadline) {
        errors.push(error.to_string());
    }
    if let Err(error) = wait_for_completion_group_exit(process_group, deadline) {
        errors.push(error.to_string());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(errors.join("; ")))
    }
}

#[cfg(unix)]
fn wait_for_completion_leader_exit(
    child: &mut std::process::Child,
    deadline: Instant,
) -> io::Result<()> {
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {}
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "fish completion leader was not reaped before the safety deadline",
            ));
        }
        thread::sleep(COMPLETION_POLL_INTERVAL);
    }
}

#[cfg(unix)]
fn append_cleanup_error(message: &str, cleanup: Option<io::Error>) -> String {
    cleanup.map_or_else(
        || message.to_string(),
        |error| format!("{message}; cleanup failed: {error}"),
    )
}

#[cfg(unix)]
fn wait_for_completion_group_exit(process_group: i32, deadline: Instant) -> io::Result<()> {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    let target = Pid::from_raw(-process_group);
    loop {
        match kill(target, None) {
            Err(Errno::ESRCH) => return Ok(()),
            Ok(()) | Err(Errno::EPERM) => {}
            Err(error) => return Err(io::Error::from(error)),
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "fish completion process group did not exit before the safety deadline",
            ));
        }
        thread::sleep(COMPLETION_POLL_INTERVAL);
    }
}

#[cfg(unix)]
fn completion_nonce() -> String {
    let counter = COMPLETION_NONCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:x}_{counter:x}_{nanos:x}", std::process::id())
}

#[cfg(any(unix, test))]
fn parse_fish_output(
    output: &[u8],
    start_marker: &str,
    record_marker: &str,
    end_marker: &str,
    start_byte: usize,
    end_byte: usize,
) -> Result<Vec<CompletionCandidate>, String> {
    let output = std::str::from_utf8(output)
        .map_err(|_| String::from("fish completion output was not UTF-8"))?;
    let mut framed = false;
    let mut finished = false;
    let mut candidates = CandidateCollector::default();
    for raw_line in output.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if !framed {
            if line == start_marker {
                framed = true;
            }
            continue;
        }
        if line == end_marker {
            finished = true;
            break;
        }
        let Some(record) = line.strip_prefix(record_marker) else {
            continue;
        };
        let (replacement, description) = record
            .split_once('\t')
            .map_or((record, None), |(candidate, description)| {
                (candidate, Some(description))
            });
        validate_candidate_field(replacement)?;
        if let Some(description) = description {
            validate_candidate_field(description)?;
        }
        candidates.push(CompletionCandidate {
            edit: CompletionEdit {
                start_byte,
                end_byte,
                replacement: replacement.to_string(),
            },
            display: replacement.to_string(),
            description: description.map(ToString::to_string),
        });
    }
    if !framed || !finished {
        return Err(String::from(
            "fish completion protocol markers were missing",
        ));
    }
    Ok(candidates.into_candidates())
}

fn validate_candidate_field(value: &str) -> Result<(), String> {
    if value.len() > COMPLETION_FIELD_LIMIT_BYTES {
        return Err(String::from("completion candidate field limit exceeded"));
    }
    if value.chars().any(is_candidate_control) {
        return Err(String::from(
            "completion candidate contains invalid controls",
        ));
    }
    Ok(())
}

fn is_candidate_control(character: char) -> bool {
    matches!(character as u32, 0x00..=0x1f | 0x7f..=0x9f)
}

#[derive(Default)]
struct CandidateCollector {
    candidates: Vec<CompletionCandidate>,
    replacements: HashSet<String>,
    retained_bytes: usize,
}

impl CandidateCollector {
    fn push(&mut self, candidate: CompletionCandidate) {
        if self.candidates.len() >= COMPLETION_CANDIDATE_LIMIT
            || self.replacements.contains(&candidate.edit.replacement)
        {
            return;
        }
        let retained_bytes = candidate
            .edit
            .replacement
            .len()
            .saturating_add(candidate.display.len())
            .saturating_add(candidate.description.as_ref().map_or(0, String::len));
        if self.retained_bytes.saturating_add(retained_bytes) > COMPLETION_RETAINED_LIMIT_BYTES {
            return;
        }
        self.retained_bytes = self.retained_bytes.saturating_add(retained_bytes);
        self.replacements.insert(candidate.edit.replacement.clone());
        self.candidates.push(candidate);
    }

    fn extend(&mut self, candidates: impl IntoIterator<Item = CompletionCandidate>) {
        for candidate in candidates {
            self.push(candidate);
        }
    }

    fn into_candidates(self) -> Vec<CompletionCandidate> {
        self.candidates
    }
}

fn merge_sorted_candidate_sources(
    sources: impl IntoIterator<Item = Vec<CompletionCandidate>>,
) -> Vec<CompletionCandidate> {
    let mut collector = CandidateCollector::default();
    for mut candidates in sources {
        candidates.sort_by(|left, right| {
            left.display
                .cmp(&right.display)
                .then_with(|| left.edit.replacement.cmp(&right.edit.replacement))
        });
        collector.extend(candidates);
    }
    collector.into_candidates()
}

fn merge_prioritized_candidates(
    mut prioritized: Vec<CompletionCandidate>,
    remaining: Vec<CompletionCandidate>,
) -> Vec<CompletionCandidate> {
    prioritized.sort_by(|left, right| {
        left.display
            .cmp(&right.display)
            .then_with(|| left.edit.replacement.cmp(&right.edit.replacement))
    });
    let mut collector = CandidateCollector::default();
    collector.extend(prioritized);
    collector.extend(remaining);
    collector.into_candidates()
}

fn generic_completion(request: &CompletionRequest, cancel: &AtomicBool) -> CompletionOutcome {
    let started = Instant::now();
    if cancel.load(Ordering::Relaxed) {
        return CompletionOutcome::Canceled;
    }
    let Some((start_byte, end_byte)) =
        simple_token_range(&request.buffer, request.cursor_byte, false, true)
    else {
        return CompletionOutcome::Empty(CompletionProvider::Generic);
    };
    let prefix = &request.buffer[start_byte..request.cursor_byte];
    let command_position = request.buffer[..start_byte].trim().is_empty();
    let candidates = if let Some(variable_prefix) = prefix.strip_prefix('$')
        && !variable_prefix.contains('/')
    {
        merge_sorted_candidate_sources([environment_candidates(
            variable_prefix,
            start_byte,
            end_byte,
            request.shell.dialect,
            cancel,
            started,
        )])
    } else if command_position && !prefix.contains('/') && !prefix.starts_with('~') {
        merge_sorted_candidate_sources([
            cwd_command_candidates(
                prefix,
                &request.cwd,
                start_byte,
                end_byte,
                request.shell.dialect,
                cancel,
                started,
            ),
            executable_candidates(
                prefix,
                &request.cwd,
                start_byte,
                end_byte,
                request.shell.dialect,
                cancel,
                started,
            ),
        ])
    } else {
        merge_sorted_candidate_sources([filesystem_candidates(
            prefix,
            &request.cwd,
            start_byte,
            end_byte,
            request.shell.dialect,
            cancel,
            started,
        )])
    };

    if cancel.load(Ordering::Relaxed) {
        return CompletionOutcome::Canceled;
    }
    if started.elapsed() >= COMPLETION_DEADLINE {
        return CompletionOutcome::Unavailable(String::from("generic completion timed out"));
    }
    if candidates.is_empty() {
        CompletionOutcome::Empty(CompletionProvider::Generic)
    } else {
        CompletionOutcome::Candidates {
            provider: CompletionProvider::Generic,
            candidates,
            notice: None,
        }
    }
}

fn simple_token_range(
    buffer: &str,
    cursor_byte: usize,
    require_end: bool,
    allow_environment_prefix: bool,
) -> Option<(usize, usize)> {
    if cursor_byte > buffer.len() || !buffer.is_char_boundary(cursor_byte) {
        return None;
    }
    if require_end && cursor_byte != buffer.len() {
        return None;
    }
    let mut start = 0;
    for (byte, character) in buffer.char_indices() {
        if byte >= cursor_byte {
            break;
        }
        if matches!(character, ' ' | '\t') {
            start = byte + character.len_utf8();
        } else if ambiguous_completion_character(character)
            && !(allow_environment_prefix && character == '$' && byte == start)
        {
            return None;
        }
    }
    let mut end = buffer.len();
    for (relative, character) in buffer[cursor_byte..].char_indices() {
        if matches!(character, ' ' | '\t') {
            end = cursor_byte + relative;
            break;
        }
        if ambiguous_completion_character(character) {
            return None;
        }
    }
    Some((start, end))
}

fn ambiguous_completion_character(character: char) -> bool {
    matches!(
        character,
        '\'' | '"'
            | '\\'
            | ';'
            | '|'
            | '&'
            | '<'
            | '>'
            | '$'
            | '`'
            | '('
            | ')'
            | '{'
            | '}'
            | '*'
            | '?'
            | '['
            | ']'
            | '\n'
            | '\r'
            | '\0'
    ) || character.is_control()
}

#[allow(clippy::too_many_arguments)]
fn cwd_command_candidates(
    prefix: &str,
    cwd: &Path,
    start_byte: usize,
    end_byte: usize,
    dialect: ShellDialect,
    cancel: &AtomicBool,
    started: Instant,
) -> Vec<CompletionCandidate> {
    let Ok(entries) = fs::read_dir(cwd) else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        if completion_should_stop(cancel, started) || candidates.len() >= COMPLETION_CANDIDATE_LIMIT
        {
            break;
        }
        let Some(name) = entry.file_name().to_str().map(ToString::to_string) else {
            continue;
        };
        if !name.starts_with(prefix) {
            continue;
        }
        let path = entry.path();
        let is_directory = path.metadata().is_ok_and(|metadata| metadata.is_dir());
        let (raw_replacement, description) = if is_directory {
            (format!("{name}/"), String::from("directory"))
        } else if is_executable_file(&path) {
            (
                format!("./{name}"),
                String::from("current directory executable"),
            )
        } else {
            continue;
        };
        let Some(mut candidate) = make_candidate(
            raw_replacement,
            Some(description),
            start_byte,
            end_byte,
            dialect,
            TokenRole::CommandName,
        ) else {
            continue;
        };
        if !is_directory {
            candidate.display = name;
        }
        candidates.push(candidate);
    }
    candidates
}

fn executable_candidates(
    prefix: &str,
    cwd: &Path,
    start_byte: usize,
    end_byte: usize,
    dialect: ShellDialect,
    cancel: &AtomicBool,
    started: Instant,
) -> Vec<CompletionCandidate> {
    let Some(path) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    executable_candidates_from_path(
        prefix, cwd, start_byte, end_byte, dialect, &path, cancel, started,
    )
}

#[allow(clippy::too_many_arguments)]
fn executable_candidates_from_path(
    prefix: &str,
    cwd: &Path,
    start_byte: usize,
    end_byte: usize,
    dialect: ShellDialect,
    search_path: &OsStr,
    cancel: &AtomicBool,
    started: Instant,
) -> Vec<CompletionCandidate> {
    let mut found = BTreeMap::<String, PathBuf>::new();
    for directory in std::env::split_paths(search_path) {
        if completion_should_stop(cancel, started) || found.len() >= COMPLETION_CANDIDATE_LIMIT {
            break;
        }
        let directory = if directory.is_absolute() {
            directory
        } else {
            cwd.join(directory)
        };
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            if completion_should_stop(cancel, started) || found.len() >= COMPLETION_CANDIDATE_LIMIT
            {
                break;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.starts_with(prefix) && is_executable_file(&entry.path()) {
                found
                    .entry(name.to_string())
                    .or_insert_with(|| entry.path());
            }
        }
    }
    found
        .into_iter()
        .filter_map(|(name, path)| {
            let replacement = if dialect == ShellDialect::Fish
                && command_name_needs_disambiguation(&name, dialect)
            {
                path.to_str()?.to_owned()
            } else {
                name.clone()
            };
            let mut candidate = make_candidate(
                replacement,
                Some(path.to_string_lossy().into_owned()),
                start_byte,
                end_byte,
                dialect,
                TokenRole::CommandName,
            )?;
            candidate.display = name;
            Some(candidate)
        })
        .collect()
}

fn is_shell_identifier(value: &str) -> bool {
    let Some((first, rest)) = value.as_bytes().split_first() else {
        return false;
    };
    (first.is_ascii_alphabetic() || *first == b'_')
        && rest
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
}

fn environment_candidates(
    prefix: &str,
    start_byte: usize,
    end_byte: usize,
    dialect: ShellDialect,
    cancel: &AtomicBool,
    started: Instant,
) -> Vec<CompletionCandidate> {
    let mut names = HashSet::new();
    let mut candidates = Vec::new();
    for (name, _) in std::env::vars_os() {
        if completion_should_stop(cancel, started) || candidates.len() >= COMPLETION_CANDIDATE_LIMIT
        {
            break;
        }
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(prefix) && is_shell_identifier(name) && names.insert(name.to_string()) {
            let replacement = format!("${name}");
            if replacement.len() > COMPLETION_FIELD_LIMIT_BYTES {
                continue;
            }
            candidates.push(CompletionCandidate {
                edit: CompletionEdit {
                    start_byte,
                    end_byte,
                    replacement: replacement.clone(),
                },
                display: replacement,
                description: Some(format!("{} environment variable", dialect.label())),
            });
        }
    }
    candidates
}

fn filesystem_candidates(
    prefix: &str,
    cwd: &Path,
    start_byte: usize,
    end_byte: usize,
    dialect: ShellDialect,
    cancel: &AtomicBool,
    started: Instant,
) -> Vec<CompletionCandidate> {
    if prefix == "~" {
        return make_candidate(
            String::from("~/"),
            Some(String::from("home directory")),
            start_byte,
            end_byte,
            dialect,
            TokenRole::Argument,
        )
        .into_iter()
        .collect();
    }
    let expanded = if prefix.starts_with("~/") {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return Vec::new();
        };
        let suffix = prefix.strip_prefix("~/").unwrap_or_default();
        home.join(suffix.trim_start_matches('/'))
    } else {
        let path = PathBuf::from(prefix);
        if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        }
    };
    let directory_prefix =
        prefix.ends_with('/') || matches!(prefix.rsplit('/').next(), Some("." | ".."));
    let (directory, name_prefix) = if prefix.is_empty() {
        (cwd.to_path_buf(), "")
    } else if directory_prefix {
        (expanded.clone(), "")
    } else {
        (
            expanded.parent().unwrap_or(cwd).to_path_buf(),
            expanded
                .file_name()
                .and_then(OsStr::to_str)
                .unwrap_or_default(),
        )
    };
    let Ok(entries) = fs::read_dir(&directory) else {
        return Vec::new();
    };
    let input_parent = if directory_prefix {
        Path::new(prefix)
    } else {
        Path::new(prefix).parent().unwrap_or(Path::new(""))
    };
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        if completion_should_stop(cancel, started) || candidates.len() >= COMPLETION_CANDIDATE_LIMIT
        {
            break;
        }
        let Some(name) = entry.file_name().to_str().map(ToString::to_string) else {
            continue;
        };
        if !name.starts_with(name_prefix) {
            continue;
        }
        // DirEntry::file_type deliberately describes the symlink itself. Path::metadata follows
        // it, which lets an accepted directory symlink retain `/` and complete its children.
        let is_directory = entry
            .path()
            .metadata()
            .is_ok_and(|metadata| metadata.is_dir());
        let mut replacement = if Path::new(prefix).is_absolute() {
            entry.path().to_string_lossy().into_owned()
        } else {
            input_parent.join(&name).to_string_lossy().into_owned()
        };
        if is_directory {
            replacement.push('/');
        }
        if let Some(candidate) = make_candidate(
            replacement,
            is_directory.then(|| String::from("directory")),
            start_byte,
            end_byte,
            dialect,
            TokenRole::Argument,
        ) {
            candidates.push(candidate);
        }
    }
    candidates
}

fn completion_should_stop(cancel: &AtomicBool, started: Instant) -> bool {
    cancel.load(Ordering::Relaxed) || started.elapsed() >= COMPLETION_DEADLINE
}

fn make_candidate(
    raw_replacement: String,
    description: Option<String>,
    start_byte: usize,
    end_byte: usize,
    dialect: ShellDialect,
    role: TokenRole,
) -> Option<CompletionCandidate> {
    if validate_candidate_field(&raw_replacement).is_err()
        || description
            .as_deref()
            .is_some_and(|description| validate_candidate_field(description).is_err())
    {
        return None;
    }
    let replacement = quote_token_for_role(&raw_replacement, dialect, role);
    if replacement.len() > COMPLETION_FIELD_LIMIT_BYTES {
        return None;
    }
    Some(CompletionCandidate {
        edit: CompletionEdit {
            start_byte,
            end_byte,
            replacement,
        },
        display: raw_replacement,
        description,
    })
}

/// Quotes a completion argument while preserving a leading `~` home expansion.
pub fn quote_token(value: &str, dialect: ShellDialect) -> String {
    quote_token_for_role(value, dialect, TokenRole::Argument)
}

/// Quotes shell data literally, without preserving completion-oriented home expansion.
pub fn quote_literal_token(value: &str, dialect: ShellDialect) -> String {
    quote_token_without_home_prefix(value, dialect, TokenRole::Argument)
}

fn quote_token_for_role(value: &str, dialect: ShellDialect, role: TokenRole) -> String {
    if role == TokenRole::Argument {
        if value == "~" {
            return String::from("~");
        }
        if let Some(suffix) = value.strip_prefix("~/") {
            if suffix.is_empty() {
                return String::from("~/");
            }
            return format!(
                "~/{}",
                quote_token_without_home_prefix(suffix, dialect, role)
            );
        }
    }
    quote_token_without_home_prefix(value, dialect, role)
}

fn quote_token_without_home_prefix(value: &str, dialect: ShellDialect, role: TokenRole) -> String {
    let special_prefix =
        value.starts_with('~') || (dialect == ShellDialect::Fish && value.starts_with('%'));
    let shell_syntax =
        role == TokenRole::CommandName && command_name_needs_disambiguation(value, dialect);
    let safe_unquoted = !value.is_empty()
        && !special_prefix
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || "_@%+=:,./~-".contains(character)
        });
    if safe_unquoted && !shell_syntax {
        return value.to_string();
    }
    match dialect {
        ShellDialect::Posix => format!("'{}'", value.replace('\'', "'\\''")),
        ShellDialect::Fish => format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'")),
    }
}

fn command_name_needs_disambiguation(value: &str, dialect: ShellDialect) -> bool {
    value.contains('=')
        || match dialect {
            ShellDialect::Posix => matches!(
                value,
                "!" | "{"
                    | "}"
                    | "case"
                    | "do"
                    | "done"
                    | "elif"
                    | "else"
                    | "esac"
                    | "fi"
                    | "for"
                    | "if"
                    | "in"
                    | "then"
                    | "until"
                    | "while"
                    | "coproc"
                    | "function"
                    | "select"
                    | "time"
                    | "foreach"
                    | "repeat"
            ),
            ShellDialect::Fish => matches!(
                value,
                "and"
                    | "begin"
                    | "break"
                    | "case"
                    | "command"
                    | "continue"
                    | "else"
                    | "end"
                    | "exec"
                    | "for"
                    | "function"
                    | "if"
                    | "not"
                    | "or"
                    | "return"
                    | "switch"
                    | "time"
                    | "while"
            ),
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::time::Duration;

    fn shell(dialect: ShellDialect) -> ResolvedShell {
        ResolvedShell {
            program: PathBuf::from(if dialect == ShellDialect::Fish {
                "fish"
            } else {
                "/bin/sh"
            }),
            dialect,
            argument_template: vec!["-c".into(), "{command}".into()],
            identity: dialect.label().to_string(),
        }
    }

    #[cfg(unix)]
    fn completion_temp_dir(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "rc-completion-{label}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("completion temp directory should be creatable");
        root
    }

    #[test]
    fn token_scanner_supports_each_utf8_cursor_boundary() {
        let input = "echo héllo";
        for cursor in 0..=input.len() {
            let result = simple_token_range(input, cursor, false, true);
            if input.is_char_boundary(cursor) {
                assert!(result.is_some(), "cursor {cursor}");
            } else {
                assert!(result.is_none(), "cursor {cursor}");
            }
        }
    }

    #[test]
    fn token_scanner_declines_quoted_escaped_and_operator_input() {
        for input in ["echo 'a", "echo a\\ b", "echo a | b", "echo $(pwd)"] {
            assert_eq!(
                simple_token_range(input, input.len(), false, true),
                None,
                "{input}"
            );
        }
    }

    #[test]
    fn fish_protocol_ignores_unframed_noise() {
        let output = b"config noise\nSTART\nRECORDcheckout\tbranch\nEND\nmore noise\n";
        let candidates = parse_fish_output(output, "START", "RECORD", "END", 4, 7)
            .expect("framed output should parse");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].edit.replacement, "checkout");
        assert_eq!(candidates[0].edit.start_byte, 4);
    }

    #[test]
    fn fish_protocol_rejects_controls_and_truncates_bounded_data() {
        assert!(
            parse_fish_output(
                b"START\nRECORDbad\x1b\nEND\n",
                "START",
                "RECORD",
                "END",
                0,
                0
            )
            .is_err()
        );

        let records = (0..=COMPLETION_CANDIDATE_LIMIT)
            .map(|index| format!("RECORDcandidate-{index}\n"))
            .collect::<String>();
        let too_many = format!("START\n{records}END\n");
        let candidates = parse_fish_output(too_many.as_bytes(), "START", "RECORD", "END", 0, 0)
            .expect("candidate overflow should retain the highest-priority prefix");
        assert_eq!(candidates.len(), COMPLETION_CANDIDATE_LIMIT);
        assert_eq!(candidates[0].display, "candidate-0");

        let records = (0..33)
            .map(|index| {
                let field = format!(
                    "{index:04}{}",
                    "x".repeat(COMPLETION_FIELD_LIMIT_BYTES.saturating_sub(4))
                );
                format!("RECORD{field}\n")
            })
            .collect::<String>();
        let retained = format!("START\n{records}END\n");
        let candidates = parse_fish_output(retained.as_bytes(), "START", "RECORD", "END", 0, 0)
            .expect("retained-data overflow should be truncated");
        let retained_bytes = candidates
            .iter()
            .map(|candidate| candidate.edit.replacement.len() + candidate.display.len())
            .sum::<usize>();
        assert_eq!(candidates.len(), 32);
        assert!(retained_bytes <= COMPLETION_RETAINED_LIMIT_BYTES);
    }

    #[test]
    fn prioritized_merge_keeps_local_candidates_ahead_of_provider_overflow() {
        let candidate = |replacement: String, display: String| CompletionCandidate {
            edit: CompletionEdit {
                start_byte: 0,
                end_byte: 0,
                replacement,
            },
            display,
            description: None,
        };
        let local = candidate(String::from("local/"), String::from("local/"));
        let provider = (0..COMPLETION_CANDIDATE_LIMIT)
            .map(|index| candidate(format!("provider-{index}"), format!("provider-{index}")))
            .collect();

        let candidates = merge_prioritized_candidates(vec![local], provider);

        assert_eq!(candidates.len(), COMPLETION_CANDIDATE_LIMIT);
        assert_eq!(candidates[0].edit.replacement, "local/");
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.edit.replacement != "provider-511")
        );
    }

    #[test]
    fn generic_completion_rejects_stale_utf8_offsets() {
        let request = CompletionRequest {
            activation_id: 1,
            request_id: 2,
            buffer_revision: 3,
            buffer: String::from("é"),
            cursor_byte: 1,
            cwd: PathBuf::from("."),
            shell: shell(ShellDialect::Posix),
        };
        assert_eq!(
            generic_completion(&request, &AtomicBool::new(false)),
            CompletionOutcome::Empty(CompletionProvider::Generic)
        );
    }

    #[test]
    fn generic_completion_recognizes_environment_prefix_in_command_position() {
        let request = CompletionRequest {
            activation_id: 1,
            request_id: 2,
            buffer_revision: 3,
            buffer: String::from("$"),
            cursor_byte: 1,
            cwd: PathBuf::from("."),
            shell: shell(ShellDialect::Posix),
        };
        let CompletionOutcome::Candidates { candidates, .. } =
            generic_completion(&request, &AtomicBool::new(false))
        else {
            panic!("the test process should expose at least one environment variable");
        };
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.edit.replacement.starts_with('$'))
        );
    }

    #[cfg(unix)]
    #[test]
    fn generic_command_completion_prioritizes_valid_cwd_paths() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = completion_temp_dir("cwd-command-priority");
        fs::create_dir(root.join("zz-local-directory"))
            .expect("local directory fixture should be creatable");
        let executable = root.join("zz-local-executable");
        fs::write(&executable, "#!/bin/sh\n").expect("local executable fixture should be writable");
        let mut permissions = executable
            .metadata()
            .expect("local executable metadata")
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions)
            .expect("local executable should be executable");
        fs::write(root.join("zz-local-regular-file"), "not executable")
            .expect("regular file fixture should be writable");

        let request = CompletionRequest {
            activation_id: 1,
            request_id: 2,
            buffer_revision: 3,
            buffer: String::new(),
            cursor_byte: 0,
            cwd: root.clone(),
            shell: shell(ShellDialect::Posix),
        };
        let CompletionOutcome::Candidates { candidates, .. } =
            generic_completion(&request, &AtomicBool::new(false))
        else {
            panic!("the local command paths should produce candidates");
        };
        assert_eq!(candidates[0].display, "zz-local-directory/");
        assert_eq!(candidates[0].edit.replacement, "zz-local-directory/");
        assert_eq!(candidates[1].display, "zz-local-executable");
        assert_eq!(candidates[1].edit.replacement, "./zz-local-executable");
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.display != "zz-local-regular-file"),
            "a non-executable file is not a valid command"
        );

        fs::remove_dir_all(root).expect("completion temp directory should be removed");
    }

    #[test]
    fn generic_candidates_decline_terminal_controls() {
        assert!(
            make_candidate(
                String::from("unsafe\u{1b}[2J"),
                None,
                0,
                0,
                ShellDialect::Posix,
                TokenRole::Argument,
            )
            .is_none()
        );
    }

    #[test]
    fn shell_identifier_requires_a_portable_leading_character() {
        for valid in ["A", "_", "RC_VALUE", "value2"] {
            assert!(is_shell_identifier(valid), "{valid}");
        }
        for invalid in ["", "1ABC", "A-B", "é"] {
            assert!(!is_shell_identifier(invalid), "{invalid}");
        }
    }

    #[test]
    fn generic_home_completion_preserves_an_expandable_tilde_prefix() {
        let candidates = filesystem_candidates(
            "~",
            Path::new("."),
            0,
            1,
            ShellDialect::Posix,
            &AtomicBool::new(false),
            Instant::now(),
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].edit.replacement, "~/");
    }

    #[cfg(unix)]
    #[test]
    fn executable_completion_resolves_relative_path_entries_from_request_cwd() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = completion_temp_dir("relative-path");
        let bin = root.join("bin");
        fs::create_dir(&bin).expect("bin directory should be creatable");
        let executable = bin.join("rc-local-command");
        fs::write(&executable, "#!/bin/sh\n").expect("executable fixture should be writable");
        let mut permissions = executable
            .metadata()
            .expect("fixture metadata should be readable")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).expect("fixture should be executable");

        let candidates = executable_candidates_from_path(
            "rc-local",
            &root,
            0,
            8,
            ShellDialect::Posix,
            OsStr::new("bin"),
            &AtomicBool::new(false),
            Instant::now(),
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].edit.replacement, "rc-local-command");
        assert_eq!(
            candidates[0].description.as_deref(),
            Some(executable.to_string_lossy().as_ref())
        );

        fs::remove_dir_all(root).expect("completion temp directory should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn executable_completion_quotes_command_position_syntax() {
        use std::os::unix::fs::PermissionsExt as _;

        const FISH_COMMAND_WORDS: [&str; 18] = [
            "and", "begin", "break", "case", "command", "continue", "else", "end", "exec", "for",
            "function", "if", "not", "or", "return", "switch", "time", "while",
        ];
        let root = completion_temp_dir("command-syntax");
        for name in FISH_COMMAND_WORDS.into_iter().chain(["RC_MODE=value", "~"]) {
            let executable = root.join(name);
            fs::write(&executable, "#!/bin/sh\n").expect("executable fixture should be writable");
            let mut permissions = executable
                .metadata()
                .expect("fixture metadata should be readable")
                .permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(&executable, permissions).expect("fixture should be executable");
        }

        let candidates = executable_candidates_from_path(
            "",
            &root,
            0,
            0,
            ShellDialect::Posix,
            root.as_os_str(),
            &AtomicBool::new(false),
            Instant::now(),
        );
        let replacement = |display: &str| {
            candidates
                .iter()
                .find(|candidate| candidate.display == display)
                .map(|candidate| candidate.edit.replacement.as_str())
        };
        assert_eq!(replacement("if"), Some("'if'"));
        assert_eq!(replacement("RC_MODE=value"), Some("'RC_MODE=value'"));
        assert_eq!(replacement("~"), Some("'~'"));

        let fish_candidates = executable_candidates_from_path(
            "",
            &root,
            0,
            0,
            ShellDialect::Fish,
            root.as_os_str(),
            &AtomicBool::new(false),
            Instant::now(),
        );
        let fish_replacement = |display: &str| {
            fish_candidates
                .iter()
                .find(|candidate| candidate.display == display)
                .map(|candidate| candidate.edit.replacement.as_str())
        };
        for name in FISH_COMMAND_WORDS {
            assert_eq!(
                fish_replacement(name),
                root.join(name).to_str(),
                "Fish command word {name:?} needs an explicit path"
            );
        }
        let assignment_path = root.join("RC_MODE=value");
        let quoted_assignment_path = quote_token_for_role(
            assignment_path
                .to_str()
                .expect("fixture path should be UTF-8"),
            ShellDialect::Fish,
            TokenRole::CommandName,
        );
        assert_eq!(
            fish_replacement("RC_MODE=value"),
            Some(quoted_assignment_path.as_str())
        );

        fs::remove_dir_all(root).expect("completion temp directory should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn filesystem_completion_preserves_terminal_dot_components() {
        let root = completion_temp_dir("terminal-dot-components");
        let current = root.join("current");
        let sibling = root.join("sibling");
        fs::create_dir(&current).expect("current directory should be creatable");
        fs::create_dir(&sibling).expect("sibling directory should be creatable");
        fs::write(current.join("local"), "local").expect("local fixture should be writable");

        let current_candidates = filesystem_candidates(
            ".",
            &current,
            0,
            1,
            ShellDialect::Posix,
            &AtomicBool::new(false),
            Instant::now(),
        );
        assert!(
            current_candidates
                .iter()
                .any(|candidate| candidate.edit.replacement == "./local")
        );

        let parent_candidates = filesystem_candidates(
            "..",
            &current,
            0,
            2,
            ShellDialect::Posix,
            &AtomicBool::new(false),
            Instant::now(),
        );
        assert!(
            parent_candidates
                .iter()
                .any(|candidate| candidate.edit.replacement == "../sibling/")
        );
        assert!(
            parent_candidates
                .iter()
                .all(|candidate| { candidate.edit.replacement.starts_with("../") })
        );

        fs::remove_dir_all(root).expect("completion temp directory should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn filesystem_completion_follows_directory_symlinks() {
        use std::os::unix::fs::symlink;

        let root = completion_temp_dir("directory-symlink");
        let target = root.join("target");
        fs::create_dir(&target).expect("target directory should be creatable");
        fs::write(target.join("child"), "child").expect("child fixture should be writable");
        symlink("target", root.join("linked")).expect("directory symlink should be creatable");

        let candidates = filesystem_candidates(
            "link",
            &root,
            0,
            4,
            ShellDialect::Posix,
            &AtomicBool::new(false),
            Instant::now(),
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].edit.replacement, "linked/");
        assert_eq!(candidates[0].description.as_deref(), Some("directory"));

        let children = filesystem_candidates(
            "linked/",
            &root,
            0,
            7,
            ShellDialect::Posix,
            &AtomicBool::new(false),
            Instant::now(),
        );
        assert!(
            children
                .iter()
                .any(|candidate| candidate.edit.replacement == "linked/child")
        );

        fs::remove_dir_all(root).expect("completion temp directory should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn limited_reader_keeps_ownership_after_its_deadline_until_the_reader_exits() {
        struct GatedReader(Arc<AtomicBool>);

        impl Read for GatedReader {
            fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
                while !self.0.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(2));
                }
                Ok(0)
            }
        }

        let release = Arc::new(AtomicBool::new(false));
        let reader = LimitedReader::spawn(
            GatedReader(Arc::clone(&release)),
            COMPLETION_STDOUT_LIMIT_BYTES,
            Arc::new(AtomicBool::new(false)),
        );
        let (finished_tx, finished_rx) = mpsc::channel();
        let owner = thread::spawn(move || {
            let result = reader.finish(Instant::now() + Duration::from_millis(10), "stdout");
            let _ = finished_tx.send(result);
        });

        assert!(matches!(
            finished_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        release.store(true, Ordering::Relaxed);
        let error = finished_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("the reader owner should finish after release")
            .expect_err("the elapsed capture deadline should still be reported");
        assert!(error_message(&error).contains("safety deadline"));
        owner.join().expect("reader owner should not panic");
    }

    #[cfg(unix)]
    #[test]
    fn limited_reader_drop_keeps_ownership_on_cancellation_paths() {
        struct GatedReader(Arc<AtomicBool>);

        impl Read for GatedReader {
            fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
                while !self.0.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(2));
                }
                Ok(0)
            }
        }

        let release = Arc::new(AtomicBool::new(false));
        let reader = LimitedReader::spawn(
            GatedReader(Arc::clone(&release)),
            COMPLETION_STDOUT_LIMIT_BYTES,
            Arc::new(AtomicBool::new(false)),
        );
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let owner = thread::spawn(move || {
            drop(reader);
            let _ = dropped_tx.send(());
        });

        assert!(matches!(
            dropped_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        release.store(true, Ordering::Relaxed);
        dropped_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("dropping the owner should finish after reader release");
        owner.join().expect("reader owner should not panic");
    }

    #[cfg(unix)]
    fn error_message(error: &FishCompletionError) -> &str {
        match error {
            FishCompletionError::Canceled => "canceled",
            FishCompletionError::Failed(message) => message,
        }
    }

    #[test]
    fn quoting_is_dialect_specific_and_preserves_spaces() {
        assert_eq!(quote_token("a b", ShellDialect::Posix), "'a b'");
        assert_eq!(quote_token("a'b", ShellDialect::Posix), "'a'\\''b'");
        assert_eq!(quote_token("a'b", ShellDialect::Fish), "'a\\'b'");
        assert_eq!(quote_token("~/a b", ShellDialect::Posix), "~/'a b'");
        assert_eq!(quote_token("~/a'b", ShellDialect::Fish), "~/'a\\'b'");
        assert_eq!(quote_token("~someone", ShellDialect::Posix), "'~someone'");
        assert_eq!(quote_token("%self", ShellDialect::Fish), "'%self'");
        assert_eq!(quote_literal_token("~", ShellDialect::Posix), "'~'");
        assert_eq!(
            quote_literal_token("~/file", ShellDialect::Posix),
            "'~/file'"
        );
        assert_eq!(quote_literal_token("~", ShellDialect::Fish), "'~'");
        assert_eq!(
            quote_literal_token("~/file", ShellDialect::Fish),
            "'~/file'"
        );
        assert_eq!(
            quote_token_for_role("if", ShellDialect::Fish, TokenRole::CommandName),
            "'if'"
        );
    }
}
