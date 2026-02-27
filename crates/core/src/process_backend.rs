use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

pub fn run_shell_command(
    cwd: &Path,
    command: &str,
    cancel_flag: Option<&AtomicBool>,
    canceled_message: &str,
) -> io::Result<std::process::Output> {
    let mut child = spawn_shell_command(cwd, command)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("failed to capture command stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("failed to capture command stderr"))?;

    let stdout_handle = thread::spawn(move || {
        let mut reader = stdout;
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer)?;
        Ok(buffer)
    });
    let stderr_handle = thread::spawn(move || {
        let mut reader = stderr;
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer)?;
        Ok(buffer)
    });

    loop {
        if cancel_flag.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            terminate_shell_command(&mut child);
            let _ = child.wait();
            let _ = stdout_handle.join();
            let _ = stderr_handle.join();
            return Err(io::Error::new(io::ErrorKind::Interrupted, canceled_message));
        }

        if let Some(status) = child.try_wait()? {
            let stdout = join_command_output_reader(stdout_handle, "stdout")?;
            let stderr = join_command_output_reader(stderr_handle, "stderr")?;
            return Ok(std::process::Output {
                status,
                stdout,
                stderr,
            });
        }

        thread::sleep(Duration::from_millis(20));
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

fn join_command_output_reader(
    handle: thread::JoinHandle<io::Result<Vec<u8>>>,
    stream: &str,
) -> io::Result<Vec<u8>> {
    handle
        .join()
        .map_err(|_| io::Error::other(format!("command {stream} reader thread panicked")))?
}
