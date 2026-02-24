#![forbid(unsafe_code)]

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState,
};
use rc_core::{
    ActivePanel, AppState, DialogButtonFocus, DialogKind, DialogState, JobRecord, JobStatus,
    PanelState, Route,
};

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

    frame.render_widget(
        Paragraph::new(state.status_line.as_str()).style(Style::default().fg(Color::DarkGray)),
        root[2],
    );

    match state.top_route() {
        Route::Dialog(dialog) => render_dialog(frame, dialog),
        Route::Jobs => render_jobs_screen(frame, state),
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
        "{} | sort:{} | tagged:{}",
        panel.cwd.to_string_lossy(),
        panel.sort_label(),
        panel.tagged_count()
    );
    let items = if panel.entries.is_empty() {
        vec![ListItem::new("<empty>")]
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
