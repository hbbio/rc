#![forbid(unsafe_code)]

mod bundled_skins;
mod skin;

use chrono::{DateTime, Local};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState, Wrap,
};
use rc_core::keymap::KeyContext;
use rc_core::layout::{
    FIND_DIALOG_HEIGHT, FIND_DIALOG_WIDTH, STANDARD_DIALOG_HEIGHT, STANDARD_DIALOG_WIDTH,
    ScreenRect, centered_overlay_rect, find_results_layout, hotlist_layout, listbox_dialog_layout,
    tree_layout, visible_window,
};
use rc_core::{
    ActivePanel, AppCommand, AppState, DialogButtonFocus, DialogKind, DialogState, FileEntry,
    FilterDialogField, FindDialogField, FindNameMode, FindResultsState, FindResultsStatus,
    HelpSpan, HelpState, JobRecord, JobStatus, MenuState, NavigationMotion, NavigationTarget,
    PairInputField, PanelCommand, PanelListingFormat, PanelState, PanelViewMode,
    QuickCdSearchStatus, QuickViewState, Route, SelectionSizeState, SettingsScreenState,
    TreeLoadState, TreeState, ViewerState, top_menus,
};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;
use syntect::highlighting::{
    Color as SyntectColor, FontStyle, HighlightIterator, HighlightState, Highlighter,
    Style as SyntectStyle, Theme,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};
use unicode_width::UnicodeWidthStr;

use skin::{UiSkin, current_skin};
pub use skin::{
    configure_skin, configure_skin_with_search_roots, current_skin_name, list_available_skins,
    list_available_skins_with_search_roots,
};

struct HighlightResources {
    theme: Theme,
}

struct CachedHighlightResources {
    skin_name: String,
    resources: Arc<HighlightResources>,
}

static HIGHLIGHT_RESOURCES: OnceLock<Mutex<Option<CachedHighlightResources>>> = OnceLock::new();
static HIGHLIGHT_SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static VIEWER_HIGHLIGHT_CACHE: OnceLock<Mutex<Option<CachedViewerHighlight>>> = OnceLock::new();
const PANEL_SIZE_COL_WIDTH: usize = 12;
const PANEL_SIZE_VALUE_WIDTH: usize = PANEL_SIZE_COL_WIDTH - 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ViewerHighlightKey {
    content_hash: u64,
    content_len: usize,
    path_hash: u64,
}

struct CachedViewerHighlight {
    key: ViewerHighlightKey,
    raw_lines: Vec<String>,
    highlighted_lines: Vec<Line<'static>>,
    parse_state: ParseState,
    highlight_state: HighlightState,
}

pub fn render(frame: &mut Frame, state: &AppState) {
    let skin = current_skin();
    let job_counts = state.jobs_status_counts();
    let menu_height = if state.show_menu_bar() { 1 } else { 0 };
    let button_height = if state.show_button_bar() { 1 } else { 0 };
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(menu_height),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(button_height),
        ])
        .split(frame.area());

    let active_menu = match state.top_route() {
        Route::Menu(menu) => Some(menu.active_menu),
        _ => None,
    };
    if state.show_menu_bar() {
        render_menu_bar(frame, root[0], skin.as_ref(), active_menu);
    }

    if let Some(viewer) = state.active_viewer() {
        render_viewer(frame, root[1], viewer, skin.as_ref());
    } else {
        if uses_single_panel_layout(state) {
            let panel = state.active_panel;
            render_panel(
                frame,
                root[1],
                panel,
                &state.panels[panel.index()],
                true,
                skin.as_ref(),
                state,
            );
        } else {
            let panel_areas = dual_panel_areas(root[1]);

            render_panel(
                frame,
                panel_areas[0],
                ActivePanel::Left,
                &state.panels[0],
                state.active_panel == ActivePanel::Left,
                skin.as_ref(),
                state,
            );
            render_panel(
                frame,
                panel_areas[1],
                ActivePanel::Right,
                &state.panels[1],
                state.active_panel == ActivePanel::Right,
                skin.as_ref(),
                state,
            );
        }
    }

    let status = if state.show_debug_status() {
        format!(
            "context: {:?} | routes:{} | skin:{} | jobs q:{} r:{} ok:{} cx:{} err:{} | {}",
            state.key_context(),
            state.route_depth(),
            skin.name(),
            job_counts.queued,
            job_counts.running,
            job_counts.succeeded,
            job_counts.canceled,
            job_counts.failed,
            state.status_line
        )
    } else {
        state.status_line.clone()
    };
    let status = fit_single_line(status, root[2].width as usize);
    frame.render_widget(Paragraph::new(status), root[2]);
    if state.show_button_bar() {
        render_button_bar(frame, root[3], skin.as_ref(), state);
    }

    match state.top_route() {
        Route::Dialog(dialog) => render_dialog(frame, dialog, skin.as_ref()),
        Route::Jobs => render_jobs_screen(frame, state, skin.as_ref()),
        Route::Viewer(_) => {}
        Route::FindResults(results) => {
            render_find_results_screen(frame, state, results, skin.as_ref())
        }
        Route::Tree(tree) => render_tree_screen(frame, state, tree, skin.as_ref()),
        Route::Hotlist => render_hotlist_screen(frame, state, skin.as_ref()),
        Route::Help(help) => render_help_screen(frame, state, help, skin.as_ref()),
        Route::Menu(menu) => render_menu_overlay(frame, state, menu, skin.as_ref()),
        Route::Settings(settings) => render_settings_screen(frame, settings, skin.as_ref()),
        Route::FileManager => {}
    }
}

fn uses_single_panel_layout(state: &AppState) -> bool {
    let panels = [ActivePanel::Left, ActivePanel::Right];
    panels
        .into_iter()
        .all(|panel| state.panel_view_mode(panel) == PanelViewMode::Listing)
        && panels
            .into_iter()
            .any(|panel| state.panel_listing_format(panel) == PanelListingFormat::Long)
}

fn dual_panel_areas(area: Rect) -> [Rect; 2] {
    let areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    [areas[0], areas[1]]
}

fn render_menu_bar(frame: &mut Frame, area: Rect, skin: &UiSkin, active_menu: Option<usize>) {
    let menu_style = skin.style("menu", "_default_");
    let hot_style = skin.style("menu", "menuhot");
    let mut spans: Vec<Span<'_>> = vec![Span::raw(" ")];
    for (index, item) in top_menus().iter().map(|menu| menu.title).enumerate() {
        if active_menu == Some(index) {
            spans.push(Span::styled(item, hot_style));
            spans.push(Span::raw("  "));
            continue;
        }
        let mut chars = item.chars();
        let first = chars.next().unwrap_or_default().to_string();
        let rest: String = chars.collect();
        spans.push(Span::styled(first, hot_style));
        spans.push(Span::styled(rest, menu_style));
        spans.push(Span::raw("  "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).style(menu_style), area);
}

type ButtonBinding = Option<(KeyContext, AppCommand, Option<u8>)>;
type ButtonLabel<'a> = (&'a str, &'a str, ButtonBinding);

fn render_button_bar(frame: &mut Frame, area: Rect, skin: &UiSkin, state: &AppState) {
    let hotkey_style = skin.style("buttonbar", "hotkey");
    let button_style = skin.style("buttonbar", "button");
    let labels: [ButtonLabel<'_>; 10] = match state.top_route() {
        Route::FindResults(results) => {
            let pause_label = if matches!(results.status, FindResultsStatus::Paused) {
                "Continue"
            } else {
                "Pause"
            };
            [
                (
                    "1",
                    "Help",
                    Some((KeyContext::FindResults, AppCommand::OpenHelp, Some(1))),
                ),
                ("2", "", None),
                ("3", "", None),
                (
                    "4",
                    "Again",
                    Some((
                        KeyContext::FindResults,
                        AppCommand::FindResultsAgain,
                        Some(4),
                    )),
                ),
                (
                    "5",
                    "Panelize",
                    Some((
                        KeyContext::FindResults,
                        AppCommand::FindResultsPanelize,
                        Some(5),
                    )),
                ),
                (
                    "6",
                    pause_label,
                    Some((
                        KeyContext::FindResults,
                        AppCommand::FindResultsTogglePause,
                        Some(6),
                    )),
                ),
                ("7", "", None),
                ("8", "", None),
                ("9", "", None),
                (
                    "10",
                    "Close",
                    Some((
                        KeyContext::FindResults,
                        AppCommand::CloseFindResults,
                        Some(10),
                    )),
                ),
            ]
        }
        Route::Help(_) => [
            (
                "1",
                "Help",
                Some((KeyContext::Help, AppCommand::OpenHelp, Some(1))),
            ),
            (
                "2",
                "Index",
                Some((KeyContext::Help, AppCommand::HelpIndex, Some(2))),
            ),
            (
                "3",
                "Prev",
                Some((KeyContext::Help, AppCommand::HelpBack, Some(3))),
            ),
            ("4", "", None),
            ("5", "", None),
            ("6", "", None),
            ("7", "", None),
            ("8", "", None),
            ("9", "", None),
            (
                "10",
                "Quit",
                Some((KeyContext::Help, AppCommand::CloseHelp, Some(10))),
            ),
        ],
        _ => [
            (
                "1",
                "Help",
                Some((KeyContext::FileManager, AppCommand::OpenHelp, Some(1))),
            ),
            (
                "2",
                "Menu",
                Some((KeyContext::FileManager, AppCommand::OpenMenu, Some(2))),
            ),
            (
                "3",
                "View",
                Some((KeyContext::FileManager, AppCommand::OpenEntry, Some(3))),
            ),
            (
                "4",
                "Edit",
                Some((KeyContext::FileManager, AppCommand::EditEntry, Some(4))),
            ),
            (
                "5",
                "Copy",
                Some((KeyContext::FileManager, AppCommand::Copy, Some(5))),
            ),
            (
                "6",
                "RenMov",
                Some((KeyContext::FileManager, AppCommand::Move, Some(6))),
            ),
            (
                "7",
                "Mkdir",
                Some((
                    KeyContext::FileManager,
                    AppCommand::OpenInputDialog,
                    Some(7),
                )),
            ),
            (
                "8",
                "Delete",
                Some((KeyContext::FileManager, AppCommand::Delete, Some(8))),
            ),
            (
                "9",
                "PullDn",
                Some((KeyContext::FileManager, AppCommand::OpenMenu, Some(9))),
            ),
            (
                "10",
                "Quit",
                Some((KeyContext::FileManager, AppCommand::Quit, Some(10))),
            ),
        ],
    };

    let mut spans: Vec<Span<'_>> = Vec::new();
    for (index, (fallback_hotkey, label, binding)) in labels.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" ", button_style));
        }
        let hotkey = binding
            .and_then(|(context, command, preferred_f)| {
                button_bar_hotkey_label(state, context, command, preferred_f)
            })
            .unwrap_or_else(|| fallback_hotkey.to_string());
        spans.push(Span::styled(hotkey, hotkey_style));
        spans.push(Span::styled(label, button_style));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)).style(button_style), area);
}

fn button_bar_hotkey_label(
    state: &AppState,
    context: KeyContext,
    command: AppCommand,
    preferred_function_key: Option<u8>,
) -> Option<String> {
    let labels = state.keybinding_labels(context, command)?;
    if let Some(function_key) = preferred_function_key {
        let preferred_label = format!("F{function_key}");
        if labels.iter().any(|label| label == &preferred_label) {
            return Some(preferred_label);
        }
    }
    labels.first().cloned()
}

fn keybinding_primary_or(
    state: &AppState,
    context: KeyContext,
    command: AppCommand,
    fallback: &str,
) -> String {
    state
        .keybinding_primary_label(context, command)
        .map_or_else(|| fallback.to_string(), ToString::to_string)
}

fn keybinding_joined_or(
    state: &AppState,
    context: KeyContext,
    command: AppCommand,
    fallback: &str,
    limit: usize,
) -> String {
    state
        .keybinding_joined_label(context, command, "/", limit)
        .unwrap_or_else(|| fallback.to_string())
}

fn panel_title(panel: &PanelState, format: PanelListingFormat) -> String {
    let panelize_suffix = if panel.is_panelized() {
        " | panelize"
    } else {
        ""
    };
    let filter_suffix = if panel.filter().is_active() {
        format!(" | filter:{}", panel.filter().display_pattern())
    } else {
        String::new()
    };
    format!(
        "{} | sort:{}{} | {}{} | tagged:{}{}",
        format.title_label(),
        panel.sort_label(),
        filter_suffix,
        panel.cwd.to_string_lossy(),
        panelize_suffix,
        panel.tagged_count(),
        if panel.loading { " | loading..." } else { "" }
    )
}

fn fit_single_line(text: impl AsRef<str>, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let sanitized = sanitize_single_line(text.as_ref());
    if UnicodeWidthStr::width(sanitized.as_str()) <= width {
        return sanitized;
    }

    if width <= 3 {
        return ".".repeat(width);
    }

    let prefix_width = width - 3;
    let mut truncated = String::new();
    for ch in sanitized.chars() {
        truncated.push(ch);
        if UnicodeWidthStr::width(truncated.as_str()) > prefix_width {
            truncated.pop();
            break;
        }
    }
    truncated.push_str("...");
    debug_assert!(UnicodeWidthStr::width(truncated.as_str()) <= width);
    truncated
}

fn sanitize_single_line(text: &str) -> String {
    text.chars()
        .map(|ch| if ch == '\n' || ch == '\r' { ' ' } else { ch })
        .collect()
}

