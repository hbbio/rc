#![forbid(unsafe_code)]

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState, Wrap,
};
use rc_core::{
    ActivePanel, AppState, DialogButtonFocus, DialogKind, DialogState, FindResultsState, JobRecord,
    JobStatus, PanelState, Route, TreeState, ViewerState,
};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style as SyntectStyle, Theme};
use syntect::parsing::{SyntaxReference, SyntaxSet};

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
    let job_counts = state.jobs_status_counts();
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new(Line::from(format!(
            "rc | context: {:?} | routes: {} | jobs q:{} r:{} ok:{} cx:{} err:{}",
            state.key_context(),
            state.route_depth(),
            job_counts.queued,
            job_counts.running,
            job_counts.succeeded,
            job_counts.canceled,
            job_counts.failed
        ))),
        root[0],
    );

    if let Some(viewer) = state.active_viewer() {
        render_viewer(frame, root[1], viewer);
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
        );
        render_panel(
            frame,
            panel_areas[1],
            &state.panels[1],
            state.active_panel == ActivePanel::Right,
        );
    }

    frame.render_widget(
        Paragraph::new(state.status_line.as_str()).style(Style::default().fg(Color::DarkGray)),
        root[2],
    );

    match state.top_route() {
        Route::Dialog(dialog) => render_dialog(frame, dialog),
        Route::Jobs => render_jobs_screen(frame, state),
        Route::Viewer(_) => {}
        Route::FindResults(results) => render_find_results_screen(frame, results),
        Route::Tree(tree) => render_tree_screen(frame, tree),
        Route::Hotlist => render_hotlist_screen(frame, state),
        Route::FileManager => {}
    }
}

fn render_panel(frame: &mut Frame, area: Rect, panel: &PanelState, active: bool) {
    let border_style = if active {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Gray)
    };

    let title = format!(
        "{} | sort:{} | tagged:{}{}",
        panel.cwd.to_string_lossy(),
        panel.sort_label(),
        panel.tagged_count(),
        if panel.loading { " | loading..." } else { "" }
    );
    let items = if panel.entries.is_empty() {
        if panel.loading {
            vec![ListItem::new("<loading...>")]
        } else {
            vec![ListItem::new("<empty>")]
        }
    } else {
        panel
            .entries
            .iter()
            .map(|entry| {
                let tag_marker = if !entry.is_parent && panel.is_tagged(&entry.path) {
                    '*'
                } else {
                    ' '
                };
                let label = if entry.is_parent {
                    String::from("..")
                } else if entry.is_dir {
                    format!("{}/", entry.name)
                } else {
                    entry.name.clone()
                };
                ListItem::new(format!("[{tag_marker}] {label}"))
            })
            .collect()
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .highlight_style(Style::default().add_modifier(Modifier::BOLD))
        .highlight_symbol(">> ");

    let mut list_state = ListState::default();
    if !panel.entries.is_empty() {
        list_state.select(Some(panel.cursor));
    }
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_viewer(frame: &mut Frame, area: Rect, viewer: &ViewerState) {
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
    let surface_style = viewer_theme_surface_style().unwrap_or_default();
    let mut paragraph = Paragraph::new(content)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
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

fn render_dialog(frame: &mut Frame, dialog: &DialogState) {
    let area = centered_rect(frame.area(), 56, 14);
    frame.render_widget(Clear, area);

    match &dialog.kind {
        DialogKind::Confirm(confirm) => {
            let block = Block::default()
                .title(dialog.title.as_str())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow));
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
                Paragraph::new(confirm.message.as_str()).alignment(Alignment::Center),
                layout[0],
            );

            let ok_style = if confirm.focus == DialogButtonFocus::Ok {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            let cancel_style = if confirm.focus == DialogButtonFocus::Cancel {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
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
                    .style(Style::default().fg(Color::DarkGray))
                    .alignment(Alignment::Center),
                layout[2],
            );
        }
        DialogKind::Input(input) => {
            let block = Block::default()
                .title(dialog.title.as_str())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow));
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
                Paragraph::new(input.prompt.as_str()).style(Style::default().fg(Color::Gray)),
                layout[0],
            );

            frame.render_widget(
                Paragraph::new(input.value.as_str()).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Blue)),
                ),
                layout[1],
            );

            frame.render_widget(
                Paragraph::new("Type text | Enter accept | Backspace delete | Esc cancel")
                    .style(Style::default().fg(Color::DarkGray)),
                layout[2],
            );
        }
        DialogKind::Listbox(listbox) => {
            let block = Block::default()
                .title(dialog.title.as_str())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow));
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
                .highlight_style(Style::default().add_modifier(Modifier::BOLD))
                .highlight_symbol(">> ");

            let mut state = ListState::default();
            if !listbox.items.is_empty() {
                state.select(Some(listbox.selected));
            }
            frame.render_stateful_widget(list, layout[0], &mut state);

            frame.render_widget(
                Paragraph::new("Up/Down move | Enter accept | Esc cancel")
                    .style(Style::default().fg(Color::DarkGray)),
                layout[1],
            );
        }
    }
}

fn render_jobs_screen(frame: &mut Frame, state: &AppState) {
    let area = centered_rect(frame.area(), 92, 24);
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title("Jobs")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
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
    .style(Style::default().add_modifier(Modifier::BOLD));

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
    .highlight_style(Style::default().add_modifier(Modifier::BOLD))
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

fn render_find_results_screen(frame: &mut Frame, results: &FindResultsState) {
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
        .border_style(Style::default().fg(Color::Yellow));
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
        Paragraph::new(root).style(Style::default().fg(Color::Gray)),
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
        .highlight_style(Style::default().add_modifier(Modifier::BOLD))
        .highlight_symbol(">> ");
    let mut state = ListState::default();
    if !results.entries.is_empty() {
        state.select(Some(results.cursor));
    }
    frame.render_stateful_widget(list, layout[1], &mut state);

    frame.render_widget(
        Paragraph::new("Enter open | Up/Down move | PgUp/PgDn | Home/End | Esc/q close")
            .style(Style::default().fg(Color::DarkGray)),
        layout[2],
    );
}

fn render_tree_screen(frame: &mut Frame, tree: &TreeState) {
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
        .border_style(Style::default().fg(Color::Yellow));
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
        Paragraph::new(root).style(Style::default().fg(Color::Gray)),
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
        .highlight_style(Style::default().add_modifier(Modifier::BOLD))
        .highlight_symbol(">> ");
    let mut state = ListState::default();
    if !tree.entries.is_empty() {
        state.select(Some(tree.cursor));
    }
    frame.render_stateful_widget(list, layout[1], &mut state);

    frame.render_widget(
        Paragraph::new("Enter open | Up/Down move | PgUp/PgDn | Home/End | Esc/q close")
            .style(Style::default().fg(Color::DarkGray)),
        layout[2],
    );
}

fn render_hotlist_screen(frame: &mut Frame, app: &AppState) {
    let area = centered_rect(frame.area(), 88, 22);
    frame.render_widget(Clear, area);

    let title = format!("Directory hotlist ({})", app.hotlist.len());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
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
        .highlight_style(Style::default().add_modifier(Modifier::BOLD))
        .highlight_symbol(">> ");

    let mut state = ListState::default();
    if !app.hotlist.is_empty() {
        state.select(Some(app.hotlist_cursor));
    }
    frame.render_stateful_widget(list, layout[0], &mut state);

    frame.render_widget(
        Paragraph::new("Enter open | a add current dir | d/delete remove | Esc/q close")
            .style(Style::default().fg(Color::DarkGray)),
        layout[1],
    );
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
