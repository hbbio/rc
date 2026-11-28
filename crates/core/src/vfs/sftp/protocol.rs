//! SFTP version 3 wire format, without remote shell parsing.
//! <https://datatracker.ietf.org/doc/html/draft-ietf-secsh-filexfer-02>
use super::super::{Cancellation, process::Duplex};
use crate::{FileEntry, FileEntryKind, FileEntryMetadata};
use std::collections::VecDeque;
use std::io::{self, Read};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, UNIX_EPOCH};

const PACKET_LIMIT: usize = 1024 * 1024;
const STRING_LIMIT: usize = 65_536;
const READ_CHUNK_SIZE: u64 = 32 * 1024;
const READ_WINDOW: usize = 8;
const TIMEOUT: Duration = Duration::from_secs(30);
const STATUS: u8 = 101;
const HANDLE: u8 = 102;
const DATA: u8 = 103;
const NAME: u8 = 104;
const ATTRS: u8 = 105;

fn invalid(message: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid SFTP response: {message}"),
    )
}

fn child_name(bytes: &[u8]) -> io::Result<PathBuf> {
    if bytes.contains(&b'/') {
        return Err(invalid("directory child name contains a separator"));
    }
    let path = PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec()));
    super::super::validate_relative(&path)?;
    if path.components().count() != 1 {
        return Err(invalid("invalid child name"));
    }
    Ok(path)
}

pub(super) fn string(bytes: &[u8], target: &mut Vec<u8>) -> io::Result<()> {
    if bytes.len() > STRING_LIMIT || bytes.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "SFTP path exceeds 64 KiB or contains NUL",
        ));
    }
    opaque_string(bytes, target)
}
fn opaque_string(bytes: &[u8], target: &mut Vec<u8>) -> io::Result<()> {
    let length = u32::try_from(bytes.len()).map_err(|_| invalid("oversized string"))?;
    target.extend_from_slice(&length.to_be_bytes());
    target.extend_from_slice(bytes);
    Ok(())
}

struct Decoder<'a>(&'a [u8]);
impl<'a> Decoder<'a> {
    fn take(&mut self, count: usize) -> io::Result<&'a [u8]> {
        if count > self.0.len() {
            return Err(invalid("truncated packet"));
        }
        let (value, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(value)
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().expect("length checked"),
        ))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().expect("length checked"),
        ))
    }
    fn string(&mut self) -> io::Result<&'a [u8]> {
        let count = self.u32()? as usize;
        if count > STRING_LIMIT {
            return Err(invalid("string exceeds 64 KiB"));
        }
        self.take(count)
    }
    fn finish(self) -> io::Result<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(invalid("trailing packet data"))
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Attributes {
    pub size: Option<u64>,
    pub mode: Option<u32>,
    uid: Option<u32>,
    gid: Option<u32>,
    atime: Option<u32>,
    mtime: Option<u32>,
    identity: Option<Vec<u8>>,
}
impl Attributes {
    fn decode(input: &mut Decoder<'_>) -> io::Result<Self> {
        let flags = input.u32()?;
        if flags & !0x8000_000f != 0 {
            return Err(invalid("unsupported attribute flags"));
        }
        let mut attrs = Self::default();
        if flags & 1 != 0 {
            attrs.size = Some(input.u64()?);
        }
        if flags & 2 != 0 {
            attrs.uid = Some(input.u32()?);
            attrs.gid = Some(input.u32()?);
        }
        if flags & 4 != 0 {
            attrs.mode = Some(input.u32()?);
        }
        if flags & 8 != 0 {
            attrs.atime = Some(input.u32()?);
            attrs.mtime = Some(input.u32()?);
        }
        if flags & 0x8000_0000 != 0 {
            let count = input.u32()?;
            if count > 1024 {
                return Err(invalid("too many extended attributes"));
            }
            for _ in 0..count {
                let key = input.string()?;
                let value = input.string()?;
                if key == b"rc-identity@rust-commander" {
                    if value.len() > 256 || attrs.identity.is_some() {
                        return Err(invalid("invalid source identity"));
                    }
                    attrs.identity = Some(value.to_vec());
                }
            }
        }
        Ok(attrs)
    }
    pub fn file_type(&self) -> io::Result<u32> {
        self.mode
            .map(|mode| mode & 0o170000)
            .ok_or_else(|| invalid("server omitted file type"))
    }
    pub fn entry(&self, path: &Path) -> io::Result<FileEntry> {
        let kind = if self.file_type()? == 0o040000 {
            FileEntryKind::Directory
        } else {
            FileEntryKind::File
        };
        Ok(FileEntry {
            name: path
                .file_name()
                .map_or_else(String::new, rc_shell::encode_path_text),
            path: path.into(),
            kind,
            size: if kind.is_dir() {
                0
            } else {
                self.size.unwrap_or(0)
            },
            modified: self
                .mtime
                .map(|seconds| UNIX_EPOCH + Duration::from_secs(u64::from(seconds))),
            metadata: FileEntryMetadata {
                mode: self.mode,
                user_id: self.uid,
                group_id: self.gid,
                accessed: self
                    .atime
                    .map(|seconds| UNIX_EPOCH + Duration::from_secs(u64::from(seconds))),
                symlink_target: (self.file_type()? == 0o120000).then_some(false),
                ..FileEntryMetadata::default()
            },
        })
    }
    pub fn verify(&self, current: &Self) -> io::Result<()> {
        // Access times may change as a consequence of this read.
        if self.identity != current.identity
            || self.size != current.size
            || self.mode != current.mode
            || self.mtime != current.mtime
            || self.uid != current.uid
            || self.gid != current.gid
        {
            return Err(io::Error::other(
                "SFTP file changed while being read; refresh and retry",
            ));
        }
        Ok(())
    }
}

