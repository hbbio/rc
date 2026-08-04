use crate::keymap::KeyContext;
use crate::{FindNameMode, FindSpec};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DialogButtonFocus {
    Ok,
    Cancel,
}

impl DialogButtonFocus {
    pub fn toggle(&mut self) {
        *self = match self {
            Self::Ok => Self::Cancel,
            Self::Cancel => Self::Ok,
        };
    }
}

#[derive(Clone, Debug)]
pub struct ConfirmDialogState {
    pub message: String,
    pub focus: DialogButtonFocus,
}

#[derive(Clone, Debug)]
pub struct InputDialogState {
    pub prompt: String,
    pub value: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PairInputField {
    #[default]
    First,
    Second,
}

impl PairInputField {
    fn toggle(&mut self) {
        *self = match self {
            Self::First => Self::Second,
            Self::Second => Self::First,
        };
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairInputDialogState {
    pub first_prompt: String,
    pub first_value: String,
    pub second_prompt: String,
    pub second_value: String,
    pub focus: PairInputField,
}

impl PairInputDialogState {
    fn focused_value_mut(&mut self) -> &mut String {
        match self.focus {
            PairInputField::First => &mut self.first_value,
            PairInputField::Second => &mut self.second_value,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ListboxDialogState {
    pub items: Vec<String>,
    pub selected: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FindDialogField {
    #[default]
    StartDirectory,
    FilenamePattern,
    NameMode,
    CaseSensitive,
    ContentPattern,
    WholeWord,
    IgnoredDirectories,
}

impl FindDialogField {
    const ALL: [Self; 7] = [
        Self::StartDirectory,
        Self::FilenamePattern,
        Self::NameMode,
        Self::CaseSensitive,
        Self::ContentPattern,
        Self::WholeWord,
        Self::IgnoredDirectories,
    ];

    const fn index(self) -> usize {
        match self {
            Self::StartDirectory => 0,
            Self::FilenamePattern => 1,
            Self::NameMode => 2,
            Self::CaseSensitive => 3,
            Self::ContentPattern => 4,
            Self::WholeWord => 5,
            Self::IgnoredDirectories => 6,
        }
    }

    const fn is_editable(self) -> bool {
        matches!(
            self,
            Self::StartDirectory
                | Self::FilenamePattern
                | Self::ContentPattern
                | Self::IgnoredDirectories
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FindDialogState {
    pub start_directory: String,
    pub filename_pattern: String,
    pub name_mode: FindNameMode,
    pub case_sensitive: bool,
    pub content_pattern: String,
    pub whole_word: bool,
    pub ignored_directories: String,
    pub focus: FindDialogField,
}

impl FindDialogState {
    pub fn from_spec(spec: &FindSpec) -> Self {
        Self {
            start_directory: spec.start_dir.to_string_lossy().into_owned(),
            filename_pattern: spec.filename_pattern.clone(),
            name_mode: spec.name_mode,
            case_sensitive: spec.case_sensitive,
            content_pattern: spec.content_pattern.clone().unwrap_or_default(),
            whole_word: spec.whole_word,
            ignored_directories: spec.ignored_directories.join(", "),
            focus: FindDialogField::FilenamePattern,
        }
    }

    pub fn to_spec(&self) -> FindSpec {
        FindSpec {
            start_dir: self.start_directory.trim().into(),
            filename_pattern: self.filename_pattern.clone(),
            name_mode: self.name_mode,
            case_sensitive: self.case_sensitive,
            content_pattern: (!self.content_pattern.is_empty())
                .then(|| self.content_pattern.clone()),
            whole_word: self.whole_word,
            ignored_directories: self
                .ignored_directories
                .split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(str::to_string)
                .collect(),
        }
    }

    fn move_focus(&mut self, delta: isize) {
        let len = FindDialogField::ALL.len();
        let index = self.focus.index();
        let next = if delta.is_negative() {
            index.checked_sub(delta.unsigned_abs()).unwrap_or(len - 1)
        } else {
            index.saturating_add(delta as usize) % len
        };
        self.focus = FindDialogField::ALL[next];
    }

    fn insert(&mut self, character: char) {
        match self.focus {
            FindDialogField::StartDirectory => self.start_directory.push(character),
            FindDialogField::FilenamePattern => self.filename_pattern.push(character),
            FindDialogField::NameMode if character == ' ' => {
                self.name_mode = match self.name_mode {
                    FindNameMode::Glob => FindNameMode::Regex,
                    FindNameMode::Regex => FindNameMode::Glob,
                };
            }
            FindDialogField::CaseSensitive if character == ' ' => {
                self.case_sensitive = !self.case_sensitive;
            }
            FindDialogField::ContentPattern => self.content_pattern.push(character),
            FindDialogField::WholeWord if character == ' ' => {
                self.whole_word = !self.whole_word;
            }
            FindDialogField::IgnoredDirectories => self.ignored_directories.push(character),
            _ => {}
        }
    }

    fn backspace(&mut self) {
        if !self.focus.is_editable() {
            return;
        }
        match self.focus {
            FindDialogField::StartDirectory => {
                self.start_directory.pop();
            }
            FindDialogField::FilenamePattern => {
                self.filename_pattern.pop();
            }
            FindDialogField::ContentPattern => {
                self.content_pattern.pop();
            }
            FindDialogField::IgnoredDirectories => {
                self.ignored_directories.pop();
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug)]
pub enum DialogKind {
    Confirm(ConfirmDialogState),
    Input(InputDialogState),
    PairInput(PairInputDialogState),
    Listbox(ListboxDialogState),
    Find(FindDialogState),
}

#[derive(Clone, Debug)]
pub struct DialogState {
    pub title: String,
    pub kind: DialogKind,
}

impl DialogState {
    pub fn confirm(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            kind: DialogKind::Confirm(ConfirmDialogState {
                message: message.into(),
                focus: DialogButtonFocus::Ok,
            }),
        }
    }

    pub fn input(
        title: impl Into<String>,
        prompt: impl Into<String>,
        initial_value: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            kind: DialogKind::Input(InputDialogState {
                prompt: prompt.into(),
                value: initial_value.into(),
            }),
        }
    }

    pub fn pair_input(
        title: impl Into<String>,
        first_prompt: impl Into<String>,
        first_value: impl Into<String>,
        second_prompt: impl Into<String>,
        second_value: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            kind: DialogKind::PairInput(PairInputDialogState {
                first_prompt: first_prompt.into(),
                first_value: first_value.into(),
                second_prompt: second_prompt.into(),
                second_value: second_value.into(),
                focus: PairInputField::First,
            }),
        }
    }

    pub fn listbox(title: impl Into<String>, items: Vec<String>, selected: usize) -> Self {
        let selected = if items.is_empty() {
            0
        } else {
            selected.min(items.len() - 1)
        };
        Self {
            title: title.into(),
            kind: DialogKind::Listbox(ListboxDialogState { items, selected }),
        }
    }

    pub fn find(spec: &FindSpec) -> Self {
        Self {
            title: String::from("Find file"),
            kind: DialogKind::Find(FindDialogState::from_spec(spec)),
        }
    }

    pub fn demo_confirm() -> Self {
        Self::confirm("Confirm", "Proceed with this action?")
    }

    pub fn demo_input() -> Self {
        Self::input("Input", "New name:", "")
    }

    pub fn demo_listbox() -> Self {
        Self::listbox(
            "Listbox",
            vec![
                String::from("Sort by name"),
                String::from("Sort by size"),
                String::from("Sort by mtime"),
            ],
            0,
        )
    }

    pub fn key_context(&self) -> KeyContext {
        match self.kind {
            DialogKind::Confirm(_) => KeyContext::Dialog,
            DialogKind::Input(_) | DialogKind::PairInput(_) => KeyContext::Input,
            DialogKind::Listbox(_) => KeyContext::Listbox,
            DialogKind::Find(_) => KeyContext::FindDialog,
        }
    }

    pub fn handle_event(&mut self, event: DialogEvent) -> DialogTransition {
        match &mut self.kind {
            DialogKind::Confirm(confirm) => match event {
                DialogEvent::FocusNext => {
                    confirm.focus.toggle();
                    DialogTransition::Stay
                }
                DialogEvent::Accept => match confirm.focus {
                    DialogButtonFocus::Ok => DialogTransition::Close(DialogResult::ConfirmAccepted),
                    DialogButtonFocus::Cancel => {
                        DialogTransition::Close(DialogResult::ConfirmDeclined)
                    }
                },
                DialogEvent::Cancel => DialogTransition::Close(DialogResult::Canceled),
                _ => DialogTransition::Stay,
            },
            DialogKind::Input(input) => match event {
                DialogEvent::InsertChar(ch) => {
                    input.value.push(ch);
                    DialogTransition::Stay
                }
                DialogEvent::Backspace => {
                    input.value.pop();
                    DialogTransition::Stay
                }
                DialogEvent::Accept => {
                    DialogTransition::Close(DialogResult::InputSubmitted(input.value.clone()))
                }
                DialogEvent::Cancel => DialogTransition::Close(DialogResult::Canceled),
                _ => DialogTransition::Stay,
            },
            DialogKind::PairInput(input) => match event {
                DialogEvent::FocusNext => {
                    input.focus.toggle();
                    DialogTransition::Stay
                }
                DialogEvent::InsertChar(ch) => {
                    input.focused_value_mut().push(ch);
                    DialogTransition::Stay
                }
                DialogEvent::Backspace => {
                    input.focused_value_mut().pop();
                    DialogTransition::Stay
                }
                DialogEvent::Accept => DialogTransition::Close(DialogResult::PairInputSubmitted {
                    first: input.first_value.clone(),
                    second: input.second_value.clone(),
                }),
                DialogEvent::Cancel => DialogTransition::Close(DialogResult::Canceled),
                _ => DialogTransition::Stay,
            },
            DialogKind::Listbox(listbox) => match event {
                DialogEvent::MoveUp => {
                    if listbox.items.is_empty() {
                        listbox.selected = 0;
                    } else {
                        listbox.selected = listbox.selected.saturating_sub(1);
                    }
                    DialogTransition::Stay
                }
                DialogEvent::MoveDown => {
                    if listbox.items.is_empty() {
                        listbox.selected = 0;
                    } else {
                        let last = listbox.items.len() - 1;
                        listbox.selected = listbox.selected.saturating_add(1).min(last);
                    }
                    DialogTransition::Stay
                }
                DialogEvent::Accept => {
                    if listbox.items.is_empty() {
                        DialogTransition::Close(DialogResult::ListboxSubmitted {
                            index: None,
                            value: None,
                        })
                    } else {
                        DialogTransition::Close(DialogResult::ListboxSubmitted {
                            index: Some(listbox.selected),
                            value: Some(listbox.items[listbox.selected].clone()),
                        })
                    }
                }
                DialogEvent::Cancel => DialogTransition::Close(DialogResult::Canceled),
                _ => DialogTransition::Stay,
            },
            DialogKind::Find(find) => match event {
                DialogEvent::FocusNext | DialogEvent::MoveDown => {
                    find.move_focus(1);
                    DialogTransition::Stay
                }
                DialogEvent::MoveUp => {
                    find.move_focus(-1);
                    DialogTransition::Stay
                }
                DialogEvent::InsertChar(character) => {
                    find.insert(character);
                    DialogTransition::Stay
                }
                DialogEvent::Backspace => {
                    find.backspace();
                    DialogTransition::Stay
                }
                DialogEvent::Accept => {
                    DialogTransition::Close(DialogResult::FindSubmitted(Box::new(find.to_spec())))
                }
                DialogEvent::Cancel => DialogTransition::Close(DialogResult::Canceled),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DialogEvent {
    FocusNext,
    MoveUp,
    MoveDown,
    InsertChar(char),
    Backspace,
    Accept,
    Cancel,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DialogResult {
    ConfirmAccepted,
    ConfirmDeclined,
    InputSubmitted(String),
    PairInputSubmitted {
        first: String,
        second: String,
    },
    ListboxSubmitted {
        index: Option<usize>,
        value: Option<String>,
    },
    FindSubmitted(Box<FindSpec>),
    Canceled,
}

impl DialogResult {
    pub fn status_line(&self) -> String {
        match self {
            Self::ConfirmAccepted => String::from("Dialog accepted"),
            Self::ConfirmDeclined => String::from("Dialog canceled"),
            Self::InputSubmitted(value) => format!("Input accepted: {value}"),
            Self::PairInputSubmitted { first, second } => {
                format!("Input accepted: {first}, {second}")
            }
            Self::ListboxSubmitted { index: _, value } => match value {
                Some(value) => format!("Listbox accepted: {value}"),
                None => String::from("Listbox accepted: <empty>"),
            },
            Self::FindSubmitted(spec) => {
                format!("Find accepted: {}", spec.display_pattern())
            }
            Self::Canceled => String::from("Dialog canceled"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DialogTransition {
    Stay,
    Close(DialogResult),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_dialog_focus_and_accept_behavior() {
        let mut dialog = DialogState::demo_confirm();
        assert_eq!(
            dialog.handle_event(DialogEvent::FocusNext),
            DialogTransition::Stay
        );
        let DialogKind::Confirm(confirm) = &dialog.kind else {
            panic!("expected confirm dialog");
        };
        assert_eq!(confirm.focus, DialogButtonFocus::Cancel);
        assert_eq!(
            dialog.handle_event(DialogEvent::Accept),
            DialogTransition::Close(DialogResult::ConfirmDeclined)
        );
    }

    #[test]
    fn confirm_dialog_cancel_event_closes_dialog() {
        let mut dialog = DialogState::demo_confirm();
        assert_eq!(
            dialog.handle_event(DialogEvent::Cancel),
            DialogTransition::Close(DialogResult::Canceled)
        );
    }

    #[test]
    fn input_dialog_editing_and_accept_behavior() {
        let mut dialog = DialogState::demo_input();
        assert_eq!(
            dialog.handle_event(DialogEvent::InsertChar('a')),
            DialogTransition::Stay
        );
        assert_eq!(
            dialog.handle_event(DialogEvent::InsertChar('b')),
            DialogTransition::Stay
        );
        assert_eq!(
            dialog.handle_event(DialogEvent::Backspace),
            DialogTransition::Stay
        );
        assert_eq!(
            dialog.handle_event(DialogEvent::InsertChar('c')),
            DialogTransition::Stay
        );

        assert_eq!(
            dialog.handle_event(DialogEvent::Accept),
            DialogTransition::Close(DialogResult::InputSubmitted(String::from("ac")))
        );
    }

    #[test]
    fn pair_input_dialog_edits_each_field_and_accepts_both() {
        let mut dialog = DialogState::pair_input("Entry", "Name:", "old", "Value:", "");
        assert_eq!(dialog.key_context(), KeyContext::Input);
        assert_eq!(
            dialog.handle_event(DialogEvent::InsertChar('!')),
            DialogTransition::Stay
        );
        assert_eq!(
            dialog.handle_event(DialogEvent::FocusNext),
            DialogTransition::Stay
        );
        assert_eq!(
            dialog.handle_event(DialogEvent::InsertChar('x')),
            DialogTransition::Stay
        );
        assert_eq!(
            dialog.handle_event(DialogEvent::Backspace),
            DialogTransition::Stay
        );
        assert_eq!(
            dialog.handle_event(DialogEvent::InsertChar('y')),
            DialogTransition::Stay
        );

        assert_eq!(
            dialog.handle_event(DialogEvent::Accept),
            DialogTransition::Close(DialogResult::PairInputSubmitted {
                first: String::from("old!"),
                second: String::from("y"),
            })
        );
    }

    #[test]
    fn listbox_dialog_selection_and_accept_behavior() {
        let mut dialog = DialogState::demo_listbox();
        assert_eq!(
            dialog.handle_event(DialogEvent::MoveDown),
            DialogTransition::Stay
        );
        assert_eq!(
            dialog.handle_event(DialogEvent::MoveDown),
            DialogTransition::Stay
        );
        assert_eq!(
            dialog.handle_event(DialogEvent::Accept),
            DialogTransition::Close(DialogResult::ListboxSubmitted {
                index: Some(2),
                value: Some(String::from("Sort by mtime")),
            })
        );
    }

    #[test]
    fn listbox_dialog_accepts_empty_state() {
        let mut dialog = DialogState {
            title: String::from("Listbox"),
            kind: DialogKind::Listbox(ListboxDialogState {
                items: Vec::new(),
                selected: 0,
            }),
        };

        assert_eq!(
            dialog.handle_event(DialogEvent::Accept),
            DialogTransition::Close(DialogResult::ListboxSubmitted {
                index: None,
                value: None,
            })
        );
    }

    #[test]
    fn find_dialog_edits_fields_and_toggles_options() {
        let spec = FindSpec::new("/tmp".into());
        let mut dialog = DialogState::find(&spec);
        assert_eq!(dialog.key_context(), KeyContext::FindDialog);

        assert_eq!(
            dialog.handle_event(DialogEvent::InsertChar('*')),
            DialogTransition::Stay
        );
        assert_eq!(
            dialog.handle_event(DialogEvent::MoveDown),
            DialogTransition::Stay
        );
        assert_eq!(
            dialog.handle_event(DialogEvent::InsertChar(' ')),
            DialogTransition::Stay
        );

        let DialogKind::Find(find) = &dialog.kind else {
            panic!("expected find dialog");
        };
        assert_eq!(find.filename_pattern, "*");
        assert_eq!(find.name_mode, FindNameMode::Regex);
    }
}
