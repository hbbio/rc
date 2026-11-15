//! Read-only FTP/FTPS using an owned curl process per operation. Credentials
//! are read from a private .netrc and passed through stdin, never argv/logs.
mod listing;

use super::{
    Backend, BackendKind, Cancellation, Mount, VfsPath, process::Duplex,
    remote_cache::MetadataCache, ssh::Endpoint,
};
use crate::{FileEntry, FileEntryKind};
use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, atomic::AtomicBool};
use std::time::{Duration, Instant};

const LIST_LIMIT: usize = 16 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Security {
    ExplicitTls,
    ImplicitTls,
    Anonymous,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Connection {
    endpoint: Endpoint,
    security: Security,
    directory: PathBuf,
}

impl Connection {
    pub fn parse(address: &str, directory: PathBuf) -> io::Result<Self> {
        let (security, address) = if let Some(address) = address.strip_prefix("ftps://") {
            (Security::ImplicitTls, address)
        } else if let Some(address) = address.strip_prefix("ftp://") {
            (Security::Anonymous, address)
        } else if let Some(address) = address.strip_prefix("ftpes://") {
            (Security::ExplicitTls, address)
        } else {
            (Security::ExplicitTls, address)
        };
        let endpoint = Endpoint::parse(address)?;
        if security == Security::Anonymous
            && endpoint.user().is_some_and(|user| user != "anonymous")
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Plain FTP supports anonymous access; use TLS for credentials",
            ));
        }
        validate_path(&directory)?;
        let directory = if directory.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            directory
        };
        Ok(Self {
            endpoint,
            security,
            directory,
        })
    }
    pub fn label(&self) -> String {
        format!(
            "{} {}",
            if self.security == Security::Anonymous {
                "FTP"
            } else {
                "FTPS"
            },
            self.endpoint.authority()
        )
    }
    fn url(&self, relative: &Path, directory: bool) -> io::Result<String> {
        let path = self.directory.join(relative);
        validate_path(&path)?;
        let scheme = if self.security == Security::ImplicitTls {
            "ftps"
        } else {
            "ftp"
        };
        let mut result = format!("{scheme}://{}/", self.endpoint.authority());
        // The first URL slash denotes the login directory. Encode an absolute
        // path's leading slash so curl sends an absolute CWD to the server.
        let bytes = path.as_os_str().as_bytes();
        for (index, byte) in bytes.iter().copied().enumerate() {
            if byte.is_ascii_alphanumeric()
                || b"-._~".contains(&byte)
                || (byte == b'/' && index != 0)
            {
                result.push(char::from(byte));
            } else {
                use std::fmt::Write;
                write!(&mut result, "%{byte:02X}").expect("String write");
            }
        }
        if directory && !result.ends_with('/') {
            result.push('/');
        }
        Ok(result)
    }
}

fn validate_path(path: &Path) -> io::Result<()> {
    if path.as_os_str().len() > 65_536
        || path
            .as_os_str()
            .as_bytes()
            .iter()
            .any(|byte| [0, b'\r', b'\n'].contains(byte))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "FTP paths must be at most 64 KiB and cannot contain NUL, CR, or LF",
        ));
    }
    Ok(())
}

struct Credentials(Vec<u8>);
impl Drop for Credentials {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}
fn credentials(path: Option<&Path>) -> io::Result<Credentials> {
    let Some(path) = path else {
        return Ok(Credentials(Vec::new()));
    };
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Credentials(Vec::new()));
        }
        Err(_) => {
            return Err(io::Error::other(
                "Cannot open ~/.netrc; use a regular private file",
            ));
        }
    };
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != nix::unistd::Uid::effective().as_raw()
        || metadata.mode() & 0o077 != 0
        || metadata.len() > 65_536
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "~/.netrc must be owned by you, have mode 0600 or stricter, and fit within 64 KiB",
        ));
    }
    let mut bytes = Credentials(Vec::new());
    file.take(65_537).read_to_end(&mut bytes.0)?;
    if bytes.0.len() > 65_536 {
        return Err(io::Error::other("~/.netrc exceeds 64 KiB"));
    }
    Ok(bytes)
}

