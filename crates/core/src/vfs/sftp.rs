//! Read-only SFTP v3 over OpenSSH. A mount owns one cancellable connection;
//! failed operations discard it and the next operation reconnects. Reread
//! explicitly drops cached metadata and the idle connection.
mod protocol;

use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{
    Backend, BackendKind, Cancellation, Mount, VfsPath, remote_cache::MetadataCache, ssh::Endpoint,
};
use crate::FileEntry;
use protocol::Session;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Connection {
    pub endpoint: Endpoint,
    pub directory: PathBuf,
}

#[derive(Clone, Copy)]
pub(super) enum Transport {
    Sftp,
    Shell,
}
impl Transport {
    fn kind(self) -> BackendKind {
        match self {
            Self::Sftp => BackendKind::Sftp,
            Self::Shell => BackendKind::Shell,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Sftp => "SFTP",
            Self::Shell => "Shell",
        }
    }
    fn scheme(self) -> &'static str {
        match self {
            Self::Sftp => "sftp",
            Self::Shell => "sh",
        }
    }
    fn command(self, endpoint: &Endpoint, program: &Path) -> std::process::Command {
        match self {
            Self::Sftp => {
                let mut command = endpoint.command(program, true);
                command.arg("sftp");
                command
            }
            Self::Shell => super::shell_link::command(endpoint, program),
        }
    }
}

struct SftpBackend {
    transport: Transport,
    endpoint: Endpoint,
    program: PathBuf,
    root: PathBuf,
    session: Mutex<Option<Session>>,
    busy: AtomicBool,
    closed: AtomicBool,
    revision: AtomicU64,
    cache: MetadataCache,
}

impl SftpBackend {
    fn connect(&self, cancel: &Cancellation<'_>) -> io::Result<Session> {
        let command = self.transport.command(&self.endpoint, &self.program);
        Session::open(command, cancel)
    }
    fn with_session<T>(
        &self,
        cancel: &Cancellation<'_>,
        action: impl FnOnce(&mut Session) -> io::Result<T>,
    ) -> io::Result<T> {
        struct Busy<'a>(&'a AtomicBool);
        impl Drop for Busy<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _busy = loop {
            cancel.check()?;
            if self.closed.load(Ordering::Acquire) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    crate::JOB_CANCELED_MESSAGE,
                ));
            }
            if self
                .busy
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                break Busy(&self.busy);
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let (revision, saved) = {
            let mut saved = self
                .session
                .lock()
                .map_err(|_| io::Error::other("SFTP connection lock poisoned"))?;
            (self.revision.load(Ordering::Acquire), saved.take())
        };
        let mut session = match saved {
            Some(session) => session,
            None => self.connect(cancel)?,
        };
        let result = cancel.finish(action(&mut session));
        if result.is_ok() {
            let mut saved = self
                .session
                .lock()
                .map_err(|_| io::Error::other("SFTP connection lock poisoned"))?;
            if !self.closed.load(Ordering::Acquire)
                && revision == self.revision.load(Ordering::Acquire)
            {
                *saved = Some(session);
            }
        } else {
            self.cache.clear();
        }
        result
    }
}

impl Backend for SftpBackend {
    fn kind(&self) -> BackendKind {
        self.transport.kind()
    }
    fn read_directory(&self, path: &Path, cancel: &Cancellation<'_>) -> io::Result<Vec<FileEntry>> {
        cancel.check()?;
        if let Some(entries) = self.cache.directory(path) {
            return Ok(entries.to_vec());
        }
        let generation = self.cache.generation();
        let entries =
            self.with_session(cancel, |session| session.list(&self.root, path, cancel))?;
        self.cache.insert(path, &entries, generation);
        Ok(entries)
    }
    fn metadata(&self, path: &Path, cancel: &Cancellation<'_>) -> io::Result<FileEntry> {
        cancel.check()?;
        if let Some(entry) = self.cache.metadata(path) {
            return Ok(entry);
        }
        self.with_session(cancel, |session| {
            session.stat(&self.root.join(path), cancel)?.entry(path)
        })
    }
    fn with_file(
        &self,
        path: &Path,
        cancel: &Cancellation<'_>,
        consume: &mut dyn FnMut(&FileEntry, &mut dyn Read) -> io::Result<()>,
    ) -> io::Result<()> {
        self.with_session(cancel, |session| {
            session.with_file(&self.root, path, cancel, consume)
        })
    }
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.invalidate();
    }
    fn invalidate(&self) {
        let session = self.session.lock().ok().and_then(|mut saved| {
            self.revision.fetch_add(1, Ordering::AcqRel);
            saved.take()
        });
        self.cache.clear();
        drop(session);
    }
}

/// Opens and lists the requested remote directory on a worker. OpenSSH uses
/// existing host keys, configuration, and agent/key authentication in batch
/// mode. An unknown/changed host key or unavailable authentication is an error.
pub fn open(
    connection: &Connection,
    return_to: VfsPath,
    cancel: Option<&AtomicBool>,
) -> io::Result<Arc<Mount>> {
    open_transport(connection, return_to, cancel, Transport::Sftp)
}

pub(super) fn open_transport(
    connection: &Connection,
    return_to: VfsPath,
    cancel: Option<&AtomicBool>,
    transport: Transport,
) -> io::Result<Arc<Mount>> {
    let program = rc_shell::find_program_on_path(OsStr::new("ssh"), return_to.local_directory())
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "Install OpenSSH to use SSH links")
        })?;
    open_with_program(connection, return_to, cancel, program, transport)
}

pub(super) fn open_with_program(
    connection: &Connection,
    return_to: VfsPath,
    cancel: Option<&AtomicBool>,
    program: PathBuf,
    transport: Transport,
) -> io::Result<Arc<Mount>> {
    let cancellation = Cancellation {
        requested: cancel,
        closed: None,
    };
    let command = transport.command(&connection.endpoint, &program);
    let mut session = Session::open(command, &cancellation)?;
    let directory = if connection.directory.as_os_str().is_empty() {
        Path::new(".")
    } else {
        &connection.directory
    };
    let root = session.realpath(directory, &cancellation)?;
    if session.stat(&root, &cancellation)?.file_type()? != 0o040000 {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "Remote location is not a directory",
        ));
    }
    let entries = session.list(&root, Path::new(""), &cancellation)?;
    let display_root = PathBuf::from(format!(
        "{}://{}/",
        transport.scheme(),
        connection.endpoint.authority()
    ))
    .join(root.strip_prefix("/").map_err(io::Error::other)?);
    let backend = SftpBackend {
        transport,
        endpoint: connection.endpoint.clone(),
        program,
        root,
        session: Mutex::new(Some(session)),
        busy: AtomicBool::new(false),
        closed: AtomicBool::new(false),
        revision: AtomicU64::new(0),
        cache: MetadataCache::default(),
    };
    backend.cache.insert(Path::new(""), &entries, 0);
    cancellation.check()?;
    Mount::new(
        format!("{} {}", transport.label(), connection.endpoint.authority()),
        display_root,
        return_to,
        Arc::new(backend),
    )
}

#[cfg(test)]
mod tests;
