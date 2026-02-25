#![forbid(unsafe_code)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use clap::Parser;
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
use rc_core::{
    AppCommand, AppState, ApplyResult, BackgroundCommand, BackgroundEvent, JobEvent, WorkerCommand,
    run_background_worker, run_worker,
};
use tracing_subscriber::EnvFilter;

const DEFAULT_SKIN_NAME: &str = "default";
const MC_CONFIG_SECTION: &str = "Midnight-Commander";
const MC_SKIN_KEY: &str = "skin";

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
}

fn main() -> Result<()> {
    init_tracing();

    let cli = Cli::parse();
    let mc_ini_path = mc_ini_path();
    let configured_skin =
        mc_ini_path
            .as_deref()
            .and_then(|path| match read_skin_from_mc_ini(path) {
                Ok(skin) => skin,
                Err(error) => {
                    tracing::warn!("failed to read mc config '{}': {error}", path.display());
                    None
                }
            });
    let initial_skin = cli
        .skin
        .clone()
        .or(configured_skin)
        .unwrap_or_else(|| String::from(DEFAULT_SKIN_NAME));
    let start_path = cli
        .path
        .unwrap_or(std::env::current_dir().context("failed to resolve current directory")?);
    let mut state = AppState::new(start_path).context("failed to initialize app state")?;
    state.set_available_skins(rc_ui::list_available_skins(cli.skin_dir.as_deref()));
    if let Err(error) = rc_ui::configure_skin(&initial_skin, cli.skin_dir.as_deref()) {
        tracing::warn!("failed to load skin '{}': {error}", initial_skin);
        state.set_status(format!("Skin '{}' unavailable: {error}", initial_skin));
    }
    state.set_active_skin_name(rc_ui::current_skin_name());
    let (keymap, keymap_report) = Keymap::bundled_mc_default_with_report()
        .context("failed to load bundled mc.default.keymap")?;
    report_keymap_parse_report(&mut state, &keymap_report);
    let skin_runtime = SkinRuntimeConfig {
        skin_dir: cli.skin_dir.clone(),
        mc_ini_path,
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

struct SkinRuntimeConfig {
    skin_dir: Option<PathBuf>,
    mc_ini_path: Option<PathBuf>,
}

fn run_app(
    state: &mut AppState,
    keymap: &Keymap,
    tick_rate: Duration,
    skin_runtime: &SkinRuntimeConfig,
) -> Result<()> {
    let (worker_tx, worker_rx) = mpsc::channel();
    let (worker_event_tx, worker_event_rx) = mpsc::channel();
    let worker_handle = thread::Builder::new()
        .name(String::from("rc-worker"))
        .spawn(move || run_worker(worker_rx, worker_event_tx))
        .map_err(|error| anyhow!("failed to spawn worker thread: {error}"))?;
    let (background_tx, background_rx) = mpsc::channel();
    let (background_event_tx, background_event_rx) = mpsc::channel();
    let background_handle = thread::Builder::new()
        .name(String::from("rc-background"))
        .spawn(move || run_background_worker(background_rx, background_event_tx))
        .map_err(|error| anyhow!("failed to spawn background worker thread: {error}"))?;

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
        RuntimeChannels {
            worker_tx: &worker_tx,
            worker_event_rx: &worker_event_rx,
            background_tx: &background_tx,
            background_event_rx: &background_event_rx,
        },
        skin_runtime,
    );
    let shutdown_result = shutdown_worker(worker_tx, worker_handle);
    let shutdown_background_result = shutdown_background_worker(background_tx, background_handle);
    let restore_result = restore_terminal(&mut terminal);

    loop_result?;
    shutdown_result?;
    shutdown_background_result?;
    restore_result?;
    Ok(())
}

fn shutdown_worker(
    worker_tx: Sender<WorkerCommand>,
    worker_handle: thread::JoinHandle<()>,
) -> Result<()> {
    let _ = worker_tx.send(WorkerCommand::Shutdown);
    worker_handle
        .join()
        .map_err(|_| anyhow!("worker thread panicked"))?;
    Ok(())
}

fn shutdown_background_worker(
    background_tx: Sender<BackgroundCommand>,
    background_handle: thread::JoinHandle<()>,
) -> Result<()> {
    let _ = background_tx.send(BackgroundCommand::Shutdown);
    background_handle
        .join()
        .map_err(|_| anyhow!("background worker thread panicked"))?;
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

struct RuntimeChannels<'a> {
    worker_tx: &'a Sender<WorkerCommand>,
    worker_event_rx: &'a Receiver<JobEvent>,
    background_tx: &'a Sender<BackgroundCommand>,
    background_event_rx: &'a Receiver<BackgroundEvent>,
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut AppState,
    keymap: &Keymap,
    tick_rate: Duration,
    channels: RuntimeChannels<'_>,
    skin_runtime: &SkinRuntimeConfig,
) -> Result<()> {
    let mut last_tick = Instant::now();
    let mut worker_disconnected = false;
    let mut background_disconnected = false;

    loop {
        drain_worker_events(state, channels.worker_event_rx, &mut worker_disconnected);
        drain_background_events(
            state,
            channels.background_event_rx,
            &mut background_disconnected,
        );

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
                            channels.worker_tx,
                            channels.background_tx,
                            skin_runtime,
                        )? =>
                {
                    return Ok(());
                }
                Event::Mouse(mouse_event)
                    if handle_mouse(
                        state,
                        mouse_event,
                        channels.worker_tx,
                        channels.background_tx,
                        skin_runtime,
                    )? =>
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
    worker_tx: &Sender<WorkerCommand>,
    background_tx: &Sender<BackgroundCommand>,
    skin_runtime: &SkinRuntimeConfig,
) -> Result<bool> {
    let context = state.key_context();

    if context == KeyContext::Input
        && let Some(command) = input_char_command(&key_event)
    {
        return Ok(
            apply_and_dispatch(state, command, worker_tx, background_tx, skin_runtime)?
                == ApplyResult::Quit,
        );
    }

    let Some(chord) = map_key_event_to_chord(key_event) else {
        return Ok(false);
    };
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

    Ok(
        apply_and_dispatch(state, command, worker_tx, background_tx, skin_runtime)?
            == ApplyResult::Quit,
    )
}

fn handle_mouse(
    state: &mut AppState,
    mouse_event: MouseEvent,
    worker_tx: &Sender<WorkerCommand>,
    background_tx: &Sender<BackgroundCommand>,
    skin_runtime: &SkinRuntimeConfig,
) -> Result<bool> {
    if !matches!(mouse_event.kind, MouseEventKind::Down(MouseButton::Left)) {
        return Ok(false);
    }

    let Some(command) = state.command_for_left_click(mouse_event.column, mouse_event.row) else {
        return Ok(false);
    };
    Ok(
        apply_and_dispatch(state, command, worker_tx, background_tx, skin_runtime)?
            == ApplyResult::Quit,
    )
}

fn apply_and_dispatch(
    state: &mut AppState,
    command: AppCommand,
    worker_tx: &Sender<WorkerCommand>,
    background_tx: &Sender<BackgroundCommand>,
    skin_runtime: &SkinRuntimeConfig,
) -> Result<ApplyResult> {
    let result = state.apply(command)?;
    apply_pending_skin_preview(state, skin_runtime);
    apply_pending_skin_change(state, skin_runtime);
    apply_pending_skin_revert(state, skin_runtime);
    dispatch_pending_worker_commands(state, worker_tx);
    dispatch_pending_background_commands(state, background_tx);
    Ok(result)
}

fn dispatch_pending_worker_commands(state: &mut AppState, worker_tx: &Sender<WorkerCommand>) {
    for command in state.take_pending_worker_commands() {
        let run_job_id = match &command {
            WorkerCommand::Run(job) => Some(job.id),
            _ => None,
        };
        if let Err(error) = worker_tx.send(command) {
            if let Some(job_id) = run_job_id {
                state.handle_job_dispatch_failure(
                    job_id,
                    format!("worker channel is unavailable: {error}"),
                );
            } else {
                state.set_status(format!("worker channel is unavailable: {error}"));
            }
            break;
        }
    }
}

fn dispatch_pending_background_commands(
    state: &mut AppState,
    background_tx: &Sender<BackgroundCommand>,
) {
    for command in state.take_pending_background_commands() {
        if let Err(error) = background_tx.send(command) {
            state.set_status(format!("background worker channel is unavailable: {error}"));
            break;
        }
    }
}

fn apply_pending_skin_change(state: &mut AppState, skin_runtime: &SkinRuntimeConfig) {
    let Some(requested_skin) = state.take_pending_skin_change() else {
        return;
    };

    match rc_ui::configure_skin(&requested_skin, skin_runtime.skin_dir.as_deref()) {
        Ok(()) => {
            let applied_skin = rc_ui::current_skin_name();
            state.set_active_skin_name(applied_skin.clone());
            if let Some(path) = skin_runtime.mc_ini_path.as_deref()
                && let Err(error) = write_skin_to_mc_ini(path, &applied_skin)
            {
                tracing::warn!(
                    "failed to persist skin '{}' to '{}': {error}",
                    applied_skin,
                    path.display()
                );
                state.set_status(format!("Skin changed to {applied_skin} (save failed)"));
                return;
            }
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

    match rc_ui::configure_skin(&requested_skin, skin_runtime.skin_dir.as_deref()) {
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

    match rc_ui::configure_skin(&original_skin, skin_runtime.skin_dir.as_deref()) {
        Ok(()) => {
            state.set_active_skin_name(rc_ui::current_skin_name());
        }
        Err(error) => {
            tracing::warn!("failed to restore skin '{}': {error}", original_skin);
            state.set_status(format!("Skin '{}' unavailable: {error}", original_skin));
        }
    }
}

fn mc_ini_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/mc/ini"))
}

fn read_skin_from_mc_ini(path: &Path) -> io::Result<Option<String>> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };

    let mut in_mc_section = false;
    for raw_line in source.lines() {
        let line = raw_line.trim();
        if let Some(section_name) = parse_ini_section_name(line) {
            in_mc_section = section_name.eq_ignore_ascii_case(MC_CONFIG_SECTION);
            continue;
        }
        if !in_mc_section || line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case(MC_SKIN_KEY) {
            let value = value.trim();
            if value.is_empty() {
                return Ok(None);
            }
            return Ok(Some(value.to_string()));
        }
    }

    Ok(None)
}

fn write_skin_to_mc_ini(path: &Path, skin: &str) -> io::Result<()> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let updated = upsert_skin_in_mc_ini(&source, skin);

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, updated)
}

