//! Disposable, versioned metadata indexes. The outer source identity and every
//! container boundary form the key; cache failures never prevent archive access.
use super::{
    ArchiveFormat, ArchiveLocation, ArchiveMember, cache, check_canceled, invalid_data, nested,
    safe_member_path,
};
use std::collections::{BTreeMap, hash_map::DefaultHasher};
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAGIC: &[u8] = b"RCARIDX1";
const FILE_LIMIT: usize = 64 * 1024 * 1024;
const CACHE_LIMIT: u64 = 256 * 1024 * 1024;
const CACHE_FILES: usize = 64;
const MEMBER_LIMIT: usize = 1_000_000;
static STORE: OnceLock<Store> = OnceLock::new();

struct Store {
    directory: PathBuf,
    maintenance: Mutex<()>,
}

/// Enables a private, best-effort on-disk archive metadata cache for this process.
/// Call once before starting workers. Existing directories must be owned by the
/// current user and inaccessible to other users. An unavailable cache may simply
/// be left disabled; archive browsing does not depend on it.
pub fn configure_archive_index_cache(directory: PathBuf) -> io::Result<()> {
    if !directory.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive cache path must be absolute",
        ));
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&directory)?;
    validate_directory(&directory)?;
    let store = Store {
        directory,
        maintenance: Mutex::new(()),
    };
    store.prune(None)?;
    STORE.set(store).map_err(|_| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "archive cache is already configured",
        )
    })
}

fn validate_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(invalid_data(
            "archive cache must be a directory, not a symlink",
        ));
    }
    validate_owner(&metadata)
}

fn validate_owner(metadata: &fs::Metadata) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != nix::unistd::geteuid().as_raw() || metadata.mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "archive cache must be private and owned by the current user",
            ));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "persistent archive caches are supported on Unix",
        ))
    }
}

pub(super) struct Entry {
    store: &'static Store,
    key: Vec<u8>,
    path: PathBuf,
}

pub(super) fn entry(
    location: &ArchiveLocation,
    source: &nested::ArchiveSource,
    identity: &cache::SourceIdentity,
) -> Option<Entry> {
    let store = STORE.get()?;
    let (root, fingerprint) = source.persistent_root(identity);
    let mut key = Vec::new();
    put_bytes(&mut key, path_bytes(root).ok()?).ok()?;
    put_bytes(&mut key, &fingerprint).ok()?;
    let mut levels = Vec::new();
    let mut current = location;
    while let Some(parent) = current.parent.as_deref() {
        levels.push((
            current.format().ok()?,
            parent.member_path(&current.archive).ok()?,
        ));
        current = parent;
    }
    key.push(format_id(current.format().ok()?));
    for (format, member) in levels.into_iter().rev() {
        key.push(format_id(format));
        put_bytes(&mut key, path_bytes(&member).ok()?).ok()?;
    }
    let mut hash = DefaultHasher::new();
    key.hash(&mut hash);
    Some(Entry {
        path: store.directory.join(format!("{:016x}.idx", hash.finish())),
        store,
        key,
    })
}

fn format_id(format: ArchiveFormat) -> u8 {
    match format {
        ArchiveFormat::Zip => 1,
        ArchiveFormat::Tar => 2,
        ArchiveFormat::TarGzip => 3,
        ArchiveFormat::TarBzip2 => 4,
    }
}

impl Entry {
    pub fn load(
        &self,
        cancel: Option<&AtomicBool>,
    ) -> io::Result<Option<BTreeMap<PathBuf, ArchiveMember>>> {
        validate_directory(&self.store.directory)?;
        let mut options = File::options();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
        }
        let mut file = match options.open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > FILE_LIMIT as u64 {
            return Err(invalid_data("invalid archive cache file"));
        }
        validate_owner(&metadata)?;
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            check_canceled(cancel)?;
            let length = file.read(&mut buffer)?;
            if length == 0 {
                break;
            }
            if bytes.len().saturating_add(length) > FILE_LIMIT {
                return Err(invalid_data("archive cache file grew beyond its limit"));
            }
            bytes.extend_from_slice(&buffer[..length]);
        }
        decode(&bytes, &self.key, cancel).map(Some)
    }

    pub fn encode(
        &self,
        members: &BTreeMap<PathBuf, ArchiveMember>,
        cancel: Option<&AtomicBool>,
    ) -> io::Result<Vec<u8>> {
        encode(members, &self.key, cancel)
    }

    pub fn save(&self, bytes: &[u8], cancel: Option<&AtomicBool>) -> io::Result<()> {
        validate_directory(&self.store.directory)?;
        let mut file = tempfile::Builder::new()
            .prefix(".index-")
            .tempfile_in(&self.store.directory)?;
        #[cfg(unix)]
        rustix::fs::flock(
            file.as_file(),
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        )?;
        file.write_all(bytes)?;
        file.flush()?;
        check_canceled(cancel)?;
        file.persist(&self.path).map_err(|error| error.error)?;
        self.store.prune(cancel)
    }
}

