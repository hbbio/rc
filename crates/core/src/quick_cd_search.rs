use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet, VecDeque};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

use crate::quick_cd::resolve_quick_cd_input;

pub const DEFAULT_QUICK_CD_MAX_RESULTS: usize = 64;
pub const DEFAULT_QUICK_CD_MAX_DIRECTORIES: usize = 50_000;

const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(40);
const SNAPSHOT_DIRECTORY_INTERVAL: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuickCdSearchSpec {
    pub query: String,
    pub cwd: PathBuf,
    pub home: Option<PathBuf>,
    pub root: PathBuf,
    pub previous_directory: Option<PathBuf>,
    pub max_results: usize,
    pub max_directories: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuickCdSuggestion {
    pub path: PathBuf,
    pub display: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct QuickCdSearchSnapshot {
    pub suggestions: Vec<QuickCdSuggestion>,
    pub visited_directories: usize,
    pub skipped_directories: usize,
    pub truncated: bool,
    pub complete: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuickCdSearchError {
    Canceled,
    ResultSinkDisconnected,
}

impl fmt::Display for QuickCdSearchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Canceled => formatter.write_str(crate::JOB_CANCELED_MESSAGE),
            Self::ResultSinkDisconnected => {
                formatter.write_str("background event channel disconnected")
            }
        }
    }
}

impl std::error::Error for QuickCdSearchError {}

pub fn run_quick_cd_search(
    spec: &QuickCdSearchSpec,
    cancel_flag: &AtomicBool,
    mut emit_snapshot: impl FnMut(QuickCdSearchSnapshot) -> bool,
) -> Result<QuickCdSearchSnapshot, QuickCdSearchError> {
    ensure_not_canceled(cancel_flag)?;

    let needle = normalized_query(&spec.query);
    let mut search = RankedSuggestions::new(spec.max_results, spec);
    if let Ok(path) =
        resolve_quick_cd_input(&spec.query, &spec.cwd, spec.previous_directory.as_deref())
        && fs::metadata(&path).is_ok_and(|metadata| metadata.is_dir())
    {
        search.insert_exact(path);
    }

    let mut queue = VecDeque::new();
    let mut discovered = HashSet::new();
    for root in search_roots(spec) {
        if discovered.insert(root.clone()) {
            queue.push_back(root);
        }
    }

    let max_directories = spec.max_directories.max(1);
    let mut visited_directories = 0_usize;
    let mut skipped_directories = 0_usize;
    let mut truncated = false;
    let mut last_emit = Instant::now();
    let mut emitted_once = false;

    while let Some(directory) = queue.pop_front() {
        ensure_not_canceled(cancel_flag)?;
        if visited_directories >= max_directories {
            truncated = true;
            break;
        }
        visited_directories = visited_directories.saturating_add(1);

        let changed = search.insert_match(directory.clone(), &needle);
        let read_dir = match fs::read_dir(&directory) {
            Ok(read_dir) => read_dir,
            Err(_) => {
                skipped_directories = skipped_directories.saturating_add(1);
                if changed {
                    emit_progress_if_due(
                        &search,
                        visited_directories,
                        skipped_directories,
                        truncated,
                        &mut last_emit,
                        &mut emitted_once,
                        &mut emit_snapshot,
                    )?;
                }
                continue;
            }
        };

        let mut children = Vec::new();
        for entry in read_dir {
            ensure_not_canceled(cancel_flag)?;
            let Ok(entry) = entry else {
                skipped_directories = skipped_directories.saturating_add(1);
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                skipped_directories = skipped_directories.saturating_add(1);
                continue;
            };
            if file_type.is_dir() {
                children.push((entry.file_name(), entry.path()));
            }
        }
        children.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));

        for (_, child) in children {
            if discovered.len() >= max_directories {
                truncated = true;
                break;
            }
            if discovered.insert(child.clone()) {
                queue.push_back(child);
            }
        }

        if changed
            || visited_directories.is_multiple_of(SNAPSHOT_DIRECTORY_INTERVAL) && !search.is_empty()
        {
            emit_progress_if_due(
                &search,
                visited_directories,
                skipped_directories,
                truncated,
                &mut last_emit,
                &mut emitted_once,
                &mut emit_snapshot,
            )?;
        }
    }

    ensure_not_canceled(cancel_flag)?;
    let final_snapshot = QuickCdSearchSnapshot {
        suggestions: search.snapshot(),
        visited_directories,
        skipped_directories,
        truncated: truncated || !queue.is_empty(),
        complete: true,
    };
    if !emit_snapshot(final_snapshot.clone()) {
        return Err(QuickCdSearchError::ResultSinkDisconnected);
    }
    Ok(final_snapshot)
}

