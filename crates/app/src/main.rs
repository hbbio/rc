#![forbid(unsafe_code)]

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode as CrosstermKeyCode, KeyEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use rc_core::keymap::{KeyChord, KeyCode, KeyContext, KeyModifiers, Keymap};
use rc_core::{
    AppCommand, AppState, ApplyResult, BackgroundCommand, BackgroundEvent, JobEvent, WorkerCommand,
    run_background_worker, run_worker,
};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(version, about = "Roadmap bootstrap for the rc file manager")]
struct Cli {
    #[arg(long, default_value_t = 200)]
    tick_rate_ms: u64,
    #[arg(long)]
    path: Option<PathBuf>,
}

fn main() -> Result<()> {
    init_tracing();

    let cli = Cli::parse();
    let start_path = cli
        .path
        .unwrap_or(std::env::current_dir().context("failed to resolve current directory")?);
    let mut state = AppState::new(start_path).context("failed to initialize app state")?;
    let keymap =
        Keymap::bundled_mc_default().context("failed to load bundled mc.default.keymap")?;

    run_app(&mut state, &keymap, Duration::from_millis(cli.tick_rate_ms))
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

fn run_app(state: &mut AppState, keymap: &Keymap, tick_rate: Duration) -> Result<()> {
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
    execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;

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
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
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
        if event::poll(timeout).context("failed to poll input")?
            && let Event::Key(key_event) = event::read().context("failed to read input event")?
            && key_event.kind == KeyEventKind::Press
            && handle_key(
                state,
                keymap,
                key_event,
                channels.worker_tx,
                channels.background_tx,
            )?
        {
            return Ok(());
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
) -> Result<bool> {
    let context = state.key_context();

    if context == KeyContext::Input
        && let Some(command) = input_char_command(&key_event)
    {
        return Ok(
            apply_and_dispatch(state, command, worker_tx, background_tx)? == ApplyResult::Quit,
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
    let Some(command) = AppCommand::from_key_command(context, key_command) else {
        return Ok(false);
    };

    Ok(apply_and_dispatch(state, command, worker_tx, background_tx)? == ApplyResult::Quit)
}

fn apply_and_dispatch(
    state: &mut AppState,
    command: AppCommand,
    worker_tx: &Sender<WorkerCommand>,
    background_tx: &Sender<BackgroundCommand>,
) -> Result<ApplyResult> {
    let result = state.apply(command)?;
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
            CrosstermKeyCode::Char('ƒ'),
            KeyModifiers::ALT,
        ))
        .expect("option-f with ALT modifier should map to a chord");
        assert_eq!(chord.code, KeyCode::Char('f'));
        assert!(chord.modifiers.alt);
    }
}
