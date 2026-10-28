//! Read-only MC extfs helpers. Installed executables are trusted local programs;
//! archive member names and helper output are untrusted data.
//!
//! Discovery checks `RC_EXTFS_PATH`, user data directories, then common MC
//! installation directories. The public `open` factory runs on a worker and
//! supports `list`/`copyout`, including standalone `+` helpers and containers
//! inside built-in archives. Listings have 16 MiB/100,000-entry limits and a
//! 64 MiB metadata budget; paths have at most 256 components. Each helper has a
//! five-minute deadline. Private temporary files are limited to 512 MiB each
//! and 1 GiB total. Symlinks, hardlink references, and special files are listed
//! but cannot be read. A zero listed size may mean unknown (as in MC's RPM
//! helper); reads then reserve the full per-file limit and report the real size.
//! See <https://github.com/MidnightCommander/mc/tree/master/src/vfs/extfs/helpers>.
mod listing;

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{Backend, BackendKind, Cancellation, Mount, VfsPath, process};
use crate::{FileEntry, archive::SourceIdentity};
use listing::Listing;

const MEMBER_LIMIT: u64 = 512 * 1024 * 1024;
const STORAGE_LIMIT: u64 = 1024 * 1024 * 1024;
const LIST_LIMIT: usize = 16 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(300);
static STORAGE_BYTES: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct Helper {
    path: PathBuf,
    name: String,
    needs_archive: bool,
}

impl Helper {
    pub fn from_path(path: &Path) -> io::Result<Self> {
        let name = path.file_name().and_then(OsStr::to_str).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid extfs helper name")
        })?;
        let needs_archive = !name.ends_with('+');
        let name = name.strip_suffix('+').unwrap_or(name);
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid extfs helper prefix",
            ));
        }
        let absolute = fs::canonicalize(path)?;
        let metadata = fs::metadata(&absolute)?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "extfs helper is not executable",
            ));
        }
        Ok(Self {
            path: absolute,
            name: name.into(),
            needs_archive,
        })
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn needs_archive(&self) -> bool {
        self.needs_archive
    }

    fn command(
        &self,
        operation: &str,
        source: Option<&PreparedSource>,
        limit: u64,
    ) -> io::Result<Command> {
        let mut command = Command::new(&self.path);
        command.arg(operation).env("LC_ALL", "C").env("TZ", "UTC");
        rc_platform::limit_child_file_size(&mut command, limit)?;
        if let Some(source) = source {
            command.arg(&source.path);
        }
        Ok(command)
    }
}

/// Earlier directories win. Invalid files and missing directories are ignored.
pub fn discover_in(directories: impl IntoIterator<Item = PathBuf>) -> Vec<Helper> {
    let mut helpers = BTreeMap::new();
    for directory in directories {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        let mut paths: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        paths.sort();
        for path in paths {
            if let Ok(helper) = Helper::from_path(&path) {
                helpers.entry(helper.name.clone()).or_insert(helper);
            }
        }
    }
    helpers.into_values().collect()
}

pub fn discover() -> Vec<Helper> {
    let mut directories = Vec::new();
    if let Some(value) = std::env::var_os("RC_EXTFS_PATH") {
        directories.extend(std::env::split_paths(&value).filter(|path| path.is_absolute()));
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    if let Some(data) = data {
        directories.push(data.join("rust-commander/extfs.d"));
        directories.push(data.join("mc/extfs.d"));
    }
    directories.extend(
        [
            "/usr/local/libexec/mc/extfs.d",
            "/usr/local/lib/mc/extfs.d",
            "/usr/libexec/mc/extfs.d",
            "/usr/lib/mc/extfs.d",
            "/usr/share/mc/extfs.d",
            "/opt/homebrew/libexec/mc/extfs.d",
            "/opt/homebrew/lib/mc/extfs.d",
            "/opt/homebrew/share/mc/extfs.d",
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    discover_in(directories)
}

struct Reservation(u64);
impl Reservation {
    fn acquire(bytes: u64) -> io::Result<Self> {
        if bytes > MEMBER_LIMIT + 1 {
            return Err(io::Error::other("extfs member exceeds 512 MiB"));
        }
        let mut used = STORAGE_BYTES.load(Ordering::Relaxed);
        loop {
            let total = used
                .checked_add(bytes)
                .filter(|value| *value <= STORAGE_LIMIT)
                .ok_or_else(|| io::Error::other("extfs temporary storage exceeds 1 GiB"))?;
            match STORAGE_BYTES.compare_exchange_weak(
                used,
                total,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(Self(bytes)),
                Err(current) => used = current,
            }
        }
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        STORAGE_BYTES.fetch_sub(self.0, Ordering::Relaxed);
    }
}

struct PrivateFile {
    path: PathBuf,
    // Drop the directory before releasing its reservation.
    _directory: tempfile::TempDir,
    _reservation: Reservation,
}
impl PrivateFile {
    fn new(bytes: u64) -> io::Result<(Self, File)> {
        let reservation = Reservation::acquire(bytes)?;
        let directory = tempfile::Builder::new().prefix("rc-extfs-").tempdir()?;
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))?;
        let path = directory.path().join("data");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        Ok((
            Self {
                path,
                _directory: directory,
                _reservation: reservation,
            },
            file,
        ))
    }
}

#[derive(Clone, Eq, PartialEq)]
struct Stamp {
    path: PathBuf,
    identity: SourceIdentity,
}
impl Stamp {
    fn new(source: &VfsPath) -> io::Result<Option<Self>> {
        let path = match source {
            VfsPath::Local(path) => path,
            VfsPath::Archive { location, .. } => location.local_archive(),
            VfsPath::Mounted(_) => return Ok(None),
        };
        let path = fs::canonicalize(path)?;
        let metadata = fs::metadata(&path)?;
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "extfs container must be a regular file",
            ));
        }
        Ok(Some(Self {
            path,
            identity: SourceIdentity::new(&metadata),
        }))
    }
    fn verify(&self) -> io::Result<()> {
        self.identity.verify(&fs::metadata(&self.path)?)
    }
}