fn render_panel(
    frame: &mut Frame,
    area: Rect,
    panel_id: ActivePanel,
    panel: &PanelState,
    active: bool,
    skin: &UiSkin,
    app: &AppState,
) {
    match app.panel_view_mode(panel_id) {
        PanelViewMode::Info => {
            render_info_panel(frame, area, panel_id, skin, app);
            return;
        }
        PanelViewMode::QuickView => {
            render_quick_view_panel(frame, area, panel_id, skin, app);
            return;
        }
        PanelViewMode::Listing => {}
    }

    let configured_format = app.panel_listing_format(panel_id);
    let format = if configured_format == PanelListingFormat::Long
        && app.panel_view_mode(panel_id.other()) != PanelViewMode::Listing
    {
        PanelListingFormat::Full
    } else {
        configured_format
    };
    let title = fit_single_line(
        panel_title(panel, format),
        area.width.saturating_sub(2) as usize,
    );

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_set(skin.panel_border_set())
        .border_style(skin.style("core", "_default_"))
        .style(skin.style("core", "_default_"));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let panel_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    if panel.entries.is_empty() {
        let placeholder = if panel.loading {
            "<loading...>"
        } else {
            "<empty>"
        };
        frame.render_widget(
            Paragraph::new(placeholder)
                .style(skin.style("core", "_default_"))
                .alignment(Alignment::Left),
            panel_layout[0],
        );
    } else {
        match format {
            PanelListingFormat::Full => {
                render_full_panel_entries(frame, panel_layout[0], panel, active, skin)
            }
            PanelListingFormat::Brief => {
                render_brief_panel_entries(frame, panel_layout[0], panel, active, skin)
            }
            PanelListingFormat::Long => {
                render_long_panel_entries(frame, panel_layout[0], panel, active, skin)
            }
        }
    }

    let selected_summary = if app.show_panel_totals() {
        panel_selection_summary(app.selection_size_state(panel_id))
    } else {
        String::new()
    };
    let disk_summary = panel_disk_summary(panel, app);
    let footer_style = if active {
        skin.style("core", "selected")
    } else {
        skin.style("core", "_default_")
    };
    let footer_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Fill(1), Constraint::Length(22)])
        .split(panel_layout[1]);
    frame.render_widget(
        Paragraph::new(selected_summary).style(footer_style),
        footer_layout[0],
    );
    frame.render_widget(
        Paragraph::new(disk_summary)
            .style(footer_style)
            .alignment(Alignment::Right),
        footer_layout[1],
    );
}

fn render_full_panel_entries(
    frame: &mut Frame,
    area: Rect,
    panel: &PanelState,
    active: bool,
    skin: &UiSkin,
) {
    let viewport_rows = area.height.saturating_sub(1).max(1) as usize;
    let (window_start, window_end) =
        visible_window(panel.entries.len(), panel.cursor, viewport_rows);
    let selected_row = panel
        .cursor
        .saturating_sub(window_start)
        .min(window_end.saturating_sub(window_start).saturating_sub(1));
    let rows: Vec<Row<'_>> = panel
        .entries
        .iter()
        .skip(window_start)
        .take(window_end.saturating_sub(window_start))
        .map(|entry| {
            Row::new(vec![
                Cell::from(panel_entry_display_label(panel, entry)),
                Cell::from(format!(
                    "{:>width$} ",
                    panel_entry_size_label(entry),
                    width = PANEL_SIZE_VALUE_WIDTH
                )),
                Cell::from(format_modified(entry.modified)),
            ])
            .style(panel_entry_style(panel, entry, false, skin))
        })
        .collect();
    let header = Row::new(vec![
        Cell::from("Name"),
        Cell::from("Size"),
        Cell::from("Modify time"),
    ])
    .style(skin.style("core", "header"));

    let table = Table::new(
        rows,
        [
            Constraint::Fill(1),
            Constraint::Length(PANEL_SIZE_COL_WIDTH as u16),
            Constraint::Length(12),
        ],
    )
    .header(header)
    .style(skin.style("core", "_default_"))
    .highlight_style(panel_selection_style(panel, active, skin))
    .column_spacing(1);

    let mut table_state = TableState::default();
    table_state.select(Some(selected_row));
    frame.render_stateful_widget(table, area, &mut table_state);
}

const BRIEF_MIN_COLUMN_WIDTH: u16 = 16;
const BRIEF_MAX_COLUMNS: usize = 9;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BriefGrid {
    entry_count: usize,
    columns: usize,
    rows: usize,
}

impl BriefGrid {
    fn new(entry_count: usize, width: u16) -> Self {
        let columns = usize::from((width / BRIEF_MIN_COLUMN_WIDTH).max(1)).min(BRIEF_MAX_COLUMNS);
        Self {
            entry_count,
            columns,
            rows: entry_count.div_ceil(columns),
        }
    }

    fn index_at(self, column: usize, row: usize) -> Option<usize> {
        if column >= self.columns || row >= self.rows {
            return None;
        }
        let index = column.checked_mul(self.rows)?.checked_add(row)?;
        (index < self.entry_count).then_some(index)
    }

    fn left_of(self, cursor: usize) -> Option<usize> {
        if self.rows == 0 {
            return None;
        }
        let row = cursor % self.rows;
        let column = (cursor / self.rows).checked_sub(1)?;
        self.index_at(column, row)
    }

    fn right_of(self, cursor: usize) -> Option<usize> {
        if self.rows == 0 {
            return None;
        }
        let row = cursor % self.rows;
        let column = (cursor / self.rows).checked_add(1)?;
        self.index_at(column, row)
    }
}

/// Resolves viewport-dependent Left/Right movement in a responsive Brief grid.
pub fn resolve_file_manager_navigation(
    state: &AppState,
    command: AppCommand,
    viewport_width: u16,
) -> AppCommand {
    let AppCommand::Navigate(
        NavigationTarget::FileManager,
        motion @ (NavigationMotion::Left | NavigationMotion::Right),
    ) = command
    else {
        return command;
    };
    let panel = state.active_panel;
    if state.panel_view_mode(panel) != PanelViewMode::Listing
        || state.panel_listing_format(panel) != PanelListingFormat::Brief
    {
        return command;
    }

    let viewport = Rect::new(0, 0, viewport_width, 1);
    let panel_width = if uses_single_panel_layout(state) {
        viewport_width
    } else {
        dual_panel_areas(viewport)[panel.index()].width
    };
    let listing_width = panel_width.saturating_sub(2);
    let panel_state = &state.panels[panel.index()];
    let grid = BriefGrid::new(panel_state.entries.len(), listing_width);
    let target = match motion {
        NavigationMotion::Left => grid.left_of(panel_state.cursor),
        NavigationMotion::Right => grid.right_of(panel_state.cursor),
        _ => unreachable!("only horizontal motions are accepted above"),
    };

    target.map_or(command, |index| {
        AppCommand::Panel(panel, PanelCommand::SelectAt(index))
    })
}

fn render_brief_panel_entries(
    frame: &mut Frame,
    area: Rect,
    panel: &PanelState,
    active: bool,
    skin: &UiSkin,
) {
    let grid = BriefGrid::new(panel.entries.len(), area.width);
    let columns = grid.columns;
    let total_rows = grid.rows;
    let selected_row = panel.cursor % total_rows.max(1);
    let viewport_rows = usize::from(area.height.max(1));
    let (window_start, window_end) = visible_window(total_rows, selected_row, viewport_rows);
    let cell_width = usize::from(area.width)
        .saturating_sub(columns.saturating_sub(1))
        .checked_div(columns)
        .unwrap_or(0);
    let rows = (window_start..window_end).map(|row_index| {
        let cells = (0..columns).map(|column| {
            let Some(entry_index) = grid.index_at(column, row_index) else {
                return Cell::from(String::new());
            };
            let Some(entry) = panel.entries.get(entry_index) else {
                return Cell::from(String::new());
            };
            Cell::from(fit_single_line(
                panel_entry_display_label(panel, entry),
                cell_width,
            ))
            .style(panel_entry_style(
                panel,
                entry,
                active && entry_index == panel.cursor,
                skin,
            ))
        });
        Row::new(cells)
    });
    let widths = vec![Constraint::Ratio(1, columns as u32); columns];
    frame.render_widget(
        Table::new(rows, widths)
            .style(skin.style("core", "_default_"))
            .column_spacing(1),
        area,
    );
}

fn render_long_panel_entries(
    frame: &mut Frame,
    area: Rect,
    panel: &PanelState,
    active: bool,
    skin: &UiSkin,
) {
    let viewport_rows = area.height.saturating_sub(1).max(1) as usize;
    let (window_start, window_end) =
        visible_window(panel.entries.len(), panel.cursor, viewport_rows);
    let selected_row = panel
        .cursor
        .saturating_sub(window_start)
        .min(window_end.saturating_sub(window_start).saturating_sub(1));
    let rows = panel
        .entries
        .iter()
        .skip(window_start)
        .take(window_end.saturating_sub(window_start))
        .map(|entry| {
            Row::new(vec![
                Cell::from(format_file_mode(entry)),
                Cell::from(optional_number(entry.metadata.hard_links)),
                Cell::from(optional_number(entry.metadata.user_id)),
                Cell::from(optional_number(entry.metadata.group_id)),
                Cell::from(panel_entry_size_label(entry)),
                Cell::from(format_modified(entry.modified)),
                Cell::from(panel_entry_display_label(panel, entry)),
            ])
            .style(panel_entry_style(panel, entry, false, skin))
        });
    let header = Row::new(vec![
        "Mode",
        "Links",
        "UID",
        "GID",
        "Size",
        "Modify time",
        "Name",
    ])
    .style(skin.style("core", "header"));
    let table = Table::new(
        rows,
        [
            Constraint::Length(10),
            Constraint::Length(5),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(PANEL_SIZE_COL_WIDTH as u16),
            Constraint::Length(12),
            Constraint::Fill(1),
        ],
    )
    .header(header)
    .style(skin.style("core", "_default_"))
    .highlight_style(panel_selection_style(panel, active, skin))
    .column_spacing(1);

    let mut table_state = TableState::default();
    table_state.select(Some(selected_row));
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn panel_entry_display_label(panel: &PanelState, entry: &FileEntry) -> String {
    let marker = if !entry.is_parent() && panel.is_tagged(&entry.path) {
        "*"
    } else {
        " "
    };
    let label = if entry.is_parent() {
        String::from("/..")
    } else if entry.is_dir() {
        format!("/{}/", entry.name)
    } else {
        entry.name.clone()
    };
    format!("{marker}{label}")
}

fn panel_entry_style(
    panel: &PanelState,
    entry: &FileEntry,
    selected: bool,
    skin: &UiSkin,
) -> Style {
    let tagged = !entry.is_parent() && panel.is_tagged(&entry.path);
    let mut style = if tagged {
        skin.style("core", "marked")
    } else {
        skin.style("core", "_default_")
    };
    if entry.is_dir() {
        style = style.patch(skin.style("filehighlight", "directory"));
    }
    if selected {
        style = style.patch(if tagged {
            skin.style("core", "markselect")
        } else {
            skin.style("core", "selected")
        });
    }
    style
}

fn panel_selection_style(panel: &PanelState, active: bool, skin: &UiSkin) -> Style {
    if !active {
        return skin.style("core", "_default_");
    }
    let selected_tagged = panel
        .selected_entry()
        .is_some_and(|entry| !entry.is_parent() && panel.is_tagged(&entry.path));
    if selected_tagged {
        skin.style("core", "markselect")
    } else {
        skin.style("core", "selected")
    }
}

fn optional_number(value: Option<impl ToString>) -> String {
    value.map_or_else(|| String::from("-"), |value| value.to_string())
}

fn format_file_mode(entry: &FileEntry) -> String {
    let Some(mode) = entry.metadata.mode else {
        return String::from("??????????");
    };
    let mut output = String::with_capacity(10);
    output.push(if entry.is_dir() { 'd' } else { '-' });
    output.push(if mode & 0o400 != 0 { 'r' } else { '-' });
    output.push(if mode & 0o200 != 0 { 'w' } else { '-' });
    output.push(special_execute_bit(mode, 0o100, 0o4000, 's', 'S'));
    output.push(if mode & 0o040 != 0 { 'r' } else { '-' });
    output.push(if mode & 0o020 != 0 { 'w' } else { '-' });
    output.push(special_execute_bit(mode, 0o010, 0o2000, 's', 'S'));
    output.push(if mode & 0o004 != 0 { 'r' } else { '-' });
    output.push(if mode & 0o002 != 0 { 'w' } else { '-' });
    output.push(special_execute_bit(mode, 0o001, 0o1000, 't', 'T'));
    output
}

fn special_execute_bit(
    mode: u32,
    execute_bit: u32,
    special_bit: u32,
    special_execute: char,
    special_no_execute: char,
) -> char {
    match (mode & execute_bit != 0, mode & special_bit != 0) {
        (true, true) => special_execute,
        (false, true) => special_no_execute,
        (true, false) => 'x',
        (false, false) => '-',
    }
}

fn render_info_panel(
    frame: &mut Frame,
    area: Rect,
    panel_id: ActivePanel,
    skin: &UiSkin,
    app: &AppState,
) {
    let source_id = panel_id.other();
    let source = &app.panels[source_id.index()];
    let title = format!("Info | {} panel selection", source_id.label());
    let block = Block::default()
        .title(fit_single_line(
            title,
            area.width.saturating_sub(2) as usize,
        ))
        .borders(Borders::ALL)
        .border_set(skin.panel_border_set())
        .border_style(skin.style("core", "_default_"))
        .style(skin.style("core", "_default_"));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut rows = vec![
        format!("Directory: {}", source.cwd.to_string_lossy()),
        format!("Sort: {}", source.sort_label()),
        format!("Filter: {}", source.filter().display_pattern()),
        format!("Tagged: {}", source.tagged_count()),
        String::new(),
    ];
    match source.selected_entry() {
        Some(entry) => {
            let entry_type = if entry.is_parent() {
                "parent directory"
            } else if entry.is_dir() {
                "directory"
            } else {
                "file"
            };
            rows.extend([
                format!("Name: {}", entry.name),
                format!("Type: {entry_type}"),
                format!("Path: {}", entry.path.to_string_lossy()),
                format!("Mode: {}", format_file_mode(entry)),
                format!("Links: {}", optional_number(entry.metadata.hard_links)),
                format!("UID: {}", optional_number(entry.metadata.user_id)),
                format!("GID: {}", optional_number(entry.metadata.group_id)),
                format!("Inode: {}", optional_number(entry.metadata.inode)),
                format!(
                    "Size: {} ({} bytes)",
                    format_human_size(entry.size),
                    entry.size
                ),
                format!("Modified: {}", format_modified(entry.modified)),
                format!("Accessed: {}", format_modified(entry.metadata.accessed)),
                format!("Changed: {}", format_modified(entry.metadata.changed)),
            ]);
        }
        None if source.loading => rows.push(String::from("Selection: loading...")),
        None => rows.push(String::from("Selection: <none>")),
    }
    rows.push(String::new());
    rows.push(format!("Filesystem: {}", panel_disk_summary(source, app)));

    let width = inner.width as usize;
    let lines: Vec<Line<'_>> = rows
        .into_iter()
        .map(|row| Line::raw(fit_single_line(row, width)))
        .collect();
    frame.render_widget(
        Paragraph::new(lines).style(skin.style("core", "_default_")),
        inner,
    );
}

fn render_quick_view_panel(
    frame: &mut Frame,
    area: Rect,
    panel_id: ActivePanel,
    skin: &UiSkin,
    app: &AppState,
) {
    let state = app.quick_view_state(panel_id);
    let title = match state.path() {
        Some(path) => format!("Quick view | {}", path.to_string_lossy()),
        None => format!("Quick view | {} panel selection", panel_id.other().label()),
    };
    let mut surface_style = skin.style("viewer", "_default_");
    if surface_style.fg.is_none() && surface_style.bg.is_none() {
        surface_style = viewer_theme_surface_style().unwrap_or_default();
    }
    let block = Block::default()
        .title(fit_single_line(
            title,
            area.width.saturating_sub(2) as usize,
        ))
        .borders(Borders::ALL)
        .border_set(skin.panel_border_set())
        .border_style(skin.style("core", "_default_"))
        .style(surface_style);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let placeholder = |message: String| {
        Paragraph::new(message)
            .style(surface_style)
            .wrap(Wrap { trim: false })
    };
    match state {
        QuickViewState::Empty => {
            frame.render_widget(placeholder(String::from("<no selection>")), inner);
        }
        QuickViewState::Directory { path } => {
            frame.render_widget(
                placeholder(format!(
                    "<directory>\n{}\n\nSelect a regular file to preview its contents.",
                    path.to_string_lossy()
                )),
                inner,
            );
        }
        QuickViewState::Loading { .. } => {
            frame.render_widget(placeholder(String::from("<loading preview...>")), inner);
        }
        QuickViewState::Failed { error, .. } => {
            frame.render_widget(
                placeholder(format!(
                    "<preview unavailable>\n{}",
                    sanitize_single_line(error)
                )),
                inner,
            );
        }
        QuickViewState::Ready(viewer) => {
            let visible_lines = inner.height.max(1) as usize;
            let content = viewer_window(viewer, visible_lines, inner.width as usize);
            let mut paragraph = Paragraph::new(content).style(surface_style);
            if viewer.wrap && !viewer.hex_mode {
                paragraph = paragraph.wrap(Wrap { trim: false });
            }
            frame.render_widget(paragraph, inner);
        }
    }
}

fn render_viewer(frame: &mut Frame, area: Rect, viewer: &ViewerState, skin: &UiSkin) {
    frame.render_widget(Clear, area);
    let visible_lines = area.height.saturating_sub(2).max(1) as usize;
    let content_width = area.width.saturating_sub(2) as usize;
    let title = fit_single_line(
        format!(
            "{} | {} {}/{} | wrap:{}",
            viewer.path().to_string_lossy(),
            if viewer.hex_mode { "row" } else { "line" },
            viewer.current_line_number(),
            viewer.line_count(),
            if viewer.wrap { "on" } else { "off" }
        ),
        area.width.saturating_sub(2) as usize,
    );
    let content = viewer_window(viewer, visible_lines, content_width);
    let mut surface_style = skin.style("viewer", "_default_");
    if surface_style.fg.is_none() && surface_style.bg.is_none() {
        surface_style = viewer_theme_surface_style().unwrap_or_default();
    }
    let mut paragraph = Paragraph::new(content)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_set(skin.panel_border_set())
                .border_style(skin.style("core", "selected"))
                .style(surface_style),
        )
        .style(surface_style);
    if viewer.wrap && !viewer.hex_mode {
        paragraph = paragraph.wrap(Wrap { trim: false });
    }
    frame.render_widget(paragraph, area);
}