fn emit_progress_if_due(
    search: &RankedSuggestions<'_>,
    visited_directories: usize,
    skipped_directories: usize,
    truncated: bool,
    last_emit: &mut Instant,
    emitted_once: &mut bool,
    emit_snapshot: &mut impl FnMut(QuickCdSearchSnapshot) -> bool,
) -> Result<(), QuickCdSearchError> {
    if *emitted_once && last_emit.elapsed() < SNAPSHOT_INTERVAL {
        return Ok(());
    }
    let snapshot = QuickCdSearchSnapshot {
        suggestions: search.snapshot(),
        visited_directories,
        skipped_directories,
        truncated,
        complete: false,
    };
    if !emit_snapshot(snapshot) {
        return Err(QuickCdSearchError::ResultSinkDisconnected);
    }
    *last_emit = Instant::now();
    *emitted_once = true;
    Ok(())
}

fn ensure_not_canceled(cancel_flag: &AtomicBool) -> Result<(), QuickCdSearchError> {
    if cancel_flag.load(AtomicOrdering::Relaxed) {
        Err(QuickCdSearchError::Canceled)
    } else {
        Ok(())
    }
}

fn search_roots(spec: &QuickCdSearchSpec) -> Vec<PathBuf> {
    let mut roots = Vec::with_capacity(3);
    roots.push(spec.cwd.clone());
    if let Some(home) = &spec.home {
        roots.push(home.clone());
    }
    roots.push(spec.root.clone());
    roots
}

fn normalized_query(query: &str) -> String {
    let trimmed = query.trim();
    let candidate = match shlex::split(trimmed) {
        Some(arguments) if arguments.len() == 1 => arguments.into_iter().next().unwrap_or_default(),
        _ => trimmed
            .trim_matches(|character| character == '\'' || character == '"')
            .to_string(),
    };
    fold_for_search(&candidate)
}

fn fold_for_search(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .map(|character| {
            if character == std::path::MAIN_SEPARATOR {
                '/'
            } else {
                character
            }
        })
        .collect()
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct MatchRank {
    tier: u8,
    anchor: u8,
    offset: usize,
    depth: usize,
    display_length: usize,
    lexical: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RankedSuggestion {
    rank: MatchRank,
    suggestion: QuickCdSuggestion,
}

impl Ord for RankedSuggestion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank.cmp(&other.rank).then_with(|| {
            self.suggestion
                .path
                .as_os_str()
                .cmp(other.suggestion.path.as_os_str())
        })
    }
}

impl PartialOrd for RankedSuggestion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

struct RankedSuggestions<'a> {
    limit: usize,
    spec: &'a QuickCdSearchSpec,
    heap: BinaryHeap<RankedSuggestion>,
    retained_paths: HashSet<PathBuf>,
}

impl<'a> RankedSuggestions<'a> {
    fn new(limit: usize, spec: &'a QuickCdSearchSpec) -> Self {
        Self {
            limit,
            spec,
            heap: BinaryHeap::with_capacity(limit),
            retained_paths: HashSet::with_capacity(limit),
        }
    }

    fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    fn insert_exact(&mut self, path: PathBuf) -> bool {
        let display = display_path(&path, &self.spec.cwd, self.spec.home.as_deref());
        let rank = self.rank(0, 0, &path, &display);
        self.insert(path, display, rank)
    }

