//! Read-only archive filesystem shared by pane navigation, previews and copy jobs.
//!
//! A backend supplies a metadata index and bounded member readers. Pane code never
//! parses a container or handles compression. TAR compression is a separate layer.
mod bzip;
mod cache;
mod gzip;
mod tar;
mod zip;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use crate::{
    FileEntry, FileEntryKind, FileEntryMetadata, SelectionSizeReport, SortField, SortMode,
};

/// Archive containers supported by read-only browsing, previews, and extraction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveFormat {
    Zip,
    Tar,
    TarGzip,
    TarBzip2,
}

impl ArchiveFormat {
    /// Infers a format from a case-insensitive filename suffix without opening the file.
    ///
    /// Recognizes `.zip`, `.tar`, `.tar.gz`, `.tgz`, `.tar.bz2`, `.tbz2`, and `.tbz`.
    /// Returns `None` for other suffixes or paths without a filename. This does not
    /// validate the archive contents; readers validate the selected format when opened.
    #[must_use]
    pub fn for_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.as_encoded_bytes().to_ascii_lowercase();
        if name.ends_with(b".zip") {
            Some(Self::Zip)
        } else if name.ends_with(b".tar.gz") || name.ends_with(b".tgz") {
            Some(Self::TarGzip)
        } else if name.ends_with(b".tar.bz2") || name.ends_with(b".tbz2") || name.ends_with(b".tbz")
        {
            Some(Self::TarBzip2)
        } else if name.ends_with(b".tar") {
            Some(Self::Tar)
        } else {
            None
        }
    }

    /// Returns the display label, using the canonical format name for aliases.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Zip => "ZIP",
            Self::Tar => "TAR",
            Self::TarGzip => "TAR.GZ",
            Self::TarBzip2 => "TAR.BZ2",
        }
    }
}

/// A read-only directory in an archive. `path()` is a display/selection identity,
/// never a local filesystem path. Only backends open the containing local file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchiveLocation {
    /// Local path to the containing archive file.
    pub archive: PathBuf,
    /// Safe member directory relative to the archive root; empty for the root itself.
    ///
    /// Readers reject absolute paths, parent traversal, and unsafe components.
    pub directory: PathBuf,
}

impl ArchiveLocation {
    /// Creates a root location without opening or validating the containing file.
    #[must_use]
    pub fn root(archive: PathBuf) -> Self {
        Self {
            archive,
            directory: PathBuf::new(),
        }
    }

    /// Infers the containing file's format using [`ArchiveFormat::for_path`].
    ///
    /// Returns [`io::ErrorKind::Unsupported`] if its filename has no supported suffix.
    pub fn format(&self) -> io::Result<ArchiveFormat> {
        ArchiveFormat::for_path(&self.archive)
            .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "unsupported archive format"))
    }

    /// Joins the archive path and member directory into a synthetic pane identity.
    ///
    /// Pass this identity to archive APIs; it does not name a local directory.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.archive.join(&self.directory)
    }

    pub(crate) fn member_path(&self, path: &Path) -> io::Result<PathBuf> {
        let member = path.strip_prefix(&self.archive).map_err(io::Error::other)?;
        if member
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid archive member path",
            ));
        }
        Ok(member.to_path_buf())
    }

    pub(crate) fn parent_path(&self) -> PathBuf {
        self.directory.parent().map_or_else(
            || {
                self.archive
                    .parent()
                    .unwrap_or(Path::new("/"))
                    .to_path_buf()
            },
            |parent| self.archive.join(parent),
        )
    }

    pub(crate) fn read_entries(
        &self,
        sort_mode: SortMode,
        show_hidden: bool,
        cancel: Option<&AtomicBool>,
    ) -> io::Result<Vec<FileEntry>> {
        let index = cache::get_index(self, cancel)?;
        if !self.directory.as_os_str().is_empty()
            && !index
                .members
                .get(&self.directory)
                .is_some_and(|member| member.is_dir)
        {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "archive directory no longer exists",
            ));
        }
        let mut entries = Vec::new();
        // Touch only this directory's children, regardless of archive size.
        for path in index.children.get(&self.directory).into_iter().flatten() {
            check_canceled(cancel)?;
            let member = &index.members[path];
            let name = path
                .file_name()
                .expect("indexed members have validated, nonempty relative paths")
                .to_string_lossy()
                .into_owned();
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            entries.push((
                member.order,
                FileEntry {
                    name,
                    path: self.archive.join(path),
                    kind: if member.is_dir {
                        FileEntryKind::Directory
                    } else {
                        FileEntryKind::File
                    },
                    size: member.size,
                    modified: member.modified,
                    metadata: FileEntryMetadata {
                        mode: member.mode,
                        ..FileEntryMetadata::default()
                    },
                },
            ));
        }
        if sort_mode.field == SortField::Unsorted {
            entries.sort_by_key(|(order, _)| *order);
        }
        let mut entries = entries
            .into_iter()
            .map(|(_, entry)| entry)
            .collect::<Vec<_>>();
        crate::sort_file_entries(&mut entries, sort_mode);
        entries.insert(0, FileEntry::parent(self.parent_path()));
        Ok(entries)
    }
}

