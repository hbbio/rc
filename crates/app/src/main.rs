#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use clap::{ArgAction, Parser};
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode as CrosstermKeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use rc_core::keymap::{KeyChord, KeyCode, KeyContext, KeyModifiers, Keymap, KeymapParseReport};
use rc_core::settings_io;
use rc_core::{
    AppCommand, AppState, ApplyResult, CommandLineInput, CompletionIntent, ExternalEditRequest,
    ExternalExecuteRequest, ForegroundShellRequest, JobRequest, MouseClickTarget, Settings,
    sanitize_display_field, sanitize_display_line,
};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::writer::BoxMakeWriter;

mod runtime;

use runtime::RuntimeBridge;

const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(500);
const DEFAULT_LOG_FILE_NAME: &str = "rc.log";
const MAX_LOG_FILE_BYTES: u64 = 8 * 1024 * 1024;
static TERMINAL_RECOVERY_FAILED: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct MouseClickTracker {
    previous: Option<TrackedMouseClick>,
}

struct TrackedMouseClick {
    column: u16,
    row: u16,
    context: KeyContext,
    target: MouseClickTarget,
    occurred_at: Instant,
}

impl MouseClickTracker {
    fn clear(&mut self) {
        self.previous = None;
    }

    fn register(
        &mut self,
        column: u16,
        row: u16,
        context: KeyContext,
        target: MouseClickTarget,
        occurred_at: Instant,
    ) -> bool {
        let is_double_click = self.previous.as_ref().is_some_and(|previous| {
            previous.column == column
                && previous.row == row
                && previous.context == context
                && previous.target == target
                && occurred_at.saturating_duration_since(previous.occurred_at)
                    <= DOUBLE_CLICK_INTERVAL
        });
        self.previous = (!is_double_click).then_some(TrackedMouseClick {
            column,
            row,
            context,
            target,
            occurred_at,
        });
        is_double_click
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "rc",
    version,
    about = "A fast asynchronous TUI file manager inspired by Midnight Commander"
)]
struct Cli {
    #[arg(long, default_value_t = 200)]
    tick_rate_ms: u64,
    #[arg(long)]
    path: Option<PathBuf>,
    #[arg(long)]
    skin: Option<String>,
    #[arg(long)]
    skin_dir: Option<PathBuf>,
    #[arg(long)]
    keymap: Option<PathBuf>,
    #[arg(
        long,
        action = ArgAction::Set,
        default_missing_value = "true",
        num_args = 0..=1,
        help = "Enable compatibility mapping for macOS Option-symbol keys (for example ƒ -> Alt-f)"
    )]
    macos_option_compat: Option<bool>,
}

#[derive(Clone, Copy, Debug)]
struct InputCompatibility {
    macos_option_symbols: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let settings_paths = settings_io::settings_paths();
    let tracing_log_path = tracing_log_path(&settings_paths, std::env::var_os("RC_LOG_FILE"));
    if let Some(error) = init_tracing(tracing_log_path.as_deref())
        && let Some(path) = tracing_log_path.as_deref()
    {
        eprintln!(
            "rc: failed to open tracing log '{}': {error}; logging is disabled",
            path.display()
        );
    }

    let mut settings = settings_io::load_settings(&settings_paths).unwrap_or_else(|error| {
        if let Some(path) = settings_paths.rc_ini_path.as_deref() {
            tracing::warn!("failed to read settings '{}': {error}", path.display());
        } else {
            tracing::warn!("failed to read settings: {error}");
        }
        Settings::default()
    });
    apply_env_overrides(&mut settings);
    apply_cli_overrides(&mut settings, &cli);

    let start_path = cli
        .path
        .unwrap_or(std::env::current_dir().context("failed to resolve current directory")?);
    let mut state = AppState::new(start_path).context("failed to initialize app state")?;
    state.replace_settings(settings.clone());
    state.refresh_panels();

    let skin_dirs = settings.appearance.skin_dirs.clone();
    state.set_available_skins(rc_ui::list_available_skins_with_search_roots(&skin_dirs));
    if let Err(error) =
        rc_ui::configure_skin_with_search_roots(&settings.appearance.skin, &skin_dirs)
    {
        tracing::warn!(
            "failed to load skin '{}': {error}",
            settings.appearance.skin
        );
        state.set_status(format!(
            "Skin '{}' unavailable: {error}",
            settings.appearance.skin
        ));
    }
    state.set_active_skin_name(rc_ui::current_skin_name());
    let (keymap, keymap_report) = load_effective_keymap(&settings, &mut state)
        .context("failed to load keymap configuration")?;
    state.set_keybinding_hints_from_keymap(&keymap);
    report_keymap_parse_report(&mut state, &keymap_report);
    let skin_runtime = SkinRuntimeConfig {
        skin_dirs,
        settings_paths,
    };
    run_app(
        &mut state,
        &keymap,
        Duration::from_millis(cli.tick_rate_ms),
        &skin_runtime,
    )
}

fn tracing_log_path(
    settings_paths: &settings_io::SettingsPaths,
    override_path: Option<OsString>,
) -> Option<PathBuf> {
    override_path
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            settings_paths
                .rc_ini_path
                .as_deref()
                .and_then(Path::parent)
                .map(|directory| directory.join(DEFAULT_LOG_FILE_NAME))
        })
}

#[derive(Debug)]
struct TracingLogWriter {
    file: File,
}

impl io::Write for TracingLogWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileExt as _;

            // FILE_WRITE_DATA is needed to truncate an oversized log, so the handle cannot be
            // append-only. An offset of -1 is Windows' atomic "write to EOF" sentinel.
            self.file.seek_write(buffer, u64::MAX)
        }
        #[cfg(not(windows))]
        {
            io::Write::write(&mut self.file, buffer)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        io::Write::flush(&mut self.file)
    }
}

fn open_tracing_log(path: &Path) -> io::Result<TracingLogWriter> {
    open_tracing_log_with_limit(path, MAX_LOG_FILE_BYTES)
}

fn open_tracing_log_with_limit(path: &Path, max_bytes: u64) -> io::Result<TracingLogWriter> {
    if max_bytes == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "tracing log size limit must be greater than zero",
        ));
    }

    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let file = open_tracing_log_file(path)?;
    prepare_tracing_log_file(file, max_bytes).map(|file| TracingLogWriter { file })
}

fn open_tracing_log_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;

        options
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;

        // Rust's append-only access mask omits FILE_WRITE_DATA, which File::set_len requires.
        const GENERIC_WRITE: u32 = 0x4000_0000;
        // Open the reparse point itself so a final-component symlink cannot redirect the log.
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options
            .access_mode(GENERIC_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }

    options.open(path).map_err(|error| {
        if fs::symlink_metadata(path).is_ok_and(|metadata| !metadata.file_type().is_file()) {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "tracing log must be a regular file",
            )
        } else {
            error
        }
    })
}

fn prepare_tracing_log_file(file: File, max_bytes: u64) -> io::Result<File> {
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "tracing log must be a regular file",
        ));
    }
    if metadata.len() >= max_bytes {
        file.set_len(0)?;
    }
    Ok(file)
}

fn init_tracing(log_path: Option<&Path>) -> Option<io::Error> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let (writer, open_error) = match log_path {
        Some(path) => match open_tracing_log(path) {
            Ok(file) => (BoxMakeWriter::new(Mutex::new(file)), None),
            Err(error) => (BoxMakeWriter::new(io::sink), Some(error)),
        },
        None => (BoxMakeWriter::new(io::sink), None),
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_target(false)
        .with_ansi(false)
        .try_init();
    open_error
}