fn upsert_skin_in_mc_ini(source: &str, skin: &str) -> String {
    let mut lines: Vec<String> = source.lines().map(|line| line.to_string()).collect();
    let mut section_start = None;

    for (index, line) in lines.iter().enumerate() {
        if let Some(section_name) = parse_ini_section_name(line)
            && section_name.eq_ignore_ascii_case(MC_CONFIG_SECTION)
        {
            section_start = Some(index);
            break;
        }
    }

    match section_start {
        Some(start) => {
            let section_end = lines
                .iter()
                .enumerate()
                .skip(start + 1)
                .find_map(|(index, line)| parse_ini_section_name(line).map(|_| index))
                .unwrap_or(lines.len());
            let skin_line = (start + 1..section_end).find(|line_index| {
                let line = lines[*line_index].trim();
                if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                    return false;
                }
                line.split_once('=')
                    .is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case(MC_SKIN_KEY))
            });

            if let Some(line_index) = skin_line {
                lines[line_index] = format!("{MC_SKIN_KEY}={skin}");
            } else {
                lines.insert(section_end, format!("{MC_SKIN_KEY}={skin}"));
            }
        }
        None => {
            if !lines.is_empty() && !lines.last().is_some_and(|line| line.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("[{MC_CONFIG_SECTION}]"));
            lines.push(format!("{MC_SKIN_KEY}={skin}"));
        }
    }

    let mut output = lines.join("\n");
    output.push('\n');
    output
}