/// Only regular files and directories can be materialized. A backend may list
/// unsupported entries, but must flag them before any destination is committed.
#[derive(Clone, Debug)]
pub(crate) struct ArchiveMember {
    // Opaque backend locator: ZIP record number or TAR stream offset.
    pub index: Option<u64>,
    order: usize,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub mode: Option<u32>,
    pub encrypted: bool,
    pub unsupported: bool,
}

impl ArchiveMember {
    fn directory() -> Self {
        Self {
            index: None,
            order: usize::MAX,
            is_dir: true,
            size: 0,
            modified: None,
            mode: None,
            encrypted: false,
            unsupported: false,
        }
    }

    pub(crate) fn ensure_extractable(&self) -> io::Result<()> {
        if self.encrypted {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "password-protected archive entries cannot yet be extracted or viewed",
            ));
        }
        if self.unsupported
            || self
                .mode
                .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o040000 | 0o100000))
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "archive extraction supports regular files and directories only (no symlinks, hard links, sparse or special files)",
            ));
        }
        Ok(())
    }
}

/// The read-only filesystem contract. No mutation methods exist on archive
/// backends; the common extraction job copies member bytes to a local directory.
trait ArchiveBackend {
    fn open_member(&mut self, member: &ArchiveMember) -> io::Result<Box<dyn Read + '_>>;
}

/// Immutable backend indexes can be shared between panes and worker lanes.
trait IndexedArchive: Send + Sync {
    fn open<'a>(
        &self,
        file: File,
        cancel: Option<&'a AtomicBool>,
    ) -> io::Result<Box<dyn ArchiveBackend + 'a>>;
    fn estimated_bytes(&self) -> usize {
        0
    }
}

pub(crate) struct ArchiveIndex {
    pub members: BTreeMap<PathBuf, ArchiveMember>,
    children: BTreeMap<PathBuf, Vec<PathBuf>>,
    identity: cache::SourceIdentity,
    backend: Box<dyn IndexedArchive>,
    estimated_bytes: usize,
}

impl ArchiveIndex {
    pub(crate) fn open(
        location: &ArchiveLocation,
        cancel: Option<&AtomicBool>,
    ) -> io::Result<Arc<Self>> {
        cache::get_index(location, cancel)
    }

    fn build(
        mut members: BTreeMap<PathBuf, ArchiveMember>,
        identity: cache::SourceIdentity,
        backend: Box<dyn IndexedArchive>,
        cancel: Option<&AtomicBool>,
    ) -> io::Result<Self> {
        let leaves = members
            .iter()
            .map(|(path, m)| (path.clone(), m.is_dir, m.size, m.modified, m.order))
            .collect::<Vec<_>>();
        for (path, is_dir, size, modified, order) in leaves {
            for parent in path
                .ancestors()
                .skip(1)
                .take_while(|path| !path.as_os_str().is_empty())
            {
                check_canceled(cancel)?;
                let directory = members
                    .entry(parent.to_path_buf())
                    .or_insert_with(ArchiveMember::directory);
                if !directory.is_dir {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "archive contains a file used as a directory",
                    ));
                }
                directory.order = directory.order.min(order);
                if directory.index.is_none() {
                    directory.modified = directory.modified.max(modified);
                }
                if !is_dir {
                    directory.size = directory.size.saturating_add(size);
                }
            }
        }
        let mut children = BTreeMap::<PathBuf, Vec<PathBuf>>::new();
        for path in members.keys() {
            check_canceled(cancel)?;
            children
                .entry(
                    path.parent()
                        .expect("nonempty relative member paths have a parent, possibly the root")
                        .to_path_buf(),
                )
                .or_default()
                .push(path.clone());
        }
        let estimated_bytes = members
            .keys()
            .fold(backend.estimated_bytes(), |total, path| {
                total.saturating_add(path.as_os_str().len().saturating_mul(3).saturating_add(256))
            });
        Ok(Self {
            members,
            children,
            identity,
            backend,
            estimated_bytes,
        })
    }

    pub(crate) fn subtree<'r>(
        &'r self,
        path: &'r Path,
    ) -> impl Iterator<Item = (&'r PathBuf, &'r ArchiveMember)> {
        self.members
            .range(path.to_path_buf()..)
            .take_while(move |(member_path, _)| member_path.starts_with(path))
    }

    pub(crate) fn selections(
        &self,
        location: &ArchiveLocation,
        sources: &[PathBuf],
    ) -> io::Result<Vec<PathBuf>> {
        let mut selected = BTreeSet::new();
        for source in sources {
            let member = location.member_path(source)?;
            if member.as_os_str().is_empty() || !self.members.contains_key(&member) {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "selected archive member no longer exists",
                ));
            }
            selected.insert(member);
        }
        Ok(selected
            .iter()
            .filter(|path| {
                !path
                    .ancestors()
                    .skip(1)
                    .any(|parent| selected.contains(parent))
            })
            .cloned()
            .collect())
    }

    fn estimated_bytes(&self) -> usize {
        self.estimated_bytes
    }
}