fn report_keymap_parse_report(state: &mut AppState, report: &KeymapParseReport) {
    state.set_keymap_parse_report(report);
    if report.unknown_actions.is_empty() && report.skipped_bindings.is_empty() {
        return;
    }

    if !report.unknown_actions.is_empty() {
        let unknown_sample = report
            .unknown_actions
            .iter()
            .take(5)
            .map(|unknown| {
                format!(
                    "{}:{} [{:?}]",
                    unknown.line, unknown.action, unknown.context
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        tracing::warn!(
            count = report.unknown_actions.len(),
            sample = %unknown_sample,
            "keymap contains unsupported action names",
        );
    }

    if !report.skipped_bindings.is_empty() {
        let skipped_sample = report
            .skipped_bindings
            .iter()
            .take(5)
            .map(|binding| {
                format!(
                    "{}:{}={} ({})",
                    binding.line, binding.action, binding.key_spec, binding.reason
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        tracing::warn!(
            count = report.skipped_bindings.len(),
            sample = %skipped_sample,
            "keymap contains invalid key bindings",
        );
    }

    state.set_status(format!(
        "Keymap loaded with {} unsupported actions and {} invalid bindings (see logs)",
        report.unknown_actions.len(),
        report.skipped_bindings.len(),
    ));
}

fn apply_env_overrides(settings: &mut Settings) {
    apply_env_overrides_with_lookup(settings, |name| std::env::var(name).ok());
}

fn apply_env_overrides_with_lookup(
    settings: &mut Settings,
    mut lookup_env: impl FnMut(&str) -> Option<String>,
) {
    if let Some(value) = lookup_env("RC_SKIN")
        && !value.trim().is_empty()
    {
        settings.appearance.skin = value.trim().to_string();
    }
    if let Some(value) = lookup_env("RC_SKIN_DIR")
        && !value.trim().is_empty()
    {
        settings
            .appearance
            .skin_dirs
            .insert(0, PathBuf::from(value));
    }
    if let Some(value) = lookup_env("RC_KEYMAP")
        && !value.trim().is_empty()
    {
        settings.configuration.keymap_override = Some(PathBuf::from(value));
    }
    if let Some(value) = lookup_env("RC_MACOS_OPTION_COMPAT")
        && let Some(parsed) = settings_io::parse_bool(&value)
    {
        settings.configuration.macos_option_symbols = parsed;
    }
    settings
        .shell
        .apply_environment_with(|name| lookup_env(name));
}

fn apply_cli_overrides(settings: &mut Settings, cli: &Cli) {
    if let Some(skin) = cli.skin.as_ref() {
        settings.appearance.skin = skin.clone();
    }
    if let Some(skin_dir) = cli.skin_dir.as_ref() {
        settings.appearance.skin_dirs.insert(0, skin_dir.clone());
    }
    if let Some(keymap) = cli.keymap.as_ref() {
        settings.configuration.keymap_override = Some(keymap.clone());
    }
    if let Some(macos_option_compat) = cli.macos_option_compat {
        settings.configuration.macos_option_symbols = macos_option_compat;
    }
}

fn load_effective_keymap(
    settings: &Settings,
    state: &mut AppState,
) -> Result<(Keymap, KeymapParseReport)> {
    let (mut keymap, mut report) = Keymap::bundled_mc_default_with_report()
        .context("failed to load bundled mc.default.keymap")?;
    let Some(path) = settings.configuration.keymap_override.as_ref() else {
        return Ok((keymap, report));
    };

    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read keymap override '{}'", path.display()))?;
    let (override_map, override_report) = Keymap::parse_with_report(&source)
        .with_context(|| format!("failed to parse keymap override '{}'", path.display()))?;
    keymap.merge_from(&override_map);
    report
        .unknown_actions
        .extend(override_report.unknown_actions);
    report
        .skipped_bindings
        .extend(override_report.skipped_bindings);
    state.set_status(format!("Loaded keymap overrides from {}", path.display()));
    Ok((keymap, report))
}

struct SkinRuntimeConfig {
    skin_dirs: Vec<PathBuf>,
    settings_paths: settings_io::SettingsPaths,
}

fn run_app(
    state: &mut AppState,
    keymap: &Keymap,
    tick_rate: Duration,
    skin_runtime: &SkinRuntimeConfig,
) -> Result<()> {
    let mut runtime = RuntimeBridge::spawn()?;

    install_emergency_terminal_panic_hook();
    let mut stdout = io::stdout();
    if let Err(error) = enter_tui_terminal_modes(&mut stdout) {
        let shutdown_error = runtime.shutdown().err();
        return Err(append_optional_error(
            error,
            shutdown_error,
            "runtime shutdown",
        ));
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = match Terminal::new(backend).context("failed to create terminal backend") {
        Ok(terminal) => terminal,
        Err(error) => {
            emergency_restore_terminal();
            let shutdown_error = runtime.shutdown().err();
            return Err(append_optional_error(
                error,
                shutdown_error,
                "runtime shutdown",
            ));
        }
    };
    if let Err(error) = terminal.clear().context("failed to clear terminal") {
        emergency_restore_terminal();
        let shutdown_error = runtime.shutdown().err();
        return Err(append_optional_error(
            error,
            shutdown_error,
            "runtime shutdown",
        ));
    }

    let loop_result = run_event_loop(
        &mut terminal,
        state,
        keymap,
        tick_rate,
        &mut runtime,
        skin_runtime,
    );
    queue_deferred_save_before_shutdown(state, &mut runtime);
    let shutdown_result = runtime.shutdown();
    let restore_result = restore_terminal(&mut terminal);

    loop_result?;
    shutdown_result?;
    restore_result?;
    Ok(())
}

fn enter_tui_terminal_modes(stdout: &mut io::Stdout) -> Result<()> {
    if let Err(error) = enable_raw_mode().context("failed to enable raw mode") {
        emergency_restore_terminal();
        return Err(error);
    }
    if let Err(error) = execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )
    .context("failed to enter alternate screen")
    {
        emergency_restore_terminal();
        return Err(error);
    }
    Ok(())
}

fn append_optional_error(
    primary: anyhow::Error,
    secondary: Option<anyhow::Error>,
    secondary_label: &str,
) -> anyhow::Error {
    match secondary {
        Some(secondary) => anyhow!("{primary}; {secondary_label} failed: {secondary}"),
        None => primary,
    }
}

fn queue_deferred_save_before_shutdown(state: &mut AppState, runtime: &mut RuntimeBridge) {
    let _ = state.promote_deferred_persist_settings_request();
    runtime.dispatch_pending_commands(state);
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let mut errors = Vec::new();
    if let Err(error) = execute!(terminal.backend_mut(), DisableBracketedPaste) {
        errors.push(format!("failed to disable bracketed paste: {error}"));
    }
    if let Err(error) = disable_raw_mode() {
        errors.push(format!("failed to disable raw mode: {error}"));
    }
    if let Err(error) = execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    ) {
        errors.push(format!("failed to leave alternate screen: {error}"));
    }
    if let Err(error) = terminal.show_cursor() {
        errors.push(format!("failed to restore cursor: {error}"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(errors.join("; ")))
    }
}

fn install_emergency_terminal_panic_hook() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    let terminal_owner = thread::current().id();
    INSTALLED.call_once(move || {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |information| {
            // A panic hook runs even for worker panics that Tokio or a JoinHandle catches. Only
            // the UI thread owns the terminal modes; restoring them from a recoverable worker
            // panic would leave the still-running event loop drawing outside the alternate screen.
            if panic_thread_owns_terminal(terminal_owner) {
                emergency_restore_terminal();
            }
            previous(information);
        }));
    });
}

fn panic_thread_owns_terminal(terminal_owner: ThreadId) -> bool {
    thread::current().id() == terminal_owner
}

fn emergency_restore_terminal() {
    let mut stdout = io::stdout();
    let _ = execute!(
        stdout,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut AppState,
    keymap: &Keymap,
    tick_rate: Duration,
    runtime: &mut RuntimeBridge,
    skin_runtime: &SkinRuntimeConfig,
) -> Result<()> {
    let mut last_tick = Instant::now();
    let mut mouse_click_tracker = MouseClickTracker::default();

    loop {
        runtime.drain_events(state);
        state.poll_deferred_work();
        runtime.dispatch_pending_commands(state);
        state.expire_status_line();
        dispatch_pending_external_edit_requests(terminal, state);
        dispatch_pending_external_execute_requests(terminal, state);
        dispatch_pending_foreground_shell_requests(terminal, state);
        if TERMINAL_RECOVERY_FAILED.load(AtomicOrdering::Relaxed) {
            return Err(anyhow!(
                "fatal terminal restoration or process-group cleanup failure"
            ));
        }

        terminal
            .draw(|frame| rc_ui::render(frame, state))
            .context("failed to draw frame")?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        let timeout = state
            .deferred_work_delay()
            .map_or(timeout, |delay| timeout.min(delay));
        if event::poll(timeout).context("failed to poll input")? {
            let viewport = terminal.size().context("failed to read terminal size")?;
            let input_event = event::read().context("failed to read input event")?;
            if matches!(&input_event, Event::Key(_)) {
                mouse_click_tracker.clear();
            }
            match input_event {
                Event::Key(key_event)
                    if key_event.kind == KeyEventKind::Press
                        && handle_key(
                            state,
                            keymap,
                            key_event,
                            viewport.width,
                            runtime,
                            skin_runtime,
                            InputCompatibility {
                                macos_option_symbols: state
                                    .settings()
                                    .configuration
                                    .macos_option_symbols,
                            },
                        )? =>
                {
                    return Ok(());
                }
                Event::Mouse(mouse_event)
                    if handle_mouse(
                        state,
                        mouse_event,
                        viewport.width,
                        viewport.height,
                        &mut mouse_click_tracker,
                        runtime,
                        skin_runtime,
                    )? =>
                {
                    return Ok(());
                }
                Event::Paste(payload) => {
                    if state.key_context() == KeyContext::CommandLine {
                        state.clear_xmap();
                        state.handle_command_line_input(CommandLineInput::Paste(payload))?;
                    }
                    // Outside command-line mode paste is intentionally discarded as one event.
                }
                _ => {}
            }
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }
}

fn handle_key(
    state: &mut AppState,
    keymap: &Keymap,
    key_event: KeyEvent,
    viewport_width: u16,
    runtime: &mut RuntimeBridge,
    skin_runtime: &SkinRuntimeConfig,
    input_compatibility: InputCompatibility,
) -> Result<bool> {
    let context = state.key_context();

    if context == KeyContext::CommandLine {
        if let Some(command) =
            command_line_keymap_command(state, keymap, &key_event, input_compatibility)
        {
            return Ok(
                apply_and_dispatch(state, command, runtime, skin_runtime)? == ApplyResult::Quit
            );
        }
        if state.is_xmap_pending() {
            state.clear_xmap();
            state.set_status("Extended keymap command not found");
            return Ok(false);
        }
        if let Some(input) = command_line_input(&key_event) {
            state.handle_command_line_input(input)?;
        }
        return Ok(false);
    }

    if matches!(context, KeyContext::Input | KeyContext::FindDialog)
        && let Some(command) = input_char_command(&key_event)
    {
        return Ok(apply_and_dispatch(state, command, runtime, skin_runtime)? == ApplyResult::Quit);
    }

    let tree_input_command = (context == KeyContext::Tree)
        .then(|| tree_search_input_command(&key_event))
        .flatten();
    let listbox_space_command = (context == KeyContext::Listbox)
        .then(|| input_char_command(&key_event))
        .flatten()
        .filter(|command| matches!(command, AppCommand::DialogInputChar(' ')));

    let Some(chord) = map_key_event_to_chord(key_event, input_compatibility) else {
        return Ok(false);
    };
    if state.capture_learn_keys_chord(chord) {
        return Ok(false);
    }
    let key_command = keymap.resolve(context, chord).or_else(|| {
        if context == KeyContext::ViewerHex {
            keymap.resolve(KeyContext::Viewer, chord)
        } else {
            None
        }
    });
    let command = key_command.and_then(|key_command| {
        AppCommand::from_key_command(context, key_command).or_else(|| {
            (context == KeyContext::FileManagerXMap)
                .then(|| AppCommand::from_key_command(KeyContext::FileManager, key_command))
                .flatten()
        })
    });
    if command.is_none() && tree_input_command.is_none() && listbox_space_command.is_none() {
        if context == KeyContext::FileManagerXMap {
            state.clear_xmap();
            state.set_status("Extended keymap command not found");
        }
        return Ok(false);
    }
    let command = command
        .or(tree_input_command)
        .or(listbox_space_command)
        .expect("a command was checked above");
    let command = rc_ui::resolve_file_manager_navigation(state, command, viewport_width);

    Ok(apply_and_dispatch(state, command, runtime, skin_runtime)? == ApplyResult::Quit)
}

fn command_line_keymap_command(
    state: &AppState,
    keymap: &Keymap,
    key_event: &KeyEvent,
    input_compatibility: InputCompatibility,
) -> Option<AppCommand> {
    if key_event
        .modifiers
        .contains(crossterm::event::KeyModifiers::SUPER)
    {
        return None;
    }
    let context = if state.is_xmap_pending() {
        KeyContext::FileManagerXMap
    } else {
        KeyContext::FileManager
    };
    let chord = map_key_event_to_chord(*key_event, input_compatibility)?;
    let command = keymap
        .resolve(context, chord)
        .and_then(|key_command| AppCommand::from_key_command(context, key_command))?;
    matches!(
        command,
        AppCommand::OpenHelp
            | AppCommand::EnterXMap
            | AppCommand::PutCurrentSelected
            | AppCommand::PutCurrentFullSelected
            | AppCommand::PutCurrentTagged
            | AppCommand::PutOtherTagged
    )
    .then_some(command)
}

fn handle_mouse(
    state: &mut AppState,
    mouse_event: MouseEvent,
    viewport_width: u16,
    viewport_height: u16,
    click_tracker: &mut MouseClickTracker,
    runtime: &mut RuntimeBridge,
    skin_runtime: &SkinRuntimeConfig,
) -> Result<bool> {
    if state.key_context() == KeyContext::CommandLine {
        state.clear_xmap();
        click_tracker.clear();
        let button_height = u16::from(state.show_button_bar());
        let status_row = viewport_height
            .saturating_sub(button_height)
            .saturating_sub(1);
        if matches!(mouse_event.kind, MouseEventKind::Down(MouseButton::Left))
            && mouse_event.row == status_row
            && let Some(cursor) =
                rc_ui::command_line_cursor_for_column(state, mouse_event.column, viewport_width)
        {
            state.handle_command_line_input(CommandLineInput::SetCursor(cursor))?;
        }
        return Ok(false);
    }
    match mouse_event.kind {
        MouseEventKind::Down(MouseButton::Left) => {}
        MouseEventKind::Down(_) => {
            click_tracker.clear();
            return Ok(false);
        }
        _ => return Ok(false),
    }

    let Some(commands) = state.commands_for_left_click(
        mouse_event.column,
        mouse_event.row,
        viewport_width,
        viewport_height,
    ) else {
        click_tracker.clear();
        return Ok(false);
    };
    let primary = commands.primary;
    let activation = commands.activation;
    let is_double_click = click_tracker.register(
        mouse_event.column,
        mouse_event.row,
        state.key_context(),
        commands.target,
        Instant::now(),
    );
    if apply_and_dispatch(state, primary, runtime, skin_runtime)? == ApplyResult::Quit {
        return Ok(true);
    }
    if is_double_click
        && let Some(activation) = activation
        && apply_and_dispatch(state, activation, runtime, skin_runtime)? == ApplyResult::Quit
    {
        return Ok(true);
    }
    Ok(false)
}

fn apply_and_dispatch(
    state: &mut AppState,
    command: AppCommand,
    runtime: &mut RuntimeBridge,
    skin_runtime: &SkinRuntimeConfig,
) -> Result<ApplyResult> {
    let result = state.apply(command)?;
    apply_pending_skin_preview(state, skin_runtime);
    apply_pending_skin_change(state, skin_runtime);
    apply_pending_skin_revert(state, skin_runtime);
    persist_dirty_settings(state, skin_runtime);
    runtime.dispatch_pending_commands(state);
    Ok(result)
}

fn persist_dirty_settings(state: &mut AppState, skin_runtime: &SkinRuntimeConfig) {
    let save_requested = state.take_pending_save_setup();
    if !save_requested {
        return;
    }

    let snapshot = state.persisted_settings_snapshot();
    state.enqueue_worker_job_request(JobRequest::PersistSettings {
        paths: skin_runtime.settings_paths.clone(),
        snapshot: Box::new(snapshot),
    });
}

fn dispatch_pending_external_edit_requests(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut AppState,
) {
    for request in state.take_pending_external_edit_requests() {
        if let Err(error) = run_external_editor_request(terminal, &request) {
            state.set_status(format!("Editor launch failed: {error}"));
        }
    }
}

fn dispatch_pending_external_execute_requests(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut AppState,
) {
    for request in state.take_pending_external_execute_requests() {
        match run_external_execute_request(terminal, &request) {
            Ok(()) => state.set_status(format!(
                "Finished executing {}",
                request.path.to_string_lossy()
            )),
            Err(error) => state.set_status(format!("Execution failed: {error}")),
        }
    }
}

fn dispatch_pending_foreground_shell_requests(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut AppState,
) {
    for request in state.take_pending_foreground_shell_requests() {
        let activation_id = request.activation_id;
        let result = run_foreground_shell_request(terminal, &request)
            .map(exit_status_number)
            .map_err(|error| sanitize_display_field(&error.to_string()));
        state.finish_foreground_shell_request(activation_id, result);
        state.refresh_active_panel();
    }
}

fn run_foreground_shell_request(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    request: &ForegroundShellRequest,
) -> Result<ExitStatus> {
    use std::io::{IsTerminal as _, Write as _};

    suspend_terminal_for_external_command(terminal)?;
    let invocation = request.shell.invocation(&request.command);
    let mut command = Command::new(&invocation.program);
    command
        .args(&invocation.arguments)
        .current_dir(&request.cwd)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let cwd = sanitize_display_field(&request.cwd.to_string_lossy());
    let submitted = sanitize_display_field(&request.command);
    let echo = sanitize_display_line(&format!("{cwd} > {submitted}"));
    let run_result = run_interactive_process_after_prepare(&mut command, || {
        println!("{echo}");
        io::stdout().flush()
    })
    .with_context(|| {
        format!(
            "failed to launch {}",
            sanitize_display_field(&request.shell.identity)
        )
    });

    let acknowledgement_result = if let Ok(status) = &run_result {
        let notice = sanitize_display_line(&format!(
            "[{}:{}] Press Enter to return to rc",
            sanitize_display_field(&request.shell.identity),
            exit_status_number(*status)
        ));
        println!("{notice}");
        let flush_result = io::stdout()
            .flush()
            .context("failed to flush command status");
        if flush_result.is_ok() && io::stdin().is_terminal() {
            let mut acknowledgement = String::new();
            io::stdin()
                .read_line(&mut acknowledgement)
                .context("failed to read command acknowledgement")
                .map(|_| ())
        } else {
            flush_result
        }
    } else {
        Ok(())
    };

    let resume_result = resume_terminal_after_external_command(terminal);
    match (run_result, acknowledgement_result, resume_result) {
        (Ok(status), Ok(()), Ok(())) => Ok(status),
        (Err(error), Ok(()), Ok(()))
        | (Ok(_), Err(error), Ok(()))
        | (Ok(_), Ok(()), Err(error)) => Err(error),
        (run, acknowledgement, resume) => {
            let errors = [run.err(), acknowledgement.err(), resume.err()]
                .into_iter()
                .flatten()
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("; ");
            Err(anyhow!(errors))
        }
    }
}

fn exit_status_number(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        status.signal().map_or(1, |signal| 128 + signal)
    }
    #[cfg(not(unix))]
    {
        1
    }
}

fn run_external_editor_request(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    request: &ExternalEditRequest,
) -> Result<()> {
    #[cfg(windows)]
    let _console_control_guard = rc_platform::ParentConsoleControlGuard::acquire()
        .context("failed to protect rc from external editor console interrupts")?;
    suspend_terminal_for_external_command(terminal)?;
    let run_result = run_external_editor_process(request);
    let resume_result = resume_terminal_after_external_command(terminal);

    match (run_result, resume_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(run_error), Ok(())) => Err(run_error),
        (Ok(()), Err(resume_error)) => Err(resume_error),
        (Err(run_error), Err(resume_error)) => Err(anyhow!(
            "editor command failed: {run_error}; terminal restore failed: {resume_error}"
        )),
    }
}

fn run_external_execute_request(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    request: &ExternalExecuteRequest,
) -> Result<()> {
    #[cfg(windows)]
    let _console_control_guard = rc_platform::ParentConsoleControlGuard::acquire()
        .context("failed to protect rc from executable console interrupts")?;
    suspend_terminal_for_external_command(terminal)?;
    let run_result = run_external_execute_process(request);
    let resume_result = resume_terminal_after_external_command(terminal);

    match (run_result, resume_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(run_error), Ok(())) => Err(run_error),
        (Ok(()), Err(resume_error)) => Err(resume_error),
        (Err(run_error), Err(resume_error)) => Err(anyhow!(
            "executable failed: {run_error}; terminal restore failed: {resume_error}"
        )),
    }
}

fn suspend_terminal_for_external_command(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    let result = suspend_terminal_modes(terminal);
    if let Err(suspend_error) = result {
        let rollback_error = resume_terminal_modes(terminal).err();
        if rollback_error.is_some() {
            TERMINAL_RECOVERY_FAILED.store(true, AtomicOrdering::Relaxed);
            emergency_restore_terminal();
        }
        return Err(append_optional_error(
            suspend_error,
            rollback_error,
            "terminal suspension rollback",
        ));
    }
    Ok(())
}

fn resume_terminal_after_external_command(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    if TERMINAL_RECOVERY_FAILED.load(AtomicOrdering::Relaxed) {
        emergency_restore_terminal();
        return Err(anyhow!(
            "terminal restoration was not confirmed; refusing to re-enter the TUI"
        ));
    }
    match resume_terminal_modes(terminal) {
        Ok(()) => Ok(()),
        Err(error) => {
            TERMINAL_RECOVERY_FAILED.store(true, AtomicOrdering::Relaxed);
            emergency_restore_terminal();
            Err(error)
        }
    }
}

fn suspend_terminal_modes(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    execute!(terminal.backend_mut(), DisableBracketedPaste)
        .context("failed to disable bracketed paste for external command")?;
    disable_raw_mode().context("failed to disable raw mode for external command")?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )
    .context("failed to leave alternate screen for external command")?;
    terminal
        .show_cursor()
        .context("failed to show cursor for external command")?;
    Ok(())
}

fn resume_terminal_modes(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    enable_raw_mode().context("failed to re-enable raw mode after external command")?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )
    .context("failed to re-enter alternate screen after external command")?;
    terminal
        .clear()
        .context("failed to clear terminal after external command")?;
    Ok(())
}

fn run_external_editor_process(request: &ExternalEditRequest) -> Result<()> {
    let command = resolve_external_editor_process_command(request)?;
    let mut process = Command::new(&command.program);
    process
        .args(&command.args)
        .current_dir(&request.cwd)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let status = run_interactive_process(&mut process).with_context(|| {
        format!(
            "failed to launch external editor command '{}'",
            request.editor_command
        )
    })?;
    if !status.success() {
        return Err(anyhow!("external editor exited with {status}"));
    }
    Ok(())
}

