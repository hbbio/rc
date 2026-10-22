//! Bzip2's independently checked blocks form compact seek points. Only offsets
//! and sizes are retained; each reader decodes one block at a time. Block markers
//! may be at any bit alignment, including those produced by pbzip2 streams.
use super::gzip::seek_target;
use super::{check_archive_io_canceled, invalid_data};
use ::bzip2::read::BzDecoder;
use std::io::{self, BufReader, Cursor, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

const BLOCK_MAGIC: u64 = 0x3141_5926_5359;
const END_MAGIC: u64 = 0x1772_4538_5090;
const MASK48: u64 = (1_u64 << 48) - 1;
const CHUNK: usize = 64 * 1024;
// Even maximally inefficient 20-bit Huffman codes for a 900k BWT block fit.
const MAX_COMPRESSED_BLOCK: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
struct Block {
    start: u64,
    end: u64,
    decoded: u64,
    size: u64,
    level: u8,
    crc: u32,
}

pub(super) struct BzipIndex {
    blocks: Vec<Block>,
    length: u64,
}

impl BzipIndex {
    pub fn estimated_bytes(&self) -> usize {
        self.blocks.len() * std::mem::size_of::<Block>()
    }
}

pub(super) struct BzipReader<'a, R> {
    input: BufReader<R>,
    cancel: Option<&'a AtomicBool>,
    blocks: Vec<Block>,
    decoder: Option<Cursor<Vec<u8>>>,
    block: Option<Block>,
    block_number: usize,
    position: u64,
    next_bit: u64,
    level: u8,
    stream_crc: u32,
    building: bool,
    done: bool,
    length: Option<u64>,
}

impl<'a, R: Read + Seek> BzipReader<'a, R> {
    pub fn build(input: R, cancel: Option<&'a AtomicBool>) -> io::Result<Self> {
        let mut input = BufReader::with_capacity(CHUNK, input);
        let mut header = [0_u8; 4];
        input.read_exact(&mut header)?;
        let level = header_level(&header)?;
        Ok(Self {
            input,
            cancel,
            blocks: Vec::new(),
            decoder: None,
            block: None,
            block_number: 0,
            position: 0,
            next_bit: 32,
            level,
            stream_crc: 0,
            building: true,
            done: false,
            length: None,
        })
    }

    pub fn indexed(input: R, index: &BzipIndex, cancel: Option<&'a AtomicBool>) -> Self {
        Self {
            input: BufReader::with_capacity(CHUNK, input),
            cancel,
            blocks: index.blocks.clone(),
            decoder: None,
            block: None,
            block_number: 0,
            position: 0,
            next_bit: 0,
            level: b'9',
            stream_crc: 0,
            building: false,
            done: false,
            length: Some(index.length),
        }
    }

    pub fn finish(mut self) -> io::Result<Arc<BzipIndex>> {
        io::copy(&mut self, &mut io::sink())?;
        Ok(Arc::new(BzipIndex {
            blocks: self.blocks,
            length: self.position,
        }))
    }

    fn load_indexed_block(&mut self, number: usize) -> io::Result<()> {
        if number >= self.blocks.len() {
            self.done = true;
            self.decoder = None;
            return Ok(());
        }
        let block = self.blocks[number].clone();
        self.decoder = None;
        let frame = read_frame(&mut self.input, &block)?;
        let decoded = decode_frame(frame, block.level, self.cancel)?;
        if decoded.len() as u64 != block.size {
            return Err(invalid_data("bzip2 indexed block size mismatch"));
        }
        self.position = block.decoded;
        self.block_number = number;
        self.decoder = Some(Cursor::new(decoded));
        self.block = Some(block);
        self.done = false;
        Ok(())
    }

