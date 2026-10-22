//! Seekable gzip using bounded snapshots of the existing miniz decoder. Unlike
//! block-only indexes this also bounds seeks for highly compressible inputs.
//! Snapshots include checksum state; the first pass validates every gzip member.
use super::*;
use miniz_oxide::inflate::stream::{InflateState, inflate};
use miniz_oxide::{DataFormat, MZError, MZFlush, MZStatus};
use std::io::{BufRead, BufReader};

const INITIAL_SPAN: u64 = 4 * 1024 * 1024;
const MAX_POINTS: usize = 256;
const BUFFER_SIZE: usize = 64 * 1024;

#[derive(Clone)]
struct Point {
    compressed: u64,
    decoded: u64,
    state: Arc<InflateState>,
    crc: crc32fast::Hasher,
    member_size: u32,
    member_finished: bool,
}

pub(super) struct GzipIndex {
    points: Vec<Point>,
    length: u64,
}

impl GzipIndex {
    pub fn estimated_bytes(&self) -> usize {
        self.points.len() * (std::mem::size_of::<InflateState>() + std::mem::size_of::<Point>())
    }
}

pub(super) struct GzipReader<'a, R> {
    input: BufReader<R>,
    cancel: Option<&'a AtomicBool>,
    state: Box<InflateState>,
    position: u64,
    crc: crc32fast::Hasher,
    member_size: u32,
    member_finished: bool,
    done: bool,
    points: Vec<Point>,
    building: bool,
    length: Option<u64>,
    span: u64,
    next_point: u64,
}

impl<'a, R: Read + Seek> GzipReader<'a, R> {
    pub fn build(input: R, cancel: Option<&'a AtomicBool>) -> io::Result<Self> {
        let mut input = BufReader::with_capacity(BUFFER_SIZE, input);
        read_header(&mut input)?;
        let mut reader = Self {
            input,
            cancel,
            state: InflateState::new_boxed(DataFormat::Raw),
            position: 0,
            crc: crc32fast::Hasher::new(),
            member_size: 0,
            member_finished: false,
            done: false,
            points: Vec::new(),
            building: true,
            length: None,
            span: INITIAL_SPAN,
            next_point: INITIAL_SPAN,
        };
        reader.save_point()?;
        Ok(reader)
    }

    pub fn indexed(
        input: R,
        index: &GzipIndex,
        cancel: Option<&'a AtomicBool>,
    ) -> io::Result<Self> {
        let mut reader = Self {
            input: BufReader::with_capacity(BUFFER_SIZE, input),
            cancel,
            state: InflateState::new_boxed(DataFormat::Raw),
            position: 0,
            crc: crc32fast::Hasher::new(),
            member_size: 0,
            member_finished: false,
            done: false,
            points: index.points.clone(),
            building: false,
            length: Some(index.length),
            span: INITIAL_SPAN,
            next_point: u64::MAX,
        };
        reader.restore(0)?;
        Ok(reader)
    }

    pub fn finish(mut self) -> io::Result<Arc<GzipIndex>> {
        io::copy(&mut self, &mut io::sink())?;
        Ok(Arc::new(GzipIndex {
            points: self.points,
            length: self.position,
        }))
    }

    fn save_point(&mut self) -> io::Result<()> {
        if self
            .points
            .last()
            .is_some_and(|point| point.decoded == self.position)
        {
            return Ok(());
        }
        self.points.push(Point {
            compressed: self.input.stream_position()?,
            decoded: self.position,
            state: Arc::from(self.state.clone()),
            crc: self.crc.clone(),
            member_size: self.member_size,
            member_finished: self.member_finished,
        });
        if self.points.len() >= MAX_POINTS {
            // Adapt to archive size instead of allowing checkpoint memory to
            // grow with the decompressed size. Keep the starting snapshot.
            let mut number = 0;
            self.points.retain(|_| {
                let keep = number % 2 == 0;
                number += 1;
                keep
            });
            self.span = self.span.saturating_mul(2);
        }
        self.next_point = self.position.saturating_add(self.span);
        Ok(())
    }

