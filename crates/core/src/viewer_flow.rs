use crate::viewer::ViewerSearchDirection;
use crate::*;

impl AppState {
    pub(super) fn apply_viewer_command(&mut self, command: AppCommand) -> CommandOutcome {
        match command {
            AppCommand::Navigate(NavigationTarget::Viewer, motion) => {
                self.apply_viewer_navigation(motion);
            }
            AppCommand::ViewerSearchForward => {
                self.open_viewer_search_dialog(ViewerSearchDirection::Forward);
            }
            AppCommand::ViewerSearchBackward => {
                self.open_viewer_search_dialog(ViewerSearchDirection::Backward);
            }
            AppCommand::ViewerSearchContinue => {
                self.continue_viewer_search(None);
            }
            AppCommand::ViewerSearchContinueBackward => {
                self.continue_viewer_search(Some(ViewerSearchDirection::Backward));
            }
            AppCommand::ViewerGoto => {
                self.open_viewer_goto_dialog();
            }
            AppCommand::ViewerToggleWrap => {
                let mut next = None;
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.toggle_wrap();
                    next = Some(viewer.wrap);
                }
                if let Some(wrap) = next {
                    self.set_status(format!(
                        "Viewer wrap {}",
                        if wrap { "enabled" } else { "disabled" }
                    ));
                }
            }
            AppCommand::ViewerToggleHex => {
                let mut next = None;
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.toggle_hex_mode();
                    next = Some(viewer.hex_mode);
                }
                if let Some(hex_mode) = next {
                    self.set_status(format!(
                        "Viewer mode {}",
                        if hex_mode { "hex" } else { "text" }
                    ));
                }
            }
            AppCommand::ViewerRefresh => self.refresh_viewer(),
            AppCommand::ViewerCopyContents => self.copy_viewer_contents(),
            _ => unreachable!("non-viewer command dispatched to viewer handler: {command:?}"),
        }

        CommandOutcome::Continue
    }

    fn apply_viewer_navigation(&mut self, motion: NavigationMotion) {
        match motion {
            NavigationMotion::Up => {
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.move_lines(-1);
                }
            }
            NavigationMotion::Down => {
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.move_lines(1);
                }
            }
            NavigationMotion::PageUp => {
                let viewer_page_step = self.settings.advanced.viewer_page_step;
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.move_pages(-1, viewer_page_step);
                }
            }
            NavigationMotion::PageDown => {
                let viewer_page_step = self.settings.advanced.viewer_page_step;
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.move_pages(1, viewer_page_step);
                }
            }
            NavigationMotion::Home => {
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.move_home();
                }
            }
            NavigationMotion::End => {
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.move_end();
                }
            }
            _ => {}
        }
    }

    pub(crate) fn open_selected_file_in_viewer(&mut self) -> bool {
        let Some(entry) = self.selected_non_parent_entry() else {
            return false;
        };
        if entry.is_dir() {
            return false;
        }

        let request = if let Some(location) = self.active_panel().mounted_location() {
            let Ok(path) = location.resolve_identity(&entry.path) else {
                return false;
            };
            JobRequest::Vfs(vfs::tasks::Task::View {
                path,
                quick_view: None,
            })
        } else {
            match self.active_panel().archive_location() {
                Some(location) => JobRequest::LoadArchiveViewer {
                    location: location.clone(),
                    path: entry.path.clone(),
                    quick_view: None,
                },
                None => JobRequest::LoadViewer {
                    path: entry.path.clone(),
                },
            }
        };
        self.queue_worker_job_request(request);
        true
    }

    pub fn active_viewer(&self) -> Option<&ViewerState> {
        self.routes.iter().rev().find_map(|route| match route {
            Route::Viewer(viewer) => Some(viewer),
            Route::CommandOutput(id) => self.command_outputs.get(id).map(|output| &output.viewer),
            _ => None,
        })
    }

    pub(crate) fn active_viewer_mut(&mut self) -> Option<&mut ViewerState> {
        let route = self
            .routes
            .iter_mut()
            .rev()
            .find(|route| matches!(route, Route::Viewer(_) | Route::CommandOutput(_)))?;
        match route {
            Route::Viewer(viewer) => Some(viewer),
            Route::CommandOutput(id) => self
                .command_outputs
                .get_mut(id)
                .map(|output| &mut output.viewer),
            _ => None,
        }
    }

    pub fn viewer_file_to_watch(&self) -> Option<&Arc<ViewerFileSnapshot>> {
        let file = self.active_viewer()?.local_file.as_ref()?;
        (!file.updated_on_disk).then_some(&file.snapshot)
    }

    pub fn set_viewer_change_detection_unavailable(&mut self, unavailable: bool) {
        if let Some(viewer) = self.active_viewer_mut()
            && let Some(file) = viewer.local_file.as_mut()
        {
            file.change_detection_unavailable = unavailable;
        }
    }

    pub fn mark_viewer_file_changed(&mut self, snapshot: &Arc<ViewerFileSnapshot>) {
        // A pending disk check may belong to a viewer that has since refreshed.
        if let Some(viewer) = self.active_viewer_mut()
            && let Some(file) = viewer.local_file.as_mut()
            && Arc::ptr_eq(&file.snapshot, snapshot)
        {
            file.updated_on_disk = true;
            file.change_detection_unavailable = false;
        }
    }

    fn refresh_viewer(&mut self) {
        let Some(viewer) = self.active_viewer() else {
            return;
        };
        let Some(file) = viewer.local_file.as_ref() else {
            self.set_status("Viewer refresh is available for local files");
            return;
        };
        if file.refresh_job.is_some() {
            return;
        }
        let request = JobRequest::RefreshViewer {
            path: viewer.path().to_path_buf(),
            nroff: viewer.uses_nroff(),
        };
        let job_id = self.queue_worker_job_request(request);
        if let Some(viewer) = self.active_viewer_mut()
            && let Some(file) = viewer.local_file.as_mut()
        {
            file.refresh_job = Some(job_id);
        }
        self.set_status("Refreshing viewer...");
    }

    pub(crate) fn finish_viewer_refresh(
        &mut self,
        job_id: JobId,
        result: Result<ViewerState, &JobError>,
    ) {
        let is_current = matches!(self.top_route(), Route::Viewer(viewer) if viewer.refresh_job() == Some(job_id));
        let Some(viewer) = self
            .routes
            .iter_mut()
            .rev()
            .chain(self.suspended_screens.iter_mut().flatten())
            .find_map(|route| match route {
                Route::Viewer(viewer) if viewer.refresh_job() == Some(job_id) => Some(viewer),
                _ => None,
            })
        else {
            return;
        };
        if let Some(file) = viewer.local_file.as_mut() {
            file.refresh_job = None;
        }
        let status = match result {
            Ok(refreshed) => {
                viewer.replace_after_refresh(refreshed);
                String::from("Viewer refreshed")
            }
            Err(error) if error.is_canceled() => String::from("Viewer refresh canceled"),
            Err(error) => format!("Viewer refresh failed ({})", error.user_message()),
        };
        if is_current {
            self.set_status(status);
        }
    }

    fn copy_viewer_contents(&mut self) {
        let Some(viewer) = self.active_viewer() else {
            return;
        };
        let Some(text) = viewer.clipboard_text() else {
            self.set_status(format!(
                "Clipboard copy failed (OSC 52 copy limit is {MAX_CLIPBOARD_TEXT_BYTES} bytes)"
            ));
            return;
        };
        let success_message = if viewer.text_is_preview() {
            "Sent viewer preview to the terminal for copying"
        } else {
            "Sent viewer contents to the terminal for copying"
        };
        self.pending_clipboard_copy_requests
            .push(ClipboardCopyRequest {
                text,
                success_message,
            });
        self.set_status("Copying viewer contents to clipboard...");
    }

    pub(crate) fn open_viewer_search_dialog(&mut self, direction: ViewerSearchDirection) {
        let Some(viewer) = self.active_viewer() else {
            self.set_status("Viewer is not active");
            return;
        };
        let initial_query = viewer.last_search_query().unwrap_or("").to_string();

        let title = match direction {
            ViewerSearchDirection::Forward => "Search",
            ViewerSearchDirection::Backward => "Search Backward",
        };
        let prompt = match direction {
            ViewerSearchDirection::Forward => "Search text:",
            ViewerSearchDirection::Backward => "Search text (backward):",
        };

        self.push_dialog(
            DialogState::input(title, prompt, initial_query),
            PendingDialogAction::ViewerSearch { direction },
        );
        self.set_status(title);
    }

    pub(crate) fn open_viewer_goto_dialog(&mut self) {
        let Some(viewer) = self.active_viewer() else {
            self.set_status("Viewer is not active");
            return;
        };
        let current_line = viewer.current_line_number().to_string();

        self.push_dialog(
            DialogState::input(
                "Goto",
                "Line number, @offset, or 0xHEX offset:",
                current_line,
            ),
            PendingDialogAction::ViewerGoto,
        );
        self.set_status("Goto");
    }

    pub(crate) fn continue_viewer_search(&mut self, direction: Option<ViewerSearchDirection>) {
        let Some(viewer) = self.active_viewer_mut() else {
            self.set_status("Viewer is not active");
            return;
        };
        let Some(_) = viewer.last_search_query() else {
            self.set_status("No previous search query");
            return;
        };

        if let Some(line) = viewer.continue_search(direction) {
            self.set_status(format!("Search hit at line {}", line.saturating_add(1)));
        } else {
            self.set_status("Search text not found");
        }
    }
}