struct PreparedSource {
    path: PathBuf,
    stamp: Option<Stamp>,
    origin: VfsPath,
    _temporary: Option<PrivateFile>,
}
impl PreparedSource {
    fn new(source: &VfsPath, cancel: &Cancellation<'_>) -> io::Result<Self> {
        let stamp = Stamp::new(source)?;
        if let VfsPath::Local(_) = source {
            return Ok(Self {
                path: stamp.as_ref().expect("local stamp").path.clone(),
                stamp,
                origin: source.clone(),
                _temporary: None,
            });
        }
        let mut temporary = None;
        source.with_file(cancel.requested, &mut |entry, reader| {
            if entry.size > MEMBER_LIMIT {
                return Err(io::Error::other("extfs container exceeds 512 MiB"));
            }
            let (private, mut file) = PrivateFile::new(entry.size)?;
            let mut buffer = [0_u8; 64 * 1024];
            let mut total = 0_u64;
            loop {
                cancel.check_read()?;
                let size = reader.read(&mut buffer)?;
                if size == 0 {
                    break;
                }
                total = total.saturating_add(size as u64);
                if total > entry.size {
                    return Err(io::Error::other("extfs container grew while being read"));
                }
                file.write_all(&buffer[..size])?;
            }
            if total != entry.size {
                return Err(io::Error::other("extfs container was truncated"));
            }
            temporary = Some(private);
            Ok(())
        })?;
        let temporary = temporary.expect("VfsPath guarantees callback");
        let prepared = Self {
            path: temporary.path.clone(),
            stamp,
            origin: source.clone(),
            _temporary: Some(temporary),
        };
        prepared.verify()?;
        Ok(prepared)
    }
    fn verify(&self) -> io::Result<()> {
        if Stamp::new(&self.origin)? != self.stamp {
            return Err(io::Error::other("extfs source changed; refresh the pane"));
        }
        self.stamp.as_ref().map_or(Ok(()), Stamp::verify)
    }
}

struct Snapshot {
    listing: Listing,
    source: Option<PreparedSource>,
    built: Instant,
}
impl Snapshot {
    fn verify(&self) -> io::Result<()> {
        self.source.as_ref().map_or(Ok(()), PreparedSource::verify)
    }
}

struct ExtfsBackend {
    helper: Helper,
    source: Option<VfsPath>,
    cached: Mutex<Option<Arc<Snapshot>>>,
    building: AtomicBool,
    closed: AtomicBool,
}
impl ExtfsBackend {
    fn snapshot(&self, cancel: &Cancellation<'_>) -> io::Result<Arc<Snapshot>> {
        // Only the single-flight flag is held across I/O. Closing a mount can
        // always clear the cache without waiting for a helper or a reader.
        struct Building<'a>(&'a AtomicBool);
        impl Drop for Building<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _building = loop {
            cancel.check()?;
            if self.closed.load(Ordering::Acquire) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    crate::JOB_CANCELED_MESSAGE,
                ));
            }
            if self
                .building
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                break Building(&self.building);
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let lock = || {
            self.cached
                .lock()
                .map_err(|_| io::Error::other("extfs index lock poisoned"))
        };
        let stamp = self.source.as_ref().map(Stamp::new).transpose()?.flatten();
        let saved = lock()?.clone();
        if let Some(snapshot) = saved {
            let saved_stamp = snapshot
                .source
                .as_ref()
                .and_then(|source| source.stamp.as_ref());
            if saved_stamp == stamp.as_ref()
                && (stamp.is_some() || snapshot.built.elapsed() < Duration::from_secs(5))
            {
                return cancel.finish(Ok(snapshot));
            }
        }
        drop(lock()?.take());
        let result = (|| {
            let source = self
                .source
                .as_ref()
                .map(|source| PreparedSource::new(source, cancel))
                .transpose()?;
            let command = self.helper.command("list", source.as_ref(), MEMBER_LIMIT)?;
            let bytes =
                process::ensure_success(process::run(command, &[], LIST_LIMIT, TIMEOUT, cancel)?)?;
            let snapshot = Arc::new(Snapshot {
                listing: Listing::parse(&bytes, cancel)?,
                source,
                built: Instant::now(),
            });
            snapshot.verify()?;
            cancel.check()?;
            let mut cached = lock()?;
            if !self.closed.load(Ordering::Acquire) {
                *cached = Some(Arc::clone(&snapshot));
            }
            Ok(snapshot)
        })();
        cancel.finish(result)
    }
}

