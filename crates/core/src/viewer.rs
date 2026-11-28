use std::fmt::Write as _;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::mpsc::Sender;
#[cfg(any(windows, test))]
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use memchr::memmem;

#[cfg(windows)]
use rc_platform::is_symlink_loop;

use crate::archive::SourceIdentity;
use crate::{
    BackgroundEvent, FOUNDATION_SLO, JOB_CANCELED_MESSAGE, JobError, JobEvent, JobId, JobRequest,
    MAX_CLIPBOARD_TEXT_BYTES, VIEWER_TEXT_PREVIEW_LIMIT_BYTES, WorkerJob,
};

const VIEWER_READ_CHUNK_BYTES: usize = 64 * 1024;
const VIEWER_HEX_ROW_BYTES: usize = 16;
// Eight offset digits, two spaces, three columns per byte, " |", ASCII, "|\n".
const HEX_COPY_ROW_BYTES: usize = 8 + 2 + 3 * VIEWER_HEX_ROW_BYTES + 2 + VIEWER_HEX_ROW_BYTES + 2;
#[cfg(any(windows, test))]
const CONTENT_CHECK_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Default)]
struct NroffState {
    escape: NroffEscape,
}

#[derive(Clone, Copy, Debug, Default)]
enum NroffEscape {
    #[default]
    Text,
    Escape,
    Csi,
    Osc,
    OscEscape,
}

impl NroffState {
    fn append(&mut self, content: &mut String, text: &str, dirty_start: &mut usize) {
        for ch in text.chars() {
            match self.escape {
                NroffEscape::Escape => {
                    self.escape = match ch {
                        '[' => NroffEscape::Csi,
                        ']' => NroffEscape::Osc,
                        _ => NroffEscape::Text,
                    };
                }
                NroffEscape::Csi => {
                    if ('@'..='~').contains(&ch) {
                        self.escape = NroffEscape::Text;
                    }
                }
                NroffEscape::Osc => {
                    if ch == '\u{7}' {
                        self.escape = NroffEscape::Text;
                    } else if ch == '\u{1b}' {
                        self.escape = NroffEscape::OscEscape;
                    }
                }
                NroffEscape::OscEscape => {
                    self.escape = if ch == '\\' {
                        NroffEscape::Text
                    } else {
                        NroffEscape::Osc
                    };
                }
                NroffEscape::Text => match ch {
                    '\u{1b}' => self.escape = NroffEscape::Escape,
                    '\u{8}' if !content.ends_with('\n') => {
                        content.pop();
                        *dirty_start = (*dirty_start).min(content.len());
                    }
                    '\u{8}' | '\r' => {}
                    _ => content.push(ch),
                },
            }
        }
    }
}

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

#[derive(Debug)]
pub struct ViewerFileSnapshot {
    path: PathBuf,
    identity: SourceIdentity,
    #[cfg(windows)]
    change_time: Option<i64>,
    #[cfg(windows)]
    contents: ViewerFileContents,
}

impl ViewerFileSnapshot {
    pub fn has_changed(&self) -> bool {
        self.check_disk(true)
    }

    /// Check metadata and run a content comparison only when it is due.
    pub fn poll_for_change(&self) -> bool {
        self.check_disk(false)
    }