    fn next_block(&mut self) -> io::Result<()> {
        if !self.building {
            return self.load_indexed_block(self.block_number);
        }
        loop {
            check_archive_io_canceled(self.cancel)?;
            let magic = read_bits(&mut self.input, self.next_bit, 48)?;
            let crc = read_bits(&mut self.input, self.next_bit + 48, 32)? as u32;
            if magic == END_MAGIC {
                if crc != self.stream_crc {
                    return Err(invalid_data("bzip2 stream checksum mismatch"));
                }
                let next_byte = (self.next_bit + 80).div_ceil(8);
                self.input.seek(SeekFrom::Start(next_byte))?;
                let mut header = [0_u8; 4];
                let count = self.input.read(&mut header[..1])?;
                if count == 0 {
                    self.done = true;
                    return Ok(());
                }
                self.input.read_exact(&mut header[1..])?;
                self.level = header_level(&header)?;
                self.stream_crc = 0;
                self.next_bit = next_byte
                    .checked_add(4)
                    .and_then(|p| p.checked_mul(8))
                    .ok_or_else(|| invalid_data("bzip2 offset overflow"))?;
                continue;
            }
            if magic != BLOCK_MAGIC {
                return Err(invalid_data("invalid bzip2 block marker"));
            }
            let (end, decoded) =
                scan_block(&mut self.input, self.next_bit, self.level, crc, self.cancel)?;
            let block = Block {
                start: self.next_bit,
                end,
                decoded: self.position,
                size: 0,
                level: self.level,
                crc,
            };
            self.next_bit = end;
            self.block = Some(block);
            self.decoder = Some(Cursor::new(decoded));
            return Ok(());
        }
    }
}

impl<R: Read + Seek> Read for BzipReader<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        check_archive_io_canceled(self.cancel)?;
        if output.is_empty() || self.done {
            return Ok(0);
        }
        loop {
            let Some(decoder) = self.decoder.as_mut() else {
                self.next_block()?;
                if self.done {
                    return Ok(0);
                }
                continue;
            };
            let count = decoder.read(output)?;
            if count != 0 {
                self.position = self
                    .position
                    .checked_add(count as u64)
                    .ok_or_else(|| invalid_data("bzip2 size overflow"))?;
                return Ok(count);
            }
            let mut block = self
                .block
                .take()
                .expect("an active bzip2 decoder has block metadata");
            let decoded_size = self.position - block.decoded;
            if self.building {
                block.size = decoded_size;
                self.stream_crc = self.stream_crc.rotate_left(1) ^ block.crc;
                self.blocks.push(block);
            } else if decoded_size != block.size {
                return Err(invalid_data("bzip2 indexed block size mismatch"));
            }
            self.block_number += 1;
            self.decoder = None;
        }
    }
}

impl<R: Read + Seek> Seek for BzipReader<'_, R> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        check_archive_io_canceled(self.cancel)?;
        let target = seek_target(self.position, self.length, from)?;
        if target == self.position {
            return Ok(target);
        }
        if let Some(length) = self.length {
            if target >= length {
                if target > length {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "TAR member extends beyond the bzip2 stream",
                    ));
                }
                self.position = target;
                self.done = true;
                self.decoder = None;
                return Ok(target);
            }
            let number = self
                .blocks
                .partition_point(|block| block.decoded <= target)
                .saturating_sub(1);
            if target < self.position || self.decoder.is_none() || number != self.block_number {
                self.load_indexed_block(number)?;
            }
        } else if target < self.position {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "backward seek while building bzip2 index",
            ));
        }
        let mut buffer = [0_u8; CHUNK];
        while self.position < target {
            let count = (target - self.position).min(buffer.len() as u64) as usize;
            if self.read(&mut buffer[..count])? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "TAR member extends beyond the bzip2 stream",
                ));
            }
        }
        Ok(self.position)
    }
}

fn header_level(header: &[u8; 4]) -> io::Result<u8> {
    if header[..3] != *b"BZh" || !matches!(header[3], b'1'..=b'9') {
        return Err(invalid_data("invalid bzip2 stream header"));
    }
    Ok(header[3])
}

fn read_bits<R: Read + Seek>(reader: &mut R, bit: u64, count: usize) -> io::Result<u64> {
    reader.seek(SeekFrom::Start(bit / 8))?;
    let offset = (bit % 8) as usize;
    let length = (offset + count).div_ceil(8);
    let mut data = [0_u8; 8];
    reader.read_exact(&mut data[8 - length..])?;
    Ok((u64::from_be_bytes(data) >> (length * 8 - offset - count)) & ((1_u64 << count) - 1))
}

