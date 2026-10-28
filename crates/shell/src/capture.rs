use std::io::{self, Read};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

enum Event {
    Bytes(OutputStream, Vec<u8>),
    Done(io::Result<()>),
}

/// Run a command with bounded streaming capture of both output pipes.
///
/// The callback receives arbitrary byte chunks, including partial UTF-8 sequences.
/// Output order within each pipe is preserved; ordering between pipes is unspecified.
/// Cancellation, an output limit, or a callback failure terminates the process group
/// on Unix (the process tree on Windows) and reaps the child. Standard input is closed.
/// Call this blocking function from a worker, never the interface event loop.
pub fn run_captured_command(
    mut command: Command,
    cancel: &AtomicBool,
    byte_limit: usize,
    mut output: impl FnMut(OutputStream, &[u8]) -> io::Result<()>,
) -> io::Result<ExitStatus> {
    if cancel.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "command canceled",
        ));
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().expect("stdout pipe requested");
    let stderr = child.stderr.take().expect("stderr pipe requested");
    let (tx, rx) = mpsc::sync_channel(16);
    let stdout_tx = tx.clone();
    let stdout_thread = thread::spawn(move || read_pipe(stdout, OutputStream::Stdout, stdout_tx));
    let stderr_thread = thread::spawn(move || read_pipe(stderr, OutputStream::Stderr, tx));
    let result = (|| {
        let mut status = None;
        let mut done = 0;
        let mut total = 0usize;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "command canceled",
                ));
            }
            if status.is_none() {
                status = child.try_wait()?;
            }
            if done == 2
                && let Some(status) = status
            {
                return Ok(status);
            }
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(Event::Bytes(stream, bytes)) => {
                    let remaining = byte_limit.saturating_sub(total);
                    let accepted = remaining.min(bytes.len());
                    if accepted > 0 {
                        output(stream, &bytes[..accepted])?;
                        total += accepted;
                    }
                    if accepted != bytes.len() {
                        return Err(io::Error::other(format!(
                            "command output exceeded {byte_limit} bytes"
                        )));
                    }
                }
                Ok(Event::Done(result)) => {
                    result?;
                    done += 1;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) if done == 2 => {
                    thread::sleep(Duration::from_millis(20))
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other("output reader stopped unexpectedly"));
                }
            }
        }
    })();
    if result.is_err() {
        super::terminate_shell_command(&mut child);
        let _ = child.wait();
    }
    // Unblock producers waiting on the bounded channel before joining them.
    drop(rx);
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();
    result
}

fn read_pipe(mut pipe: impl Read, stream: OutputStream, tx: SyncSender<Event>) {
    let result = (|| {
        let mut buffer = [0; 8192];
        loop {
            let count = match pipe.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            if count == 0 {
                return Ok(());
            }
            if tx
                .send(Event::Bytes(stream, buffer[..count].to_vec()))
                .is_err()
            {
                return Ok(());
            }
        }
    })();
    let _ = tx.send(Event::Done(result));
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    fn shell(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn captures_both_streams_and_nonzero_exit_without_newline() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let status = run_captured_command(
            shell("printf out; printf err >&2; exit 7"),
            &AtomicBool::new(false),
            1024,
            |stream, bytes| {
                match stream {
                    OutputStream::Stdout => stdout.extend_from_slice(bytes),
                    OutputStream::Stderr => stderr.extend_from_slice(bytes),
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(stdout, b"out");
        assert_eq!(stderr, b"err");
        assert_eq!(status.code(), Some(7));
    }

    #[test]
    fn output_limit_preserves_prefix_and_stops_infinite_producer() {
        let mut bytes = Vec::new();
        let error = run_captured_command(
            shell("while :; do printf abc; done"),
            &AtomicBool::new(false),
            101,
            |_, chunk| {
                bytes.extend_from_slice(chunk);
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(bytes.len(), 101);
        assert!(error.to_string().contains("exceeded"));
    }

    #[test]
    fn cancellation_stops_descendants_that_hold_pipes_after_shell_exit() {
        let cancel = AtomicBool::new(false);
        let started = std::time::Instant::now();
        let error =
            run_captured_command(shell("(sleep 30) & printf ready"), &cancel, 1024, |_, _| {
                cancel.store(true, Ordering::Relaxed);
                Ok(())
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn callback_failure_stops_a_blocked_writer() {
        let error = run_captured_command(
            shell("while :; do printf abc; done"),
            &AtomicBool::new(false),
            1024,
            |_, _| Err(io::Error::other("consumer closed")),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "consumer closed");
    }
}