    fn check_disk(&self, force_content_check: bool) -> bool {
        self.compare_to_disk(force_content_check)
            .unwrap_or_else(|error| {
                matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) || is_symlink_loop(&error)
            })
    }

    fn compare_to_disk(&self, force_content_check: bool) -> io::Result<bool> {
        #[cfg(windows)]
        let file = rc_platform::open_file_for_metadata(&self.path)?;
        #[cfg(windows)]
        let metadata = file.metadata()?;
        #[cfg(not(windows))]
        let metadata = fs::metadata(&self.path)?;
        if !self.matches_metadata(&metadata) {
            return Ok(true);
        }
        #[cfg(windows)]
        let changed = {
            if let Some(loaded) = self.change_time
                && let Some(current) = rc_platform::file_change_time(&file)
            {
                return Ok(current != loaded);
            }
            // FAT and exFAT lack an independent change timestamp. Compare the
            // original preview bytes at a lower frequency than metadata checks.
            if !force_content_check && !self.contents.begin_automatic_check(Instant::now()) {
                return Ok(false);
            }
            let input = crate::vfs::open_local_file(&self.path)?;
            !self.matches_metadata(&input.metadata()?) || self.contents.has_changed(input)?
        };
        #[cfg(not(windows))]
        let changed = {
            let _ = force_content_check;
            false
        };
        Ok(changed)
    }

    fn matches_metadata(&self, metadata: &fs::Metadata) -> bool {
        metadata.is_file() && self.identity == SourceIdentity::new(metadata)
    }
}

#[cfg(any(windows, test))]
#[derive(Debug)]
struct ViewerFileContents {
    length: usize,
    fingerprint: u64,
    next_automatic_check: Mutex<Instant>,
}

#[cfg(any(windows, test))]
impl ViewerFileContents {
    fn new(bytes: &[u8], loaded_at: Instant) -> Self {
        Self {
            length: bytes.len(),
            fingerprint: preview_fingerprint(bytes, bytes.len())
                .expect("the loaded bytes are available"),
            next_automatic_check: Mutex::new(loaded_at + CONTENT_CHECK_INTERVAL),
        }
    }

    fn begin_automatic_check(&self, now: Instant) -> bool {
        let mut next_check = self
            .next_automatic_check
            .lock()
            .expect("viewer content check schedule lock");
        if now < *next_check {
            return false;
        }
        *next_check = now + CONTENT_CHECK_INTERVAL;
        true
    }

    fn has_changed(&self, input: impl Read) -> io::Result<bool> {
        match preview_fingerprint(input, self.length) {
            Ok(fingerprint) => Ok(fingerprint != self.fingerprint),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(true),
            Err(error) => Err(error),
        }
    }
}

#[cfg(any(windows, test))]
fn preview_fingerprint(mut input: impl Read, mut remaining: usize) -> io::Result<u64> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut buffer = [0; VIEWER_READ_CHUNK_BYTES];
    while remaining != 0 {
        let chunk = remaining.min(buffer.len());
        input.read_exact(&mut buffer[..chunk])?;
        hasher.write(&buffer[..chunk]);
        remaining -= chunk;
    }
    Ok(hasher.finish())
}

#[cfg(unix)]
fn is_symlink_loop(error: &io::Error) -> bool {
    error.raw_os_error() == Some(nix::libc::ELOOP)
}

#[cfg(not(any(unix, windows)))]
fn is_symlink_loop(_: &io::Error) -> bool {
    false
}

#[derive(Clone, Debug)]
pub struct ViewerState {
    path: PathBuf,
    pub(crate) local_file: Option<Box<ViewerLocalFile>>,
    pub(crate) archive_location: Option<crate::ArchiveLocation>,
    pub(crate) mounted_location: Option<Box<crate::vfs::MountedPath>>,
    output: Option<Box<ViewerOutputState>>,
    nroff: Option<NroffState>,
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

#[derive(Clone, Debug)]
pub(crate) struct ViewerLocalFile {
    pub snapshot: Arc<ViewerFileSnapshot>,
    pub updated_on_disk: bool,
    pub change_detection_unavailable: bool,
    pub refresh_job: Option<JobId>,
}

#[derive(Clone, Debug)]
struct ViewerOutputState {
    status: String,
    utf8_tail: Vec<u8>,
    content_hasher: std::collections::hash_map::DefaultHasher,
}

impl ViewerState {
    pub fn open(path: PathBuf) -> io::Result<Self> {
        Self::open_local(path, None, None)
    }

