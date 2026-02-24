#![forbid(unsafe_code)]

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use rc_core::{ActivePanel, AppState, PanelState};

pub fn render(frame: &mut Frame, state: &AppState) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new(Line::from("rc | milestone 0/1 bootstrap")),
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
}

fn render_panel(frame: &mut Frame, area: Rect, panel: &PanelState, active: bool) {
    let border_style = if active {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Gray)
    };

    let title = panel.cwd.to_string_lossy().into_owned();
    let items = if panel.entries.is_empty() {
        vec![ListItem::new("<empty>")]
    } else {
        panel
            .entries
            .iter()
            .map(|entry| {
                if entry.is_parent {
                    ListItem::new("..")
                } else {
                    let suffix = if entry.is_dir { "/" } else { "" };
                    ListItem::new(format!("{}{}", entry.name, suffix))
                }
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
