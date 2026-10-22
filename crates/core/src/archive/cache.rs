//! Process-local, single-flight LRU cache. No archive payloads or sidecar files
//! are written. In-flight users retain their immutable snapshot across eviction.
use super::*;
use std::collections::VecDeque;
use std::fs::Metadata;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

const CACHE_ARCHIVES: usize = 8;
const CACHE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SourceIdentity {
    pub size: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    unix: (u64, u64, i64, i64),
}

impl SourceIdentity {
    fn new(metadata: &Metadata) -> Self {
        Self {
            size: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            #[cfg(unix)]
            unix: {
                use std::os::unix::fs::MetadataExt;
                (
                    metadata.dev(),
                    metadata.ino(),
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                )
            },
        }
    }

    pub fn verify(&self, metadata: &Metadata) -> io::Result<()> {
        if metadata.is_file() && *self == Self::new(metadata) {
            Ok(())
        } else {
            Err(invalid_data(
                "archive changed while it was being read; refresh the pane",
            ))
        }
    }
}

#[derive(Default)]
struct SlotState {
    building: bool,
    index: Option<Arc<ArchiveIndex>>,
}

#[derive(Default)]
struct Slot {
    state: Mutex<SlotState>,
    changed: Condvar,
}

struct Entry {
    path: PathBuf,
    identity: SourceIdentity,
    format: ArchiveFormat,
    slot: Arc<Slot>,
}

static CACHE: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();

// Always wake waiters on error or panic. A canceled first reader must never
// poison the archive for the other pane or hold a worker lane indefinitely.
struct Building(Arc<Slot>);
impl Drop for Building {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.building = false;
        self.0.changed.notify_all();
    }
}

pub(super) fn get_index(
    location: &ArchiveLocation,
    cancel: Option<&AtomicBool>,
) -> io::Result<Arc<ArchiveIndex>> {
    check_canceled(cancel)?;
    if location
        .directory
        .components()
        .any(|component| match component {
            Component::Normal(name) => !safe_component(name),
            _ => true,
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid archive directory",
        ));
    }
    let format = location.format()?;
    let path = fs::canonicalize(&location.archive)?;
    // Opening before inspecting metadata also rejects FIFOs without blocking.
    let mut file = open_source(&path)?;
    let identity = SourceIdentity::new(&file.metadata()?);
    let cache = CACHE.get_or_init(Mutex::default);
    let _trim = TrimOnDrop(cache);
    let slot = {
        let mut entries = cache.lock().unwrap_or_else(|e| e.into_inner());
        let existing = entries
            .iter()
            .position(|entry| {
                entry.path == path && entry.identity == identity && entry.format == format
            })
            .and_then(|position| entries.remove(position));
        let entry = existing.unwrap_or_else(|| Entry {
            path: path.clone(),
            identity: identity.clone(),
            format,
            slot: Arc::default(),
        });
        let slot = Arc::clone(&entry.slot);
        // Remove superseded snapshots for this source. Existing readers own Arc.
        entries.retain(|entry| entry.path != path);
        entries.push_back(entry);
        slot
    };
    loop {
        check_canceled(cancel)?;
        let mut state = slot.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(index) = &state.index {
            return Ok(Arc::clone(index));
        }
        if state.building {
            // Cancellation remains responsive even when another worker owns the scan.
            let _ = slot.changed.wait_timeout(state, Duration::from_millis(25));
            continue;
        }
        state.building = true;
        drop(state);
        let building = Building(Arc::clone(&slot));
        file.rewind()?;
        let result = match format {
            ArchiveFormat::Zip => zip::build_index(&mut file, cancel),
            ArchiveFormat::Tar | ArchiveFormat::TarGzip | ArchiveFormat::TarBzip2 => {
                tar::build_index(&mut file, format, cancel)
            }
        };
        let (members, backend) = normalize_archive_io_result(result, cancel)?;
        identity.verify(&file.metadata()?)?;
        identity.verify(&fs::metadata(&path)?)?;
        let index = Arc::new(ArchiveIndex::build(
            members,
            identity.clone(),
            backend,
            cancel,
        )?);
        check_canceled(cancel)?;
        slot.state.lock().unwrap_or_else(|e| e.into_inner()).index = Some(Arc::clone(&index));
        drop(building);
        return Ok(index);
    }
}