struct FtpBackend {
    connection: Connection,
    program: PathBuf,
    netrc: Option<PathBuf>,
    cache: MetadataCache,
}
impl FtpBackend {
    fn start(&self, path: &Path, directory: bool, cancel: &Cancellation<'_>) -> io::Result<Duplex> {
        let credentials = if self.connection.security == Security::Anonymous {
            Credentials(Vec::new())
        } else {
            credentials(self.netrc.as_deref())?
        };
        if credentials.0.is_empty()
            && self.connection.endpoint.user().is_some()
            && self.connection.security != Security::Anonymous
        {
            return Err(io::Error::other(
                "Add this FTP login to a private ~/.netrc file",
            ));
        }
        let mut command = Command::new(&self.program);
        command.args([
            "--disable",
            "--silent",
            "--show-error",
            "--globoff",
            "--path-as-is",
            "--proto",
            "=ftp,ftps",
            "--connect-timeout",
            "15",
            "--max-time",
            "300",
            "--speed-limit",
            "1",
            "--speed-time",
            "30",
            "--ftp-method",
            "singlecwd",
            "--ftp-pasv",
            "--ftp-skip-pasv-ip",
            "--proxy",
            "",
        ]);
        // Disabling curlrc, proxy environment, URL passwords, and diagnostics
        // prevents configuration from silently relaxing transport guarantees.
        if self.connection.security != Security::Anonymous {
            command.args(["--ssl-reqd", "--tlsv1.2"]);
        }
        if credentials.0.is_empty() {
            command.args(["--user", "anonymous:anonymous@"]);
        } else {
            command.args(["--netrc-file", "/dev/stdin"]);
        }
        if directory {
            command.args(["--request", "MLSD"]);
        }
        command
            .arg("--url")
            .arg(self.connection.url(path, directory)?);
        let mut transport = Duplex::spawn(command, 128 * 1024, cancel)?;
        transport.hide_diagnostics();
        transport.write_all(
            &credentials.0,
            Instant::now() + Duration::from_secs(30),
            cancel,
        )?;
        transport.close_input();
        Ok(transport)
    }
    fn listing(&self, path: &Path, cancel: &Cancellation<'_>) -> io::Result<Vec<FileEntry>> {
        let mut transport = self.start(path, true, cancel)?;
        let deadline = Instant::now() + TIMEOUT;
        let mut bytes = Vec::new();
        let mut buffer = [0; 8192];
        loop {
            let count = transport.read_chunk(&mut buffer, deadline, cancel)?;
            if count == 0 {
                break;
            }
            if bytes.len().saturating_add(count) > LIST_LIMIT {
                return Err(io::Error::other("FTP listing exceeds 16 MiB"));
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
        listing::parse(path, &bytes, cancel)
    }
    fn fresh_metadata(&self, path: &Path, cancel: &Cancellation<'_>) -> io::Result<FileEntry> {
        if path.as_os_str().is_empty() {
            self.listing(path, cancel)?;
            return Ok(FileEntry {
                name: String::new(),
                path: PathBuf::new(),
                kind: FileEntryKind::Directory,
                size: 0,
                modified: None,
                metadata: Default::default(),
            });
        }
        self.listing(path.parent().unwrap_or(Path::new("")), cancel)?
            .into_iter()
            .find(|entry| entry.path == path)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "FTP entry no longer exists"))
    }
}

