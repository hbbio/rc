use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

use crate::*;

impl AppState {
    pub(super) fn apply_navigation_command(
        &mut self,
        command: AppCommand,
    ) -> io::Result<CommandOutcome> {
        match command {
            AppCommand::Navigate(NavigationTarget::FileManager, motion) => {
                self.apply_file_manager_navigation(motion);
            }
            AppCommand::ToggleTag => {
                let selected = self.active_panel().selected_entry();
                if selected.is_none() {
                    self.set_status("No entry selected");
                } else if selected.is_some_and(FileEntry::is_parent) {
                    self.set_status("Parent entry cannot be tagged");
                } else {
                    let added = self.active_panel_mut().toggle_tag_on_cursor();
                    self.active_panel_mut().move_cursor(1);
                    let count = self.active_panel().tagged_count();
                    self.set_status(if added {
                        format!("Tagged entry ({count} total)")
                    } else {
                        format!("Untagged entry ({count} total)")
                    });
                }
            }
            AppCommand::InvertTags => {
                self.active_panel_mut().invert_tags();
                let count = self.active_panel().tagged_count();
                self.set_status(format!("Inverted tags ({count} selected)"));
            }
            AppCommand::SortNext => {
                self.active_panel_mut().cycle_sort_field();
                self.refresh_active_panel();
                let label = self.active_panel().sort_label();
                self.set_status(format!("Sort: {label}"));
            }
            AppCommand::SortReverse => {
                self.active_panel_mut().toggle_sort_direction();
                self.refresh_active_panel();
                let label = self.active_panel().sort_label();
                self.set_status(format!("Sort: {label}"));
            }
            AppCommand::Copy => self.start_copy_dialog(),
            AppCommand::Move => self.start_move_dialog(),
            AppCommand::Delete => {
                if self.settings.confirmation.confirm_delete {
                    self.start_delete_confirmation();
                } else {
                    let targets = self.selected_operation_paths();
                    if targets.is_empty() {
                        self.set_status("Delete requires a selected or tagged entry");
                    } else {
                        self.queue_delete_job(targets);
                    }
                }
            }
            AppCommand::CancelJob => {
                if matches!(self.top_route(), Route::FindResults(_)) {
                    self.cancel_active_find_search();
                } else {
                    self.cancel_latest_job();
                }
            }
            AppCommand::OpenEntry => {
                if self.open_selected_directory() {
                    self.queue_panel_refresh(self.active_panel);
                    self.set_status("Loading selected directory...");
                } else if self.open_selected_file_in_viewer() {
                    self.set_status("Opening viewer...");
                } else {
                    self.set_status("No entry selected");
                }
            }
            AppCommand::EditEntry => match self.open_selected_file_in_editor() {
                EditSelectionResult::OpenedExternal => {
                    self.set_status("Opening external editor...")
                }
                EditSelectionResult::NoEditorResolved => self
                    .set_status("No external editor found; set editor_command, EDITOR, or VISUAL"),
                EditSelectionResult::NoEntrySelected => self.set_status("No entry selected"),
                EditSelectionResult::SelectedEntryIsDirectory => {
                    self.set_status("Directory cannot be edited");
                }
            },
            AppCommand::CdUp => {
                if self.exit_panelize_mode() {
                    self.queue_panel_refresh(self.active_panel);
                    self.set_status("Leaving panelize mode...");
                } else if self.go_parent_directory() {
                    self.queue_panel_refresh(self.active_panel);
                    self.set_status("Loading parent directory...");
                } else {
                    self.set_status("Already at filesystem root");
                }
            }
            AppCommand::Reread => {
                self.refresh_active_panel();
                self.set_status("Refreshing active panel...");
            }
            AppCommand::Navigate(NavigationTarget::FindResults, motion) => {
                self.apply_find_results_navigation(motion);
            }
            AppCommand::FindResultsOpenEntry => {
                self.open_selected_find_result()?;
            }
            AppCommand::FindResultsPanelize => self.panelize_find_results(),
            AppCommand::FindResultsAgain => self.open_find_again_dialog(),
            AppCommand::FindResultsTogglePause => self.toggle_active_find_pause(),
            AppCommand::Navigate(NavigationTarget::Tree, motion) => {
                self.apply_tree_navigation(motion);
            }
            AppCommand::TreeOpenEntry => {
                self.open_selected_tree_entry()?;
            }
            AppCommand::TreeRescan => self.rescan_selected_tree(),
            AppCommand::TreeForget => self.forget_selected_tree_entry(),
            AppCommand::TreeToggleNavigation => self.toggle_tree_navigation_mode(),
            AppCommand::TreeCopy => self.start_tree_transfer(TransferKind::Copy),
            AppCommand::TreeMove => self.start_tree_transfer(TransferKind::Move),
            AppCommand::TreeMkdir => self.start_tree_mkdir(),
            AppCommand::TreeDelete => self.start_tree_delete(),
            AppCommand::TreeSearchNext => self.search_next_tree_entry(),
            AppCommand::TreeSearchBackspace => self.remove_tree_search_char(),
            AppCommand::TreeSearchAppend(ch) => self.append_tree_search_char(ch),
            AppCommand::Navigate(NavigationTarget::Hotlist, motion) => {
                self.apply_hotlist_navigation(motion);
            }
            AppCommand::HotlistOpenEntry => {
                self.open_selected_hotlist_entry()?;
            }
            AppCommand::HotlistAddCurrentDirectory => self.start_hotlist_add_dialog(),
            AppCommand::HotlistEditSelected => self.start_hotlist_edit_dialog(),
            AppCommand::HotlistRemoveSelected => self.remove_selected_hotlist_entry(),
            _ => {
                unreachable!("non-navigation command dispatched to navigation handler: {command:?}")
            }
        }

        Ok(CommandOutcome::Continue)
    }