struct Packet {
    kind: u8,
    bytes: Vec<u8>,
}
impl Packet {
    fn status(&self) -> io::Result<(u32, String)> {
        if self.kind != STATUS {
            return Err(invalid("expected status packet"));
        }
        let mut input = Decoder(&self.bytes);
        let code = input.u32()?;
        let message = rc_shell::sanitize_display_lossy(input.string()?);
        input.string()?;
        input.finish()?;
        Ok((code, message))
    }
    fn expect(&self, kind: u8) -> io::Result<Decoder<'_>> {
        if self.kind == kind {
            return Ok(Decoder(&self.bytes));
        }
        if self.kind == STATUS {
            let (code, message) = self.status()?;
            let kind = match code {
                2 => io::ErrorKind::NotFound,
                3 => io::ErrorKind::PermissionDenied,
                6 | 7 => io::ErrorKind::ConnectionAborted,
                8 => io::ErrorKind::Unsupported,
                _ => io::ErrorKind::Other,
            };
            return Err(io::Error::new(
                kind,
                format!("SFTP status {code}: {message}"),
            ));
        }
        Err(invalid("unexpected packet type"))
    }
    fn eof(&self) -> io::Result<bool> {
        if self.kind == STATUS {
            Ok(self.status()?.0 == 1)
        } else {
            Ok(false)
        }
    }

    fn read_data(&self, requested: usize) -> io::Result<&[u8]> {
        if self.eof()? {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "SFTP file was truncated",
            ));
        }
        let mut input = self.expect(DATA)?;
        let bytes = input.string()?;
        input.finish()?;
        if bytes.is_empty() || bytes.len() > requested {
            return Err(invalid("empty or oversized file data"));
        }
        Ok(bytes)
    }
}

