//! Typed locations and a common read contract for native files, archives, and
//! mounted helper/remote backends. Display identities are never native paths.
use std::collections::HashSet;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::{ArchiveLocation, FileEntry, FileEntryKind, FileEntryMetadata, SortMode};

pub mod tasks;
pub(crate) mod walk;

#[cfg(unix)]
pub mod extfs;
#[cfg(unix)]
mod process;
#[cfg(unix)]
mod remote_cache;
#[cfg(unix)]
pub mod sftp;
#[cfg(unix)]
pub mod shell_link;
#[cfg(unix)]
pub mod ssh;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendKind {
    Local,
    Archive,
    Helper,
    Sftp,
    Ftp,
    Shell,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capabilities {
    pub read_only: bool,
    pub native_paths: bool,
}

/// Cancellation combines the originating job with the mount's lifetime.
pub struct Cancellation<'a> {
    requested: Option<&'a AtomicBool>,
    closed: Option<&'a AtomicBool>,
}

impl Cancellation<'_> {
    pub fn is_canceled(&self) -> bool {
        self.requested
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
            || self.closed.is_some_and(|flag| flag.load(Ordering::Acquire))
    }
    pub fn check(&self) -> io::Result<()> {
        if self.is_canceled() {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                crate::JOB_CANCELED_MESSAGE,
            ))
        } else {
            Ok(())
        }
    }
    /// `Read` consumers often retry Interrupted; use a nonretrying error inside
    /// readers and normalize it at the operation boundary with `finish`.
    pub fn check_read(&self) -> io::Result<()> {
        if self.is_canceled() {
            Err(io::Error::other(crate::JOB_CANCELED_MESSAGE))
        } else {
            Ok(())
        }
    }
    pub fn finish<T>(&self, result: io::Result<T>) -> io::Result<T> {
        self.check()?;
        result
    }
}

/// Backend paths are relative to the mount root. Listings contain immediate
/// children only, without `.`/`..`. Reads are scoped to a callback so backends
/// can borrow a container or connection without materializing whole files.
pub trait Backend: Send + Sync {
    fn kind(&self) -> BackendKind;
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            read_only: true,
            native_paths: false,
        }
    }
    fn read_directory(&self, path: &Path, cancel: &Cancellation<'_>) -> io::Result<Vec<FileEntry>>;
    fn metadata(&self, path: &Path, cancel: &Cancellation<'_>) -> io::Result<FileEntry>;
    fn with_file(
        &self,
        path: &Path,
        cancel: &Cancellation<'_>,
        consume: &mut dyn FnMut(&FileEntry, &mut dyn Read) -> io::Result<()>,
    ) -> io::Result<()>;
    /// Release resources without waiting for readers; their cancellation context
    /// is already marked closed when this hook runs.
    fn close(&self) {}
    /// Drop reusable metadata without interrupting readers of an older snapshot.
    fn invalidate(&self) {}
    /// Native containers that must never be overwritten by extraction.
    fn protected_local_paths(&self) -> Vec<PathBuf> {
        Vec::new()
    }
}

static NEXT_MOUNT: AtomicU64 = AtomicU64::new(1);

pub struct Mount {
    id: u64,
    label: String,
    display_root: PathBuf,
    return_to: VfsPath,
    backend: Arc<dyn Backend>,
    closed: AtomicBool,
}

impl fmt::Debug for Mount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mount")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("kind", &self.backend.kind())
            .field("closed", &self.is_closed())
            .finish()
    }
}

impl Mount {
    pub fn new(
        label: String,
        display_root: PathBuf,
        return_to: VfsPath,
        backend: Arc<dyn Backend>,
    ) -> io::Result<Arc<Self>> {
        if display_root.as_os_str().is_empty()
            || display_root.as_os_str().as_encoded_bytes().contains(&0)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid VFS display root",
            ));
        }
        Ok(Arc::new(Self {
            id: NEXT_MOUNT.fetch_add(1, Ordering::Relaxed),
            label,
            display_root,
            return_to,
            backend,
            closed: AtomicBool::new(false),
        }))
    }
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn label(&self) -> &str {
        &self.label
    }
    pub fn kind(&self) -> BackendKind {
        self.backend.kind()
    }
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
    pub fn return_to(&self) -> &VfsPath {
        &self.return_to
    }
    pub fn close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.backend.close();
        }
    }
    pub fn invalidate(&self) {
        if !self.is_closed() {
            self.backend.invalidate();
        }
    }
    pub fn root(self: &Arc<Self>) -> VfsPath {
        VfsPath::Mounted(MountedPath {
            mount: Arc::clone(self),
            relative: PathBuf::new(),
        })
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        self.close();
    }
}