fn viewer_window(viewer: &ViewerState, visible_lines: usize, width: usize) -> Text<'static> {
    if viewer.hex_mode {
        hex_viewer_window(viewer, visible_lines, width)
    } else {
        highlighted_viewer_window(viewer, visible_lines)
            .unwrap_or_else(|| plain_viewer_window(viewer, visible_lines, width))
    }
}

fn highlighted_viewer_window(viewer: &ViewerState, visible_lines: usize) -> Option<Text<'static>> {
    let resources = highlight_resources()?;
    let cache_lock = viewer_highlight_cache();
    let mut cache_guard = cache_lock.lock().ok()?;
    let key = viewer_highlight_key(viewer);

    if cache_guard.as_ref().is_none_or(|cached| cached.key != key) {
        *cache_guard = Some(CachedViewerHighlight::new(viewer, resources.as_ref())?);
    }
    let cache = cache_guard.as_mut()?;
    let total_lines = cache.raw_lines.len();
    if total_lines == 0 {
        return Some(Text::raw(String::new()));
    }

    let start = viewer.scroll.min(total_lines.saturating_sub(1));
    let end = start.saturating_add(visible_lines.max(1)).min(total_lines);
    cache
        .ensure_highlighted_up_to(end, resources.as_ref())
        .ok()?;

    Some(Text::from(cache.highlighted_lines[start..end].to_vec()))
}

fn highlight_resources() -> Option<Arc<HighlightResources>> {
    let skin = current_skin();
    let skin_name = skin.name().to_string();
    let cache = HIGHLIGHT_RESOURCES.get_or_init(|| Mutex::new(None));
    let mut cache_guard = cache.lock().ok()?;
    if cache_guard
        .as_ref()
        .is_none_or(|cached| cached.skin_name != skin_name)
    {
        let resources = Arc::new(build_highlight_resources_for_skin(skin.as_ref())?);
        *cache_guard = Some(CachedHighlightResources {
            skin_name,
            resources: Arc::clone(&resources),
        });
        if let Ok(mut viewer_cache) = viewer_highlight_cache().lock() {
            *viewer_cache = None;
        }
        return Some(resources);
    }
    cache_guard
        .as_ref()
        .map(|cached| Arc::clone(&cached.resources))
}

fn build_highlight_resources_for_skin(skin: &UiSkin) -> Option<HighlightResources> {
    let themes = syntect::highlighting::ThemeSet::load_defaults();
    let mut theme = themes
        .themes
        .get("base16-ocean.dark")
        .cloned()
        .or_else(|| themes.themes.values().next().cloned())?;

    let viewer_style = skin.style("viewer", "_default_");
    if let Some(foreground) = viewer_style.fg.and_then(syntect_color_from_ratatui) {
        theme.settings.foreground = Some(foreground);
    }
    if let Some(background) = viewer_style.bg.and_then(syntect_color_from_ratatui) {
        theme.settings.background = Some(background);
    }
    for scope in &mut theme.scopes {
        // Keep syntax foreground accents, but let the viewer surface own the background.
        scope.style.background = None;
    }

    Some(HighlightResources { theme })
}

fn highlight_syntax_set() -> &'static SyntaxSet {
    HIGHLIGHT_SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn viewer_syntax<'a>(syntax_set: &'a SyntaxSet, viewer: &ViewerState) -> &'a SyntaxReference {
    syntax_set
        .find_syntax_for_file(viewer.path())
        .ok()
        .flatten()
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text())
}

fn viewer_theme_surface_style() -> Option<Style> {
    let resources = highlight_resources()?;
    let mut style = Style::default();

    if let Some(background) = resources.theme.settings.background {
        style = style.bg(Color::Rgb(background.r, background.g, background.b));
    }
    if let Some(foreground) = resources.theme.settings.foreground {
        style = style.fg(Color::Rgb(foreground.r, foreground.g, foreground.b));
    }

    Some(style)
}

fn viewer_highlight_key(viewer: &ViewerState) -> ViewerHighlightKey {
    ViewerHighlightKey {
        content_hash: viewer.content_fingerprint(),
        content_len: viewer.content().len(),
        path_hash: viewer.path_fingerprint(),
    }
}

fn viewer_highlight_cache() -> &'static Mutex<Option<CachedViewerHighlight>> {
    VIEWER_HIGHLIGHT_CACHE.get_or_init(|| Mutex::new(None))
}

impl CachedViewerHighlight {
    fn new(viewer: &ViewerState, resources: &HighlightResources) -> Option<Self> {
        let syntax_set = highlight_syntax_set();
        let syntax = viewer_syntax(syntax_set, viewer);
        let mut raw_lines: Vec<String> = viewer.content().lines().map(sanitize_text_line).collect();
        if raw_lines.is_empty() {
            raw_lines.push(String::new());
        }

        let highlighter = Highlighter::new(&resources.theme);
        let highlight_state = HighlightState::new(&highlighter, ScopeStack::new());

        Some(Self {
            key: viewer_highlight_key(viewer),
            raw_lines,
            highlighted_lines: Vec::new(),
            parse_state: ParseState::new(syntax),
            highlight_state,
        })
    }

    fn ensure_highlighted_up_to(
        &mut self,
        end: usize,
        resources: &HighlightResources,
    ) -> Result<(), ()> {
        let syntax_set = highlight_syntax_set();
        let highlighter = Highlighter::new(&resources.theme);
        while self.highlighted_lines.len() < end {
            let index = self.highlighted_lines.len();
            let raw_line = self.raw_lines.get(index).ok_or(())?;
            let operations = self
                .parse_state
                .parse_line(raw_line.as_str(), syntax_set)
                .map_err(|_| ())?;
            let spans: Vec<Span<'static>> = HighlightIterator::new(
                &mut self.highlight_state,
                &operations[..],
                raw_line.as_str(),
                &highlighter,
            )
            .map(|(style, text)| Span::styled(text.to_string(), syntect_style(style)))
            .collect();
            self.highlighted_lines.push(Line::from(spans));
        }
        Ok(())
    }
}

fn plain_viewer_window(viewer: &ViewerState, visible_lines: usize, width: usize) -> Text<'static> {
    let mut raw_lines: Vec<&str> = viewer.content().lines().collect();
    if raw_lines.is_empty() {
        raw_lines.push("");
    }
    let start = viewer.scroll.min(raw_lines.len().saturating_sub(1));
    let end = start
        .saturating_add(visible_lines.max(1))
        .min(raw_lines.len());

    let lines: Vec<Line<'static>> = raw_lines[start..end]
        .iter()
        .map(|line| pad_line_to_width(sanitize_text_line(line), width))
        .collect();
    Text::from(lines)
}

fn hex_viewer_window(viewer: &ViewerState, visible_lines: usize, width: usize) -> Text<'static> {
    let total_rows = ((viewer.bytes.len().saturating_add(15)).saturating_div(16)).max(1);
    let start = viewer.scroll.min(total_rows.saturating_sub(1));
    let end = start.saturating_add(visible_lines.max(1)).min(total_rows);
    let mut lines = Vec::with_capacity(end.saturating_sub(start));

    for row in start..end {
        let offset = row.saturating_mul(16);
        let chunk_end = offset.saturating_add(16).min(viewer.bytes.len());
        let chunk = &viewer.bytes[offset..chunk_end];

        let mut hex = String::new();
        let mut ascii = String::new();
        for index in 0..16 {
            if index < chunk.len() {
                let byte = chunk[index];
                if !hex.is_empty() {
                    hex.push(' ');
                }
                hex.push_str(&format!("{byte:02x}"));
                let ch = byte as char;
                if ch.is_ascii_graphic() || ch == ' ' {
                    ascii.push(ch);
                } else {
                    ascii.push('.');
                }
            } else {
                if !hex.is_empty() {
                    hex.push(' ');
                }
                hex.push_str("  ");
                ascii.push(' ');
            }
        }

        lines.push(pad_line_to_width(
            format!("{offset:08x}  {hex}  |{ascii}|"),
            width,
        ));
    }

    Text::from(lines)
}

fn sanitize_text_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for ch in line.chars() {
        if ch == '\t' {
            out.push_str("    ");
        } else if ch.is_control() {
            out.push('.');
        } else {
            out.push(ch);
        }
    }
    out
}

fn pad_line_to_width(mut line: String, width: usize) -> Line<'static> {
    let len = line.chars().count();
    if len < width {
        line.push_str(&" ".repeat(width - len));
    }
    Line::from(line)
}

fn syntect_style(style: SyntectStyle) -> Style {
    let mut ratatui_style = Style::default().fg(Color::Rgb(
        style.foreground.r,
        style.foreground.g,
        style.foreground.b,
    ));

    if style.font_style.contains(FontStyle::BOLD) {
        ratatui_style = ratatui_style.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        ratatui_style = ratatui_style.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        ratatui_style = ratatui_style.add_modifier(Modifier::UNDERLINED);
    }

    ratatui_style
}

fn syntect_color_from_ratatui(color: Color) -> Option<SyntectColor> {
    let (r, g, b) = match color {
        Color::Reset => return None,
        Color::Black => (0, 0, 0),
        Color::Red => (128, 0, 0),
        Color::Green => (0, 128, 0),
        Color::Yellow => (128, 128, 0),
        Color::Blue => (0, 0, 128),
        Color::Magenta => (128, 0, 128),
        Color::Cyan => (0, 128, 128),
        Color::Gray => (192, 192, 192),
        Color::DarkGray => (128, 128, 128),
        Color::LightRed => (255, 0, 0),
        Color::LightGreen => (0, 255, 0),
        Color::LightYellow => (255, 255, 0),
        Color::LightBlue => (0, 0, 255),
        Color::LightMagenta => (255, 0, 255),
        Color::LightCyan => (0, 255, 255),
        Color::White => (255, 255, 255),
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(index) => indexed_color_rgb(index),
    };

    Some(SyntectColor { r, g, b, a: 0xFF })
}

fn indexed_color_rgb(index: u8) -> (u8, u8, u8) {
    const ANSI_16: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (128, 0, 0),
        (0, 128, 0),
        (128, 128, 0),
        (0, 0, 128),
        (128, 0, 128),
        (0, 128, 128),
        (192, 192, 192),
        (128, 128, 128),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (0, 0, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];

    match index {
        0..=15 => ANSI_16[index as usize],
        16..=231 => {
            let level = [0, 95, 135, 175, 215, 255];
            let offset = index - 16;
            let red = level[(offset / 36) as usize];
            let green = level[((offset % 36) / 6) as usize];
            let blue = level[(offset % 6) as usize];
            (red, green, blue)
        }
        232..=255 => {
            let gray = 8u8.saturating_add((index - 232).saturating_mul(10));
            (gray, gray, gray)
        }
    }
}

