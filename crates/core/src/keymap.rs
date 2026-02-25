use std::collections::HashMap;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum KeyContext {
    FileManager,
    FileManagerXMap,
    Help,
    Jobs,
    FindResults,
    Tree,
    Hotlist,
    Dialog,
    Input,
    Listbox,
    Menu,
    Editor,
    Viewer,
    ViewerHex,
    DiffViewer,
}

impl KeyContext {
    fn from_section(section_name: &str) -> Option<Self> {
        let normalized = section_name.trim().to_ascii_lowercase();
        if normalized == "viewer:hex" {
            return Some(Self::ViewerHex);
        }
        if normalized == "filemanager:xmap" || normalized == "panel:xmap" {
            return Some(Self::FileManagerXMap);
        }

        let base = section_name
            .split(':')
            .next()
            .unwrap_or(section_name)
            .trim()
            .to_ascii_lowercase();

        match base.as_str() {
            "filemanager" | "panel" => Some(Self::FileManager),
            "help" => Some(Self::Help),
            "jobs" => Some(Self::Jobs),
            "find" | "findresults" => Some(Self::FindResults),
            "tree" => Some(Self::Tree),
            "hotlist" => Some(Self::Hotlist),
            "dialog" => Some(Self::Dialog),
            "input" => Some(Self::Input),
            "listbox" => Some(Self::Listbox),
            "menu" => Some(Self::Menu),
            "editor" => Some(Self::Editor),
            "viewer" => Some(Self::Viewer),
            "diffviewer" => Some(Self::DiffViewer),
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
    OpenHelp,
    OpenMenu,
    Quit,
    PanelOther,
    EnterXMap,
    CursorLeft,
    CursorRight,
    CursorUp,
    CursorDown,
    PageUp,
    PageDown,
    Home,
    End,
    OpenEntry,
    CdUp,
    Reread,
    ToggleTag,
    InvertTags,
    SortNext,
    SortReverse,
    Copy,
    Move,
    Delete,
    CancelJob,
    OpenJobs,
    CloseJobs,
    OpenFindDialog,
    OpenTree,
    OpenHotlist,
    OpenPanelizeDialog,
    AddHotlist,
    RemoveHotlist,
    OpenConfirmDialog,
    OpenInputDialog,
    OpenListboxDialog,
    OpenSkinDialog,
    HelpIndex,
    HelpBack,
    HelpLinkNext,
    HelpLinkPrev,
    HelpNodeNext,
    HelpNodePrev,
    HelpHalfPageDown,
    HelpHalfPageUp,
    Search,
    SearchBackward,
    SearchContinue,
    SearchContinueBackward,
    Goto,
    ToggleWrap,
    ToggleHex,
    DialogAccept,
    DialogCancel,
    DialogFocusNext,
    DialogBackspace,
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
            "help" => Self::OpenHelp,
            "menu" | "openmenu" | "pulldown" => Self::OpenMenu,
            "usermenu" => Self::OpenMenu,
            "quit" => Self::Quit,
            "panelother" => Self::PanelOther,
            "extendedkeymap" => Self::EnterXMap,
            "left" => Self::CursorLeft,
            "right" => Self::CursorRight,
            "up" => Self::CursorUp,
            "down" => Self::CursorDown,
            "pageup" | "pgup" => Self::PageUp,
            "pagedown" | "pgdn" => Self::PageDown,
            "halfpagedown" => Self::HelpHalfPageDown,
            "halfpageup" => Self::HelpHalfPageUp,
            "home" | "top" => Self::Home,
            "end" | "bottom" => Self::End,
            "enter" | "view" | "viewfile" => Self::OpenEntry,
            "edit" => Self::OpenEntry,
            "cdup" => Self::CdUp,
            "reread" => Self::Reread,
            "toggletag" | "mark" => Self::ToggleTag,
            "inverttags" | "markinverse" => Self::InvertTags,
            "sortnext" => Self::SortNext,
            "sortreverse" => Self::SortReverse,
            "copy" | "filecopy" => Self::Copy,
            "move" | "renmov" | "rename" => Self::Move,
            "delete" | "filedelete" | "remove" => Self::Delete,
            "canceljob" | "jobcancel" => Self::CancelJob,
            "openjobs" | "jobsopen" => Self::OpenJobs,
            "jobs" => Self::OpenJobs,
            "closejobs" | "jobsclose" => Self::CloseJobs,
            "find" | "findfile" | "openfind" | "openfinddialog" => Self::OpenFindDialog,
            "tree" | "directorytree" | "opentree" => Self::OpenTree,
            "hotlist" | "directoryhotlist" | "openhotlist" => Self::OpenHotlist,
            "panelize" | "externalpanelize" | "openpanelize" | "openpanelizedialog" => {
                Self::OpenPanelizeDialog
            }
            "addhotlist" | "hotlistadd" => Self::AddHotlist,
            "removehotlist" | "hotlistremove" | "deletehotlist" => Self::RemoveHotlist,
            "openconfirmdialog" | "democonfirmdialog" => Self::OpenConfirmDialog,
            "openinputdialog" | "demoinputdialog" | "makedir" | "mkdir" => Self::OpenInputDialog,
            "openlistboxdialog" | "demolistboxdialog" => Self::OpenListboxDialog,
            "openskindialog" | "skin" | "skins" => Self::OpenSkinDialog,
            "index" => Self::HelpIndex,
            "back" => Self::HelpBack,
            "linknext" => Self::HelpLinkNext,
            "linkprev" => Self::HelpLinkPrev,
            "nodenext" => Self::HelpNodeNext,
            "nodeprev" => Self::HelpNodePrev,
            "search" => Self::Search,
            "searchback" | "searchbackward" | "searchreverse" => Self::SearchBackward,
            "searchcontinue" | "searchnext" => Self::SearchContinue,
            "searchcontinueback" | "searchcontinuebackward" | "searchprev" => {
                Self::SearchContinueBackward
            }
            "goto" => Self::Goto,
            "togglewrap" | "togglewrapmode" | "wrapmode" => Self::ToggleWrap,
            "togglehex" | "togglehexmode" | "hexmode" => Self::ToggleHex,
            "ok" | "dialogaccept" => Self::DialogAccept,
            "cancel" | "dialogcancel" => Self::DialogCancel,
            "focusnext" | "dialogfocusnext" => Self::DialogFocusNext,
            "backspace" | "dialogbackspace" => Self::DialogBackspace,
            _ => Self::Unknown(name.trim().to_string()),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct UnknownAction {
    pub line: usize,
    pub context: KeyContext,
    pub action: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SkippedKeyBinding {
    pub line: usize,
    pub context: KeyContext,
    pub action: String,
    pub key_spec: String,
    pub reason: String,
}

#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct KeymapParseReport {
    pub unknown_actions: Vec<UnknownAction>,
    pub skipped_bindings: Vec<SkippedKeyBinding>,
}

#[derive(Debug, Default)]
pub struct Keymap {
    bindings: HashMap<KeyContext, HashMap<KeyChord, KeyCommand>>,
}

impl Keymap {
    pub fn bundled_mc_default() -> Result<Self, KeymapParseError> {
        let (keymap, _) = Self::bundled_mc_default_with_report()?;
        Ok(keymap)
    }

    pub fn bundled_mc_default_with_report() -> Result<(Self, KeymapParseReport), KeymapParseError> {
        Self::parse_with_report(include_str!("../assets/mc.default.keymap"))
    }

    pub fn parse(source: &str) -> Result<Self, KeymapParseError> {
        let (keymap, _) = Self::parse_with_report(source)?;
        Ok(keymap)
    }

    pub fn parse_with_report(source: &str) -> Result<(Self, KeymapParseReport), KeymapParseError> {
        let mut keymap = Self::default();
        let mut report = KeymapParseReport::default();
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
            if let KeyCommand::Unknown(unknown_action) = command {
                report.unknown_actions.push(UnknownAction {
                    line: line_number,
                    context: current_context,
                    action: unknown_action,
                });
                continue;
            }

            for token in chord_spec.split(';') {
                let token = token.trim();
                if token.is_empty() {
                    continue;
                }
                let chord = match parse_key_token(token) {
                    Ok(chord) => chord,
                    Err(reason) => {
                        report.skipped_bindings.push(SkippedKeyBinding {
                            line: line_number,
                            context: current_context,
                            action: action_name.trim().to_string(),
                            key_spec: token.to_string(),
                            reason,
                        });
                        continue;
                    }
                };
                keymap
                    .bindings
                    .entry(current_context)
                    .or_default()
                    .insert(chord, command.clone());
            }
        }

        Ok((keymap, report))
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
    let parts: Vec<&str> = normalized.split('-').collect();
    if parts.is_empty() {
        return Err(String::from("empty key token"));
    }

    let mut modifiers = KeyModifiers::default();
    let mut index = 0usize;
    loop {
        if index + 1 >= parts.len() {
            break;
        }
        let part = parts[index];
        match part {
            "ctrl" | "control" | "c" => {
                modifiers.ctrl = true;
                index += 1;
            }
            "alt" | "meta" | "m" | "a" => {
                modifiers.alt = true;
                index += 1;
            }
            "shift" | "s" => {
                modifiers.shift = true;
                index += 1;
            }
            _ => break,
        }
    }

    let key_name = parts[index..].join("-");
    if key_name.is_empty() {
        return Err(String::from("missing key name"));
    }
    let code = parse_key_code(key_name)?;
    Ok(KeyChord { code, modifiers })
}

fn parse_key_code(name: String) -> Result<KeyCode, String> {
    let name = name.as_str();
    match name {
        "enter" | "return" => return Ok(KeyCode::Enter),
        "esc" | "escape" => return Ok(KeyCode::Esc),
        "tab" => return Ok(KeyCode::Tab),
        "backtab" => {
            return Ok(KeyCode::Tab);
        }
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
        "question" => return Ok(KeyCode::Char('?')),
        "backslash" => return Ok(KeyCode::Char('\\')),
        "slash" => return Ok(KeyCode::Char('/')),
        "comma" => return Ok(KeyCode::Char(',')),
        "period" | "dot" => return Ok(KeyCode::Char('.')),
        "plus" => return Ok(KeyCode::Char('+')),
        "minus" => return Ok(KeyCode::Char('-')),
        "underscore" => return Ok(KeyCode::Char('_')),
        "equal" => return Ok(KeyCode::Char('=')),
        "semicolon" => return Ok(KeyCode::Char(';')),
        "colon" => return Ok(KeyCode::Char(':')),
        "quote" | "apostrophe" => return Ok(KeyCode::Char('\'')),
        "backquote" | "grave" => return Ok(KeyCode::Char('`')),
        "less" => return Ok(KeyCode::Char('<')),
        "greater" => return Ok(KeyCode::Char('>')),
        "asterisk" => return Ok(KeyCode::Char('*')),
        "exclamation" => return Ok(KeyCode::Char('!')),
        "space" => return Ok(KeyCode::Char(' ')),
        "prime" => return Ok(KeyCode::Char('\'')),
        "kpplus" => return Ok(KeyCode::Char('+')),
        "kpminus" => return Ok(KeyCode::Char('-')),
        "kpmultiply" => return Ok(KeyCode::Char('*')),
        "kpdivide" => return Ok(KeyCode::Char('/')),
        "kpperiod" | "kpdot" => return Ok(KeyCode::Char('.')),
        "kpcomma" => return Ok(KeyCode::Char(',')),
        "kpenter" => return Ok(KeyCode::Enter),
        "kp0" => return Ok(KeyCode::Char('0')),
        "kp1" => return Ok(KeyCode::Char('1')),
        "kp2" => return Ok(KeyCode::Char('2')),
        "kp3" => return Ok(KeyCode::Char('3')),
        "kp4" => return Ok(KeyCode::Char('4')),
        "kp5" => return Ok(KeyCode::Char('5')),
        "kp6" => return Ok(KeyCode::Char('6')),
        "kp7" => return Ok(KeyCode::Char('7')),
        "kp8" => return Ok(KeyCode::Char('8')),
        "kp9" => return Ok(KeyCode::Char('9')),
        "kphome" => return Ok(KeyCode::Home),
        "kpend" => return Ok(KeyCode::End),
        "kpup" => return Ok(KeyCode::Up),
        "kpdown" => return Ok(KeyCode::Down),
        "kpleft" => return Ok(KeyCode::Left),
        "kpright" => return Ok(KeyCode::Right),
        "kppgup" => return Ok(KeyCode::PageUp),
        "kppgdn" => return Ok(KeyCode::PageDown),
        "kpinsert" => return Ok(KeyCode::Insert),
        "kpdelete" => return Ok(KeyCode::Delete),
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
FocusNext = tab
"#;

        let keymap = Keymap::parse(source).expect("keymap should parse");
        let tab = KeyChord::new(KeyCode::Tab);

        assert_eq!(
            keymap.resolve(KeyContext::FileManager, tab),
            Some(&KeyCommand::PanelOther)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Dialog, tab),
            Some(&KeyCommand::DialogFocusNext),
        );
    }

    #[test]
    fn parser_supports_jobs_context_bindings() {
        let source = r#"
[jobs]
Up = up
CloseJobs = esc
"#;

        let keymap = Keymap::parse(source).expect("keymap should parse");
        assert_eq!(
            keymap.resolve(KeyContext::Jobs, KeyChord::new(KeyCode::Up)),
            Some(&KeyCommand::CursorUp)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Jobs, KeyChord::new(KeyCode::Esc)),
            Some(&KeyCommand::CloseJobs)
        );
    }

    #[test]
    fn parser_supports_find_tree_and_hotlist_contexts() {
        let source = r#"
[findresults]
Up = up
Quit = esc
Panelize = f5

[tree]
Down = down
Quit = q

[hotlist]
OpenHotlist = h
AddHotlist = a
RemoveHotlist = d
"#;

        let keymap = Keymap::parse(source).expect("keymap should parse");
        assert_eq!(
            keymap.resolve(KeyContext::FindResults, KeyChord::new(KeyCode::Up)),
            Some(&KeyCommand::CursorUp)
        );
        assert_eq!(
            keymap.resolve(KeyContext::FindResults, KeyChord::new(KeyCode::Esc)),
            Some(&KeyCommand::Quit)
        );
        assert_eq!(
            keymap.resolve(KeyContext::FindResults, KeyChord::new(KeyCode::F(5))),
            Some(&KeyCommand::OpenPanelizeDialog)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Tree, KeyChord::new(KeyCode::Down)),
            Some(&KeyCommand::CursorDown)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Tree, KeyChord::new(KeyCode::Char('q'))),
            Some(&KeyCommand::Quit)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Hotlist, KeyChord::new(KeyCode::Char('h'))),
            Some(&KeyCommand::OpenHotlist)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Hotlist, KeyChord::new(KeyCode::Char('a'))),
            Some(&KeyCommand::AddHotlist)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Hotlist, KeyChord::new(KeyCode::Char('d'))),
            Some(&KeyCommand::RemoveHotlist)
        );
    }

    #[test]
    fn parser_supports_help_context_bindings() {
        let source = r#"
[help]
Help = f1
Index = f2
Back = f3
LinkNext = tab
LinkPrev = s-tab
NodeNext = n
NodePrev = p
HalfPageDown = d
HalfPageUp = u
Top = g
Bottom = s-g
"#;

        let keymap = Keymap::parse(source).expect("keymap should parse");
        assert_eq!(
            keymap.resolve(KeyContext::Help, KeyChord::new(KeyCode::F(1))),
            Some(&KeyCommand::OpenHelp)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Help, KeyChord::new(KeyCode::F(2))),
            Some(&KeyCommand::HelpIndex)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Help, KeyChord::new(KeyCode::F(3))),
            Some(&KeyCommand::HelpBack)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Help, KeyChord::new(KeyCode::Tab)),
            Some(&KeyCommand::HelpLinkNext)
        );
        assert_eq!(
            keymap.resolve(
                KeyContext::Help,
                KeyChord {
                    code: KeyCode::Tab,
                    modifiers: KeyModifiers {
                        ctrl: false,
                        alt: false,
                        shift: true,
                    },
                },
            ),
            Some(&KeyCommand::HelpLinkPrev)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Help, KeyChord::new(KeyCode::Char('n'))),
            Some(&KeyCommand::HelpNodeNext)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Help, KeyChord::new(KeyCode::Char('p'))),
            Some(&KeyCommand::HelpNodePrev)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Help, KeyChord::new(KeyCode::Char('d'))),
            Some(&KeyCommand::HelpHalfPageDown)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Help, KeyChord::new(KeyCode::Char('u'))),
            Some(&KeyCommand::HelpHalfPageUp)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Help, KeyChord::new(KeyCode::Char('g'))),
            Some(&KeyCommand::Home)
        );
        assert_eq!(
            keymap.resolve(
                KeyContext::Help,
                KeyChord {
                    code: KeyCode::Char('g'),
                    modifiers: KeyModifiers {
                        ctrl: false,
                        alt: false,
                        shift: true,
                    },
                },
            ),
            Some(&KeyCommand::End)
        );
    }

    #[test]
    fn parser_supports_menu_context_bindings() {
        let source = r#"
[filemanager]
Menu = f9

[menu]
Up = up
Down = down
Left = left
Right = right
Ok = enter
Cancel = esc
Quit = f10
"#;

        let keymap = Keymap::parse(source).expect("keymap should parse");
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, KeyChord::new(KeyCode::F(9))),
            Some(&KeyCommand::OpenMenu)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Menu, KeyChord::new(KeyCode::Up)),
            Some(&KeyCommand::CursorUp)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Menu, KeyChord::new(KeyCode::Down)),
            Some(&KeyCommand::CursorDown)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Menu, KeyChord::new(KeyCode::Left)),
            Some(&KeyCommand::CursorLeft)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Menu, KeyChord::new(KeyCode::Right)),
            Some(&KeyCommand::CursorRight)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Menu, KeyChord::new(KeyCode::Enter)),
            Some(&KeyCommand::DialogAccept)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Menu, KeyChord::new(KeyCode::Esc)),
            Some(&KeyCommand::DialogCancel)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Menu, KeyChord::new(KeyCode::F(10))),
            Some(&KeyCommand::Quit)
        );
    }

    #[test]
    fn parser_supports_editor_viewer_and_diffviewer_contexts() {
        let source = r#"
[editor]
Up = up

[viewer]
PageDown = pgdn

[viewer:hex]
Home = home

[diffviewer]
End = end
"#;

        let keymap = Keymap::parse(source).expect("keymap should parse");
        assert_eq!(
            keymap.resolve(KeyContext::Editor, KeyChord::new(KeyCode::Up)),
            Some(&KeyCommand::CursorUp)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Viewer, KeyChord::new(KeyCode::PageDown)),
            Some(&KeyCommand::PageDown)
        );
        assert_eq!(
            keymap.resolve(KeyContext::ViewerHex, KeyChord::new(KeyCode::Home)),
            Some(&KeyCommand::Home)
        );
        assert_eq!(
            keymap.resolve(KeyContext::DiffViewer, KeyChord::new(KeyCode::End)),
            Some(&KeyCommand::End)
        );
    }

    #[test]
    fn parser_maps_viewer_specific_actions() {
        let source = r#"
[viewer]
Search = f7
SearchBackward = s-f7
SearchContinue = n
SearchContinueBackward = s-n
Goto = g
ToggleWrap = w
ToggleHex = h
"#;

        let keymap = Keymap::parse(source).expect("keymap should parse");
        assert_eq!(
            keymap.resolve(KeyContext::Viewer, KeyChord::new(KeyCode::F(7))),
            Some(&KeyCommand::Search)
        );
        assert_eq!(
            keymap.resolve(
                KeyContext::Viewer,
                KeyChord {
                    code: KeyCode::F(7),
                    modifiers: KeyModifiers {
                        ctrl: false,
                        alt: false,
                        shift: true,
                    },
                },
            ),
            Some(&KeyCommand::SearchBackward)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Viewer, KeyChord::new(KeyCode::Char('n'))),
            Some(&KeyCommand::SearchContinue)
        );
        assert_eq!(
            keymap.resolve(
                KeyContext::Viewer,
                KeyChord {
                    code: KeyCode::Char('n'),
                    modifiers: KeyModifiers {
                        ctrl: false,
                        alt: false,
                        shift: true,
                    },
                },
            ),
            Some(&KeyCommand::SearchContinueBackward)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Viewer, KeyChord::new(KeyCode::Char('g'))),
            Some(&KeyCommand::Goto)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Viewer, KeyChord::new(KeyCode::Char('w'))),
            Some(&KeyCommand::ToggleWrap)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Viewer, KeyChord::new(KeyCode::Char('h'))),
            Some(&KeyCommand::ToggleHex)
        );
    }

    #[test]
    fn parser_maps_keypad_named_keys() {
        let source = r#"
[filemanager]
Up = kpup
Down = kpdown
Home = kphome
End = kpend
PageUp = kppgup
PageDown = kppgdn
Reread = kp1
"#;

        let keymap = Keymap::parse(source).expect("keymap should parse");
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, KeyChord::new(KeyCode::Up)),
            Some(&KeyCommand::CursorUp)
        );
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, KeyChord::new(KeyCode::Down)),
            Some(&KeyCommand::CursorDown)
        );
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, KeyChord::new(KeyCode::Home)),
            Some(&KeyCommand::Home)
        );
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, KeyChord::new(KeyCode::End)),
            Some(&KeyCommand::End)
        );
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, KeyChord::new(KeyCode::PageUp)),
            Some(&KeyCommand::PageUp)
        );
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, KeyChord::new(KeyCode::PageDown)),
            Some(&KeyCommand::PageDown)
        );
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, KeyChord::new(KeyCode::Char('1'))),
            Some(&KeyCommand::Reread)
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

    #[test]
    fn parser_maps_copy_move_delete_and_cancel_actions() {
        let source = r#"
[filemanager]
Copy = f5
RenMov = f6
Delete = f8
CancelJob = alt-j
OpenJobs = f3
"#;

        let keymap = Keymap::parse(source).expect("keymap should parse");
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, KeyChord::new(KeyCode::F(5))),
            Some(&KeyCommand::Copy)
        );
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, KeyChord::new(KeyCode::F(6))),
            Some(&KeyCommand::Move)
        );
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, KeyChord::new(KeyCode::F(8))),
            Some(&KeyCommand::Delete)
        );
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, KeyChord::new(KeyCode::F(3))),
            Some(&KeyCommand::OpenJobs)
        );
        let cancel_job = KeyChord {
            code: KeyCode::Char('j'),
            modifiers: KeyModifiers {
                ctrl: false,
                alt: true,
                shift: false,
            },
        };
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, cancel_job),
            Some(&KeyCommand::CancelJob)
        );
    }

    #[test]
    fn parser_maps_common_mc_filemanager_action_names() {
        let source = r#"
[filemanager]
UserMenu = f2
View = f3
Edit = f4
MakeDir = f7
"#;

        let keymap = Keymap::parse(source).expect("keymap should parse");
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, KeyChord::new(KeyCode::F(2))),
            Some(&KeyCommand::OpenMenu)
        );
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, KeyChord::new(KeyCode::F(3))),
            Some(&KeyCommand::OpenEntry)
        );
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, KeyChord::new(KeyCode::F(4))),
            Some(&KeyCommand::OpenEntry)
        );
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, KeyChord::new(KeyCode::F(7))),
            Some(&KeyCommand::OpenInputDialog)
        );
    }

    #[test]
    fn parser_maps_skin_dialog_action() {
        let source = r#"
[filemanager]
Skin = alt-s
"#;

        let keymap = Keymap::parse(source).expect("keymap should parse");
        let skin = KeyChord {
            code: KeyCode::Char('s'),
            modifiers: KeyModifiers {
                ctrl: false,
                alt: true,
                shift: false,
            },
        };
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, skin),
            Some(&KeyCommand::OpenSkinDialog)
        );
    }

    #[test]
    fn parser_maps_ctrl_slash_find_binding() {
        let source = r#"
[filemanager]
OpenFindDialog = ctrl-slash
"#;

        let keymap = Keymap::parse(source).expect("keymap should parse");
        let chord = KeyChord {
            code: KeyCode::Char('/'),
            modifiers: KeyModifiers {
                ctrl: true,
                alt: false,
                shift: false,
            },
        };
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, chord),
            Some(&KeyCommand::OpenFindDialog)
        );
    }

    #[test]
    fn parser_maps_panelize_action_names() {
        let source = r#"
[filemanager]
OpenPanelizeDialog = alt-p
Panelize = ctrl-p
"#;

        let keymap = Keymap::parse(source).expect("keymap should parse");
        let alt_p = KeyChord {
            code: KeyCode::Char('p'),
            modifiers: KeyModifiers {
                ctrl: false,
                alt: true,
                shift: false,
            },
        };
        let ctrl_p = KeyChord {
            code: KeyCode::Char('p'),
            modifiers: KeyModifiers {
                ctrl: true,
                alt: false,
                shift: false,
            },
        };
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, alt_p),
            Some(&KeyCommand::OpenPanelizeDialog)
        );
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, ctrl_p),
            Some(&KeyCommand::OpenPanelizeDialog)
        );
    }

    #[test]
    fn parser_maps_extended_keymap_and_xmap_commands() {
        let source = r#"
[filemanager]
ExtendedKeyMap = ctrl-x

[filemanager:xmap]
Jobs = j
ExternalPanelize = exclamation
"#;

        let keymap = Keymap::parse(source).expect("keymap should parse");
        let ctrl_x = KeyChord {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers {
                ctrl: true,
                alt: false,
                shift: false,
            },
        };
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, ctrl_x),
            Some(&KeyCommand::EnterXMap)
        );
        assert_eq!(
            keymap.resolve(
                KeyContext::FileManagerXMap,
                KeyChord::new(KeyCode::Char('j'))
            ),
            Some(&KeyCommand::OpenJobs)
        );
        assert_eq!(
            keymap.resolve(
                KeyContext::FileManagerXMap,
                KeyChord::new(KeyCode::Char('!'))
            ),
            Some(&KeyCommand::OpenPanelizeDialog)
        );
    }

    #[test]
    fn parser_reports_unknown_actions_instead_of_failing() {
        let source = r#"
[filemanager]
TotallyUnknownAction = f1
Reread = ctrl-r
"#;

        let (keymap, report) = Keymap::parse_with_report(source).expect("keymap should parse");
        assert_eq!(report.unknown_actions.len(), 1);
        assert_eq!(report.unknown_actions[0].action, "TotallyUnknownAction");
        assert_eq!(
            keymap.resolve(
                KeyContext::FileManager,
                KeyChord {
                    code: KeyCode::Char('r'),
                    modifiers: KeyModifiers {
                        ctrl: true,
                        alt: false,
                        shift: false,
                    }
                }
            ),
            Some(&KeyCommand::Reread)
        );
    }

    #[test]
    fn parser_handles_fixture_with_xmap_and_named_keys() {
        let fixture = include_str!("../assets/mc.default.keymap.fixture");
        let (keymap, report) = Keymap::parse_with_report(fixture).expect("fixture should parse");

        let alt_question = KeyChord {
            code: KeyCode::Char('?'),
            modifiers: KeyModifiers {
                ctrl: false,
                alt: true,
                shift: false,
            },
        };
        let ctrl_backslash = KeyChord {
            code: KeyCode::Char('\\'),
            modifiers: KeyModifiers {
                ctrl: true,
                alt: false,
                shift: false,
            },
        };
        let ctrl_t = KeyChord {
            code: KeyCode::Char('t'),
            modifiers: KeyModifiers {
                ctrl: true,
                alt: false,
                shift: false,
            },
        };

        assert_eq!(
            keymap.resolve(KeyContext::FileManagerXMap, alt_question),
            Some(&KeyCommand::PanelOther)
        );
        assert_eq!(
            keymap.resolve(KeyContext::FileManagerXMap, ctrl_backslash),
            Some(&KeyCommand::Reread)
        );
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, ctrl_t),
            Some(&KeyCommand::ToggleTag)
        );
        assert!(
            report
                .unknown_actions
                .iter()
                .any(|unknown| unknown.action == "UnmappedFutureAction"),
            "fixture should exercise unknown action reporting",
        );
    }

    #[test]
    fn bundled_keymap_includes_upstream_extended_map_binding() {
        let keymap = Keymap::bundled_mc_default().expect("bundled keymap should parse");
        let ctrl_x = KeyChord {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers {
                ctrl: true,
                alt: false,
                shift: false,
            },
        };
        assert_eq!(
            keymap.resolve(KeyContext::FileManager, ctrl_x),
            Some(&KeyCommand::EnterXMap)
        );
    }

    #[test]
    fn bundled_keymap_includes_external_panelize_xmap_binding() {
        let keymap = Keymap::bundled_mc_default().expect("bundled keymap should parse");
        assert_eq!(
            keymap.resolve(
                KeyContext::FileManagerXMap,
                KeyChord::new(KeyCode::Char('!'))
            ),
            Some(&KeyCommand::OpenPanelizeDialog)
        );
    }

    #[test]
    fn bundled_keymap_supports_tab_focus_switch_for_panelize_dialogs() {
        let keymap = Keymap::bundled_mc_default().expect("bundled keymap should parse");
        let tab = KeyChord::new(KeyCode::Tab);

        assert_eq!(
            keymap.resolve(KeyContext::Input, tab),
            Some(&KeyCommand::DialogFocusNext)
        );
        assert_eq!(
            keymap.resolve(KeyContext::Listbox, tab),
            Some(&KeyCommand::DialogFocusNext)
        );
    }

    #[test]
    fn bundled_keymap_no_longer_reports_common_mc_actions_as_unknown() {
        let (_, report) =
            Keymap::bundled_mc_default_with_report().expect("bundled keymap should parse");

        for action in ["UserMenu", "View", "Edit", "MakeDir"] {
            assert!(
                !report
                    .unknown_actions
                    .iter()
                    .any(|unknown| unknown.action.eq_ignore_ascii_case(action)),
                "unexpected unknown action report for {action}",
            );
        }
    }
}
