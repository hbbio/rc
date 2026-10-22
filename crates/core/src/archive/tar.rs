//! TAR metadata parser and member backend. Compression readers implement seek in
//! the logical TAR stream, so neither pane code nor extraction depends on codecs.
use super::bzip::{BzipIndex, BzipReader};
use super::gzip::{GzipIndex, GzipReader};
use super::{
    ArchiveBackend, ArchiveFormat, ArchiveMember, CancelReader, IndexedArchive,
    check_archive_io_canceled, invalid_data, safe_member_path,
};
use ::tar::Header;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

enum CompressionIndex {
    Plain,
    Gzip(Arc<GzipIndex>),
    Bzip(Arc<BzipIndex>),
}

struct TarIndex {
    compression: CompressionIndex,
}
struct TarBackend<'a> {
    input: Box<dyn ReadSeek + 'a>,
}

impl IndexedArchive for TarIndex {
    fn open<'a>(
        &self,
        file: File,
        cancel: Option<&'a AtomicBool>,
    ) -> io::Result<Box<dyn ArchiveBackend + 'a>> {
        let input = CancelReader { file, cancel };
        let input: Box<dyn ReadSeek + 'a> = match &self.compression {
            CompressionIndex::Plain => Box::new(BufReader::with_capacity(64 * 1024, input)),
            CompressionIndex::Gzip(index) => Box::new(GzipReader::indexed(input, index, cancel)?),
            CompressionIndex::Bzip(index) => Box::new(BzipReader::indexed(input, index, cancel)),
        };
        // Check cancellation even when the compression reader has buffered data.
        Ok(Box::new(TarBackend {
            input: Box::new(CancelReader {
                file: input,
                cancel,
            }),
        }))
    }

    fn estimated_bytes(&self) -> usize {
        match &self.compression {
            CompressionIndex::Plain => 0,
            CompressionIndex::Gzip(index) => index.estimated_bytes(),
            CompressionIndex::Bzip(index) => index.estimated_bytes(),
        }
    }
}

impl ArchiveBackend for TarBackend<'_> {
    fn open_member(&mut self, member: &ArchiveMember) -> io::Result<Box<dyn Read + '_>> {
        let offset = member
            .index
            .ok_or_else(|| invalid_data("TAR member has no offset"))?;
        let current = self.input.stream_position()?;
        if let Ok(distance) = i64::try_from(i128::from(offset) - i128::from(current)) {
            self.input.as_mut().seek_relative(distance)?;
        } else {
            self.input.seek(SeekFrom::Start(offset))?;
        }
        Ok(Box::new(self.input.by_ref().take(member.size)))
    }
}

pub(super) fn build_index(
    file: &mut File,
    format: ArchiveFormat,
    cancel: Option<&AtomicBool>,
) -> io::Result<(BTreeMap<PathBuf, ArchiveMember>, Box<dyn IndexedArchive>)> {
    let size = file.metadata()?.len();
    let input = CancelReader { file, cancel };
    let (members, compression) = match format {
        ArchiveFormat::Tar => {
            let mut input = BufReader::with_capacity(64 * 1024, input);
            (
                scan(&mut input, Some(size), cancel)?,
                CompressionIndex::Plain,
            )
        }
        ArchiveFormat::TarGzip => {
            let mut input = GzipReader::build(input, cancel)?;
            let members = scan(&mut input, None, cancel)?;
            (members, CompressionIndex::Gzip(input.finish()?))
        }
        ArchiveFormat::TarBzip2 => {
            let mut input = BzipReader::build(input, cancel)?;
            let members = scan(&mut input, None, cancel)?;
            (members, CompressionIndex::Bzip(input.finish()?))
        }
        ArchiveFormat::Zip => unreachable!("ZIP has its own backend"),
    };
    Ok((members, Box::new(TarIndex { compression })))
}