    fn restore(&mut self, point: usize) -> io::Result<()> {
        let point = &self.points[point];
        self.input.seek(SeekFrom::Start(point.compressed))?;
        self.state.as_mut().clone_from(point.state.as_ref());
        self.position = point.decoded;
        self.crc = point.crc.clone();
        self.member_size = point.member_size;
        self.member_finished = point.member_finished;
        self.done = false;
        Ok(())
    }

    fn finish_member(&mut self) -> io::Result<()> {
        let mut trailer = [0_u8; 8];
        self.input.read_exact(&mut trailer)?;
        let crc = u32::from_le_bytes(trailer[..4].try_into().unwrap());
        let size = u32::from_le_bytes(trailer[4..].try_into().unwrap());
        if crc != self.crc.clone().finalize() || size != self.member_size {
            return Err(invalid_data("gzip checksum or size mismatch"));
        }
        // Accept trailing zero padding and concatenated gzip streams.
        loop {
            let input = self.input.fill_buf()?;
            let zeros = input.iter().take_while(|byte| **byte == 0).count();
            if zeros == 0 {
                break;
            }
            self.input.consume(zeros);
        }
        if self.input.fill_buf()?.is_empty() {
            self.done = true;
            return Ok(());
        }
        read_header(&mut self.input)?;
        self.state.reset(DataFormat::Raw);
        self.crc = crc32fast::Hasher::new();
        self.member_size = 0;
        self.member_finished = false;
        Ok(())
    }
}

impl<R: Read + Seek> Read for GzipReader<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        check_archive_io_canceled(self.cancel)?;
        if output.is_empty() || self.done {
            return Ok(0);
        }
        loop {
            check_archive_io_canceled(self.cancel)?;
            if self.member_finished {
                self.finish_member()?;
                if self.done {
                    return Ok(0);
                }
            }
            if self.building && self.position >= self.next_point {
                self.save_point()?;
            }
            let input = self.input.fill_buf()?;
            let input_empty = input.is_empty();
            let result = inflate(&mut self.state, input, output, MZFlush::None);
            self.input.consume(result.bytes_consumed);
            match result.status {
                Ok(MZStatus::StreamEnd) => self.member_finished = true,
                Ok(_) | Err(MZError::Buf) => {}
                Err(_) => return Err(invalid_data("invalid gzip DEFLATE data")),
            }
            let count = result.bytes_written;
            self.crc.update(&output[..count]);
            self.member_size = self.member_size.wrapping_add(count as u32);
            self.position = self
                .position
                .checked_add(count as u64)
                .ok_or_else(|| invalid_data("gzip size overflow"))?;
            if count > 0 {
                return Ok(count);
            }
            if self.member_finished {
                continue;
            }
            if result.bytes_consumed == 0 {
                return Err(io::Error::new(
                    if input_empty {
                        io::ErrorKind::UnexpectedEof
                    } else {
                        io::ErrorKind::InvalidData
                    },
                    "gzip decoder could not finish the stream",
                ));
            }
        }
    }
}

impl<R: Read + Seek> Seek for GzipReader<'_, R> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        check_archive_io_canceled(self.cancel)?;
        let target = seek_target(self.position, self.length, from)?;
        if target == self.position {
            return Ok(target);
        }
        let nearest = self
            .points
            .partition_point(|point| point.decoded <= target)
            .saturating_sub(1);
        if target < self.position
            || (!self.building && self.points[nearest].decoded > self.position)
        {
            self.restore(nearest)?;
        }
        skip_to(self, target)?;
        Ok(self.position)
    }
}

