//! Native notifications are invalidations, never an authoritative filesystem diff.
//! Watch registration and path resolution stay off the terminal thread. Only loaded
//! directories are watched. FSEvents can subscribe to a tree without walking it;
//! backends that emulate recursion instead watch the bounded set of loaded directories.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use notify::event::{CreateKind, ModifyKind, RemoveKind};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use rc_core::{ActivePanel, AppState};
use tokio::sync::watch;

const COALESCE_DELAY: Duration = Duration::from_millis(100);
const WATCH_POLL: Duration = Duration::from_millis(50);
const RETRY_DELAY: Duration = Duration::from_secs(2);
const EVENT_CAPACITY: usize = 1024;

#[derive(Default, PartialEq, Eq)]
struct Targets {
    directories: BTreeSet<PathBuf>,
    roots: BTreeSet<PathBuf>,
    tree_root: Option<PathBuf>,
}

#[derive(Default)]
struct Changes {
    directories: BTreeSet<PathBuf>,
    tree: BTreeSet<PathBuf>,
    warning: Option<String>,
}

impl Changes {
    fn invalidate(&mut self, paths: impl IntoIterator<Item = PathBuf>, tree: bool) {
        for path in paths {
            if tree {
                self.tree.insert(path.clone());
            }
            self.directories.insert(path);
        }
    }
}

pub(crate) struct FilesystemWatch {
    targets_tx: watch::Sender<Arc<Targets>>,
    changes: Arc<Mutex<Changes>>,
    panels: [Option<PathBuf>; 2],
    tree_key: Option<(PathBuf, u64)>,
    tree_directories: BTreeSet<PathBuf>,
    dirty_panels: [Option<Instant>; 2],
    dirty_tree: BTreeSet<PathBuf>,
    tree_dirty_since: Option<Instant>,
}

impl FilesystemWatch {
    pub(crate) fn spawn(tracing_log: Option<PathBuf>) -> std::io::Result<Self> {
        let (targets_tx, targets_rx) = watch::channel(Arc::new(Targets::default()));
        let changes = Arc::new(Mutex::new(Changes::default()));
        let worker_changes = Arc::clone(&changes);
        // Dropping the sender stops this thread. Do not join on the terminal
        // thread: registering a watch on an unavailable mount can block in the OS.
        thread::Builder::new()
            .name(String::from("rc-filesystem-watch"))
            .spawn(move || run(targets_rx, worker_changes, tracing_log))?;
        Ok(Self {
            targets_tx,
            changes,
            panels: [None, None],
            tree_key: None,
            tree_directories: BTreeSet::new(),
            dirty_panels: [None, None],
            dirty_tree: BTreeSet::new(),
            tree_dirty_since: None,
        })
    }

    pub(crate) fn poll(&mut self, state: &mut AppState) {
        self.sync_targets(state);
        let changes = std::mem::take(&mut *self.changes.lock().expect("watch changes lock"));
        let now = Instant::now();
        if let Some(warning) = changes.warning {
            state.set_status(warning);
        }
        for (index, path) in self.panels.iter().enumerate() {
            if path
                .as_ref()
                .is_some_and(|path| changes.directories.contains(path))
            {
                self.dirty_panels[index].get_or_insert(now);
            }
        }
        self.dirty_tree
            .extend(changes.tree.intersection(&self.tree_directories).cloned());
        if !self.dirty_tree.is_empty() {
            self.tree_dirty_since.get_or_insert(now);
        }
        for panel in [ActivePanel::Left, ActivePanel::Right] {
            let dirty = &mut self.dirty_panels[panel.index()];
            if dirty.is_some_and(|since| now.duration_since(since) >= COALESCE_DELAY)
                && state.refresh_directory_if_idle(panel)
            {
                *dirty = None;
            }
        }
        if self
            .tree_dirty_since
            .is_some_and(|since| now.duration_since(since) >= COALESCE_DELAY)
            && state.refresh_tree_if_idle(&self.dirty_tree.iter().cloned().collect::<Vec<_>>())
        {
            self.dirty_tree.clear();
            self.tree_dirty_since = None;
        }
    }

    fn sync_targets(&mut self, state: &AppState) {
        let panels = state
            .panels
            .each_ref()
            .map(|panel| (!panel.is_panelized()).then(|| panel.cwd.clone()));
        let tree = state.directory_tree();
        let tree_key = tree.map(|tree| (tree.root().to_path_buf(), tree.entries_revision()));
        if panels == self.panels && tree_key == self.tree_key {
            return;
        }
        for (index, path) in panels.iter().enumerate() {
            if path != &self.panels[index] {
                self.dirty_panels[index] = None;
            }
        }
        self.panels = panels;
        if tree_key != self.tree_key {
            self.tree_directories = tree
                .into_iter()
                .flat_map(|tree| tree.entries().iter().map(|entry| entry.path.clone()))
                .collect();
            self.dirty_tree
                .retain(|path| self.tree_directories.contains(path));
            if self.dirty_tree.is_empty() {
                self.tree_dirty_since = None;
            }
            self.tree_key = tree_key;
        }
        let mut directories = self.tree_directories.clone();
        directories.extend(self.panels.iter().flatten().cloned());
        let mut roots: BTreeSet<_> = self.panels.iter().flatten().cloned().collect();
        roots.extend(self.tree_key.iter().map(|(root, _)| root.clone()));
        let targets = Arc::new(Targets {
            directories,
            roots,
            tree_root: self.tree_key.as_ref().map(|(root, _)| root.clone()),
        });
        self.targets_tx.send_if_modified(|current| {
            if *current == targets {
                return false;
            }
            *current = targets;
            true
        });
    }
}