// Use tar-rs for standard/GNU numeric and path fields. Handle extension records
// explicitly so malicious long-name/PAX sizes cannot trigger large allocations.
fn scan<R: Read + Seek>(
    input: &mut R,
    length: Option<u64>,
    cancel: Option<&AtomicBool>,
) -> io::Result<BTreeMap<PathBuf, ArchiveMember>> {
    const MAX_EXTENSION: u64 = 1024 * 1024;
    let mut members = BTreeMap::<PathBuf, ArchiveMember>::new();
    let mut global = BTreeMap::<String, Vec<u8>>::new();
    let mut local = BTreeMap::<String, Vec<u8>>::new();
    let mut long_name = None;
    let mut order = 0;
    loop {
        check_archive_io_canceled(cancel)?;
        let mut raw = [0_u8; 512];
        if input.read(&mut raw[..1])? == 0 {
            break;
        }
        input.read_exact(&mut raw[1..])?;
        if raw.iter().all(|byte| *byte == 0) {
            break;
        }
        let header = Header::from_byte_slice(&raw);
        let checksum: u32 = raw
            .iter()
            .enumerate()
            .map(|(index, byte)| {
                if (148..156).contains(&index) {
                    32
                } else {
                    u32::from(*byte)
                }
            })
            .sum();
        if header.cksum()? != checksum {
            return Err(invalid_data("TAR header checksum mismatch"));
        }
        let kind = header.entry_type();
        let mut size = header.entry_size()?;
        let extension = kind.is_gnu_longname()
            || kind.is_gnu_longlink()
            || kind.is_pax_local_extensions()
            || kind.is_pax_global_extensions();
        if extension {
            if size > MAX_EXTENSION {
                return Err(invalid_data("TAR metadata record exceeds 1 MiB"));
            }
            let mut data = vec![0_u8; size as usize];
            input.read_exact(&mut data)?;
            if kind.is_gnu_longname() {
                if data.last() == Some(&0) {
                    data.pop();
                }
                long_name = Some(data);
            } else if kind.is_pax_local_extensions() {
                parse_pax(&data, &mut local)?;
            } else if kind.is_pax_global_extensions() {
                parse_pax(&data, &mut global)?;
            }
            let padding = (512 - size % 512) % 512;
            input.seek_relative(padding as i64)?;
            continue;
        }
        let value = |name: &str| local.get(name).or_else(|| global.get(name));
        if let Some(value) = value("size") {
            size = parse_u64(value)?;
        }
        // Old GNU sparse extension headers precede the physical payload.
        let mut extended =
            kind.is_gnu_sparse() && header.as_gnu().is_some_and(|header| header.is_extended());
        let mut extensions = 0;
        while extended {
            check_archive_io_canceled(cancel)?;
            extensions += 1;
            if extensions > 4096 {
                return Err(invalid_data("too many GNU sparse metadata records"));
            }
            let mut sparse = [0_u8; 512];
            input.read_exact(&mut sparse)?;
            extended = sparse[504] != 0;
        }
        let offset = input.stream_position()?;
        let next = offset
            .checked_add(size)
            .and_then(|position| position.checked_add((512 - size % 512) % 512))
            .ok_or_else(|| invalid_data("TAR size overflow"))?;
        if length.is_some_and(|length| next > length) {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "TAR member extends beyond the archive",
            ));
        }
        let header_name = header.path_bytes();
        let name = value("path")
            .map(Vec::as_slice)
            .or(long_name.as_deref())
            .unwrap_or(&header_name);
        if let Some(path) = safe_member_path(name)? {
            let regular = matches!(kind.as_byte(), 0 | b'0' | b'7');
            let is_dir = kind.is_dir() || (regular && name.ends_with(b"/"));
            let sparse = kind.is_gnu_sparse()
                || local
                    .keys()
                    .chain(global.keys())
                    .any(|key| key.starts_with("GNU.sparse.") || key == "SCHILY.realsize");
            let modified = if let Some(value) = value("mtime") {
                Some(parse_time(value)?)
            } else {
                header
                    .mtime()
                    .ok()
                    .and_then(|seconds| UNIX_EPOCH.checked_add(Duration::from_secs(seconds)))
            };
            let mode_type = if is_dir {
                0o040000
            } else if kind.is_symlink() {
                0o120000
            } else if regular {
                0o100000
            } else {
                0o010000
            };
            let member = ArchiveMember {
                index: Some(offset),
                order,
                is_dir,
                size: if is_dir {
                    0
                } else if kind.is_gnu_sparse() {
                    header.size()?
                } else {
                    size
                },
                modified,
                mode: header.mode().ok().map(|mode| (mode & 0o7777) | mode_type),
                encrypted: false,
                unsupported: sparse || (!is_dir && !regular),
            };
            if members
                .get(&path)
                .is_some_and(|previous| previous.is_dir != is_dir)
            {
                return Err(invalid_data("TAR has conflicting member paths"));
            }
            // TAR permits repeated names (e.g. tar --append). The last record wins.
            members.insert(path, member);
        }
        order += 1;
        local.clear();
        long_name = None;
        input.seek_relative(i64::try_from(next - offset).map_err(io::Error::other)?)?;
    }
    if !local.is_empty() || long_name.is_some() {
        return Err(invalid_data("TAR metadata record has no following member"));
    }
    Ok(members)
}

fn parse_pax(mut data: &[u8], values: &mut BTreeMap<String, Vec<u8>>) -> io::Result<()> {
    while !data.is_empty() {
        let space =
            memchr::memchr(b' ', data).ok_or_else(|| invalid_data("invalid PAX record length"))?;
        let size = usize::try_from(parse_u64(&data[..space])?).map_err(io::Error::other)?;
        if size <= space + 2 || size > data.len() || data[size - 1] != b'\n' {
            return Err(invalid_data("invalid PAX record"));
        }
        let record = &data[space + 1..size - 1];
        let equals = memchr::memchr(b'=', record).ok_or_else(|| invalid_data("invalid PAX key"))?;
        let key = std::str::from_utf8(&record[..equals]).map_err(io::Error::other)?;
        if matches!(key, "path" | "size" | "mtime" | "SCHILY.realsize")
            || key.starts_with("GNU.sparse.")
        {
            let value = &record[equals + 1..];
            if value.is_empty() {
                values.remove(key);
            } else {
                values.insert(key.to_owned(), value.to_vec());
            }
        }
        data = &data[size..];
    }
    Ok(())
}

