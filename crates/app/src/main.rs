#![forbid(unsafe_code)]

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::execute;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use rc_core::{ActivePanel, AppState};
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

    run_app(&mut state, Duration::from_millis(cli.tick_rate_ms))
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

fn run_app(state: &mut AppState, tick_rate: Duration) -> Result<()> {
    enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("failed to create terminal backend")?;
    terminal.clear().context("failed to clear terminal")?;

    let loop_result = run_event_loop(&mut terminal, state, tick_rate);
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
                if key_event.kind == KeyEventKind::Press && handle_key(state, key_event.code)? {
                    return Ok(());
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }
}

fn handle_key(state: &mut AppState, code: KeyCode) -> Result<bool> {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
        KeyCode::Tab => {
            state.toggle_active_panel();
            state.set_status(format!(
                "Active panel: {}",
                active_panel_label(state.active_panel)
            ));
        }
        KeyCode::Up => state.move_cursor(-1),
        KeyCode::Down => state.move_cursor(1),
        KeyCode::Enter => {
            if state.open_selected_directory()? {
                state.set_status("Opened selected directory");
            } else {
                state.set_status("Selected entry is not a directory");
            }
        }
        KeyCode::Backspace => {
            if state.go_parent_directory()? {
                state.set_status("Moved to parent directory");
            } else {
                state.set_status("Already at filesystem root");
            }
        }
        KeyCode::Char('r') => {
            state.refresh_active_panel()?;
            state.set_status("Refreshed active panel");
        }
        _ => {}
    }

    Ok(false)
}

fn active_panel_label(panel: ActivePanel) -> &'static str {
    match panel {
        ActivePanel::Left => "left",
        ActivePanel::Right => "right",
    }
}
