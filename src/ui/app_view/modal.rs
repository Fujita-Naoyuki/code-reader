//! UI that sits in front of the panes, and which of it is on top. Windows are drawn after
//! the panes, so a plain Escape would otherwise reach a focused search box underneath too
//! (D02). The frame takes Escape up front and hands it to the top layer only.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Modal {
    Help,
    UnsavedChanges,
    SearchEverywhere,
    References,
    WorkspaceFile,
    GitDiscard,
    GitCreateBranch,
    GitSwitchBranch,
    GitCommitDetails,
    RenameWorkspace,
    RecentWorkspaces,
    AiCommand,
}

impl CodeReaderApp {
    /// The front-most layer, ignoring the Help window (which describes what is under it).
    pub(super) fn top_dialog(&self) -> Option<Modal> {
        if self.pending_unsaved_action.is_some() {
            Some(Modal::UnsavedChanges)
        } else if self.search_overlay.open {
            Some(Modal::SearchEverywhere)
        } else if self.references_overlay.open {
            Some(Modal::References)
        } else if self.workspace_file_dialog.is_some() {
            Some(Modal::WorkspaceFile)
        } else if self.pending_git_discard.is_some() {
            Some(Modal::GitDiscard)
        } else if self.git_create_branch_root.is_some() {
            Some(Modal::GitCreateBranch)
        } else if self.git_switch_branch_root.is_some() {
            Some(Modal::GitSwitchBranch)
        } else if self.git_commit_details.is_some() {
            Some(Modal::GitCommitDetails)
        } else if self.editing_session_name {
            Some(Modal::RenameWorkspace)
        } else if self.recent_workspaces_open {
            Some(Modal::RecentWorkspaces)
        } else if self.ai_command_open {
            Some(Modal::AiCommand)
        } else {
            None
        }
    }

    pub(super) fn top_modal(&self) -> Option<Modal> {
        if self.shortcuts_help_open {
            Some(Modal::Help)
        } else {
            self.top_dialog()
        }
    }

    /// Runs at the start of the frame: consumes a plain Escape for the top layer. Help closes
    /// right here; other windows read `modal_escape` when they are drawn.
    pub(super) fn dispatch_modal_escape(&mut self, ui: &egui::Ui) {
        self.modal_escape = None;
        let Some(top) = self.top_modal() else {
            return;
        };
        if !ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            return;
        }
        if top == Modal::Help {
            self.shortcuts_help_open = false;
        } else {
            self.modal_escape = Some(top);
        }
    }
}