pub(super) struct Session {
    transport: Duplex,
    next_id: u32,
    deadline: Option<Instant>,
}
impl Session {
    pub fn open(command: Command, cancel: &Cancellation<'_>) -> io::Result<Self> {
        let transport = Duplex::spawn(command, PACKET_LIMIT + 4, cancel)?;
        let mut session = Self {
            transport,
            next_id: 0,
            deadline: None,
        };
        let deadline = Instant::now() + TIMEOUT;
        session.send(&[1, 0, 0, 0, 3], deadline, cancel)?;
        let bytes = session.receive(deadline, cancel)?;
        if bytes[0] != 2 {
            return Err(invalid("missing version handshake"));
        }
        let mut input = Decoder(&bytes[1..]);
        if input.u32()? != 3 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "SFTP server must support protocol version 3",
            ));
        }
        while !input.0.is_empty() {
            input.string()?;
            input.string()?;
        }
        Ok(session)
    }
    fn send(
        &mut self,
        bytes: &[u8],
        deadline: Instant,
        cancel: &Cancellation<'_>,
    ) -> io::Result<()> {
        if bytes.is_empty() || bytes.len() > PACKET_LIMIT {
            return Err(invalid("invalid outbound packet length"));
        }
        let mut packet = Vec::with_capacity(bytes.len() + 4);
        packet.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        packet.extend_from_slice(bytes);
        self.transport.write_all(&packet, deadline, cancel)
    }
    fn receive(&mut self, deadline: Instant, cancel: &Cancellation<'_>) -> io::Result<Vec<u8>> {
        let mut length = [0_u8; 4];
        self.transport.read_exact(&mut length, deadline, cancel)?;
        let length = u32::from_be_bytes(length) as usize;
        if length == 0 || length > PACKET_LIMIT {
            return Err(invalid("packet length exceeds 1 MiB"));
        }
        let mut bytes = vec![0; length];
        self.transport.read_exact(&mut bytes, deadline, cancel)?;
        Ok(bytes)
    }
    fn request(
        &mut self,
        kind: u8,
        arguments: &[u8],
        cancel: &Cancellation<'_>,
    ) -> io::Result<Packet> {
        let deadline = self.request_deadline();
        let id = self.send_request(kind, arguments, deadline, cancel)?;
        let (response_id, packet) = self.receive_response(deadline, cancel)?;
        if response_id != id {
            return Err(invalid("request ID mismatch"));
        }
        Ok(packet)
    }

    fn request_deadline(&self) -> Instant {
        let deadline = Instant::now() + TIMEOUT;
        self.deadline.map_or(deadline, |end| end.min(deadline))
    }

    fn send_request(
        &mut self,
        kind: u8,
        arguments: &[u8],
        deadline: Instant,
        cancel: &Cancellation<'_>,
    ) -> io::Result<u32> {
        self.next_id = self.next_id.wrapping_add(1);
        let id = self.next_id;
        let mut bytes = Vec::with_capacity(5 + arguments.len());
        bytes.push(kind);
        bytes.extend_from_slice(&id.to_be_bytes());
        bytes.extend_from_slice(arguments);
        self.send(&bytes, deadline, cancel)?;
        Ok(id)
    }

    fn receive_response(
        &mut self,
        deadline: Instant,
        cancel: &Cancellation<'_>,
    ) -> io::Result<(u32, Packet)> {
        let response = self.receive(deadline, cancel)?;
        let mut input = Decoder(&response);
        let kind = input.take(1)?[0];
        let id = input.u32()?;
        let packet = Packet {
            kind,
            bytes: input.0.to_vec(),
        };
        Ok((id, packet))
    }
    pub fn realpath(&mut self, path: &Path, cancel: &Cancellation<'_>) -> io::Result<PathBuf> {
        let mut arguments = Vec::new();
        string(path.as_os_str().as_bytes(), &mut arguments)?;
        let packet = self.request(16, &arguments, cancel)?;
        let mut input = packet.expect(NAME)?;
        if input.u32()? != 1 {
            return Err(invalid("canonical path response must contain one name"));
        }
        let name = input.string()?;
        let path = PathBuf::from(std::ffi::OsString::from_vec(name.to_vec()));
        if name.contains(&0)
            || !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err(invalid("invalid canonical path"));
        }
        input.string()?;
        Attributes::decode(&mut input)?;
        input.finish()?;
        Ok(path)
    }
    pub fn stat(&mut self, path: &Path, cancel: &Cancellation<'_>) -> io::Result<Attributes> {
        let mut arguments = Vec::new();
        string(path.as_os_str().as_bytes(), &mut arguments)?;
        self.attributes(7, &arguments, cancel)
    }
    fn fstat(&mut self, handle: &[u8], cancel: &Cancellation<'_>) -> io::Result<Attributes> {
        let mut arguments = Vec::new();
        opaque_string(handle, &mut arguments)?;
        self.attributes(8, &arguments, cancel)
    }
    fn attributes(
        &mut self,
        kind: u8,
        arguments: &[u8],
        cancel: &Cancellation<'_>,
    ) -> io::Result<Attributes> {
        let packet = self.request(kind, arguments, cancel)?;
        let mut input = packet.expect(ATTRS)?;
        let attrs = Attributes::decode(&mut input)?;
        input.finish()?;
        Ok(attrs)
    }
    fn handle(
        &mut self,
        kind: u8,
        arguments: &[u8],
        cancel: &Cancellation<'_>,
    ) -> io::Result<Vec<u8>> {
        let packet = self.request(kind, arguments, cancel)?;
        let mut input = packet.expect(HANDLE)?;
        let handle = input.string()?.to_vec();
        input.finish()?;
        if handle.len() > 256 {
            return Err(invalid("invalid handle length"));
        }
        Ok(handle)
    }
    fn close(&mut self, handle: &[u8], cancel: &Cancellation<'_>) -> io::Result<()> {
        let mut arguments = Vec::new();
        opaque_string(handle, &mut arguments)?;
        let packet = self.request(4, &arguments, cancel)?;
        let (code, message) = packet.status()?;
        if code != 0 {
            return Err(io::Error::other(format!("SFTP close failed: {message}")));
        }
        Ok(())
    }
    pub fn list(
        &mut self,
        root: &Path,
        relative: &Path,
        cancel: &Cancellation<'_>,
    ) -> io::Result<Vec<FileEntry>> {
        let previous = self
            .deadline
            .replace(Instant::now() + Duration::from_secs(300));
        let result = self.list_inner(root, relative, cancel);
        self.deadline = previous;
        result
    }
    fn list_inner(
        &mut self,
        root: &Path,
        relative: &Path,
        cancel: &Cancellation<'_>,
    ) -> io::Result<Vec<FileEntry>> {
        let path = root.join(relative);
        let mut arguments = Vec::new();
        string(path.as_os_str().as_bytes(), &mut arguments)?;
        let handle = self.handle(11, &arguments, cancel)?;
        let mut arguments = Vec::new();
        opaque_string(&handle, &mut arguments)?;
        let mut entries = Vec::new();
        let mut names = std::collections::HashSet::new();
        let mut received = 0_usize;
        let mut bytes = 0_usize;
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            cancel.check()?;
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "SFTP directory listing timed out",
                ));
            }
            let packet = self.request(12, &arguments, cancel)?;
            if packet.eof()? {
                break;
            }
            let mut input = packet.expect(NAME)?;
            let count = input.u32()? as usize;
            received = received.saturating_add(count);
            if count == 0 || received > 100_000 {
                return Err(invalid("empty listing batch or more than 100,000 entries"));
            }
            for _ in 0..count {
                cancel.check()?;
                let name = input.string()?;
                input.string()?;
                let mut attrs = Attributes::decode(&mut input)?;
                if name == b"." || name == b".." {
                    continue;
                }
                let name = child_name(name)?;
                if !names.insert(name.clone()) {
                    return Err(invalid("duplicate directory name"));
                }
                let relative = relative.join(name);
                bytes = bytes.saturating_add((relative.as_os_str().len() + 128).saturating_mul(4));
                if bytes > 16 * 1024 * 1024 {
                    return Err(invalid("listing metadata exceeds 16 MiB"));
                }
                if attrs.mode.is_none() {
                    attrs = self.stat(&root.join(&relative), cancel)?;
                }
                entries.push(attrs.entry(&relative)?);
            }
            input.finish()?;
        }
        self.close(&handle, cancel)?;
        Ok(entries)
    }
    pub fn with_file(
        &mut self,
        root: &Path,
        relative: &Path,
        cancel: &Cancellation<'_>,
        consume: &mut dyn FnMut(&FileEntry, &mut dyn Read) -> io::Result<()>,
    ) -> io::Result<()> {
        let path = root.join(relative);
        let listed = self.stat(&path, cancel)?;
        if listed.file_type()? != 0o100000 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "SFTP links and special files cannot be read",
            ));
        }
        let mut arguments = Vec::new();
        string(path.as_os_str().as_bytes(), &mut arguments)?;
        arguments.extend_from_slice(&1_u32.to_be_bytes()); // SSH_FXF_READ, never create/write.
        arguments.extend_from_slice(&0_u32.to_be_bytes()); // no attributes
        let handle = self.handle(3, &arguments, cancel)?;
        let attrs = self.fstat(&handle, cancel)?;
        listed.verify(&attrs)?;
        let size = attrs
            .size
            .ok_or_else(|| invalid("server omitted regular file size"))?;
        attrs.verify(&self.stat(&path, cancel)?)?;
        let entry = attrs.entry(relative)?;
        let mut reader = FileReader {
            session: self,
            handle: &handle,
            offset: 0,
            size,
            cancel,
            reads: VecDeque::with_capacity(READ_WINDOW),
            failed: false,
        };
        consume(&entry, &mut reader)?;
        reader.drain()?;
        attrs.verify(&self.fstat(&handle, cancel)?)?;
        attrs.verify(&self.stat(&path, cancel)?)?;
        self.close(&handle, cancel)
    }
}

