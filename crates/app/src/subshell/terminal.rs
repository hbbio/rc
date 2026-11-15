use std::num::NonZeroU16;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Paragraph};

const MAX_DIMENSION: u16 = 512;
const SCROLLBACK_ROWS: usize = 1_000;
const REPLY_LIMIT: usize = 64 * 1024;

#[derive(Default)]
struct Replies(Vec<u8>);

impl Replies {
    fn push(&mut self, bytes: &[u8]) {
        if self.0.len().saturating_add(bytes.len()) <= REPLY_LIMIT {
            self.0.extend_from_slice(bytes);
        }
    }
}

impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        first: Option<u8>,
        second: Option<u8>,
        params: &[&[u16]],
        code: char,
    ) {
        if second.is_some() {
            return;
        }
        let value = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (first, code, value) {
            (None, 'c', 0) => self.push(b"\x1b[?1;2c"),
            (Some(b'>'), 'c', 0) => self.push(b"\x1b[>0;1;0c"),
            (None, 'n', 5) => self.push(b"\x1b[0n"),
            (None, 'n', 6) | (Some(b'?'), 'n', 6) => {
                let (row, col) = screen.cursor_position();
                let prefix = if first.is_some() { "?" } else { "" };
                self.push(format!("\x1b[{prefix}{};{}R", row + 1, col + 1).as_bytes());
            }
            (Some(b'?'), 'u', 0) => self.push(b"\x1b[?0u"),
            (Some(b'>'), 'q', 0) => self.push(b"\x1bP>|rc\x1b\\"),
            (None, 't', 18) => {
                let (rows, cols) = screen.size();
                self.push(format!("\x1b[8;{rows};{cols}t").as_bytes());
            }
            _ => {}
        }
    }

    fn unhandled_escape(
        &mut self,
        _: &mut vt100::Screen,
        first: Option<u8>,
        second: Option<u8>,
        code: u8,
    ) {
        if first.is_none() && second.is_none() && code == b'Z' {
            self.push(b"\x1b[?1;2c");
        }
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        match params {
            [b"10", b"?"] => self.push(b"\x1b]10;rgb:cccc/cccc/cccc\x1b\\"),
            [b"11", b"?"] => self.push(b"\x1b]11;rgb:0000/0000/0000\x1b\\"),
            _ => {}
        }
    }
}

pub(super) struct TerminalView {
    parser: vt100::Parser<Replies>,
}

pub(super) fn dimensions(width: u16, height: u16) -> (u16, u16) {
    (
        height.saturating_sub(1).clamp(1, MAX_DIMENSION),
        width.clamp(1, MAX_DIMENSION),
    )
}

impl TerminalView {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new_with_callbacks(
                nonzero(rows),
                nonzero(cols),
                SCROLLBACK_ROWS,
                Replies::default(),
            ),
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.set_size(nonzero(rows), nonzero(cols));
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    pub fn fork(&self) -> Self {
        let (rows, cols) = self.screen().size();
        let mut copy = Self::new(rows.get(), cols.get());
        *copy.parser.screen_mut() = self.screen().clone();
        copy
    }

    pub fn align_cursor(&mut self, source: &vt100::Screen) {
        self.parser
            .process(source.attributes_formatted().as_bytes());
        self.parser
            .process(source.input_mode_formatted().as_bytes());
        self.parser
            .process(source.cursor_state_formatted().as_bytes());
    }

    pub fn process(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.parser.process(bytes);
        std::mem::take(&mut self.parser.callbacks_mut().0)
    }

    pub fn show_command(&mut self, command: &str) {
        self.live();
        // Render the submitted command instead of the private transport script.
        let command = command.replace('\x1b', "^[").replace('\r', "^M");
        self.parser.process(command.as_bytes());
        self.parser.process(b"\r\n");
    }

    pub fn scroll(&mut self, rows: isize) {
        let offset = self
            .parser
            .screen()
            .scrollback()
            .saturating_add_signed(rows);
        self.parser.screen_mut().set_scrollback(offset);
    }

    pub fn live(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }

    pub fn render(&self, frame: &mut Frame, footer: &str) {
        let area = frame.area();
        if area.is_empty() {
            return;
        }
        let body = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        frame.render_widget(
            Block::default().style(Style::default().fg(Color::Gray).bg(Color::Black)),
            body,
        );
        let screen = self.screen();
        let (rows, cols) = screen.size();
        for row in 0..body.height.min(rows.get()) {
            for col in 0..body.width.min(cols.get()) {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }
                let mut style = Style::default()
                    .fg(color(cell.fgcolor(), Color::Gray))
                    .bg(color(cell.bgcolor(), Color::Black));
                for (enabled, modifier) in [
                    (cell.bold(), Modifier::BOLD),
                    (cell.dim(), Modifier::DIM),
                    (cell.italic(), Modifier::ITALIC),
                    (cell.underline(), Modifier::UNDERLINED),
                    (cell.inverse(), Modifier::REVERSED),
                ] {
                    if enabled {
                        style = style.add_modifier(modifier);
                    }
                }
                let symbol = if cell.has_contents() {
                    cell.contents()
                } else {
                    " "
                };
                frame.buffer_mut()[(area.x + col, area.y + row)]
                    .set_symbol(symbol)
                    .set_style(style);
            }
        }
        if !screen.hide_cursor() && screen.scrollback() == 0 {
            let (row, col) = screen.cursor_position();
            if row < body.height && col < body.width {
                frame.set_cursor_position((area.x + col, area.y + row));
            }
        }
        let footer_area = Rect::new(area.x, area.bottom() - 1, area.width, 1);
        frame.render_widget(
            Paragraph::new(footer).style(Style::default().fg(Color::White).bg(Color::DarkGray)),
            footer_area,
        );
    }
}

fn nonzero(value: u16) -> NonZeroU16 {
    NonZeroU16::new(value.clamp(1, MAX_DIMENSION)).expect("clamped terminal dimension")
}

fn color(color: vt100::Color, default: Color) -> Color {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Idx(index) => Color::Indexed(index),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn terminal_queries_use_emulated_state_and_survive_fragmented_input() {
        let mut view = TerminalView::new(8, 20);
        view.process(b"abc\x1b[");
        assert_eq!(view.process(b"6n\x1b[0c"), b"\x1b[1;4R\x1b[?1;2c");
        assert_eq!(
            view.process(b"\x1b]11;?\x1b\\"),
            b"\x1b]11;rgb:0000/0000/0000\x1b\\"
        );
        assert!(view.process(b"\x1b]52;c;secret\x07").is_empty());
    }

    #[test]
    fn alternate_screen_and_styles_restore_without_replaying_host_escape_sequences() {
        let mut view = TerminalView::new(4, 16);
        view.process(b"primary\x1b[?1049h\x1b[2J\x1b[H\x1b[31mfull screen");
        let mut terminal = Terminal::new(TestBackend::new(16, 5)).unwrap();
        terminal
            .draw(|frame| view.render(frame, "Ctrl+O panels"))
            .unwrap();
        assert_eq!(terminal.backend().buffer()[(0, 0)].symbol(), "f");
        assert_eq!(terminal.backend().buffer()[(0, 0)].fg, Color::Indexed(1));
        view.process(b"\x1b[?1049l");
        assert!(view.screen().contents().contains("primary"));
        assert!(!view.screen().contents().contains("full screen"));
    }
}