fn render_dialog(frame: &mut Frame, dialog: &DialogState, skin: &UiSkin) {
    let (width, height) = if matches!(&dialog.kind, DialogKind::Find(_) | DialogKind::QuickCd(_)) {
        (FIND_DIALOG_WIDTH, FIND_DIALOG_HEIGHT)
    } else {
        (STANDARD_DIALOG_WIDTH, STANDARD_DIALOG_HEIGHT)
    };
    let area = centered_rect(frame.area(), width, height);
    frame.render_widget(Clear, area);

    match &dialog.kind {
        DialogKind::Confirm(confirm) => {
            let block = Block::default()
                .title(dialog.title.as_str())
                .borders(Borders::ALL)
                .border_set(skin.dialog_border_set())
                .border_style(skin.style("dialog", "_default_"))
                .style(skin.style("dialog", "_default_"));
            let inner = block.inner(area);
            frame.render_widget(block, area);

            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(2),
                    Constraint::Length(2),
                    Constraint::Length(1),
                ])
                .split(inner);

            frame.render_widget(
                Paragraph::new(confirm.message.as_str())
                    .style(skin.style("dialog", "_default_"))
                    .alignment(Alignment::Center),
                layout[0],
            );

            let ok_style = if confirm.focus == DialogButtonFocus::Ok {
                skin.style("dialog", "dfocus")
            } else {
                skin.style("dialog", "_default_")
            };
            let cancel_style = if confirm.focus == DialogButtonFocus::Cancel {
                skin.style("dialog", "dfocus")
            } else {
                skin.style("dialog", "_default_")
            };

            let buttons = Line::from(vec![
                Span::styled(" [ OK ] ", ok_style),
                Span::raw("  "),
                Span::styled(" [ Cancel ] ", cancel_style),
            ]);
            frame.render_widget(
                Paragraph::new(buttons).alignment(Alignment::Center),
                layout[1],
            );

            frame.render_widget(
                Paragraph::new("Enter accept | Tab switch | Esc cancel")
                    .style(skin.style("core", "disabled"))
                    .alignment(Alignment::Center),
                layout[2],
            );
        }
        DialogKind::Input(input) => {
            let block = Block::default()
                .title(dialog.title.as_str())
                .borders(Borders::ALL)
                .border_set(skin.dialog_border_set())
                .border_style(skin.style("dialog", "_default_"))
                .style(skin.style("dialog", "_default_"));
            let inner = block.inner(area);
            frame.render_widget(block, area);

            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Length(3),
                    Constraint::Length(1),
                ])
                .split(inner);

            frame.render_widget(
                Paragraph::new(input.prompt.as_str()).style(skin.style("dialog", "_default_")),
                layout[0],
            );

            frame.render_widget(
                Paragraph::new(input.value.as_str()).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_set(skin.panel_border_set())
                        .border_style(skin.style("dialog", "dfocus"))
                        .style(skin.style("core", "input")),
                ),
                layout[1],
            );

            frame.render_widget(
                Paragraph::new("Type text | Enter accept | Backspace delete | Esc cancel")
                    .style(skin.style("core", "disabled")),
                layout[2],
            );
        }
        DialogKind::QuickCd(quick_cd) => {
            let block = Block::default()
                .title(dialog.title.as_str())
                .borders(Borders::ALL)
                .border_set(skin.dialog_border_set())
                .border_style(skin.style("dialog", "_default_"))
                .style(skin.style("dialog", "_default_"));
            let inner = block.inner(area);
            frame.render_widget(block, area);

            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Length(3),
                    Constraint::Min(3),
                    Constraint::Length(2),
                ])
                .split(inner);
            frame.render_widget(
                Paragraph::new("Path or substring:").style(skin.style("dialog", "_default_")),
                layout[0],
            );
            frame.render_widget(
                Paragraph::new(quick_cd.value.as_str()).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_set(skin.panel_border_set())
                        .border_style(skin.style("dialog", "dfocus"))
                        .style(skin.style("core", "input")),
                ),
                layout[1],
            );

            let list_area = layout[2];
            let item_width = list_area.width.saturating_sub(3) as usize;
            let viewport_rows = list_area.height.max(1) as usize;
            let (window_start, window_end) =
                visible_window(quick_cd.suggestions.len(), quick_cd.selected, viewport_rows);
            let items: Vec<ListItem<'_>> = if quick_cd.suggestions.is_empty() {
                vec![ListItem::new(match &quick_cd.search_status {
                    QuickCdSearchStatus::Idle => {
                        "Type to search the current directory, home, and filesystem root"
                    }
                    QuickCdSearchStatus::Searching { .. } => "<searching for directories...>",
                    QuickCdSearchStatus::Complete { .. } => "<no matching directories>",
                    QuickCdSearchStatus::Failed(_) => "<directory search unavailable>",
                })]
            } else {
                quick_cd
                    .suggestions
                    .iter()
                    .skip(window_start)
                    .take(window_end.saturating_sub(window_start))
                    .map(|suggestion| {
                        ListItem::new(fit_single_line(&suggestion.display, item_width))
                    })
                    .collect()
            };
            let list = List::new(items)
                .style(skin.style("dialog", "_default_"))
                .highlight_style(skin.style("dialog", "dfocus"))
                .highlight_symbol(">> ");
            let mut state = ListState::default();
            if !quick_cd.suggestions.is_empty() {
                state.select(Some(
                    quick_cd
                        .selected
                        .saturating_sub(window_start)
                        .min(window_end.saturating_sub(window_start).saturating_sub(1)),
                ));
            }
            frame.render_stateful_widget(list, list_area, &mut state);

            let search_summary = match &quick_cd.search_status {
                QuickCdSearchStatus::Idle => String::from("Enter path or search text"),
                QuickCdSearchStatus::Searching {
                    visited_directories,
                    skipped_directories,
                } => {
                    format!("Searching: {visited_directories} dirs, {skipped_directories} skipped")
                }
                QuickCdSearchStatus::Complete {
                    visited_directories,
                    skipped_directories,
                    truncated,
                } => format!(
                    "{} match(es) · {visited_directories} dirs, {skipped_directories} skipped{}",
                    quick_cd.suggestions.len(),
                    if *truncated { " · bounded" } else { "" }
                ),
                QuickCdSearchStatus::Failed(error) => format!("Search failed: {error}"),
            };
            frame.render_widget(
                Paragraph::new(format!(
                    "{search_summary}\nUp/Down choose | Enter open | Esc cancel"
                ))
                .style(skin.style("core", "disabled"))
                .wrap(Wrap { trim: false }),
                layout[3],
            );
        }
        DialogKind::PairInput(input) => {
            let block = Block::default()
                .title(dialog.title.as_str())
                .borders(Borders::ALL)
                .border_set(skin.dialog_border_set())
                .border_style(skin.style("dialog", "_default_"))
                .style(skin.style("dialog", "_default_"));
            let inner = block.inner(area);
            frame.render_widget(block, area);

            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Length(3),
                    Constraint::Length(1),
                    Constraint::Length(3),
                    Constraint::Length(1),
                ])
                .split(inner);
            let field_block = |focused| {
                Block::default()
                    .borders(Borders::ALL)
                    .border_set(skin.panel_border_set())
                    .border_style(if focused {
                        skin.style("dialog", "dfocus")
                    } else {
                        skin.style("dialog", "_default_")
                    })
                    .style(skin.style("core", "input"))
            };

            frame.render_widget(
                Paragraph::new(input.first_prompt.as_str())
                    .style(skin.style("dialog", "_default_")),
                layout[0],
            );
            frame.render_widget(
                Paragraph::new(input.first_value.as_str())
                    .block(field_block(input.focus == PairInputField::First)),
                layout[1],
            );
            frame.render_widget(
                Paragraph::new(input.second_prompt.as_str())
                    .style(skin.style("dialog", "_default_")),
                layout[2],
            );
            frame.render_widget(
                Paragraph::new(input.second_value.as_str())
                    .block(field_block(input.focus == PairInputField::Second)),
                layout[3],
            );
            frame.render_widget(
                Paragraph::new("Type text | Tab next field | Enter accept | Esc cancel")
                    .style(skin.style("core", "disabled")),
                layout[4],
            );
        }
        DialogKind::Listbox(listbox) => {
            let block = Block::default()
                .title(dialog.title.as_str())
                .borders(Borders::ALL)
                .border_set(skin.dialog_border_set())
                .border_style(skin.style("dialog", "_default_"))
                .style(skin.style("dialog", "_default_"));
            frame.render_widget(block, area);

            let footer_height = if listbox.footer_hint.is_some() { 2 } else { 1 };
            let overlay = listbox_dialog_layout(frame_area(frame), footer_height);
            let list_area = terminal_rect(overlay.list);
            let footer_area = terminal_rect(overlay.footer);

            let items: Vec<ListItem<'_>> = if listbox.items.is_empty() {
                vec![ListItem::new("<empty>")]
            } else {
                let viewport_rows = list_area.height.max(1) as usize;
                let (window_start, window_end) =
                    visible_window(listbox.items.len(), listbox.selected, viewport_rows);
                let item_width = list_area.width.saturating_sub(3) as usize;
                listbox
                    .items
                    .iter()
                    .skip(window_start)
                    .take(window_end.saturating_sub(window_start))
                    .map(|item| ListItem::new(fit_single_line(item, item_width)))
                    .collect()
            };
            let list = List::new(items)
                .style(skin.style("dialog", "_default_"))
                .highlight_style(skin.style("dialog", "dfocus"))
                .highlight_symbol(">> ");

            let mut state = ListState::default();
            if !listbox.items.is_empty() {
                let viewport_rows = list_area.height.max(1) as usize;
                let (window_start, window_end) =
                    visible_window(listbox.items.len(), listbox.selected, viewport_rows);
                let selected_row = listbox
                    .selected
                    .saturating_sub(window_start)
                    .min(window_end.saturating_sub(window_start).saturating_sub(1));
                state.select(Some(selected_row));
            }
            frame.render_stateful_widget(list, list_area, &mut state);

            frame.render_widget(
                Paragraph::new(
                    listbox
                        .footer_hint
                        .as_deref()
                        .unwrap_or("Up/Down move | Enter accept | Esc cancel"),
                )
                .style(skin.style("core", "disabled"))
                .wrap(Wrap { trim: false }),
                footer_area,
            );
        }
        DialogKind::Find(find) => {
            let block = Block::default()
                .title(dialog.title.as_str())
                .borders(Borders::ALL)
                .border_set(skin.dialog_border_set())
                .border_style(skin.style("dialog", "_default_"))
                .style(skin.style("dialog", "_default_"));
            let inner = block.inner(area);
            frame.render_widget(block, area);

            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(7), Constraint::Length(2)])
                .split(inner);
            let normal = skin.style("dialog", "_default_");
            let focused = skin.style("dialog", "dfocus");
            let input = skin.style("core", "input");
            let row = |field: FindDialogField, label: &str, value: String| {
                let is_focused = find.focus == field;
                Line::from(vec![
                    Span::styled(if is_focused { "> " } else { "  " }, focused),
                    Span::styled(
                        format!("{label:<21}"),
                        if is_focused { focused } else { normal },
                    ),
                    Span::styled(value, if is_focused { focused } else { input }),
                ])
            };
            let rows = vec![
                row(
                    FindDialogField::StartDirectory,
                    "Starting directory",
                    find.start_directory.clone(),
                ),
                row(
                    FindDialogField::FilenamePattern,
                    "Filename pattern",
                    if find.filename_pattern.is_empty() {
                        String::from("<all files>")
                    } else {
                        find.filename_pattern.clone()
                    },
                ),
                row(
                    FindDialogField::NameMode,
                    "Pattern mode",
                    find.name_mode.label().to_string(),
                ),
                row(
                    FindDialogField::CaseSensitive,
                    "Case sensitive",
                    checkbox_label(find.case_sensitive),
                ),
                row(
                    FindDialogField::ContentPattern,
                    "Containing text",
                    if find.content_pattern.is_empty() {
                        String::from("<disabled>")
                    } else {
                        find.content_pattern.clone()
                    },
                ),
                row(
                    FindDialogField::WholeWord,
                    "Whole words",
                    checkbox_label(find.whole_word),
                ),
                row(
                    FindDialogField::IgnoredDirectories,
                    "Ignore dirs (comma)",
                    if find.ignored_directories.is_empty() {
                        String::from("<none>")
                    } else {
                        find.ignored_directories.clone()
                    },
                ),
            ];
            frame.render_widget(Paragraph::new(rows).style(normal), layout[0]);
            frame.render_widget(
                Paragraph::new(
                    "Tab/Up/Down field | Space toggle | F2 tree picker\nEnter search | Esc cancel | Empty filename matches all",
                )
                .style(skin.style("core", "disabled")),
                layout[1],
            );
        }
        DialogKind::Filter(filter) => {
            let block = Block::default()
                .title(dialog.title.as_str())
                .borders(Borders::ALL)
                .border_set(skin.dialog_border_set())
                .border_style(skin.style("dialog", "_default_"))
                .style(skin.style("dialog", "_default_"));
            let inner = block.inner(area);
            frame.render_widget(block, area);

            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(4), Constraint::Length(2)])
                .split(inner);
            let normal = skin.style("dialog", "_default_");
            let focused = skin.style("dialog", "dfocus");
            let input = skin.style("core", "input");
            let row = |field: FilterDialogField, label: &str, value: String| {
                let is_focused = filter.focus == field;
                Line::from(vec![
                    Span::styled(if is_focused { "> " } else { "  " }, focused),
                    Span::styled(
                        format!("{label:<17}"),
                        if is_focused { focused } else { normal },
                    ),
                    Span::styled(value, if is_focused { focused } else { input }),
                ])
            };
            let pattern = if filter.pattern.is_empty() {
                String::from("<disabled>")
            } else {
                filter.pattern.clone()
            };
            let mode = match filter.name_mode {
                FindNameMode::Glob => "shell pattern",
                FindNameMode::Regex => "regular expression",
            };
            let rows = vec![
                row(FilterDialogField::Pattern, "Pattern", pattern),
                row(
                    FilterDialogField::FilesOnly,
                    "Files only",
                    checkbox_label(filter.files_only),
                ),
                row(
                    FilterDialogField::NameMode,
                    "Pattern mode",
                    mode.to_string(),
                ),
                row(
                    FilterDialogField::CaseSensitive,
                    "Case sensitive",
                    checkbox_label(filter.case_sensitive),
                ),
            ];
            frame.render_widget(Paragraph::new(rows).style(normal), layout[0]);
            frame.render_widget(
                Paragraph::new(
                    "Tab/Up/Down field | Space toggle\nEnter apply | Esc cancel | Empty pattern disables",
                )
                .style(skin.style("core", "disabled")),
                layout[1],
            );
        }
    }
}

fn checkbox_label(checked: bool) -> String {
    if checked {
        String::from("[x]")
    } else {
        String::from("[ ]")
    }
}

