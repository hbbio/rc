//! Persistent interactive Unix shell transport and its private prompt protocol.
//!
//! Supports Bash, Zsh, Fish, and sh/dash with the standard command template.
//! Unsupported shells/templates return `ErrorKind::Unsupported`, allowing a
//! caller to retain its existing foreground execution path.
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::sys::signal::{Signal, killpg};
use nix::sys::stat::Mode;
use nix::unistd::{Pid, mkfifo};
use portable_pty::{Child, MasterPty, PtySize, native_pty_system};

use crate::{ResolvedShell, ShellDialect};

mod integration;

const IO_TICK: Duration = Duration::from_millis(5);
const INPUT_LIMIT: usize = 64 * 1024;
const PROTOCOL_LIMIT: usize = 16 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellPrompt {
    pub command_id: u64,
    pub directory: PathBuf,
    pub exit_status: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubshellEvent {
    Output(Vec<u8>),
    Prompt(ShellPrompt),
    Exited(i32),
    Error(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TransportEvent {
    Event(SubshellEvent),
    Started(u64),
    Prepared(ShellPrompt),
    Redraw(ShellPrompt),
}

impl From<SubshellEvent> for TransportEvent {
    fn from(event: SubshellEvent) -> Self {
        Self::Event(event)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SubshellOptions {
    pub rows: u16,
    pub columns: u16,
    pub read_startup_files: bool,
}

impl Default for SubshellOptions {
    fn default() -> Self {
        Self {
            rows: 24,
            columns: 80,
            read_startup_files: true,
        }
    }
}

pub struct PersistentShell {
    master: Option<Box<dyn MasterPty + Send>>,
    child: Box<dyn Child + Send + Sync>,
    input: mpsc::SyncSender<Vec<u8>>,
    events: mpsc::Receiver<TransportEvent>,
    stopped: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    files: tempfile::TempDir,
    control: File,
    command_files: BTreeMap<u64, tempfile::NamedTempFile>,
    next_command_id: u64,
    awaiting_command: Option<u64>,
    started_command: Option<u64>,
    pending_gate: Option<u64>,
    cancel_requested: bool,
    interrupt_sent: bool,
    prepared_prompt: Option<ShellPrompt>,
    dialect: ShellDialect,
    prompt: Option<ShellPrompt>,
    at_prompt: bool,
    incomplete_input: bool,
    exit_observed: Option<(i32, Instant)>,
    exited: bool,
}

impl PersistentShell {
    pub fn spawn(
        shell: &ResolvedShell,
        directory: &Path,
        options: SubshellOptions,
    ) -> io::Result<Self> {
        let files = tempfile::Builder::new().prefix("rc-shell-").tempdir()?;
        let control_path = files.path().join("control");
        mkfifo(&control_path, Mode::S_IRUSR | Mode::S_IWUSR)?;
        let control = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(OFlag::O_NONBLOCK.bits())
            .open(&control_path)?;
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
        let nonce = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let command = integration::command(
            shell,
            directory,
            files.path(),
            &nonce,
            options.read_startup_files,
        )?;
        let pair = native_pty_system()
            .openpty(size(options.rows, options.columns))
            .map_err(pty_error)?;
        let fd = pair
            .master
            .as_raw_fd()
            .ok_or_else(|| io::Error::other("PTY does not expose a Unix descriptor"))?;
        let flags = OFlag::from_bits_truncate(fcntl(fd, FcntlArg::F_GETFL)?);
        fcntl(fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
        let reader = pair.master.try_clone_reader().map_err(pty_error)?;
        let writer = pair.master.take_writer().map_err(pty_error)?;
        let child = pair.slave.spawn_command(command).map_err(pty_error)?;
        drop(pair.slave);
        let stopped = Arc::new(AtomicBool::new(false));
        let (events_tx, events) = mpsc::sync_channel(128);
        let (input, input_rx) = mpsc::sync_channel(32);
        let reader_stop = Arc::clone(&stopped);
        let reader_events = events_tx.clone();
        let reader = thread::spawn(move || read_output(reader, &nonce, reader_events, reader_stop));
        let writer_stop = Arc::clone(&stopped);
        let writer = thread::spawn(move || write_input(writer, input_rx, events_tx, writer_stop));
        Ok(Self {
            master: Some(pair.master),
            child,
            input,
            events,
            stopped,
            reader: Some(reader),
            writer: Some(writer),
            files,
            control,
            command_files: BTreeMap::new(),
            next_command_id: 1,
            awaiting_command: None,
            started_command: None,
            pending_gate: None,
            cancel_requested: false,
            interrupt_sent: false,
            prepared_prompt: None,
            dialect: shell.dialect,
            prompt: None,
            at_prompt: false,
            incomplete_input: false,
            exit_observed: None,
            exited: false,
        })
    }

    pub fn at_prompt(&self) -> bool {
        self.at_prompt
            && !self.exited
            && self.exit_observed.is_none()
            && self.master.as_ref().is_some_and(|pty| {
                pty.process_group_leader().map(|pid| pid as u32) == self.child.process_id()
            })
    }

    pub fn last_prompt(&self) -> Option<&ShellPrompt> {
        self.prompt.as_ref()
    }

    pub fn process_id(&self) -> Option<u32> {
        self.child.process_id()
    }

    pub fn poll(&mut self) -> Vec<SubshellEvent> {
        let mut events = Vec::new();
        // Bound each UI poll even when a child produces output continuously.
        for event in self.events.try_iter().take(128) {
            let mut event = match event {
                TransportEvent::Event(event) => event,
                TransportEvent::Started(id) => {
                    self.prepared_prompt = None;
                    if self.awaiting_command == Some(id) && self.started_command != Some(id) {
                        self.pending_gate = Some(id);
                    }
                    continue;
                }
                TransportEvent::Prepared(prompt) => {
                    self.prepared_prompt = Some(prompt);
                    continue;
                }
                TransportEvent::Redraw(prompt)
                    if self
                        .prepared_prompt
                        .as_ref()
                        .is_some_and(|prepared| prepared.command_id == prompt.command_id) =>
                {
                    SubshellEvent::Prompt(
                        self.prepared_prompt
                            .take()
                            .expect("matching prepared prompt"),
                    )
                }
                TransportEvent::Redraw(prompt) if self.cancel_requested && self.interrupt_sent => {
                    SubshellEvent::Prompt(prompt)
                }
                TransportEvent::Redraw(_) => continue,
            };
            if let SubshellEvent::Prompt(prompt) = &mut event {
                if self
                    .awaiting_command
                    .is_some_and(|id| id != prompt.command_id)
                {
                    continue;
                }
                if self.cancel_requested {
                    prompt.exit_status = 130;
                }
                self.cancel_requested = false;
                self.interrupt_sent = false;
                self.awaiting_command = None;
                self.pending_gate = None;
                self.prompt = Some(prompt.clone());
                self.at_prompt = !self.incomplete_input;
                // A canceled line may still be queued behind a prompt redraw.
                // Only retire files once their ID (or a later one) executed.
                self.command_files.retain(|id, _| *id > prompt.command_id);
            }
            events.push(event);
        }
        if let Some(id) = self.pending_gate {
            let decision = if self.cancel_requested {
                "cancel"
            } else {
                "go"
            };
            // A single short record is atomic on the nonblocking FIFO. Retry
            // a full pipe on a later poll without blocking terminal input.
            match self
                .control
                .write_all(format!("{id} {decision}\n").as_bytes())
            {
                Ok(()) => {
                    self.started_command = Some(id);
                    self.pending_gate = None;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => {
                    self.pending_gate = None;
                    events.push(SubshellEvent::Error(error.to_string()));
                }
            }
        }
        if !self.exited && self.exit_observed.is_none() {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.at_prompt = false;
                    self.exit_observed = Some((status.exit_code() as i32, Instant::now()));
                }
                Err(error) => events.push(SubshellEvent::Error(error.to_string())),
                Ok(None) => {}
            }
        }
        if !self.exited
            && let Some((status, observed)) = self.exit_observed
            && ((events.len() < 128 && self.reader.as_ref().is_none_or(JoinHandle::is_finished))
                || observed.elapsed() >= Duration::from_millis(100))
        {
            self.exited = true;
            events.push(SubshellEvent::Exited(status));
        }
        events
    }

    pub fn resize(&self, rows: u16, columns: u16) -> io::Result<()> {
        self.master
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "shell has closed"))?
            .resize(size(rows, columns))
            .map_err(pty_error)
    }

    /// Send terminal input. Any partial input makes directory/command injection
    /// unavailable until the next prompt, so it cannot overwrite an edited line.
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        // Cancel queued commands through their private launch gate. A terminal
        // interrupt before the line editor finishes can lose the protocol ID.
        if bytes == [3]
            && self.awaiting_command.is_some()
            && self.started_command != self.awaiting_command
        {
            self.cancel_requested = true;
            self.incomplete_input = false;
            self.at_prompt = false;
            return Ok(());
        }
        self.write_response(bytes)?;
        if !bytes.is_empty() {
            self.at_prompt = false;
            for byte in bytes {
                self.incomplete_input = !matches!(byte, b'\r' | b'\n' | 3 | 4);
                if *byte == 3 {
                    self.cancel_requested = true;
                    self.interrupt_sent = true;
                }
            }
        }
        Ok(())
    }

    /// Send terminal query replies without treating them as an edited command.
    pub fn write_response(&self, bytes: &[u8]) -> io::Result<()> {
        if self.exited || self.exit_observed.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "shell has exited",
            ));
        }
        if bytes.len() > INPUT_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "shell input exceeds 64 KiB",
            ));
        }
        if bytes.is_empty() {
            return Ok(());
        }
        self.input
            .try_send(bytes.to_vec())
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => {
                    io::Error::new(io::ErrorKind::WouldBlock, "shell input queue is full")
                }
                mpsc::TrySendError::Disconnected(_) => {
                    io::Error::new(io::ErrorKind::BrokenPipe, "shell input closed")
                }
            })?;
        Ok(())
    }

    pub fn interrupt(&mut self) -> io::Result<()> {
        self.write(&[3])
    }

    /// Source a private script in the existing shell so variables, functions and
    /// directory changes persist. The script remains alive until a prompt arrives.
    pub fn run_command(&mut self, command: &str) -> io::Result<()> {
        if !self.at_prompt() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "shell is busy or has unfinished input",
            ));
        }
        if command.len() > INPUT_LIMIT || command.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "shell command exceeds 64 KiB or contains NUL",
            ));
        }
        if self.command_files.len() >= 128 {
            return Err(io::Error::other(
                "too many unacknowledged shell commands; restart the shell",
            ));
        }
        let id = self.next_command_id;
        self.next_command_id = id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("shell command IDs exhausted"))?;
        let script = integration::command_script(command, self.dialect);
        let mut file = tempfile::Builder::new()
            .prefix("command-")
            .tempfile_in(self.files.path())?;
        file.write_all(&script)?;
        file.flush()?;
        // Assign the protocol ID outside the sourced file: Fish parses the
        // entire file first, so a syntax error would otherwise skip its prelude.
        // Only an ASCII basename traverses the terminal input parser. The
        // private directory may itself contain control or non-UTF-8 bytes.
        let line = integration::command_line(
            id,
            file.path().file_name().expect("temporary filename"),
            self.dialect,
        );
        self.write(&line)?;
        self.awaiting_command = Some(id);
        self.started_command = None;
        self.cancel_requested = false;
        self.interrupt_sent = false;
        self.command_files.insert(id, file);
        Ok(())
    }

    pub fn synchronize_directory(&mut self, directory: &Path) -> io::Result<bool> {
        if self
            .prompt
            .as_ref()
            .is_some_and(|prompt| prompt.directory == directory)
        {
            return Ok(false);
        }
        if !directory.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "shell directory must be absolute",
            ));
        }
        // A native argument reference is expanded by command_script without
        // interpreting the directory as shell syntax.
        self.run_command(&format!(
            "cd -- {}",
            crate::quote_native_argument(directory.as_os_str(), self.dialect)
        ))?;
        Ok(true)
    }

    pub fn shutdown(&mut self) {
        if self.stopped.swap(true, Ordering::Relaxed) {
            return;
        }
        let foreground = self
            .master
            .as_ref()
            .and_then(|pty| pty.process_group_leader());
        let leader = self
            .child
            .process_id()
            .and_then(|pid| i32::try_from(pid).ok());
        let mut reaped =
            self.exit_observed.is_some() || matches!(self.child.try_wait(), Ok(Some(_)));
        let was_running = !reaped;
        if !reaped {
            for group in [foreground, leader].into_iter().flatten() {
                if group > 1 {
                    let _ = killpg(Pid::from_raw(group), Signal::SIGHUP);
                }
            }
        }
        let deadline = Instant::now() + Duration::from_millis(100);
        while !reaped && Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                reaped = true;
                break;
            }
            thread::sleep(IO_TICK);
        }
        for group in [foreground, leader].into_iter().flatten() {
            if was_running && group > 1 && (!reaped || Some(group) != leader) {
                let _ = killpg(Pid::from_raw(group), Signal::SIGKILL);
            }
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        // Close every master descriptor before reaping. On macOS a terminating
        // child can remain inside terminal teardown while the master is open.
        self.master.take();
        if !reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        self.exited = true;
        self.at_prompt = false;
    }
}