fn run_external_execute_process(request: &ExternalExecuteRequest) -> Result<()> {
    let mut process = Command::new(&request.path);
    process
        .current_dir(&request.cwd)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let status = run_interactive_process(&mut process).with_context(|| {
        format!(
            "failed to launch executable '{}'",
            request.path.to_string_lossy()
        )
    })?;
    if !status.success() {
        return Err(anyhow!("executable exited with {status}"));
    }
    Ok(())
}

#[derive(Debug)]
struct InteractiveProcessError {
    error: io::Error,
}

impl InteractiveProcessError {
    #[cfg(unix)]
    fn terminal_fatal(error: io::Error) -> Self {
        TERMINAL_RECOVERY_FAILED.store(true, AtomicOrdering::Relaxed);
        eprintln!(
            "rc: fatal terminal handoff failure: {}",
            sanitize_display_field(&error.to_string())
        );
        Self { error }
    }
}

impl From<io::Error> for InteractiveProcessError {
    fn from(error: io::Error) -> Self {
        Self { error }
    }
}

impl std::fmt::Display for InteractiveProcessError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for InteractiveProcessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

type InteractiveProcessResult = Result<ExitStatus, InteractiveProcessError>;

fn run_interactive_process(command: &mut Command) -> InteractiveProcessResult {
    run_interactive_process_after_prepare(command, || Ok(()))
}

#[cfg(windows)]
fn run_interactive_process_after_prepare(
    command: &mut Command,
    prepare: impl FnOnce() -> io::Result<()>,
) -> InteractiveProcessResult {
    let _console_control_guard =
        rc_platform::ParentConsoleControlGuard::acquire().map_err(InteractiveProcessError::from)?;
    prepare().map_err(InteractiveProcessError::from)?;
    command.status().map_err(InteractiveProcessError::from)
}

#[cfg(not(any(unix, windows)))]
fn run_interactive_process_after_prepare(
    command: &mut Command,
    prepare: impl FnOnce() -> io::Result<()>,
) -> InteractiveProcessResult {
    prepare().map_err(InteractiveProcessError::from)?;
    command.status().map_err(InteractiveProcessError::from)
}

#[cfg(unix)]
fn run_interactive_process_after_prepare(
    command: &mut Command,
    prepare: impl FnOnce() -> io::Result<()>,
) -> InteractiveProcessResult {
    use std::os::unix::process::CommandExt as _;

    use nix::errno::Errno;
    use nix::sys::signal::Signal;
    use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
    use nix::unistd::Pid;

    // Crossterm raw mode has already been disabled. Capture the complete cooked state once,
    // before display output or a child can mutate the controlling terminal.
    let mut terminal = UnixTerminalHandoff::capture()
        .map_err(|error| io_error_context(error, "failed to capture controlling terminal"))?;
    if let Err(error) =
        prepare().map_err(|error| io_error_context(error, "external-command preparation failed"))
    {
        return Err(finalize_interactive_error(terminal.as_mut(), error));
    }

    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let error = io_error_context(error, "failed to spawn interactive command");
            return Err(finalize_interactive_error(terminal.as_mut(), error));
        }
    };
    let child_pid = match i32::try_from(child.id()).map(Pid::from_raw) {
        Ok(child_pid) => child_pid,
        Err(_) => {
            let kill_error = child.kill().err();
            let wait_error = child.wait().err();
            let cleanup_error = match (kill_error, wait_error) {
                (Some(kill_error), Some(wait_error)) => Some(io::Error::other(format!(
                    "failed to kill unrepresentable child: {kill_error}; failed to reap it: \
                     {wait_error}"
                ))),
                (Some(error), None) | (None, Some(error)) => Some(error),
                (None, None) => None,
            };
            let error = combine_io_errors(
                io::Error::other("child process ID does not fit in pid_t"),
                cleanup_error,
                None,
            );
            return Err(finalize_interactive_error(terminal.as_mut(), error));
        }
    };

    if let Some(terminal) = terminal.as_mut()
        && let Err(error) = terminal.handoff(child_pid)
    {
        return Err(abort_interactive_process(
            &mut child,
            child_pid,
            Some(terminal),
            error,
        ));
    }

    let status = loop {
        match waitpid(child_pid, Some(WaitPidFlag::WUNTRACED)) {
            Ok(WaitStatus::Exited(_, code)) => break unix_exit_status_from_code(code),
            Ok(WaitStatus::Signaled(_, signal, dumped_core)) => {
                break unix_exit_status_from_signal(signal, dumped_core);
            }
            Ok(WaitStatus::Stopped(_, Signal::SIGTTIN | Signal::SIGTTOU)) if terminal.is_some() => {
                // The child can touch the terminal in the spawn-to-handoff race. It is now the
                // foreground group, so this is internal and must not suspend rc visibly.
                if let Err(error) = continue_process_group(child_pid) {
                    return Err(abort_interactive_process(
                        &mut child,
                        child_pid,
                        terminal.as_mut(),
                        error,
                    ));
                }
            }
            Ok(WaitStatus::Stopped(_, _)) => {
                let Some(terminal) = terminal.as_mut() else {
                    return Err(abort_interactive_process(
                        &mut child,
                        child_pid,
                        None,
                        io::Error::other("non-terminal foreground command stopped"),
                    ));
                };
                if let Err(error) = terminal.suspend_rc_and_resume_job(child_pid) {
                    return Err(abort_interactive_process(
                        &mut child,
                        child_pid,
                        Some(terminal),
                        error,
                    ));
                }
            }
            Ok(_) => {}
            Err(Errno::EINTR) => {}
            Err(error) => {
                return Err(abort_interactive_process(
                    &mut child,
                    child_pid,
                    terminal.as_mut(),
                    io::Error::from(error),
                ));
            }
        }
    };

    let cleanup_result = shutdown_process_group(child_pid);
    let restore_result = terminal
        .as_mut()
        .map_or(Ok(()), UnixTerminalHandoff::finalize);
    combine_finalizer_results(cleanup_result, restore_result)
        .map_err(InteractiveProcessError::terminal_fatal)?;
    Ok(status)
}

#[cfg(unix)]
fn finalize_interactive_error(
    terminal: Option<&mut UnixTerminalHandoff>,
    error: io::Error,
) -> InteractiveProcessError {
    let restore_error = terminal.and_then(|terminal| terminal.finalize().err());
    let fatal = restore_error.is_some();
    let error = combine_io_errors(error, None, restore_error);
    if fatal {
        InteractiveProcessError::terminal_fatal(error)
    } else {
        InteractiveProcessError::from(error)
    }
}

#[cfg(unix)]
fn abort_interactive_process(
    child: &mut std::process::Child,
    process_group: nix::unistd::Pid,
    terminal: Option<&mut UnixTerminalHandoff>,
    error: io::Error,
) -> InteractiveProcessError {
    let cleanup_error = terminate_process_group(child, process_group).err();
    let restore_error = terminal.and_then(|terminal| terminal.finalize().err());
    let fatal = cleanup_error.is_some() || restore_error.is_some();
    let error = combine_io_errors(error, cleanup_error, restore_error);
    if fatal {
        InteractiveProcessError::terminal_fatal(error)
    } else {
        InteractiveProcessError::from(error)
    }
}

#[cfg(unix)]
fn unix_exit_status_from_code(code: i32) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt as _;

    ExitStatus::from_raw(code << 8)
}

#[cfg(unix)]
fn unix_exit_status_from_signal(signal: nix::sys::signal::Signal, dumped_core: bool) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt as _;

    let core_dump_flag = if dumped_core { 0x80 } else { 0 };
    ExitStatus::from_raw(signal as i32 | core_dump_flag)
}

#[cfg(unix)]
fn continue_process_group(process_group: nix::unistd::Pid) -> io::Result<()> {
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, killpg};

    match killpg(process_group, Signal::SIGCONT) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(io::Error::from(error)),
    }
}

#[cfg(unix)]
fn terminate_process_group(
    child: &mut std::process::Child,
    process_group: nix::unistd::Pid,
) -> io::Result<()> {
    use nix::sys::signal::Signal;

    send_process_group_signal(process_group, Signal::SIGKILL)?;
    let wait_result = child.wait();
    let group_result = wait_for_process_group_exit(process_group, Duration::from_secs(1));
    match (wait_result, group_result) {
        (Ok(_), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(wait_error), Err(group_error)) => Err(io::Error::other(format!(
            "failed to reap command leader: {wait_error}; process group cleanup failed: \
             {group_error}"
        ))),
    }
}

#[cfg(unix)]
fn shutdown_process_group(process_group: nix::unistd::Pid) -> io::Result<()> {
    use nix::sys::signal::Signal;

    if !process_group_exists(process_group)? {
        return Ok(());
    }
    send_process_group_signal(process_group, Signal::SIGHUP)?;
    send_process_group_signal(process_group, Signal::SIGCONT)?;
    if wait_for_process_group_exit(process_group, Duration::from_millis(250)).is_ok() {
        return Ok(());
    }
    send_process_group_signal(process_group, Signal::SIGTERM)?;
    if wait_for_process_group_exit(process_group, Duration::from_millis(750)).is_ok() {
        return Ok(());
    }
    send_process_group_signal(process_group, Signal::SIGKILL)?;
    wait_for_process_group_exit(process_group, Duration::from_secs(1))
}

#[cfg(unix)]
fn send_process_group_signal(
    process_group: nix::unistd::Pid,
    signal: nix::sys::signal::Signal,
) -> io::Result<()> {
    use nix::errno::Errno;
    use nix::sys::signal::killpg;

    match killpg(process_group, signal) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(io::Error::from(error)),
    }
}

#[cfg(unix)]
fn process_group_exists(process_group: nix::unistd::Pid) -> io::Result<bool> {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    let group_target = Pid::from_raw(-process_group.as_raw());
    match kill(group_target, None) {
        Ok(()) | Err(Errno::EPERM) => Ok(true),
        Err(Errno::ESRCH) => Ok(false),
        Err(error) => Err(io::Error::from(error)),
    }
}

#[cfg(unix)]
fn wait_for_process_group_exit(
    process_group: nix::unistd::Pid,
    timeout: Duration,
) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        reap_process_group_children(process_group)?;
        if !process_group_exists(process_group)? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "command process group {} did not exit within {} ms",
                    process_group.as_raw(),
                    timeout.as_millis()
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(unix)]
fn reap_process_group_children(process_group: nix::unistd::Pid) -> io::Result<()> {
    use nix::errno::Errno;
    use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
    use nix::unistd::Pid;

    let group_target = Pid::from_raw(-process_group.as_raw());
    loop {
        match waitpid(group_target, Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Exited(_, _)) | Ok(WaitStatus::Signaled(_, _, _)) => {}
            Ok(WaitStatus::StillAlive) | Err(Errno::ECHILD) => return Ok(()),
            Ok(_) => {}
            Err(Errno::EINTR) => {}
            Err(error) => return Err(io::Error::from(error)),
        }
    }
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminalOwnership {
    Rc,
    Child,
    Finalized,
}

#[cfg(unix)]
struct UnixTerminalHandoff {
    terminal: File,
    rc_process_group: nix::unistd::Pid,
    rc_termios: nix::sys::termios::Termios,
    job_termios: Option<nix::sys::termios::Termios>,
    ownership: TerminalOwnership,
}

#[cfg(unix)]
impl UnixTerminalHandoff {
    fn capture() -> io::Result<Option<Self>> {
        use std::os::fd::AsRawFd as _;

        use nix::fcntl::{FcntlArg, FdFlag, fcntl};
        use nix::unistd::{getpgrp, tcgetpgrp};

        let (terminal, tty_open_error) =
            match OpenOptions::new().read(true).write(true).open("/dev/tty") {
                Ok(terminal) => (terminal, None),
                Err(tty_error) => match File::open("/dev/stdin") {
                    // A separately opened standard-input descriptor is an acceptable fallback
                    // only after tcgetpgrp below verifies that it is rc's controlling terminal.
                    Ok(terminal) => (terminal, Some(tty_error)),
                    Err(stdin_error)
                        if is_not_a_terminal_error(&tty_error)
                            && is_not_a_terminal_error(&stdin_error) =>
                    {
                        return Ok(None);
                    }
                    Err(stdin_error) => {
                        return Err(io::Error::new(
                            tty_error.kind(),
                            format!(
                                "failed to open /dev/tty: {tty_error}; failed to open the stdin \
                                 terminal fallback: {stdin_error}"
                            ),
                        ));
                    }
                },
            };
        let descriptor_flags = fcntl(terminal.as_raw_fd(), FcntlArg::F_GETFD)
            .map(FdFlag::from_bits_truncate)
            .map_err(io::Error::from)
            .map_err(|error| io_error_context(error, "failed to read terminal descriptor flags"))?;
        fcntl(
            terminal.as_raw_fd(),
            FcntlArg::F_SETFD(descriptor_flags | FdFlag::FD_CLOEXEC),
        )
        .map_err(io::Error::from)
        .map_err(|error| io_error_context(error, "failed to set close-on-exec on terminal"))?;

        let rc_process_group = getpgrp();
        let foreground = match tcgetpgrp(&terminal).map_err(io::Error::from) {
            Ok(foreground) => foreground,
            Err(error)
                if tty_open_error.as_ref().is_some_and(is_not_a_terminal_error)
                    && is_not_a_terminal_error(&error) =>
            {
                return Ok(None);
            }
            Err(error) => {
                let error = tty_open_error.map_or_else(
                    || error.to_string(),
                    |tty_error| {
                        format!(
                            "failed to open /dev/tty: {tty_error}; stdin fallback was not a \
                             usable controlling terminal: {error}"
                        )
                    },
                );
                return Err(io::Error::other(error));
            }
        };
        if foreground != rc_process_group {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "rc is not the foreground process group of its controlling terminal",
            ));
        }
        let rc_termios = terminal_attributes(&terminal)
            .map_err(|error| io_error_context(error, "failed to capture cooked terminal state"))?;
        Ok(Some(Self {
            terminal,
            rc_process_group,
            rc_termios,
            job_termios: None,
            ownership: TerminalOwnership::Rc,
        }))
    }

    fn handoff(&mut self, process_group: nix::unistd::Pid) -> io::Result<()> {
        if self.ownership == TerminalOwnership::Finalized {
            return Err(io::Error::other("terminal handoff guard is finalized"));
        }
        with_sigttou_blocked(|| {
            set_terminal_process_group_after_spawn(&self.terminal, process_group).map_err(
                |error| io_error_context(error, "failed to give terminal to command group"),
            )?;
            self.ownership = TerminalOwnership::Child;
            Ok(())
        })?;
        continue_process_group(process_group)
            .map_err(|error| io_error_context(error, "failed to continue command group"))
    }

    fn restore_rc(&mut self, final_restore: bool) -> io::Result<()> {
        if self.ownership == TerminalOwnership::Finalized {
            return Ok(());
        }
        let result = with_sigttou_blocked(|| {
            let foreground_result =
                set_terminal_process_group(&self.terminal, self.rc_process_group);
            let termios_result = set_terminal_attributes(&self.terminal, &self.rc_termios);
            match (foreground_result, termios_result) {
                (Ok(()), Ok(())) => Ok(()),
                (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
                (Err(foreground_error), Err(termios_error)) => Err(io::Error::other(format!(
                    "failed to reclaim terminal foreground group: {foreground_error}; \
                     failed to restore cooked terminal state: {termios_error}"
                ))),
            }
        });
        if result.is_ok() {
            self.ownership = if final_restore {
                TerminalOwnership::Finalized
            } else {
                TerminalOwnership::Rc
            };
        }
        result
    }

    fn finalize(&mut self) -> io::Result<()> {
        self.restore_rc(true)
    }

    fn suspend_rc_and_resume_job(&mut self, process_group: nix::unistd::Pid) -> io::Result<()> {
        use nix::sys::signal::Signal;

        // Snapshot the stopped job before reclaiming or changing any terminal attributes.
        self.job_termios = Some(terminal_attributes(&self.terminal)?);
        self.restore_rc(false)?;
        send_process_group_signal(self.rc_process_group, Signal::SIGTSTP)?;
        self.wait_until_rc_is_foreground()?;

        let job_termios = self
            .job_termios
            .as_ref()
            .ok_or_else(|| io::Error::other("stopped job terminal snapshot is unavailable"))?;
        with_sigttou_blocked(|| {
            set_terminal_process_group(&self.terminal, process_group)?;
            set_terminal_attributes(&self.terminal, job_termios)?;
            self.ownership = TerminalOwnership::Child;
            Ok(())
        })?;
        continue_process_group(process_group)
    }

    fn wait_until_rc_is_foreground(&self) -> io::Result<()> {
        use nix::sys::signal::Signal;
        use nix::unistd::tcgetpgrp;

        loop {
            let foreground = tcgetpgrp(&self.terminal).map_err(io::Error::from)?;
            if foreground == self.rc_process_group {
                return Ok(());
            }
            // A background SIGCONT must stop rc again instead of stealing the terminal.
            send_process_group_signal(self.rc_process_group, Signal::SIGTTIN)?;
        }
    }
}