    pub fn open_cancellable(path: PathBuf, cancel_flag: &AtomicBool) -> io::Result<Self> {
        Self::open_local(path, Some(cancel_flag), None)
    }

    fn open_local(
        path: PathBuf,
        cancel_flag: Option<&AtomicBool>,
        reported_size: Option<u64>,
    ) -> io::Result<Self> {
        ensure_viewer_not_canceled(cancel_flag)?;
        ensure_regular_file(&fs::metadata(&path)?)?;
        let mut file = crate::vfs::open_local_file(&path)?;
        // Use the opened handle's metadata so a replacement between stat and
        // open cannot pair one version's bytes with another version's identity.
        let metadata = file.metadata()?;
        ensure_regular_file(&metadata)?;
        #[cfg(windows)]
        let change_time = if rc_platform::volume_supports_change_time(&file) {
            rc_platform::file_change_time(&file)
        } else {
            None
        };
        let text_limit = FOUNDATION_SLO
            .viewer_memory_soft_limit_bytes
            .clamp(1, VIEWER_TEXT_PREVIEW_LIMIT_BYTES);
        let (bytes, hit_read_limit) = read_prefix(&mut file, text_limit, cancel_flag)?;
        let file_snapshot = Arc::new(ViewerFileSnapshot {
            path: path.clone(),
            identity: SourceIdentity::new(&metadata),
            #[cfg(windows)]
            change_time,
            #[cfg(windows)]
            contents: ViewerFileContents::new(&bytes, Instant::now()),
        });
        let total_size = reported_size.unwrap_or(metadata.len());
        let mut viewer = Self::from_prefix(path, total_size, bytes, hit_read_limit);
        viewer.local_file = Some(Box::new(ViewerLocalFile {
            snapshot: file_snapshot,
            updated_on_disk: false,
            change_detection_unavailable: false,
            refresh_job: None,
        }));
        Ok(viewer)
    }

    /// Loads a bounded preview of a regular archive member synchronously.
    ///
    /// `path` must be `location.archive.join(member)`, with `member` relative to
    /// the archive root. The returned viewer retains that synthetic identity.
    /// Preview bytes are limited by the viewer memory budget, but building the
    /// shared index for a compressed TAR can first decompress its entire stream.
    /// Nested containers may use private temporary files; the viewed member
    /// itself is read directly into the bounded preview buffer.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Interrupted`] when canceled,
    /// [`io::ErrorKind::NotFound`] for an absent member, and
    /// [`io::ErrorKind::Unsupported`] for directories or unsupported member types.
    /// Invalid paths, unreadable or malformed archives, and source changes also
    /// return errors rather than a partially loaded viewer.
    pub fn open_archive_cancellable(
        location: &crate::ArchiveLocation,
        path: PathBuf,
        cancel: &AtomicBool,
    ) -> io::Result<Self> {
        Self::open_vfs_cancellable(
            crate::vfs::VfsPath::Archive {
                location: location.clone(),
                path,
            },
            cancel,
        )
    }

    pub fn open_vfs_cancellable(
        path: crate::vfs::VfsPath,
        cancel: &AtomicBool,
    ) -> io::Result<Self> {
        if let crate::vfs::VfsPath::Local(path) = path {
            return Self::open_cancellable(path, cancel);
        }
        let limit = FOUNDATION_SLO
            .viewer_memory_soft_limit_bytes
            .clamp(1, VIEWER_TEXT_PREVIEW_LIMIT_BYTES);
        let mut result = None;
        path.with_file(Some(cancel), &mut |entry, input| {
            let (bytes, hit_limit) = read_prefix(input, limit, Some(cancel))?;
            result = Some((entry.size, bytes, hit_limit));
            Ok(())
        })?;
        let (size, bytes, hit_limit) =
            result.ok_or_else(|| io::Error::other("VFS backend did not provide file data"))?;
        let mut viewer = Self::from_prefix(path.identity(), size, bytes, hit_limit);
        match path {
            crate::vfs::VfsPath::Archive { location, .. } => {
                viewer.archive_location = Some(location)
            }
            crate::vfs::VfsPath::Mounted(path) => viewer.mounted_location = Some(Box::new(path)),
            _ => {}
        }
        Ok(viewer)
    }

