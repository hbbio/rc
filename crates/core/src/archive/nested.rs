//! Private, bounded materialization of inner containers. Weak references on the
//! parent index reuse a materialization without creating ownership cycles.
use super::{ArchiveIndex, ArchiveLocation, ArchiveReader, cache, check_canceled, invalid_data};
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, TryLockError};
use std::time::Duration;

pub(super) const MAX_DEPTH: usize = 8;
const MEMBER_LIMIT: u64 = 512 * 1024 * 1024;
const STORAGE_LIMIT: u64 = 1024 * 1024 * 1024;
static STORAGE_BYTES: AtomicU64 = AtomicU64::new(0);

struct Reservation(u64);
impl Reservation {
    fn acquire(bytes: u64) -> io::Result<Self> {
        Self::acquire_with_limit(bytes, STORAGE_LIMIT)
    }

    fn acquire_with_limit(bytes: u64, limit: u64) -> io::Result<Self> {
        let reserve = || {
            let mut used = STORAGE_BYTES.load(Ordering::Relaxed);
            loop {
                let total = used
                    .checked_add(bytes)
                    .filter(|total| *total <= limit)
                    .ok_or(())?;
                match STORAGE_BYTES.compare_exchange_weak(
                    used,
                    total,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return Ok::<(), ()>(()),
                    Err(current) => used = current,
                }
            }
        };
        for _ in 0..=MAX_DEPTH {
            if reserve().is_ok() {
                return Ok(Self(bytes));
            }
            if !cache::release_unused_indexes() {
                break;
            }
        }
        Err(io::Error::other(
            "nested archive temporary storage exceeds 1 GiB",
        ))
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        STORAGE_BYTES.fetch_sub(self.0, Ordering::Relaxed);
    }
}

pub(super) struct ArchiveSource {
    pub path: PathBuf,
    // Keep the private file and all ancestors alive for in-flight readers.
    _temporary: Option<tempfile::NamedTempFile>,
    _reservation: Option<Reservation>,
    parent: Option<Arc<ArchiveIndex>>,
}

impl ArchiveSource {
    pub fn persistent_root(&self, identity: &cache::SourceIdentity) -> (&std::path::Path, Vec<u8>) {
        match &self.parent {
            Some(parent) => parent.source.persistent_root(&parent.identity),
            None => (&self.path, identity.fingerprint()),
        }
    }

    pub fn verify_parent(&self) -> io::Result<()> {
        self.parent
            .as_ref()
            .map_or(Ok(()), |parent| parent.verify_source())
    }
}

pub(super) fn resolve(
    location: &ArchiveLocation,
    cancel: Option<&AtomicBool>,
) -> io::Result<Arc<ArchiveSource>> {
    check_canceled(cancel)?;
    let Some(parent) = location.parent.as_deref() else {
        return Ok(Arc::new(ArchiveSource {
            path: fs::canonicalize(&location.archive)?,
            _temporary: None,
            _reservation: None,
            parent: None,
        }));
    };
    // Public locations can be constructed directly: validate depth here too.
    let mut depth = 1;
    let mut ancestor = Some(parent);
    while let Some(location) = ancestor {
        depth += 1;
        if depth > MAX_DEPTH {
            return Err(invalid_data("archive nesting exceeds eight levels"));
        }
        ancestor = location.parent.as_deref();
    }
    let member_path = parent.member_path(&location.archive)?;
    let index = cache::get_index(parent, cancel)?;
    let member = index
        .members
        .get(&member_path)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "nested archive member no longer exists",
            )
        })?
        .clone();
    member.ensure_extractable()?;
    if member.is_dir || member.size > MEMBER_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "nested archives must be regular files of at most 512 MiB",
        ));
    }
    let mut materialized = loop {
        check_canceled(cancel)?;
        match index.nested.try_lock() {
            Ok(lock) => break lock,
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(10)),
            Err(TryLockError::Poisoned(_)) => {
                return Err(io::Error::other("nested archive cache lock is poisoned"));
            }
        }
    };
    materialized.retain(|_, source| source.strong_count() != 0);
    if let Some(source) = materialized
        .get(&member_path)
        .and_then(std::sync::Weak::upgrade)
    {
        source.verify_parent()?;
        return Ok(source);
    }
    let reservation = Reservation::acquire(member.size)?;
    let mut file = tempfile::Builder::new().prefix("rc-nested-").tempfile()?;
    let mut reader = ArchiveReader::from_index(Arc::clone(&index), cancel)?;
    let result = (|| {
        let mut input = reader.open_member(&member)?;
        let mut copied = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            check_canceled(cancel)?;
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            copied = copied.saturating_add(read as u64);
            if copied > member.size {
                return Err(invalid_data("nested member exceeds its advertised size"));
            }
            file.write_all(&buffer[..read])?;
        }
        if copied != member.size {
            return Err(invalid_data("nested member is truncated"));
        }
        Ok(())
    })();
    super::normalize_archive_io_result(result, cancel)?;
    reader.verify_source()?;
    file.flush()?;
    let source = Arc::new(ArchiveSource {
        path: file.path().to_path_buf(),
        _temporary: Some(file),
        _reservation: Some(reservation),
        parent: Some(Arc::clone(&index)),
    });
    materialized.insert(member_path, Arc::downgrade(&source));
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_reclaims_entire_unused_ancestor_chain() {
        // Isolate the process-global cache/budget from concurrently running tests.
        const FLAG: &str = "RC_TEST_NESTED_RECLAMATION";
        if std::env::var_os(FLAG).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "archive::nested::tests::storage_reclaims_entire_unused_ancestor_chain",
                    "--nocapture",
                ])
                .env(FLAG, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        fn zip_bytes(bytes: &[u8]) -> Vec<u8> {
            let mut zip = ::zip::ZipWriter::new(io::Cursor::new(Vec::new()));
            zip.start_file("inner.zip", ::zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
            zip.finish().unwrap().into_inner()
        }
        let files = tempfile::tempdir().unwrap();
        let path = files.path().join("outer.zip");
        let mut bytes = zip_bytes(b"leaf");
        for _ in 0..3 {
            bytes = zip_bytes(&bytes);
        }
        fs::write(&path, bytes).unwrap();
        let mut location = ArchiveLocation::root(path);
        for _ in 0..3 {
            let path = location.archive.join("inner.zip");
            location = ArchiveLocation::nested(location, path).unwrap();
        }
        let index = cache::get_index(&location, None).unwrap();
        let source = Arc::downgrade(&index.source);
        drop(index);
        let used = STORAGE_BYTES.load(Ordering::Relaxed);
        assert!(used > 1);
        // A single pass releases only the leaf. All three ancestors must be
        // reclaimed before this reservation can fit into the small test budget.
        let reservation = Reservation::acquire_with_limit(used, used + 1).unwrap();
        assert!(source.upgrade().is_none());
        assert_eq!(STORAGE_BYTES.load(Ordering::Relaxed), used);
        drop(reservation);
        assert_eq!(STORAGE_BYTES.load(Ordering::Relaxed), 0);
    }
}
