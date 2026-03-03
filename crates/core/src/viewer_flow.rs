use crate::viewer::ViewerSearchDirection;
use crate::*;

impl AppState {
    pub(super) fn apply_viewer_command(&mut self, command: AppCommand) -> CommandOutcome {
        match command {
            AppCommand::ViewerMoveUp => {
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.move_lines(-1);
                }
            }
            AppCommand::ViewerMoveDown => {
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.move_lines(1);
                }
            }
            AppCommand::ViewerPageUp => {
                let viewer_page_step = self.settings.advanced.viewer_page_step;
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.move_pages(-1, viewer_page_step);
                }
            }
            AppCommand::ViewerPageDown => {
                let viewer_page_step = self.settings.advanced.viewer_page_step;
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.move_pages(1, viewer_page_step);
                }
            }
            AppCommand::ViewerHome => {
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.move_home();
                }
            }
            AppCommand::ViewerEnd => {
                if let Some(viewer) = self.active_viewer_mut() {
                    viewer.move_end();
                }
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
            _ => unreachable!("non-viewer command dispatched to viewer handler: {command:?}"),
        }

        CommandOutcome::Continue
    }

    pub(crate) fn open_selected_file_in_viewer(&mut self) -> bool {
        let Some(entry) = self.selected_non_parent_entry() else {
            return false;
        };
        if entry.is_dir {
            return false;
        }

        self.queue_worker_job_request(JobRequest::LoadViewer {
            path: entry.path.clone(),
        });
        true
    }

    pub fn active_viewer(&self) -> Option<&ViewerState> {
        self.routes.iter().rev().find_map(|route| match route {
            Route::Viewer(viewer) => Some(viewer),
            _ => None,
        })
    }

    pub(crate) fn active_viewer_mut(&mut self) -> Option<&mut ViewerState> {
        self.routes.iter_mut().rev().find_map(|route| match route {
            Route::Viewer(viewer) => Some(viewer),
            _ => None,
        })
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

        self.pending_dialog_action = Some(PendingDialogAction::ViewerSearch { direction });
        self.routes.push(Route::Dialog(DialogState::input(
            title,
            prompt,
            initial_query,
        )));
        self.set_status(title);
    }

    pub(crate) fn open_viewer_goto_dialog(&mut self) {
        let Some(viewer) = self.active_viewer() else {
            self.set_status("Viewer is not active");
            return;
        };
        let current_line = viewer.current_line_number().to_string();

        self.pending_dialog_action = Some(PendingDialogAction::ViewerGoto);
        self.routes.push(Route::Dialog(DialogState::input(
            "Goto",
            "Line number, @offset, or 0xHEX offset:",
            current_line,
        )));
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
