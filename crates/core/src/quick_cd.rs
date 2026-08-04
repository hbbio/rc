use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use crate::*;

const QUICK_CD_SEARCH_DEBOUNCE: Duration = Duration::from_millis(90);

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum QuickCdError {
    Empty,
    InvalidQuoting,
    TooManyArguments,
    NoPreviousDirectory,
    HomeDirectoryUnavailable,
    UnknownUser(String),
    HomeLookup(String),
}

#[derive(Debug)]
pub(crate) struct QuickCdSearchWorkflow {
    job_id: Option<JobId>,
    request_id: u64,
    next_request_id: u64,
    pending: Option<PendingQuickCdSearch>,
}

#[derive(Debug)]
struct PendingQuickCdSearch {
    request_id: u64,
    spec: QuickCdSearchSpec,
    due_at: Instant,
}

impl Default for QuickCdSearchWorkflow {
    fn default() -> Self {
        Self {
            job_id: None,
            request_id: 0,
            next_request_id: 1,
            pending: None,
        }
    }
}

impl QuickCdSearchWorkflow {
    fn schedule(&mut self, spec: QuickCdSearchSpec, now: Instant) -> Option<JobId> {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.request_id = request_id;
        self.pending = Some(PendingQuickCdSearch {
            request_id,
            spec,
            due_at: now.checked_add(QUICK_CD_SEARCH_DEBOUNCE).unwrap_or(now),
        });
        self.job_id.take()
    }

    fn invalidate(&mut self) -> Option<JobId> {
        self.request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.pending = None;
        self.job_id.take()
    }

    fn take_due(&mut self, now: Instant) -> Option<(u64, QuickCdSearchSpec)> {
        let pending = self.pending.as_ref()?;
        if pending.due_at > now {
            return None;
        }
        let pending = self.pending.take()?;
        Some((pending.request_id, pending.spec))
    }

    fn delay(&self, now: Instant) -> Option<Duration> {
        self.pending
            .as_ref()
            .map(|pending| pending.due_at.saturating_duration_since(now))
    }

    fn is_current(&self, request_id: u64) -> bool {
        self.request_id == request_id
    }

    fn set_job_id(&mut self, job_id: JobId) {
        self.job_id = Some(job_id);
    }

    fn is_job(&self, job_id: JobId) -> bool {
        self.job_id == Some(job_id)
    }

    fn clear_job(&mut self, job_id: JobId) -> bool {
        if !self.is_job(job_id) {
            return false;
        }
        self.job_id = None;
        true
    }
}

impl fmt::Display for QuickCdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("enter a directory"),
            Self::InvalidQuoting => formatter.write_str("unclosed quote or trailing escape"),
            Self::TooManyArguments => {
                formatter.write_str("enter one directory (quote paths containing spaces)")
            }
            Self::NoPreviousDirectory => formatter.write_str("no previous directory"),
            Self::HomeDirectoryUnavailable => formatter.write_str("home directory is unavailable"),
            Self::UnknownUser(user) => write!(formatter, "unknown user '{user}'"),
            Self::HomeLookup(error) => write!(formatter, "home lookup failed: {error}"),
        }
    }
}

impl AppState {
    pub(crate) fn start_quick_cd_dialog(&mut self) {
        self.open_quick_cd_dialog(String::new());
        self.set_status("Quick cd: type a path or substring");
    }

