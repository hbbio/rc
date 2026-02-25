#![forbid(unsafe_code)]

mod skin;

use chrono::{DateTime, Local};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState, Wrap,
};
use rc_core::{
    ActivePanel, AppState, DialogButtonFocus, DialogKind, DialogState, FileEntry, FindResultsState,
    HelpSpan, HelpState, JobRecord, JobStatus, MenuState, PanelState, Route, TreeState,
    ViewerState, top_menus,
};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color as SyntectColor, FontStyle, Style as SyntectStyle, Theme};
use syntect::parsing::{SyntaxReference, SyntaxSet};

#[cfg(unix)]
use nix::sys::statvfs::statvfs;

use skin::{UiSkin, current_skin};
pub use skin::{configure_skin, current_skin_name, list_available_skins};

struct HighlightResources {
    syntax_set: SyntaxSet,
    theme: Theme,
}

static HIGHLIGHT_RESOURCES: OnceLock<Option<HighlightResources>> = OnceLock::new();
static VIEWER_HIGHLIGHT_CACHE: OnceLock<Mutex<Option<CachedViewerHighlight>>> = OnceLock::new();
static DISK_USAGE_SUMMARY_CACHE: OnceLock<Mutex<HashMap<std::path::PathBuf, (Instant, String)>>> =
    OnceLock::new();
const PANEL_SIZE_COL_WIDTH: usize = 12;
const PANEL_SIZE_VALUE_WIDTH: usize = PANEL_SIZE_COL_WIDTH - 1;
const DISK_USAGE_CACHE_TTL: Duration = Duration::from_millis(750);
const DISK_USAGE_CACHE_MAX_ENTRIES: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ViewerHighlightKey {
    content_ptr: usize,
    content_len: usize,
}

struct CachedViewerHighlight {
    key: ViewerHighlightKey,
    raw_lines: Vec<String>,
    highlighted_lines: Vec<Line<'static>>,
    highlighter: HighlightLines<'static>,
}

pub fn render(frame: &mut Frame, state: &AppState) {
    let skin = current_skin();
    let job_counts = state.jobs_status_counts();
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let active_menu = match state.top_route() {
        Route::Menu(menu) => Some(menu.active_menu),
        _ => None,
    };
    render_menu_bar(frame, root[0], skin.as_ref(), active_menu);

    if let Some(viewer) = state.active_viewer() {
        render_viewer(frame, root[1], viewer, skin.as_ref());
    } else {
        let panel_areas = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(root[1]);

        render_panel(
            frame,
            panel_areas[0],
            &state.panels[0],
            state.active_panel == ActivePanel::Left,
            skin.as_ref(),
        );
        render_panel(
            frame,
            panel_areas[1],
            &state.panels[1],
            state.active_panel == ActivePanel::Right,
            skin.as_ref(),
        );
    }

    let status = format!(
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
    );
    frame.render_widget(Paragraph::new(status), root[2]);
    render_button_bar(frame, root[3], skin.as_ref(), state.top_route());

    match state.top_route() {
        Route::Dialog(dialog) => render_dialog(frame, dialog, skin.as_ref()),
        Route::Jobs => render_jobs_screen(frame, state, skin.as_ref()),
        Route::Viewer(_) => {}
        Route::FindResults(results) => render_find_results_screen(frame, results, skin.as_ref()),
        Route::Tree(tree) => render_tree_screen(frame, tree, skin.as_ref()),
        Route::Hotlist => render_hotlist_screen(frame, state, skin.as_ref()),
        Route::Help(help) => render_help_screen(frame, help, skin.as_ref()),
        Route::Menu(menu) => render_menu_overlay(frame, menu, skin.as_ref()),
        Route::FileManager => {}
    }
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

fn render_button_bar(frame: &mut Frame, area: Rect, skin: &UiSkin, route: &Route) {
    let hotkey_style = skin.style("buttonbar", "hotkey");
    let button_style = skin.style("buttonbar", "button");
    let labels: [(&str, &str); 10] = match route {
        Route::FindResults(_) => [
            ("1", "Help"),
            ("2", ""),
            ("3", ""),
            ("4", ""),
            ("5", "Panelize"),
            ("6", ""),
            ("7", ""),
            ("8", ""),
            ("9", ""),
            ("10", "Close"),
        ],
        Route::Help(_) => [
            ("1", "Help"),
            ("2", "Index"),
            ("3", "Prev"),
            ("4", ""),
            ("5", ""),
            ("6", ""),
            ("7", ""),
            ("8", ""),
            ("9", ""),
            ("10", "Quit"),
        ],
        _ => [
            ("1", "Help"),
            ("2", "Menu"),
            ("3", "View"),
            ("4", "Edit"),
            ("5", "Copy"),
            ("6", "RenMov"),
            ("7", "Mkdir"),
            ("8", "Delete"),
            ("9", "PullDn"),
            ("10", "Quit"),
        ],
    };

    let mut spans: Vec<Span<'_>> = Vec::new();
    for (index, (number, label)) in labels.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" ", button_style));
        }
        spans.push(Span::styled(number, hotkey_style));
        spans.push(Span::styled(label, button_style));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)).style(button_style), area);
}