fn render_jobs_screen(frame: &mut Frame, state: &AppState, skin: &UiSkin) {
    let (width, height) = state.jobs_dialog_size();
    let area = centered_rect(frame.area(), width, height);
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title("Jobs")
        .borders(Borders::ALL)
        .border_set(skin.dialog_border_set())
        .border_style(skin.style("dialog", "_default_"))
        .style(skin.style("dialog", "_default_"));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header = Row::new(vec![
        Cell::from("id"),
        Cell::from("kind"),
        Cell::from("status"),
        Cell::from("progress"),
        Cell::from("current"),
        Cell::from("error"),
    ])
    .style(skin.style("core", "header"));

    let rows: Vec<Row<'_>> = if state.jobs.jobs().is_empty() {
        vec![Row::new(vec![
            Cell::from("-"),
            Cell::from("-"),
            Cell::from("empty"),
            Cell::from("-"),
            Cell::from("-"),
            Cell::from("-"),
        ])]
    } else {
        state.jobs.jobs().iter().map(job_row).collect()
    };

    let cancel = state
        .keybinding_joined_label(KeyContext::Jobs, AppCommand::CancelJob, " / ", 1)
        .unwrap_or_else(|| String::from("Alt-J"));
    let close = state
        .keybinding_joined_label(KeyContext::Jobs, AppCommand::CloseJobsScreen, " / ", 2)
        .unwrap_or_else(|| String::from("Esc/q"));
    let table = Table::new(
        rows,
        [
            Constraint::Length(5),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(20),
            Constraint::Length(20),
            Constraint::Min(10),
        ],
    )
    .header(header)
    .style(skin.style("dialog", "_default_"))
    .highlight_style(skin.style("core", "selected"))
    .highlight_symbol(">> ")
    .block(
        Block::default()
            .borders(Borders::NONE)
            .title(format!("Up/Down select | {cancel} cancel | {close} close")),
    );

    let mut table_state = TableState::default();
    if !state.jobs.jobs().is_empty() {
        table_state.select(Some(state.jobs_cursor));
    }
    frame.render_stateful_widget(table, inner, &mut table_state);
}

fn render_find_results_screen(
    frame: &mut Frame,
    app: &AppState,
    results: &FindResultsState,
    skin: &UiSkin,
) {
    let screen_layout = find_results_layout(frame_area(frame));
    let area = terminal_rect(screen_layout.outer);
    frame.render_widget(Clear, area);

    let mut title_flags = vec![results.status.label()];
    if results
        .report
        .as_ref()
        .is_some_and(|report| report.truncated)
    {
        title_flags.push("limit reached");
    }
    if results
        .report
        .as_ref()
        .is_some_and(|report| report.issue_count > 0)
    {
        title_flags.push("read errors");
    }
    let title = format!(
        "Find results: '{}' ({}) | {}",
        results.spec.display_pattern(),
        results.entries.len(),
        title_flags.join(", ")
    );
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_set(skin.dialog_border_set())
        .border_style(skin.style("dialog", "_default_"))
        .style(skin.style("dialog", "_default_"));
    frame.render_widget(block, area);

    let summary_area = terminal_rect(screen_layout.header);
    let list_area = terminal_rect(screen_layout.list);
    let footer_area = terminal_rect(screen_layout.footer);

    let mut summary = vec![Line::from(format!(
        "Root: {} | {} | {}",
        results.spec.start_dir.to_string_lossy(),
        results.spec.name_mode.label(),
        if results.spec.case_sensitive {
            "case-sensitive"
        } else {
            "case-insensitive"
        }
    ))];
    let detail = match (&results.status, results.report.as_ref()) {
        (FindResultsStatus::Failed(message), _) => format!("Error: {message}"),
        (_, Some(report)) if report.issue_count > 0 => {
            let first = report.issues.first().map_or_else(
                || String::from("details unavailable"),
                |issue| {
                    format!(
                        "{}: {} ({})",
                        issue.kind.label(),
                        issue.message,
                        issue.path.to_string_lossy()
                    )
                },
            );
            format!("Skipped {} item(s); {first}", report.issue_count)
        }
        (_, Some(report)) if report.truncated => {
            format!(
                "Stopped at the configured {}-result limit",
                report.matched_entries
            )
        }
        _ => String::new(),
    };
    summary.push(Line::from(detail));
    frame.render_widget(
        Paragraph::new(summary).style(skin.style("dialog", "_default_")),
        summary_area,
    );

    let items: Vec<ListItem<'_>> = if results.entries.is_empty() {
        if results.is_active() {
            vec![ListItem::new("<searching...>")]
        } else {
            vec![ListItem::new("<no matches>")]
        }
    } else {
        let viewport_rows = list_area.height.max(1) as usize;
        let (window_start, window_end) =
            visible_window(results.entries.len(), results.cursor, viewport_rows);
        results
            .entries
            .iter()
            .skip(window_start)
            .take(window_end.saturating_sub(window_start))
            .map(|entry| {
                let mut label = entry
                    .path
                    .strip_prefix(&results.spec.start_dir)
                    .unwrap_or(&entry.path)
                    .to_string_lossy()
                    .into_owned();
                if entry.is_dir && !label.ends_with('/') {
                    label.push('/');
                }
                ListItem::new(label)
            })
            .collect()
    };
    let list = List::new(items)
        .style(skin.style("dialog", "_default_"))
        .highlight_style(skin.style("core", "selected"))
        .highlight_symbol(">> ");
    let mut state = ListState::default();
    if !results.entries.is_empty() {
        let viewport_rows = list_area.height.max(1) as usize;
        let (window_start, window_end) =
            visible_window(results.entries.len(), results.cursor, viewport_rows);
        let selected_row = results
            .cursor
            .saturating_sub(window_start)
            .min(window_end.saturating_sub(window_start).saturating_sub(1));
        state.select(Some(selected_row));
    }
    frame.render_stateful_widget(list, list_area, &mut state);

    let open = app
        .keybinding_primary_label(KeyContext::FindResults, AppCommand::FindResultsOpenEntry)
        .unwrap_or("Enter")
        .to_string();
    let panelize = app
        .keybinding_primary_label(KeyContext::FindResults, AppCommand::FindResultsPanelize)
        .unwrap_or("F5")
        .to_string();
    let again = app
        .keybinding_primary_label(KeyContext::FindResults, AppCommand::FindResultsAgain)
        .unwrap_or("F4")
        .to_string();
    let pause = app
        .keybinding_primary_label(KeyContext::FindResults, AppCommand::FindResultsTogglePause)
        .unwrap_or("F6")
        .to_string();
    let cancel = app
        .keybinding_joined_label(KeyContext::FindResults, AppCommand::CancelJob, " / ", 1)
        .unwrap_or_else(|| String::from("Alt-J"));
    let close = app
        .keybinding_joined_label(
            KeyContext::FindResults,
            AppCommand::CloseFindResults,
            " / ",
            2,
        )
        .unwrap_or_else(|| String::from("Esc/q"));
    frame.render_widget(
        Paragraph::new(format!(
            "{open} locate | {again} again | {panelize} panelize | {pause} pause/continue\nUp/Down/PgUp/PgDn move | {cancel} cancel exact search | {close} close"
        ))
        .style(skin.style("core", "disabled")),
        footer_area,
    );
}

fn render_tree_screen(frame: &mut Frame, app: &AppState, tree: &TreeState, skin: &UiSkin) {
    let screen_layout = tree_layout(frame_area(frame));
    let area = terminal_rect(screen_layout.outer);
    frame.render_widget(Clear, area);

    let state_label = match tree.load_state() {
        TreeLoadState::Loading => String::from(" | loading..."),
        TreeLoadState::Ready(summary) => {
            let mut labels = Vec::new();
            if summary.depth_limit_reached {
                labels.push("depth limit");
            }
            if summary.entry_limit_reached {
                labels.push("entry limit");
            }
            if summary.skipped_items > 0 {
                labels.push("read errors");
            }
            if labels.is_empty() {
                String::new()
            } else {
                format!(" | {}", labels.join(", "))
            }
        }
        TreeLoadState::Canceled => String::from(" | canceled"),
        TreeLoadState::Failed(_) => String::from(" | failed"),
    };
    let title = format!(
        "Directory tree ({}){state_label} | {}",
        tree.entries().len(),
        tree.navigation_mode().label()
    );
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_set(skin.dialog_border_set())
        .border_style(skin.style("dialog", "_default_"))
        .style(skin.style("dialog", "_default_"));
    frame.render_widget(block, area);

    let root_area = terminal_rect(screen_layout.header);
    let list_area = terminal_rect(screen_layout.list);
    let footer_area = terminal_rect(screen_layout.footer);

    let mut root = format!("Root: {}", tree.root().to_string_lossy());
    match tree.load_state() {
        TreeLoadState::Ready(summary) if summary.skipped_items > 0 => {
            root.push_str(&format!(
                " | skipped {} unreadable item(s)",
                summary.skipped_items
            ));
        }
        TreeLoadState::Failed(message) => root.push_str(&format!(" | Error: {message}")),
        _ => {}
    }
    let root = fit_single_line(root, root_area.width as usize);
    frame.render_widget(
        Paragraph::new(root).style(skin.style("dialog", "_default_")),
        root_area,
    );

    let visible_count = tree.visible_entry_count();
    let visible_cursor = tree.visible_cursor();
    let items: Vec<ListItem<'_>> = if visible_count == 0 {
        vec![ListItem::new("<empty tree>")]
    } else {
        let viewport_rows = list_area.height.max(1) as usize;
        let (window_start, window_end) =
            visible_window(visible_count, visible_cursor, viewport_rows);
        tree.visible_entries()
            .skip(window_start)
            .take(window_end.saturating_sub(window_start))
            .map(|entry| {
                let name = if entry.depth == 0 {
                    entry.path.to_string_lossy().into_owned()
                } else {
                    let leaf = path_leaf_label(&entry.path);
                    format!("{}{leaf}/", "  ".repeat(entry.depth))
                };
                ListItem::new(name)
            })
            .collect()
    };
    let list = List::new(items)
        .style(skin.style("dialog", "_default_"))
        .highlight_style(skin.style("core", "selected"))
        .highlight_symbol(">> ");
    let mut state = ListState::default();
    if visible_count > 0 {
        let viewport_rows = list_area.height.max(1) as usize;
        let (window_start, window_end) =
            visible_window(visible_count, visible_cursor, viewport_rows);
        let selected_row = visible_cursor
            .saturating_sub(window_start)
            .min(window_end.saturating_sub(window_start).saturating_sub(1));
        state.select(Some(selected_row));
    }
    frame.render_stateful_widget(list, list_area, &mut state);

    let open = app
        .keybinding_primary_label(KeyContext::Tree, AppCommand::TreeOpenEntry)
        .unwrap_or("Enter")
        .to_string();
    let close = app
        .keybinding_joined_label(KeyContext::Tree, AppCommand::CloseTree, " / ", 2)
        .unwrap_or_else(|| String::from("Esc/q"));
    let rescan = app
        .keybinding_primary_label(KeyContext::Tree, AppCommand::TreeRescan)
        .unwrap_or("F2");
    let forget = app
        .keybinding_primary_label(KeyContext::Tree, AppCommand::TreeForget)
        .unwrap_or("F3");
    let mode = app
        .keybinding_primary_label(KeyContext::Tree, AppCommand::TreeToggleNavigation)
        .unwrap_or("F4");
    let copy = app
        .keybinding_primary_label(KeyContext::Tree, AppCommand::TreeCopy)
        .unwrap_or("F5");
    let move_directory = app
        .keybinding_primary_label(KeyContext::Tree, AppCommand::TreeMove)
        .unwrap_or("F6");
    let mkdir = app
        .keybinding_primary_label(KeyContext::Tree, AppCommand::TreeMkdir)
        .unwrap_or("F7");
    let delete = app
        .keybinding_primary_label(KeyContext::Tree, AppCommand::TreeDelete)
        .unwrap_or("F8");
    let search_next = app
        .keybinding_primary_label(KeyContext::Tree, AppCommand::TreeSearchNext)
        .unwrap_or("C-s");
    let search = if tree.search_query().is_empty() {
        String::from("Search: <empty>")
    } else {
        format!("Search: {}", tree.search_query())
    };
    let footer_width = footer_area.width as usize;
    let action_hints = fit_single_line(
        format!(
            "{rescan} scan | {forget} forget | {mode} mode | {copy} copy | {move_directory} move | {mkdir} mkdir | {delete} delete"
        ),
        footer_width,
    );
    let navigation_hints = fit_single_line(
        format!(
            "{search} | arrows navigate | {search_next} next | Backspace edit | {open} open | {close} close"
        ),
        footer_width,
    );
    frame.render_widget(
        Paragraph::new(vec![Line::from(action_hints), Line::from(navigation_hints)])
            .style(skin.style("core", "disabled")),
        footer_area,
    );
}

fn render_hotlist_screen(frame: &mut Frame, app: &AppState, skin: &UiSkin) {
    let screen_layout = hotlist_layout(frame_area(frame));
    let area = terminal_rect(screen_layout.outer);
    frame.render_widget(Clear, area);

    let hotlist = app.hotlist();
    let title = format!("Directory hotlist ({})", hotlist.len());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_set(skin.dialog_border_set())
        .border_style(skin.style("dialog", "_default_"))
        .style(skin.style("dialog", "_default_"));
    frame.render_widget(block, area);

    let list_area = terminal_rect(screen_layout.list);
    let footer_area = terminal_rect(screen_layout.footer);

    let items: Vec<ListItem<'_>> = if hotlist.is_empty() {
        vec![ListItem::new("<empty hotlist>")]
    } else {
        let viewport_rows = list_area.height.max(1) as usize;
        let viewport_width = list_area.width.saturating_sub(3) as usize;
        let (window_start, window_end) =
            visible_window(hotlist.len(), app.hotlist_cursor, viewport_rows);
        hotlist
            .iter()
            .skip(window_start)
            .take(window_end.saturating_sub(window_start))
            .map(|entry| {
                ListItem::new(fit_single_line(
                    format!("{}  —  {}", entry.label, entry.path.to_string_lossy()),
                    viewport_width,
                ))
            })
            .collect()
    };
    let list = List::new(items)
        .style(skin.style("dialog", "_default_"))
        .highlight_style(skin.style("core", "selected"))
        .highlight_symbol(">> ");

    let mut state = ListState::default();
    if !hotlist.is_empty() {
        let viewport_rows = list_area.height.max(1) as usize;
        let (window_start, window_end) =
            visible_window(hotlist.len(), app.hotlist_cursor, viewport_rows);
        let selected_row = app
            .hotlist_cursor
            .saturating_sub(window_start)
            .min(window_end.saturating_sub(window_start).saturating_sub(1));
        state.select(Some(selected_row));
    }
    frame.render_stateful_widget(list, list_area, &mut state);

    let open = app
        .keybinding_primary_label(KeyContext::Hotlist, AppCommand::HotlistOpenEntry)
        .unwrap_or("Enter")
        .to_string();
    let add = app
        .keybinding_primary_label(KeyContext::Hotlist, AppCommand::HotlistAddCurrentDirectory)
        .unwrap_or("a")
        .to_string();
    let edit = app
        .keybinding_joined_label(
            KeyContext::Hotlist,
            AppCommand::HotlistEditSelected,
            " / ",
            2,
        )
        .unwrap_or_else(|| String::from("e / F4"));
    let remove = app
        .keybinding_joined_label(
            KeyContext::Hotlist,
            AppCommand::HotlistRemoveSelected,
            " / ",
            2,
        )
        .unwrap_or_else(|| String::from("d/delete"));
    let close = app
        .keybinding_joined_label(KeyContext::Hotlist, AppCommand::CloseHotlist, " / ", 2)
        .unwrap_or_else(|| String::from("Esc/q"));
    frame.render_widget(
        Paragraph::new(format!(
            "{open} open | {add} add | {edit} edit | {remove} remove | {close} close"
        ))
        .style(skin.style("core", "disabled")),
        footer_area,
    );
}