    pub(crate) fn submit_quick_cd(&mut self, input: String, selected_path: Option<PathBuf>) {
        let panel = self.active_panel;
        let cwd = self.active_panel().cwd.clone();
        let previous = self.previous_panel_directories[panel.index()].as_deref();
        let destination = match selected_path {
            Some(path) => lexically_normalize(&path),
            None => match resolve_quick_cd_input(&input, &cwd, previous) {
                Ok(destination) => destination,
                Err(error) => {
                    self.reopen_quick_cd_after_error(input, error.to_string());
                    return;
                }
            },
        };

        let metadata = match fs::metadata(&destination) {
            Ok(metadata) => metadata,
            Err(error) => {
                self.reopen_quick_cd_after_error(
                    input,
                    format!("{}: {error}", destination.to_string_lossy()),
                );
                return;
            }
        };
        if !metadata.is_dir() {
            self.reopen_quick_cd_after_error(
                input,
                format!("{} is not a directory", destination.to_string_lossy()),
            );
            return;
        }
        if destination == cwd {
            self.set_status(format!("Already in {}", destination.to_string_lossy()));
            return;
        }

        match self.set_active_panel_directory(destination.clone()) {
            Ok(true) => self.set_status(format!(
                "Loading directory {}...",
                destination.to_string_lossy()
            )),
            Ok(false) => self.reopen_quick_cd_after_error(
                input,
                format!(
                    "{} is not an accessible directory",
                    destination.to_string_lossy()
                ),
            ),
            Err(error) => self.reopen_quick_cd_after_error(input, error.to_string()),
        }
    }

    pub(crate) fn remember_previous_directory(
        &mut self,
        panel: ActivePanel,
        previous_directory: PathBuf,
    ) {
        if self.panels[panel.index()].cwd != previous_directory {
            self.previous_panel_directories[panel.index()] = Some(previous_directory);
        }
    }

    fn open_quick_cd_dialog(&mut self, initial_value: String) {
        self.push_dialog(
            DialogState::quick_cd(initial_value),
            PendingDialogAction::QuickCd,
        );
        self.sync_quick_cd_search();
    }

    fn reopen_quick_cd_after_error(&mut self, input: String, error: String) {
        self.open_quick_cd_dialog(input);
        self.set_status(format!("Quick cd failed: {error}"));
    }

    pub(crate) fn sync_quick_cd_search(&mut self) {
        let query = match self.routes.last() {
            Some(Route::Dialog(dialog))
                if matches!(dialog.action(), Some(PendingDialogAction::QuickCd)) =>
            {
                match &dialog.kind {
                    DialogKind::QuickCd(quick_cd) => quick_cd.value.clone(),
                    _ => return,
                }
            }
            _ => return,
        };

        if query.trim().is_empty() {
            self.stop_quick_cd_search();
            if let Some(quick_cd) = self.quick_cd_dialog_mut() {
                quick_cd.clear_search();
            }
            return;
        }

        let cwd = self.active_panel().cwd.clone();
        let home = system_home_directory(None).ok().flatten();
        let root = filesystem_root(&cwd);
        let previous_directory = self.previous_panel_directories[self.active_panel.index()].clone();
        if let Some(quick_cd) = self.quick_cd_dialog_mut() {
            quick_cd.begin_search();
        }
        let spec = QuickCdSearchSpec {
            query,
            cwd,
            home,
            root,
            previous_directory,
            max_results: DEFAULT_QUICK_CD_MAX_RESULTS,
            max_directories: DEFAULT_QUICK_CD_MAX_DIRECTORIES,
        };
        if let Some(previous_job_id) = self.quick_cd_search.schedule(spec, Instant::now()) {
            let _ = self.request_cancel_for_job(previous_job_id);
        }
    }

    pub fn poll_deferred_work(&mut self) {
        self.poll_deferred_work_at(Instant::now());
    }

    pub fn deferred_work_delay(&self) -> Option<Duration> {
        self.quick_cd_search.delay(Instant::now())
    }

    pub(crate) fn poll_deferred_work_at(&mut self, now: Instant) {
        let Some((request_id, spec)) = self.quick_cd_search.take_due(now) else {
            return;
        };
        let request = JobRequest::QuickCdSearch { spec, request_id };
        let job_id = self.queue_transient_worker_job_request(request);
        self.quick_cd_search.set_job_id(job_id);
    }

    pub(crate) fn handle_quick_cd_search_snapshot(
        &mut self,
        request_id: u64,
        snapshot: QuickCdSearchSnapshot,
    ) {
        if !self.quick_cd_search.is_current(request_id) {
            return;
        }
        if let Some(quick_cd) = self.quick_cd_dialog_mut() {
            quick_cd.apply_search_snapshot(snapshot);
        }
    }

