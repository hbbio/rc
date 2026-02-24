#![forbid(unsafe_code)]

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode as CrosstermKeyCode, KeyEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use rc_core::keymap::{KeyChord, KeyCode, KeyContext, KeyModifiers, Keymap};
use rc_core::{AppCommand, AppState, ApplyResult};
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
    enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("failed to create terminal backend")?;
    terminal.clear().context("failed to clear terminal")?;

    let loop_result = run_event_loop(&mut terminal, state, keymap, tick_rate);
    let restore_result = restore_terminal(&mut terminal);

    loop_result?;
    restore_result?;
    Ok(())
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode().context("failed to disable raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .context("failed to leave alternate screen")?;
    terminal.show_cursor().context("failed to restore cursor")?;
    Ok(())
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut AppState,
    keymap: &Keymap,
    tick_rate: Duration,
) -> Result<()> {
    let mut last_tick = Instant::now();

    loop {
        terminal
            .draw(|frame| rc_ui::render(frame, state))
            .context("failed to draw frame")?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout).context("failed to poll input")? {
            if let Event::Key(key_event) = event::read().context("failed to read input event")? {
                if key_event.kind == KeyEventKind::Press && handle_key(state, keymap, key_event)? {
                    return Ok(());
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }
}

fn handle_key(state: &mut AppState, keymap: &Keymap, key_event: KeyEvent) -> Result<bool> {
    let context = state.key_context();

    if context == KeyContext::Input {
        if let Some(command) = input_char_command(&key_event) {
            return Ok(state.apply(command)? == ApplyResult::Quit);
        }
    }

    let Some(chord) = map_key_event_to_chord(key_event) else {
        return Ok(false);
    };
    let Some(key_command) = keymap.resolve(context, chord) else {
        return Ok(false);
    };
    let Some(command) = AppCommand::from_key_command(context, key_command) else {
        return Ok(false);
    };

    Ok(state.apply(command)? == ApplyResult::Quit)
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

    if no_shortcut_modifiers {
        if let CrosstermKeyCode::Char(ch) = key_event.code {
            return Some(AppCommand::DialogInputChar(ch));
        }
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