fn panel_title(panel: &PanelState) -> String {
    let panelize_suffix = if panel.is_panelized() {
        " | panelize"
    } else {
        ""
    };
    format!(
        "{}{} | sort:{} | tagged:{}{}",
        panel.cwd.to_string_lossy(),
        panelize_suffix,
        panel.sort_label(),
        panel.tagged_count(),
        if panel.loading { " | loading..." } else { "" }
    )
}

fn render_panel(frame: &mut Frame, area: Rect, panel: &PanelState, active: bool, skin: &UiSkin) {
    let title = panel_title(panel);
    let selected_tagged = panel
        .selected_entry()
        .is_some_and(|entry| !entry.is_parent && panel.is_tagged(&entry.path));
    let highlight_style = if !active {
        skin.style("core", "_default_")
    } else if selected_tagged {
        skin.style("core", "markselect")
    } else {
        skin.style("core", "selected")
    };

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
        let viewport_rows = panel_layout[0].height.saturating_sub(1).max(1) as usize;
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
                let tagged = !entry.is_parent && panel.is_tagged(&entry.path);
                let mut entry_style = if tagged {
                    skin.style("core", "marked")
                } else {
                    skin.style("core", "_default_")
                };
                if entry.is_dir {
                    entry_style = entry_style.patch(skin.style("filehighlight", "directory"));
                }

                let marker = if tagged { "*" } else { " " };
                let label = if entry.is_parent {
                    String::from("/..")
                } else if entry.is_dir {
                    format!("/{}/", entry.name)
                } else {
                    entry.name.clone()
                };
                Row::new(vec![
                    Cell::from(format!("{marker}{label}")),
                    Cell::from(format!(
                        "{:>width$} ",
                        panel_entry_size_label(entry),
                        width = PANEL_SIZE_VALUE_WIDTH
                    )),
                    Cell::from(format_modified(entry.modified)),
                ])
                .style(entry_style)
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
        .highlight_style(highlight_style)
        .column_spacing(1);

        let mut table_state = TableState::default();
        table_state.select(Some(selected_row));
        frame.render_stateful_widget(table, panel_layout[0], &mut table_state);
    }

    let (selected_count, selected_size) = panel_selected_totals(panel);
    let selected_summary = if selected_count == 0 {
        String::new()
    } else {
        format!(
            "{} in {} {}",
            format_human_size(selected_size),
            selected_count,
            if selected_count == 1 { "file" } else { "files" }
        )
    };
    let disk_summary = panel_disk_summary(panel);
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