pub(super) fn seek_target(position: u64, length: Option<u64>, from: SeekFrom) -> io::Result<u64> {
    let (base, offset) = match from {
        SeekFrom::Start(position) => return Ok(position),
        SeekFrom::Current(offset) => (position, offset),
        SeekFrom::End(offset) => (
            length.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    "archive length is not yet indexed",
                )
            })?,
            offset,
        ),
    };
    base.checked_add_signed(offset)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid archive seek"))
}

fn skip_to<R: Read + Seek>(reader: &mut GzipReader<'_, R>, target: u64) -> io::Result<()> {
    let mut buffer = [0_u8; BUFFER_SIZE];
    while reader.position < target {
        let count = (target - reader.position).min(buffer.len() as u64) as usize;
        if reader.read(&mut buffer[..count])? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "TAR member extends beyond the gzip stream",
            ));
        }
    }
    Ok(())
}

// Header metadata is skipped in bounded buffers. Names/comments can be large;
// they are never retained in checkpoints and cannot cause unbounded allocation.
fn read_header<R: BufRead>(reader: &mut R) -> io::Result<()> {
    let mut header = [0_u8; 10];
    reader.read_exact(&mut header)?;
    if header[..3] != [0x1f, 0x8b, 8] || header[3] & 0xe0 != 0 {
        return Err(invalid_data("invalid gzip header"));
    }
    let flags = header[3];
    let mut crc = crc32fast::Hasher::new();
    crc.update(&header);
    if flags & 4 != 0 {
        let mut size = [0_u8; 2];
        reader.read_exact(&mut size)?;
        crc.update(&size);
        let mut remaining = usize::from(u16::from_le_bytes(size));
        let mut buffer = [0_u8; 4096];
        while remaining > 0 {
            let count = remaining.min(buffer.len());
            reader.read_exact(&mut buffer[..count])?;
            crc.update(&buffer[..count]);
            remaining -= count;
        }
    }
    for flag in [8, 16] {
        if flags & flag != 0 {
            loop {
                let buffer = reader.fill_buf()?;
                if buffer.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "unterminated gzip header",
                    ));
                }
                let end = memchr::memchr(0, buffer);
                let count = end.map_or(buffer.len(), |end| end + 1);
                crc.update(&buffer[..count]);
                reader.consume(count);
                if end.is_some() {
                    break;
                }
            }
        }
    }
    if flags & 2 != 0 {
        let mut stored = [0_u8; 2];
        reader.read_exact(&mut stored)?;
        if u16::from_le_bytes(stored) != crc.finalize() as u16 {
            return Err(invalid_data("gzip header checksum mismatch"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use std::sync::atomic::AtomicUsize;

    fn encode(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn checkpoint_seek_skips_corrupt_preceding_data_and_checks_the_trailer() {
        let data = (0..3 * INITIAL_SPAN as usize + 937)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let compressed = encode(&data);
        let index = GzipReader::build(Cursor::new(&compressed), None)
            .unwrap()
            .finish()
            .unwrap();
        assert!(index.points.len() >= 3);
        // Deliberately damage the beginning AFTER building the index. A read
        // near EOF can only succeed if it actually resumes a later checkpoint.
        let mut damaged = compressed;
        damaged[12] ^= 0xff;
        let mut reader = GzipReader::indexed(Cursor::new(&damaged), &index, None).unwrap();
        let offset = data.len() - 317;
        reader.seek(SeekFrom::Start(offset as u64)).unwrap();
        let mut output = Vec::new();
        reader.read_to_end(&mut output).unwrap();
        assert_eq!(output, data[offset..]);
        reader.seek(SeekFrom::Start(INITIAL_SPAN + 193)).unwrap();
        let mut output = [0_u8; 97];
        reader.read_exact(&mut output).unwrap();
        assert_eq!(output, data[INITIAL_SPAN as usize + 193..][..97]);
        let mut sequential = flate2::read::GzDecoder::new(Cursor::new(damaged));
        assert!(io::copy(&mut sequential, &mut io::sink()).is_err());
    }

    #[test]
    fn checkpoint_memory_is_bounded_by_compacting_older_points() {
        let compressed = encode(&vec![b'x'; 2 * 1024 * 1024]);
        let mut builder = GzipReader::build(Cursor::new(compressed), None).unwrap();
        // Exercise compaction with a small span, without a multi-gigabyte fixture.
        builder.span = 1024;
        builder.next_point = 1024;
        let mut buffer = [0_u8; 1024];
        while builder.read(&mut buffer).unwrap() != 0 {}
        assert!(builder.span > 1024);
        assert!(builder.points.len() < MAX_POINTS);
        let index = builder.finish().unwrap();
        let mut reader = GzipReader::indexed(
            Cursor::new(encode(&vec![b'x'; 2 * 1024 * 1024])),
            &index,
            None,
        )
        .unwrap();
        for offset in [5, 170_000, 700_000, 2_000_000, 0] {
            reader.seek(SeekFrom::Start(offset)).unwrap();
            let mut buffer = [0_u8; 211];
            reader.read_exact(&mut buffer).unwrap();
            assert_eq!(buffer, [b'x'; 211]);
        }
    }

    struct CancelOnRead<'a> {
        input: Cursor<Vec<u8>>,
        cancel: &'a AtomicBool,
        calls: &'a AtomicUsize,
    }
    impl Read for CancelOnRead<'_> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            assert!(
                self.calls.fetch_add(1, Ordering::Relaxed) < 4,
                "canceled gzip read was retried"
            );
            let count = self.input.read(&mut output[..1])?;
            self.cancel.store(true, Ordering::Relaxed);
            Ok(count)
        }
    }
    impl Seek for CancelOnRead<'_> {
        fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
            self.input.seek(from)
        }
    }

    #[test]
    fn cancellation_during_header_and_with_buffered_payload_is_non_retryable() {
        let compressed = encode(&vec![b'x'; 1024 * 1024]);
        let cancel = AtomicBool::new(false);
        let calls = AtomicUsize::new(0);
        let input = CancelReader {
            file: CancelOnRead {
                input: Cursor::new(compressed.clone()),
                cancel: &cancel,
                calls: &calls,
            },
            cancel: Some(&cancel),
        };
        let result = GzipReader::build(input, Some(&cancel));
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::Other);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        cancel.store(false, Ordering::Relaxed);
        let mut reader = GzipReader::build(Cursor::new(compressed), Some(&cancel)).unwrap();
        reader.read_exact(&mut [0_u8; 9]).unwrap();
        cancel.store(true, Ordering::Relaxed);
        let result = reader.read_exact(&mut [0_u8; 100]);
        assert_eq!(result.as_ref().unwrap_err().kind(), io::ErrorKind::Other);
        assert_eq!(
            normalize_archive_io_result(result, Some(&cancel))
                .unwrap_err()
                .kind(),
            io::ErrorKind::Interrupted
        );
    }

    #[test]
    fn gzip_optional_header_fields_crc_concatenation_and_truncation_are_checked() {
        let mut header = vec![0x1f, 0x8b, 8, 2 | 4 | 8 | 16, 0, 0, 0, 0, 0, 3];
        header.extend_from_slice(&[3, 0, 1, 2, 3]);
        header.extend_from_slice(b"file.tar\0a comment\0");
        let crc = crc32fast::hash(&header) as u16;
        header.extend_from_slice(&crc.to_le_bytes());
        header.extend_from_slice(&encode(b"first")[10..]);
        header.extend(encode(b"second"));
        let mut reader = GzipReader::build(Cursor::new(&header), None).unwrap();
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"firstsecond");
        let mut corrupt = header.clone();
        corrupt[15] ^= 1;
        assert!(GzipReader::build(Cursor::new(corrupt), None).is_err());
        header.pop();
        assert!(
            GzipReader::build(Cursor::new(header), None)
                .unwrap()
                .finish()
                .is_err()
        );
    }
}
