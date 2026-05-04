use crate::keymap::{KeyCommand, KeyContext};
use crate::{AppCommand, NavigationMotion, NavigationTarget};

impl AppCommand {
    pub fn from_key_command(context: KeyContext, key_command: &KeyCommand) -> Option<Self> {
        if let Some(command) = navigation_command(context, key_command) {
            return Some(command);
        }

        match (context, key_command) {
            (_, KeyCommand::OpenHelp) => Some(Self::OpenHelp),
            (KeyContext::FileManager, KeyCommand::OpenMenu) => Some(Self::OpenMenu),
            (KeyContext::Menu, KeyCommand::Quit) => Some(Self::CloseMenu),
            (KeyContext::Menu, KeyCommand::DialogCancel) => Some(Self::CloseMenu),
            (KeyContext::Menu, KeyCommand::DialogAccept)
            | (KeyContext::Menu, KeyCommand::OpenEntry) => Some(Self::MenuAccept),
            (KeyContext::FileManager, KeyCommand::Quit) => Some(Self::Quit),
            (KeyContext::Help, KeyCommand::Quit) => Some(Self::CloseHelp),
            (KeyContext::Viewer, KeyCommand::Quit) => Some(Self::CloseViewer),
            (KeyContext::FindResults, KeyCommand::Quit) => Some(Self::CloseFindResults),
            (KeyContext::Tree, KeyCommand::Quit) => Some(Self::CloseTree),
            (KeyContext::Hotlist, KeyCommand::Quit) => Some(Self::CloseHotlist),
            (KeyContext::FileManager, KeyCommand::PanelOther) => Some(Self::SwitchPanel),
            (KeyContext::FileManager, KeyCommand::ToggleTag) => Some(Self::ToggleTag),
            (KeyContext::FileManager, KeyCommand::InvertTags) => Some(Self::InvertTags),
            (KeyContext::FileManager, KeyCommand::SortNext) => Some(Self::SortNext),
            (KeyContext::FileManager, KeyCommand::SortReverse) => Some(Self::SortReverse),
            (KeyContext::FileManager, KeyCommand::Copy) => Some(Self::Copy),
            (KeyContext::FileManager, KeyCommand::Move) => Some(Self::Move),
            (KeyContext::FileManager, KeyCommand::Delete) => Some(Self::Delete),
            (KeyContext::FileManager, KeyCommand::CancelJob) => Some(Self::CancelJob),
            (KeyContext::FileManager, KeyCommand::OpenJobs) => Some(Self::OpenJobsScreen),
            (KeyContext::Jobs, KeyCommand::CloseJobs) => Some(Self::CloseJobsScreen),
            (KeyContext::Jobs, KeyCommand::CancelJob) => Some(Self::CancelJob),
            (KeyContext::Listbox, KeyCommand::CursorUp) => Some(Self::DialogListboxUp),
            (KeyContext::Listbox, KeyCommand::CursorDown) => Some(Self::DialogListboxDown),
            (KeyContext::Listbox, KeyCommand::OpenInputDialog) => Some(Self::PanelizePresetAdd),
            (KeyContext::Listbox, KeyCommand::OpenConfirmDialog) => Some(Self::PanelizePresetEdit),
            (KeyContext::Listbox, KeyCommand::Delete) => Some(Self::PanelizePresetRemove),
            (KeyContext::Help, KeyCommand::OpenEntry) => Some(Self::HelpFollowLink),
            (KeyContext::Help, KeyCommand::HelpBack) => Some(Self::HelpBack),
            (KeyContext::Help, KeyCommand::HelpIndex) => Some(Self::HelpIndex),
            (KeyContext::Help, KeyCommand::HelpLinkNext) => Some(Self::HelpLinkNext),
            (KeyContext::Help, KeyCommand::HelpLinkPrev) => Some(Self::HelpLinkPrev),
            (KeyContext::Help, KeyCommand::HelpNodeNext) => Some(Self::HelpNodeNext),
            (KeyContext::Help, KeyCommand::HelpNodePrev) => Some(Self::HelpNodePrev),
            (KeyContext::FileManager, KeyCommand::OpenEntry) => Some(Self::OpenEntry),
            (KeyContext::FileManager, KeyCommand::EditEntry) => Some(Self::EditEntry),
            (KeyContext::FileManager, KeyCommand::CdUp) => Some(Self::CdUp),
            (KeyContext::FileManager, KeyCommand::Reread) => Some(Self::Reread),
            (KeyContext::FileManager, KeyCommand::OpenFindDialog) => Some(Self::OpenFindDialog),
            (KeyContext::FindResults, KeyCommand::OpenEntry) => Some(Self::FindResultsOpenEntry),
            (KeyContext::FindResults, KeyCommand::OpenPanelizeDialog) => {
                Some(Self::FindResultsPanelize)
            }
            (KeyContext::FindResults, KeyCommand::CancelJob) => Some(Self::CancelJob),
            (KeyContext::FileManager, KeyCommand::OpenTree) => Some(Self::OpenTree),
            (KeyContext::Tree, KeyCommand::OpenEntry) => Some(Self::TreeOpenEntry),
            (KeyContext::FileManager, KeyCommand::OpenHotlist) => Some(Self::OpenHotlist),
            (KeyContext::FileManager, KeyCommand::OpenPanelizeDialog) => {
                Some(Self::OpenPanelizeDialog)
            }
            (KeyContext::FileManager, KeyCommand::EnterXMap) => Some(Self::EnterXMap),
            (KeyContext::Hotlist, KeyCommand::OpenEntry) => Some(Self::HotlistOpenEntry),
            (KeyContext::Hotlist, KeyCommand::OpenHotlist) => Some(Self::OpenHotlist),
            (KeyContext::Hotlist, KeyCommand::AddHotlist) => Some(Self::HotlistAddCurrentDirectory),
            (KeyContext::Hotlist, KeyCommand::RemoveHotlist) => Some(Self::HotlistRemoveSelected),
            (KeyContext::ViewerHex, KeyCommand::Quit) => Some(Self::CloseViewer),
            (KeyContext::Viewer, KeyCommand::Search) => Some(Self::ViewerSearchForward),
            (KeyContext::Viewer, KeyCommand::SearchBackward) => Some(Self::ViewerSearchBackward),
            (KeyContext::Viewer, KeyCommand::SearchContinue) => Some(Self::ViewerSearchContinue),
            (KeyContext::Viewer, KeyCommand::SearchContinueBackward) => {
                Some(Self::ViewerSearchContinueBackward)
            }
            (KeyContext::Viewer, KeyCommand::Goto) => Some(Self::ViewerGoto),
            (KeyContext::Viewer, KeyCommand::ToggleWrap) => Some(Self::ViewerToggleWrap),
            (KeyContext::ViewerHex, KeyCommand::Search) => Some(Self::ViewerSearchForward),
            (KeyContext::ViewerHex, KeyCommand::SearchBackward) => Some(Self::ViewerSearchBackward),
            (KeyContext::ViewerHex, KeyCommand::SearchContinue) => Some(Self::ViewerSearchContinue),
            (KeyContext::ViewerHex, KeyCommand::SearchContinueBackward) => {
                Some(Self::ViewerSearchContinueBackward)
            }
            (KeyContext::ViewerHex, KeyCommand::Goto) => Some(Self::ViewerGoto),
            (KeyContext::ViewerHex, KeyCommand::ToggleWrap) => Some(Self::ViewerToggleWrap),
            (KeyContext::Viewer, KeyCommand::ToggleHex)
            | (KeyContext::ViewerHex, KeyCommand::ToggleHex) => Some(Self::ViewerToggleHex),
            (KeyContext::FileManager, KeyCommand::OpenConfirmDialog) => {
                Some(Self::OpenConfirmDialog)
            }
            (KeyContext::FileManager, KeyCommand::OpenInputDialog) => Some(Self::OpenInputDialog),
            (KeyContext::FileManager, KeyCommand::OpenListboxDialog) => {
                Some(Self::OpenListboxDialog)
            }
            (KeyContext::FileManager, KeyCommand::OpenSkinDialog) => Some(Self::OpenSkinDialog),
            (KeyContext::FileManager, KeyCommand::OpenOptionsConfiguration) => {
                Some(Self::OpenOptionsConfiguration)
            }
            (KeyContext::FileManager, KeyCommand::OpenOptionsLayout) => {
                Some(Self::OpenOptionsLayout)
            }
            (KeyContext::FileManager, KeyCommand::OpenOptionsPanelOptions) => {
                Some(Self::OpenOptionsPanelOptions)
            }
            (KeyContext::FileManager, KeyCommand::OpenOptionsConfirmation) => {
                Some(Self::OpenOptionsConfirmation)
            }
            (KeyContext::FileManager, KeyCommand::OpenOptionsAppearance) => {
                Some(Self::OpenOptionsAppearance)
            }
            (KeyContext::FileManager, KeyCommand::OpenOptionsDisplayBits) => {
                Some(Self::OpenOptionsDisplayBits)
            }
            (KeyContext::FileManager, KeyCommand::OpenOptionsLearnKeys) => {
                Some(Self::OpenOptionsLearnKeys)
            }
            (KeyContext::FileManager, KeyCommand::OpenOptionsVirtualFs) => {
                Some(Self::OpenOptionsVirtualFs)
            }
            (KeyContext::FileManager, KeyCommand::SaveSetup) => Some(Self::SaveSetup),
            (_, KeyCommand::DialogAccept) => Some(Self::DialogAccept),
            (_, KeyCommand::DialogCancel) => Some(Self::DialogCancel),
            (_, KeyCommand::DialogFocusNext) => Some(Self::DialogFocusNext),
            (_, KeyCommand::DialogBackspace) => Some(Self::DialogBackspace),
            (_, KeyCommand::Unknown(_)) => None,
            _ => None,
        }
    }
}

