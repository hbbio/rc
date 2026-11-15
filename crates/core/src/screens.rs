use crate::*;

const MAX_SUSPENDED_SCREENS: usize = 16;

#[derive(Clone, Debug)]
pub(crate) enum ScreenTarget {
    Panels,
    Current,
    Suspended(usize),
    Output(JobId),
    Jobs,
    Shell,
}

impl AppState {
    pub(crate) fn output_screen_is_open(&self, id: JobId) -> bool {
        self.routes
            .iter()
            .chain(self.suspended_screens.iter().flatten())
            .any(|route| matches!(route, Route::CommandOutput(open) if *open == id))
    }

    fn screen_label(&self, routes: &[Route]) -> String {
        match routes.last() {
            Some(Route::Viewer(viewer)) => format!("View: {}", viewer.title()),
            Some(Route::CommandOutput(id)) => self.command_outputs.get(id).map_or_else(
                || format!("Output #{id}"),
                |output| format!("Output #{id}: {}", output.viewer.title()),
            ),
            Some(Route::FindResults(results)) => {
                format!("Find: {}", results.spec.display_pattern())
            }
            Some(Route::Tree(tree)) => format!("Tree: {}", tree.root().display()),
            Some(Route::Jobs) => String::from("Background jobs"),
            Some(Route::Hotlist) => String::from("Directory hotlist"),
            Some(Route::Help(_)) => String::from("Help"),
            _ => String::from("File manager"),
        }
    }

    pub(crate) fn open_screen_list(&mut self) {
        // Modal drafts and the tree picker share pending state with their parent.
        // Keep those workflows together until they are accepted or canceled.
        if self.pending_find_tree_picker.is_some()
            || self.routes.iter().any(|r| {
                matches!(
                    r,
                    Route::Dialog(_) | Route::Settings(_) | Route::CommandLine(_) | Route::Menu(_)
                )
            })
        {
            self.set_status("Finish the current dialog before switching screens");
            return;
        }
        let mut labels = vec![String::from("File manager")];
        let mut targets = vec![ScreenTarget::Panels];
        let selected = if self.routes.len() > 1 {
            labels.push(format!("{} (current)", self.screen_label(&self.routes)));
            targets.push(ScreenTarget::Current);
            1
        } else {
            0
        };
        for (index, routes) in self.suspended_screens.iter().enumerate() {
            labels.push(self.screen_label(routes));
            targets.push(ScreenTarget::Suspended(index));
        }
        let mut output_ids: Vec<_> = self
            .command_outputs
            .keys()
            .copied()
            .filter(|id| !self.output_screen_is_open(*id))
            .collect();
        output_ids.sort_by_key(|id| id.0);
        for id in output_ids {
            labels.push(self.screen_label(&[Route::CommandOutput(id)]));
            targets.push(ScreenTarget::Output(id));
        }
        if !self
            .routes
            .iter()
            .chain(self.suspended_screens.iter().flatten())
            .any(|r| matches!(r, Route::Jobs))
        {
            labels.push(String::from("Background jobs"));
            targets.push(ScreenTarget::Jobs);
        }
        if self.shell_available {
            labels.push(String::from("Shell"));
            targets.push(ScreenTarget::Shell);
        }
        let labels = labels
            .into_iter()
            .map(|s| sanitize_display_line(&s))
            .collect();
        self.push_dialog(
            DialogState::listbox("Screens", labels, selected),
            PendingDialogAction::SelectScreen { targets },
        );
        self.set_status("Choose a screen; the current screen stays open");
    }

    pub(crate) fn select_screen(&mut self, target: ScreenTarget) {
        if matches!(target, ScreenTarget::Shell) {
            self.pending_panel_toggle = true;
            return;
        }
        if matches!(target, ScreenTarget::Current)
            || (matches!(target, ScreenTarget::Panels) && self.routes.len() == 1)
        {
            return;
        }
        let leaving_screen = self.routes.len() > 1;
        if leaving_screen
            && self.suspended_screens.len() >= MAX_SUSPENDED_SCREENS
            && !matches!(target, ScreenTarget::Suspended(_))
        {
            self.set_status("Screen limit reached; restore and close a screen first");
            return;
        }
        let next = match target {
            ScreenTarget::Current | ScreenTarget::Shell => return,
            ScreenTarget::Panels => vec![Route::FileManager],
            ScreenTarget::Suspended(index) => {
                if index >= self.suspended_screens.len() {
                    return;
                }
                self.suspended_screens.remove(index)
            }
            ScreenTarget::Output(id) => {
                if !self.command_outputs.contains_key(&id) {
                    self.set_status("That command output has expired");
                    return;
                }
                vec![Route::FileManager, Route::CommandOutput(id)]
            }
            ScreenTarget::Jobs => vec![Route::FileManager, Route::Jobs],
        };
        let previous = std::mem::replace(&mut self.routes, next);
        if leaving_screen {
            self.suspended_screens.push(previous);
        }
        self.xmap_pending = false;
        // Filesystem watches only cover the visible tree. Refresh a restored
        // tree to pick up mutations that happened while it was suspended.
        if let Some(root) = self.directory_tree().map(|tree| tree.root().to_path_buf()) {
            self.rescan_tree_for_impacts(&[root]);
        }
        self.set_status(format!("Screen: {}", self.screen_label(&self.routes)));
    }
}