fn render_menu_overlay(frame: &mut Frame, state: &AppState, menu: &MenuState, skin: &UiSkin) {
    let area = frame.area();
    if area.height <= 2 {
        return;
    }

    let popup_x = menu.popup_origin_x().min(area.width.saturating_sub(1));
    let popup_y = 1u16;
    let popup_width = state
        .menu_popup_width(menu)
        .min(area.width.saturating_sub(popup_x));
    let popup_height = menu.popup_height().min(area.height.saturating_sub(popup_y));
    if popup_width == 0 || popup_height == 0 {
        return;
    }

    let popup = Rect::new(popup_x, popup_y, popup_width, popup_height);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(skin.dialog_border_set())
        .border_style(skin.style("menu", "_default_"))
        .style(skin.style("menu", "_default_"));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    // Reserve one column for the list highlight symbol.
    let content_width = inner.width.saturating_sub(1) as usize;
    let items: Vec<ListItem<'_>> = menu
        .active_entries()
        .iter()
        .map(|entry| {
            if !entry.selectable && entry.label.is_empty() {
                let line = "-".repeat(content_width.max(1));
                return ListItem::new(line);
            }

            let entry_style = if entry.is_implemented() {
                skin.style("menu", "_default_")
            } else {
                skin.style("menu", "menuinactive")
            };
            let shortcut = state.menu_entry_shortcut_label(entry);
            if shortcut.is_empty() {
                return ListItem::new(entry.label).style(entry_style);
            }

            let label_width = entry.label.chars().count();
            let shortcut_width = shortcut.chars().count();
            let spacing = content_width
                .saturating_sub(label_width.saturating_add(shortcut_width))
                .max(1);
            ListItem::new(format!(
                "{}{}{}",
                entry.label,
                " ".repeat(spacing),
                shortcut
            ))
            .style(entry_style)
        })
        .collect();
    let list = List::new(items)
        .style(skin.style("menu", "_default_"))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol(" ");
    let mut state = ListState::default();
    if !menu.active_entries().is_empty() {
        state.select(Some(menu.selected_entry));
    }
    frame.render_stateful_widget(list, inner, &mut state);
}

fn render_settings_screen(frame: &mut Frame, settings: &SettingsScreenState, skin: &UiSkin) {
    let area = centered_rect(frame.area(), 82, 20);
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!("Options - {}", settings.title))
        .borders(Borders::ALL)
        .border_set(skin.dialog_border_set())
        .border_style(skin.style("dialog", "_default_"))
        .style(skin.style("dialog", "_default_"));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let items: Vec<ListItem<'_>> = if settings.entries.is_empty() {
        vec![ListItem::new("<empty>")]
    } else {
        settings
            .entries
            .iter()
            .map(|item| ListItem::new(item.text()))
            .collect()
    };
    let list = List::new(items)
        .style(skin.style("dialog", "_default_"))
        .highlight_style(skin.style("dialog", "dfocus"))
        .highlight_symbol(">> ");
    let mut state = ListState::default();
    if !settings.entries.is_empty() {
        state.select(Some(settings.selected_entry));
    }
    frame.render_stateful_widget(list, layout[0], &mut state);

    frame.render_widget(
        Paragraph::new("Up/Down move | Enter apply | Esc close")
            .style(skin.style("core", "disabled")),
        layout[1],
    );
}

fn render_help_screen(frame: &mut Frame, app: &AppState, help: &HelpState, skin: &UiSkin) {
    let (width, height) = app.help_dialog_size();
    let area = centered_rect(frame.area(), width, height);
    frame.render_widget(Clear, area);

    let title = format!("Help - {}", help.current_title());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_set(skin.dialog_border_set())
        .border_style(skin.style("dialog", "_default_"))
        .style(skin.style("dialog", "_default_"));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let base_style = skin.style("dialog", "_default_");
    let link_style = skin.style("menu", "menuhot");
    let selected_link_style = skin.style("dialog", "dfocus");
    let selected_link = help.selected_link();
    let lines: Vec<Line<'_>> = help
        .lines()
        .iter()
        .map(|line| {
            let spans = line
                .spans
                .iter()
                .map(|span| match span {
                    HelpSpan::Text(text) => Span::styled(text.as_str(), base_style),
                    HelpSpan::Link { label, link_index } => {
                        let style = if selected_link == Some(*link_index) {
                            selected_link_style
                        } else {
                            link_style
                        };
                        Span::styled(label.as_str(), style)
                    }
                })
                .collect::<Vec<_>>();
            Line::from(spans)
        })
        .collect();
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .style(base_style)
            .scroll((help.scroll() as u16, 0))
            .wrap(Wrap { trim: false }),
        layout[0],
    );

    let link_cycle = keybinding_joined_or(
        app,
        KeyContext::Help,
        AppCommand::HelpLinkNext,
        "Tab/Shift-Tab",
        2,
    );
    let follow = keybinding_joined_or(
        app,
        KeyContext::Help,
        AppCommand::HelpFollowLink,
        "Enter",
        2,
    );
    let index = keybinding_joined_or(app, KeyContext::Help, AppCommand::HelpIndex, "F2/c", 2);
    let back = keybinding_joined_or(app, KeyContext::Help, AppCommand::HelpBack, "F3/Left", 3);
    let node_cycle = format!(
        "{}/{}",
        keybinding_primary_or(app, KeyContext::Help, AppCommand::HelpNodeNext, "n"),
        keybinding_primary_or(app, KeyContext::Help, AppCommand::HelpNodePrev, "p")
    );
    let close = keybinding_joined_or(app, KeyContext::Help, AppCommand::CloseHelp, "Esc/F10", 2);

    frame.render_widget(
        Paragraph::new(format!(
            "{link_cycle} link | {follow} follow | {index} index | {back} back | {node_cycle} node | {close} close"
        ))
        .style(skin.style("core", "disabled")),
        layout[1],
    );
}

fn panel_entry_size_label(entry: &FileEntry) -> String {
    if entry.is_parent() {
        return String::from("UP--DIR");
    }
    format_human_size_compact(entry.size)
}

fn panel_selection_summary(state: &SelectionSizeState) -> String {
    match state {
        SelectionSizeState::Empty => String::new(),
        SelectionSizeState::Calculating { selected_items } => {
            format!(
                "Calculating size of {selected_items} {}...",
                item_label(*selected_items)
            )
        }
        SelectionSizeState::Ready {
            selected_items,
            apparent_bytes,
            unreadable_entries: 0,
        } => format!(
            "{} in {selected_items} {}",
            format_human_size(*apparent_bytes),
            item_label(*selected_items)
        ),
        SelectionSizeState::Ready {
            selected_items,
            apparent_bytes,
            unreadable_entries,
        } => format!(
            "{} in {selected_items} {} (partial: {unreadable_entries} unreadable)",
            format_human_size(*apparent_bytes),
            item_label(*selected_items)
        ),
        SelectionSizeState::Failed {
            selected_items,
            error,
        } => format!(
            "Size unavailable for {selected_items} {}: {error}",
            item_label(*selected_items)
        ),
    }
}

fn item_label(count: usize) -> &'static str {
    if count == 1 { "item" } else { "items" }
}

fn panel_disk_summary(panel: &PanelState, _app: &AppState) -> String {
    let Some(disk_usage) = panel.disk_usage else {
        return String::from("- / - (-%)");
    };
    let free = disk_usage.free_bytes;
    let total = disk_usage.total_bytes;
    if total == 0 {
        return String::from("0b / 0b (0%)");
    }
    let percent = free.saturating_mul(100) / total;
    format!(
        "{} / {} ({}%)",
        format_human_size(free),
        format_human_size(total),
        percent
    )
}

fn format_modified(modified: Option<SystemTime>) -> String {
    modified
        .map(|time| {
            let local: DateTime<Local> = DateTime::from(time);
            let now = Local::now();
            if local < now - chrono::Duration::days(365) {
                local.format("%b %Y").to_string()
            } else {
                local.format("%b %e %H:%M").to_string()
            }
        })
        .unwrap_or_default()
}

fn format_human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["b", "kb", "Mb", "Gb", "Tb", "Pb"];
    format_human_size_with_units(bytes, &UNITS)
}

fn format_human_size_compact(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["", "k", "M", "G", "T", "P"];
    format_human_size_with_units(bytes, &UNITS)
}

fn format_human_size_with_units(bytes: u64, units: &[&str; 6]) -> String {
    if bytes == 0 {
        return format!("0{}", units[0]);
    }

    let mut value = bytes as f64;
    let mut unit_index = 0usize;
    while value >= 1024.0 && unit_index < units.len() - 1 {
        value /= 1024.0;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{bytes}{}", units[0])
    } else if unit_index == 1 && value >= 10.0 {
        format!("{}{}", value.round() as u64, units[unit_index])
    } else {
        format!(
            "{}{}",
            trim_trailing_decimal(format!("{value:.2}")),
            units[unit_index]
        )
    }
}

fn trim_trailing_decimal(mut value: String) -> String {
    while value.ends_with('0') {
        value.pop();
    }
    if value.ends_with('.') {
        value.pop();
    }
    value
}

fn path_leaf_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn job_row(job: &JobRecord) -> Row<'_> {
    let status = match job.status {
        JobStatus::Queued => "queued",
        JobStatus::Running => "running",
        JobStatus::Succeeded => "ok",
        JobStatus::Canceled => "canceled",
        JobStatus::Failed => "failed",
    };
    let progress = job
        .progress
        .as_ref()
        .map(|progress| {
            format!(
                "{}% {}/{}",
                progress.percent(),
                progress.items_done,
                progress.items_total
            )
        })
        .unwrap_or_else(|| String::from("-"));
    let current = job
        .progress
        .as_ref()
        .and_then(|progress| progress.current_path.as_deref())
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("-"));
    let error = job
        .last_error
        .as_ref()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| String::from("-"));

    Row::new(vec![
        Cell::from(job.id.to_string()),
        Cell::from(job.kind.label()),
        Cell::from(status),
        Cell::from(progress),
        Cell::from(current),
        Cell::from(error),
    ])
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    terminal_rect(centered_overlay_rect(screen_rect(area), width, height))
}

fn screen_rect(area: Rect) -> ScreenRect {
    ScreenRect::new(area.x, area.y, area.width, area.height)
}

fn frame_area(frame: &Frame<'_>) -> ScreenRect {
    screen_rect(frame.area())
}

