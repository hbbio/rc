use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering as AtomicOrdering;

use crate::*;

impl AppState {
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

    fn pause_active_find_results(&mut self) -> bool {
        let Some(Route::FindResults(results)) = self.routes.pop() else {
            return false;
        };
        self.set_find_job_paused(results.job_id, true);
        self.paused_find_results = Some(results);
        true
    }

    fn resume_paused_find_results(&mut self) -> bool {
        if matches!(self.top_route(), Route::FindResults(_)) {
            return true;
        }
        let Some(results) = self.paused_find_results.take() else {
            return false;
        };
        self.set_find_job_paused(results.job_id, false);
        self.routes.push(Route::FindResults(results));
        true
    }

    pub(crate) fn open_find_dialog(&mut self) {
        if self.resume_paused_find_results() {
            self.set_status("Resumed find results");
            return;
        }

        let base_dir = self.active_panel().cwd.clone();
        self.pending_dialog_action = Some(PendingDialogAction::FindQuery { base_dir });
        self.routes.push(Route::Dialog(DialogState::input(
            "Find file",
            "Name contains:",
            "",
        )));
        self.set_status("Find file");
    }

    pub(crate) fn close_find_results(&mut self) {
        if matches!(self.top_route(), Route::FindResults(_)) {
            self.routes.pop();
            self.set_status("Closed find results");
        }
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
        self.pending_panel_focus = None;

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
            self.pending_panel_focus = Some((self.active_panel, selected.path.clone()));
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
                results.query.clone(),
                results.base_dir.clone(),
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
        self.pending_panelize_revert = Some((active_panel, previous_source));
        self.pause_active_find_results();
        self.queue_panel_refresh(active_panel);
        self.set_status(format!("Panelizing {result_count} find result(s)..."));
    }

    pub(crate) fn open_tree_screen(&mut self) {
        if matches!(self.top_route(), Route::Tree(_)) {
            return;
        }
        let root = self.active_panel().cwd.clone();
        self.routes
            .push(Route::Tree(TreeState::loading(root.clone())));
        self.queue_worker_job_request(JobRequest::BuildTree {
            root,
            max_depth: self.settings.advanced.tree_max_depth,
            max_entries: self.settings.advanced.tree_max_entries,
        });
        self.set_status("Loading directory tree...");
    }

    pub(crate) fn close_tree_screen(&mut self) {
        if matches!(self.top_route(), Route::Tree(_)) {
            self.routes.pop();
            self.set_status("Closed directory tree");
        }
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

    pub(crate) fn open_selected_tree_entry(&mut self) -> io::Result<()> {
        let selected = match self.top_route() {
            Route::Tree(tree) => tree.selected_entry().cloned(),
            _ => None,
        };
        let Some(selected) = selected else {
            self.set_status("No tree entry selected");
            return Ok(());
        };

        if self.set_active_panel_directory(selected.path.clone())? {
            self.routes.pop();
            self.set_status(format!(
                "Opened directory {}",
                selected.path.to_string_lossy()
            ));
        } else {
            self.set_status("Selected tree entry is not an accessible directory");
        }
        Ok(())
    }

    pub(crate) fn open_hotlist_screen(&mut self) {
        if !matches!(self.top_route(), Route::Hotlist) {
            self.routes.push(Route::Hotlist);
        }
        self.clamp_hotlist_cursor();
        self.set_status("Opened directory hotlist");
    }

    pub(crate) fn close_hotlist_screen(&mut self) {
        if matches!(self.top_route(), Route::Hotlist) {
            self.routes.pop();
            self.set_status("Closed directory hotlist");
        }
    }

    fn clamp_hotlist_cursor(&mut self) {
        if self.hotlist.is_empty() {
            self.hotlist_cursor = 0;
        } else if self.hotlist_cursor >= self.hotlist.len() {
            self.hotlist_cursor = self.hotlist.len() - 1;
        }
    }

    pub(crate) fn move_hotlist_cursor(&mut self, delta: isize) {
        if self.hotlist.is_empty() {
            self.hotlist_cursor = 0;
            return;
        }
        let last = self.hotlist.len() - 1;
        let next = if delta.is_negative() {
            self.hotlist_cursor.saturating_sub(delta.unsigned_abs())
        } else {
            self.hotlist_cursor.saturating_add(delta as usize).min(last)
        };
        self.hotlist_cursor = next;
    }

    pub(crate) fn move_hotlist_page(&mut self, pages: isize) {
        self.move_hotlist_cursor(pages.saturating_mul(self.settings.advanced.page_step as isize));
    }

    pub(crate) fn move_hotlist_home(&mut self) {
        self.hotlist_cursor = 0;
    }

    pub(crate) fn move_hotlist_end(&mut self) {
        if self.hotlist.is_empty() {
            self.hotlist_cursor = 0;
        } else {
            self.hotlist_cursor = self.hotlist.len() - 1;
        }
    }

    pub(crate) fn add_current_directory_to_hotlist(&mut self) {
        let cwd = self.active_panel().cwd.clone();
        if self.hotlist.iter().any(|entry| entry == &cwd) {
            self.hotlist_cursor = self
                .hotlist
                .iter()
                .position(|entry| entry == &cwd)
                .unwrap_or(self.hotlist_cursor);
            self.set_status("Directory already exists in hotlist");
            return;
        }
        self.hotlist.push(cwd.clone());
        self.hotlist_cursor = self.hotlist.len() - 1;
        self.settings.configuration.hotlist = self.hotlist.clone();
        self.settings.mark_dirty();
        self.set_status(format!("Added {} to hotlist", cwd.to_string_lossy()));
    }

    pub(crate) fn remove_selected_hotlist_entry(&mut self) {
        if self.hotlist.is_empty() {
            self.set_status("Hotlist is empty");
            return;
        }
        let removed = self.hotlist.remove(self.hotlist_cursor);
        self.clamp_hotlist_cursor();
        self.settings.configuration.hotlist = self.hotlist.clone();
        self.settings.mark_dirty();
        self.set_status(format!(
            "Removed {} from hotlist",
            removed.to_string_lossy()
        ));
    }

    pub(crate) fn open_selected_hotlist_entry(&mut self) -> io::Result<()> {
        let Some(path) = self.hotlist.get(self.hotlist_cursor).cloned() else {
            self.set_status("No hotlist entry selected");
            return Ok(());
        };

        if self.set_active_panel_directory(path.clone())? {
            self.routes.pop();
            self.set_status(format!("Opened directory {}", path.to_string_lossy()));
        } else {
            self.set_status("Selected hotlist path is not an accessible directory");
        }
        Ok(())
    }

    fn set_active_panel_directory(&mut self, destination: PathBuf) -> io::Result<bool> {
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