impl Drop for PersistentShell {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn size(rows: u16, columns: u16) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: columns.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn pty_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

fn send_event(
    tx: &mpsc::SyncSender<TransportEvent>,
    event: impl Into<TransportEvent>,
    stopped: &AtomicBool,
) -> bool {
    let mut event = event.into();
    while !stopped.load(Ordering::Relaxed) {
        match tx.try_send(event) {
            Ok(()) => return true,
            Err(mpsc::TrySendError::Disconnected(_)) => return false,
            Err(mpsc::TrySendError::Full(returned)) => {
                event = returned;
                thread::sleep(IO_TICK);
            }
        }
    }
    false
}

fn read_output(
    mut reader: Box<dyn Read + Send>,
    nonce: &str,
    events: mpsc::SyncSender<TransportEvent>,
    stopped: Arc<AtomicBool>,
) {
    let mut decoder = PromptDecoder::new(nonce);
    let mut buffer = [0; 8192];
    while !stopped.load(Ordering::Relaxed) {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(length) => {
                for event in decoder.feed(&buffer[..length]) {
                    if !send_event(&events, event, &stopped) {
                        return;
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(IO_TICK),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                send_event(&events, SubshellEvent::Error(error.to_string()), &stopped);
                break;
            }
        }
    }
    if !decoder.pending.is_empty() {
        send_event(&events, SubshellEvent::Output(decoder.pending), &stopped);
    }
}

fn write_input(
    mut writer: Box<dyn Write + Send>,
    input: mpsc::Receiver<Vec<u8>>,
    events: mpsc::SyncSender<TransportEvent>,
    stopped: Arc<AtomicBool>,
) {
    while !stopped.load(Ordering::Relaxed) {
        let bytes = match input.recv_timeout(IO_TICK) {
            Ok(bytes) => bytes,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let mut offset = 0;
        while offset < bytes.len() && !stopped.load(Ordering::Relaxed) {
            match writer.write(&bytes[offset..]) {
                Ok(0) => {
                    send_event(
                        &events,
                        SubshellEvent::Error("PTY input closed".into()),
                        &stopped,
                    );
                    return;
                }
                Ok(length) => offset += length,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(IO_TICK),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    send_event(&events, SubshellEvent::Error(error.to_string()), &stopped);
                    return;
                }
            }
        }
    }
}

struct PromptDecoder {
    prefix: Vec<u8>,
    pending: Vec<u8>,
}

impl PromptDecoder {
    fn new(nonce: &str) -> Self {
        Self {
            prefix: format!("\x1b]777;rc;{nonce};").into_bytes(),
            pending: Vec::new(),
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> Vec<TransportEvent> {
        let mut events = Vec::new();
        let mut output = Vec::new();
        for byte in bytes {
            if self.pending.is_empty() && *byte != self.prefix[0] {
                output.push(*byte);
                continue;
            }
            self.pending.push(*byte);
            if self.pending.len() <= self.prefix.len() {
                if !self.prefix.starts_with(&self.pending) {
                    let restart = self.pending.last() == self.prefix.first();
                    let length = self.pending.len() - usize::from(restart);
                    output.extend(self.pending.drain(..length));
                }
            } else if *byte == 7 {
                if let Some(prompt) = self.parse_prompt() {
                    if !output.is_empty() {
                        events.push(SubshellEvent::Output(std::mem::take(&mut output)).into());
                    }
                    events.push(prompt);
                } else {
                    output.extend_from_slice(&self.pending);
                }
                self.pending.clear();
            } else if self.pending.len() >= PROTOCOL_LIMIT {
                output.append(&mut self.pending);
            }
        }
        if !output.is_empty() {
            events.push(SubshellEvent::Output(output).into());
        }
        events
    }

    fn parse_prompt(&self) -> Option<TransportEvent> {
        let payload = &self.pending[self.prefix.len()..self.pending.len() - 1];
        if let Some(id) = payload.strip_prefix(b"s") {
            return Some(TransportEvent::Started(
                std::str::from_utf8(id).ok()?.parse().ok()?,
            ));
        }
        let mut fields = payload.splitn(3, |byte| *byte == b';');
        let id = std::str::from_utf8(fields.next()?).ok()?;
        let redraw = id.starts_with('r');
        let prepared = id.starts_with('p');
        let command_id = if redraw || prepared { &id[1..] } else { id }
            .parse()
            .ok()?;
        let mut exit_status: i32 = std::str::from_utf8(fields.next()?).ok()?.parse().ok()?;
        // Korn shell reports a terminating signal as 256 + signal, while
        // other supported shells use 128 + signal.
        if (256..=383).contains(&exit_status) {
            exit_status -= 128;
        }
        if !(0..=255).contains(&exit_status) {
            return None;
        }
        let hex = std::str::from_utf8(fields.next()?).ok()?;
        let path =
            crate::native_arguments::decode_native_variable(&format!("RC_NATIVE_HEX_{hex}"))?;
        let directory = PathBuf::from(path);
        if !directory.is_absolute() {
            return None;
        }
        let prompt = ShellPrompt {
            command_id,
            directory,
            exit_status,
        };
        Some(if redraw {
            TransportEvent::Redraw(prompt)
        } else if prepared {
            TransportEvent::Prepared(prompt)
        } else {
            SubshellEvent::Prompt(prompt).into()
        })
    }
}

#[cfg(test)]
mod tests;
