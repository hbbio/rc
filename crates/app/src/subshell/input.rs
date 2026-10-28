use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

pub(super) fn key_bytes(key: KeyEvent, screen: &vt100::Screen) -> Option<Vec<u8>> {
    if key
        .modifiers
        .intersects(KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META)
    {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let modifier = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let mut bytes = match key.code {
        KeyCode::Char(ch) if ctrl => {
            let byte = match ch {
                ' ' | '@' => 0,
                'a'..='z' | 'A'..='Z' | '['..='_' => (ch as u8) & 0x1f,
                '?' => 127,
                _ => return None,
            };
            vec![byte]
        }
        KeyCode::Char(ch) => ch.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![127],
        KeyCode::Tab if !shift => vec![b'\t'],
        KeyCode::Esc => vec![27],
        KeyCode::Tab | KeyCode::BackTab => return Some(b"\x1b[Z".to_vec()),
        KeyCode::Up
        | KeyCode::Down
        | KeyCode::Right
        | KeyCode::Left
        | KeyCode::Home
        | KeyCode::End => {
            let final_byte = match key.code {
                KeyCode::Up => 'A',
                KeyCode::Down => 'B',
                KeyCode::Right => 'C',
                KeyCode::Left => 'D',
                KeyCode::Home => 'H',
                _ => 'F',
            };
            return Some(if modifier != 1 {
                format!("\x1b[1;{modifier}{final_byte}").into_bytes()
            } else if screen.application_cursor() {
                format!("\x1bO{final_byte}").into_bytes()
            } else {
                format!("\x1b[{final_byte}").into_bytes()
            });
        }
        KeyCode::F(number @ 1..=4) => {
            let final_byte = char::from(b'P' + number - 1);
            return Some(
                if modifier == 1 {
                    format!("\x1bO{final_byte}")
                } else {
                    format!("\x1b[1;{modifier}{final_byte}")
                }
                .into_bytes(),
            );
        }
        KeyCode::Insert
        | KeyCode::Delete
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::F(5..=12) => {
            let number = match key.code {
                KeyCode::Insert => 2,
                KeyCode::Delete => 3,
                KeyCode::PageUp => 5,
                KeyCode::PageDown => 6,
                KeyCode::F(n) => [15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)],
                _ => unreachable!(),
            };
            return Some(
                if modifier == 1 {
                    format!("\x1b[{number}~")
                } else {
                    format!("\x1b[{number};{modifier}~")
                }
                .into_bytes(),
            );
        }
        _ => return None,
    };
    if alt {
        bytes.insert(0, 27);
    }
    Some(bytes)
}

pub(super) fn paste_bytes(text: &str, bracketed: bool) -> Result<Vec<u8>, &'static str> {
    if text.len() > 64 * 1024 - 12 {
        return Err("Paste exceeds 64 KiB");
    }
    let mut bytes = Vec::with_capacity(text.len() + 12);
    if bracketed {
        bytes.extend_from_slice(b"\x1b[200~");
    }
    // ESC in pasted text must not terminate bracketed paste or inject terminal
    // keys. Preserve normal multiline text, tabs and Unicode.
    bytes.extend(
        text.as_bytes()
            .iter()
            .copied()
            .filter(|byte| !matches!(byte, 0 | 27)),
    );
    if bracketed {
        bytes.extend_from_slice(b"\x1b[201~");
    }
    Ok(bytes)
}

pub(super) fn mouse_bytes(event: MouseEvent, screen: &vt100::Screen) -> Option<Vec<u8>> {
    use vt100::MouseProtocolMode as Mode;
    let mode = screen.mouse_protocol_mode();
    if mode == Mode::None {
        return None;
    }
    let button = |button: MouseButton| match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let (mut code, release) = match event.kind {
        MouseEventKind::Down(b) => (button(b), false),
        MouseEventKind::Up(b) if mode != Mode::Press => (button(b), true),
        MouseEventKind::Drag(b) if matches!(mode, Mode::ButtonMotion | Mode::AnyMotion) => {
            (32 + button(b), false)
        }
        MouseEventKind::Moved if mode == Mode::AnyMotion => (35, false),
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
        MouseEventKind::ScrollLeft => (66, false),
        MouseEventKind::ScrollRight => (67, false),
        _ => return None,
    };
    if event.modifiers.contains(KeyModifiers::SHIFT) {
        code += 4;
    }
    if event.modifiers.contains(KeyModifiers::ALT) {
        code += 8;
    }
    if event.modifiers.contains(KeyModifiers::CONTROL) {
        code += 16;
    }
    let (rows, cols) = screen.size();
    if event.column >= cols.get() || event.row >= rows.get() {
        return None;
    }
    let col = event.column + 1;
    let row = event.row + 1;
    match screen.mouse_protocol_encoding() {
        vt100::MouseProtocolEncoding::Sgr => Some(
            format!(
                "\x1b[<{code};{col};{row}{}",
                if release { 'm' } else { 'M' }
            )
            .into_bytes(),
        ),
        encoding => {
            if release {
                code = 3 + (code & 28);
            }
            let mut bytes = b"\x1b[M".to_vec();
            for value in [code + 32, col + 32, row + 32] {
                if encoding == vt100::MouseProtocolEncoding::Default {
                    bytes.push(u8::try_from(value).ok()?);
                } else {
                    let mut encoded = [0; 4];
                    bytes.extend_from_slice(
                        char::from_u32(u32::from(value))?
                            .encode_utf8(&mut encoded)
                            .as_bytes(),
                    );
                }
            }
            Some(bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_input_respects_modes_modifiers_and_paste_boundaries() {
        let mut parser = vt100::Parser::default();
        assert_eq!(
            key_bytes(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                parser.screen()
            ),
            Some(vec![3])
        );
        assert_eq!(
            key_bytes(
                KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL),
                parser.screen()
            ),
            Some(b"\x1b[1;5D".to_vec())
        );
        parser.process(b"\x1b[?1h");
        assert_eq!(
            key_bytes(
                KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
                parser.screen()
            ),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            paste_bytes("a\x1b[201~b\n", true).unwrap(),
            b"\x1b[200~a[201~b\n\x1b[201~"
        );
        assert!(paste_bytes(&"x".repeat(64 * 1024), true).is_err());
        parser.process(b"\x1b[?1000h\x1b[?1006h");
        let event = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 3,
            row: 4,
            modifiers: KeyModifiers::CONTROL,
        };
        assert_eq!(
            mouse_bytes(event, parser.screen()),
            Some(b"\x1b[<16;4;5m".to_vec())
        );
    }
}
