use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

pub const COMMAND_PLACEHOLDER: &str = "{command}";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellDialect {
    Posix,
    Fish,
}

impl ShellDialect {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "posix" => Some(Self::Posix),
            "fish" => Some(Self::Fish),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Posix => "posix",
            Self::Fish => "fish",
        }
    }

    pub fn default_arguments(self) -> Vec<String> {
        vec![String::from("-c"), String::from(COMMAND_PLACEHOLDER)]
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ShellHistoryMode {
    Off,
    #[default]
    Session,
}

impl ShellHistoryMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "session" => Some(Self::Session),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Session => "session",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CustomShell {
    pub program: String,
    pub dialect: ShellDialect,
    pub arguments: Vec<String>,
}

impl CustomShell {
    pub fn new(
        program: impl Into<String>,
        dialect: ShellDialect,
        arguments: Option<Vec<String>>,
    ) -> Result<Self, String> {
        let program = program.into();
        if program.trim().is_empty() {
            return Err(String::from("custom shell program is empty"));
        }
        if program.contains('\0') {
            return Err(String::from("custom shell program contains NUL"));
        }
        let arguments = arguments.unwrap_or_else(|| dialect.default_arguments());
        validate_argument_template(&arguments)?;
        Ok(Self {
            program,
            dialect,
            arguments,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShellMode {
    Auto,
    Custom(CustomShell),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellSettings {
    pub mode: ShellMode,
    pub history: ShellHistoryMode,
    /// A nonfatal configuration diagnostic to surface once after startup.
    pub diagnostic: Option<String>,
}

impl Default for ShellSettings {
    fn default() -> Self {
        Self {
            mode: ShellMode::Auto,
            history: ShellHistoryMode::Session,
            diagnostic: None,
        }
    }
}

impl ShellSettings {
    pub fn auto(history: ShellHistoryMode) -> Self {
        Self {
            mode: ShellMode::Auto,
            history,
            diagnostic: None,
        }
    }

    pub fn custom(
        program: impl Into<String>,
        dialect: ShellDialect,
        arguments: Option<Vec<String>>,
        history: ShellHistoryMode,
    ) -> Result<Self, String> {
        Ok(Self {
            mode: ShellMode::Custom(CustomShell::new(program, dialect, arguments)?),
            history,
            diagnostic: None,
        })
    }

    pub fn with_diagnostic(mut self, diagnostic: impl Into<String>) -> Self {
        self.push_diagnostic(diagnostic);
        self
    }

    pub fn push_diagnostic(&mut self, diagnostic: impl Into<String>) {
        let diagnostic = diagnostic.into();
        self.diagnostic = Some(match self.diagnostic.take() {
            Some(previous) => format!("{previous}; {diagnostic}"),
            None => diagnostic,
        });
    }

    /// Apply the `RC_SHELL*` contract without ever mixing individual execution fields.
    pub fn apply_environment_with(&mut self, mut lookup: impl FnMut(&str) -> Option<String>) {
        let shell = lookup("RC_SHELL");
        let dialect = lookup("RC_SHELL_DIALECT");
        let arguments = lookup("RC_SHELL_ARGV_JSON");

        let execution_override_present = shell
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            || dialect.is_some()
            || arguments.is_some();
        if execution_override_present {
            match parse_environment_execution_override(
                shell.as_deref(),
                dialect.as_deref(),
                arguments.as_deref(),
            ) {
                Ok(mode) => self.mode = mode,
                Err(error) => self.push_diagnostic(format!(
                    "ignored invalid shell environment override: {error}"
                )),
            }
        }

        if let Some(history) = lookup("RC_SHELL_HISTORY") {
            match ShellHistoryMode::parse(&history) {
                Some(history) => self.history = history,
                None => self.push_diagnostic(format!(
                    "ignored invalid RC_SHELL_HISTORY value '{history}'"
                )),
            }
        }
    }

    pub fn apply_environment(&mut self) {
        self.apply_environment_with(|name| std::env::var(name).ok());
    }
}

fn parse_environment_execution_override(
    shell: Option<&str>,
    dialect: Option<&str>,
    arguments_json: Option<&str>,
) -> Result<ShellMode, String> {
    let Some(shell) = shell.filter(|value| !value.trim().is_empty()) else {
        return Err(String::from(
            "RC_SHELL_DIALECT/RC_SHELL_ARGV_JSON requires a non-empty RC_SHELL",
        ));
    };
    if shell.trim().eq_ignore_ascii_case("auto") {
        if dialect.is_some() || arguments_json.is_some() {
            return Err(String::from(
                "RC_SHELL=auto cannot be combined with dialect or argv",
            ));
        }
        return Ok(ShellMode::Auto);
    }

    let dialect = dialect
        .and_then(ShellDialect::parse)
        .ok_or_else(|| String::from("a custom RC_SHELL requires RC_SHELL_DIALECT=posix|fish"))?;
    let arguments = arguments_json.map(parse_argument_json).transpose()?;
    Ok(ShellMode::Custom(CustomShell::new(
        shell.to_string(),
        dialect,
        arguments,
    )?))
}

pub fn parse_argument_json(value: &str) -> Result<Vec<String>, String> {
    let parsed: serde_json::Value = serde_json::from_str(value)
        .map_err(|error| format!("RC_SHELL_ARGV_JSON is not valid JSON: {error}"))?;
    let values = parsed
        .as_array()
        .ok_or_else(|| String::from("RC_SHELL_ARGV_JSON must be an array of strings"))?;
    let mut arguments = Vec::with_capacity(values.len());
    for value in values {
        let value = value
            .as_str()
            .ok_or_else(|| String::from("RC_SHELL_ARGV_JSON must contain only strings"))?;
        arguments.push(value.to_string());
    }
    validate_argument_template(&arguments)?;
    Ok(arguments)
}

pub fn validate_argument_template(arguments: &[String]) -> Result<(), String> {
    if arguments.iter().any(|argument| argument.contains('\0')) {
        return Err(String::from("shell argument contains NUL"));
    }
    let placeholders = arguments
        .iter()
        .filter(|argument| argument.as_str() == COMMAND_PLACEHOLDER)
        .count();
    if placeholders != 1 {
        return Err(String::from(
            "shell arguments must contain exactly one standalone {command}",
        ));
    }
    if arguments
        .iter()
        .any(|argument| argument != COMMAND_PLACEHOLDER && argument.contains(COMMAND_PLACEHOLDER))
    {
        return Err(String::from(
            "{command} may not be embedded inside another shell argument",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedShell {
    pub program: PathBuf,
    pub dialect: ShellDialect,
    pub argument_template: Vec<OsString>,
    pub identity: String,
}

impl ResolvedShell {
    pub fn invocation(&self, command: &str) -> ShellInvocation {
        let arguments = self
            .argument_template
            .iter()
            .map(|argument| {
                if argument == OsStr::new(COMMAND_PLACEHOLDER) {
                    OsString::from(command)
                } else {
                    argument.clone()
                }
            })
            .collect();
        ShellInvocation {
            program: self.program.clone(),
            arguments,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellInvocation {
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellResolution {
    pub shell: ResolvedShell,
    pub diagnostic: Option<String>,
}

#[cfg(unix)]
pub fn resolve_shell(settings: &ShellSettings, cwd: &Path) -> io::Result<ShellResolution> {
    if let ShellMode::Custom(custom) = &settings.mode {
        return Ok(ShellResolution {
            shell: resolved_custom(custom, cwd),
            diagnostic: settings.diagnostic.clone(),
        });
    }

    if let Some(value) = std::env::var_os("SHELL")
        && let Some(shell) = resolve_known_program(PathBuf::from(value), cwd, true)
    {
        return Ok(ShellResolution {
            shell,
            diagnostic: settings.diagnostic.clone(),
        });
    }

    if let Some(path) = login_shell_path()
        && let Some(shell) = resolve_known_program(path, cwd, true)
    {
        return Ok(ShellResolution {
            shell,
            diagnostic: settings.diagnostic.clone(),
        });
    }

    if let Some(path) = find_program_on_path(OsStr::new("fish"), cwd)
        && let Some(shell) = resolve_known_program(path, cwd, false)
    {
        return Ok(ShellResolution {
            shell,
            diagnostic: settings.diagnostic.clone(),
        });
    }

    Ok(ShellResolution {
        shell: built_in_shell(PathBuf::from("/bin/sh"), ShellDialect::Posix),
        diagnostic: settings.diagnostic.clone(),
    })
}

#[cfg(not(unix))]
pub fn resolve_shell(_settings: &ShellSettings, _cwd: &Path) -> io::Result<ShellResolution> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "command line is not yet supported on this platform",
    ))
}

#[cfg(unix)]
fn resolved_custom(custom: &CustomShell, cwd: &Path) -> ResolvedShell {
    let program = resolve_program_path(PathBuf::from(&custom.program), cwd);
    let identity = program_identity(&program);
    ResolvedShell {
        program,
        dialect: custom.dialect,
        argument_template: custom.arguments.iter().map(OsString::from).collect(),
        identity,
    }
}

#[cfg(unix)]
fn built_in_shell(program: PathBuf, dialect: ShellDialect) -> ResolvedShell {
    ResolvedShell {
        identity: program_identity(&program),
        program,
        dialect,
        argument_template: dialect
            .default_arguments()
            .into_iter()
            .map(OsString::from)
            .collect(),
    }
}

#[cfg(unix)]
fn resolve_known_program(
    program: PathBuf,
    cwd: &Path,
    require_available: bool,
) -> Option<ResolvedShell> {
    let basename = program.file_name()?.to_string_lossy().to_ascii_lowercase();
    let dialect = match basename.as_str() {
        "fish" => ShellDialect::Fish,
        "sh" | "bash" | "dash" | "ksh" | "zsh" => ShellDialect::Posix,
        _ => return None,
    };
    let resolved = if require_available {
        resolve_available_program_path(program, cwd)?
    } else {
        resolve_program_path(program, cwd)
    };
    Some(built_in_shell(resolved, dialect))
}

#[cfg(unix)]
fn resolve_available_program_path(program: PathBuf, cwd: &Path) -> Option<PathBuf> {
    let search_path = std::env::var_os("PATH");
    resolve_available_program_path_on_search_path(program, cwd, search_path.as_deref())
}

#[cfg(unix)]
fn resolve_available_program_path_on_search_path(
    program: PathBuf,
    cwd: &Path,
    search_path: Option<&OsStr>,
) -> Option<PathBuf> {
    if program.is_absolute() {
        return is_executable_file(&program).then_some(program);
    }
    if program.components().count() == 1 {
        return find_program_on_search_path(program.as_os_str(), cwd, search_path?);
    }
    let resolved = anchor_to_cwd(&program, cwd)?;
    is_executable_file(&resolved).then_some(resolved)
}

#[cfg(unix)]
fn resolve_program_path(program: PathBuf, cwd: &Path) -> PathBuf {
    if program.is_absolute() {
        return program;
    }
    if program.components().count() == 1 {
        return find_program_on_path(program.as_os_str(), cwd).unwrap_or(program);
    }
    anchor_to_cwd(&program, cwd).unwrap_or(program)
}

#[cfg(unix)]
fn program_identity(program: &Path) -> String {
    program
        .file_name()
        .filter(|name| !name.is_empty())
        .unwrap_or(program.as_os_str())
        .to_string_lossy()
        .into_owned()
}

#[cfg(unix)]
pub(crate) fn is_executable_file(path: &Path) -> bool {
    use nix::unistd::{AccessFlags, access};

    path.is_file() && access(path, AccessFlags::X_OK).is_ok()
}

#[cfg(not(unix))]
pub(crate) fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

pub fn find_program_on_path(program: &OsStr, cwd: &Path) -> Option<PathBuf> {
    if Path::new(program).components().count() > 1 {
        let path = anchor_to_cwd(Path::new(program), cwd)?;
        return is_executable_file(&path).then_some(path);
    }
    let search_path = std::env::var_os("PATH")?;
    find_program_on_search_path(program, cwd, &search_path)
}

fn find_program_on_search_path(
    program: &OsStr,
    cwd: &Path,
    search_path: &OsStr,
) -> Option<PathBuf> {
    if Path::new(program).components().count() > 1 {
        let path = anchor_to_cwd(Path::new(program), cwd)?;
        return is_executable_file(&path).then_some(path);
    }
    std::env::split_paths(search_path)
        .filter_map(|directory| anchor_to_cwd(&directory, cwd))
        .map(|directory| directory.join(program))
        .find(|candidate| is_executable_file(candidate))
}

fn anchor_to_cwd(path: &Path, cwd: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    let cwd = if cwd.is_absolute() {
        cwd.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(cwd)
    };
    Some(cwd.join(path))
}

#[cfg(unix)]
fn login_shell_path() -> Option<PathBuf> {
    use nix::unistd::{Uid, User};

    User::from_uid(Uid::current())
        .ok()
        .flatten()
        .map(|user| user.shell)
        .filter(|path| !path.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn make_temp_dir(label: &str) -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("rc-shell-{label}-{stamp}"));
        std::fs::create_dir_all(&root).expect("temp directory should be creatable");
        root
    }

    #[test]
    fn argument_template_requires_one_standalone_placeholder() {
        assert!(validate_argument_template(&["-c".into(), "{command}".into()]).is_ok());
        assert!(validate_argument_template(&["-c".into()]).is_err());
        assert!(validate_argument_template(&["prefix-{command}".into()]).is_err());
        assert!(validate_argument_template(&["{command}".into(), "{command}".into()]).is_err());
    }

    #[test]
    fn environment_execution_override_is_atomic() {
        let original = ShellMode::Custom(
            CustomShell::new("zsh", ShellDialect::Posix, None).expect("valid shell"),
        );
        let mut settings = ShellSettings {
            mode: original.clone(),
            history: ShellHistoryMode::Session,
            diagnostic: None,
        };
        settings.apply_environment_with(|name| match name {
            "RC_SHELL" => Some(String::from("fish")),
            _ => None,
        });
        assert_eq!(settings.mode, original);
        assert!(settings.diagnostic.is_some());
    }

    #[test]
    fn invocation_replaces_the_whole_placeholder_argument() {
        let custom = CustomShell::new(
            "/opt/example shell",
            ShellDialect::Posix,
            Some(vec!["--flag".into(), "{command}".into()]),
        )
        .expect("valid custom shell");
        let shell = ResolvedShell {
            program: PathBuf::from(&custom.program),
            dialect: custom.dialect,
            argument_template: custom.arguments.into_iter().map(OsString::from).collect(),
            identity: String::from("example shell"),
        };
        let invocation = shell.invocation("printf '%s' \"$x\"");
        assert_eq!(invocation.arguments[1], OsStr::new("printf '%s' \"$x\""));
    }

    #[cfg(unix)]
    #[test]
    fn executable_check_respects_the_callers_permission_class() {
        use nix::unistd::Uid;
        use std::os::unix::fs::PermissionsExt;

        let root = make_temp_dir("executable-permissions");
        let program = root.join("command");
        std::fs::write(&program, "#!/bin/sh\n").expect("test command should be writable");
        let mut permissions = std::fs::metadata(&program)
            .expect("test command metadata should be readable")
            .permissions();
        permissions.set_mode(0o001);
        std::fs::set_permissions(&program, permissions)
            .expect("test command permissions should be settable");

        if !Uid::current().is_root() {
            assert!(!is_executable_file(&program));
        }

        let mut permissions = std::fs::metadata(&program)
            .expect("test command metadata should be readable")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&program, permissions)
            .expect("test command permissions should be settable");
        assert!(is_executable_file(&program));

        std::fs::remove_dir_all(root).expect("temp directory should be removable");
    }

    #[cfg(unix)]
    #[test]
    fn relative_path_entries_are_resolved_from_the_prompt_cwd() {
        use std::os::unix::fs::PermissionsExt;

        let root = make_temp_dir("relative-path");
        let prompt_cwd = root.join("panel");
        let bin = prompt_cwd.join("bin");
        std::fs::create_dir_all(&bin).expect("bin directory should be creatable");
        let program = bin.join("rc-relative-path-test-shell");
        std::fs::write(&program, "#!/bin/sh\n").expect("test shell should be writable");
        let mut permissions = std::fs::metadata(&program)
            .expect("test shell metadata should be readable")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&program, permissions).expect("test shell should be executable");

        let resolved = find_program_on_search_path(
            OsStr::new("rc-relative-path-test-shell"),
            &prompt_cwd,
            OsStr::new("bin"),
        );

        assert_eq!(resolved, Some(program));
        std::fs::remove_dir_all(root).expect("temp directory should be removable");
    }

    #[cfg(unix)]
    #[test]
    fn unresolved_bare_auto_shell_name_is_rejected() {
        use std::os::unix::fs::PermissionsExt;

        let root = make_temp_dir("unresolved-auto-shell");
        let prompt_cwd = root.join("panel");
        std::fs::create_dir_all(&prompt_cwd).expect("panel directory should be creatable");
        let program = prompt_cwd.join("fish");
        std::fs::write(&program, "#!/bin/sh\n").expect("test shell should be writable");
        let mut permissions = std::fs::metadata(&program)
            .expect("test shell metadata should be readable")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&program, permissions).expect("test shell should be executable");

        let resolved = resolve_available_program_path_on_search_path(
            PathBuf::from("fish"),
            &prompt_cwd,
            Some(OsStr::new("missing-bin")),
        );

        assert_eq!(resolved, None);
        std::fs::remove_dir_all(root).expect("temp directory should be removable");
    }
}