#[derive(Default)]
struct Registrations {
    // One physical watch can serve both panels and any lexical/symlink aliases.
    aliases: BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    watched: BTreeSet<PathBuf>,
    recursive: BTreeSet<PathBuf>,
    roots: BTreeSet<PathBuf>,
    retry: bool,
    obsolete: BTreeSet<PathBuf>,
}

impl Registrations {
    fn reconcile(
        &mut self,
        watcher: &mut impl Watcher,
        targets: &Targets,
        changes: &mut Changes,
        reset: bool,
    ) {
        let mut aliases: BTreeMap<PathBuf, BTreeSet<PathBuf>> = BTreeMap::new();
        let mut desired = BTreeSet::new();
        self.retry = false;
        for path in &targets.directories {
            let physical = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            aliases
                .entry(physical.clone())
                .or_default()
                .insert(path.clone());
            desired.insert(physical);
        }
        self.roots = targets.roots.clone();
        for root in &targets.roots {
            if let Ok(physical) = std::fs::canonicalize(root) {
                self.roots.insert(physical);
            }
            // Events name a symlink in its physical parent (e.g. /private/tmp),
            // not necessarily in the lexical parent displayed by the panel.
            if let (Some(parent), Some(name)) = (root.parent(), root.file_name())
                && let Ok(parent) = std::fs::canonicalize(parent)
            {
                self.roots.insert(parent.join(name));
            }
        }
        self.roots.extend(symlink_dependencies(&targets.roots));
        // Ancestor sentinels detect deletion/replacement of watched roots and
        // symlink retargeting, and allow a missing directory to reappear.
        for root in &self.roots {
            for ancestor in root.ancestors().skip(1) {
                desired.insert(
                    std::fs::canonicalize(ancestor).unwrap_or_else(|_| ancestor.to_path_buf()),
                );
                // FSEvents streams recurse internally even for nonrecursive
                // watches. Keep sentinels local instead of subscribing to /.
                if cfg!(target_os = "macos") {
                    break;
                }
            }
        }
        // Unlike inotify/kqueue, FSEvents recursion costs no directory walk or
        // per-descendant registration. Keep one stream root and filter events
        // through `aliases`, so unloaded descendants still never trigger scans.
        let recursive: BTreeSet<PathBuf> = if cfg!(target_os = "macos") {
            targets
                .tree_root
                .iter()
                .map(|root| std::fs::canonicalize(root).unwrap_or_else(|_| root.clone()))
                .collect()
        } else {
            BTreeSet::new()
        };
        desired.retain(|path| {
            !recursive
                .iter()
                .any(|root| path != root && path.starts_with(root))
        });
        desired.extend(recursive.iter().cloned());
        let removed: Vec<_> = self
            .watched
            .iter()
            .filter(|path| {
                reset
                    || self.obsolete.contains(*path)
                    || !desired.contains(*path)
                    || self.recursive.contains(*path) != recursive.contains(*path)
            })
            .cloned()
            .collect();
        if removed.is_empty() && desired == self.watched {
            self.aliases = aliases;
            self.obsolete.clear();
            return;
        }
        let mut paths = watcher.paths_mut();
        for path in removed {
            let _ = paths.remove(&path);
            self.watched.remove(&path);
        }
        self.obsolete.clear();
        for path in desired
            .difference(&self.watched)
            .cloned()
            .collect::<Vec<_>>()
        {
            let mode = if recursive.contains(&path) {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            match paths.add(&path, mode) {
                Ok(()) => {
                    self.watched.insert(path.clone());
                    if let Some(paths) = aliases.get(&path) {
                        // Watch first, then reconcile the scan-to-watch gap.
                        changes.invalidate(paths.iter().cloned(), true);
                    }
                }
                Err(error) => {
                    self.retry = true;
                    tracing::warn!(path = %path.display(), %error, "filesystem watch unavailable");
                    if changes.warning.is_none() {
                        changes.warning = Some(format!(
                            "Auto-refresh unavailable for {}: {error}",
                            path.display()
                        ));
                    }
                }
            }
        }
        if let Err(error) = paths.commit() {
            self.retry = true;
            self.obsolete.extend(self.watched.iter().cloned());
            changes.warning = Some(format!("Auto-refresh unavailable: {error}"));
        }
        // Updating FSEvents paths restarts the stream. Reconcile its brief gap
        // for retained directories as well as newly registered ones.
        if cfg!(target_os = "macos") {
            changes.invalidate(targets.directories.iter().cloned(), true);
        }
        self.recursive = recursive;
        self.aliases = aliases;
    }

    /// Returns true if watches might now refer to obsolete filesystem objects.
    fn event(&mut self, event: &Event, changes: &mut Changes) -> bool {
        if event.need_rescan() || event.paths.is_empty() {
            changes.invalidate(self.aliases.values().flatten().cloned(), true);
            return true;
        }
        if matches!(event.kind, EventKind::Access(_)) {
            return false; // Our own readdir/stat/preview reads must not trigger a loop.
        }
        let structural = tree_change(event.kind);
        let mut reset = false;
        for path in &event.paths {
            for directory in [Some(path.as_path()), path.parent()].into_iter().flatten() {
                if let Some(aliases) = self.aliases.get(directory) {
                    changes.invalidate(aliases.iter().cloned(), structural);
                }
            }
            if root_change(event.kind) && self.roots.iter().any(|root| root.starts_with(path)) {
                changes.invalidate(self.aliases.values().flatten().cloned(), true);
                reset = true;
            } else if matches!(
                event.kind,
                EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_))
            ) && self.watched.contains(path)
            {
                self.obsolete.insert(path.clone());
            }
        }
        reset
    }
}