#[cfg(unix)]
fn is_not_a_terminal_error(error: &io::Error) -> bool {
    use nix::errno::Errno;

    matches!(
        error.raw_os_error(),
        Some(code)
            if code == Errno::ENXIO as i32
                || code == Errno::ENODEV as i32
                || code == Errno::ENOTTY as i32
    )
}

#[cfg(unix)]
impl Drop for UnixTerminalHandoff {
    fn drop(&mut self) {
        let _ = self.finalize();
    }
}

#[cfg(unix)]
struct SigttouMaskGuard {
    previous: nix::sys::signal::SigSet,
    active: bool,
}

#[cfg(unix)]
impl SigttouMaskGuard {
    fn block() -> io::Result<Self> {
        use nix::sys::signal::{SigSet, SigmaskHow, Signal, pthread_sigmask};

        let mut blocked = SigSet::empty();
        blocked.add(Signal::SIGTTOU);
        let mut previous = SigSet::empty();
        pthread_sigmask(SigmaskHow::SIG_BLOCK, Some(&blocked), Some(&mut previous))
            .map_err(io::Error::from)?;
        Ok(Self {
            previous,
            active: true,
        })
    }

    fn restore(&mut self) -> io::Result<()> {
        use nix::sys::signal::{SigmaskHow, pthread_sigmask};

        if !self.active {
            return Ok(());
        }
        pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(&self.previous), None)
            .map_err(io::Error::from)?;
        self.active = false;
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for SigttouMaskGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[cfg(unix)]
fn with_sigttou_blocked<T>(operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let mut mask = SigttouMaskGuard::block()?;
    let operation_result = operation();
    let mask_result = mask.restore();
    match (operation_result, mask_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(operation_error), Err(mask_error)) => Err(io::Error::other(format!(
            "terminal operation failed: {operation_error}; failed to restore signal mask: \
             {mask_error}"
        ))),
    }
}

#[cfg(unix)]
fn terminal_attributes(terminal: &File) -> io::Result<nix::sys::termios::Termios> {
    use nix::errno::Errno;
    use nix::sys::termios::tcgetattr;

    loop {
        match tcgetattr(terminal) {
            Ok(attributes) => return Ok(attributes),
            Err(Errno::EINTR) => {}
            Err(error) => return Err(io::Error::from(error)),
        }
    }
}

#[cfg(unix)]
fn set_terminal_attributes(
    terminal: &File,
    attributes: &nix::sys::termios::Termios,
) -> io::Result<()> {
    use nix::errno::Errno;
    use nix::sys::termios::{SetArg, tcsetattr};

    for attempt in 0..3 {
        let set_arg = if attempt == 0 {
            SetArg::TCSADRAIN
        } else {
            // Darwin may synthesize PENDIN while canonical mode is restored. Flushing pending
            // child input on a retry lets the driver accept the saved flags byte-for-byte.
            SetArg::TCSAFLUSH
        };
        loop {
            match tcsetattr(terminal, set_arg, attributes) {
                Ok(()) => break,
                Err(Errno::EINTR) => {}
                Err(error) => return Err(io::Error::from(error)),
            }
        }
        let observed = terminal_attributes(terminal)?;
        if observed == *attributes {
            return Ok(());
        }
        if attempt == 2 {
            return Err(io::Error::other(
                "terminal driver did not accept the complete saved termios value",
            ));
        }
    }
    unreachable!("the bounded terminal-attribute loop always returns")
}

#[cfg(unix)]
fn set_terminal_process_group(terminal: &File, process_group: nix::unistd::Pid) -> io::Result<()> {
    use nix::errno::Errno;
    use nix::unistd::tcsetpgrp;

    loop {
        match tcsetpgrp(terminal, process_group) {
            Ok(()) => return Ok(()),
            Err(Errno::EINTR) => {}
            Err(error) => return Err(io::Error::from(error)),
        }
    }
}

#[cfg(unix)]
fn set_terminal_process_group_after_spawn(
    terminal: &File,
    process_group: nix::unistd::Pid,
) -> io::Result<()> {
    use nix::errno::Errno;
    use nix::unistd::tcsetpgrp;

    // Darwin can briefly report EPERM while a freshly spawned process group becomes visible to
    // the terminal driver. Keep that race internal, but leave genuine failures bounded.
    let deadline = Instant::now() + Duration::from_millis(100);
    loop {
        match tcsetpgrp(terminal, process_group) {
            Ok(()) => return Ok(()),
            Err(Errno::EINTR) => {}
            Err(Errno::EPERM) if Instant::now() < deadline => std::thread::yield_now(),
            Err(error) => return Err(io::Error::from(error)),
        }
    }
}

#[cfg(unix)]
fn combine_finalizer_results(cleanup: io::Result<()>, restore: io::Result<()>) -> io::Result<()> {
    match (cleanup, restore) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(cleanup_error), Err(restore_error)) => Err(io::Error::other(format!(
            "process group cleanup failed: {cleanup_error}; terminal restoration failed: \
             {restore_error}"
        ))),
    }
}

#[cfg(unix)]
fn combine_io_errors(
    primary: io::Error,
    cleanup: Option<io::Error>,
    restore: Option<io::Error>,
) -> io::Error {
    let mut message = primary.to_string();
    if let Some(error) = cleanup {
        message.push_str(&format!("; process group cleanup failed: {error}"));
    }
    if let Some(error) = restore {
        message.push_str(&format!("; terminal restoration failed: {error}"));
    }
    io::Error::new(primary.kind(), message)
}

#[cfg(unix)]
fn io_error_context(error: io::Error, context: &str) -> io::Error {
    io::Error::new(error.kind(), format!("{context}: {error}"))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExternalProcessCommand {
    program: String,
    args: Vec<OsString>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExternalEditorParseStyle {
    Posix,
    Windows,
}

fn native_external_editor_parse_style() -> ExternalEditorParseStyle {
    if cfg!(windows) {
        ExternalEditorParseStyle::Windows
    } else {
        ExternalEditorParseStyle::Posix
    }
}

fn split_external_editor_command(
    command: &str,
    style: ExternalEditorParseStyle,
) -> Option<Vec<String>> {
    match style {
        ExternalEditorParseStyle::Posix => shlex::split(command),
        ExternalEditorParseStyle::Windows => split_windows_command_line(command),
    }
}

fn split_windows_command_line(command: &str) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut in_quotes = false;
    let mut token_started = false;

    while let Some(ch) = chars.next() {
        if !in_quotes && matches!(ch, ' ' | '\t') {
            if token_started {
                parts.push(std::mem::take(&mut current));
                token_started = false;
            }
            continue;
        }

        if ch == '"' {
            in_quotes = !in_quotes;
            token_started = true;
            continue;
        }

        if ch == '\\' {
            let mut slash_count = 1_usize;
            while matches!(chars.peek(), Some('\\')) {
                chars.next();
                slash_count += 1;
            }

            if matches!(chars.peek(), Some('"')) {
                for _ in 0..(slash_count / 2) {
                    current.push('\\');
                }
                if slash_count.is_multiple_of(2) {
                    chars.next();
                    in_quotes = !in_quotes;
                } else {
                    chars.next();
                    current.push('"');
                }
                token_started = true;
                continue;
            }

            for _ in 0..slash_count {
                current.push('\\');
            }
            token_started = true;
            continue;
        }

        current.push(ch);
        token_started = true;
    }

    if in_quotes {
        return None;
    }
    if token_started {
        parts.push(current);
    }

    Some(parts)
}

fn resolve_external_editor_process_command(
    request: &ExternalEditRequest,
) -> Result<ExternalProcessCommand> {
    let Some(mut parts) = split_external_editor_command(
        &request.editor_command,
        native_external_editor_parse_style(),
    ) else {
        return Err(anyhow!(
            "failed to parse external editor command '{}'",
            request.editor_command
        ));
    };
    if parts.is_empty() {
        return Err(anyhow!("external editor command is empty"));
    }

    let program = parts.remove(0);
    let mut args = Vec::with_capacity(parts.len() + 1);
    let mut inserted_path = false;
    for part in parts {
        if part.contains("{path}") {
            let Some(path) = request.path.to_str() else {
                return Err(anyhow!(
                    "external editor command '{}' uses {{path}} placeholder but selected path is not valid UTF-8",
                    request.editor_command
                ));
            };
            args.push(OsString::from(part.replace("{path}", path)));
            inserted_path = true;
        } else {
            args.push(OsString::from(part));
        }
    }
    if !inserted_path {
        args.push(request.path.as_os_str().to_os_string());
    }

    Ok(ExternalProcessCommand { program, args })
}

fn apply_pending_skin_change(state: &mut AppState, skin_runtime: &SkinRuntimeConfig) {
    let Some(requested_skin) = state.take_pending_skin_change() else {
        return;
    };

    match rc_ui::configure_skin_with_search_roots(&requested_skin, &skin_runtime.skin_dirs) {
        Ok(()) => {
            let applied_skin = rc_ui::current_skin_name();
            state.set_active_skin_name(applied_skin.clone());
            state.mark_settings_dirty();
            state.set_status(format!("Skin changed to {applied_skin}"));
        }
        Err(error) => {
            tracing::warn!("failed to load skin '{}': {error}", requested_skin);
            state.set_status(format!("Skin '{}' unavailable: {error}", requested_skin));
        }
    }
}

fn apply_pending_skin_preview(state: &mut AppState, skin_runtime: &SkinRuntimeConfig) {
    let Some(requested_skin) = state.take_pending_skin_preview() else {
        return;
    };

    match rc_ui::configure_skin_with_search_roots(&requested_skin, &skin_runtime.skin_dirs) {
        Ok(()) => {
            state.set_preview_skin_name(rc_ui::current_skin_name());
        }
        Err(error) => {
            tracing::warn!("failed to preview skin '{}': {error}", requested_skin);
            state.set_status(format!("Skin '{}' unavailable: {error}", requested_skin));
        }
    }
}

fn apply_pending_skin_revert(state: &mut AppState, skin_runtime: &SkinRuntimeConfig) {
    let Some(original_skin) = state.take_pending_skin_revert() else {
        return;
    };

    match rc_ui::configure_skin_with_search_roots(&original_skin, &skin_runtime.skin_dirs) {
        Ok(()) => {
            state.clear_preview_skin_name();
        }
        Err(error) => {
            tracing::warn!("failed to restore skin '{}': {error}", original_skin);
            state.set_status(format!("Skin '{}' unavailable: {error}", original_skin));
        }
    }
}

fn command_line_input(key_event: &KeyEvent) -> Option<CommandLineInput> {
    use crossterm::event::KeyModifiers as CrosstermKeyModifiers;

    if key_event
        .modifiers
        .intersects(CrosstermKeyModifiers::ALT | CrosstermKeyModifiers::SUPER)
    {
        return None;
    }
    let control = key_event.modifiers.contains(CrosstermKeyModifiers::CONTROL);
    if control {
        let CrosstermKeyCode::Char(character) = key_event.code else {
            return None;
        };
        return match character.to_ascii_lowercase() {
            'a' => Some(CommandLineInput::Home),
            'e' => Some(CommandLineInput::End),
            'w' => Some(CommandLineInput::DeletePreviousWord),
            'u' => Some(CommandLineInput::DeleteFromStart),
            'k' => Some(CommandLineInput::DeleteToEnd),
            'c' => Some(CommandLineInput::Clear),
            _ => None,
        };
    }

    match key_event.code {
        CrosstermKeyCode::Char(character) => Some(CommandLineInput::Character(
            shifted_text_character(character, key_event.modifiers),
        )),
        CrosstermKeyCode::Left => Some(CommandLineInput::Left),
        CrosstermKeyCode::Right => Some(CommandLineInput::Right),
        CrosstermKeyCode::Home => Some(CommandLineInput::Home),
        CrosstermKeyCode::End => Some(CommandLineInput::End),
        CrosstermKeyCode::Backspace => Some(CommandLineInput::Backspace),
        CrosstermKeyCode::Delete => Some(CommandLineInput::Delete),
        CrosstermKeyCode::Up => Some(CommandLineInput::HistoryPrevious),
        CrosstermKeyCode::Down => Some(CommandLineInput::HistoryNext),
        CrosstermKeyCode::Tab if key_event.modifiers.contains(CrosstermKeyModifiers::SHIFT) => {
            Some(CommandLineInput::Complete(CompletionIntent::Reverse))
        }
        CrosstermKeyCode::Tab => Some(CommandLineInput::Complete(CompletionIntent::Forward)),
        CrosstermKeyCode::BackTab => Some(CommandLineInput::Complete(CompletionIntent::Reverse)),
        CrosstermKeyCode::Enter => Some(CommandLineInput::Enter),
        CrosstermKeyCode::Esc => Some(CommandLineInput::Escape),
        _ => None,
    }
}

fn shifted_text_character(mut character: char, modifiers: crossterm::event::KeyModifiers) -> char {
    if !modifiers.contains(crossterm::event::KeyModifiers::SHIFT) {
        return character;
    }
    if character.is_ascii_lowercase() {
        character.make_ascii_uppercase();
    } else if let Some(symbol) = map_shifted_ascii_symbol(character) {
        character = symbol;
    }
    character
}

fn input_char_command(key_event: &KeyEvent) -> Option<AppCommand> {
    let no_shortcut_modifiers = !key_event
        .modifiers
        .contains(crossterm::event::KeyModifiers::CONTROL)
        && !key_event
            .modifiers
            .contains(crossterm::event::KeyModifiers::ALT)
        && !key_event
            .modifiers
            .contains(crossterm::event::KeyModifiers::SUPER);

    if no_shortcut_modifiers && let CrosstermKeyCode::Char(ch) = key_event.code {
        return Some(AppCommand::DialogInputChar(ch));
    }

    None
}

fn tree_search_input_command(key_event: &KeyEvent) -> Option<AppCommand> {
    let no_shortcut_modifiers = !key_event.modifiers.intersects(
        crossterm::event::KeyModifiers::CONTROL
            | crossterm::event::KeyModifiers::ALT
            | crossterm::event::KeyModifiers::SUPER,
    );
    if !no_shortcut_modifiers {
        return None;
    }

    match key_event.code {
        CrosstermKeyCode::Char(ch) => Some(AppCommand::TreeSearchAppend(ch)),
        CrosstermKeyCode::Backspace => Some(AppCommand::TreeSearchBackspace),
        _ => None,
    }
}

fn map_key_event_to_chord(
    key_event: KeyEvent,
    input_compatibility: InputCompatibility,
) -> Option<KeyChord> {
    let key_event = normalize_key_event_for_compatibility(key_event, input_compatibility);
    let mut modifiers = KeyModifiers {
        ctrl: key_event
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL),
        alt: key_event
            .modifiers
            .contains(crossterm::event::KeyModifiers::ALT),
        shift: key_event
            .modifiers
            .contains(crossterm::event::KeyModifiers::SHIFT),
    };

    let code = match key_event.code {
        CrosstermKeyCode::Char(ch) => {
            let mut ch = ch;
            if ch.is_ascii_uppercase() {
                modifiers.shift = true;
                KeyCode::Char(ch.to_ascii_lowercase())
            } else {
                if modifiers.shift
                    && let Some(symbol) = map_shifted_ascii_symbol(ch)
                {
                    ch = symbol;
                }
                if !ch.is_ascii_alphabetic() {
                    modifiers.shift = false;
                }
                KeyCode::Char(ch)
            }
        }
        CrosstermKeyCode::Enter => KeyCode::Enter,
        CrosstermKeyCode::Esc => KeyCode::Esc,
        CrosstermKeyCode::Tab => KeyCode::Tab,
        CrosstermKeyCode::BackTab => {
            modifiers.shift = true;
            KeyCode::Tab
        }
        CrosstermKeyCode::Backspace => KeyCode::Backspace,
        CrosstermKeyCode::Up => KeyCode::Up,
        CrosstermKeyCode::Down => KeyCode::Down,
        CrosstermKeyCode::Left => KeyCode::Left,
        CrosstermKeyCode::Right => KeyCode::Right,
        CrosstermKeyCode::Home => KeyCode::Home,
        CrosstermKeyCode::End => KeyCode::End,
        CrosstermKeyCode::PageUp => KeyCode::PageUp,
        CrosstermKeyCode::PageDown => KeyCode::PageDown,
        CrosstermKeyCode::Insert => KeyCode::Insert,
        CrosstermKeyCode::Delete => KeyCode::Delete,
        CrosstermKeyCode::F(number) => KeyCode::F(number),
        _ => return None,
    };

    Some(KeyChord { code, modifiers })
}

