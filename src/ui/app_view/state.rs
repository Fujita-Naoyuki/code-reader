//! `CodeReaderApp`'s state, grouped by the feature that owns it.

use super::*;

/// Which parts of the window are showing.
pub(super) struct LayoutState {
    pub(super) sidebar_view: SidebarView,
    pub(super) show_workspace_sidebar: bool,
    pub(super) show_right_pane: bool,
    pub(super) recent_workspaces_open: bool,
    pub(super) shortcuts_help_open: bool,
    pub(super) ai_command_open: bool,
    /// The layer that gets this frame's Escape; see `dispatch_modal_escape`.
    pub(super) modal_escape: Option<Modal>,
    /// An action that would drop unsaved edits, waiting for Save / Discard / Cancel.
    pub(super) pending_unsaved_action: Option<UnsavedChangesAction>,
}

impl Default for LayoutState {
    fn default() -> Self {
        Self {
            sidebar_view: SidebarView::Workspace,
            show_workspace_sidebar: true,
            show_right_pane: false,
            recent_workspaces_open: false,
            shortcuts_help_open: false,
            ai_command_open: false,
            modal_escape: None,
            pending_unsaved_action: None,
        }
    }
}

/// The workspace tree and what watches the files behind it.
pub(super) struct WorkspaceTreeState {
    pub(super) snapshot: WorkspaceSnapshot,
    pub(super) next_change_poll: Instant,
    pub(super) change_receiver: Option<Receiver<WorkspaceChangeScanResult>>,
    pub(super) dragging_file: Option<PathBuf>,
    pub(super) selected_path: Option<PathBuf>,
    pub(super) pending_reveal_path: Option<PathBuf>,
    /// The tree, not an editor, gets F2 and Delete.
    pub(super) sidebar_active: bool,
    pub(super) file_dialog: Option<WorkspaceFileDialog>,
    pub(super) root_open: HashMap<PathBuf, bool>,
}

impl WorkspaceTreeState {
    pub(super) fn new(snapshot: WorkspaceSnapshot) -> Self {
        Self {
            snapshot,
            next_change_poll: Instant::now() + WORKSPACE_CHANGE_POLL_INTERVAL,
            change_receiver: None,
            dragging_file: None,
            selected_path: None,
            pending_reveal_path: None,
            sidebar_active: false,
            file_dialog: None,
            root_open: HashMap::new(),
        }
    }
}

/// Search Everywhere, the references list and workspace text search.
pub(super) struct SearchState {
    pub(super) overlay: SearchOverlay,
    pub(super) references_overlay: ReferencesOverlay,
    pub(super) index_revision: u64,
    pub(super) workspace: WorkspaceSearchState,
    pub(super) workspace_receiver: Option<Receiver<WorkspaceSearchResult>>,
    pub(super) next_workspace_id: u64,
    pub(super) symbol_index: SymbolIndex,
    pub(super) symbol_index_receiver: Option<Receiver<IndexingResult>>,
}

impl SearchState {
    pub(super) fn new(symbol_index_receiver: Receiver<IndexingResult>) -> Self {
        Self {
            overlay: SearchOverlay::default(),
            references_overlay: ReferencesOverlay::default(),
            index_revision: 0,
            workspace: WorkspaceSearchState::default(),
            workspace_receiver: None,
            next_workspace_id: 0,
            symbol_index: SymbolIndex::new(),
            symbol_index_receiver: Some(symbol_index_receiver),
        }
    }
}

/// The language servers and what their last answer offered.
pub(super) struct LspState {
    pub(super) go: LspSession,
    pub(super) rust: LspSession,
    pub(super) definition_candidates: Vec<DefinitionCandidate>,
    pub(super) location_list_kind: LocationListKind,
}

impl Default for LspState {
    fn default() -> Self {
        Self {
            go: LspSession::default(),
            rust: LspSession::default(),
            definition_candidates: Vec::new(),
            location_list_kind: LocationListKind::Definitions,
        }
    }
}

/// Where the user has been.
#[derive(Default)]
pub(super) struct NavigationState {
    pub(super) left_history: NavigationHistory,
    pub(super) right_history: NavigationHistory,
    pub(super) recent_files: Vec<PathBuf>,
}

/// The Git sidebar and its dialogs.
#[derive(Default)]
pub(super) struct GitPanelState {
    pub(super) repositories: Vec<GitRepositoryState>,
    pub(super) repositories_loaded: bool,
    pub(super) commit_messages: HashMap<PathBuf, String>,
    pub(super) repo_open: HashMap<PathBuf, bool>,
    pub(super) new_branch_names: HashMap<PathBuf, String>,
    pub(super) create_branch_root: Option<PathBuf>,
    pub(super) switch_branch_root: Option<PathBuf>,
    /// Branch highlighted with Up/Down in the Switch Branch window; Enter switches to it.
    pub(super) switch_branch_cursor: Option<String>,
    pub(super) pending_discard: Option<GitDiscardRequest>,
    pub(super) commit_details: Option<GitCommitDetailsWindow>,
}

/// Renaming or deleting a workspace session in progress.
#[derive(Default)]
pub(super) struct SessionEditState {
    pub(super) pending_delete_id: Option<String>,
    pub(super) editing_name: bool,
    pub(super) name_edit: String,
    pub(super) name_select_all: bool,
}

/// The "Shift x2" gesture; see `consume_search_shortcuts`.
#[derive(Default)]
pub(super) struct ShiftTapState {
    pub(super) last_down: bool,
    /// Shift is down and nothing else (keys, text, other modifiers) has happened since it
    /// went down, so releasing it counts as a lone Shift tap.
    pub(super) candidate: bool,
    pub(super) last_tap_time: Option<f64>,
}
