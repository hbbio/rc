use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::Frame;
use rc_core::{ActivePanel, AppState, ForegroundShellRequest, sanitize_display_field};
use rc_shell::subshell::{PersistentShell, ShellPrompt, SubshellEvent, SubshellOptions};
use rc_shell::{ResolvedShell, resolve_shell};

mod input;
mod terminal;

use terminal::{TerminalView, dimensions};

#[cfg(test)]
mod tests;

fn same_directory(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (std::fs::metadata(left), std::fs::metadata(right)) {
        (Ok(left), Ok(right)) => left.dev() == right.dev() && left.ino() == right.ino(),
        _ => false,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PanelContext {
    panel: ActivePanel,
    cwd: PathBuf,
    local: PathBuf,
}

impl PanelContext {
    fn capture(state: &AppState) -> Self {
        Self {
            panel: state.active_panel,
            cwd: state.active_panel().cwd.clone(),
            local: state.active_panel().local_directory().to_path_buf(),
        }
    }
}

struct DirectorySync {
    directory: PathBuf,
    context: PanelContext,
}

struct Session {
    backend: PersistentShell,
    specification: ResolvedShell,
    view: TerminalView,
    echo: Option<TerminalView>,
    size: (u16, u16),
    initialized: bool,
    startup_activity: Instant,
    context: PanelContext,
    observed: PanelContext,
    pending: Option<ForegroundShellRequest>,
    running: Option<ForegroundShellRequest>,
    synchronizing: Option<DirectorySync>,
    queued_input: Vec<u8>,
    input_blocked: bool,
    notice: Option<String>,
}

#[derive(Default)]
pub(super) struct ShellController {
    session: Option<Session>,
    visible: bool,
}

impl ShellController {
    pub fn visible(&self) -> bool {
        self.visible
    }
    pub fn active(&self) -> bool {
        self.session.is_some()
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    pub fn update(
        &mut self,
        state: &mut AppState,
        width: u16,
        height: u16,
    ) -> Vec<ForegroundShellRequest> {
        let size = dimensions(width, height);
        if state.take_panel_toggle_request() {
            if self.visible {
                self.visible = false;
            } else if self.session.is_some() {
                self.visible = true;
            } else {
                let cwd = state.active_panel().local_directory();
                match resolve_shell(&state.settings().shell, cwd)
                    .and_then(|resolution| self.start(state, resolution.shell, size, true))
                {
                    Ok(()) => self.visible = true,
                    Err(error) => state.set_status(format!(
                        "Cannot open shell: {}",
                        sanitize_display_field(&error.to_string())
                    )),
                }
            }
        }

        let mut fallback = Vec::new();
        for request in state.take_pending_foreground_shell_requests() {
            if self
                .session
                .as_ref()
                .is_some_and(|session| session.specification != request.shell)
            {
                if self.session.as_ref().is_some_and(Session::busy) {
                    state.finish_persistent_shell_request(
                        &request,
                        Err("The current shell is busy; return with Ctrl+O".into()),
                    );
                    continue;
                }
                self.session = None;
            }
            if self.session.is_none() {
                match self.start(state, request.shell.clone(), size, true) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::Unsupported => {
                        fallback.push(request);
                        continue;
                    }
                    Err(error) => {
                        state.finish_persistent_shell_request(&request, Err(error.to_string()));
                        continue;
                    }
                }
            }
            let session = self.session.as_mut().expect("started shell");
            if session.pending.is_some()
                || session.running.is_some()
                || session.synchronizing.is_some()
                || (session.initialized && !session.backend.at_prompt())
            {
                state.finish_persistent_shell_request(
                    &request,
                    Err("The shell is busy or has unfinished input; return with Ctrl+O".into()),
                );
            } else {
                session.context = PanelContext::capture(state);
                session.pending = Some(request);
                self.visible = true;
            }
        }

        let finished = self.session.as_mut().and_then(|session| {
            if session.size != size {
                session.size = size;
                session.view.resize(size.0, size.1);
                if let Some(echo) = &mut session.echo {
                    echo.resize(size.0, size.1);
                }
                if let Err(error) = session.backend.resize(size.0, size.1) {
                    return Some(Err(error.to_string()));
                }
            }
            session.tick(state)
        });
        if let Some(result) = finished {
            let mut session = self.session.take().expect("finished shell");
            session.finish_requests(state, result.clone());
            self.visible = false;
            match result {
                Ok(code) => state.set_status(format!("Shell exited ({code})")),
                Err(error) => {
                    state.set_status(format!("Shell closed: {}", sanitize_display_field(&error)))
                }
            }
        }
        state.set_shell_available(self.session.is_some());
        fallback
    }

    fn start(
        &mut self,
        state: &AppState,
        specification: ResolvedShell,
        size: (u16, u16),
        read_startup_files: bool,
    ) -> io::Result<()> {
        let context = PanelContext::capture(state);
        let backend = PersistentShell::spawn(
            &specification,
            &context.local,
            SubshellOptions {
                rows: size.0,
                columns: size.1,
                read_startup_files,
            },
        )?;
        self.session = Some(Session {
            backend,
            specification,
            view: TerminalView::new(size.0, size.1),
            echo: None,
            size,
            initialized: false,
            startup_activity: Instant::now(),
            context: context.clone(),
            observed: context,
            pending: None,
            running: None,
            synchronizing: None,
            queued_input: Vec::new(),
            input_blocked: false,
            notice: None,
        });
        Ok(())
    }

    pub fn render(&self, frame: &mut Frame) {
        let Some(session) = &self.session else {
            return;
        };
        let status = session.notice.as_deref().unwrap_or_else(|| {
            if !session.initialized {
                "Starting shell…"
            } else if session.synchronizing.is_some() {
                "Changing directory…"
            } else if session.busy() {
                "Running / editing · Ctrl+C cancels or clears input"
            } else {
                "Ready"
            }
        });
        session.view.render(
            frame,
            &format!(" Ctrl+O panels · Shift+PgUp/PgDn scroll · {status}"),
        );
    }

    pub fn input(&mut self, event: Event, state: &mut AppState) {
        let Some(session) = &mut self.session else {
            return;
        };
        let result = match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.code == KeyCode::PageUp && key.modifiers == KeyModifiers::SHIFT {
                    session
                        .view
                        .scroll(isize::try_from(session.size.0).unwrap_or(1));
                    return;
                }
                if key.code == KeyCode::PageDown && key.modifiers == KeyModifiers::SHIFT {
                    session
                        .view
                        .scroll(-isize::try_from(session.size.0).unwrap_or(1));
                    return;
                }
                input::key_bytes(key, session.view.screen())
                    .map(|bytes| session.send_input(&bytes, state))
            }
            Event::Paste(text) => Some(
                input::paste_bytes(&text, session.view.screen().bracketed_paste())
                    .map_err(io::Error::other)
                    .and_then(|bytes| session.send_input(&bytes, state)),
            ),
            Event::Mouse(mouse) => {
                if session.view.screen().mouse_protocol_mode() == vt100::MouseProtocolMode::None {
                    match mouse.kind {
                        MouseEventKind::ScrollUp => session.view.scroll(3),
                        MouseEventKind::ScrollDown => session.view.scroll(-3),
                        _ => {}
                    }
                    return;
                }
                input::mouse_bytes(mouse, session.view.screen())
                    .map(|bytes| session.send_input(&bytes, state))
            }
            _ => None,
        };
        if let Some(Err(error)) = result {
            session.notice = Some(sanitize_display_field(&error.to_string()));
        }
    }
}