impl Backend for ExtfsBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Helper
    }
    fn read_directory(&self, path: &Path, cancel: &Cancellation<'_>) -> io::Result<Vec<FileEntry>> {
        let snapshot = self.snapshot(cancel)?;
        if !snapshot.listing.member(path)?.entry.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "extfs path is not a directory",
            ));
        }
        Ok(snapshot
            .listing
            .children
            .get(path)
            .into_iter()
            .flatten()
            .map(|path| snapshot.listing.members[path].entry.clone())
            .collect())
    }
    fn metadata(&self, path: &Path, cancel: &Cancellation<'_>) -> io::Result<FileEntry> {
        Ok(self.snapshot(cancel)?.listing.member(path)?.entry.clone())
    }
    fn with_file(
        &self,
        path: &Path,
        cancel: &Cancellation<'_>,
        consume: &mut dyn FnMut(&FileEntry, &mut dyn Read) -> io::Result<()>,
    ) -> io::Result<()> {
        let snapshot = self.snapshot(cancel)?;
        let member = snapshot.listing.member(path)?;
        if !member.extractable {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "extfs links and special files cannot be read",
            ));
        }
        snapshot.verify()?;
        if member.entry.size > MEMBER_LIMIT {
            return Err(io::Error::other("extfs member exceeds 512 MiB"));
        }
        // Permit one extra byte so a helper that swallows a child's SIGXFSZ
        // cannot turn an oversized/truncated output into a successful read.
        let limit = if member.entry.size == 0 {
            MEMBER_LIMIT + 1
        } else {
            member.entry.size + 1
        };
        let (private, file) = PrivateFile::new(limit)?;
        drop(file);
        let mut command = self
            .helper
            .command("copyout", snapshot.source.as_ref(), limit)?;
        command.arg(path).arg(&private.path);
        process::ensure_success(process::run(command, &[], LIST_LIMIT, TIMEOUT, cancel)?)?;
        snapshot.verify()?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
            .open(&private.path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.len() >= limit
            || (member.entry.size != 0 && metadata.len() != member.entry.size)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "extfs copyout did not produce the listed regular file size",
            ));
        }
        cancel.check()?;
        let mut entry = member.entry.clone();
        entry.size = metadata.len();
        consume(&entry, &mut file)?;
        snapshot.verify()?;
        cancel.check()
    }
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let old = self.cached.lock().ok().and_then(|mut cached| cached.take());
        drop(old);
    }
    fn protected_local_paths(&self) -> Vec<PathBuf> {
        match &self.source {
            Some(VfsPath::Local(path)) => vec![path.clone()],
            Some(source) => source.protected_local_paths(),
            None => Vec::new(),
        }
    }
}

/// Run on a background worker: opening validates the helper and loads its index.
pub fn open(
    helper: Helper,
    source: Option<VfsPath>,
    return_to: VfsPath,
    cancel: Option<&AtomicBool>,
) -> io::Result<Arc<Mount>> {
    if helper.needs_archive != source.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            if helper.needs_archive {
                "this extfs helper requires an archive"
            } else {
                "this extfs helper does not accept an archive"
            },
        ));
    }
    let prefix = format!("{}://", helper.name);
    let display_root = source.as_ref().map_or_else(
        || PathBuf::from(&prefix),
        |source| source.identity().join(&prefix),
    );
    let label = format!("extfs {}", helper.name);
    let backend = Arc::new(ExtfsBackend {
        helper,
        source,
        cached: Mutex::new(None),
        building: AtomicBool::new(false),
        closed: AtomicBool::new(false),
    });
    backend.snapshot(&Cancellation {
        requested: cancel,
        closed: None,
    })?;
    Mount::new(label, display_root, return_to, backend)
}

#[cfg(test)]
mod tests;