fn parse_ini_section_name(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.starts_with('[') && line.ends_with(']') {
        return Some(line[1..line.len() - 1].trim());
    }
    None
}

fn drain_worker_events(
    state: &mut AppState,
    worker_event_rx: &Receiver<JobEvent>,
    worker_disconnected: &mut bool,
) {
    loop {
        match worker_event_rx.try_recv() {
            Ok(event) => state.handle_job_event(event),
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                if !*worker_disconnected {
                    state.set_status("Worker channel disconnected");
                    *worker_disconnected = true;
                }
                break;
            }
        }
    }
}

fn drain_background_events(
    state: &mut AppState,
    background_event_rx: &Receiver<BackgroundEvent>,
    background_disconnected: &mut bool,
) {
    loop {
        match background_event_rx.try_recv() {
            Ok(event) => state.handle_background_event(event),
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                if !*background_disconnected {
                    state.set_status("Background worker channel disconnected");
                    *background_disconnected = true;
                }
                break;
            }
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

fn map_key_event_to_chord(key_event: KeyEvent) -> Option<KeyChord> {
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
            if !modifiers.ctrl
                && let Some(mapped) = map_macos_option_symbol(ch)
            {
                modifiers.alt = true;
                ch = mapped;
            }
            if ch.is_ascii_uppercase() {
                modifiers.shift = true;
                KeyCode::Char(ch.to_ascii_lowercase())
            } else {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode as CrosstermKeyCode, KeyEvent, KeyModifiers};
    use std::sync::mpsc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use std::{env, fs};

    #[test]
    fn macos_option_symbols_map_to_alt_key_chords() {
        let chord = map_key_event_to_chord(KeyEvent::new(
            CrosstermKeyCode::Char('ƒ'),
            KeyModifiers::NONE,
        ))
        .expect("option-f should map to a chord");
        assert_eq!(chord.code, KeyCode::Char('f'));
        assert!(chord.modifiers.alt);

        let chord = map_key_event_to_chord(KeyEvent::new(
            CrosstermKeyCode::Char('†'),
            KeyModifiers::NONE,
        ))
        .expect("option-t should map to a chord");
        assert_eq!(chord.code, KeyCode::Char('t'));
        assert!(chord.modifiers.alt);

        let chord = map_key_event_to_chord(KeyEvent::new(
            CrosstermKeyCode::Char('˙'),
            KeyModifiers::NONE,
        ))
        .expect("option-h should map to a chord");
        assert_eq!(chord.code, KeyCode::Char('h'));
        assert!(chord.modifiers.alt);

        let chord = map_key_event_to_chord(KeyEvent::new(
            CrosstermKeyCode::Char('ß'),
            KeyModifiers::NONE,
        ))
        .expect("option-s should map to a chord");
        assert_eq!(chord.code, KeyCode::Char('s'));
        assert!(chord.modifiers.alt);

        let chord = map_key_event_to_chord(KeyEvent::new(
            CrosstermKeyCode::Char('ƒ'),
            KeyModifiers::ALT,
        ))
        .expect("option-f with ALT modifier should map to a chord");
        assert_eq!(chord.code, KeyCode::Char('f'));
        assert!(chord.modifiers.alt);
    }

    #[test]
    fn shifted_symbol_char_drops_shift_modifier_for_lookup() {
        let chord = map_key_event_to_chord(KeyEvent::new(
            CrosstermKeyCode::Char('!'),
            KeyModifiers::SHIFT,
        ))
        .expect("shift+1 should map to exclamation");
        assert_eq!(chord.code, KeyCode::Char('!'));
        assert!(!chord.modifiers.shift);
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
        let (worker_tx, _worker_rx) = mpsc::channel();
        let (background_tx, _background_rx) = mpsc::channel();
        let skin_runtime = SkinRuntimeConfig {
            skin_dir: None,
            mc_ini_path: None,
        };

        handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Char('x'), KeyModifiers::CONTROL),
            &worker_tx,
            &background_tx,
            &skin_runtime,
        )
        .expect("ctrl-x should enter xmap mode");
        handle_key(
            &mut state,
            &keymap,
            KeyEvent::new(CrosstermKeyCode::Char('!'), KeyModifiers::SHIFT),
            &worker_tx,
            &background_tx,
            &skin_runtime,
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
    fn read_skin_from_mc_ini_uses_midnight_commander_section() {
        let source = "\
[Midnight-Commander]
skin=darkfar
";
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let path = env::temp_dir().join(format!("rc-read-mc-ini-{stamp}.ini"));
        fs::write(&path, source).expect("test ini should be written");

        let skin = read_skin_from_mc_ini(&path).expect("skin should parse from ini");
        assert_eq!(skin, Some(String::from("darkfar")));

        fs::remove_file(&path).expect("test ini should be removed");
    }

    #[test]
    fn upsert_skin_in_mc_ini_updates_existing_skin_value() {
        let source = "\
[Midnight-Commander]
verbose=true
skin=default
";
        let updated = upsert_skin_in_mc_ini(source, "xoria256");
        assert!(updated.contains("skin=xoria256"));
        assert!(
            !updated.contains("skin=default"),
            "previous skin key should be replaced"
        );
    }

    #[test]
    fn upsert_skin_in_mc_ini_adds_section_when_missing() {
        let updated = upsert_skin_in_mc_ini("[Layout]\nmenubar_visible=true\n", "julia256");
        assert!(updated.contains("[Midnight-Commander]"));
        assert!(updated.contains("skin=julia256"));
    }
}