fn render_viewer(frame: &mut Frame, area: Rect, viewer: &ViewerState, skin: &UiSkin) {
    frame.render_widget(Clear, area);
    let visible_lines = area.height.saturating_sub(2).max(1) as usize;
    let content_width = area.width.saturating_sub(2) as usize;
    let title = format!(
        "{} | {} {}/{} | wrap:{}",
        viewer.path.to_string_lossy(),
        if viewer.hex_mode { "row" } else { "line" },
        viewer.current_line_number(),
        viewer.line_count(),
        if viewer.wrap { "on" } else { "off" }
    );
    let content = if viewer.hex_mode {
        hex_viewer_window(viewer, visible_lines, content_width)
    } else {
        highlighted_viewer_window(viewer, visible_lines)
            .unwrap_or_else(|| plain_viewer_window(viewer, visible_lines, content_width))
    };
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

fn highlighted_viewer_window(viewer: &ViewerState, visible_lines: usize) -> Option<Text<'static>> {
    let resources = HIGHLIGHT_RESOURCES.get_or_init(build_highlight_resources);
    let resources: &'static HighlightResources = resources.as_ref()?;
    let cache_lock = viewer_highlight_cache();
    let mut cache_guard = cache_lock.lock().ok()?;
    let key = viewer_highlight_key(viewer);

    if cache_guard.as_ref().is_none_or(|cached| cached.key != key) {
        *cache_guard = Some(CachedViewerHighlight::new(viewer, resources));
    }
    let cache = cache_guard.as_mut()?;
    let total_lines = cache.raw_lines.len();
    if total_lines == 0 {
        return Some(Text::raw(String::new()));
    }

    let start = viewer.scroll.min(total_lines.saturating_sub(1));
    let end = start.saturating_add(visible_lines.max(1)).min(total_lines);
    cache
        .ensure_highlighted_up_to(end, &resources.syntax_set)
        .ok()?;

    Some(Text::from(cache.highlighted_lines[start..end].to_vec()))
}

fn build_highlight_resources() -> Option<HighlightResources> {
    let syntax_set = SyntaxSet::load_defaults_newlines();
    let themes = syntect::highlighting::ThemeSet::load_defaults();
    let mut theme = themes
        .themes
        .get("base16-ocean.dark")
        .cloned()
        .or_else(|| themes.themes.values().next().cloned())?;

    let skin = current_skin();
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

    Some(HighlightResources { syntax_set, theme })
}

fn viewer_syntax<'a>(syntax_set: &'a SyntaxSet, viewer: &ViewerState) -> &'a SyntaxReference {
    syntax_set
        .find_syntax_for_file(&viewer.path)
        .ok()
        .flatten()
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text())
}

fn viewer_theme_surface_style() -> Option<Style> {
    let resources = HIGHLIGHT_RESOURCES.get_or_init(build_highlight_resources);
    let resources = resources.as_ref()?;
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
        content_ptr: viewer.content.as_ptr() as usize,
        content_len: viewer.content.len(),
    }
}

fn viewer_highlight_cache() -> &'static Mutex<Option<CachedViewerHighlight>> {
    VIEWER_HIGHLIGHT_CACHE.get_or_init(|| Mutex::new(None))
}

impl CachedViewerHighlight {
    fn new(viewer: &ViewerState, resources: &'static HighlightResources) -> Self {
        let syntax = viewer_syntax(&resources.syntax_set, viewer);
        let mut raw_lines: Vec<String> = viewer.content.lines().map(sanitize_text_line).collect();
        if raw_lines.is_empty() {
            raw_lines.push(String::new());
        }

        Self {
            key: viewer_highlight_key(viewer),
            raw_lines,
            highlighted_lines: Vec::new(),
            highlighter: HighlightLines::new(syntax, &resources.theme),
        }
    }

    fn ensure_highlighted_up_to(&mut self, end: usize, syntax_set: &SyntaxSet) -> Result<(), ()> {
        while self.highlighted_lines.len() < end {
            let index = self.highlighted_lines.len();
            let raw_line = self.raw_lines.get(index).ok_or(())?;
            let ranges = self
                .highlighter
                .highlight_line(raw_line.as_str(), syntax_set)
                .map_err(|_| ())?;
            let spans: Vec<Span<'static>> = ranges
                .into_iter()
                .map(|(style, text)| Span::styled(text.to_string(), syntect_style(style)))
                .collect();
            self.highlighted_lines.push(Line::from(spans));
        }
        Ok(())
    }
}

fn plain_viewer_window(viewer: &ViewerState, visible_lines: usize, width: usize) -> Text<'static> {
    let mut raw_lines: Vec<&str> = viewer.content.lines().collect();
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
    let area = centered_rect(frame.area(), 56, 14);
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
        DialogKind::Listbox(listbox) => {
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
                .constraints([Constraint::Min(1), Constraint::Length(1)])
                .split(inner);

            let items: Vec<ListItem<'_>> = if listbox.items.is_empty() {
                vec![ListItem::new("<empty>")]
            } else {
                listbox
                    .items
                    .iter()
                    .map(|item| ListItem::new(item.as_str()))
                    .collect()
            };
            let list = List::new(items)
                .style(skin.style("dialog", "_default_"))
                .highlight_style(skin.style("dialog", "dfocus"))
                .highlight_symbol(">> ");

            let mut state = ListState::default();
            if !listbox.items.is_empty() {
                state.select(Some(listbox.selected));
            }
            frame.render_stateful_widget(list, layout[0], &mut state);

            frame.render_widget(
                Paragraph::new("Up/Down move | Enter accept | Esc cancel")
                    .style(skin.style("core", "disabled")),
                layout[1],
            );
        }
    }
}