impl Session {
    fn busy(&self) -> bool {
        self.pending.is_some()
            || self.running.is_some()
            || self.synchronizing.is_some()
            || !self.backend.at_prompt()
    }

    fn tick(&mut self, state: &mut AppState) -> Option<Result<i32, String>> {
        for event in self.backend.poll() {
            match event {
                SubshellEvent::Output(bytes) => {
                    let replies = self.echo.as_mut().unwrap_or(&mut self.view).process(&bytes);
                    if !replies.is_empty()
                        && let Err(error) = self.backend.write_response(&replies)
                        && error.kind() != io::ErrorKind::WouldBlock
                    {
                        return Some(Err(error.to_string()));
                    }
                }
                SubshellEvent::CommandStarted(_) => {
                    if let Some(echo) = self.echo.take() {
                        self.view.align_cursor(echo.screen());
                    }
                }
                SubshellEvent::Prompt(prompt) => self.prompt(state, prompt),
                SubshellEvent::Exited(status) => return Some(Ok(status)),
                SubshellEvent::Error(error) => return Some(Err(error)),
            }
        }
        if !self.initialized && self.startup_activity.elapsed() > Duration::from_secs(15) {
            return Some(Err(
                "Shell prompt did not initialize; check the shell startup files".into(),
            ));
        }
        if !self.backend.at_prompt() || self.synchronizing.is_some() {
            self.flush_input(state);
            return None;
        }
        if let Some(request) = &self.pending {
            if self
                .backend
                .last_prompt()
                .is_none_or(|prompt| !same_directory(&prompt.directory, &request.cwd))
            {
                let directory = request.cwd.clone();
                let context = self.context.clone();
                if let Err(error) = self.sync(directory, context) {
                    self.discard_input(&error.to_string());
                    let request = self.pending.take().expect("pending command");
                    state.finish_persistent_shell_request(&request, Err(error.to_string()));
                }
            } else {
                let request = self.pending.take().expect("pending command");
                let echo = self.view.fork();
                match self.backend.run_command(&request.command) {
                    Ok(()) => {
                        self.echo = Some(echo);
                        self.view.show_command(&request.command);
                        self.observed = self.context.clone();
                        self.running = Some(request);
                        self.notice = None;
                    }
                    Err(error) => {
                        self.discard_input(&error.to_string());
                        state.finish_persistent_shell_request(&request, Err(error.to_string()))
                    }
                }
            }
        } else {
            let current = PanelContext::capture(state);
            if current != self.observed
                && let Err(error) = self.sync(current.local.clone(), current.clone())
            {
                self.notice = Some(sanitize_display_field(&error.to_string()));
                self.discard_input(&error.to_string());
                self.observed = current;
            }
        }
        self.flush_input(state);
        None
    }