struct QueuedRead {
    id: u32,
    start: u64,
    end: u64,
    data: Option<Vec<u8>>,
}

struct FileReader<'a, 'c> {
    session: &'a mut Session,
    handle: &'a [u8],
    offset: u64,
    size: u64,
    cancel: &'c Cancellation<'c>,
    // Keep at most eight chunks in flight or buffered, in file order.
    reads: VecDeque<QueuedRead>,
    failed: bool,
}

impl FileReader<'_, '_> {
    fn send_read(&mut self, start: u64, end: u64) -> io::Result<QueuedRead> {
        let mut arguments = Vec::with_capacity(self.handle.len() + 16);
        opaque_string(self.handle, &mut arguments)?;
        arguments.extend_from_slice(&start.to_be_bytes());
        arguments.extend_from_slice(&((end - start) as u32).to_be_bytes());
        let deadline = self.session.request_deadline();
        let id = self
            .session
            .send_request(5, &arguments, deadline, self.cancel)?;
        Ok(QueuedRead {
            id,
            start,
            end,
            data: None,
        })
    }

    fn receive_read(&mut self) -> io::Result<()> {
        let deadline = self.session.request_deadline();
        let (id, packet) = self.session.receive_response(deadline, self.cancel)?;
        let read = self
            .reads
            .iter_mut()
            .find(|read| read.id == id && read.data.is_none())
            .ok_or_else(|| invalid("unexpected or duplicate read request ID"))?;
        read.data = Some(packet.read_data((read.end - read.start) as usize)?.to_vec());
        Ok(())
    }