impl Backend for FtpBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Ftp
    }
    fn read_directory(&self, path: &Path, cancel: &Cancellation<'_>) -> io::Result<Vec<FileEntry>> {
        cancel.check()?;
        if let Some(entries) = self.cache.directory(path) {
            return Ok(entries.to_vec());
        }
        let generation = self.cache.generation();
        let entries = self.listing(path, cancel)?;
        self.cache.insert(path, &entries, generation);
        Ok(entries)
    }
    fn metadata(&self, path: &Path, cancel: &Cancellation<'_>) -> io::Result<FileEntry> {
        cancel.check()?;
        if let Some(entry) = self.cache.metadata(path) {
            return Ok(entry);
        }
        self.fresh_metadata(path, cancel)
    }
    fn with_file(
        &self,
        path: &Path,
        cancel: &Cancellation<'_>,
        consume: &mut dyn FnMut(&FileEntry, &mut dyn Read) -> io::Result<()>,
    ) -> io::Result<()> {
        let before = self.fresh_metadata(path, cancel)?;
        if before.kind != FileEntryKind::File
            || before.metadata.symlink_target.is_some()
            || before
                .metadata
                .mode
                .is_some_and(|mode| mode & 0o170000 != 0o100000)
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "FTP links and special files cannot be read",
            ));
        }
        let transport = self.start(path, false, cancel)?;
        let mut reader = FileReader {
            transport,
            cancel,
            deadline: Instant::now() + TIMEOUT,
            bytes: 0,
            size: before.size,
        };
        let result = consume(&before, &mut reader);
        cancel.finish(result)?;
        if reader.bytes == before.size {
            let mut extra = [0];
            if reader.read(&mut extra)? != 0 {
                return Err(io::Error::other("FTP file grew during the read"));
            }
        }
        drop(reader);
        let after = self.fresh_metadata(path, cancel)?;
        if before.kind != after.kind
            || before.size != after.size
            || before.modified != after.modified
            || before.metadata != after.metadata
        {
            return Err(io::Error::other(
                "FTP file changed while being read; refresh and retry",
            ));
        }
        Ok(())
    }
    fn close(&self) {
        self.cache.clear();
    }
    fn invalidate(&self) {
        self.cache.clear();
    }
}

struct FileReader<'a> {
    transport: Duplex,
    cancel: &'a Cancellation<'a>,
    deadline: Instant,
    bytes: u64,
    size: u64,
}
impl Read for FileReader<'_> {
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        self.cancel.check_read()?;
        if target.is_empty() {
            return Ok(0);
        }
        let count = self
            .transport
            .read_chunk(target, self.deadline, self.cancel)
            .map_err(|error| {
                if error.kind() == io::ErrorKind::Interrupted {
                    io::Error::other(error.to_string())
                } else {
                    error
                }
            })?;
        self.bytes = self.bytes.saturating_add(count as u64);
        if count == 0 && self.bytes != self.size {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "FTP file was truncated",
            ));
        }
        if self.bytes > self.size {
            return Err(io::Error::other("FTP file grew during the read"));
        }
        Ok(count)
    }
}

pub fn open(
    connection: &Connection,
    return_to: VfsPath,
    cancel: Option<&AtomicBool>,
) -> io::Result<Arc<Mount>> {
    let program = rc_shell::find_program_on_path(OsStr::new("curl"), return_to.local_directory())
        .ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "Install curl with FTP/FTPS support to use FTP links",
        )
    })?;
    let netrc = crate::current_user_home_directory().map(|home| home.join(".netrc"));
    open_with_program(connection, return_to, cancel, program, netrc)
}
fn open_with_program(
    connection: &Connection,
    return_to: VfsPath,
    cancel: Option<&AtomicBool>,
    program: PathBuf,
    netrc: Option<PathBuf>,
) -> io::Result<Arc<Mount>> {
    let cancellation = Cancellation {
        requested: cancel,
        closed: None,
    };
    let backend = FtpBackend {
        connection: connection.clone(),
        program,
        netrc,
        cache: MetadataCache::default(),
    };
    let entries = backend.listing(Path::new(""), &cancellation)?;
    backend.cache.insert(Path::new(""), &entries, 0);
    let scheme = match connection.security {
        Security::Anonymous => "ftp",
        Security::ExplicitTls => "ftpes",
        Security::ImplicitTls => "ftps",
    };
    let root = PathBuf::from(format!("{scheme}://{}/", connection.endpoint.authority())).join(
        connection
            .directory
            .strip_prefix("/")
            .unwrap_or(&connection.directory),
    );
    cancellation.check()?;
    Mount::new(connection.label(), root, return_to, Arc::new(backend))
}

#[cfg(test)]
mod tests;