fn parse_u64(value: &[u8]) -> io::Result<u64> {
    std::str::from_utf8(value)
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| invalid_data("invalid TAR numeric metadata"))
}

fn parse_time(value: &[u8]) -> io::Result<SystemTime> {
    let text = std::str::from_utf8(value).map_err(io::Error::other)?;
    let negative = text.starts_with('-');
    let text = text.strip_prefix('-').unwrap_or(text);
    let (seconds, fraction) = text.split_once('.').unwrap_or((text, ""));
    let seconds = seconds.parse::<u64>().map_err(io::Error::other)?;
    if !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_data("invalid TAR timestamp"));
    }
    let mut nanos = 0_u32;
    for index in 0..9 {
        nanos =
            nanos * 10 + u32::from(fraction.as_bytes().get(index).copied().unwrap_or(b'0') - b'0');
    }
    let duration = Duration::new(seconds, nanos);
    (if negative {
        UNIX_EPOCH.checked_sub(duration)
    } else {
        UNIX_EPOCH.checked_add(duration)
    })
    .ok_or_else(|| invalid_data("TAR timestamp is out of range"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::normalize_archive_io_result;
    use std::cell::Cell;
    use std::io::Cursor;
    use std::path::Path;
    use std::sync::atomic::Ordering;

    struct Counted<'a> {
        file: Cursor<Vec<u8>>,
        bytes: &'a Cell<usize>,
        calls: &'a Cell<usize>,
    }
    impl Read for Counted<'_> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            let count = self.file.read(output)?;
            self.bytes.set(self.bytes.get() + count);
            self.calls.set(self.calls.get() + 1);
            Ok(count)
        }
    }
    impl Seek for Counted<'_> {
        fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
            self.file.seek(from)
        }
    }

    #[test]
    fn plain_tar_scans_headers_by_seeking_over_payloads_and_reuses_input_buffers() {
        let mut builder = ::tar::Builder::new(Vec::new());
        let mut header = Header::new_gnu();
        header.set_size(16 * 1024 * 1024);
        header.set_mode(0o644);
        header.set_mtime(1);
        header.set_cksum();
        builder
            .append_data(
                &mut header,
                "large",
                io::repeat(b'x').take(16 * 1024 * 1024),
            )
            .unwrap();
        for number in 0..2000 {
            header.set_size(1);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("small/{number}"), b"x".as_slice())
                .unwrap();
        }
        let raw = builder.into_inner().unwrap();
        let length = raw.len() as u64;
        let bytes = Cell::new(0);
        let calls = Cell::new(0);
        let counted = Counted {
            file: Cursor::new(raw),
            bytes: &bytes,
            calls: &calls,
        };
        let mut input = BufReader::with_capacity(64 * 1024, counted);
        let members = scan(&mut input, Some(length), None).unwrap();
        assert_eq!(members.len(), 2001);
        assert!(
            bytes.get() < 3 * 1024 * 1024,
            "plain TAR must skip the 16 MiB payload"
        );
        assert!(
            calls.get() < 50,
            "small members must reuse buffered directory headers"
        );
        bytes.set(0);
        calls.set(0);
        let mut backend = TarBackend {
            input: Box::new(CancelReader {
                file: input,
                cancel: None,
            }),
        };
        for number in 0..2000 {
            let member = &members[Path::new(&format!("small/{number}"))];
            let mut output = Vec::new();
            backend
                .open_member(member)
                .unwrap()
                .read_to_end(&mut output)
                .unwrap();
            assert_eq!(output, b"x");
        }
        assert!(
            calls.get() < 50,
            "member reads must preserve seek_relative and stream_position buffering"
        );
        assert!(bytes.get() < 3 * 1024 * 1024);
    }

    #[test]
    fn cancellation_in_a_partial_tar_header_is_not_retried() {
        struct Partial<'a> {
            cancel: &'a AtomicBool,
            calls: usize,
        }
        impl Read for Partial<'_> {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                self.calls += 1;
                assert!(self.calls <= 2, "canceled TAR read kept retrying");
                output[0] = b'x';
                self.cancel.store(true, Ordering::Relaxed);
                Ok(1)
            }
        }
        impl Seek for Partial<'_> {
            fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
                Ok(0)
            }
        }
        let cancel = AtomicBool::new(false);
        let mut reader = CancelReader {
            file: Partial {
                cancel: &cancel,
                calls: 0,
            },
            cancel: Some(&cancel),
        };
        let error =
            normalize_archive_io_result(scan(&mut reader, None, Some(&cancel)), Some(&cancel))
                .err()
                .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(reader.file.calls, 1);
    }
}