    fn read_next(&mut self, target: &mut [u8]) -> io::Result<usize> {
        let mut next = self.reads.back().map_or(self.offset, |read| read.end);
        while self.reads.len() < READ_WINDOW && next < self.size {
            let end = next.saturating_add(READ_CHUNK_SIZE).min(self.size);
            let read = self.send_read(next, end)?;
            self.reads.push_back(read);
            next = end;
        }

        loop {
            let front = self
                .reads
                .front()
                .expect("read window covers the current offset");
            let Some(data) = &front.data else {
                self.receive_read()?;
                continue;
            };
            let unread = &data[(self.offset - front.start) as usize..];
            if unread.is_empty() {
                // A server may return a short chunk. Request its missing tail
                // before delivering already-buffered chunks at higher offsets.
                let read = self.send_read(self.offset, front.end)?;
                *self.reads.front_mut().expect("nonempty read window") = read;
                continue;
            }
            let count = unread.len().min(target.len());
            target[..count].copy_from_slice(&unread[..count]);
            self.offset += count as u64;
            if self.offset == front.end {
                self.reads.pop_front();
            }
            return Ok(count);
        }
    }

    fn drain(&mut self) -> io::Result<()> {
        if self.failed {
            return Err(invalid("cannot reuse a failed file reader"));
        }
        // Successful previews may stop before consuming the window. Consume
        // outstanding replies before the next synchronous request on the session.
        while self.reads.iter().any(|read| read.data.is_none()) {
            self.receive_read()?;
        }
        Ok(())
    }
}

