use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::*;

#[derive(Debug, Eq, PartialEq)]
enum QuickCdError {
    Empty,
    InvalidQuoting,
    TooManyArguments,
    NoPreviousDirectory,
    HomeDirectoryUnavailable,
    UnknownUser(String),
    HomeLookup(String),
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
        self.set_status("Quick cd: enter directory");
    }

    pub(crate) fn submit_quick_cd(&mut self, input: String) {
        let panel = self.active_panel;
        let cwd = self.active_panel().cwd.clone();
        let previous = self.previous_panel_directories[panel.index()].as_deref();
        let destination = match resolve_quick_cd_input(&input, &cwd, previous) {
            Ok(destination) => destination,
            Err(error) => {
                self.reopen_quick_cd_after_error(input, error.to_string());
                return;
            }
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
            DialogState::input("Quick cd", "Directory:", initial_value),
            PendingDialogAction::QuickCd,
        );
    }

    fn reopen_quick_cd_after_error(&mut self, input: String, error: String) {
        self.open_quick_cd_dialog(input);
        self.set_status(format!("Quick cd failed: {error}"));
    }
}

fn resolve_quick_cd_input(
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