fn normalize_key_event_for_compatibility(
    mut key_event: KeyEvent,
    input_compatibility: InputCompatibility,
) -> KeyEvent {
    if !input_compatibility.macos_option_symbols {
        return key_event;
    }

    if key_event
        .modifiers
        .contains(crossterm::event::KeyModifiers::CONTROL)
    {
        return key_event;
    }

    if let CrosstermKeyCode::Char(ch) = key_event.code
        && let Some(mapped) = map_macos_option_symbol(ch)
    {
        key_event.code = CrosstermKeyCode::Char(mapped);
        key_event.modifiers |= crossterm::event::KeyModifiers::ALT;
    }

    key_event
}

fn map_macos_option_symbol(ch: char) -> Option<char> {
    match ch {
        'ß' => Some('s'),
        'ƒ' => Some('f'),
        '†' => Some('t'),
        '˙' => Some('h'),
        '∆' => Some('j'),
        '¬' => Some('l'),
        '¿' => Some('?'),
        '•' | '°' => Some('*'),
        _ => None,
    }
}

fn map_shifted_ascii_symbol(ch: char) -> Option<char> {
    match ch {
        '`' => Some('~'),
        '1' => Some('!'),
        '2' => Some('@'),
        '3' => Some('#'),
        '4' => Some('$'),
        '5' => Some('%'),
        '6' => Some('^'),
        '7' => Some('&'),
        '8' => Some('*'),
        '9' => Some('('),
        '0' => Some(')'),
        '-' => Some('_'),
        '=' => Some('+'),
        '[' => Some('{'),
        ']' => Some('}'),
        '\\' => Some('|'),
        ';' => Some(':'),
        '\'' => Some('"'),
        ',' => Some('<'),
        '.' => Some('>'),
        '/' => Some('?'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{self, RuntimeCommand};
    use crossterm::event::{KeyCode as CrosstermKeyCode, KeyEvent, KeyModifiers};
    use rc_core::{WorkerCommand, build_tree_ready_event};
    use std::sync::atomic::AtomicBool;
    use std::time::{SystemTime, UNIX_EPOCH};
    use std::{env, fs};
    #[cfg(unix)]
    use std::{
        ffi::OsString,
        os::unix::ffi::{OsStrExt, OsStringExt},
    };

    const TEST_VIEWPORT_WIDTH: u16 = 120;

    fn compat_enabled() -> InputCompatibility {
        InputCompatibility {
            macos_option_symbols: true,
        }
    }

    fn compat_disabled() -> InputCompatibility {
        InputCompatibility {
            macos_option_symbols: false,
        }
    }

    fn test_runtime_bridge() -> RuntimeBridge {
        runtime::test_runtime_bridge_with_capacity(4).0
    }

    #[test]
    fn tracing_log_path_defaults_next_to_rc_settings_and_honors_override() {
        let settings_paths = settings_io::SettingsPaths {
            mc_ini_path: None,
            rc_ini_path: Some(PathBuf::from("/config/rc/settings.ini")),
        };

        assert_eq!(
            tracing_log_path(&settings_paths, None),
            Some(PathBuf::from("/config/rc/rc.log"))
        );
        assert_eq!(
            tracing_log_path(&settings_paths, Some(OsString::new())),
            Some(PathBuf::from("/config/rc/rc.log")),
            "an empty override should preserve the default"
        );
        assert_eq!(
            tracing_log_path(
                &settings_paths,
                Some(OsString::from("/logs/interactive.log")),
            ),
            Some(PathBuf::from("/logs/interactive.log"))
        );
    }

    #[test]
    fn tracing_log_appends_and_resets_only_after_reaching_its_limit() {
        use std::io::Write as _;

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after the Unix epoch")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-tracing-log-{stamp}"));
        let path = root.join("nested/rc.log");

        let mut file = open_tracing_log_with_limit(&path, 12).expect("create tracing log");
        file.write_all(b"first\n").expect("write first log entry");
        drop(file);

        let mut file = open_tracing_log_with_limit(&path, 12).expect("reopen tracing log");
        file.write_all(b"second\n")
            .expect("append second log entry");
        drop(file);
        assert_eq!(
            fs::read(&path).expect("read appended tracing log"),
            b"first\nsecond\n"
        );

        drop(open_tracing_log_with_limit(&path, 12).expect("reset oversized tracing log"));
        assert!(fs::read(&path).expect("read reset tracing log").is_empty());

        fs::remove_dir_all(root).expect("remove tracing log test directory");
    }

    #[cfg(unix)]
    #[test]
    fn tracing_log_rejects_symbolic_links() {
        use std::os::unix::fs::symlink;

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after the Unix epoch")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-tracing-symlink-{stamp}"));
        fs::create_dir_all(&root).expect("create tracing log test directory");
        let target = root.join("target.log");
        let link = root.join("rc.log");
        fs::write(&target, b"preserve me").expect("create tracing log target");
        symlink(&target, &link).expect("create tracing log symlink");

        let error = open_tracing_log_with_limit(&link, 1).expect_err("reject tracing log symlink");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            fs::read(&target).expect("read tracing log target"),
            b"preserve me",
            "rejecting the symlink must not truncate its target"
        );
        fs::remove_dir_all(root).expect("remove tracing log test directory");
    }

    #[cfg(unix)]
    #[test]
    fn tracing_log_size_check_uses_the_open_handle_after_a_path_swap() {
        use std::os::unix::fs::symlink;

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after the Unix epoch")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-tracing-race-{stamp}"));
        fs::create_dir_all(&root).expect("create tracing log test directory");
        let path = root.join("rc.log");
        let opened_path = root.join("opened.log");
        let target = root.join("target.log");
        fs::write(&path, b"oversized").expect("create original tracing log");
        fs::write(&target, b"preserve me").expect("create symlink target");

        let file = open_tracing_log_file(&path).expect("securely open the tracing log");
        fs::rename(&path, &opened_path).expect("move the already-open log");
        symlink(&target, &path).expect("replace the pathname with a symlink");
        drop(prepare_tracing_log_file(file, 1).expect("prepare the opened log handle"));

        assert!(
            fs::read(&opened_path)
                .expect("read the file behind the opened handle")
                .is_empty()
        );
        assert_eq!(
            fs::read(&target).expect("read tracing log target"),
            b"preserve me",
            "a path swap must not redirect truncation to the symlink target"
        );
        fs::remove_dir_all(root).expect("remove tracing log test directory");
    }

    #[test]
    fn mouse_click_tracker_requires_matching_recent_clicks() {
        let started_at = Instant::now();
        let mut tracker = MouseClickTracker::default();
        let target = MouseClickTarget::Command(AppCommand::HotlistSelectAt(3));

        assert!(!tracker.register(10, 12, KeyContext::Hotlist, target.clone(), started_at));
        assert!(tracker.register(
            10,
            12,
            KeyContext::Hotlist,
            target.clone(),
            started_at + Duration::from_millis(100),
        ));
        assert!(
            !tracker.register(
                10,
                12,
                KeyContext::Hotlist,
                target.clone(),
                started_at + Duration::from_millis(150),
            ),
            "a completed pair should not turn a third click into another double-click"
        );
        tracker.clear();
        assert!(!tracker.register(
            10,
            12,
            KeyContext::Hotlist,
            target.clone(),
            started_at + Duration::from_millis(175),
        ));
        assert!(!tracker.register(
            11,
            12,
            KeyContext::Hotlist,
            target,
            started_at + Duration::from_millis(200),
        ));
        let tree_target = MouseClickTarget::TreeEntry(PathBuf::from("/tree/entry"));
        assert!(!tracker.register(
            11,
            12,
            KeyContext::Tree,
            tree_target.clone(),
            started_at + Duration::from_millis(250),
        ));
        assert!(!tracker.register(
            11,
            12,
            KeyContext::Tree,
            tree_target,
            started_at + DOUBLE_CLICK_INTERVAL + Duration::from_secs(1),
        ));
    }

    #[test]
    fn double_click_requires_the_same_logical_target_after_viewport_recentering() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-hotlist-recenter-click-{stamp}"));
        fs::create_dir_all(&root).expect("must create hotlist target");
        let mut state = AppState::new(root.clone()).expect("app should initialize");
        state.settings_mut().configuration.hotlist = (0..24)
            .map(|index| rc_core::HotlistEntry::new(format!("Entry {index}"), root.clone()))
            .collect();
        state
            .apply(AppCommand::OpenHotlist)
            .expect("hotlist should open");
        let viewport_width = 40;
        let viewport_height = 12;
        let list = rc_core::layout::hotlist_layout(rc_core::layout::ScreenRect::new(
            0,
            0,
            viewport_width,
            viewport_height,
        ))
        .list;
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: list.x,
            row: list.y + list.height - 1,
            modifiers: KeyModifiers::NONE,
        };
        let (mut runtime, _command_rx) = runtime::test_runtime_bridge_with_capacity(4);
        let skin_runtime = SkinRuntimeConfig {
            skin_dirs: Vec::new(),
            settings_paths: settings_io::SettingsPaths {
                mc_ini_path: None,
                rc_ini_path: None,
            },
        };
        let mut tracker = MouseClickTracker::default();

        let first_target = state
            .commands_for_left_click(click.column, click.row, viewport_width, viewport_height)
            .expect("first click should hit a hotlist entry")
            .primary;
        handle_mouse(
            &mut state,
            click,
            viewport_width,
            viewport_height,
            &mut tracker,
            &mut runtime,
            &skin_runtime,
        )
        .expect("first click should select the hotlist entry");
        let second_target = state
            .commands_for_left_click(click.column, click.row, viewport_width, viewport_height)
            .expect("second click should hit a hotlist entry")
            .primary;
        assert_ne!(first_target, second_target, "the viewport should recenter");

        handle_mouse(
            &mut state,
            click,
            viewport_width,
            viewport_height,
            &mut tracker,
            &mut runtime,
            &skin_runtime,
        )
        .expect("second click should select its newly resolved entry");
        assert_eq!(
            state.key_context(),
            KeyContext::Hotlist,
            "a different logical target at the same coordinates is not a double-click"
        );

        fs::remove_dir_all(root).expect("must remove temp root");
    }

    #[test]
    fn tree_double_click_requires_the_same_entry_after_projection_changes() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-tree-project-click-{stamp}"));
        let alpha = root.join("alpha");
        let parent = root.join("parent");
        let child_a = parent.join("child-a");
        let child_b = parent.join("child-b");
        fs::create_dir_all(&alpha).expect("must create sibling directory");
        fs::create_dir_all(&child_a).expect("must create first child directory");
        fs::create_dir_all(&child_b).expect("must create second child directory");

        let mut state = AppState::new(root.clone()).expect("app should initialize");
        state
            .apply(AppCommand::OpenTree)
            .expect("tree screen should open");
        let scan_job_id = match state.top_route() {
            rc_core::Route::Tree(tree) => tree.scan_job_id().expect("tree scan should be pending"),
            _ => panic!("top route should be tree"),
        };
        state.take_pending_worker_commands();
        let tree_ready =
            build_tree_ready_event(scan_job_id, root.clone(), 8, 64, &AtomicBool::new(false))
                .expect("tree fixture should scan");
        state.handle_background_event(tree_ready);
        state
            .apply(AppCommand::TreeSelectVisibleAt(2))
            .expect("parent should be selectable");
        assert!(matches!(
            state.top_route(),
            rc_core::Route::Tree(tree)
                if tree.selected_entry().is_some_and(|entry| entry.path == parent)
        ));

        let viewport_width = 120;
        let viewport_height = 40;
        let list = rc_core::layout::tree_layout(rc_core::layout::ScreenRect::new(
            0,
            0,
            viewport_width,
            viewport_height,
        ))
        .list;
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: list.x,
            row: list.y + 3,
            modifiers: KeyModifiers::NONE,
        };
        let (mut runtime, _command_rx) = runtime::test_runtime_bridge_with_capacity(4);
        let skin_runtime = SkinRuntimeConfig {
            skin_dirs: Vec::new(),
            settings_paths: settings_io::SettingsPaths {
                mc_ini_path: None,
                rc_ini_path: None,
            },
        };
        let mut tracker = MouseClickTracker::default();

        handle_mouse(
            &mut state,
            click,
            viewport_width,
            viewport_height,
            &mut tracker,
            &mut runtime,
            &skin_runtime,
        )
        .expect("first click should select child-a");
        assert!(matches!(
            state.top_route(),
            rc_core::Route::Tree(tree)
                if tree.selected_entry().is_some_and(|entry| entry.path == child_a)
        ));

        handle_mouse(
            &mut state,
            click,
            viewport_width,
            viewport_height,
            &mut tracker,
            &mut runtime,
            &skin_runtime,
        )
        .expect("second click should select child-b without activating it");
        assert_eq!(
            state.key_context(),
            KeyContext::Tree,
            "the same projection index must not double-click a different tree entry"
        );
        assert!(matches!(
            state.top_route(),
            rc_core::Route::Tree(tree)
                if tree.selected_entry().is_some_and(|entry| entry.path == child_b)
        ));

        handle_mouse(
            &mut state,
            click,
            viewport_width,
            viewport_height,
            &mut tracker,
            &mut runtime,
            &skin_runtime,
        )
        .expect("third click should complete a double-click on child-b");
        assert_eq!(state.key_context(), KeyContext::FileManager);
        assert_eq!(state.active_panel().cwd, child_b);

        fs::remove_dir_all(root).expect("must remove temp root");
    }

    #[test]
    fn hotlist_double_click_opens_directory_and_dispatches_refresh() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-hotlist-double-click-{stamp}"));
        let target = root.join("target");
        fs::create_dir_all(&target).expect("must create hotlist target");
        let expected_target = fs::canonicalize(&target).expect("target should canonicalize");
        let mut state = AppState::new(root.clone()).expect("app should initialize");
        state.settings_mut().configuration.hotlist =
            vec![rc_core::HotlistEntry::new("Target", target.clone())];
        state
            .apply(AppCommand::OpenHotlist)
            .expect("hotlist should open");
        let viewport_width = 120;
        let viewport_height = 40;
        let list = rc_core::layout::hotlist_layout(rc_core::layout::ScreenRect::new(
            0,
            0,
            viewport_width,
            viewport_height,
        ))
        .list;
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: list.x,
            row: list.y,
            modifiers: KeyModifiers::NONE,
        };
        let (mut runtime, mut command_rx) = runtime::test_runtime_bridge_with_capacity(4);
        let skin_runtime = SkinRuntimeConfig {
            skin_dirs: Vec::new(),
            settings_paths: settings_io::SettingsPaths {
                mc_ini_path: None,
                rc_ini_path: None,
            },
        };
        let mut tracker = MouseClickTracker::default();

        handle_mouse(
            &mut state,
            click,
            viewport_width,
            viewport_height,
            &mut tracker,
            &mut runtime,
            &skin_runtime,
        )
        .expect("first click should select the hotlist entry");
        assert_eq!(state.key_context(), KeyContext::Hotlist);

        handle_mouse(
            &mut state,
            click,
            viewport_width,
            viewport_height,
            &mut tracker,
            &mut runtime,
            &skin_runtime,
        )
        .expect("second click should activate the hotlist entry");
        assert_eq!(state.key_context(), KeyContext::FileManager);
        assert_eq!(state.active_panel().cwd, expected_target);
        match command_rx.try_recv() {
            Ok(RuntimeCommand::Worker {
                command: WorkerCommand::Run(job),
                ..
            }) => assert!(matches!(
                &job.request,
                JobRequest::RefreshPanel { cwd, .. } if cwd == &expected_target
            )),
            Ok(other) => panic!("unexpected double-click runtime command: {other:?}"),
            Err(error) => panic!("double-click refresh should dispatch: {error}"),
        }

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn macos_option_symbols_map_to_alt_key_chords() {
        let chord = map_key_event_to_chord(
            KeyEvent::new(CrosstermKeyCode::Char('ƒ'), KeyModifiers::NONE),
            compat_enabled(),
        )
        .expect("option-f should map to a chord");
        assert_eq!(chord.code, KeyCode::Char('f'));
        assert!(chord.modifiers.alt);

        let chord = map_key_event_to_chord(
            KeyEvent::new(CrosstermKeyCode::Char('†'), KeyModifiers::NONE),
            compat_enabled(),
        )
        .expect("option-t should map to a chord");
        assert_eq!(chord.code, KeyCode::Char('t'));
        assert!(chord.modifiers.alt);

        let chord = map_key_event_to_chord(
            KeyEvent::new(CrosstermKeyCode::Char('˙'), KeyModifiers::NONE),
            compat_enabled(),
        )
        .expect("option-h should map to a chord");
        assert_eq!(chord.code, KeyCode::Char('h'));
        assert!(chord.modifiers.alt);

        let chord = map_key_event_to_chord(
            KeyEvent::new(CrosstermKeyCode::Char('ß'), KeyModifiers::NONE),
            compat_enabled(),
        )
        .expect("option-s should map to a chord");
        assert_eq!(chord.code, KeyCode::Char('s'));
        assert!(chord.modifiers.alt);

        let chord = map_key_event_to_chord(
            KeyEvent::new(CrosstermKeyCode::Char('ƒ'), KeyModifiers::ALT),
            compat_enabled(),
        )
        .expect("option-f with ALT modifier should map to a chord");
        assert_eq!(chord.code, KeyCode::Char('f'));
        assert!(chord.modifiers.alt);
    }

    #[test]
    fn macos_option_symbols_do_not_map_when_compat_is_disabled() {
        let chord = map_key_event_to_chord(
            KeyEvent::new(CrosstermKeyCode::Char('ƒ'), KeyModifiers::NONE),
            compat_disabled(),
        )
        .expect("raw symbol should still map to a chord");
        assert_eq!(chord.code, KeyCode::Char('ƒ'));
        assert!(!chord.modifiers.alt);
    }

    #[test]
    fn shifted_symbol_char_drops_shift_modifier_for_lookup() {
        let chord = map_key_event_to_chord(
            KeyEvent::new(CrosstermKeyCode::Char('!'), KeyModifiers::SHIFT),
            compat_enabled(),
        )
        .expect("shift+1 should map to exclamation");
        assert_eq!(chord.code, KeyCode::Char('!'));
        assert!(!chord.modifiers.shift);
    }

    #[test]
    fn shifted_digit_char_maps_to_shifted_symbol_for_lookup() {
        let chord = map_key_event_to_chord(
            KeyEvent::new(CrosstermKeyCode::Char('1'), KeyModifiers::SHIFT),
            compat_enabled(),
        )
        .expect("shift+1 should map to exclamation");
        assert_eq!(chord.code, KeyCode::Char('!'));
        assert!(!chord.modifiers.shift);
    }

    #[test]
    fn settings_precedence_cli_overrides_env_and_persisted_values() {
        let mut settings = Settings::default();
        settings.appearance.skin = String::from("persisted-skin");
        settings.appearance.skin_dirs = vec![PathBuf::from("/persisted/skins")];
        settings.configuration.keymap_override = Some(PathBuf::from("/persisted/keymap"));
        settings.configuration.macos_option_symbols = false;

        apply_env_overrides_with_lookup(&mut settings, |name| match name {
            "RC_SKIN" => Some(String::from("env-skin")),
            "RC_SKIN_DIR" => Some(String::from("/env/skins")),
            "RC_KEYMAP" => Some(String::from("/env/keymap")),
            "RC_MACOS_OPTION_COMPAT" => Some(String::from("off")),
            _ => None,
        });
        assert_eq!(settings.appearance.skin, "env-skin");
        assert_eq!(
            settings.configuration.keymap_override.as_deref(),
            Some(std::path::Path::new("/env/keymap"))
        );
        assert!(!settings.configuration.macos_option_symbols);
        assert_eq!(
            settings.appearance.skin_dirs,
            vec![
                PathBuf::from("/env/skins"),
                PathBuf::from("/persisted/skins")
            ]
        );

        let cli = Cli {
            tick_rate_ms: 200,
            path: None,
            skin: Some(String::from("cli-skin")),
            skin_dir: Some(PathBuf::from("/cli/skins")),
            keymap: Some(PathBuf::from("/cli/keymap")),
            macos_option_compat: Some(true),
        };
        apply_cli_overrides(&mut settings, &cli);

        assert_eq!(settings.appearance.skin, "cli-skin");
        assert_eq!(
            settings.configuration.keymap_override.as_deref(),
            Some(std::path::Path::new("/cli/keymap"))
        );
        assert!(settings.configuration.macos_option_symbols);
        assert_eq!(
            settings.appearance.skin_dirs,
            vec![
                PathBuf::from("/cli/skins"),
                PathBuf::from("/env/skins"),
                PathBuf::from("/persisted/skins")
            ]
        );
    }

    #[test]
    fn settings_precedence_without_cli_macos_option_override_keeps_existing_value() {
        let mut settings = Settings::default();
        settings.configuration.macos_option_symbols = false;

        apply_env_overrides_with_lookup(&mut settings, |name| match name {
            "RC_MACOS_OPTION_COMPAT" => Some(String::from("off")),
            _ => None,
        });

        let cli = Cli {
            tick_rate_ms: 200,
            path: None,
            skin: None,
            skin_dir: None,
            keymap: None,
            macos_option_compat: None,
        };
        apply_cli_overrides(&mut settings, &cli);

        assert!(!settings.configuration.macos_option_symbols);
    }

    #[test]
    fn external_editor_command_parser_appends_path_by_default() {
        let request = ExternalEditRequest {
            editor_command: String::from("nvim --clean"),
            path: PathBuf::from("/tmp/note.txt"),
            cwd: PathBuf::from("/tmp"),
        };

        let command =
            resolve_external_editor_process_command(&request).expect("command should parse");
        assert_eq!(command.program, "nvim");
        assert_eq!(
            command.args,
            vec![OsString::from("--clean"), OsString::from("/tmp/note.txt"),]
        );
    }

    #[test]
    fn external_editor_command_parser_substitutes_path_placeholder() {
        let request = ExternalEditRequest {
            editor_command: String::from("code --goto {path}:1"),
            path: PathBuf::from("/tmp/note.txt"),
            cwd: PathBuf::from("/tmp"),
        };

        let command =
            resolve_external_editor_process_command(&request).expect("command should parse");
        assert_eq!(command.program, "code");
        assert_eq!(
            command.args,
            vec![OsString::from("--goto"), OsString::from("/tmp/note.txt:1")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn external_editor_command_parser_preserves_non_utf8_path_without_placeholder() {
        let non_utf8_name =
            OsString::from_vec(vec![b'n', b'o', b't', b'e', 0x80, b'.', b't', b'x', b't']);
        let request = ExternalEditRequest {
            editor_command: String::from("nvim"),
            path: PathBuf::from(&non_utf8_name),
            cwd: PathBuf::from("/tmp"),
        };

        let command =
            resolve_external_editor_process_command(&request).expect("command should parse");
        assert_eq!(command.program, "nvim");
        assert_eq!(command.args.len(), 1);
        assert_eq!(
            command.args[0].as_os_str().as_bytes(),
            non_utf8_name.as_os_str().as_bytes()
        );
    }

    #[cfg(unix)]
    #[test]
    fn external_editor_command_parser_rejects_non_utf8_path_with_placeholder() {
        let request = ExternalEditRequest {
            editor_command: String::from("code --goto {path}:1"),
            path: PathBuf::from(OsString::from_vec(vec![b'n', b'o', b't', b'e', 0x80])),
            cwd: PathBuf::from("/tmp"),
        };
        let error = resolve_external_editor_process_command(&request)
            .expect_err("non-utf8 placeholder expansion should fail");
        assert!(
            error
                .to_string()
                .contains("selected path is not valid UTF-8"),
            "placeholder expansion should fail loudly for non-utf8 paths"
        );
    }

    #[test]
    fn external_editor_command_parser_rejects_invalid_templates() {
        let request = ExternalEditRequest {
            editor_command: String::from("\"unterminated"),
            path: PathBuf::from("/tmp/note.txt"),
            cwd: PathBuf::from("/tmp"),
        };
        let error =
            resolve_external_editor_process_command(&request).expect_err("parse should fail");
        assert!(
            error
                .to_string()
                .contains("failed to parse external editor command"),
            "invalid shell-like syntax should be rejected"
        );
    }

    #[test]
    fn windows_command_splitter_preserves_drive_letter_paths() {
        let parts = split_external_editor_command(
            r#"C:\Windows\notepad.exe"#,
            ExternalEditorParseStyle::Windows,
        )
        .expect("windows command should parse");
        assert_eq!(parts, vec![String::from(r#"C:\Windows\notepad.exe"#)]);
    }

    #[test]
    fn windows_command_splitter_handles_quoted_program_path() {
        let parts = split_external_editor_command(
            r#""C:\Program Files\Notepad++\notepad++.exe" --goto "{path}:1""#,
            ExternalEditorParseStyle::Windows,
        )
        .expect("windows command should parse");
        assert_eq!(
            parts,
            vec![
                String::from(r#"C:\Program Files\Notepad++\notepad++.exe"#),
                String::from("--goto"),
                String::from("{path}:1"),
            ]
        );
    }

    #[test]
    fn windows_command_splitter_rejects_unterminated_quotes() {
        let parts = split_external_editor_command(
            r#""C:\Program Files\Notepad++\notepad++.exe --goto"#,
            ExternalEditorParseStyle::Windows,
        );
        assert!(
            parts.is_none(),
            "unterminated windows-style quoting should fail to parse"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn interactive_process_isolates_terminal_interrupts() {
        use std::io::{BufRead as _, BufReader, Write as _};
        use std::os::unix::process::ExitStatusExt as _;
        use std::sync::mpsc;
        use std::thread;

        const FIXTURE_ENV: &str = "RC_JOB_CONTROL_TEST_FIXTURE";
        const READY_MARKER: &str = "RC_CHILD_READY";
        const SURVIVED_MARKER: &str = "RC_PARENT_SURVIVED";

        if env::var_os(FIXTURE_ENV).is_some() {
            let terminal = File::open("/dev/stdin").expect("fixture stdin should be a PTY");
            let before = terminal_attributes(&terminal).expect("baseline termios");
            let mut command = Command::new("/bin/sh");
            command
                .args([
                    "-c",
                    "stty -echo eof '^A'; printf 'RC_CHILD_READY\\n'; exec /bin/sleep 30",
                ])
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
            let status = run_interactive_process(&mut command)
                .expect("interactive child should launch and return its signal status");
            assert_eq!(status.signal(), Some(2), "child should receive SIGINT");
            let after = terminal_attributes(&terminal).expect("restored termios");
            assert_eq!(after, before, "signal exit must restore exact termios");
            println!("{SURVIVED_MARKER}");
            return;
        }

        // BSD `script` gives the nested test process a real controlling pseudo-terminal.
        // Sending ETX through the master then exercises the kernel's foreground-process-group
        // signal routing exactly as a user pressing Ctrl-C would.
        let mut session = Command::new("/usr/bin/script")
            .args(["-q", "/dev/null"])
            .arg(env::current_exe().expect("test executable path should resolve"))
            .args([
                "--exact",
                "tests::interactive_process_isolates_terminal_interrupts",
                "--nocapture",
            ])
            .env(FIXTURE_ENV, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("pseudo-terminal fixture should launch");
        let stdout = session
            .stdout
            .take()
            .expect("fixture stdout should be piped");
        let (line_tx, line_rx) = mpsc::channel();
        let transcript_thread = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut transcript = String::new();
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        transcript.push_str(&line);
                        let _ = line_tx.send(line);
                    }
                    Err(error) => {
                        transcript.push_str(&format!("<read error: {error}>"));
                        break;
                    }
                }
            }
            transcript
        });

        let ready_deadline = Instant::now() + Duration::from_secs(5);
        let mut ready = false;
        while Instant::now() < ready_deadline {
            let remaining = ready_deadline.saturating_duration_since(Instant::now());
            match line_rx.recv_timeout(remaining.min(Duration::from_millis(100))) {
                Ok(line) if line.contains(READY_MARKER) => {
                    ready = true;
                    break;
                }
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        if ready {
            // Let the parent complete tcsetpgrp after the child writes its marker.
            thread::sleep(Duration::from_millis(100));
            session
                .stdin
                .as_mut()
                .expect("fixture stdin should be piped")
                .write_all(&[3])
                .expect("Ctrl-C should be written to the pseudo-terminal");
        }
        let exit_deadline = Instant::now() + Duration::from_secs(5);
        let mut status = None;
        while Instant::now() < exit_deadline {
            status = session
                .try_wait()
                .expect("fixture status should be readable");
            if status.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let exited_promptly = status.is_some();
        if !exited_promptly {
            let _ = session.kill();
        }
        let _ = session.wait();
        let transcript = transcript_thread
            .join()
            .expect("reader thread should finish");

        assert!(
            ready,
            "child never became ready; transcript: {transcript:?}"
        );
        assert!(
            exited_promptly,
            "job-control fixture did not exit promptly; transcript: {transcript:?}"
        );
        assert!(
            transcript.contains(SURVIVED_MARKER),
            "Ctrl-C terminated rc's process group; transcript: {transcript:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn descendant_cleanup_kills_stopped_signal_resistant_group_within_bound() {
        use std::os::unix::process::CommandExt as _;
        use std::thread;

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let ready = env::temp_dir().join(format!(
            "rc-foreground-descendant-ready-{}-{stamp}",
            std::process::id()
        ));
        let mut leader = Command::new("/bin/sh");
        leader
            .args([
                "-c",
                "/bin/sh -c 'trap \"\" HUP TERM; : > \"$1\"; sleep 0.2; kill -STOP $$; exec \
                 sleep 30' rc-child \"$1\" & sleep 0.05",
                "rc-test",
            ])
            .arg(&ready)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        let mut leader = leader.spawn().expect("leader should spawn");
        let process_group = nix::unistd::Pid::from_raw(
            i32::try_from(leader.id()).expect("test PID should fit pid_t"),
        );
        let leader_deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if leader
                .try_wait()
                .expect("leader status should be readable")
                .is_some()
            {
                break;
            }
            if Instant::now() >= leader_deadline {
                let _ = send_process_group_signal(process_group, nix::sys::signal::Signal::SIGKILL);
                let _ = leader.wait();
                panic!("shell leader did not exit after starting its background descendant");
            }
            thread::sleep(Duration::from_millis(5));
        }

        let ready_deadline = Instant::now() + Duration::from_secs(1);
        while !ready.exists() && Instant::now() < ready_deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(ready.exists(), "background descendant should become ready");
        assert!(
            process_group_exists(process_group).expect("group existence should be readable"),
            "the stopped descendant should outlive its shell leader"
        );

        let started = Instant::now();
        shutdown_process_group(process_group).expect("bounded cleanup should remove the group");
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(900), "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
        assert!(!process_group_exists(process_group).expect("group check"));
        let _ = fs::remove_file(ready);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn interactive_process_restores_the_exact_cooked_termios_snapshot() {
        const FIXTURE_ENV: &str = "RC_TERMIOS_RESTORE_TEST_FIXTURE";

        if env::var_os(FIXTURE_ENV).is_some() {
            let terminal = File::open("/dev/stdin").expect("fixture stdin should be a PTY");
            for (script, expected_code) in [
                ("stty -echo -icanon -isig eof '^A'", 0),
                ("stty -echo -icanon -isig eof '^A'; exit 7", 7),
            ] {
                let before = terminal_attributes(&terminal).expect("baseline termios");
                let mut command = Command::new("/bin/sh");
                command
                    .args(["-c", script])
                    .stdin(Stdio::inherit())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit());
                let status = run_interactive_process(&mut command).expect("helper should run");
                assert_eq!(status.code(), Some(expected_code));
                let after = terminal_attributes(&terminal).expect("restored termios");
                assert_eq!(after, before, "the complete termios value must be restored");
            }
            return;
        }

        use std::io::Read as _;
        use std::thread;

        let mut session = Command::new("/usr/bin/script")
            .args(["-q", "/dev/null"])
            .arg(env::current_exe().expect("test executable path should resolve"))
            .args([
                "--exact",
                "tests::interactive_process_restores_the_exact_cooked_termios_snapshot",
                "--nocapture",
            ])
            .env(FIXTURE_ENV, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("pseudo-terminal fixture should launch");
        // Keep the script input open so it does not inject an EOF character and set Darwin's
        // transient PENDIN terminal flag while the exact-state assertion is running.
        let stdin_guard = session.stdin.take().expect("script stdin should be piped");
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = session
                .try_wait()
                .expect("fixture status should be readable")
            {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = session.kill();
                break session.wait().expect("killed fixture should be reaped");
            }
            thread::sleep(Duration::from_millis(10));
        };
        drop(stdin_guard);
        let mut stdout = Vec::new();
        session
            .stdout
            .take()
            .expect("script stdout should be piped")
            .read_to_end(&mut stdout)
            .expect("fixture stdout should be readable");
        let mut stderr = Vec::new();
        session
            .stderr
            .take()
            .expect("script stderr should be piped")
            .read_to_end(&mut stderr)
            .expect("fixture stderr should be readable");
        assert!(
            status.success(),
            "PTY fixture failed with {}: stdout={:?}, stderr={:?}",
            status,
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&stderr),
        );
    }

    #[test]
    fn command_line_key_translation_is_exclusive_and_complete() {
        use crossterm::event::KeyModifiers as CrosstermKeyModifiers;

        assert_eq!(
            command_line_input(&KeyEvent::new(
                CrosstermKeyCode::Tab,
                CrosstermKeyModifiers::SHIFT,
            )),
            Some(CommandLineInput::Complete(CompletionIntent::Reverse))
        );
        assert_eq!(
            command_line_input(&KeyEvent::new(
                CrosstermKeyCode::Char('c'),
                CrosstermKeyModifiers::CONTROL,
            )),
            Some(CommandLineInput::Clear)
        );
        assert_eq!(
            command_line_input(&KeyEvent::new(
                CrosstermKeyCode::Char('x'),
                CrosstermKeyModifiers::ALT,
            )),
            None
        );
        assert_eq!(
            command_line_input(&KeyEvent::new(
                CrosstermKeyCode::Char('a'),
                CrosstermKeyModifiers::SHIFT,
            )),
            Some(CommandLineInput::Character('A'))
        );
        assert_eq!(
            command_line_input(&KeyEvent::new(
                CrosstermKeyCode::Char('1'),
                CrosstermKeyModifiers::SHIFT,
            )),
            Some(CommandLineInput::Character('!'))
        );
        assert_eq!(
            command_line_input(&KeyEvent::new(
                CrosstermKeyCode::Char('!'),
                CrosstermKeyModifiers::SHIFT,
            )),
            Some(CommandLineInput::Character('!'))
        );
    }

    #[cfg(unix)]
    #[test]
    fn mc_file_shortcuts_insert_from_panels_while_command_line_is_open() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-command-line-shortcuts-{stamp}"));
        fs::create_dir_all(&root).expect("fixture directory");
        fs::write(root.join("alpha"), b"a").expect("alpha fixture");
        fs::write(root.join("beta"), b"b").expect("beta fixture");

        let mut state = AppState::new(root.clone()).expect("app should initialize");
        state
            .active_panel_mut()
            .refresh()
            .expect("active panel listing");
        state.panels[rc_core::ActivePanel::Right.index()]
            .refresh()
            .expect("passive panel listing");
        let select = |state: &AppState, panel: rc_core::ActivePanel, name: &str| {
            state.panels[panel.index()]
                .entries
                .iter()
                .position(|entry| entry.name == name)
                .expect("fixture entry")
        };
        state.active_panel_mut().cursor = select(&state, rc_core::ActivePanel::Left, "alpha");
        state.open_command_line();
        let request = state
            .take_pending_shell_resolution_request()
            .expect("shell resolution request");
        state.handle_shell_resolution_response(rc_core::resolve_shell_request_blocking(request));

        let keymap = Keymap::bundled_mc_default().expect("bundled keymap should parse");
        let mut runtime = test_runtime_bridge();
        let skin_runtime = SkinRuntimeConfig {
            skin_dirs: Vec::new(),
            settings_paths: settings_io::SettingsPaths {
                mc_ini_path: None,
                rc_ini_path: None,
            },
        };
        let mut press = |state: &mut AppState, code, modifiers| {
            handle_key(
                state,
                &keymap,
                KeyEvent::new(code, modifiers),
                TEST_VIEWPORT_WIDTH,
                &mut runtime,
                &skin_runtime,
                compat_enabled(),
            )
            .expect("shortcut should be handled");
        };

        press(&mut state, CrosstermKeyCode::Enter, KeyModifiers::ALT);
        assert_eq!(state.command_line_model().value(), "alpha");

        state
            .handle_command_line_input(CommandLineInput::Clear)
            .expect("clear selected name");
        for name in ["beta", "alpha"] {
            state.active_panel_mut().cursor = select(&state, rc_core::ActivePanel::Left, name);
            assert!(state.active_panel_mut().toggle_tag_on_cursor());
        }
        press(
            &mut state,
            CrosstermKeyCode::Char('x'),
            KeyModifiers::CONTROL,
        );
        assert!(state.is_xmap_pending());
        press(&mut state, CrosstermKeyCode::Char('t'), KeyModifiers::NONE);
        assert_eq!(state.command_line_model().value(), "alpha beta");
        assert!(!state.is_xmap_pending());

        state
            .handle_command_line_input(CommandLineInput::Clear)
            .expect("clear tagged names");
        state.panels[rc_core::ActivePanel::Right.index()].cursor =
            select(&state, rc_core::ActivePanel::Right, "beta");
        press(
            &mut state,
            CrosstermKeyCode::Char('x'),
            KeyModifiers::CONTROL,
        );
        press(
            &mut state,
            CrosstermKeyCode::Char('t'),
            KeyModifiers::CONTROL,
        );
        assert_eq!(state.command_line_model().value(), "beta");

        state
            .handle_command_line_input(CommandLineInput::Clear)
            .expect("clear passive name");
        state.active_panel_mut().cursor = select(&state, rc_core::ActivePanel::Left, "alpha");
        press(
            &mut state,
            CrosstermKeyCode::Enter,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert_eq!(
            state.command_line_model().value(),
            root.join("alpha").to_string_lossy()
        );

        press(&mut state, CrosstermKeyCode::F(1), KeyModifiers::NONE);
        assert_eq!(state.key_context(), KeyContext::Help);
        let rc_core::Route::Help(help) = state.top_route() else {
            panic!("F1 should open help over the command line");
        };
        assert_eq!(help.current_id(), "command-line");
        state
            .apply(AppCommand::CloseHelp)
            .expect("help should return to the command line");
        assert_eq!(state.key_context(), KeyContext::CommandLine);
        assert_eq!(
            state.command_line_model().value(),
            root.join("alpha").to_string_lossy(),
            "opening help must preserve the command draft"
        );

        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn only_the_ui_thread_owns_emergency_terminal_restoration() {
        let terminal_owner = thread::current().id();
        assert!(panic_thread_owns_terminal(terminal_owner));
        assert!(
            !thread::spawn(move || panic_thread_owns_terminal(terminal_owner))
                .join()
                .expect("ownership check thread should finish")
        );
    }

    #[test]
    fn learn_keys_capture_consumes_next_key_event() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-learn-keys-handle-key-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let mut state = AppState::new(root.clone()).expect("app should initialize");
        let keymap = Keymap::bundled_mc_default().expect("bundled keymap should parse");
        let mut runtime = test_runtime_bridge();
        let skin_runtime = SkinRuntimeConfig {
            skin_dirs: Vec::new(),
            settings_paths: settings_io::SettingsPaths {
                mc_ini_path: None,
                rc_ini_path: None,
            },
        };

        state
            .apply(AppCommand::OpenOptionsLearnKeys)
            .expect("learn keys options should open");
        for _ in 0..4 {
            state
                .apply(AppCommand::DialogListboxDown)
                .expect("selection should move down");
        }
        state
            .apply(AppCommand::DialogAccept)
            .expect("capture should start");

        let quit = handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Char('x'), KeyModifiers::CONTROL),
            TEST_VIEWPORT_WIDTH,
            &mut runtime,
            &skin_runtime,
            compat_enabled(),
        )
        .expect("capture key should be handled");
        assert!(!quit);
        assert_eq!(
            state.settings().learn_keys.last_learned_binding.as_deref(),
            Some("Ctrl-x")
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn brief_listing_left_and_right_follow_responsive_columns() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-brief-navigation-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        for index in 0..12 {
            fs::write(root.join(format!("entry-{index:02}")), index.to_string())
                .expect("brief-listing fixture should be written");
        }
        let mut state = AppState::new(root.clone()).expect("app should initialize");
        state
            .active_panel_mut()
            .refresh()
            .expect("active panel should load fixtures");
        state
            .apply(AppCommand::CycleListingFormat)
            .expect("listing format should cycle to Brief");
        assert_eq!(state.active_panel().entries.len(), 13);
        assert_eq!(
            state.panel_listing_format(state.active_panel),
            rc_core::PanelListingFormat::Brief
        );
        let keymap = Keymap::bundled_mc_default().expect("bundled keymap should parse");
        let mut runtime = test_runtime_bridge();
        let skin_runtime = SkinRuntimeConfig {
            skin_dirs: Vec::new(),
            settings_paths: settings_io::SettingsPaths {
                mc_ini_path: None,
                rc_ini_path: None,
            },
        };

        handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Right, KeyModifiers::NONE),
            TEST_VIEWPORT_WIDTH,
            &mut runtime,
            &skin_runtime,
            compat_enabled(),
        )
        .expect("Right should move to the next Brief column");
        assert_eq!(state.active_panel().cursor, 5);

        state.active_panel_mut().cursor = 10;
        handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Right, KeyModifiers::NONE),
            TEST_VIEWPORT_WIDTH,
            &mut runtime,
            &skin_runtime,
            compat_enabled(),
        )
        .expect("Right at the final Brief column should be handled");
        assert_eq!(state.active_panel().cursor, 10);
        handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Left, KeyModifiers::NONE),
            TEST_VIEWPORT_WIDTH,
            &mut runtime,
            &skin_runtime,
            compat_enabled(),
        )
        .expect("Left should return to the preceding Brief column");
        assert_eq!(state.active_panel().cursor, 5);

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn sort_order_space_key_toggles_reverse() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-sort-order-space-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let mut state = AppState::new(root.clone()).expect("app should initialize");
        state
            .apply(AppCommand::OpenSortOrder)
            .expect("sort-order dialog should open");
        assert_eq!(state.key_context(), KeyContext::Listbox);
        assert!(!state.active_panel().sort_mode.reverse);
        let keymap = Keymap::bundled_mc_default().expect("bundled keymap should parse");
        let mut runtime = test_runtime_bridge();
        let skin_runtime = SkinRuntimeConfig {
            skin_dirs: Vec::new(),
            settings_paths: settings_io::SettingsPaths {
                mc_ini_path: None,
                rc_ini_path: None,
            },
        };

        handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Char(' '), KeyModifiers::NONE),
            TEST_VIEWPORT_WIDTH,
            &mut runtime,
            &skin_runtime,
            compat_enabled(),
        )
        .expect("Space should toggle reverse sorting");
        let rc_core::Route::Dialog(dialog) = state.top_route() else {
            panic!("sort-order dialog should remain open");
        };
        let rc_core::DialogKind::Listbox(listbox) = &dialog.kind else {
            panic!("sort-order dialog should remain a listbox");
        };
        assert!(
            listbox
                .footer_hint
                .as_deref()
                .is_some_and(|hint| hint.contains("Reverse: on"))
        );

        state
            .apply(AppCommand::DialogAccept)
            .expect("sort order should apply");
        assert!(state.active_panel().sort_mode.reverse);

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn tree_unbound_characters_search_while_bound_q_still_closes() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-tree-input-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let mut state = AppState::new(root.clone()).expect("app should initialize");
        state
            .apply(AppCommand::OpenTree)
            .expect("tree screen should open");
        let keymap = Keymap::bundled_mc_default().expect("bundled keymap should parse");
        let mut runtime = test_runtime_bridge();
        let skin_runtime = SkinRuntimeConfig {
            skin_dirs: Vec::new(),
            settings_paths: settings_io::SettingsPaths {
                mc_ini_path: None,
                rc_ini_path: None,
            },
        };

        handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Char('b'), KeyModifiers::NONE),
            TEST_VIEWPORT_WIDTH,
            &mut runtime,
            &skin_runtime,
            compat_enabled(),
        )
        .expect("unbound tree character should be handled");
        assert!(matches!(
            state.top_route(),
            rc_core::Route::Tree(tree) if tree.search_query() == "b"
        ));

        handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Backspace, KeyModifiers::NONE),
            TEST_VIEWPORT_WIDTH,
            &mut runtime,
            &skin_runtime,
            compat_enabled(),
        )
        .expect("tree backspace should be handled");
        assert!(matches!(
            state.top_route(),
            rc_core::Route::Tree(tree) if tree.search_query().is_empty()
        ));

        handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Char('q'), KeyModifiers::NONE),
            TEST_VIEWPORT_WIDTH,
            &mut runtime,
            &skin_runtime,
            compat_enabled(),
        )
        .expect("bound tree quit key should be handled");
        assert_eq!(state.key_context(), KeyContext::FileManager);

        drop(runtime);
        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn ctrl_x_h_opens_hotlist_quick_add_editor() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-ctrlx-hotlist-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let mut state = AppState::new(root.clone()).expect("app should initialize");
        let keymap = Keymap::bundled_mc_default().expect("bundled keymap should parse");
        let mut runtime = test_runtime_bridge();
        let skin_runtime = SkinRuntimeConfig {
            skin_dirs: Vec::new(),
            settings_paths: settings_io::SettingsPaths {
                mc_ini_path: None,
                rc_ini_path: None,
            },
        };

        handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Char('x'), KeyModifiers::CONTROL),
            TEST_VIEWPORT_WIDTH,
            &mut runtime,
            &skin_runtime,
            compat_enabled(),
        )
        .expect("ctrl-x should enter xmap mode");
        handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Char('h'), KeyModifiers::NONE),
            TEST_VIEWPORT_WIDTH,
            &mut runtime,
            &skin_runtime,
            compat_enabled(),
        )
        .expect("ctrl-x h should open hotlist quick-add");

        assert_eq!(state.key_context(), KeyContext::Input);
        assert!(state.status_line.contains("Add hotlist entry"));

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn slash_submits_quick_cd_and_dispatches_the_refresh() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-slash-quick-cd-{stamp}"));
        let child = root.join("d");
        fs::create_dir_all(&child).expect("must create child directory");
        let mut state = AppState::new(root.clone()).expect("app should initialize");
        let keymap = Keymap::bundled_mc_default().expect("bundled keymap should parse");
        let (mut runtime, mut command_rx) = runtime::test_runtime_bridge_with_capacity(4);
        let skin_runtime = SkinRuntimeConfig {
            skin_dirs: Vec::new(),
            settings_paths: settings_io::SettingsPaths {
                mc_ini_path: None,
                rc_ini_path: None,
            },
        };

        handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Char('/'), KeyModifiers::NONE),
            TEST_VIEWPORT_WIDTH,
            &mut runtime,
            &skin_runtime,
            compat_enabled(),
        )
        .expect("slash should open quick cd");
        assert_eq!(state.key_context(), KeyContext::Input);

        handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Char('d'), KeyModifiers::NONE),
            TEST_VIEWPORT_WIDTH,
            &mut runtime,
            &skin_runtime,
            compat_enabled(),
        )
        .expect("directory character should be entered");
        handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Enter, KeyModifiers::NONE),
            TEST_VIEWPORT_WIDTH,
            &mut runtime,
            &skin_runtime,
            compat_enabled(),
        )
        .expect("quick cd should submit");

        assert_eq!(state.key_context(), KeyContext::FileManager);
        assert_eq!(state.active_panel().cwd, child);
        match command_rx.try_recv() {
            Ok(RuntimeCommand::Worker {
                command: WorkerCommand::Run(job),
                ..
            }) => assert!(matches!(
                &job.request,
                JobRequest::RefreshPanel { cwd, .. } if cwd == &child
            )),
            Ok(other) => panic!("unexpected quick cd runtime command: {other:?}"),
            Err(error) => panic!("quick cd refresh should dispatch: {error}"),
        }

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn ctrl_x_exclamation_opens_external_panelize_dialog() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-ctrlx-panelize-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let mut state = AppState::new(root.clone()).expect("app should initialize");
        let keymap = Keymap::bundled_mc_default().expect("bundled keymap should parse");
        let mut runtime = test_runtime_bridge();
        let skin_runtime = SkinRuntimeConfig {
            skin_dirs: Vec::new(),
            settings_paths: settings_io::SettingsPaths {
                mc_ini_path: None,
                rc_ini_path: None,
            },
        };

        handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Char('x'), KeyModifiers::CONTROL),
            TEST_VIEWPORT_WIDTH,
            &mut runtime,
            &skin_runtime,
            compat_enabled(),
        )
        .expect("ctrl-x should enter xmap mode");
        handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Char('!'), KeyModifiers::SHIFT),
            TEST_VIEWPORT_WIDTH,
            &mut runtime,
            &skin_runtime,
            compat_enabled(),
        )
        .expect("ctrl-x ! should open external panelize");

        assert_eq!(state.key_context(), KeyContext::Listbox);
        assert!(
            state.status_line.contains("External panelize"),
            "status line should acknowledge external panelize dialog"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn ctrl_x_shift_digit_opens_external_panelize_dialog() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-ctrlx-panelize-digit-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let mut state = AppState::new(root.clone()).expect("app should initialize");
        let keymap = Keymap::bundled_mc_default().expect("bundled keymap should parse");
        let mut runtime = test_runtime_bridge();
        let skin_runtime = SkinRuntimeConfig {
            skin_dirs: Vec::new(),
            settings_paths: settings_io::SettingsPaths {
                mc_ini_path: None,
                rc_ini_path: None,
            },
        };

        handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Char('x'), KeyModifiers::CONTROL),
            TEST_VIEWPORT_WIDTH,
            &mut runtime,
            &skin_runtime,
            compat_enabled(),
        )
        .expect("ctrl-x should enter xmap mode");
        handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Char('1'), KeyModifiers::SHIFT),
            TEST_VIEWPORT_WIDTH,
            &mut runtime,
            &skin_runtime,
            compat_enabled(),
        )
        .expect("ctrl-x shift+1 should open external panelize");

        assert_eq!(state.key_context(), KeyContext::Listbox);
        assert!(
            state.status_line.contains("External panelize"),
            "status line should acknowledge external panelize dialog"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn save_setup_queues_persist_settings_job_without_sync_write() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-save-setup-job-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut state = AppState::new(root.clone()).expect("app should initialize");
        let (mut runtime, mut command_rx) = runtime::test_runtime_bridge_with_capacity(4);
        let mc_ini = root.join("mc.ini");
        let rc_ini = root.join("settings.ini");
        let skin_runtime = SkinRuntimeConfig {
            skin_dirs: Vec::new(),
            settings_paths: settings_io::SettingsPaths {
                mc_ini_path: Some(mc_ini.clone()),
                rc_ini_path: Some(rc_ini.clone()),
            },
        };

        apply_and_dispatch(
            &mut state,
            AppCommand::SaveSetup,
            &mut runtime,
            &skin_runtime,
        )
        .expect("save setup dispatch should succeed");

        assert!(
            !mc_ini.exists() && !rc_ini.exists(),
            "save setup should enqueue persistence instead of writing inline"
        );

        match command_rx.try_recv() {
            Ok(RuntimeCommand::Worker {
                command: WorkerCommand::Run(job),
                ..
            }) => match &job.request {
                JobRequest::PersistSettings { paths, .. } => {
                    assert_eq!(paths.mc_ini_path.as_deref(), Some(mc_ini.as_path()));
                    assert_eq!(paths.rc_ini_path.as_deref(), Some(rc_ini.as_path()));
                }
                _ => panic!("save setup should enqueue persist settings request"),
            },
            Ok(other) => panic!("unexpected runtime command: {other:?}"),
            Err(error) => panic!("runtime queue should contain a save-setup job: {error}"),
        }

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn shutdown_preparation_queues_deferred_save_setup_request() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-shutdown-deferred-save-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut state = AppState::new(root.clone()).expect("app should initialize");
        let (mut runtime, mut command_rx) = runtime::test_runtime_bridge_with_capacity(4);
        let settings_paths = settings_io::SettingsPaths {
            mc_ini_path: Some(root.join("mc.ini")),
            rc_ini_path: Some(root.join("settings.ini")),
        };
        let first_snapshot = state.persisted_settings_snapshot();
        let mut deferred_snapshot = state.persisted_settings_snapshot();
        deferred_snapshot.appearance.skin = String::from("deferred-shutdown-skin");

        let first_id = state.enqueue_worker_job_request(JobRequest::PersistSettings {
            paths: settings_paths.clone(),
            snapshot: Box::new(first_snapshot),
        });
        runtime.dispatch_pending_commands(&mut state);
        match command_rx.try_recv() {
            Ok(RuntimeCommand::Worker {
                command: WorkerCommand::Run(job),
                ..
            }) => {
                assert_eq!(job.id, first_id, "first save setup should dispatch");
            }
            Ok(other) => panic!("unexpected runtime command for first save setup: {other:?}"),
            Err(error) => panic!("first save setup should dispatch: {error}"),
        }

        let deferred_id = state.enqueue_worker_job_request(JobRequest::PersistSettings {
            paths: settings_paths,
            snapshot: Box::new(deferred_snapshot.clone()),
        });
        assert_eq!(
            deferred_id, first_id,
            "deferred save should attach to the active save request"
        );
        assert!(
            command_rx.try_recv().is_err(),
            "deferred save should not dispatch before shutdown preparation"
        );

        queue_deferred_save_before_shutdown(&mut state, &mut runtime);

        match command_rx.try_recv() {
            Ok(RuntimeCommand::Worker {
                command: WorkerCommand::Run(job),
                ..
            }) => match &job.request {
                JobRequest::PersistSettings { snapshot, .. } => {
                    assert_eq!(
                        snapshot.appearance.skin, deferred_snapshot.appearance.skin,
                        "shutdown preparation should dispatch the deferred save snapshot",
                    );
                }
                other => panic!("expected deferred persist settings request, got {other:?}"),
            },
            Ok(other) => panic!("unexpected runtime command for deferred save setup: {other:?}"),
            Err(error) => panic!("deferred save should dispatch during shutdown prep: {error}"),
        }

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn shutdown_preparation_dispatches_already_pending_save_setup_request() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-shutdown-pending-save-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut state = AppState::new(root.clone()).expect("app should initialize");
        let (mut runtime, mut command_rx) = runtime::test_runtime_bridge_with_capacity(4);
        let settings_paths = settings_io::SettingsPaths {
            mc_ini_path: Some(root.join("mc.ini")),
            rc_ini_path: Some(root.join("settings.ini")),
        };
        let snapshot = state.persisted_settings_snapshot();
        let queued_id = state.enqueue_worker_job_request(JobRequest::PersistSettings {
            paths: settings_paths,
            snapshot: Box::new(snapshot),
        });
        assert!(
            command_rx.try_recv().is_err(),
            "pending save setup job should not dispatch before shutdown preparation"
        );

        queue_deferred_save_before_shutdown(&mut state, &mut runtime);

        match command_rx.try_recv() {
            Ok(RuntimeCommand::Worker {
                command: WorkerCommand::Run(job),
                ..
            }) => {
                assert_eq!(
                    job.id, queued_id,
                    "shutdown preparation should dispatch already pending save setup job"
                );
                assert!(
                    matches!(job.request, JobRequest::PersistSettings { .. }),
                    "dispatched pending job should be persist settings"
                );
            }
            Ok(other) => panic!("unexpected runtime command for pending save setup: {other:?}"),
            Err(error) => {
                panic!("pending save setup should dispatch during shutdown prep: {error}")
            }
        }

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn bounded_runtime_queue_requeues_overflowed_job() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-runtime-overflow-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut state = AppState::new(root.clone()).expect("app should initialize");
        let (mut runtime, _command_rx) = runtime::test_runtime_bridge_with_capacity(1);
        state.enqueue_worker_job_request(JobRequest::Mkdir {
            path: root.join("queued"),
        });
        state.enqueue_worker_job_request(JobRequest::Mkdir {
            path: root.join("overflow"),
        });

        runtime.dispatch_pending_commands(&mut state);

        let counts = state.jobs_status_counts();
        assert_eq!(counts.queued, 2, "overflowed jobs should remain queued");
        assert_eq!(counts.failed, 0, "queue backpressure should not fail jobs");
        assert!(
            state.status_line.contains("runtime queue is full"),
            "status should report queue backpressure"
        );
        let pending = state.take_pending_worker_commands();
        assert_eq!(pending.len(), 1, "overflowed command should be requeued");

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn bounded_runtime_queue_preserves_unsent_commands_after_overflow() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-runtime-overflow-preserve-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut state = AppState::new(root.clone()).expect("app should initialize");
        let (mut runtime, _command_rx) = runtime::test_runtime_bridge_with_capacity(1);
        state.enqueue_worker_job_request(JobRequest::Mkdir {
            path: root.join("queued"),
        });
        let overflowed_id = state.enqueue_worker_job_request(JobRequest::Mkdir {
            path: root.join("overflow"),
        });
        let retained_id = state.enqueue_worker_job_request(JobRequest::Mkdir {
            path: root.join("retained"),
        });

        runtime.dispatch_pending_commands(&mut state);

        let pending = state.take_pending_worker_commands();
        assert_eq!(
            pending.len(),
            2,
            "overflowed command and all subsequent commands should remain pending"
        );
        match &pending[0] {
            WorkerCommand::Run(job) => {
                assert_eq!(
                    job.id, overflowed_id,
                    "first pending command should be the overflowed job"
                );
            }
            other => panic!("expected overflowed run command, got {other:?}"),
        }
        match &pending[1] {
            WorkerCommand::Run(job) => {
                assert_eq!(
                    job.id, retained_id,
                    "latest unsent command should be preserved"
                );
            }
            other => panic!("expected retained run command, got {other:?}"),
        }

        let counts = state.jobs_status_counts();
        assert_eq!(
            counts.queued, 3,
            "queued, overflowed, and retained jobs should stay queued"
        );
        assert_eq!(
            counts.failed, 0,
            "overflowed job should not be marked failed"
        );
        assert_eq!(
            state
                .jobs
                .job(overflowed_id)
                .expect("overflowed job should still have a record")
                .status,
            rc_core::JobStatus::Queued
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }

    #[test]
    fn bounded_runtime_queue_requeues_cancel_command_when_full() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-runtime-overflow-cancel-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut state = AppState::new(root.clone()).expect("app should initialize");
        let (mut runtime, _command_rx) = runtime::test_runtime_bridge_with_capacity(1);
        let job_id = state.enqueue_worker_job_request(JobRequest::Mkdir {
            path: root.join("queued"),
        });
        state
            .apply(AppCommand::CancelJob)
            .expect("cancel should enqueue a cancel command");

        runtime.dispatch_pending_commands(&mut state);

        let pending = state.take_pending_worker_commands();
        assert_eq!(
            pending.len(),
            1,
            "one command should stay pending under backpressure"
        );
        assert!(
            matches!(
                pending.first(),
                Some(WorkerCommand::Run(job))
                    if job.id == job_id && matches!(job.request, JobRequest::Mkdir { .. })
            ),
            "pending command should keep the original work item after cancel dispatches first"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }
}
