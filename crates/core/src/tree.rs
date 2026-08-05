use std::collections::{BTreeSet, BinaryHeap, HashMap};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

use crate::{JOB_CANCELED_MESSAGE, JobId};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TreeEntryScan {
    depth_limit_reached: bool,
    entry_limit_reached: bool,
    skipped_items: usize,
    first_issue: Option<TreeScanIssue>,
}

impl TreeEntryScan {
    fn record_issue(&mut self, path: PathBuf, error: &io::Error) {
        self.skipped_items = self.skipped_items.saturating_add(1);
        if self.first_issue.is_none() {
            self.first_issue = Some(TreeScanIssue {
                path,
                message: error.to_string(),
            });
        }
    }

    fn merge(&mut self, other: Self) {
        self.depth_limit_reached |= other.depth_limit_reached;
        self.entry_limit_reached |= other.entry_limit_reached;
        self.skipped_items = self.skipped_items.saturating_add(other.skipped_items);
        if self.first_issue.is_none() {
            self.first_issue = other.first_issue;
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeEntry {
    pub path: PathBuf,
    pub depth: usize,
    scan: TreeEntryScan,
}

impl TreeEntry {
    fn new(path: PathBuf, depth: usize) -> Self {
        Self {
            path,
            depth,
            scan: TreeEntryScan::default(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeScanIssue {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TreeScanSummary {
    pub depth_limit_reached: bool,
    pub entry_limit_reached: bool,
    pub skipped_items: usize,
    pub first_issue: Option<TreeScanIssue>,
}

impl TreeScanSummary {
    pub fn is_truncated(&self) -> bool {
        self.depth_limit_reached || self.entry_limit_reached
    }

    fn from_entries(entries: &[TreeEntry]) -> Self {
        let mut summary = Self::default();
        for entry in entries {
            summary.depth_limit_reached |= entry.scan.depth_limit_reached;
            summary.entry_limit_reached |= entry.scan.entry_limit_reached;
            summary.skipped_items = summary
                .skipped_items
                .saturating_add(entry.scan.skipped_items);
            if summary.first_issue.is_none() {
                summary.first_issue = entry.scan.first_issue.clone();
            }
        }
        summary
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeBuildResult {
    pub entries: Vec<TreeEntry>,
    pub summary: TreeScanSummary,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TreeLoadState {
    Loading,
    Ready(TreeScanSummary),
    Canceled,
    Failed(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreeNavigationMode {
    Dynamic,
    Static,
}

impl TreeNavigationMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Dynamic => "dynamic",
            Self::Static => "static",
        }
    }
}

#[derive(Clone, Debug, Default)]
struct TreeIndex {
    parents: Vec<Option<usize>>,
    subtree_ends: Vec<usize>,
    first_children: Vec<Option<usize>>,
    previous_siblings: Vec<Option<usize>>,
    next_siblings: Vec<Option<usize>>,
    search_keys: Vec<String>,
    positions: HashMap<PathBuf, usize>,
}

impl TreeIndex {
    fn build(entries: &[TreeEntry]) -> Self {
        let len = entries.len();
        let mut index = Self {
            parents: vec![None; len],
            subtree_ends: vec![len; len],
            first_children: vec![None; len],
            previous_siblings: vec![None; len],
            next_siblings: vec![None; len],
            search_keys: entries.iter().map(tree_entry_search_key).collect(),
            positions: HashMap::with_capacity(len),
        };
        let mut open_ancestors = Vec::<usize>::new();
        let mut last_child = vec![None; len];

        for (entry_index, entry) in entries.iter().enumerate() {
            index.positions.insert(entry.path.clone(), entry_index);
            while open_ancestors.len() > entry.depth {
                if let Some(completed) = open_ancestors.pop() {
                    index.subtree_ends[completed] = entry_index;
                }
            }

            let parent = entry
                .depth
                .checked_sub(1)
                .and_then(|depth| open_ancestors.get(depth).copied());
            index.parents[entry_index] = parent;
            if let Some(parent) = parent {
                if index.first_children[parent].is_none() {
                    index.first_children[parent] = Some(entry_index);
                }
                if let Some(previous) = last_child[parent] {
                    index.previous_siblings[entry_index] = Some(previous);
                    index.next_siblings[previous] = Some(entry_index);
                }
                last_child[parent] = Some(entry_index);
            }

            debug_assert_eq!(open_ancestors.len(), entry.depth);
            open_ancestors.push(entry_index);
        }

        while let Some(completed) = open_ancestors.pop() {
            index.subtree_ends[completed] = len;
        }
        index
    }
}

#[derive(Clone, Debug)]
enum TreeScanTarget {
    Full,
    Subtree {
        path: PathBuf,
        base_depth: usize,
        tree_entry_limit: usize,
    },
}

#[derive(Clone, Debug)]
struct PendingTreeScan {
    job_id: JobId,
    root: PathBuf,
    target: TreeScanTarget,
}

#[derive(Clone, Debug)]
pub(crate) struct TreeRescanPlan {
    pub scan_root: PathBuf,
    pub scan_max_depth: usize,
    pub scan_max_entries: usize,
    target_depth: usize,
    tree_entry_limit: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct TreeScanCompletion {
    pub full_scan: bool,
    pub scan_root: PathBuf,
    pub scanned_entries: usize,
    pub known_entries: usize,
    pub summary: TreeScanSummary,
}

#[derive(Clone, Debug)]
pub struct TreeState {
    root: PathBuf,
    entries: Vec<TreeEntry>,
    cursor: usize,
    load_state: TreeLoadState,
    navigation_mode: TreeNavigationMode,
    search_query: String,
    index: TreeIndex,
    visible_indices: Vec<usize>,
    pending_scan: Option<PendingTreeScan>,
}

impl TreeState {
    pub(crate) fn loading(job_id: JobId, root: PathBuf) -> Self {
        let entries = vec![TreeEntry::new(root.clone(), 0)];
        let mut tree = Self {
            root: root.clone(),
            entries,
            cursor: 0,
            load_state: TreeLoadState::Loading,
            navigation_mode: TreeNavigationMode::Dynamic,
            search_query: String::new(),
            index: TreeIndex::default(),
            visible_indices: Vec::new(),
            pending_scan: Some(PendingTreeScan {
                job_id,
                root,
                target: TreeScanTarget::Full,
            }),
        };
        tree.rebuild_index_and_projection();
        tree
    }

    pub fn is_loading(&self) -> bool {
        self.pending_scan.is_some() && matches!(self.load_state, TreeLoadState::Loading)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn entries(&self) -> &[TreeEntry] {
        &self.entries
    }

    pub fn load_state(&self) -> &TreeLoadState {
        &self.load_state
    }

    pub const fn navigation_mode(&self) -> TreeNavigationMode {
        self.navigation_mode
    }

    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    pub fn scan_job_id(&self) -> Option<JobId> {
        self.pending_scan.as_ref().map(|scan| scan.job_id)
    }

    pub fn visible_entry_count(&self) -> usize {
        self.visible_indices.len()
    }

    pub fn visible_cursor(&self) -> usize {
        self.visible_indices
            .binary_search(&self.cursor)
            .unwrap_or_default()
    }

    pub(crate) fn visible_entry(&self, visible_index: usize) -> Option<&TreeEntry> {
        let entry_index = *self.visible_indices.get(visible_index)?;
        self.entries.get(entry_index)
    }

    pub fn visible_entries(&self) -> impl ExactSizeIterator<Item = &TreeEntry> {
        self.visible_indices
            .iter()
            .map(|&index| &self.entries[index])
    }

    pub fn selected_entry(&self) -> Option<&TreeEntry> {
        self.entries.get(self.cursor)
    }

    pub fn select_path(&mut self, path: &Path) -> bool {
        let Some(index) = self.position(path) else {
            return false;
        };
        self.set_cursor(index);
        true
    }

    pub(crate) fn select_visible_index(&mut self, visible_index: usize) -> bool {
        let Some(&entry_index) = self.visible_indices.get(visible_index) else {
            return false;
        };
        self.set_cursor(entry_index);
        true
    }

    pub(crate) fn plan_selected_rescan(
        &self,
        max_depth: usize,
        max_entries: usize,
    ) -> Option<TreeRescanPlan> {
        self.rescan_plan_for_index(self.cursor, max_depth, max_entries)
    }

    pub(crate) fn plan_rescan_for_impacts(
        &self,
        impacts: &[PathBuf],
        max_depth: usize,
        max_entries: usize,
    ) -> Option<TreeRescanPlan> {
        let mut affected = impacts
            .iter()
            .filter_map(|path| self.deepest_known_ancestor(path));
        let mut common = affected.next()?;
        for affected_index in affected {
            while !(common <= affected_index && affected_index < self.index.subtree_ends[common]) {
                common = self.index.parents[common]?;
            }
        }
        self.rescan_plan_for_index(common, max_depth, max_entries)
    }

    pub(crate) fn begin_rescan(&mut self, job_id: JobId, plan: TreeRescanPlan) {
        self.pending_scan = Some(PendingTreeScan {
            job_id,
            root: plan.scan_root.clone(),
            target: TreeScanTarget::Subtree {
                path: plan.scan_root,
                base_depth: plan.target_depth,
                tree_entry_limit: plan.tree_entry_limit,
            },
        });
        self.load_state = TreeLoadState::Loading;
    }

    pub(crate) fn apply_build_result(
        &mut self,
        job_id: JobId,
        root: &Path,
        result: TreeBuildResult,
    ) -> Option<TreeScanCompletion> {
        if !self
            .pending_scan
            .as_ref()
            .is_some_and(|pending| pending.job_id == job_id && pending.root.as_path() == root)
        {
            return None;
        }
        let pending = self
            .pending_scan
            .take()
            .expect("matching tree scan should still be pending");
        let selected_path = self.selected_entry().map(|entry| entry.path.clone());
        let scanned_entries = result.entries.len();
        let full_scan = matches!(&pending.target, TreeScanTarget::Full);

        match pending.target {
            TreeScanTarget::Full => {
                self.entries = result.entries;
            }
            TreeScanTarget::Subtree {
                path,
                base_depth,
                tree_entry_limit,
            } => {
                let preserve_global_entry_limit = path != self.root
                    && TreeScanSummary::from_entries(&self.entries).entry_limit_reached;
                let Some(start) = self.position(&path) else {
                    self.load_state =
                        TreeLoadState::Ready(TreeScanSummary::from_entries(&self.entries));
                    return None;
                };
                let end = self.index.subtree_ends[start];
                let replacement = result.entries.into_iter().map(|mut entry| {
                    entry.depth = entry.depth.saturating_add(base_depth);
                    entry
                });
                self.entries.splice(start..end, replacement);
                if self.entries.len() > tree_entry_limit {
                    self.entries.truncate(tree_entry_limit);
                    if let Some(last) = self.entries.last_mut() {
                        last.scan.entry_limit_reached = true;
                    }
                } else if preserve_global_entry_limit
                    && !self
                        .entries
                        .iter()
                        .any(|entry| entry.scan.entry_limit_reached)
                    && let Some(last) = self.entries.last_mut()
                {
                    last.scan.entry_limit_reached = true;
                }
            }
        }

        self.index = TreeIndex::build(&self.entries);
        let fallback_path = root.to_path_buf();
        self.cursor = selected_path
            .as_ref()
            .and_then(|path| self.position(path))
            .or_else(|| self.position(&fallback_path))
            .unwrap_or_default();
        self.rebuild_projection();
        let summary = TreeScanSummary::from_entries(&self.entries);
        self.load_state = TreeLoadState::Ready(summary.clone());
        Some(TreeScanCompletion {
            full_scan,
            scan_root: root.to_path_buf(),
            scanned_entries,
            known_entries: self.entries.len(),
            summary,
        })
    }

    pub(crate) fn mark_canceled(&mut self, job_id: JobId) -> bool {
        if !self
            .pending_scan
            .as_ref()
            .is_some_and(|scan| scan.job_id == job_id)
        {
            return false;
        }
        self.pending_scan = None;
        self.load_state = TreeLoadState::Canceled;
        true
    }

    pub(crate) fn mark_failed(&mut self, job_id: JobId, message: String) -> bool {
        if !self
            .pending_scan
            .as_ref()
            .is_some_and(|scan| scan.job_id == job_id)
        {
            return false;
        }
        self.pending_scan = None;
        self.load_state = TreeLoadState::Failed(message);
        true
    }

    pub(crate) fn cancel_scan_for_local_change(&mut self) -> Option<JobId> {
        let job_id = self.pending_scan.take().map(|scan| scan.job_id)?;
        self.load_state = TreeLoadState::Ready(TreeScanSummary::from_entries(&self.entries));
        Some(job_id)
    }

    pub(crate) fn move_cursor(&mut self, delta: isize) {
        let next = match self.navigation_mode {
            TreeNavigationMode::Dynamic if delta < 0 => self.index.previous_siblings[self.cursor],
            TreeNavigationMode::Dynamic if delta > 0 => self.index.next_siblings[self.cursor],
            TreeNavigationMode::Dynamic => None,
            TreeNavigationMode::Static => offset_index(self.cursor, delta, self.entries.len()),
        };
        if let Some(next) = next {
            self.set_cursor(next);
        }
    }

    pub(crate) fn move_parent(&mut self) {
        if let Some(parent) = self.index.parents.get(self.cursor).copied().flatten() {
            self.set_cursor(parent);
        }
    }

    pub(crate) fn move_first_child(&mut self) {
        if let Some(child) = self
            .index
            .first_children
            .get(self.cursor)
            .copied()
            .flatten()
        {
            self.set_cursor(child);
        }
    }

    pub(crate) fn move_page(&mut self, pages: isize, page_step: usize) {
        self.move_visible(pages.saturating_mul(page_step as isize));
    }

    pub(crate) fn move_home(&mut self) {
        if let Some(&first) = self.visible_indices.first() {
            self.set_cursor(first);
        }
    }

    pub(crate) fn move_end(&mut self) {
        if let Some(&last) = self.visible_indices.last() {
            self.set_cursor(last);
        }
    }

    pub(crate) fn toggle_navigation_mode(&mut self) -> TreeNavigationMode {
        self.navigation_mode = match self.navigation_mode {
            TreeNavigationMode::Dynamic => TreeNavigationMode::Static,
            TreeNavigationMode::Static => TreeNavigationMode::Dynamic,
        };
        self.rebuild_projection();
        self.navigation_mode
    }

    pub(crate) fn append_search_char(&mut self, ch: char) -> bool {
        if ch.is_control() {
            return false;
        }
        self.search_query.push(ch);
        self.select_search_match(true)
    }

    pub(crate) fn remove_search_char(&mut self) -> bool {
        if self.search_query.pop().is_none() {
            return false;
        }
        if self.search_query.is_empty() {
            return true;
        }
        self.select_search_match(true)
    }

    pub(crate) fn search_next(&mut self) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        let start = (self.cursor + 1) % self.entries.len();
        if let Some(found) = self.find_search_match(start) {
            self.set_cursor(found);
            return true;
        }
        false
    }

    pub(crate) fn forget_selected(&mut self) -> Option<PathBuf> {
        if self.cursor == 0 || self.entries.is_empty() {
            return None;
        }
        let removed_path = self.entries[self.cursor].path.clone();
        let parent = self.index.parents[self.cursor].unwrap_or_default();
        let end = self.index.subtree_ends[self.cursor];
        self.entries.drain(self.cursor..end);
        self.cursor = parent.min(self.entries.len().saturating_sub(1));
        self.rebuild_index_and_projection();
        self.load_state = TreeLoadState::Ready(TreeScanSummary::from_entries(&self.entries));
        Some(removed_path)
    }

    fn rescan_plan_for_index(
        &self,
        index: usize,
        max_depth: usize,
        max_entries: usize,
    ) -> Option<TreeRescanPlan> {
        let entry = self.entries.get(index)?;
        if max_entries == 0 {
            return None;
        }
        let scan_max_entries = max_entries.checked_sub(index)?.max(1);
        Some(TreeRescanPlan {
            scan_root: entry.path.clone(),
            scan_max_depth: max_depth.saturating_sub(entry.depth),
            scan_max_entries,
            target_depth: entry.depth,
            tree_entry_limit: max_entries,
        })
    }

    fn deepest_known_ancestor(&self, path: &Path) -> Option<usize> {
        path.ancestors()
            .find_map(|ancestor| self.index.positions.get(ancestor).copied())
    }

    fn position(&self, path: &Path) -> Option<usize> {
        self.index.positions.get(path).copied()
    }

    fn move_visible(&mut self, delta: isize) {
        let current = self.visible_cursor();
        if let Some(next) = offset_index(current, delta, self.visible_indices.len()) {
            self.set_cursor(self.visible_indices[next]);
        }
    }

    fn set_cursor(&mut self, cursor: usize) {
        if cursor >= self.entries.len() {
            return;
        }
        self.cursor = cursor;
        self.rebuild_projection();
    }

    fn select_search_match(&mut self, include_current: bool) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        let query = self.search_query.to_lowercase();
        let current_matches = include_current && self.entry_matches_search(self.cursor, &query);
        let found = if current_matches {
            Some(self.cursor)
        } else {
            self.find_search_match_with_query((self.cursor + 1) % self.entries.len(), &query)
        };
        if let Some(found) = found {
            self.set_cursor(found);
            true
        } else {
            false
        }
    }

    fn find_search_match(&self, start: usize) -> Option<usize> {
        let query = self.search_query.to_lowercase();
        self.find_search_match_with_query(start, &query)
    }

    fn find_search_match_with_query(&self, start: usize, query: &str) -> Option<usize> {
        (0..self.entries.len())
            .map(|offset| (start + offset) % self.entries.len())
            .find(|&index| self.entry_matches_search(index, query))
    }

    fn entry_matches_search(&self, index: usize, query: &str) -> bool {
        self.index
            .search_keys
            .get(index)
            .is_some_and(|name| name.starts_with(query))
    }

    fn rebuild_index_and_projection(&mut self) {
        self.index = TreeIndex::build(&self.entries);
        self.cursor = self.cursor.min(self.entries.len().saturating_sub(1));
        self.rebuild_projection();
    }

    fn rebuild_projection(&mut self) {
        self.visible_indices.clear();
        if self.entries.is_empty() {
            return;
        }
        if self.navigation_mode == TreeNavigationMode::Static {
            self.visible_indices.extend(0..self.entries.len());
            return;
        }

        let mut ancestor = Some(self.cursor);
        while let Some(index) = ancestor {
            self.visible_indices.push(index);
            ancestor = self.index.parents[index];
        }

        let mut first_sibling = self.cursor;
        while let Some(previous) = self.index.previous_siblings[first_sibling] {
            first_sibling = previous;
        }
        let mut sibling = Some(first_sibling);
        while let Some(index) = sibling {
            self.visible_indices.push(index);
            sibling = self.index.next_siblings[index];
        }

        let mut child = self.index.first_children[self.cursor];
        while let Some(index) = child {
            self.visible_indices.push(index);
            child = self.index.next_siblings[index];
        }

        self.visible_indices.sort_unstable();
        self.visible_indices.dedup();
    }
}

fn offset_index(index: usize, delta: isize, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(if delta.is_negative() {
        index.saturating_sub(delta.unsigned_abs())
    } else {
        index.saturating_add(delta as usize).min(len - 1)
    })
}

fn tree_entry_search_key(entry: &TreeEntry) -> String {
    entry
        .path
        .file_name()
        .unwrap_or(entry.path.as_os_str())
        .to_string_lossy()
        .to_lowercase()
}

#[derive(Debug, Default)]
pub(crate) struct TreeMutationTracker {
    pending: HashMap<JobId, Vec<PathBuf>>,
    completed_impacts: BTreeSet<PathBuf>,
}

impl TreeMutationTracker {
    pub(crate) fn track(&mut self, job_id: JobId, impacts: Vec<PathBuf>) {
        self.pending.insert(job_id, impacts);
    }

    pub(crate) fn finish(&mut self, job_id: JobId, succeeded: bool) -> Option<Vec<PathBuf>> {
        let impacts = self.pending.remove(&job_id)?;
        if succeeded {
            self.completed_impacts.extend(impacts);
        }
        if self.pending.is_empty() && !self.completed_impacts.is_empty() {
            Some(
                std::mem::take(&mut self.completed_impacts)
                    .into_iter()
                    .collect(),
            )
        } else {
            None
        }
    }
}

pub(crate) fn build_tree_entries(
    root: &Path,
    max_depth: usize,
    max_entries: usize,
    cancel_flag: Option<&AtomicBool>,
) -> io::Result<TreeBuildResult> {
    build_tree_entries_with_cancel_check(root, max_depth, max_entries, || {
        cancel_flag.is_some_and(|flag| flag.load(AtomicOrdering::Relaxed))
    })
}

fn build_tree_entries_with_cancel_check(
    root: &Path,
    max_depth: usize,
    max_entries: usize,
    mut is_canceled: impl FnMut() -> bool,
) -> io::Result<TreeBuildResult> {
    if max_entries == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "tree entry limit must be greater than zero",
        ));
    }
    ensure_not_canceled(&mut is_canceled)?;

    let root_metadata = fs::metadata(root).map_err(|error| contextual_io_error(root, error))?;
    if !root_metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("tree root is not a directory: {}", root.to_string_lossy()),
        ));
    }

    let mut entries = Vec::<TreeEntry>::with_capacity(max_entries.min(256));
    let mut stack = vec![(root.to_path_buf(), 0_usize)];

    while let Some((directory, depth)) = stack.pop() {
        ensure_not_canceled(&mut is_canceled)?;
        if entries.len() >= max_entries {
            if let Some(last) = entries.last_mut() {
                last.scan.entry_limit_reached = true;
            }
            break;
        }

        let current = entries.len();
        entries.push(TreeEntry::new(directory.clone(), depth));

        let child_limit = if depth >= max_depth {
            0
        } else {
            max_entries.saturating_sub(entries.len())
        };
        let children = match collect_child_directories(&directory, child_limit, &mut is_canceled) {
            Ok(children) => children,
            Err(error) if is_canceled_error(&error) => return Err(error),
            Err(error) if depth == 0 => return Err(contextual_io_error(&directory, error)),
            Err(error) => {
                entries[current].scan.record_issue(directory, &error);
                continue;
            }
        };

        entries[current].scan.merge(children.scan);
        if depth >= max_depth {
            entries[current].scan.depth_limit_reached |= children.omitted;
            continue;
        }
        entries[current].scan.entry_limit_reached |= children.omitted;

        for child in children.paths.into_iter().rev() {
            stack.push((child, depth.saturating_add(1)));
        }
    }

    let summary = TreeScanSummary::from_entries(&entries);
    Ok(TreeBuildResult { entries, summary })
}

struct ChildDirectories {
    paths: Vec<PathBuf>,
    omitted: bool,
    scan: TreeEntryScan,
}

fn collect_child_directories(
    directory: &Path,
    limit: usize,
    is_canceled: &mut impl FnMut() -> bool,
) -> io::Result<ChildDirectories> {
    ensure_not_canceled(is_canceled)?;
    let read_dir = fs::read_dir(directory)?;
    let mut smallest = BinaryHeap::<(TreeSortKey, PathBuf)>::new();
    let mut omitted = false;
    let mut scan = TreeEntryScan::default();

    for entry_result in read_dir {
        ensure_not_canceled(is_canceled)?;
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(error) => {
                scan.record_issue(directory.to_path_buf(), &error);
                continue;
            }
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                scan.record_issue(entry.path(), &error);
                continue;
            }
        };
        if !file_type.is_dir() {
            continue;
        }

        if limit == 0 {
            return Ok(ChildDirectories {
                paths: Vec::new(),
                omitted: true,
                scan,
            });
        }

        let path = entry.path();
        smallest.push((tree_sort_key(&path), path));
        if smallest.len() > limit {
            smallest.pop();
            omitted = true;
        }
    }

    ensure_not_canceled(is_canceled)?;
    let mut paths: Vec<PathBuf> = smallest.into_iter().map(|(_key, path)| path).collect();
    paths.sort_by_cached_key(|path| tree_sort_key(path));
    ensure_not_canceled(is_canceled)?;
    Ok(ChildDirectories {
        paths,
        omitted,
        scan,
    })
}

type TreeSortKey = (String, OsString);

fn tree_sort_key(path: &Path) -> TreeSortKey {
    let name = path.file_name().unwrap_or(path.as_os_str());
    (name.to_string_lossy().to_lowercase(), name.to_os_string())
}

fn ensure_not_canceled(is_canceled: &mut impl FnMut() -> bool) -> io::Result<()> {
    if is_canceled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            JOB_CANCELED_MESSAGE,
        ));
    }
    Ok(())
}