fn render_jobs_screen(frame: &mut Frame, state: &AppState, skin: &UiSkin) {
    let area = centered_rect(frame.area(), 92, 24);
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
            .title("Up/Down select | Alt-J cancel | Esc/q close"),
    );

    let mut table_state = TableState::default();
    if !state.jobs.jobs().is_empty() {
        table_state.select(Some(state.jobs_cursor));
    }
    frame.render_stateful_widget(table, inner, &mut table_state);
}

fn render_find_results_screen(frame: &mut Frame, results: &FindResultsState, skin: &UiSkin) {
    let area = centered_rect(frame.area(), 96, 28);
    frame.render_widget(Clear, area);

    let title = format!(
        "Find results: '{}' ({}){}",
        results.query,
        results.entries.len(),
        if results.loading { " | loading..." } else { "" }
    );
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
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let root = format!("Root: {}", results.base_dir.to_string_lossy());
    frame.render_widget(
        Paragraph::new(root).style(skin.style("dialog", "_default_")),
        layout[0],
    );

    let items: Vec<ListItem<'_>> = if results.entries.is_empty() {
        if results.loading {
            vec![ListItem::new("<searching...>")]
        } else {
            vec![ListItem::new("<no matches>")]
        }
    } else {
        let viewport_rows = layout[1].height.max(1) as usize;
        let (window_start, window_end) =
            visible_window(results.entries.len(), results.cursor, viewport_rows);
        results
            .entries
            .iter()
            .skip(window_start)
            .take(window_end.saturating_sub(window_start))
            .map(|entry| {
                let mut label = entry.path.to_string_lossy().into_owned();
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
        let viewport_rows = layout[1].height.max(1) as usize;
        let (window_start, window_end) =
            visible_window(results.entries.len(), results.cursor, viewport_rows);
        let selected_row = results
            .cursor
            .saturating_sub(window_start)
            .min(window_end.saturating_sub(window_start).saturating_sub(1));
        state.select(Some(selected_row));
    }
    frame.render_stateful_widget(list, layout[1], &mut state);

    frame.render_widget(
        Paragraph::new(
            "Enter locate | F5 panelize | Up/Down move | PgUp/PgDn | Home/End | Alt-J cancel | Esc/q close",
        )
        .style(skin.style("core", "disabled")),
        layout[2],
    );
}

fn render_tree_screen(frame: &mut Frame, tree: &TreeState, skin: &UiSkin) {
    let area = centered_rect(frame.area(), 88, 28);
    frame.render_widget(Clear, area);

    let title = format!(
        "Directory tree ({}){}",
        tree.entries.len(),
        if tree.loading { " | loading..." } else { "" }
    );
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
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let root = format!("Root: {}", tree.root.to_string_lossy());
    frame.render_widget(
        Paragraph::new(root).style(skin.style("dialog", "_default_")),
        layout[0],
    );

    let items: Vec<ListItem<'_>> = if tree.entries.is_empty() {
        vec![ListItem::new("<empty tree>")]
    } else {
        let viewport_rows = layout[1].height.max(1) as usize;
        let (window_start, window_end) =
            visible_window(tree.entries.len(), tree.cursor, viewport_rows);
        tree.entries
            .iter()
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
    if !tree.entries.is_empty() {
        let viewport_rows = layout[1].height.max(1) as usize;
        let (window_start, window_end) =
            visible_window(tree.entries.len(), tree.cursor, viewport_rows);
        let selected_row = tree
            .cursor
            .saturating_sub(window_start)
            .min(window_end.saturating_sub(window_start).saturating_sub(1));
        state.select(Some(selected_row));
    }
    frame.render_stateful_widget(list, layout[1], &mut state);

    frame.render_widget(
        Paragraph::new("Enter open | Up/Down move | PgUp/PgDn | Home/End | Esc/q close")
            .style(skin.style("core", "disabled")),
        layout[2],
    );
}

fn render_hotlist_screen(frame: &mut Frame, app: &AppState, skin: &UiSkin) {
    let area = centered_rect(frame.area(), 88, 22);
    frame.render_widget(Clear, area);

    let title = format!("Directory hotlist ({})", app.hotlist.len());
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

    let items: Vec<ListItem<'_>> = if app.hotlist.is_empty() {
        vec![ListItem::new("<empty hotlist>")]
    } else {
        let viewport_rows = layout[0].height.max(1) as usize;
        let (window_start, window_end) =
            visible_window(app.hotlist.len(), app.hotlist_cursor, viewport_rows);
        app.hotlist
            .iter()
            .skip(window_start)
            .take(window_end.saturating_sub(window_start))
            .map(|path| ListItem::new(path.to_string_lossy().into_owned()))
            .collect()
    };
    let list = List::new(items)
        .style(skin.style("dialog", "_default_"))
        .highlight_style(skin.style("core", "selected"))
        .highlight_symbol(">> ");

    let mut state = ListState::default();
    if !app.hotlist.is_empty() {
        let viewport_rows = layout[0].height.max(1) as usize;
        let (window_start, window_end) =
            visible_window(app.hotlist.len(), app.hotlist_cursor, viewport_rows);
        let selected_row = app
            .hotlist_cursor
            .saturating_sub(window_start)
            .min(window_end.saturating_sub(window_start).saturating_sub(1));
        state.select(Some(selected_row));
    }
    frame.render_stateful_widget(list, layout[0], &mut state);

    frame.render_widget(
        Paragraph::new("Enter open | a add current dir | d/delete remove | Esc/q close")
            .style(skin.style("core", "disabled")),
        layout[1],
    );
}