fn terminal_rect(area: ScreenRect) -> Rect {
    Rect::new(area.x, area.y, area.width, area.height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::{Buffer, Cell};
    use rc_core::{
        AppCommand, AppState, BackgroundEvent, FileEntryKind, FileEntryMetadata, JobError,
        JobEvent, JobRequest, PanelCommand, WorkerCommand, build_tree_ready_event,
        execute_worker_job, measure_selection_size, refresh_panel_event,
    };
    use std::env;
    use std::fs;
    use std::sync::mpsc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn render_to_buffer(state: &AppState, width: u16, height: u16) -> Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test backend should initialize");
        terminal
            .draw(|frame| render(frame, state))
            .expect("render should succeed");
        terminal.backend().buffer().clone()
    }

    fn render_to_text(state: &AppState, width: u16, height: u16) -> String {
        let buffer = render_to_buffer(state, width, height);
        buffer_to_text(&buffer)
    }

    fn buffer_to_text(buffer: &Buffer) -> String {
        let area = buffer.area;
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn first_cell_for_text<'a>(buffer: &'a Buffer, text: &str) -> Option<&'a Cell> {
        let area = buffer.area;
        for y in 0..area.height {
            let mut row = String::new();
            for x in 0..area.width {
                row.push_str(buffer[(x, y)].symbol());
            }
            if let Some(column) = row.find(text) {
                return Some(&buffer[(column as u16, y)]);
            }
        }
        None
    }

    fn drain_background(state: &mut AppState) {
        loop {
            let mut progressed = false;

            let worker_commands = state.take_pending_worker_commands();
            if !worker_commands.is_empty() {
                progressed = true;
            }
            for command in worker_commands {
                match command {
                    WorkerCommand::Run(job) => {
                        let job = *job;
                        let job_id = job.id;
                        let (event_tx, event_rx) = mpsc::channel();
                        match &job.request {
                            JobRequest::RefreshPanel {
                                panel,
                                cwd,
                                source,
                                sort_mode,
                                filter,
                                show_hidden_files,
                                cached_panelized_entries,
                                request_id,
                            } => {
                                let _ = event_tx.send(JobEvent::Started { id: job_id });
                                let cancel_flag = job.cancel_flag();
                                state.handle_background_event(refresh_panel_event(
                                    rc_core::PanelRefreshStreamRequest {
                                        panel: *panel,
                                        cwd: cwd.clone(),
                                        source: source.clone(),
                                        sort_mode: *sort_mode,
                                        filter: filter.clone(),
                                        show_hidden_files: *show_hidden_files,
                                        cached_panelized_entries: cached_panelized_entries.clone(),
                                        request_id: *request_id,
                                    },
                                    cancel_flag.as_ref(),
                                ));
                                let _ = event_tx.send(JobEvent::Finished {
                                    id: job_id,
                                    result: Ok(()),
                                });
                            }
                            JobRequest::LoadViewer { path } => {
                                let _ = event_tx.send(JobEvent::Started { id: job_id });
                                let viewer_result = rc_core::ViewerState::open(path.clone())
                                    .map_err(|error| error.to_string());
                                state.handle_background_event(BackgroundEvent::ViewerLoaded {
                                    path: path.clone(),
                                    result: viewer_result.clone(),
                                });
                                let result =
                                    viewer_result.map(|_| ()).map_err(JobError::from_message);
                                let _ = event_tx.send(JobEvent::Finished { id: job_id, result });
                            }
                            JobRequest::QuickCdSearch { spec, request_id } => {
                                let _ = event_tx.send(JobEvent::Started { id: job_id });
                                let cancel_flag = job.cancel_flag();
                                let result = rc_core::run_quick_cd_search(
                                    spec,
                                    cancel_flag.as_ref(),
                                    |snapshot| {
                                        state.handle_background_event(
                                            BackgroundEvent::QuickCdSearchUpdated {
                                                request_id: *request_id,
                                                snapshot,
                                            },
                                        );
                                        true
                                    },
                                )
                                .map(|_| ())
                                .map_err(|error| JobError::from_message(error.to_string()));
                                let _ = event_tx.send(JobEvent::Finished { id: job_id, result });
                            }
                            JobRequest::LoadQuickView {
                                panel,
                                path,
                                request_id,
                            } => {
                                let _ = event_tx.send(JobEvent::Started { id: job_id });
                                let viewer_result = rc_core::ViewerState::open(path.clone())
                                    .map_err(|error| error.to_string());
                                state.handle_background_event(BackgroundEvent::QuickViewLoaded {
                                    panel: *panel,
                                    path: path.clone(),
                                    request_id: *request_id,
                                    result: viewer_result.clone(),
                                });
                                let result =
                                    viewer_result.map(|_| ()).map_err(JobError::from_message);
                                let _ = event_tx.send(JobEvent::Finished { id: job_id, result });
                            }
                            JobRequest::MeasureSelection {
                                panel,
                                paths,
                                request_id,
                            } => {
                                let _ = event_tx.send(JobEvent::Started { id: job_id });
                                let cancel_flag = job.cancel_flag();
                                let result = measure_selection_size(paths, cancel_flag.as_ref())
                                    .map(|report| {
                                        state.handle_background_event(
                                            BackgroundEvent::SelectionSizeMeasured {
                                                panel: *panel,
                                                request_id: *request_id,
                                                report,
                                            },
                                        );
                                    })
                                    .map_err(|error| {
                                        if error.kind() == std::io::ErrorKind::Interrupted {
                                            JobError::canceled()
                                        } else {
                                            JobError::from_io(error)
                                        }
                                    });
                                let _ = event_tx.send(JobEvent::Finished { id: job_id, result });
                            }
                            JobRequest::BuildTree {
                                root,
                                max_depth,
                                max_entries,
                            } => {
                                let _ = event_tx.send(JobEvent::Started { id: job_id });
                                let cancel_flag = job.cancel_flag();
                                let result = build_tree_ready_event(
                                    job_id,
                                    root.clone(),
                                    *max_depth,
                                    *max_entries,
                                    cancel_flag.as_ref(),
                                )
                                .map(|event| state.handle_background_event(event))
                                .map_err(JobError::from_io);
                                let _ = event_tx.send(JobEvent::Finished { id: job_id, result });
                            }
                            _ => {
                                execute_worker_job(job, &event_tx);
                            }
                        }
                        for event in event_rx.try_iter() {
                            state.handle_job_event(event);
                        }
                    }
                    WorkerCommand::Cancel(_) | WorkerCommand::Shutdown => {}
                }
            }

            if !progressed {
                break;
            }
        }
    }

    fn app_with_loaded_panels(root: std::path::PathBuf) -> AppState {
        let mut app = AppState::new(root).expect("app should initialize");
        app.refresh_panels();
        drain_background(&mut app);
        app
    }

    fn temp_root(label: &str) -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let path = env::temp_dir().join(format!("rc-ui-test-{label}-{stamp}"));
        fs::create_dir_all(&path).expect("temp root should be creatable");
        path
    }

    #[test]
    fn render_draws_complete_find_form() {
        let root = temp_root("find-form");
        let mut app = app_with_loaded_panels(root.clone());
        app.apply(AppCommand::OpenFindDialog)
            .expect("find dialog should open");

        let rendered = render_to_text(&app, 120, 40);
        assert!(rendered.contains("Find file"));
        assert!(rendered.contains("Starting directory"));
        assert!(rendered.contains("Filename pattern"));
        assert!(rendered.contains("Pattern mode"));
        assert!(rendered.contains("Case sensitive"));
        assert!(rendered.contains("Containing text"));
        assert!(rendered.contains("Whole words"));
        assert!(rendered.contains("Ignore dirs (comma)"));
        assert!(rendered.contains("F2 tree picker"));

        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[test]
    fn render_draws_pair_input_fields_and_values() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).expect("test backend should initialize");
        let dialog =
            DialogState::pair_input("Edit entry", "Name:", "Documentation", "Path:", "/tmp/docs");
        terminal
            .draw(|frame| render_dialog(frame, &dialog, current_skin().as_ref()))
            .expect("dialog should render");
        let rendered = buffer_to_text(terminal.backend().buffer());
        assert!(rendered.contains("Edit entry"));
        assert!(rendered.contains("Name:"));
        assert!(rendered.contains("Documentation"));
        assert!(rendered.contains("Path:"));
        assert!(rendered.contains("/tmp/docs"));
        assert!(rendered.contains("Tab next field"));
    }

    #[test]
    fn render_draws_quick_cd_query_ranked_results_and_search_state() {
        let backend = TestBackend::new(110, 32);
        let mut terminal = Terminal::new(backend).expect("test backend should initialize");
        let mut dialog = DialogState::quick_cd("project");
        let DialogKind::QuickCd(quick_cd) = &mut dialog.kind else {
            panic!("quick-cd dialog should be constructed");
        };
        quick_cd.apply_search_snapshot(rc_core::QuickCdSearchSnapshot {
            suggestions: vec![
                rc_core::QuickCdSuggestion {
                    path: "/work/project".into(),
                    display: String::from("./project"),
                },
                rc_core::QuickCdSuggestion {
                    path: "/home/user/archive-project".into(),
                    display: String::from("~/archive-project"),
                },
            ],
            visited_directories: 418,
            skipped_directories: 3,
            truncated: true,
            complete: true,
        });
        quick_cd.selected = 1;

        terminal
            .draw(|frame| render_dialog(frame, &dialog, current_skin().as_ref()))
            .expect("dialog should render");
        let rendered = buffer_to_text(terminal.backend().buffer());
        assert!(rendered.contains("Quick cd"));
        assert!(rendered.contains("Path or substring:"));
        assert!(rendered.contains("project"));
        assert!(rendered.contains("./project"));
        assert!(rendered.contains("~/archive-project"));
        assert!(rendered.contains("2 match(es) · 418 dirs, 3 skipped · bounded"));
        assert!(rendered.contains("Up/Down choose"));
    }

    #[test]
    fn render_hotlist_shows_labels_paths_and_crud_hints() {
        let root = temp_root("hotlist");
        let docs = std::path::PathBuf::from("/tmp/docs");
        let mut app = app_with_loaded_panels(root.clone());
        app.settings_mut().configuration.hotlist = vec![
            rc_core::HotlistEntry::new("Documentation", docs.clone()),
            rc_core::HotlistEntry::new("Injected\nlabel", "/tmp".into()),
        ];
        app.apply(AppCommand::OpenHotlist)
            .expect("hotlist should open");

        let rendered = render_to_text(&app, 120, 40);
        assert!(rendered.contains("Directory hotlist (2)"));
        assert!(rendered.contains("Documentation"));
        assert!(rendered.contains(docs.to_string_lossy().as_ref()));
        assert!(rendered.contains("Injected label"));
        assert!(rendered.contains("add"));
        assert!(rendered.contains("edit"));
        assert!(rendered.contains("remove"));

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn render_panelize_presets_shows_names_and_management_hints() {
        let root = temp_root("panelize-presets");
        let mut app = app_with_loaded_panels(root.clone());
        app.apply(AppCommand::OpenPanelizeDialog)
            .expect("panelize dialog should open");

        let rendered = render_to_text(&app, 100, 30);
        assert!(rendered.contains("External panelize"));
        assert!(rendered.contains("All files"));
        assert!(rendered.contains("Tab command"));
        assert!(rendered.contains("F2 add"));
        assert!(rendered.contains("F4 edit"));
        assert!(rendered.contains("F8 remove"));

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn render_find_results_surfaces_partial_and_truncated_state() {
        let root = temp_root("find-results");
        let mut app = app_with_loaded_panels(root.clone());
        app.apply(AppCommand::OpenFindDialog)
            .expect("find dialog should open");
        for character in "*.rs".chars() {
            app.apply(AppCommand::DialogInputChar(character))
                .expect("find input should succeed");
        }
        app.apply(AppCommand::DialogAccept)
            .expect("find should start");
        let job_id = app.jobs.last_job().expect("find job should exist").id;
        app.handle_background_event(BackgroundEvent::FindEntriesChunk {
            job_id,
            entries: vec![rc_core::FindResultEntry {
                path: root.join("match.rs"),
                is_dir: false,
            }],
        });
        app.handle_background_event(BackgroundEvent::FindCompleted {
            job_id,
            report: rc_core::FindSearchReport {
                matched_entries: 1,
                issue_count: 1,
                issues: vec![rc_core::FindSearchIssue {
                    kind: rc_core::FindSearchIssueKind::ReadDirectory,
                    path: root.join("denied"),
                    message: String::from("permission denied"),
                }],
                truncated: true,
                ..rc_core::FindSearchReport::default()
            },
        });

        let rendered = render_to_text(&app, 120, 40);
        assert!(rendered.contains("partial, limit reached, read errors"));
        assert!(rendered.contains("permission denied"));
        assert!(rendered.contains("match.rs"));
        assert!(rendered.contains("again"));
        assert!(rendered.contains("pause/continue"));

        fs::remove_dir_all(&root).expect("temp root should be removable");
    }

    #[test]
    fn human_size_format_matches_expected_style() {
        assert_eq!(format_human_size(24 * 1024), "24kb");
        assert_eq!(
            format_human_size((5.11_f64 * 1024.0 * 1024.0) as u64),
            "5.11Mb"
        );
        assert_eq!(format_human_size(1_342_177_280), "1.25Gb");
        assert_eq!(format_human_size_compact(24 * 1024), "24k");
        assert_eq!(
            format_human_size_compact((5.11_f64 * 1024.0 * 1024.0) as u64),
            "5.11M"
        );
        assert_eq!(format_human_size_compact(1_342_177_280), "1.25G");
    }

    #[test]
    fn format_modified_uses_year_for_entries_older_than_one_year() {
        let old = SystemTime::now()
            .checked_sub(Duration::from_secs(366 * 24 * 60 * 60))
            .expect("old timestamp should be representable");
        let old_local: DateTime<Local> = DateTime::from(old);
        assert_eq!(
            format_modified(Some(old)),
            old_local.format("%b %Y").to_string()
        );

        let recent = SystemTime::now()
            .checked_sub(Duration::from_secs(60))
            .expect("recent timestamp should be representable");
        let recent_local: DateTime<Local> = DateTime::from(recent);
        assert_eq!(
            format_modified(Some(recent)),
            recent_local.format("%b %e %H:%M").to_string()
        );
    }

    #[test]
    fn render_draws_file_manager_panels() {
        let root = temp_root("panels");
        fs::write(root.join("entry.txt"), "demo").expect("file should be creatable");
        let app = app_with_loaded_panels(root.clone());
        let frame = render_to_text(&app, 100, 30);
        assert!(
            frame.contains("context: FileManager"),
            "frame should include file manager context header"
        );
        assert!(
            frame.contains("entry.txt"),
            "frame should include panel entry names"
        );
        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn panel_footer_reports_recursive_size_for_a_tagged_directory() {
        let root = temp_root("recursive-selected-total");
        let selected = root.join("selected");
        fs::create_dir_all(selected.join("nested")).expect("nested directory should be creatable");
        fs::write(selected.join("nested/payload"), vec![0_u8; 3 * 1024])
            .expect("payload should be writable");
        let mut app = app_with_loaded_panels(root.clone());
        app.active_panel_mut().cursor = app
            .active_panel()
            .entries
            .iter()
            .position(|entry| entry.path == selected)
            .expect("selected directory should be listed");

        app.apply(AppCommand::ToggleTag)
            .expect("directory should be taggable");
        let calculating = render_to_text(&app, 120, 24);
        assert!(calculating.contains("Calculating size of 1 item..."));

        drain_background(&mut app);
        let measured = render_to_text(&app, 120, 24);
        assert!(measured.contains("3kb in 1 item"));

        fs::remove_dir_all(root).expect("temporary root should be removable");
    }

    #[test]
    fn render_draws_filter_form_and_marks_the_filtered_panel_title() {
        let root = temp_root("filter-dialog");
        fs::write(root.join("main.rs"), "fn main() {}").expect("fixture should be creatable");
        let mut app = app_with_loaded_panels(root.clone());
        app.apply(AppCommand::Panel(
            ActivePanel::Left,
            PanelCommand::OpenFilter,
        ))
        .expect("filter dialog should open");
        for character in "*.rs".chars() {
            app.apply(AppCommand::DialogInputChar(character))
                .expect("filter input should succeed");
        }

        let dialog_frame = render_to_text(&app, 100, 30);
        assert!(dialog_frame.contains("Pattern"));
        assert!(dialog_frame.contains("*.rs"));
        assert!(dialog_frame.contains("Files only"));
        assert!(dialog_frame.contains("shell pattern"));

        app.apply(AppCommand::DialogAccept)
            .expect("filter should apply");
        drain_background(&mut app);
        let panel_frame = render_to_text(&app, 120, 30);
        assert!(panel_frame.contains("filter:*.rs"));

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn brief_listing_uses_multiple_responsive_name_columns() {
        let root = temp_root("brief-listing");
        for name in ["alpha", "bravo", "charlie", "delta"] {
            fs::write(root.join(name), name).expect("file should be creatable");
        }
        let mut app = app_with_loaded_panels(root.clone());
        app.apply(AppCommand::Panel(
            ActivePanel::Left,
            PanelCommand::OpenListingFormat,
        ))
        .expect("listing format dialog should open");
        app.apply(AppCommand::DialogListboxSelectAt(1))
            .expect("brief format should be selected");
        app.apply(AppCommand::DialogAccept)
            .expect("brief format should be applied");

        let frame = render_to_text(&app, 120, 24);
        assert!(frame.contains("brief | sort:name asc"));
        assert!(
            frame.lines().any(|line| {
                ["alpha", "bravo", "charlie", "delta"]
                    .into_iter()
                    .filter(|name| line.contains(name))
                    .count()
                    >= 2
            }),
            "brief format should place multiple names on one row"
        );

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn brief_grid_moves_between_existing_cells_on_the_same_visual_row() {
        let grid = BriefGrid::new(13, 58);

        assert_eq!(grid.columns, 3);
        assert_eq!(grid.rows, 5);
        assert_eq!(grid.right_of(0), Some(5));
        assert_eq!(grid.right_of(5), Some(10));
        assert_eq!(grid.right_of(10), None);
        assert_eq!(grid.left_of(10), Some(5));
        assert_eq!(grid.right_of(4), Some(9));
        assert_eq!(grid.right_of(9), None, "the last column has no row four");
    }

    #[test]
    fn long_listing_uses_full_width_metadata_columns() {
        let root = temp_root("long-listing");
        fs::write(root.join("entry.txt"), "demo").expect("file should be creatable");
        let other = root.join("other");
        fs::create_dir_all(&other).expect("other panel directory should be creatable");
        fs::write(other.join("right-only.txt"), "hidden")
            .expect("other panel file should be creatable");
        let mut app = app_with_loaded_panels(root.clone());
        app.panels[ActivePanel::Right.index()].cwd = other;
        app.panels[ActivePanel::Right.index()]
            .refresh()
            .expect("other panel should refresh");
        app.apply(AppCommand::Panel(
            ActivePanel::Left,
            PanelCommand::OpenListingFormat,
        ))
        .expect("listing format dialog should open");
        app.apply(AppCommand::DialogListboxSelectAt(2))
            .expect("long format should be selected");
        app.apply(AppCommand::DialogAccept)
            .expect("long format should be applied");

        let frame = render_to_text(&app, 120, 24);
        assert!(frame.contains("long | sort:name asc"));
        assert!(frame.lines().any(|line| {
            ["Mode", "Links", "UID", "GID", "Modify time", "Name"]
                .into_iter()
                .all(|label| line.contains(label))
        }));
        assert!(frame.contains("entry.txt"));
        assert!(
            !frame.contains("right-only.txt"),
            "long format should allocate the content area to one panel"
        );

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn file_mode_formatter_handles_special_execute_bits() {
        let entry = FileEntry {
            name: String::from("tool"),
            path: "tool".into(),
            kind: FileEntryKind::File,
            size: 0,
            modified: None,
            metadata: FileEntryMetadata {
                mode: Some(0o104751),
                ..FileEntryMetadata::default()
            },
        };

        assert_eq!(format_file_mode(&entry), "-rwsr-x--x");
    }

    #[test]
    fn render_info_panel_tracks_the_other_panel_selection() {
        let root = temp_root("info-panel");
        fs::write(root.join("entry.txt"), "demo").expect("file should be creatable");
        let mut app = app_with_loaded_panels(root.clone());
        app.active_panel = ActivePanel::Right;
        app.move_cursor(1);
        app.apply(AppCommand::Panel(
            ActivePanel::Left,
            PanelCommand::SetView(PanelViewMode::Info),
        ))
        .expect("left info panel should open");

        let frame = render_to_text(&app, 120, 30);
        assert!(frame.contains("Info | right panel selection"));
        assert!(frame.contains("Name: entry.txt"));
        assert!(frame.contains("Type: file"));
        assert!(frame.contains("Size: 4b (4 bytes)"));

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn render_quick_view_panel_uses_background_loaded_content() {
        let root = temp_root("quick-view-panel");
        let preview_path = root.join("preview.txt");
        fs::write(&preview_path, "quick-view-payload\nsecond line\n")
            .expect("preview file should be creatable");
        let mut app = app_with_loaded_panels(root.clone());
        app.apply(AppCommand::Panel(
            ActivePanel::Right,
            PanelCommand::OpenListingFormat,
        ))
        .expect("right listing format should open");
        app.apply(AppCommand::DialogListboxSelectAt(2))
            .expect("long format should be selected");
        app.apply(AppCommand::DialogAccept)
            .expect("long format should be applied");
        app.panels[ActivePanel::Right.index()].cursor = app.panels[ActivePanel::Right.index()]
            .entries
            .iter()
            .position(|entry| entry.path == preview_path)
            .expect("preview file should be listed");
        app.apply(AppCommand::Panel(
            ActivePanel::Left,
            PanelCommand::SetView(PanelViewMode::QuickView),
        ))
        .expect("quick view should open");

        let loading_frame = render_to_text(&app, 100, 24);
        assert!(loading_frame.contains("<loading preview...>"));

        drain_background(&mut app);
        let loaded_frame = render_to_text(&app, 100, 24);
        assert!(loaded_frame.contains("Quick view"));
        assert!(loaded_frame.contains("preview.txt"));
        assert!(loaded_frame.contains("quick-view-payload"));
        assert!(
            loaded_frame.contains("full | sort:name asc"),
            "quick view should retain a split layout even when its source requested long format"
        );

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn fit_single_line_sanitizes_and_truncates() {
        assert_eq!(fit_single_line("abc\ndef", 20), "abc def");
        assert_eq!(fit_single_line("abcdefg", 6), "abc...");
        assert_eq!(fit_single_line("abcdefg", 2), "..");
        assert_eq!(fit_single_line("你好世界", 5), "你...");
        assert_eq!(
            fit_single_line("e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}", 5),
            "e\u{301}e\u{301}..."
        );
        assert!(
            unicode_width::UnicodeWidthStr::width(fit_single_line("你好世界", 5).as_str()) <= 5,
            "truncated output should fit requested terminal width"
        );
    }

    #[test]
    fn render_status_line_sanitizes_newlines_and_clips_width() {
        let root = temp_root("status-clamp");
        let mut app = app_with_loaded_panels(root.clone());
        app.settings_mut().layout.show_debug_status = false;
        app.set_status(format!("line1\nline2 {}", "x".repeat(200)));

        let frame = render_to_text(&app, 40, 12);
        let lines: Vec<&str> = frame.lines().collect();
        let status_line = lines[lines.len().saturating_sub(2)];

        assert!(
            status_line.contains("line1 line2"),
            "status should replace newlines with spaces"
        );
        assert!(
            status_line.trim_end().ends_with("..."),
            "long status should be clipped to one line with ellipsis"
        );

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[cfg(unix)]
    #[test]
    fn panel_title_marks_panelize_panels() {
        let root = temp_root("panelize-title");
        fs::write(root.join("entry.txt"), "demo").expect("file should be creatable");
        let mut app = app_with_loaded_panels(root.clone());
        app.apply(AppCommand::OpenPanelizeDialog)
            .expect("panelize dialog should open");
        app.apply(AppCommand::DialogAccept)
            .expect("default panelize preset should run");
        drain_background(&mut app);

        let title = panel_title(
            app.active_panel(),
            app.panel_listing_format(app.active_panel),
        );
        assert!(
            title.contains("panelize"),
            "panel title should indicate panelize mode"
        );

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn render_tree_reports_depth_truncation() {
        let root = temp_root("tree-depth-limit");
        fs::create_dir_all(root.join("branch").join("deep"))
            .expect("tree fixture should be creatable");
        let mut app = app_with_loaded_panels(root.clone());
        app.settings_mut().advanced.tree_max_depth = 1;
        app.apply(AppCommand::OpenTree)
            .expect("tree route should open");
        drain_background(&mut app);

        let frame = render_to_text(&app, 120, 40);
        assert!(
            frame.contains("Directory tree (2) | depth limit"),
            "{frame}"
        );

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn render_tree_switches_between_dynamic_and_static_views() {
        let root = temp_root("tree-navigation-mode");
        fs::create_dir_all(root.join("alpha").join("deep"))
            .expect("tree fixture should be creatable");
        fs::create_dir_all(root.join("beta")).expect("tree fixture should be creatable");
        let mut app = app_with_loaded_panels(root.clone());
        app.apply(AppCommand::OpenTree)
            .expect("tree route should open");
        drain_background(&mut app);

        let dynamic = render_to_text(&app, 120, 40);
        assert!(dynamic.contains("| dynamic"), "{dynamic}");
        assert!(dynamic.contains("alpha/"), "{dynamic}");
        assert!(dynamic.contains("beta/"), "{dynamic}");
        assert!(!dynamic.contains("deep/"), "{dynamic}");

        app.apply(AppCommand::TreeToggleNavigation)
            .expect("tree navigation should toggle");
        app.apply(AppCommand::TreeSearchAppend('d'))
            .expect("tree search should update");
        let static_view = render_to_text(&app, 120, 40);
        assert!(static_view.contains("| static"), "{static_view}");
        assert!(static_view.contains("deep/"), "{static_view}");
        assert!(static_view.contains("Search: d"), "{static_view}");

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn render_draws_viewer_hex_mode() {
        let root = temp_root("viewer-hex");
        let file_path = root.join("bin.dat");
        fs::write(
            &file_path,
            b"0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .expect("file should be creatable");

        let mut app = app_with_loaded_panels(root.clone());
        let index = app
            .active_panel()
            .entries
            .iter()
            .position(|entry| entry.path == file_path)
            .expect("file should be listed");
        app.active_panel_mut().cursor = index;
        app.apply(AppCommand::OpenEntry)
            .expect("viewer command should succeed");
        drain_background(&mut app);
        app.apply(AppCommand::ViewerToggleHex)
            .expect("hex mode should toggle");

        let frame = render_to_text(&app, 120, 40);
        assert!(
            frame.contains("context: ViewerHex"),
            "frame should show viewer hex key context"
        );
        assert!(
            frame.contains("00000000"),
            "frame should render hex offsets"
        );

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn viewer_highlight_key_tracks_path_and_content_fingerprints() {
        let root = temp_root("viewer-highlight-key");
        let first_path = root.join("first.txt");
        let second_path = root.join("second.txt");
        let third_path = root.join("third.txt");
        fs::write(&first_path, "abc").expect("first fixture should be writable");
        fs::write(&second_path, "abc").expect("second fixture should be writable");
        fs::write(&third_path, "xyz").expect("third fixture should be writable");

        let first_viewer = rc_core::ViewerState::open(first_path.clone())
            .expect("first viewer fixture should open");
        let second_viewer = rc_core::ViewerState::open(second_path.clone())
            .expect("second viewer fixture should open");
        let third_viewer = rc_core::ViewerState::open(third_path.clone())
            .expect("third viewer fixture should open");
        let first_key = viewer_highlight_key(&first_viewer);
        let second_key = viewer_highlight_key(&second_viewer);
        let third_key = viewer_highlight_key(&third_viewer);
        assert_ne!(
            first_key, second_key,
            "cache key should differ for identical content at different paths"
        );
        assert_ne!(
            first_key, third_key,
            "cache key should differ for different content with the same byte length"
        );

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn viewer_highlight_cache_populates_lines_incrementally() {
        let root = temp_root("viewer-highlight-incremental");
        let file_path = root.join("viewer.rs");
        let content = (0..200)
            .map(|index| format!("fn line_{index}() {{}}\n"))
            .collect::<String>();
        fs::write(&file_path, content).expect("viewer fixture should be writable");

        let viewer = rc_core::ViewerState::open(file_path).expect("viewer fixture should open");
        let resources = build_highlight_resources_for_skin(current_skin().as_ref())
            .expect("highlight resources should initialize");
        let mut cache = CachedViewerHighlight::new(&viewer, &resources)
            .expect("highlight cache should initialize");
        assert_eq!(
            cache.highlighted_lines.len(),
            0,
            "cache should start without precomputed highlighted lines"
        );
        assert!(
            cache.raw_lines.len() >= 200,
            "cache should preserve all source lines for deferred highlighting"
        );

        cache
            .ensure_highlighted_up_to(5, &resources)
            .expect("highlighting first visible range should succeed");
        assert_eq!(
            cache.highlighted_lines.len(),
            5,
            "highlighting should compute only the first requested window"
        );
        assert!(
            cache.highlighted_lines.len() < cache.raw_lines.len(),
            "first window highlight should not eagerly process the entire file"
        );

        cache
            .ensure_highlighted_up_to(9, &resources)
            .expect("highlighting larger range should succeed");
        assert_eq!(
            cache.highlighted_lines.len(),
            9,
            "expanding the window should append only newly visible highlights"
        );

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn render_draws_help_overlay() {
        let root = temp_root("help");
        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.apply(AppCommand::OpenHelp)
            .expect("help route should open");

        let frame = render_to_text(&app, 120, 40);
        assert!(
            frame.contains("Help - File Manager"),
            "frame should include help title"
        );
        assert!(
            frame.contains("Tab/Shift-Tab"),
            "frame should include help viewer hint line"
        );

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn render_draws_menu_overlay() {
        let root = temp_root("menu");
        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.apply(AppCommand::OpenMenuAt(1))
            .expect("menu route should open");

        let frame = render_to_text(&app, 120, 40);
        assert!(
            frame.contains("File"),
            "frame should include active menu title"
        );
        assert_eq!(
            frame.matches("File").count(),
            2,
            "menu title should appear in top menu and status, not be repeated in popup title"
        );
        assert!(
            frame.contains("Copy"),
            "frame should include menu entry labels"
        );
        assert!(
            frame.contains("F10"),
            "function-key shortcuts should keep all digits\n{frame}"
        );
        assert!(
            frame.contains("M-c"),
            "meta-key shortcuts should keep trailing characters\n{frame}"
        );

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn render_styles_unimplemented_menu_entries_as_inactive() {
        let root = temp_root("menu-inactive");
        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.apply(AppCommand::OpenMenuAt(1))
            .expect("menu route should open");

        let buffer = render_to_buffer(&app, 120, 40);
        let skin = current_skin();
        let inactive_style = skin.style("menu", "menuinactive");
        let disabled_cell = first_cell_for_text(&buffer, "View file...")
            .expect("unimplemented menu entry should render");
        assert_eq!(
            disabled_cell.fg,
            inactive_style.fg.unwrap_or(Color::Reset),
            "unimplemented menu label should use inactive foreground"
        );
        assert_eq!(
            disabled_cell.bg,
            inactive_style.bg.unwrap_or(Color::Reset),
            "unimplemented menu label should use inactive background"
        );

        let enabled_cell =
            first_cell_for_text(&buffer, "Copy").expect("implemented menu entry should render");
        assert_ne!(
            disabled_cell.fg, enabled_cell.fg,
            "implemented and unimplemented menu entries should be visually distinct"
        );

        fs::remove_dir_all(root).expect("temp root should be removable");
    }
}