impl Read for FileReader<'_, '_> {
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        self.cancel.check_read()?;
        if self.failed {
            return Err(invalid("cannot reuse a failed file reader"));
        }
        if target.is_empty() || self.offset == self.size {
            return Ok(0);
        }
        self.read_next(target).map_err(|error| {
            self.failed = true;
            if error.kind() == io::ErrorKind::Interrupted {
                io::Error::other(error.to_string())
            } else {
                error
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    const FIXTURE_SIZE: u64 = READ_CHUNK_SIZE * (READ_WINDOW as u64 + 1) + 177;
    const FIXTURE_ROOT: &str = "/fixture";
    const FIXTURE_FILE: &str = "file";

    fn fixture(mode: &str, size: u64, cancel: &Cancellation<'_>) -> Option<Session> {
        let mut command = Command::new("python3");
        command.args([
            "-c",
            include_str!("read_fixture.py"),
            mode,
            &size.to_string(),
            &READ_WINDOW.to_string(),
            &READ_CHUNK_SIZE.to_string(),
        ]);
        match Session::open(command, cancel) {
            Ok(session) => Some(session),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                eprintln!("SFTP read fixture requires python3");
                None
            }
            Err(error) => panic!("SFTP read fixture failed: {error}"),
        }
    }

    fn fixture_payload(size: u64) -> Vec<u8> {
        (0..size).map(|index| (index % 251) as u8).collect()
    }

    fn read_fixture(
        session: &mut Session,
        cancel: &Cancellation<'_>,
        mut consume: impl FnMut(&FileEntry, &mut dyn Read) -> io::Result<()>,
    ) -> io::Result<()> {
        session.with_file(
            Path::new(FIXTURE_ROOT),
            Path::new(FIXTURE_FILE),
            cancel,
            &mut consume,
        )
    }

    #[test]
    fn decoding_rejects_malformed_fields_and_preserves_native_names() {
        for bytes in [b"".as_slice(), b"..", b"/x", b"a/", b"a/.", b"a\0b", b"a/b"] {
            assert!(child_name(bytes).is_err(), "{bytes:?}");
        }
        assert_eq!(
            child_name(b" \xff\n").unwrap().as_os_str().as_bytes(),
            b" \xff\n"
        );
        assert!(Decoder(&[0, 0, 0]).u32().is_err());
        assert!(Decoder(&[0, 1, 0, 1]).string().is_err());
        assert!(Decoder(&[0, 0, 0, 2, 1]).string().is_err());
        assert!(Attributes::decode(&mut Decoder(&[0, 0, 0, 16])).is_err());
        assert!(Attributes::decode(&mut Decoder(&[0, 0, 0, 1])).is_err());
        let mut opaque = Vec::new();
        opaque_string(&[], &mut opaque).unwrap();
        assert_eq!(Decoder(&opaque).string().unwrap(), b"");
        opaque.clear();
        opaque_string(&[0, 255, 0], &mut opaque).unwrap();
        assert_eq!(Decoder(&opaque).string().unwrap(), &[0, 255, 0]);
    }

    #[test]
    fn malformed_handshakes_fail_without_large_allocations() {
        for script in [
            "printf '\\000\\000\\000\\000'",
            "printf '\\177\\377\\377\\377'",
            "printf '\\000\\000\\000\\005\\002\\000\\000\\000\\004'",
            "printf '\\000\\000\\000\\005\\002\\000'",
        ] {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", script]);
            let cancel = Cancellation {
                requested: None,
                closed: None,
            };
            assert!(Session::open(command, &cancel).is_err(), "{script}");
        }
    }

    #[test]
    fn pipelined_reads_match_reordered_replies_and_fill_short_chunks() {
        let cancel = Cancellation {
            requested: None,
            closed: None,
        };
        let Some(mut session) = fixture("reorder", FIXTURE_SIZE, &cancel) else {
            return;
        };
        let mut received = Vec::new();
        read_fixture(&mut session, &cancel, |_, reader| {
            reader.read_to_end(&mut received)?;
            Ok(())
        })
        .unwrap();
        assert_eq!(received, fixture_payload(FIXTURE_SIZE));
    }

    #[test]
    fn pipelined_reads_reject_duplicate_response_ids() {
        let cancel = Cancellation {
            requested: None,
            closed: None,
        };
        let Some(mut session) = fixture("duplicate", FIXTURE_SIZE, &cancel) else {
            return;
        };
        let error = read_fixture(&mut session, &cancel, |_, reader| {
            reader.read_exact(&mut [0; 17])
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("duplicate read request ID"));
    }

    #[test]
    fn short_consumers_drain_outstanding_reads_before_reusing_the_session() {
        let cancel = Cancellation {
            requested: None,
            closed: None,
        };
        let Some(mut session) = fixture("immediate", FIXTURE_SIZE, &cancel) else {
            return;
        };
        // Both callbacks share the same session, including the closing stat
        // and close requests made after each callback returns.
        for _ in 0..2 {
            let mut received = [0_u8; 17];
            read_fixture(&mut session, &cancel, |_, reader| {
                reader.read_exact(&mut received)
            })
            .unwrap();
            assert_eq!(received.as_slice(), fixture_payload(received.len() as u64));
        }
    }

    #[test]
    fn cancellation_interrupts_pipelined_reads_waiting_for_replies() {
        let flag = AtomicBool::new(false);
        let cancel = Cancellation {
            requested: Some(&flag),
            closed: None,
        };
        let Some(mut session) = fixture("stall", FIXTURE_SIZE, &cancel) else {
            return;
        };
        let started = Instant::now();
        let result = read_fixture(&mut session, &cancel, |_, reader| {
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    std::thread::sleep(Duration::from_millis(50));
                    flag.store(true, Ordering::Relaxed);
                });
                reader.read_exact(&mut [0; 17])
            })
        });
        // Interrupted would be retried by Read::read_exact. The operation
        // boundary converts the nonretrying reader error back to cancellation.
        assert_eq!(result.as_ref().unwrap_err().kind(), io::ErrorKind::Other);
        assert_eq!(
            cancel.finish(result).unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    #[ignore = "controlled 100 ms read latency benchmark"]
    fn sftp_read_latency_benchmark() {
        let size = 1024 * 1024;
        let expected = fixture_payload(size);
        let cancel = Cancellation {
            requested: None,
            closed: None,
        };
        let mut serial_session = fixture("latency", size, &cancel).expect("python3 required");
        let mut serial_data = Vec::new();
        let started = Instant::now();
        for offset in (0..size).step_by(READ_CHUNK_SIZE as usize) {
            let mut arguments = Vec::new();
            opaque_string(b"file", &mut arguments).unwrap();
            arguments.extend_from_slice(&offset.to_be_bytes());
            arguments.extend_from_slice(&(READ_CHUNK_SIZE as u32).to_be_bytes());
            let response = serial_session.request(5, &arguments, &cancel).unwrap();
            serial_data.extend_from_slice(response.read_data(READ_CHUNK_SIZE as usize).unwrap());
        }
        let serial = started.elapsed();
        assert_eq!(serial_data, expected);

        let mut session = fixture("latency", size, &cancel).expect("python3 required");
        let mut pipelined_data = Vec::new();
        let mut pipelined = Duration::ZERO;
        read_fixture(&mut session, &cancel, |_, reader| {
            let started = Instant::now();
            reader.read_to_end(&mut pipelined_data)?;
            pipelined = started.elapsed();
            Ok(())
        })
        .unwrap();
        assert_eq!(pipelined_data, expected);
        println!(
            "1 MiB at 100 ms simulated read latency: serial {serial:.2?}, pipelined {pipelined:.2?}"
        );
        assert!(pipelined < serial / 2);
    }
}