fn render_menu_overlay(frame: &mut Frame, menu: &MenuState, skin: &UiSkin) {
    let area = frame.area();
    if area.height <= 2 {
        return;
    }

    let popup_x = menu.popup_origin_x().min(area.width.saturating_sub(1));
    let popup_y = 1u16;
    let popup_width = menu.popup_width().min(area.width.saturating_sub(popup_x));
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

            if entry.shortcut.is_empty() {
                return ListItem::new(entry.label);
            }

            let label_width = entry.label.chars().count();
            let shortcut_width = entry.shortcut.chars().count();
            let spacing = content_width
                .saturating_sub(label_width.saturating_add(shortcut_width))
                .max(1);
            ListItem::new(format!(
                "{}{}{}",
                entry.label,
                " ".repeat(spacing),
                entry.shortcut
            ))
        })
        .collect();
    let list = List::new(items)
        .style(skin.style("menu", "_default_"))
        .highlight_style(skin.style("dialog", "dfocus"))
        .highlight_symbol(" ");
    let mut state = ListState::default();
    if !menu.active_entries().is_empty() {
        state.select(Some(menu.selected_entry));
    }
    frame.render_stateful_widget(list, inner, &mut state);
}

fn render_help_screen(frame: &mut Frame, help: &HelpState, skin: &UiSkin) {
    let area = centered_rect(frame.area(), 116, 36);
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

    frame.render_widget(
        Paragraph::new("Tab/Shift-Tab link | Enter follow | F2/c index | F3/Left back | n/p node | Esc/F10 close")
            .style(skin.style("core", "disabled")),
        layout[1],
    );
}

fn panel_entry_size_label(entry: &FileEntry) -> String {
    if entry.is_parent {
        return String::from("UP--DIR");
    }
    format_human_size_compact(entry.size)
}

fn panel_selected_totals(panel: &PanelState) -> (usize, u64) {
    if panel.tagged_count() == 0 {
        return (0, 0);
    }

    let mut count = 0usize;
    let mut size = 0u64;

    for entry in &panel.entries {
        if entry.is_parent || !panel.is_tagged(&entry.path) {
            continue;
        }
        count = count.saturating_add(1);
        size = size.saturating_add(entry.size);
    }

    (count, size)
}