    fn apply_file_manager_navigation(&mut self, motion: NavigationMotion) {
        match motion {
            NavigationMotion::Up => self.move_cursor(-1),
            NavigationMotion::Down => self.move_cursor(1),
            NavigationMotion::PageUp => {
                let page_step = self.settings.advanced.page_step;
                self.active_panel_mut().move_cursor_page(-1, page_step);
            }
            NavigationMotion::PageDown => {
                let page_step = self.settings.advanced.page_step;
                self.active_panel_mut().move_cursor_page(1, page_step);
            }
            NavigationMotion::Home => self.active_panel_mut().move_cursor_home(),
            NavigationMotion::End => self.active_panel_mut().move_cursor_end(),
            _ => {}
        }
    }

    fn apply_find_results_navigation(&mut self, motion: NavigationMotion) {
        match motion {
            NavigationMotion::Up => self.move_find_results_cursor(-1),
            NavigationMotion::Down => self.move_find_results_cursor(1),
            NavigationMotion::PageUp => self.move_find_results_page(-1),
            NavigationMotion::PageDown => self.move_find_results_page(1),
            NavigationMotion::Home => self.move_find_results_home(),
            NavigationMotion::End => self.move_find_results_end(),
            _ => {}
        }
    }

    fn apply_tree_navigation(&mut self, motion: NavigationMotion) {
        match motion {
            NavigationMotion::Up => self.move_tree_cursor(-1),
            NavigationMotion::Down => self.move_tree_cursor(1),
            NavigationMotion::Left => self.move_tree_parent(),
            NavigationMotion::Right => self.move_tree_first_child(),
            NavigationMotion::PageUp => self.move_tree_page(-1),
            NavigationMotion::PageDown => self.move_tree_page(1),
            NavigationMotion::Home => self.move_tree_home(),
            NavigationMotion::End => self.move_tree_end(),
            _ => {}
        }
    }

    fn apply_hotlist_navigation(&mut self, motion: NavigationMotion) {
        match motion {
            NavigationMotion::Up => self.move_hotlist_cursor(-1),
            NavigationMotion::Down => self.move_hotlist_cursor(1),
            NavigationMotion::PageUp => self.move_hotlist_page(-1),
            NavigationMotion::PageDown => self.move_hotlist_page(1),
            NavigationMotion::Home => self.move_hotlist_home(),
            NavigationMotion::End => self.move_hotlist_end(),
            _ => {}
        }
    }

    pub(crate) fn find_results_by_job_id(&self, job_id: JobId) -> Option<&FindResultsState> {
        self.routes
            .iter()
            .rev()
            .find_map(|route| match route {
                Route::FindResults(results) if results.job_id == job_id => Some(results),
                _ => None,
            })
            .or_else(|| {
                self.paused_find_results
                    .as_ref()
                    .filter(|results| results.job_id == job_id)
            })
    }

