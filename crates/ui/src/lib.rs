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
    JobRecord, JobStatus, PanelState, Route, TreeState, ViewerState,
};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style as SyntectStyle, Theme};
use syntect::parsing::{SyntaxReference, SyntaxSet};

#[cfg(unix)]
use nix::sys::statvfs::statvfs;

pub use skin::configure_skin;
use skin::{UiSkin, current_skin};

struct HighlightResources {
    syntax_set: SyntaxSet,
    theme: Theme,
}

static HIGHLIGHT_RESOURCES: OnceLock<Option<HighlightResources>> = OnceLock::new();
static VIEWER_HIGHLIGHT_CACHE: OnceLock<Mutex<Option<CachedViewerHighlight>>> = OnceLock::new();

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

    render_menu_bar(frame, root[0], skin.as_ref());

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
    frame.render_widget(
        Paragraph::new(status).style(skin.style("statusbar", "_default_")),
        root[2],
    );
    render_button_bar(frame, root[3], skin.as_ref());

    match state.top_route() {
        Route::Dialog(dialog) => render_dialog(frame, dialog, skin.as_ref()),
        Route::Jobs => render_jobs_screen(frame, state, skin.as_ref()),
        Route::Viewer(_) => {}
        Route::FindResults(results) => render_find_results_screen(frame, results, skin.as_ref()),
        Route::Tree(tree) => render_tree_screen(frame, tree, skin.as_ref()),
        Route::Hotlist => render_hotlist_screen(frame, state, skin.as_ref()),
        Route::FileManager => {}
    }
}

fn render_menu_bar(frame: &mut Frame, area: Rect, skin: &UiSkin) {
    let menu_style = skin.style("menu", "_default_");
    let hot_style = skin.style("menu", "menuhot");
    let items = ["Left", "File", "Command", "Options", "Right"];
    let mut spans: Vec<Span<'_>> = vec![Span::raw(" ")];
    for item in items {
        let mut chars = item.chars();
        let first = chars.next().unwrap_or_default().to_string();
        let rest: String = chars.collect();
        spans.push(Span::styled(first, hot_style));
        spans.push(Span::styled(rest, menu_style));
        spans.push(Span::raw("  "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).style(menu_style), area);
}

fn render_button_bar(frame: &mut Frame, area: Rect, skin: &UiSkin) {
    let hotkey_style = skin.style("buttonbar", "hotkey");
    let button_style = skin.style("buttonbar", "button");
    let labels = [
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
    ];

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

fn render_panel(frame: &mut Frame, area: Rect, panel: &PanelState, active: bool, skin: &UiSkin) {
    let title = format!(
        "{} | sort:{} | tagged:{}{}",
        panel.cwd.to_string_lossy(),
        panel.sort_label(),
        panel.tagged_count(),
        if panel.loading { " | loading..." } else { "" }
    );
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
        let rows: Vec<Row<'_>> = panel
            .entries
            .iter()
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
                    Cell::from(panel_entry_size_label(entry)),
                    Cell::from(format_modified(entry.modified)),
                ])
                .style(entry_style)
            })
            .collect();
        let header = Row::new(vec![
            Cell::from("Name"),
            Cell::from("Size"),
            Cell::from("Modify"),
        ])
        .style(skin.style("core", "header"));

        let table = Table::new(
            rows,
            [
                Constraint::Fill(1),
                Constraint::Length(11),
                Constraint::Length(12),
            ],
        )
        .header(header)
        .style(skin.style("core", "_default_"))
        .highlight_style(highlight_style)
        .column_spacing(1);

        let mut table_state = TableState::default();
        table_state.select(Some(panel.cursor));
        frame.render_stateful_widget(table, panel_layout[0], &mut table_state);
    }

    let (selected_count, selected_size) = panel_selected_totals(panel);
    let selected_summary = format!(
        "{} in {} {}",
        format_bytes(selected_size),
        selected_count,
        if selected_count == 1 { "file" } else { "files" }
    );
    let disk_summary = panel_disk_summary(panel);
    let footer_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Fill(1), Constraint::Length(22)])
        .split(panel_layout[1]);
    frame.render_widget(
        Paragraph::new(selected_summary).style(skin.style("statusbar", "_default_")),
        footer_layout[0],
    );
    frame.render_widget(
        Paragraph::new(disk_summary)
            .style(skin.style("statusbar", "_default_"))
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
    let theme = themes
        .themes
        .get("base16-ocean.dark")
        .cloned()
        .or_else(|| themes.themes.values().next().cloned())?;

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
    let mut ratatui_style = Style::default()
        .fg(Color::Rgb(
            style.foreground.r,
            style.foreground.g,
            style.foreground.b,
        ))
        .bg(Color::Rgb(
            style.background.r,
            style.background.g,
            style.background.b,
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
        results
            .entries
            .iter()
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
        state.select(Some(results.cursor));
    }
    frame.render_stateful_widget(list, layout[1], &mut state);

    frame.render_widget(
        Paragraph::new(
            "Enter locate | Up/Down move | PgUp/PgDn | Home/End | Alt-J cancel | Esc/q close",
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
        tree.entries
            .iter()
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
        state.select(Some(tree.cursor));
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
        app.hotlist
            .iter()
            .map(|path| ListItem::new(path.to_string_lossy().into_owned()))
            .collect()
    };
    let list = List::new(items)
        .style(skin.style("dialog", "_default_"))
        .highlight_style(skin.style("core", "selected"))
        .highlight_symbol(">> ");

    let mut state = ListState::default();
    if !app.hotlist.is_empty() {
        state.select(Some(app.hotlist_cursor));
    }
    frame.render_stateful_widget(list, layout[0], &mut state);

    frame.render_widget(
        Paragraph::new("Enter open | a add current dir | d/delete remove | Esc/q close")
            .style(skin.style("core", "disabled")),
        layout[1],
    );
}

fn panel_entry_size_label(entry: &FileEntry) -> String {
    if entry.is_parent {
        return String::from("UP--DIR");
    }
    format_with_commas(entry.size)
}

fn panel_selected_totals(panel: &PanelState) -> (usize, u64) {
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
    let Some((free, total)) = disk_usage(panel.cwd.as_path()) else {
        return String::from("- / - (-%)");
    };
    if total == 0 {
        return String::from("0B / 0B (0%)");
    }
    let percent = free.saturating_mul(100) / total;
    format!(
        "{} / {} ({}%)",
        format_capacity(free),
        format_capacity(total),
        percent
    )
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
            local.format("%b %e %H:%M").to_string()
        })
        .unwrap_or_default()
}

fn format_bytes(bytes: u64) -> String {
    format!("{} B", format_with_commas(bytes))
}

fn format_with_commas(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

fn format_capacity(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    const TIB: u64 = GIB * 1024;

    if bytes >= TIB {
        format_capacity_unit(bytes, TIB, "T")
    } else if bytes >= GIB {
        format_capacity_unit(bytes, GIB, "G")
    } else if bytes >= MIB {
        format_capacity_unit(bytes, MIB, "M")
    } else if bytes >= KIB {
        format_capacity_unit(bytes, KIB, "K")
    } else {
        format!("{bytes}B")
    }
}

fn format_capacity_unit(bytes: u64, unit: u64, suffix: &str) -> String {
    let value = bytes as f64 / unit as f64;
    if value < 10.0 {
        let rounded = format!("{value:.1}");
        let rounded = rounded.strip_suffix(".0").unwrap_or(&rounded);
        format!("{rounded}{suffix}")
    } else {
        format!("{}{suffix}", value.round() as u64)
    }
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
}
