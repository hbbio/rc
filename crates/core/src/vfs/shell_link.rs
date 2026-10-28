//! SSH shell links use a fixed, read-only Python helper instead of requiring
//! the remote sshd's SFTP subsystem. Paths only cross the binary transport.
use super::{Mount, VfsPath, sftp, ssh::Endpoint};
use std::io;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, atomic::AtomicBool};

const SCRIPT: &str = include_str!("shell_link.py");

pub(super) fn command(endpoint: &Endpoint, program: &Path) -> Command {
    let mut command = endpoint.command(program, false);
    // OpenSSH submits one command string to the user's remote shell. Only this
    // fixed source is quoted here; filenames never appear in the command.
    let encoded: String = SCRIPT.bytes().map(|byte| format!("{byte:02x}")).collect();
    command.arg(format!(
        "python3 -I -u -c 'exec(bytes.fromhex(\"{encoded}\"))'"
    ));
    command
}

pub fn open(
    connection: &sftp::Connection,
    return_to: VfsPath,
    cancel: Option<&AtomicBool>,
) -> io::Result<Arc<Mount>> {
    sftp::open_transport(connection, return_to, cancel, sftp::Transport::Shell)
}

#[cfg(test)]
mod tests;