    fn sync(&mut self, directory: PathBuf, context: PanelContext) -> io::Result<()> {
        let echo = self.view.fork();
        if self.backend.synchronize_directory(&directory)? {
            self.echo = Some(echo);
            self.synchronizing = Some(DirectorySync { directory, context });
        } else {
            self.context = context.clone();
            self.observed = context;
        }
        Ok(())
    }

    fn prompt(&mut self, state: &mut AppState, prompt: ShellPrompt) {
        if !self.initialized {
            self.initialized = true;
            // Startup files may cd; the first command still belongs to the
            // panel directory from which this shell was opened.
            if !same_directory(&prompt.directory, &self.observed.local)
                && let Err(error) = self.sync(self.observed.local.clone(), self.observed.clone())
            {
                self.notice = Some(error.to_string());
            }
            return;
        }
        if let Some(sync) = self.synchronizing.take() {
            self.observed = sync.context.clone();
            self.context = sync.context;
            if prompt.exit_status != 0 || !same_directory(&prompt.directory, &sync.directory) {
                let error = "Shell could not enter the panel directory".to_string();
                self.notice = Some(error.clone());
                self.discard_input(&error);
                if let Some(request) = self.pending.take() {
                    state.finish_persistent_shell_request(&request, Err(error));
                }
            }
            return;
        }
        if let Some(request) = self.running.take() {
            state.finish_persistent_shell_request(&request, Ok(prompt.exit_status));
            state.refresh_panels_in_directory(&request.cwd);
        }
        if !same_directory(&prompt.directory, &self.context.local)
            && let Err(error) = state.follow_shell_directory(
                self.context.panel,
                &self.context.cwd,
                &prompt.directory,
            )
        {
            self.notice = Some(error.to_string());
        }
        state.refresh_panels_in_directory(&prompt.directory);
        let current = PanelContext::capture(state);
        if same_directory(&current.local, &prompt.directory) {
            self.observed = current.clone();
            self.context = current;
        }
    }

    fn send_input(&mut self, bytes: &[u8], state: &mut AppState) -> io::Result<()> {
        self.startup_activity = Instant::now();
        if bytes == [3] {
            self.queued_input.clear();
            self.input_blocked = false;
        }
        if self.input_blocked {
            return Err(io::Error::other(
                "Queued input discarded; press Ctrl+C to continue",
            ));
        }
        self.notice = None;
        if bytes == [3]
            && let Some(request) = self.pending.take()
        {
            state.finish_persistent_shell_request(&request, Err("Command canceled".into()));
            return if self.initialized {
                Ok(())
            } else {
                self.backend.write(bytes)
            };
        }
        if (self.echo.is_some()
            || self.synchronizing.is_some()
            || (self.initialized && self.pending.is_some())
            || !self.queued_input.is_empty())
            && bytes != [3]
        {
            return self.queue_input(bytes);
        }
        if self.backend.at_prompt() {
            self.context = PanelContext::capture(state);
        }
        self.view.live();
        match self.backend.write(bytes) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => self.queue_input(bytes),
            result => result,
        }
    }

    fn queue_input(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.queued_input.len().saturating_add(bytes.len()) > 64 * 1024 {
            self.discard_input("Queued input exceeded 64 KiB");
            self.input_blocked = true;
            return Err(io::Error::other(
                "Queued input exceeded 64 KiB; press Ctrl+C to continue",
            ));
        }
        self.queued_input.extend_from_slice(bytes);
        Ok(())
    }

    fn discard_input(&mut self, reason: &str) {
        if !self.queued_input.is_empty() {
            self.queued_input.clear();
            self.input_blocked = true;
            self.notice = Some(format!(
                "{reason}; queued input discarded; Ctrl+C to continue"
            ));
        }
    }

    fn flush_input(&mut self, state: &AppState) {
        if self.queued_input.is_empty()
            || self.input_blocked
            || self.echo.is_some()
            || self.synchronizing.is_some()
            || (self.initialized && self.pending.is_some())
        {
            return;
        }
        if self.backend.at_prompt() {
            self.context = PanelContext::capture(state);
        }
        match self.backend.write(&self.queued_input) {
            Ok(()) => {
                self.queued_input.clear();
                self.view.live();
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => self.discard_input(&error.to_string()),
        }
    }

    fn finish_requests(&mut self, state: &mut AppState, result: Result<i32, String>) {
        if let Some(request) = self.pending.take() {
            state.finish_persistent_shell_request(
                &request,
                Err("Shell closed before the command started".into()),
            );
        }
        if let Some(request) = self.running.take() {
            state.finish_persistent_shell_request(&request, result);
            state.refresh_panels_in_directory(&request.cwd);
        }
    }
}
