//! Worker-only subprocess capture with nonblocking pipes. No reader thread can
//! be stranded by a descendant that keeps an inherited pipe open.
use super::Cancellation;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

const TICK: Duration = Duration::from_millis(5);
const STDERR_LIMIT: usize = 64 * 1024;

pub(super) struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

struct OwnedChild {
    child: Child,
    reaped: bool,
}
impl OwnedChild {
    fn status(&mut self) -> io::Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        if status.is_some() && !self.reaped {
            // The leader is finished; stop inherited-pipe holders immediately.
            self.kill_group();
            self.reaped = true;
        }
        Ok(status)
    }
    fn kill_group(&self) {
        if let Ok(pid) = i32::try_from(self.child.id()) {
            let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !self.reaped {
            self.kill_group();
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
    let fd = pipe.as_raw_fd();
    let flags = OFlag::from_bits_truncate(fcntl(fd, FcntlArg::F_GETFL)?);
    fcntl(fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
    Ok(())
}

fn drain(pipe: &mut impl Read, output: &mut Vec<u8>, limit: usize) -> io::Result<bool> {
    let mut buffer = [0_u8; 8192];
    for _ in 0..8 {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(size) => {
                if output.len().saturating_add(size) > limit {
                    return Err(io::Error::other("VFS helper output exceeded its limit"));
                }
                output.extend_from_slice(&buffer[..size]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

pub(super) fn run(
    mut command: Command,
    input: &[u8],
    output_limit: usize,
    timeout: Duration,
    cancel: &Cancellation<'_>,
) -> io::Result<Output> {
    cancel.check()?;
    if input.len() > 64 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "VFS helper input exceeds 64 KiB",
        ));
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = OwnedChild {
        child: command.spawn()?,
        reaped: false,
    };
    let mut stdin = child.child.stdin.take();
    let mut stdout = child.child.stdout.take().expect("requested stdout");
    let mut stderr = child.child.stderr.take().expect("requested stderr");
    nonblocking(stdin.as_ref().expect("requested stdin"))?;
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let started = Instant::now();
    let mut written = 0;
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    let mut status = None;
    let mut exited_at = None;
    loop {
        cancel.check()?;
        if started.elapsed() >= timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "VFS helper timed out",
            ));
        }
        if written == input.len() {
            stdin = None;
        }
        if let Some(pipe) = &mut stdin {
            match pipe.write(&input[written..]) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "VFS helper input closed",
                    ));
                }
                Ok(size) => written += size,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(error),
            }
        }
        let stdout_done = drain(&mut stdout, &mut stdout_bytes, output_limit)?;
        let stderr_done = drain(&mut stderr, &mut stderr_bytes, STDERR_LIMIT)?;
        if status.is_none() {
            status = child.status()?;
            if status.is_some() {
                exited_at = Some(Instant::now());
            }
        }
        if let Some(status) = status {
            if stdout_done && stderr_done {
                cancel.check()?;
                return Ok(Output {
                    status,
                    stdout: stdout_bytes,
                    stderr: stderr_bytes,
                });
            }
            if exited_at.is_some_and(|time| time.elapsed() > Duration::from_millis(250)) {
                return Err(io::Error::other(
                    "VFS helper left an inherited output pipe open",
                ));
            }
        }
        std::thread::sleep(TICK);
    }
}

pub(super) fn ensure_success(output: Output) -> io::Result<Vec<u8>> {
    if output.status.success() {
        return Ok(output.stdout);
    }
    let detail = rc_shell::sanitize_display_lossy(&output.stderr);
    let detail = detail.trim();
    Err(io::Error::other(if detail.is_empty() {
        format!("VFS helper exited with {}", output.status)
    } else {
        format!("VFS helper failed: {detail}")
    }))
}