    fn from_prefix(path: PathBuf, total_size: u64, bytes: Vec<u8>, hit_read_limit: bool) -> Self {
        let path_fingerprint = fingerprint(&path);
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

        Self {
            path,
            local_file: None,
            archive_location: None,
            mounted_location: None,
            output: None,
            nroff: None,
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
        }
    }

    #[cfg(test)]
    pub(crate) fn open_with_reported_size_for_test(
        path: PathBuf,
        total_size: u64,
    ) -> io::Result<Self> {
        Self::open_local(path, None, Some(total_size))
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

    pub fn title(&self) -> String {
        match &self.output {
            Some(output) => format!("[{}] {}", output.status, self.path.display()),
            None => self.path.display().to_string(),
        }
    }

    pub(crate) fn command_output(title: String) -> Self {
        let mut viewer = Self::from_prefix(PathBuf::from(title), 0, Vec::new(), false);
        viewer.output = Some(Box::new(ViewerOutputState {
            status: "running".into(),
            utf8_tail: Vec::new(),
            content_hasher: std::collections::hash_map::DefaultHasher::new(),
        }));
        viewer
    }

    pub(crate) fn append_output(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
        let output = self.output.as_mut().expect("command output viewer");
        if self.nroff.is_some() {
            output.content_hasher.write(bytes);
        }
        let mut pending = std::mem::take(&mut output.utf8_tail);
        pending.extend_from_slice(bytes);
        let mut remaining = pending.as_slice();
        let mut start = self.content.len();
        let content = &mut self.content;
        let nroff = &mut self.nroff;
        let mut append = |text: &str| {
            if let Some(nroff) = nroff {
                nroff.append(content, text, &mut start);
            } else {
                content.push_str(text);
            }
        };
        while !remaining.is_empty() {
            match std::str::from_utf8(remaining) {
                Ok(text) => {
                    append(text);
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    append(
                        std::str::from_utf8(&remaining[..valid]).expect("validated UTF-8 prefix"),
                    );
                    remaining = &remaining[valid..];
                    match error.error_len() {
                        Some(count) => {
                            append("\u{fffd}");
                            remaining = &remaining[count..];
                        }
                        None => {
                            output.utf8_tail.extend_from_slice(remaining);
                            break;
                        }
                    }
                }
            }
        }
        self.index_appended_output(start);
    }

    fn index_appended_output(&mut self, start: usize) {
        for index in start.saturating_sub(1)..self.content.len().saturating_sub(1) {
            if self.content.as_bytes()[index] == b'\n'
                && self
                    .line_offsets
                    .last()
                    .is_none_or(|last| *last < index + 1)
            {
                self.line_offsets.push(index + 1);
            }
        }
        let output = self.output.as_mut().expect("command output viewer");
        if self.nroff.is_some() {
            // Nroff overstrikes can remove text. Hash its original byte stream,
            // plus visible length so a final incomplete UTF-8 tail invalidates
            // the cache without making the key depend on chunk boundaries.
            self.content_fingerprint =
                fingerprint(&(output.content_hasher.finish(), self.content.len()));
        } else {
            output
                .content_hasher
                .write(&self.content.as_bytes()[start..]);
            self.content_fingerprint = output.content_hasher.finish();
        }
    }

    pub(crate) fn finish_output(&mut self, status: String) {
        let output = self.output.as_mut().expect("command output viewer");
        if !output.utf8_tail.is_empty() {
            output.utf8_tail.clear();
            let start = self.content.len();
            self.content.push('\u{fffd}');
            self.index_appended_output(start);
        }
        self.set_output_status(status);
    }

    pub(crate) fn set_output_status(&mut self, status: String) {
        if let Some(output) = self.output.as_mut() {
            output.status = status;
        }
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn updated_on_disk(&self) -> bool {
        self.local_file
            .as_ref()
            .is_some_and(|file| file.updated_on_disk)
    }

    pub fn change_detection_unavailable(&self) -> bool {
        self.local_file
            .as_ref()
            .is_some_and(|file| file.change_detection_unavailable)
    }

    pub(crate) fn refresh_job(&self) -> Option<JobId> {
        self.local_file.as_ref()?.refresh_job
    }

    pub(crate) fn uses_nroff(&self) -> bool {
        self.nroff.is_some()
    }

    pub(crate) fn replace_after_refresh(&mut self, mut refreshed: Self) {
        refreshed.hex_mode = self.hex_mode;
        refreshed.wrap = self.wrap;
        refreshed.scroll = self.scroll.min(refreshed.line_count().saturating_sub(1));
        refreshed.last_search_query = self.last_search_query.take();
        refreshed.last_search_direction = self.last_search_direction;
        *self = refreshed;
    }

    pub(crate) fn clipboard_text(&self) -> Option<String> {
        if !self.hex_mode {
            return (self.content.len() <= MAX_CLIPBOARD_TEXT_BYTES).then(|| self.content.clone());
        }
        let rows = self.bytes.len().div_ceil(VIEWER_HEX_ROW_BYTES);
        if rows > MAX_CLIPBOARD_TEXT_BYTES / HEX_COPY_ROW_BYTES {
            return None;
        }
        let mut text = String::with_capacity(rows * HEX_COPY_ROW_BYTES);
        for (row, bytes) in self.bytes.chunks(VIEWER_HEX_ROW_BYTES).enumerate() {
            write_hex_row(&mut text, row * VIEWER_HEX_ROW_BYTES, bytes);
            text.push('\n');
        }
        Some(text)
    }

    pub fn hex_line(&self, row: usize) -> Option<String> {
        if row >= self.hex_line_count() {
            return None;
        }
        let offset = row * VIEWER_HEX_ROW_BYTES;
        let end = (offset + VIEWER_HEX_ROW_BYTES).min(self.bytes.len());
        let mut line = String::new();
        write_hex_row(&mut line, offset, &self.bytes[offset..end]);
        Some(line)
    }

    pub(crate) fn apply_action_view_options(&mut self, options: crate::action_macros::ViewOptions) {
        self.hex_mode = options.hex;
        if options.nroff {
            let original = std::mem::take(&mut self.content);
            let mut state = NroffState::default();
            state.append(&mut self.content, &original, &mut 0);
            self.line_offsets = compute_line_offsets(&self.content);
            self.content_fingerprint = fingerprint(&self.content);
            self.nroff = Some(state);
        }
    }

    /// Access a text line without scanning the preceding contents, even in hex mode.
    pub fn text_line(&self, index: usize) -> Option<&str> {
        let start = *self.line_offsets.get(index)?;
        let end = self
            .line_offsets
            .get(index + 1)
            .copied()
            .unwrap_or(self.content.len());
        let line = &self.content[start..end];
        Some(match line.strip_suffix('\n') {
            Some(line) => line.strip_suffix('\r').unwrap_or(line),
            None => line,
        })
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
        // Raw bytes and lossy text do not necessarily share the same offsets.
        self.last_search_match_offset = None;
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
        let content = if self.hex_mode {
            self.bytes.as_slice()
        } else {
            self.content.as_bytes()
        };
        let found = match direction {
            ViewerSearchDirection::Forward => find_forward_wrap(content, query.as_bytes(), start),
            ViewerSearchDirection::Backward => find_backward_wrap(content, query.as_bytes(), start),
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
                .saturating_mul(VIEWER_HEX_ROW_BYTES)
                .min(self.bytes.len().saturating_sub(1));
        }
        let index = self.scroll.min(self.line_count().saturating_sub(1));
        self.line_offsets[index]
    }

    fn line_index_for_offset(&self, offset: usize) -> usize {
        if self.hex_mode {
            return offset
                .saturating_div(VIEWER_HEX_ROW_BYTES)
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
        self.bytes.len().div_ceil(VIEWER_HEX_ROW_BYTES).max(1)
    }
}

fn write_hex_row(text: &mut String, offset: usize, bytes: &[u8]) {
    write!(text, "{offset:08x}  ").expect("writing to String cannot fail");
    for byte in bytes {
        write!(text, "{byte:02x} ").expect("writing to String cannot fail");
    }
    for _ in bytes.len()..VIEWER_HEX_ROW_BYTES {
        text.push_str("   ");
    }
    text.push_str(" |");
    for &byte in bytes {
        text.push(if byte.is_ascii_graphic() || byte == b' ' {
            char::from(byte)
        } else {
            '.'
        });
    }
    for _ in bytes.len()..VIEWER_HEX_ROW_BYTES {
        text.push(' ');
    }
    text.push('|');
}

fn ensure_regular_file(metadata: &fs::Metadata) -> io::Result<()> {
    if metadata.is_file() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "viewer supports regular files only",
        ))
    }
}

pub fn execute_viewer_refresh(
    job: WorkerJob,
    jobs: &Sender<JobEvent>,
    events: &Sender<BackgroundEvent>,
) {
    let cancel = job.cancel_flag();
    let JobRequest::RefreshViewer { path, nroff } = job.request else {
        return;
    };
    let job_id = job.id;
    let _ = jobs.send(JobEvent::Started { id: job_id });
    let result = ViewerState::open_cancellable(path, &cancel)
        .and_then(|mut viewer| {
            if nroff {
                viewer.apply_action_view_options(crate::action_macros::ViewOptions {
                    hex: viewer.hex_mode,
                    nroff,
                });
            }
            if viewer
                .local_file
                .as_ref()
                .is_some_and(|file| file.snapshot.has_changed())
            {
                return Err(io::Error::other("file changed during refresh"));
            }
            ensure_viewer_not_canceled(Some(&cancel))?;
            events
                .send(BackgroundEvent::ViewerRefreshed { job_id, viewer })
                .map_err(|_| io::Error::other("background event channel disconnected"))
        })
        .map_err(JobError::from_io);
    let _ = jobs.send(JobEvent::Finished { id: job_id, result });
}

fn read_prefix(
    file: &mut (impl Read + ?Sized),
    byte_limit: usize,
    cancel_flag: Option<&AtomicBool>,
) -> io::Result<(Vec<u8>, bool)> {
    let probe_limit = byte_limit.saturating_add(1);
    let mut bytes = Vec::with_capacity(byte_limit);
    let mut buffer = [0_u8; VIEWER_READ_CHUNK_BYTES];
    while bytes.len() < probe_limit {
        ensure_viewer_not_canceled(cancel_flag)?;
        let remaining = probe_limit.saturating_sub(bytes.len());
        let chunk_len = remaining.min(VIEWER_READ_CHUNK_BYTES);
        let read = file.read(&mut buffer[..chunk_len])?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    ensure_viewer_not_canceled(cancel_flag)?;
    let hit_read_limit = bytes.len() > byte_limit;
    if hit_read_limit {
        bytes.truncate(byte_limit);
    }
    Ok((bytes, hit_read_limit))
}

fn ensure_viewer_not_canceled(cancel_flag: Option<&AtomicBool>) -> io::Result<()> {
    if cancel_flag.is_some_and(|flag| flag.load(AtomicOrdering::Relaxed)) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            JOB_CANCELED_MESSAGE,
        ));
    }
    Ok(())
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

