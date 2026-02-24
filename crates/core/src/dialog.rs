use crate::keymap::KeyContext;

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

#[derive(Clone, Debug)]
pub struct ListboxDialogState {
    pub items: Vec<String>,
    pub selected: usize,
}

#[derive(Clone, Debug)]
pub enum DialogKind {
    Confirm(ConfirmDialogState),
    Input(InputDialogState),
    Listbox(ListboxDialogState),
}

#[derive(Clone, Debug)]
pub struct DialogState {
    pub title: String,
    pub kind: DialogKind,
}

impl DialogState {
    pub fn demo_confirm() -> Self {
        Self {
            title: String::from("Confirm"),
            kind: DialogKind::Confirm(ConfirmDialogState {
                message: String::from("Proceed with this action?"),
                focus: DialogButtonFocus::Ok,
            }),
        }
    }

    pub fn demo_input() -> Self {
        Self {
            title: String::from("Input"),
            kind: DialogKind::Input(InputDialogState {
                prompt: String::from("New name:"),
                value: String::new(),
            }),
        }
    }

    pub fn demo_listbox() -> Self {
        Self {
            title: String::from("Listbox"),
            kind: DialogKind::Listbox(ListboxDialogState {
                items: vec![
                    String::from("Sort by name"),
                    String::from("Sort by size"),
                    String::from("Sort by mtime"),
                ],
                selected: 0,
            }),
        }
    }

    pub fn key_context(&self) -> KeyContext {
        match self.kind {
            DialogKind::Confirm(_) => KeyContext::Dialog,
            DialogKind::Input(_) => KeyContext::Input,
            DialogKind::Listbox(_) => KeyContext::Listbox,
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
    ListboxSubmitted {
        index: Option<usize>,
        value: Option<String>,
    },
    Canceled,
}

impl DialogResult {
    pub fn status_line(&self) -> String {
        match self {
            Self::ConfirmAccepted => String::from("Dialog accepted"),
            Self::ConfirmDeclined => String::from("Dialog canceled"),
            Self::InputSubmitted(value) => format!("Input accepted: {value}"),
            Self::ListboxSubmitted { index: _, value } => match value {
                Some(value) => format!("Listbox accepted: {value}"),
                None => String::from("Listbox accepted: <empty>"),
            },
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
}
