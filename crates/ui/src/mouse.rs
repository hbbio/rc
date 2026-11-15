use ratatui::layout::{Constraint, Flex, Layout, Position, Rect};
use ratatui::widgets::{Block, Borders};
use rc_core::{
    ActivePanel, AppCommand, AppState, DialogKind, MouseClickCommands, MouseClickTarget,
    NavigationMotion, NavigationTarget, PanelCommand, PanelListingFormat, PanelViewMode,
    PanelViewport, Route,
};

use crate::{
    BriefGrid, dual_panel_areas, panel_content_areas, panel_display_format, root_areas,
    uses_single_panel_layout,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScrollDirection {
    Up,
    Down,
}

/// Wheel and terminal touchpad reports scroll three rows. Only the top route
/// receives them; panels underneath a modal never move.
pub fn commands_for_scroll(
    state: &AppState,
    column: u16,
    row: u16,
    width: u16,
    height: u16,
    direction: ScrollDirection,
) -> Vec<AppCommand> {
    use crate::{centered_rect, terminal_rect};
    use rc_core::layout::{
        FIND_DIALOG_HEIGHT, FIND_DIALOG_WIDTH, ScreenRect, find_results_layout, hotlist_layout,
        listbox_dialog_layout, tree_layout,
    };
    let viewport = Rect::new(0, 0, width, height);
    let point = Position::new(column, row);
    if !viewport.contains(point) {
        return Vec::new();
    }
    let up = direction == ScrollDirection::Up;
    let motion = if up {
        NavigationMotion::Up
    } else {
        NavigationMotion::Down
    };
    let navigate = |target| AppCommand::Navigate(target, motion);
    let list = if up {
        AppCommand::DialogListboxUp
    } else {
        AppCommand::DialogListboxDown
    };
    let screen = ScreenRect::new(0, 0, width, height);
    let inner = |area| Block::default().borders(Borders::ALL).inner(area);
    let (area, command) = match state.top_route() {
        Route::FileManager => return panel_scroll(state, viewport, point, up),
        Route::Viewer(_) | Route::CommandOutput(_) => (
            inner(root_areas(viewport, state)[1]),
            navigate(NavigationTarget::Viewer),
        ),
        Route::Help(_) => {
            let (width, height) = state.help_dialog_size();
            (
                inner(centered_rect(viewport, width, height)),
                navigate(NavigationTarget::Help),
            )
        }
        Route::Jobs => {
            let (width, height) = state.jobs_dialog_size();
            (
                inner(centered_rect(viewport, width, height)),
                navigate(NavigationTarget::Jobs),
            )
        }
        Route::Settings(_) => (inner(centered_rect(viewport, 82, 20)), list),
        Route::FindResults(_) => (
            terminal_rect(find_results_layout(screen).list),
            navigate(NavigationTarget::FindResults),
        ),
        Route::Tree(_) => (
            terminal_rect(tree_layout(screen).list),
            navigate(NavigationTarget::Tree),
        ),
        Route::Hotlist => (
            terminal_rect(hotlist_layout(screen).list),
            navigate(NavigationTarget::Hotlist),
        ),
        Route::Menu(menu) => {
            if height <= 2 {
                return Vec::new();
            }
            let x = menu.popup_origin_x().min(width.saturating_sub(1));
            let popup = Rect::new(
                x,
                1,
                state.menu_popup_width(menu).min(width.saturating_sub(x)),
                menu.popup_height().min(height.saturating_sub(1)),
            );
            (inner(popup), navigate(NavigationTarget::Menu))
        }
        Route::Dialog(dialog) => match &dialog.kind {
            DialogKind::Listbox(listbox) => (
                terminal_rect(
                    listbox_dialog_layout(
                        screen,
                        if listbox.footer_hint.is_some() { 2 } else { 1 },
                    )
                    .list,
                ),
                list,
            ),
            DialogKind::QuickCd(_) => {
                let area = inner(centered_rect(
                    viewport,
                    FIND_DIALOG_WIDTH,
                    FIND_DIALOG_HEIGHT,
                ));
                let areas = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Length(3),
                    Constraint::Min(3),
                    Constraint::Length(2),
                ])
                .split(area);
                (areas[2], list)
            }
            _ => return Vec::new(),
        },
        Route::CommandLine(_) => return Vec::new(),
    };
    if area.contains(point) {
        vec![command; 3]
    } else {
        Vec::new()
    }
}

fn panel_scroll(state: &AppState, viewport: Rect, point: Position, up: bool) -> Vec<AppCommand> {
    let content = root_areas(viewport, state)[1];
    let (panel_id, area) = if uses_single_panel_layout(state) {
        (state.active_panel, content)
    } else {
        let areas = dual_panel_areas(content);
        let Some(panel) = [ActivePanel::Left, ActivePanel::Right]
            .into_iter()
            .find(|panel| areas[panel.index()].contains(point))
        else {
            return Vec::new();
        };
        (panel, areas[panel.index()])
    };
    let inner = Block::default().borders(Borders::ALL).inner(area);
    if !inner.contains(point) {
        return Vec::new();
    }
    match state.panel_view_mode(panel_id) {
        PanelViewMode::QuickView => {
            return vec![AppCommand::Panel(
                panel_id,
                PanelCommand::ScrollQuickView(if up { -3 } else { 3 }),
            )];
        }
        PanelViewMode::Info => return Vec::new(),
        PanelViewMode::Listing => {}
    }
    let mut listing = panel_content_areas(inner)[0];
    let panel = &state.panels[panel_id.index()];
    let columns = if panel_display_format(state, panel_id) == PanelListingFormat::Brief {
        BriefGrid::new(panel.entries.len(), listing.width).columns
    } else {
        listing.y = listing.y.saturating_add(1);
        listing.height = listing.height.saturating_sub(1);
        1
    };
    if !listing.contains(point) || panel.entries.is_empty() {
        return Vec::new();
    }
    let total = panel.entries.len().div_ceil(columns);
    let rows = usize::from(listing.height);
    let visible = rows.min(total);
    let (first, _) = panel.visible_window(rows, columns);
    let shift = |value: usize, max: usize| {
        if up {
            value.saturating_sub(3)
        } else {
            value.saturating_add(3).min(max)
        }
    };
    let cursor = panel.cursor.min(panel.entries.len() - 1);
    let column = cursor / total;
    let last_row = (panel.entries.len() - 1 - column * total).min(total - 1);
    // A partial final Brief column must stop at its last cell, never jump
    // backward into a preceding column when scrolling down.
    let first = shift(first, total.saturating_sub(visible)).min(last_row);
    let selected =
        shift(cursor % total, last_row).clamp(first, (first + visible - 1).min(last_row));
    vec![AppCommand::Panel(
        panel_id,
        PanelCommand::SelectAtInViewport {
            index: column * total + selected,
            viewport: PanelViewport {
                first_row: first,
                rows,
                columns,
            },
        },
    )]
}

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
