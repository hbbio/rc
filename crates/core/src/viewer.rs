use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::{FOUNDATION_SLO, VIEWER_TEXT_PREVIEW_LIMIT_BYTES};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ViewerSearchDirection {
    Forward,
    Backward,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ViewerGotoTarget {
    Line(usize),
    Offset(usize),
}

#[derive(Clone, Debug)]
pub struct ViewerState {
    path: PathBuf,
    pub bytes: Vec<u8>,
    content: String,
    text_is_preview: bool,
    content_fingerprint: u64,
    path_fingerprint: u64,
    pub scroll: usize,
    pub wrap: bool,
    pub hex_mode: bool,
    line_offsets: Vec<usize>,
    last_search_query: Option<String>,
    last_search_match_offset: Option<usize>,
    last_search_direction: ViewerSearchDirection,
}

impl ViewerState {
    pub fn open(path: PathBuf) -> io::Result<Self> {
        let total_size = fs::metadata(&path)?.len();
        Self::open_with_reported_size(path, total_size)
    }

    fn open_with_reported_size(path: PathBuf, total_size: u64) -> io::Result<Self> {
        let path_fingerprint = fingerprint(&path);
        let text_limit = FOUNDATION_SLO
            .viewer_memory_soft_limit_bytes
            .clamp(1, VIEWER_TEXT_PREVIEW_LIMIT_BYTES);
        let (bytes, hit_read_limit) = read_file_prefix(&path, text_limit)?;
        let observed_size = if hit_read_limit {
            total_size.max(bytes.len().saturating_add(1) as u64)
        } else {
            total_size.max(bytes.len() as u64)
        };
        let text_is_preview = observed_size > bytes.len() as u64;
        let content_bytes = bytes.as_slice();
        let content = String::from_utf8_lossy(content_bytes).into_owned();
        let hex_mode = should_default_to_hex_mode(&bytes) || text_is_preview;
        let content_fingerprint = if text_is_preview {
            fingerprint(&(observed_size, content.as_str()))
        } else {
            fingerprint(&content)
        };
        let line_offsets = compute_line_offsets(&content);

        Ok(Self {
            path,
            bytes,
            content,
            text_is_preview,
            content_fingerprint,
            path_fingerprint,
            scroll: 0,
            wrap: false,
            hex_mode,
            line_offsets,
            last_search_query: None,
            last_search_match_offset: None,
            last_search_direction: ViewerSearchDirection::Forward,
        })
    }

    #[cfg(test)]
    pub(crate) fn open_with_reported_size_for_test(
        path: PathBuf,
        total_size: u64,
    ) -> io::Result<Self> {
        Self::open_with_reported_size(path, total_size)
    }

    pub fn line_count(&self) -> usize {
        if self.hex_mode {
            self.hex_line_count()
        } else {
            self.line_offsets.len()
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn text_is_preview(&self) -> bool {
        self.text_is_preview
    }

    pub fn content_fingerprint(&self) -> u64 {
        self.content_fingerprint
    }

    pub fn path_fingerprint(&self) -> u64 {
        self.path_fingerprint
    }

    pub fn current_line_number(&self) -> usize {
        self.scroll.saturating_add(1)
    }

    pub fn last_search_query(&self) -> Option<&str> {
        self.last_search_query.as_deref()
    }

    pub fn move_lines(&mut self, delta: isize) {
        let max = self.line_count().saturating_sub(1);
        if delta.is_negative() {
            self.scroll = self.scroll.saturating_sub(delta.unsigned_abs());
        } else {
            self.scroll = self.scroll.saturating_add(delta as usize).min(max);
        }
    }

    pub fn move_pages(&mut self, pages: isize, viewer_page_step: usize) {
        self.move_lines(pages.saturating_mul(viewer_page_step as isize));
    }

    pub fn move_home(&mut self) {
        self.scroll = 0;
    }

    pub fn move_end(&mut self) {
        self.scroll = self.line_count().saturating_sub(1);
    }

    pub fn toggle_wrap(&mut self) {
        self.wrap = !self.wrap;
    }

    pub fn toggle_hex_mode(&mut self) {
        self.hex_mode = !self.hex_mode;
        self.scroll = self.scroll.min(self.line_count().saturating_sub(1));
    }

    pub(crate) fn start_search(
        &mut self,
        query: String,
        direction: ViewerSearchDirection,
    ) -> Option<usize> {
        self.last_search_query = Some(query);
        self.last_search_direction = direction;
        self.last_search_match_offset = None;
        self.continue_search(Some(direction))
    }

    pub(crate) fn continue_search(
        &mut self,
        direction: Option<ViewerSearchDirection>,
    ) -> Option<usize> {
        let query = self.last_search_query.as_deref()?;
        if query.is_empty() {
            return None;
        }
        let direction = direction.unwrap_or(self.last_search_direction);
        let start = match direction {
            ViewerSearchDirection::Forward => self
                .last_search_match_offset
                .map(|offset| offset.saturating_add(query.len()))
                .unwrap_or_else(|| self.current_line_offset()),
            ViewerSearchDirection::Backward => self
                .last_search_match_offset
                .unwrap_or_else(|| self.current_line_offset()),
        };
        let found = match direction {
            ViewerSearchDirection::Forward => find_forward_wrap(&self.content, query, start),
            ViewerSearchDirection::Backward => find_backward_wrap(&self.content, query, start),
        }?;

        self.last_search_match_offset = Some(found);
        self.last_search_direction = direction;
        self.scroll = self.line_index_for_offset(found);
        Some(self.scroll)
    }

    pub(crate) fn goto_input(&mut self, input: &str) -> Result<usize, String> {
        let target = parse_viewer_goto_target(input)?;
        match target {
            ViewerGotoTarget::Line(line) => {
                if line == 0 {
                    return Err(String::from("line numbers start at 1"));
                }
                self.scroll = line
                    .saturating_sub(1)
                    .min(self.line_count().saturating_sub(1));
            }
            ViewerGotoTarget::Offset(offset) => {
                let max_offset = if self.hex_mode {
                    self.bytes.len()
                } else {
                    self.content.len()
                };
                let bounded = offset.min(max_offset);
                self.scroll = self.line_index_for_offset(bounded);
            }
        }
        Ok(self.current_line_number())
    }

    fn current_line_offset(&self) -> usize {
        if self.hex_mode {
            return self
                .scroll
                .saturating_mul(16)
                .min(self.bytes.len().saturating_sub(1));
        }
        let index = self.scroll.min(self.line_count().saturating_sub(1));
        self.line_offsets[index]
    }

    fn line_index_for_offset(&self, offset: usize) -> usize {
        if self.hex_mode {
            return offset
                .saturating_div(16)
                .min(self.hex_line_count().saturating_sub(1));
        }
        if self.line_offsets.is_empty() {
            return 0;
        }
        let bounded = offset.min(self.content.len());
        match self.line_offsets.binary_search(&bounded) {
            Ok(index) => index,
            Err(0) => 0,
            Err(index) => index.saturating_sub(1),
        }
    }

    fn hex_line_count(&self) -> usize {
        let lines = (self.bytes.len().saturating_add(15)).saturating_div(16);
        lines.max(1)
    }
}

fn read_file_prefix(path: &Path, byte_limit: usize) -> io::Result<(Vec<u8>, bool)> {
    let mut file = fs::File::open(path)?;
    let probe_limit = byte_limit.saturating_add(1);
    let mut bytes = Vec::with_capacity(byte_limit);
    file.by_ref()
        .take(probe_limit as u64)
        .read_to_end(&mut bytes)?;
    let hit_read_limit = bytes.len() > byte_limit;
    if hit_read_limit {
        bytes.truncate(byte_limit);
    }
    Ok((bytes, hit_read_limit))
}

fn compute_line_offsets(content: &str) -> Vec<usize> {
    let mut offsets = vec![0];
    for (index, byte) in content.bytes().enumerate() {
        if byte == b'\n' && index.saturating_add(1) < content.len() {
            offsets.push(index + 1);
        }
    }
    offsets
}

fn fingerprint(value: &(impl Hash + ?Sized)) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn should_default_to_hex_mode(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }

    let sample = &bytes[..bytes.len().min(4096)];
    if sample.contains(&0) {
        return true;
    }

    let suspicious = sample
        .iter()
        .filter(|byte| {
            let byte = **byte;
            !(byte.is_ascii_graphic() || matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
        })
        .count();
    suspicious.saturating_mul(100) / sample.len() > 30
}

fn find_forward_wrap(content: &str, query: &str, start: usize) -> Option<usize> {
    let start = start.min(content.len());
    if let Some(relative) = content[start..].find(query) {
        return Some(start + relative);
    }
    if start == 0 {
        return None;
    }
    content[..start].find(query)
}

fn find_backward_wrap(content: &str, query: &str, start: usize) -> Option<usize> {
    let start = start.min(content.len());
    if let Some(index) = content[..start].rfind(query) {
        return Some(index);
    }
    if start >= content.len() {
        return None;
    }
    content[start..]
        .rfind(query)
        .map(|relative| start + relative)
}

fn parse_viewer_goto_target(input: &str) -> Result<ViewerGotoTarget, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(String::from("target is empty"));
    }

    if let Some(rest) = trimmed.strip_prefix('@') {
        let value = rest
            .trim()
            .parse::<usize>()
            .map_err(|_| String::from("invalid decimal offset"))?;
        return Ok(ViewerGotoTarget::Offset(value));
    }

    if let Some(rest) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        let value = usize::from_str_radix(rest.trim(), 16)
            .map_err(|_| String::from("invalid hex offset"))?;
        return Ok(ViewerGotoTarget::Offset(value));
    }

    let lowered = trimmed.to_ascii_lowercase();
    if let Some(rest) = lowered.strip_prefix("line:") {
        let value = rest
            .trim()
            .parse::<usize>()
            .map_err(|_| String::from("invalid line number"))?;
        return Ok(ViewerGotoTarget::Line(value));
    }
    if let Some(rest) = lowered.strip_prefix("offset:") {
        let value = rest
            .trim()
            .parse::<usize>()
            .map_err(|_| String::from("invalid decimal offset"))?;
        return Ok(ViewerGotoTarget::Offset(value));
    }

    let value = trimmed
        .parse::<usize>()
        .map_err(|_| String::from("invalid line number"))?;
    Ok(ViewerGotoTarget::Line(value))
}