struct TrimOnDrop(&'static Mutex<VecDeque<Entry>>);
impl Drop for TrimOnDrop {
    fn drop(&mut self) {
        trim_cache(self.0);
    }
}

fn trim_cache(cache: &Mutex<VecDeque<Entry>>) {
    let mut entries = cache.lock().unwrap_or_else(|e| e.into_inner());
    let mut bytes = entries
        .iter()
        .filter_map(|entry| entry.slot.state.try_lock().ok())
        .filter_map(|state| state.index.as_ref().map(|index| index.estimated_bytes()))
        .fold(0_usize, usize::saturating_add);
    // Keep at least the most recent archive, even if its metadata exceeds the
    // soft budget: otherwise every directory change would rebuild a large TAR.
    while entries.len() > 1 && (entries.len() > CACHE_ARCHIVES || bytes > CACHE_BYTES) {
        let removable = entries.iter().take(entries.len() - 1).position(|entry| {
            entry
                .slot
                .state
                .try_lock()
                .is_ok_and(|state| !state.building)
        });
        let Some(position) = removable else {
            break;
        };
        let entry = entries.remove(position).unwrap();
        if let Ok(state) = entry.slot.state.lock()
            && let Some(index) = &state.index
        {
            bytes = bytes.saturating_sub(index.estimated_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture(name: &str) -> (tempfile::TempDir, ArchiveLocation) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(name);
        write_tar(&path, "first", b"contents");
        (root, ArchiveLocation::root(path))
    }

    fn write_tar(path: &Path, name: &str, bytes: &[u8]) {
        let mut builder = ::tar::Builder::new(Vec::new());
        let mut header = ::tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(1);
        header.set_cksum();
        builder.append_data(&mut header, name, bytes).unwrap();
        let bytes = builder.into_inner().unwrap();
        if path.extension().unwrap() == "tgz" {
            let mut encoder = flate2::write::GzEncoder::new(
                File::create(path).unwrap(),
                flate2::Compression::fast(),
            );
            encoder.write_all(&bytes).unwrap();
            encoder.finish().unwrap();
        } else {
            fs::write(path, bytes).unwrap();
        }
    }

    #[test]
    fn listings_and_size_requests_reuse_the_same_immutable_index() {
        for name in ["example.tar", "example.tgz"] {
            let (_root, location) = fixture(name);
            let first = get_index(&location, None).unwrap();
            let nested = ArchiveLocation {
                directory: "unused".into(),
                ..location.clone()
            };
            let next = get_index(&nested, None).unwrap();
            assert!(Arc::ptr_eq(&first, &next));
            let cancel = AtomicBool::new(false);
            let size =
                measure_archive_selection(&location, &[location.archive.join("first")], &cancel)
                    .unwrap();
            assert_eq!(size.apparent_bytes, 8);
            assert!(Arc::ptr_eq(&first, &get_index(&location, None).unwrap()));
        }
    }

    #[test]
    fn source_changes_invalidate_metadata_and_in_flight_reader_snapshots() {
        let (_root, location) = fixture("example.tgz");
        let first = get_index(&location, None).unwrap();
        let reader = ArchiveReader::open(&location, None).unwrap();
        write_tar(&location.archive, "second", b"new content");
        // Force a distinct mtime even on filesystems with coarse timestamps.
        filetime::set_file_mtime(
            &location.archive,
            filetime::FileTime::from_unix_time(100, 0),
        )
        .unwrap();
        assert!(reader.verify_source().is_err());
        let next = get_index(&location, None).unwrap();
        assert!(!Arc::ptr_eq(&first, &next));
        assert!(next.members.contains_key(Path::new("second")));
        assert!(!next.members.contains_key(Path::new("first")));
    }

    #[test]
    fn failed_or_canceled_builds_do_not_cache_partial_results() {
        let (_root, location) = fixture("example.tgz");
        let valid = fs::read(&location.archive).unwrap();
        fs::write(&location.archive, &valid[..valid.len() / 2]).unwrap();
        assert!(get_index(&location, None).is_err());
        fs::write(&location.archive, valid).unwrap();
        let cancel = AtomicBool::new(true);
        assert_eq!(
            get_index(&location, Some(&cancel)).err().unwrap().kind(),
            io::ErrorKind::Interrupted
        );
        cancel.store(false, Ordering::Relaxed);
        assert!(
            get_index(&location, Some(&cancel))
                .unwrap()
                .members
                .contains_key(Path::new("first"))
        );
    }

    #[test]
    fn waiting_for_another_index_builder_is_cancelable_and_owner_can_retry() {
        let (_root, location) = fixture("example.tar");
        let path = fs::canonicalize(&location.archive).unwrap();
        let slot = Arc::new(Slot::default());
        slot.state.lock().unwrap().building = true;
        let owner = Building(Arc::clone(&slot));
        CACHE
            .get_or_init(Mutex::default)
            .lock()
            .unwrap()
            .push_back(Entry {
                identity: SourceIdentity::new(&fs::metadata(&path).unwrap()),
                format: ArchiveFormat::Tar,
                path,
                slot,
            });
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_location = location.clone();
        let flag = Arc::clone(&cancel);
        let worker = std::thread::spawn(move || {
            tx.send(
                get_index(&worker_location, Some(&flag))
                    .err()
                    .map(|error| error.kind()),
            )
            .unwrap();
        });
        // No timing assumption: if it has not reached the wait yet, it still
        // must observe the same cancellation before acquiring the owner's slot.
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            Some(io::ErrorKind::Interrupted)
        );
        worker.join().unwrap();
        drop(owner);
        assert!(get_index(&location, None).is_ok());
    }
}
