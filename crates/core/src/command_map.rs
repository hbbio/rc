use crate::AppCommand;
use crate::keymap::{KeyCommand, KeyContext};

impl AppCommand {
    pub fn from_key_command(context: KeyContext, key_command: &KeyCommand) -> Option<Self> {
        match (context, key_command) {
            (_, KeyCommand::OpenHelp) => Some(Self::OpenHelp),
            (KeyContext::FileManager, KeyCommand::OpenMenu) => Some(Self::OpenMenu),
            (KeyContext::Menu, KeyCommand::Quit) => Some(Self::CloseMenu),
            (KeyContext::Menu, KeyCommand::DialogCancel) => Some(Self::CloseMenu),
            (KeyContext::Menu, KeyCommand::DialogAccept)
            | (KeyContext::Menu, KeyCommand::OpenEntry) => Some(Self::MenuAccept),
            (KeyContext::Menu, KeyCommand::CursorUp) => Some(Self::MenuMoveUp),
            (KeyContext::Menu, KeyCommand::CursorDown) => Some(Self::MenuMoveDown),
            (KeyContext::Menu, KeyCommand::CursorLeft) => Some(Self::MenuMoveLeft),
            (KeyContext::Menu, KeyCommand::CursorRight) => Some(Self::MenuMoveRight),
            (KeyContext::Menu, KeyCommand::Home) => Some(Self::MenuHome),
            (KeyContext::Menu, KeyCommand::End) => Some(Self::MenuEnd),
            (KeyContext::FileManager, KeyCommand::Quit) => Some(Self::Quit),
            (KeyContext::Help, KeyCommand::Quit) => Some(Self::CloseHelp),
            (KeyContext::Viewer, KeyCommand::Quit) => Some(Self::CloseViewer),
            (KeyContext::FindResults, KeyCommand::Quit) => Some(Self::CloseFindResults),
            (KeyContext::Tree, KeyCommand::Quit) => Some(Self::CloseTree),
            (KeyContext::Hotlist, KeyCommand::Quit) => Some(Self::CloseHotlist),
            (KeyContext::FileManager, KeyCommand::PanelOther) => Some(Self::SwitchPanel),
            (KeyContext::FileManager, KeyCommand::CursorUp) => Some(Self::MoveUp),
            (KeyContext::FileManager, KeyCommand::CursorDown) => Some(Self::MoveDown),
            (KeyContext::FileManager, KeyCommand::PageUp) => Some(Self::PageUp),
            (KeyContext::FileManager, KeyCommand::PageDown) => Some(Self::PageDown),
            (KeyContext::FileManager, KeyCommand::Home) => Some(Self::MoveHome),
            (KeyContext::FileManager, KeyCommand::End) => Some(Self::MoveEnd),
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
            (KeyContext::Jobs, KeyCommand::CursorUp) => Some(Self::JobsMoveUp),
            (KeyContext::Jobs, KeyCommand::CursorDown) => Some(Self::JobsMoveDown),
            (KeyContext::Jobs, KeyCommand::CancelJob) => Some(Self::CancelJob),
            (KeyContext::Listbox, KeyCommand::CursorUp) => Some(Self::DialogListboxUp),
            (KeyContext::Listbox, KeyCommand::CursorDown) => Some(Self::DialogListboxDown),
            (KeyContext::Listbox, KeyCommand::OpenInputDialog) => Some(Self::PanelizePresetAdd),
            (KeyContext::Listbox, KeyCommand::OpenConfirmDialog) => Some(Self::PanelizePresetEdit),
            (KeyContext::Listbox, KeyCommand::Delete) => Some(Self::PanelizePresetRemove),
            (KeyContext::Help, KeyCommand::CursorUp) => Some(Self::HelpMoveUp),
            (KeyContext::Help, KeyCommand::CursorDown) => Some(Self::HelpMoveDown),
            (KeyContext::Help, KeyCommand::PageUp) => Some(Self::HelpPageUp),
            (KeyContext::Help, KeyCommand::PageDown) => Some(Self::HelpPageDown),
            (KeyContext::Help, KeyCommand::HelpHalfPageUp) => Some(Self::HelpHalfPageUp),
            (KeyContext::Help, KeyCommand::HelpHalfPageDown) => Some(Self::HelpHalfPageDown),
            (KeyContext::Help, KeyCommand::Home) => Some(Self::HelpHome),
            (KeyContext::Help, KeyCommand::End) => Some(Self::HelpEnd),
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
            (KeyContext::FindResults, KeyCommand::CursorUp) => Some(Self::FindResultsMoveUp),
            (KeyContext::FindResults, KeyCommand::CursorDown) => Some(Self::FindResultsMoveDown),
            (KeyContext::FindResults, KeyCommand::PageUp) => Some(Self::FindResultsPageUp),
            (KeyContext::FindResults, KeyCommand::PageDown) => Some(Self::FindResultsPageDown),
            (KeyContext::FindResults, KeyCommand::Home) => Some(Self::FindResultsHome),
            (KeyContext::FindResults, KeyCommand::End) => Some(Self::FindResultsEnd),
            (KeyContext::FindResults, KeyCommand::OpenEntry) => Some(Self::FindResultsOpenEntry),
            (KeyContext::FindResults, KeyCommand::OpenPanelizeDialog) => {
                Some(Self::FindResultsPanelize)
            }
            (KeyContext::FindResults, KeyCommand::CancelJob) => Some(Self::CancelJob),
            (KeyContext::FileManager, KeyCommand::OpenTree) => Some(Self::OpenTree),
            (KeyContext::Tree, KeyCommand::CursorUp) => Some(Self::TreeMoveUp),
            (KeyContext::Tree, KeyCommand::CursorDown) => Some(Self::TreeMoveDown),
            (KeyContext::Tree, KeyCommand::PageUp) => Some(Self::TreePageUp),
            (KeyContext::Tree, KeyCommand::PageDown) => Some(Self::TreePageDown),
            (KeyContext::Tree, KeyCommand::Home) => Some(Self::TreeHome),
            (KeyContext::Tree, KeyCommand::End) => Some(Self::TreeEnd),
            (KeyContext::Tree, KeyCommand::OpenEntry) => Some(Self::TreeOpenEntry),
            (KeyContext::FileManager, KeyCommand::OpenHotlist) => Some(Self::OpenHotlist),
            (KeyContext::FileManager, KeyCommand::OpenPanelizeDialog) => {
                Some(Self::OpenPanelizeDialog)
            }
            (KeyContext::FileManager, KeyCommand::EnterXMap) => Some(Self::EnterXMap),
            (KeyContext::Hotlist, KeyCommand::CursorUp) => Some(Self::HotlistMoveUp),
            (KeyContext::Hotlist, KeyCommand::CursorDown) => Some(Self::HotlistMoveDown),
            (KeyContext::Hotlist, KeyCommand::PageUp) => Some(Self::HotlistPageUp),
            (KeyContext::Hotlist, KeyCommand::PageDown) => Some(Self::HotlistPageDown),
            (KeyContext::Hotlist, KeyCommand::Home) => Some(Self::HotlistHome),
            (KeyContext::Hotlist, KeyCommand::End) => Some(Self::HotlistEnd),
            (KeyContext::Hotlist, KeyCommand::OpenEntry) => Some(Self::HotlistOpenEntry),
            (KeyContext::Hotlist, KeyCommand::OpenHotlist) => Some(Self::OpenHotlist),
            (KeyContext::Hotlist, KeyCommand::AddHotlist) => Some(Self::HotlistAddCurrentDirectory),
            (KeyContext::Hotlist, KeyCommand::RemoveHotlist) => Some(Self::HotlistRemoveSelected),
            (KeyContext::Viewer, KeyCommand::CursorUp) => Some(Self::ViewerMoveUp),
            (KeyContext::Viewer, KeyCommand::CursorDown) => Some(Self::ViewerMoveDown),
            (KeyContext::Viewer, KeyCommand::PageUp) => Some(Self::ViewerPageUp),
            (KeyContext::Viewer, KeyCommand::PageDown) => Some(Self::ViewerPageDown),
            (KeyContext::Viewer, KeyCommand::Home) => Some(Self::ViewerHome),
            (KeyContext::Viewer, KeyCommand::End) => Some(Self::ViewerEnd),
            (KeyContext::ViewerHex, KeyCommand::Quit) => Some(Self::CloseViewer),
            (KeyContext::ViewerHex, KeyCommand::CursorUp) => Some(Self::ViewerMoveUp),
            (KeyContext::ViewerHex, KeyCommand::CursorDown) => Some(Self::ViewerMoveDown),
            (KeyContext::ViewerHex, KeyCommand::PageUp) => Some(Self::ViewerPageUp),
            (KeyContext::ViewerHex, KeyCommand::PageDown) => Some(Self::ViewerPageDown),
            (KeyContext::ViewerHex, KeyCommand::Home) => Some(Self::ViewerHome),
            (KeyContext::ViewerHex, KeyCommand::End) => Some(Self::ViewerEnd),
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