fn is_canceled_error(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::Interrupted && error.to_string() == JOB_CANCELED_MESSAGE
}

fn contextual_io_error(path: &Path, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("cannot scan {}: {error}", path.to_string_lossy()),
    )
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-tree-{label}-{stamp}"));
        fs::create_dir_all(&root).expect("tree fixture should be creatable");
        root
    }

    fn loaded_tree(root: &Path, max_depth: usize, max_entries: usize) -> TreeState {
        let job_id = JobId(1);
        let result = build_tree_entries(root, max_depth, max_entries, None)
            .expect("tree fixture should scan");
        let mut tree = TreeState::loading(job_id, root.to_path_buf());
        tree.apply_build_result(job_id, root, result)
            .expect("initial scan should apply");
        tree
    }

    fn visible_paths(tree: &TreeState) -> Vec<PathBuf> {
        tree.visible_entries()
            .map(|entry| entry.path.clone())
            .collect()
    }

    #[test]
    fn tree_scan_is_sorted_preorder_with_contiguous_descendants() {
        let root = temp_root("preorder");
        fs::create_dir_all(root.join("beta").join("child-b"))
            .expect("beta fixture should be creatable");
        fs::create_dir_all(root.join("alpha").join("child-a"))
            .expect("alpha fixture should be creatable");

        let result = build_tree_entries(&root, 4, 64, None).expect("tree should scan");
        let relative: Vec<(PathBuf, usize)> = result
            .entries
            .iter()
            .map(|entry| {
                (
                    entry
                        .path
                        .strip_prefix(&root)
                        .expect("entry should be under root")
                        .to_path_buf(),
                    entry.depth,
                )
            })
            .collect();
        assert_eq!(
            relative,
            vec![
                (PathBuf::new(), 0),
                (PathBuf::from("alpha"), 1),
                (PathBuf::from("alpha/child-a"), 2),
                (PathBuf::from("beta"), 1),
                (PathBuf::from("beta/child-b"), 2),
            ]
        );
        assert_eq!(result.summary, TreeScanSummary::default());

        fs::remove_dir_all(root).expect("tree fixture should be removable");
    }

    #[test]
    fn tree_scan_reports_depth_and_entry_truncation() {
        let root = temp_root("limits");
        fs::create_dir_all(root.join("a").join("deep")).expect("deep fixture should be creatable");
        fs::create_dir_all(root.join("b")).expect("sibling fixture should be creatable");

        let depth_limited =
            build_tree_entries(&root, 1, 64, None).expect("depth-limited tree should scan");
        assert!(depth_limited.summary.depth_limit_reached);
        assert!(!depth_limited.summary.entry_limit_reached);

        let entry_limited =
            build_tree_entries(&root, 4, 2, None).expect("entry-limited tree should scan");
        assert!(entry_limited.summary.entry_limit_reached);
        assert_eq!(entry_limited.entries.len(), 2);
        assert_eq!(entry_limited.entries[1].path, root.join("a"));

        fs::remove_dir_all(root).expect("tree fixture should be removable");
    }

    #[test]
    fn tree_scan_checks_cancellation_during_traversal() {
        let root = temp_root("cancel");
        for index in 0..8 {
            fs::create_dir_all(root.join(format!("dir-{index}")))
                .expect("cancellation fixture should be creatable");
        }

        let mut checks = 0_usize;
        let error = build_tree_entries_with_cancel_check(&root, 4, 64, || {
            checks = checks.saturating_add(1);
            checks > 4
        })
        .expect_err("tree scan should observe cancellation");
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(error.to_string(), JOB_CANCELED_MESSAGE);
        assert!(
            checks > 4,
            "scan should perform repeated cancellation checks"
        );

        fs::remove_dir_all(root).expect("tree fixture should be removable");
    }

    #[test]
    fn tree_scan_rejects_invalid_roots_and_limits() {
        let root = temp_root("invalid");
        let file = root.join("file.txt");
        fs::write(&file, "file").expect("file fixture should be writable");

        let zero_limit =
            build_tree_entries(&root, 4, 0, None).expect_err("zero entry limit should be rejected");
        assert_eq!(zero_limit.kind(), io::ErrorKind::InvalidInput);

        let file_root =
            build_tree_entries(&file, 4, 64, None).expect_err("file root should be rejected");
        assert_eq!(file_root.kind(), io::ErrorKind::InvalidInput);

        fs::remove_dir_all(root).expect("tree fixture should be removable");
    }

    #[test]
    fn dynamic_navigation_uses_parent_child_and_sibling_links() {
        let root = temp_root("dynamic-navigation");
        let alpha = root.join("alpha");
        let alpha_one = alpha.join("one");
        let alpha_two = alpha.join("two");
        let deep = alpha_one.join("deep");
        let beta = root.join("beta");
        fs::create_dir_all(&deep).expect("deep fixture should be creatable");
        fs::create_dir_all(&alpha_two).expect("sibling fixture should be creatable");
        fs::create_dir_all(&beta).expect("root sibling fixture should be creatable");
        let mut tree = loaded_tree(&root, 8, 64);

        assert_eq!(
            visible_paths(&tree),
            vec![root.clone(), alpha.clone(), beta.clone()]
        );
        tree.move_first_child();
        assert_eq!(tree.selected_entry().map(|entry| &entry.path), Some(&alpha));
        assert_eq!(
            visible_paths(&tree),
            vec![
                root.clone(),
                alpha.clone(),
                alpha_one.clone(),
                alpha_two.clone(),
                beta.clone(),
            ]
        );
        tree.move_first_child();
        assert_eq!(
            tree.selected_entry().map(|entry| &entry.path),
            Some(&alpha_one)
        );
        assert_eq!(
            visible_paths(&tree),
            vec![
                root.clone(),
                alpha.clone(),
                alpha_one.clone(),
                deep,
                alpha_two.clone(),
            ]
        );
        tree.move_cursor(1);
        assert_eq!(
            tree.selected_entry().map(|entry| &entry.path),
            Some(&alpha_two)
        );
        tree.move_parent();
        assert_eq!(tree.selected_entry().map(|entry| &entry.path), Some(&alpha));

        tree.toggle_navigation_mode();
        assert_eq!(tree.navigation_mode, TreeNavigationMode::Static);
        assert_eq!(tree.visible_entry_count(), tree.entries.len());

        fs::remove_dir_all(root).expect("tree fixture should be removable");
    }

    #[test]
    fn incremental_search_matches_prefixes_and_cycles() {
        let root = temp_root("search");
        let alpha = root.join("alpha");
        let beta = root.join("beta");
        let bravo = root.join("bravo");
        fs::create_dir_all(&alpha).expect("alpha fixture should be creatable");
        fs::create_dir_all(&beta).expect("beta fixture should be creatable");
        fs::create_dir_all(&bravo).expect("bravo fixture should be creatable");
        let mut tree = loaded_tree(&root, 4, 64);

        assert!(tree.append_search_char('b'));
        assert_eq!(tree.selected_entry().map(|entry| &entry.path), Some(&beta));
        assert!(tree.append_search_char('r'));
        assert_eq!(tree.selected_entry().map(|entry| &entry.path), Some(&bravo));
        assert!(tree.remove_search_char());
        assert_eq!(tree.search_query, "b");
        assert!(tree.search_next());
        assert_eq!(tree.selected_entry().map(|entry| &entry.path), Some(&beta));

        fs::remove_dir_all(root).expect("tree fixture should be removable");
    }

    #[test]
    fn failed_incremental_search_next_keeps_the_cursor_fixed() {
        let root = temp_root("failed-search-next");
        fs::create_dir_all(root.join("alpha")).expect("alpha fixture should be creatable");
        fs::create_dir_all(root.join("beta")).expect("beta fixture should be creatable");
        let mut tree = loaded_tree(&root, 4, 64);

        assert!(!tree.append_search_char('z'));
        let selected_before = tree
            .selected_entry()
            .map(|entry| entry.path.clone())
            .expect("tree should retain a selected entry");
        assert!(!tree.search_next());
        assert_eq!(
            tree.selected_entry().map(|entry| &entry.path),
            Some(&selected_before),
            "an unsuccessful search must not select an unrelated directory"
        );

        fs::remove_dir_all(root).expect("tree fixture should be removable");
    }

    #[test]
    fn forget_removes_a_contiguous_cached_subtree() {
        let root = temp_root("forget");
        let alpha = root.join("alpha");
        let alpha_child = alpha.join("child");
        let beta = root.join("beta");
        fs::create_dir_all(&alpha_child).expect("alpha fixture should be creatable");
        fs::create_dir_all(&beta).expect("beta fixture should be creatable");
        let mut tree = loaded_tree(&root, 4, 64);
        tree.move_first_child();

        assert_eq!(tree.forget_selected(), Some(alpha.clone()));
        assert_eq!(
            tree.entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>(),
            vec![root.clone(), beta]
        );
        assert_eq!(tree.selected_entry().map(|entry| &entry.path), Some(&root));

        fs::remove_dir_all(root).expect("tree fixture should be removable");
    }

    #[test]
    fn selected_rescan_replaces_only_that_preorder_subtree() {
        let root = temp_root("subtree-rescan");
        let alpha = root.join("alpha");
        let old = alpha.join("old");
        let beta = root.join("beta");
        fs::create_dir_all(&old).expect("old fixture should be creatable");
        fs::create_dir_all(&beta).expect("beta fixture should be creatable");
        let mut tree = loaded_tree(&root, 6, 64);
        tree.move_first_child();

        fs::remove_dir_all(&old).expect("old fixture should be removable");
        let new = alpha.join("new");
        fs::create_dir_all(&new).expect("new fixture should be creatable");
        let plan = tree
            .plan_selected_rescan(6, 64)
            .expect("selected rescan should be plannable");
        let result = build_tree_entries(
            &plan.scan_root,
            plan.scan_max_depth,
            plan.scan_max_entries,
            None,
        )
        .expect("selected subtree should scan");
        let job_id = JobId(2);
        tree.begin_rescan(job_id, plan);
        tree.apply_build_result(job_id, &alpha, result)
            .expect("selected rescan should merge");

        assert_eq!(
            tree.entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>(),
            vec![root.clone(), alpha.clone(), new, beta]
        );
        assert_eq!(tree.selected_entry().map(|entry| &entry.path), Some(&alpha));

        fs::remove_dir_all(root).expect("tree fixture should be removable");
    }

    #[test]
    fn partial_rescan_preserves_a_global_entry_limit_marker() {
        let root = temp_root("subtree-entry-limit");
        let alpha = root.join("alpha");
        for child in ["one", "two", "three"] {
            fs::create_dir_all(alpha.join(child)).expect("alpha child should be creatable");
        }
        fs::create_dir_all(root.join("beta")).expect("beta fixture should be creatable");
        let mut tree = loaded_tree(&root, 6, 4);
        assert!(matches!(
            &tree.load_state,
            TreeLoadState::Ready(summary) if summary.entry_limit_reached
        ));
        assert!(tree.select_path(&alpha));

        for child in ["one", "two", "three"] {
            fs::remove_dir_all(alpha.join(child)).expect("alpha child should be removable");
        }
        let plan = tree
            .plan_selected_rescan(6, 4)
            .expect("selected rescan should be plannable");
        let result = build_tree_entries(
            &plan.scan_root,
            plan.scan_max_depth,
            plan.scan_max_entries,
            None,
        )
        .expect("selected subtree should scan");
        let job_id = JobId(2);
        tree.begin_rescan(job_id, plan);
        tree.apply_build_result(job_id, &alpha, result)
            .expect("selected rescan should merge");

        assert_eq!(
            tree.entries.len(),
            2,
            "unknown beta remains outside the cache"
        );
        assert!(matches!(
            &tree.load_state,
            TreeLoadState::Ready(summary) if summary.entry_limit_reached
        ));

        fs::remove_dir_all(root).expect("tree fixture should be removable");
    }

    #[test]
    fn mutation_tracker_coalesces_successful_impacts() {
        let mut tracker = TreeMutationTracker::default();
        tracker.track(JobId(1), vec![PathBuf::from("/a")]);
        tracker.track(JobId(2), vec![PathBuf::from("/b")]);

        assert_eq!(tracker.finish(JobId(1), true), None);
        assert_eq!(
            tracker.finish(JobId(2), true),
            Some(vec![PathBuf::from("/a"), PathBuf::from("/b")])
        );
    }
}