#[derive(Clone, Debug)]
pub struct MountedPath {
    mount: Arc<Mount>,
    relative: PathBuf,
}

impl PartialEq for MountedPath {
    fn eq(&self, other: &Self) -> bool {
        self.mount.id == other.mount.id && self.relative == other.relative
    }
}
impl Eq for MountedPath {}

impl MountedPath {
    pub fn mount(&self) -> &Arc<Mount> {
        &self.mount
    }
    pub fn relative(&self) -> &Path {
        &self.relative
    }
    pub fn at(&self, relative: PathBuf) -> io::Result<Self> {
        validate_relative(&relative)?;
        Ok(Self {
            mount: Arc::clone(&self.mount),
            relative,
        })
    }
    pub fn identity(&self) -> PathBuf {
        self.mount.display_root.join(&self.relative)
    }
    pub fn resolve_identity(&self, identity: &Path) -> io::Result<Self> {
        self.at(identity
            .strip_prefix(&self.mount.display_root)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "path belongs to another VFS mount",
                )
            })?
            .to_path_buf())
    }
    fn cancellation<'a>(&'a self, requested: Option<&'a AtomicBool>) -> Cancellation<'a> {
        Cancellation {
            requested,
            closed: Some(&self.mount.closed),
        }
    }
}

/// A file or directory identity. Only `Local` can yield an OS filesystem path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VfsPath {
    Local(PathBuf),
    Archive {
        location: ArchiveLocation,
        path: PathBuf,
    },
    Mounted(MountedPath),
}

impl From<ArchiveLocation> for VfsPath {
    fn from(location: ArchiveLocation) -> Self {
        Self::Archive {
            path: location.path(),
            location,
        }
    }
}

