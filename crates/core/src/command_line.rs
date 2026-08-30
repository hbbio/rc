use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rc_shell::{
    CompletionCandidate, CompletionEdit, CompletionOutcome, CompletionRequest, CompletionResponse,
    LiteralCd, ResolvedShell, ShellHistoryMode, ShellResolution, ShellSettings, parse_literal_cd,
    resolve_shell,
};
use tui_input::{Input, InputRequest};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{AppState, Route, current_user_home_directory};

pub const COMMAND_BUFFER_LIMIT_BYTES: usize = 64 * 1024;
pub const PASTE_PAYLOAD_LIMIT_BYTES: usize = 64 * 1024;
pub const COMMAND_HISTORY_LIMIT_ENTRIES: usize = 2_000;
pub const COMMAND_HISTORY_LIMIT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionIntent {
    Forward,
    Reverse,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandLineInput {
    Character(char),
    Left,
    Right,
    Home,
    End,
    Backspace,
    Delete,
    HistoryPrevious,
    HistoryNext,
    DeletePreviousWord,
    DeleteFromStart,
    DeleteToEnd,
    Clear,
    Complete(CompletionIntent),
    Enter,
    Escape,
    Paste(String),
    SetCursor(usize),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForegroundShellRequest {
    pub activation_id: u64,
    pub command: String,
    pub cwd: PathBuf,
    pub shell: ResolvedShell,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellResolutionRequest {
    pub request_id: u64,
    pub cwd: PathBuf,
    pub settings: ShellSettings,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellResolutionResponse {
    pub request_id: u64,
    pub cwd: PathBuf,
    pub result: Result<ShellResolution, String>,
}

pub fn resolve_shell_request_blocking(request: ShellResolutionRequest) -> ShellResolutionResponse {
    let result = resolve_shell(&request.settings, &request.cwd).map_err(|error| error.to_string());
    ShellResolutionResponse {
        request_id: request.request_id,
        cwd: request.cwd,
        result,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionCancellation {
    pub activation_id: u64,
    pub request_id: u64,
}

#[derive(Clone, Debug)]
struct HistoryDraft {
    value: String,
    cursor: usize,
}

#[derive(Debug)]
pub struct CommandLineModel {
    input: Input,
    revision: u64,
    history: VecDeque<String>,
    history_bytes: usize,
    history_index: Option<usize>,
    history_draft: Option<HistoryDraft>,
    history_mode: ShellHistoryMode,
    last_exit_status: Option<i32>,
}

impl Default for CommandLineModel {
    fn default() -> Self {
        Self::new(ShellHistoryMode::Session)
    }
}

impl CommandLineModel {
    pub fn new(history_mode: ShellHistoryMode) -> Self {
        Self {
            input: Input::default(),
            revision: 0,
            history: VecDeque::new(),
            history_bytes: 0,
            history_index: None,
            history_draft: None,
            history_mode,
            last_exit_status: None,
        }
    }

    pub fn value(&self) -> &str {
        self.input.value()
    }

    pub fn cursor_codepoint(&self) -> usize {
        self.input.cursor()
    }

    pub fn cursor_byte(&self) -> usize {
        codepoint_to_byte(self.input.value(), self.input.cursor())
    }

    pub fn visual_cursor(&self) -> usize {
        // tui-input 0.15 uses a newer unicode-width table than Ratatui 0.28. Calculate
        // presentation metrics with the workspace table so editing, rendering, and mouse hit
        // testing all agree on the same terminal columns.
        UnicodeWidthStr::width(&self.value()[..self.cursor_byte()])
    }

    pub fn visual_scroll(&self, width: usize) -> usize {
        let target = self.visual_cursor().saturating_sub(width);
        let mut scroll = 0_usize;
        for grapheme in self.value().graphemes(true) {
            if scroll >= target {
                break;
            }
            scroll = scroll.saturating_add(UnicodeWidthStr::width(grapheme));
        }
        scroll
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn history(&self) -> &VecDeque<String> {
        &self.history
    }

    pub fn history_bytes(&self) -> usize {
        self.history_bytes
    }

    pub fn history_mode(&self) -> ShellHistoryMode {
        self.history_mode
    }

    pub fn last_exit_status(&self) -> Option<i32> {
        self.last_exit_status
    }

    pub fn set_history_mode(&mut self, mode: ShellHistoryMode) {
        self.history_mode = mode;
        self.leave_history_navigation();
    }

    pub fn autosuggestion(&self) -> Option<&str> {
        if self.history_mode == ShellHistoryMode::Off
            || self.history_index.is_some()
            || self.cursor_byte() != self.value().len()
        {
            return None;
        }
        self.history
            .iter()
            .rev()
            .find(|entry| entry.len() > self.value().len() && entry.starts_with(self.value()))
            .map(String::as_str)
    }

    fn set_last_exit_status(&mut self, status: i32) {
        self.last_exit_status = Some(status);
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }

    fn replace(&mut self, value: String, cursor: usize) {
        self.input = Input::new(value).with_cursor(cursor);
        self.bump_revision();
    }

    fn clear_after_success(&mut self) {
        self.leave_history_navigation();
        if !self.value().is_empty() {
            self.input.reset();
            self.bump_revision();
        }
    }

    fn record_history(&mut self, command: &str) {
        if self.history_mode == ShellHistoryMode::Off || command.is_empty() {
            return;
        }
        if self.history.back().is_some_and(|entry| entry == command) {
            return;
        }
        self.history.push_back(command.to_string());
        self.history_bytes = self.history_bytes.saturating_add(command.len());
        while self.history.len() > COMMAND_HISTORY_LIMIT_ENTRIES
            || self.history_bytes > COMMAND_HISTORY_LIMIT_BYTES
        {
            if let Some(evicted) = self.history.pop_front() {
                self.history_bytes = self.history_bytes.saturating_sub(evicted.len());
            } else {
                break;
            }
        }
    }

    fn leave_history_navigation(&mut self) {
        self.history_index = None;
        self.history_draft = None;
    }

    fn history_previous(&mut self) -> bool {
        if self.history_mode == ShellHistoryMode::Off || self.history.is_empty() {
            return false;
        }
        let index = match self.history_index {
            None => {
                self.history_draft = Some(HistoryDraft {
                    value: self.value().to_string(),
                    cursor: self.cursor_codepoint(),
                });
                self.history.len().saturating_sub(1)
            }
            Some(index) => index.saturating_sub(1),
        };
        if self.history_index == Some(index) {
            return false;
        }
        self.history_index = Some(index);
        let value = self.history[index].clone();
        let cursor = value.chars().count();
        self.replace(value, cursor);
        true
    }

    fn history_next(&mut self) -> bool {
        if self.history_mode == ShellHistoryMode::Off {
            return false;
        }
        let Some(index) = self.history_index else {
            return false;
        };
        if index + 1 < self.history.len() {
            let next = index + 1;
            self.history_index = Some(next);
            let value = self.history[next].clone();
            let cursor = value.chars().count();
            self.replace(value, cursor);
        } else {
            let draft = self.history_draft.take().unwrap_or(HistoryDraft {
                value: String::new(),
                cursor: 0,
            });
            self.history_index = None;
            self.replace(draft.value, draft.cursor);
        }
        true
    }

    fn accept_autosuggestion(&mut self) -> bool {
        let Some(suggestion) = self.autosuggestion().map(ToString::to_string) else {
            return false;
        };
        let cursor = suggestion.chars().count();
        self.replace(suggestion, cursor);
        true
    }

    fn edit(&mut self, request: InputRequest) -> bool {
        self.leave_history_navigation();
        let changed = self.input.handle(request);
        if changed.is_some_and(|changed| changed.value) {
            self.bump_revision();
            true
        } else {
            false
        }
    }

    fn move_cursor(&mut self, request: InputRequest) {
        let _ = self.input.handle(request);
    }

    fn insert_character(&mut self, character: char) -> Result<bool, &'static str> {
        if character.is_control() {
            return Ok(false);
        }
        if self.value().len().saturating_add(character.len_utf8()) > COMMAND_BUFFER_LIMIT_BYTES {
            return Err("Command is limited to 64 KiB");
        }
        Ok(self.edit(InputRequest::InsertChar(character)))
    }

    fn insert_paste(&mut self, raw: &str) -> Result<bool, &'static str> {
        let sanitized = sanitize_paste(raw)?;
        if self.value().len().saturating_add(sanitized.len()) > COMMAND_BUFFER_LIMIT_BYTES {
            return Err("Paste would exceed the 64 KiB command limit");
        }
        if sanitized.is_empty() {
            return Ok(false);
        }
        self.leave_history_navigation();
        let cursor_byte = self.cursor_byte();
        let mut value = String::with_capacity(self.value().len().saturating_add(sanitized.len()));
        value.push_str(&self.value()[..cursor_byte]);
        value.push_str(&sanitized);
        value.push_str(&self.value()[cursor_byte..]);
        let cursor = self
            .cursor_codepoint()
            .saturating_add(sanitized.chars().count());
        self.replace(value, cursor);
        Ok(true)
    }

    fn apply_completion(&mut self, edit: &CompletionEdit) -> Result<(), &'static str> {
        validate_completion_edit(self.value(), edit)?;
        let new_len = self
            .value()
            .len()
            .saturating_sub(edit.end_byte.saturating_sub(edit.start_byte))
            .saturating_add(edit.replacement.len());
        if new_len > COMMAND_BUFFER_LIMIT_BYTES {
            return Err("Completion would exceed the 64 KiB command limit");
        }
        let mut value = String::with_capacity(new_len);
        value.push_str(&self.value()[..edit.start_byte]);
        value.push_str(&edit.replacement);
        value.push_str(&self.value()[edit.end_byte..]);
        let cursor_byte = edit.start_byte.saturating_add(edit.replacement.len());
        let cursor = value[..cursor_byte].chars().count();
        self.leave_history_navigation();
        self.replace(value, cursor);
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub enum CommandLineCompletionState {
    #[default]
    Closed,
    Pending(PendingCompletion),
    Open(OpenCompletion),
}

#[derive(Clone, Debug)]
pub struct PendingCompletion {
    pub request_id: u64,
    pub buffer_revision: u64,
    pub original_buffer: String,
    pub intent: CompletionIntent,
}

#[derive(Clone, Debug)]
pub struct OpenCompletion {
    pub request_id: u64,
    pub buffer_revision: u64,
    pub original_buffer: String,
    pub candidates: Vec<CompletionCandidate>,
    pub selected: usize,
}

#[derive(Clone, Debug)]
pub struct CommandLineSession {
    pub activation_id: u64,
    pub shell: ResolvedShell,
    pub completion: CommandLineCompletionState,
    pub notice: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingShellResolution {
    request_id: u64,
    cwd: PathBuf,
    settings: ShellSettings,
    previous_status_line: String,
    previous_status_expires_at: Option<Instant>,
    transient_status_line: String,
    transient_status_expires_at: Option<Instant>,
    transient_status_generation: u64,
}

impl CommandLineSession {
    pub fn selected_completion(&self) -> Option<&CompletionCandidate> {
        let CommandLineCompletionState::Open(completion) = &self.completion else {
            return None;
        };
        completion.candidates.get(completion.selected)
    }

    pub fn completion_candidates(&self) -> Option<(&[CompletionCandidate], usize)> {
        let CommandLineCompletionState::Open(completion) = &self.completion else {
            return None;
        };
        Some((&completion.candidates, completion.selected))
    }
}

impl AppState {
    pub fn command_line_model(&self) -> &CommandLineModel {
        &self.command_line
    }

    pub fn command_line_session(&self) -> Option<&CommandLineSession> {
        let Route::CommandLine(session) = self.top_route() else {
            return None;
        };
        Some(session)
    }

    pub fn open_command_line(&mut self) {
        #[cfg(windows)]
        {
            self.set_status("Command line is not yet supported on Windows");
            return;
        }
        #[cfg(not(any(unix, windows)))]
        {
            self.set_status("Command line is not yet supported on this platform");
            return;
        }
        #[cfg(unix)]
        {
            if matches!(self.top_route(), Route::CommandLine(_)) {
                return;
            }
            if self.pending_shell_resolution.is_some() {
                self.set_status("Shell selection is still pending");
                let transient_status_line = self.status_line.clone();
                let transient_status_expires_at = self.status_expires_at;
                if let Some(pending) = self.pending_shell_resolution.as_mut() {
                    pending.transient_status_line = transient_status_line;
                    pending.transient_status_expires_at = transient_status_expires_at;
                    pending.transient_status_generation = self.status_message_generation;
                }
                return;
            }
            if !matches!(self.top_route(), Route::FileManager) {
                return;
            }
            let cwd = self.active_panel().cwd.clone();
            let request_id = self.next_shell_resolution_request_id;
            self.next_shell_resolution_request_id =
                self.next_shell_resolution_request_id.saturating_add(1);
            let settings = self.settings.shell.clone();
            let previous_status_line = self.status_line.clone();
            let previous_status_expires_at = self.status_expires_at;
            self.set_status("Resolving shell...");
            self.pending_shell_resolution = Some(PendingShellResolution {
                request_id,
                cwd: cwd.clone(),
                settings: settings.clone(),
                previous_status_line,
                previous_status_expires_at,
                transient_status_line: self.status_line.clone(),
                transient_status_expires_at: self.status_expires_at,
                transient_status_generation: self.status_message_generation,
            });
            self.pending_shell_resolution_request = Some(ShellResolutionRequest {
                request_id,
                cwd,
                settings,
            });
        }
    }

    pub fn take_pending_shell_resolution_request(&mut self) -> Option<ShellResolutionRequest> {
        self.pending_shell_resolution_request.take()
    }

    pub fn restore_pending_shell_resolution_request(&mut self, request: ShellResolutionRequest) {
        let is_current = self
            .pending_shell_resolution
            .as_ref()
            .is_some_and(|pending| pending.request_id == request.request_id);
        if is_current && self.pending_shell_resolution_request.is_none() {
            self.pending_shell_resolution_request = Some(request);
        }
    }

    pub fn handle_shell_resolution_response(&mut self, response: ShellResolutionResponse) {
        let is_current = self
            .pending_shell_resolution
            .as_ref()
            .is_some_and(|pending| {
                pending.request_id == response.request_id && pending.cwd == response.cwd
            });
        if !is_current {
            return;
        }
        let pending = self
            .pending_shell_resolution
            .take()
            .expect("pending shell resolution checked above");
        self.pending_shell_resolution_request = None;

        if !matches!(self.top_route(), Route::FileManager)
            || self.active_panel().cwd != pending.cwd
            || self.settings.shell != pending.settings
        {
            self.set_status("Shell selection became stale; open the command line again");
            return;
        }

        match response.result {
            Ok(resolution) => {
                let now = Instant::now();
                let transient_status_expired = self.status_line.is_empty()
                    && self.status_expires_at.is_none()
                    && pending.transient_status_expires_at.is_some();
                let status_is_unchanged = self.status_message_generation
                    == pending.transient_status_generation
                    && (self.status_line == pending.transient_status_line
                        || transient_status_expired);
                if status_is_unchanged {
                    if pending
                        .previous_status_expires_at
                        .is_some_and(|expires_at| now >= expires_at)
                    {
                        self.status_line.clear();
                        self.status_expires_at = None;
                    } else {
                        self.status_line = pending.previous_status_line;
                        self.status_expires_at = pending.previous_status_expires_at;
                    }
                    self.status_message_generation = self.status_message_generation.wrapping_add(1);
                }
                let activation_id = self.next_command_line_activation_id;
                self.next_command_line_activation_id =
                    self.next_command_line_activation_id.saturating_add(1);
                self.routes.push(Route::CommandLine(CommandLineSession {
                    activation_id,
                    shell: resolution.shell,
                    completion: CommandLineCompletionState::Closed,
                    notice: resolution.diagnostic,
                }));
            }
            Err(error) => self.set_status(format!("Command line unavailable: {error}")),
        }
    }

    pub fn handle_shell_resolution_runtime_unavailable(&mut self, message: &str) {
        if self.pending_shell_resolution.take().is_some() {
            self.pending_shell_resolution_request = None;
            self.set_status(format!("Command line unavailable: {message}"));
        }
    }

    pub fn handle_command_line_input(&mut self, input: CommandLineInput) -> io::Result<()> {
        if !matches!(self.top_route(), Route::CommandLine(_)) {
            return Ok(());
        }
        if matches!(
            &input,
            CommandLineInput::Complete(_) | CommandLineInput::Enter
        ) && self.literal_cd_transition_pending()
        {
            self.set_command_line_notice("cd: waiting for directory refresh");
            return Ok(());
        }
        match input {
            CommandLineInput::Escape => {
                self.cancel_and_close_completion();
                self.routes.pop();
            }
            CommandLineInput::Complete(intent) => self.handle_completion_key(intent),
            CommandLineInput::Enter => self.handle_command_line_enter()?,
            CommandLineInput::Paste(raw) => {
                // Validate and sanitize before changing *any* editor/session state.
                match sanitize_paste(&raw) {
                    Ok(sanitized)
                        if self
                            .command_line
                            .value()
                            .len()
                            .saturating_add(sanitized.len())
                            <= COMMAND_BUFFER_LIMIT_BYTES =>
                    {
                        self.cancel_and_close_completion();
                        if let Err(notice) = self.command_line.insert_paste(&raw) {
                            self.set_command_line_notice(notice);
                        } else {
                            self.clear_command_line_notice();
                        }
                    }
                    Ok(_) => {
                        self.set_command_line_notice("Paste would exceed the 64 KiB command limit")
                    }
                    Err(notice) => self.set_command_line_notice(notice),
                }
            }
            input => {
                self.cancel_and_close_completion();
                self.apply_editor_input(input);
            }
        }
        Ok(())
    }

    fn apply_editor_input(&mut self, input: CommandLineInput) {
        let result = match input {
            CommandLineInput::Character(character) => self.command_line.insert_character(character),
            CommandLineInput::Left => {
                self.command_line.move_cursor(InputRequest::GoToPrevChar);
                Ok(false)
            }
            CommandLineInput::Right => {
                if !self.command_line.accept_autosuggestion() {
                    self.command_line.move_cursor(InputRequest::GoToNextChar);
                }
                Ok(false)
            }
            CommandLineInput::Home => {
                self.command_line.move_cursor(InputRequest::GoToStart);
                Ok(false)
            }
            CommandLineInput::End => {
                if !self.command_line.accept_autosuggestion() {
                    self.command_line.move_cursor(InputRequest::GoToEnd);
                }
                Ok(false)
            }
            CommandLineInput::Backspace => Ok(self.command_line.edit(InputRequest::DeletePrevChar)),
            CommandLineInput::Delete => Ok(self.command_line.edit(InputRequest::DeleteNextChar)),
            CommandLineInput::DeletePreviousWord => {
                Ok(self.command_line.edit(InputRequest::DeletePrevWord))
            }
            CommandLineInput::DeleteFromStart => {
                Ok(self.command_line.edit(InputRequest::DeleteFromStart))
            }
            CommandLineInput::DeleteToEnd => {
                Ok(self.command_line.edit(InputRequest::DeleteTillEnd))
            }
            CommandLineInput::Clear => Ok(self.command_line.edit(InputRequest::DeleteLine)),
            CommandLineInput::SetCursor(cursor) => {
                self.command_line
                    .move_cursor(InputRequest::SetCursor(cursor));
                Ok(false)
            }
            CommandLineInput::HistoryPrevious => Ok(self.command_line.history_previous()),
            CommandLineInput::HistoryNext => Ok(self.command_line.history_next()),
            _ => Ok(false),
        };
        match result {
            Ok(_) => self.clear_command_line_notice(),
            Err(notice) => self.set_command_line_notice(notice),
        }
    }

    fn handle_completion_key(&mut self, intent: CompletionIntent) {
        let state = self
            .command_line_session()
            .map(|session| session.completion.clone())
            .unwrap_or_default();
        match state {
            CommandLineCompletionState::Pending(_) => {}
            CommandLineCompletionState::Open(mut completion) => {
                let len = completion.candidates.len();
                if len == 0 {
                    self.close_completion_without_cancel();
                    return;
                }
                completion.selected = match intent {
                    CompletionIntent::Forward => (completion.selected + 1) % len,
                    CompletionIntent::Reverse => (completion.selected + len - 1) % len,
                };
                if let Some(Route::CommandLine(session)) = self.routes.last_mut() {
                    session.completion = CommandLineCompletionState::Open(completion);
                }
            }
            CommandLineCompletionState::Closed => {
                let request_id = self.next_completion_request_id;
                self.next_completion_request_id = self.next_completion_request_id.saturating_add(1);
                let (activation_id, shell) = match self.command_line_session() {
                    Some(session) => (session.activation_id, session.shell.clone()),
                    None => return,
                };
                let request = CompletionRequest {
                    activation_id,
                    request_id,
                    buffer_revision: self.command_line.revision(),
                    buffer: self.command_line.value().to_string(),
                    cursor_byte: self.command_line.cursor_byte(),
                    cwd: self.active_panel().cwd.clone(),
                    shell,
                };
                if let Some(Route::CommandLine(session)) = self.routes.last_mut() {
                    session.notice = None;
                    session.completion = CommandLineCompletionState::Pending(PendingCompletion {
                        request_id,
                        buffer_revision: request.buffer_revision,
                        original_buffer: request.buffer.clone(),
                        intent,
                    });
                }
                self.pending_completion_requests.push(request);
            }
        }
    }

    fn handle_command_line_enter(&mut self) -> io::Result<()> {
        let completion = self
            .command_line_session()
            .map(|session| session.completion.clone())
            .unwrap_or_default();
        match completion {
            CommandLineCompletionState::Open(completion) => {
                let Some(candidate) = completion.candidates.get(completion.selected) else {
                    self.close_completion_without_cancel();
                    return Ok(());
                };
                let edit = candidate.edit.clone();
                self.close_completion_without_cancel();
                match self.command_line.apply_completion(&edit) {
                    Ok(()) => self.clear_command_line_notice(),
                    Err(notice) => self.set_command_line_notice(notice),
                }
                return Ok(());
            }
            CommandLineCompletionState::Pending(_) => self.cancel_and_close_completion(),
            CommandLineCompletionState::Closed => {}
        }

        let command = self.command_line.value().to_string();
        if command.is_empty() {
            return Ok(());
        }
        let (activation_id, shell) = match self.command_line_session() {
            Some(session) => (session.activation_id, session.shell.clone()),
            None => return Ok(()),
        };
        let cwd = self.active_panel().cwd.clone();
        let buffer_revision = self.command_line.revision();
        self.command_line.record_history(&command);
        if let Some(operation) = parse_literal_cd(&command, shell.dialect) {
            self.apply_literal_cd(operation, &cwd, activation_id, buffer_revision)?;
            return Ok(());
        }
        self.pending_foreground_shell_requests
            .push(ForegroundShellRequest {
                activation_id,
                command,
                cwd,
                shell,
            });
        Ok(())
    }

    fn apply_literal_cd(
        &mut self,
        operation: LiteralCd,
        cwd: &Path,
        activation_id: u64,
        buffer_revision: u64,
    ) -> io::Result<()> {
        let destination = match operation {
            LiteralCd::Home => current_user_home_directory(),
            LiteralCd::Previous => {
                self.previous_panel_directories[self.active_panel.index()].clone()
            }
            LiteralCd::Path { path, expand_home } => Some(expand_cd_path(path, cwd, expand_home)),
        };
        let Some(destination) = destination else {
            self.set_command_line_notice("Directory is unavailable");
            self.command_line.set_last_exit_status(1);
            return Ok(());
        };
        if self.set_active_panel_directory(destination)? {
            let attached = self.schedule_literal_cd_transition(
                self.active_panel,
                activation_id,
                buffer_revision,
            );
            assert!(
                attached,
                "accepted literal cd must have an unclaimed panel refresh revert"
            );
            if let Some(Route::CommandLine(session)) = self.routes.last_mut() {
                session.notice = Some(String::from("Changing directory…"));
                session.completion = CommandLineCompletionState::Closed;
            }
        } else {
            self.command_line.set_last_exit_status(1);
            self.set_command_line_notice("cd: directory is not accessible");
        }
        Ok(())
    }

    pub(crate) fn finish_literal_cd_transition(
        &mut self,
        activation_id: u64,
        buffer_revision: u64,
        succeeded: bool,
    ) {
        self.command_line
            .set_last_exit_status(if succeeded { 0 } else { 1 });
        let active = self
            .command_line_session()
            .is_some_and(|session| session.activation_id == activation_id);
        if !active {
            return;
        }
        if succeeded && self.command_line.revision() == buffer_revision {
            self.command_line.clear_after_success();
        }
        if let Some(Route::CommandLine(session)) = self.routes.last_mut() {
            session.completion = CommandLineCompletionState::Closed;
            session.notice = (!succeeded)
                .then(|| String::from("cd: directory refresh failed; previous directory restored"));
        }
    }

    pub fn take_pending_completion_requests(&mut self) -> Vec<CompletionRequest> {
        std::mem::take(&mut self.pending_completion_requests)
    }

    pub fn take_pending_completion_cancellations(&mut self) -> Vec<CompletionCancellation> {
        std::mem::take(&mut self.pending_completion_cancellations)
    }

    pub fn take_pending_foreground_shell_requests(&mut self) -> Vec<ForegroundShellRequest> {
        std::mem::take(&mut self.pending_foreground_shell_requests)
    }

    pub fn handle_completion_response(&mut self, response: CompletionResponse) {
        let Some(Route::CommandLine(session)) = self.routes.last_mut() else {
            return;
        };
        if session.activation_id != response.activation_id {
            return;
        }
        let CommandLineCompletionState::Pending(pending) = &session.completion else {
            return;
        };
        if pending.request_id != response.request_id
            || pending.buffer_revision != response.buffer_revision
            || pending.original_buffer != response.original_buffer
            || self.command_line.revision() != response.buffer_revision
            || self.command_line.value() != response.original_buffer
        {
            return;
        }
        let pending = pending.clone();
        match response.outcome {
            CompletionOutcome::Candidates {
                candidates, notice, ..
            } => {
                let candidates: Vec<_> = candidates
                    .into_iter()
                    .filter(|candidate| {
                        validate_completion_edit(&response.original_buffer, &candidate.edit).is_ok()
                    })
                    .collect();
                if candidates.is_empty() {
                    session.completion = CommandLineCompletionState::Closed;
                } else {
                    let selected = match pending.intent {
                        CompletionIntent::Forward => 0,
                        CompletionIntent::Reverse => candidates.len() - 1,
                    };
                    session.completion = CommandLineCompletionState::Open(OpenCompletion {
                        request_id: response.request_id,
                        buffer_revision: response.buffer_revision,
                        original_buffer: response.original_buffer,
                        candidates,
                        selected,
                    });
                }
                session.notice = notice;
            }
            CompletionOutcome::Empty(_) | CompletionOutcome::Canceled => {
                session.completion = CommandLineCompletionState::Closed;
            }
            CompletionOutcome::Unavailable(error) => {
                session.completion = CommandLineCompletionState::Closed;
                session.notice = Some(error);
            }
        }
    }

    pub fn finish_foreground_shell_request(
        &mut self,
        activation_id: u64,
        result: Result<i32, String>,
    ) {
        let active = self
            .command_line_session()
            .is_some_and(|session| session.activation_id == activation_id);
        if !active {
            return;
        }
        match result {
            Ok(status) => {
                self.command_line.set_last_exit_status(status);
                self.command_line.clear_after_success();
                self.clear_command_line_notice();
            }
            Err(error) => self.set_command_line_notice(format!("Command failed: {error}")),
        }
    }

    fn cancel_and_close_completion(&mut self) {
        let cancellation = self.command_line_session().and_then(|session| {
            let CommandLineCompletionState::Pending(pending) = &session.completion else {
                return None;
            };
            Some(CompletionCancellation {
                activation_id: session.activation_id,
                request_id: pending.request_id,
            })
        });
        if let Some(cancellation) = cancellation {
            self.pending_completion_cancellations.push(cancellation);
        }
        self.close_completion_without_cancel();
    }

    fn close_completion_without_cancel(&mut self) {
        if let Some(Route::CommandLine(session)) = self.routes.last_mut() {
            session.completion = CommandLineCompletionState::Closed;
        }
    }

    fn set_command_line_notice(&mut self, notice: impl Into<String>) {
        if let Some(Route::CommandLine(session)) = self.routes.last_mut() {
            session.notice = Some(notice.into());
        }
    }

    fn clear_command_line_notice(&mut self) {
        if let Some(Route::CommandLine(session)) = self.routes.last_mut() {
            session.notice = None;
        }
    }
}

fn expand_cd_path(path: PathBuf, cwd: &Path, expand_home: bool) -> PathBuf {
    if expand_home {
        let text = path.to_string_lossy();
        if text == "~" {
            return current_user_home_directory().unwrap_or(path);
        }
        if let Some(suffix) = text.strip_prefix("~/")
            && let Some(home) = current_user_home_directory()
        {
            // PathBuf::join replaces its base for an absolute suffix. A repeated slash after
            // the tilde (for example `~//tmp`) leaves such a suffix after strip_prefix, but
            // shells still interpret the path relative to HOME.
            return home.join(suffix.trim_start_matches('/'));
        }
    }
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

pub fn sanitize_paste(raw: &str) -> Result<String, &'static str> {
    if raw.len() > PASTE_PAYLOAD_LIMIT_BYTES {
        return Err("Paste payload exceeds 64 KiB");
    }
    if raw.contains('\0') {
        return Err("Paste contains NUL and was rejected");
    }
    let mut sanitized = String::with_capacity(raw.len());
    let mut in_line_separator_run = false;
    for character in raw.chars() {
        let line_separator = matches!(character, '\r' | '\n' | '\u{2028}' | '\u{2029}');
        if line_separator {
            if !in_line_separator_run {
                sanitized.push(' ');
                in_line_separator_run = true;
            }
            continue;
        }
        in_line_separator_run = false;
        match character {
            '\t' => sanitized.push(' '),
            character if is_c0_c1_or_del(character) => {}
            character => sanitized.push(character),
        }
    }
    if sanitized.len() > COMMAND_BUFFER_LIMIT_BYTES {
        return Err("Sanitized paste exceeds 64 KiB");
    }
    Ok(sanitized)
}

fn is_c0_c1_or_del(character: char) -> bool {
    matches!(character as u32, 0x00..=0x1f | 0x7f..=0x9f)
}

fn validate_completion_edit(buffer: &str, edit: &CompletionEdit) -> Result<(), &'static str> {
    if edit.start_byte > edit.end_byte
        || edit.end_byte > buffer.len()
        || !buffer.is_char_boundary(edit.start_byte)
        || !buffer.is_char_boundary(edit.end_byte)
        || edit.replacement.contains('\0')
    {
        return Err("Completion returned an invalid text edit");
    }
    Ok(())
}

fn codepoint_to_byte(value: &str, cursor: usize) -> usize {
    value
        .char_indices()
        .nth(cursor)
        .map_or(value.len(), |(byte, _)| byte)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::{
        BackgroundEvent, JobError, JobRequest, PanelRefreshResult, WorkerCommand, WorkerJob,
    };
    #[cfg(unix)]
    use rc_shell::CompletionProvider;

    fn model_with_history() -> CommandLineModel {
        CommandLineModel::new(ShellHistoryMode::Session)
    }

    #[cfg(unix)]
    fn open_command_line_for_test(app: &mut AppState) {
        app.open_command_line();
        let request = app
            .take_pending_shell_resolution_request()
            .expect("shell resolution should be queued");
        app.handle_shell_resolution_response(resolve_shell_request_blocking(request));
        assert!(app.command_line_session().is_some());
    }

    #[cfg(unix)]
    fn take_panel_refresh_job(app: &mut AppState) -> WorkerJob {
        app.take_pending_worker_commands()
            .into_iter()
            .find_map(|command| {
                let WorkerCommand::Run(job) = command else {
                    return None;
                };
                matches!(&job.request, JobRequest::RefreshPanel { .. }).then(|| *job)
            })
            .expect("panel refresh should be queued")
    }

    #[cfg(unix)]
    fn finish_panel_refresh(
        app: &mut AppState,
        job: WorkerJob,
        result: Result<PanelRefreshResult, String>,
    ) {
        let JobRequest::RefreshPanel {
            panel,
            cwd,
            source,
            sort_mode,
            filter,
            request_id,
            ..
        } = job.request
        else {
            panic!("expected panel refresh request");
        };
        app.handle_background_event(BackgroundEvent::PanelRefreshed {
            panel,
            cwd,
            source,
            sort_mode,
            filter,
            request_id,
            disk_usage: None,
            result,
        });
    }

    #[cfg(unix)]
    fn command_line_cd_fixture(label: &str) -> (PathBuf, PathBuf, PathBuf) {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("rc-command-line-{label}-{stamp}"));
        let current = root.join("parent/current");
        let destination = root.join("parent/destination");
        std::fs::create_dir_all(&current).expect("current directory should be creatable");
        std::fs::create_dir(&destination).expect("destination should be creatable");
        (root, current, destination)
    }

    #[cfg(unix)]
    fn empty_panel_refresh_result() -> PanelRefreshResult {
        PanelRefreshResult {
            entries: Vec::new(),
            panelized_entries: None,
            canonical_cwd: None,
            canonical_home_directory: None,
        }
    }

    #[test]
    fn paste_sanitization_is_atomic_and_follows_line_rules() {
        assert_eq!(
            sanitize_paste("  a\r\n\n\u{2028}b\tc\u{7f}  ").expect("valid paste"),
            "  a b c  "
        );
        assert!(sanitize_paste("a\0b").is_err());
        assert!(sanitize_paste(&"x".repeat(PASTE_PAYLOAD_LIMIT_BYTES + 1)).is_err());
    }

    #[test]
    fn history_restores_the_pre_navigation_draft_and_evicts_oldest() {
        let mut model = model_with_history();
        model.record_history("one");
        model.record_history("two");
        model.insert_paste("draft").expect("draft should insert");
        assert!(model.history_previous());
        assert_eq!(model.value(), "two");
        assert!(model.history_previous());
        assert_eq!(model.value(), "one");
        assert!(model.history_next());
        assert!(model.history_next());
        assert_eq!(model.value(), "draft");

        for index in 0..=COMMAND_HISTORY_LIMIT_ENTRIES {
            model.record_history(&format!("command-{index}"));
        }
        assert_eq!(model.history.len(), COMMAND_HISTORY_LIMIT_ENTRIES);
        assert!(model.history_bytes <= COMMAND_HISTORY_LIMIT_BYTES);
    }

    #[test]
    fn grapheme_editing_changes_revision_once() {
        let mut model = model_with_history();
        model.insert_paste("e\u{301}x").expect("text should insert");
        let revision = model.revision();
        model.move_cursor(InputRequest::GoToPrevChar);
        assert_eq!(model.revision(), revision);
        assert!(model.edit(InputRequest::DeletePrevChar));
        assert_eq!(model.value(), "x");
        assert_eq!(model.revision(), revision + 1);
    }

    #[test]
    fn visual_metrics_use_the_same_width_table_as_the_renderer() {
        let mut model = model_with_history();
        model.insert_paste("a☰").expect("test text should insert");

        // unicode-width 0.1 (used by Ratatui 0.28) treats U+2630 as one column, while
        // tui-input's 0.2 dependency treats it as two. The public model must match Ratatui.
        assert_eq!(model.visual_cursor(), 2);
        assert_eq!(model.visual_scroll(1), 1);
    }

    #[cfg(unix)]
    #[test]
    fn repeated_slashes_after_tilde_stay_beneath_home() {
        let home = current_user_home_directory().expect("Unix test should resolve HOME");
        assert_eq!(
            expand_cd_path(PathBuf::from("~//tmp"), Path::new("/unrelated"), true),
            home.join("tmp")
        );
        assert_eq!(
            expand_cd_path(PathBuf::from("~////"), Path::new("/unrelated"), true),
            home
        );
    }

    #[cfg(unix)]
    #[test]
    fn literal_cd_commits_only_after_its_normalized_directory_refresh_succeeds() {
        let (root, current, destination) = command_line_cd_fixture("commit");
        let mut app = AppState::new(current).expect("app");
        app.take_pending_worker_commands();
        open_command_line_for_test(&mut app);
        app.handle_command_line_input(CommandLineInput::Paste(String::from("cd ../destination")))
            .expect("literal cd should be accepted");
        app.handle_command_line_input(CommandLineInput::Enter)
            .expect("literal cd should start");

        assert_eq!(app.active_panel().cwd, destination);
        assert_eq!(app.command_line_model().value(), "cd ../destination");
        assert_eq!(app.command_line_model().last_exit_status(), None);
        let job = take_panel_refresh_job(&mut app);
        app.handle_command_line_input(CommandLineInput::Enter)
            .expect("duplicate submit should be ignored");
        app.handle_command_line_input(CommandLineInput::Complete(CompletionIntent::Forward))
            .expect("completion while changing directory should be ignored");
        assert!(app.take_pending_foreground_shell_requests().is_empty());
        assert!(app.take_pending_completion_requests().is_empty());

        finish_panel_refresh(&mut app, job, Ok(empty_panel_refresh_result()));

        assert_eq!(app.active_panel().cwd, root.join("parent/destination"));
        assert_eq!(app.command_line_model().value(), "");
        assert_eq!(app.command_line_model().last_exit_status(), Some(0));
        assert!(app.active_panel_mut().go_parent());
        assert_eq!(app.active_panel().cwd, root.join("parent"));

        std::fs::remove_dir_all(root).expect("fixture should be removable");
    }

    #[cfg(unix)]
    #[test]
    fn failed_literal_cd_refresh_rolls_back_and_keeps_the_command() {
        let (root, current, _) = command_line_cd_fixture("rollback");
        let mut app = AppState::new(current.clone()).expect("app");
        app.take_pending_worker_commands();
        open_command_line_for_test(&mut app);
        app.handle_command_line_input(CommandLineInput::Paste(String::from("cd ../destination")))
            .expect("literal cd should be accepted");
        app.handle_command_line_input(CommandLineInput::Enter)
            .expect("literal cd should start");
        let job = take_panel_refresh_job(&mut app);

        finish_panel_refresh(&mut app, job, Err(String::from("permission denied")));

        assert_eq!(app.active_panel().cwd, current);
        assert_eq!(app.command_line_model().value(), "cd ../destination");
        assert_eq!(app.command_line_model().last_exit_status(), Some(1));
        assert!(
            app.command_line_session()
                .and_then(|session| session.notice.as_deref())
                .is_some_and(|notice| notice.contains("previous directory restored"))
        );

        app.handle_command_line_input(CommandLineInput::Clear)
            .expect("old command should clear");
        app.handle_command_line_input(CommandLineInput::Paste(String::from("pwd")))
            .expect("new command should insert");
        app.handle_command_line_input(CommandLineInput::Enter)
            .expect("new command should submit");
        let foreground = app
            .take_pending_foreground_shell_requests()
            .pop()
            .expect("foreground command should be queued");
        assert_eq!(foreground.cwd, current);

        std::fs::remove_dir_all(root).expect("fixture should be removable");
    }

    #[cfg(unix)]
    #[test]
    fn literal_cd_dispatch_failure_uses_the_same_rollback() {
        let (root, current, _) = command_line_cd_fixture("dispatch-failure");
        let mut app = AppState::new(current.clone()).expect("app");
        app.take_pending_worker_commands();
        open_command_line_for_test(&mut app);
        app.handle_command_line_input(CommandLineInput::Paste(String::from("cd ../destination")))
            .expect("literal cd should be accepted");
        app.handle_command_line_input(CommandLineInput::Enter)
            .expect("literal cd should start");
        let job = take_panel_refresh_job(&mut app);

        app.handle_job_dispatch_failure(job.id, JobError::dispatch("runtime unavailable"));

        assert_eq!(app.active_panel().cwd, current);
        assert_eq!(app.command_line_model().value(), "cd ../destination");
        assert_eq!(app.command_line_model().last_exit_status(), Some(1));

        std::fs::remove_dir_all(root).expect("fixture should be removable");
    }

    #[cfg(unix)]
    #[test]
    fn successful_literal_cd_does_not_clear_a_newer_draft() {
        let (root, current, destination) = command_line_cd_fixture("newer-draft");
        let mut app = AppState::new(current).expect("app");
        app.take_pending_worker_commands();
        open_command_line_for_test(&mut app);
        app.handle_command_line_input(CommandLineInput::Paste(String::from("cd ../destination")))
            .expect("literal cd should be accepted");
        app.handle_command_line_input(CommandLineInput::Enter)
            .expect("literal cd should start");
        let job = take_panel_refresh_job(&mut app);
        app.handle_command_line_input(CommandLineInput::Character('x'))
            .expect("newer draft should be editable while refresh is pending");

        finish_panel_refresh(&mut app, job, Ok(empty_panel_refresh_result()));

        assert_eq!(app.active_panel().cwd, destination);
        assert_eq!(app.command_line_model().value(), "cd ../destinationx");
        assert_eq!(app.command_line_model().last_exit_status(), Some(0));

        std::fs::remove_dir_all(root).expect("fixture should be removable");
    }

    #[cfg(unix)]
    #[test]
    fn command_line_activation_queues_shell_resolution_before_opening_route() {
        let root = std::env::current_dir().expect("cwd");
        let mut app = AppState::new(root.clone()).expect("app");
        app.set_status("Panel refresh complete");
        let previous_status_expires_at = app.status_expires_at;

        app.open_command_line();

        assert!(app.command_line_session().is_none());
        assert_eq!(app.status_line, "Resolving shell...");
        let request = app
            .take_pending_shell_resolution_request()
            .expect("one shell-resolution request should be queued");
        assert_eq!(request.cwd, root);
        app.open_command_line();
        assert!(app.take_pending_shell_resolution_request().is_none());
        assert_eq!(app.status_line, "Shell selection is still pending");

        app.handle_shell_resolution_response(resolve_shell_request_blocking(request));

        assert!(app.command_line_session().is_some());
        assert_eq!(app.status_line, "Panel refresh complete");
        assert_eq!(app.status_expires_at, previous_status_expires_at);
        app.handle_command_line_input(CommandLineInput::Escape)
            .expect("command line should close");
        assert!(app.command_line_session().is_none());
        assert_eq!(app.status_line, "Panel refresh complete");
    }

    #[cfg(unix)]
    #[test]
    fn shell_resolution_does_not_overwrite_a_newer_status() {
        let root = std::env::current_dir().expect("cwd");
        let mut app = AppState::new(root).expect("app");

        app.open_command_line();
        let request = app
            .take_pending_shell_resolution_request()
            .expect("one shell-resolution request should be queued");
        app.set_status("Background refresh finished");
        let newer_status_expires_at = app.status_expires_at;

        app.handle_shell_resolution_response(resolve_shell_request_blocking(request));

        assert!(app.command_line_session().is_some());
        assert_eq!(app.status_line, "Background refresh finished");
        assert_eq!(app.status_expires_at, newer_status_expires_at);
    }

    #[cfg(unix)]
    #[test]
    fn expired_resolution_notice_restores_a_persistent_previous_status() {
        let root = std::env::current_dir().expect("cwd");
        let mut app = AppState::new(root).expect("app");
        assert_eq!(app.status_line, "Press F1 for help");
        assert!(app.status_expires_at.is_none());

        app.open_command_line();
        let request = app
            .take_pending_shell_resolution_request()
            .expect("one shell-resolution request should be queued");
        let resolving_expires_at = app
            .status_expires_at
            .expect("resolving notice should have a deadline");
        app.expire_status_line_at(resolving_expires_at);
        assert!(app.status_line.is_empty());

        app.handle_shell_resolution_response(resolve_shell_request_blocking(request));

        assert!(app.command_line_session().is_some());
        assert_eq!(app.status_line, "Press F1 for help");
        assert!(app.status_expires_at.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn stale_completion_response_cannot_reopen_pager() {
        let root = std::env::current_dir().expect("cwd");
        let mut app = AppState::new(root).expect("app");
        open_command_line_for_test(&mut app);
        app.handle_command_line_input(CommandLineInput::Complete(CompletionIntent::Forward))
            .expect("completion request");
        let request = app
            .take_pending_completion_requests()
            .pop()
            .expect("request");
        app.handle_command_line_input(CommandLineInput::Character('x'))
            .expect("edit");
        app.handle_completion_response(CompletionResponse {
            activation_id: request.activation_id,
            request_id: request.request_id,
            buffer_revision: request.buffer_revision,
            original_buffer: request.buffer,
            outcome: CompletionOutcome::Candidates {
                provider: CompletionProvider::Generic,
                candidates: vec![CompletionCandidate {
                    edit: CompletionEdit {
                        start_byte: 0,
                        end_byte: 0,
                        replacement: String::from("echo"),
                    },
                    display: String::from("echo"),
                    description: None,
                }],
                notice: None,
            },
        });
        assert!(matches!(
            app.command_line_session().expect("session").completion,
            CommandLineCompletionState::Closed
        ));
    }

    #[cfg(unix)]
    #[test]
    fn completion_acceptance_is_one_edit_and_does_not_execute() {
        let root = std::env::current_dir().expect("cwd");
        let mut app = AppState::new(root).expect("app");
        open_command_line_for_test(&mut app);
        app.handle_command_line_input(CommandLineInput::Complete(CompletionIntent::Reverse))
            .expect("completion request");
        let request = app
            .take_pending_completion_requests()
            .pop()
            .expect("request");
        app.handle_completion_response(CompletionResponse {
            activation_id: request.activation_id,
            request_id: request.request_id,
            buffer_revision: request.buffer_revision,
            original_buffer: request.buffer,
            outcome: CompletionOutcome::Candidates {
                provider: CompletionProvider::Generic,
                candidates: ["a", "b"]
                    .into_iter()
                    .map(|replacement| CompletionCandidate {
                        edit: CompletionEdit {
                            start_byte: 0,
                            end_byte: 0,
                            replacement: replacement.to_string(),
                        },
                        display: replacement.to_string(),
                        description: None,
                    })
                    .collect(),
                notice: None,
            },
        });
        let revision = app.command_line.revision();
        app.handle_command_line_input(CommandLineInput::Enter)
            .expect("accept completion");
        assert_eq!(app.command_line.value(), "b");
        assert_eq!(app.command_line.revision(), revision + 1);
        assert!(app.pending_foreground_shell_requests.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn pending_enter_cancels_completion_and_executes_the_unchanged_buffer() {
        let root = std::env::current_dir().expect("cwd");
        let mut app = AppState::new(root).expect("app");
        open_command_line_for_test(&mut app);
        app.handle_command_line_input(CommandLineInput::Paste(String::from("printf unchanged")))
            .expect("paste");
        app.handle_command_line_input(CommandLineInput::Complete(CompletionIntent::Forward))
            .expect("completion request");
        let completion_request = app
            .take_pending_completion_requests()
            .pop()
            .expect("request");

        app.handle_command_line_input(CommandLineInput::Enter)
            .expect("submit while pending");
        assert_eq!(
            app.take_pending_completion_cancellations(),
            vec![CompletionCancellation {
                activation_id: completion_request.activation_id,
                request_id: completion_request.request_id,
            }]
        );
        let foreground = app
            .take_pending_foreground_shell_requests()
            .pop()
            .expect("foreground request");
        assert_eq!(foreground.command, "printf unchanged");
    }

    #[cfg(unix)]
    #[test]
    fn open_pager_cycles_without_editing_and_ordinary_input_closes_it() {
        let root = std::env::current_dir().expect("cwd");
        let mut app = AppState::new(root).expect("app");
        open_command_line_for_test(&mut app);
        app.handle_command_line_input(CommandLineInput::Complete(CompletionIntent::Forward))
            .expect("completion request");
        let request = app
            .take_pending_completion_requests()
            .pop()
            .expect("request");
        app.handle_completion_response(CompletionResponse {
            activation_id: request.activation_id,
            request_id: request.request_id,
            buffer_revision: request.buffer_revision,
            original_buffer: request.buffer,
            outcome: CompletionOutcome::Candidates {
                provider: CompletionProvider::Generic,
                candidates: ["a", "b"]
                    .into_iter()
                    .map(|replacement| CompletionCandidate {
                        edit: CompletionEdit {
                            start_byte: 0,
                            end_byte: 0,
                            replacement: replacement.to_string(),
                        },
                        display: replacement.to_string(),
                        description: None,
                    })
                    .collect(),
                notice: None,
            },
        });
        let revision = app.command_line_model().revision();
        app.handle_command_line_input(CommandLineInput::Complete(CompletionIntent::Forward))
            .expect("cycle forward");
        assert_eq!(
            app.command_line_session()
                .expect("session")
                .selected_completion()
                .expect("candidate")
                .display,
            "b"
        );
        app.handle_command_line_input(CommandLineInput::Complete(CompletionIntent::Reverse))
            .expect("cycle backward");
        assert_eq!(
            app.command_line_session()
                .expect("session")
                .selected_completion()
                .expect("candidate")
                .display,
            "a"
        );
        assert_eq!(app.command_line_model().revision(), revision);

        app.handle_command_line_input(CommandLineInput::Character('x'))
            .expect("ordinary edit");
        assert_eq!(app.command_line_model().value(), "x");
        assert!(matches!(
            app.command_line_session().expect("session").completion,
            CommandLineCompletionState::Closed
        ));
    }

    #[cfg(unix)]
    #[test]
    fn authoritative_empty_response_closes_without_fallback_notice() {
        let root = std::env::current_dir().expect("cwd");
        let mut app = AppState::new(root).expect("app");
        open_command_line_for_test(&mut app);
        app.handle_command_line_input(CommandLineInput::Complete(CompletionIntent::Forward))
            .expect("completion request");
        let request = app
            .take_pending_completion_requests()
            .pop()
            .expect("request");
        app.handle_completion_response(CompletionResponse {
            activation_id: request.activation_id,
            request_id: request.request_id,
            buffer_revision: request.buffer_revision,
            original_buffer: request.buffer,
            outcome: CompletionOutcome::Empty(CompletionProvider::Fish),
        });
        let session = app.command_line_session().expect("session");
        assert!(matches!(
            session.completion,
            CommandLineCompletionState::Closed
        ));
        assert!(session.notice.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn typing_past_the_buffer_limit_is_rejected_without_revision_change() {
        let root = std::env::current_dir().expect("cwd");
        let mut app = AppState::new(root).expect("app");
        open_command_line_for_test(&mut app);
        app.handle_command_line_input(CommandLineInput::Paste(
            "x".repeat(COMMAND_BUFFER_LIMIT_BYTES),
        ))
        .expect("maximum-sized paste");
        let revision = app.command_line_model().revision();
        app.handle_command_line_input(CommandLineInput::Character('y'))
            .expect("limit rejection is nonfatal");
        assert_eq!(
            app.command_line_model().value().len(),
            COMMAND_BUFFER_LIMIT_BYTES
        );
        assert_eq!(app.command_line_model().revision(), revision);
        assert!(
            app.command_line_session()
                .expect("session")
                .notice
                .as_deref()
                .is_some_and(|notice| notice.contains("64 KiB"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn escape_retains_draft_and_revision_but_discards_activation_state() {
        let root = std::env::current_dir().expect("cwd");
        let mut app = AppState::new(root).expect("app");
        open_command_line_for_test(&mut app);
        let first_activation = app.command_line_session().expect("session").activation_id;
        app.handle_command_line_input(CommandLineInput::Paste(String::from("echo draft")))
            .expect("paste");
        let revision = app.command_line_model().revision();
        app.handle_command_line_input(CommandLineInput::Escape)
            .expect("escape");
        assert!(matches!(app.top_route(), Route::FileManager));

        open_command_line_for_test(&mut app);
        assert_eq!(app.command_line_model().value(), "echo draft");
        assert_eq!(app.command_line_model().revision(), revision);
        assert_ne!(
            app.command_line_session()
                .expect("new session")
                .activation_id,
            first_activation
        );
        assert!(matches!(
            app.command_line_session().expect("session").completion,
            CommandLineCompletionState::Closed
        ));
    }

    #[cfg(unix)]
    #[test]
    fn completed_nonzero_command_is_history_and_clears_the_buffer() {
        let root = std::env::current_dir().expect("cwd");
        let mut app = AppState::new(root).expect("app");
        open_command_line_for_test(&mut app);
        app.handle_command_line_input(CommandLineInput::Paste(String::from("exit 7")))
            .expect("paste");
        app.handle_command_line_input(CommandLineInput::Enter)
            .expect("submit");
        let request = app
            .take_pending_foreground_shell_requests()
            .pop()
            .expect("foreground request");
        assert_eq!(request.command, "exit 7");
        assert_eq!(
            request
                .shell
                .invocation(&request.command)
                .arguments
                .last()
                .unwrap(),
            "exit 7"
        );
        app.finish_foreground_shell_request(request.activation_id, Ok(7));
        assert_eq!(app.command_line_model().value(), "");
        assert_eq!(app.command_line_model().last_exit_status(), Some(7));
        assert_eq!(app.command_line_model().history().back().unwrap(), "exit 7");
    }

    #[cfg(unix)]
    #[test]
    fn rejected_paste_leaves_revision_and_completion_unchanged() {
        let root = std::env::current_dir().expect("cwd");
        let mut app = AppState::new(root).expect("app");
        open_command_line_for_test(&mut app);
        app.handle_command_line_input(CommandLineInput::Complete(CompletionIntent::Forward))
            .expect("completion");
        let revision = app.command_line_model().revision();
        let pending = app
            .command_line_session()
            .expect("session")
            .completion
            .clone();
        app.handle_command_line_input(CommandLineInput::Paste(format!("{}\0", "x".repeat(100))))
            .expect("rejection is nonfatal");
        assert_eq!(app.command_line_model().revision(), revision);
        assert!(matches!(
            (&pending, &app.command_line_session().expect("session").completion),
            (
                CommandLineCompletionState::Pending(left),
                CommandLineCompletionState::Pending(right)
            ) if left.request_id == right.request_id
        ));
        assert!(app.take_pending_completion_cancellations().is_empty());
    }

    #[test]
    fn history_off_disables_recording_navigation_and_suggestions() {
        let mut model = CommandLineModel::new(ShellHistoryMode::Off);
        model.record_history("secret");
        assert!(model.history().is_empty());
        assert!(!model.history_previous());
        assert!(model.autosuggestion().is_none());
    }
}