fn symlink_dependencies(roots: &BTreeSet<PathBuf>) -> BTreeSet<PathBuf> {
    let mut pending: Vec<_> = roots.iter().cloned().collect();
    let mut links = BTreeSet::new();
    while let Some(path) = pending.pop() {
        for component in path.ancestors() {
            let Ok(target) = std::fs::read_link(component) else {
                continue;
            };
            let (Some(parent), Some(name)) = (component.parent(), component.file_name()) else {
                continue;
            };
            let Ok(parent) = std::fs::canonicalize(parent) else {
                continue;
            };
            // Keep the link's own identity before following its target. Walking
            // that target's components also finds links hidden inside absolute
            // or relative targets. Physical identities deduplicate shared chains
            // and terminate cycles, including relative targets that grow paths.
            if links.insert(parent.join(name)) {
                pending.push(parent.join(target));
            }
        }
    }
    links
}

fn tree_change(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::Any
            | EventKind::Other
            | EventKind::Create(CreateKind::Any | CreateKind::Folder | CreateKind::Other)
            | EventKind::Remove(RemoveKind::Any | RemoveKind::Folder | RemoveKind::Other)
            | EventKind::Modify(ModifyKind::Name(_) | ModifyKind::Any | ModifyKind::Other)
    )
}

fn root_change(kind: EventKind) -> bool {
    // inotify reports directory symlinks as files when they are unlinked or
    // recreated. Root identity changes are independent of tree-entry changes.
    tree_change(kind) || matches!(kind, EventKind::Create(_) | EventKind::Remove(_))
}

fn run(
    mut targets_rx: watch::Receiver<Arc<Targets>>,
    mailbox: Arc<Mutex<Changes>>,
    tracing_log: Option<PathBuf>,
) {
    // Logging is already initialized. Resolve once on this worker so relative
    // paths and symlinked parent directories match the native event paths.
    let tracing_log = tracing_log.and_then(|path| std::fs::canonicalize(path).ok());
    let (event_tx, event_rx) = mpsc::sync_channel(EVENT_CAPACITY);
    let (interests_tx, interests_rx) = watch::channel(Arc::new(Targets::default()));
    let overflow = Arc::new(AtomicBool::new(false));
    let event_overflow = Arc::clone(&overflow);
    let mut watcher = match notify::recommended_watcher(move |event: notify::Result<Event>| {
        let event = match event {
            Ok(event) => {
                let Some(event) =
                    filter_event(event, &interests_rx.borrow(), tracing_log.as_deref())
                else {
                    return;
                };
                Ok(event)
            }
            error => error,
        };
        if matches!(event_tx.try_send(event), Err(mpsc::TrySendError::Full(_))) {
            event_overflow.store(true, Ordering::Release);
        }
    }) {
        Ok(watcher) => watcher,
        Err(error) => {
            mailbox.lock().expect("watch changes lock").warning =
                Some(format!("Auto-refresh unavailable: {error}"));
            return;
        }
    };
    let mut registrations = Registrations::default();
    let mut targets = Arc::new(Targets::default());
    let mut retry_at = Instant::now();
    let mut last_warning = None;
    loop {
        let mut changes = Changes::default();
        let configuration_changed = match targets_rx.has_changed() {
            Ok(changed) => changed,
            Err(_) => break,
        };
        if configuration_changed {
            let next = Arc::clone(&targets_rx.borrow_and_update());
            changes.invalidate(
                next.directories.difference(&targets.directories).cloned(),
                true,
            );
            targets = next;
        }
        let mut reset = overflow.swap(false, Ordering::AcqRel);
        // Bounded draining prevents continuous writers from starving registration
        // changes, publication, and shutdown. Overflow means reconcile everything.
        let first = event_rx.recv_timeout(WATCH_POLL).ok();
        for event in first
            .into_iter()
            .chain(event_rx.try_iter())
            .take(EVENT_CAPACITY)
        {
            match event {
                Ok(event) => reset |= registrations.event(&event, &mut changes),
                Err(error) => {
                    tracing::warn!(%error, "filesystem notification error");
                    reset = true;
                }
            }
        }
        if reset {
            changes.invalidate(targets.directories.iter().cloned(), true);
        }
        if configuration_changed
            || reset
            || (!registrations.obsolete.is_empty() && !registrations.retry)
            || (registrations.retry && Instant::now() >= retry_at)
        {
            registrations.reconcile(&mut watcher, &targets, &mut changes, reset);
            interests_tx.send_replace(Arc::new(Targets {
                directories: registrations.aliases.keys().cloned().collect(),
                roots: registrations.roots.clone(),
                tree_root: None,
            }));
            retry_at = Instant::now() + RETRY_DELAY;
        }
        if changes.warning == last_warning {
            changes.warning = None;
        } else if changes.warning.is_some() {
            last_warning = changes.warning.clone();
        }
        if !changes.directories.is_empty() || changes.warning.is_some() {
            let mut pending = mailbox.lock().expect("watch changes lock");
            pending
                .directories
                .retain(|path| targets.directories.contains(path));
            pending
                .tree
                .retain(|path| targets.directories.contains(path));
            pending.directories.extend(changes.directories);
            pending.tree.extend(changes.tree);
            pending.warning = changes.warning.or(pending.warning.take());
        }
    }
}