    pub(crate) fn find_results_by_job_id_mut(
        &mut self,
        job_id: JobId,
    ) -> Option<&mut FindResultsState> {
        if let Some(results) = self.routes.iter_mut().rev().find_map(|route| match route {
            Route::FindResults(results) if results.job_id == job_id => Some(results),
            _ => None,
        }) {
            return Some(results);
        }

        self.paused_find_results
            .as_mut()
            .filter(|results| results.job_id == job_id)
    }

    fn set_find_job_paused(&self, job_id: JobId, paused: bool) {
        if let Some(flag) = self.find_pause_flags.get(&job_id) {
            flag.store(paused, AtomicOrdering::Relaxed);
        }
    }

    fn pop_find_results(&mut self) -> Option<FindResultsState> {
        if !matches!(self.top_route(), Route::FindResults(_)) {
            return None;
        }
        match self.routes.pop() {
            Some(Route::FindResults(results)) => Some(results),
            _ => None,
        }
    }

    fn pause_active_find_results(&mut self) -> bool {
        let Some(mut results) = self.pop_find_results() else {
            return false;
        };
        if matches!(results.status, FindResultsStatus::Running) {
            self.set_find_job_paused(results.job_id, true);
            results.status = FindResultsStatus::Paused;
        }
        self.paused_find_results = Some(results);
        true
    }

    fn resume_paused_find_results(&mut self) -> bool {
        if matches!(self.top_route(), Route::FindResults(_)) {
            return true;
        }
        let Some(mut results) = self.paused_find_results.take() else {
            return false;
        };
        if matches!(results.status, FindResultsStatus::Paused) {
            self.set_find_job_paused(results.job_id, false);
            results.status = FindResultsStatus::Running;
        }
        self.routes.push(Route::FindResults(results));
        true
    }

    pub(crate) fn open_find_dialog(&mut self) {
        if self.resume_paused_find_results() {
            self.set_status("Resumed find results");
            return;
        }

        let spec = FindSpec::new(self.active_panel().cwd.clone());
        self.push_find_dialog(spec);
        self.set_status("Find file");
    }

    fn push_find_dialog(&mut self, spec: FindSpec) {
        self.push_dialog(DialogState::find(&spec), PendingDialogAction::FindSearch);
    }