// A 48-bit marker can occur incidentally in compressed payloads. Validate each
// candidate's block CRC before exposing bytes; false candidates are extended to
// the next marker. Retain one decoded block, never the complete TAR stream.
fn scan_block<R: Read + Seek>(
    reader: &mut R,
    start: u64,
    level: u8,
    crc: u32,
    cancel: Option<&AtomicBool>,
) -> io::Result<(u64, Vec<u8>)> {
    reader.seek(SeekFrom::Start(start / 8))?;
    let mut data = Vec::new();
    let mut chunk = [0_u8; CHUNK];
    let mut search = (start % 8 + 48) as usize;
    let mut last_error = None;
    loop {
        check_archive_io_canceled(cancel)?;
        if let Some(end) = find_marker(&data, search) {
            let bits = end - (start % 8) as usize;
            let encoded = frame(&data, (start % 8) as usize, bits, level, crc);
            match decode_frame(encoded, level, cancel) {
                Ok(decoded) => return Ok(((start / 8) * 8 + end as u64, decoded)),
                Err(error) => last_error = Some(error),
            }
            search = end + 1;
            continue;
        }
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            return Err(last_error.unwrap_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "unterminated bzip2 block")
            }));
        }
        search = search.max(data.len().saturating_sub(6) * 8);
        data.extend_from_slice(&chunk[..count]);
        if data.len() > MAX_COMPRESSED_BLOCK {
            return Err(
                last_error.unwrap_or_else(|| invalid_data("bzip2 block exceeds format limits"))
            );
        }
    }
}

fn decode_frame(frame: Vec<u8>, level: u8, cancel: Option<&AtomicBool>) -> io::Result<Vec<u8>> {
    let mut decoder = BzDecoder::new(Cursor::new(frame));
    // RLE1 can expand a BWT block by at most 259/5. Most blocks are <=900k;
    // even an extreme run-length block stays bounded to about 47 MB.
    let limit = usize::from(level - b'0') * 100_000 * 52;
    let mut decoded = Vec::new();
    let mut buffer = [0_u8; CHUNK];
    loop {
        check_archive_io_canceled(cancel)?;
        let count = decoder.read(&mut buffer)?;
        if count == 0 {
            return Ok(decoded);
        }
        if decoded.len() + count > limit {
            return Err(invalid_data("bzip2 decoded block exceeds format limits"));
        }
        if decoded.len() + count > decoded.capacity() {
            let capacity = (decoded.capacity().max(CHUNK) * 2).min(limit);
            decoded
                .try_reserve_exact(capacity - decoded.len())
                .map_err(io::Error::other)?;
        }
        decoded.extend_from_slice(&buffer[..count]);
    }
}

// SIMD memchr finds candidate full bytes for each of the eight possible bit
// alignments; a 48-bit comparison then rejects incidental prefix matches.
fn find_marker(data: &[u8], from_bit: usize) -> Option<usize> {
    let mut first = None;
    for magic in [BLOCK_MAGIC, END_MAGIC] {
        for alignment in 0..8 {
            let delta = usize::from(alignment != 0);
            let prefix = (magic >> if alignment == 0 { 40 } else { 32 + alignment }) as u8;
            let start = (from_bit / 8 + delta).min(data.len());
            for position in memchr::memchr_iter(prefix, &data[start..]) {
                let byte = start + position - delta;
                let bit = byte * 8 + alignment;
                if bit < from_bit {
                    continue;
                }
                if first.is_some_and(|first| bit >= first) {
                    break;
                }
                let Some(window) = data.get(byte..byte + 7) else {
                    break;
                };
                let mut bytes = [0_u8; 8];
                bytes[1..].copy_from_slice(window);
                if ((u64::from_be_bytes(bytes) >> (8 - alignment)) & MASK48) == magic {
                    first = Some(bit);
                    break;
                }
            }
        }
    }
    first
}

fn read_frame<R: Read + Seek>(reader: &mut R, block: &Block) -> io::Result<Vec<u8>> {
    let bits = usize::try_from(block.end - block.start).map_err(io::Error::other)?;
    let offset = (block.start % 8) as usize;
    let length = (offset + bits).div_ceil(8);
    if length > MAX_COMPRESSED_BLOCK + CHUNK {
        return Err(invalid_data("invalid indexed bzip2 block size"));
    }
    reader.seek(SeekFrom::Start(block.start / 8))?;
    let mut data = vec![0_u8; length + 1];
    reader.read_exact(&mut data[..length])?;
    Ok(frame(&data, offset, bits, block.level, block.crc))
}

