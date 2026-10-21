use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use chrono::TimeZone;
use zip::ZipArchive;

use crate::{
    FileEntry, FileEntryKind, FileEntryMetadata, SelectionSizeReport, SortField, SortMode,
};

/// A read-only directory inside a local ZIP file. Member paths are always relative
/// to the archive; `path()` is a display/selection identity, never a local path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZipLocation {
    pub archive: PathBuf,
    pub directory: PathBuf,
}

impl ZipLocation {
    pub fn root(archive: PathBuf) -> Self {
        Self {
            archive,
            directory: PathBuf::new(),
        }
    }

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
                "invalid ZIP member path",
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
        let reader = ZipReader::open(self, cancel)?;
        if !self.directory.as_os_str().is_empty()
            && !reader
                .members
                .get(&self.directory)
                .is_some_and(|member| member.is_dir)
        {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "ZIP directory no longer exists",
            ));
        }
        let mut entries = Vec::new();
        for (path, member) in &reader.members {
            check_canceled(cancel)?;
            if path.parent() != Some(self.directory.as_path()) {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
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

pub(crate) struct CancelReader<'a, R = File> {
    file: R,
    cancel: Option<&'a AtomicBool>,
}

impl<R: Read> Read for CancelReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        check_zip_io_canceled(self.cancel)?;
        self.file.read(buffer)
    }
}

impl<R: Seek> Seek for CancelReader<'_, R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        check_zip_io_canceled(self.cancel)?;
        self.file.seek(position)
    }
}

fn check_zip_io_canceled(cancel: Option<&AtomicBool>) -> io::Result<()> {
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        // read_exact and other Read helpers retry Interrupted indefinitely. Keep
        // cancellation non-retryable while zip or its decompressor owns the read.
        return Err(io::Error::other(crate::JOB_CANCELED_MESSAGE));
    }
    Ok(())
}

pub(crate) fn normalize_zip_io_result<T>(
    result: io::Result<T>,
    cancel: Option<&AtomicBool>,
) -> io::Result<T> {
    // Only restore rc's Interrupted/canceled convention after returning from zip.
    // Inspect the flag because zip may wrap the original I/O error while parsing.
    check_canceled(cancel)?;
    result
}

fn open_zip_archive<R: Read + Seek>(
    reader: R,
    cancel: Option<&AtomicBool>,
) -> io::Result<ZipArchive<R>> {
    check_canceled(cancel)?;
    normalize_zip_io_result(ZipArchive::new(reader).map_err(zip_error), cancel)
}

#[derive(Clone, Debug)]
pub(crate) struct ZipMember {
    pub index: Option<usize>,
    // First central-directory record mentioning this entry or an implicit folder.
    order: usize,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub mode: Option<u32>,
    pub encrypted: bool,
}

impl ZipMember {
    fn directory() -> Self {
        Self {
            index: None,
            order: usize::MAX,
            is_dir: true,
            size: 0,
            modified: None,
            mode: None,
            encrypted: false,
        }
    }

    pub(crate) fn ensure_extractable(&self) -> io::Result<()> {
        if self.encrypted {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "password-protected ZIP entries cannot yet be extracted or viewed",
            ));
        }
        if self
            .mode
            .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o040000 | 0o100000))
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "ZIP extraction supports regular files and directories only (no symlinks or special files)",
            ));
        }
        Ok(())
    }
}

pub(crate) struct ZipReader<'a> {
    pub archive: ZipArchive<CancelReader<'a>>,
    pub members: BTreeMap<PathBuf, ZipMember>,
}