    fn insert_match(&mut self, path: PathBuf, needle: &str) -> bool {
        if needle.is_empty() || self.retained_paths.contains(&path) {
            return false;
        }
        let display = display_path(&path, &self.spec.cwd, self.spec.home.as_deref());
        let absolute = fold_for_search(&path.to_string_lossy());
        let display_folded = fold_for_search(&display);
        let basename = path
            .file_name()
            .map(|name| fold_for_search(&name.to_string_lossy()))
            .unwrap_or_else(|| absolute.clone());
        let components: Vec<String> = path
            .components()
            .map(|component| fold_for_search(&component.as_os_str().to_string_lossy()))
            .collect();
        let Some((tier, offset)) =
            match_rank(needle, &basename, &components, &display_folded, &absolute)
        else {
            return false;
        };
        let rank = self.rank(tier, offset, &path, &display);
        self.insert(path, display, rank)
    }

    fn rank(&self, tier: u8, offset: usize, path: &Path, display: &str) -> MatchRank {
        MatchRank {
            tier,
            anchor: anchor_priority(path, &self.spec.cwd, self.spec.home.as_deref()),
            offset,
            depth: path.components().count(),
            display_length: display.chars().count(),
            lexical: fold_for_search(display),
        }
    }

    fn insert(&mut self, path: PathBuf, display: String, rank: MatchRank) -> bool {
        if self.limit == 0 || self.retained_paths.contains(&path) {
            return false;
        }
        let candidate = RankedSuggestion {
            rank,
            suggestion: QuickCdSuggestion {
                path: path.clone(),
                display,
            },
        };
        if self.heap.len() < self.limit {
            self.heap.push(candidate);
            self.retained_paths.insert(path);
            return true;
        }

        let Some(worst) = self.heap.peek() else {
            return false;
        };
        if candidate >= *worst {
            return false;
        }
        if let Some(removed) = self.heap.pop() {
            self.retained_paths.remove(&removed.suggestion.path);
        }
        self.heap.push(candidate);
        self.retained_paths.insert(path);
        true
    }

    fn snapshot(&self) -> Vec<QuickCdSuggestion> {
        let mut ranked: Vec<_> = self.heap.iter().cloned().collect();
        ranked.sort_unstable();
        ranked
            .into_iter()
            .map(|candidate| candidate.suggestion)
            .collect()
    }
}

fn match_rank(
    needle: &str,
    basename: &str,
    components: &[String],
    display: &str,
    absolute: &str,
) -> Option<(u8, usize)> {
    if basename == needle {
        return Some((1, 0));
    }
    if let Some(offset) = basename.find(needle)
        && offset == 0
    {
        return Some((2, 0));
    }
    if components.iter().any(|component| component == needle) {
        return Some((3, 0));
    }
    if display == needle || absolute == needle {
        return Some((3, 0));
    }
    if let Some(offset) = display.find(needle)
        && offset == 0
    {
        return Some((4, 0));
    }
    if let Some(offset) = absolute.find(needle)
        && offset == 0
    {
        return Some((4, 0));
    }
    if let Some(offset) = basename.find(needle) {
        return Some((5, offset));
    }
    if let Some(offset) = components
        .iter()
        .filter_map(|component| component.find(needle))
        .min()
    {
        return Some((6, offset));
    }
    display
        .find(needle)
        .map(|offset| (7, offset))
        .or_else(|| absolute.find(needle).map(|offset| (8, offset)))
}

fn anchor_priority(path: &Path, cwd: &Path, home: Option<&Path>) -> u8 {
    if path.starts_with(cwd) {
        0
    } else if home.is_some_and(|home| path.starts_with(home)) {
        1
    } else {
        2
    }
}