pub(crate) struct ArchiveReader<'a> {
    index: Arc<ArchiveIndex>,
    backend: Box<dyn ArchiveBackend + 'a>,
    path: PathBuf,
    cancel: Option<&'a AtomicBool>,
}

impl Deref for ArchiveReader<'_> {
    type Target = ArchiveIndex;
    fn deref(&self) -> &Self::Target {
        &self.index
    }
}

impl<'a> ArchiveReader<'a> {
    pub fn open(location: &ArchiveLocation, cancel: Option<&'a AtomicBool>) -> io::Result<Self> {
        let index = cache::get_index(location, cancel)?;
        let file = open_source(&location.archive)?;
        index.identity.verify(&file.metadata()?)?;
        let backend = normalize_archive_io_result(index.backend.open(file, cancel), cancel)?;
        Ok(Self {
            index,
            backend,
            path: location.archive.clone(),
            cancel,
        })
    }

    pub fn open_member(&mut self, member: &ArchiveMember) -> io::Result<Box<dyn Read + '_>> {
        check_canceled(self.cancel)?;
        member.ensure_extractable()?;
        normalize_archive_io_result(self.backend.open_member(member), self.cancel)
    }

    pub fn verify_source(&self) -> io::Result<()> {
        check_canceled(self.cancel)?;
        self.index.identity.verify(&fs::metadata(&self.path)?)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn inspect_member_opens<F>(self, inspect: F) -> Self
    where
        F: FnMut(&ArchiveMember) + 'a,
    {
        // Let extraction tests inspect on-disk staging before reading payloads,
        // without racing a worker or allocating an artificially large archive.
        struct InspectingBackend<'a, F> {
            backend: Box<dyn ArchiveBackend + 'a>,
            inspect: F,
        }
        impl<F: FnMut(&ArchiveMember)> ArchiveBackend for InspectingBackend<'_, F> {
            fn open_member(&mut self, member: &ArchiveMember) -> io::Result<Box<dyn Read + '_>> {
                (self.inspect)(member);
                self.backend.open_member(member)
            }
        }
        Self {
            backend: Box::new(InspectingBackend {
                backend: self.backend,
                inspect,
            }),
            ..self
        }
    }
}

/// Measures apparent bytes for selected archive members, including directory descendants.
///
/// Each path must be a synthetic identity formed as `location.archive.join(member)`,
/// where `member` is relative to the archive root, not to `location.directory`.
/// Duplicate selections and descendants of selected directories are counted once.
/// Sizes come from metadata and do not imply that a member can be extracted.
///
/// This function blocks while acquiring or building the shared metadata index. The
/// first scan of a compressed TAR decompresses and validates the entire stream;
/// subsequent calls can reuse the index. No extracted files are written.
///
/// # Errors
///
/// Returns [`io::ErrorKind::Interrupted`] when `cancel` is set, and an error for
/// invalid or missing member paths, unreadable or malformed archives, unsupported
/// formats, or changes to the archive during indexing. Failures return no partial
/// report; successful reports have `unreadable_entries` equal to zero.
pub fn measure_archive_selection(
    location: &ArchiveLocation,
    paths: &[PathBuf],
    cancel: &AtomicBool,
) -> io::Result<SelectionSizeReport> {
    let index = cache::get_index(location, Some(cancel))?;
    let selected = index.selections(location, paths)?;
    let mut apparent_bytes = 0_u64;
    for path in selected {
        check_canceled(Some(cancel))?;
        apparent_bytes = apparent_bytes.saturating_add(index.members[&path].size);
    }
    Ok(SelectionSizeReport {
        apparent_bytes,
        unreadable_entries: 0,
    })
}