impl<'a> ZipReader<'a> {
    pub(crate) fn subtree<'r>(
        &'r self,
        path: &'r Path,
    ) -> impl Iterator<Item = (&'r PathBuf, &'r ZipMember)> {
        // Path ordering keeps a directory and its descendants contiguous. This
        // avoids rescanning every member for every tagged file in a large ZIP.
        self.members
            .range(path.to_path_buf()..)
            .take_while(move |(member_path, _)| member_path.starts_with(path))
    }

    pub fn open(location: &ZipLocation, cancel: Option<&'a AtomicBool>) -> io::Result<Self> {
        check_canceled(cancel)?;
        if location
            .directory
            .components()
            .any(|component| match component {
                Component::Normal(name) => !name.to_str().is_some_and(safe_component),
                _ => true,
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid ZIP directory",
            ));
        }
        let mut options = File::options();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(nix::fcntl::OFlag::O_NONBLOCK.bits());
        }
        let file = options.open(&location.archive)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "ZIP browsing requires a regular file",
            ));
        }
        let archive = open_zip_archive(CancelReader { file, cancel }, cancel)?;
        let mut members = BTreeMap::new();
        for index in 0..archive.len() {
            check_canceled(cancel)?;
            // This reads central-directory metadata only, even for encrypted,
            // corrupt, or unsupported compressed payloads.
            let entry = archive.by_index_data(index).map_err(zip_error)?;
            let name = entry.name().map_err(zip_error)?;
            let Some(path) = safe_member_path(&name) else {
                continue;
            };
            let member = ZipMember {
                index: Some(index),
                order: index,
                is_dir: entry.is_dir(),
                size: if entry.is_dir() { 0 } else { entry.size() },
                modified: entry.last_modified().and_then(zip_time),
                mode: entry.unix_mode(),
                encrypted: entry.encrypted(),
            };
            if members.insert(path, member).is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "ZIP has conflicting member paths",
                ));
            }
        }
        let leaves = members
            .iter()
            .map(|(path, member)| {
                (
                    path.clone(),
                    member.is_dir,
                    member.size,
                    member.modified,
                    member.order,
                )
            })
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
                    .or_insert_with(ZipMember::directory);
                if !directory.is_dir {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "ZIP contains a file used as a directory",
                    ));
                }
                directory.order = directory.order.min(order);
                if directory.index.is_none() {
                    // An implicit folder has no timestamp of its own. Use its
                    // newest descendant, preserving explicitly stored dates.
                    directory.modified = directory.modified.max(modified);
                }
                if !is_dir {
                    directory.size = directory.size.saturating_add(size);
                }
            }
        }
        Ok(Self { archive, members })
    }

    pub(crate) fn selections(
        &self,
        location: &ZipLocation,
        sources: &[PathBuf],
    ) -> io::Result<Vec<PathBuf>> {
        let mut selected = BTreeSet::new();
        for source in sources {
            let member = location.member_path(source)?;
            if member.as_os_str().is_empty() || !self.members.contains_key(&member) {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "selected ZIP member no longer exists",
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
}

pub fn measure_zip_selection(
    location: &ZipLocation,
    paths: &[PathBuf],
    cancel: &AtomicBool,
) -> io::Result<SelectionSizeReport> {
    let reader = ZipReader::open(location, Some(cancel))?;
    let selected = reader.selections(location, paths)?;
    let mut apparent_bytes = 0_u64;
    for path in selected {
        check_canceled(Some(cancel))?;
        apparent_bytes = apparent_bytes.saturating_add(reader.members[&path].size);
    }
    Ok(SelectionSizeReport {
        apparent_bytes,
        unreadable_entries: 0,
    })
}

// Validate on every platform, including Windows drive names, separators and ADS.
// Never turn an unsafe path into a different, apparently safe member name.
fn safe_member_path(name: &str) -> Option<PathBuf> {
    if name.starts_with('/') || name.contains(['\\', '\0', ':']) {
        return None;
    }
    let mut path = PathBuf::new();
    for part in name.split('/') {
        match part {
            ".." => return None,
            "" | "." => {}
            part if safe_component(part) => path.push(part),
            _ => return None,
        }
    }
    (!path.as_os_str().is_empty()).then_some(path)
}

fn safe_component(name: &str) -> bool {
    if name.contains(['\\', '/', '\0', ':']) {
        return false;
    }
    #[cfg(windows)]
    {
        // Win32 strips these suffixes and treats device names specially, even
        // when they have a file extension. Do not allow them to alias paths.
        if name.ends_with(['.', ' ']) {
            return false;
        }
        let stem = name
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            return false;
        }
    }
    true
}

fn zip_time(time: zip::DateTime) -> Option<SystemTime> {
    chrono::Local
        .with_ymd_and_hms(
            i32::from(time.year()),
            u32::from(time.month()),
            u32::from(time.day()),
            u32::from(time.hour()),
            u32::from(time.minute()),
            u32::from(time.second()),
        )
        .earliest()
        .map(SystemTime::from)
}

pub(crate) fn zip_error(error: zip::result::ZipError) -> io::Error {
    match error {
        zip::result::ZipError::Io(error) => error,
        error => io::Error::new(io::ErrorKind::InvalidData, format!("ZIP: {error}")),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::io::{Cursor, Write};
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    struct Fixture {
        bytes: Vec<u8>,
        central_header: u64,
        local_header: u64,
        payload: u64,
    }

    impl Fixture {
        fn new(compression: CompressionMethod) -> Self {
            let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
            writer
                .start_file(
                    "entry.txt",
                    SimpleFileOptions::default().compression_method(compression),
                )
                .unwrap();
            writer.write_all(&[b'x'; 1024]).unwrap();
            let bytes = writer.finish().unwrap().into_inner();
            let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
            let entry = archive.by_index_data(0).unwrap();
            let central_header = entry.central_header_start();
            let local_header = entry.header_start();
            let payload = archive.by_index(0).unwrap().data_start().unwrap();
            Self {
                bytes,
                central_header,
                local_header,
                payload,
            }
        }
    }

    struct CancelDuringRead<'a> {
        file: Cursor<Vec<u8>>,
        offset: u64,
        armed: &'a Cell<bool>,
        fired: &'a Cell<bool>,
        cancel: &'a AtomicBool,
    }

    impl Read for CancelDuringRead<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.armed.get()
                && !self.fired.get()
                && self.file.position() == self.offset
                && !buffer.is_empty()
            {
                // Cancel after returning only part of the requested data. A
                // read_exact already in progress must call CancelReader again.
                let count = self.file.read(&mut buffer[..1])?;
                self.fired.set(true);
                self.cancel.store(true, Ordering::Relaxed);
                return Ok(count);
            }
            self.file.read(buffer)
        }
    }

    impl Seek for CancelDuringRead<'_> {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.file.seek(position)
        }
    }

    struct ReadBudget<'a, R> {
        reader: R,
        calls: usize,
        canceled_reads: &'a Cell<usize>,
        cancel: &'a AtomicBool,
    }

    impl<R: Read> Read for ReadBudget<'_, R> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.calls += 1;
            // Bound a regression instead of hanging the entire test process.
            assert!(
                self.calls <= 128,
                "ZIP parser kept retrying a canceled read"
            );
            if self.cancel.load(Ordering::Relaxed) {
                self.canceled_reads.set(self.canceled_reads.get() + 1);
            }
            self.reader.read(buffer)
        }
    }

    impl<R: Seek> Seek for ReadBudget<'_, R> {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.reader.seek(position)
        }
    }

    fn reader<'a>(
        bytes: Vec<u8>,
        offset: u64,
        armed: &'a Cell<bool>,
        fired: &'a Cell<bool>,
        cancel: &'a AtomicBool,
        canceled_reads: &'a Cell<usize>,
    ) -> impl Read + Seek + 'a {
        ReadBudget {
            reader: CancelReader {
                file: CancelDuringRead {
                    file: Cursor::new(bytes),
                    offset,
                    armed,
                    fired,
                    cancel,
                },
                cancel: Some(cancel),
            },
            calls: 0,
            canceled_reads,
            cancel,
        }
    }

    fn assert_canceled(error: io::Error) {
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(crate::JobError::from_io(error).is_canceled());
    }

    #[test]
    fn zip_metadata_cancellation_during_header_and_filename_reads_stops_retrying() {
        let fixture = Fixture::new(CompressionMethod::Stored);
        for offset in [fixture.central_header, fixture.central_header + 46] {
            let cancel = AtomicBool::new(false);
            let armed = Cell::new(true);
            let fired = Cell::new(false);
            let canceled_reads = Cell::new(0);
            let input = reader(
                fixture.bytes.clone(),
                offset,
                &armed,
                &fired,
                &cancel,
                &canceled_reads,
            );
            let error = open_zip_archive(input, Some(&cancel))
                .err()
                .expect("in-flight scan must be canceled");
            assert!(fired.get(), "cancellation must occur during a real read");
            assert_eq!(
                canceled_reads.get(),
                1,
                "canceled reads must not be retried"
            );
            assert_canceled(error);
        }
    }

    #[test]
    fn zip_member_header_cancellation_stops_retrying() {
        let fixture = Fixture::new(CompressionMethod::Deflated);
        let cancel = AtomicBool::new(false);
        let armed = Cell::new(false);
        let fired = Cell::new(false);
        let canceled_reads = Cell::new(0);
        let input = reader(
            fixture.bytes,
            fixture.local_header,
            &armed,
            &fired,
            &cancel,
            &canceled_reads,
        );
        let mut archive = open_zip_archive(input, Some(&cancel)).unwrap();
        armed.set(true);
        let error = normalize_zip_io_result(archive.by_index(0).map_err(zip_error), Some(&cancel))
            .err()
            .expect("in-flight member open must be canceled");
        assert!(fired.get());
        assert_eq!(canceled_reads.get(), 1);
        assert_canceled(error);
    }

    #[test]
    fn zip_payload_cancellation_is_normalized_for_stored_and_deflated_members() {
        for compression in [CompressionMethod::Stored, CompressionMethod::Deflated] {
            let fixture = Fixture::new(compression);
            let cancel = AtomicBool::new(false);
            let armed = Cell::new(false);
            let fired = Cell::new(false);
            let canceled_reads = Cell::new(0);
            let input = reader(
                fixture.bytes,
                fixture.payload,
                &armed,
                &fired,
                &cancel,
                &canceled_reads,
            );
            let mut archive = open_zip_archive(input, Some(&cancel)).unwrap();
            let mut member = archive.by_index(0).unwrap();
            armed.set(true);
            let error = normalize_zip_io_result(member.read(&mut [0; 1024]), Some(&cancel))
                .expect_err("in-flight member read must be canceled");
            assert!(fired.get());
            assert!(canceled_reads.get() <= 1);
            assert_canceled(error);
        }
    }

    #[test]
    fn zip_seek_cancellation_is_non_retryable_until_it_leaves_the_io_boundary() {
        let cancel = AtomicBool::new(true);
        let mut reader = CancelReader {
            file: Cursor::new(Vec::<u8>::new()),
            cancel: Some(&cancel),
        };
        let result = reader.seek(SeekFrom::Start(0));
        assert_eq!(result.as_ref().unwrap_err().kind(), io::ErrorKind::Other);
        assert_canceled(normalize_zip_io_result(result, Some(&cancel)).unwrap_err());
    }

    #[test]
    fn zip_error_normalization_preserves_real_errors_and_handles_wrapped_cancellation() {
        let cancel = AtomicBool::new(false);
        let error = normalize_zip_io_result::<()>(
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied")),
            Some(&cancel),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), "denied");
        cancel.store(true, Ordering::Relaxed);
        let error = normalize_zip_io_result::<()>(
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ZIP parser replaced the read error",
            )),
            Some(&cancel),
        )
        .unwrap_err();
        assert_canceled(error);
    }
}