fn frame(data: &[u8], offset: usize, bits: usize, level: u8, crc: u32) -> Vec<u8> {
    let mut output = vec![0_u8; 4 + (bits + 80).div_ceil(8)];
    output[..4].copy_from_slice(&[b'B', b'Z', b'h', level]);
    for index in 0..bits.div_ceil(8) {
        let mut byte = data[index] << offset;
        if offset != 0 {
            byte |= data.get(index + 1).copied().unwrap_or(0) >> (8 - offset);
        }
        if index * 8 + 8 > bits {
            byte &= 0xff << (8 - bits % 8);
        }
        output[4 + index] = byte;
    }
    for (value, count, start) in [
        (END_MAGIC, 48, 32 + bits),
        (u64::from(crc), 32, 32 + bits + 48),
    ] {
        for bit in 0..count {
            if value & (1 << (count - bit - 1)) != 0 {
                let position = start + bit;
                output[position / 8] |= 1 << (7 - position % 8);
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::normalize_archive_io_result;
    use std::io::Write;
    use std::sync::atomic::Ordering;

    fn data() -> Vec<u8> {
        let mut seed = 0x1234_5678_u32;
        (0..1_100_000)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect()
    }
    fn encode(data: &[u8], level: u32) -> Vec<u8> {
        let mut encoder =
            ::bzip2::write::BzEncoder::new(Vec::new(), ::bzip2::Compression::new(level));
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn block_index_seeks_at_non_byte_boundaries_without_decoding_preceding_blocks() {
        let data = data();
        for level in [1, 9] {
            let compressed = encode(&data, level);
            let index = BzipReader::build(Cursor::new(&compressed), None)
                .unwrap()
                .finish()
                .unwrap();
            assert!(index.blocks.len() >= 2);
            assert!(index.blocks.iter().any(|block| block.start % 8 != 0));
            let mut damaged = compressed;
            damaged[20] ^= 0xff;
            let mut reader = BzipReader::indexed(Cursor::new(&damaged), &index, None);
            let offset = data.len() - 317;
            reader.seek(SeekFrom::Start(offset as u64)).unwrap();
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, data[offset..]);
            let boundary = index.blocks[1].decoded;
            reader.seek(SeekFrom::Start(boundary)).unwrap();
            let mut bytes = [0_u8; 57];
            reader.read_exact(&mut bytes).unwrap();
            assert_eq!(bytes, data[boundary as usize..][..57]);
            let mut sequential = BzDecoder::new(Cursor::new(damaged));
            assert!(io::copy(&mut sequential, &mut io::sink()).is_err());
        }
    }

    #[test]
    fn block_reader_checks_data_and_stream_checksums_and_truncation() {
        let data = data();
        let compressed = encode(&data, 1);
        for offset in [15, compressed.len() - 2] {
            let mut damaged = compressed.clone();
            damaged[offset] ^= 0xff;
            assert!(
                BzipReader::build(Cursor::new(damaged), None)
                    .unwrap()
                    .finish()
                    .is_err()
            );
        }
        let mut truncated = compressed;
        truncated.truncate(truncated.len() / 2);
        assert!(
            BzipReader::build(Cursor::new(truncated), None)
                .unwrap()
                .finish()
                .is_err()
        );
    }

    #[test]
    fn buffered_block_and_seek_cancellation_is_non_retryable() {
        let compressed = encode(&vec![b'x'; 1024 * 1024], 1);
        let cancel = AtomicBool::new(false);
        let mut reader = BzipReader::build(Cursor::new(compressed), Some(&cancel)).unwrap();
        reader.read_exact(&mut [0_u8; 9]).unwrap();
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            reader.read_exact(&mut [0_u8; 100]).unwrap_err().kind(),
            io::ErrorKind::Other
        );
        let result = reader.seek(SeekFrom::Start(500));
        assert_eq!(result.as_ref().unwrap_err().kind(), io::ErrorKind::Other);
        assert_eq!(
            normalize_archive_io_result(result, Some(&cancel))
                .unwrap_err()
                .kind(),
            io::ErrorKind::Interrupted
        );
    }

    #[test]
    fn marker_scanner_finds_both_markers_at_every_bit_alignment_across_chunks() {
        for magic in [BLOCK_MAGIC, END_MAGIC] {
            for alignment in 0..8 {
                let bit = (CHUNK - 3) * 8 + alignment;
                let mut data = vec![0_u8; CHUNK + 16];
                for index in 0..48 {
                    if magic & (1 << (47 - index)) != 0 {
                        data[(bit + index) / 8] |= 1 << (7 - (bit + index) % 8);
                    }
                }
                assert_eq!(find_marker(&data, (CHUNK - 7) * 8), Some(bit));
                assert_eq!(find_marker(&data, bit + 1), None);
            }
        }
    }
}
