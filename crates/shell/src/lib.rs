#![forbid(unsafe_code)]

use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessOutputLimits {
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessExit {
    pub success: bool,
    pub status_label: String,
    pub stderr: Vec<u8>,
}

pub trait ProcessBackend {
    fn run_shell_command_streaming(
        &self,
        cwd: &Path,
        command: &str,
        cancel_flag: Option<&AtomicBool>,
        canceled_message: &str,
        limits: ProcessOutputLimits,
        stdout_line: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> io::Result<ProcessExit>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LocalProcessBackend;

impl ProcessBackend for LocalProcessBackend {
    fn run_shell_command_streaming(
        &self,
        cwd: &Path,
        command: &str,
        cancel_flag: Option<&AtomicBool>,
        canceled_message: &str,
        limits: ProcessOutputLimits,
        stdout_line: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> io::Result<ProcessExit> {
        run_shell_command_streaming_impl(
            cwd,
            command,
            cancel_flag,
            canceled_message,
            limits,
            stdout_line,
        )
    }
}

enum StdoutReaderEvent {
    Line(Vec<u8>),
    LimitExceeded(usize),
    Finished(io::Result<()>),
}

fn run_shell_command_streaming_impl(
    cwd: &Path,
    command: &str,
    cancel_flag: Option<&AtomicBool>,
    canceled_message: &str,
    limits: ProcessOutputLimits,
    stdout_line: &mut dyn FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<ProcessExit> {
    let mut child = spawn_shell_command(cwd, command)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("failed to capture command stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("failed to capture command stderr"))?;

    let (stdout_event_tx, stdout_event_rx) = mpsc::channel();
    let stdout_handle = thread::spawn(move || {
        let result = read_stdout_lines(stdout, limits.stdout_bytes, stdout_event_tx.clone());
        let _ = stdout_event_tx.send(StdoutReaderEvent::Finished(result));
    });

    let stderr_overflow = Arc::new(AtomicBool::new(false));
    let stderr_overflow_reader = Arc::clone(&stderr_overflow);
    let stderr_handle = thread::spawn(move || {
        read_bounded_stream(
            stderr,
            limits.stderr_bytes,
            stderr_overflow_reader,
            "stderr",
        )
    });

    let mut child_status = None;
    let mut stdout_done = false;
    loop {
        if cancel_flag.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            terminate_shell_command(&mut child);
            let _ = child.wait();
            let _ = stdout_handle.join();
            let _ = stderr_handle.join();
            return Err(io::Error::new(io::ErrorKind::Interrupted, canceled_message));
        }

        if stderr_overflow.load(Ordering::Relaxed) {
            terminate_shell_command(&mut child);
            let _ = child.wait();
            let _ = stdout_handle.join();
            let _ = stderr_handle.join();
            return Err(io::Error::other(format!(
                "command stderr exceeded {} bytes",
                limits.stderr_bytes
            )));
        }

        while let Ok(event) = stdout_event_rx.try_recv() {
            if let Err(error) = handle_stdout_event(event, &mut stdout_done, stdout_line) {
                terminate_shell_command(&mut child);
                let _ = child.wait();
                let _ = stdout_handle.join();
                let _ = stderr_handle.join();
                return Err(error);
            }
        }

        if child_status.is_none() {
            child_status = child.try_wait()?;
        }
        if child_status.is_some() && stdout_done {
            break;
        }

        match stdout_event_rx.recv_timeout(Duration::from_millis(20)) {
            Ok(event) => {
                if let Err(error) = handle_stdout_event(event, &mut stdout_done, stdout_line) {
                    terminate_shell_command(&mut child);
                    let _ = child.wait();
                    let _ = stdout_handle.join();
                    let _ = stderr_handle.join();
                    return Err(error);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                stdout_done = true;
            }
        }
    }

    stdout_handle
        .join()
        .map_err(|_| io::Error::other("command stdout reader thread panicked"))?;
    let stderr = stderr_handle
        .join()
        .map_err(|_| io::Error::other("command stderr reader thread panicked"))??;
    let status = child_status.ok_or_else(|| io::Error::other("command exited without status"))?;
    Ok(ProcessExit {
        success: status.success(),
        status_label: status.to_string(),
        stderr,
    })
}

fn handle_stdout_event(
    event: StdoutReaderEvent,
    stdout_done: &mut bool,
    stdout_line: &mut dyn FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<()> {
    match event {
        StdoutReaderEvent::Line(line) => stdout_line(&line),
        StdoutReaderEvent::LimitExceeded(limit) => Err(io::Error::other(format!(
            "command stdout exceeded {limit} bytes"
        ))),
        StdoutReaderEvent::Finished(result) => {
            *stdout_done = true;
            result
        }
    }
}

fn read_stdout_lines<R: Read>(
    mut reader: R,
    byte_limit: usize,
    tx: mpsc::Sender<StdoutReaderEvent>,
) -> io::Result<()> {
    let mut buffer = [0_u8; 8192];
    let mut current_line = Vec::new();
    let mut total = 0_usize;

    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            if !current_line.is_empty() {
                let _ = tx.send(StdoutReaderEvent::Line(current_line));
            }
            return Ok(());
        }

        total = total.saturating_add(bytes_read);
        if total > byte_limit {
            let _ = tx.send(StdoutReaderEvent::LimitExceeded(byte_limit));
            return Ok(());
        }

        for byte in &buffer[..bytes_read] {
            current_line.push(*byte);
            if *byte == b'\n' {
                let line = std::mem::take(&mut current_line);
                if tx.send(StdoutReaderEvent::Line(line)).is_err() {
                    return Ok(());
                }
            }
        }
    }
}

fn read_bounded_stream<R: Read>(
    mut reader: R,
    byte_limit: usize,
    overflow: Arc<AtomicBool>,
    stream: &str,
) -> io::Result<Vec<u8>> {
    let mut buffer = [0_u8; 8192];
    let mut output = Vec::new();
    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            return Ok(output);
        }
        let available = byte_limit.saturating_sub(output.len());
        if bytes_read > available {
            output.extend_from_slice(&buffer[..available]);
            overflow.store(true, Ordering::Relaxed);
            return Err(io::Error::other(format!(
                "command {stream} exceeded {byte_limit} bytes"
            )));
        }
        output.extend_from_slice(&buffer[..bytes_read]);
    }
}

#[cfg(unix)]
fn spawn_shell_command(cwd: &Path, command: &str) -> io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;

    Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
}

#[cfg(windows)]
fn spawn_shell_command(cwd: &Path, command: &str) -> io::Result<std::process::Child> {
    Command::new("cmd")
        .arg("/C")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}

#[cfg(unix)]
fn terminate_shell_command(child: &mut std::process::Child) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    let Ok(pid) = i32::try_from(child.id()) else {
        let _ = child.kill();
        return;
    };

    let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
}

#[cfg(windows)]
fn terminate_shell_command(child: &mut std::process::Child) {
    let pid = child.id().to_string();
    let status = Command::new("taskkill")
        .arg("/PID")
        .arg(&pid)
        .arg("/T")
        .arg("/F")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    if !matches!(status, Ok(exit_status) if exit_status.success()) {
        let _ = child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdout_reader_reports_byte_limit() {
        let (tx, rx) = mpsc::channel();
        read_stdout_lines(&b"abcdef"[..], 3, tx).expect("reader should report limit cleanly");

        match rx.recv().expect("reader should emit an event") {
            StdoutReaderEvent::LimitExceeded(limit) => assert_eq!(limit, 3),
            _ => panic!("expected stdout limit event"),
        }
    }

    #[test]
    fn stdout_reader_splits_lines_without_dropping_tail() {
        let (tx, rx) = mpsc::channel();
        read_stdout_lines(&b"one\ntwo"[..], 16, tx).expect("reader should finish cleanly");

        let mut lines = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let StdoutReaderEvent::Line(line) = event {
                lines.push(line);
            }
        }
        assert_eq!(lines, vec![b"one\n".to_vec(), b"two".to_vec()]);
    }
}