impl VfsPath {
    /// Relative input follows typed parents, including leaving a mount at its
    /// root. Absolute input selects a native path unless it is an identity in
    /// this virtual location.
    pub fn resolve_input(&self, input: &Path) -> io::Result<Self> {
        let root = match self {
            Self::Mounted(path) => Some(path.mount().root()),
            Self::Archive { location, .. } => Some(location.in_directory(PathBuf::new()).into()),
            _ => None,
        };
        if let Some(root) = root
            && let Ok(relative) = input.strip_prefix(root.identity())
        {
            return root.resolve_relative_input(relative);
        }
        if input.is_absolute() {
            return Ok(Self::Local(input.to_path_buf()));
        }
        self.resolve_relative_input(input)
    }
    fn resolve_relative_input(&self, input: &Path) -> io::Result<Self> {
        let mut location = self.clone();
        for component in input.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    if let Some(parent) = location.parent()? {
                        location = parent;
                    }
                }
                Component::Normal(name) => {
                    location = location.resolve_identity(location.identity().join(name))?
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "invalid VFS input path",
                    ));
                }
            }
        }
        Ok(location)
    }
    pub fn protected_local_paths(&self) -> Vec<PathBuf> {
        match self {
            Self::Local(_) => Vec::new(),
            Self::Archive { location, .. } => vec![location.local_archive().to_path_buf()],
            Self::Mounted(path) => path.mount.backend.protected_local_paths(),
        }
    }
    pub fn identity(&self) -> PathBuf {
        match self {
            Self::Local(path) | Self::Archive { path, .. } => path.clone(),
            Self::Mounted(path) => path.identity(),
        }
    }
    pub fn native_path(&self) -> Option<&Path> {
        match self {
            Self::Local(path) => Some(path),
            _ => None,
        }
    }
    pub fn local_directory(&self) -> &Path {
        match self {
            Self::Local(path) => path,
            Self::Archive { location, .. } => {
                location.local_archive().parent().unwrap_or(Path::new("."))
            }
            Self::Mounted(path) => path.mount.return_to.local_directory(),
        }
    }
    pub fn kind(&self) -> BackendKind {
        match self {
            Self::Local(_) => BackendKind::Local,
            Self::Archive { .. } => BackendKind::Archive,
            Self::Mounted(path) => path.mount.kind(),
        }
    }
    pub fn capabilities(&self) -> Capabilities {
        match self {
            Self::Local(_) => Capabilities {
                read_only: false,
                native_paths: true,
            },
            Self::Archive { .. } => Capabilities {
                read_only: true,
                native_paths: false,
            },
            Self::Mounted(path) => Capabilities {
                native_paths: false,
                ..path.mount.backend.capabilities()
            },
        }
    }
    pub fn resolve_identity(&self, identity: PathBuf) -> io::Result<Self> {
        match self {
            Self::Local(_) => Ok(Self::Local(identity)),
            Self::Archive { location, .. } => {
                location.member_path(&identity)?;
                Ok(Self::Archive {
                    location: location.clone(),
                    path: identity,
                })
            }
            Self::Mounted(path) => Ok(Self::Mounted(path.resolve_identity(&identity)?)),
        }
    }
    pub fn parent(&self) -> io::Result<Option<Self>> {
        match self {
            Self::Local(path) => Ok(path
                .parent()
                .map(|parent| Self::Local(parent.to_path_buf()))),
            Self::Archive { location, path } => {
                let member = location.member_path(path)?;
                if let Some(parent) = member.parent() {
                    Ok(Some(location.in_directory(parent.to_path_buf()).into()))
                } else {
                    Ok(Some(
                        location
                            .parent
                            .as_deref()
                            .cloned()
                            .map(Into::into)
                            .unwrap_or_else(|| {
                                Self::Local(
                                    location
                                        .local_archive()
                                        .parent()
                                        .unwrap_or(Path::new("/"))
                                        .to_path_buf(),
                                )
                            }),
                    ))
                }
            }
            Self::Mounted(path) => Ok(Some(match path.relative.parent() {
                Some(parent) => Self::Mounted(path.at(parent.to_path_buf())?),
                None => path.mount.return_to.clone(),
            })),
        }
    }
    pub fn read_directory(
        &self,
        sort: SortMode,
        show_hidden: bool,
        cancel: Option<&AtomicBool>,
    ) -> io::Result<Vec<FileEntry>> {
        crate::archive::check_canceled(cancel)?;
        match self {
            Self::Local(path) => match cancel {
                Some(cancel) => crate::panel::read_entries_with_visibility_cancel(
                    path,
                    sort,
                    show_hidden,
                    Some(cancel),
                ),
                None => crate::panel::read_entries_with_visibility(path, sort, show_hidden),
            },
            Self::Archive { location, path } => location
                .in_directory(location.member_path(path)?)
                .read_entries(sort, show_hidden, cancel),
            Self::Mounted(path) => {
                let cancellation = path.cancellation(cancel);
                cancellation.check()?;
                let mut entries = cancellation.finish(
                    path.mount
                        .backend
                        .read_directory(&path.relative, &cancellation),
                )?;
                let mut seen = HashSet::new();
                for entry in &mut entries {
                    cancellation.check()?;
                    validate_relative(&entry.path)?;
                    if entry.is_parent()
                        || entry.path.parent() != Some(path.relative.as_path())
                        || !seen.insert(entry.path.clone())
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "VFS backend returned an invalid or duplicate child path",
                        ));
                    }
                    entry.name = entry
                        .path
                        .file_name()
                        .expect("validated child path")
                        .to_string_lossy()
                        .into_owned();
                    entry.path = path.mount.display_root.join(&entry.path);
                }
                if !show_hidden {
                    entries.retain(|entry| {
                        !entry
                            .path
                            .file_name()
                            .is_some_and(|name| name.as_encoded_bytes().starts_with(b"."))
                    });
                }
                crate::sort_file_entries(&mut entries, sort);
                if let Some(parent) = self.parent()? {
                    entries.insert(0, FileEntry::parent(parent.identity()));
                }
                cancellation.check()?;
                Ok(entries)
            }
        }
    }
    pub fn metadata(&self, cancel: Option<&AtomicBool>) -> io::Result<FileEntry> {
        crate::archive::check_canceled(cancel)?;
        match self {
            Self::Local(path) => {
                let metadata = std::fs::metadata(path)?;
                let name = path
                    .file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .into_owned();
                let mut entry = if metadata.is_dir() {
                    FileEntry::directory_from_metadata(name, path.clone(), Some(&metadata))
                } else {
                    FileEntry::file_from_metadata(name, path.clone(), Some(&metadata))
                };
                entry.metadata.symlink_target = crate::panel::read_local_metadata(path).1;
                Ok(entry)
            }
            Self::Archive { location, path } => {
                let index = crate::archive::ArchiveIndex::open(location, cancel)?;
                let member = location.member_path(path)?;
                if member.as_os_str().is_empty() {
                    return Ok(FileEntry {
                        name: location
                            .archive
                            .file_name()
                            .unwrap_or(location.archive.as_os_str())
                            .to_string_lossy()
                            .into_owned(),
                        path: path.clone(),
                        kind: FileEntryKind::Directory,
                        size: 0,
                        modified: None,
                        metadata: FileEntryMetadata::default(),
                    });
                }
                let member = index.members.get(&member).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "archive member no longer exists")
                })?;
                Ok(FileEntry {
                    name: path
                        .file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy()
                        .into_owned(),
                    path: path.clone(),
                    kind: if member.is_dir {
                        FileEntryKind::Directory
                    } else {
                        FileEntryKind::File
                    },
                    size: member.size,
                    modified: member.modified,
                    metadata: FileEntryMetadata {
                        mode: member.mode,
                        ..Default::default()
                    },
                })
            }
            Self::Mounted(path) => {
                let cancellation = path.cancellation(cancel);
                cancellation.check()?;
                let mut entry = cancellation
                    .finish(path.mount.backend.metadata(&path.relative, &cancellation))?;
                if entry.path != path.relative || entry.is_parent() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "VFS metadata names a different path",
                    ));
                }
                entry.path = path.identity();
                entry.name = path
                    .relative
                    .file_name()
                    .unwrap_or(path.relative.as_os_str())
                    .to_string_lossy()
                    .into_owned();
                cancellation.check()?;
                Ok(entry)
            }
        }
    }
    pub fn with_file(
        &self,
        cancel: Option<&AtomicBool>,
        consume: &mut dyn FnMut(&FileEntry, &mut dyn Read) -> io::Result<()>,
    ) -> io::Result<()> {
        crate::archive::check_canceled(cancel)?;
        match self {
            Self::Local(path) => {
                let mut options = File::options();
                options.read(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(nix::libc::O_NONBLOCK);
                }
                let file = options.open(path)?;
                let metadata = file.metadata()?;
                if !metadata.is_file() {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "VFS reads require a regular file",
                    ));
                }
                let entry = FileEntry::file_from_metadata(
                    path.file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy()
                        .into_owned(),
                    path.clone(),
                    Some(&metadata),
                );
                let cancellation = Cancellation {
                    requested: cancel,
                    closed: None,
                };
                cancellation.finish(consume(
                    &entry,
                    &mut CheckedReader {
                        input: file,
                        cancel: &cancellation,
                    },
                ))
            }
            Self::Archive { location, path } => {
                let mut reader = crate::archive::ArchiveReader::open(location, cancel)?;
                let member = reader
                    .members
                    .get(&location.member_path(path)?)
                    .cloned()
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::NotFound, "archive member no longer exists")
                    })?;
                if member.is_dir {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "VFS reads require a regular file",
                    ));
                }
                let entry = FileEntry {
                    name: path
                        .file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy()
                        .into_owned(),
                    path: path.clone(),
                    kind: FileEntryKind::File,
                    size: member.size,
                    modified: member.modified,
                    metadata: FileEntryMetadata {
                        mode: member.mode,
                        ..Default::default()
                    },
                };
                let mut input = reader.open_member(&member)?;
                crate::archive::normalize_archive_io_result(consume(&entry, &mut input), cancel)?;
                drop(input);
                reader.verify_source()
            }
            Self::Mounted(path) => {
                let cancellation = path.cancellation(cancel);
                cancellation.check()?;
                let mut calls = 0_usize;
                let mut callback_error = None;
                let result = path.mount.backend.with_file(
                    &path.relative,
                    &cancellation,
                    &mut |entry, input| {
                        calls = calls.saturating_add(1);
                        let result = if calls != 1
                            || entry.path != path.relative
                            || entry.kind != FileEntryKind::File
                        {
                            Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "VFS reader returned invalid file metadata",
                            ))
                        } else {
                            let mut entry = entry.clone();
                            entry.path = path.identity();
                            consume(
                                &entry,
                                &mut CheckedReader {
                                    input,
                                    cancel: &cancellation,
                                },
                            )
                        };
                        match result {
                            Ok(()) => Ok(()),
                            Err(error) => {
                                let forwarded = io::Error::new(error.kind(), error.to_string());
                                callback_error.get_or_insert(error);
                                Err(forwarded)
                            }
                        }
                    },
                );
                cancellation.finish(callback_error.map_or(result, Err))?;
                if calls != 1 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "VFS backend did not provide file data",
                    ));
                }
                Ok(())
            }
        }
    }
}

struct CheckedReader<'a, R> {
    input: R,
    cancel: &'a Cancellation<'a>,
}
impl<R: Read> Read for CheckedReader<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.cancel.check_read()?;
        let result = self.input.read(output);
        self.cancel.check_read()?;
        result
    }
}

pub fn validate_relative(path: &Path) -> io::Result<()> {
    if path.as_os_str().as_encoded_bytes().contains(&0)
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "VFS path must stay relative to its mount root",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