fn display_path(path: &Path, cwd: &Path, home: Option<&Path>) -> String {
    if path == cwd {
        return String::from(".");
    }
    if let Ok(relative) = path.strip_prefix(cwd) {
        return format!("./{}", relative.to_string_lossy());
    }
    if let Some(home) = home {
        if path == home {
            return String::from("~");
        }
        if let Ok(relative) = path.strip_prefix(home) {
            return format!("~/{}", relative.to_string_lossy());
        }
    }
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::sync::atomic::AtomicBool;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-quick-cd-search-{label}-{stamp}"));
        fs::create_dir_all(&root).expect("temp root should be creatable");
        root
    }

    fn spec(query: &str, cwd: PathBuf, home: PathBuf, root: PathBuf) -> QuickCdSearchSpec {
        QuickCdSearchSpec {
            query: query.to_string(),
            cwd,
            home: Some(home),
            root,
            previous_directory: None,
            max_results: 16,
            max_directories: 128,
        }
    }

    #[test]
    fn search_ranks_equal_matches_by_cwd_then_home_then_root() {
        let root = temp_root("anchors");
        let cwd = root.join("work");
        let home = root.join("home");
        let cwd_match = cwd.join("needle");
        let home_match = home.join("needle");
        let root_match = root.join("elsewhere").join("needle");
        for directory in [&cwd_match, &home_match, &root_match] {
            fs::create_dir_all(directory).expect("fixture directory should be creatable");
        }

        let mut snapshots = Vec::new();
        let final_snapshot = run_quick_cd_search(
            &spec("needle", cwd, home, root.clone()),
            &AtomicBool::new(false),
            |snapshot| {
                snapshots.push(snapshot);
                true
            },
        )
        .expect("search should succeed");

        let paths: Vec<_> = final_snapshot
            .suggestions
            .iter()
            .map(|suggestion| suggestion.path.as_path())
            .collect();
        assert_eq!(paths[..3], [&cwd_match, &home_match, &root_match]);
        assert_eq!(
            final_snapshot.visited_directories, 7,
            "overlapping cwd/home/root seeds should be traversed once"
        );
        assert!(snapshots.iter().any(|snapshot| !snapshot.complete));
        assert!(snapshots.last().is_some_and(|snapshot| snapshot.complete));

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn search_matches_case_insensitive_path_substrings() {
        let root = temp_root("substring");
        let cwd = root.join("work");
        let home = root.join("home");
        let match_path = home.join("Archive").join("Project-Delta");
        fs::create_dir_all(&cwd).expect("cwd should be creatable");
        fs::create_dir_all(&match_path).expect("matching path should be creatable");

        let final_snapshot = run_quick_cd_search(
            &spec("archive/project-d", cwd, home, root.clone()),
            &AtomicBool::new(false),
            |_| true,
        )
        .expect("search should succeed");

        assert!(
            final_snapshot
                .suggestions
                .iter()
                .any(|suggestion| suggestion.path == match_path)
        );
        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn search_is_bounded_and_cancellable() {
        let root = temp_root("bounds");
        for index in 0..12 {
            fs::create_dir_all(root.join(format!("branch-{index}")).join("match"))
                .expect("fixture branch should be creatable");
        }
        let mut bounded = spec("match", root.clone(), root.clone(), root.clone());
        bounded.max_directories = 4;

        let final_snapshot = run_quick_cd_search(&bounded, &AtomicBool::new(false), |_| true)
            .expect("bounded search should succeed");
        assert!(final_snapshot.truncated);
        assert!(final_snapshot.visited_directories <= 4);

        let canceled = AtomicBool::new(true);
        assert_eq!(
            run_quick_cd_search(&bounded, &canceled, |_| true),
            Err(QuickCdSearchError::Canceled)
        );
        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[cfg(unix)]
    #[test]
    fn search_does_not_follow_directory_symlinks() {
        use std::os::unix::fs::symlink;

        let root = temp_root("symlink");
        let outside = root.with_extension("outside");
        let hidden_match = outside.join("only-through-link");
        fs::create_dir_all(&hidden_match).expect("outside fixture should be creatable");
        symlink(&outside, root.join("linked")).expect("directory symlink should be creatable");

        let final_snapshot = run_quick_cd_search(
            &spec(
                "only-through-link",
                root.clone(),
                root.clone(),
                root.clone(),
            ),
            &AtomicBool::new(false),
            |_| true,
        )
        .expect("search should succeed");

        assert!(
            final_snapshot.suggestions.is_empty(),
            "a directory reachable only through a symlink must not be traversed"
        );
        fs::remove_dir_all(root).expect("temp root should be removable");
        fs::remove_dir_all(outside).expect("outside fixture should be removable");
    }
}