    pub(crate) fn handle_quick_cd_search_job_finished(&mut self, job_id: JobId) {
        self.quick_cd_search.clear_job(job_id);
    }

    pub(crate) fn handle_quick_cd_search_job_failure(&mut self, job_id: JobId, error: &JobError) {
        if !self.quick_cd_search.clear_job(job_id) {
            return;
        }
        if let Some(quick_cd) = self.quick_cd_dialog_mut() {
            quick_cd.fail_search(if error.is_canceled() {
                "Search canceled".to_string()
            } else {
                error.user_message()
            });
        }
    }

    pub(crate) fn handle_quick_cd_search_cancel_requested(&mut self, job_id: JobId) {
        if !self.quick_cd_search.is_job(job_id) {
            return;
        }
        let _ = self.quick_cd_search.invalidate();
        if let Some(quick_cd) = self.quick_cd_dialog_mut() {
            quick_cd.fail_search("Search canceled");
        }
    }

    pub(crate) fn stop_quick_cd_search(&mut self) {
        if let Some(job_id) = self.quick_cd_search.invalidate() {
            let _ = self.request_cancel_for_job(job_id);
        }
    }

    fn quick_cd_dialog_mut(&mut self) -> Option<&mut QuickCdDialogState> {
        let Some(Route::Dialog(dialog)) = self.routes.last_mut() else {
            return None;
        };
        if !matches!(dialog.action(), Some(PendingDialogAction::QuickCd)) {
            return None;
        }
        match &mut dialog.kind {
            DialogKind::QuickCd(quick_cd) => Some(quick_cd),
            _ => None,
        }
    }
}

fn filesystem_root(path: &Path) -> PathBuf {
    path.ancestors()
        .last()
        .filter(|root| !root.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(std::path::MAIN_SEPARATOR.to_string()))
}

pub(crate) fn resolve_quick_cd_input(
    input: &str,
    cwd: &Path,
    previous_directory: Option<&Path>,
) -> Result<PathBuf, QuickCdError> {
    resolve_quick_cd_input_with_home(input, cwd, previous_directory, system_home_directory)
}

fn resolve_quick_cd_input_with_home(
    input: &str,
    cwd: &Path,
    previous_directory: Option<&Path>,
    mut home_directory: impl FnMut(Option<&str>) -> Result<Option<PathBuf>, String>,
) -> Result<PathBuf, QuickCdError> {
    let arguments = shlex::split(input).ok_or(QuickCdError::InvalidQuoting)?;
    let argument = match arguments.as_slice() {
        [] => return Err(QuickCdError::Empty),
        [argument] => argument,
        _ => return Err(QuickCdError::TooManyArguments),
    };

    if argument == "-" {
        return previous_directory
            .map(Path::to_path_buf)
            .ok_or(QuickCdError::NoPreviousDirectory);
    }

    let expanded = if let Some(tilde) = parse_tilde(argument) {
        let home = home_directory(tilde.user)
            .map_err(QuickCdError::HomeLookup)?
            .ok_or_else(|| match tilde.user {
                Some(user) => QuickCdError::UnknownUser(user.to_string()),
                None => QuickCdError::HomeDirectoryUnavailable,
            })?;
        home.join(tilde.remainder)
    } else {
        PathBuf::from(argument)
    };
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    };
    Ok(lexically_normalize(&absolute))
}

struct TildeExpansion<'a> {
    user: Option<&'a str>,
    remainder: &'a str,
}

fn parse_tilde(argument: &str) -> Option<TildeExpansion<'_>> {
    let suffix = argument.strip_prefix('~')?;
    let (user, remainder) = match suffix.split_once('/') {
        Some((user, remainder)) => (user, remainder),
        None => (suffix, ""),
    };
    Some(TildeExpansion {
        user: (!user.is_empty()).then_some(user),
        remainder,
    })
}

fn lexically_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                Some(Component::ParentDir) | None if !normalized.has_root() => {
                    normalized.push(component.as_os_str());
                }
                _ => {}
            },
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

