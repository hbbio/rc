use ratatui::layout::{Flex, Layout, Position, Rect};
use ratatui::widgets::{Block, Borders};
use rc_core::{
    ActivePanel, AppCommand, AppState, MouseClickCommands, MouseClickTarget, PanelCommand,
    PanelListingFormat, PanelViewMode, PanelViewport, Route,
};

use crate::{
    BriefGrid, dual_panel_areas, panel_content_areas, panel_display_format, root_areas,
    uses_single_panel_layout,
};

/// Resolve clicks using the same panel geometry and visible rows as rendering.
pub fn commands_for_left_click(
    state: &AppState,
    column: u16,
    row: u16,
    viewport_width: u16,
    viewport_height: u16,
) -> Option<MouseClickCommands> {
    let viewport = Rect::new(0, 0, viewport_width, viewport_height);
    let position = Position::new(column, row);
    if !viewport.contains(position) {
        return None;
    }
    if let Some(commands) =
        state.commands_for_left_click(column, row, viewport_width, viewport_height)
    {
        return Some(commands);
    }
    if !matches!(state.top_route(), Route::FileManager) {
        return None;
    }

    let content = root_areas(viewport, state)[1];
    let (panel_id, area) = if uses_single_panel_layout(state) {
        (state.active_panel, content)
    } else {
        let areas = dual_panel_areas(content);
        let panel = [ActivePanel::Left, ActivePanel::Right]
            .into_iter()
            .find(|panel| areas[panel.index()].contains(position))?;
        (panel, areas[panel.index()])
    };
    if state.panel_view_mode(panel_id) != PanelViewMode::Listing {
        return None;
    }

    let inner = Block::default().borders(Borders::ALL).inner(area);
    let listing = panel_content_areas(inner)[0];
    if !listing.contains(position) {
        return None;
    }
    let panel = &state.panels[panel_id.index()];
    let (index, viewport) = match panel_display_format(state, panel_id) {
        PanelListingFormat::Brief => {
            let grid = BriefGrid::new(panel.entries.len(), listing.width);
            // Match Table's column spacing, flex, and rounding at every width.
            let columns = Layout::horizontal(grid.column_widths())
                .flex(Flex::Start)
                .spacing(1)
                .split(listing);
            let column = columns.iter().position(|area| area.contains(position))?;
            let viewport_rows = usize::from(listing.height);
            let (start, end) = panel.visible_window(viewport_rows, grid.columns);
            let row = start + usize::from(row - listing.y);
            if row >= end {
                return None;
            }
            (
                grid.index_at(column, row)?,
                PanelViewport {
                    first_row: start,
                    rows: viewport_rows,
                    columns: grid.columns,
                },
            )
        }
        PanelListingFormat::Full | PanelListingFormat::Long | PanelListingFormat::User => {
            // Full and Long tables reserve their first row for column headers.
            let row = row.checked_sub(listing.y + 1)?;
            let viewport_rows = usize::from(listing.height.saturating_sub(1));
            let (start, end) = panel.visible_window(viewport_rows, 1);
            let index = start + usize::from(row);
            if index >= end {
                return None;
            }
            (
                index,
                PanelViewport {
                    first_row: start,
                    rows: viewport_rows,
                    columns: 1,
                },
            )
        }
    };

    Some(MouseClickCommands {
        primary: AppCommand::Panel(
            panel_id,
            PanelCommand::SelectAtInViewport { index, viewport },
        ),
        activation: Some(AppCommand::OpenEntry),
        target: MouseClickTarget::PanelEntry(panel_id, panel.entries.get(index)?.path.clone()),
    })
}