    pub(crate) fn start_find_search(&mut self, mut spec: FindSpec) {
        if spec.start_dir.as_os_str().is_empty() {
            self.push_find_dialog(spec);
            self.set_status("Find start directory cannot be empty");
            return;
        }
        if spec.start_dir.is_relative() {
            spec.start_dir = self.active_panel().cwd.join(&spec.start_dir);
        }
        match fs::metadata(&spec.start_dir) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                self.push_find_dialog(spec);
                self.set_status("Find start path is not a directory");
                return;
            }
            Err(error) => {
                self.push_find_dialog(spec);
                self.set_status(format!("Find start directory is inaccessible: {error}"));
                return;
            }
        }
        if let Err(error) = spec.validate() {
            self.push_find_dialog(spec);
            self.set_status(error.to_string());
            return;
        }

        self.discard_paused_find_results();
        let request = JobRequest::Find {
            spec: spec.clone(),
            max_results: self.settings.advanced.max_find_results,
        };
        let mut worker_job = self.jobs.enqueue(request);
        let job_id = worker_job.id;
        let pause_flag = Arc::new(AtomicBool::new(false));
        self.find_pause_flags.insert(job_id, pause_flag.clone());
        worker_job.set_find_pause_flag(pause_flag);
        self.routes
            .push(Route::FindResults(FindResultsState::loading(job_id, spec)));
        self.queue_worker_job(worker_job);
    }

    fn discard_paused_find_results(&mut self) {
        let Some(results) = self.paused_find_results.take() else {
            return;
        };
        if results.is_active() {
            let _ = self.request_cancel_for_job(results.job_id);
        }
    }

    fn open_find_again_dialog(&mut self) {
        let Some(results) = self.pop_find_results() else {
            return;
        };
        let spec = results.spec.clone();
        if results.is_active() {
            let _ = self.request_cancel_for_job(results.job_id);
        }
        self.discard_paused_find_results();
        self.push_find_dialog(spec);
        self.set_status("Find again");
    }

    fn toggle_active_find_pause(&mut self) {
        let state = match self.top_route() {
            Route::FindResults(results) => Some((results.job_id, results.status.clone())),
            _ => None,
        };
        let Some((job_id, status)) = state else {
            return;
        };
        let next_status = match status {
            FindResultsStatus::Running => {
                self.set_find_job_paused(job_id, true);
                FindResultsStatus::Paused
            }
            FindResultsStatus::Paused => {
                self.set_find_job_paused(job_id, false);
                FindResultsStatus::Running
            }
            FindResultsStatus::Canceling => {
                self.set_status("Find cancellation is already pending");
                return;
            }
            _ => {
                self.set_status("Find is no longer running");
                return;
            }
        };
        let label = next_status.label();
        if let Some(Route::FindResults(results)) = self.routes.last_mut() {
            results.status = next_status;
        }
        self.set_status(format!("Find {label}"));
    }

    fn cancel_active_find_search(&mut self) {
        let active = match self.top_route() {
            Route::FindResults(results) if results.is_active() => Some(results.job_id),
            _ => None,
        };
        let Some(job_id) = active else {
            self.set_status("Find is no longer running");
            return;
        };
        if self.request_cancel_for_job(job_id) {
            if let Some(Route::FindResults(results)) = self.routes.last_mut() {
                results.status = FindResultsStatus::Canceling;
            }
            self.set_status(format!("Canceling find job #{job_id}"));
        } else {
            self.set_status(format!("Find job #{job_id} cannot be canceled"));
        }
    }

    pub(crate) fn open_find_tree_picker(&mut self) {
        let form = match self.top_route() {
            Route::Dialog(dialog) => match &dialog.kind {
                DialogKind::Find(form) => Some(form.clone()),
                _ => None,
            },
            _ => None,
        };
        let Some(form) = form else {
            self.set_status("Find tree picker is unavailable here");
            return;
        };
        self.routes.pop();

        let candidate = PathBuf::from(form.start_directory.trim());
        let candidate = if candidate.is_relative() {
            self.active_panel().cwd.join(candidate)
        } else {
            candidate
        };
        let root = fs::metadata(&candidate)
            .ok()
            .filter(fs::Metadata::is_dir)
            .map(|_| candidate)
            .unwrap_or_else(|| self.active_panel().cwd.clone());
        self.pending_find_tree_picker = Some(form);
        self.open_tree_at(root);
        self.set_status("Choose the find start directory");
    }

    fn restore_find_tree_picker(&mut self, selected: Option<PathBuf>) -> bool {
        let Some(mut form) = self.pending_find_tree_picker.take() else {
            return false;
        };
        if let Some(selected) = selected {
            form.start_directory = selected.to_string_lossy().into_owned();
        }
        self.push_dialog(
            DialogState {
                title: String::from("Find file"),
                kind: DialogKind::Find(form),
            },
            PendingDialogAction::FindSearch,
        );
        true
    }

    pub(crate) fn close_find_results(&mut self) {
        let active_job = match self.top_route() {
            Route::FindResults(results) if results.is_active() => Some(results.job_id),
            Route::FindResults(_) => None,
            _ => return,
        };
        self.routes.pop();
        if let Some(job_id) = active_job {
            let _ = self.request_cancel_for_job(job_id);
        }
        self.set_status("Closed find results");
    }

    pub(crate) fn move_find_results_cursor(&mut self, delta: isize) {
        let Some(Route::FindResults(results)) = self.routes.last_mut() else {
            return;
        };
        results.move_cursor(delta);
    }

    pub(crate) fn move_find_results_page(&mut self, pages: isize) {
        let Some(Route::FindResults(results)) = self.routes.last_mut() else {
            return;
        };
        results.move_page(pages, self.settings.advanced.page_step);
    }

    pub(crate) fn move_find_results_home(&mut self) {
        let Some(Route::FindResults(results)) = self.routes.last_mut() else {
            return;
        };
        results.move_home();
    }

    pub(crate) fn move_find_results_end(&mut self) {
        let Some(Route::FindResults(results)) = self.routes.last_mut() else {
            return;
        };
        results.move_end();
    }

    pub(crate) fn open_selected_find_result(&mut self) -> io::Result<()> {
        let selected = match self.top_route() {
            Route::FindResults(results) => results.selected_entry().cloned(),
            _ => None,
        };
        let Some(selected) = selected else {
            self.set_status("No find result selected");
            return Ok(());
        };
        self.clear_pending_panel_focus_target();

        if selected.is_dir {
            if self.set_active_panel_directory(selected.path.clone())? {
                self.pause_active_find_results();
                self.set_status(format!(
                    "Opened directory {} (Alt-F back to find)",
                    selected.path.to_string_lossy()
                ));
            } else {
                self.set_status("Selected result is not an accessible directory");
            }
            return Ok(());
        }

        let Some(parent_dir) = selected.path.parent().map(Path::to_path_buf) else {
            self.set_status("Selected result has no parent directory");
            return Ok(());
        };
        if self.set_active_panel_directory(parent_dir.clone())? {
            self.set_pending_panel_focus_target(self.active_panel, selected.path.clone());
            self.pause_active_find_results();
            self.set_status(format!(
                "Locating {} in {} (Alt-F back to find)",
                selected
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| selected.path.to_string_lossy().into_owned()),
                parent_dir.to_string_lossy()
            ));
        } else {
            self.set_status("Selected result parent directory is not accessible");
        }
        Ok(())
    }

    pub(crate) fn panelize_find_results(&mut self) {
        let Some((query, base_dir, paths)) = (match self.top_route() {
            Route::FindResults(results) => Some((
                results.spec.display_pattern().to_string(),
                results.spec.start_dir.clone(),
                results
                    .entries
                    .iter()
                    .map(|entry| entry.path.clone())
                    .collect::<Vec<_>>(),
            )),
            _ => None,
        }) else {
            self.set_status("Find results are not active");
            return;
        };

        if paths.is_empty() {
            self.set_status("No find results to panelize");
            return;
        }

        let result_count = paths.len();
        let active_panel = self.active_panel;
        let previous_source = self.active_panel().source.clone();
        {
            let panel = self.active_panel_mut();
            panel.source = PanelListingSource::FindResults {
                query,
                base_dir,
                paths,
            };
            panel.cursor = 0;
            panel.tagged.clear();
            panel.loading = true;
        }
        self.schedule_panelize_revert_for_panel_refresh(active_panel, previous_source);
        self.pause_active_find_results();
        self.queue_panel_refresh(active_panel);
        self.set_status(format!("Panelizing {result_count} find result(s)..."));
    }

    pub(crate) fn open_tree_screen(&mut self) {
        if matches!(self.top_route(), Route::Tree(_)) {
            return;
        }
        let root = self.active_panel().cwd.clone();
        self.open_tree_at(root);
        self.set_status("Loading directory tree...");
    }

    fn open_tree_at(&mut self, root: PathBuf) {
        let job_id = self.queue_worker_job_request(JobRequest::BuildTree {
            root: root.clone(),
            max_depth: self.settings.advanced.tree_max_depth,
            max_entries: self.settings.advanced.tree_max_entries,
        });
        self.routes
            .push(Route::Tree(Box::new(TreeState::loading(job_id, root))));
    }

    pub(crate) fn close_tree_screen(&mut self) {
        let Some(Route::Tree(tree)) = self.routes.last() else {
            return;
        };
        let pending_job = tree.scan_job_id();
        self.routes.pop();
        if let Some(job_id) = pending_job {
            let _ = self.request_cancel_for_job(job_id);
        }
        if self.restore_find_tree_picker(None) {
            self.set_status("Find tree selection canceled");
        } else {
            self.set_status("Closed directory tree");
        }
    }

    pub(crate) fn tree_by_job_id_mut(&mut self, job_id: JobId) -> Option<&mut TreeState> {
        self.routes.iter_mut().rev().find_map(|route| match route {
            Route::Tree(tree) if tree.scan_job_id() == Some(job_id) => Some(tree.as_mut()),
            _ => None,
        })
    }

    pub(crate) fn move_tree_cursor(&mut self, delta: isize) {
        let Some(Route::Tree(tree)) = self.routes.last_mut() else {
            return;
        };
        tree.move_cursor(delta);
    }

    pub(crate) fn move_tree_page(&mut self, pages: isize) {
        let Some(Route::Tree(tree)) = self.routes.last_mut() else {
            return;
        };
        tree.move_page(pages, self.settings.advanced.page_step);
    }

    pub(crate) fn move_tree_parent(&mut self) {
        let Some(Route::Tree(tree)) = self.routes.last_mut() else {
            return;
        };
        tree.move_parent();
    }

    pub(crate) fn move_tree_first_child(&mut self) {
        let Some(Route::Tree(tree)) = self.routes.last_mut() else {
            return;
        };
        tree.move_first_child();
    }

    pub(crate) fn move_tree_home(&mut self) {
        let Some(Route::Tree(tree)) = self.routes.last_mut() else {
            return;
        };
        tree.move_home();
    }

    pub(crate) fn move_tree_end(&mut self) {
        let Some(Route::Tree(tree)) = self.routes.last_mut() else {
            return;
        };
        tree.move_end();
    }

    pub(crate) fn rescan_selected_tree(&mut self) {
        let plan = match self.top_route() {
            Route::Tree(tree) => tree.plan_selected_rescan(
                self.settings.advanced.tree_max_depth,
                self.settings.advanced.tree_max_entries,
            ),
            _ => None,
        };
        let Some(plan) = plan else {
            self.set_status("Selected directory cannot be rescanned");
            return;
        };
        let label = format!("Rescanning {}...", plan.scan_root.to_string_lossy());
        self.queue_tree_rescan(plan, label);
    }

    pub(crate) fn rescan_tree_for_impacts(&mut self, impacts: &[PathBuf]) {
        let plan = self.routes.iter().rev().find_map(|route| match route {
            Route::Tree(tree) => tree.plan_rescan_for_impacts(
                impacts,
                self.settings.advanced.tree_max_depth,
                self.settings.advanced.tree_max_entries,
            ),
            _ => None,
        });
        if let Some(plan) = plan {
            let label = format!(
                "Updating directory tree from {}...",
                plan.scan_root.to_string_lossy()
            );
            self.queue_tree_rescan(plan, label);
        }
    }

    fn queue_tree_rescan(&mut self, plan: TreeRescanPlan, status: String) {
        let pending_job = self.routes.iter().rev().find_map(|route| match route {
            Route::Tree(tree) => tree.scan_job_id(),
            _ => None,
        });
        if let Some(job_id) = pending_job {
            let _ = self.request_cancel_for_job(job_id);
        }
        let job_id = self.queue_worker_job_request(JobRequest::BuildTree {
            root: plan.scan_root.clone(),
            max_depth: plan.scan_max_depth,
            max_entries: plan.scan_max_entries,
        });
        if let Some(tree) = self.routes.iter_mut().rev().find_map(|route| match route {
            Route::Tree(tree) => Some(tree),
            _ => None,
        }) {
            tree.begin_rescan(job_id, plan);
            self.set_status(status);
        }
    }

    pub(crate) fn forget_selected_tree_entry(&mut self) {
        let (is_root, pending_job) = match self.top_route() {
            Route::Tree(tree) => (
                tree.selected_entry()
                    .is_some_and(|entry| entry.path == tree.root()),
                tree.scan_job_id(),
            ),
            _ => return,
        };
        if is_root {
            self.set_status("The tree root cannot be forgotten");
            return;
        }
        if let Some(job_id) = pending_job {
            let _ = self.request_cancel_for_job(job_id);
        }
        let removed = match self.routes.last_mut() {
            Some(Route::Tree(tree)) => {
                let _ = tree.cancel_scan_for_local_change();
                tree.forget_selected()
            }
            _ => None,
        };
        if let Some(path) = removed {
            self.set_status(format!("Forgot cached subtree {}", path.to_string_lossy()));
        } else {
            self.set_status("No tree directory selected");
        }
    }

    pub(crate) fn toggle_tree_navigation_mode(&mut self) {
        let mode = match self.routes.last_mut() {
            Some(Route::Tree(tree)) => tree.toggle_navigation_mode(),
            _ => return,
        };
        self.set_status(format!("Tree navigation: {}", mode.label()));
    }

    pub(crate) fn append_tree_search_char(&mut self, ch: char) {
        let outcome = match self.routes.last_mut() {
            Some(Route::Tree(tree)) => {
                let matched = tree.append_search_char(ch);
                Some((matched, tree.search_query().to_string()))
            }
            _ => None,
        };
        if let Some((matched, query)) = outcome {
            if matched {
                self.set_status(format!("Tree search: {query}"));
            } else {
                self.set_status(format!("No directory starts with '{query}'"));
            }
        }
    }

    pub(crate) fn remove_tree_search_char(&mut self) {
        let query = match self.routes.last_mut() {
            Some(Route::Tree(tree)) => {
                if !tree.remove_search_char() {
                    return;
                }
                tree.search_query().to_string()
            }
            _ => return,
        };
        self.set_status(if query.is_empty() {
            String::from("Tree search cleared")
        } else {
            format!("Tree search: {query}")
        });
    }

    pub(crate) fn search_next_tree_entry(&mut self) {
        let outcome = match self.routes.last_mut() {
            Some(Route::Tree(tree)) => {
                let matched = tree.search_next();
                Some((matched, tree.search_query().to_string()))
            }
            _ => None,
        };
        if let Some((matched, query)) = outcome {
            if matched && !query.is_empty() {
                self.set_status(format!("Tree search: {query}"));
            } else if !matched && !query.is_empty() {
                self.set_status(format!("No further directory starts with '{query}'"));
            }
        }
    }

    fn start_tree_transfer(&mut self, kind: TransferKind) {
        let selected = match self.top_route() {
            Route::Tree(tree) => tree
                .selected_entry()
                .map(|entry| (entry.path.clone(), entry.path == tree.root())),
            _ => None,
        };
        let Some((source, is_root)) = selected else {
            self.set_status("No tree directory selected");
            return;
        };
        if kind == TransferKind::Move && is_root {
            self.set_status("The tree root cannot be moved");
            return;
        }
        let source_base_dir = source
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| source.clone());
        let destination_dir = self.passive_panel().cwd.clone();
        self.start_transfer_dialog_for_paths(
            kind,
            vec![source],
            source_base_dir,
            destination_dir,
            OperationOrigin::Tree,
        );
    }

    fn start_tree_mkdir(&mut self) {
        let selected = match self.top_route() {
            Route::Tree(tree) => tree.selected_entry().map(|entry| entry.path.clone()),
            _ => None,
        };
        if let Some(base_dir) = selected {
            self.start_mkdir_dialog_at(base_dir, OperationOrigin::Tree);
        } else {
            self.set_status("No tree directory selected");
        }
    }

    fn start_tree_delete(&mut self) {
        let selected = match self.top_route() {
            Route::Tree(tree) => tree
                .selected_entry()
                .map(|entry| (entry.path.clone(), entry.path == tree.root())),
            _ => None,
        };
        let Some((target, is_root)) = selected else {
            self.set_status("No tree directory selected");
            return;
        };
        if is_root {
            self.set_status("The tree root cannot be deleted");
        } else if self.settings.confirmation.confirm_delete {
            self.start_delete_confirmation_for_targets(vec![target], OperationOrigin::Tree);
        } else {
            self.queue_delete_job_from(vec![target], OperationOrigin::Tree);
        }
    }

    pub(crate) fn open_selected_tree_entry(&mut self) -> io::Result<()> {
        let selected = match self.top_route() {
            Route::Tree(tree) => tree
                .selected_entry()
                .map(|entry| (entry.clone(), tree.scan_job_id())),
            _ => None,
        };
        let Some((selected, pending_job)) = selected else {
            self.set_status("No tree entry selected");
            return Ok(());
        };

        if self.pending_find_tree_picker.is_some() {
            self.routes.pop();
            if let Some(job_id) = pending_job {
                let _ = self.request_cancel_for_job(job_id);
            }
            self.restore_find_tree_picker(Some(selected.path.clone()));
            self.set_status(format!(
                "Find start directory: {}",
                selected.path.to_string_lossy()
            ));
            return Ok(());
        }

        if self.set_active_panel_directory(selected.path.clone())? {
            self.routes.pop();
            if let Some(job_id) = pending_job {
                let _ = self.request_cancel_for_job(job_id);
            }
            self.set_status(format!(
                "Opened directory {}",
                selected.path.to_string_lossy()
            ));
        } else {
            self.set_status("Selected tree entry is not an accessible directory");
        }
        Ok(())
    }

    pub(crate) fn set_active_panel_directory(&mut self, destination: PathBuf) -> io::Result<bool> {
        let metadata = match fs::metadata(&destination) {
            Ok(metadata) => metadata,
            Err(_) => return Ok(false),
        };
        if !metadata.is_dir() {
            return Ok(false);
        }

        let panel = self.active_panel_mut();
        panel.cwd = destination;
        panel.cursor = 0;
        panel.source = PanelListingSource::Directory;
        panel.tagged.clear();
        panel.entries.clear();
        panel.loading = true;
        self.queue_panel_refresh(self.active_panel);
        Ok(true)
    }
}
