use super::{
    ArchiveBackend, ArchiveMember, CancelReader, IndexedArchive, check_canceled, invalid_data,
    normalize_archive_io_result, safe_member_path,
};
use ::zip::ZipArchive;
use chrono::TimeZone;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::SystemTime;

struct ZipIndex;

pub(super) fn restored_index() -> Box<dyn IndexedArchive> {
    Box::new(ZipIndex)
}
struct ZipBackend<'a> {
    archive: ZipArchive<CancelReader<'a>>,
}

impl IndexedArchive for ZipIndex {
    fn open<'a>(
        &self,
        file: File,
        cancel: Option<&'a AtomicBool>,
    ) -> io::Result<Box<dyn ArchiveBackend + 'a>> {
        let archive = open_zip_archive(CancelReader { file, cancel }, cancel)?;
        Ok(Box::new(ZipBackend { archive }))
    }
}

impl ArchiveBackend for ZipBackend<'_> {
    fn open_member(&mut self, member: &ArchiveMember) -> io::Result<Box<dyn Read + '_>> {
        let index = usize::try_from(
            member
                .index
                .ok_or_else(|| invalid_data("ZIP member has no record"))?,
        )
        .map_err(io::Error::other)?;
        let input = self.archive.by_index(index).map_err(zip_error)?;
        Ok(Box::new(input))
    }
}

pub(super) fn build_index(
    file: &mut File,
    cancel: Option<&AtomicBool>,
) -> io::Result<(BTreeMap<PathBuf, ArchiveMember>, Box<dyn IndexedArchive>)> {
    let archive = open_zip_archive(CancelReader { file, cancel }, cancel)?;
    let mut members = BTreeMap::new();
    for index in 0..archive.len() {
        check_canceled(cancel)?;
        // Central-directory metadata only. No member payload is decompressed.
        let entry = archive.by_index_data(index).map_err(zip_error)?;
        let Some(path) = safe_member_path(entry.name_raw())? else {
            continue;
        };
        let member = ArchiveMember {
            index: Some(index as u64),
            order: index,
            is_dir: entry.is_dir(),
            size: if entry.is_dir() { 0 } else { entry.size() },
            modified: entry.last_modified().and_then(zip_time),
            mode: entry.unix_mode(),
            encrypted: entry.encrypted(),
            unsupported: false,
        };
        if members.insert(path, member).is_some() {
            return Err(invalid_data("ZIP has conflicting member paths"));
        }
    }
    Ok((members, Box::new(ZipIndex)))
}

fn open_zip_archive<R: Read + Seek>(
    reader: R,
    cancel: Option<&AtomicBool>,
) -> io::Result<ZipArchive<R>> {
    check_canceled(cancel)?;
    normalize_archive_io_result(ZipArchive::new(reader).map_err(zip_error), cancel)
}

fn zip_time(time: ::zip::DateTime) -> Option<SystemTime> {
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

pub(crate) fn zip_error(error: ::zip::result::ZipError) -> io::Error {
    match error {
        ::zip::result::ZipError::Io(error) => error,
        error => io::Error::new(io::ErrorKind::InvalidData, format!("ZIP: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};
    use std::cell::Cell;
    use std::io::{Cursor, SeekFrom, Write};
    use std::sync::atomic::Ordering;

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
        let error =
            normalize_archive_io_result(archive.by_index(0).map_err(zip_error), Some(&cancel))
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
            let error = normalize_archive_io_result(member.read(&mut [0; 1024]), Some(&cancel))
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
        assert_canceled(normalize_archive_io_result(result, Some(&cancel)).unwrap_err());
    }

    #[test]
    fn zip_error_normalization_preserves_real_errors_and_handles_wrapped_cancellation() {
        let cancel = AtomicBool::new(false);
        let error = normalize_archive_io_result::<()>(
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied")),
            Some(&cancel),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), "denied");
        cancel.store(true, Ordering::Relaxed);
        let error = normalize_archive_io_result::<()>(
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
