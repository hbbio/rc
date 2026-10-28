use std::path::Path;

use crate::{ActivePanel, AppState, DialogKind, DialogState, PanelFilter, PendingDialogAction};

impl AppState {
    pub(crate) fn open_group_selection_dialog(
        &mut self,
        panel: ActivePanel,
        select: bool,
        filter: &PanelFilter,
    ) {
        let mut dialog = DialogState::filter(filter);
        dialog.title = if select {
            "Select group"
        } else {
            "Unselect group"
        }
        .to_string();
        if let DialogKind::Filter(state) = &mut dialog.kind {
            state.require_pattern = true;
        }
        self.push_dialog(
            dialog,
            PendingDialogAction::SetGroupSelection { panel, select },
        );
    }

    pub(crate) fn apply_group_selection(
        &mut self,
        panel: ActivePanel,
        select: bool,
        filter: PanelFilter,
    ) {
        if filter.pattern.is_empty() {
            self.open_group_selection_dialog(panel, select, &filter);
            self.set_status("Enter a selection pattern");
            return;
        }
        // Listing filters keep directories visible in files-only mode. Selection
        // must instead leave directory tags unchanged and match their names only
        // when the caller explicitly includes them.
        let matcher = match (PanelFilter {
            files_only: false,
            ..filter.clone()
        })
        .compile()
        {
            Ok(matcher) => matcher,
            Err(error) => {
                self.open_group_selection_dialog(panel, select, &filter);
                self.set_status(format!("Selection unchanged: {error}"));
                return;
            }
        };
        let state = &mut self.panels[panel.index()];
        let mut changed = 0;
        for entry in &state.entries {
            if entry.is_parent()
                || (filter.files_only && entry.is_dir())
                || !entry.path.file_name().is_some_and(|name| {
                    let encoding = state
                        .filename_encoding
                        .effective_for_name(name, &state.source);
                    if encoding == crate::FilenameEncoding::Utf8 {
                        matcher.matches_path(Path::new(name))
                    } else {
                        matcher.matches_path(Path::new(&encoding.decode(name)))
                    }
                })
            {
                continue;
            }
            changed += usize::from(if select {
                state.tagged.insert(entry.path.clone())
            } else {
                state.tagged.remove(&entry.path)
            });
        }
        self.sync_selection_size(panel, false);
        let action = if select { "Selected" } else { "Unselected" };
        self.set_status(format!("{action} {changed} entries"));
    }
}