#[cfg(unix)]
fn system_home_directory(user: Option<&str>) -> Result<Option<PathBuf>, String> {
    use nix::unistd::{Uid, User};

    if user.is_none()
        && let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty())
    {
        return Ok(Some(PathBuf::from(home)));
    }

    let account = match user {
        Some(user) => User::from_name(user),
        None => User::from_uid(Uid::current()),
    }
    .map_err(|error| error.to_string())?;
    Ok(account.map(|account| account.dir))
}

#[cfg(windows)]
fn system_home_directory(user: Option<&str>) -> Result<Option<PathBuf>, String> {
    if let Some(user) = user {
        return Err(format!("named-user expansion is unavailable for '{user}'"));
    }
    if let Some(profile) = std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
        return Ok(Some(PathBuf::from(profile)));
    }
    let drive = std::env::var_os("HOMEDRIVE");
    let path = std::env::var_os("HOMEPATH");
    Ok(drive.zip(path).map(|(drive, path)| {
        let mut home = PathBuf::from(drive);
        home.push(path);
        home
    }))
}

#[cfg(not(any(unix, windows)))]
fn system_home_directory(user: Option<&str>) -> Result<Option<PathBuf>, String> {
    if let Some(user) = user {
        return Err(format!("named-user expansion is unavailable for '{user}'"));
    }
    Ok(std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn test_root() -> PathBuf {
        PathBuf::from(r"C:\work")
    }

    #[cfg(not(windows))]
    fn test_root() -> PathBuf {
        PathBuf::from("/work")
    }

    fn home_lookup(user: Option<&str>) -> Result<Option<PathBuf>, String> {
        match user {
            None => Ok(Some(test_root().join("home/current"))),
            Some("alice") => Ok(Some(test_root().join("home/alice"))),
            Some(_) => Ok(None),
        }
    }

    #[test]
    fn resolves_relative_quoted_and_normalized_paths() {
        let root = test_root();
        let resolved =
            resolve_quick_cd_input_with_home("projects/./rc/../clean", &root, None, home_lookup)
                .expect("path should resolve");
        assert_eq!(resolved, root.join("projects/clean"));

        let resolved = resolve_quick_cd_input_with_home(
            r#""projects/clean code"/../rc"#,
            &root,
            None,
            home_lookup,
        )
        .expect("quoted path should resolve");
        assert_eq!(resolved, root.join("projects/rc"));
    }

    #[test]
    fn expands_current_and_named_user_homes() {
        let root = test_root();
        assert_eq!(
            resolve_quick_cd_input_with_home("~/src", &root, None, home_lookup),
            Ok(root.join("home/current/src"))
        );
        assert_eq!(
            resolve_quick_cd_input_with_home("~alice/src", &root, None, home_lookup),
            Ok(root.join("home/alice/src"))
        );
        assert_eq!(
            resolve_quick_cd_input_with_home("~missing", &root, None, home_lookup),
            Err(QuickCdError::UnknownUser(String::from("missing")))
        );
    }

    #[test]
    fn resolves_previous_directory_and_rejects_invalid_input() {
        let root = test_root();
        let previous = root.join("previous");
        assert_eq!(
            resolve_quick_cd_input_with_home("-", &root, Some(&previous), home_lookup),
            Ok(previous)
        );
        assert_eq!(
            resolve_quick_cd_input_with_home("-", &root, None, home_lookup),
            Err(QuickCdError::NoPreviousDirectory)
        );
        assert_eq!(
            resolve_quick_cd_input_with_home("", &root, None, home_lookup),
            Err(QuickCdError::Empty)
        );
        assert_eq!(
            resolve_quick_cd_input_with_home("one two", &root, None, home_lookup),
            Err(QuickCdError::TooManyArguments)
        );
        assert_eq!(
            resolve_quick_cd_input_with_home("\"unfinished", &root, None, home_lookup),
            Err(QuickCdError::InvalidQuoting)
        );
    }
}