fn open_source(path: &Path) -> io::Result<File> {
    let mut options = File::options();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::fcntl::OFlag::O_NONBLOCK.bits());
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "archive browsing requires a regular file",
        ));
    }
    Ok(file)
}

pub(crate) struct CancelReader<'a, R = File> {
    file: R,
    cancel: Option<&'a AtomicBool>,
}

impl<R: Read> Read for CancelReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        check_archive_io_canceled(self.cancel)?;
        self.file.read(buffer)
    }
}

impl<R: Seek> Seek for CancelReader<'_, R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        check_archive_io_canceled(self.cancel)?;
        self.file.seek(position)
    }

    fn seek_relative(&mut self, offset: i64) -> io::Result<()> {
        check_archive_io_canceled(self.cancel)?;
        self.file.seek_relative(offset)
    }

    fn stream_position(&mut self) -> io::Result<u64> {
        check_archive_io_canceled(self.cancel)?;
        self.file.stream_position()
    }
}

fn check_archive_io_canceled(cancel: Option<&AtomicBool>) -> io::Result<()> {
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        // Read helpers retry Interrupted. All container and compression I/O must
        // fail without retry; restore rc's cancellation convention at the boundary.
        return Err(io::Error::other(crate::JOB_CANCELED_MESSAGE));
    }
    Ok(())
}

pub(crate) fn normalize_archive_io_result<T>(
    result: io::Result<T>,
    cancel: Option<&AtomicBool>,
) -> io::Result<T> {
    check_canceled(cancel)?;
    result
}

pub(crate) fn check_canceled(cancel: Option<&AtomicBool>) -> io::Result<()> {
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            crate::JOB_CANCELED_MESSAGE,
        ));
    }
    Ok(())
}

// Validate on every platform, including Windows drive names, separators and ADS.
// Never turn an unsafe path into a different, apparently safe member name.
fn safe_member_path(name: &[u8]) -> io::Result<Option<PathBuf>> {
    if name.starts_with(b"/")
        || name.iter().any(|byte| matches!(byte, b'\\' | 0 | b':'))
        || name.len() > 65_536
        || name
            .split(|byte| *byte == b'/')
            .enumerate()
            .any(|(depth, part)| depth >= 256 || part == b"..")
    {
        return Ok(None);
    }

    // Archive paths are byte strings. Preserve their native identity; only display
    // labels may be lossy. Platforms without byte paths must report the limit.
    #[cfg(unix)]
    let name = {
        use std::os::unix::ffi::OsStrExt;
        OsStr::from_bytes(name)
    };
    #[cfg(not(unix))]
    let name = OsStr::new(std::str::from_utf8(name).map_err(|_| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "non-UTF-8 archive member paths cannot be represented on this platform",
        )
    })?);

    let mut path = PathBuf::new();
    for part in Path::new(name).components() {
        match part {
            Component::CurDir => {}
            Component::Normal(name) if safe_component(name) => path.push(name),
            _ => return Ok(None),
        }
    }
    Ok((!path.as_os_str().is_empty()).then_some(path))
}

fn safe_component(name: &OsStr) -> bool {
    let name = name.as_encoded_bytes();
    if name
        .iter()
        .any(|byte| matches!(byte, b'\\' | b'/' | 0 | b':'))
    {
        return false;
    }
    #[cfg(windows)]
    {
        if matches!(name.last(), Some(b'.' | b' ')) {
            return false;
        }
        let stem = name
            .split(|byte| *byte == b'.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if matches!(
            stem.as_slice(),
            b"CON" | b"PRN" | b"AUX" | b"NUL" | b"CONIN$" | b"CONOUT$"
        ) || (stem.len() == 4
            && (stem.starts_with(b"COM") || stem.starts_with(b"LPT"))
            && matches!(stem[3], b'1'..=b'9'))
        {
            return false;
        }
    }
    true
}

fn invalid_data(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
