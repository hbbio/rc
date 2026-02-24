use std::collections::HashMap;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum KeyContext {
    FileManager,
    Dialog,
    Input,
    Listbox,
    Menu,
}

impl KeyContext {
    fn from_section(section_name: &str) -> Option<Self> {
        let base = section_name
            .split(':')
            .next()
            .unwrap_or(section_name)
            .trim()
            .to_ascii_lowercase();

        match base.as_str() {
            "filemanager" | "panel" => Some(Self::FileManager),
            "dialog" => Some(Self::Dialog),
            "input" => Some(Self::Input),
            "listbox" => Some(Self::Listbox),
            "menu" => Some(Self::Menu),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Default)]
pub struct KeyModifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum KeyCode {
    Char(char),
    F(u8),
    Enter,
    Esc,
    Tab,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct KeyChord {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyChord {
    pub const fn new(code: KeyCode) -> Self {
        Self {
            code,
            modifiers: KeyModifiers {
                ctrl: false,
                alt: false,
                shift: false,
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeyCommand {
    Quit,
    PanelOther,
    CursorUp,
    CursorDown,
    OpenEntry,
    CdUp,
    Reread,
    Unknown(String),
}

impl KeyCommand {
    fn from_name(name: &str) -> Self {
        let normalized: String = name
            .trim()
            .chars()
            .filter(|ch| !matches!(ch, '_' | '-' | ' '))
            .flat_map(char::to_lowercase)
            .collect();

        match normalized.as_str() {
            "quit" => Self::Quit,
            "panelother" => Self::PanelOther,
            "up" => Self::CursorUp,
            "down" => Self::CursorDown,
            "enter" => Self::OpenEntry,
            "cdup" => Self::CdUp,
            "reread" => Self::Reread,
            _ => Self::Unknown(name.trim().to_string()),
        }
    }
}

#[derive(Debug, Default)]
pub struct Keymap {
    bindings: HashMap<KeyContext, HashMap<KeyChord, KeyCommand>>,
}

impl Keymap {
    pub fn bundled_mc_default() -> Result<Self, KeymapParseError> {
        Self::parse(include_str!("../assets/mc.default.keymap"))
    }

    pub fn parse(source: &str) -> Result<Self, KeymapParseError> {
        let mut keymap = Self::default();
        let mut context: Option<KeyContext> = None;

        for (line_index, raw_line) in source.lines().enumerate() {
            let line_number = line_index + 1;
            let line = raw_line.trim();

            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }

            if line.starts_with('[') && line.ends_with(']') {
                let section = &line[1..line.len() - 1];
                context = KeyContext::from_section(section);
                continue;
            }

            let Some(current_context) = context else {
                continue;
            };
            let Some((action_name, chord_spec)) = line.split_once('=') else {
                continue;
            };

            let command = KeyCommand::from_name(action_name);
            for token in chord_spec.split(';') {
                let token = token.trim();
                if token.is_empty() {
                    continue;
                }
                let chord = parse_key_token(token).map_err(|msg| KeymapParseError {
                    line: line_number,
                    message: msg,
                })?;
                keymap
                    .bindings
                    .entry(current_context)
                    .or_default()
                    .insert(chord, command.clone());
            }
        }

        Ok(keymap)
    }

    pub fn resolve(&self, context: KeyContext, chord: KeyChord) -> Option<&KeyCommand> {
        self.bindings
            .get(&context)
            .and_then(|keys| keys.get(&chord))
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct KeymapParseError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for KeymapParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "failed to parse keymap at line {}: {}",
            self.line, self.message
        )
    }
}

impl std::error::Error for KeymapParseError {}

fn parse_key_token(token: &str) -> Result<KeyChord, String> {
    let normalized = token.trim().to_ascii_lowercase();
    let parts: Vec<&str> = normalized
        .split('-')
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        return Err(String::from("empty key token"));
    }

    let mut modifiers = KeyModifiers::default();
    for modifier in &parts[..parts.len() - 1] {
        match *modifier {
            "ctrl" | "control" => modifiers.ctrl = true,
            "alt" | "meta" => modifiers.alt = true,
            "shift" => modifiers.shift = true,
            other => return Err(format!("unsupported modifier '{other}'")),
        }
    }

    let key_name = parts[parts.len() - 1];
    let code = parse_key_code(key_name)?;
    Ok(KeyChord { code, modifiers })
}

fn parse_key_code(name: &str) -> Result<KeyCode, String> {
    match name {
        "enter" | "return" => return Ok(KeyCode::Enter),
        "esc" | "escape" => return Ok(KeyCode::Esc),
        "tab" => return Ok(KeyCode::Tab),
        "backspace" | "bs" => return Ok(KeyCode::Backspace),
        "up" => return Ok(KeyCode::Up),
        "down" => return Ok(KeyCode::Down),
        "left" => return Ok(KeyCode::Left),
        "right" => return Ok(KeyCode::Right),
        "home" => return Ok(KeyCode::Home),
        "end" => return Ok(KeyCode::End),
        "pgup" | "pageup" => return Ok(KeyCode::PageUp),
        "pgdn" | "pagedown" => return Ok(KeyCode::PageDown),
        "insert" | "ins" => return Ok(KeyCode::Insert),
        "delete" | "del" => return Ok(KeyCode::Delete),
        "space" => return Ok(KeyCode::Char(' ')),
        _ => {}
    }

    if name.len() == 1 {
        let ch = name.chars().next().expect("single-char token");
        return Ok(KeyCode::Char(ch));
    }

    if let Some(rest) = name.strip_prefix('f') {
        let value: u8 = rest
            .parse()
            .map_err(|_| format!("invalid function key '{name}'"))?;
        return Ok(KeyCode::F(value));
    }

    Err(format!("unsupported key '{name}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_supports_multiple_sections_and_context_switching() {
        let source = r#"
[filemanager]
PanelOther = tab

[dialog]
PanelOther = f1
"#;

        let keymap = Keymap::parse(source).expect("keymap should parse");
        let tab = KeyChord::new(KeyCode::Tab);

        assert_eq!(
            keymap.resolve(KeyContext::FileManager, tab),
            Some(&KeyCommand::PanelOther)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Dialog, tab),
            None,
            "same key should resolve differently by context",
        );
    }

    #[test]
    fn parser_reads_modifiers_and_multiple_bindings() {
        let source = r#"
[filemanager]
Reread = ctrl-r; r
"#;

        let keymap = Keymap::parse(source).expect("keymap should parse");
        let reread_ctrl = KeyChord {
            code: KeyCode::Char('r'),
            modifiers: KeyModifiers {
                ctrl: true,
                alt: false,
                shift: false,
            },
        };
        let reread_plain = KeyChord::new(KeyCode::Char('r'));

        assert_eq!(
            keymap.resolve(KeyContext::FileManager, reread_ctrl),
            Some(&KeyCommand::Reread)
        );
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, reread_plain),
            Some(&KeyCommand::Reread)
        );
    }
}