fn navigation_command(context: KeyContext, key_command: &KeyCommand) -> Option<AppCommand> {
    let target = match context {
        KeyContext::FileManager | KeyContext::FileManagerXMap => NavigationTarget::FileManager,
        KeyContext::Jobs => NavigationTarget::Jobs,
        KeyContext::Menu => NavigationTarget::Menu,
        KeyContext::Help => NavigationTarget::Help,
        KeyContext::FindResults => NavigationTarget::FindResults,
        KeyContext::Tree => NavigationTarget::Tree,
        KeyContext::Hotlist => NavigationTarget::Hotlist,
        KeyContext::Viewer | KeyContext::ViewerHex => NavigationTarget::Viewer,
        _ => return None,
    };

    let motion = match (target, key_command) {
        (_, KeyCommand::CursorUp) => NavigationMotion::Up,
        (_, KeyCommand::CursorDown) => NavigationMotion::Down,
        (NavigationTarget::Menu, KeyCommand::CursorLeft) => NavigationMotion::Left,
        (NavigationTarget::Menu, KeyCommand::CursorRight) => NavigationMotion::Right,
        (
            NavigationTarget::FileManager
            | NavigationTarget::Help
            | NavigationTarget::FindResults
            | NavigationTarget::Tree
            | NavigationTarget::Hotlist
            | NavigationTarget::Viewer,
            KeyCommand::PageUp,
        ) => NavigationMotion::PageUp,
        (
            NavigationTarget::FileManager
            | NavigationTarget::Help
            | NavigationTarget::FindResults
            | NavigationTarget::Tree
            | NavigationTarget::Hotlist
            | NavigationTarget::Viewer,
            KeyCommand::PageDown,
        ) => NavigationMotion::PageDown,
        (NavigationTarget::Help, KeyCommand::HelpHalfPageUp) => NavigationMotion::HalfPageUp,
        (NavigationTarget::Help, KeyCommand::HelpHalfPageDown) => NavigationMotion::HalfPageDown,
        (
            NavigationTarget::FileManager
            | NavigationTarget::Menu
            | NavigationTarget::Help
            | NavigationTarget::FindResults
            | NavigationTarget::Tree
            | NavigationTarget::Hotlist
            | NavigationTarget::Viewer,
            KeyCommand::Home,
        ) => NavigationMotion::Home,
        (
            NavigationTarget::FileManager
            | NavigationTarget::Menu
            | NavigationTarget::Help
            | NavigationTarget::FindResults
            | NavigationTarget::Tree
            | NavigationTarget::Hotlist
            | NavigationTarget::Viewer,
            KeyCommand::End,
        ) => NavigationMotion::End,
        _ => return None,
    };

    Some(AppCommand::Navigate(target, motion))
}