fn find_forward_wrap(content: &[u8], query: &[u8], start: usize) -> Option<usize> {
    let start = start.min(content.len());
    if let Some(relative) = memmem::find(&content[start..], query) {
        return Some(start + relative);
    }
    if start == 0 {
        return None;
    }
    // A match starting before the cursor may extend across it.
    memmem::find(content, query)
}

fn find_backward_wrap(content: &[u8], query: &[u8], start: usize) -> Option<usize> {
    let start = start.min(content.len());
    if start > 0 {
        let end = start
            .saturating_add(query.len().saturating_sub(1))
            .min(content.len());
        if let Some(index) = memmem::rfind(&content[..end], query) {
            return Some(index);
        }
    }
    if start >= content.len() {
        return None;
    }
    memmem::rfind(&content[start..], query).map(|relative| start + relative)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn viewer(bytes: &[u8]) -> ViewerState {
        let mut viewer = ViewerState::from_prefix(
            PathBuf::from("fixture"),
            bytes.len() as u64,
            bytes.to_vec(),
            false,
        );
        viewer.hex_mode = true;
        viewer
    }

    #[test]
    fn clipboard_preflights_text_and_hex_dump_size() {
        let bytes = vec![b'a'; MAX_CLIPBOARD_TEXT_BYTES + 1];
        let mut text_viewer = viewer(&bytes);
        text_viewer.hex_mode = false;
        assert!(text_viewer.clipboard_text().is_none());

        let rows = MAX_CLIPBOARD_TEXT_BYTES / HEX_COPY_ROW_BYTES;
        let bytes_at_limit = rows * VIEWER_HEX_ROW_BYTES;
        let hex_text = viewer(&bytes[..bytes_at_limit]).clipboard_text().unwrap();
        assert_eq!(hex_text.len(), rows * HEX_COPY_ROW_BYTES);
        assert!(
            viewer(&bytes[..bytes_at_limit + 1])
                .clipboard_text()
                .is_none()
        );
    }

    #[test]
    fn content_check_detects_same_length_edits_with_the_original_last_write_time() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.bin");
        let mut bytes = vec![0xff; VIEWER_READ_CHUNK_BYTES + 1];
        fs::write(&path, &bytes).unwrap();
        let original_modified = fs::metadata(&path).unwrap().modified().unwrap();
        let contents = ViewerFileContents::new(&bytes, Instant::now());
        assert!(
            !contents
                .has_changed(fs::File::open(&path).unwrap())
                .unwrap()
        );

        bytes[VIEWER_READ_CHUNK_BYTES] = 0xfe;
        fs::write(&path, &bytes).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(original_modified)
            .unwrap();
        let file = fs::File::open(&path).unwrap();
        let metadata = file.metadata().unwrap();
        assert_eq!(metadata.len(), bytes.len() as u64);
        assert_eq!(metadata.modified().unwrap(), original_modified);
        assert!(contents.has_changed(file).unwrap());
    }

    #[test]
    fn automatic_content_checks_wait_between_comparisons() {
        let loaded_at = Instant::now();
        let contents = ViewerFileContents::new(b"preview", loaded_at);
        let metadata_check_interval = Duration::from_millis(500);
        assert!(!contents.begin_automatic_check(loaded_at + metadata_check_interval));

        let first_check = loaded_at + CONTENT_CHECK_INTERVAL;
        assert!(contents.begin_automatic_check(first_check));
        assert!(!contents.begin_automatic_check(first_check + metadata_check_interval));
        assert!(contents.begin_automatic_check(first_check + CONTENT_CHECK_INTERVAL));
    }

    #[cfg(unix)]
    #[test]
    fn viewer_marks_a_path_replaced_by_a_symlink_loop_as_changed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.txt");
        fs::write(&path, "original").unwrap();
        let viewer = ViewerState::open(path.clone()).unwrap();
        let snapshot = &viewer.local_file.as_ref().unwrap().snapshot;
        assert!(!snapshot.has_changed());

        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("report.txt", &path).unwrap();
        assert!(snapshot.has_changed());
        assert_eq!(viewer.content(), "original");
    }

    #[cfg(windows)]
    #[test]
    fn viewer_detects_a_same_size_edit_with_last_write_time_restored() {
        use std::io::Write as _;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.txt");
        fs::write(&path, b"old").unwrap();
        let original_modified = fs::metadata(&path).unwrap().modified().unwrap();
        let mut viewer = ViewerState::open(path.clone()).unwrap();
        let snapshot = Arc::get_mut(&mut viewer.local_file.as_mut().unwrap().snapshot).unwrap();
        assert!(!snapshot.has_changed());

        // Separate the two writes even on a system with a coarse clock.
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .write_all(b"new")
            .unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(original_modified)
            .unwrap();

        assert_eq!(
            snapshot.identity,
            SourceIdentity::new(&fs::metadata(&path).unwrap())
        );
        assert!(snapshot.has_changed());
        snapshot.change_time = None;
        *snapshot.contents.next_automatic_check.get_mut().unwrap() =
            Instant::now() + CONTENT_CHECK_INTERVAL;
        assert!(!snapshot.poll_for_change());
        assert!(snapshot.has_changed());
        *snapshot.contents.next_automatic_check.get_mut().unwrap() = Instant::now();
        assert!(snapshot.poll_for_change());
    }

    #[test]
    fn hex_search_handles_utf8_and_matches_crossing_the_cursor() {
        let content = format!("{}é target", "a".repeat(15));
        for direction in [
            ViewerSearchDirection::Forward,
            ViewerSearchDirection::Backward,
        ] {
            for (query, expected_offset) in [("é", 15), ("target", 18)] {
                let mut viewer = viewer(content.as_bytes());
                viewer.move_lines(1);
                assert!(viewer.start_search(query.into(), direction).is_some());
                assert_eq!(viewer.last_search_match_offset, Some(expected_offset));
                assert!(viewer.continue_search(None).is_some(), "search should wrap");
                assert_eq!(viewer.last_search_match_offset, Some(expected_offset));
            }
        }
    }

    #[test]
    fn hex_search_uses_raw_offsets_and_resets_them_on_mode_switch() {
        let mut bytes = vec![0xff; 15];
        bytes.extend_from_slice(b"target target");
        let mut viewer = viewer(&bytes);
        assert_eq!(
            viewer.start_search("target".into(), ViewerSearchDirection::Forward),
            Some(0)
        );
        assert_eq!(viewer.last_search_match_offset, Some(15));
        assert_eq!(viewer.continue_search(None), Some(1));
        assert_eq!(viewer.last_search_match_offset, Some(22));
        viewer.toggle_hex_mode();
        assert!(viewer.last_search_match_offset.is_none());
        assert_eq!(viewer.continue_search(None), Some(0));
        assert_eq!(viewer.last_search_match_offset, Some(45));
        viewer.toggle_hex_mode();
        assert_eq!(viewer.continue_search(None), Some(0));
        assert_eq!(viewer.last_search_match_offset, Some(15));
    }

    #[test]
    fn search_handles_empty_files_queries_and_missing_matches() {
        for bytes in [&b""[..], &b"contents"[..]] {
            for direction in [
                ViewerSearchDirection::Forward,
                ViewerSearchDirection::Backward,
            ] {
                let mut viewer = viewer(bytes);
                assert_eq!(viewer.start_search(String::new(), direction), None);
                assert_eq!(viewer.start_search("absent".into(), direction), None);
            }
        }
    }

    #[test]
    fn indexed_text_lines_match_string_lines() {
        for content in ["", "\n", "a\r\nb\né\n", "a\r", "a\n\n"] {
            let viewer = viewer(content.as_bytes());
            let expected: Vec<_> = if content.is_empty() {
                vec![""]
            } else {
                content.lines().collect()
            };
            for (index, line) in expected.iter().enumerate() {
                assert_eq!(viewer.text_line(index), Some(*line));
            }
            assert_eq!(viewer.text_line(expected.len()), None);
        }
    }
}