fn filter_event(
    mut event: Event,
    interests: &Targets,
    tracing_log: Option<&Path>,
) -> Option<Event> {
    // A refresh logs its completion. Do not let that write schedule another
    // refresh, or even occupy the bounded event queue. Namespace changes still
    // matter: deleting/renaming the log must remove its old listing entry.
    // Preserve unrelated paths and notification-loss/rescan markers as well.
    // Windows FILE_ACTION_MODIFIED is reported as ModifyKind::Any.
    if !event.need_rescan()
        && matches!(
            event.kind,
            EventKind::Modify(ModifyKind::Any | ModifyKind::Data(_) | ModifyKind::Metadata(_))
        )
        && !event.paths.is_empty()
        && let Some(log) = tracing_log
    {
        event.paths.retain(|path| path != log);
        if event.paths.is_empty() {
            return None;
        }
    }
    relevant_event(&event, interests).then_some(event)
}

fn relevant_event(event: &Event, interests: &Targets) -> bool {
    if event.need_rescan() {
        return true;
    }
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    event.paths.is_empty()
        || event.paths.iter().any(|path| {
            interests.directories.contains(path)
                || path
                    .parent()
                    .is_some_and(|parent| interests.directories.contains(parent))
                || (root_change(event.kind)
                    && interests.roots.iter().any(|root| root.starts_with(path)))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, DataChange, Flag, RenameMode};
    use rc_core::{AppCommand, PanelState};
    use std::fs;
    use std::sync::atomic::AtomicU64;

    static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            loop {
                let id = NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
                let path =
                    std::env::temp_dir().join(format!("rc-watch-{}-{id}", std::process::id()));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(fs::canonicalize(path).unwrap()),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("create fixture {}: {error}", path.display()),
                }
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn registrations(paths: &[&str]) -> Registrations {
        Registrations {
            aliases: paths
                .iter()
                .map(|path| (PathBuf::from(path), BTreeSet::from([PathBuf::from(path)])))
                .collect(),
            watched: paths.iter().map(PathBuf::from).collect(),
            ..Registrations::default()
        }
    }

    #[test]
    fn parallel_fixtures_have_exclusive_directories() {
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    (0..8).map(|_| Fixture::new()).collect::<Vec<_>>()
                })
            })
            .collect();
        let fixtures: Vec<_> = threads
            .into_iter()
            .flat_map(|thread| thread.join().unwrap())
            .collect();
        let paths: BTreeSet<_> = fixtures.iter().map(|fixture| &fixture.0).collect();
        assert_eq!(paths.len(), 128);
        assert!(paths.iter().all(|path| path.is_dir()));
    }

    #[test]
    fn tracing_writes_are_excluded_without_losing_structural_events_or_rescan_markers() {
        let targets = Targets {
            directories: BTreeSet::from(["/watched".into()]),
            ..Targets::default()
        };
        let log = Path::new("/watched/trace.log");
        for kind in [
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            EventKind::Modify(ModifyKind::Metadata(notify::event::MetadataKind::Any)),
        ] {
            assert!(
                filter_event(Event::new(kind).add_path(log.into()), &targets, Some(log)).is_none()
            );
        }
        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Remove(RemoveKind::File),
            EventKind::Modify(ModifyKind::Name(RenameMode::From)),
            EventKind::Any,
            EventKind::Other,
        ] {
            let event =
                filter_event(Event::new(kind).add_path(log.into()), &targets, Some(log)).unwrap();
            assert_eq!(event.kind, kind);
            assert_eq!(event.paths, vec![log.to_path_buf()]);
        }
        let event = filter_event(
            Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
                .add_path(log.into())
                .add_path("/watched/other.log".into()),
            &targets,
            Some(log),
        )
        .unwrap();
        assert_eq!(
            event.paths,
            vec![log.to_path_buf(), PathBuf::from("/watched/other.log")]
        );
        let event = Event::new(EventKind::Other)
            .set_flag(Flag::Rescan)
            .add_path(log.into());
        assert!(
            filter_event(event, &targets, Some(log))
                .unwrap()
                .need_rescan()
        );
        assert!(filter_event(Event::new(EventKind::Any), &targets, Some(log)).is_some());
        assert!(
            filter_event(
                Event::new(EventKind::Create(CreateKind::File)).add_path(log.into()),
                &targets,
                None
            )
            .is_some()
        );
    }

    #[test]
    fn windows_generic_modifications_ignore_only_tracing_writes() {
        let targets = Targets {
            directories: BTreeSet::from(["/watched".into()]),
            ..Targets::default()
        };
        let log = Path::new("/watched/trace.log");
        // notify maps Windows FILE_ACTION_MODIFIED to ModifyKind::Any.
        let write = Event::new(EventKind::Modify(ModifyKind::Any)).add_path(log.into());
        assert!(filter_event(write.clone(), &targets, Some(log)).is_none());
        assert!(filter_event(write.clone(), &targets, None).is_some());
        let mixed = filter_event(
            write.clone().add_path("/watched/other.txt".into()),
            &targets,
            Some(log),
        )
        .unwrap();
        assert_eq!(mixed.paths, vec![PathBuf::from("/watched/other.txt")]);
        assert!(
            filter_event(write.set_flag(Flag::Rescan), &targets, Some(log))
                .unwrap()
                .need_rescan()
        );

        for kind in [
            EventKind::Create(CreateKind::Any),
            EventKind::Remove(RemoveKind::Any),
            EventKind::Modify(ModifyKind::Name(RenameMode::From)),
            EventKind::Modify(ModifyKind::Name(RenameMode::To)),
        ] {
            let event =
                filter_event(Event::new(kind).add_path(log.into()), &targets, Some(log)).unwrap();
            assert_eq!(event.kind, kind);
            assert_eq!(event.paths, vec![log.to_path_buf()]);
            let mut changes = Changes::default();
            registrations(&["/watched"]).event(&event, &mut changes);
            assert!(changes.directories.contains(Path::new("/watched")));
        }
    }

    #[test]
    fn routes_both_rename_endpoints_and_ignores_unloaded_descendants_and_reads() {
        let mut registrations = registrations(&["/a", "/b"]);
        let mut changes = Changes::default();
        registrations.event(
            &Event::new(EventKind::Access(AccessKind::Read)).add_path("/a/file".into()),
            &mut changes,
        );
        registrations.event(
            &Event::new(EventKind::Create(CreateKind::File)).add_path("/a/unloaded/file".into()),
            &mut changes,
        );
        assert!(changes.directories.is_empty());
        let event = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path("/a/old".into())
            .add_path("/b/new".into());
        assert!(!registrations.event(&event, &mut changes));
        assert_eq!(
            changes.directories,
            BTreeSet::from(["/a".into(), "/b".into()])
        );
        assert_eq!(changes.tree, changes.directories);
    }

    #[test]
    fn file_writes_refresh_listings_without_rescanning_the_directory_tree() {
        let mut registrations = registrations(&["/a"]);
        let mut changes = Changes::default();
        for _ in 0..10_000 {
            registrations.event(
                &Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Any)))
                    .add_path("/a/file".into()),
                &mut changes,
            );
        }
        assert_eq!(changes.directories.len(), 1);
        assert!(changes.tree.is_empty());
    }

    #[test]
    fn lost_events_invalidate_every_directory_even_without_paths() {
        let mut registrations = registrations(&["/a", "/b"]);
        let mut changes = Changes::default();
        assert!(registrations.event(
            &Event::new(EventKind::Other).set_flag(Flag::Rescan),
            &mut changes
        ));
        assert_eq!(changes.directories.len(), 2);
        assert_eq!(changes.tree.len(), 2);
    }

    #[test]
    fn callback_filters_read_and_unloaded_subtree_traffic_before_the_bounded_queue() {
        let targets = Targets {
            directories: BTreeSet::from(["/a/loaded".into()]),
            roots: BTreeSet::from(["/a/loaded".into()]),
            tree_root: None,
        };
        assert!(!relevant_event(
            &Event::new(EventKind::Access(AccessKind::Read)).add_path("/a/loaded/file".into()),
            &targets
        ));
        assert!(!relevant_event(
            &Event::new(EventKind::Create(CreateKind::File))
                .add_path("/a/loaded/unloaded/file".into()),
            &targets
        ));
        assert!(relevant_event(
            &Event::new(EventKind::Create(CreateKind::File)).add_path("/a/loaded/file".into()),
            &targets
        ));
        assert!(relevant_event(
            &Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::From)))
                .add_path("/a".into()),
            &targets
        ));
        assert!(relevant_event(
            &Event::new(EventKind::Other).set_flag(Flag::Rescan),
            &targets
        ));
    }

    #[test]
    fn dirty_panels_wait_without_cancellation_and_navigation_discards_old_invalidations() {
        let fixture = Fixture::new();
        let other = fixture.0.join("other");
        fs::create_dir(&other).unwrap();
        let mut app = AppState::new(fixture.0.clone()).unwrap();
        app.refresh_panels();
        let mut watch = FilesystemWatch::spawn(None).unwrap();
        watch.poll(&mut app);
        watch.dirty_panels = [Some(Instant::now() - COALESCE_DELAY); 2];
        watch.poll(&mut app);
        assert!(watch.dirty_panels.iter().all(Option::is_some));
        let commands = app.take_pending_worker_commands();
        assert_eq!(commands.len(), 2);
        assert!(
            commands
                .iter()
                .all(|command| matches!(command, rc_core::WorkerCommand::Run(_)))
        );
        app.panels[0] = PanelState::new(other).unwrap();
        watch.poll(&mut app);
        assert!(watch.dirty_panels[0].is_none());
        assert!(watch.dirty_panels[1].is_some());
    }

    #[test]
    fn deleting_a_cached_subdirectory_only_invalidates_its_parent_and_watch() {
        let mut registrations = registrations(&["/a", "/a/child", "/b"]);
        let mut changes = Changes::default();
        assert!(!registrations.event(
            &Event::new(EventKind::Remove(RemoveKind::Folder)).add_path("/a/child".into()),
            &mut changes
        ));
        assert_eq!(registrations.obsolete, BTreeSet::from(["/a/child".into()]));
        assert!(!changes.directories.contains(&PathBuf::from("/b")));
        assert!(changes.tree.contains(&PathBuf::from("/a")));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_aliases_share_a_physical_watch_and_receive_the_same_invalidation() {
        let fixture = Fixture::new();
        let real = fixture.0.join("real");
        let alias = fixture.0.join("alias");
        fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let targets = Targets {
            directories: BTreeSet::from([real.clone(), alias.clone()]),
            ..Targets::default()
        };
        let mut registrations = Registrations::default();
        let mut watcher = notify::NullWatcher::new(|_| {}, notify::Config::default()).unwrap();
        registrations.reconcile(&mut watcher, &targets, &mut Changes::default(), false);
        assert_eq!(registrations.watched.len(), 1);
        let mut changes = Changes::default();
        registrations.event(
            &Event::new(EventKind::Create(CreateKind::File)).add_path(real.join("new")),
            &mut changes,
        );
        assert_eq!(changes.directories, targets.directories);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_retargeting_is_detected_when_its_parent_also_has_an_alias() {
        let fixture = Fixture::new();
        let parent = fixture.0.join("parent");
        let parent_alias = fixture.0.join("parent-alias");
        let real = fixture.0.join("real");
        fs::create_dir(&parent).unwrap();
        fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&parent, &parent_alias).unwrap();
        std::os::unix::fs::symlink(&real, parent.join("link")).unwrap();
        let root = parent_alias.join("link");
        let targets = Targets {
            directories: BTreeSet::from([root.clone()]),
            roots: BTreeSet::from([root]),
            tree_root: None,
        };
        let mut registrations = Registrations::default();
        let mut watcher = notify::NullWatcher::new(|_| {}, notify::Config::default()).unwrap();
        registrations.reconcile(&mut watcher, &targets, &mut Changes::default(), false);
        let interests = Targets {
            directories: registrations.aliases.keys().cloned().collect(),
            roots: registrations.roots.clone(),
            tree_root: None,
        };
        for kind in [
            EventKind::Remove(RemoveKind::File),
            EventKind::Create(CreateKind::File),
        ] {
            let event = Event::new(kind).add_path(parent.join("link"));
            assert!(relevant_event(&event, &interests));
            assert!(registrations.event(&event, &mut Changes::default()));
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_ancestor_sentinels_survive_canonicalization_and_rearm_to_the_new_target() {
        let fixture = Fixture::new();
        let parent = fixture.0.join("p");
        let old_target = fixture.0.join("old");
        let new_target = fixture.0.join("new");
        fs::create_dir(&parent).unwrap();
        fs::create_dir_all(old_target.join("sub")).unwrap();
        fs::create_dir_all(new_target.join("sub")).unwrap();
        let link = parent.join("link");
        std::os::unix::fs::symlink(&old_target, &link).unwrap();
        let root = link.join("sub");
        let targets = Targets {
            directories: BTreeSet::from([root.clone()]),
            roots: BTreeSet::from([root.clone()]),
            tree_root: None,
        };
        let mut registrations = Registrations::default();
        let mut watcher = notify::NullWatcher::new(|_| {}, notify::Config::default()).unwrap();
        registrations.reconcile(&mut watcher, &targets, &mut Changes::default(), false);
        assert!(
            registrations.watched.contains(&parent),
            "watch the symlink's namespace, not just its target"
        );
        assert!(registrations.roots.contains(&link));
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&new_target, &link).unwrap();
        let event = Event::new(EventKind::Remove(RemoveKind::File)).add_path(link);
        let interests = Targets {
            directories: registrations.aliases.keys().cloned().collect(),
            roots: registrations.roots.clone(),
            tree_root: None,
        };
        assert!(relevant_event(&event, &interests));
        let mut changes = Changes::default();
        assert!(registrations.event(&event, &mut changes));
        assert!(changes.directories.contains(&root));
        registrations.reconcile(&mut watcher, &targets, &mut changes, true);
        assert!(!registrations.aliases.contains_key(&old_target.join("sub")));
        assert_eq!(
            registrations.aliases.get(&new_target.join("sub")),
            Some(&BTreeSet::from([root]))
        );
    }

    #[cfg(unix)]
    #[test]
    fn intermediate_symlink_targets_have_sentinels_for_absolute_and_relative_chains() {
        let fixture = Fixture::new();
        let p = fixture.0.join("p");
        let q = fixture.0.join("q");
        let old = fixture.0.join("old");
        let new = fixture.0.join("new");
        fs::create_dir(&p).unwrap();
        fs::create_dir(&q).unwrap();
        fs::create_dir_all(old.join("sub")).unwrap();
        fs::create_dir_all(new.join("sub")).unwrap();
        let middle = q.join("middle");
        std::os::unix::fs::symlink("../old", &middle).unwrap();
        for (name, target) in [
            ("absolute", middle.clone()),
            ("relative", PathBuf::from("../q/middle")),
        ] {
            let link = p.join(name);
            std::os::unix::fs::symlink(target, &link).unwrap();
            let root = link.join("sub");
            let targets = Targets {
                directories: BTreeSet::from([root.clone()]),
                roots: BTreeSet::from([root.clone()]),
                tree_root: Some(root.clone()),
            };
            let mut registrations = Registrations::default();
            let mut watcher = notify::NullWatcher::new(|_| {}, notify::Config::default()).unwrap();
            registrations.reconcile(&mut watcher, &targets, &mut Changes::default(), false);
            assert!(
                registrations.roots.contains(&middle),
                "retain the intermediate link's identity"
            );
            assert!(
                registrations.watched.contains(&q),
                "watch the intermediate link's parent"
            );
            let interests = Targets {
                directories: registrations.aliases.keys().cloned().collect(),
                roots: registrations.roots.clone(),
                tree_root: None,
            };
            let event = Event::new(EventKind::Remove(RemoveKind::File)).add_path(middle.clone());
            assert!(relevant_event(&event, &interests));
            let mut changes = Changes::default();
            assert!(registrations.event(&event, &mut changes));
            assert!(changes.directories.contains(&root));
            fs::remove_file(&middle).unwrap();
            std::os::unix::fs::symlink("../new", &middle).unwrap();
            registrations.reconcile(&mut watcher, &targets, &mut changes, true);
            assert!(!registrations.aliases.contains_key(&old.join("sub")));
            assert!(registrations.aliases.contains_key(&new.join("sub")));
            fs::remove_file(&middle).unwrap();
            std::os::unix::fs::symlink("../old", &middle).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_dependencies_follow_target_components_and_terminate_cycles() {
        let fixture = Fixture::new();
        let p = fixture.0.join("p");
        let q = fixture.0.join("q");
        let r = fixture.0.join("r");
        fs::create_dir(&p).unwrap();
        fs::create_dir(&q).unwrap();
        fs::create_dir(&r).unwrap();
        let link = p.join("link");
        let through = q.join("through");
        let middle = r.join("middle");
        std::os::unix::fs::symlink("../q/through/middle", &link).unwrap();
        std::os::unix::fs::symlink("../r", &through).unwrap();
        std::os::unix::fs::symlink("../p/link/sub", &middle).unwrap();
        let dependencies = symlink_dependencies(&BTreeSet::from([link.join("child")]));
        assert_eq!(
            dependencies,
            BTreeSet::from([link.clone(), through, middle.clone()])
        );
        // A broken terminal target still needs the intermediate sentinel so
        // recreating/retargeting the link can restore automatic updates.
        fs::remove_file(&middle).unwrap();
        std::os::unix::fs::symlink("../missing", &middle).unwrap();
        assert_eq!(
            symlink_dependencies(&BTreeSet::from([link.join("child")])),
            dependencies
        );
    }

    fn pump_until(
        watch: &mut FilesystemWatch,
        runtime: &mut crate::runtime::RuntimeBridge,
        app: &mut AppState,
        check: impl Fn(&AppState) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            runtime.drain_events(app);
            watch.poll(app);
            runtime.dispatch_pending_commands(app);
            if check(app) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "native watch did not converge: {}",
                app.status_line
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn pump_for(
        watch: &mut FilesystemWatch,
        runtime: &mut crate::runtime::RuntimeBridge,
        app: &mut AppState,
        duration: Duration,
    ) {
        let deadline = Instant::now() + duration;
        pump_until(watch, runtime, app, |_| Instant::now() >= deadline);
    }

    #[test]
    fn native_notifications_do_not_refresh_in_response_to_the_apps_tracing_log() {
        // Install the app's global logging setup in an isolated test process,
        // without changing subscribers or callsite interests in parallel tests.
        const CHILD: &str = "RC_TEST_TRACING_REFRESH_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "filesystem_watch::tests::native_notifications_do_not_refresh_in_response_to_the_apps_tracing_log", "--nocapture"])
                .env(CHILD, "1")
                .output().unwrap();
            assert!(
                output.status.success(),
                "tracing regression failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let fixture = Fixture::new();
        let log_path = fixture.0.join("trace.log");
        let writer = crate::open_tracing_log(&log_path).unwrap();
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new("debug"))
            .with_writer(Mutex::new(writer))
            .with_ansi(false)
            .try_init()
            .unwrap();
        {
            let mut app = AppState::new(fixture.0.clone()).unwrap();
            let mut watch = FilesystemWatch::spawn(Some(log_path.clone())).unwrap();
            let mut runtime = crate::runtime::RuntimeBridge::spawn().unwrap();
            pump_until(&mut watch, &mut runtime, &mut app, |app| {
                app.panels.iter().all(|panel| {
                    !panel.loading && panel.entries.iter().any(|e| e.name == "trace.log")
                })
            });
            pump_for(
                &mut watch,
                &mut runtime,
                &mut app,
                Duration::from_millis(700),
            );
            fs::write(
                fixture.0.join("external"),
                "external change must still refresh",
            )
            .unwrap();
            pump_until(&mut watch, &mut runtime, &mut app, |app| {
                app.panels
                    .iter()
                    .all(|panel| panel.entries.iter().any(|e| e.name == "external"))
            });
            pump_for(
                &mut watch,
                &mut runtime,
                &mut app,
                Duration::from_millis(700),
            );
            let last_job = app.jobs.last_job().map(|job| job.id);
            for _ in 0..20 {
                tracing::info!("write to the watched tracing log");
            }
            pump_for(
                &mut watch,
                &mut runtime,
                &mut app,
                Duration::from_millis(1200),
            );
            assert_eq!(
                app.jobs.last_job().map(|job| job.id),
                last_job,
                "logging must not schedule refreshes"
            );
            let log = fs::read_to_string(&log_path).unwrap();
            assert!(
                log.contains("job finished successfully"),
                "refresh logging missing: {log}"
            );
            // The writer remains open, but removing its directory entry is a
            // real namespace change and must update both panes immediately.
            fs::remove_file(&log_path).unwrap();
            pump_until(&mut watch, &mut runtime, &mut app, |app| {
                app.panels
                    .iter()
                    .all(|panel| !panel.entries.iter().any(|e| e.name == "trace.log"))
            });
            pump_for(
                &mut watch,
                &mut runtime,
                &mut app,
                Duration::from_millis(700),
            );
            let last_job = app.jobs.last_job().map(|job| job.id);
            tracing::info!("writing to the unlinked tracing log");
            pump_for(
                &mut watch,
                &mut runtime,
                &mut app,
                Duration::from_millis(700),
            );
            assert_eq!(app.jobs.last_job().map(|job| job.id), last_job);
            drop(watch);
            runtime.shutdown().unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn native_notifications_follow_a_retargeted_symlink_ancestor_in_both_panes_and_tree() {
        check_native_symlink_retarget(false);
    }

    #[cfg(unix)]
    #[test]
    fn native_notifications_follow_a_retargeted_intermediate_symlink_in_both_panes_and_tree() {
        check_native_symlink_retarget(true);
    }

    #[cfg(unix)]
    fn check_native_symlink_retarget(intermediate: bool) {
        let fixture = Fixture::new();
        let parent = fixture.0.join("p");
        let old_target = fixture.0.join("old");
        let new_target = fixture.0.join("new");
        fs::create_dir(&parent).unwrap();
        fs::create_dir_all(old_target.join("sub/before")).unwrap();
        fs::create_dir_all(new_target.join("sub/after")).unwrap();
        let link = parent.join("link");
        let retarget = if intermediate {
            let q = fixture.0.join("q");
            fs::create_dir(&q).unwrap();
            let middle = q.join("middle");
            std::os::unix::fs::symlink(&old_target, &middle).unwrap();
            std::os::unix::fs::symlink("../q/middle", &link).unwrap();
            middle
        } else {
            std::os::unix::fs::symlink(&old_target, &link).unwrap();
            link.clone()
        };
        let mut app = AppState::new(link.join("sub")).unwrap();
        app.apply(AppCommand::OpenTree).unwrap();
        let mut watch = FilesystemWatch::spawn(None).unwrap();
        let mut runtime = crate::runtime::RuntimeBridge::spawn().unwrap();
        pump_until(&mut watch, &mut runtime, &mut app, |app| {
            app.directory_tree().is_some_and(|tree| {
                tree.scan_job_id().is_none()
                    && tree.entries().iter().any(|e| e.path.ends_with("before"))
            })
        });
        pump_for(
            &mut watch,
            &mut runtime,
            &mut app,
            Duration::from_millis(700),
        );
        fs::remove_file(&retarget).unwrap();
        std::os::unix::fs::symlink(&new_target, &retarget).unwrap();
        pump_until(&mut watch, &mut runtime, &mut app, |app| {
            app.panels.iter().all(|panel| {
                panel.entries.iter().any(|e| e.name == "after")
                    && !panel.entries.iter().any(|e| e.name == "before")
            }) && app.directory_tree().is_some_and(|tree| {
                tree.entries().iter().any(|e| e.path.ends_with("after"))
                    && !tree.entries().iter().any(|e| e.path.ends_with("before"))
            })
        });
        pump_for(
            &mut watch,
            &mut runtime,
            &mut app,
            Duration::from_millis(400),
        );
        fs::create_dir(new_target.join("sub/after-rearm")).unwrap();
        pump_until(&mut watch, &mut runtime, &mut app, |app| {
            app.panels
                .iter()
                .all(|panel| panel.entries.iter().any(|e| e.name == "after-rearm"))
                && app.directory_tree().is_some_and(|tree| {
                    tree.entries()
                        .iter()
                        .any(|e| e.path.ends_with("after-rearm"))
                })
        });
        drop(watch);
        runtime.shutdown().unwrap();
    }

    #[test]
    fn native_notifications_update_both_panes_and_nested_tree_and_follow_navigation() {
        let fixture = Fixture::new();
        let root = &fixture.0;
        let child = root.join("child");
        let other = root.join("other");
        fs::create_dir(&child).unwrap();
        fs::create_dir(&other).unwrap();
        let mut app = AppState::new(root.clone()).unwrap();
        app.apply(AppCommand::OpenTree).unwrap();
        let mut watch = FilesystemWatch::spawn(None).unwrap();
        let mut runtime = crate::runtime::RuntimeBridge::spawn().unwrap();
        pump_until(&mut watch, &mut runtime, &mut app, |app| {
            app.directory_tree().is_some_and(|tree| {
                tree.scan_job_id().is_none() && tree.entries().iter().any(|e| e.path == child)
            })
        });
        // Let watch registration finish before writing, so this exercises actual
        // OS events rather than only the initial scan-to-watch reconciliation.
        let settle_until = Instant::now() + Duration::from_millis(700);
        pump_until(&mut watch, &mut runtime, &mut app, |_| {
            Instant::now() >= settle_until
        });
        fs::write(root.join("external"), "outside the app").unwrap();
        fs::create_dir(child.join("nested")).unwrap();
        pump_until(&mut watch, &mut runtime, &mut app, |app| {
            app.panels
                .iter()
                .all(|p| p.entries.iter().any(|e| e.name == "external"))
                && app
                    .directory_tree()
                    .unwrap()
                    .entries()
                    .iter()
                    .any(|e| e.path == child.join("nested"))
        });
        fs::rename(root.join("external"), root.join("renamed")).unwrap();
        fs::remove_dir(child.join("nested")).unwrap();
        pump_until(&mut watch, &mut runtime, &mut app, |app| {
            app.panels.iter().all(|p| {
                p.entries.iter().any(|e| e.name == "renamed")
                    && !p.entries.iter().any(|e| e.name == "external")
            }) && !app
                .directory_tree()
                .unwrap()
                .entries()
                .iter()
                .any(|e| e.path == child.join("nested"))
        });
        app.panels[1] = PanelState::new(other.clone()).unwrap();
        watch.poll(&mut app);
        fs::write(other.join("new-directory"), "new watch").unwrap();
        pump_until(&mut watch, &mut runtime, &mut app, |app| {
            app.panels[1]
                .entries
                .iter()
                .any(|e| e.name == "new-directory")
        });
        assert_eq!(app.panels[1].cwd, other);
        assert!(
            !app.panels[0]
                .entries
                .iter()
                .any(|e| e.name == "new-directory")
        );
        // Replace a watched directory itself, then verify the watch follows the
        // pathname instead of continuing to observe only the old inode/handle.
        fs::rename(&other, root.join("old-other")).unwrap();
        fs::create_dir(&other).unwrap();
        fs::write(other.join("replacement"), "replacement directory").unwrap();
        pump_until(&mut watch, &mut runtime, &mut app, |app| {
            app.panels[1]
                .entries
                .iter()
                .any(|e| e.name == "replacement")
                && !app.panels[1]
                    .entries
                    .iter()
                    .any(|e| e.name == "new-directory")
        });
        let settle_until = Instant::now() + Duration::from_millis(400);
        pump_until(&mut watch, &mut runtime, &mut app, |_| {
            Instant::now() >= settle_until
        });
        fs::write(other.join("after-rearm"), "watch still works").unwrap();
        pump_until(&mut watch, &mut runtime, &mut app, |app| {
            app.panels[1]
                .entries
                .iter()
                .any(|e| e.name == "after-rearm")
        });
        drop(watch);
        runtime.shutdown().unwrap();
    }
}