fn panel_disk_summary(panel: &PanelState) -> String {
    let now = Instant::now();
    let cache = DISK_USAGE_SUMMARY_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut cache) = cache.lock() {
        if let Some((captured_at, cached)) = cache.get(panel.cwd.as_path())
            && now.saturating_duration_since(*captured_at) <= DISK_USAGE_CACHE_TTL
        {
            return cached.clone();
        }

        let summary = compute_disk_summary(panel.cwd.as_path());
        if cache.len() >= DISK_USAGE_CACHE_MAX_ENTRIES {
            cache.retain(|_, (captured_at, _)| {
                now.saturating_duration_since(*captured_at) <= DISK_USAGE_CACHE_TTL
            });
            if cache.len() >= DISK_USAGE_CACHE_MAX_ENTRIES
                && let Some(path) = cache.keys().next().cloned()
            {
                cache.remove(path.as_path());
            }
        }
        cache.insert(panel.cwd.clone(), (now, summary.clone()));
        return summary;
    }

    compute_disk_summary(panel.cwd.as_path())
}

fn compute_disk_summary(path: &Path) -> String {
    let Some((free, total)) = disk_usage(path) else {
        return String::from("- / - (-%)");
    };
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

fn visible_window(total: usize, cursor: usize, viewport_rows: usize) -> (usize, usize) {
    if total == 0 || viewport_rows == 0 {
        return (0, 0);
    }

    let visible = viewport_rows.min(total);
    let mut start = cursor.saturating_sub(visible / 2);
    if start + visible > total {
        start = total.saturating_sub(visible);
    }
    let end = start + visible;
    (start, end)
}

#[cfg(unix)]
fn disk_usage(path: &Path) -> Option<(u64, u64)> {
    let stats = statvfs(path).ok()?;
    let fragment_size = stats.fragment_size() as u64;
    if fragment_size == 0 {
        return None;
    }

    let total = bytes_from_blocks(stats.blocks() as u64, fragment_size);
    let free = bytes_from_blocks(stats.blocks_available() as u64, fragment_size);
    Some((free, total))
}

#[cfg(not(unix))]
fn disk_usage(_path: &Path) -> Option<(u64, u64)> {
    None
}

fn bytes_from_blocks(blocks: u64, block_size: u64) -> u64 {
    ((blocks as u128).saturating_mul(block_size as u128)).min(u64::MAX as u128) as u64
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
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(width),
            Constraint::Fill(1),
        ])
        .split(area);

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(height),
            Constraint::Fill(1),
        ])
        .split(horizontal[1]);

    vertical[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use rc_core::{AppCommand, AppState, BackgroundCommand, run_background_worker};
    use std::env;
    use std::fs;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn render_to_text(state: &AppState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test backend should initialize");
        terminal
            .draw(|frame| render(frame, state))
            .expect("render should succeed");
        let buffer = terminal.backend().buffer();
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

    fn drain_background(state: &mut AppState) {
        loop {
            let commands = state.take_pending_background_commands();
            if commands.is_empty() {
                break;
            }

            let (command_tx, command_rx) = mpsc::channel();
            let (event_tx, event_rx) = mpsc::channel();
            let handle = thread::spawn(move || run_background_worker(command_rx, event_tx));

            for command in commands {
                command_tx
                    .send(command)
                    .expect("background command should send");
                let event = event_rx
                    .recv_timeout(Duration::from_secs(1))
                    .expect("background event should arrive");
                state.handle_background_event(event);
            }
            command_tx
                .send(BackgroundCommand::Shutdown)
                .expect("background shutdown should send");
            handle
                .join()
                .expect("background worker should shut down cleanly");
        }
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
        let app = AppState::new(root.clone()).expect("app should initialize");
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

    #[cfg(unix)]
    #[test]
    fn panel_title_marks_panelize_panels() {
        let root = temp_root("panelize-title");
        fs::write(root.join("entry.txt"), "demo").expect("file should be creatable");
        let mut app = AppState::new(root.clone()).expect("app should initialize");
        app.apply(AppCommand::OpenPanelizeDialog)
            .expect("panelize dialog should open");
        app.apply(AppCommand::DialogAccept)
            .expect("default panelize preset should run");
        drain_background(&mut app);

        let title = panel_title(app.active_panel());
        assert!(
            title.contains("panelize"),
            "panel title should indicate panelize mode"
        );

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

        let mut app = AppState::new(root.clone()).expect("app should initialize");
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
}