impl Store {
    fn prune(&self, cancel: Option<&AtomicBool>) -> io::Result<()> {
        let Ok(_guard) = self.maintenance.try_lock() else {
            return Ok(());
        };
        let mut entries = Vec::new();
        let mut total = 0_u64;
        for entry in fs::read_dir(&self.directory)?.take(4096) {
            check_canceled(cancel)?;
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            #[cfg(unix)]
            if name.starts_with(".index-") {
                self.remove_abandoned_write(&entry.path());
                continue;
            }
            if name.len() != 20
                || !name.ends_with(".idx")
                || !name.as_bytes()[..16].iter().all(u8::is_ascii_hexdigit)
            {
                continue;
            }
            let metadata = fs::symlink_metadata(entry.path())?;
            if !metadata.is_file() || validate_owner(&metadata).is_err() {
                continue;
            }
            total = total.saturating_add(metadata.len());
            entries.push((
                metadata.modified().unwrap_or(UNIX_EPOCH),
                entry.path(),
                metadata.len(),
            ));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let mut count = entries.len();
        for (_, path, size) in entries {
            if count <= CACHE_FILES && total <= CACHE_LIMIT {
                break;
            }
            check_canceled(cancel)?;
            match fs::remove_file(path) {
                Ok(()) => {
                    count -= 1;
                    total = total.saturating_sub(size);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    #[cfg(unix)]
    fn remove_abandoned_write(&self, path: &Path) {
        use std::os::unix::fs::OpenOptionsExt;
        let Ok(file) = File::options()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
            .open(path)
        else {
            return;
        };
        let Ok(metadata) = file.metadata() else {
            return;
        };
        if !metadata.is_file() || validate_owner(&metadata).is_err() {
            return;
        }
        // Active writers hold this lock through atomic replacement. A crashed
        // writer releases it automatically, even if the temporary name remains.
        if rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive).is_ok() {
            let _ = fs::remove_file(path);
        }
    }
}

fn path_bytes(path: &Path) -> io::Result<&[u8]> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(path.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        path.to_str()
            .map(str::as_bytes)
            .ok_or_else(|| invalid_data("unsupported cache path encoding"))
    }
}

fn put_bytes(output: &mut Vec<u8>, bytes: &[u8]) -> io::Result<()> {
    let size =
        u32::try_from(bytes.len()).map_err(|_| invalid_data("archive cache field is too large"))?;
    output.extend_from_slice(&size.to_le_bytes());
    output.extend_from_slice(bytes);
    Ok(())
}

fn put_time(output: &mut Vec<u8>, time: Option<SystemTime>) {
    let Some(time) = time else {
        output.push(0);
        return;
    };
    let (sign, duration) = match time.duration_since(UNIX_EPOCH) {
        Ok(value) => (1, value),
        Err(error) => (2, error.duration()),
    };
    output.push(sign);
    output.extend_from_slice(&duration.as_secs().to_le_bytes());
    output.extend_from_slice(&duration.subsec_nanos().to_le_bytes());
}

fn encode(
    members: &BTreeMap<PathBuf, ArchiveMember>,
    key: &[u8],
    cancel: Option<&AtomicBool>,
) -> io::Result<Vec<u8>> {
    if members.len() > MEMBER_LIMIT {
        return Err(invalid_data("archive metadata is too large to persist"));
    }
    let mut bytes = MAGIC.to_vec();
    put_bytes(&mut bytes, key)?;
    bytes.extend_from_slice(&(members.len() as u32).to_le_bytes());
    for (path, member) in members {
        check_canceled(cancel)?;
        put_bytes(&mut bytes, path_bytes(path)?)?;
        bytes.push(u8::from(member.index.is_some()));
        if let Some(index) = member.index {
            bytes.extend_from_slice(&index.to_le_bytes());
        }
        bytes.extend_from_slice(&(member.order as u64).to_le_bytes());
        bytes.push(u8::from(member.is_dir));
        bytes.extend_from_slice(&member.size.to_le_bytes());
        put_time(&mut bytes, member.modified);
        bytes.push(u8::from(member.mode.is_some()));
        if let Some(mode) = member.mode {
            bytes.extend_from_slice(&mode.to_le_bytes());
        }
        bytes.extend_from_slice(&[u8::from(member.encrypted), u8::from(member.unsupported)]);
        if bytes.len() > FILE_LIMIT - 4 {
            return Err(invalid_data("archive metadata is too large to persist"));
        }
    }
    let checksum = crc32fast::hash(&bytes);
    bytes.extend_from_slice(&checksum.to_le_bytes());
    Ok(bytes)
}

fn number<const N: usize>(input: &mut Cursor<&[u8]>) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}
fn boolean(input: &mut Cursor<&[u8]>) -> io::Result<bool> {
    match number::<1>(input)?[0] {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(invalid_data("invalid cache flag")),
    }
}
fn field<'a>(input: &mut Cursor<&'a [u8]>, limit: usize) -> io::Result<&'a [u8]> {
    let size = u32::from_le_bytes(number(input)?) as usize;
    let start = input.position() as usize;
    let end = start
        .checked_add(size)
        .filter(|end| size <= limit && *end <= input.get_ref().len())
        .ok_or_else(|| invalid_data("invalid cache field length"))?;
    let bytes = &input.get_ref()[start..end];
    input.set_position(end as u64);
    Ok(bytes)
}
fn read_time(input: &mut Cursor<&[u8]>) -> io::Result<Option<SystemTime>> {
    let sign = number::<1>(input)?[0];
    if sign == 0 {
        return Ok(None);
    }
    let seconds = u64::from_le_bytes(number(input)?);
    let nanos = u32::from_le_bytes(number(input)?);
    if nanos >= 1_000_000_000 {
        return Err(invalid_data("invalid cache timestamp"));
    }
    let duration = Duration::new(seconds, nanos);
    match sign {
        1 => UNIX_EPOCH.checked_add(duration),
        2 => UNIX_EPOCH.checked_sub(duration),
        _ => None,
    }
    .map(Some)
    .ok_or_else(|| invalid_data("invalid cache timestamp"))
}
fn decode(
    bytes: &[u8],
    key: &[u8],
    cancel: Option<&AtomicBool>,
) -> io::Result<BTreeMap<PathBuf, ArchiveMember>> {
    let split = bytes
        .len()
        .checked_sub(4)
        .ok_or_else(|| invalid_data("truncated archive cache"))?;
    if crc32fast::hash(&bytes[..split]) != u32::from_le_bytes(bytes[split..].try_into().unwrap()) {
        return Err(invalid_data("archive cache checksum mismatch"));
    }
    let mut input = Cursor::new(&bytes[..split]);
    if number::<8>(&mut input)?.as_slice() != MAGIC || field(&mut input, 1024 * 1024)? != key {
        return Err(invalid_data("archive cache identity mismatch"));
    }
    let count = u32::from_le_bytes(number(&mut input)?) as usize;
    if count > MEMBER_LIMIT {
        return Err(invalid_data("archive cache has too many members"));
    }
    let mut members = BTreeMap::new();
    for _ in 0..count {
        check_canceled(cancel)?;
        let path = safe_member_path(field(&mut input, 65_536)?)?
            .ok_or_else(|| invalid_data("unsafe cached member path"))?;
        let index = if boolean(&mut input)? {
            Some(u64::from_le_bytes(number(&mut input)?))
        } else {
            None
        };
        let order = usize::try_from(u64::from_le_bytes(number(&mut input)?))
            .map_err(|_| invalid_data("invalid cached member order"))?;
        let is_dir = boolean(&mut input)?;
        let size = u64::from_le_bytes(number(&mut input)?);
        let modified = read_time(&mut input)?;
        let mode = if boolean(&mut input)? {
            Some(u32::from_le_bytes(number(&mut input)?))
        } else {
            None
        };
        let encrypted = boolean(&mut input)?;
        let unsupported = boolean(&mut input)?;
        if (!is_dir && index.is_none())
            || members
                .insert(
                    path,
                    ArchiveMember {
                        index,
                        order,
                        is_dir,
                        size,
                        modified,
                        mode,
                        encrypted,
                        unsupported,
                    },
                )
                .is_some()
        {
            return Err(invalid_data("invalid or duplicate cached member"));
        }
    }
    if input.position() != split as u64 {
        return Err(invalid_data("trailing archive cache data"));
    }
    Ok(members)
}

#[cfg(all(test, unix))]
mod tests;
