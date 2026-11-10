use std::io::{self, Read};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
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
    #[cfg(unix)]
    if let Err(error) = set_nonblocking(&stdout).and_then(|()| set_nonblocking(&stderr)) {
        super::terminate_shell_command(&mut child);
        let _ = child.wait();
        return Err(error);
    }
    let stopped = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::sync_channel(16);
    let stdout_tx = tx.clone();
    let stdout_stop = Arc::clone(&stopped);
    let stdout_thread =
        thread::spawn(move || read_pipe(stdout, OutputStream::Stdout, stdout_tx, stdout_stop));
    let stderr_stop = Arc::clone(&stopped);
    let stderr_thread =
        thread::spawn(move || read_pipe(stderr, OutputStream::Stderr, tx, stderr_stop));
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
    // Stop idle readers even if a detached descendant still holds a write end.
    stopped.store(true, Ordering::Relaxed);
    // Unblock producers waiting on the bounded channel before joining them.
    drop(rx);
    finish_reader(stdout_thread);
    finish_reader(stderr_thread);
    result
}

#[cfg(unix)]
fn set_nonblocking(pipe: &impl std::os::fd::AsRawFd) -> io::Result<()> {
    use nix::fcntl::{FcntlArg, OFlag, fcntl};

    let fd = pipe.as_raw_fd();
    let flags = OFlag::from_bits_truncate(fcntl(fd, FcntlArg::F_GETFL)?);
    fcntl(fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
    Ok(())
}

fn finish_reader(reader: thread::JoinHandle<()>) {
    // Other platforms use blocking pipe reads. Detach an unfinished reader so
    // an inherited writer cannot hold up cancellation or application shutdown.
    #[cfg(not(unix))]
    if !reader.is_finished() {
        return;
    }
    let _ = reader.join();
}

fn read_pipe(
    mut pipe: impl Read,
    stream: OutputStream,
    tx: SyncSender<Event>,
    stopped: Arc<AtomicBool>,
) {
    let result = (|| {
        let mut buffer = [0; 8192];
        loop {
            if stopped.load(Ordering::Relaxed) {
                return Ok(());
            }
            let count = match pipe.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(20));
                    continue;
                }
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
    fn detached_pipe_holder() {
        let Ok(stage) = std::env::var("RC_CAPTURE_TEST_DETACHED") else {
            return;
        };
        if stage == "spawn" {
            detached_pipe_command("hold")
                .spawn()
                .unwrap()
                .wait()
                .unwrap();
        } else {
            use std::io::Write;
            nix::unistd::setsid().unwrap();
            println!("\ndetached-pid:{}", std::process::id());
            io::stdout().flush().unwrap();
            thread::sleep(Duration::from_secs(30));
        }
    }

    fn detached_pipe_command(stage: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "capture::tests::detached_pipe_holder",
                "--nocapture",
            ])
            .env("RC_CAPTURE_TEST_DETACHED", stage);
        command
    }

    #[test]
    fn cancellation_does_not_wait_for_detached_pipe_writers() {
        let (pid_tx, pid_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let cancel = AtomicBool::new(false);
            let mut stdout = Vec::new();
            let result = run_captured_command(
                detached_pipe_command("spawn"),
                &cancel,
                8192,
                |stream, bytes| {
                    if stream == OutputStream::Stdout {
                        stdout.extend_from_slice(bytes);
                        if let Some(pid) = String::from_utf8_lossy(&stdout)
                            .split_inclusive('\n')
                            .find_map(|line| {
                                line.strip_prefix("detached-pid:")?
                                    .strip_suffix('\n')?
                                    .parse::<i32>()
                                    .ok()
                            })
                        {
                            pid_tx.send(pid).unwrap();
                            cancel.store(true, Ordering::Relaxed);
                        }
                    }
                    Ok(())
                },
            );
            result_tx.send(result).unwrap();
        });
        let pid = pid_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let result = result_rx.recv_timeout(Duration::from_secs(2));
        // Also release blocked readers on regression, so the test cannot hang.
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGKILL,
        );
        worker.join().unwrap();
        let error = result
            .expect("capture cleanup waited for a detached writer")
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
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
