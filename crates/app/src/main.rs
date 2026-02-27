#![forbid(unsafe_code)]

use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use clap::{ArgAction, Parser};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode as CrosstermKeyCode, KeyEvent,
    KeyEventKind, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use rc_core::keymap::{KeyChord, KeyCode, KeyContext, KeyModifiers, Keymap, KeymapParseReport};
use rc_core::settings_io;
use rc_core::{AppCommand, AppState, ApplyResult, ExternalEditRequest, JobRequest, Settings};
use tracing_subscriber::EnvFilter;

mod runtime;

use runtime::RuntimeBridge;

#[derive(Debug, Parser)]
#[command(version, about = "Roadmap bootstrap for the rc file manager")]
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
    init_tracing();

    let cli = Cli::parse();
    let settings_paths = settings_io::settings_paths();
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

fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("rc=info,warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .try_init();
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

    enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .context("failed to enter alternate screen")?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("failed to create terminal backend")?;
    terminal.clear().context("failed to clear terminal")?;

    let loop_result = run_event_loop(
        &mut terminal,
        state,
        keymap,
        tick_rate,
        &mut runtime,
        skin_runtime,
    );
    let shutdown_result = runtime.shutdown();
    let restore_result = restore_terminal(&mut terminal);

    loop_result?;
    shutdown_result?;
    restore_result?;
    Ok(())
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode().context("failed to disable raw mode")?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )
    .context("failed to leave alternate screen")?;
    terminal.show_cursor().context("failed to restore cursor")?;
    Ok(())
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

    loop {
        runtime.drain_events(state);
        dispatch_pending_external_edit_requests(terminal, state);

        terminal
            .draw(|frame| rc_ui::render(frame, state))
            .context("failed to draw frame")?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout).context("failed to poll input")? {
            match event::read().context("failed to read input event")? {
                Event::Key(key_event)
                    if key_event.kind == KeyEventKind::Press
                        && handle_key(
                            state,
                            keymap,
                            key_event,
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
                    if handle_mouse(state, mouse_event, runtime, skin_runtime)? =>
                {
                    return Ok(());
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
    runtime: &RuntimeBridge,
    skin_runtime: &SkinRuntimeConfig,
    input_compatibility: InputCompatibility,
) -> Result<bool> {
    let context = state.key_context();

    if context == KeyContext::Input
        && let Some(command) = input_char_command(&key_event)
    {
        return Ok(apply_and_dispatch(state, command, runtime, skin_runtime)? == ApplyResult::Quit);
    }

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
    let Some(key_command) = key_command else {
        if context == KeyContext::FileManagerXMap {
            state.clear_xmap();
            state.set_status("Extended keymap command not found");
        }
        return Ok(false);
    };
    let command = AppCommand::from_key_command(context, key_command).or_else(|| {
        (context == KeyContext::FileManagerXMap)
            .then(|| AppCommand::from_key_command(KeyContext::FileManager, key_command))
            .flatten()
    });
    let Some(command) = command else {
        return Ok(false);
    };

    Ok(apply_and_dispatch(state, command, runtime, skin_runtime)? == ApplyResult::Quit)
}

fn handle_mouse(
    state: &mut AppState,
    mouse_event: MouseEvent,
    runtime: &RuntimeBridge,
    skin_runtime: &SkinRuntimeConfig,
) -> Result<bool> {
    if !matches!(mouse_event.kind, MouseEventKind::Down(MouseButton::Left)) {
        return Ok(false);
    }

    let Some(command) = state.command_for_left_click(mouse_event.column, mouse_event.row) else {
        return Ok(false);
    };
    Ok(apply_and_dispatch(state, command, runtime, skin_runtime)? == ApplyResult::Quit)
}

fn apply_and_dispatch(
    state: &mut AppState,
    command: AppCommand,
    runtime: &RuntimeBridge,
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

fn run_external_editor_request(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    request: &ExternalEditRequest,
) -> Result<()> {
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

fn suspend_terminal_for_external_command(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    disable_raw_mode().context("failed to disable raw mode for external editor")?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )
    .context("failed to leave alternate screen for external editor")?;
    terminal
        .show_cursor()
        .context("failed to show cursor for external editor")?;
    Ok(())
}

fn resume_terminal_after_external_command(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    enable_raw_mode().context("failed to re-enable raw mode after external editor")?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture
    )
    .context("failed to re-enter alternate screen after external editor")?;
    terminal
        .clear()
        .context("failed to clear terminal after external editor")?;
    Ok(())
}

#[cfg(unix)]
fn run_external_editor_process(request: &ExternalEditRequest) -> Result<()> {
    let command = format!("{} \"$1\"", request.editor_command);
    let status = Command::new("sh")
        .arg("-c")
        .arg(command)
        .arg("rc-editor")
        .arg(&request.path)
        .current_dir(&request.cwd)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| {
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

#[cfg(windows)]
fn run_external_editor_process(request: &ExternalEditRequest) -> Result<()> {
    let escaped_path = request.path.to_string_lossy().replace('"', "\"\"");
    let command = format!("{} \"{}\"", request.editor_command, escaped_path);
    let status = Command::new("cmd")
        .arg("/C")
        .arg(command)
        .current_dir(&request.cwd)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| {
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

fn apply_pending_skin_change(state: &mut AppState, skin_runtime: &SkinRuntimeConfig) {
    let Some(requested_skin) = state.take_pending_skin_change() else {
        return;
    };

    match rc_ui::configure_skin_with_search_roots(&requested_skin, &skin_runtime.skin_dirs) {
        Ok(()) => {
            let applied_skin = rc_ui::current_skin_name();
            state.set_active_skin_name(applied_skin.clone());
            state.settings_mut().appearance.skin = applied_skin.clone();
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
            state.set_active_skin_name(rc_ui::current_skin_name());
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
            state.set_active_skin_name(rc_ui::current_skin_name());
        }
        Err(error) => {
            tracing::warn!("failed to restore skin '{}': {error}", original_skin);
            state.set_status(format!("Skin '{}' unavailable: {error}", original_skin));
        }
    }
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
    use rc_core::WorkerCommand;
    use std::time::{SystemTime, UNIX_EPOCH};
    use std::{env, fs};

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
    fn learn_keys_capture_consumes_next_key_event() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-learn-keys-handle-key-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let mut state = AppState::new(root.clone()).expect("app should initialize");
        let keymap = Keymap::bundled_mc_default().expect("bundled keymap should parse");
        let runtime = test_runtime_bridge();
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
            &runtime,
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
    fn ctrl_x_exclamation_opens_external_panelize_dialog() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-ctrlx-panelize-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");
        let mut state = AppState::new(root.clone()).expect("app should initialize");
        let keymap = Keymap::bundled_mc_default().expect("bundled keymap should parse");
        let runtime = test_runtime_bridge();
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
            &runtime,
            &skin_runtime,
            compat_enabled(),
        )
        .expect("ctrl-x should enter xmap mode");
        handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Char('!'), KeyModifiers::SHIFT),
            &runtime,
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
        let runtime = test_runtime_bridge();
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
            &runtime,
            &skin_runtime,
            compat_enabled(),
        )
        .expect("ctrl-x should enter xmap mode");
        handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Char('1'), KeyModifiers::SHIFT),
            &runtime,
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
        let (runtime, mut command_rx) = runtime::test_runtime_bridge_with_capacity(4);
        let mc_ini = root.join("mc.ini");
        let rc_ini = root.join("settings.ini");
        let skin_runtime = SkinRuntimeConfig {
            skin_dirs: Vec::new(),
            settings_paths: settings_io::SettingsPaths {
                mc_ini_path: Some(mc_ini.clone()),
                rc_ini_path: Some(rc_ini.clone()),
            },
        };

        apply_and_dispatch(&mut state, AppCommand::SaveSetup, &runtime, &skin_runtime)
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
    fn bounded_runtime_queue_marks_overflowed_job_failed() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-runtime-overflow-{stamp}"));
        fs::create_dir_all(&root).expect("must create temp root");

        let mut state = AppState::new(root.clone()).expect("app should initialize");
        let (runtime, _command_rx) = runtime::test_runtime_bridge_with_capacity(1);
        state.enqueue_worker_job_request(JobRequest::Mkdir {
            path: root.join("queued"),
        });
        state.enqueue_worker_job_request(JobRequest::Mkdir {
            path: root.join("overflow"),
        });

        runtime.dispatch_pending_commands(&mut state);

        let counts = state.jobs_status_counts();
        assert_eq!(counts.queued, 1, "first job should remain queued");
        assert_eq!(counts.failed, 1, "overflowed job should be marked failed");
        assert!(
            state.status_line.contains("runtime queue is full"),
            "status should report queue backpressure"
        );

        fs::remove_dir_all(&root).expect("must remove temp root");
    }
}
