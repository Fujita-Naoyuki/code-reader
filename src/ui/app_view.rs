use std::{
    collections::HashMap,
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use eframe::egui::{
    self, Color32, FontId, RichText, ScrollArea, TextBuffer, TextEdit, TextFormat, TextStyle,
    scroll_area::ScrollSource,
    text::{CCursor, CCursorRange, LayoutJob},
    text_edit::TextEditState,
};
use markdown::{
    ParseOptions,
    mdast::{AlignKind, Node},
};

use crate::core::{
    cli_agent::{CliAgentCommand, build_cli_agent_command},
    document::Document,
    external_definitions::resolve_external_definition_on_demand,
    file_tree::{
        FileDiscoveryOptions, FileNode, FileTree, build_file_tree, discover_workspace,
        discover_workspace_files,
    },
    location::{Location, Position, Range},
    lsp::{
        LspClient, LspClientEvent, LspLocationResponse, LspRequestKind, LspServerConfig,
        LspServerEvent, LspServerEventKind,
    },
    markdown::MarkdownDocument,
    session::{
        AppState, OpenFiles, PanePositions, SessionBookmark, TextPosition, WorkspaceSession,
    },
    symbol::SymbolSource,
    symbol_index::SymbolIndex,
    symbol_indexing::{IndexingResult, start_symbol_indexing},
    syntax::{HighlightKind, HighlightSpan},
    workspace::Workspace,
};
use crate::features::{
    definition::resolve_definition,
    search::{SearchResult, search_everywhere},
};
use crate::languages::LanguageRegistry;
use crate::platform;

const DEFAULT_TAB_WIDTH: usize = 4;
const MARKDOWN_SIDE_PANE_DEFAULT_WIDTH: f32 = 180.0;
const MARKDOWN_SIDE_PANE_MIN_WIDTH: f32 = 120.0;
const MARKDOWN_SIDE_PANE_MAX_WIDTH: f32 = 560.0;
const MARKDOWN_SIDE_PANE_MIN_REMAINING_WIDTH: f32 = 360.0;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;
const NAVIGATION_BREADCRUMB_LIMIT: usize = 5;
const WORKSPACE_SEARCH_MAX_MATCHES: usize = 2000;
const WORKSPACE_SEARCH_PREVIEW_CHARS: usize = 140;
const WORKSPACE_SECTION_HEADER_HEIGHT: f32 = 24.0;
const WORKSPACE_FOOTER_BODY_ALLOWANCE: f32 = 64.0;
const WORKSPACE_FILE_TREE_MIN_HEIGHT: f32 = 120.0;
const EDITOR_PAGE_SCROLL_RATIO: f32 = 0.85;
const TEXTEDIT_SOURCE_MIN_ROWS: usize = 32;
const EDITOR_TEXT_CHUNK_TARGET_BYTES: usize = 64 * 1024;
const VIRTUAL_SOURCE_MIN_BYTES: usize = 512 * 1024;
const VIRTUAL_SOURCE_MIN_LINES: usize = 10_000;
const VIRTUAL_LINE_LAYOUT_CACHE_LIMIT: usize = 4096;
const VIRTUAL_LINE_LAYOUT_CACHE_KEEP_FRAMES: u64 = 180;
const VIRTUAL_WHEEL_LINE_SCROLL_SPEED: f32 = 40.0;
const VIRTUAL_SCROLLBAR_BAR_WIDTH: f32 = 6.0;
const VIRTUAL_SCROLLBAR_HIT_WIDTH: f32 = 12.0;
const EXTERNAL_RUST_HIGHLIGHT_MAX_BYTES: usize = 512 * 1024;
const WORKSPACE_CHANGE_POLL_INTERVAL: Duration = Duration::from_secs(1);
const BOOKMARK_GUTTER_WIDTH: f32 = 18.0;
const GO_LSP_REQUEST_TIMEOUT: Duration = Duration::from_millis(3000);
const RUST_LSP_REQUEST_TIMEOUT: Duration = Duration::from_millis(3000);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pane {
    Left,
    Right,
}

impl Pane {
    fn label(self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Right => "Right",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidebarView {
    Workspace,
    Search,
    Git,
}

impl SidebarView {
    fn all() -> [Self; 3] {
        [Self::Workspace, Self::Search, Self::Git]
    }

    fn label(self) -> &'static str {
        match self {
            Self::Workspace => "Workspace",
            Self::Search => "Search",
            Self::Git => "Git",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WorkspaceFileDialog {
    NewFile {
        folder: PathBuf,
        file_name: String,
        error: Option<String>,
    },
    Rename {
        path: PathBuf,
        new_name: String,
        select_stem: bool,
        error: Option<String>,
    },
    Delete {
        path: PathBuf,
        error: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WorkspaceFileDialogAction {
    CreateFile { folder: PathBuf, file_name: String },
    RenameFile { path: PathBuf, new_name: String },
    DeleteFile { path: PathBuf },
}

pub struct CodeReaderApp {
    language_registry: LanguageRegistry,
    app_state: AppState,
    state_path: PathBuf,
    focused_pane: Pane,
    left: EditorPane,
    right: EditorPane,
    file_tree: FileTree,
    workspace_snapshot: WorkspaceSnapshot,
    next_workspace_change_poll: Instant,
    workspace_change_receiver: Option<Receiver<WorkspaceChangeScanResult>>,
    symbol_index: SymbolIndex,
    symbol_index_receiver: Option<Receiver<IndexingResult>>,
    external_file_count: usize,
    external_package_count: usize,
    definition_candidates: Vec<DefinitionCandidate>,
    location_list_kind: LocationListKind,
    go_lsp_client: Option<LspClient>,
    pending_go_lsp_request: Option<PendingGoLspRequest>,
    go_lsp_synced_hashes: HashMap<PathBuf, u64>,
    go_lsp_request_started_at: HashMap<u64, Instant>,
    go_lsp_debug: LspDebugState,
    rust_lsp_client: Option<LspClient>,
    pending_rust_lsp_request: Option<PendingGoLspRequest>,
    rust_lsp_synced_hashes: HashMap<PathBuf, u64>,
    rust_lsp_request_started_at: HashMap<u64, Instant>,
    rust_lsp_debug: LspDebugState,
    left_navigation_history: NavigationHistory,
    right_navigation_history: NavigationHistory,
    recent_files: Vec<PathBuf>,
    markdown_modes: HashMap<PathBuf, MarkdownMode>,
    search_overlay: SearchOverlay,
    references_overlay: ReferencesOverlay,
    search_index_revision: u64,
    workspace_search: WorkspaceSearchState,
    workspace_search_receiver: Option<Receiver<WorkspaceSearchResult>>,
    next_workspace_search_id: u64,
    git_repositories: Vec<GitRepositoryState>,
    git_repositories_loaded: bool,
    git_commit_messages: HashMap<PathBuf, String>,
    git_repo_open: HashMap<PathBuf, bool>,
    git_new_branch_names: HashMap<PathBuf, String>,
    git_create_branch_root: Option<PathBuf>,
    git_switch_branch_root: Option<PathBuf>,
    /// Branch highlighted with Up/Down in the Switch Branch window; Enter switches to it.
    git_switch_branch_cursor: Option<String>,
    pending_git_discard: Option<GitDiscardRequest>,
    git_commit_details: Option<GitCommitDetailsWindow>,
    dragging_workspace_file: Option<PathBuf>,
    selected_workspace_path: Option<PathBuf>,
    pending_workspace_tree_reveal_path: Option<PathBuf>,
    workspace_sidebar_active: bool,
    workspace_file_dialog: Option<WorkspaceFileDialog>,
    sidebar_view: SidebarView,
    show_workspace_sidebar: bool,
    show_right_pane: bool,
    recent_workspaces_open: bool,
    shortcuts_help_open: bool,
    ai_command_open: bool,
    workspace_root_open: HashMap<PathBuf, bool>,
    last_shift_down: bool,
    /// Shift is down and nothing else (keys, text, other modifiers) has happened since it
    /// went down, so releasing it counts as a lone Shift tap.
    shift_tap_candidate: bool,
    last_shift_tap_time: Option<f64>,
    tab_width: usize,
    status_message: String,
    pending_delete_session_id: Option<String>,
    editing_session_name: bool,
    session_name_edit: String,
    session_name_select_all: bool,
    app_perf_debug: AppPerfDebug,
}

impl CodeReaderApp {
    pub fn new(
        creation_context: &eframe::CreationContext<'_>,
        language_registry: LanguageRegistry,
    ) -> Self {
        configure_fonts(&creation_context.egui_ctx);
        egui_extras::install_image_loaders(&creation_context.egui_ctx);

        let state_path = platform::current().state_file_path();
        let fallback_root = platform::current().current_dir();
        let app_state =
            load_state(&state_path).unwrap_or_else(|| AppState::new(fallback_root.clone()));
        let current_session = app_state
            .current_session()
            .cloned()
            .unwrap_or_else(|| WorkspaceSession::new("Default", vec![fallback_root]));
        let file_tree = discover_workspace(
            &current_session.root_folders,
            FileDiscoveryOptions::default(),
        );
        let workspace_snapshot = workspace_snapshot(&file_tree.files);
        let left = current_session
            .open_files
            .left
            .clone()
            .map(|path| {
                editor_pane_from_session_path(Pane::Left, path, current_session.pane_positions.left)
            })
            .unwrap_or_else(|| EditorPane::empty(Pane::Left));
        let right = current_session
            .open_files
            .right
            .clone()
            .map(|path| {
                editor_pane_from_session_path(
                    Pane::Right,
                    path,
                    current_session.pane_positions.right,
                )
            })
            .unwrap_or_else(|| EditorPane::empty(Pane::Right));
        let recent_files = recent_files_from_open_files(&current_session.open_files);
        let symbol_index_receiver =
            start_symbol_indexing(file_tree.files.clone(), language_registry.clone());

        let mut app = Self {
            language_registry,
            app_state,
            state_path,
            focused_pane: Pane::Left,
            left,
            right,
            file_tree,
            workspace_snapshot,
            next_workspace_change_poll: Instant::now() + WORKSPACE_CHANGE_POLL_INTERVAL,
            workspace_change_receiver: None,
            symbol_index: SymbolIndex::new(),
            symbol_index_receiver: Some(symbol_index_receiver),
            external_file_count: 0,
            external_package_count: 0,
            definition_candidates: Vec::new(),
            location_list_kind: LocationListKind::Definitions,
            go_lsp_client: None,
            pending_go_lsp_request: None,
            go_lsp_synced_hashes: HashMap::new(),
            go_lsp_request_started_at: HashMap::new(),
            go_lsp_debug: LspDebugState::default(),
            rust_lsp_client: None,
            pending_rust_lsp_request: None,
            rust_lsp_synced_hashes: HashMap::new(),
            rust_lsp_request_started_at: HashMap::new(),
            rust_lsp_debug: LspDebugState::default(),
            left_navigation_history: NavigationHistory::default(),
            right_navigation_history: NavigationHistory::default(),
            recent_files,
            markdown_modes: HashMap::new(),
            search_overlay: SearchOverlay::default(),
            references_overlay: ReferencesOverlay::default(),
            search_index_revision: 0,
            workspace_search: WorkspaceSearchState::default(),
            workspace_search_receiver: None,
            next_workspace_search_id: 0,
            git_repositories: Vec::new(),
            git_repositories_loaded: false,
            git_commit_messages: HashMap::new(),
            git_repo_open: HashMap::new(),
            git_new_branch_names: HashMap::new(),
            git_create_branch_root: None,
            git_switch_branch_root: None,
            git_switch_branch_cursor: None,
            pending_git_discard: None,
            git_commit_details: None,
            dragging_workspace_file: None,
            selected_workspace_path: None,
            pending_workspace_tree_reveal_path: None,
            workspace_sidebar_active: false,
            workspace_file_dialog: None,
            sidebar_view: SidebarView::Workspace,
            show_workspace_sidebar: true,
            show_right_pane: false,
            recent_workspaces_open: false,
            shortcuts_help_open: false,
            ai_command_open: false,
            workspace_root_open: HashMap::new(),
            last_shift_down: false,
            shift_tap_candidate: false,
            last_shift_tap_time: None,
            tab_width: DEFAULT_TAB_WIDTH,
            status_message: "Session restored.".to_owned(),
            pending_delete_session_id: None,
            editing_session_name: false,
            session_name_edit: String::new(),
            session_name_select_all: false,
            app_perf_debug: AppPerfDebug::default(),
        };
        app.warm_up_rust_lsp_for_workspace();
        app
    }

    fn focused_editor_mut(&mut self) -> &mut EditorPane {
        match self.focused_pane {
            Pane::Left => &mut self.left,
            Pane::Right => &mut self.right,
        }
    }

    fn focused_editor(&self) -> &EditorPane {
        match self.focused_pane {
            Pane::Left => &self.left,
            Pane::Right => &self.right,
        }
    }

    fn editor_mut(&mut self, pane: Pane) -> &mut EditorPane {
        match pane {
            Pane::Left => &mut self.left,
            Pane::Right => &mut self.right,
        }
    }

    fn editor(&self, pane: Pane) -> &EditorPane {
        match pane {
            Pane::Left => &self.left,
            Pane::Right => &self.right,
        }
    }

    fn focused_navigation_history(&self) -> &NavigationHistory {
        self.navigation_history(self.focused_pane)
    }

    fn focused_navigation_history_mut(&mut self) -> &mut NavigationHistory {
        self.navigation_history_mut(self.focused_pane)
    }

    fn navigation_history(&self, pane: Pane) -> &NavigationHistory {
        match pane {
            Pane::Left => &self.left_navigation_history,
            Pane::Right => &self.right_navigation_history,
        }
    }

    fn navigation_history_mut(&mut self, pane: Pane) -> &mut NavigationHistory {
        match pane {
            Pane::Left => &mut self.left_navigation_history,
            Pane::Right => &mut self.right_navigation_history,
        }
    }

    fn push_focused_navigation_history(&mut self, point: NavigationPoint) {
        self.focused_navigation_history_mut().push_back(point);
    }

    fn clear_focused_navigation_history(&mut self) {
        self.focused_navigation_history_mut().clear();
    }

    fn open_file_in_focused_pane(&mut self, path: PathBuf) {
        self.open_file_in_pane(self.focused_pane, path);
    }

    fn open_file_in_pane(&mut self, pane: Pane, path: PathBuf) {
        self.navigation_history_mut(pane).clear();
        self.remember_markdown_mode(pane);
        let mut next = EditorPane::from_path(pane, path);
        self.restore_markdown_mode(&mut next);
        if let Some(path) = &next.path {
            if is_external_rust_path(path) {
                // Rust definition navigation is handled by rust-analyzer. Indexing external
                // Rust sources here can be very expensive and is no longer needed for F12.
            } else if is_external_path(path) {
                self.symbol_index
                    .add_external_file(path, &self.language_registry);
            } else {
                self.symbol_index.add_file(path, &self.language_registry);
            }
            self.remember_recent_file(path.clone());
        }
        *self.editor_mut(pane) = next;
        self.sync_open_files();
        self.save_state();
    }

    fn open_markdown_link_in_focused_pane(&mut self, path: PathBuf, fragment: Option<String>) {
        if !path.exists() {
            self.status_message = format!("Markdown link target not found: {}", path.display());
            return;
        }
        if !is_markdown_path(&path) {
            self.status_message = format!(
                "Only local Markdown links are supported: {}",
                path.display()
            );
            return;
        }

        if let Some(current) = self.focused_editor().navigation_point() {
            self.push_focused_navigation_history(current);
        }

        self.remember_focused_markdown_mode();
        let mut next = EditorPane::from_path(self.focused_pane, path.clone());
        self.restore_markdown_mode(&mut next);

        let mut fragment_status = None;
        if let Some(fragment) = fragment.as_deref().filter(|fragment| !fragment.is_empty()) {
            match MarkdownDocument::parse(path.clone(), next.text.as_str().to_owned()) {
                Ok(document) => {
                    if let Some(target) = markdown_fragment_target(&document, fragment) {
                        next.markdown_active_page_id = Some(target.page_id);
                        next.markdown_active_heading_id = target.heading_id;
                        next.target_position = Some(target.position);
                        next.pending_navigation = Some(PendingEditorNavigation {
                            position: target.position,
                            focus_editor: true,
                        });
                        next.cursor_position = Some(target.position);
                    } else {
                        fragment_status = Some(format!("Fragment not found: #{fragment}"));
                    }
                }
                Err(error) => {
                    fragment_status = Some(format!("Opened Markdown, but parse failed: {error}"));
                }
            }
        }

        self.symbol_index.add_file(&path, &self.language_registry);
        self.remember_recent_file(path.clone());
        self.reveal_workspace_file_in_tree(&path);
        *self.focused_editor_mut() = next;
        self.sync_open_files();
        self.save_state();

        self.status_message =
            fragment_status.unwrap_or_else(|| format!("Opened Markdown link: {}", path.display()));
    }

    fn open_external_url(&mut self, url: &str) {
        match platform::current().open_url(url) {
            Ok(()) => {
                self.status_message = format!("Opened external URL: {url}");
            }
            Err(error) => {
                self.status_message = format!("Failed to open external URL: {url} ({error})");
            }
        }
    }

    fn remember_focused_markdown_mode(&mut self) {
        self.remember_markdown_mode(self.focused_pane);
    }

    fn remember_markdown_mode(&mut self, pane: Pane) {
        let editor = self.editor(pane);
        if editor.is_markdown()
            && let Some(path) = &editor.path
        {
            self.markdown_modes
                .insert(path.clone(), editor.markdown_mode);
        }
    }

    fn restore_markdown_mode(&self, editor: &mut EditorPane) {
        if editor.is_markdown()
            && let Some(path) = &editor.path
            && let Some(mode) = self.markdown_modes.get(path)
        {
            editor.markdown_mode = *mode;
        }
    }

    fn current_session(&self) -> Option<&WorkspaceSession> {
        self.app_state.current_session()
    }

    fn current_session_mut(&mut self) -> Option<&mut WorkspaceSession> {
        self.app_state.current_session_mut()
    }

    fn primary_root(&self) -> PathBuf {
        self.current_session()
            .and_then(|session| session.root_folders.first())
            .cloned()
            .unwrap_or_else(|| platform::current().current_dir())
    }

    fn selected_workspace_path(&self) -> Option<&Path> {
        let path = self.selected_workspace_path.as_deref()?;
        (path.exists() && self.path_is_in_current_workspace(path)).then_some(path)
    }

    fn selected_workspace_new_file_folder(&self) -> PathBuf {
        if let Some(path) = self.selected_workspace_path() {
            if path.is_dir() {
                return path.to_path_buf();
            }
            if let Some(parent) = path.parent() {
                return parent.to_path_buf();
            }
        }
        self.primary_root()
    }

    fn selected_workspace_file(&self) -> Option<PathBuf> {
        self.selected_workspace_path()
            .filter(|path| path.is_file())
            .map(Path::to_path_buf)
    }

    fn path_is_in_current_workspace(&self, path: &Path) -> bool {
        self.current_session().is_some_and(|session| {
            session
                .root_folders
                .iter()
                .any(|root| path.starts_with(root))
        })
    }

    fn workspace_tree_file_path(&self, path: &Path) -> Option<PathBuf> {
        self.file_tree
            .files
            .iter()
            .find(|file| paths_match(file, path))
            .cloned()
    }

    fn reveal_workspace_file_in_tree(&mut self, path: &Path) {
        let Some(tree_path) = self.workspace_tree_file_path(path) else {
            return;
        };

        self.selected_workspace_path = Some(tree_path.clone());
        self.pending_workspace_tree_reveal_path = Some(tree_path.clone());
        for root in self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default()
        {
            if tree_path.starts_with(&root) {
                self.workspace_root_open.insert(root, true);
                break;
            }
        }
    }

    fn start_workspace_new_file(&mut self) {
        let folder = self.selected_workspace_new_file_folder();
        if !folder.is_dir() {
            self.status_message = format!("Folder not found: {}", folder.display());
            return;
        }
        self.workspace_file_dialog = Some(WorkspaceFileDialog::NewFile {
            folder,
            file_name: String::new(),
            error: None,
        });
    }

    fn start_workspace_rename_file(&mut self) {
        let Some(path) = self.selected_workspace_file() else {
            self.status_message = "Select a file to rename.".to_owned();
            return;
        };
        let new_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        self.workspace_file_dialog = Some(WorkspaceFileDialog::Rename {
            path,
            new_name,
            select_stem: true,
            error: None,
        });
    }

    fn start_workspace_delete_file(&mut self) {
        let Some(path) = self.selected_workspace_file() else {
            self.status_message = "Select a file to delete.".to_owned();
            return;
        };
        self.workspace_file_dialog = Some(WorkspaceFileDialog::Delete { path, error: None });
    }

    fn handle_workspace_file_shortcuts(&mut self, ui: &egui::Ui) {
        if !self.workspace_file_shortcuts_enabled() {
            return;
        }

        if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::F2)) {
            self.start_workspace_rename_file();
        } else if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Delete))
        {
            self.start_workspace_delete_file();
        }
    }

    fn workspace_file_shortcuts_enabled(&self) -> bool {
        self.show_workspace_sidebar
            && self.sidebar_view == SidebarView::Workspace
            && self.workspace_sidebar_active
            && self.workspace_file_dialog.is_none()
            && !self.editing_session_name
            && !self.search_overlay.open
    }

    fn create_workspace_file(&mut self, folder: &Path, file_name: &str) -> Result<(), String> {
        if !folder.is_dir() {
            return Err(format!("Folder not found: {}", folder.display()));
        }
        if !self.path_is_in_current_workspace(folder) {
            return Err("Folder is outside the current workspace.".to_owned());
        }

        let file_name = validate_workspace_file_name(file_name)?;
        let path = folder.join(&file_name);
        if path.exists() {
            return Err(format!("File already exists: {}", path.display()));
        }

        let content = initial_workspace_file_content(&path);
        fs::write(&path, content)
            .map_err(|error| format!("Failed to create file: {} ({error})", path.display()))?;

        self.selected_workspace_path = Some(path.clone());
        self.pending_workspace_tree_reveal_path = Some(path.clone());
        self.rebuild_file_tree();
        self.open_file_in_focused_pane(path.clone());
        self.status_message = format!("Created file: {}", path.display());
        Ok(())
    }

    fn rename_workspace_file(&mut self, path: &Path, new_name: &str) -> Result<(), String> {
        if !path.is_file() {
            return Err("Select a file to rename.".to_owned());
        }
        if !self.path_is_in_current_workspace(path) {
            return Err("File is outside the current workspace.".to_owned());
        }

        let new_name = validate_workspace_file_name(new_name)?;
        let Some(parent) = path.parent() else {
            return Err("File has no parent folder.".to_owned());
        };
        let next_path = parent.join(new_name);
        if next_path == path {
            return Err("The new name is the same as the current name.".to_owned());
        }
        if next_path.exists() {
            return Err(format!("File already exists: {}", next_path.display()));
        }

        fs::rename(path, &next_path).map_err(|error| {
            format!(
                "Failed to rename file: {} -> {} ({error})",
                path.display(),
                next_path.display()
            )
        })?;

        self.remap_workspace_file_references(path, &next_path);
        self.selected_workspace_path = Some(next_path.clone());
        self.pending_workspace_tree_reveal_path = Some(next_path.clone());
        self.rebuild_file_tree();
        self.status_message = format!("Renamed file: {}", next_path.display());
        self.save_state();
        Ok(())
    }

    fn delete_workspace_file(&mut self, path: &Path) -> Result<(), String> {
        if !path.is_file() {
            return Err("Select a file to delete.".to_owned());
        }
        if !self.path_is_in_current_workspace(path) {
            return Err("File is outside the current workspace.".to_owned());
        }

        fs::remove_file(path)
            .map_err(|error| format!("Failed to delete file: {} ({error})", path.display()))?;

        self.remove_workspace_file_references(path);
        self.selected_workspace_path = None;
        self.pending_workspace_tree_reveal_path = None;
        self.rebuild_file_tree();
        self.status_message = format!("Deleted file: {}", path.display());
        self.save_state();
        Ok(())
    }

    fn remap_workspace_file_references(&mut self, old_path: &Path, new_path: &Path) {
        for editor in [&mut self.left, &mut self.right] {
            if editor.path.as_deref() == Some(old_path) {
                editor.path = Some(new_path.to_path_buf());
                editor.title = file_title(new_path);
                editor.read_only = is_external_path(new_path);
                editor.mark_text_changed();
            }
        }
        self.left_navigation_history.remap_path(old_path, new_path);
        self.right_navigation_history.remap_path(old_path, new_path);
        for recent in &mut self.recent_files {
            if recent == old_path {
                *recent = new_path.to_path_buf();
            }
        }
        if let Some(session) = self.current_session_mut() {
            for bookmark in &mut session.bookmarks {
                if bookmark.path == old_path {
                    bookmark.path = new_path.to_path_buf();
                    bookmark.label = format!(
                        "{}:{}",
                        new_path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("<invalid>"),
                        bookmark.position.line + 1
                    );
                }
            }
            session.touch();
        }
    }

    fn remove_workspace_file_references(&mut self, path: &Path) {
        if self.left.path.as_deref() == Some(path) {
            self.left = EditorPane::empty(Pane::Left);
        }
        if self.right.path.as_deref() == Some(path) {
            self.right = EditorPane::empty(Pane::Right);
        }
        self.left_navigation_history.retain_path_not(path);
        self.right_navigation_history.retain_path_not(path);
        self.recent_files.retain(|recent| recent != path);
        if let Some(session) = self.current_session_mut() {
            session.bookmarks.retain(|bookmark| bookmark.path != path);
            session.touch();
        }
    }

    fn rebuild_file_tree(&mut self) {
        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        self.file_tree = discover_workspace(&roots, FileDiscoveryOptions::default());
        self.workspace_snapshot = workspace_snapshot(&self.file_tree.files);
        self.bump_search_index_revision();
        self.next_workspace_change_poll = Instant::now() + WORKSPACE_CHANGE_POLL_INTERVAL;
        self.workspace_change_receiver = None;
        self.go_lsp_client = None;
        self.pending_go_lsp_request = None;
        self.go_lsp_synced_hashes.clear();
        self.go_lsp_request_started_at.clear();
        self.go_lsp_debug = LspDebugState::default();
        self.rust_lsp_client = None;
        self.pending_rust_lsp_request = None;
        self.rust_lsp_synced_hashes.clear();
        self.rust_lsp_request_started_at.clear();
        self.rust_lsp_debug = LspDebugState::default();
        self.external_file_count = 0;
        self.external_package_count = 0;
        self.symbol_index_receiver = Some(start_symbol_indexing(
            self.file_tree.files.clone(),
            self.language_registry.clone(),
        ));
        self.warm_up_rust_lsp_for_workspace();
        self.git_repositories_loaded = false;
        self.git_repositories.clear();
        self.status_message = format!("Discovered {} files.", self.file_tree.files.len());
    }

    fn poll_workspace_changes(&mut self, context: &egui::Context) {
        context.request_repaint_after(WORKSPACE_CHANGE_POLL_INTERVAL);

        if let Some(receiver) = self.workspace_change_receiver.take() {
            match receiver.try_recv() {
                Ok(result) => {
                    self.apply_workspace_change_scan(result);
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.workspace_change_receiver = Some(receiver);
                    context.request_repaint_after(Duration::from_millis(100));
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {}
            }
        }

        let now = Instant::now();
        if now < self.next_workspace_change_poll {
            return;
        }
        self.next_workspace_change_poll = now + WORKSPACE_CHANGE_POLL_INTERVAL;

        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        self.workspace_change_receiver = Some(start_workspace_change_scan(roots));
        context.request_repaint_after(Duration::from_millis(100));
    }

    fn apply_workspace_change_scan(&mut self, result: WorkspaceChangeScanResult) {
        let roots = result.roots;
        let next_files = result.files;
        let next_snapshot = result.snapshot;
        if next_snapshot == self.workspace_snapshot {
            return;
        }

        let diff = workspace_snapshot_diff(&self.workspace_snapshot, &next_snapshot);
        let change_count = diff.len();
        let full_symbol_reindex =
            diff.requires_full_symbol_reindex() || self.symbol_index_receiver.is_some();
        let next_file_tree = build_file_tree(&roots, next_files);
        self.file_tree = next_file_tree;
        self.workspace_snapshot = next_snapshot;
        if full_symbol_reindex {
            self.external_file_count = 0;
            self.external_package_count = 0;
            self.symbol_index_receiver = Some(start_symbol_indexing(
                self.file_tree.files.clone(),
                self.language_registry.clone(),
            ));
        } else {
            for path in &diff.removed {
                self.symbol_index.remove_file(path);
            }
            for path in diff.added.iter().chain(&diff.modified) {
                self.symbol_index
                    .replace_file(path, &self.language_registry);
            }
        }
        self.bump_search_index_revision();
        self.reload_git_repositories();
        let symbol_update = if full_symbol_reindex {
            "reindexing all symbols"
        } else {
            "updated changed symbols"
        };
        self.status_message = format!(
            "Detected {change_count} external file changes; refreshed workspace, Git, and {symbol_update}."
        );
    }

    fn create_session(&mut self) {
        let Some(root) = pick_workspace_folder("New workspace session") else {
            self.status_message = "New session canceled.".to_owned();
            return;
        };

        self.editing_session_name = false;
        self.session_name_edit.clear();
        self.pending_delete_session_id = None;
        self.app_state.create_session(root);
        self.left = EditorPane::empty(Pane::Left);
        self.right = EditorPane::empty(Pane::Right);
        self.focused_pane = Pane::Left;
        self.show_right_pane = false;
        self.dragging_workspace_file = None;
        self.git_commit_details = None;
        self.selected_workspace_path = None;
        self.pending_workspace_tree_reveal_path = None;
        self.workspace_sidebar_active = false;
        self.close_references_overlay();
        self.recent_files.clear();
        self.rebuild_file_tree();
        self.status_message = "Created a new session.".to_owned();
        self.save_state();
    }

    fn add_root_to_current_session(&mut self) {
        let Some(path) = pick_workspace_folder("Add workspace folder") else {
            self.status_message = "Add folder canceled.".to_owned();
            return;
        };

        if let Some(session) = self.current_session_mut() {
            if !session.root_folders.iter().any(|root| root == &path) {
                session.root_folders.push(path.clone());
                session.touch();
            } else {
                self.status_message = format!("Folder is already in workspace: {}", path.display());
                return;
            }
        }

        self.rebuild_file_tree();
        self.status_message = format!("Added folder: {}", path.display());
        self.save_state();
    }

    fn remove_root_from_current_session(&mut self, path: &Path) {
        let removed = self.current_session_mut().and_then(|session| {
            if session.root_folders.len() <= 1 {
                return None;
            }

            let index = session.root_folders.iter().position(|root| root == path)?;
            let removed = session.root_folders.remove(index);
            session.touch();
            Some(removed)
        });

        match removed {
            Some(path) => {
                self.rebuild_file_tree();
                self.status_message = format!("Removed folder: {}", path.display());
                self.save_state();
            }
            None => {
                self.status_message = "Keep at least one root folder in a session.".to_owned();
            }
        }
    }

    fn switch_session(&mut self, session_id: &str) {
        self.sync_open_files();
        self.pending_delete_session_id = None;
        self.editing_session_name = false;
        self.session_name_edit.clear();
        self.app_state.switch_to(session_id);

        if let Some(session) = self.current_session().cloned() {
            self.left = session
                .open_files
                .left
                .clone()
                .map(|path| {
                    editor_pane_from_session_path(Pane::Left, path, session.pane_positions.left)
                })
                .unwrap_or_else(|| EditorPane::empty(Pane::Left));
            self.right = session
                .open_files
                .right
                .clone()
                .map(|path| {
                    editor_pane_from_session_path(Pane::Right, path, session.pane_positions.right)
                })
                .unwrap_or_else(|| EditorPane::empty(Pane::Right));
            self.recent_files = recent_files_from_open_files(&session.open_files);
        }

        self.focused_pane = Pane::Left;
        self.left_navigation_history.clear();
        self.right_navigation_history.clear();
        self.definition_candidates.clear();
        self.close_references_overlay();
        self.show_right_pane = false;
        self.dragging_workspace_file = None;
        self.git_commit_details = None;
        self.selected_workspace_path = None;
        self.pending_workspace_tree_reveal_path = None;
        self.workspace_sidebar_active = false;
        self.rebuild_file_tree();
        self.status_message = "Switched session.".to_owned();
        self.save_state();
    }

    fn delete_session(&mut self, session_id: &str) {
        match self.app_state.delete_session(session_id) {
            Ok(session) => {
                self.pending_delete_session_id = None;
                self.status_message = format!("Deleted session: {}", session.display_name);
                self.save_state();
            }
            Err(error) => {
                self.pending_delete_session_id = None;
                self.status_message = error;
            }
        }
    }

    fn start_session_rename(&mut self) {
        let Some(session) = self.current_session() else {
            self.status_message = "No current session to rename.".to_owned();
            return;
        };

        self.session_name_edit = session.display_name.clone();
        self.editing_session_name = true;
        self.session_name_select_all = true;
        self.pending_delete_session_id = None;
    }

    fn cancel_session_rename(&mut self) {
        self.editing_session_name = false;
        self.session_name_edit.clear();
        self.session_name_select_all = false;
    }

    fn commit_session_rename(&mut self) {
        let name = self.session_name_edit.trim().to_owned();
        match self.app_state.rename_current_session(&name) {
            Ok(()) => {
                self.editing_session_name = false;
                self.session_name_edit.clear();
                self.session_name_select_all = false;
                self.sync_open_files();
                match save_state(&self.state_path, &self.app_state) {
                    Ok(()) => {
                        self.status_message = format!("Renamed session: {name}");
                    }
                    Err(error) => {
                        self.status_message = format!("Renamed session, but save failed: {error}");
                    }
                }
            }
            Err(error) => {
                self.status_message = error;
            }
        }
    }

    fn sync_open_files(&mut self) {
        let left = self.left.path.clone();
        let right = self.right.path.clone();
        let pane_positions = PanePositions {
            left: text_position_from_position(self.left.cursor_position),
            right: text_position_from_position(self.right.cursor_position),
        };
        if let Some(session) = self.current_session_mut() {
            session.open_files = OpenFiles { left, right };
            session.pane_positions = pane_positions;
            session.touch();
        }
    }

    fn save_state(&mut self) {
        self.sync_open_files();
        match save_state(&self.state_path, &self.app_state) {
            Ok(()) => {}
            Err(error) => {
                self.status_message = format!("Failed to save state: {error}");
            }
        }
    }

    fn save_state_silent(&mut self) {
        self.sync_open_files();
        if let Err(error) = save_state(&self.state_path, &self.app_state) {
            self.status_message = format!("Failed to save state: {error}");
        }
    }

    fn poll_symbol_index(&mut self) {
        let Some(receiver) = self.symbol_index_receiver.take() else {
            return;
        };

        match receiver.try_recv() {
            Ok(result) => {
                self.symbol_index = result.index;
                self.external_file_count = result.external_file_count;
                self.external_package_count = result.external_package_count;
                self.bump_search_index_revision();
                self.status_message =
                    format!("Indexed {} workspace symbols.", self.symbol_index.len());
            }
            Err(mpsc::TryRecvError::Empty) => {
                self.symbol_index_receiver = Some(receiver);
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.status_message = "Symbol indexing failed.".to_owned();
            }
        }
    }

    fn poll_workspace_search(&mut self, context: &egui::Context) {
        let Some(receiver) = self.workspace_search_receiver.take() else {
            return;
        };

        match receiver.try_recv() {
            Ok(result) => {
                if result.search_id == self.workspace_search.active_search_id {
                    self.workspace_search.results = result.results;
                    self.workspace_search.searched_query = result.query.clone();
                    self.workspace_search.searching = false;

                    let match_count = self.workspace_search.match_count();
                    let file_count = self.workspace_search.results.len();
                    self.status_message =
                        format!("Workspace search: {match_count} results in {file_count} files.");
                }
            }
            Err(mpsc::TryRecvError::Empty) => {
                context.request_repaint_after(Duration::from_millis(100));
                self.workspace_search_receiver = Some(receiver);
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.workspace_search.searching = false;
                self.status_message = "Workspace search failed.".to_owned();
            }
        }
    }

    fn go_to_definition(&mut self) {
        let editor = self.focused_editor();
        let Some(path) = editor.path.clone() else {
            self.status_message = "No file is open in the focused pane.".to_owned();
            return;
        };
        let Some(position) = editor.cursor_position else {
            self.status_message = "Place the cursor on a symbol before F12.".to_owned();
            return;
        };
        let Some(language) = self.language_registry.language_for_path(&path) else {
            self.status_message = "No language support for this file.".to_owned();
            return;
        };

        let document = Document::new(path.clone(), editor.text.as_str().to_owned());
        let identifier = language
            .identifier_at(&document, position)
            .map(|identifier| identifier.name)
            .unwrap_or_else(|| "<unknown>".to_owned());
        if language.id().0 == "go" {
            self.definition_candidates.clear();
            self.location_list_kind = LocationListKind::Definitions;
            self.close_references_overlay();
            self.request_go_lsp_definition(document.clone(), position, identifier.clone());
            return;
        }
        if language.id().0 == "rust" {
            self.definition_candidates.clear();
            self.location_list_kind = LocationListKind::Definitions;
            self.close_references_overlay();
            self.request_rust_lsp_definition(document.clone(), position, identifier.clone());
            return;
        }

        let workspace = Workspace {
            roots: self
                .current_session()
                .map(|session| session.root_folders.clone())
                .unwrap_or_default(),
        };
        let mut symbols = self.symbol_index.symbols().to_vec();
        symbols.extend(language.extract_symbols(&document));
        let context = crate::core::language::ResolveContext {
            workspace: &workspace,
            symbols: &symbols,
        };
        let mut locations = resolve_definition(language, &document, position, &context);
        if locations.is_empty() && identifier != "<unknown>" {
            let resolution = resolve_external_definition_on_demand(
                &workspace.roots,
                &identifier,
                None,
                &mut self.symbol_index,
                &self.language_registry,
            );
            if resolution.indexed_file_count > 0 {
                self.bump_search_index_revision();
                self.external_file_count =
                    self.external_file_count.max(resolution.external_file_count);
                self.external_package_count = self
                    .external_package_count
                    .max(resolution.external_package_count);
            }
            locations = resolution.locations;
        }
        locations.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.range.start.line.cmp(&right.range.start.line))
                .then_with(|| left.range.start.character.cmp(&right.range.start.character))
        });
        locations.dedup();

        self.handle_locations(&identifier, locations, LocationListKind::Definitions);
    }

    fn ensure_go_lsp_client(&mut self) {
        if self.go_lsp_client.is_some() {
            return;
        }
        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        self.go_lsp_client = Some(platform::current().start_lsp(LspServerConfig::go(roots)));
        self.go_lsp_debug.last_status = "starting".to_owned();
    }

    fn sync_open_go_documents_to_lsp(&mut self) {
        let documents = [Pane::Left, Pane::Right]
            .into_iter()
            .filter_map(|pane| {
                let editor = self.editor(pane);
                let path = editor.path.as_ref()?;
                if !is_go_path(path) {
                    return None;
                }
                Some(Document::new(path.clone(), editor.text.as_str().to_owned()))
            })
            .collect::<Vec<_>>();

        for document in documents {
            self.sync_go_document_to_lsp(document);
        }
    }

    fn sync_go_document_to_lsp(&mut self, document: Document) {
        let text_hash = stable_hash(document.text.as_str());
        if self.go_lsp_synced_hashes.get(&document.path) == Some(&text_hash) {
            return;
        }

        self.ensure_go_lsp_client();
        let result = self
            .go_lsp_client
            .as_mut()
            .expect("Go LSP client is initialized")
            .sync_document(document.clone());

        match result {
            Ok(request_id) => {
                self.go_lsp_synced_hashes
                    .insert(document.path.clone(), text_hash);
                self.record_go_lsp_request_started(request_id, LspRequestKind::Sync);
                self.go_lsp_debug.sync_count += 1;
                self.go_lsp_debug.last_status = format!("sync_queued {}", document.path.display());
            }
            Err(error) => {
                self.go_lsp_debug.last_status = "sync_queue_failed".to_owned();
                self.go_lsp_debug.last_error = Some(error);
            }
        }
    }

    fn record_go_lsp_request_started(&mut self, request_id: u64, kind: LspRequestKind) -> Instant {
        let started_at = Instant::now();
        self.go_lsp_request_started_at
            .insert(request_id, started_at);
        self.go_lsp_debug.last_request_id = Some(request_id);
        self.go_lsp_debug.last_kind = Some(kind);
        started_at
    }

    fn request_go_lsp_definition(
        &mut self,
        document: Document,
        position: Position,
        identifier: String,
    ) -> bool {
        self.ensure_go_lsp_client();
        let origin = self.focused_editor().navigation_point();

        let Some(client) = &mut self.go_lsp_client else {
            return false;
        };
        match client.request_definition(document.clone(), position) {
            Ok(request_id) => {
                let started_at =
                    self.record_go_lsp_request_started(request_id, LspRequestKind::Definition);
                self.pending_go_lsp_request = Some(PendingGoLspRequest {
                    request_id,
                    kind: LspRequestKind::Definition,
                    document,
                    position,
                    identifier: identifier.clone(),
                    origin,
                    started_at,
                });
                self.definition_candidates.clear();
                self.location_list_kind = LocationListKind::Definitions;
                self.go_lsp_debug.definition_count += 1;
                self.go_lsp_debug.last_status = "definition_queued".to_owned();
                self.go_lsp_debug
                    .record_definition_status("definition_queued", None, None, 0);
                self.status_message = format!("Resolving `{identifier}` with gopls...");
                true
            }
            Err(error) => {
                self.go_lsp_debug.last_status = "definition_queue_failed".to_owned();
                self.go_lsp_debug.last_error = Some(error.clone());
                self.go_lsp_debug.record_definition_status(
                    "definition_queue_failed",
                    Some(error.clone()),
                    None,
                    0,
                );
                self.definition_candidates.clear();
                self.location_list_kind = LocationListKind::Definitions;
                self.status_message = format!("Go LSP unavailable. ({error})");
                false
            }
        }
    }

    fn request_go_lsp_references(&mut self, pending: PendingGoLspRequest) -> bool {
        let Some(client) = &mut self.go_lsp_client else {
            return false;
        };
        match client.request_references(pending.document.clone(), pending.position) {
            Ok(request_id) => {
                let started_at =
                    self.record_go_lsp_request_started(request_id, LspRequestKind::References);
                let identifier = pending.identifier.clone();
                self.pending_go_lsp_request = Some(PendingGoLspRequest {
                    request_id,
                    kind: LspRequestKind::References,
                    document: pending.document,
                    position: pending.position,
                    identifier: identifier.clone(),
                    origin: pending.origin,
                    started_at,
                });
                self.go_lsp_debug.reference_count += 1;
                self.go_lsp_debug.last_status = "references_queued".to_owned();
                self.status_message =
                    format!("Finding references for `{identifier}` with gopls...");
                true
            }
            Err(error) => {
                self.go_lsp_debug.last_status = "references_queue_failed".to_owned();
                self.go_lsp_debug.last_error = Some(error.clone());
                self.status_message = format!("Go references request failed. ({error})");
                false
            }
        }
    }

    fn poll_go_lsp(&mut self, context: &egui::Context) {
        self.sync_open_go_documents_to_lsp();

        let mut responses = Vec::new();
        if let Some(client) = &self.go_lsp_client {
            while let Some(event) = client.try_recv() {
                match event {
                    LspClientEvent::Response(response) => responses.push(response),
                    LspClientEvent::ServerEvent(event) => {
                        self.go_lsp_debug.record_server_event(&event);
                    }
                }
            }
        }

        for response in responses {
            self.handle_go_lsp_response(response);
        }

        let Some(pending) = self.pending_go_lsp_request.clone() else {
            return;
        };
        if pending.started_at.elapsed() < GO_LSP_REQUEST_TIMEOUT {
            context.request_repaint_after(Duration::from_millis(50));
            return;
        }

        self.pending_go_lsp_request = None;
        self.go_lsp_request_started_at.remove(&pending.request_id);
        self.go_lsp_debug.timeout_count += 1;
        self.go_lsp_debug.last_duration_ms =
            Some(pending.started_at.elapsed().as_secs_f64() * 1000.0);
        match pending.kind {
            LspRequestKind::WarmUp => {}
            LspRequestKind::Definition => {
                self.go_lsp_debug.last_status = "definition_timeout".to_owned();
                self.go_lsp_debug.last_location_count = 0;
                self.go_lsp_debug.record_definition_status(
                    "definition_timeout",
                    None,
                    self.go_lsp_debug.last_duration_ms,
                    0,
                );
                self.definition_candidates.clear();
                self.location_list_kind = LocationListKind::Definitions;
                self.status_message = format!(
                    "gopls timed out for `{}`. Wait a moment and retry F12.",
                    pending.identifier
                );
            }
            LspRequestKind::References => {
                self.definition_candidates.clear();
                self.close_references_overlay();
                self.location_list_kind = LocationListKind::References;
                self.go_lsp_debug.last_status = "references_timeout".to_owned();
                self.status_message =
                    format!("gopls references timed out for `{}`.", pending.identifier);
            }
            LspRequestKind::Sync => {}
        }
    }

    fn handle_go_lsp_response(&mut self, response: LspLocationResponse) {
        if response.kind == LspRequestKind::Sync {
            self.handle_go_lsp_sync_response(response);
            return;
        }

        let Some(pending) = self.pending_go_lsp_request.clone() else {
            return;
        };
        if response.request_id != pending.request_id || response.kind != pending.kind {
            return;
        }
        self.pending_go_lsp_request = None;
        let duration_ms = self
            .go_lsp_request_started_at
            .remove(&response.request_id)
            .map(|started_at| started_at.elapsed().as_secs_f64() * 1000.0);
        self.go_lsp_debug.last_duration_ms = duration_ms;

        let locations = match response.result {
            Ok(locations) => {
                self.go_lsp_debug.last_status = match response.kind {
                    LspRequestKind::WarmUp => "warmup_ok",
                    LspRequestKind::Definition => "definition_ok",
                    LspRequestKind::References => "references_ok",
                    LspRequestKind::Sync => "sync_ok",
                }
                .to_owned();
                self.go_lsp_debug.last_error = None;
                self.go_lsp_debug.last_location_count = locations.len();
                if response.kind == LspRequestKind::Definition {
                    self.go_lsp_debug.record_definition_status(
                        "definition_ok",
                        None,
                        duration_ms,
                        locations.len(),
                    );
                }
                locations
            }
            Err(error) => {
                self.go_lsp_debug.last_error = Some(error.clone());
                self.go_lsp_debug.last_status = match response.kind {
                    LspRequestKind::WarmUp => "warmup_failed",
                    LspRequestKind::Definition => "definition_failed",
                    LspRequestKind::References => "references_failed",
                    LspRequestKind::Sync => "sync_failed",
                }
                .to_owned();
                if pending.kind == LspRequestKind::Definition {
                    self.go_lsp_debug.record_definition_status(
                        "definition_failed",
                        Some(error.clone()),
                        duration_ms,
                        0,
                    );
                }
                self.definition_candidates.clear();
                self.close_references_overlay();
                self.location_list_kind = LocationListKind::Definitions;
                self.status_message =
                    format!("Go LSP failed for `{}`. ({error})", pending.identifier);
                return;
            }
        };

        match pending.kind {
            LspRequestKind::Definition => {
                if locations.len() == 1
                    && location_contains_position(
                        &locations[0],
                        &pending.document.path,
                        pending.position,
                    )
                {
                    if self.request_go_lsp_references(pending.clone()) {
                        return;
                    }
                }
                self.handle_locations_with_origin(
                    &pending.identifier,
                    locations,
                    LocationListKind::Definitions,
                    pending.origin.clone(),
                );
            }
            LspRequestKind::References => {
                self.handle_locations_with_origin(
                    &pending.identifier,
                    locations,
                    LocationListKind::References,
                    pending.origin.clone(),
                );
            }
            LspRequestKind::WarmUp => {}
            LspRequestKind::Sync => {}
        }
    }

    fn handle_go_lsp_sync_response(&mut self, response: LspLocationResponse) {
        self.go_lsp_debug.last_duration_ms = self
            .go_lsp_request_started_at
            .remove(&response.request_id)
            .map(|started_at| started_at.elapsed().as_secs_f64() * 1000.0);
        self.go_lsp_debug.last_request_id = Some(response.request_id);
        self.go_lsp_debug.last_kind = Some(response.kind);
        match response.result {
            Ok(locations) => {
                self.go_lsp_debug.last_status = "sync_ok".to_owned();
                self.go_lsp_debug.last_error = None;
                self.go_lsp_debug.last_location_count = locations.len();
            }
            Err(error) => {
                self.go_lsp_debug.last_status = "sync_failed".to_owned();
                self.go_lsp_debug.last_error = Some(error);
            }
        }
    }

    fn ensure_rust_lsp_client(&mut self) {
        if self.rust_lsp_client.is_some() {
            return;
        }
        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        self.rust_lsp_client =
            Some(platform::current().start_lsp(LspServerConfig::rust_analyzer(roots)));
        self.rust_lsp_debug.mark_started();
    }

    fn warm_up_rust_lsp_for_workspace(&mut self) {
        if !self.workspace_has_rust_source() {
            return;
        }

        self.ensure_rust_lsp_client();
        let Some(client) = &mut self.rust_lsp_client else {
            return;
        };

        match client.warm_up() {
            Ok(request_id) => {
                self.record_rust_lsp_request_started(request_id, LspRequestKind::WarmUp);
                self.rust_lsp_debug.warmup_count += 1;
                self.rust_lsp_debug.last_status = "warmup_queued".to_owned();
            }
            Err(error) => {
                self.rust_lsp_debug.last_status = "warmup_queue_failed".to_owned();
                self.rust_lsp_debug.last_error = Some(error);
            }
        }
    }

    fn workspace_has_rust_source(&self) -> bool {
        self.file_tree
            .files
            .iter()
            .any(|path| is_rust_path(path) && !is_external_rust_path(path))
    }

    fn sync_open_rust_documents_to_lsp(&mut self) {
        let documents = [Pane::Left, Pane::Right]
            .into_iter()
            .filter_map(|pane| {
                let editor = self.editor(pane);
                let path = editor.path.as_ref()?;
                if !is_rust_path(path) || is_external_rust_path(path) {
                    return None;
                }
                Some(Document::new(path.clone(), editor.text.as_str().to_owned()))
            })
            .collect::<Vec<_>>();

        for document in documents {
            self.sync_rust_document_to_lsp(document);
        }
    }

    fn sync_rust_document_to_lsp(&mut self, document: Document) {
        let text_hash = stable_hash(document.text.as_str());
        if self.rust_lsp_synced_hashes.get(&document.path) == Some(&text_hash) {
            return;
        }

        self.ensure_rust_lsp_client();
        let result = self
            .rust_lsp_client
            .as_mut()
            .expect("Rust LSP client is initialized")
            .sync_document(document.clone());

        match result {
            Ok(request_id) => {
                self.rust_lsp_synced_hashes
                    .insert(document.path.clone(), text_hash);
                self.record_rust_lsp_request_started(request_id, LspRequestKind::Sync);
                self.rust_lsp_debug.sync_count += 1;
                self.rust_lsp_debug.last_status =
                    format!("sync_queued {}", document.path.display());
            }
            Err(error) => {
                self.rust_lsp_debug.last_status = "sync_queue_failed".to_owned();
                self.rust_lsp_debug.last_error = Some(error);
            }
        }
    }

    fn record_rust_lsp_request_started(
        &mut self,
        request_id: u64,
        kind: LspRequestKind,
    ) -> Instant {
        let started_at = Instant::now();
        self.rust_lsp_request_started_at
            .insert(request_id, started_at);
        self.rust_lsp_debug.last_request_id = Some(request_id);
        self.rust_lsp_debug.last_kind = Some(kind);
        started_at
    }

    fn request_rust_lsp_definition(
        &mut self,
        document: Document,
        position: Position,
        identifier: String,
    ) -> bool {
        self.ensure_rust_lsp_client();
        let origin = self.focused_editor().navigation_point();

        let Some(client) = &mut self.rust_lsp_client else {
            return false;
        };
        match client.request_definition(document.clone(), position) {
            Ok(request_id) => {
                let started_at =
                    self.record_rust_lsp_request_started(request_id, LspRequestKind::Definition);
                self.pending_rust_lsp_request = Some(PendingGoLspRequest {
                    request_id,
                    kind: LspRequestKind::Definition,
                    document,
                    position,
                    identifier: identifier.clone(),
                    origin,
                    started_at,
                });
                self.definition_candidates.clear();
                self.location_list_kind = LocationListKind::Definitions;
                self.rust_lsp_debug.definition_count += 1;
                self.rust_lsp_debug.last_status = "definition_queued".to_owned();
                self.rust_lsp_debug
                    .record_definition_status("definition_queued", None, None, 0);
                self.status_message = format!("Resolving `{identifier}` with rust-analyzer...");
                true
            }
            Err(error) => {
                self.rust_lsp_debug.last_status = "definition_queue_failed".to_owned();
                self.rust_lsp_debug.last_error = Some(error.clone());
                self.rust_lsp_debug.record_definition_status(
                    "definition_queue_failed",
                    Some(error.clone()),
                    None,
                    0,
                );
                self.status_message = format!("Rust LSP unavailable. ({error})");
                false
            }
        }
    }

    fn request_rust_lsp_references(&mut self, pending: PendingGoLspRequest) -> bool {
        let Some(client) = &mut self.rust_lsp_client else {
            return false;
        };
        match client.request_references(pending.document.clone(), pending.position) {
            Ok(request_id) => {
                let started_at =
                    self.record_rust_lsp_request_started(request_id, LspRequestKind::References);
                let identifier = pending.identifier.clone();
                self.pending_rust_lsp_request = Some(PendingGoLspRequest {
                    request_id,
                    kind: LspRequestKind::References,
                    document: pending.document,
                    position: pending.position,
                    identifier: identifier.clone(),
                    origin: pending.origin,
                    started_at,
                });
                self.rust_lsp_debug.reference_count += 1;
                self.rust_lsp_debug.last_status = "references_queued".to_owned();
                self.status_message =
                    format!("Finding references for `{identifier}` with rust-analyzer...");
                true
            }
            Err(error) => {
                self.rust_lsp_debug.last_status = "references_queue_failed".to_owned();
                self.rust_lsp_debug.last_error = Some(error.clone());
                self.status_message = format!("Rust references request failed. ({error})");
                false
            }
        }
    }

    fn poll_rust_lsp(&mut self, context: &egui::Context) {
        self.sync_open_rust_documents_to_lsp();

        let mut responses = Vec::new();
        if let Some(client) = &self.rust_lsp_client {
            while let Some(event) = client.try_recv() {
                match event {
                    LspClientEvent::Response(response) => responses.push(response),
                    LspClientEvent::ServerEvent(event) => {
                        self.rust_lsp_debug.record_server_event(&event);
                    }
                }
            }
        }

        for response in responses {
            self.handle_rust_lsp_response(response);
        }

        let Some(pending) = self.pending_rust_lsp_request.clone() else {
            return;
        };
        if pending.started_at.elapsed() < RUST_LSP_REQUEST_TIMEOUT {
            context.request_repaint_after(Duration::from_millis(50));
            return;
        }

        self.pending_rust_lsp_request = None;
        self.rust_lsp_request_started_at.remove(&pending.request_id);
        self.rust_lsp_debug.timeout_count += 1;
        self.rust_lsp_debug.last_duration_ms =
            Some(pending.started_at.elapsed().as_secs_f64() * 1000.0);
        match pending.kind {
            LspRequestKind::WarmUp => {}
            LspRequestKind::Definition => {
                self.rust_lsp_debug.last_status = "definition_timeout".to_owned();
                self.rust_lsp_debug.record_definition_status(
                    "definition_timeout",
                    None,
                    self.rust_lsp_debug.last_duration_ms,
                    0,
                );
                self.definition_candidates.clear();
                self.location_list_kind = LocationListKind::Definitions;
                self.status_message = self.rust_lsp_definition_pending_message(
                    &pending.identifier,
                    "did not answer before timeout",
                );
            }
            LspRequestKind::References => {
                self.definition_candidates.clear();
                self.close_references_overlay();
                self.location_list_kind = LocationListKind::References;
                self.rust_lsp_debug.last_status = "references_timeout".to_owned();
                self.status_message = format!(
                    "rust-analyzer references timed out for `{}`.",
                    pending.identifier
                );
            }
            LspRequestKind::Sync => {}
        }
    }

    fn handle_rust_lsp_response(&mut self, response: LspLocationResponse) {
        if response.kind == LspRequestKind::WarmUp {
            self.handle_rust_lsp_warmup_response(response);
            return;
        }
        if response.kind == LspRequestKind::Sync {
            self.handle_rust_lsp_sync_response(response);
            return;
        }

        let Some(pending) = self.pending_rust_lsp_request.clone() else {
            return;
        };
        if response.request_id != pending.request_id || response.kind != pending.kind {
            return;
        }
        self.pending_rust_lsp_request = None;
        let duration_ms = self
            .rust_lsp_request_started_at
            .remove(&response.request_id)
            .map(|started_at| started_at.elapsed().as_secs_f64() * 1000.0);
        self.rust_lsp_debug.last_duration_ms = duration_ms;

        let locations = match response.result {
            Ok(locations) => {
                self.rust_lsp_debug.last_status = match response.kind {
                    LspRequestKind::WarmUp => "warmup_ok",
                    LspRequestKind::Definition => "definition_ok",
                    LspRequestKind::References => "references_ok",
                    LspRequestKind::Sync => "sync_ok",
                }
                .to_owned();
                self.rust_lsp_debug.last_error = None;
                self.rust_lsp_debug.last_location_count = locations.len();
                if response.kind == LspRequestKind::Definition {
                    self.rust_lsp_debug.record_definition_status(
                        "definition_ok",
                        None,
                        duration_ms,
                        locations.len(),
                    );
                }
                locations
            }
            Err(error) => {
                self.rust_lsp_debug.last_error = Some(error.clone());
                self.rust_lsp_debug.last_status = match response.kind {
                    LspRequestKind::WarmUp => "warmup_failed",
                    LspRequestKind::Definition => "definition_failed",
                    LspRequestKind::References => "references_failed",
                    LspRequestKind::Sync => "sync_failed",
                }
                .to_owned();
                if pending.kind == LspRequestKind::Definition {
                    self.rust_lsp_debug.record_definition_status(
                        "definition_failed",
                        Some(error.clone()),
                        duration_ms,
                        0,
                    );
                }
                self.definition_candidates.clear();
                self.close_references_overlay();
                self.location_list_kind = LocationListKind::Definitions;
                self.status_message =
                    format!("Rust LSP failed for `{}`. ({error})", pending.identifier);
                return;
            }
        };

        match pending.kind {
            LspRequestKind::Definition => {
                if locations.is_empty() {
                    self.rust_lsp_debug.last_status = "definition_empty".to_owned();
                    self.rust_lsp_debug.record_definition_status(
                        "definition_empty",
                        None,
                        duration_ms,
                        0,
                    );
                    self.definition_candidates.clear();
                    self.location_list_kind = LocationListKind::Definitions;
                    self.status_message = self.rust_lsp_definition_pending_message(
                        &pending.identifier,
                        "returned no definition",
                    );
                    return;
                }
                if locations.len() == 1
                    && location_contains_position(
                        &locations[0],
                        &pending.document.path,
                        pending.position,
                    )
                {
                    if self.request_rust_lsp_references(pending.clone()) {
                        return;
                    }
                }
                self.handle_locations_with_origin(
                    &pending.identifier,
                    locations,
                    LocationListKind::Definitions,
                    pending.origin.clone(),
                );
            }
            LspRequestKind::References => {
                self.handle_locations_with_origin(
                    &pending.identifier,
                    locations,
                    LocationListKind::References,
                    pending.origin.clone(),
                );
            }
            LspRequestKind::WarmUp => {}
            LspRequestKind::Sync => {}
        }
    }

    fn handle_rust_lsp_warmup_response(&mut self, response: LspLocationResponse) {
        self.rust_lsp_debug.last_duration_ms = self
            .rust_lsp_request_started_at
            .remove(&response.request_id)
            .map(|started_at| started_at.elapsed().as_secs_f64() * 1000.0);
        self.rust_lsp_debug.last_request_id = Some(response.request_id);
        self.rust_lsp_debug.last_kind = Some(response.kind);
        match response.result {
            Ok(locations) => {
                self.rust_lsp_debug.last_status = "warmup_ok".to_owned();
                self.rust_lsp_debug.last_error = None;
                self.rust_lsp_debug.last_location_count = locations.len();
            }
            Err(error) => {
                self.rust_lsp_debug.last_status = "warmup_failed".to_owned();
                self.rust_lsp_debug.last_error = Some(error);
            }
        }
    }

    fn handle_rust_lsp_sync_response(&mut self, response: LspLocationResponse) {
        self.rust_lsp_debug.last_duration_ms = self
            .rust_lsp_request_started_at
            .remove(&response.request_id)
            .map(|started_at| started_at.elapsed().as_secs_f64() * 1000.0);
        self.rust_lsp_debug.last_request_id = Some(response.request_id);
        self.rust_lsp_debug.last_kind = Some(response.kind);
        match response.result {
            Ok(locations) => {
                self.rust_lsp_debug.last_status = "sync_ok".to_owned();
                self.rust_lsp_debug.last_error = None;
                self.rust_lsp_debug.last_location_count = locations.len();
            }
            Err(error) => {
                self.rust_lsp_debug.last_status = "sync_failed".to_owned();
                self.rust_lsp_debug.last_error = Some(error);
            }
        }
    }

    fn rust_lsp_definition_pending_message(&self, identifier: &str, reason: &str) -> String {
        let mut message = format!(
            "rust-analyzer {reason} for `{identifier}`. It may still be indexing workspace/dependencies; wait a moment and retry F12."
        );
        if !self.rust_lsp_debug.initialized {
            message.push_str(" Current state: starting.");
        } else if let Some(server_message) = &self.rust_lsp_debug.last_server_message
            && !server_message.is_empty()
        {
            message.push_str(&format!(" Last server message: {server_message}"));
        }
        message
    }

    fn status_bar_message(&self) -> String {
        if self.status_message.contains("rust-analyzer") {
            return self.status_message.clone();
        }

        let Some(rust_lsp_status) = self.rust_lsp_readiness_status_message() else {
            return self.status_message.clone();
        };

        if self.status_message.is_empty() {
            rust_lsp_status
        } else {
            format!("{} | {}", self.status_message, rust_lsp_status)
        }
    }

    fn rust_lsp_readiness_status_message(&self) -> Option<String> {
        if self.rust_lsp_client.is_none() {
            return None;
        }
        if !self.has_open_workspace_rust_document() && self.pending_rust_lsp_request.is_none() {
            return None;
        }

        let status = self.rust_lsp_readiness_status();
        Some(format!("rust-analyzer {}", status.status_bar_label()))
    }

    fn rust_lsp_readiness_status(&self) -> RustLspReadinessStatus {
        if self.rust_lsp_client.is_none() {
            return RustLspReadinessStatus::NotStarted;
        }

        if self.rust_lsp_debug.reader_error_count > 0
            && self.rust_lsp_debug.last_status == "reader_error"
        {
            return RustLspReadinessStatus::Error;
        }

        if !self.rust_lsp_debug.initialized {
            return RustLspReadinessStatus::WarmingUp("starting");
        }

        if self.rust_lsp_debug.active_progress_count > 0 {
            return RustLspReadinessStatus::WarmingUp("indexing");
        }

        if self.rust_lsp_debug.last_definition_success_at.is_some() {
            return RustLspReadinessStatus::Ready("definition");
        }

        if self.rust_lsp_debug.last_diagnostics_at.is_some() {
            return RustLspReadinessStatus::Ready("diagnostics");
        }

        if self.rust_lsp_debug.sync_count > 0 || self.rust_lsp_debug.warmup_count > 0 {
            return RustLspReadinessStatus::WarmingUp("workspace/dependencies");
        }

        RustLspReadinessStatus::WarmingUp("initializing")
    }

    fn rust_lsp_readiness_report_line(&self) -> String {
        format!(
            "rust_lsp_readiness={} active_progress={} started_age_ms={} initialized_age_ms={} last_progress_age_ms={} last_diagnostics_age_ms={} last_definition_success_age_ms={}",
            self.rust_lsp_readiness_status().report_label(),
            self.rust_lsp_debug.active_progress_count,
            debug_instant_age_ms(self.rust_lsp_debug.started_at),
            debug_instant_age_ms(self.rust_lsp_debug.initialized_at),
            debug_instant_age_ms(self.rust_lsp_debug.last_progress_at),
            debug_instant_age_ms(self.rust_lsp_debug.last_diagnostics_at),
            debug_instant_age_ms(self.rust_lsp_debug.last_definition_success_at)
        )
    }

    fn has_open_workspace_rust_document(&self) -> bool {
        [Pane::Left, Pane::Right].into_iter().any(|pane| {
            self.editor(pane)
                .path
                .as_deref()
                .is_some_and(|path| is_rust_path(path) && !is_external_rust_path(path))
        })
    }

    fn handle_locations(
        &mut self,
        identifier: &str,
        locations: Vec<Location>,
        kind: LocationListKind,
    ) {
        self.handle_locations_with_origin(identifier, locations, kind, None);
    }

    fn handle_locations_with_origin(
        &mut self,
        identifier: &str,
        locations: Vec<Location>,
        kind: LocationListKind,
        origin: Option<NavigationPoint>,
    ) {
        self.location_list_kind = kind;
        match locations.len() {
            0 => {
                self.definition_candidates.clear();
                self.close_references_overlay();
                self.status_message =
                    format!("No {} found for `{identifier}`.", kind.singular_label());
            }
            1 => {
                self.definition_candidates.clear();
                self.close_references_overlay();
                self.open_definition_location_with_origin(locations[0].clone(), origin);
                self.status_message =
                    format!("Jumped to {} of `{identifier}`.", kind.singular_label());
            }
            _ => {
                self.definition_candidates = locations
                    .into_iter()
                    .map(|location| {
                        let preview = (kind == LocationListKind::References)
                            .then(|| location_line_preview(&location))
                            .flatten();
                        DefinitionCandidate {
                            label: location_result_label_with_preview(
                                &location,
                                kind,
                                preview.as_deref(),
                            ),
                            location,
                            preview,
                        }
                    })
                    .collect();
                if kind == LocationListKind::References {
                    self.open_references_overlay(identifier);
                    self.status_message = format!(
                        "Found {} references for `{identifier}`.",
                        self.definition_candidates.len()
                    );
                } else {
                    self.close_references_overlay();
                    self.status_message = format!(
                        "Multiple definitions returned for `{identifier}`. Use Report for details."
                    );
                }
            }
        }
    }

    fn open_definition_location(&mut self, location: Location) {
        self.open_definition_location_with_origin(location, None);
    }

    fn open_definition_location_with_origin(
        &mut self,
        location: Location,
        origin: Option<NavigationPoint>,
    ) {
        self.open_location_in_focused_pane_with_origin(location, true, origin);
    }

    fn open_location_in_focused_pane(&mut self, location: Location, push_history: bool) {
        self.open_location_in_focused_pane_with_origin(location, push_history, None);
    }

    fn open_location_in_focused_pane_with_origin(
        &mut self,
        location: Location,
        push_history: bool,
        origin: Option<NavigationPoint>,
    ) {
        let history_point = origin.or_else(|| self.focused_editor().navigation_point());
        if let Some(current) = history_point {
            if push_history {
                self.push_focused_navigation_history(current);
            }
        }

        self.open_navigation_point_in_focused_pane(NavigationPoint {
            path: location.path,
            position: location.range.start,
        });
    }

    fn open_navigation_point_in_focused_pane(&mut self, point: NavigationPoint) {
        let mut next = EditorPane::from_path(self.focused_pane, point.path.clone());
        next.target_position = Some(point.position);
        next.pending_navigation = Some(PendingEditorNavigation {
            position: point.position,
            focus_editor: true,
        });
        next.cursor_position = Some(point.position);
        let opened_path = next.path.clone();

        if let Some(path) = opened_path.as_deref() {
            if is_external_rust_path(path) {
                // Rust definition navigation is handled by rust-analyzer. Indexing external
                // Rust sources here can be very expensive and is no longer needed for F12.
            } else if is_external_path(path) {
                self.symbol_index
                    .add_external_file(path, &self.language_registry);
            } else {
                self.symbol_index.add_file(path, &self.language_registry);
            }
            self.remember_recent_file(path.to_path_buf());
            self.reveal_workspace_file_in_tree(path);
        }

        *self.focused_editor_mut() = next;
        self.sync_open_files();
        self.save_state();
    }

    fn go_back(&mut self) {
        let current = self.focused_editor().navigation_point();
        let Some(previous) = self.focused_navigation_history_mut().go_back(current) else {
            self.status_message = "No previous navigation.".to_owned();
            return;
        };

        self.open_navigation_point_in_focused_pane(previous);
        self.status_message = "Returned to previous location.".to_owned();
    }

    fn go_forward(&mut self) {
        let current = self.focused_editor().navigation_point();
        let Some(next) = self.focused_navigation_history_mut().go_forward(current) else {
            self.status_message = "No next navigation.".to_owned();
            return;
        };

        self.open_navigation_point_in_focused_pane(next);
        self.status_message = "Moved to next location.".to_owned();
    }

    fn open_navigation_history_entry(&mut self, index: usize) {
        let current = self.focused_editor().navigation_point();
        let Some(point) = self
            .focused_navigation_history_mut()
            .open_back_entry(index, current)
        else {
            self.status_message = "Navigation history entry is no longer available.".to_owned();
            return;
        };

        let label = navigation_point_label(&point);
        self.open_navigation_point_in_focused_pane(point);
        self.status_message = format!("Opened navigation history: {label}");
    }

    fn add_bookmark_from_current_position(&mut self, temporary: bool) {
        let Some(path) = self.focused_editor().path.clone() else {
            self.status_message = "Open a file before adding a bookmark.".to_owned();
            return;
        };
        let position = self
            .focused_editor()
            .cursor_position
            .unwrap_or(Position::new(0, 0));
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("<invalid>");
        let label = format!("{file_name}:{}", position.line + 1);
        let bookmark = SessionBookmark {
            label,
            path: path.clone(),
            position: TextPosition::new(position.line, position.character),
            temporary,
        };

        if let Some(session) = self.current_session_mut() {
            session.bookmarks.retain(|existing| {
                existing.path != bookmark.path
                    || existing.position != bookmark.position
                    || existing.temporary != bookmark.temporary
            });
            session.bookmarks.insert(0, bookmark);
            session.touch();
            self.bump_search_index_revision();
        }

        self.status_message = if temporary {
            "Added temporary bookmark.".to_owned()
        } else {
            "Added bookmark.".to_owned()
        };
        if !temporary {
            self.save_state();
        }
    }

    fn bookmark_lines_for_path(&self, path: Option<&Path>) -> Vec<u32> {
        let Some(path) = path else {
            return Vec::new();
        };
        let mut lines = self
            .current_session()
            .map(|session| {
                session
                    .bookmarks
                    .iter()
                    .filter(|bookmark| bookmark.path == path)
                    .map(|bookmark| bookmark.position.line)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        lines.sort_unstable();
        lines.dedup();
        lines
    }

    fn toggle_bookmark_at_line(&mut self, path: PathBuf, line: u32) {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("<invalid>");
        let label = format!("{file_name}:{}", line + 1);
        let mut removed_count = 0;

        if let Some(session) = self.current_session_mut() {
            let before = session.bookmarks.len();
            session
                .bookmarks
                .retain(|bookmark| bookmark.path != path || bookmark.position.line != line);
            removed_count = before.saturating_sub(session.bookmarks.len());

            if removed_count == 0 {
                session.bookmarks.insert(
                    0,
                    SessionBookmark {
                        label: label.clone(),
                        path,
                        position: TextPosition::new(line, 0),
                        temporary: false,
                    },
                );
            }
            session.touch();
            self.bump_search_index_revision();
        }

        if removed_count == 0 {
            self.status_message = format!("Added bookmark: {label}");
        } else {
            self.status_message = format!("Removed bookmark: {label}");
        }
        self.save_state();
    }

    fn open_bookmark(&mut self, bookmark: SessionBookmark) {
        let position = Position::new(bookmark.position.line, bookmark.position.character);
        let location = Location::new(bookmark.path, Range::new(position, position));
        self.open_location_in_focused_pane(location, true);
        self.status_message = format!("Opened bookmark: {}", bookmark.label);
    }

    fn remove_bookmark(&mut self, index: usize) {
        let mut removed = None;
        if let Some(session) = self.current_session_mut()
            && index < session.bookmarks.len()
        {
            removed = Some(session.bookmarks.remove(index));
            session.touch();
        }

        if let Some(removed) = removed {
            self.bump_search_index_revision();
            self.status_message = format!("Removed bookmark: {}", removed.label);
            if !removed.temporary {
                self.save_state();
            }
        }
    }

    fn handle_editor_action(&mut self, action: EditorAction) {
        match action {
            EditorAction::GoToDefinition => self.go_to_definition(),
            EditorAction::Back => self.go_back(),
            EditorAction::OpenNavigationHistory { index } => {
                self.open_navigation_history_entry(index);
            }
            EditorAction::OpenGitCommitDetails {
                repository_name,
                root,
                commit,
            } => {
                self.open_git_commit_details(repository_name, root, commit);
            }
            EditorAction::SaveFile => self.save_focused_file(),
            EditorAction::OpenMarkdownLink { path, fragment } => {
                self.open_markdown_link_in_focused_pane(path, fragment);
            }
            EditorAction::OpenExternalUrl(url) => {
                self.open_external_url(&url);
            }
            EditorAction::ToggleBookmark { path, line } => {
                self.toggle_bookmark_at_line(path, line);
            }
            EditorAction::ShowStatus(message) => {
                self.status_message = message;
            }
        }
    }

    fn has_csv_selection(&self) -> bool {
        self.left.has_active_csv_selection() || self.right.has_active_csv_selection()
    }

    fn copy_virtual_selection_to_clipboard(&mut self, ui: &egui::Ui) -> bool {
        let Some(text) = self.focused_editor().virtual_selection_text() else {
            return false;
        };

        ui.ctx().copy_text(text.clone());

        match copy_text_to_system_clipboard(&text) {
            Ok(()) => {
                self.status_message =
                    format!("Copied virtual selection: {} chars.", text.chars().count());
            }
            Err(error) => {
                self.status_message = format!(
                    "Virtual selection copy requested, but system clipboard failed: {error}"
                );
            }
        }
        true
    }

    fn copy_diff_selection_to_clipboard(&mut self, ui: &egui::Ui) -> bool {
        let Some(text) = self.focused_editor().diff_selection_text() else {
            return false;
        };

        ui.ctx().copy_text(text.clone());

        match copy_text_to_system_clipboard(&text) {
            Ok(()) => {
                self.status_message =
                    format!("Copied diff selection: {} chars.", text.chars().count());
            }
            Err(error) => {
                self.status_message =
                    format!("Diff selection copy requested, but system clipboard failed: {error}");
            }
        }
        true
    }

    fn copy_selected_csv_to_clipboard(&mut self, ui: &egui::Ui) {
        let Some(text) = self
            .focused_editor()
            .csv_selection_text()
            .or_else(|| self.left.csv_selection_text())
            .or_else(|| self.right.csv_selection_text())
        else {
            self.status_message = "No CSV row or column selection to copy.".to_owned();
            return;
        };

        ui.ctx().copy_text(text.clone());

        match copy_text_to_system_clipboard(&text) {
            Ok(()) => {
                self.status_message =
                    format!("Copied CSV selection: {} chars.", text.chars().count());
            }
            Err(error) => {
                self.status_message =
                    format!("CSV copy requested, but system clipboard failed: {error}");
            }
        }
    }

    fn paste_csv_text_from_clipboard(&mut self, text: &str) {
        let pane = if self.focused_editor().has_active_csv_selection() {
            Some(self.focused_pane)
        } else if self.left.has_active_csv_selection() {
            Some(Pane::Left)
        } else if self.right.has_active_csv_selection() {
            Some(Pane::Right)
        } else {
            None
        };

        let Some(pane) = pane else {
            self.status_message = "No CSV row or column selection to paste into.".to_owned();
            return;
        };

        let result = match pane {
            Pane::Left => self.left.paste_csv_text(text),
            Pane::Right => self.right.paste_csv_text(text),
        };

        match result {
            Ok(cell_count) => {
                self.focused_pane = pane;
                self.status_message = format!("Pasted CSV into {cell_count} cells.");
            }
            Err(error) => {
                self.status_message = format!("CSV paste failed: {error}");
            }
        }
    }

    fn delete_selected_csv(&mut self) {
        let pane = if self.focused_editor().has_active_csv_selection() {
            Some(self.focused_pane)
        } else if self.left.has_active_csv_selection() {
            Some(Pane::Left)
        } else if self.right.has_active_csv_selection() {
            Some(Pane::Right)
        } else {
            None
        };

        let Some(pane) = pane else {
            self.status_message = "No CSV row or column selection to delete.".to_owned();
            return;
        };

        let result = match pane {
            Pane::Left => self.left.delete_csv_selection(),
            Pane::Right => self.right.delete_csv_selection(),
        };

        match result {
            Ok(label) => {
                self.focused_pane = pane;
                self.status_message = format!("Deleted CSV {label}.");
            }
            Err(error) => {
                self.status_message = format!("CSV delete failed: {error}");
            }
        }
    }

    fn cli_agent_command(&self) -> Result<CliAgentCommand, String> {
        let roots = self
            .current_session()
            .map(|session| session.root_folders.as_slice())
            .unwrap_or(&[]);
        build_cli_agent_command(&self.app_state.cli_agent, roots)
    }

    fn copy_cli_agent_command(&mut self) {
        match self.cli_agent_command() {
            Ok(command) => match copy_text_to_system_clipboard(&command.command) {
                Ok(()) => {
                    self.status_message = format!(
                        "Copied {} command: base {}.",
                        command.agent_name,
                        command.base_dir.display()
                    );
                }
                Err(error) => {
                    self.status_message = format!("AI command copy failed: {error}");
                }
            },
            Err(error) => {
                self.status_message = format!("AI command unavailable: {error}");
            }
        }
    }

    fn copy_diagnostics_report(&mut self) {
        let report = self.diagnostics_report_text();
        match copy_text_to_system_clipboard(&report) {
            Ok(()) => {
                self.status_message = "Copied diagnostics report.".to_owned();
            }
            Err(error) => {
                self.status_message = format!("Diagnostics report copy failed: {error}");
            }
        }
    }

    fn open_markdown_search(&mut self) -> bool {
        let editor = self.focused_editor_mut();
        if !editor.is_markdown() || editor.markdown_mode == MarkdownMode::Markdown {
            return false;
        }

        editor.markdown_search.open = true;
        editor.markdown_search.focus_requested = true;
        editor.markdown_search.current_match = 0;
        true
    }

    fn open_source_search(&mut self) -> bool {
        let editor = self.focused_editor_mut();
        if !editor.can_source_search() && !editor.can_markdown_source_search() {
            return false;
        }

        editor.source_search.open = true;
        editor.source_search.focus_requested = true;
        editor.source_search.current_match = 0;
        editor.source_search.scrolled_match = None;
        true
    }

    fn remember_recent_file(&mut self, path: PathBuf) {
        self.recent_files.retain(|recent| recent != &path);
        self.recent_files.insert(0, path);
        self.recent_files.truncate(30);
        self.bump_search_index_revision();
    }

    fn bump_search_index_revision(&mut self) {
        self.search_index_revision = self.search_index_revision.wrapping_add(1);
        self.search_overlay.cache = None;
    }

    fn open_search_overlay(&mut self) {
        self.close_references_overlay();
        self.search_overlay.open = true;
        self.search_overlay.selected_index = 0;
    }

    fn close_search_overlay(&mut self) {
        self.search_overlay.open = false;
        self.search_overlay.query.clear();
        self.search_overlay.selected_index = 0;
        self.search_overlay.cache = None;
    }

    fn open_references_overlay(&mut self, symbol: &str) {
        self.references_overlay.open = true;
        self.references_overlay.query.clear();
        self.references_overlay.selected_index = 0;
        self.references_overlay.symbol = symbol.to_owned();
    }

    fn close_references_overlay(&mut self) {
        self.references_overlay.open = false;
        self.references_overlay.query.clear();
        self.references_overlay.selected_index = 0;
        self.references_overlay.symbol.clear();
    }

    fn search_overlay_results(&mut self, bookmarks: &[SessionBookmark]) -> Vec<SearchResult> {
        if let Some(cache) = &self.search_overlay.cache
            && cache.query == self.search_overlay.query
            && cache.revision == self.search_index_revision
        {
            return cache.results.clone();
        }

        let results = search_everywhere(
            &self.search_overlay.query,
            &self.file_tree.files,
            self.symbol_index.symbols(),
            bookmarks,
            &self.recent_files,
        );

        self.search_overlay.cache = Some(SearchOverlayCache {
            query: self.search_overlay.query.clone(),
            revision: self.search_index_revision,
            results: results.clone(),
        });
        results
    }

    fn open_workspace_search(&mut self) {
        self.show_workspace_sidebar = true;
        self.sidebar_view = SidebarView::Search;
        self.workspace_search.open = true;
        self.workspace_search.focus_requested = true;
        self.last_shift_tap_time = None;
    }

    fn close_workspace_search(&mut self) {
        self.workspace_search.open = false;
        self.workspace_search.query.clear();
        self.workspace_search.results.clear();
        self.workspace_search.searched_query.clear();
        self.workspace_search.focus_requested = false;
        self.workspace_search.searching = false;
        self.workspace_search_receiver = None;
    }

    fn run_workspace_search(&mut self) {
        let query = self.workspace_search.query.trim().to_owned();
        if query.is_empty() {
            self.workspace_search.results.clear();
            self.workspace_search.searched_query.clear();
            self.workspace_search.searching = false;
            self.workspace_search_receiver = None;
            self.status_message = "Workspace search query is empty.".to_owned();
            return;
        }

        self.next_workspace_search_id = self.next_workspace_search_id.wrapping_add(1);
        let search_id = self.next_workspace_search_id;
        let files = self.file_tree.files.clone();
        let query_for_thread = query.clone();
        let (sender, receiver) = mpsc::channel();

        thread::spawn(move || {
            let results =
                workspace_search_results(&files, &query_for_thread, WORKSPACE_SEARCH_MAX_MATCHES);
            let _ = sender.send(WorkspaceSearchResult {
                search_id,
                query: query_for_thread,
                results,
            });
        });

        self.workspace_search.active_search_id = search_id;
        self.workspace_search.searching = true;
        self.workspace_search.searched_query = query.clone();
        self.workspace_search_receiver = Some(receiver);
        self.status_message = format!("Workspace search started: {query}");
    }

    fn workspace_search_ui(&mut self, ui: &mut egui::Ui) {
        let mut run_search = false;
        let mut close_search = false;
        let mut open_location = None;

        let input_id = ui.id().with("workspace_search_query");
        let input_had_focus = ui.memory(|memory| memory.has_focus(input_id));
        if input_had_focus && ui.input(raw_enter_pressed) {
            run_search = true;
        }
        // egui drops focus at the start of the frame that carries a plain Escape, so check
        // who had focus in the previous frame.
        if ui.memory(|memory| memory.had_focus_last_frame(input_id)) && ui.input(raw_escape_pressed)
        {
            close_search = true;
        }

        let response = ui.add(
            TextEdit::singleline(&mut self.workspace_search.query)
                .id(input_id)
                .hint_text("Search workspace")
                .desired_width(f32::INFINITY),
        );
        if self.workspace_search.focus_requested {
            ui.memory_mut(|memory| memory.request_focus(input_id));
            self.workspace_search.focus_requested = false;
        }

        let input_has_focus =
            response.has_focus() || ui.memory(|memory| memory.has_focus(input_id));
        if !input_had_focus && input_has_focus && ui.input(raw_enter_pressed) {
            run_search = true;
        }
        if !input_had_focus && input_has_focus && ui.input(raw_escape_pressed) {
            close_search = true;
        }

        ui.horizontal(|ui| {
            if ui.button("Search").clicked() {
                run_search = true;
            }
            if ui.button("Clear").clicked() {
                close_search = true;
            }
        });

        let match_count = self.workspace_search.match_count();
        let result_label = if self.workspace_search.searching {
            format!(
                "Searching {} files for `{}`",
                self.file_tree.files.len(),
                self.workspace_search.searched_query
            )
        } else {
            format!(
                "{match_count} results in {} files",
                self.workspace_search.results.len()
            )
        };
        ui.label(RichText::new(result_label).weak());

        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        let results = self.workspace_search.results.clone();
        ScrollArea::vertical()
            .id_salt("workspace_search_results")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.workspace_search.searching {
                    ui.label(RichText::new("Searching...").weak());
                    return;
                }
                if !self.workspace_search.searched_query.is_empty() && results.is_empty() {
                    ui.label(RichText::new("No results").weak());
                    return;
                }

                for file_result in results {
                    let label = format!(
                        "{} ({})",
                        workspace_search_display_path(&file_result.path, &roots),
                        file_result.matches.len()
                    );
                    ui.push_id(&file_result.path, |ui| {
                        egui::CollapsingHeader::new(RichText::new(label).monospace())
                            .default_open(true)
                            .show(ui, |ui| {
                                for (match_index, search_match) in
                                    file_result.matches.iter().enumerate()
                                {
                                    let line_label = format!(
                                        "{:>5}: {}",
                                        search_match.position.line + 1,
                                        search_match.preview
                                    );
                                    let clicked = ui
                                        .push_id(match_index, |ui| {
                                            ui.button(RichText::new(line_label).monospace())
                                                .clicked()
                                        })
                                        .inner;
                                    if clicked {
                                        let range = Range::new(
                                            search_match.position,
                                            search_match.position,
                                        );
                                        open_location =
                                            Some(Location::new(file_result.path.clone(), range));
                                    }
                                }
                            });
                    });
                }
            });

        if run_search {
            self.run_workspace_search();
        }
        if close_search {
            self.close_workspace_search();
        }
        if let Some(location) = open_location {
            let path = location.path.clone();
            let line = location.range.start.line + 1;
            self.open_location_in_focused_pane(location, true);
            self.status_message = format!("Opened search result: {}:{line}", path.display());
        }
    }

    fn refresh_git_view(&mut self) {
        self.reload_git_repositories();
        self.status_message = format!("Git: {} repositories.", self.git_repositories.len());
    }

    fn ensure_git_repositories_loaded(&mut self) {
        if !self.git_repositories_loaded {
            self.reload_git_repositories();
        }
    }

    fn reload_git_repositories(&mut self) {
        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        self.git_repositories = discover_git_repositories(&roots);
        self.git_repositories_loaded = true;
        self.git_commit_messages
            .retain(|root, _| self.git_repositories.iter().any(|repo| &repo.root == root));
        self.git_repo_open
            .retain(|root, _| self.git_repositories.iter().any(|repo| &repo.root == root));
        self.git_new_branch_names
            .retain(|root, _| self.git_repositories.iter().any(|repo| &repo.root == root));
        if self
            .git_create_branch_root
            .as_ref()
            .is_some_and(|root| !self.git_repositories.iter().any(|repo| &repo.root == root))
        {
            self.git_create_branch_root = None;
        }
        if self
            .git_switch_branch_root
            .as_ref()
            .is_some_and(|root| !self.git_repositories.iter().any(|repo| &repo.root == root))
        {
            self.git_switch_branch_root = None;
            self.git_switch_branch_cursor = None;
        }
        for repository in &self.git_repositories {
            self.git_commit_messages
                .entry(repository.root.clone())
                .or_default();
            self.git_repo_open
                .entry(repository.root.clone())
                .or_insert(true);
            self.git_new_branch_names
                .entry(repository.root.clone())
                .or_default();
        }
    }

    fn git_ui(&mut self, ui: &mut egui::Ui) {
        if !self.git_repositories_loaded {
            self.refresh_git_view();
        }

        if self.git_repositories.is_empty() {
            ui.label(RichText::new("No Git repositories found in this workspace.").weak());
            return;
        }

        egui::CollapsingHeader::new("Repositories")
            .default_open(true)
            .show(ui, |ui| {
                for repository in self.git_repositories.clone() {
                    let tracking = repository.tracking.as_ref().and_then(|tracking| {
                        if tracking.ahead == 0 && tracking.behind == 0 {
                            None
                        } else {
                            let upstream = repository.upstream.as_deref().unwrap_or("upstream");
                            Some((
                                format!("{}↓ {}↑", tracking.behind, tracking.ahead),
                                format!(
                                    "{} commits to pull from {upstream}\n{} commits to push to {upstream}",
                                    tracking.behind, tracking.ahead
                                ),
                            ))
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&repository.name));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            self.git_repository_menu(ui, &repository);
                            if ui.button("Pull").clicked() {
                                self.pull_git_repository(&repository);
                            }
                            if let Some((label, tooltip)) = tracking {
                                ui.label(RichText::new(label).weak()).on_hover_text(tooltip);
                            }

                            let branch_label = format!("{} ▾", repository.branch);
                            if ui
                                .add_sized(
                                    [72.0, ui.spacing().interact_size.y],
                                    egui::Label::new(RichText::new(branch_label).monospace())
                                        .sense(egui::Sense::click())
                                        .truncate(),
                                )
                                .on_hover_text(format!("{}\nSwitch Branch", repository.branch))
                                .clicked()
                            {
                                self.git_switch_branch_root = Some(repository.root.clone());
                            }
                        });
                    });
                    if let Some(error) = &repository.error {
                        ui.label(
                            RichText::new(error)
                                .small()
                                .color(ui.visuals().warn_fg_color),
                        );
                    }
                }
            });

        egui::CollapsingHeader::new("Changes")
            .default_open(true)
            .show(ui, |ui| {
                for repository in self.git_repositories.clone() {
                    let change_count = repository.changes.len();
                    let default_open = self
                        .git_repo_open
                        .get(&repository.root)
                        .copied()
                        .unwrap_or(true);
                    let state_id = ui.make_persistent_id(("git_repo_changes", &repository.root));
                    let state = egui::collapsing_header::CollapsingState::load_with_default_open(
                        ui.ctx(),
                        state_id,
                        default_open,
                    );
                    let mut open_all_diffs = false;
                    let mut open_history = false;
                    let header = state.show_header(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(format!("{} ({change_count})", repository.name));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .button("History")
                                        .on_hover_text("Open Git history")
                                        .clicked()
                                    {
                                        open_history = true;
                                    }
                                    if ui
                                        .add_enabled(
                                            !repository.changes.is_empty(),
                                            egui::Button::new("All Diffs"),
                                        )
                                        .on_hover_text("Open all repository diffs")
                                        .clicked()
                                    {
                                        open_all_diffs = true;
                                    }
                                },
                            );
                        });
                    });
                    self.git_repo_open
                        .insert(repository.root.clone(), header.is_open());
                    if open_all_diffs {
                        self.open_git_all_diff(&repository);
                    }
                    if open_history {
                        self.open_git_history(&repository);
                    }
                    header.body(|ui| {
                        let mut commit = false;
                        let mut commit_and_push = false;
                        let message = self
                            .git_commit_messages
                            .entry(repository.root.clone())
                            .or_default();
                        ui.add(
                            TextEdit::multiline(message)
                                .id(ui.make_persistent_id(("git_commit_message", &repository.root)))
                                .hint_text("Commit message")
                                .desired_width(f32::INFINITY)
                                .desired_rows(4),
                        );
                        ui.horizontal(|ui| {
                            if ui.button("Commit").clicked() {
                                commit = true;
                            }
                            if ui.button("Commit & Push").clicked() {
                                commit_and_push = true;
                            }
                        });

                        if commit {
                            self.commit_git_repository(&repository, false);
                        }
                        if commit_and_push {
                            self.commit_git_repository(&repository, true);
                        }

                        self.git_changes_files_ui(ui, &repository);
                    });
                    ui.add_space(4.0);
                }
            });
    }

    fn git_repository_menu(&mut self, ui: &mut egui::Ui, repository: &GitRepositoryState) {
        ui.menu_button("...", |ui| {
            if ui.button("Fetch").clicked() {
                self.fetch_git_repository(repository);
                ui.close();
            }

            let push_label = if repository.upstream.is_some() {
                "Push"
            } else {
                "Publish Branch"
            };
            let can_push = repository.upstream.is_some()
                || repository
                    .branches
                    .iter()
                    .any(|branch| branch == &repository.branch);
            if ui
                .add_enabled(can_push, egui::Button::new(push_label))
                .on_hover_text(if repository.upstream.is_some() {
                    "Push commits to the upstream branch"
                } else {
                    "Push this branch and set upstream"
                })
                .clicked()
            {
                self.push_git_repository(repository);
                ui.close();
            }

            if ui.button("Switch Branch...").clicked() {
                self.git_switch_branch_root = Some(repository.root.clone());
                ui.close();
            }

            if ui.button("Create Branch...").clicked() {
                self.git_create_branch_root = Some(repository.root.clone());
                self.git_new_branch_names
                    .entry(repository.root.clone())
                    .or_default()
                    .clear();
                ui.close();
            }
        })
        .response
        .on_hover_text("More Git actions");
    }

    fn git_changes_files_ui(&mut self, ui: &mut egui::Ui, repository: &GitRepositoryState) {
        let staged_changes = repository
            .changes
            .iter()
            .filter(|change| change.is_staged())
            .collect::<Vec<_>>();
        let changed_files = repository
            .changes
            .iter()
            .filter(|change| change.has_worktree_change())
            .collect::<Vec<_>>();

        if repository.changes.is_empty() {
            ui.label(RichText::new("No changes").small().weak());
            return;
        }

        if !staged_changes.is_empty() {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Staged Changes").small().strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("- Unstage All").clicked() {
                        self.unstage_all_git_changes(repository);
                    }
                });
            });
            for change in staged_changes {
                self.git_change_row(ui, repository, change, false);
            }
            ui.add_space(6.0);
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new("Changed Files").small().strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(!changed_files.is_empty(), egui::Button::new("+ Stage All"))
                    .clicked()
                {
                    self.stage_all_git_changes(repository);
                }
            });
        });
        if changed_files.is_empty() {
            ui.label(RichText::new("No unstaged changes").small().weak());
        }
        for change in changed_files {
            self.git_change_row(ui, repository, change, true);
        }
    }

    fn git_change_row(
        &mut self,
        ui: &mut egui::Ui,
        repository: &GitRepositoryState,
        change: &GitChange,
        show_worktree_actions: bool,
    ) {
        let row_width = ui.available_width();
        let row_height = 24.0;
        let status_width = 28.0;
        let action_button_width = 24.0;
        let action_gap = 4.0;
        let action_count = if show_worktree_actions { 2.0 } else { 1.0 };
        let action_width = action_count * action_button_width + (action_count - 1.0) * action_gap;
        let column_gap = 8.0;
        let (row_rect, _) =
            ui.allocate_exact_size(egui::vec2(row_width, row_height), egui::Sense::hover());
        let status_rect =
            egui::Rect::from_min_size(row_rect.min, egui::vec2(status_width, row_height));
        let action_rect = egui::Rect::from_min_size(
            egui::pos2(row_rect.right() - action_width, row_rect.top() + 1.0),
            egui::vec2(action_width, row_height - 2.0),
        );
        let path_rect = egui::Rect::from_min_max(
            egui::pos2(status_rect.right() + column_gap, row_rect.top()),
            egui::pos2(action_rect.left() - column_gap, row_rect.bottom()),
        );
        let monospace_font = TextStyle::Monospace.resolve(ui.style());
        let draw_left_text = |ui: &egui::Ui, rect: egui::Rect, text: &str| {
            ui.painter().with_clip_rect(rect).text(
                rect.left_center(),
                egui::Align2::LEFT_CENTER,
                text,
                monospace_font.clone(),
                ui.visuals().text_color(),
            );
        };

        draw_left_text(ui, status_rect, change.display_status());
        let path_response = ui.interact(
            path_rect,
            ui.make_persistent_id(("git_change_path", &repository.root, &change.path)),
            egui::Sense::click(),
        );
        if path_response.hovered() {
            ui.painter().rect_filled(
                path_rect.shrink2(egui::vec2(0.0, 2.0)),
                2.0,
                ui.visuals().widgets.hovered.bg_fill,
            );
        }
        draw_left_text(ui, path_rect, &change.path);
        if path_response
            .on_hover_text(format!("{}\nOpen diff", change.path))
            .clicked()
        {
            let scope = if show_worktree_actions {
                GitDiffScope::Worktree
            } else {
                GitDiffScope::Staged
            };
            self.open_git_file_diff(repository, change, scope);
        }

        if show_worktree_actions {
            let discard_rect = egui::Rect::from_min_size(
                action_rect.min,
                egui::vec2(action_button_width, action_rect.height()),
            );
            let stage_rect = egui::Rect::from_min_size(
                egui::pos2(discard_rect.right() + action_gap, action_rect.top()),
                egui::vec2(action_button_width, action_rect.height()),
            );
            if ui
                .put(discard_rect, egui::Button::new("↶"))
                .on_hover_text("Discard changes")
                .clicked()
            {
                self.pending_git_discard = Some(GitDiscardRequest {
                    repository_root: repository.root.clone(),
                    repository_name: repository.name.clone(),
                    path: change.path.clone(),
                    untracked: change.is_untracked(),
                });
            }
            if ui
                .put(stage_rect, egui::Button::new("+"))
                .on_hover_text("Stage file")
                .clicked()
            {
                self.stage_git_file(repository, change);
            }
        } else {
            if ui
                .put(action_rect, egui::Button::new("-"))
                .on_hover_text("Unstage file")
                .clicked()
            {
                self.unstage_git_file(repository, change);
            }
        }
    }

    fn open_git_all_diff(&mut self, repository: &GitRepositoryState) {
        match git_all_diff_text(repository) {
            Ok(text) => {
                let title = format!("{}: All Diffs", repository.name);
                let diff_view = GitDiffView {
                    title,
                    repository_name: repository.name.clone(),
                    repository_root: Some(repository.root.clone()),
                    file_path: None,
                    scope: GitDiffScope::All,
                    context_mode: GitDiffContextMode::Changes,
                    untracked: false,
                    return_history: None,
                };
                let pane = self.focused_pane;
                *self.focused_editor_mut() = EditorPane::from_diff(pane, diff_view, text);
                self.status_message = format!("Opened all diffs: {}", repository.name);
            }
            Err(error) => {
                self.status_message =
                    format!("Open all diffs failed: {} | {error}", repository.name);
            }
        }
    }

    fn open_git_file_diff(
        &mut self,
        repository: &GitRepositoryState,
        change: &GitChange,
        scope: GitDiffScope,
    ) {
        let context_mode = GitDiffContextMode::Changes;
        match git_file_diff_text(repository, change, scope, context_mode) {
            Ok(text) => {
                let suffix = match scope {
                    GitDiffScope::All => "Diff",
                    GitDiffScope::Worktree => "Diff",
                    GitDiffScope::Staged => "Staged Diff",
                    GitDiffScope::Commit => "Commit Diff",
                };
                let title = format!("{}: {suffix}", change.path);
                let diff_view = GitDiffView {
                    title,
                    repository_name: repository.name.clone(),
                    repository_root: Some(repository.root.clone()),
                    file_path: Some(change.path.clone()),
                    scope,
                    context_mode,
                    untracked: change.is_untracked(),
                    return_history: None,
                };
                let pane = self.focused_pane;
                *self.focused_editor_mut() = EditorPane::from_diff(pane, diff_view, text);
                self.status_message = format!("Opened diff: {} | {}", repository.name, change.path);
            }
            Err(error) => {
                self.status_message = format!(
                    "Open diff failed: {} | {} | {error}",
                    repository.name, change.path
                );
            }
        }
    }

    fn open_git_history(&mut self, repository: &GitRepositoryState) {
        match git_history_nodes(repository, true) {
            Ok(rows) => {
                let history_view = GitHistoryView {
                    repository_name: repository.name.clone(),
                    root: repository.root.clone(),
                    rows,
                    selected_commit_hash: None,
                };
                let pane = self.focused_pane;
                *self.focused_editor_mut() = EditorPane::from_git_history(pane, history_view);
                self.status_message = format!("Opened Git graph: {}", repository.name);
            }
            Err(error) => {
                self.status_message =
                    format!("Open Git graph failed: {} | {error}", repository.name);
            }
        }
    }

    fn open_git_commit_details(
        &mut self,
        repository_name: String,
        root: PathBuf,
        commit: GitCommitNode,
    ) {
        let (files, error) = match git_commit_changed_files(&root, &commit.hash) {
            Ok(files) => (files, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        self.git_commit_details = Some(GitCommitDetailsWindow {
            repository_name,
            root,
            commit: commit.clone(),
            files,
            error,
        });
        self.status_message = format!(
            "Opened commit details: {} {}",
            commit.short_hash, commit.subject
        );
    }

    fn open_git_commit_diff(&mut self, details: &GitCommitDetailsWindow) {
        let return_history = self.focused_editor().git_history_view.clone().map(|view| {
            let editor = self.focused_editor();
            GitHistoryReturnState {
                view,
                scroll_y: editor.scroll_y,
                scroll_x: editor.scroll_x,
            }
        });
        match git_commit_diff_text(&details.root, &details.commit.hash) {
            Ok(text) => {
                let short_hash = details.commit.short_hash.clone();
                let title = format!("{}: {short_hash} Diff", details.repository_name);
                let diff_view = GitDiffView {
                    title,
                    repository_name: details.repository_name.clone(),
                    repository_root: Some(details.root.clone()),
                    file_path: None,
                    scope: GitDiffScope::Commit,
                    context_mode: GitDiffContextMode::Changes,
                    untracked: false,
                    return_history,
                };
                let pane = self.focused_pane;
                *self.focused_editor_mut() = EditorPane::from_diff(pane, diff_view, text);
                self.status_message = format!("Opened commit diff: {short_hash}");
            }
            Err(error) => {
                self.status_message = format!(
                    "Open commit diff failed: {} | {error}",
                    details.commit.short_hash
                );
            }
        }
    }

    fn fetch_git_repository(&mut self, repository: &GitRepositoryState) {
        match git_command_output(&repository.root, &["fetch"]) {
            Ok(output) => {
                let detail = if output.is_empty() {
                    "Fetch completed.".to_owned()
                } else {
                    output
                };
                self.status_message =
                    format!("Git fetch completed: {} | {detail}", repository.name);
            }
            Err(error) => {
                self.status_message = format!("Git fetch failed: {} | {error}", repository.name);
            }
        }
        self.reload_git_repositories();
    }

    fn pull_git_repository(&mut self, repository: &GitRepositoryState) {
        match git_command_output(&repository.root, &["pull", "--ff-only"]) {
            Ok(output) => {
                let detail = if output.is_empty() {
                    "Already up to date.".to_owned()
                } else {
                    output
                };
                self.status_message = format!("Git pull completed: {} | {detail}", repository.name);
            }
            Err(error) => {
                self.status_message = format!("Git pull failed: {} | {error}", repository.name);
            }
        }
        self.reload_git_repositories();
    }

    fn push_git_repository(&mut self, repository: &GitRepositoryState) {
        match push_or_publish_git_branch(repository) {
            Ok(output) => {
                let detail = if output.is_empty() {
                    if repository.upstream.is_some() {
                        "Push completed.".to_owned()
                    } else {
                        "Branch published.".to_owned()
                    }
                } else {
                    output
                };
                if repository.upstream.is_some() {
                    self.status_message =
                        format!("Git push completed: {} | {detail}", repository.name);
                } else {
                    self.status_message =
                        format!("Git publish completed: {} | {detail}", repository.name);
                }
            }
            Err(error) => {
                if repository.upstream.is_some() {
                    self.status_message = format!("Git push failed: {} | {error}", repository.name);
                } else {
                    self.status_message =
                        format!("Git publish failed: {} | {error}", repository.name);
                }
            }
        }
        self.reload_git_repositories();
    }

    fn commit_git_repository(&mut self, repository: &GitRepositoryState, push_after_commit: bool) {
        let message = self
            .git_commit_messages
            .get(&repository.root)
            .map(|message| message.trim().to_owned())
            .unwrap_or_default();
        if message.is_empty() {
            self.status_message =
                format!("Git commit failed: {} | message is empty", repository.name);
            return;
        }

        match git_command_output(&repository.root, &["commit", "-m", &message]) {
            Ok(output) => {
                self.git_commit_messages
                    .entry(repository.root.clone())
                    .or_default()
                    .clear();
                let commit_detail = if output.is_empty() {
                    "Commit completed.".to_owned()
                } else {
                    output
                };
                if push_after_commit {
                    match push_or_publish_git_branch(repository) {
                        Ok(push_output) => {
                            let push_detail = if push_output.is_empty() {
                                "Push completed.".to_owned()
                            } else {
                                push_output
                            };
                            self.status_message = format!(
                                "Git commit & push completed: {} | {commit_detail} | {push_detail}",
                                repository.name
                            );
                        }
                        Err(error) => {
                            self.status_message = format!(
                                "Git commit completed but push failed: {} | {commit_detail} | {error}",
                                repository.name
                            );
                        }
                    }
                } else {
                    self.status_message = format!(
                        "Git commit completed: {} | {commit_detail}",
                        repository.name
                    );
                }
            }
            Err(error) => {
                self.status_message = format!("Git commit failed: {} | {error}", repository.name);
            }
        }
        self.reload_git_repositories();
    }

    fn stage_all_git_changes(&mut self, repository: &GitRepositoryState) {
        match git_command_output(&repository.root, &["add", "-A"]) {
            Ok(output) => {
                let detail = if output.is_empty() {
                    "Staged all changes.".to_owned()
                } else {
                    output
                };
                self.status_message =
                    format!("Git stage all completed: {} | {detail}", repository.name);
            }
            Err(error) => {
                self.status_message =
                    format!("Git stage all failed: {} | {error}", repository.name);
            }
        }
        self.reload_git_repositories();
    }

    fn stage_git_file(&mut self, repository: &GitRepositoryState, change: &GitChange) {
        match git_command_output(&repository.root, &["add", "--", &change.path]) {
            Ok(output) => {
                let detail = if output.is_empty() {
                    format!("Staged {}.", change.path)
                } else {
                    output
                };
                self.status_message =
                    format!("Git stage completed: {} | {detail}", repository.name);
            }
            Err(error) => {
                self.status_message = format!(
                    "Git stage failed: {} | {} | {error}",
                    repository.name, change.path
                );
            }
        }
        self.reload_git_repositories();
    }

    fn unstage_all_git_changes(&mut self, repository: &GitRepositoryState) {
        match git_command_output(&repository.root, &["restore", "--staged", "--", "."]) {
            Ok(output) => {
                let detail = if output.is_empty() {
                    "Unstaged all changes.".to_owned()
                } else {
                    output
                };
                self.status_message =
                    format!("Git unstage all completed: {} | {detail}", repository.name);
            }
            Err(error) => {
                self.status_message =
                    format!("Git unstage all failed: {} | {error}", repository.name);
            }
        }
        self.reload_git_repositories();
    }

    fn unstage_git_file(&mut self, repository: &GitRepositoryState, change: &GitChange) {
        match git_command_output(
            &repository.root,
            &["restore", "--staged", "--", &change.path],
        ) {
            Ok(output) => {
                let detail = if output.is_empty() {
                    format!("Unstaged {}.", change.path)
                } else {
                    output
                };
                self.status_message =
                    format!("Git unstage completed: {} | {detail}", repository.name);
            }
            Err(error) => {
                self.status_message = format!(
                    "Git unstage failed: {} | {} | {error}",
                    repository.name, change.path
                );
            }
        }
        self.reload_git_repositories();
    }

    fn discard_git_file(&mut self, request: &GitDiscardRequest) {
        let args = if request.untracked {
            ["clean", "-f", "--", request.path.as_str()]
        } else {
            ["restore", "--worktree", "--", request.path.as_str()]
        };
        match git_command_output(&request.repository_root, &args) {
            Ok(output) => {
                let detail = if output.is_empty() {
                    format!("Discarded {}.", request.path)
                } else {
                    output
                };
                self.status_message = format!(
                    "Git discard completed: {} | {detail}",
                    request.repository_name
                );
            }
            Err(error) => {
                self.status_message = format!(
                    "Git discard failed: {} | {} | {error}",
                    request.repository_name, request.path
                );
            }
        }
        self.reload_git_repositories();
    }

    fn switch_git_branch(&mut self, repository: &GitRepositoryState, branch_name: &str) {
        match git_command_output(&repository.root, &["switch", branch_name]) {
            Ok(output) => {
                let detail = if output.is_empty() {
                    format!("Switched to {branch_name}.")
                } else {
                    output
                };
                self.status_message =
                    format!("Git branch switched: {} | {detail}", repository.name);
            }
            Err(error) => {
                self.status_message =
                    format!("Git branch switch failed: {} | {error}", repository.name);
            }
        }
        self.reload_git_repositories();
    }

    fn create_git_branch(&mut self, repository: &GitRepositoryState, branch_name: &str) {
        match git_command_output(&repository.root, &["switch", "-c", branch_name]) {
            Ok(output) => {
                let detail = if output.is_empty() {
                    format!("Created and switched to {branch_name}.")
                } else {
                    output
                };
                self.status_message = format!("Git branch created: {} | {detail}", repository.name);
            }
            Err(error) => {
                self.status_message =
                    format!("Git branch create failed: {} | {error}", repository.name);
            }
        }
        self.reload_git_repositories();
    }

    fn consume_search_shortcuts(&mut self, ui: &mut egui::Ui) -> bool {
        let shortcut_pressed = ui.input_mut(|input| {
            input.consume_key(egui::Modifiers::CTRL, egui::Key::P)
                || input.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::O)
        });

        if shortcut_pressed {
            self.last_shift_tap_time = None;
            return true;
        }

        // "Shift x2" means two lone Shift taps: a Shift that is part of Shift+Enter,
        // Ctrl+Shift+Z or Shift+Arrow does not count, and neither does a pair with other
        // input in between. Taps count on release, so the second one never shares a frame
        // with a key the opened overlay would take.
        let (shift_down, other_input, time) = ui.input(|input| {
            let other_modifier = input.modifiers.ctrl
                || input.modifiers.alt
                || input.modifiers.mac_cmd
                || input.modifiers.command;
            let other_event = input.raw.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::Paste(_)
                        | egui::Event::Copy
                        | egui::Event::Cut
                        | egui::Event::Ime(_)
                )
            });
            (
                input.modifiers.shift,
                other_modifier || other_event,
                input.time,
            )
        });
        let shift_pressed = shift_down && !self.last_shift_down;
        let shift_released = !shift_down && self.last_shift_down;
        self.last_shift_down = shift_down;

        if other_input {
            self.shift_tap_candidate = false;
            self.last_shift_tap_time = None;
            return false;
        }
        if shift_pressed {
            self.shift_tap_candidate = true;
        }
        if !shift_released || !std::mem::take(&mut self.shift_tap_candidate) {
            return false;
        }
        if self.search_overlay.open {
            return false;
        }

        let should_open = self
            .last_shift_tap_time
            .map(|last_time| time - last_time <= 0.45)
            .unwrap_or(false);
        self.last_shift_tap_time = Some(time);

        if should_open {
            self.last_shift_tap_time = None;
        }

        should_open
    }

    fn show_search_overlay(&mut self, ui: &mut egui::Ui) -> Option<SearchResult> {
        if !self.search_overlay.open {
            return None;
        }

        let bookmarks = self
            .current_session()
            .map(|session| session.bookmarks.clone())
            .unwrap_or_default();
        let results = self.search_overlay_results(&bookmarks);

        if self.search_overlay.selected_index >= results.len() {
            self.search_overlay.selected_index = results.len().saturating_sub(1);
        }

        let mut selected_result = None;
        let mut close = false;

        egui::Window::new("Search Everywhere")
            .id(egui::Id::new("search_everywhere_overlay"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
            .fixed_size(egui::vec2(680.0, 460.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                let (escape, enter, arrow_up, arrow_down) = ui.input_mut(|input| {
                    (
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                    )
                });

                let query_response = ui.add(
                    TextEdit::singleline(&mut self.search_overlay.query)
                        .hint_text("Search files, symbols, bookmarks")
                        .desired_width(f32::INFINITY),
                );
                query_response.request_focus();
                if query_response.changed() {
                    self.search_overlay.selected_index = 0;
                }

                if escape {
                    close = true;
                }
                if arrow_down && !results.is_empty() {
                    self.search_overlay.selected_index =
                        (self.search_overlay.selected_index + 1).min(results.len() - 1);
                }
                if arrow_up && !results.is_empty() {
                    self.search_overlay.selected_index =
                        self.search_overlay.selected_index.saturating_sub(1);
                }
                if enter {
                    selected_result = results
                        .get(self.search_overlay.selected_index)
                        .cloned()
                        .or_else(|| results.first().cloned());
                }

                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} results", results.len())).weak());
                    ui.separator();
                    ui.label(RichText::new("Enter: open | Esc: close").weak());
                });
                ui.separator();

                let results_width = ui.available_width();
                let kind_width = 88.0;
                let name_width = 260.0;
                let row_height = 24.0;
                let column_gap = 16.0;
                let body_font = TextStyle::Body.resolve(ui.style());
                let name_font = TextStyle::Monospace.resolve(ui.style());
                let weak_text_color = ui.visuals().weak_text_color();
                let draw_left_text = |ui: &egui::Ui,
                                      rect: egui::Rect,
                                      text: &str,
                                      font: egui::FontId,
                                      color: egui::Color32| {
                    ui.painter().with_clip_rect(rect).text(
                        rect.left_center(),
                        egui::Align2::LEFT_CENTER,
                        text,
                        font,
                        color,
                    );
                };
                ScrollArea::vertical()
                    .id_salt("search_everywhere_results")
                    .auto_shrink([false, false])
                    .max_height(360.0)
                    .show(ui, |ui| {
                        ui.set_min_width(results_width);

                        if results.is_empty() {
                            ui.label(RichText::new("No results").weak());
                            return;
                        }

                        for (index, result) in results.iter().enumerate() {
                            let selected = index == self.search_overlay.selected_index;
                            let (row_rect, _) = ui.allocate_exact_size(
                                egui::vec2(results_width, row_height),
                                egui::Sense::hover(),
                            );
                            let kind_rect = egui::Rect::from_min_size(
                                row_rect.min,
                                egui::vec2(kind_width, row_height),
                            );
                            let name_rect = egui::Rect::from_min_size(
                                egui::pos2(kind_rect.right() + column_gap, row_rect.top()),
                                egui::vec2(name_width, row_height),
                            );
                            let detail_rect = egui::Rect::from_min_max(
                                egui::pos2(name_rect.right() + column_gap, row_rect.top()),
                                row_rect.right_bottom(),
                            );
                            let response = ui.interact(
                                row_rect,
                                ui.make_persistent_id(("search_everywhere_result", index)),
                                egui::Sense::click(),
                            );

                            if response.hovered() {
                                self.search_overlay.selected_index = index;
                            }
                            if response.clicked() {
                                selected_result = Some(result.clone());
                            }

                            if response.hovered() {
                                ui.painter().rect_filled(
                                    row_rect.shrink2(egui::vec2(0.0, 2.0)),
                                    2.0,
                                    ui.visuals().widgets.hovered.bg_fill,
                                );
                            }
                            if selected {
                                ui.painter().rect_filled(
                                    row_rect.shrink2(egui::vec2(0.0, 2.0)),
                                    2.0,
                                    ui.visuals().selection.bg_fill,
                                );
                            }

                            let text_color = if selected {
                                ui.visuals().selection.stroke.color
                            } else {
                                ui.visuals().text_color()
                            };
                            let detail_color = if selected {
                                ui.visuals().selection.stroke.color
                            } else {
                                weak_text_color
                            };
                            draw_left_text(
                                ui,
                                kind_rect.shrink2(egui::vec2(4.0, 0.0)),
                                result.kind.label(),
                                body_font.clone(),
                                text_color,
                            );
                            draw_left_text(
                                ui,
                                name_rect,
                                &result.label,
                                name_font.clone(),
                                text_color,
                            );
                            draw_left_text(
                                ui,
                                detail_rect,
                                &result.detail,
                                body_font.clone(),
                                detail_color,
                            );
                        }
                    });
            });

        if close {
            self.close_search_overlay();
            return None;
        }

        if selected_result.is_some() {
            self.close_search_overlay();
        }

        selected_result
    }

    fn show_references_overlay(&mut self, ui: &mut egui::Ui) -> Option<Location> {
        if !self.references_overlay.open {
            return None;
        }
        if self.location_list_kind != LocationListKind::References
            || self.definition_candidates.is_empty()
        {
            self.close_references_overlay();
            return None;
        }

        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        let entries = reference_overlay_entries(
            &self.definition_candidates,
            &roots,
            &self.references_overlay.query,
        );
        if self.references_overlay.selected_index >= entries.len() {
            self.references_overlay.selected_index = entries.len().saturating_sub(1);
        }
        let mut files = entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        files.sort();
        files.dedup();
        let file_count = files.len();

        let mut selected_location = None;
        let mut close = false;

        egui::Window::new("References")
            .id(egui::Id::new("references_overlay"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
            .fixed_size(egui::vec2(720.0, 520.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                let (escape, enter, arrow_up, arrow_down) = ui.input_mut(|input| {
                    (
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                    )
                });

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Symbol").weak());
                    ui.label(RichText::new(&self.references_overlay.symbol).monospace());
                });
                let query_response = ui.add(
                    TextEdit::singleline(&mut self.references_overlay.query)
                        .hint_text("Filter references")
                        .desired_width(f32::INFINITY),
                );
                query_response.request_focus();
                if query_response.changed() {
                    self.references_overlay.selected_index = 0;
                }

                if escape {
                    close = true;
                }
                if arrow_down && !entries.is_empty() {
                    self.references_overlay.selected_index =
                        (self.references_overlay.selected_index + 1).min(entries.len() - 1);
                }
                if arrow_up && !entries.is_empty() {
                    self.references_overlay.selected_index =
                        self.references_overlay.selected_index.saturating_sub(1);
                }
                if enter {
                    selected_location = entries
                        .get(self.references_overlay.selected_index)
                        .map(|entry| entry.location.clone())
                        .or_else(|| entries.first().map(|entry| entry.location.clone()));
                }

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} references in {} files",
                            entries.len(),
                            file_count
                        ))
                        .weak(),
                    );
                    ui.separator();
                    ui.label(RichText::new("Enter: open | Esc: close").weak());
                });
                ui.separator();

                let results_width = ui.available_width();
                ScrollArea::vertical()
                    .id_salt("references_overlay_results")
                    .auto_shrink([false, false])
                    .max_height(410.0)
                    .show(ui, |ui| {
                        ui.set_min_width(results_width);
                        if entries.is_empty() {
                            ui.label(RichText::new("No references").weak());
                            return;
                        }

                        let mut index = 0;
                        while index < entries.len() {
                            let group_path = entries[index].path.clone();
                            let group_display_path = entries[index].display_path.clone();
                            let group_start = index;
                            while index < entries.len() && entries[index].path == group_path {
                                index += 1;
                            }
                            let group_end = index;

                            ui.horizontal(|ui| {
                                ui.label(RichText::new(group_display_path).monospace().strong());
                                ui.label(
                                    RichText::new(format!(
                                        "{} references",
                                        group_end - group_start
                                    ))
                                    .weak(),
                                );
                            });

                            for (entry_index, entry) in
                                entries[group_start..group_end].iter().enumerate()
                            {
                                let selected_index = group_start + entry_index;
                                let selected =
                                    selected_index == self.references_overlay.selected_index;
                                let text = if entry.preview.is_empty() {
                                    format!("{:>5}", entry.line_number)
                                } else {
                                    format!("{:>5}  {}", entry.line_number, entry.preview)
                                };
                                let response = ui.add_sized(
                                    egui::vec2(results_width, 24.0),
                                    egui::Button::new(RichText::new(text).monospace())
                                        .selected(selected),
                                );
                                if response.hovered() {
                                    self.references_overlay.selected_index = selected_index;
                                }
                                if response.clicked() {
                                    selected_location = Some(entry.location.clone());
                                }
                            }
                            ui.add_space(6.0);
                        }
                    });
            });

        if close {
            self.close_references_overlay();
            return None;
        }

        if selected_location.is_some() {
            self.close_references_overlay();
        }

        selected_location
    }

    fn show_workspace_rename_window(&mut self, ui: &mut egui::Ui) {
        if !self.editing_session_name {
            return;
        }

        let current_name = self
            .current_session()
            .map(|session| session.display_name.clone())
            .unwrap_or_else(|| "No session".to_owned());
        let input_id = egui::Id::new("workspace_rename_name");
        if self.session_name_select_all {
            let mut state =
                TextEditState::load(ui.ctx(), input_id).unwrap_or_else(TextEditState::default);
            state.cursor.set_char_range(Some(CCursorRange::two(
                CCursor::new(0),
                CCursor::new(self.session_name_edit.chars().count()),
            )));
            state.store(ui.ctx(), input_id);
            self.session_name_select_all = false;
        }

        let mut rename = false;
        let mut close = false;
        egui::Window::new("Rename Workspace")
            .id(egui::Id::new("workspace_rename_window"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
            .fixed_size(egui::vec2(420.0, 180.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                let (escape, enter) = ui.input_mut(|input| {
                    (
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                    )
                });
                if escape {
                    close = true;
                }

                ui.label(RichText::new("Esc: close").weak());
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Current").weak());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(current_name.clone()).monospace());
                    });
                });
                ui.add_space(8.0);
                ui.label("New name");
                ui.add_sized(
                    [ui.available_width(), 22.0],
                    TextEdit::singleline(&mut self.session_name_edit)
                        .id(input_id)
                        .font(TextStyle::Monospace),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Rename").clicked() || enter {
                        rename = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
                ui.memory_mut(|memory| memory.request_focus(input_id));
            });

        if rename {
            self.commit_session_rename();
        } else if close {
            self.cancel_session_rename();
        }
    }

    fn show_workspace_file_dialog(&mut self, ui: &mut egui::Ui) {
        let Some(mut dialog) = self.workspace_file_dialog.take() else {
            return;
        };

        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        let mut keep_open = true;
        let mut action = None;

        match &mut dialog {
            WorkspaceFileDialog::NewFile {
                folder,
                file_name,
                error,
            } => {
                let folder_label = workspace_search_display_path(folder, &roots);
                egui::Window::new("New File")
                    .id(egui::Id::new("workspace_new_file_window"))
                    .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
                    .fixed_size(egui::vec2(420.0, 180.0))
                    .collapsible(false)
                    .resizable(false)
                    .show(ui.ctx(), |ui| {
                        if ui.input_mut(|input| {
                            input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                        }) {
                            keep_open = false;
                        }

                        ui.label(RichText::new("Esc: close").weak());
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Folder").weak());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(RichText::new(folder_label.clone()).monospace());
                                },
                            );
                        });
                        ui.add_space(8.0);
                        ui.label("File name");
                        let edit_id = ui.id().with("workspace_new_file_name");
                        ui.add_sized(
                            [ui.available_width(), 22.0],
                            TextEdit::singleline(file_name)
                                .id(edit_id)
                                .font(TextStyle::Monospace),
                        );
                        if let Some(error) = error {
                            ui.label(
                                RichText::new(error.as_str()).color(ui.visuals().warn_fg_color),
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Create").clicked()
                                || ui.input(|input| input.key_pressed(egui::Key::Enter))
                            {
                                action = Some(WorkspaceFileDialogAction::CreateFile {
                                    folder: folder.clone(),
                                    file_name: file_name.clone(),
                                });
                            }
                            if ui.button("Cancel").clicked() {
                                keep_open = false;
                            }
                        });
                        ui.memory_mut(|memory| memory.request_focus(edit_id));
                    });
            }
            WorkspaceFileDialog::Rename {
                path,
                new_name,
                select_stem,
                error,
            } => {
                let path_label = workspace_search_display_path(path, &roots);
                egui::Window::new("Rename File")
                    .id(egui::Id::new("workspace_rename_file_window"))
                    .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
                    .fixed_size(egui::vec2(420.0, 180.0))
                    .collapsible(false)
                    .resizable(false)
                    .show(ui.ctx(), |ui| {
                        if ui.input_mut(|input| {
                            input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                        }) {
                            keep_open = false;
                        }

                        ui.label(RichText::new("Esc: close").weak());
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Current").weak());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(RichText::new(path_label.clone()).monospace());
                                },
                            );
                        });
                        ui.add_space(8.0);
                        ui.label("New name");
                        let edit_id = ui.id().with("workspace_rename_file_name");
                        if *select_stem {
                            let selection_end = file_name_stem_selection_char_len(new_name);
                            let mut state = TextEditState::load(ui.ctx(), edit_id)
                                .unwrap_or_else(TextEditState::default);
                            state.cursor.set_char_range(Some(CCursorRange::two(
                                CCursor::new(0),
                                CCursor::new(selection_end),
                            )));
                            state.store(ui.ctx(), edit_id);
                            *select_stem = false;
                        }
                        ui.add_sized(
                            [ui.available_width(), 22.0],
                            TextEdit::singleline(new_name)
                                .id(edit_id)
                                .font(TextStyle::Monospace),
                        );
                        if let Some(error) = error {
                            ui.label(
                                RichText::new(error.as_str()).color(ui.visuals().warn_fg_color),
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Rename").clicked()
                                || ui.input(|input| input.key_pressed(egui::Key::Enter))
                            {
                                action = Some(WorkspaceFileDialogAction::RenameFile {
                                    path: path.clone(),
                                    new_name: new_name.clone(),
                                });
                            }
                            if ui.button("Cancel").clicked() {
                                keep_open = false;
                            }
                        });
                        ui.memory_mut(|memory| memory.request_focus(edit_id));
                    });
            }
            WorkspaceFileDialog::Delete { path, error } => {
                let path_label = workspace_search_display_path(path, &roots);
                egui::Window::new("Delete File")
                    .id(egui::Id::new("workspace_delete_file_window"))
                    .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
                    .fixed_size(egui::vec2(420.0, 160.0))
                    .collapsible(false)
                    .resizable(false)
                    .show(ui.ctx(), |ui| {
                        if ui.input_mut(|input| {
                            input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                        }) {
                            keep_open = false;
                        }

                        ui.label(RichText::new("Esc: close").weak());
                        ui.separator();
                        ui.label(RichText::new(path_label.clone()).monospace());
                        ui.label(
                            RichText::new("This cannot be undone.")
                                .color(ui.visuals().warn_fg_color),
                        );
                        if let Some(error) = error {
                            ui.label(
                                RichText::new(error.as_str()).color(ui.visuals().warn_fg_color),
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Delete").clicked()
                                || ui.input(|input| input.key_pressed(egui::Key::Enter))
                            {
                                action = Some(WorkspaceFileDialogAction::DeleteFile {
                                    path: path.clone(),
                                });
                            }
                            if ui.button("Cancel").clicked() {
                                keep_open = false;
                            }
                        });
                    });
            }
        }

        if let Some(action) = action {
            let result = match action {
                WorkspaceFileDialogAction::CreateFile { folder, file_name } => {
                    self.create_workspace_file(&folder, &file_name)
                }
                WorkspaceFileDialogAction::RenameFile { path, new_name } => {
                    self.rename_workspace_file(&path, &new_name)
                }
                WorkspaceFileDialogAction::DeleteFile { path } => self.delete_workspace_file(&path),
            };

            match result {
                Ok(()) => keep_open = false,
                Err(message) => match &mut dialog {
                    WorkspaceFileDialog::NewFile { error, .. }
                    | WorkspaceFileDialog::Rename { error, .. }
                    | WorkspaceFileDialog::Delete { error, .. } => {
                        *error = Some(message);
                    }
                },
            }
        }

        if keep_open {
            self.workspace_file_dialog = Some(dialog);
        }
    }

    fn show_recent_workspaces_window(&mut self, ui: &mut egui::Ui) {
        if !self.recent_workspaces_open {
            return;
        }

        let mut sessions = self.app_state.sessions.clone();
        sessions.sort_by(|left, right| {
            right
                .last_updated_unix
                .cmp(&left.last_updated_unix)
                .then_with(|| left.display_name.cmp(&right.display_name))
        });
        let current_session_id = self.app_state.current_session_id.clone();
        let session_count = self.app_state.sessions.len();
        let mut close = false;

        egui::Window::new("Recent Workspaces")
            .id(egui::Id::new("recent_workspaces_window"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
            .fixed_size(egui::vec2(640.0, 420.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
                {
                    close = true;
                }

                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} workspaces", sessions.len())).weak());
                    ui.separator();
                    ui.label(RichText::new("Esc: close").weak());
                });
                ui.separator();

                ScrollArea::vertical()
                    .id_salt("recent_workspaces_results")
                    .auto_shrink([false, false])
                    .max_height(360.0)
                    .show(ui, |ui| {
                        if sessions.is_empty() {
                            ui.label(RichText::new("No recent workspaces").weak());
                            return;
                        }

                        for session in sessions {
                            let is_current = session.id == current_session_id;
                            let is_pending_delete =
                                self.pending_delete_session_id.as_deref() == Some(&session.id);
                            let can_delete = !is_current && session_count > 1;
                            let title = if is_current {
                                format!("▶ {}", session.display_name)
                            } else {
                                session.display_name.clone()
                            };
                            let row_width = ui.available_width();
                            let name_width = 180.0;
                            let age_width = 76.0;
                            let row_height = 22.0;
                            let column_gap = 16.0;
                            let age_label = recent_workspace_age_label(session.last_updated_unix);
                            let row_text_color = ui.visuals().weak_text_color();
                            let body_font = TextStyle::Body.resolve(ui.style());
                            let name_font = TextStyle::Body.resolve(ui.style());
                            let draw_left_text =
                                |ui: &egui::Ui,
                                 rect: egui::Rect,
                                 text: String,
                                 font: egui::FontId,
                                 color: egui::Color32| {
                                    ui.painter().with_clip_rect(rect).text(
                                        rect.left_center(),
                                        egui::Align2::LEFT_CENTER,
                                        text,
                                        font,
                                        color,
                                    );
                                };

                            let (row_rect, _) = ui.allocate_exact_size(
                                egui::vec2(row_width, row_height),
                                egui::Sense::hover(),
                            );
                            let name_rect = egui::Rect::from_min_size(
                                row_rect.min,
                                egui::vec2(name_width, row_height),
                            );
                            let age_rect = egui::Rect::from_min_size(
                                egui::pos2(name_rect.right() + column_gap, row_rect.top()),
                                egui::vec2(age_width, row_height),
                            );
                            let delete_rect = egui::Rect::from_min_size(
                                egui::pos2(row_rect.right() - 22.0, row_rect.top() + 1.0),
                                egui::vec2(22.0, row_height - 2.0),
                            );
                            let path_rect = egui::Rect::from_min_max(
                                egui::pos2(age_rect.right() + column_gap, row_rect.top()),
                                egui::pos2(delete_rect.left() - 8.0, row_rect.bottom()),
                            );

                            let name_response = ui.interact(
                                name_rect,
                                ui.make_persistent_id(("recent_workspace", &session.id)),
                                egui::Sense::click(),
                            );
                            if name_response.hovered() {
                                ui.painter().rect_filled(
                                    name_rect.shrink2(egui::vec2(0.0, 2.0)),
                                    2.0,
                                    ui.visuals().widgets.hovered.bg_fill,
                                );
                            }
                            if is_current {
                                ui.painter().rect_filled(
                                    name_rect.shrink2(egui::vec2(0.0, 2.0)),
                                    2.0,
                                    ui.visuals().selection.bg_fill,
                                );
                            }
                            if name_response.clicked() && !is_current && !is_pending_delete {
                                self.switch_session(&session.id);
                                close = true;
                            }

                            draw_left_text(
                                ui,
                                name_rect.shrink2(egui::vec2(4.0, 0.0)),
                                title,
                                name_font.clone(),
                                if is_current {
                                    ui.visuals().selection.stroke.color
                                } else {
                                    ui.visuals().text_color()
                                },
                            );
                            draw_left_text(
                                ui,
                                age_rect,
                                age_label,
                                body_font.clone(),
                                row_text_color,
                            );
                            let primary_root = session
                                .root_folders
                                .first()
                                .map(|root| root.display().to_string())
                                .unwrap_or_else(|| "<no root>".to_owned());
                            draw_left_text(
                                ui,
                                path_rect,
                                primary_root,
                                body_font.clone(),
                                row_text_color,
                            );

                            if can_delete
                                && ui
                                    .put(delete_rect, egui::Button::new("×"))
                                    .on_hover_text("Delete session")
                                    .clicked()
                            {
                                self.pending_delete_session_id = Some(session.id.clone());
                            }
                            if is_pending_delete {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("Delete this session?")
                                            .small()
                                            .color(ui.visuals().warn_fg_color),
                                    );
                                    if ui.button("Delete").clicked() {
                                        self.delete_session(&session.id);
                                    }
                                    if ui.button("Cancel").clicked() {
                                        self.pending_delete_session_id = None;
                                    }
                                });
                            }
                            for root in session.root_folders.iter().skip(1).take(3) {
                                let (root_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(row_width, row_height),
                                    egui::Sense::hover(),
                                );
                                let root_path_rect = egui::Rect::from_min_max(
                                    egui::pos2(path_rect.left(), root_rect.top()),
                                    egui::pos2(path_rect.right(), root_rect.bottom()),
                                );
                                draw_left_text(
                                    ui,
                                    root_path_rect,
                                    format!("+ {}", root.display()),
                                    body_font.clone(),
                                    row_text_color,
                                );
                            }
                            if session.root_folders.len() > 4 {
                                let (more_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(row_width, row_height),
                                    egui::Sense::hover(),
                                );
                                let more_path_rect = egui::Rect::from_min_max(
                                    egui::pos2(path_rect.left(), more_rect.top()),
                                    egui::pos2(path_rect.right(), more_rect.bottom()),
                                );
                                draw_left_text(
                                    ui,
                                    more_path_rect,
                                    format!("+ {} more roots", session.root_folders.len() - 4),
                                    body_font,
                                    row_text_color,
                                );
                            }
                            ui.add_space(6.0);
                        }
                    });
            });

        if close {
            self.recent_workspaces_open = false;
            self.pending_delete_session_id = None;
        }
    }

    fn show_ai_command_window(&mut self, ui: &mut egui::Ui) {
        if !self.ai_command_open {
            return;
        }

        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        let mut close = false;
        let mut copy = false;
        let mut changed = false;

        egui::Window::new("AI Cmd")
            .id(egui::Id::new("ai_command_window"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 88.0))
            .fixed_size(egui::vec2(520.0, 300.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
                {
                    close = true;
                }

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Esc: close").weak());
                });
                ui.separator();

                {
                    let settings = &mut self.app_state.cli_agent;
                    ui.label("Command");
                    changed |= ui
                        .add(
                            TextEdit::singleline(&mut settings.executable)
                                .desired_width(f32::INFINITY),
                        )
                        .changed();
                    ui.add_space(6.0);

                    ui.label("Add dir option");
                    changed |= ui
                        .add(
                            TextEdit::singleline(&mut settings.add_dir_arg)
                                .desired_width(f32::INFINITY),
                        )
                        .changed();
                }

                ui.add_space(8.0);
                ui.label(RichText::new("Preview").strong());
                match build_cli_agent_command(&self.app_state.cli_agent, &roots) {
                    Ok(command) => {
                        ui.group(|ui| {
                            ui.set_width(ui.available_width());
                            ui.label(RichText::new(command.preview).monospace());
                        });
                    }
                    Err(error) => {
                        ui.label(RichText::new(error).color(ui.visuals().warn_fg_color));
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_copy =
                        build_cli_agent_command(&self.app_state.cli_agent, &roots).is_ok();
                    if ui
                        .add_enabled(can_copy, egui::Button::new("Copy"))
                        .clicked()
                    {
                        copy = true;
                    }
                });
            });

        if changed {
            self.save_state_silent();
        }
        if copy {
            self.copy_cli_agent_command();
        }
        if close {
            self.ai_command_open = false;
        }
    }

    fn show_shortcuts_help_window(&mut self, ui: &mut egui::Ui) {
        if !self.shortcuts_help_open {
            return;
        }

        let groups = self.shortcut_help_groups();
        egui::Window::new("Help")
            .id(egui::Id::new("shortcuts_help_window"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 56.0))
            .fixed_size(egui::vec2(520.0, 560.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.shortcut_context_title()).strong());
                    ui.separator();
                    ui.label(RichText::new("Esc: close").weak());
                });
                ui.separator();

                let row_height = 24.0;
                let shortcut_width = 164.0;
                let column_gap = 20.0;
                let body_font = TextStyle::Body.resolve(ui.style());
                let monospace_font = TextStyle::Monospace.resolve(ui.style());
                let weak_text_color = ui.visuals().weak_text_color();
                let draw_left_text = |ui: &egui::Ui,
                                      rect: egui::Rect,
                                      text: &str,
                                      font: egui::FontId,
                                      color: egui::Color32| {
                    ui.painter().with_clip_rect(rect).text(
                        rect.left_center(),
                        egui::Align2::LEFT_CENTER,
                        text,
                        font,
                        color,
                    );
                };

                ScrollArea::vertical()
                    .id_salt("shortcuts_help_groups")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (group_index, group) in groups.into_iter().enumerate() {
                            let row_width = ui.available_width();
                            if group_index > 0 {
                                ui.add_space(10.0);
                            }

                            let (group_rect, _) = ui.allocate_exact_size(
                                egui::vec2(row_width, row_height),
                                egui::Sense::hover(),
                            );
                            ui.painter().rect_filled(
                                group_rect.shrink2(egui::vec2(0.0, 2.0)),
                                2.0,
                                ui.visuals().extreme_bg_color,
                            );
                            draw_left_text(
                                ui,
                                group_rect.shrink2(egui::vec2(6.0, 0.0)),
                                group.title,
                                body_font.clone(),
                                ui.visuals().text_color(),
                            );
                            ui.add_space(4.0);

                            let (header_rect, _) = ui.allocate_exact_size(
                                egui::vec2(row_width, row_height),
                                egui::Sense::hover(),
                            );
                            let shortcut_header_rect = egui::Rect::from_min_size(
                                header_rect.min,
                                egui::vec2(shortcut_width, row_height),
                            );
                            let action_header_rect = egui::Rect::from_min_max(
                                egui::pos2(
                                    shortcut_header_rect.right() + column_gap,
                                    header_rect.top(),
                                ),
                                header_rect.right_bottom(),
                            );
                            draw_left_text(
                                ui,
                                shortcut_header_rect,
                                "Shortcut",
                                body_font.clone(),
                                weak_text_color,
                            );
                            draw_left_text(
                                ui,
                                action_header_rect,
                                "Action",
                                body_font.clone(),
                                weak_text_color,
                            );
                            ui.separator();

                            for shortcut in group.shortcuts {
                                let (row_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(row_width, row_height),
                                    egui::Sense::hover(),
                                );
                                let shortcut_rect = egui::Rect::from_min_size(
                                    row_rect.min,
                                    egui::vec2(shortcut_width, row_height),
                                );
                                let action_rect = egui::Rect::from_min_max(
                                    egui::pos2(shortcut_rect.right() + column_gap, row_rect.top()),
                                    row_rect.right_bottom(),
                                );
                                draw_left_text(
                                    ui,
                                    shortcut_rect,
                                    shortcut.keys,
                                    monospace_font.clone(),
                                    ui.visuals().text_color(),
                                );
                                draw_left_text(
                                    ui,
                                    action_rect,
                                    shortcut.label,
                                    body_font.clone(),
                                    ui.visuals().text_color(),
                                );
                            }
                            ui.add_space(8.0);
                        }
                    });
            });
    }

    fn shortcut_context_title(&self) -> &'static str {
        if self.search_overlay.open {
            return "Search Everywhere";
        }
        if self.recent_workspaces_open {
            return "Recent Workspaces";
        }
        if self.git_create_branch_root.is_some() {
            return "Create Branch";
        }
        if self.git_switch_branch_root.is_some() {
            return "Switch Branch";
        }
        if self.pending_git_discard.is_some() {
            return "Discard Changes";
        }
        if self.editing_session_name {
            return "Rename Workspace";
        }
        if self.sidebar_view == SidebarView::Search && self.workspace_search.open {
            return "Workspace Search";
        }
        if self.show_workspace_sidebar
            && self.sidebar_view == SidebarView::Workspace
            && self.workspace_sidebar_active
        {
            return "Workspace Files";
        }

        let editor = self.focused_editor();
        if editor.markdown_search.open {
            "Markdown Search"
        } else if editor.source_search.open {
            "Source Search"
        } else if editor.is_markdown() {
            match editor.markdown_mode {
                MarkdownMode::Preview => match editor.markdown_focus_target {
                    MarkdownFocusTarget::Pages => "Markdown Pages",
                    MarkdownFocusTarget::Outline => "Markdown Outline",
                    MarkdownFocusTarget::Body => "Markdown Preview",
                },
                MarkdownMode::Markdown => "Markdown Source",
                MarkdownMode::Both => match editor.markdown_focus_target {
                    MarkdownFocusTarget::Pages => "Markdown Pages",
                    MarkdownFocusTarget::Outline => "Markdown Outline",
                    MarkdownFocusTarget::Body => "Markdown Both",
                },
            }
        } else if editor.is_csv() && editor.csv_mode == CsvMode::Grid {
            "CSV Grid"
        } else if editor.diff_view.is_some() {
            "Diff View"
        } else if editor.git_history_view.is_some() {
            "Git History"
        } else {
            "Source"
        }
    }

    fn status_bar_shortcuts(&self) -> Vec<ShortcutHint> {
        if self.search_overlay.open {
            return shortcuts(&[("Enter", "Open"), ("Up/Down", "Select"), ("Esc", "Close")]);
        }
        if self.recent_workspaces_open {
            return shortcuts(&[("Esc", "Close")]);
        }
        if self.git_create_branch_root.is_some()
            || self.git_switch_branch_root.is_some()
            || self.pending_git_discard.is_some()
            || self.editing_session_name
        {
            return shortcuts(&[("Enter", "Confirm"), ("Esc", "Close")]);
        }
        if self.sidebar_view == SidebarView::Search && self.workspace_search.open {
            return shortcuts(&[
                ("Enter", "Search"),
                ("Esc", "Clear"),
                ("Ctrl+Shift+F", "Workspace"),
            ]);
        }
        if self.show_workspace_sidebar
            && self.sidebar_view == SidebarView::Workspace
            && self.workspace_sidebar_active
        {
            return shortcuts(&[("F2", "Rename"), ("Delete", "Delete")]);
        }

        let editor = self.focused_editor();
        if editor.is_markdown() {
            return match editor.markdown_mode {
                MarkdownMode::Preview => match editor.markdown_focus_target {
                    MarkdownFocusTarget::Pages => {
                        shortcuts(&[("Up/Down", "Page"), ("Enter", "Open"), ("Ctrl+F", "Find")])
                    }
                    MarkdownFocusTarget::Outline => shortcuts(&[
                        ("Up/Down", "Heading"),
                        ("Enter", "Jump"),
                        ("Ctrl+F", "Find"),
                    ]),
                    MarkdownFocusTarget::Body => shortcuts(&[
                        ("Ctrl+PgUp/PgDn", "Page"),
                        ("Ctrl+F", "Find"),
                        ("Alt+1/2/3", "Mode"),
                    ]),
                },
                MarkdownMode::Markdown => shortcuts(&[
                    ("Alt+1/2/3", "Mode"),
                    ("Ctrl+F", "Find"),
                    ("Ctrl+S", "Save"),
                    ("F12", "Definition"),
                ]),
                MarkdownMode::Both => shortcuts(&[
                    ("Ctrl+PgUp/PgDn", "Page"),
                    ("Ctrl+F", "Find"),
                    ("Alt+1/2/3", "Mode"),
                ]),
            };
        }
        if editor.is_csv() && editor.csv_mode == CsvMode::Grid {
            return if editor.has_active_csv_selection() {
                shortcuts(&[
                    ("Ctrl+C", "Copy Row/Column"),
                    ("Ctrl+V", "Paste Row/Column"),
                    ("Delete", "Remove Row/Column"),
                    ("Ctrl+Z/Y", "Undo/Redo"),
                ])
            } else {
                shortcuts(&[
                    ("Tab", "Next Cell"),
                    ("Enter", "Next Row"),
                    ("Arrows", "Move Cell"),
                    ("Ctrl+Z/Y", "Undo/Redo"),
                ])
            };
        }
        if editor.diff_view.is_some() || editor.git_history_view.is_some() {
            return shortcuts(&[("PgUp/PgDn", "Scroll"), ("Alt+Left", "Back")]);
        }

        shortcuts(&[
            ("F12", "Definition"),
            ("Ctrl+F", "Find"),
            ("Alt+Left/Right", "History"),
            ("Ctrl+S", "Save"),
        ])
    }

    fn shortcut_help_groups(&self) -> Vec<ShortcutGroup> {
        let mut groups = Vec::new();
        groups.push(ShortcutGroup {
            title: self.shortcut_context_title(),
            shortcuts: self.context_shortcuts(),
        });
        groups.push(ShortcutGroup {
            title: "Common",
            shortcuts: shortcuts(&[
                ("Ctrl+F", "Find current file"),
                ("Ctrl+Shift+F", "Workspace Search"),
                ("Ctrl+P", "Search Everywhere"),
                ("Ctrl+Shift+O", "Search Everywhere"),
                ("Shift x2", "Search Everywhere"),
                ("Ctrl+Shift+A", "Copy AI command"),
                ("Alt+Left", "Back"),
                ("Alt+Right", "Forward"),
                ("F12", "Go to Definition"),
                ("Ctrl+S", "Save"),
                ("F1", "Show Help"),
            ]),
        });
        groups.push(ShortcutGroup {
            title: "Search",
            shortcuts: shortcuts(&[
                ("Enter", "Next / open selected result"),
                ("Shift+Enter", "Previous match"),
                ("F3", "Next match"),
                ("Shift+F3", "Previous match"),
                ("Esc", "Close"),
                ("Up/Down", "Move selection"),
            ]),
        });
        groups
    }

    fn context_shortcuts(&self) -> Vec<ShortcutHint> {
        match self.shortcut_context_title() {
            "Search Everywhere" => shortcuts(&[
                ("Enter", "Open selected result"),
                ("Up/Down", "Move selection"),
                ("Esc", "Close"),
            ]),
            "Recent Workspaces" => shortcuts(&[("Esc", "Close")]),
            "Switch Branch" => shortcuts(&[
                ("Up/Down", "Select branch"),
                ("Enter", "Switch"),
                ("Esc", "Close"),
            ]),
            "Create Branch" | "Discard Changes" | "Rename Workspace" => {
                shortcuts(&[("Enter", "Confirm"), ("Esc", "Close")])
            }
            "Workspace Search" => shortcuts(&[("Enter", "Run search"), ("Esc", "Clear")]),
            "Workspace Files" => shortcuts(&[
                ("F2", "Rename selected file"),
                ("Delete", "Delete selected file"),
                ("Esc", "Close dialogs"),
            ]),
            "Markdown Search" | "Source Search" => shortcuts(&[
                ("Enter", "Next match"),
                ("Shift+Enter", "Previous match"),
                ("F3", "Next match"),
                ("Shift+F3", "Previous match"),
                ("Esc", "Close"),
            ]),
            "Markdown Preview" | "Markdown Both" => shortcuts(&[
                ("Alt+1", "Preview mode"),
                ("Alt+2", "Markdown source mode"),
                ("Alt+3", "Both mode"),
                ("Ctrl+PageUp", "Previous page"),
                ("Ctrl+PageDown", "Next page"),
                ("Ctrl+F", "Find rendered Markdown"),
            ]),
            "Markdown Pages" => shortcuts(&[
                ("Up/Down", "Select page"),
                ("Enter", "Open page"),
                ("Ctrl+PageUp", "Previous page"),
                ("Ctrl+PageDown", "Next page"),
            ]),
            "Markdown Outline" => shortcuts(&[
                ("Up/Down", "Select heading"),
                ("Enter", "Jump to heading"),
                ("Ctrl+F", "Find rendered Markdown"),
            ]),
            "Markdown Source" => shortcuts(&[
                ("Alt+1/2/3", "Change Markdown mode"),
                ("Ctrl+F", "Find source"),
                ("F12", "Go to Definition"),
                ("Ctrl+S", "Save"),
            ]),
            "CSV Grid" => shortcuts(&[
                ("Ctrl+Z", "Undo CSV grid edit"),
                ("Ctrl+Y", "Redo CSV grid edit"),
                ("Ctrl+Shift+Z", "Redo CSV grid edit"),
                ("Ctrl+C", "Copy selected row or column as CSV"),
                ("Ctrl+V", "Paste CSV into selected row or column"),
                ("Delete", "Delete selected row or column"),
                ("Backspace", "Delete selected row or column"),
                ("Tab", "Next cell"),
                ("Enter", "Next row"),
                ("Home/End", "First / last column"),
                ("Arrow keys", "Move between cells"),
            ]),
            "Diff View" | "Git History" => {
                shortcuts(&[("PageUp", "Page up"), ("PageDown", "Page down")])
            }
            _ => shortcuts(&[
                ("F12", "Go to Definition"),
                ("Ctrl+F", "Find source"),
                ("F3", "Next match"),
                ("Shift+F3", "Previous match"),
                ("Ctrl+A", "Select all"),
                ("Ctrl+C", "Copy selection"),
                ("Ctrl+V", "Paste"),
                ("Ctrl+Z", "Undo"),
                ("Ctrl+Y", "Redo"),
                ("Ctrl+Shift+Z", "Redo"),
                ("Arrows", "Move cursor"),
                ("Shift+Arrows", "Extend selection"),
                ("PageUp/PageDown", "Page scroll"),
            ]),
        }
    }

    fn show_git_create_branch_window(&mut self, ui: &mut egui::Ui) {
        let Some(repository_root) = self.git_create_branch_root.clone() else {
            return;
        };
        let Some(repository) = self
            .git_repositories
            .iter()
            .find(|repository| repository.root == repository_root)
            .cloned()
        else {
            self.git_create_branch_root = None;
            return;
        };

        let mut create_branch = None;
        let mut close = false;
        let input_id = egui::Id::new(("git_create_branch_name", &repository.root));

        egui::Window::new("Create Branch")
            .id(egui::Id::new("git_create_branch_window"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 96.0))
            .fixed_size(egui::vec2(420.0, 160.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                let (escape, enter) = ui.input_mut(|input| {
                    (
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                    )
                });
                if escape {
                    close = true;
                }

                ui.label(RichText::new("Esc: close").weak());
                ui.separator();

                ui.horizontal(|ui| {
                    ui.label(RichText::new(&repository.name).monospace());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("from: {}", repository.branch))
                                .monospace()
                                .weak(),
                        )
                        .on_hover_text(&repository.branch);
                    });
                });
                ui.separator();

                ui.label(RichText::new("Branch name"));
                let new_branch_name = self
                    .git_new_branch_names
                    .entry(repository.root.clone())
                    .or_default();
                let response = ui.add(
                    TextEdit::singleline(new_branch_name)
                        .id(input_id)
                        .hint_text("feature/new-branch")
                        .desired_width(f32::INFINITY),
                );
                response.request_focus();

                let can_create = !new_branch_name.trim().is_empty();
                if enter && can_create {
                    create_branch = Some(new_branch_name.trim().to_owned());
                    new_branch_name.clear();
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(can_create, egui::Button::new("Create"))
                        .clicked()
                    {
                        create_branch = Some(new_branch_name.trim().to_owned());
                        new_branch_name.clear();
                    }
                });
            });

        if let Some(branch_name) = create_branch {
            self.git_create_branch_root = None;
            self.create_git_branch(&repository, &branch_name);
        } else if close {
            self.git_create_branch_root = None;
        }
    }

    fn show_git_switch_branch_window(&mut self, ui: &mut egui::Ui) {
        let Some(repository_root) = self.git_switch_branch_root.clone() else {
            return;
        };
        let Some(repository) = self
            .git_repositories
            .iter()
            .find(|repository| repository.root == repository_root)
            .cloned()
        else {
            self.git_switch_branch_root = None;
            return;
        };

        let mut switch_to_branch = None;
        let mut close = false;
        let mut cursor = self
            .git_switch_branch_cursor
            .clone()
            .filter(|branch| repository.branches.contains(branch))
            .unwrap_or_else(|| repository.branch.clone());

        egui::Window::new("Switch Branch")
            .id(egui::Id::new("git_switch_branch_window"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 96.0))
            .fixed_size(egui::vec2(480.0, 320.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                let (escape, up, down, enter) = ui.input_mut(|input| {
                    (
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                    )
                });
                if escape {
                    close = true;
                }
                let cursor_index = repository
                    .branches
                    .iter()
                    .position(|branch| branch == &cursor);
                let moved_index = match cursor_index {
                    Some(index) if up => Some(index.saturating_sub(1)),
                    Some(index) if down => {
                        Some((index + 1).min(repository.branches.len().saturating_sub(1)))
                    }
                    None if up || down => Some(0),
                    _ => None,
                };
                if let Some(branch) = moved_index.and_then(|index| repository.branches.get(index)) {
                    cursor = branch.clone();
                }
                if enter {
                    if cursor == repository.branch || !repository.branches.contains(&cursor) {
                        close = true;
                    } else {
                        switch_to_branch = Some(cursor.clone());
                    }
                }

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{} branches", repository.branches.len())).weak(),
                    );
                    ui.separator();
                    ui.label(RichText::new("Up/Down: select  Enter: switch  Esc: close").weak());
                });
                ui.separator();

                ui.horizontal(|ui| {
                    ui.label(RichText::new(&repository.name).monospace());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("current: {}", repository.branch))
                                .monospace()
                                .weak(),
                        );
                    });
                });
                ui.separator();

                if repository.branches.is_empty() {
                    ui.label(RichText::new("No local branches").weak());
                } else {
                    let row_height = 24.0;
                    let current_width = 72.0;
                    let body_font = TextStyle::Body.resolve(ui.style());
                    let mono_font = TextStyle::Monospace.resolve(ui.style());
                    let draw_left_text =
                        |ui: &egui::Ui,
                         rect: egui::Rect,
                         text: &str,
                         font: egui::FontId,
                         color: egui::Color32| {
                            ui.painter().with_clip_rect(rect).text(
                                rect.left_center(),
                                egui::Align2::LEFT_CENTER,
                                text,
                                font,
                                color,
                            );
                        };
                    ScrollArea::vertical()
                        .id_salt("git_switch_branch_list")
                        .max_height(220.0)
                        .show(ui, |ui| {
                            for branch in &repository.branches {
                                let selected = branch == &repository.branch;
                                let row_width = ui.available_width();
                                let (row_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(row_width, row_height),
                                    egui::Sense::hover(),
                                );
                                let current_rect = egui::Rect::from_min_size(
                                    egui::pos2(row_rect.right() - current_width, row_rect.top()),
                                    egui::vec2(current_width, row_height),
                                );
                                let branch_rect = egui::Rect::from_min_max(
                                    row_rect.min,
                                    egui::pos2(current_rect.left() - 8.0, row_rect.bottom()),
                                );
                                let response = ui.interact(
                                    row_rect,
                                    ui.make_persistent_id(("git_switch_branch", branch)),
                                    if selected {
                                        egui::Sense::hover()
                                    } else {
                                        egui::Sense::click()
                                    },
                                );
                                let highlighted = branch == &cursor;
                                if response.hovered() || highlighted {
                                    ui.painter().rect_filled(
                                        row_rect.shrink2(egui::vec2(0.0, 2.0)),
                                        2.0,
                                        ui.visuals().widgets.hovered.bg_fill,
                                    );
                                }
                                if highlighted && (up || down) {
                                    ui.scroll_to_rect(row_rect, None);
                                }
                                if selected {
                                    ui.painter().rect_filled(
                                        row_rect.shrink2(egui::vec2(0.0, 2.0)),
                                        2.0,
                                        ui.visuals().selection.bg_fill,
                                    );
                                    if highlighted {
                                        ui.painter().rect_stroke(
                                            row_rect.shrink2(egui::vec2(0.0, 2.0)),
                                            2.0,
                                            ui.visuals().widgets.hovered.fg_stroke,
                                            egui::StrokeKind::Inside,
                                        );
                                    }
                                }

                                let text_color = if selected {
                                    ui.visuals().selection.stroke.color
                                } else {
                                    ui.visuals().text_color()
                                };
                                draw_left_text(
                                    ui,
                                    branch_rect.shrink2(egui::vec2(4.0, 0.0)),
                                    branch,
                                    mono_font.clone(),
                                    text_color,
                                );
                                if selected {
                                    draw_left_text(
                                        ui,
                                        current_rect,
                                        "current",
                                        body_font.clone(),
                                        ui.visuals().selection.stroke.color,
                                    );
                                }
                                if !selected
                                    && response.on_hover_text("Switch to this branch").clicked()
                                {
                                    switch_to_branch = Some(branch.clone());
                                }
                            }
                        });
                }
            });

        if let Some(branch_name) = switch_to_branch {
            self.git_switch_branch_root = None;
            self.git_switch_branch_cursor = None;
            self.switch_git_branch(&repository, &branch_name);
        } else if close {
            self.git_switch_branch_root = None;
            self.git_switch_branch_cursor = None;
        } else {
            self.git_switch_branch_cursor = Some(cursor);
        }
    }

    fn show_git_discard_window(&mut self, ui: &mut egui::Ui) {
        let Some(request) = self.pending_git_discard.clone() else {
            return;
        };

        let mut discard = false;
        let mut close = false;
        egui::Window::new("Discard Changes")
            .id(egui::Id::new("git_discard_window"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 96.0))
            .fixed_size(egui::vec2(440.0, 180.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                let (escape, enter) = ui.input_mut(|input| {
                    (
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                    )
                });
                if escape {
                    close = true;
                } else if enter {
                    discard = true;
                }

                ui.label(RichText::new("Repository").small().weak());
                ui.label(RichText::new(&request.repository_name).monospace());
                ui.add_space(8.0);
                ui.label(RichText::new("File").small().weak());
                ui.label(RichText::new(&request.path).monospace());
                ui.add_space(8.0);
                ui.label(
                    RichText::new("This will discard local changes for this file.")
                        .small()
                        .color(ui.visuals().warn_fg_color),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Enter confirms, so mark Discard as the default button.
                    if ui
                        .add(egui::Button::new("Discard").stroke(ui.visuals().selection.stroke))
                        .clicked()
                    {
                        discard = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });

        if discard {
            self.pending_git_discard = None;
            self.discard_git_file(&request);
        } else if close {
            self.pending_git_discard = None;
        }
    }

    fn show_git_commit_details_window(&mut self, ui: &mut egui::Ui) {
        let Some(details) = self.git_commit_details.clone() else {
            return;
        };

        let mut close = false;
        let mut open_commit_diff = false;
        egui::Window::new("Commit Details")
            .id(egui::Id::new("git_commit_details_window"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
            .fixed_size(egui::vec2(760.0, 520.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                let escape = ui
                    .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
                if escape {
                    close = true;
                }

                ui.horizontal(|ui| {
                    ui.label(RichText::new(&details.repository_name).monospace());
                    ui.separator();
                    ui.label(RichText::new(&details.commit.short_hash).monospace());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Open Diff").clicked() {
                            open_commit_diff = true;
                        }
                    });
                });
                ui.separator();

                ui.label(RichText::new(&details.commit.subject).strong());
                ui.add_space(4.0);
                egui::Grid::new("git_commit_details_metadata")
                    .num_columns(2)
                    .spacing([16.0, 4.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Author").weak());
                        ui.label(&details.commit.author);
                        ui.end_row();
                        ui.label(RichText::new("Date").weak());
                        ui.label(&details.commit.date);
                        ui.end_row();
                        ui.label(RichText::new("Hash").weak());
                        ui.label(RichText::new(&details.commit.hash).monospace());
                        ui.end_row();
                        ui.label(RichText::new("Parents").weak());
                        ui.label(if details.commit.parents.is_empty() {
                            "none".to_owned()
                        } else {
                            details.commit.parents.join(", ")
                        });
                        ui.end_row();
                    });

                if !details.commit.refs.is_empty() {
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Refs").weak());
                        for reference in &details.commit.refs {
                            let text = RichText::new(reference)
                                .monospace()
                                .small()
                                .color(Color32::WHITE);
                            egui::Frame::new()
                                .fill(git_ref_badge_color(reference))
                                .corner_radius(3.0)
                                .inner_margin(egui::Margin::symmetric(5, 2))
                                .show(ui, |ui| {
                                    ui.label(text);
                                });
                        }
                    });
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Changed Files").strong());
                    ui.label(RichText::new(format!("{}", details.files.len())).weak());
                });
                ui.separator();

                if let Some(error) = &details.error {
                    ui.label(RichText::new(error).color(ui.visuals().warn_fg_color));
                } else if details.files.is_empty() {
                    ui.label(RichText::new("No changed files").weak());
                } else {
                    ScrollArea::vertical()
                        .id_salt("git_commit_details_files")
                        .max_height(220.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for file in &details.files {
                                let display_path = file.display_path();
                                let label =
                                    format!("{:<3} {}", file.display_status(), display_path);
                                ui.label(RichText::new(label).monospace());
                            }
                        });
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });

        if open_commit_diff {
            self.open_git_commit_diff(&details);
            self.git_commit_details = None;
            return;
        }
        if close {
            self.git_commit_details = None;
        }
    }

    fn open_search_result(&mut self, result: SearchResult) {
        let kind = result.kind.label();
        let label = result.label.clone();
        self.open_location_in_focused_pane(result.location, true);
        self.status_message = format!("Opened {kind}: {label}");
    }

    fn save_focused_file(&mut self) {
        match self.focused_editor_mut().save_to_disk() {
            Ok(path) => {
                self.symbol_index.add_file(&path, &self.language_registry);
                self.bump_search_index_revision();
                self.status_message = format!("Saved file: {}", path.display());
            }
            Err(error) => {
                self.status_message = error;
            }
        }
    }

    fn perf_report_text(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "CodeReader perf report: build={} sidebar={} right_pane={} focused={}",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            self.show_workspace_sidebar,
            self.show_right_pane,
            self.focused_pane.label()
        ));
        lines.push(format!(
            "workspace_files={} symbols={} git_repositories={}",
            self.file_tree.files.len(),
            self.symbol_index.len(),
            self.git_repositories.len()
        ));
        lines.push(format!(
            "app_frame: frame={} total_ms={:.2} avg120_ms={:.2} max120_ms={:.2} stable_dt_ms={:.2} unstable_dt_ms={:.2} focused={} scroll_y={:.1}->{:.1} delta_y={:.1} raw_wheel=({:.1},{:.1}) smooth_scroll=({:.1},{:.1}) page_key={:?}",
            self.app_perf_debug.frame_index,
            self.app_perf_debug.last_total_ms,
            self.app_perf_debug.avg_total_ms,
            self.app_perf_debug.max_total_ms,
            self.app_perf_debug.last_stable_dt_ms,
            self.app_perf_debug.last_unstable_dt_ms,
            self.app_perf_debug.focused_pane.label(),
            self.app_perf_debug.scroll_before_y,
            self.app_perf_debug.scroll_after_y,
            self.app_perf_debug.scroll_delta_y,
            self.app_perf_debug.raw_wheel_delta.x,
            self.app_perf_debug.raw_wheel_delta.y,
            self.app_perf_debug.smooth_scroll_delta.x,
            self.app_perf_debug.smooth_scroll_delta.y,
            self.app_perf_debug.page_key
        ));
        for pane in [Pane::Left, Pane::Right] {
            let editor = self.editor(pane);
            if let Some(perf) = &editor.editor_perf_debug {
                lines.push(perf.report_line(pane, &editor.title));
            } else {
                lines.push(format!("{}: no perf sample", pane.label()));
            }
            lines.extend(editor.virtual_click_debug.report_lines(pane));
        }
        lines.join("\n")
    }

    fn diagnostics_report_text(&self) -> String {
        let mut lines = Vec::new();
        lines.push(self.perf_report_text());
        lines.push(String::new());
        lines.push("App details:".to_owned());
        lines.push(format!("status={}", self.status_message));
        lines.push(format!(
            "sidebar_view={} tab_width={} indexing={}",
            self.sidebar_view.label(),
            self.tab_width,
            self.symbol_index_receiver.is_some()
        ));
        lines.push(format!(
            "external_files={} external_packages={}",
            self.external_file_count, self.external_package_count
        ));
        lines.push(format!(
            "go_lsp={} initialized={} pending={} status={} last_kind={} last_request={} last_ms={} locations={} warmups={} syncs={} definitions={} references={} fallbacks={} timeouts={} error={} definition_status={} definition_ms={} definition_locations={} definition_error={} server_req={} server_notif={} reader_errors={} progress={} diagnostics={} last_server={}:{} server_message={}",
            self.go_lsp_client.is_some(),
            self.go_lsp_debug.initialized,
            self.pending_go_lsp_request
                .as_ref()
                .map(|request| format!("{:?}#{}", request.kind, request.request_id))
                .unwrap_or_else(|| "none".to_owned()),
            debug_or_none(&self.go_lsp_debug.last_status),
            self.go_lsp_debug
                .last_kind
                .map(|kind| format!("{kind:?}"))
                .unwrap_or_else(|| "none".to_owned()),
            self.go_lsp_debug
                .last_request_id
                .map(|request_id| request_id.to_string())
                .unwrap_or_else(|| "none".to_owned()),
            self.go_lsp_debug
                .last_duration_ms
                .map(|duration| format!("{duration:.2}"))
                .unwrap_or_else(|| "none".to_owned()),
            self.go_lsp_debug.last_location_count,
            self.go_lsp_debug.warmup_count,
            self.go_lsp_debug.sync_count,
            self.go_lsp_debug.definition_count,
            self.go_lsp_debug.reference_count,
            self.go_lsp_debug.fallback_count,
            self.go_lsp_debug.timeout_count,
            self.go_lsp_debug
                .last_error
                .as_deref()
                .map(debug_or_none)
                .unwrap_or_else(|| "none".to_owned()),
            debug_or_none(&self.go_lsp_debug.last_definition_status),
            self.go_lsp_debug
                .last_definition_duration_ms
                .map(|duration| format!("{duration:.2}"))
                .unwrap_or_else(|| "none".to_owned()),
            self.go_lsp_debug.last_definition_location_count,
            self.go_lsp_debug
                .last_definition_error
                .as_deref()
                .map(debug_or_none)
                .unwrap_or_else(|| "none".to_owned()),
            self.go_lsp_debug.server_request_count,
            self.go_lsp_debug.server_notification_count,
            self.go_lsp_debug.reader_error_count,
            self.go_lsp_debug.progress_count,
            self.go_lsp_debug.diagnostics_count,
            debug_or_none(&self.go_lsp_debug.last_server_kind),
            debug_or_none(&self.go_lsp_debug.last_server_method),
            self.go_lsp_debug
                .last_server_message
                .as_deref()
                .map(debug_or_none)
                .unwrap_or_else(|| "none".to_owned())
        ));
        lines.push(format!(
            "rust_lsp={} initialized={} pending={} status={} last_kind={} last_request={} last_ms={} locations={} warmups={} syncs={} definitions={} references={} fallbacks={} timeouts={} error={} definition_status={} definition_ms={} definition_locations={} definition_error={} server_req={} server_notif={} reader_errors={} progress={} diagnostics={} last_server={}:{} server_message={}",
            self.rust_lsp_client.is_some(),
            self.rust_lsp_debug.initialized,
            self.pending_rust_lsp_request
                .as_ref()
                .map(|request| format!("{:?}#{}", request.kind, request.request_id))
                .unwrap_or_else(|| "none".to_owned()),
            debug_or_none(&self.rust_lsp_debug.last_status),
            self.rust_lsp_debug
                .last_kind
                .map(|kind| format!("{kind:?}"))
                .unwrap_or_else(|| "none".to_owned()),
            self.rust_lsp_debug
                .last_request_id
                .map(|request_id| request_id.to_string())
                .unwrap_or_else(|| "none".to_owned()),
            self.rust_lsp_debug
                .last_duration_ms
                .map(|duration| format!("{duration:.2}"))
                .unwrap_or_else(|| "none".to_owned()),
            self.rust_lsp_debug.last_location_count,
            self.rust_lsp_debug.warmup_count,
            self.rust_lsp_debug.sync_count,
            self.rust_lsp_debug.definition_count,
            self.rust_lsp_debug.reference_count,
            self.rust_lsp_debug.fallback_count,
            self.rust_lsp_debug.timeout_count,
            self.rust_lsp_debug
                .last_error
                .as_deref()
                .map(debug_or_none)
                .unwrap_or_else(|| "none".to_owned()),
            debug_or_none(&self.rust_lsp_debug.last_definition_status),
            self.rust_lsp_debug
                .last_definition_duration_ms
                .map(|duration| format!("{duration:.2}"))
                .unwrap_or_else(|| "none".to_owned()),
            self.rust_lsp_debug.last_definition_location_count,
            self.rust_lsp_debug
                .last_definition_error
                .as_deref()
                .map(debug_or_none)
                .unwrap_or_else(|| "none".to_owned()),
            self.rust_lsp_debug.server_request_count,
            self.rust_lsp_debug.server_notification_count,
            self.rust_lsp_debug.reader_error_count,
            self.rust_lsp_debug.progress_count,
            self.rust_lsp_debug.diagnostics_count,
            debug_or_none(&self.rust_lsp_debug.last_server_kind),
            debug_or_none(&self.rust_lsp_debug.last_server_method),
            self.rust_lsp_debug
                .last_server_message
                .as_deref()
                .map(debug_or_none)
                .unwrap_or_else(|| "none".to_owned())
        ));
        lines.push(self.rust_lsp_readiness_report_line());
        match self.current_session() {
            Some(session) => {
                lines.push(format!(
                    "session={} roots={}",
                    session.display_name,
                    session.root_folders.len()
                ));
                for root in &session.root_folders {
                    lines.push(format!("  root={}", root.display()));
                }
            }
            None => lines.push("session=none".to_owned()),
        }
        match self.cli_agent_command() {
            Ok(command) => {
                lines.push(format!(
                    "ai_command: agent={} base_dir={}",
                    command.agent_name,
                    command.base_dir.display()
                ));
                lines.push(format!("ai_command_text={}", command.command));
            }
            Err(error) => {
                lines.push(format!("ai_command_error={error}"));
            }
        }

        lines.push(String::new());
        lines.push("Editor details:".to_owned());
        lines.extend(self.left.debug_report_lines(self.tab_width));
        lines.extend(self.right.debug_report_lines(self.tab_width));

        lines.push(String::new());
        lines.push(format!(
            "{}: {}",
            self.location_list_kind.report_title(),
            self.definition_candidates.len()
        ));
        if self.definition_candidates.is_empty() {
            lines.push("  none".to_owned());
        } else {
            for candidate in &self.definition_candidates {
                lines.push(format!(
                    "  {} -> {}:{}:{}",
                    candidate.label,
                    candidate.location.path.display(),
                    candidate.location.range.start.line + 1,
                    candidate.location.range.start.character + 1
                ));
            }
        }

        lines.push(String::new());
        self.append_symbol_report_lines(&mut lines);

        lines.join("\n")
    }

    fn append_symbol_report_lines(&self, lines: &mut Vec<String>) {
        const TOP_FILE_LIMIT: usize = 12;

        let mut source_counts: HashMap<&'static str, usize> = HashMap::new();
        let mut kind_counts: HashMap<String, usize> = HashMap::new();
        let mut file_counts: HashMap<PathBuf, usize> = HashMap::new();

        for symbol in self.symbol_index.symbols() {
            let source = match symbol.source {
                SymbolSource::Workspace => "workspace",
                SymbolSource::ExternalLibrary => "external",
            };
            *source_counts.entry(source).or_default() += 1;
            *kind_counts
                .entry(format!("{source}:{:?}", symbol.kind))
                .or_default() += 1;
            *file_counts.entry(symbol.location.path.clone()).or_default() += 1;
        }

        lines.push(format!("Symbol index: total={}", self.symbol_index.len()));
        lines.push(format!(
            "  by_source workspace={} external={}",
            source_counts.get("workspace").copied().unwrap_or(0),
            source_counts.get("external").copied().unwrap_or(0)
        ));

        let mut kind_counts = kind_counts.into_iter().collect::<Vec<_>>();
        kind_counts.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        lines.push("  by_kind:".to_owned());
        for (kind, count) in kind_counts {
            lines.push(format!("    {kind}={count}"));
        }

        let mut file_counts = file_counts.into_iter().collect::<Vec<_>>();
        file_counts.sort_by(|left, right| {
            right.1.cmp(&left.1).then_with(|| {
                left.0
                    .display()
                    .to_string()
                    .cmp(&right.0.display().to_string())
            })
        });
        lines.push(format!(
            "  top_symbol_files: showing={}",
            file_counts.len().min(TOP_FILE_LIMIT)
        ));
        for (path, count) in file_counts.into_iter().take(TOP_FILE_LIMIT) {
            lines.push(format!("    {count} {}", path.display()));
        }

        for pane in [Pane::Left, Pane::Right] {
            let editor = self.editor(pane);
            let Some(path) = &editor.path else {
                lines.push(format!("  {} current_file_symbols: no file", pane.label()));
                continue;
            };

            let current_symbols = self
                .symbol_index
                .symbols()
                .iter()
                .filter(|symbol| symbol.location.path == *path)
                .collect::<Vec<_>>();
            lines.push(format!(
                "  {} current_file_symbols: total={} showing=all path={}",
                pane.label(),
                current_symbols.len(),
                path.display()
            ));
            for symbol in current_symbols {
                lines.push(format!(
                    "    {:?} {}{} -> {}:{}",
                    symbol.kind,
                    symbol
                        .container_name
                        .as_ref()
                        .map(|container| format!("{container}::"))
                        .unwrap_or_default(),
                    symbol.name,
                    symbol.location.range.start.line + 1,
                    symbol.location.range.start.character + 1
                ));
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record_app_perf_sample(
        &mut self,
        total_ms: f32,
        stable_dt_ms: f32,
        unstable_dt_ms: f32,
        focused_pane: Pane,
        scroll_before_y: f32,
        scroll_after_y: f32,
        raw_wheel_delta: egui::Vec2,
        smooth_scroll_delta: egui::Vec2,
        page_key: Option<&'static str>,
    ) {
        const APP_PERF_SAMPLE_LIMIT: usize = 120;

        self.app_perf_debug.frame_index = self.app_perf_debug.frame_index.wrapping_add(1);
        self.app_perf_debug.last_total_ms = total_ms;
        self.app_perf_debug.last_stable_dt_ms = stable_dt_ms;
        self.app_perf_debug.last_unstable_dt_ms = unstable_dt_ms;
        self.app_perf_debug.focused_pane = focused_pane;
        self.app_perf_debug.scroll_before_y = scroll_before_y;
        self.app_perf_debug.scroll_after_y = scroll_after_y;
        self.app_perf_debug.scroll_delta_y = scroll_after_y - scroll_before_y;
        self.app_perf_debug.raw_wheel_delta = raw_wheel_delta;
        self.app_perf_debug.smooth_scroll_delta = smooth_scroll_delta;
        self.app_perf_debug.page_key = page_key;

        self.app_perf_debug.frame_samples.push(total_ms);
        if self.app_perf_debug.frame_samples.len() > APP_PERF_SAMPLE_LIMIT {
            let remove_count = self
                .app_perf_debug
                .frame_samples
                .len()
                .saturating_sub(APP_PERF_SAMPLE_LIMIT);
            self.app_perf_debug.frame_samples.drain(0..remove_count);
        }

        if self.app_perf_debug.frame_samples.is_empty() {
            self.app_perf_debug.avg_total_ms = 0.0;
            self.app_perf_debug.max_total_ms = 0.0;
        } else {
            let total = self.app_perf_debug.frame_samples.iter().sum::<f32>();
            self.app_perf_debug.avg_total_ms =
                total / self.app_perf_debug.frame_samples.len() as f32;
            self.app_perf_debug.max_total_ms = self
                .app_perf_debug
                .frame_samples
                .iter()
                .copied()
                .fold(0.0, f32::max);
        }
    }
}

impl eframe::App for CodeReaderApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let app_frame_start = Instant::now();
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
        let focused_pane_at_start = self.focused_pane;
        let scroll_before_y = self.focused_editor().scroll_y;
        let (stable_dt_ms, unstable_dt_ms, raw_wheel_delta, smooth_scroll_delta, page_key) = ui
            .input(|input| {
                let raw_wheel_delta = input
                    .raw
                    .events
                    .iter()
                    .filter_map(|event| {
                        if let egui::Event::MouseWheel { delta, .. } = event {
                            Some(*delta)
                        } else {
                            None
                        }
                    })
                    .fold(egui::Vec2::ZERO, |sum, delta| sum + delta);
                let page_key = if input.key_pressed(egui::Key::PageDown) {
                    Some("PageDown")
                } else if input.key_pressed(egui::Key::PageUp) {
                    Some("PageUp")
                } else {
                    None
                };
                (
                    input.stable_dt * 1000.0,
                    input.unstable_dt * 1000.0,
                    raw_wheel_delta,
                    input.smooth_scroll_delta(),
                    page_key,
                )
            });
        // Help is drawn last but sits on top, so take its Escape before a focused search box
        // in a panel sees it.
        if self.shortcuts_help_open
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.shortcuts_help_open = false;
        }
        self.poll_workspace_changes(ui.ctx());
        self.poll_symbol_index();
        self.poll_workspace_search(ui.ctx());
        self.poll_go_lsp(ui.ctx());
        self.poll_rust_lsp(ui.ctx());

        if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::F1)) {
            self.shortcuts_help_open = true;
        }
        if ui.input_mut(|input| {
            input.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::F)
        }) {
            self.open_workspace_search();
        } else if ui.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::F))
            && !self.open_markdown_search()
            && !self.open_source_search()
        {
            self.open_search_overlay();
        }

        if self.consume_search_shortcuts(ui) {
            self.open_search_overlay();
        }
        if !self.search_overlay.open && ui.input_mut(consume_ai_command_shortcut) {
            self.copy_cli_agent_command();
        }
        self.handle_workspace_file_shortcuts(ui);
        if !self.search_overlay.open && ui.input_mut(consume_navigation_back_shortcut) {
            self.go_back();
        }
        if !self.search_overlay.open && ui.input_mut(consume_navigation_forward_shortcut) {
            self.go_forward();
        }

        if !self.search_overlay.open && self.focused_editor().is_csv_grid() {
            if ui.input_mut(consume_undo_shortcut) {
                if self.focused_editor_mut().apply_csv_undo() {
                    self.status_message = "CSV undo.".to_owned();
                } else {
                    self.status_message = "No CSV edit to undo.".to_owned();
                }
            } else if ui.input_mut(consume_redo_shortcut) {
                if self.focused_editor_mut().apply_csv_redo() {
                    self.status_message = "CSV redo.".to_owned();
                } else {
                    self.status_message = "No CSV edit to redo.".to_owned();
                }
            }
        }

        if !self.search_overlay.open
            && self.focused_editor().has_diff_text_selection()
            && ui.input_mut(consume_copy_shortcut)
        {
            self.copy_diff_selection_to_clipboard(ui);
        } else if !self.search_overlay.open
            && (self.focused_editor().has_virtual_text_selection() || self.has_csv_selection())
            && ui.input_mut(consume_copy_shortcut)
            && !self.copy_virtual_selection_to_clipboard(ui)
            && self.has_csv_selection()
        {
            self.copy_selected_csv_to_clipboard(ui);
        }
        if self.has_csv_selection() {
            if let Some(text) = ui.input_mut(consume_paste_text) {
                self.paste_csv_text_from_clipboard(&text);
            } else if ui.input_mut(consume_paste_shortcut) {
                match read_text_from_system_clipboard() {
                    Ok(text) => self.paste_csv_text_from_clipboard(&text),
                    Err(error) => {
                        self.status_message =
                            format!("CSV paste requested, but clipboard read failed: {error}");
                    }
                }
            }
        }
        if self.has_csv_selection() && ui.input_mut(consume_delete_shortcut) {
            self.delete_selected_csv();
        }

        let pressed_f12 =
            !self.search_overlay.open && ui.input(|input| input.key_pressed(egui::Key::F12));
        if pressed_f12 {
            self.go_to_definition();
        }
        if ui.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::S)) {
            self.save_focused_file();
        }
        if !self.show_right_pane && self.focused_pane == Pane::Right {
            self.focused_pane = Pane::Left;
        }

        egui::Panel::top("top_bar").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(self.show_workspace_sidebar, "Sidebar")
                    .on_hover_text("Show or hide the workspace sidebar")
                    .clicked()
                {
                    self.show_workspace_sidebar = !self.show_workspace_sidebar;
                }
                if ui
                    .selectable_label(self.show_right_pane, "Right Pane")
                    .on_hover_text("Show or hide the right editor pane")
                    .clicked()
                {
                    self.show_right_pane = !self.show_right_pane;
                    if !self.show_right_pane && self.focused_pane == Pane::Right {
                        self.focused_pane = Pane::Left;
                    }
                }
                ui.separator();
                if ui
                    .button("New Workspace")
                    .on_hover_text("Create a new workspace")
                    .clicked()
                {
                    self.create_session();
                }
                if ui
                    .button("Workspaces")
                    .on_hover_text("Show recent workspaces")
                    .clicked()
                {
                    self.recent_workspaces_open = true;
                }
                if ui
                    .button("Everywhere")
                    .on_hover_text("Open Search Everywhere")
                    .clicked()
                {
                    self.recent_workspaces_open = false;
                    self.open_search_overlay();
                }
                if ui.button("AI Cmd").clicked() {
                    self.ai_command_open = true;
                }
                if ui.button("Help").on_hover_text("Show shortcuts").clicked() {
                    self.shortcuts_help_open = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("Report")
                        .on_hover_text("Copy perf and diagnostics report")
                        .clicked()
                    {
                        self.copy_diagnostics_report();
                    }
                });
            });
        });

        let status_message = self.status_bar_message();
        let status_shortcuts = self.status_bar_shortcuts();
        egui::Panel::bottom("status_bar").show_inside(ui, |ui| {
            const STATUS_BAR_HEIGHT: f32 = 26.0;
            const STATUS_BAR_PADDING_X: f32 = 6.0;
            const STATUS_BAR_GAP: f32 = 12.0;
            const STATUS_BAR_FONT_SIZE: f32 = 13.0;

            let shortcut_text = status_shortcuts
                .iter()
                .map(|shortcut| format!("{} {}", shortcut.keys, shortcut.label))
                .collect::<Vec<_>>()
                .join(" | ");
            let shortcut_color = ui.visuals().weak_text_color();
            let shortcut_width = if shortcut_text.is_empty() {
                0.0
            } else {
                ui.painter()
                    .layout_no_wrap(
                        shortcut_text.clone(),
                        FontId::monospace(STATUS_BAR_FONT_SIZE),
                        shortcut_color,
                    )
                    .size()
                    .x
                    + STATUS_BAR_PADDING_X
            };

            let (row_rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), STATUS_BAR_HEIGHT),
                egui::Sense::hover(),
            );
            let inner_rect = row_rect.shrink2(egui::vec2(STATUS_BAR_PADDING_X, 0.0));
            let shortcut_width = shortcut_width.min(inner_rect.width() * 0.55);
            let shortcut_rect = egui::Rect::from_min_max(
                egui::pos2(inner_rect.right() - shortcut_width, inner_rect.top()),
                inner_rect.right_bottom(),
            );
            let status_right = if shortcut_width > 0.0 {
                shortcut_rect.left() - STATUS_BAR_GAP
            } else {
                inner_rect.right()
            };
            let status_rect = egui::Rect::from_min_max(
                inner_rect.left_top(),
                egui::pos2(status_right.max(inner_rect.left()), inner_rect.bottom()),
            );

            let status_color = ui.visuals().text_color();
            let status_font = FontId::proportional(STATUS_BAR_FONT_SIZE);
            let status_text = elide_text_to_width(
                ui,
                &status_message,
                status_font.clone(),
                status_color,
                status_rect.width(),
            );
            let status_galley = ui
                .painter()
                .layout_no_wrap(status_text, status_font, status_color);
            let status_pos = egui::pos2(
                status_rect.left(),
                status_rect.center().y - status_galley.size().y * 0.5,
            );
            ui.painter().with_clip_rect(status_rect).galley(
                status_pos,
                status_galley,
                status_color,
            );
            ui.interact(
                status_rect,
                ui.id().with("status_bar_message"),
                egui::Sense::hover(),
            )
            .on_hover_text(status_message.as_str());

            if !shortcut_text.is_empty() && shortcut_rect.width() > 0.0 {
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(shortcut_rect)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    |ui| {
                        ui.add_sized(
                            shortcut_rect.size(),
                            egui::Label::new(
                                RichText::new(shortcut_text)
                                    .size(STATUS_BAR_FONT_SIZE)
                                    .monospace()
                                    .color(shortcut_color),
                            )
                            .truncate(),
                        );
                    },
                );
            }
        });

        if self.show_workspace_sidebar {
            let session_name = self
                .current_session()
                .map(|session| session.display_name.clone())
                .unwrap_or_else(|| "No session".to_owned());

            egui::Panel::left("workspace_sidebar")
                .resizable(true)
                .default_size(280.0)
                .size_range(220.0..=420.0)
                .show_inside(ui, |ui| {
                    ui.horizontal(|ui| {
                        for view in SidebarView::all() {
                            if ui
                                .selectable_label(self.sidebar_view == view, view.label())
                                .clicked()
                            {
                                self.sidebar_view = view;
                                if view == SidebarView::Search {
                                    self.workspace_search.open = true;
                                    self.workspace_search.focus_requested = true;
                                } else if view == SidebarView::Git {
                                    self.refresh_git_view();
                                }
                            }
                        }
                    });
                    ui.separator();

                    match self.sidebar_view {
                        SidebarView::Workspace => {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&session_name).strong());
                                if ui.button("Rename").clicked() {
                                    self.start_session_rename();
                                }
                                if ui.button("Add").clicked() {
                                    self.add_root_to_current_session();
                                }
                            });

                            ui.add_space(4.0);
                            ui.separator();
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Files").strong());
                                if ui.button("New File").clicked() {
                                    self.workspace_sidebar_active = true;
                                    self.start_workspace_new_file();
                                }
                                let has_selected_file = self.selected_workspace_file().is_some();
                                if ui
                                    .add_enabled(has_selected_file, egui::Button::new("Rename"))
                                    .on_hover_text("Rename selected file")
                                    .clicked()
                                {
                                    self.workspace_sidebar_active = true;
                                    self.start_workspace_rename_file();
                                }
                                if ui
                                    .add_enabled(has_selected_file, egui::Button::new("Delete"))
                                    .on_hover_text("Delete selected file")
                                    .clicked()
                                {
                                    self.workspace_sidebar_active = true;
                                    self.start_workspace_delete_file();
                                }
                            });

                            let footer_header_count = 1;
                            let file_tree_height = workspace_file_tree_height(
                                ui.available_height(),
                                footer_header_count,
                            );
                            self.ensure_git_repositories_loaded();
                            let git_file_statuses =
                                workspace_git_file_statuses(&self.git_repositories);
                            ScrollArea::vertical()
                                .id_salt("workspace_file_tree")
                                .max_height(file_tree_height)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    let root_count = self
                                        .current_session()
                                        .map(|session| session.root_folders.len())
                                        .unwrap_or_default();
                                    let file_tree = self.file_tree.clone();
                                    draw_workspace_roots(
                                        ui,
                                        &file_tree.roots,
                                        root_count,
                                        &git_file_statuses,
                                        self,
                                    );
                                });

                            let bookmark_id = ui.make_persistent_id("bookmarks");
                            let bookmark_state =
                                egui::collapsing_header::CollapsingState::load_with_default_open(
                                    ui.ctx(),
                                    bookmark_id,
                                    true,
                                );
                            bookmark_state
                                .show_header(ui, |ui| {
                                    ui.label("Bookmarks");
                                })
                                .body(|ui| {
                                    let bookmarks = self
                                        .current_session()
                                        .map(|session| session.bookmarks.clone())
                                        .unwrap_or_default();

                                    if bookmarks.is_empty() {
                                        ui.label(RichText::new("No bookmarks").small().weak());
                                    }

                                    let mut action = None;
                                    for (index, bookmark) in bookmarks.into_iter().enumerate() {
                                        ui.horizontal(|ui| {
                                            if ui
                                                .button(RichText::new(&bookmark.label).monospace())
                                                .on_hover_text("Open bookmark")
                                                .clicked()
                                            {
                                                action =
                                                    Some(BookmarkAction::Open(bookmark.clone()));
                                            }

                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    if ui
                                                        .button("×")
                                                        .on_hover_text("Remove bookmark")
                                                        .clicked()
                                                    {
                                                        action =
                                                            Some(BookmarkAction::Remove(index));
                                                    }
                                                },
                                            );
                                        });
                                    }

                                    if let Some(action) = action {
                                        match action {
                                            BookmarkAction::Open(bookmark) => {
                                                self.open_bookmark(bookmark)
                                            }
                                            BookmarkAction::Remove(index) => {
                                                self.remove_bookmark(index)
                                            }
                                        }
                                    }
                                });
                        }
                        SidebarView::Search => {
                            self.workspace_search_ui(ui);
                        }
                        SidebarView::Git => {
                            self.git_ui(ui);
                        }
                    }
                });
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).fill(ui.visuals().panel_fill))
            .show_inside(ui, |ui| {
                let editor_can_take_focus = !self.editing_session_name;
                let left_navigation_history = self.left_navigation_history.back.clone();
                let left_bookmark_lines = self.bookmark_lines_for_path(self.left.path.as_deref());
                let right_bookmark_lines = self.bookmark_lines_for_path(self.right.path.as_deref());
                let dragging_workspace_file = self.dragging_workspace_file.clone();
                let mut right_drop = None;

                if self.show_right_pane || dragging_workspace_file.is_some() {
                    ui.columns(2, |columns| {
                        let right_navigation_history = self.right_navigation_history.back.clone();
                        self.left.ui(
                            &mut columns[0],
                            editor_can_take_focus && self.focused_pane == Pane::Left,
                            &self.language_registry,
                            self.tab_width,
                            &left_navigation_history,
                            &left_bookmark_lines,
                        );
                        if editor_can_take_focus && ui_was_primary_clicked(&columns[0]) {
                            self.workspace_sidebar_active = false;
                            self.focused_pane = Pane::Left;
                        }
                        if let Some(action) = self.left.take_pending_action() {
                            self.focused_pane = Pane::Left;
                            self.handle_editor_action(action);
                        }

                        if self.show_right_pane {
                            self.right.ui(
                                &mut columns[1],
                                editor_can_take_focus && self.focused_pane == Pane::Right,
                                &self.language_registry,
                                self.tab_width,
                                &right_navigation_history,
                                &right_bookmark_lines,
                            );
                            if editor_can_take_focus && ui_was_primary_clicked(&columns[1]) {
                                self.workspace_sidebar_active = false;
                                self.focused_pane = Pane::Right;
                            }
                            if let Some(action) = self.right.take_pending_action() {
                                self.focused_pane = Pane::Right;
                                self.handle_editor_action(action);
                            }
                            if let Some(path) = dragging_workspace_file.clone()
                                && ui_rect_contains_pointer(&columns[1])
                                && columns[1].input(|input| input.pointer.any_released())
                            {
                                right_drop = Some(path);
                            }
                        } else if let Some(path) = dragging_workspace_file.clone()
                            && workspace_right_pane_drop_target(&mut columns[1], &path)
                        {
                            right_drop = Some(path);
                        }
                    });
                } else {
                    self.left.ui(
                        ui,
                        editor_can_take_focus && self.focused_pane == Pane::Left,
                        &self.language_registry,
                        self.tab_width,
                        &left_navigation_history,
                        &left_bookmark_lines,
                    );
                    if editor_can_take_focus && ui_was_primary_clicked(ui) {
                        self.workspace_sidebar_active = false;
                        self.focused_pane = Pane::Left;
                    }
                    if let Some(action) = self.left.take_pending_action() {
                        self.focused_pane = Pane::Left;
                        self.handle_editor_action(action);
                    }
                }

                if let Some(path) = right_drop {
                    self.show_right_pane = true;
                    self.focused_pane = Pane::Right;
                    self.dragging_workspace_file = None;
                    self.open_file_in_pane(Pane::Right, path);
                } else if self.dragging_workspace_file.is_some()
                    && ui.input(|input| input.pointer.any_released())
                {
                    self.dragging_workspace_file = None;
                }
            });

        if let Some(result) = self.show_search_overlay(ui) {
            self.open_search_result(result);
        }
        if let Some(location) = self.show_references_overlay(ui) {
            self.open_definition_location(location);
        }
        self.show_ai_command_window(ui);
        self.show_recent_workspaces_window(ui);
        self.show_workspace_rename_window(ui);
        self.show_workspace_file_dialog(ui);
        self.show_git_switch_branch_window(ui);
        self.show_git_create_branch_window(ui);
        self.show_git_discard_window(ui);
        self.show_git_commit_details_window(ui);
        self.show_shortcuts_help_window(ui);
        register_focused_test_node(ui.ctx());

        let scroll_after_y = self.editor(focused_pane_at_start).scroll_y;
        self.record_app_perf_sample(
            elapsed_ms(app_frame_start),
            stable_dt_ms,
            unstable_dt_ms,
            focused_pane_at_start,
            scroll_before_y,
            scroll_after_y,
            raw_wheel_delta,
            smooth_scroll_delta,
            page_key,
        );
    }
}

#[derive(Debug, Clone, Default)]
struct SearchOverlay {
    open: bool,
    query: String,
    selected_index: usize,
    cache: Option<SearchOverlayCache>,
}

#[derive(Debug, Clone, Default)]
struct ReferencesOverlay {
    open: bool,
    query: String,
    selected_index: usize,
    symbol: String,
}

#[derive(Debug, Clone)]
struct ReferenceOverlayEntry {
    path: PathBuf,
    display_path: String,
    line_number: u32,
    preview: String,
    location: Location,
}

#[derive(Debug, Clone)]
struct SearchOverlayCache {
    query: String,
    revision: u64,
    results: Vec<SearchResult>,
}

#[derive(Debug, Clone, Default)]
struct WorkspaceSearchState {
    open: bool,
    focus_requested: bool,
    searching: bool,
    active_search_id: u64,
    query: String,
    searched_query: String,
    results: Vec<WorkspaceSearchFileResult>,
}

impl WorkspaceSearchState {
    fn match_count(&self) -> usize {
        self.results.iter().map(|result| result.matches.len()).sum()
    }
}

#[derive(Debug)]
struct WorkspaceSearchResult {
    search_id: u64,
    query: String,
    results: Vec<WorkspaceSearchFileResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceSearchFileResult {
    path: PathBuf,
    matches: Vec<WorkspaceSearchMatch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceSearchMatch {
    position: Position,
    preview: String,
}

type WorkspaceSnapshot = HashMap<PathBuf, WorkspaceFileFingerprint>;

#[derive(Debug)]
struct WorkspaceChangeScanResult {
    roots: Vec<PathBuf>,
    files: Vec<PathBuf>,
    snapshot: WorkspaceSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkspaceFileFingerprint {
    modified_unix_nanos: u128,
    len: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct WorkspaceSnapshotDiff {
    added: Vec<PathBuf>,
    modified: Vec<PathBuf>,
    removed: Vec<PathBuf>,
}

impl WorkspaceSnapshotDiff {
    fn len(&self) -> usize {
        self.added.len() + self.modified.len() + self.removed.len()
    }

    fn requires_full_symbol_reindex(&self) -> bool {
        self.added
            .iter()
            .chain(&self.modified)
            .chain(&self.removed)
            .any(|path| is_dependency_manifest_path(path))
    }
}

#[derive(Debug, Clone)]
struct GitRepositoryState {
    root: PathBuf,
    name: String,
    branch: String,
    upstream: Option<String>,
    tracking: Option<GitTrackingStatus>,
    branches: Vec<String>,
    changes: Vec<GitChange>,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitTrackingStatus {
    ahead: usize,
    behind: usize,
}

#[derive(Debug, Clone)]
struct GitHistoryView {
    repository_name: String,
    root: PathBuf,
    rows: Vec<GitGraphRow>,
    selected_commit_hash: Option<String>,
}

#[derive(Debug, Clone)]
struct GitCommitDetailsWindow {
    repository_name: String,
    root: PathBuf,
    commit: GitCommitNode,
    files: Vec<GitCommitFileChange>,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitCommitNode {
    hash: String,
    short_hash: String,
    parents: Vec<String>,
    subject: String,
    author: String,
    date: String,
    refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitGraphRow {
    graph: String,
    commit: Option<GitCommitNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitCommitFileChange {
    status: String,
    path: String,
    old_path: Option<String>,
}

impl GitCommitFileChange {
    fn display_status(&self) -> &str {
        if self.status.starts_with('R') {
            "R"
        } else if self.status.starts_with('C') {
            "C"
        } else {
            self.status.as_str()
        }
    }

    fn display_path(&self) -> String {
        match &self.old_path {
            Some(old_path) => format!("{old_path} -> {}", self.path),
            None => self.path.clone(),
        }
    }
}

#[derive(Debug, Clone)]
struct GitChange {
    status: String,
    index_status: char,
    worktree_status: char,
    path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkspaceGitStatus {
    Modified,
    Added,
    Untracked,
}

impl WorkspaceGitStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Untracked => "?",
        }
    }

    fn color(self) -> Color32 {
        match self {
            Self::Modified => Color32::from_rgb(196, 160, 72),
            Self::Added => Color32::from_rgb(72, 160, 92),
            Self::Untracked => Color32::from_rgb(86, 156, 214),
        }
    }
}

impl GitChange {
    fn is_staged(&self) -> bool {
        self.index_status != ' ' && self.index_status != '?'
    }

    fn has_worktree_change(&self) -> bool {
        self.worktree_status != ' ' || self.is_untracked()
    }

    fn is_untracked(&self) -> bool {
        self.index_status == '?' && self.worktree_status == '?'
    }

    fn display_status(&self) -> &str {
        if self.status.is_empty() {
            "?"
        } else {
            &self.status
        }
    }

    fn workspace_status(&self) -> Option<WorkspaceGitStatus> {
        if self.is_untracked() {
            Some(WorkspaceGitStatus::Untracked)
        } else if self.index_status == 'A' || self.worktree_status == 'A' {
            Some(WorkspaceGitStatus::Added)
        } else if self.index_status == 'M' || self.worktree_status == 'M' {
            Some(WorkspaceGitStatus::Modified)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
struct GitDiscardRequest {
    repository_root: PathBuf,
    repository_name: String,
    path: String,
    untracked: bool,
}

#[derive(Debug, Clone)]
struct ShortcutHint {
    keys: &'static str,
    label: &'static str,
}

#[derive(Debug, Clone)]
struct ShortcutGroup {
    title: &'static str,
    shortcuts: Vec<ShortcutHint>,
}

fn shortcuts(items: &[(&'static str, &'static str)]) -> Vec<ShortcutHint> {
    items
        .iter()
        .map(|(keys, label)| ShortcutHint { keys, label })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkdownMode {
    Preview,
    Markdown,
    Both,
}

impl MarkdownMode {
    fn all() -> [Self; 3] {
        [Self::Preview, Self::Markdown, Self::Both]
    }

    fn label(self) -> &'static str {
        match self {
            Self::Preview => "Preview",
            Self::Markdown => "Markdown",
            Self::Both => "Both",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CsvMode {
    Grid,
    Source,
}

impl CsvMode {
    fn all() -> [Self; 2] {
        [Self::Grid, Self::Source]
    }

    fn label(self) -> &'static str {
        match self {
            Self::Grid => "Grid",
            Self::Source => "Source",
        }
    }
}

#[derive(Debug, Clone, Default)]
struct MarkdownSearchState {
    open: bool,
    focus_requested: bool,
    query: String,
    current_match: usize,
    match_count: usize,
    scrolled_match: Option<usize>,
}

#[derive(Debug, Clone, Default)]
struct SourceSearchState {
    open: bool,
    focus_requested: bool,
    query: String,
    current_match: usize,
    match_count: usize,
    scrolled_match: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MarkdownSearchMatch {
    page_id: String,
    page_index: usize,
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceSearchMatch {
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MarkdownFootnoteScrollTarget {
    Reference(String),
    Definition(String),
}

#[derive(Debug, Clone)]
struct MarkdownDocumentCache {
    path: PathBuf,
    source_len: usize,
    source_hash: u64,
    document: Result<Rc<MarkdownDocument>, String>,
}

#[derive(Debug, Clone)]
struct SyntaxHighlightCache {
    path: Option<PathBuf>,
    source_len: usize,
    source_hash: u64,
    spans: Rc<Vec<HighlightSpan>>,
    line_span_ranges: Rc<Vec<std::ops::Range<usize>>>,
}

#[derive(Debug, Clone)]
struct SyntaxHighlightPending {
    path: Option<PathBuf>,
    source_len: usize,
    source_hash: u64,
}

#[derive(Debug, Clone)]
struct SyntaxHighlightResult {
    path: Option<PathBuf>,
    source_len: usize,
    source_hash: u64,
    spans: Vec<HighlightSpan>,
}

#[derive(Debug, Clone, Copy)]
struct EditorMetricsCache {
    source_len: usize,
    source_hash: u64,
    line_count: usize,
    longest_line_chars: usize,
}

impl EditorMetricsCache {
    fn editor_width(self, ui: &egui::Ui, available_width: f32) -> f32 {
        let char_width = editor_char_width(ui);
        (self.longest_line_chars as f32 * char_width + 64.0).max(available_width)
    }
}

#[derive(Debug, Clone, Copy)]
struct LineInfo {
    start: usize,
    leading_end: usize,
    content_end: usize,
    end: usize,
    char_count: usize,
    is_blank: bool,
}

#[derive(Debug, Clone)]
struct LineIndex {
    source_len: usize,
    source_hash: u64,
    lines: Vec<LineInfo>,
    longest_line_chars: usize,
}

impl LineIndex {
    fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn line(&self, index: usize) -> Option<LineInfo> {
        self.lines.get(index).copied()
    }

    fn editor_width(&self, ui: &egui::Ui, available_width: f32) -> f32 {
        let char_width = editor_char_width(ui);
        (self.longest_line_chars as f32 * char_width + 64.0).max(available_width)
    }
}

struct EditorTextBuffer {
    text: String,
    chunks: Vec<TextChunk>,
    text_hash: u64,
    line_index: Rc<LineIndex>,
}

struct TextChunk {
    start: usize,
    char_start: usize,
    text: String,
}

impl TextChunk {
    fn end(&self) -> usize {
        self.start + self.text.len()
    }

    fn char_end(&self) -> usize {
        self.char_start + self.text.chars().count()
    }
}

impl EditorTextBuffer {
    fn new(text: String) -> Self {
        let text_hash = stable_hash(&text);
        let chunks = build_text_chunks(&text);
        let line_index = Rc::new(build_line_index_from_chunks(&chunks, text.len(), text_hash));
        Self {
            text,
            chunks,
            text_hash,
            line_index,
        }
    }

    fn as_str(&self) -> &str {
        &self.text
    }

    fn len(&self) -> usize {
        self.text.len()
    }

    fn fingerprint(&self) -> (usize, u64) {
        (self.text.len(), self.text_hash)
    }

    fn line_index(&self) -> Rc<LineIndex> {
        self.line_index.clone()
    }

    fn line_text(&self, line: LineInfo) -> Option<&str> {
        let chunk = self.chunk_for_range(line.start, line.content_end)?;
        let start = line.start - chunk.start;
        let end = line.content_end - chunk.start;
        chunk.text.get(start..end)
    }

    fn line_text_or_empty(&self, line: LineInfo) -> &str {
        self.line_text(line).unwrap_or_else(|| {
            self.text
                .get(line.start..line.content_end)
                .unwrap_or_default()
        })
    }

    fn char_index_to_byte_index(&self, char_index: usize) -> Option<usize> {
        if char_index == self.char_len() {
            return Some(self.text.len());
        }

        let chunk = self.chunk_for_char_index(char_index)?;
        let local_char_index = char_index - chunk.char_start;
        chunk
            .text
            .char_indices()
            .nth(local_char_index)
            .map(|(index, _)| chunk.start + index)
    }

    fn char_index_to_position(&self, char_index: usize) -> Option<Position> {
        let byte_index = self.char_index_to_byte_index(char_index)?;
        self.byte_index_to_position(byte_index)
    }

    fn byte_index_to_position(&self, byte_index: usize) -> Option<Position> {
        let byte_index = clamp_to_char_boundary(self.as_str(), byte_index);
        let line_index = self
            .line_index
            .lines
            .partition_point(|line| line.end <= byte_index)
            .min(self.line_index.lines.len().saturating_sub(1));
        let line = self.line_index.line(line_index)?;
        let character = byte_index.saturating_sub(line.start) as u32;
        Some(Position::new(line_index as u32, character))
    }

    fn position_to_char_index(&self, position: Position) -> Option<usize> {
        let line = self.line_index.line(position.line as usize)?;
        let target_byte = (line.start + position.character as usize).min(line.content_end);
        let target_byte = clamp_to_char_boundary(self.as_str(), target_byte);
        let chunk = self.chunk_for_byte_index(target_byte)?;
        let local_byte = target_byte - chunk.start;
        Some(chunk.char_start + chunk.text[..local_byte].chars().count())
    }

    fn next_char_at(&self, char_index: usize) -> Option<char> {
        let byte_index = self.char_index_to_byte_index(char_index)?;
        self.as_str()[byte_index..].chars().next()
    }

    fn replace(&mut self, text: String) {
        self.text = text;
        self.refresh_metadata();
    }

    fn insert_str(&mut self, byte_index: usize, text: &str) {
        self.text.insert_str(byte_index, text);
        self.refresh_metadata();
    }

    fn replace_byte_range(&mut self, range: std::ops::Range<usize>, replacement: &str) {
        self.text.replace_range(range, replacement);
        self.refresh_metadata();
    }

    fn refresh_metadata(&mut self) {
        self.text_hash = stable_hash(&self.text);
        self.chunks = build_text_chunks(&self.text);
        self.line_index = Rc::new(build_line_index_from_chunks(
            &self.chunks,
            self.text.len(),
            self.text_hash,
        ));
    }

    fn chunk_for_range(&self, start: usize, end: usize) -> Option<&TextChunk> {
        let index = self
            .chunks
            .partition_point(|chunk| chunk.end() <= start)
            .min(self.chunks.len());
        let chunk = self.chunks.get(index)?;
        (start >= chunk.start && end <= chunk.end()).then_some(chunk)
    }

    fn chunk_for_byte_index(&self, byte_index: usize) -> Option<&TextChunk> {
        if byte_index == self.text.len() {
            return self.chunks.last();
        }

        let index = self
            .chunks
            .partition_point(|chunk| chunk.end() <= byte_index)
            .min(self.chunks.len());
        self.chunks.get(index)
    }

    fn chunk_for_char_index(&self, char_index: usize) -> Option<&TextChunk> {
        let index = self
            .chunks
            .partition_point(|chunk| chunk.char_end() <= char_index)
            .min(self.chunks.len());
        self.chunks.get(index)
    }

    fn char_len(&self) -> usize {
        self.chunks.last().map_or(0, TextChunk::char_end)
    }
}

impl std::ops::Deref for EditorTextBuffer {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl TextBuffer for EditorTextBuffer {
    fn is_mutable(&self) -> bool {
        true
    }

    fn as_str(&self) -> &str {
        self.as_str()
    }

    fn insert_text(&mut self, text: &str, char_index: usize) -> usize {
        let byte_index = self
            .char_index_to_byte_index(char_index)
            .unwrap_or_else(|| self.text.len());
        self.text.insert_str(byte_index, text);
        self.refresh_metadata();
        text.chars().count()
    }

    fn delete_char_range(&mut self, char_range: std::ops::Range<usize>) {
        if char_range.start > char_range.end {
            return;
        }

        let byte_start = self
            .char_index_to_byte_index(char_range.start)
            .unwrap_or(self.text.len());
        let byte_end = self
            .char_index_to_byte_index(char_range.end)
            .unwrap_or(self.text.len());
        if byte_start <= byte_end {
            self.text.drain(byte_start..byte_end);
            self.refresh_metadata();
        }
    }

    fn clear(&mut self) {
        self.text.clear();
        self.refresh_metadata();
    }

    fn replace_with(&mut self, text: &str) {
        self.replace(text.to_owned());
    }

    fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        self.refresh_metadata();
        text
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }
}

struct CachedVirtualLineLayout {
    line_start: usize,
    line_content_end: usize,
    galley: Arc<egui::Galley>,
    last_used_frame: u64,
}

struct VirtualLineLayoutCache {
    source_len: usize,
    source_hash: u64,
    dark_mode: bool,
    entries: HashMap<usize, CachedVirtualLineLayout>,
}

struct CachedVirtualLineNumberLayout {
    galley: Arc<egui::Galley>,
    last_used_frame: u64,
}

#[derive(Debug, Clone, Copy)]
struct VirtualScrollbarLayout {
    content_rect: egui::Rect,
    vertical_rect: Option<egui::Rect>,
    horizontal_rect: Option<egui::Rect>,
}

struct VirtualLineNumberLayoutCache {
    line_count: usize,
    dark_mode: bool,
    inactive_color: Color32,
    active_color: Color32,
    entries: HashMap<(usize, bool), CachedVirtualLineNumberLayout>,
}

#[derive(Debug, Clone, Copy)]
struct BracketPairCache {
    source_len: usize,
    source_hash: u64,
    cursor_position: Option<Position>,
    pair: Option<(usize, usize)>,
}

#[derive(Debug, Clone)]
struct SourceSearchCache {
    source_len: usize,
    source_hash: u64,
    query: String,
    matches: Rc<Vec<SourceSearchMatch>>,
}

#[derive(Debug, Clone)]
struct DiffDisplayCache {
    source_len: usize,
    source_hash: u64,
    rows: Rc<Vec<DiffDisplayRow>>,
}

#[derive(Debug, Clone, Default)]
struct EditorPerfDebug {
    view: &'static str,
    total_ms: f32,
    header_ms: f32,
    body_ms: f32,
    visible_rows: usize,
    total_rows: usize,
    galley_hits: usize,
    galley_misses: usize,
    line_number_hits: usize,
    line_number_misses: usize,
    text_len: usize,
    scroll_y: f32,
    scroll_x: f32,
    scroll_delta_y: f32,
    source_search_open: bool,
    source_search_matches: usize,
    syntax_spans: usize,
    virtual_cache_entries: usize,
    line_number_cache_entries: usize,
}

#[derive(Debug, Clone, Default)]
struct VirtualClickDebug {
    next_event_index: u64,
    events: Vec<VirtualClickEventDebug>,
    last_accept: Option<VirtualClickAcceptDebug>,
}

#[derive(Debug, Clone)]
struct VirtualClickEventDebug {
    index: u64,
    source: &'static str,
    accepted: bool,
    reason: &'static str,
    pointer_pos: Option<egui::Pos2>,
    response_rect: egui::Rect,
    viewport_rect: egui::Rect,
    text_clip_rect: egui::Rect,
    vertical_scrollbar_rect: Option<egui::Rect>,
    horizontal_scrollbar_rect: Option<egui::Rect>,
    response_hovered: bool,
    response_contains_pointer: bool,
    response_clicked: bool,
    response_primary_clicked: bool,
    primary_pressed: bool,
    primary_released: bool,
    primary_clicked: bool,
    any_pressed: bool,
    any_released: bool,
    any_click: bool,
    dragging: bool,
    response_hit: bool,
    viewport_hit: bool,
    text_hit: bool,
    scrollbar_hit: bool,
    pane_focused: bool,
    text_edit_focused: bool,
    scroll_x: f32,
    scroll_y: f32,
    row_height: f32,
    line_count: usize,
    visible_range: std::ops::Range<usize>,
}

#[derive(Debug, Clone)]
struct VirtualClickAcceptDebug {
    event_index: u64,
    pointer_pos: egui::Pos2,
    line: usize,
    character: u32,
    cursor_position: Position,
    text_left: f32,
    text_clip_left: f32,
    x_offset: f32,
}

impl VirtualClickDebug {
    fn push_event(&mut self, mut event: VirtualClickEventDebug) -> u64 {
        self.next_event_index = self.next_event_index.wrapping_add(1);
        event.index = self.next_event_index;
        self.events.push(event);
        if self.events.len() > 8 {
            let remove_count = self.events.len().saturating_sub(8);
            self.events.drain(0..remove_count);
        }
        self.next_event_index
    }

    fn report_lines(&self, pane: Pane) -> Vec<String> {
        let pane_label = pane.label();
        let mut lines = Vec::new();
        if let Some(accept) = &self.last_accept {
            lines.push(accept.report_line(pane_label));
        } else {
            lines.push(format!("{pane_label} virtual_click_accept: none"));
        }
        if self.events.is_empty() {
            lines.push(format!("{pane_label} virtual_click_events: none"));
        } else {
            for event in &self.events {
                lines.push(event.report_line(pane_label));
            }
        }
        lines
    }
}

impl VirtualClickEventDebug {
    fn report_line(&self, pane_label: &str) -> String {
        format!(
            "{pane_label} virtual_click_event#{}: source={} accepted={} reason={} pointer={} hits(response/view/text/scrollbar)={}/{}/{}/{} input(primary p/r/c={}/{}/{}, any p/r/c={}/{}/{}, dragging={}) response(hovered={} contains={} clicked={} primary_clicked={}) focus(pane={} text_edit={}) scroll=({:.1},{:.1}) row_h={:.2} lines={} visible={:?} rects(response={} viewport={} text_clip={} scrollbar={})",
            self.index,
            self.source,
            self.accepted,
            self.reason,
            format_virtual_pos(self.pointer_pos),
            self.response_hit,
            self.viewport_hit,
            self.text_hit,
            self.scrollbar_hit,
            self.primary_pressed,
            self.primary_released,
            self.primary_clicked,
            self.any_pressed,
            self.any_released,
            self.any_click,
            self.dragging,
            self.response_hovered,
            self.response_contains_pointer,
            self.response_clicked,
            self.response_primary_clicked,
            self.pane_focused,
            self.text_edit_focused,
            self.scroll_x,
            self.scroll_y,
            self.row_height,
            self.line_count,
            self.visible_range,
            format_virtual_rect(self.response_rect),
            format_virtual_rect(self.viewport_rect),
            format_virtual_rect(self.text_clip_rect),
            format_virtual_scrollbar_rects(
                self.vertical_scrollbar_rect,
                self.horizontal_scrollbar_rect
            ),
        )
    }
}

impl VirtualClickAcceptDebug {
    fn report_line(&self, pane_label: &str) -> String {
        format!(
            "{pane_label} virtual_click_accept: event#{} line={} char={} cursor={}:{} pointer={} text_left={:.1} text_clip_left={:.1} x_offset={:.1}",
            self.event_index,
            self.line + 1,
            self.character + 1,
            self.cursor_position.line + 1,
            self.cursor_position.character + 1,
            format_virtual_pos(Some(self.pointer_pos)),
            self.text_left,
            self.text_clip_left,
            self.x_offset,
        )
    }
}

fn format_virtual_pos(position: Option<egui::Pos2>) -> String {
    position
        .map(|position| format!("({:.1},{:.1})", position.x, position.y))
        .unwrap_or_else(|| "None".to_owned())
}

fn format_virtual_rect(rect: egui::Rect) -> String {
    format!(
        "({:.1},{:.1})-({:.1},{:.1})",
        rect.left(),
        rect.top(),
        rect.right(),
        rect.bottom()
    )
}

fn format_virtual_optional_rect(rect: Option<egui::Rect>) -> String {
    rect.map(format_virtual_rect)
        .unwrap_or_else(|| "None".to_owned())
}

fn format_virtual_scrollbar_rects(
    vertical_rect: Option<egui::Rect>,
    horizontal_rect: Option<egui::Rect>,
) -> String {
    format!(
        "v:{} h:{}",
        format_virtual_optional_rect(vertical_rect),
        format_virtual_optional_rect(horizontal_rect)
    )
}

#[derive(Debug, Clone, Default)]
struct VirtualFrameCounters {
    galley_hits: usize,
    galley_misses: usize,
    line_number_hits: usize,
    line_number_misses: usize,
}

#[derive(Debug, Clone)]
struct AppPerfDebug {
    last_total_ms: f32,
    last_stable_dt_ms: f32,
    last_unstable_dt_ms: f32,
    avg_total_ms: f32,
    max_total_ms: f32,
    frame_samples: Vec<f32>,
    frame_index: u64,
    focused_pane: Pane,
    scroll_before_y: f32,
    scroll_after_y: f32,
    scroll_delta_y: f32,
    raw_wheel_delta: egui::Vec2,
    smooth_scroll_delta: egui::Vec2,
    page_key: Option<&'static str>,
}

impl Default for AppPerfDebug {
    fn default() -> Self {
        Self {
            last_total_ms: 0.0,
            last_stable_dt_ms: 0.0,
            last_unstable_dt_ms: 0.0,
            avg_total_ms: 0.0,
            max_total_ms: 0.0,
            frame_samples: Vec::new(),
            frame_index: 0,
            focused_pane: Pane::Left,
            scroll_before_y: 0.0,
            scroll_after_y: 0.0,
            scroll_delta_y: 0.0,
            raw_wheel_delta: egui::Vec2::ZERO,
            smooth_scroll_delta: egui::Vec2::ZERO,
            page_key: None,
        }
    }
}

impl EditorPerfDebug {
    fn report_line(&self, pane: Pane, title: &str) -> String {
        let pane_label = pane.label();
        format!(
            "{}: title={:?} view={} total_ms={:.2} header_ms={:.2} body_ms={:.2} rows={}/{} text_len={} scroll=({:.1},{:.1}) scroll_delta_y={:.1} galley_hit/miss={}/{} line_no_hit/miss={}/{} search_open={} search_matches={} syntax_spans={} cache_entries={}/{}",
            pane_label,
            title,
            self.view,
            self.total_ms,
            self.header_ms,
            self.body_ms,
            self.visible_rows,
            self.total_rows,
            self.text_len,
            self.scroll_x,
            self.scroll_y,
            self.scroll_delta_y,
            self.galley_hits,
            self.galley_misses,
            self.line_number_hits,
            self.line_number_misses,
            self.source_search_open,
            self.source_search_matches,
            self.syntax_spans,
            self.virtual_cache_entries,
            self.line_number_cache_entries
        )
    }
}

#[derive(Debug, Clone)]
struct GitDiffView {
    title: String,
    repository_name: String,
    repository_root: Option<PathBuf>,
    file_path: Option<String>,
    scope: GitDiffScope,
    context_mode: GitDiffContextMode,
    untracked: bool,
    return_history: Option<GitHistoryReturnState>,
}

#[derive(Debug, Clone)]
struct GitHistoryReturnState {
    view: GitHistoryView,
    scroll_y: f32,
    scroll_x: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitDiffScope {
    All,
    Worktree,
    Staged,
    Commit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitDiffContextMode {
    Changes,
    FullFile,
}

fn diff_view_can_switch_context(diff_view: &GitDiffView) -> bool {
    diff_view.file_path.is_some()
        && diff_view.repository_root.is_some()
        && !diff_view.untracked
        && matches!(
            diff_view.scope,
            GitDiffScope::Worktree | GitDiffScope::Staged
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffDisplayMode {
    Unified,
    SideBySide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum DiffSide {
    Old,
    New,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiffSideSelectionPoint {
    row: usize,
    column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiffSideSelection {
    side: DiffSide,
    anchor: DiffSideSelectionPoint,
    head: DiffSideSelectionPoint,
    dragging: bool,
}

impl DiffSideSelection {
    fn normalized(self) -> Option<(DiffSideSelectionPoint, DiffSideSelectionPoint)> {
        if self.anchor == self.head {
            return None;
        }
        if (self.anchor.row, self.anchor.column) <= (self.head.row, self.head.column) {
            Some((self.anchor, self.head))
        } else {
            Some((self.head, self.anchor))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiffUnifiedSelection {
    anchor: DiffSideSelectionPoint,
    head: DiffSideSelectionPoint,
    dragging: bool,
}

impl DiffUnifiedSelection {
    fn normalized(self) -> Option<(DiffSideSelectionPoint, DiffSideSelectionPoint)> {
        if self.anchor == self.head {
            return None;
        }
        if (self.anchor.row, self.anchor.column) <= (self.head.row, self.head.column) {
            Some((self.anchor, self.head))
        } else {
            Some((self.head, self.anchor))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingEditorNavigation {
    position: Position,
    focus_editor: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VirtualTextSelection {
    anchor: Position,
    head: Position,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NormalizedVirtualSelection {
    start: Position,
    end: Position,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum VirtualEditCommand {
    Insert(String),
    Backspace,
    Delete,
    Undo,
    Redo,
}

const VIRTUAL_EDIT_HISTORY_LIMIT: usize = 512;
const CSV_EDIT_HISTORY_LIMIT: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
struct VirtualEditHistoryEntry {
    start: usize,
    removed: String,
    inserted: String,
    before_cursor: Option<Position>,
    after_cursor: Option<Position>,
    before_selection: Option<VirtualTextSelection>,
    after_selection: Option<VirtualTextSelection>,
}

#[derive(Debug, Clone, PartialEq)]
struct CsvEditHistoryEntry {
    text: String,
    csv_selection: Option<CsvSelection>,
    selected_csv_cell: Option<CsvCellPosition>,
    scroll_y: f32,
    scroll_x: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VirtualNavigationTarget {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    SelectAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VirtualNavigationCommand {
    target: VirtualNavigationTarget,
    extend_selection: bool,
}

impl VirtualTextSelection {
    fn normalized(self) -> Option<NormalizedVirtualSelection> {
        if self.anchor == self.head {
            return None;
        }
        let (start, end) = if position_leq(self.anchor, self.head) {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        };
        Some(NormalizedVirtualSelection { start, end })
    }
}

struct EditorPane {
    pane: Pane,
    title: String,
    path: Option<PathBuf>,
    text: EditorTextBuffer,
    read_only: bool,
    cursor_position: Option<Position>,
    target_position: Option<Position>,
    pending_navigation: Option<PendingEditorNavigation>,
    virtual_selection: Option<VirtualTextSelection>,
    virtual_ime_preedit: Option<String>,
    virtual_undo_stack: Vec<VirtualEditHistoryEntry>,
    virtual_redo_stack: Vec<VirtualEditHistoryEntry>,
    /// Wheel scrolling the large-file editor applied immediately but egui is still going to
    /// deliver as smoothed deltas on later frames; those are skipped up to this amount.
    virtual_wheel_lead: egui::Vec2,
    scroll_y: f32,
    scroll_max_y: f32,
    scroll_x: f32,
    diff_side_left_scroll_x: f32,
    diff_side_right_scroll_x: f32,
    selected_csv_cell: Option<CsvCellPosition>,
    csv_selection: Option<CsvSelection>,
    csv_drag: Option<CsvSelectionDrag>,
    csv_debug_message: Option<String>,
    csv_mode: CsvMode,
    csv_undo_stack: Vec<CsvEditHistoryEntry>,
    csv_redo_stack: Vec<CsvEditHistoryEntry>,
    csv_edit_history_cell: Option<CsvCellPosition>,
    markdown_mode: MarkdownMode,
    markdown_active_page_id: Option<String>,
    markdown_active_heading_id: Option<String>,
    markdown_focus_target: MarkdownFocusTarget,
    markdown_show_pages: bool,
    markdown_pages_width: f32,
    markdown_pages_resize_start_width: Option<f32>,
    markdown_show_outline: bool,
    markdown_search: MarkdownSearchState,
    source_search: SourceSearchState,
    markdown_document_cache: Option<MarkdownDocumentCache>,
    syntax_highlight_cache: Option<SyntaxHighlightCache>,
    syntax_highlight_pending: Option<SyntaxHighlightPending>,
    syntax_highlight_receiver: Option<Receiver<SyntaxHighlightResult>>,
    editor_metrics_cache: Option<EditorMetricsCache>,
    virtual_line_layout_cache: Option<VirtualLineLayoutCache>,
    virtual_line_number_layout_cache: Option<VirtualLineNumberLayoutCache>,
    editor_render_debug: Option<EditorRenderDebug>,
    editor_perf_debug: Option<EditorPerfDebug>,
    virtual_click_debug: VirtualClickDebug,
    bracket_pair_cache: Option<BracketPairCache>,
    source_search_cache: Option<SourceSearchCache>,
    markdown_image_cache: HashMap<PathBuf, Result<egui::TextureHandle, String>>,
    markdown_mermaid_cache: HashMap<String, Result<String, String>>,
    markdown_pending_footnote_scroll: Option<MarkdownFootnoteScrollTarget>,
    diff_view: Option<GitDiffView>,
    git_history_view: Option<GitHistoryView>,
    diff_display_mode: DiffDisplayMode,
    diff_collapsed_files: HashMap<String, bool>,
    diff_side_selection: Option<DiffSideSelection>,
    diff_unified_selection: Option<DiffUnifiedSelection>,
    diff_display_cache: Option<DiffDisplayCache>,
    pending_action: Option<EditorAction>,
}

impl EditorPane {
    fn empty(pane: Pane) -> Self {
        let text = "Select a file from the workspace sidebar.".to_owned();
        Self {
            pane,
            title: "No file open".to_owned(),
            path: None,
            text: EditorTextBuffer::new(text),
            read_only: true,
            cursor_position: None,
            target_position: None,
            pending_navigation: None,
            virtual_selection: None,
            virtual_ime_preedit: None,
            virtual_undo_stack: Vec::new(),
            virtual_redo_stack: Vec::new(),
            virtual_wheel_lead: egui::Vec2::ZERO,
            scroll_y: 0.0,
            scroll_max_y: 0.0,
            scroll_x: 0.0,
            diff_side_left_scroll_x: 0.0,
            diff_side_right_scroll_x: 0.0,
            selected_csv_cell: None,
            csv_selection: None,
            csv_drag: None,
            csv_debug_message: None,
            csv_mode: CsvMode::Grid,
            csv_undo_stack: Vec::new(),
            csv_redo_stack: Vec::new(),
            csv_edit_history_cell: None,
            markdown_mode: MarkdownMode::Preview,
            markdown_active_page_id: None,
            markdown_active_heading_id: None,
            markdown_focus_target: MarkdownFocusTarget::Body,
            markdown_show_pages: true,
            markdown_pages_width: MARKDOWN_SIDE_PANE_DEFAULT_WIDTH,
            markdown_pages_resize_start_width: None,
            markdown_show_outline: false,
            markdown_search: MarkdownSearchState::default(),
            source_search: SourceSearchState::default(),
            markdown_document_cache: None,
            syntax_highlight_cache: None,
            syntax_highlight_pending: None,
            syntax_highlight_receiver: None,
            editor_metrics_cache: None,
            virtual_line_layout_cache: None,
            virtual_line_number_layout_cache: None,
            editor_render_debug: None,
            editor_perf_debug: None,
            virtual_click_debug: VirtualClickDebug::default(),
            bracket_pair_cache: None,
            source_search_cache: None,
            markdown_image_cache: HashMap::new(),
            markdown_mermaid_cache: HashMap::new(),
            markdown_pending_footnote_scroll: None,
            diff_view: None,
            git_history_view: None,
            diff_display_mode: DiffDisplayMode::Unified,
            diff_collapsed_files: HashMap::new(),
            diff_side_selection: None,
            diff_unified_selection: None,
            diff_display_cache: None,
            pending_action: None,
        }
    }

    fn from_path(pane: Pane, path: PathBuf) -> Self {
        match fs::read_to_string(&path) {
            Ok(text) => Self {
                pane,
                title: path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("<invalid>")
                    .to_owned(),
                read_only: is_external_path(&path),
                path: Some(path),
                text: EditorTextBuffer::new(text),
                cursor_position: None,
                target_position: None,
                pending_navigation: None,
                virtual_selection: None,
                virtual_ime_preedit: None,
                virtual_undo_stack: Vec::new(),
                virtual_redo_stack: Vec::new(),
                virtual_wheel_lead: egui::Vec2::ZERO,
                scroll_y: 0.0,
                scroll_max_y: 0.0,
                scroll_x: 0.0,
                diff_side_left_scroll_x: 0.0,
                diff_side_right_scroll_x: 0.0,
                selected_csv_cell: None,
                csv_selection: None,
                csv_drag: None,
                csv_debug_message: None,
                csv_mode: CsvMode::Grid,
                csv_undo_stack: Vec::new(),
                csv_redo_stack: Vec::new(),
                csv_edit_history_cell: None,
                markdown_mode: MarkdownMode::Preview,
                markdown_active_page_id: None,
                markdown_active_heading_id: None,
                markdown_focus_target: MarkdownFocusTarget::Body,
                markdown_show_pages: true,
                markdown_pages_width: MARKDOWN_SIDE_PANE_DEFAULT_WIDTH,
                markdown_pages_resize_start_width: None,
                markdown_show_outline: false,
                markdown_search: MarkdownSearchState::default(),
                source_search: SourceSearchState::default(),
                markdown_document_cache: None,
                syntax_highlight_cache: None,
                syntax_highlight_pending: None,
                syntax_highlight_receiver: None,
                editor_metrics_cache: None,
                virtual_line_layout_cache: None,
                virtual_line_number_layout_cache: None,
                editor_render_debug: None,
                editor_perf_debug: None,
                virtual_click_debug: VirtualClickDebug::default(),
                bracket_pair_cache: None,
                source_search_cache: None,
                markdown_image_cache: HashMap::new(),
                markdown_mermaid_cache: HashMap::new(),
                markdown_pending_footnote_scroll: None,
                diff_view: None,
                git_history_view: None,
                diff_display_mode: DiffDisplayMode::Unified,
                diff_collapsed_files: HashMap::new(),
                diff_side_selection: None,
                diff_unified_selection: None,
                diff_display_cache: None,
                pending_action: None,
            },
            Err(error) => {
                let text = format!("Failed to read file: {error}");
                Self {
                    pane,
                    title: "Unable to open file".to_owned(),
                    path: Some(path),
                    text: EditorTextBuffer::new(text),
                    read_only: true,
                    cursor_position: None,
                    target_position: None,
                    pending_navigation: None,
                    virtual_selection: None,
                    virtual_ime_preedit: None,
                    virtual_undo_stack: Vec::new(),
                    virtual_redo_stack: Vec::new(),
                    virtual_wheel_lead: egui::Vec2::ZERO,
                    scroll_y: 0.0,
                    scroll_max_y: 0.0,
                    scroll_x: 0.0,
                    diff_side_left_scroll_x: 0.0,
                    diff_side_right_scroll_x: 0.0,
                    selected_csv_cell: None,
                    csv_selection: None,
                    csv_drag: None,
                    csv_debug_message: None,
                    csv_mode: CsvMode::Grid,
                    csv_undo_stack: Vec::new(),
                    csv_redo_stack: Vec::new(),
                    csv_edit_history_cell: None,
                    markdown_mode: MarkdownMode::Preview,
                    markdown_active_page_id: None,
                    markdown_active_heading_id: None,
                    markdown_focus_target: MarkdownFocusTarget::Body,
                    markdown_show_pages: true,
                    markdown_pages_width: MARKDOWN_SIDE_PANE_DEFAULT_WIDTH,
                    markdown_pages_resize_start_width: None,
                    markdown_show_outline: false,
                    markdown_search: MarkdownSearchState::default(),
                    source_search: SourceSearchState::default(),
                    markdown_document_cache: None,
                    syntax_highlight_cache: None,
                    syntax_highlight_pending: None,
                    syntax_highlight_receiver: None,
                    editor_metrics_cache: None,
                    virtual_line_layout_cache: None,
                    virtual_line_number_layout_cache: None,
                    editor_render_debug: None,
                    editor_perf_debug: None,
                    virtual_click_debug: VirtualClickDebug::default(),
                    bracket_pair_cache: None,
                    source_search_cache: None,
                    markdown_image_cache: HashMap::new(),
                    markdown_mermaid_cache: HashMap::new(),
                    markdown_pending_footnote_scroll: None,
                    diff_view: None,
                    git_history_view: None,
                    diff_display_mode: DiffDisplayMode::Unified,
                    diff_collapsed_files: HashMap::new(),
                    diff_side_selection: None,
                    diff_unified_selection: None,
                    diff_display_cache: None,
                    pending_action: None,
                }
            }
        }
    }

    fn from_diff(pane: Pane, diff_view: GitDiffView, text: String) -> Self {
        Self {
            pane,
            title: diff_view.title.clone(),
            path: None,
            text: EditorTextBuffer::new(text),
            read_only: true,
            cursor_position: None,
            target_position: None,
            pending_navigation: None,
            virtual_selection: None,
            virtual_ime_preedit: None,
            virtual_undo_stack: Vec::new(),
            virtual_redo_stack: Vec::new(),
            virtual_wheel_lead: egui::Vec2::ZERO,
            scroll_y: 0.0,
            scroll_max_y: 0.0,
            scroll_x: 0.0,
            diff_side_left_scroll_x: 0.0,
            diff_side_right_scroll_x: 0.0,
            selected_csv_cell: None,
            csv_selection: None,
            csv_drag: None,
            csv_debug_message: None,
            csv_mode: CsvMode::Grid,
            csv_undo_stack: Vec::new(),
            csv_redo_stack: Vec::new(),
            csv_edit_history_cell: None,
            markdown_mode: MarkdownMode::Preview,
            markdown_active_page_id: None,
            markdown_active_heading_id: None,
            markdown_focus_target: MarkdownFocusTarget::Body,
            markdown_show_pages: true,
            markdown_pages_width: MARKDOWN_SIDE_PANE_DEFAULT_WIDTH,
            markdown_pages_resize_start_width: None,
            markdown_show_outline: false,
            markdown_search: MarkdownSearchState::default(),
            source_search: SourceSearchState::default(),
            markdown_document_cache: None,
            syntax_highlight_cache: None,
            syntax_highlight_pending: None,
            syntax_highlight_receiver: None,
            editor_metrics_cache: None,
            virtual_line_layout_cache: None,
            virtual_line_number_layout_cache: None,
            editor_render_debug: None,
            editor_perf_debug: None,
            virtual_click_debug: VirtualClickDebug::default(),
            bracket_pair_cache: None,
            source_search_cache: None,
            markdown_image_cache: HashMap::new(),
            markdown_mermaid_cache: HashMap::new(),
            markdown_pending_footnote_scroll: None,
            diff_view: Some(diff_view),
            git_history_view: None,
            diff_display_mode: DiffDisplayMode::Unified,
            diff_collapsed_files: HashMap::new(),
            diff_side_selection: None,
            diff_unified_selection: None,
            diff_display_cache: None,
            pending_action: None,
        }
    }

    fn from_git_history(pane: Pane, history_view: GitHistoryView) -> Self {
        let text = String::new();
        Self {
            pane,
            title: format!("{}: Git Graph", history_view.repository_name),
            path: None,
            text: EditorTextBuffer::new(text),
            read_only: true,
            cursor_position: None,
            target_position: None,
            pending_navigation: None,
            virtual_selection: None,
            virtual_ime_preedit: None,
            virtual_undo_stack: Vec::new(),
            virtual_redo_stack: Vec::new(),
            virtual_wheel_lead: egui::Vec2::ZERO,
            scroll_y: 0.0,
            scroll_max_y: 0.0,
            scroll_x: 0.0,
            diff_side_left_scroll_x: 0.0,
            diff_side_right_scroll_x: 0.0,
            selected_csv_cell: None,
            csv_selection: None,
            csv_drag: None,
            csv_debug_message: None,
            csv_mode: CsvMode::Grid,
            csv_undo_stack: Vec::new(),
            csv_redo_stack: Vec::new(),
            csv_edit_history_cell: None,
            markdown_mode: MarkdownMode::Preview,
            markdown_active_page_id: None,
            markdown_active_heading_id: None,
            markdown_focus_target: MarkdownFocusTarget::Body,
            markdown_show_pages: true,
            markdown_pages_width: MARKDOWN_SIDE_PANE_DEFAULT_WIDTH,
            markdown_pages_resize_start_width: None,
            markdown_show_outline: false,
            markdown_search: MarkdownSearchState::default(),
            source_search: SourceSearchState::default(),
            markdown_document_cache: None,
            syntax_highlight_cache: None,
            syntax_highlight_pending: None,
            syntax_highlight_receiver: None,
            editor_metrics_cache: None,
            virtual_line_layout_cache: None,
            virtual_line_number_layout_cache: None,
            editor_render_debug: None,
            editor_perf_debug: None,
            virtual_click_debug: VirtualClickDebug::default(),
            bracket_pair_cache: None,
            source_search_cache: None,
            markdown_image_cache: HashMap::new(),
            markdown_mermaid_cache: HashMap::new(),
            markdown_pending_footnote_scroll: None,
            diff_view: None,
            git_history_view: Some(history_view),
            diff_display_mode: DiffDisplayMode::Unified,
            diff_collapsed_files: HashMap::new(),
            diff_side_selection: None,
            diff_unified_selection: None,
            diff_display_cache: None,
            pending_action: None,
        }
    }

    fn navigation_point(&self) -> Option<NavigationPoint> {
        Some(NavigationPoint {
            path: self.path.clone()?,
            position: self.cursor_position.unwrap_or(Position::new(0, 0)),
        })
    }

    fn take_pending_action(&mut self) -> Option<EditorAction> {
        self.pending_action.take()
    }

    fn text_fingerprint(&self) -> (usize, u64) {
        self.text.fingerprint()
    }

    fn mark_text_changed(&mut self) {
        self.text.refresh_metadata();
        self.syntax_highlight_pending = None;
        self.syntax_highlight_receiver = None;
        self.virtual_line_layout_cache = None;
        self.virtual_line_number_layout_cache = None;
    }

    fn replace_text(&mut self, text: String) {
        self.text.replace(text);
        self.syntax_highlight_pending = None;
        self.syntax_highlight_receiver = None;
        self.virtual_line_layout_cache = None;
        self.virtual_line_number_layout_cache = None;
        self.virtual_undo_stack.clear();
        self.virtual_redo_stack.clear();
    }

    fn capture_csv_history_entry(&self) -> CsvEditHistoryEntry {
        CsvEditHistoryEntry {
            text: self.text.as_str().to_owned(),
            csv_selection: self.csv_selection,
            selected_csv_cell: self.selected_csv_cell,
            scroll_y: self.scroll_y,
            scroll_x: self.scroll_x,
        }
    }

    fn push_csv_undo_entry(&mut self) {
        let entry = self.capture_csv_history_entry();
        self.push_csv_undo_entry_value(entry);
    }

    fn push_csv_undo_entry_value(&mut self, entry: CsvEditHistoryEntry) {
        if self.csv_undo_stack.last() != Some(&entry) {
            self.csv_undo_stack.push(entry);
            limit_csv_edit_history(&mut self.csv_undo_stack);
        }
        self.csv_redo_stack.clear();
    }

    fn restore_csv_history_entry(&mut self, entry: CsvEditHistoryEntry) {
        self.text.replace(entry.text);
        self.text.refresh_metadata();
        self.syntax_highlight_pending = None;
        self.syntax_highlight_receiver = None;
        self.virtual_line_layout_cache = None;
        self.virtual_line_number_layout_cache = None;
        self.editor_metrics_cache = None;
        self.source_search_cache = None;
        self.csv_selection = entry.csv_selection;
        self.selected_csv_cell = entry.selected_csv_cell;
        self.csv_drag = None;
        self.csv_edit_history_cell = None;
        self.scroll_y = entry.scroll_y;
        self.scroll_x = entry.scroll_x;
        self.cursor_position = entry
            .selected_csv_cell
            .map(|cell| Position::new(cell.row as u32, cell.column as u32));
    }

    fn apply_csv_undo(&mut self) -> bool {
        let Some(entry) = self.csv_undo_stack.pop() else {
            return false;
        };
        let current = self.capture_csv_history_entry();
        self.restore_csv_history_entry(entry);
        self.csv_redo_stack.push(current);
        limit_csv_edit_history(&mut self.csv_redo_stack);
        self.csv_debug_message = Some("csv undo".to_owned());
        true
    }

    fn apply_csv_redo(&mut self) -> bool {
        let Some(entry) = self.csv_redo_stack.pop() else {
            return false;
        };
        let current = self.capture_csv_history_entry();
        self.restore_csv_history_entry(entry);
        self.csv_undo_stack.push(current);
        limit_csv_edit_history(&mut self.csv_undo_stack);
        self.csv_debug_message = Some("csv redo".to_owned());
        true
    }

    fn apply_immediate_wheel_scroll(&mut self, ui: &egui::Ui, max_scroll_y: f32) {
        let scroll_rect = ui.available_rect_before_wrap();
        let is_pointer_over_editor = ui.rect_contains_pointer(scroll_rect);
        let scroll_delta_y = ui.input(|input| input.smooth_scroll_delta().y);

        if is_pointer_over_editor && scroll_delta_y.abs() > f32::EPSILON {
            self.scroll_y = (self.scroll_y - scroll_delta_y).clamp(0.0, max_scroll_y.max(0.0));
            ui.input_mut(|input| {
                input.smooth_scroll_delta.y = 0.0;
            });
        }
    }

    fn save_to_disk(&mut self) -> Result<PathBuf, String> {
        if self.read_only {
            return Err("Focused file is read-only.".to_owned());
        }

        let Some(path) = self.path.clone() else {
            return Err("No file is open in the focused pane.".to_owned());
        };

        if self.is_csv() {
            let text = parse_csv(&self.text)
                .map(|document| {
                    serialize_csv_for_save(&document, CsvTextStyle::of(self.text.as_str()))
                })
                .unwrap_or_else(|_| self.text.as_str().to_owned());
            self.replace_text(text);
        } else {
            let text = strip_trailing_whitespace(&self.text);
            self.replace_text(text);
        }
        fs::write(&path, self.text.as_str())
            .map(|()| path)
            .map_err(|error| format!("Failed to save file: {error}"))
    }

    fn is_csv(&self) -> bool {
        self.path.as_deref().is_some_and(is_csv_path)
    }

    fn is_csv_grid(&self) -> bool {
        self.is_csv() && self.csv_mode == CsvMode::Grid
    }

    fn is_markdown(&self) -> bool {
        self.path.as_deref().is_some_and(is_markdown_path)
    }

    fn can_source_search(&self) -> bool {
        self.path.is_some()
            && !self.is_markdown()
            && (!self.is_csv() || self.csv_mode == CsvMode::Source)
    }

    fn can_markdown_source_search(&self) -> bool {
        self.is_markdown() && self.markdown_mode == MarkdownMode::Markdown
    }

    fn has_active_csv_selection(&self) -> bool {
        self.is_csv_grid() && self.csv_selection.is_some()
    }

    fn has_virtual_text_selection(&self) -> bool {
        self.virtual_selection
            .and_then(|selection| selection.normalized())
            .is_some()
    }

    fn has_diff_text_selection(&self) -> bool {
        match self.diff_display_mode {
            DiffDisplayMode::Unified => self
                .diff_unified_selection
                .and_then(|selection| selection.normalized())
                .is_some(),
            DiffDisplayMode::SideBySide => self
                .diff_side_selection
                .and_then(|selection| selection.normalized())
                .is_some(),
        }
    }

    fn virtual_selection_text(&self) -> Option<String> {
        let selection = self
            .virtual_selection
            .and_then(|selection| selection.normalized())?;
        virtual_selection_text(&self.text, selection)
    }

    fn diff_selection_text(&self) -> Option<String> {
        let rows = parse_diff_display_rows(&self.text);
        match self.diff_display_mode {
            DiffDisplayMode::Unified => {
                let selection = self.diff_unified_selection?;
                let render_rows = unified_diff_render_rows(&rows, &self.diff_collapsed_files);
                diff_unified_selection_text_from_render_rows(&render_rows, selection)
            }
            DiffDisplayMode::SideBySide => {
                let selection = self.diff_side_selection?;
                let render_rows = side_by_side_diff_render_rows(&rows, &self.diff_collapsed_files);
                diff_side_selection_text_from_render_rows(&render_rows, selection)
            }
        }
    }

    fn csv_selection_text(&self) -> Option<String> {
        if !self.has_active_csv_selection() {
            return None;
        }
        let selection = self.csv_selection?;
        let document = parse_csv(&self.text).ok()?;
        Some(copy_csv_selection_as_csv(&document, selection))
    }

    fn paste_csv_text(&mut self, text: &str) -> Result<usize, String> {
        if self.read_only {
            return Err("Focused file is read-only.".to_owned());
        }

        let selection = self
            .csv_selection
            .ok_or_else(|| "No CSV row or column selection.".to_owned())?;
        let mut document = parse_csv(&self.text)?;
        self.push_csv_undo_entry();
        self.csv_edit_history_cell = None;
        let cell_count = paste_csv_selection_from_text(&mut document, selection, text)?;
        self.replace_text(serialize_csv(
            &document,
            CsvTextStyle::of(self.text.as_str()),
        ));
        self.csv_debug_message = Some(format!("csv pasted {cell_count} cells"));
        Ok(cell_count)
    }

    fn delete_csv_selection(&mut self) -> Result<String, String> {
        if self.read_only {
            return Err("Focused file is read-only.".to_owned());
        }

        let selection = self
            .csv_selection
            .ok_or_else(|| "No CSV row or column selection.".to_owned())?;
        let mut document = parse_csv(&self.text)?;
        self.push_csv_undo_entry();
        self.csv_edit_history_cell = None;
        let (next_selection, label) = delete_csv_selection(&mut document, selection);
        self.replace_text(serialize_csv(
            &document,
            CsvTextStyle::of(self.text.as_str()),
        ));
        self.csv_selection = Some(next_selection);
        self.selected_csv_cell = None;
        self.csv_drag = None;
        self.csv_debug_message = Some(format!("csv deleted {label}"));
        Ok(label)
    }

    fn handle_virtual_text_input(
        &mut self,
        ui: &mut egui::Ui,
        focused: bool,
        text_edit_id: egui::Id,
        tab_width: usize,
    ) -> bool {
        if self.read_only
            || !focused
            || !virtual_editor_can_handle_key(ui, text_edit_id)
            || self.source_search.open
        {
            return false;
        }

        let (ime_command, ime_changed) = self.consume_virtual_ime_input(ui);
        let command = ime_command.or_else(|| self.consume_virtual_edit_command(ui, tab_width));
        let Some(command) = command else {
            if ime_changed {
                ui.ctx().request_repaint();
            }
            return false;
        };

        let edited = match command {
            VirtualEditCommand::Insert(text) => self.apply_virtual_insert(&text),
            VirtualEditCommand::Backspace => self.apply_virtual_backspace(),
            VirtualEditCommand::Delete => self.apply_virtual_delete(),
            VirtualEditCommand::Undo => self.apply_virtual_undo(),
            VirtualEditCommand::Redo => self.apply_virtual_redo(),
        };

        if edited {
            self.mark_text_changed();
            self.target_position = None;
            self.virtual_ime_preedit = None;
            ui.ctx().request_repaint();
        } else if ime_changed {
            ui.ctx().request_repaint();
        }
        edited
    }

    fn handle_virtual_navigation_input(
        &mut self,
        ui: &mut egui::Ui,
        focused: bool,
        text_edit_id: egui::Id,
        line_index: &LineIndex,
        row_height: f32,
        viewport_height: f32,
        text_clip_width: f32,
        char_width: f32,
        max_scroll_x: f32,
        max_scroll_y: f32,
    ) -> bool {
        if !focused || !virtual_editor_can_handle_key(ui, text_edit_id) || self.source_search.open {
            return false;
        }

        let Some(command) = consume_virtual_navigation_command(ui) else {
            return false;
        };

        self.apply_virtual_navigation(
            command,
            line_index,
            row_height,
            viewport_height,
            text_clip_width,
            char_width,
            max_scroll_x,
            max_scroll_y,
        )
    }

    fn consume_virtual_ime_input(
        &mut self,
        ui: &mut egui::Ui,
    ) -> (Option<VirtualEditCommand>, bool) {
        let mut command = None;
        let mut changed = false;

        ui.input_mut(|input| {
            input.events.retain(|event| {
                let egui::Event::Ime(ime_event) = event else {
                    return true;
                };

                changed = true;
                if let Some(next_command) = self.apply_virtual_ime_event(ime_event) {
                    command = Some(next_command);
                }
                false
            });
        });

        (command, changed)
    }

    fn apply_virtual_ime_event(
        &mut self,
        ime_event: &egui::ImeEvent,
    ) -> Option<VirtualEditCommand> {
        match ime_event {
            egui::ImeEvent::Enabled => {
                self.virtual_ime_preedit.get_or_insert_with(String::new);
                None
            }
            egui::ImeEvent::Preedit(text) => {
                if text == "\n" || text == "\r" || text.is_empty() {
                    self.virtual_ime_preedit = None;
                } else {
                    self.virtual_ime_preedit = Some(text.clone());
                }
                None
            }
            egui::ImeEvent::Commit(text) => {
                self.virtual_ime_preedit = None;
                if text != "\n" && text != "\r" && !text.is_empty() {
                    Some(VirtualEditCommand::Insert(text.clone()))
                } else {
                    None
                }
            }
            egui::ImeEvent::Disabled => {
                self.virtual_ime_preedit = None;
                None
            }
        }
    }

    fn consume_virtual_edit_command(
        &self,
        ui: &mut egui::Ui,
        tab_width: usize,
    ) -> Option<VirtualEditCommand> {
        let cursor_index = self
            .cursor_position
            .and_then(|position| self.text.position_to_char_index(position))
            .unwrap_or(0);

        ui.input_mut(|input| {
            if consume_undo_shortcut(input) {
                return Some(VirtualEditCommand::Undo);
            }
            if consume_redo_shortcut(input) {
                return Some(VirtualEditCommand::Redo);
            }
            if let Some(text) = consume_paste_text(input) {
                return Some(VirtualEditCommand::Insert(text));
            }
            if consume_paste_shortcut(input)
                && let Ok(text) = read_text_from_system_clipboard()
            {
                return Some(VirtualEditCommand::Insert(text));
            }
            if input.consume_key(egui::Modifiers::NONE, egui::Key::Tab) {
                return Some(VirtualEditCommand::Insert(" ".repeat(tab_width)));
            }
            if input.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                return Some(VirtualEditCommand::Insert(smart_newline(
                    &self.text,
                    cursor_index,
                    tab_width,
                )));
            }
            if input.consume_key(egui::Modifiers::NONE, egui::Key::Backspace) {
                return Some(VirtualEditCommand::Backspace);
            }
            if input.consume_key(egui::Modifiers::NONE, egui::Key::Delete) {
                return Some(VirtualEditCommand::Delete);
            }

            let mut command = None;
            input.events.retain(|event| {
                let egui::Event::Text(text) = event else {
                    return true;
                };
                if text.is_empty() || input.modifiers.command || input.modifiers.ctrl {
                    return true;
                }

                command = Some(VirtualEditCommand::Insert(virtual_input_text(
                    text,
                    &self.text,
                    cursor_index,
                )));
                false
            });
            command
        })
    }

    fn apply_virtual_navigation(
        &mut self,
        command: VirtualNavigationCommand,
        line_index: &LineIndex,
        row_height: f32,
        viewport_height: f32,
        text_clip_width: f32,
        char_width: f32,
        max_scroll_x: f32,
        max_scroll_y: f32,
    ) -> bool {
        if line_index.line_count() == 0 {
            self.cursor_position = Some(Position::new(0, 0));
            self.virtual_selection = None;
            return true;
        }

        let current = self
            .cursor_position
            .map(|position| clamp_virtual_position(line_index, position))
            .unwrap_or_else(|| Position::new(0, 0));
        let anchor = self
            .virtual_selection
            .map(|selection| selection.anchor)
            .unwrap_or(current);
        let next = match command.target {
            VirtualNavigationTarget::Left => virtual_position_left(&self.text, current),
            VirtualNavigationTarget::Right => virtual_position_right(&self.text, current),
            VirtualNavigationTarget::Up => virtual_position_vertical(line_index, current, -1),
            VirtualNavigationTarget::Down => virtual_position_vertical(line_index, current, 1),
            VirtualNavigationTarget::Home => virtual_position_line_home(line_index, current),
            VirtualNavigationTarget::End => virtual_position_line_end(line_index, current),
            VirtualNavigationTarget::PageUp => {
                let rows = virtual_page_row_delta(row_height, viewport_height);
                self.scroll_y = (self.scroll_y - rows as f32 * row_height).clamp(0.0, max_scroll_y);
                virtual_position_vertical(line_index, current, -rows)
            }
            VirtualNavigationTarget::PageDown => {
                let rows = virtual_page_row_delta(row_height, viewport_height);
                self.scroll_y = (self.scroll_y + rows as f32 * row_height).clamp(0.0, max_scroll_y);
                virtual_position_vertical(line_index, current, rows)
            }
            VirtualNavigationTarget::SelectAll => {
                let start = Position::new(0, 0);
                let end = virtual_document_end(line_index);
                self.cursor_position = Some(end);
                self.virtual_selection = Some(VirtualTextSelection {
                    anchor: start,
                    head: end,
                });
                self.virtual_ime_preedit = None;
                self.ensure_virtual_cursor_visible(
                    line_index,
                    row_height,
                    viewport_height,
                    text_clip_width,
                    char_width,
                    max_scroll_x,
                    max_scroll_y,
                );
                self.target_position = None;
                return true;
            }
        };

        self.cursor_position = Some(next);
        if command.extend_selection {
            self.virtual_selection = Some(VirtualTextSelection { anchor, head: next });
        } else {
            self.virtual_selection = None;
        }
        self.virtual_ime_preedit = None;
        self.ensure_virtual_cursor_visible(
            line_index,
            row_height,
            viewport_height,
            text_clip_width,
            char_width,
            max_scroll_x,
            max_scroll_y,
        );
        self.target_position = None;
        true
    }

    fn ensure_virtual_cursor_visible(
        &mut self,
        line_index: &LineIndex,
        row_height: f32,
        viewport_height: f32,
        text_clip_width: f32,
        char_width: f32,
        max_scroll_x: f32,
        max_scroll_y: f32,
    ) {
        let Some(position) = self.cursor_position else {
            return;
        };
        let position = clamp_virtual_position(line_index, position);
        let cursor_top = position.line as f32 * row_height;
        let cursor_bottom = cursor_top + row_height;
        if cursor_top < self.scroll_y {
            self.scroll_y = cursor_top.clamp(0.0, max_scroll_y);
        } else if cursor_bottom > self.scroll_y + viewport_height {
            self.scroll_y = (cursor_bottom - viewport_height).clamp(0.0, max_scroll_y);
        }

        let cursor_x =
            visual_column_for_position(None, &self.text, line_index, position, char_width);
        let horizontal_padding = 8.0 * char_width;
        if cursor_x < self.scroll_x {
            self.scroll_x = cursor_x.clamp(0.0, max_scroll_x);
        } else if cursor_x + horizontal_padding > self.scroll_x + text_clip_width {
            self.scroll_x =
                (cursor_x + horizontal_padding - text_clip_width).clamp(0.0, max_scroll_x);
        }
    }

    fn apply_virtual_insert(&mut self, text: &str) -> bool {
        if self.read_only {
            return false;
        }
        let Some(range) = self.virtual_edit_range_for_insert() else {
            return false;
        };
        self.replace_virtual_byte_range(range, text)
    }

    fn apply_virtual_backspace(&mut self) -> bool {
        if self.read_only {
            return false;
        }
        if let Some(range) = self.virtual_selection_byte_range() {
            return self.replace_virtual_byte_range(range, "");
        }

        let Some(cursor) = self.cursor_position else {
            return false;
        };
        let Some(cursor_byte) = virtual_position_to_byte_index(&self.text, cursor) else {
            return false;
        };
        if cursor_byte == 0 {
            return false;
        }
        let start = previous_char_boundary(self.text.as_str(), cursor_byte);
        self.replace_virtual_byte_range(start..cursor_byte, "")
    }

    fn apply_virtual_delete(&mut self) -> bool {
        if self.read_only {
            return false;
        }
        if let Some(range) = self.virtual_selection_byte_range() {
            return self.replace_virtual_byte_range(range, "");
        }

        let Some(cursor) = self.cursor_position else {
            return false;
        };
        let Some(cursor_byte) = virtual_position_to_byte_index(&self.text, cursor) else {
            return false;
        };
        if cursor_byte >= self.text.len() {
            return false;
        }
        let end = next_char_boundary(self.text.as_str(), cursor_byte);
        self.replace_virtual_byte_range(cursor_byte..end, "")
    }

    fn apply_virtual_undo(&mut self) -> bool {
        if self.read_only {
            return false;
        }
        let Some(entry) = self.virtual_undo_stack.pop() else {
            return false;
        };
        let start = entry.start;
        let end = start + entry.inserted.len();
        self.text.replace_byte_range(start..end, &entry.removed);
        self.cursor_position = entry.before_cursor;
        self.virtual_selection = entry.before_selection;
        self.virtual_redo_stack.push(entry);
        limit_virtual_edit_history(&mut self.virtual_redo_stack);
        true
    }

    fn apply_virtual_redo(&mut self) -> bool {
        if self.read_only {
            return false;
        }
        let Some(entry) = self.virtual_redo_stack.pop() else {
            return false;
        };
        let start = entry.start;
        let end = start + entry.removed.len();
        self.text.replace_byte_range(start..end, &entry.inserted);
        self.cursor_position = entry.after_cursor;
        self.virtual_selection = entry.after_selection;
        self.virtual_undo_stack.push(entry);
        limit_virtual_edit_history(&mut self.virtual_undo_stack);
        true
    }

    fn virtual_edit_range_for_insert(&self) -> Option<std::ops::Range<usize>> {
        self.virtual_selection_byte_range().or_else(|| {
            let cursor = self.cursor_position.unwrap_or(Position::new(0, 0));
            let byte = virtual_position_to_byte_index(&self.text, cursor)?;
            Some(byte..byte)
        })
    }

    fn virtual_selection_byte_range(&self) -> Option<std::ops::Range<usize>> {
        let selection = self
            .virtual_selection
            .and_then(|selection| selection.normalized())?;
        virtual_selection_byte_range(&self.text, selection)
    }

    fn replace_virtual_byte_range(
        &mut self,
        range: std::ops::Range<usize>,
        replacement: &str,
    ) -> bool {
        let Some(removed) = self.text.as_str().get(range.clone()).map(ToOwned::to_owned) else {
            return false;
        };
        if removed == replacement {
            return false;
        }
        let before_cursor = self.cursor_position;
        let before_selection = self.virtual_selection;
        let start = range.start;
        self.text.replace_byte_range(range, replacement);
        let cursor_byte = start + replacement.len();
        self.cursor_position = self.text.byte_index_to_position(cursor_byte);
        self.virtual_selection = None;
        self.virtual_undo_stack.push(VirtualEditHistoryEntry {
            start,
            removed,
            inserted: replacement.to_owned(),
            before_cursor,
            after_cursor: self.cursor_position,
            before_selection,
            after_selection: self.virtual_selection,
        });
        limit_virtual_edit_history(&mut self.virtual_undo_stack);
        self.virtual_redo_stack.clear();
        true
    }

    fn handle_input_assistance(
        &mut self,
        ui: &mut egui::Ui,
        text_edit_id: egui::Id,
        tab_width: usize,
    ) {
        if !ui.memory(|memory| memory.has_focus(text_edit_id)) {
            return;
        }

        let mut state = TextEditState::load(ui.ctx(), text_edit_id).unwrap_or_default();
        let Some(cursor_range) = state.cursor.char_range() else {
            return;
        };
        if cursor_range.primary.index != cursor_range.secondary.index {
            return;
        }

        let cursor_index = cursor_range.primary.index;
        let mut command = None;

        ui.input_mut(|input| {
            if input.consume_key(egui::Modifiers::NONE, egui::Key::Tab) {
                command = Some(InputAssistCommand::Insert {
                    text: " ".repeat(tab_width),
                    cursor_delta: tab_width,
                });
                return;
            }

            if input.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                let text = smart_newline(&self.text, cursor_index, tab_width);
                command = Some(InputAssistCommand::Insert {
                    cursor_delta: text.chars().count(),
                    text,
                });
                return;
            }

            input.events.retain(|event| {
                let egui::Event::Text(text) = event else {
                    return true;
                };
                let mut chars = text.chars();
                let Some(character) = chars.next() else {
                    return true;
                };
                if chars.next().is_some() {
                    return true;
                }

                if let Some(close) = closing_pair(character) {
                    command = Some(InputAssistCommand::Insert {
                        text: format!("{character}{close}"),
                        cursor_delta: 1,
                    });
                    return false;
                }

                if is_closing_bracket(character)
                    && self.text.next_char_at(cursor_index) == Some(character)
                {
                    command = Some(InputAssistCommand::Move { cursor_delta: 1 });
                    return false;
                }

                true
            });
        });

        let Some(command) = command else {
            return;
        };

        let (new_cursor_index, text_changed) = match command {
            InputAssistCommand::Insert { text, cursor_delta } => {
                let Some(byte_index) = self.text.char_index_to_byte_index(cursor_index) else {
                    return;
                };
                self.text.insert_str(byte_index, &text);
                self.mark_text_changed();
                (cursor_index + cursor_delta, true)
            }
            InputAssistCommand::Move { cursor_delta } => (cursor_index + cursor_delta, false),
        };

        let cursor = CCursor::new(new_cursor_index);
        state.cursor.set_char_range(Some(CCursorRange::one(cursor)));
        state.store(ui.ctx(), text_edit_id);
        self.cursor_position = self.text.char_index_to_position(new_cursor_index);
        if text_changed {
            self.target_position = None;
        }
        ui.ctx().request_repaint();
    }

    fn set_csv_mode(&mut self, mode: CsvMode) {
        if self.csv_mode == mode {
            return;
        }
        self.csv_mode = mode;
        self.csv_edit_history_cell = None;
        if mode == CsvMode::Grid {
            self.close_source_search();
        }
    }

    fn csv_editor_ui(
        &mut self,
        ui: &mut egui::Ui,
        focused: bool,
        language_registry: &LanguageRegistry,
        tab_width: usize,
    ) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("CSV").small().strong());
            ui.separator();
            for mode in CsvMode::all() {
                if ui
                    .selectable_label(self.csv_mode == mode, mode.label())
                    .on_hover_text(format!("Switch to {} mode", mode.label()))
                    .clicked()
                {
                    self.set_csv_mode(mode);
                }
            }
        });
        ui.separator();

        match self.csv_mode {
            CsvMode::Grid => self.csv_ui(ui, focused),
            CsvMode::Source => {
                self.source_editor_ui(ui, focused, language_registry, tab_width, &[])
            }
        }
    }

    fn csv_ui(&mut self, ui: &mut egui::Ui, focused: bool) {
        let mut document = match parse_csv(&self.text) {
            Ok(document) => document,
            Err(error) => {
                ui.label(
                    RichText::new(format!("CSV parse error: {error}"))
                        .color(ui.visuals().warn_fg_color),
                );
                return;
            }
        };

        let column_count = document.column_count();
        if column_count == 0 {
            ui.label(RichText::new("Empty CSV").weak());
            return;
        }

        let column_widths = csv_column_widths(ui, &document, column_count);
        let header_height = editor_row_height(ui) + 10.0;
        let row_height = editor_row_height(ui) + 8.0;
        let row_number_width =
            ((document.rows.len() + 1).to_string().len() as f32 * 10.0 + 22.0).max(44.0);
        let mut changed = false;
        let mut next_focus = None;

        if self.csv_selection.is_none()
            && self.selected_csv_cell.is_none()
            && !document.rows.is_empty()
        {
            self.selected_csv_cell = Some(CsvCellPosition { row: 0, column: 0 });
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new("CSV VIEW").small().strong());
            if let Some(selection) = self.csv_selection {
                ui.label(RichText::new(csv_selection_label(selection)).small().weak());
            }
            if let Some(message) = &self.csv_debug_message {
                ui.label(RichText::new(message).small().weak());
            }
        });

        ScrollArea::both()
            .id_salt(format!("csv_grid_{:?}", self.pane))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if let Some(action) = paint_csv_header(
                    ui,
                    row_number_width,
                    header_height,
                    &column_widths,
                    self.csv_selection,
                    self.csv_drag,
                ) {
                    self.csv_selection = Some(match action {
                        CsvHeaderAction::Select(column) => CsvSelection::Columns {
                            start: column,
                            end: column,
                        },
                        CsvHeaderAction::Press(column) => {
                            self.csv_drag = Some(CsvSelectionDrag {
                                kind: CsvSelectionKind::Columns,
                                anchor: column,
                            });
                            CsvSelection::Columns {
                                start: column,
                                end: column,
                            }
                        }
                        CsvHeaderAction::ExtendDrag(column) => {
                            let start = self
                                .csv_drag
                                .filter(|drag| drag.kind == CsvSelectionKind::Columns)
                                .map(|drag| drag.anchor)
                                .unwrap_or(column);
                            CsvSelection::Columns { start, end: column }
                        }
                    });
                    self.csv_debug_message = Some(format!(
                        "csv column action: {:?} -> {:?}",
                        action, self.csv_selection
                    ));
                    self.selected_csv_cell = None;
                    self.csv_edit_history_cell = None;
                    ui.ctx().request_repaint();
                }

                for row in 0..document.rows.len() {
                    ui.horizontal(|ui| {
                        let active_row = self.selected_csv_cell.is_some_and(|cell| cell.row == row)
                            && self.csv_selection.is_none();
                        let selected_row = self
                            .csv_selection
                            .is_some_and(|selection| selection.contains(row, 0));
                        let row_header_response = paint_csv_row_header(
                            ui,
                            row + 1,
                            row_number_width,
                            row_height,
                            active_row || selected_row,
                        );
                        if let Some(action) = csv_pointer_action_for_rect(
                            ui,
                            row_header_response.rect,
                            row,
                            CsvSelectionKind::Rows,
                            self.csv_drag,
                        ) {
                            self.csv_selection = Some(match action {
                                CsvHeaderAction::Select(row) | CsvHeaderAction::Press(row) => {
                                    self.csv_drag = Some(CsvSelectionDrag {
                                        kind: CsvSelectionKind::Rows,
                                        anchor: row,
                                    });
                                    CsvSelection::Rows {
                                        start: row,
                                        end: row,
                                    }
                                }
                                CsvHeaderAction::ExtendDrag(row) => {
                                    let start = self
                                        .csv_drag
                                        .filter(|drag| drag.kind == CsvSelectionKind::Rows)
                                        .map(|drag| drag.anchor)
                                        .unwrap_or(row);
                                    CsvSelection::Rows { start, end: row }
                                }
                            });
                            self.csv_debug_message = Some(format!(
                                "csv row action: {:?} -> {:?}",
                                action, self.csv_selection
                            ));
                            self.selected_csv_cell = None;
                            self.csv_edit_history_cell = None;
                            ui.ctx().request_repaint();
                        }

                        for (column, width) in column_widths.iter().enumerate().take(column_count) {
                            let id = ui
                                .id()
                                .with(format!("csv_cell_{:?}_{row}_{column}", self.pane));
                            let row_count = document.rows.len();
                            let is_selected = self.csv_selection.is_none()
                                && self.selected_csv_cell == Some(CsvCellPosition { row, column });
                            let is_range_selected = self
                                .csv_selection
                                .is_some_and(|selection| selection.contains(row, column));
                            let text = document
                                .rows
                                .get(row)
                                .and_then(|fields| fields.get(column))
                                .cloned()
                                .unwrap_or_default();
                            let response = paint_csv_cell(
                                ui,
                                &text,
                                *width,
                                row_height,
                                active_row || is_range_selected,
                                is_selected || is_range_selected,
                            );

                            if response.clicked() {
                                self.selected_csv_cell = Some(CsvCellPosition { row, column });
                                self.csv_selection = None;
                                self.csv_drag = None;
                                self.csv_edit_history_cell = None;
                                self.csv_debug_message =
                                    Some(format!("csv cell selected: R{}C{}", row + 1, column + 1));
                                self.cursor_position =
                                    Some(Position::new(row as u32, column as u32));
                                ui.memory_mut(|memory| memory.request_focus(id));
                                ui.ctx().request_repaint();
                            }

                            if is_selected {
                                let edit_rect = response.rect.shrink2(egui::vec2(3.0, 2.0));
                                let cell_position = CsvCellPosition { row, column };
                                let before_edit = self.capture_csv_history_entry();
                                let cell = document.cell_mut(row, column);
                                if focused
                                    && let Some(target) = csv_navigation_target(
                                        ui,
                                        id,
                                        row,
                                        column,
                                        row_count,
                                        column_count,
                                        cell.chars().count(),
                                    )
                                {
                                    next_focus = Some(target);
                                }

                                let output = ui
                                    .scope_builder(
                                        egui::UiBuilder::new().max_rect(edit_rect),
                                        |ui| {
                                            TextEdit::singleline(cell)
                                                .id(id)
                                                .font(TextStyle::Monospace)
                                                .desired_width(edit_rect.width())
                                                .interactive(!self.read_only)
                                                .show(ui)
                                        },
                                    )
                                    .inner;

                                if output.response.changed() {
                                    if self.csv_edit_history_cell != Some(cell_position) {
                                        self.push_csv_undo_entry_value(before_edit);
                                        self.csv_edit_history_cell = Some(cell_position);
                                    }
                                    changed = true;
                                    self.target_position = None;
                                }

                                if output.response.has_focus() {
                                    self.cursor_position =
                                        Some(Position::new(row as u32, column as u32));
                                } else if focused {
                                    ui.memory_mut(|memory| memory.request_focus(id));
                                }
                            }
                        }
                    });
                }
            });

        if !ui.input(|input| input.pointer.primary_down()) {
            self.csv_drag = None;
        }

        if changed {
            self.replace_text(serialize_csv(
                &document,
                CsvTextStyle::of(self.text.as_str()),
            ));
        }

        if let Some(target) = next_focus
            && self.selected_csv_cell != Some(target)
        {
            let needs_new_cell = document
                .rows
                .get(target.row)
                .and_then(|row| row.get(target.column))
                .is_none();
            if needs_new_cell {
                self.push_csv_undo_entry();
                document.ensure_cell(target.row, target.column);
                self.replace_text(serialize_csv(
                    &document,
                    CsvTextStyle::of(self.text.as_str()),
                ));
            }
            self.selected_csv_cell = Some(target);
            self.csv_edit_history_cell = None;
            self.cursor_position = Some(Position::new(target.row as u32, target.column as u32));
            let id = ui.id().with(format!(
                "csv_cell_{:?}_{}_{}",
                self.pane, target.row, target.column
            ));
            ui.memory_mut(|memory| memory.request_focus(id));
            ui.ctx().request_repaint();
        }
    }

    fn markdown_ui(
        &mut self,
        ui: &mut egui::Ui,
        focused: bool,
        language_registry: &LanguageRegistry,
        tab_width: usize,
        bookmark_lines: &[u32],
    ) {
        let Some(path) = self.path.clone() else {
            ui.label(RichText::new("No Markdown file open.").weak());
            return;
        };

        let document = match self.cached_markdown_document(&path) {
            Ok(document) => document,
            Err(error) => {
                ui.label(
                    RichText::new(format!("Markdown parse error: {error}"))
                        .small()
                        .color(ui.visuals().error_fg_color),
                );
                ui.separator();
                self.markdown_source_ui(ui, focused, tab_width, None, None, bookmark_lines);
                return;
            }
        };

        if self
            .markdown_active_page_id
            .as_deref()
            .and_then(|page_id| document.page_by_id(page_id))
            .is_none()
        {
            self.markdown_active_page_id = document.pages.first().map(|page| page.id.clone());
        }
        if matches!(
            self.markdown_mode,
            MarkdownMode::Markdown | MarkdownMode::Both
        ) {
            self.update_markdown_active_page_from_cursor(&document);
        }

        if focused
            && matches!(
                self.markdown_mode,
                MarkdownMode::Preview | MarkdownMode::Both
            )
        {
            self.handle_markdown_preview_navigation(ui, &document);
        }
        if focused {
            self.handle_markdown_mode_shortcuts(ui, &document);
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new("MARKDOWN").small().strong());
            ui.separator();
            for mode in MarkdownMode::all() {
                if ui
                    .selectable_label(self.markdown_mode == mode, mode.label())
                    .on_hover_text(format!("Switch to {} mode", mode.label()))
                    .clicked()
                {
                    self.set_markdown_mode(mode, &document);
                }
            }
            if self.markdown_mode != MarkdownMode::Markdown {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .selectable_label(self.markdown_show_outline, "Outline")
                        .on_hover_text("Show or hide the outline pane")
                        .clicked()
                    {
                        self.markdown_show_outline = !self.markdown_show_outline;
                        self.normalize_markdown_focus_target();
                    }
                    if ui
                        .selectable_label(self.markdown_show_pages, "Pages")
                        .on_hover_text("Show or hide the pages pane")
                        .clicked()
                    {
                        self.markdown_show_pages = !self.markdown_show_pages;
                        self.normalize_markdown_focus_target();
                    }
                });
            }
        });

        let matches = if self.markdown_search.open {
            let matches = markdown_search_matches(&document, &self.markdown_search.query);
            self.markdown_search.match_count = matches.len();
            if self.markdown_search.current_match >= matches.len() {
                self.markdown_search.current_match = matches.len().saturating_sub(1);
            }
            if focused {
                self.handle_markdown_search_global_shortcuts(ui, &document, &matches);
            }
            self.markdown_search_ui(ui, &document, &matches);
            matches
        } else {
            self.markdown_search.match_count = 0;
            Vec::new()
        };

        ui.separator();

        let active_page_range = self.markdown_active_source_line_range(&document);
        let active_search_match = self.markdown_active_search_match(&matches);
        match self.markdown_mode {
            MarkdownMode::Preview => {
                self.markdown_preview_ui(ui, &document, language_registry, active_search_match)
            }
            MarkdownMode::Markdown => self.markdown_source_ui(
                ui,
                focused,
                tab_width,
                active_page_range,
                Some(&document),
                bookmark_lines,
            ),
            MarkdownMode::Both => self.markdown_both_ui(
                ui,
                focused,
                &document,
                language_registry,
                tab_width,
                active_page_range,
                active_search_match,
                bookmark_lines,
            ),
        }
    }

    fn cached_markdown_document(&mut self, path: &Path) -> Result<Rc<MarkdownDocument>, String> {
        let (source_len, source_hash) = self.text_fingerprint();

        if let Some(cache) = &self.markdown_document_cache
            && cache.path == path
            && cache.source_len == source_len
            && cache.source_hash == source_hash
        {
            return cache.document.clone();
        }

        let document =
            MarkdownDocument::parse(path.to_path_buf(), self.text.as_str().to_owned()).map(Rc::new);
        self.markdown_document_cache = Some(MarkdownDocumentCache {
            path: path.to_path_buf(),
            source_len,
            source_hash,
            document: document.clone(),
        });
        document
    }

    fn cached_syntax_highlight_spans(
        &mut self,
        ui: &egui::Ui,
        language_registry: &LanguageRegistry,
        source_len: usize,
        source_hash: u64,
    ) -> Rc<Vec<HighlightSpan>> {
        self.poll_syntax_highlight(ui);

        let path = self.path.clone();
        if let Some(cache) = &self.syntax_highlight_cache
            && cache.path == path
            && cache.source_len == source_len
            && cache.source_hash == source_hash
        {
            return cache.spans.clone();
        }

        if path
            .as_deref()
            .is_some_and(|path| is_external_rust_path(path))
            && source_len > EXTERNAL_RUST_HIGHLIGHT_MAX_BYTES
        {
            let spans = Rc::new(Vec::new());
            self.syntax_highlight_cache = Some(SyntaxHighlightCache {
                path,
                source_len,
                source_hash,
                spans: spans.clone(),
                line_span_ranges: Rc::new(vec![0..0; self.text.line_index().line_count()]),
            });
            return spans;
        }

        if path
            .as_deref()
            .and_then(|path| language_registry.language_for_path(path))
            .is_none()
        {
            let spans = Rc::new(Vec::new());
            self.syntax_highlight_cache = Some(SyntaxHighlightCache {
                path,
                source_len,
                source_hash,
                spans: spans.clone(),
                line_span_ranges: Rc::new(vec![0..0; self.text.line_index().line_count()]),
            });
            return spans;
        }

        let pending_matches = self
            .syntax_highlight_pending
            .as_ref()
            .is_some_and(|pending| {
                pending.path == path
                    && pending.source_len == source_len
                    && pending.source_hash == source_hash
            });
        if !pending_matches {
            self.syntax_highlight_receiver = path.as_ref().map(|path| {
                start_syntax_highlighting(
                    path.clone(),
                    self.text.as_str().to_owned(),
                    language_registry.clone(),
                    source_len,
                    source_hash,
                )
            });
            self.syntax_highlight_pending = Some(SyntaxHighlightPending {
                path: path.clone(),
                source_len,
                source_hash,
            });
        }

        ui.ctx().request_repaint_after(Duration::from_millis(30));
        Rc::new(Vec::new())
    }

    fn poll_syntax_highlight(&mut self, ui: &egui::Ui) {
        let Some(receiver) = self.syntax_highlight_receiver.take() else {
            return;
        };

        match receiver.try_recv() {
            Ok(result) => {
                let (current_source_len, current_source_hash) = self.text_fingerprint();
                let current_path = self.path.clone();
                let pending_matches =
                    self.syntax_highlight_pending
                        .as_ref()
                        .is_some_and(|pending| {
                            pending.path == result.path
                                && pending.source_len == result.source_len
                                && pending.source_hash == result.source_hash
                        });
                let current_matches = current_path == result.path
                    && current_source_len == result.source_len
                    && current_source_hash == result.source_hash;
                if pending_matches && current_matches {
                    self.syntax_highlight_cache = Some(SyntaxHighlightCache {
                        path: result.path,
                        source_len: result.source_len,
                        source_hash: result.source_hash,
                        line_span_ranges: Rc::new(build_highlight_line_span_ranges(
                            &result.spans,
                            self.text.line_index().as_ref(),
                        )),
                        spans: Rc::new(result.spans),
                    });
                    self.syntax_highlight_pending = None;
                    self.virtual_line_layout_cache = None;
                    ui.ctx().request_repaint();
                }
            }
            Err(mpsc::TryRecvError::Empty) => {
                self.syntax_highlight_receiver = Some(receiver);
                ui.ctx().request_repaint_after(Duration::from_millis(30));
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.syntax_highlight_pending = None;
            }
        }
    }

    fn cached_syntax_highlight_line_span_ranges(
        &self,
        source_len: usize,
        source_hash: u64,
    ) -> Rc<Vec<std::ops::Range<usize>>> {
        self.syntax_highlight_cache
            .as_ref()
            .filter(|cache| {
                cache.path == self.path
                    && cache.source_len == source_len
                    && cache.source_hash == source_hash
            })
            .map(|cache| cache.line_span_ranges.clone())
            .unwrap_or_else(|| Rc::new(vec![0..0; self.text.line_index().line_count()]))
    }

    fn cached_editor_metrics(&mut self, source_len: usize, source_hash: u64) -> EditorMetricsCache {
        if let Some(cache) = self.editor_metrics_cache
            && cache.source_len == source_len
            && cache.source_hash == source_hash
        {
            return cache;
        }

        let line_index = self.cached_line_index(source_len, source_hash);

        let cache = EditorMetricsCache {
            source_len,
            source_hash,
            line_count: line_index.line_count(),
            longest_line_chars: line_index.longest_line_chars,
        };
        self.editor_metrics_cache = Some(cache);
        cache
    }

    fn cached_line_index(&mut self, source_len: usize, source_hash: u64) -> Rc<LineIndex> {
        let line_index = self.text.line_index();
        debug_assert_eq!(line_index.source_len, source_len);
        debug_assert_eq!(line_index.source_hash, source_hash);
        line_index
    }

    fn cached_virtual_line_galley(
        &mut self,
        ui: &egui::Ui,
        line_number: usize,
        line_info: LineInfo,
        source_len: usize,
        source_hash: u64,
        highlight_spans: &[HighlightSpan],
        highlight_line_span_range: std::ops::Range<usize>,
        bracket_pair: Option<(usize, usize)>,
        source_search_matches: &[SourceSearchMatch],
        active_source_search_match: Option<usize>,
        frame_counters: Option<&mut VirtualFrameCounters>,
    ) -> Arc<egui::Galley> {
        let mut frame_counters = frame_counters;
        let can_cache = source_search_matches.is_empty()
            && !line_contains_bracket_pair(line_info, bracket_pair);
        let dark_mode = ui.visuals().dark_mode;
        let frame = ui.ctx().cumulative_frame_nr();

        if can_cache {
            let cache =
                self.virtual_line_layout_cache
                    .get_or_insert_with(|| VirtualLineLayoutCache {
                        source_len,
                        source_hash,
                        dark_mode,
                        entries: HashMap::new(),
                    });
            if cache.source_len != source_len
                || cache.source_hash != source_hash
                || cache.dark_mode != dark_mode
            {
                *cache = VirtualLineLayoutCache {
                    source_len,
                    source_hash,
                    dark_mode,
                    entries: HashMap::new(),
                };
            }

            if let Some(entry) = cache.entries.get_mut(&line_number)
                && entry.line_start == line_info.start
                && entry.line_content_end == line_info.content_end
            {
                entry.last_used_frame = frame;
                if let Some(counters) = frame_counters.as_deref_mut() {
                    counters.galley_hits += 1;
                }
                return entry.galley.clone();
            }
        }
        if let Some(counters) = frame_counters.as_deref_mut() {
            counters.galley_misses += 1;
        }

        let mut job = highlighted_line_layout_job(
            ui,
            &self.text,
            line_info,
            highlight_spans,
            highlight_line_span_range,
            bracket_pair,
            source_search_matches,
            active_source_search_match,
        );
        job.wrap.max_width = f32::INFINITY;
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));

        if can_cache && let Some(cache) = &mut self.virtual_line_layout_cache {
            cache.entries.insert(
                line_number,
                CachedVirtualLineLayout {
                    line_start: line_info.start,
                    line_content_end: line_info.content_end,
                    galley: galley.clone(),
                    last_used_frame: frame,
                },
            );
            prune_virtual_line_layout_cache(cache, frame);
        }

        galley
    }

    fn cached_virtual_line_number_galley(
        &mut self,
        ui: &egui::Ui,
        line_number: usize,
        line_count: usize,
        active: bool,
        inactive_color: Color32,
        active_color: Color32,
        frame_counters: Option<&mut VirtualFrameCounters>,
    ) -> Arc<egui::Galley> {
        let mut frame_counters = frame_counters;
        let dark_mode = ui.visuals().dark_mode;
        let frame = ui.ctx().cumulative_frame_nr();
        let cache = self
            .virtual_line_number_layout_cache
            .get_or_insert_with(|| VirtualLineNumberLayoutCache {
                line_count,
                dark_mode,
                inactive_color,
                active_color,
                entries: HashMap::new(),
            });
        if cache.line_count != line_count
            || cache.dark_mode != dark_mode
            || cache.inactive_color != inactive_color
            || cache.active_color != active_color
        {
            *cache = VirtualLineNumberLayoutCache {
                line_count,
                dark_mode,
                inactive_color,
                active_color,
                entries: HashMap::new(),
            };
        }

        let key = (line_number, active);
        if let Some(entry) = cache.entries.get_mut(&key) {
            entry.last_used_frame = frame;
            if let Some(counters) = frame_counters.as_deref_mut() {
                counters.line_number_hits += 1;
            }
            return entry.galley.clone();
        }
        if let Some(counters) = frame_counters.as_deref_mut() {
            counters.line_number_misses += 1;
        }

        let color = if active { active_color } else { inactive_color };
        let galley =
            ui.painter()
                .layout_no_wrap((line_number + 1).to_string(), editor_font_id(), color);
        cache.entries.insert(
            key,
            CachedVirtualLineNumberLayout {
                galley: galley.clone(),
                last_used_frame: frame,
            },
        );
        prune_virtual_line_number_layout_cache(cache, frame);

        galley
    }

    fn cached_bracket_pair(
        &mut self,
        source_len: usize,
        source_hash: u64,
    ) -> Option<(usize, usize)> {
        let cursor_position = self.cursor_position;
        if let Some(cache) = self.bracket_pair_cache
            && cache.source_len == source_len
            && cache.source_hash == source_hash
            && cache.cursor_position == cursor_position
        {
            return cache.pair;
        }

        let pair =
            cursor_position.and_then(|position| find_matching_bracket_pair(&self.text, position));
        self.bracket_pair_cache = Some(BracketPairCache {
            source_len,
            source_hash,
            cursor_position,
            pair,
        });
        pair
    }

    fn cached_source_search_matches(
        &mut self,
        source_len: usize,
        source_hash: u64,
    ) -> Rc<Vec<SourceSearchMatch>> {
        let query = self.source_search.query.clone();
        if let Some(cache) = &self.source_search_cache
            && cache.source_len == source_len
            && cache.source_hash == source_hash
            && cache.query == query
        {
            return cache.matches.clone();
        }

        let matches = Rc::new(source_search_matches(&self.text, &query));
        self.source_search_cache = Some(SourceSearchCache {
            source_len,
            source_hash,
            query,
            matches: matches.clone(),
        });
        matches
    }

    fn set_markdown_mode(&mut self, mode: MarkdownMode, document: &MarkdownDocument) {
        if self.markdown_mode == mode {
            return;
        }

        if matches!(mode, MarkdownMode::Markdown | MarkdownMode::Both) {
            self.prepare_markdown_source_entry(document);
            self.markdown_focus_target = MarkdownFocusTarget::Body;
        }
        if mode == MarkdownMode::Both && self.markdown_focus_target == MarkdownFocusTarget::Outline
        {
            self.markdown_focus_target = MarkdownFocusTarget::Body;
        }
        self.markdown_mode = mode;
    }

    fn handle_markdown_mode_shortcuts(&mut self, ui: &mut egui::Ui, document: &MarkdownDocument) {
        let mut next_mode = None;
        ui.input_mut(|input| {
            if input.consume_key(egui::Modifiers::ALT, egui::Key::Num1) {
                next_mode = Some(MarkdownMode::Preview);
            } else if input.consume_key(egui::Modifiers::ALT, egui::Key::Num2) {
                next_mode = Some(MarkdownMode::Markdown);
            } else if input.consume_key(egui::Modifiers::ALT, egui::Key::Num3) {
                next_mode = Some(MarkdownMode::Both);
            }
        });

        if let Some(mode) = next_mode {
            self.set_markdown_mode(mode, document);
            ui.ctx().request_repaint();
        }
    }

    fn markdown_search_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        matches: &[MarkdownSearchMatch],
    ) {
        let search_id = ui.id().with(format!("markdown_search_{:?}", self.pane));
        let mut close = false;
        let mut go_next = false;
        let mut go_previous = false;
        let mut keep_focus = false;

        ui.horizontal(|ui| {
            ui.label(RichText::new("Find").small().strong());
            let input_had_focus = ui.memory(|memory| memory.has_focus(search_id));
            if input_had_focus {
                match ui.input(raw_search_enter_direction) {
                    Some(1) => {
                        go_next = true;
                        keep_focus = true;
                    }
                    Some(-1) => {
                        go_previous = true;
                        keep_focus = true;
                    }
                    _ => {}
                }
            }
            // egui drops focus at the start of the frame that carries a plain Escape, so check
            // who had focus in the previous frame.
            if ui.memory(|memory| memory.had_focus_last_frame(search_id))
                && ui.input(raw_escape_pressed)
            {
                close = true;
            }

            let response = ui.add(
                TextEdit::singleline(&mut self.markdown_search.query)
                    .id(search_id)
                    .hint_text("Search rendered markdown")
                    .desired_width(220.0),
            );
            if response.changed() {
                self.markdown_search.current_match = 0;
                self.markdown_search.scrolled_match = None;
            }
            if self.markdown_search.focus_requested {
                response.request_focus();
                self.markdown_search.focus_requested = false;
            }
            let input_has_focus =
                response.has_focus() || ui.memory(|memory| memory.has_focus(search_id));
            if !input_had_focus && input_has_focus {
                match ui.input(raw_search_enter_direction) {
                    Some(1) => {
                        go_next = true;
                        keep_focus = true;
                    }
                    Some(-1) => {
                        go_previous = true;
                        keep_focus = true;
                    }
                    _ => {}
                }
                if ui.input(raw_escape_pressed) {
                    close = true;
                }
            }

            let count_label = if self.markdown_search.query.trim().is_empty() {
                "0 results".to_owned()
            } else if matches.is_empty() {
                "No results".to_owned()
            } else {
                format!(
                    "{}/{}",
                    self.markdown_search.current_match + 1,
                    matches.len()
                )
            };
            ui.label(RichText::new(count_label).small().weak());

            if ui
                .add_enabled(!matches.is_empty(), egui::Button::new("Previous"))
                .on_hover_text("Previous search match")
                .clicked()
            {
                go_previous = true;
            }
            if ui
                .add_enabled(!matches.is_empty(), egui::Button::new("Next"))
                .on_hover_text("Next search match")
                .clicked()
            {
                go_next = true;
            }
            if ui.button("Close").on_hover_text("Close search").clicked() {
                close = true;
            }

            if keep_focus && !close {
                ui.memory_mut(|memory| memory.request_focus(search_id));
            }
        });

        if close {
            self.markdown_search.open = false;
            self.markdown_search.focus_requested = false;
            self.markdown_search.query.clear();
            self.markdown_search.current_match = 0;
            self.markdown_search.match_count = 0;
            self.markdown_search.scrolled_match = None;
            return;
        }

        if go_next {
            self.move_markdown_search_match(document, matches, 1);
        } else if go_previous {
            self.move_markdown_search_match(document, matches, -1);
        } else {
            self.navigate_to_current_markdown_match(matches);
        }
    }

    fn handle_markdown_search_global_shortcuts(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        matches: &[MarkdownSearchMatch],
    ) {
        if matches.is_empty() {
            return;
        }

        let mut go_next = false;
        let mut go_previous = false;
        ui.input_mut(|input| {
            if input.key_pressed(egui::Key::F3) {
                if input.modifiers.shift {
                    go_previous = true;
                } else {
                    go_next = true;
                }
                let _ = input.consume_key(input.modifiers, egui::Key::F3);
            }
        });

        if go_next {
            self.move_markdown_search_match(document, matches, 1);
        } else if go_previous {
            self.move_markdown_search_match(document, matches, -1);
        }
    }

    fn move_markdown_search_match(
        &mut self,
        document: &MarkdownDocument,
        matches: &[MarkdownSearchMatch],
        delta: isize,
    ) {
        if matches.is_empty() {
            return;
        }

        let len = matches.len() as isize;
        let current = self.markdown_search.current_match as isize;
        self.markdown_search.current_match = (current + delta).rem_euclid(len) as usize;
        self.markdown_search.scrolled_match = None;
        self.navigate_to_current_markdown_match(matches);

        if let Some(search_match) = self.markdown_active_search_match(matches)
            && let Some(page) = document.page_by_id(&search_match.page_id)
        {
            let position = Position::new(page.source_range.start.line as u32, 0);
            self.cursor_position = Some(position);
        }
    }

    fn navigate_to_current_markdown_match(&mut self, matches: &[MarkdownSearchMatch]) {
        let Some(search_match) = self.markdown_active_search_match(matches) else {
            return;
        };
        if self.markdown_active_page_id.as_deref() != Some(search_match.page_id.as_str()) {
            self.markdown_active_heading_id = None;
        }
        self.markdown_active_page_id = Some(search_match.page_id.clone());
    }

    fn markdown_active_search_match(
        &self,
        matches: &[MarkdownSearchMatch],
    ) -> Option<MarkdownSearchMatch> {
        if self.markdown_search.query.trim().is_empty() {
            return None;
        }
        matches.get(self.markdown_search.current_match).cloned()
    }

    fn markdown_preview_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        language_registry: &LanguageRegistry,
        active_search_match: Option<MarkdownSearchMatch>,
    ) {
        let available_height = ui.available_height();
        let available_width = ui.available_width();
        let left_width = if self.markdown_show_pages {
            self.markdown_pages_width =
                clamp_markdown_side_pane_width(self.markdown_pages_width, available_width);
            self.markdown_pages_width
        } else {
            0.0
        };
        let right_width = if self.markdown_show_outline {
            markdown_side_pane_width(available_width)
        } else {
            0.0
        };
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(available_width, available_height),
            egui::Sense::hover(),
        );
        let separator_width = 8.0;
        let separator_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let mut left = rect.left();
        let mut right = rect.right();

        if self.markdown_show_pages {
            let page_rect = egui::Rect::from_min_size(
                egui::pos2(left, rect.top()),
                egui::vec2(left_width, rect.height()),
            );
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(page_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
                |ui| {
                    ui.set_clip_rect(page_rect);
                    self.markdown_page_nav_ui(ui, document);
                },
            );
            left = page_rect.right();
            let handle_rect = egui::Rect::from_min_size(
                egui::pos2(left, rect.top()),
                egui::vec2(separator_width, rect.height()),
            );
            self.markdown_pages_resize_handle(ui, handle_rect, rect.left(), available_width);
            left += separator_width;
        }

        if self.markdown_show_outline {
            let outline_left = (right - right_width).max(left);
            let outline_rect = egui::Rect::from_min_size(
                egui::pos2(outline_left, rect.top()),
                egui::vec2((right - outline_left).max(0.0), rect.height()),
            );
            let separator_x = outline_rect.left() - separator_width * 0.5;
            ui.painter().line_segment(
                [
                    egui::pos2(separator_x, rect.top()),
                    egui::pos2(separator_x, rect.bottom()),
                ],
                egui::Stroke::new(1.0, separator_color),
            );
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(outline_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
                |ui| {
                    ui.set_clip_rect(outline_rect);
                    self.markdown_outline_ui(ui, document);
                },
            );
            right = outline_rect.left() - separator_width;
        }

        let body_rect = egui::Rect::from_min_max(
            egui::pos2(left, rect.top()),
            egui::pos2(right.max(left), rect.bottom()),
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(body_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
            |ui| {
                ui.set_clip_rect(body_rect);
                self.markdown_body_ui(ui, document, language_registry, active_search_match);
            },
        );
    }

    fn markdown_both_ui(
        &mut self,
        ui: &mut egui::Ui,
        focused: bool,
        document: &MarkdownDocument,
        language_registry: &LanguageRegistry,
        tab_width: usize,
        active_page_range: Option<(u32, u32)>,
        active_search_match: Option<MarkdownSearchMatch>,
        bookmark_lines: &[u32],
    ) {
        let available_height = ui.available_height();
        let available_width = ui.available_width();
        let separator_width = 8.0;
        let page_width = if self.markdown_show_pages {
            self.markdown_pages_width =
                clamp_markdown_side_pane_width(self.markdown_pages_width, available_width);
            self.markdown_pages_width
        } else {
            0.0
        };
        let outline_width = if self.markdown_show_outline {
            markdown_side_pane_width(available_width)
        } else {
            0.0
        };
        let separator_count =
            1 + usize::from(self.markdown_show_pages) + usize::from(self.markdown_show_outline);
        let remaining_width = (available_width
            - page_width
            - outline_width
            - separator_width * separator_count as f32)
            .max(360.0);
        let preview_width = (remaining_width * 0.48).max(220.0);
        let editor_width = (remaining_width - preview_width - 8.0).max(220.0);

        ui.horizontal_top(|ui| {
            if self.markdown_show_pages {
                ui.allocate_ui_with_layout(
                    egui::vec2(page_width, available_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.markdown_page_nav_ui(ui, document),
                );
                let (handle_rect, _) = ui.allocate_exact_size(
                    egui::vec2(separator_width, available_height),
                    egui::Sense::hover(),
                );
                let pane_left = handle_rect.left() - page_width;
                self.markdown_pages_resize_handle(ui, handle_rect, pane_left, available_width);
            }
            ui.allocate_ui_with_layout(
                egui::vec2(preview_width, available_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.markdown_body_ui(ui, document, language_registry, active_search_match),
            );
            ui.separator();
            ui.allocate_ui_with_layout(
                egui::vec2(editor_width, available_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.label(self.markdown_focus_label(ui, "Markdown", MarkdownFocusTarget::Body));
                    ui.add_space(4.0);
                    self.markdown_source_ui(
                        ui,
                        focused,
                        tab_width,
                        active_page_range,
                        Some(document),
                        bookmark_lines,
                    );
                },
            );
            if self.markdown_show_outline {
                ui.separator();
                ui.allocate_ui_with_layout(
                    egui::vec2(outline_width, available_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.markdown_outline_ui(ui, document),
                );
            }
        });
    }

    fn markdown_pages_resize_handle(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        pane_left: f32,
        available_width: f32,
    ) {
        let hit_rect = rect.expand(6.0);
        let response = ui.interact(
            hit_rect,
            ui.make_persistent_id(("markdown_pages_resize", self.pane)),
            egui::Sense::click_and_drag(),
        );

        let pointer_position = ui.input(|input| input.pointer.interact_pos());
        let primary_pressed = ui.input(|input| input.pointer.primary_pressed());
        let primary_down = ui.input(|input| input.pointer.primary_down());
        if primary_pressed && pointer_position.is_some_and(|position| hit_rect.contains(position)) {
            self.markdown_pages_resize_start_width = Some(self.markdown_pages_width);
        }
        if self.markdown_pages_resize_start_width.is_some() && primary_down {
            if let Some(pointer_position) = pointer_position {
                self.markdown_pages_width =
                    clamp_markdown_side_pane_width(pointer_position.x - pane_left, available_width);
            }
            ui.ctx().request_repaint();
        }
        if !primary_down {
            self.markdown_pages_resize_start_width = None;
        }

        let resizing = self.markdown_pages_resize_start_width.is_some();
        if response.hovered() || resizing {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }

        let stroke_color = if response.hovered() || resizing {
            ui.visuals().widgets.active.bg_stroke.color
        } else {
            ui.visuals().widgets.noninteractive.bg_stroke.color
        };
        ui.painter().line_segment(
            [
                egui::pos2(rect.center().x, rect.top()),
                egui::pos2(rect.center().x, rect.bottom()),
            ],
            egui::Stroke::new(1.0, stroke_color),
        );
    }

    fn markdown_page_nav_ui(&mut self, ui: &mut egui::Ui, document: &MarkdownDocument) {
        ui.label(self.markdown_focus_label(ui, "Pages", MarkdownFocusTarget::Pages));
        ui.add_space(4.0);

        ScrollArea::vertical()
            .id_salt(format!("markdown_pages_{:?}", self.pane))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if document.pages.is_empty() {
                    ui.label(RichText::new("No pages").weak());
                    return;
                }

                for page in &document.pages {
                    let indent = (page.depth.saturating_sub(1) as f32) * 12.0;
                    let selected =
                        self.markdown_active_page_id.as_deref() == Some(page.id.as_str());
                    let row_height = ui.text_style_height(&TextStyle::Body) + 4.0;
                    let (row_rect, row_response) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), row_height),
                        egui::Sense::click(),
                    );
                    if selected {
                        ui.painter()
                            .rect_filled(row_rect, 2.0, ui.visuals().selection.bg_fill);
                    }

                    let text_rect = egui::Rect::from_min_max(
                        egui::pos2(row_rect.left() + indent, row_rect.top()),
                        row_rect.right_bottom(),
                    );
                    let font_id = TextStyle::Body.resolve(ui.style());
                    let text = ellipsize_text_to_width(
                        ui,
                        &page.title,
                        font_id.clone(),
                        text_rect.width().max(0.0),
                    );
                    let color = if selected {
                        ui.visuals().selection.stroke.color
                    } else {
                        ui.visuals().text_color()
                    };
                    ui.painter().with_clip_rect(text_rect).text(
                        text_rect.left_center(),
                        egui::Align2::LEFT_CENTER,
                        text,
                        font_id,
                        color,
                    );

                    if row_response.on_hover_text(&page.title).clicked() {
                        self.select_markdown_page(page.id.clone());
                        if self.markdown_mode == MarkdownMode::Both {
                            self.move_markdown_source_to_page(page);
                        } else {
                            self.request_markdown_focus(ui, MarkdownFocusTarget::Body);
                        }
                    }
                }
            });
    }

    fn markdown_body_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        language_registry: &LanguageRegistry,
        active_search_match: Option<MarkdownSearchMatch>,
    ) {
        let Some(page) = document.active_or_first_page(self.markdown_active_page_id.as_deref())
        else {
            ui.label(RichText::new("Empty Markdown document").weak());
            return;
        };
        let search_query = active_search_match
            .as_ref()
            .filter(|search_match| search_match.page_id == page.id)
            .map(|_| self.markdown_search.query.trim().to_owned());

        self.markdown_page_header_ui(ui, document, page, search_query.as_deref());
        ui.add_space(6.0);

        ScrollArea::vertical()
            .id_salt(format!("markdown_body_{:?}", self.pane))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let body_id = self.markdown_focus_id(ui, MarkdownFocusTarget::Body);
                let body_response = ui.interact(ui.max_rect(), body_id, egui::Sense::click());
                if body_response.clicked() {
                    self.request_markdown_focus(ui, MarkdownFocusTarget::Body);
                }
                let page_body_start = markdown_page_body_start_offset(document, page);
                let page_source =
                    document.source_text_between(page_body_start, page.source_range.end.offset);
                let outline = document.page_outline(&page.id);
                if page_source.trim().is_empty() {
                    ui.label(RichText::new("No rendered text for this page.").weak());
                } else if outline.is_empty() {
                    self.markdown_render_source_fragment_ui(
                        ui,
                        document,
                        language_registry,
                        page_body_start,
                        page_source,
                        search_query.as_deref(),
                    );
                } else {
                    self.markdown_body_sections_ui(
                        ui,
                        document,
                        language_registry,
                        page,
                        &outline,
                        search_query.as_deref(),
                    );
                }
                self.markdown_footnotes_ui(
                    ui,
                    document,
                    language_registry,
                    page,
                    search_query.as_deref(),
                );
            });
    }

    fn markdown_body_sections_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        language_registry: &LanguageRegistry,
        page: &crate::core::markdown::MarkdownPage,
        outline: &[&crate::core::markdown::MarkdownHeading],
        search_query: Option<&str>,
    ) {
        let page_body_start = markdown_page_body_start_offset(document, page);
        if let Some(first_heading) = outline.first() {
            self.markdown_render_source_fragment_ui(
                ui,
                document,
                language_registry,
                page_body_start,
                document
                    .source_text_between(page_body_start, first_heading.source_range.start.offset),
                search_query,
            );
        }

        for (index, heading) in outline.iter().enumerate() {
            let selected =
                self.markdown_active_heading_id.as_deref() == Some(heading.slug.as_str());
            ui.add_space(4.0);
            let heading_job = markdown_plain_text_layout_job(
                ui,
                &heading.title,
                search_query,
                markdown_heading_text_format(ui, heading.level),
            );
            let response = ui.selectable_label(selected, heading_job);
            if response.clicked() {
                self.markdown_active_heading_id = Some(heading.slug.clone());
            }
            if selected {
                response.scroll_to_me(Some(egui::Align::Center));
            } else if self.should_scroll_to_markdown_search_match(search_query, &heading.title) {
                response.scroll_to_me(Some(egui::Align::Center));
            }

            let section_end = outline
                .get(index + 1)
                .map(|next| next.source_range.start.offset)
                .unwrap_or(page.source_range.end.offset);
            self.markdown_render_source_fragment_ui(
                ui,
                document,
                language_registry,
                heading.source_range.end.offset,
                document.source_text_between(heading.source_range.end.offset, section_end),
                search_query,
            );
        }
    }

    fn markdown_render_source_fragment_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        language_registry: &LanguageRegistry,
        fragment_start_offset: usize,
        source: &str,
        search_query: Option<&str>,
    ) {
        let Ok(ast) = markdown::to_mdast(source, &ParseOptions::gfm()) else {
            let text = MarkdownDocument::rendered_text_for_source(source);
            if !text.trim().is_empty() {
                let job = markdown_plain_text_layout_job(
                    ui,
                    text.trim(),
                    search_query,
                    markdown_inline_format(ui, MarkdownInlineStyle::default()),
                );
                let response = ui.add(egui::Label::new(job).wrap());
                if self.should_scroll_to_markdown_search_match(search_query, text.trim()) {
                    response.scroll_to_me(Some(egui::Align::Center));
                }
            }
            return;
        };

        self.markdown_render_node_ui(
            ui,
            document,
            language_registry,
            &ast,
            fragment_start_offset,
            0,
            0,
            search_query,
        );
    }

    fn markdown_render_nodes_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        language_registry: &LanguageRegistry,
        nodes: &[Node],
        fragment_start_offset: usize,
        depth: usize,
        ordered_start: u32,
        search_query: Option<&str>,
    ) {
        for (index, node) in nodes.iter().enumerate() {
            self.markdown_render_node_ui(
                ui,
                document,
                language_registry,
                node,
                fragment_start_offset,
                depth,
                ordered_start + index as u32,
                search_query,
            );
        }
    }

    fn markdown_render_node_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        language_registry: &LanguageRegistry,
        node: &Node,
        fragment_start_offset: usize,
        depth: usize,
        list_index: u32,
        search_query: Option<&str>,
    ) {
        match node {
            Node::Root(root) => self.markdown_render_nodes_ui(
                ui,
                document,
                language_registry,
                &root.children,
                fragment_start_offset,
                depth,
                0,
                search_query,
            ),
            Node::Paragraph(paragraph) => {
                let plain_text = markdown_inline_plain_text(&paragraph.children);
                if markdown_nodes_need_widgets(&paragraph.children) {
                    let response = ui
                        .horizontal_wrapped(|ui| {
                            self.markdown_inline_widgets_ui(
                                ui,
                                document,
                                &paragraph.children,
                                MarkdownInlineStyle::default(),
                                search_query,
                            );
                        })
                        .response;
                    if self.should_scroll_to_markdown_search_match(search_query, &plain_text) {
                        response.scroll_to_me(Some(egui::Align::Center));
                    }
                    ui.add_space(4.0);
                } else {
                    let job = markdown_inline_layout_job(ui, &paragraph.children, search_query);
                    if !job.text.trim().is_empty() {
                        let response = ui.add(egui::Label::new(job).wrap());
                        if self.should_scroll_to_markdown_search_match(search_query, &plain_text) {
                            response.scroll_to_me(Some(egui::Align::Center));
                        }
                        ui.add_space(4.0);
                    }
                }
            }
            Node::Heading(heading) => {
                let title = markdown_inline_plain_text(&heading.children);
                let job = markdown_plain_text_layout_job(
                    ui,
                    &title,
                    search_query,
                    markdown_heading_text_format(ui, heading.depth),
                );
                let response = ui.add(egui::Label::new(job).wrap());
                if self.should_scroll_to_markdown_search_match(search_query, &title) {
                    response.scroll_to_me(Some(egui::Align::Center));
                }
                ui.add_space(4.0);
            }
            Node::Blockquote(blockquote) => {
                egui::Frame::new()
                    .fill(markdown_blockquote_background(ui))
                    .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        self.markdown_render_nodes_ui(
                            ui,
                            document,
                            language_registry,
                            &blockquote.children,
                            fragment_start_offset,
                            depth + 1,
                            0,
                            search_query,
                        );
                    });
                ui.add_space(6.0);
            }
            Node::List(list) => {
                let start = list.start.unwrap_or(1);
                for (index, child) in list.children.iter().enumerate() {
                    let marker = if list.ordered {
                        start + index as u32
                    } else {
                        0
                    };
                    self.markdown_render_node_ui(
                        ui,
                        document,
                        language_registry,
                        child,
                        fragment_start_offset,
                        depth,
                        marker,
                        search_query,
                    );
                }
                ui.add_space(4.0);
            }
            Node::ListItem(item) => {
                ui.horizontal_top(|ui| {
                    ui.add_space(depth as f32 * 14.0);
                    if let Some(checked) = item.checked {
                        let mut checked = checked;
                        ui.add_enabled(false, egui::Checkbox::new(&mut checked, ""));
                    } else if list_index > 0 {
                        ui.label(RichText::new(format!("{list_index}.")).monospace());
                    } else {
                        ui.label(RichText::new("-").monospace());
                    }
                    ui.vertical(|ui| {
                        self.markdown_render_nodes_ui(
                            ui,
                            document,
                            language_registry,
                            &item.children,
                            fragment_start_offset,
                            depth + 1,
                            0,
                            search_query,
                        );
                    });
                });
            }
            Node::ThematicBreak(_) => {
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 10.0),
                    egui::Sense::hover(),
                );
                ui.painter().line_segment(
                    [
                        egui::pos2(rect.left(), rect.center().y),
                        egui::pos2(rect.right(), rect.center().y),
                    ],
                    egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                );
                ui.add_space(6.0);
            }
            Node::Code(code) => {
                let code_id = markdown_node_ui_id(fragment_start_offset, node, "code");
                if code.lang.as_deref().is_some_and(is_mermaid_language) {
                    self.markdown_mermaid_block_ui(ui, language_registry, &code_id, &code.value);
                } else {
                    self.markdown_code_block_ui(
                        ui,
                        language_registry,
                        &code_id,
                        code.lang.as_deref(),
                        &code.value,
                    );
                }
            }
            Node::Table(table) => {
                let table_id = markdown_node_ui_id(fragment_start_offset, node, "table");
                self.markdown_table_ui(ui, &table_id, &table.children, &table.align, search_query);
            }
            Node::Image(image) => {
                self.markdown_image_ui(ui, document, &image.url, &image.alt);
            }
            Node::ImageReference(image) => {
                self.markdown_image_ui(ui, document, &image.identifier, &image.alt);
            }
            Node::Html(html) => {
                if !html.value.trim().is_empty() {
                    ui.label(RichText::new(&html.value).monospace().weak());
                    ui.add_space(4.0);
                }
            }
            Node::FootnoteDefinition(_) => {}
            _ => {
                if let Some(children) = node.children() {
                    self.markdown_render_nodes_ui(
                        ui,
                        document,
                        language_registry,
                        children,
                        fragment_start_offset,
                        depth,
                        0,
                        search_query,
                    );
                }
            }
        }
    }

    fn markdown_inline_widgets_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        children: &[Node],
        style: MarkdownInlineStyle,
        search_query: Option<&str>,
    ) {
        for child in children {
            match child {
                Node::Text(text) => {
                    let job = markdown_plain_text_layout_job(
                        ui,
                        &text.value,
                        search_query,
                        markdown_inline_format(ui, style),
                    );
                    if !job.text.is_empty() {
                        ui.add(egui::Label::new(job).wrap());
                    }
                }
                Node::InlineCode(code) => {
                    let mut format = markdown_inline_format(ui, style);
                    format.font_id = editor_font_id();
                    format.background = markdown_code_background(ui);
                    format.expand_bg = 3.0;
                    let job = markdown_plain_text_layout_job(ui, &code.value, search_query, format);
                    ui.add(egui::Label::new(job).wrap());
                }
                Node::Break(_) => {
                    ui.end_row();
                }
                Node::Emphasis(emphasis) => {
                    let mut style = style;
                    style.emphasis = true;
                    self.markdown_inline_widgets_ui(
                        ui,
                        document,
                        &emphasis.children,
                        style,
                        search_query,
                    );
                }
                Node::Strong(strong) => {
                    let mut style = style;
                    style.strong = true;
                    self.markdown_inline_widgets_ui(
                        ui,
                        document,
                        &strong.children,
                        style,
                        search_query,
                    );
                }
                Node::Delete(delete) => {
                    let mut style = style;
                    style.delete = true;
                    self.markdown_inline_widgets_ui(
                        ui,
                        document,
                        &delete.children,
                        style,
                        search_query,
                    );
                }
                Node::Link(link) => {
                    let label = markdown_inline_plain_text(&link.children);
                    let label = if label.trim().is_empty() {
                        link.url.as_str()
                    } else {
                        label.as_str()
                    };
                    let mut job = LayoutJob::default();
                    let mut link_style = style;
                    link_style.link = true;
                    append_markdown_text_with_search(
                        ui,
                        &mut job,
                        label,
                        markdown_inline_format(ui, link_style),
                        search_query,
                    );
                    let response = ui
                        .add(egui::Label::new(job).sense(egui::Sense::click()).wrap())
                        .on_hover_text(&link.url);
                    if response.hovered() {
                        ui.output_mut(|output| output.cursor_icon = egui::CursorIcon::PointingHand);
                    }
                    if response.clicked() {
                        self.activate_markdown_link(document, &link.url);
                    }
                }
                Node::LinkReference(link) => {
                    let label = markdown_inline_plain_text(&link.children);
                    let mut job = LayoutJob::default();
                    let mut link_style = style;
                    link_style.link = true;
                    append_markdown_text_with_search(
                        ui,
                        &mut job,
                        &label,
                        markdown_inline_format(ui, link_style),
                        search_query,
                    );
                    let response = ui
                        .add(egui::Label::new(job).sense(egui::Sense::click()).wrap())
                        .on_hover_text(&link.identifier);
                    if response.hovered() {
                        ui.output_mut(|output| output.cursor_icon = egui::CursorIcon::PointingHand);
                    }
                    if response.clicked() {
                        self.pending_action = Some(EditorAction::ShowStatus(format!(
                            "Reference links are not resolved yet: {}",
                            link.identifier
                        )));
                    }
                }
                Node::Image(image) => {
                    self.markdown_image_ui(ui, document, &image.url, &image.alt);
                }
                Node::ImageReference(image) => {
                    self.markdown_image_ui(ui, document, &image.identifier, &image.alt);
                }
                Node::FootnoteReference(footnote) => {
                    let number = markdown_footnote_number(document, &footnote.identifier)
                        .map(|number| number.to_string())
                        .unwrap_or_else(|| {
                            footnote
                                .label
                                .clone()
                                .unwrap_or_else(|| footnote.identifier.clone())
                        });
                    let response = ui
                        .link(RichText::new(format!("[{number}]")).small().raised())
                        .on_hover_text(format!("Jump to footnote {}", footnote.identifier));
                    if response.clicked() {
                        self.markdown_pending_footnote_scroll = Some(
                            MarkdownFootnoteScrollTarget::Definition(footnote.identifier.clone()),
                        );
                    }
                    if self
                        .markdown_pending_footnote_scroll
                        .as_ref()
                        .is_some_and(|target| matches!(target, MarkdownFootnoteScrollTarget::Reference(identifier) if identifier == &footnote.identifier))
                    {
                        response.scroll_to_me(Some(egui::Align::Center));
                        self.markdown_pending_footnote_scroll = None;
                    }
                }
                Node::InlineMath(math) => {
                    let mut format = markdown_inline_format(ui, style);
                    format.font_id = editor_font_id();
                    let job = markdown_plain_text_layout_job(ui, &math.value, search_query, format);
                    ui.add(egui::Label::new(job).wrap());
                }
                _ => {
                    if let Some(children) = child.children() {
                        self.markdown_inline_widgets_ui(
                            ui,
                            document,
                            children,
                            style,
                            search_query,
                        );
                    }
                }
            }
        }
    }

    fn activate_markdown_link(&mut self, document: &MarkdownDocument, target: &str) {
        let target = target.trim();
        if target.is_empty() {
            self.pending_action = Some(EditorAction::ShowStatus(
                "Markdown link target is empty.".to_owned(),
            ));
            return;
        }
        if is_external_markdown_link(target) {
            self.pending_action = Some(EditorAction::OpenExternalUrl(target.to_owned()));
            return;
        }

        let (path_part, fragment) = split_markdown_link_target(target);
        let path = if path_part.is_empty() {
            document.path.clone()
        } else {
            resolve_markdown_link_path(&document.path, &path_part)
        };

        self.pending_action = Some(EditorAction::OpenMarkdownLink { path, fragment });
    }

    fn should_scroll_to_markdown_search_match(
        &mut self,
        search_query: Option<&str>,
        text: &str,
    ) -> bool {
        let Some(query) = search_query.filter(|query| !query.is_empty()) else {
            return false;
        };
        if self.markdown_search.scrolled_match == Some(self.markdown_search.current_match) {
            return false;
        }
        if text.to_lowercase().contains(&query.to_lowercase()) {
            self.markdown_search.scrolled_match = Some(self.markdown_search.current_match);
            return true;
        }
        false
    }

    fn markdown_code_block_ui(
        &mut self,
        ui: &mut egui::Ui,
        language_registry: &LanguageRegistry,
        block_id: &str,
        language: Option<&str>,
        code: &str,
    ) {
        egui::Frame::new()
            .fill(markdown_code_background(ui))
            .stroke(egui::Stroke::new(
                1.0,
                ui.visuals().widgets.noninteractive.bg_stroke.color,
            ))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(language.unwrap_or("code"))
                            .small()
                            .monospace()
                            .weak(),
                    );
                    let copy_response = ui.small_button("Copy").on_hover_text("Copy code block");
                    if copy_response.clicked()
                        || (copy_response.has_focus() && ui.input_mut(consume_copy_shortcut))
                    {
                        copy_code_block_to_clipboard(ui, code);
                    }
                });
                ui.separator();
                ScrollArea::horizontal()
                    .id_salt(format!("markdown_code_{:?}_{block_id}", self.pane))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        let path = language.and_then(markdown_code_path_for_language);
                        let mut job =
                            syntax_layout_job(ui, path.as_deref(), code, language_registry, None);
                        job.wrap.max_width = f32::INFINITY;
                        ui.label(job);
                    });
            });
        ui.add_space(8.0);
    }

    fn markdown_mermaid_block_ui(
        &mut self,
        ui: &mut egui::Ui,
        language_registry: &LanguageRegistry,
        block_id: &str,
        code: &str,
    ) {
        let render_result = self
            .markdown_mermaid_cache
            .entry(code.to_owned())
            .or_insert_with(|| render_mermaid_svg(code))
            .clone();

        let Ok(svg) = render_result else {
            self.markdown_code_block_ui(ui, language_registry, block_id, Some("mermaid"), code);
            if let Some(Err(error)) = self.markdown_mermaid_cache.get(code) {
                ui.label(
                    RichText::new(format!("Mermaid rendering failed: {error}"))
                        .small()
                        .weak(),
                );
            }
            return;
        };

        egui::Frame::new()
            .fill(markdown_code_background(ui))
            .stroke(egui::Stroke::new(
                1.0,
                ui.visuals().widgets.noninteractive.bg_stroke.color,
            ))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("mermaid").small().monospace().weak());
                    let copy_response =
                        ui.small_button("Copy").on_hover_text("Copy Mermaid source");
                    if copy_response.clicked()
                        || (copy_response.has_focus() && ui.input_mut(consume_copy_shortcut))
                    {
                        copy_code_block_to_clipboard(ui, code);
                    }
                });
                ui.separator();
                ScrollArea::horizontal()
                    .id_salt(format!("markdown_mermaid_{:?}_{block_id}", self.pane))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        let uri = format!("bytes://markdown_mermaid_{}.svg", stable_hash(&svg));
                        ui.add(egui::Image::from_bytes(uri, svg.into_bytes()));
                    });
            });
        ui.add_space(8.0);
    }

    fn markdown_footnotes_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        language_registry: &LanguageRegistry,
        page: &crate::core::markdown::MarkdownPage,
        search_query: Option<&str>,
    ) {
        let definitions = markdown_footnote_definitions_for_page(document, page);
        if definitions.is_empty() {
            return;
        }

        ui.add_space(12.0);
        ui.separator();
        ui.label(RichText::new("Footnotes").strong());
        ui.add_space(4.0);

        for definition in definitions {
            let number = markdown_footnote_number(document, &definition.identifier).unwrap_or(0);
            let response = ui
                .horizontal_top(|ui| {
                    ui.label(RichText::new(format!("{number}.")).small().monospace());
                    ui.vertical(|ui| {
                        self.markdown_render_nodes_ui(
                            ui,
                            document,
                            language_registry,
                            &definition.children,
                            definition
                                .position
                                .as_ref()
                                .map(|position| position.start.offset)
                                .unwrap_or(page.source_range.start.offset),
                            0,
                            0,
                            search_query,
                        );
                    });
                    if ui
                        .small_button("Back")
                        .on_hover_text("Back to footnote reference")
                        .clicked()
                    {
                        self.markdown_pending_footnote_scroll = Some(
                            MarkdownFootnoteScrollTarget::Reference(definition.identifier.clone()),
                        );
                    }
                })
                .response;
            if self
                .markdown_pending_footnote_scroll
                .as_ref()
                .is_some_and(|target| matches!(target, MarkdownFootnoteScrollTarget::Definition(identifier) if identifier == &definition.identifier))
            {
                response.scroll_to_me(Some(egui::Align::Center));
                self.markdown_pending_footnote_scroll = None;
            }
        }
        ui.add_space(8.0);
    }

    fn markdown_table_ui(
        &self,
        ui: &mut egui::Ui,
        table_id: &str,
        rows: &[Node],
        align: &[AlignKind],
        search_query: Option<&str>,
    ) {
        ScrollArea::horizontal()
            .id_salt(format!("markdown_table_{:?}_{table_id}", self.pane))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new(format!("markdown_table_grid_{:?}_{table_id}", self.pane))
                    .striped(true)
                    .spacing(egui::vec2(14.0, 6.0))
                    .show(ui, |ui| {
                        for (row_index, row) in rows.iter().enumerate() {
                            let Node::TableRow(row) = row else {
                                continue;
                            };
                            for (cell_index, cell) in row.children.iter().enumerate() {
                                let Node::TableCell(cell) = cell else {
                                    continue;
                                };
                                let mut job =
                                    markdown_inline_layout_job(ui, &cell.children, search_query);
                                if row_index == 0 {
                                    for section in &mut job.sections {
                                        section.format.color = ui.visuals().strong_text_color();
                                    }
                                }
                                ui.with_layout(
                                    markdown_table_cell_layout(
                                        align.get(cell_index).copied().unwrap_or(AlignKind::None),
                                    ),
                                    |ui| {
                                        ui.label(job);
                                    },
                                );
                            }
                            ui.end_row();
                        }
                    });
            });
        ui.add_space(8.0);
    }

    fn markdown_image_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        url: &str,
        alt: &str,
    ) {
        let Some(path) = markdown_resolve_local_image_path(document, url) else {
            ui.label(RichText::new(format!("[image: {alt}] {url}")).weak());
            return;
        };

        let result = self
            .markdown_image_cache
            .entry(path.clone())
            .or_insert_with(|| load_markdown_image_texture(ui.ctx(), &path));

        match result {
            Ok(texture) => {
                let response = ui.add(
                    egui::Image::from_texture(&*texture)
                        .max_width(ui.available_width())
                        .maintain_aspect_ratio(true),
                );
                if !alt.trim().is_empty() {
                    response.on_hover_text(alt);
                }
            }
            Err(error) => {
                ui.label(RichText::new(format!("[image: {alt}] {error}")).weak());
            }
        }
        ui.add_space(6.0);
    }

    fn markdown_page_header_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        page: &crate::core::markdown::MarkdownPage,
        search_query: Option<&str>,
    ) {
        let Some(index) = document.page_index_by_id(&page.id) else {
            return;
        };
        let previous = index
            .checked_sub(1)
            .and_then(|index| document.pages.get(index));
        let next = document.pages.get(index + 1);

        ui.horizontal(|ui| {
            let mut title_format = markdown_heading_text_format(ui, 2);
            if self.markdown_focus_target == MarkdownFocusTarget::Body {
                title_format.color = markdown_focus_color(ui);
            }
            let title_job =
                markdown_plain_text_layout_job(ui, &page.title, search_query, title_format);
            let title_response = ui.add(egui::Label::new(title_job));
            if self.should_scroll_to_markdown_search_match(search_query, &page.title) {
                title_response.scroll_to_me(Some(egui::Align::Center));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(next.is_some(), egui::Button::new("Next"))
                    .on_hover_text("Go to next page")
                    .clicked()
                    && let Some(next) = next
                {
                    self.select_markdown_page(next.id.clone());
                    if self.markdown_mode == MarkdownMode::Both {
                        self.move_markdown_source_to_page(next);
                    } else {
                        self.request_markdown_focus(ui, MarkdownFocusTarget::Body);
                    }
                }

                ui.label(RichText::new(format!(
                    "{}/{}",
                    index + 1,
                    document.pages.len()
                )));

                if ui
                    .add_enabled(previous.is_some(), egui::Button::new("Previous"))
                    .on_hover_text("Go to previous page")
                    .clicked()
                    && let Some(previous) = previous
                {
                    self.select_markdown_page(previous.id.clone());
                    if self.markdown_mode == MarkdownMode::Both {
                        self.move_markdown_source_to_page(previous);
                    } else {
                        self.request_markdown_focus(ui, MarkdownFocusTarget::Body);
                    }
                }
            });
        });
    }

    fn markdown_outline_ui(&mut self, ui: &mut egui::Ui, document: &MarkdownDocument) {
        ui.label(self.markdown_focus_label(ui, "Outline", MarkdownFocusTarget::Outline));
        ui.add_space(4.0);

        let Some(page) = document.active_or_first_page(self.markdown_active_page_id.as_deref())
        else {
            ui.label(RichText::new("No outline").weak());
            return;
        };

        ScrollArea::vertical()
            .id_salt(format!("markdown_outline_{:?}", self.pane))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let outline = document.page_outline(&page.id);
                if outline.is_empty() {
                    ui.label(RichText::new("No headings").weak());
                    return;
                }

                for heading in outline {
                    ui.horizontal(|ui| {
                        ui.add_space((heading.level.saturating_sub(1) as f32) * 8.0);
                        let selected = self.markdown_active_heading_id.as_deref()
                            == Some(heading.slug.as_str());
                        if ui
                            .selectable_label(selected, RichText::new(&heading.title).small())
                            .clicked()
                        {
                            self.markdown_active_heading_id = Some(heading.slug.clone());
                            self.request_markdown_focus(ui, MarkdownFocusTarget::Body);
                            ui.ctx().request_repaint();
                        }
                    });
                }
            });
    }

    fn markdown_source_ui(
        &mut self,
        ui: &mut egui::Ui,
        focused: bool,
        tab_width: usize,
        active_page_range: Option<(u32, u32)>,
        document: Option<&MarkdownDocument>,
        bookmark_lines: &[u32],
    ) {
        let text_edit_id = ui.id().with(format!("markdown_source_{:?}", self.pane));
        let (source_len, source_hash) = self.text_fingerprint();
        let source_search_matches = if self.source_search.open {
            let matches = self.cached_source_search_matches(source_len, source_hash);
            self.source_search.match_count = matches.len();
            if self.source_search.current_match >= matches.len() {
                self.source_search.current_match = matches.len().saturating_sub(1);
                self.source_search.scrolled_match = None;
            }
            if focused {
                self.handle_source_search_global_shortcuts(ui, matches.as_slice());
            }
            self.source_search_ui(ui, matches.as_slice());
            matches
        } else {
            self.source_search.match_count = 0;
            Rc::new(Vec::new())
        };
        if self.source_search.open {
            ui.separator();
        }

        let pending_navigation = self.pending_navigation.take();
        let pending_position = pending_navigation.map(|navigation| navigation.position);
        let pending_focus_editor =
            pending_navigation.is_some_and(|navigation| navigation.focus_editor);
        let pending_scroll_y = pending_position.map(|position| {
            let row_height = editor_scroll_row_height(ui);
            (position.line as f32 * row_height - ui.available_height() * 0.35).max(0.0)
        });
        let viewport_height = ui.available_height();
        if let Some(offset) = pending_scroll_y {
            self.scroll_y = offset;
        }
        let metrics = self.cached_editor_metrics(source_len, source_hash);
        let editor_width = metrics.editor_width(ui, ui.available_width());
        let line_count = metrics.line_count;
        let estimated_max_scroll_y =
            estimated_textedit_source_max_scroll_y(ui, line_count, viewport_height);
        let max_scroll_y = if self.scroll_max_y > 0.0 {
            self.scroll_max_y
        } else {
            estimated_max_scroll_y
        };
        self.scroll_y = self.scroll_y.clamp(0.0, max_scroll_y);
        if let Some(delta) = editor_page_scroll_delta(ui, focused, text_edit_id, viewport_height) {
            self.scroll_y = (self.scroll_y + delta).clamp(0.0, max_scroll_y);
        }
        self.apply_immediate_wheel_scroll(ui, max_scroll_y);

        let scroll_area = ScrollArea::both()
            .id_salt(format!("markdown_source_scroll_{:?}", self.pane))
            .auto_shrink([false, false])
            .vertical_scroll_offset(self.scroll_y);

        let scroll_output = scroll_area.show(ui, |ui| {
            if let Some(target_position) = pending_position
                && let Some(char_index) = self.text.position_to_char_index(target_position)
            {
                let cursor = CCursor::new(char_index);
                let mut state = TextEditState::load(ui.ctx(), text_edit_id)
                    .unwrap_or_else(TextEditState::default);
                state.cursor.set_char_range(Some(CCursorRange::one(cursor)));
                state.store(ui.ctx(), text_edit_id);
                if pending_focus_editor {
                    ui.memory_mut(|memory| memory.request_focus(text_edit_id));
                }
                self.cursor_position = Some(target_position);
            }

            if focused && !self.read_only {
                self.handle_input_assistance(ui, text_edit_id, tab_width);
            }
            if focused
                && !self.read_only
                && !self.markdown_search.open
                && !self.source_search.open
                && !ui.memory(|memory| memory.has_focus(text_edit_id))
            {
                ui.memory_mut(|memory| memory.request_focus(text_edit_id));
            }

            ui.horizontal_top(|ui| {
                let gutter_rect = allocate_line_number_gutter(ui, line_count);

                let mut layouter = |ui: &egui::Ui, text: &dyn TextBuffer, _wrap_width: f32| {
                    let mut job = markdown_source_layout_job(ui, text.as_str());
                    job.wrap.max_width = f32::INFINITY;
                    ui.fonts_mut(|fonts| fonts.layout_job(job))
                };

                let output = TextEdit::multiline(&mut self.text)
                    .id(text_edit_id)
                    .font(TextStyle::Monospace)
                    .code_editor()
                    .frame(egui::Frame::NONE)
                    .desired_width(editor_width)
                    .desired_rows(TEXTEDIT_SOURCE_MIN_ROWS)
                    .layouter(&mut layouter)
                    .interactive(!self.read_only)
                    .show(ui);

                if let Some(cursor_range) = output.cursor_range {
                    let next_position =
                        self.text.char_index_to_position(cursor_range.primary.index);
                    if self.cursor_position != next_position {
                        self.cursor_position = next_position;
                        if let Some(document) = document {
                            self.update_markdown_active_page_from_cursor(document);
                        }
                        ui.ctx().request_repaint();
                    }
                }

                let current_line = self.cursor_position.map(|position| position.line);
                let row_height = editor_row_height(ui);
                let viewport_rect = ui.clip_rect();
                self.editor_render_debug = Some(capture_editor_render_debug(
                    &output,
                    line_count,
                    line_count,
                    row_height,
                    self.scroll_y,
                    viewport_rect,
                    current_line,
                ));
                paint_markdown_page_range_background(ui, &output, viewport_rect, active_page_range);
                paint_current_line_background(ui, &output, viewport_rect, current_line);
                if let Some(line) = paint_line_number_gutter(
                    ui,
                    gutter_rect,
                    line_count,
                    current_line,
                    bookmark_lines,
                    &output,
                    viewport_rect,
                ) && let Some(path) = self.path.clone()
                {
                    self.pending_action = Some(EditorAction::ToggleBookmark { path, line });
                }

                if output.response.changed() {
                    self.mark_text_changed();
                    if self.source_search.open {
                        self.source_search.scrolled_match = None;
                    }
                    self.target_position = None;
                    self.markdown_active_heading_id = None;
                    if let Some(document) = document {
                        self.update_markdown_active_page_from_cursor(document);
                    }
                }
            });
        });
        self.scroll_max_y =
            scroll_area_vertical_max_scroll_y(scroll_output.content_size, scroll_output.inner_rect);
        self.scroll_y = scroll_output.state.offset.y.clamp(0.0, self.scroll_max_y);
        drop(source_search_matches);
    }

    fn prepare_markdown_source_entry(&mut self, document: &MarkdownDocument) {
        let Some(page) = document.active_or_first_page(self.markdown_active_page_id.as_deref())
        else {
            return;
        };
        let position = Position::new(
            page.source_range.start.line as u32,
            page.source_range.start.column as u32,
        );
        self.pending_navigation = Some(PendingEditorNavigation {
            position,
            focus_editor: true,
        });
        self.cursor_position = Some(position);
    }

    fn markdown_active_source_line_range(&self, document: &MarkdownDocument) -> Option<(u32, u32)> {
        let page = document.active_or_first_page(self.markdown_active_page_id.as_deref())?;
        let start = page.source_range.start.line as u32;
        let mut end = page.source_range.end.line as u32;

        if page.source_range.end.column == 0 && end > start {
            end -= 1;
        }

        Some((start, end.max(start)))
    }

    fn update_markdown_active_page_from_cursor(&mut self, document: &MarkdownDocument) {
        let Some(position) = self.cursor_position else {
            return;
        };
        let cursor_line = position.line as usize;
        let Some(page) = document.pages.iter().find(|page| {
            let start = page.source_range.start.line;
            let mut end = page.source_range.end.line;
            if page.source_range.end.column == 0 && end > start {
                end -= 1;
            }
            (start..=end.max(start)).contains(&cursor_line)
        }) else {
            return;
        };

        if self.markdown_active_page_id.as_deref() != Some(page.id.as_str()) {
            self.markdown_active_heading_id = None;
            self.markdown_active_page_id = Some(page.id.clone());
        }
    }

    fn handle_markdown_preview_navigation(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
    ) {
        if document.pages.is_empty() {
            return;
        }

        let mut next_index = None;
        let mut next_heading_index = None;
        let mut focus_body = false;

        ui.input_mut(|input| {
            if self.markdown_focus_target == MarkdownFocusTarget::Pages {
                if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                    next_index = self.markdown_page_index_with_delta(document, -1);
                    return;
                }
                if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                    next_index = self.markdown_page_index_with_delta(document, 1);
                    return;
                }
                if input.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                    next_index = document
                        .active_or_first_page_index(self.markdown_active_page_id.as_deref());
                    focus_body = true;
                    return;
                }
            }

            if self.markdown_focus_target == MarkdownFocusTarget::Outline {
                if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                    next_heading_index = self.markdown_outline_index_with_delta(document, -1);
                    return;
                }
                if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                    next_heading_index = self.markdown_outline_index_with_delta(document, 1);
                    return;
                }
                if input.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                    focus_body = true;
                    return;
                }
            }

            if input.consume_key(egui::Modifiers::CTRL, egui::Key::PageUp) {
                next_index = self.markdown_page_index_with_delta(document, -1);
                focus_body = true;
                return;
            }
            if input.consume_key(egui::Modifiers::CTRL, egui::Key::PageDown) {
                next_index = self.markdown_page_index_with_delta(document, 1);
                focus_body = true;
            }
        });

        if let Some(next_index) = next_index
            && let Some(page) = document.pages.get(next_index)
        {
            self.select_markdown_page(page.id.clone());
            if self.markdown_mode == MarkdownMode::Both && focus_body {
                self.move_markdown_source_to_page(page);
            } else if focus_body {
                self.request_markdown_focus(ui, MarkdownFocusTarget::Body);
            }
            ui.ctx().request_repaint();
        }
        if let Some(next_heading_index) = next_heading_index
            && let Some(page) =
                document.active_or_first_page(self.markdown_active_page_id.as_deref())
            && let Some(heading) = document.page_outline(&page.id).get(next_heading_index)
        {
            self.markdown_active_heading_id = Some(heading.slug.clone());
            ui.ctx().request_repaint();
        }
        if focus_body && next_index.is_none() {
            self.request_markdown_focus(ui, MarkdownFocusTarget::Body);
        }
    }

    fn markdown_page_index_with_delta(
        &self,
        document: &MarkdownDocument,
        delta: isize,
    ) -> Option<usize> {
        let index = document.active_or_first_page_index(self.markdown_active_page_id.as_deref())?;
        let max_index = document.pages.len().saturating_sub(1) as isize;
        Some((index as isize + delta).clamp(0, max_index) as usize)
    }

    fn markdown_outline_index_with_delta(
        &self,
        document: &MarkdownDocument,
        delta: isize,
    ) -> Option<usize> {
        let page = document.active_or_first_page(self.markdown_active_page_id.as_deref())?;
        let outline = document.page_outline(&page.id);
        if outline.is_empty() {
            return None;
        }
        let index = self
            .markdown_active_heading_id
            .as_deref()
            .and_then(|heading_id| {
                outline
                    .iter()
                    .position(|heading| heading.slug.as_str() == heading_id)
            })
            .unwrap_or(0);
        let max_index = outline.len().saturating_sub(1) as isize;
        Some((index as isize + delta).clamp(0, max_index) as usize)
    }

    fn select_markdown_page(&mut self, page_id: String) {
        if self.markdown_active_page_id.as_deref() != Some(page_id.as_str()) {
            self.markdown_active_heading_id = None;
        }
        self.markdown_active_page_id = Some(page_id);
    }

    fn move_markdown_source_to_page(&mut self, page: &crate::core::markdown::MarkdownPage) {
        let position = Position::new(page.source_range.start.line as u32, 0);
        self.cursor_position = Some(position);
        self.target_position = Some(position);
        self.pending_navigation = Some(PendingEditorNavigation {
            position,
            focus_editor: false,
        });
        self.markdown_focus_target = MarkdownFocusTarget::Body;
    }

    fn markdown_focus_id(&self, ui: &egui::Ui, target: MarkdownFocusTarget) -> egui::Id {
        ui.id()
            .with(format!("markdown_focus_{:?}_{target:?}", self.pane))
    }

    fn request_markdown_focus(&mut self, ui: &mut egui::Ui, target: MarkdownFocusTarget) {
        self.markdown_focus_target = target;
        let focus_id = self.markdown_focus_id(ui, target);
        ui.memory_mut(|memory| memory.request_focus(focus_id));
    }

    fn visible_markdown_focus_targets(&self) -> Vec<MarkdownFocusTarget> {
        if self.markdown_mode == MarkdownMode::Markdown {
            return vec![MarkdownFocusTarget::Body];
        }

        let mut targets = Vec::new();
        if self.markdown_show_pages {
            targets.push(MarkdownFocusTarget::Pages);
        }
        targets.push(MarkdownFocusTarget::Body);
        if self.markdown_show_outline {
            targets.push(MarkdownFocusTarget::Outline);
        }
        targets
    }

    fn normalize_markdown_focus_target(&mut self) {
        let targets = self.visible_markdown_focus_targets();
        if !targets.contains(&self.markdown_focus_target) {
            self.markdown_focus_target = MarkdownFocusTarget::Body;
        }
    }

    fn markdown_focus_label(
        &self,
        ui: &egui::Ui,
        label: &'static str,
        target: MarkdownFocusTarget,
    ) -> RichText {
        let text = RichText::new(label).small().strong();
        if self.markdown_focus_target == target {
            text.color(markdown_focus_color(ui))
                .background_color(markdown_focus_background(ui))
        } else {
            text
        }
    }

    fn source_search_ui(&mut self, ui: &mut egui::Ui, matches: &[SourceSearchMatch]) {
        let search_id = ui.id().with(format!("source_search_{:?}", self.pane));
        let mut close = false;
        let mut go_next = false;
        let mut go_previous = false;
        let mut query_changed = false;
        let mut keep_focus = false;

        ui.horizontal(|ui| {
            ui.label(RichText::new("Find").small().strong());
            let input_had_focus = ui.memory(|memory| memory.has_focus(search_id));
            if input_had_focus {
                match ui.input(raw_search_enter_direction) {
                    Some(1) => {
                        go_next = true;
                        keep_focus = true;
                    }
                    Some(-1) => {
                        go_previous = true;
                        keep_focus = true;
                    }
                    _ => {}
                }
            }
            // egui drops focus at the start of the frame that carries a plain Escape, so check
            // who had focus in the previous frame.
            if ui.memory(|memory| memory.had_focus_last_frame(search_id))
                && ui.input(raw_escape_pressed)
            {
                close = true;
            }

            let response = ui.add(
                TextEdit::singleline(&mut self.source_search.query)
                    .id(search_id)
                    .hint_text("Search current file")
                    .desired_width(220.0),
            );
            if response.changed() {
                self.source_search.current_match = 0;
                self.source_search.scrolled_match = None;
                query_changed = true;
            }
            if self.source_search.focus_requested {
                response.request_focus();
                self.source_search.focus_requested = false;
            }
            let input_has_focus =
                response.has_focus() || ui.memory(|memory| memory.has_focus(search_id));
            if !input_had_focus && input_has_focus {
                match ui.input(raw_search_enter_direction) {
                    Some(1) => {
                        go_next = true;
                        keep_focus = true;
                    }
                    Some(-1) => {
                        go_previous = true;
                        keep_focus = true;
                    }
                    _ => {}
                }
                if ui.input(raw_escape_pressed) {
                    close = true;
                }
            }

            let count_label = if self.source_search.query.trim().is_empty() {
                "0 results".to_owned()
            } else if matches.is_empty() {
                "No results".to_owned()
            } else {
                format!("{}/{}", self.source_search.current_match + 1, matches.len())
            };
            ui.label(RichText::new(count_label).small().weak());

            if ui
                .add_enabled(!matches.is_empty(), egui::Button::new("Previous"))
                .on_hover_text("Previous search match")
                .clicked()
            {
                go_previous = true;
            }
            if ui
                .add_enabled(!matches.is_empty(), egui::Button::new("Next"))
                .on_hover_text("Next search match")
                .clicked()
            {
                go_next = true;
            }
            if ui.button("Close").on_hover_text("Close search").clicked() {
                close = true;
            }

            if keep_focus && !close {
                ui.memory_mut(|memory| memory.request_focus(search_id));
            }
        });

        if close {
            self.close_source_search();
            return;
        }

        if query_changed {
            return;
        }

        if go_next {
            self.move_source_search_match(matches, 1);
        } else if go_previous {
            self.move_source_search_match(matches, -1);
        } else {
            self.navigate_to_current_source_match(matches);
        }
    }

    fn handle_source_search_global_shortcuts(
        &mut self,
        ui: &mut egui::Ui,
        matches: &[SourceSearchMatch],
    ) {
        if matches.is_empty() {
            return;
        }

        let mut go_next = false;
        let mut go_previous = false;
        ui.input_mut(|input| {
            if input.key_pressed(egui::Key::F3) {
                if input.modifiers.shift {
                    go_previous = true;
                } else {
                    go_next = true;
                }
                let _ = input.consume_key(input.modifiers, egui::Key::F3);
            }
        });

        if go_next {
            self.move_source_search_match(matches, 1);
        } else if go_previous {
            self.move_source_search_match(matches, -1);
        }
    }

    fn move_source_search_match(&mut self, matches: &[SourceSearchMatch], delta: isize) {
        if matches.is_empty() {
            return;
        }

        let len = matches.len() as isize;
        let current = self.source_search.current_match as isize;
        self.source_search.current_match = (current + delta).rem_euclid(len) as usize;
        self.source_search.scrolled_match = None;
        self.navigate_to_current_source_match(matches);
    }

    fn navigate_to_current_source_match(&mut self, matches: &[SourceSearchMatch]) {
        if self.source_search.scrolled_match == Some(self.source_search.current_match) {
            return;
        }
        let Some(search_match) = matches.get(self.source_search.current_match) else {
            return;
        };
        if let Some(position) = self.text.byte_index_to_position(search_match.start) {
            self.pending_navigation = Some(PendingEditorNavigation {
                position,
                focus_editor: false,
            });
            self.cursor_position = Some(position);
            self.target_position = None;
            self.source_search.scrolled_match = Some(self.source_search.current_match);
        }
    }

    fn close_source_search(&mut self) {
        self.source_search.open = false;
        self.source_search.focus_requested = false;
        self.source_search.query.clear();
        self.source_search.current_match = 0;
        self.source_search.match_count = 0;
        self.source_search.scrolled_match = None;
    }

    fn source_editor_ui(
        &mut self,
        ui: &mut egui::Ui,
        focused: bool,
        language_registry: &LanguageRegistry,
        tab_width: usize,
        bookmark_lines: &[u32],
    ) {
        let total_start = Instant::now();
        let scroll_before_y = self.scroll_y;
        let (source_len, source_hash) = self.text_fingerprint();
        let source_search_matches = if self.source_search.open {
            let matches = self.cached_source_search_matches(source_len, source_hash);
            self.source_search.match_count = matches.len();
            if self.source_search.current_match >= matches.len() {
                self.source_search.current_match = matches.len().saturating_sub(1);
                self.source_search.scrolled_match = None;
            }
            if focused {
                self.handle_source_search_global_shortcuts(ui, matches.as_slice());
            }
            self.source_search_ui(ui, matches.as_slice());
            matches
        } else {
            self.source_search.match_count = 0;
            Rc::new(Vec::new())
        };
        let active_source_search_match = self
            .source_search
            .open
            .then_some(self.source_search.current_match);
        let source_search_match_count = source_search_matches.len();
        if self.source_search.open {
            ui.separator();
        }

        let pending_navigation = self.pending_navigation.take();
        let pending_position = pending_navigation.map(|navigation| navigation.position);
        let pending_focus_editor =
            pending_navigation.is_some_and(|navigation| navigation.focus_editor);
        let pending_scroll_y = pending_position.map(|position| {
            let row_height = editor_scroll_row_height(ui);
            (position.line as f32 * row_height - ui.available_height() * 0.35).max(0.0)
        });
        let text_edit_id = ui.id().with(format!("editor_text_{:?}", self.pane));
        let viewport_height = ui.available_height();
        if let Some(offset) = pending_scroll_y {
            self.scroll_y = offset;
        }
        let line_index = self.cached_line_index(source_len, source_hash);
        if should_use_virtual_source_view(source_len, line_index.line_count()) {
            self.virtual_source_editor_ui(
                ui,
                focused,
                text_edit_id,
                language_registry,
                tab_width,
                source_len,
                source_hash,
                source_search_matches,
                active_source_search_match,
                pending_position,
                bookmark_lines,
            );
            return;
        }
        let metrics = self.cached_editor_metrics(source_len, source_hash);
        let line_count = metrics.line_count;
        let editor_width = metrics.editor_width(ui, ui.available_width());
        let estimated_max_scroll_y =
            estimated_textedit_source_max_scroll_y(ui, line_count, viewport_height);
        let max_scroll_y = if self.scroll_max_y > 0.0 {
            self.scroll_max_y
        } else {
            estimated_max_scroll_y
        };
        self.scroll_y = self.scroll_y.clamp(0.0, max_scroll_y);
        if let Some(delta) = editor_page_scroll_delta(ui, focused, text_edit_id, viewport_height) {
            self.scroll_y = (self.scroll_y + delta).clamp(0.0, max_scroll_y);
        }
        self.apply_immediate_wheel_scroll(ui, max_scroll_y);

        let scroll_area = ScrollArea::both()
            .id_salt(format!("editor_pane_{:?}", self.pane))
            .auto_shrink([false, false])
            .vertical_scroll_offset(self.scroll_y);

        let body_start = Instant::now();
        let scroll_output = scroll_area.show(ui, |ui| {
            if let Some(target_position) = pending_position
                && let Some(char_index) = self.text.position_to_char_index(target_position)
            {
                let cursor = CCursor::new(char_index);
                let mut state = TextEditState::load(ui.ctx(), text_edit_id)
                    .unwrap_or_else(TextEditState::default);
                state.cursor.set_char_range(Some(CCursorRange::one(cursor)));
                state.store(ui.ctx(), text_edit_id);
                if pending_focus_editor {
                    ui.memory_mut(|memory| memory.request_focus(text_edit_id));
                }
                self.cursor_position = Some(target_position);
            }
            if focused && !self.read_only {
                self.handle_input_assistance(ui, text_edit_id, tab_width);
            }

            let bracket_pair = self.cached_bracket_pair(source_len, source_hash);
            let highlight_spans =
                self.cached_syntax_highlight_spans(ui, language_registry, source_len, source_hash);

            ui.horizontal_top(|ui| {
                let gutter_rect = allocate_line_number_gutter(ui, line_count);

                let mut layouter = move |ui: &egui::Ui, text: &dyn TextBuffer, _wrap_width: f32| {
                    let mut job = highlighted_layout_job(
                        ui,
                        text.as_str(),
                        highlight_spans.as_slice(),
                        bracket_pair,
                        source_search_matches.as_slice(),
                        active_source_search_match,
                    );
                    job.wrap.max_width = f32::INFINITY;
                    ui.fonts_mut(|fonts| fonts.layout_job(job))
                };

                let output = TextEdit::multiline(&mut self.text)
                    .id(text_edit_id)
                    .font(TextStyle::Monospace)
                    .code_editor()
                    .frame(egui::Frame::NONE)
                    .desired_width(editor_width)
                    .desired_rows(TEXTEDIT_SOURCE_MIN_ROWS)
                    .layouter(&mut layouter)
                    .interactive(!self.read_only)
                    .show(ui);

                if let Some(cursor_range) = output.cursor_range {
                    let next_position =
                        self.text.char_index_to_position(cursor_range.primary.index);
                    if self.cursor_position != next_position {
                        self.cursor_position = next_position;
                        ui.ctx().request_repaint();
                    }
                }

                let current_line = self.cursor_position.map(|position| position.line);
                let row_height = editor_row_height(ui);
                let viewport_rect = ui.clip_rect();
                self.editor_render_debug = Some(capture_editor_render_debug(
                    &output,
                    line_count,
                    line_count,
                    row_height,
                    self.scroll_y,
                    viewport_rect,
                    current_line,
                ));
                paint_current_line_background(ui, &output, viewport_rect, current_line);
                if !self.is_csv() {
                    paint_indent_guides(
                        ui,
                        &self.text,
                        tab_width,
                        current_line,
                        &output,
                        viewport_rect,
                    );
                }
                if let Some(line) = paint_line_number_gutter(
                    ui,
                    gutter_rect,
                    line_count,
                    current_line,
                    bookmark_lines,
                    &output,
                    viewport_rect,
                ) && let Some(path) = self.path.clone()
                {
                    self.pending_action = Some(EditorAction::ToggleBookmark { path, line });
                }

                if output.response.changed() {
                    self.mark_text_changed();
                    self.target_position = None;
                }

                output.response.context_menu(|ui| {
                    if ui.button("Go to Definition").clicked() {
                        self.pending_action = Some(EditorAction::GoToDefinition);
                        ui.close();
                    }
                    if ui.button("Back").clicked() {
                        self.pending_action = Some(EditorAction::Back);
                        ui.close();
                    }
                });
            });
        });
        self.scroll_max_y =
            scroll_area_vertical_max_scroll_y(scroll_output.content_size, scroll_output.inner_rect);
        self.scroll_y = scroll_output.state.offset.y.clamp(0.0, self.scroll_max_y);
        let body_ms = elapsed_ms(body_start);
        let visible_rows = self
            .editor_render_debug
            .as_ref()
            .and_then(|debug| debug.anchored_range.clone())
            .map(|range| range.end.saturating_sub(range.start))
            .unwrap_or(0);
        self.editor_perf_debug = Some(EditorPerfDebug {
            view: "source-textedit",
            total_ms: elapsed_ms(total_start),
            header_ms: 0.0,
            body_ms,
            visible_rows,
            total_rows: line_index.line_count(),
            text_len: source_len,
            scroll_y: self.scroll_y,
            scroll_x: self.scroll_x,
            scroll_delta_y: self.scroll_y - scroll_before_y,
            source_search_open: self.source_search.open,
            source_search_matches: source_search_match_count,
            syntax_spans: self
                .syntax_highlight_cache
                .as_ref()
                .map(|cache| cache.spans.len())
                .unwrap_or(0),
            virtual_cache_entries: self
                .virtual_line_layout_cache
                .as_ref()
                .map(|cache| cache.entries.len())
                .unwrap_or(0),
            line_number_cache_entries: self
                .virtual_line_number_layout_cache
                .as_ref()
                .map(|cache| cache.entries.len())
                .unwrap_or(0),
            ..Default::default()
        });
    }

    fn virtual_source_editor_ui(
        &mut self,
        ui: &mut egui::Ui,
        focused: bool,
        text_edit_id: egui::Id,
        language_registry: &LanguageRegistry,
        tab_width: usize,
        mut source_len: usize,
        mut source_hash: u64,
        source_search_matches: Rc<Vec<SourceSearchMatch>>,
        active_source_search_match: Option<usize>,
        pending_position: Option<Position>,
        bookmark_lines: &[u32],
    ) {
        let total_start = Instant::now();
        let scroll_before_y = self.scroll_y;
        if !focused {
            self.virtual_ime_preedit = None;
        }
        if let Some(position) = pending_position {
            self.cursor_position = Some(position);
            self.virtual_selection = None;
            self.virtual_ime_preedit = None;
        }
        if self.handle_virtual_text_input(ui, focused, text_edit_id, tab_width) {
            (source_len, source_hash) = self.text_fingerprint();
        }

        let line_index = self.cached_line_index(source_len, source_hash);
        let line_count = line_index.line_count();
        let row_height = editor_scroll_row_height(ui);
        let gutter_width = line_number_gutter_width(ui, line_count);
        let editor_width = line_index.editor_width(ui, ui.available_width());
        let total_width = gutter_width + editor_width;
        let total_height = line_count.max(1) as f32 * row_height;
        let mut current_line = self.cursor_position.map(|position| position.line);
        let bracket_pair = self.cached_bracket_pair(source_len, source_hash);
        let highlight_spans =
            self.cached_syntax_highlight_spans(ui, language_registry, source_len, source_hash);
        let highlight_line_span_ranges =
            self.cached_syntax_highlight_line_span_ranges(source_len, source_hash);

        let viewport_size = ui.available_size();
        let viewport_size = egui::vec2(viewport_size.x.max(1.0), viewport_size.y.max(1.0));
        let (viewport_rect, _) = ui.allocate_exact_size(viewport_size, egui::Sense::hover());
        // The viewport is the focusable widget behind `text_edit_id`: keys reach the editor
        // only while it holds focus, and the lock keeps Tab / arrows from moving focus away.
        let response = ui.interact(viewport_rect, text_edit_id, egui::Sense::click_and_drag());
        if response.has_focus() {
            let pressed_outside = ui.input(|input| {
                input.pointer.any_pressed()
                    && input
                        .pointer
                        .interact_pos()
                        .is_some_and(|position| !viewport_rect.contains(position))
            });
            if pressed_outside {
                response.surrender_focus();
            } else {
                ui.memory_mut(|memory| {
                    memory.set_focus_lock_filter(
                        text_edit_id,
                        egui::EventFilter {
                            tab: true,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            escape: false,
                        },
                    );
                });
            }
        }
        let scrollbar_layout = virtual_scrollbar_layout(viewport_rect, total_width, total_height);
        let content_rect = scrollbar_layout.content_rect;
        let max_scroll_y = (total_height - content_rect.height()).max(0.0);
        let max_scroll_x = (total_width - content_rect.width()).max(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, max_scroll_y);
        self.scroll_x = self.scroll_x.clamp(0.0, max_scroll_x);

        if response.hovered() {
            let (immediate_scroll, smooth_scroll, shift) = ui.input(|input| {
                (
                    immediate_wheel_scroll_delta(&input.raw.events, content_rect.height()),
                    input.smooth_scroll_delta(),
                    input.modifiers.shift,
                )
            });
            // Wheel notches are applied at once; egui smooths the same notch over the next
            // frames, so that later smoothed delta is skipped instead of scrolling twice.
            let scroll_delta = match immediate_scroll {
                Some(raw) => {
                    let raw = if shift {
                        egui::vec2(raw.x + raw.y, 0.0)
                    } else {
                        raw
                    };
                    self.virtual_wheel_lead += raw - smooth_scroll;
                    raw
                }
                None => consume_wheel_lead(&mut self.virtual_wheel_lead, smooth_scroll),
            };
            if scroll_delta != egui::Vec2::ZERO {
                self.scroll_y = (self.scroll_y - scroll_delta.y).clamp(0.0, max_scroll_y);
                self.scroll_x = (self.scroll_x - scroll_delta.x).clamp(0.0, max_scroll_x);
            }
            if smooth_scroll != egui::Vec2::ZERO {
                ui.input_mut(|input| {
                    input.smooth_scroll_delta = egui::Vec2::ZERO;
                });
                ui.ctx().request_repaint();
            }
        } else {
            self.virtual_wheel_lead = egui::Vec2::ZERO;
        }

        let visible_start = (self.scroll_y / row_height).floor().max(0.0) as usize;
        let visible_end = (((self.scroll_y + content_rect.height()) / row_height).ceil() as usize
            + 1)
        .min(line_count);
        let visible_range = visible_start.min(line_count)..visible_end;
        let visible_rows = visible_range.end.saturating_sub(visible_range.start);
        let painter = ui.painter_at(viewport_rect);
        let inactive_color = ui.visuals().weak_text_color();
        let active_color = ui.visuals().text_color();
        let char_width = editor_char_width(ui);
        let content_left = content_rect.left();
        let text_clip_left = content_left + gutter_width + 1.0;
        let text_clip_rect = egui::Rect::from_min_max(
            egui::pos2(text_clip_left, content_rect.top()),
            content_rect.right_bottom(),
        );
        let text_painter = ui.painter_at(text_clip_rect);
        let text_left = content_left + gutter_width + 8.0 - self.scroll_x;

        if self.handle_virtual_navigation_input(
            ui,
            focused,
            text_edit_id,
            line_index.as_ref(),
            row_height,
            content_rect.height(),
            text_clip_rect.width(),
            char_width,
            max_scroll_x,
            max_scroll_y,
        ) {
            current_line = self.cursor_position.map(|position| position.line);
            response.request_focus();
            ui.memory_mut(|memory| memory.request_focus(text_edit_id));
            ui.ctx().request_repaint();
        }

        if let Some((anchor, head)) = virtual_editor_drag_selection(
            ui,
            &response,
            line_index.as_ref(),
            &self.text,
            content_rect,
            text_clip_rect,
            content_rect.width(),
            content_rect.height(),
            text_left,
            row_height,
            self.scroll_y,
            char_width,
        ) {
            self.virtual_selection = Some(VirtualTextSelection { anchor, head });
            self.cursor_position = Some(head);
            self.virtual_ime_preedit = None;
            current_line = Some(head.line);
            self.target_position = None;
            response.request_focus();
            ui.memory_mut(|memory| memory.request_focus(text_edit_id));
            ui.ctx().request_repaint();
        }

        let click_input = virtual_editor_click_input(
            ui,
            &response,
            text_edit_id,
            focused,
            content_rect,
            text_clip_rect,
            content_rect.width(),
            content_rect.height(),
            self.scroll_x,
            self.scroll_y,
            row_height,
            line_count,
            visible_range.clone(),
        );
        let click_event_index = click_input
            .event
            .map(|event| self.virtual_click_debug.push_event(event));
        if let Some(pointer_pos) = click_input.position {
            let line = ((pointer_pos.y - content_rect.top() + self.scroll_y) / row_height)
                .floor()
                .max(0.0) as usize;
            let line = line.min(line_count.saturating_sub(1));
            if let Some(line_info) = line_index.line(line) {
                let line_text = self.text.line_text_or_empty(line_info);
                let character = virtual_click_byte_column(
                    Some(ui),
                    line_text,
                    pointer_pos.x,
                    text_left,
                    text_clip_rect.left(),
                    char_width,
                );
                let position = Position::new(line as u32, character);
                if click_input.double_clicked {
                    if let Some(selection) =
                        virtual_word_selection_at_position(line_text, line as u32, character)
                    {
                        self.cursor_position = Some(selection.head);
                        current_line = Some(selection.head.line);
                        self.virtual_selection = Some(selection);
                    } else {
                        self.cursor_position = Some(position);
                        current_line = Some(position.line);
                        self.virtual_selection = None;
                    }
                } else {
                    self.cursor_position = Some(position);
                    current_line = Some(position.line);
                    self.virtual_selection = None;
                }
                self.virtual_ime_preedit = None;
                self.target_position = None;
                if let Some(event_index) = click_event_index {
                    self.virtual_click_debug.last_accept = Some(VirtualClickAcceptDebug {
                        event_index,
                        pointer_pos,
                        line,
                        character,
                        cursor_position: position,
                        text_left,
                        text_clip_left: text_clip_rect.left(),
                        x_offset: virtual_click_x_offset(
                            pointer_pos.x,
                            text_left,
                            text_clip_rect.left(),
                        ),
                    });
                }
                response.request_focus();
                ui.memory_mut(|memory| memory.request_focus(text_edit_id));
                ui.ctx().request_repaint();
            }
        }

        self.editor_render_debug = Some(capture_virtual_editor_render_debug(
            line_count,
            row_height,
            self.scroll_y,
            content_rect,
            visible_range.clone(),
            current_line,
        ));
        response.context_menu(|ui| {
            if ui.button("Go to Definition").clicked() {
                self.pending_action = Some(EditorAction::GoToDefinition);
                ui.close();
            }
            if ui.button("Back").clicked() {
                self.pending_action = Some(EditorAction::Back);
                ui.close();
            }
        });

        let body_start = Instant::now();
        let mut frame_counters = VirtualFrameCounters::default();
        if let Some(current_line) = current_line
            && visible_range.contains(&(current_line as usize))
        {
            let top = content_rect.top() + current_line as f32 * row_height - self.scroll_y;
            let row_rect = egui::Rect::from_min_size(
                egui::pos2(content_rect.left(), top),
                egui::vec2(content_rect.width(), row_height),
            );
            painter.rect_filled(
                row_rect.intersect(content_rect),
                0.0,
                current_line_overlay(ui),
            );
        }

        if !self.is_csv() {
            paint_virtual_indent_guides(
                ui,
                line_index.as_ref(),
                &self.text,
                visible_range.clone(),
                tab_width,
                current_line,
                text_left,
                row_height,
                self.scroll_y,
                text_clip_rect,
            );
        }

        paint_virtual_selection(
            ui,
            line_index.as_ref(),
            &self.text,
            self.virtual_selection
                .and_then(|selection| selection.normalized()),
            visible_range.clone(),
            text_left,
            text_clip_rect,
            row_height,
            self.scroll_y,
            char_width,
            content_rect,
        );

        for line_number in visible_range {
            let Some(line_info) = line_index.line(line_number) else {
                continue;
            };
            let top = content_rect.top() + line_number as f32 * row_height - self.scroll_y;
            let is_current_line = current_line == Some(line_number as u32);
            let bookmark_rect = egui::Rect::from_min_max(
                egui::pos2(content_left, top),
                egui::pos2(
                    (content_left + BOOKMARK_GUTTER_WIDTH).min(content_rect.right()),
                    top + row_height,
                ),
            );
            let bookmark_response = ui.interact(
                bookmark_rect,
                text_edit_id.with(("virtual_bookmark_gutter", line_number)),
                egui::Sense::click(),
            );
            let bookmarked = bookmark_lines.contains(&(line_number as u32));
            if bookmarked || bookmark_response.hovered() || is_current_line {
                painter.text(
                    bookmark_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    if bookmarked { "★" } else { "☆" },
                    bookmark_gutter_font_id(row_height),
                    if bookmarked {
                        ui.visuals().selection.bg_fill
                    } else {
                        inactive_color
                    },
                );
            }
            if bookmark_response.clicked()
                && let Some(path) = self.path.clone()
            {
                self.pending_action = Some(EditorAction::ToggleBookmark {
                    path,
                    line: line_number as u32,
                });
            }
            let line_number_galley = self.cached_virtual_line_number_galley(
                ui,
                line_number,
                line_count,
                is_current_line,
                inactive_color,
                active_color,
                Some(&mut frame_counters),
            );
            painter.galley(
                egui::pos2(
                    content_left + gutter_width - 8.0 - line_number_galley.size().x,
                    top,
                ),
                line_number_galley,
                if is_current_line {
                    active_color
                } else {
                    inactive_color
                },
            );

            let galley = self.cached_virtual_line_galley(
                ui,
                line_number,
                line_info,
                source_len,
                source_hash,
                highlight_spans.as_slice(),
                highlight_line_span_ranges
                    .get(line_number)
                    .cloned()
                    .unwrap_or(0..0),
                bracket_pair,
                source_search_matches.as_slice(),
                active_source_search_match,
                Some(&mut frame_counters),
            );
            text_painter.galley(
                egui::pos2(text_left, top),
                galley,
                ui.visuals().text_color(),
            );
        }

        let cursor_rect = virtual_cursor_rect(
            Some(ui),
            line_index.as_ref(),
            &self.text,
            self.cursor_position,
            visible_start..visible_end,
            text_left,
            text_clip_rect,
            row_height,
            self.scroll_y,
            char_width,
            content_rect,
        );
        if focused && !self.read_only && virtual_editor_can_handle_key(ui, text_edit_id) {
            if let Some(cursor_rect) = cursor_rect {
                let to_global = ui
                    .ctx()
                    .layer_transform_to_global(ui.layer_id())
                    .unwrap_or_default();
                ui.output_mut(|output| {
                    output.ime = Some(egui::output::IMEOutput {
                        rect: to_global * viewport_rect,
                        cursor_rect: to_global * cursor_rect,
                    });
                });
            }
        }
        if response.hovered() && !self.read_only {
            ui.output_mut(|output| {
                output.mutable_text_under_cursor = true;
            });
        }

        paint_virtual_ime_preedit(
            ui,
            self.virtual_ime_preedit.as_deref(),
            cursor_rect,
            text_clip_rect,
            content_rect,
            row_height,
        );
        paint_virtual_cursor(
            ui,
            line_index.as_ref(),
            &self.text,
            self.cursor_position,
            visible_start..visible_end,
            text_left,
            text_clip_rect,
            row_height,
            self.scroll_y,
            char_width,
            content_rect,
        );

        painter.line_segment(
            [
                egui::pos2(content_left + gutter_width, content_rect.top()),
                egui::pos2(content_left + gutter_width, content_rect.bottom()),
            ],
            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        );

        handle_and_paint_virtual_scrollbars(
            ui,
            viewport_rect,
            total_width,
            total_height,
            &mut self.scroll_x,
            &mut self.scroll_y,
        );
        let body_ms = elapsed_ms(body_start);
        self.editor_perf_debug = Some(EditorPerfDebug {
            view: "source-virtual",
            total_ms: elapsed_ms(total_start),
            header_ms: 0.0,
            body_ms,
            visible_rows,
            total_rows: line_count,
            galley_hits: frame_counters.galley_hits,
            galley_misses: frame_counters.galley_misses,
            line_number_hits: frame_counters.line_number_hits,
            line_number_misses: frame_counters.line_number_misses,
            text_len: source_len,
            scroll_y: self.scroll_y,
            scroll_x: self.scroll_x,
            scroll_delta_y: self.scroll_y - scroll_before_y,
            source_search_open: self.source_search.open,
            source_search_matches: source_search_matches.len(),
            syntax_spans: highlight_spans.len(),
            virtual_cache_entries: self
                .virtual_line_layout_cache
                .as_ref()
                .map(|cache| cache.entries.len())
                .unwrap_or(0),
            line_number_cache_entries: self
                .virtual_line_number_layout_cache
                .as_ref()
                .map(|cache| cache.entries.len())
                .unwrap_or(0),
        });
    }

    fn debug_report_lines(&self, tab_width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        lines.push(format!(
            "{} editor: title={} tab_width={}",
            self.pane.label(),
            self.title,
            tab_width
        ));
        if let Some(path) = &self.path {
            lines.push(format!("{} path={}", self.pane.label(), path.display()));
        } else {
            lines.push(format!("{} path=none", self.pane.label()));
        }
        if let Some(position) = self.cursor_position {
            lines.push(format!(
                "{} cursor={}:{}",
                self.pane.label(),
                position.line + 1,
                position.character + 1
            ));
        }
        if let Some(position) = self.target_position {
            lines.push(format!(
                "{} definition_target={}:{}",
                self.pane.label(),
                position.line + 1,
                position.character + 1
            ));
        }
        if self.is_csv() {
            lines.push(format!(
                "{} csv_mode={}",
                self.pane.label(),
                self.csv_mode.label()
            ));
            if self.csv_mode == CsvMode::Grid
                && let Some(cell) = self.selected_csv_cell
            {
                lines.push(format!(
                    "{} csv_cell=R{}C{}",
                    self.pane.label(),
                    cell.row + 1,
                    cell.column + 1
                ));
            }
        }
        if self.is_markdown() {
            lines.push(format!(
                "{} markdown_mode={}",
                self.pane.label(),
                self.markdown_mode.label()
            ));
            if !self.markdown_search.query.is_empty() {
                lines.push(format!(
                    "{} markdown_search={}/{} query={}",
                    self.pane.label(),
                    self.markdown_search.current_match + 1,
                    self.markdown_search.match_count,
                    self.markdown_search.query
                ));
            }
        } else if self.source_search.open && !self.source_search.query.is_empty() {
            lines.push(format!(
                "{} source_search={}/{} query={}",
                self.pane.label(),
                self.source_search.current_match + 1,
                self.source_search.match_count,
                self.source_search.query
            ));
        }
        if let Some(debug) = &self.editor_render_debug {
            lines.push(format!(
                "{} render: lines={} text_lines={} galley_rows={} delta={}",
                self.pane.label(),
                debug.line_count,
                debug.text_line_count,
                debug.galley_rows,
                debug.galley_rows as isize - debug.text_line_count as isize
            ));
            lines.push(format!(
                "{} render_scroll: y={:.1} row_h={:.2} clip={:.1}..{:.1} viewport={:.1}..{:.1} galley_y={:.1}",
                self.pane.label(),
                debug.scroll_y,
                debug.row_height,
                debug.clip_top,
                debug.clip_bottom,
                debug.viewport_top,
                debug.viewport_bottom,
                debug.galley_pos_y
            ));
            lines.push(format!(
                "{} visible_rows: first={:?}@{:?} last={:?} anchor={:?}@{:?} range={:?} current_line={:?}",
                self.pane.label(),
                debug.first_visible_row,
                debug.first_visible_top.map(|value| format!("{value:.1}")),
                debug.last_visible_row,
                debug.anchor_line,
                debug.anchor_top.map(|value| format!("{value:.1}")),
                debug.anchored_range,
                debug.current_line
            ));
        }
        if let Some(perf) = &self.editor_perf_debug {
            lines.push(format!(
                "{} perf: view={} total={:.2}ms body={:.2}ms rows={}/{} scroll_delta_y={:.1}",
                self.pane.label(),
                perf.view,
                perf.total_ms,
                perf.body_ms,
                perf.visible_rows,
                perf.total_rows,
                perf.scroll_delta_y
            ));
            lines.push(format!(
                "{} perf_cache: galley={}/{} line_no={}/{} syntax={} search={}/{}",
                self.pane.label(),
                perf.galley_hits,
                perf.galley_misses,
                perf.line_number_hits,
                perf.line_number_misses,
                perf.syntax_spans,
                perf.source_search_open,
                perf.source_search_matches
            ));
        }
        if let Some(accept) = &self.virtual_click_debug.last_accept {
            lines.push(accept.report_line(self.pane.label()));
        } else {
            lines.push(format!("{} virtual_click_accept: none", self.pane.label()));
        }
        for event in self.virtual_click_debug.events.iter().rev().take(4) {
            lines.push(event.report_line(self.pane.label()));
        }
        lines
    }

    fn reload_current_file_diff_context(&mut self, context_mode: GitDiffContextMode) {
        let Some(diff_view) = self.diff_view.clone() else {
            return;
        };
        if !diff_view_can_switch_context(&diff_view) || diff_view.context_mode == context_mode {
            return;
        }
        let Some(root) = diff_view.repository_root.clone() else {
            self.pending_action = Some(EditorAction::ShowStatus(
                "Reload diff failed: repository root is unknown.".to_owned(),
            ));
            return;
        };
        let Some(path) = diff_view.file_path.clone() else {
            return;
        };
        match git_file_diff_text_for_path(
            &root,
            path.as_str(),
            diff_view.scope,
            context_mode,
            diff_view.untracked,
        ) {
            Ok(text) => {
                self.replace_text(text);
                self.scroll_y = 0.0;
                self.scroll_x = 0.0;
                self.diff_display_cache = None;
                self.diff_side_selection = None;
                self.diff_unified_selection = None;
                if let Some(diff_view) = &mut self.diff_view {
                    diff_view.context_mode = context_mode;
                }
                let label = match context_mode {
                    GitDiffContextMode::Changes => "Changes",
                    GitDiffContextMode::FullFile => "Full File",
                };
                self.pending_action = Some(EditorAction::ShowStatus(format!(
                    "Diff context: {label} | {path}"
                )));
            }
            Err(error) => {
                self.pending_action = Some(EditorAction::ShowStatus(format!(
                    "Reload diff failed: {path} | {error}"
                )));
            }
        }
    }

    fn diff_view_ui(&mut self, ui: &mut egui::Ui, focused: bool) {
        let return_history = self
            .diff_view
            .as_ref()
            .and_then(|diff_view| diff_view.return_history.clone());
        let mut back_to_history = false;
        let mut next_context_mode = None;
        if let Some(diff_view) = &self.diff_view {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&diff_view.repository_name).small().weak());
                ui.separator();
                let scope = match diff_view.scope {
                    GitDiffScope::All => "All Diffs",
                    GitDiffScope::Worktree => "Worktree",
                    GitDiffScope::Staged => "Staged",
                    GitDiffScope::Commit => "Commit",
                };
                ui.label(RichText::new(scope).small().weak());
                if let Some(path) = &diff_view.file_path {
                    ui.separator();
                    ui.label(RichText::new(path).small().monospace().weak());
                }
                if return_history.is_some() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Back to History").clicked() {
                            back_to_history = true;
                        }
                    });
                }
            });
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut self.diff_display_mode,
                    DiffDisplayMode::Unified,
                    "Unified",
                );
                ui.selectable_value(
                    &mut self.diff_display_mode,
                    DiffDisplayMode::SideBySide,
                    "Side by Side",
                );
                if diff_view_can_switch_context(diff_view) {
                    ui.separator();
                    if ui
                        .selectable_label(
                            diff_view.context_mode == GitDiffContextMode::Changes,
                            "Changes",
                        )
                        .clicked()
                    {
                        next_context_mode = Some(GitDiffContextMode::Changes);
                    }
                    if ui
                        .selectable_label(
                            diff_view.context_mode == GitDiffContextMode::FullFile,
                            "Full File",
                        )
                        .clicked()
                    {
                        next_context_mode = Some(GitDiffContextMode::FullFile);
                    }
                }
            });
            ui.separator();
        }
        if let Some(context_mode) = next_context_mode {
            self.reload_current_file_diff_context(context_mode);
        }
        if back_to_history && let Some(return_history) = return_history {
            let pane = self.pane;
            let mut next = EditorPane::from_git_history(pane, return_history.view);
            next.scroll_y = return_history.scroll_y;
            next.scroll_x = return_history.scroll_x;
            *self = next;
            return;
        }

        let (source_len, source_hash) = self.text_fingerprint();
        let rows = self.cached_diff_display_rows(source_len, source_hash);
        let display_mode = self.diff_display_mode;
        let side_by_side = display_mode == DiffDisplayMode::SideBySide;
        if side_by_side {
            self.diff_unified_selection = None;
        } else {
            self.diff_side_selection = None;
        }
        let side_scrollbar_height = if side_by_side { 20.0 } else { 0.0 };
        let viewport_height = (ui.available_height() - side_scrollbar_height).max(0.0);
        if let Some(delta) = diff_page_scroll_delta(ui, focused, viewport_height) {
            self.scroll_y = (self.scroll_y + delta).max(0.0);
        }
        self.apply_immediate_wheel_scroll(ui, self.scroll_max_y);

        let scroll_area = if side_by_side {
            ScrollArea::vertical()
        } else {
            ScrollArea::both()
        }
        .id_salt(format!("git_diff_view_{:?}", self.pane))
        .auto_shrink([false, false])
        .scroll_source(ScrollSource {
            drag: false,
            ..ScrollSource::ALL
        })
        .max_height(viewport_height)
        .vertical_scroll_offset(self.scroll_y);

        let side_scroll = DiffSideBySideScroll {
            left_x: self.diff_side_left_scroll_x,
            right_x: self.diff_side_right_scroll_x,
        };
        let scroll_output = scroll_area.show(ui, |ui| {
            if side_by_side {
                ui.set_min_width(ui.available_width());
            } else {
                ui.set_min_width(diff_view_min_width(rows.as_slice(), display_mode));
            }
            render_diff_rows(
                ui,
                rows.as_slice(),
                display_mode,
                &mut self.diff_collapsed_files,
                &mut self.diff_side_selection,
                &mut self.diff_unified_selection,
                self.scroll_y,
                side_scroll,
            )
        });
        if let Some(selection) = &mut self.diff_side_selection
            && !ui.input(|input| input.pointer.primary_down())
        {
            selection.dragging = false;
        }
        if let Some(selection) = &mut self.diff_unified_selection
            && !ui.input(|input| input.pointer.primary_down())
        {
            selection.dragging = false;
        }
        self.scroll_max_y =
            scroll_area_vertical_max_scroll_y(scroll_output.content_size, scroll_output.inner_rect);
        self.scroll_y = scroll_output.state.offset.y.clamp(0.0, self.scroll_max_y);
        if side_by_side {
            let viewport_width = scroll_output
                .inner_rect
                .intersect(ui.clip_rect())
                .width()
                .max(ui.available_width());
            diff_side_by_side_scrollbars_ui(
                ui,
                &mut self.diff_side_left_scroll_x,
                &mut self.diff_side_right_scroll_x,
                &scroll_output.inner,
                viewport_width,
            );
        }
        if self.diff_view.as_ref().is_some_and(|diff_view| {
            diff_view.file_path.is_some() && diff_view.scope != GitDiffScope::All
        }) {
            paint_diff_overview_markers(
                ui,
                scroll_output.inner_rect,
                &scroll_output.inner,
                editor_scroll_row_height(ui),
            );
        }
    }

    fn cached_diff_display_rows(
        &mut self,
        source_len: usize,
        source_hash: u64,
    ) -> Rc<Vec<DiffDisplayRow>> {
        if let Some(cache) = &self.diff_display_cache
            && cache.source_len == source_len
            && cache.source_hash == source_hash
        {
            return cache.rows.clone();
        }

        let rows = Rc::new(parse_diff_display_rows(&self.text));
        self.diff_display_cache = Some(DiffDisplayCache {
            source_len,
            source_hash,
            rows: rows.clone(),
        });
        rows
    }

    fn git_history_ui(&mut self, ui: &mut egui::Ui, focused: bool) {
        let total_start = Instant::now();
        let scroll_before_y = self.scroll_y;
        let Some(history_view) = self.git_history_view.clone() else {
            return;
        };

        let header_start = Instant::now();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(history_view.root.display().to_string())
                    .small()
                    .weak(),
            );
            ui.separator();
            let commit_count = history_view
                .rows
                .iter()
                .filter(|row| row.commit.is_some())
                .count();
            ui.label(RichText::new(format!("{commit_count} commits")).small());
        });
        ui.separator();
        let header_ms = elapsed_ms(header_start);

        let viewport_height = ui.available_height();
        if let Some(delta) = diff_page_scroll_delta(ui, focused, viewport_height) {
            self.scroll_y = (self.scroll_y + delta).max(0.0);
        }
        self.apply_immediate_wheel_scroll(ui, self.scroll_max_y);

        let scroll_area = ScrollArea::both()
            .id_salt(format!("git_history_view_{:?}", self.pane))
            .auto_shrink([false, false])
            .vertical_scroll_offset(self.scroll_y);

        let mut clicked_commit = None;
        let body_start = Instant::now();
        let scroll_output = scroll_area.show(ui, |ui| {
            let min_width = git_history_min_width();
            ui.set_min_width(min_width);
            git_history_header_ui(ui, min_width);
            for row in &history_view.rows {
                let selected = row.commit.as_ref().is_some_and(|commit| {
                    history_view
                        .selected_commit_hash
                        .as_deref()
                        .is_some_and(|hash| hash == commit.hash.as_str())
                });
                let response = git_history_row_ui(ui, row, min_width, selected);
                if response.clicked()
                    && let Some(commit) = row.commit.clone()
                {
                    clicked_commit = Some(commit);
                }
            }
        });
        if let Some(commit) = clicked_commit {
            if let Some(history_view) = self.git_history_view.as_mut() {
                history_view.selected_commit_hash = Some(commit.hash.clone());
            }
            self.pending_action = Some(EditorAction::OpenGitCommitDetails {
                repository_name: history_view.repository_name.clone(),
                root: history_view.root.clone(),
                commit,
            });
            ui.ctx().request_repaint();
        }
        self.scroll_max_y =
            scroll_area_vertical_max_scroll_y(scroll_output.content_size, scroll_output.inner_rect);
        self.scroll_y = scroll_output.state.offset.y.clamp(0.0, self.scroll_max_y);
        self.editor_perf_debug = Some(EditorPerfDebug {
            view: "git-history",
            total_ms: elapsed_ms(total_start),
            header_ms,
            body_ms: elapsed_ms(body_start),
            visible_rows: history_view.rows.len(),
            total_rows: history_view.rows.len(),
            text_len: 0,
            scroll_y: self.scroll_y,
            scroll_x: self.scroll_x,
            scroll_delta_y: self.scroll_y - scroll_before_y,
            ..Default::default()
        });
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        focused: bool,
        language_registry: &LanguageRegistry,
        tab_width: usize,
        navigation_history: &[NavigationPoint],
        bookmark_lines: &[u32],
    ) {
        let frame = egui::Frame::group(ui.style()).fill(if focused {
            ui.visuals().faint_bg_color
        } else {
            ui.visuals().extreme_bg_color
        });

        frame.show(ui, |ui| {
            ui.set_min_height(ui.available_height());

            ui.horizontal(|ui| {
                ui.label(RichText::new(&self.title).strong());
                paint_navigation_breadcrumb(
                    ui,
                    navigation_history,
                    self.navigation_point(),
                    |index| {
                        self.pending_action = Some(EditorAction::OpenNavigationHistory { index });
                    },
                );

                if self.read_only {
                    ui.label(
                        RichText::new("READ ONLY")
                            .small()
                            .color(ui.visuals().warn_fg_color),
                    );
                } else if ui.button("Save").clicked() {
                    self.pending_action = Some(EditorAction::SaveFile);
                }
            });

            ui.separator();

            if self.diff_view.is_some() {
                self.diff_view_ui(ui, focused);
                return;
            }

            if self.git_history_view.is_some() {
                self.git_history_ui(ui, focused);
                return;
            }

            if self.is_csv() {
                self.csv_editor_ui(ui, focused, language_registry, tab_width);
                return;
            }

            if self.is_markdown() {
                self.markdown_ui(ui, focused, language_registry, tab_width, bookmark_lines);
                return;
            }

            self.source_editor_ui(ui, focused, language_registry, tab_width, bookmark_lines);
        });
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct MarkdownInlineStyle {
    emphasis: bool,
    strong: bool,
    link: bool,
    delete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkdownFocusTarget {
    Pages,
    Body,
    Outline,
}

fn markdown_inline_layout_job(
    ui: &egui::Ui,
    children: &[Node],
    search_query: Option<&str>,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    for child in children {
        append_markdown_inline(
            ui,
            &mut job,
            child,
            MarkdownInlineStyle::default(),
            search_query,
        );
    }
    job.wrap.max_width = ui.available_width();
    job
}

fn markdown_plain_text_layout_job(
    ui: &egui::Ui,
    text: &str,
    search_query: Option<&str>,
    format: TextFormat,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    append_markdown_text_with_search(ui, &mut job, text, format, search_query);
    job.wrap.max_width = ui.available_width();
    job
}

fn markdown_search_matches(document: &MarkdownDocument, query: &str) -> Vec<MarkdownSearchMatch> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }

    let query_lower = query.to_lowercase();
    let mut matches = Vec::new();

    for (page_index, page) in document.pages.iter().enumerate() {
        let page_text = document.page_rendered_text(page);
        let page_text_lower = page_text.to_lowercase();
        let mut cursor = 0;

        while let Some(relative_start) = page_text_lower[cursor..].find(&query_lower) {
            let start = cursor + relative_start;
            let end = start + query_lower.len();
            matches.push(MarkdownSearchMatch {
                page_id: page.id.clone(),
                page_index,
                start,
                end,
            });
            cursor = end.max(start + 1);
        }
    }

    matches
}

fn source_search_matches(source: &str, query: &str) -> Vec<SourceSearchMatch> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }

    source
        .match_indices(query)
        .map(|(start, text)| SourceSearchMatch {
            start,
            end: start + text.len(),
        })
        .collect()
}

fn workspace_search_results(
    files: &[PathBuf],
    query: &str,
    max_matches: usize,
) -> Vec<WorkspaceSearchFileResult> {
    let query = query.trim();
    if query.is_empty() || max_matches == 0 {
        return Vec::new();
    }

    let mut results = Vec::new();
    let mut remaining_matches = max_matches;

    for path in files {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let Some(mut result) = workspace_search_file(path.clone(), &text, query) else {
            continue;
        };

        if result.matches.len() > remaining_matches {
            result.matches.truncate(remaining_matches);
        }
        remaining_matches = remaining_matches.saturating_sub(result.matches.len());
        results.push(result);

        if remaining_matches == 0 {
            break;
        }
    }

    results
}

fn workspace_search_file(
    path: PathBuf,
    text: &str,
    query: &str,
) -> Option<WorkspaceSearchFileResult> {
    if query.is_empty() {
        return None;
    }

    let mut matches = Vec::new();

    for (line_index, line) in text.lines().enumerate() {
        let mut cursor = 0;
        while let Some(relative_start) = line[cursor..].find(query) {
            let character = cursor + relative_start;
            matches.push(WorkspaceSearchMatch {
                position: Position::new(line_index as u32, character as u32),
                preview: workspace_search_preview(line),
            });
            cursor = character + query.len();
        }
    }

    if matches.is_empty() {
        None
    } else {
        Some(WorkspaceSearchFileResult { path, matches })
    }
}

fn workspace_search_preview(line: &str) -> String {
    let trimmed = line.trim();
    let mut preview = trimmed
        .chars()
        .take(WORKSPACE_SEARCH_PREVIEW_CHARS)
        .collect::<String>();
    if trimmed.chars().count() > WORKSPACE_SEARCH_PREVIEW_CHARS {
        preview.push_str("...");
    }
    preview
}

fn workspace_search_display_path(path: &Path, roots: &[PathBuf]) -> String {
    roots
        .iter()
        .filter_map(|root| path.strip_prefix(root).ok())
        .min_by_key(|relative| relative.components().count())
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn discover_git_repositories(roots: &[PathBuf]) -> Vec<GitRepositoryState> {
    let mut repositories = Vec::new();
    let mut seen_roots = Vec::new();

    for root in roots {
        let Some(repo_root) = git_top_level(root) else {
            continue;
        };
        if seen_roots.iter().any(|seen| seen == &repo_root) {
            continue;
        }
        seen_roots.push(repo_root.clone());
        repositories.push(git_repository_state(repo_root));
    }

    repositories.sort_by(|left, right| left.name.cmp(&right.name));
    repositories
}

fn git_repository_state(root: PathBuf) -> GitRepositoryState {
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<repository>")
        .to_owned();
    let branch = git_output(&root, &["branch", "--show-current"])
        .ok()
        .filter(|branch| !branch.is_empty())
        .or_else(|| git_output(&root, &["rev-parse", "--short", "HEAD"]).ok())
        .unwrap_or_else(|| "unknown".to_owned());
    let upstream = git_output(
        &root,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )
    .ok()
    .filter(|upstream| !upstream.is_empty());
    let tracking = upstream.as_ref().and_then(|_| {
        git_output(
            &root,
            &["rev-list", "--left-right", "--count", "HEAD...@{u}"],
        )
        .ok()
        .and_then(|output| parse_git_tracking_status(&output))
    });
    let branches = git_output(&root, &["branch", "--format=%(refname:short)"])
        .map(|output| parse_git_branches(&output))
        .unwrap_or_default();

    let (changes, error) = match git_output(&root, &["status", "--porcelain=v1", "-z", "-uall"]) {
        Ok(output) => (parse_git_status(&output), None),
        Err(error) => (Vec::new(), Some(error)),
    };

    GitRepositoryState {
        root,
        name,
        branch,
        upstream,
        tracking,
        branches,
        changes,
        error,
    }
}

fn git_top_level(path: &Path) -> Option<PathBuf> {
    let output = git_output(path, &["rev-parse", "--show-toplevel"]).ok()?;
    let path = PathBuf::from(output);
    if path.as_os_str().is_empty() {
        None
    } else {
        Some(path)
    }
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<String, String> {
    git_command_output(cwd, args)
}

fn git_command_output(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    hide_windows_console(&mut command);
    isolate_git_config_for_tests(&mut command);
    let output = command
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;

    let stdout = trim_git_command_output(&String::from_utf8_lossy(&output.stdout));
    let stderr = trim_git_command_output(&String::from_utf8_lossy(&output.stderr));
    if output.status.success() {
        if stderr.is_empty() {
            Ok(stdout)
        } else if stdout.is_empty() {
            Ok(stderr)
        } else {
            Ok(format!("{stdout}\n{stderr}"))
        }
    } else {
        Err(if stdout.is_empty() && stderr.is_empty() {
            format!("git exited with status {}", output.status)
        } else if stdout.is_empty() {
            stderr
        } else if stderr.is_empty() {
            stdout
        } else {
            format!("{stdout}\n{stderr}")
        })
    }
}

/// Tests run real `git` against temporary repositories. Ignore the developer's global and
/// system configuration (signing, hooks, aliases, credential helpers) so results are stable.
fn isolate_git_config_for_tests(command: &mut Command) {
    #[cfg(test)]
    {
        command
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                std::env::temp_dir().join("code-reader-test-gitconfig"),
            )
            .env("GIT_TERMINAL_PROMPT", "0");
    }
    #[cfg(not(test))]
    let _ = command;
}

fn hide_windows_console(command: &mut Command) {
    #[cfg(windows)]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }
}

fn git_history_nodes(
    repository: &GitRepositoryState,
    show_remote_branches: bool,
) -> Result<Vec<GitGraphRow>, String> {
    const FORMAT: &str = "%x1f%H%x1f%P%x1f%an%x1f%ad%x1f%D%x1f%s%x1e";
    let revision_scope = if show_remote_branches {
        "--all"
    } else {
        "--branches"
    };
    let output = git_command_output(
        &repository.root,
        &[
            "log",
            revision_scope,
            "--graph",
            "--date-order",
            "-n",
            "200",
            "--date=iso-strict",
            &format!("--pretty=format:{FORMAT}"),
        ],
    )?;
    Ok(parse_git_history_graph_log(&output))
}

fn git_commit_changed_files(
    root: &Path,
    commit_hash: &str,
) -> Result<Vec<GitCommitFileChange>, String> {
    let output = git_command_output(
        root,
        &[
            "show",
            "--name-status",
            "--format=",
            "--find-renames",
            commit_hash,
        ],
    )?;
    Ok(parse_git_commit_changed_files(&output))
}

fn git_commit_diff_text(root: &Path, commit_hash: &str) -> Result<String, String> {
    git_command_output(root, &["show", "--format=", "--find-renames", commit_hash])
}

fn parse_git_history_graph_log(output: &str) -> Vec<GitGraphRow> {
    output
        .lines()
        .filter_map(|record| {
            let record = record.trim_end_matches('\r');
            if record.is_empty() {
                return None;
            }
            let Some((graph, commit_fields)) = record.split_once('\x1f') else {
                let graph = record.trim_end().to_owned();
                return git_graph_prefix_has_marks(&graph).then_some(GitGraphRow {
                    graph,
                    commit: None,
                });
            };
            let graph = graph.trim_end().to_owned();
            let commit_fields = commit_fields.trim_end_matches('\x1e');
            let fields = commit_fields.splitn(6, '\x1f').collect::<Vec<_>>();
            if fields.len() != 6 {
                return None;
            }
            let hash = fields[0].to_owned();
            let parents = fields[1]
                .split_whitespace()
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            let refs = parse_git_history_refs(fields[4]);
            Some(GitGraphRow {
                graph,
                commit: Some(GitCommitNode {
                    short_hash: hash.chars().take(8).collect(),
                    hash,
                    parents,
                    author: fields[2].to_owned(),
                    date: display_git_history_date(fields[3]),
                    refs,
                    subject: fields[5].to_owned(),
                }),
            })
        })
        .collect()
}

fn parse_git_commit_changed_files(output: &str) -> Vec<GitCommitFileChange> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() {
                return None;
            }
            let parts = line.split('\t').collect::<Vec<_>>();
            let status = parts.first()?.trim();
            if status.is_empty() {
                return None;
            }
            if (status.starts_with('R') || status.starts_with('C')) && parts.len() >= 3 {
                return Some(GitCommitFileChange {
                    status: status.to_owned(),
                    old_path: Some(parts[1].to_owned()),
                    path: parts[2].to_owned(),
                });
            }
            Some(GitCommitFileChange {
                status: status.to_owned(),
                path: parts.get(1)?.to_string(),
                old_path: None,
            })
        })
        .collect()
}

fn git_graph_prefix_has_marks(graph: &str) -> bool {
    graph
        .chars()
        .any(|character| matches!(character, '*' | '|' | '/' | '\\' | '_' | '-'))
}

fn parse_git_history_refs(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|reference| !reference.is_empty())
        .filter_map(|reference| {
            if reference == "HEAD" {
                None
            } else if let Some((_, branch)) = reference.split_once(" -> ") {
                Some(branch.to_owned())
            } else {
                Some(reference.to_owned())
            }
        })
        .collect()
}

fn display_git_history_date(text: &str) -> String {
    let mut date = text.chars().take(16).collect::<String>();
    date = date.replace('T', " ");
    date
}

fn git_graph_color(color_index: usize) -> Color32 {
    GIT_GRAPH_COLORS[color_index % GIT_GRAPH_COLORS.len()]
}

const GIT_GRAPH_COLORS: [Color32; 8] = [
    Color32::from_rgb(0, 150, 220),
    Color32::from_rgb(225, 0, 152),
    Color32::from_rgb(37, 216, 70),
    Color32::from_rgb(242, 158, 0),
    Color32::from_rgb(169, 73, 235),
    Color32::from_rgb(86, 182, 194),
    Color32::from_rgb(224, 108, 117),
    Color32::from_rgb(229, 192, 123),
];

fn git_all_diff_text(repository: &GitRepositoryState) -> Result<String, String> {
    let mut sections = Vec::new();

    let staged = git_command_output(&repository.root, &["diff", "--staged"])?;
    if !staged.trim().is_empty() {
        sections.push(format_git_diff_section("Staged Changes", &staged));
    }

    let worktree = git_command_output(&repository.root, &["diff"])?;
    if !worktree.trim().is_empty() {
        sections.push(format_git_diff_section("Worktree Changes", &worktree));
    }

    for change in repository
        .changes
        .iter()
        .filter(|change| change.is_untracked())
    {
        sections.push(format_git_diff_section(
            "Untracked",
            &untracked_file_diff_text(repository, change)?,
        ));
    }

    if sections.is_empty() {
        Ok("No diffs.".to_owned())
    } else {
        Ok(sections.join("\n"))
    }
}

fn git_file_diff_text(
    repository: &GitRepositoryState,
    change: &GitChange,
    scope: GitDiffScope,
    context_mode: GitDiffContextMode,
) -> Result<String, String> {
    git_file_diff_text_for_path(
        &repository.root,
        change.path.as_str(),
        scope,
        context_mode,
        change.is_untracked(),
    )
}

fn git_file_diff_text_for_path(
    root: &Path,
    path: &str,
    scope: GitDiffScope,
    context_mode: GitDiffContextMode,
    untracked: bool,
) -> Result<String, String> {
    if untracked && scope == GitDiffScope::Worktree {
        return untracked_file_diff_text_from_path(root, path);
    }

    let args = git_file_diff_args(path, scope, context_mode);
    let diff = git_command_output(root, &args)?;
    if diff.trim().is_empty() {
        Ok(format!("No diff for {path}."))
    } else {
        Ok(diff)
    }
}

fn git_file_diff_args(
    path: &str,
    scope: GitDiffScope,
    context_mode: GitDiffContextMode,
) -> Vec<&str> {
    let full_file = context_mode == GitDiffContextMode::FullFile;
    match (scope, full_file) {
        (GitDiffScope::All | GitDiffScope::Worktree, false) => vec!["diff", "--", path],
        (GitDiffScope::All | GitDiffScope::Worktree, true) => {
            vec!["diff", "-U999999", "--", path]
        }
        (GitDiffScope::Staged, false) => vec!["diff", "--staged", "--", path],
        (GitDiffScope::Staged, true) => vec!["diff", "--staged", "-U999999", "--", path],
        (GitDiffScope::Commit, false) => vec!["diff", "--", path],
        (GitDiffScope::Commit, true) => vec!["diff", "-U999999", "--", path],
    }
}

fn format_git_diff_section(title: &str, text: &str) -> String {
    format!("diff --code-reader {title}\n{text}\n")
}

fn untracked_file_diff_text(
    repository: &GitRepositoryState,
    change: &GitChange,
) -> Result<String, String> {
    untracked_file_diff_text_from_path(&repository.root, change.path.as_str())
}

fn untracked_file_diff_text_from_path(root: &Path, relative_path: &str) -> Result<String, String> {
    let path = root.join(relative_path);
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read untracked file: {error}"))?;
    let mut diff = format!(
        "diff --git a/{0} b/{0}\nnew file mode 100644\n--- /dev/null\n+++ b/{0}\n@@ -0,0 +1,{1} @@\n",
        relative_path,
        text.lines().count().max(1)
    );
    for line in text.lines() {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    if text.is_empty() {
        diff.push('+');
        diff.push('\n');
    }
    Ok(diff)
}

fn trim_git_command_output(output: &str) -> String {
    output.trim_end_matches(&['\r', '\n'][..]).to_owned()
}

/// Pushes the current branch, publishing it (`push -u <remote> <branch>`) when it has no
/// upstream yet.
fn push_or_publish_git_branch(repository: &GitRepositoryState) -> Result<String, String> {
    if repository.upstream.is_some() {
        return git_command_output(&repository.root, &["push"]);
    }
    if !repository
        .branches
        .iter()
        .any(|branch| branch == &repository.branch)
    {
        return Err("current HEAD is not on a named branch".to_owned());
    }
    let remote = git_default_remote(&repository.root)?;
    git_command_output(
        &repository.root,
        &["push", "-u", &remote, &repository.branch],
    )
}

/// Parses `git status --porcelain=v1 -z` output. NUL-separated entries keep paths with
/// spaces or non-ASCII characters verbatim (no quoting or octal escapes); a rename or copy
/// entry is followed by an extra entry holding the original path.
fn parse_git_status(output: &str) -> Vec<GitChange> {
    let mut entries = output.split('\0');
    let mut changes = Vec::new();
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        let mut chars = entry.chars();
        let index_status = chars.next().unwrap_or(' ');
        let worktree_status = chars.next().unwrap_or(' ');
        if matches!(index_status, 'R' | 'C') || matches!(worktree_status, 'R' | 'C') {
            entries.next();
        }
        let Some(path) = entry.get(3..) else {
            continue;
        };
        let status = entry.get(0..2).unwrap_or_default().trim().to_owned();
        changes.push(GitChange {
            status: if status.is_empty() {
                "?".to_owned()
            } else {
                status
            },
            index_status,
            worktree_status,
            path: path.to_owned(),
        });
    }
    changes
}

fn parse_git_branches(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn git_default_remote(root: &Path) -> Result<String, String> {
    let output = git_command_output(root, &["remote"])?;
    preferred_git_remote(&output).ok_or_else(|| "no Git remote is configured".to_owned())
}

fn preferred_git_remote(output: &str) -> Option<String> {
    let remotes = output
        .lines()
        .map(str::trim)
        .filter(|remote| !remote.is_empty())
        .collect::<Vec<_>>();
    remotes
        .iter()
        .find(|remote| **remote == "origin")
        .or_else(|| remotes.first())
        .map(|remote| (*remote).to_owned())
}

fn parse_git_tracking_status(output: &str) -> Option<GitTrackingStatus> {
    let mut counts = output.split_whitespace();
    let ahead = counts.next()?.parse().ok()?;
    let behind = counts.next()?.parse().ok()?;
    Some(GitTrackingStatus { ahead, behind })
}

fn workspace_git_file_statuses(
    repositories: &[GitRepositoryState],
) -> HashMap<PathBuf, WorkspaceGitStatus> {
    let mut statuses = HashMap::new();
    for repository in repositories {
        for change in &repository.changes {
            let Some(status) = change.workspace_status() else {
                continue;
            };
            statuses.insert(
                git_change_workspace_path(&repository.root, &change.path),
                status,
            );
        }
    }
    statuses
}

fn git_change_workspace_path(repository_root: &Path, git_path: &str) -> PathBuf {
    let mut path = repository_root.to_path_buf();
    for component in git_path.split('/') {
        if !component.is_empty() {
            path.push(component);
        }
    }
    path
}

fn workspace_snapshot(files: &[PathBuf]) -> WorkspaceSnapshot {
    files
        .iter()
        .filter_map(|path| Some((path.clone(), workspace_file_fingerprint(path)?)))
        .collect()
}

fn start_workspace_change_scan(roots: Vec<PathBuf>) -> Receiver<WorkspaceChangeScanResult> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let files = discover_workspace_files(&roots, FileDiscoveryOptions::default());
        let snapshot = workspace_snapshot(&files);
        let _ = sender.send(WorkspaceChangeScanResult {
            roots,
            files,
            snapshot,
        });
    });
    receiver
}

fn workspace_file_fingerprint(path: &Path) -> Option<WorkspaceFileFingerprint> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let modified_unix_nanos = modified
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    Some(WorkspaceFileFingerprint {
        modified_unix_nanos,
        len: metadata.len(),
    })
}

fn workspace_snapshot_change_count(
    previous: &WorkspaceSnapshot,
    next: &WorkspaceSnapshot,
) -> usize {
    workspace_snapshot_diff(previous, next).len()
}

fn workspace_snapshot_diff(
    previous: &WorkspaceSnapshot,
    next: &WorkspaceSnapshot,
) -> WorkspaceSnapshotDiff {
    let mut diff = WorkspaceSnapshotDiff::default();

    for (path, fingerprint) in next {
        match previous.get(path) {
            Some(previous_fingerprint) if previous_fingerprint == fingerprint => {}
            Some(_) => diff.modified.push(path.clone()),
            None => diff.added.push(path.clone()),
        }
    }

    for path in previous.keys() {
        if !next.contains_key(path) {
            diff.removed.push(path.clone());
        }
    }

    diff.added.sort();
    diff.modified.sort();
    diff.removed.sort();
    diff
}

fn is_dependency_manifest_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, "Cargo.toml" | "Cargo.lock" | "go.mod" | "go.sum"))
}

fn workspace_file_tree_height(available_height: f32, footer_header_count: usize) -> f32 {
    if !available_height.is_finite() {
        return WORKSPACE_FILE_TREE_MIN_HEIGHT;
    }

    let reserved_headers = footer_header_count as f32 * WORKSPACE_SECTION_HEADER_HEIGHT + 8.0;
    let reserved_footer = footer_header_count as f32 * WORKSPACE_SECTION_HEADER_HEIGHT
        + WORKSPACE_FOOTER_BODY_ALLOWANCE;
    let max_tree_height = (available_height - reserved_headers).max(60.0);
    (available_height - reserved_footer)
        .max(WORKSPACE_FILE_TREE_MIN_HEIGHT)
        .min(max_tree_height)
}

fn append_markdown_inline(
    ui: &egui::Ui,
    job: &mut LayoutJob,
    node: &Node,
    style: MarkdownInlineStyle,
    search_query: Option<&str>,
) {
    match node {
        Node::Text(text) => append_markdown_text_with_search(
            ui,
            job,
            &text.value,
            markdown_inline_format(ui, style),
            search_query,
        ),
        Node::InlineCode(code) => {
            let mut format = markdown_inline_format(ui, style);
            format.font_id = editor_font_id();
            format.background = markdown_code_background(ui);
            format.expand_bg = 3.0;
            append_markdown_text_with_search(ui, job, &code.value, format, search_query);
        }
        Node::Break(_) => job.append("\n", 0.0, markdown_inline_format(ui, style)),
        Node::Emphasis(emphasis) => {
            let mut style = style;
            style.emphasis = true;
            for child in &emphasis.children {
                append_markdown_inline(ui, job, child, style, search_query);
            }
        }
        Node::Strong(strong) => {
            let mut style = style;
            style.strong = true;
            for child in &strong.children {
                append_markdown_inline(ui, job, child, style, search_query);
            }
        }
        Node::Delete(delete) => {
            let mut style = style;
            style.delete = true;
            for child in &delete.children {
                append_markdown_inline(ui, job, child, style, search_query);
            }
        }
        Node::Link(link) => {
            let mut style = style;
            style.link = true;
            for child in &link.children {
                append_markdown_inline(ui, job, child, style, search_query);
            }
            let mut format = markdown_inline_format(ui, style);
            format.color = ui.visuals().weak_text_color();
            job.append(&format!(" ({})", link.url), 0.0, format);
        }
        Node::LinkReference(link) => {
            let mut style = style;
            style.link = true;
            for child in &link.children {
                append_markdown_inline(ui, job, child, style, search_query);
            }
        }
        Node::Image(image) => {
            job.append(
                &format!("[image: {}]", image.alt),
                0.0,
                markdown_inline_format(ui, style),
            );
        }
        Node::ImageReference(image) => {
            job.append(
                &format!("[image: {}]", image.alt),
                0.0,
                markdown_inline_format(ui, style),
            );
        }
        Node::InlineMath(math) => {
            let mut format = markdown_inline_format(ui, style);
            format.font_id = editor_font_id();
            job.append(&math.value, 0.0, format);
        }
        _ => {
            if let Some(children) = node.children() {
                for child in children {
                    append_markdown_inline(ui, job, child, style, search_query);
                }
            }
        }
    }
}

fn append_markdown_text_with_search(
    ui: &egui::Ui,
    job: &mut LayoutJob,
    text: &str,
    format: TextFormat,
    search_query: Option<&str>,
) {
    let Some(query) = search_query.filter(|query| !query.is_empty()) else {
        job.append(text, 0.0, format);
        return;
    };

    let text_lower = text.to_lowercase();
    let query_lower = query.to_lowercase();
    let mut cursor = 0;

    while let Some(relative_start) = text_lower[cursor..].find(&query_lower) {
        let start = cursor + relative_start;
        let end = start + query_lower.len();
        let start = clamp_to_char_boundary(text, start.min(text.len()));
        let end = clamp_to_char_boundary(text, end.min(text.len()));

        if cursor < start {
            job.append(&text[cursor..start], 0.0, format.clone());
        }

        let mut match_format = format.clone();
        match_format.background = markdown_search_match_background(ui);
        match_format.expand_bg = 3.0;
        match_format.color = ui.visuals().strong_text_color();
        job.append(&text[start..end], 0.0, match_format);
        cursor = end.max(start + 1).min(text.len());
    }

    if cursor < text.len() {
        job.append(&text[cursor..], 0.0, format);
    }
}

fn markdown_inline_format(ui: &egui::Ui, style: MarkdownInlineStyle) -> TextFormat {
    let mut format = TextFormat {
        font_id: FontId::new(14.0, egui::FontFamily::Proportional),
        color: if style.strong {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().text_color()
        },
        ..Default::default()
    };
    format.italics = style.emphasis;
    if style.link {
        format.color = Color32::from_rgb(56, 132, 214);
        format.underline = egui::Stroke::new(1.0, format.color);
    }
    if style.delete {
        format.strikethrough = egui::Stroke::new(1.0, ui.visuals().weak_text_color());
    }
    format
}

fn markdown_heading_text_format(ui: &egui::Ui, depth: u8) -> TextFormat {
    TextFormat {
        font_id: FontId::new(markdown_heading_size(depth), egui::FontFamily::Proportional),
        color: ui.visuals().strong_text_color(),
        ..Default::default()
    }
}

fn markdown_inline_plain_text(children: &[Node]) -> String {
    let mut text = String::new();
    for child in children {
        match child {
            Node::Text(node) => text.push_str(&node.value),
            Node::InlineCode(node) => text.push_str(&node.value),
            Node::InlineMath(node) => text.push_str(&node.value),
            Node::Break(_) => text.push('\n'),
            Node::Image(node) => text.push_str(&node.alt),
            Node::ImageReference(node) => text.push_str(&node.alt),
            _ => {
                if let Some(children) = child.children() {
                    text.push_str(&markdown_inline_plain_text(children));
                }
            }
        }
    }
    text
}

fn markdown_side_pane_width(available_width: f32) -> f32 {
    clamp_markdown_side_pane_width(MARKDOWN_SIDE_PANE_DEFAULT_WIDTH, available_width)
}

fn clamp_markdown_side_pane_width(width: f32, available_width: f32) -> f32 {
    let max_width = (available_width - MARKDOWN_SIDE_PANE_MIN_REMAINING_WIDTH)
        .min(MARKDOWN_SIDE_PANE_MAX_WIDTH)
        .max(MARKDOWN_SIDE_PANE_MIN_WIDTH);
    width.clamp(MARKDOWN_SIDE_PANE_MIN_WIDTH, max_width)
}

fn ellipsize_text_to_width(
    ui: &egui::Ui,
    text: &str,
    font_id: egui::FontId,
    max_width: f32,
) -> String {
    if max_width <= 0.0 {
        return String::new();
    }

    let fits = |value: &str| {
        ui.painter()
            .layout_no_wrap(value.to_owned(), font_id.clone(), Color32::WHITE)
            .size()
            .x
            <= max_width
    };

    if fits(text) {
        return text.to_owned();
    }
    if !fits("...") {
        return String::new();
    }

    let char_count = text.chars().count();
    let mut low = 0;
    let mut high = char_count;
    while low < high {
        let mid = (low + high).div_ceil(2);
        let candidate = format!("{}...", text.chars().take(mid).collect::<String>());
        if fits(&candidate) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }

    format!("{}...", text.chars().take(low).collect::<String>())
}

fn markdown_nodes_need_widgets(nodes: &[Node]) -> bool {
    nodes.iter().any(|node| match node {
        Node::Link(_)
        | Node::LinkReference(_)
        | Node::Image(_)
        | Node::ImageReference(_)
        | Node::FootnoteReference(_) => true,
        _ => node
            .children()
            .is_some_and(|children| markdown_nodes_need_widgets(children)),
    })
}

fn markdown_heading_size(depth: u8) -> f32 {
    match depth {
        1 => 24.0,
        2 => 21.0,
        3 => 18.0,
        4 => 16.0,
        5 => 15.0,
        _ => 14.0,
    }
}

fn markdown_page_body_start_offset(
    document: &MarkdownDocument,
    page: &crate::core::markdown::MarkdownPage,
) -> usize {
    document
        .headings
        .iter()
        .find(|heading| {
            (heading.level == 1 || heading.level == 2)
                && heading.source_range.start.offset == page.source_range.start.offset
        })
        .map(|heading| heading.source_range.end.offset)
        .unwrap_or(page.source_range.start.offset)
}

fn markdown_code_path_for_language(language: &str) -> Option<PathBuf> {
    let extension = match language.to_ascii_lowercase().as_str() {
        "rs" | "rust" => "rs",
        "go" | "golang" => "go",
        _ => return None,
    };
    Some(PathBuf::from(format!("snippet.{extension}")))
}

fn is_mermaid_language(language: &str) -> bool {
    language.eq_ignore_ascii_case("mermaid") || language.eq_ignore_ascii_case("mmd")
}

fn render_mermaid_svg(code: &str) -> Result<String, String> {
    mmdflux::render_diagram(
        code,
        mmdflux::OutputFormat::Svg,
        &mmdflux::RenderConfig::default(),
    )
    .map_err(|error| error.to_string())
}

fn stable_hash(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn markdown_footnote_number(document: &MarkdownDocument, identifier: &str) -> Option<usize> {
    markdown_footnote_definitions(document)
        .iter()
        .position(|definition| definition.identifier == identifier)
        .map(|index| index + 1)
}

fn markdown_footnote_definitions_for_page<'a>(
    document: &'a MarkdownDocument,
    page: &crate::core::markdown::MarkdownPage,
) -> Vec<&'a markdown::mdast::FootnoteDefinition> {
    markdown_footnote_definitions(document)
        .into_iter()
        .filter(|definition| {
            definition.position.as_ref().is_some_and(|position| {
                page.source_range.start.offset <= position.start.offset
                    && position.start.offset < page.source_range.end.offset
            })
        })
        .collect()
}

fn markdown_footnote_definitions(
    document: &MarkdownDocument,
) -> Vec<&markdown::mdast::FootnoteDefinition> {
    let mut definitions = Vec::new();
    collect_markdown_footnote_definitions(&document.ast, &mut definitions);
    definitions.sort_by_key(|definition| {
        definition
            .position
            .as_ref()
            .map(|position| position.start.offset)
            .unwrap_or(usize::MAX)
    });
    definitions
}

fn collect_markdown_footnote_definitions<'a>(
    node: &'a Node,
    definitions: &mut Vec<&'a markdown::mdast::FootnoteDefinition>,
) {
    if let Node::FootnoteDefinition(definition) = node {
        definitions.push(definition);
    }

    if let Some(children) = node.children() {
        for child in children {
            collect_markdown_footnote_definitions(child, definitions);
        }
    }
}

fn markdown_table_cell_layout(align: AlignKind) -> egui::Layout {
    match align {
        AlignKind::Right => egui::Layout::right_to_left(egui::Align::Center),
        AlignKind::Center => egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
        AlignKind::Left | AlignKind::None => egui::Layout::left_to_right(egui::Align::Center),
    }
}

fn markdown_node_ui_id(fragment_start_offset: usize, node: &Node, fallback_kind: &str) -> String {
    node.position()
        .map(|position| {
            format!(
                "{}_{}_{}",
                fallback_kind,
                fragment_start_offset + position.start.offset,
                fragment_start_offset + position.end.offset
            )
        })
        .unwrap_or_else(|| format!("{fallback_kind}_{fragment_start_offset}"))
}

fn markdown_resolve_local_image_path(document: &MarkdownDocument, url: &str) -> Option<PathBuf> {
    if url.trim().is_empty()
        || url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("data:")
    {
        return None;
    }

    let path = PathBuf::from(url);
    if path.is_absolute() {
        Some(path)
    } else {
        document.path.parent().map(|parent| parent.join(path))
    }
}

fn copy_code_block_to_clipboard(ui: &egui::Ui, code: &str) {
    ui.ctx().copy_text(code.to_owned());
    let _ = copy_text_to_system_clipboard(code);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MarkdownFragmentTarget {
    page_id: String,
    heading_id: Option<String>,
    position: Position,
}

fn is_external_markdown_link(target: &str) -> bool {
    let target = target.trim().to_ascii_lowercase();
    target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
        || target.starts_with("data:")
        || target.starts_with("file://")
}

fn split_markdown_link_target(target: &str) -> (String, Option<String>) {
    let Some((path, fragment)) = target.split_once('#') else {
        return (percent_decode_lossy(target.trim()), None);
    };

    let fragment = percent_decode_lossy(fragment.trim());
    (
        percent_decode_lossy(path.trim()),
        (!fragment.is_empty()).then_some(fragment),
    )
}

fn resolve_markdown_link_path(current_document_path: &Path, target_path: &str) -> PathBuf {
    let path = PathBuf::from(target_path);
    if path.is_absolute() {
        path
    } else {
        current_document_path
            .parent()
            .map(|parent| parent.join(&path))
            .unwrap_or(path)
    }
}

fn markdown_fragment_target(
    document: &MarkdownDocument,
    fragment: &str,
) -> Option<MarkdownFragmentTarget> {
    let fragment = markdown_normalize_fragment(fragment);
    if fragment.is_empty() {
        return None;
    }

    if let Some(page) = document.pages.iter().find(|page| page.id == fragment) {
        return Some(MarkdownFragmentTarget {
            page_id: page.id.clone(),
            heading_id: None,
            position: Position::new(
                page.source_range.start.line as u32,
                page.source_range.start.column as u32,
            ),
        });
    }

    document.headings.iter().find_map(|heading| {
        if !markdown_heading_matches_fragment(heading, &fragment) {
            return None;
        }
        let page_id = heading.page_id.clone().or_else(|| {
            document
                .pages
                .iter()
                .find(|page| {
                    page.source_range.start.offset <= heading.source_range.start.offset
                        && heading.source_range.start.offset <= page.source_range.end.offset
                })
                .map(|page| page.id.clone())
        })?;

        Some(MarkdownFragmentTarget {
            page_id,
            heading_id: Some(heading.slug.clone()),
            position: Position::new(
                heading.source_range.start.line as u32,
                heading.source_range.start.column as u32,
            ),
        })
    })
}

fn markdown_heading_matches_fragment(
    heading: &crate::core::markdown::MarkdownHeading,
    fragment: &str,
) -> bool {
    if heading.slug == fragment {
        return true;
    }

    let heading_prefix = format!("h{}-", heading.level);
    heading
        .slug
        .strip_prefix(&heading_prefix)
        .is_some_and(|slug| slug == fragment)
}

fn markdown_normalize_fragment(fragment: &str) -> String {
    let fragment = percent_decode_lossy(fragment.trim().trim_start_matches('#'));
    let mut slug = String::new();
    let mut previous_dash = false;

    for character in fragment.trim().chars() {
        if character.is_ascii_alphanumeric() {
            slug.extend(character.to_lowercase());
            previous_dash = false;
        } else if !character.is_ascii() && !character.is_whitespace() {
            slug.push_str(&format!("u{:x}", character as u32));
            previous_dash = false;
        } else if !previous_dash && !slug.is_empty() {
            slug.push('-');
            previous_dash = true;
        }
    }

    while slug.ends_with('-') {
        slug.pop();
    }

    slug
}

fn percent_decode_lossy(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            decoded.push(high * 16 + low);
            index += 3;
            continue;
        }

        decoded.push(bytes[index]);
        index += 1;
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn load_markdown_image_texture(
    context: &egui::Context,
    path: &Path,
) -> Result<egui::TextureHandle, String> {
    if !path.exists() {
        return Err(format!("not found: {}", path.display()));
    }

    let image = image::ImageReader::open(path)
        .map_err(|error| format!("open {}: {error}", path.display()))?
        .decode()
        .map_err(|error| format!("decode {}: {error}", path.display()))?
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let pixels = image.as_flat_samples();
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());

    Ok(context.load_texture(
        format!("markdown_image:{}", path.display()),
        color_image,
        egui::TextureOptions::LINEAR,
    ))
}

fn markdown_code_background(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(36, 38, 42)
    } else {
        Color32::from_rgb(244, 246, 248)
    }
}

fn markdown_blockquote_background(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(38, 42, 46)
    } else {
        Color32::from_rgb(248, 249, 250)
    }
}

fn editor_pane_from_session_path(
    pane: Pane,
    path: PathBuf,
    text_position: TextPosition,
) -> EditorPane {
    let mut editor = EditorPane::from_path(pane, path);
    let position = Position::new(text_position.line, text_position.character);
    editor.target_position = Some(position);
    editor.pending_navigation = Some(PendingEditorNavigation {
        position,
        focus_editor: true,
    });
    editor.cursor_position = Some(position);
    editor
}

fn location_contains_position(location: &Location, path: &Path, position: Position) -> bool {
    paths_match(&location.path, path) && range_contains_position(location.range, position)
}

fn paths_match(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn range_contains_position(range: Range, position: Position) -> bool {
    if position.line < range.start.line || position.line > range.end.line {
        return false;
    }
    if position.line == range.start.line && position.character < range.start.character {
        return false;
    }
    if position.line == range.end.line && position.character > range.end.character {
        return false;
    }
    true
}

fn location_result_label_with_preview(
    location: &Location,
    kind: LocationListKind,
    preview: Option<&str>,
) -> String {
    let base = format!(
        "{}:{}",
        location.path.display(),
        location.range.start.line + 1
    );
    if kind != LocationListKind::References {
        return base;
    }

    match preview {
        Some(preview) if !preview.is_empty() => format!("{base}  {preview}"),
        _ => base,
    }
}

fn location_line_preview(location: &Location) -> Option<String> {
    let text = fs::read_to_string(&location.path).ok()?;
    let line = text.lines().nth(location.range.start.line as usize)?;
    Some(truncate_location_preview(line.trim()))
}

fn reference_overlay_entries(
    candidates: &[DefinitionCandidate],
    roots: &[PathBuf],
    query: &str,
) -> Vec<ReferenceOverlayEntry> {
    let query = query.trim().to_lowercase();
    let mut entries = candidates
        .iter()
        .filter_map(|candidate| {
            let display_path = workspace_search_display_path(&candidate.location.path, roots);
            let line_number = candidate.location.range.start.line + 1;
            let preview = candidate.preview.clone().unwrap_or_default();
            let file_name = candidate
                .location
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            let search_text =
                format!("{display_path} {file_name} {line_number} {preview}").to_lowercase();
            if !query.is_empty() && !search_text.contains(&query) {
                return None;
            }
            Some(ReferenceOverlayEntry {
                path: candidate.location.path.clone(),
                display_path,
                line_number,
                preview,
                location: candidate.location.clone(),
            })
        })
        .collect::<Vec<_>>();

    entries.sort_by(|left, right| {
        left.display_path
            .cmp(&right.display_path)
            .then_with(|| left.line_number.cmp(&right.line_number))
            .then_with(|| {
                left.location
                    .range
                    .start
                    .character
                    .cmp(&right.location.range.start.character)
            })
    });
    entries
}

fn truncate_location_preview(value: &str) -> String {
    const LIMIT: usize = 120;
    if value.chars().count() <= LIMIT {
        return value.to_owned();
    }
    let mut preview = value
        .chars()
        .take(LIMIT.saturating_sub(3))
        .collect::<String>();
    preview.push_str("...");
    preview
}

fn elide_text_to_width(
    ui: &egui::Ui,
    text: &str,
    font_id: FontId,
    color: Color32,
    max_width: f32,
) -> String {
    if max_width <= 0.0 {
        return String::new();
    }
    if ui
        .painter()
        .layout_no_wrap(text.to_owned(), font_id.clone(), color)
        .size()
        .x
        <= max_width
    {
        return text.to_owned();
    }

    const ELLIPSIS: &str = "...";
    let ellipsis_width = ui
        .painter()
        .layout_no_wrap(ELLIPSIS.to_owned(), font_id.clone(), color)
        .size()
        .x;
    if ellipsis_width > max_width {
        return String::new();
    }

    let characters = text.chars().collect::<Vec<_>>();
    let mut low = 0usize;
    let mut high = characters.len();
    while low < high {
        let middle = (low + high).div_ceil(2);
        let mut candidate = characters[..middle].iter().collect::<String>();
        candidate.push_str(ELLIPSIS);
        let width = ui
            .painter()
            .layout_no_wrap(candidate, font_id.clone(), color)
            .size()
            .x;
        if width <= max_width {
            low = middle;
        } else {
            high = middle.saturating_sub(1);
        }
    }

    let mut result = characters[..low].iter().collect::<String>();
    result.push_str(ELLIPSIS);
    result
}

fn text_position_from_position(position: Option<Position>) -> TextPosition {
    position
        .map(|position| TextPosition::new(position.line, position.character))
        .unwrap_or_default()
}

fn paint_navigation_breadcrumb(
    ui: &mut egui::Ui,
    navigation_history: &[NavigationPoint],
    current: Option<NavigationPoint>,
    mut open_history_entry: impl FnMut(usize),
) {
    if navigation_history.is_empty() {
        return;
    }

    let Some(current) = current else {
        return;
    };

    ui.separator();

    let visible_previous_count = NAVIGATION_BREADCRUMB_LIMIT.saturating_sub(1);
    let visible_start = navigation_history
        .len()
        .saturating_sub(visible_previous_count);
    if visible_start > 0 {
        ui.label(RichText::new("...").small().weak());
        ui.label(RichText::new(">").small().weak());
    }

    for (index, point) in navigation_history.iter().enumerate().skip(visible_start) {
        let response = ui
            .small_button(RichText::new(navigation_point_label(point)).monospace())
            .on_hover_text(navigation_point_hover_text(point));
        if response.clicked() {
            open_history_entry(index);
        }
        ui.label(RichText::new(">").small().weak());
    }

    ui.label(
        RichText::new(navigation_point_label(&current))
            .small()
            .strong()
            .monospace(),
    )
    .on_hover_text(navigation_point_hover_text(&current));
}

fn navigation_point_label(point: &NavigationPoint) -> String {
    let file_name = point
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<unnamed>");
    format!("{file_name}:{}", point.position.line + 1)
}

fn navigation_point_hover_text(point: &NavigationPoint) -> String {
    format!(
        "{}:{}:{}",
        point.path.display(),
        point.position.line + 1,
        point.position.character + 1
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EditorAction {
    GoToDefinition,
    Back,
    OpenNavigationHistory {
        index: usize,
    },
    OpenGitCommitDetails {
        repository_name: String,
        root: PathBuf,
        commit: GitCommitNode,
    },
    SaveFile,
    OpenMarkdownLink {
        path: PathBuf,
        fragment: Option<String>,
    },
    OpenExternalUrl(String),
    ToggleBookmark {
        path: PathBuf,
        line: u32,
    },
    ShowStatus(String),
}

#[derive(Debug, Clone)]
enum InputAssistCommand {
    Insert { text: String, cursor_delta: usize },
    Move { cursor_delta: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CsvCellPosition {
    row: usize,
    column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CsvSelection {
    Rows { start: usize, end: usize },
    Columns { start: usize, end: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CsvHeaderAction {
    Select(usize),
    Press(usize),
    ExtendDrag(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CsvSelectionDrag {
    kind: CsvSelectionKind,
    anchor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CsvSelectionKind {
    Rows,
    Columns,
}

impl CsvSelection {
    fn row_range(self) -> Option<(usize, usize)> {
        match self {
            Self::Rows { start, end } => Some(ordered_range(start, end)),
            Self::Columns { .. } => None,
        }
    }

    fn column_range(self) -> Option<(usize, usize)> {
        match self {
            Self::Columns { start, end } => Some(ordered_range(start, end)),
            Self::Rows { .. } => None,
        }
    }

    fn contains(self, row: usize, column: usize) -> bool {
        if let Some((start, end)) = self.row_range() {
            return (start..=end).contains(&row);
        }

        if let Some((start, end)) = self.column_range() {
            return (start..=end).contains(&column);
        }

        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CsvDocument {
    rows: Vec<Vec<String>>,
}

impl CsvDocument {
    fn column_count(&self) -> usize {
        self.rows.iter().map(Vec::len).max().unwrap_or(0)
    }

    fn ensure_cell(&mut self, row: usize, column: usize) {
        let column_count = self.column_count().max(column + 1);

        while self.rows.len() <= row {
            self.rows.push(vec![String::new(); column_count]);
        }

        for fields in &mut self.rows {
            if fields.len() < column_count {
                fields.resize(column_count, String::new());
            }
        }
    }

    fn cell_mut(&mut self, row: usize, column: usize) -> &mut String {
        self.ensure_cell(row, column);

        if self.rows[row].len() <= column {
            self.rows[row].resize(column + 1, String::new());
        }

        &mut self.rows[row][column]
    }
}

#[derive(Debug, Clone)]
enum BookmarkAction {
    Open(SessionBookmark),
    Remove(usize),
}

#[derive(Debug, Clone)]
struct EditorRenderDebug {
    line_count: usize,
    text_line_count: usize,
    galley_rows: usize,
    scroll_y: f32,
    row_height: f32,
    clip_top: f32,
    clip_bottom: f32,
    viewport_top: f32,
    viewport_bottom: f32,
    galley_pos_y: f32,
    first_visible_row: Option<usize>,
    first_visible_top: Option<f32>,
    last_visible_row: Option<usize>,
    anchor_line: Option<usize>,
    anchor_top: Option<f32>,
    anchored_range: Option<std::ops::Range<usize>>,
    current_line: Option<u32>,
}

#[derive(Debug, Clone)]
struct DefinitionCandidate {
    label: String,
    location: Location,
    preview: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct LspDebugState {
    last_status: String,
    last_error: Option<String>,
    last_kind: Option<LspRequestKind>,
    last_request_id: Option<u64>,
    last_duration_ms: Option<f64>,
    last_location_count: usize,
    last_definition_status: String,
    last_definition_error: Option<String>,
    last_definition_duration_ms: Option<f64>,
    last_definition_location_count: usize,
    warmup_count: usize,
    sync_count: usize,
    definition_count: usize,
    reference_count: usize,
    fallback_count: usize,
    timeout_count: usize,
    initialized: bool,
    started_at: Option<Instant>,
    initialized_at: Option<Instant>,
    last_progress_at: Option<Instant>,
    last_diagnostics_at: Option<Instant>,
    last_definition_success_at: Option<Instant>,
    active_progress_count: usize,
    server_request_count: usize,
    server_notification_count: usize,
    reader_error_count: usize,
    progress_count: usize,
    diagnostics_count: usize,
    last_server_kind: String,
    last_server_method: String,
    last_server_message: Option<String>,
}

impl LspDebugState {
    fn mark_started(&mut self) {
        let now = Instant::now();
        self.started_at = Some(now);
        self.last_status = "starting".to_owned();
    }

    fn record_definition_status(
        &mut self,
        status: &str,
        error: Option<String>,
        duration_ms: Option<f64>,
        location_count: usize,
    ) {
        self.last_definition_status = status.to_owned();
        self.last_definition_error = error;
        self.last_definition_duration_ms = duration_ms;
        self.last_definition_location_count = location_count;
        if status == "definition_ok" && location_count > 0 {
            self.last_definition_success_at = Some(Instant::now());
        }
    }

    fn record_server_event(&mut self, event: &LspServerEvent) {
        let now = Instant::now();
        match event.kind {
            LspServerEventKind::Request => self.server_request_count += 1,
            LspServerEventKind::Notification => self.server_notification_count += 1,
            LspServerEventKind::ReaderError => self.reader_error_count += 1,
            LspServerEventKind::Initialized => {
                self.initialized = true;
                self.initialized_at = Some(now);
            }
        }
        if event.method == "$/progress" {
            self.progress_count += 1;
            self.last_progress_at = Some(now);
            if event
                .message
                .as_deref()
                .is_some_and(|message| message.contains("kind=begin"))
            {
                self.active_progress_count += 1;
            } else if event
                .message
                .as_deref()
                .is_some_and(|message| message.contains("kind=end"))
            {
                self.active_progress_count = self.active_progress_count.saturating_sub(1);
            }
        }
        if let Some(count) = event.diagnostics_count {
            self.diagnostics_count = count;
            self.last_diagnostics_at = Some(now);
        }
        self.last_server_kind = format!("{:?}", event.kind);
        self.last_server_method = event.method.clone();
        self.last_server_message = event.message.clone();
        if event.kind == LspServerEventKind::ReaderError {
            self.last_error = event.message.clone();
            self.last_status = "reader_error".to_owned();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RustLspReadinessStatus {
    NotStarted,
    WarmingUp(&'static str),
    Ready(&'static str),
    Error,
}

impl RustLspReadinessStatus {
    fn status_bar_label(self) -> String {
        match self {
            Self::NotStarted => "not started".to_owned(),
            Self::WarmingUp(reason) => format!("warming up... ({reason})"),
            Self::Ready(reason) => format!("ready ({reason})"),
            Self::Error => "error".to_owned(),
        }
    }

    fn report_label(self) -> String {
        match self {
            Self::NotStarted => "not_started".to_owned(),
            Self::WarmingUp(reason) => format!("warming_up reason={reason}"),
            Self::Ready(reason) => format!("ready reason={reason}"),
            Self::Error => "error".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocationListKind {
    Definitions,
    References,
}

impl LocationListKind {
    fn report_title(self) -> &'static str {
        match self {
            Self::Definitions => "Definition locations",
            Self::References => "References",
        }
    }

    fn singular_label(self) -> &'static str {
        match self {
            Self::Definitions => "definition",
            Self::References => "reference",
        }
    }
}

#[derive(Debug, Clone)]
struct PendingGoLspRequest {
    request_id: u64,
    kind: LspRequestKind,
    document: Document,
    position: Position,
    identifier: String,
    origin: Option<NavigationPoint>,
    started_at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NavigationPoint {
    path: PathBuf,
    position: Position,
}

#[derive(Debug, Clone, Default)]
struct NavigationHistory {
    back: Vec<NavigationPoint>,
    forward: Vec<NavigationPoint>,
}

impl NavigationHistory {
    fn push_back(&mut self, point: NavigationPoint) {
        if self.back.last() != Some(&point) {
            self.back.push(point);
        }
        self.forward.clear();
        self.trim_back();
    }

    fn go_back(&mut self, current: Option<NavigationPoint>) -> Option<NavigationPoint> {
        let previous = self.back.pop()?;
        if let Some(current) = current {
            push_unique(&mut self.forward, current);
        }
        Some(previous)
    }

    fn go_forward(&mut self, current: Option<NavigationPoint>) -> Option<NavigationPoint> {
        let next = self.forward.pop()?;
        if let Some(current) = current {
            push_unique(&mut self.back, current);
            self.trim_back();
        }
        Some(next)
    }

    fn open_back_entry(
        &mut self,
        index: usize,
        current: Option<NavigationPoint>,
    ) -> Option<NavigationPoint> {
        if index >= self.back.len() {
            return None;
        }

        let point = self.back[index].clone();
        let later_entries = self.back.drain(index + 1..).collect::<Vec<_>>();
        self.back.truncate(index);

        if let Some(current) = current {
            push_unique(&mut self.forward, current);
        }
        for entry in later_entries.into_iter().rev() {
            push_unique(&mut self.forward, entry);
        }

        Some(point)
    }

    fn clear(&mut self) {
        self.back.clear();
        self.forward.clear();
    }

    fn remap_path(&mut self, old_path: &Path, new_path: &Path) {
        for point in self.back.iter_mut().chain(self.forward.iter_mut()) {
            if point.path == old_path {
                point.path = new_path.to_path_buf();
            }
        }
    }

    fn retain_path_not(&mut self, path: &Path) {
        self.back.retain(|point| point.path != path);
        self.forward.retain(|point| point.path != path);
    }

    fn trim_back(&mut self) {
        let previous_limit = NAVIGATION_BREADCRUMB_LIMIT.saturating_sub(1);
        if self.back.len() > previous_limit {
            let drain_count = self.back.len() - previous_limit;
            self.back.drain(0..drain_count);
        }
    }
}

fn push_unique(points: &mut Vec<NavigationPoint>, point: NavigationPoint) {
    if points.last() != Some(&point) {
        points.push(point);
    }
}

fn configure_fonts(context: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    insert_static_font(
        &mut fonts,
        "moralerspace_neon_regular",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/fonts/MoralerspaceNeon-Regular.ttf"
        )),
        &[
            FontFamilyInsertion::MonospacePrimary,
            FontFamilyInsertion::ProportionalFallback,
        ],
    );
    context.set_fonts(fonts);

    let mut style = (*context.global_style()).clone();
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(14.0, egui::FontFamily::Monospace),
    );
    context.set_global_style(style);
}

#[derive(Clone, Copy)]
enum FontFamilyInsertion {
    MonospacePrimary,
    ProportionalFallback,
}

fn insert_static_font(
    fonts: &mut egui::FontDefinitions,
    name: &str,
    bytes: &'static [u8],
    insertions: &[FontFamilyInsertion],
) {
    fonts.font_data.insert(
        name.to_owned(),
        Arc::new(egui::FontData::from_static(bytes)),
    );

    for insertion in insertions {
        match insertion {
            FontFamilyInsertion::MonospacePrimary => {
                insert_font_family_name(fonts, egui::FontFamily::Monospace, name, 0);
            }
            FontFamilyInsertion::ProportionalFallback => {
                push_font_family_name(fonts, egui::FontFamily::Proportional, name);
            }
        }
    }
}

fn insert_font_family_name(
    fonts: &mut egui::FontDefinitions,
    family: egui::FontFamily,
    name: &str,
    index: usize,
) {
    let family = fonts.families.entry(family).or_default();
    family.retain(|font_name| font_name != name);
    family.insert(index.min(family.len()), name.to_owned());
}

fn push_font_family_name(fonts: &mut egui::FontDefinitions, family: egui::FontFamily, name: &str) {
    let family = fonts.families.entry(family).or_default();
    family.retain(|font_name| font_name != name);
    family.push(name.to_owned());
}

fn syntax_layout_job(
    ui: &egui::Ui,
    path: Option<&Path>,
    source: &str,
    language_registry: &LanguageRegistry,
    bracket_pair: Option<(usize, usize)>,
) -> LayoutJob {
    let mut spans = path
        .and_then(|path| {
            let language = language_registry.language_for_path(path)?;
            let document = Document::new(path.to_path_buf(), source.to_owned());
            Some(language.highlight_spans(&document))
        })
        .unwrap_or_default();
    spans.sort_by_key(|span| (span.range.start, span.range.end));

    highlighted_layout_job(ui, source, &spans, bracket_pair, &[], None)
}

fn build_text_chunks(source: &str) -> Vec<TextChunk> {
    if source.is_empty() {
        return vec![TextChunk {
            start: 0,
            char_start: 0,
            text: String::new(),
        }];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_start = 0;
    let mut current_char_start = 0;
    let mut cursor = 0;
    let mut char_cursor = 0;

    for line in source.split_inclusive('\n') {
        if current.is_empty() {
            current_start = cursor;
            current_char_start = char_cursor;
        }
        if !current.is_empty() && current.len() + line.len() > EDITOR_TEXT_CHUNK_TARGET_BYTES {
            chunks.push(TextChunk {
                start: current_start,
                char_start: current_char_start,
                text: std::mem::take(&mut current),
            });
            current_start = cursor;
            current_char_start = char_cursor;
        }

        current.push_str(line);
        cursor += line.len();
        char_cursor += line.chars().count();

        if current.len() >= EDITOR_TEXT_CHUNK_TARGET_BYTES {
            chunks.push(TextChunk {
                start: current_start,
                char_start: current_char_start,
                text: std::mem::take(&mut current),
            });
        }
    }

    if !current.is_empty() {
        chunks.push(TextChunk {
            start: current_start,
            char_start: current_char_start,
            text: current,
        });
    }

    chunks
}

fn build_line_index(source: &str, source_len: usize, source_hash: u64) -> LineIndex {
    let chunks = build_text_chunks(source);
    build_line_index_from_chunks(&chunks, source_len, source_hash)
}

fn build_line_index_from_chunks(
    chunks: &[TextChunk],
    source_len: usize,
    source_hash: u64,
) -> LineIndex {
    let mut lines = Vec::new();
    let mut longest_line_chars = 0;
    let mut last_end = 0;

    for chunk in chunks {
        let mut line_start = chunk.start;
        for line in chunk.text.split_inclusive('\n') {
            let end = line_start + line.len();
            let content_end = end
                - line
                    .as_bytes()
                    .iter()
                    .rev()
                    .take_while(|byte| matches!(byte, b'\n' | b'\r'))
                    .count();
            let char_count = line.trim_end_matches(['\r', '\n']).chars().count();
            longest_line_chars = longest_line_chars.max(char_count);
            let leading_len = line
                .bytes()
                .take_while(|byte| matches!(byte, b' ' | b'\t'))
                .count();
            let leading_end = (line_start + leading_len).min(content_end);
            let is_blank = line[..content_end.saturating_sub(line_start)]
                .bytes()
                .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'));
            lines.push(LineInfo {
                start: line_start,
                leading_end,
                content_end,
                end,
                char_count,
                is_blank,
            });
            line_start = end;
            last_end = end;
        }
    }

    if lines.is_empty() || last_end < source_len {
        let start = last_end;
        let trailing = chunks
            .last()
            .and_then(|chunk| chunk.text.get(start.saturating_sub(chunk.start)..))
            .unwrap_or_default();
        let content_end = source_len
            - trailing
                .as_bytes()
                .iter()
                .rev()
                .take_while(|byte| matches!(byte, b'\n' | b'\r'))
                .count();
        let char_count = trailing.trim_end_matches(['\r', '\n']).chars().count();
        longest_line_chars = longest_line_chars.max(char_count);
        let leading_len = trailing
            .bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
        let leading_end = (start + leading_len).min(content_end);
        let is_blank = trailing[..content_end.saturating_sub(start)]
            .bytes()
            .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'));
        lines.push(LineInfo {
            start,
            leading_end,
            content_end,
            end: source_len,
            char_count,
            is_blank,
        });
    }

    let ends_with_newline = chunks
        .last()
        .and_then(|chunk| chunk.text.as_bytes().last())
        .is_some_and(|byte| *byte == b'\n');
    if ends_with_newline {
        lines.push(LineInfo {
            start: source_len,
            leading_end: source_len,
            content_end: source_len,
            end: source_len,
            char_count: 0,
            is_blank: true,
        });
    }

    LineIndex {
        source_len,
        source_hash,
        lines,
        longest_line_chars,
    }
}

fn build_highlight_line_span_ranges(
    spans: &[HighlightSpan],
    line_index: &LineIndex,
) -> Vec<std::ops::Range<usize>> {
    line_index
        .lines
        .iter()
        .map(|line| {
            let start = spans.partition_point(|span| span.range.end <= line.start);
            let end =
                start + spans[start..].partition_point(|span| span.range.start < line.content_end);
            start..end
        })
        .collect()
}

fn highlighted_layout_job(
    ui: &egui::Ui,
    source: &str,
    spans: &[HighlightSpan],
    bracket_pair: Option<(usize, usize)>,
    search_matches: &[SourceSearchMatch],
    active_search_match: Option<usize>,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut cursor = 0;
    let default_format = text_format(ui, None);

    for span in spans {
        let start = clamp_to_char_boundary(source, span.range.start);
        let end = clamp_to_char_boundary(source, span.range.end);

        if start < cursor || start >= end || end > source.len() {
            continue;
        }

        if cursor < start {
            append_text_segment(
                &mut job,
                source,
                cursor,
                start,
                default_format.clone(),
                bracket_pair,
                search_matches,
                active_search_match,
                ui,
            );
        }

        append_text_segment(
            &mut job,
            source,
            start,
            end,
            text_format(ui, Some(span.kind)),
            bracket_pair,
            search_matches,
            active_search_match,
            ui,
        );
        cursor = end;
    }

    if cursor < source.len() {
        append_text_segment(
            &mut job,
            source,
            cursor,
            source.len(),
            default_format,
            bracket_pair,
            search_matches,
            active_search_match,
            ui,
        );
    }

    job
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DiffDisplayRow {
    Section(String),
    FileHeader {
        path: String,
        status: &'static str,
    },
    Hunk(String),
    HiddenLines(usize),
    Line {
        line_label: String,
        content: String,
        kind: DiffLineKind,
    },
    Message(String),
}

#[derive(Debug, Clone, Copy)]
struct DiffCell<'a> {
    line_label: &'a str,
    content: &'a str,
    kind: DiffLineKind,
}

enum DiffRenderRow<'a> {
    Section(&'a str),
    FileHeader {
        path: &'a str,
        status: &'static str,
    },
    HiddenLines(usize),
    UnifiedLine {
        line_label: &'a str,
        marker: &'static str,
        content: &'a str,
        kind: DiffLineKind,
    },
    SideBySide {
        old: DiffCell<'a>,
        new: DiffCell<'a>,
    },
    Message(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffMarkerKind {
    Added,
    Removed,
    Modified,
}

#[derive(Debug, Clone, Copy)]
struct DiffOverviewMarker {
    row_index: usize,
    kind: DiffMarkerKind,
}

#[derive(Debug, Default)]
struct DiffRenderSummary {
    total_rows: usize,
    markers: Vec<DiffOverviewMarker>,
    left_content_width: f32,
    right_content_width: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct DiffSideBySideScroll {
    left_x: f32,
    right_x: f32,
}

fn render_diff_rows(
    ui: &mut egui::Ui,
    rows: &[DiffDisplayRow],
    mode: DiffDisplayMode,
    collapsed_files: &mut HashMap<String, bool>,
    side_selection: &mut Option<DiffSideSelection>,
    unified_selection: &mut Option<DiffUnifiedSelection>,
    scroll_y: f32,
    side_scroll: DiffSideBySideScroll,
) -> DiffRenderSummary {
    let render_rows = diff_render_rows(rows, mode, collapsed_files);
    let markers = diff_overview_markers(&render_rows);
    let row_height = editor_scroll_row_height(ui);
    let gutter_width = diff_gutter_width(ui, rows, mode);
    let (left_content_width, right_content_width) =
        diff_side_by_side_content_widths(ui, &render_rows, gutter_width);
    let start = ((scroll_y / row_height).floor() as usize).saturating_sub(8);
    let visible_count = ((ui.available_height() / row_height).ceil() as usize).saturating_add(18);
    let end = (start + visible_count).min(render_rows.len());

    ui.add_space(start as f32 * row_height);
    let visible_height = (end - start) as f32 * row_height;
    let (visible_rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(1.0), visible_height),
        egui::Sense::empty(),
    );
    handle_diff_selection_input(
        ui,
        &render_rows,
        mode,
        start,
        end,
        visible_rect,
        row_height,
        gutter_width,
        side_selection,
        unified_selection,
        side_scroll,
    );
    paint_diff_visible_background_runs(
        ui,
        &render_rows,
        mode,
        start,
        end,
        visible_rect,
        row_height,
    );
    for (row_index, row) in render_rows[start..end].iter().enumerate() {
        let row_rect = diff_visible_row_rect(visible_rect, row_index, row_height);
        render_diff_render_row(
            ui,
            row_rect,
            start + row_index,
            row,
            collapsed_files,
            gutter_width,
            side_selection.as_ref(),
            unified_selection.as_ref(),
            side_scroll,
        );
    }
    ui.add_space(render_rows.len().saturating_sub(end) as f32 * row_height);

    DiffRenderSummary {
        total_rows: render_rows.len(),
        markers,
        left_content_width,
        right_content_width,
    }
}

fn diff_visible_row_rect(
    visible_rect: egui::Rect,
    row_index: usize,
    row_height: f32,
) -> egui::Rect {
    let top = visible_rect.top() + row_index as f32 * row_height;
    egui::Rect::from_min_size(
        egui::pos2(visible_rect.left(), top),
        egui::vec2(visible_rect.width(), row_height),
    )
}

#[allow(clippy::too_many_arguments)]
fn handle_diff_selection_input(
    ui: &mut egui::Ui,
    rows: &[DiffRenderRow<'_>],
    mode: DiffDisplayMode,
    start: usize,
    end: usize,
    visible_rect: egui::Rect,
    row_height: f32,
    gutter_width: f32,
    side_selection: &mut Option<DiffSideSelection>,
    unified_selection: &mut Option<DiffUnifiedSelection>,
    side_scroll: DiffSideBySideScroll,
) {
    if start >= end || !visible_rect.is_positive() {
        return;
    }
    let _response = ui.interact(
        visible_rect,
        ui.auto_id_with("diff_visible_text_selection"),
        egui::Sense::drag(),
    );
    let Some(pointer_pos) = ui.ctx().pointer_interact_pos() else {
        return;
    };
    let primary_pressed = ui.input(|input| input.pointer.primary_pressed());
    let primary_down = ui.input(|input| input.pointer.primary_down());
    let char_width = editor_char_width(ui);
    let pointer_in_visible = visible_rect.intersect(ui.clip_rect()).contains(pointer_pos);
    match mode {
        DiffDisplayMode::Unified => handle_unified_diff_selection_input(
            ui,
            rows,
            start,
            end,
            visible_rect,
            row_height,
            gutter_width,
            unified_selection,
            char_width,
            pointer_pos,
            primary_pressed && pointer_in_visible,
            primary_down,
        ),
        DiffDisplayMode::SideBySide => handle_side_by_side_diff_selection_input(
            ui,
            rows,
            start,
            end,
            visible_rect,
            row_height,
            gutter_width,
            side_selection,
            side_scroll,
            char_width,
            pointer_pos,
            primary_pressed && pointer_in_visible,
            primary_down,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_unified_diff_selection_input(
    ui: &egui::Ui,
    rows: &[DiffRenderRow<'_>],
    start: usize,
    end: usize,
    visible_rect: egui::Rect,
    row_height: f32,
    gutter_width: f32,
    unified_selection: &mut Option<DiffUnifiedSelection>,
    char_width: f32,
    pointer_pos: egui::Pos2,
    start_selection: bool,
    primary_down: bool,
) {
    if start_selection {
        let Some((row_index, row_rect)) =
            diff_row_at_pointer(visible_rect, start, end, row_height, pointer_pos.y, false)
        else {
            return;
        };
        let Some((text, content_rect)) =
            unified_diff_selection_target(rows, row_index, row_rect, gutter_width)
        else {
            return;
        };
        if !content_rect.contains(pointer_pos) {
            return;
        }
        let point = diff_selection_point_for_pointer(
            row_index,
            &text,
            content_rect.left(),
            pointer_pos.x,
            char_width,
        );
        *unified_selection = Some(DiffUnifiedSelection {
            anchor: point,
            head: point,
            dragging: true,
        });
        ui.ctx().request_repaint();
        return;
    }

    if !primary_down || !unified_selection.is_some_and(|selection| selection.dragging) {
        return;
    }
    let Some((row_index, row_rect)) =
        diff_row_at_pointer(visible_rect, start, end, row_height, pointer_pos.y, true)
    else {
        return;
    };
    let Some((text, content_rect)) =
        unified_diff_selection_target(rows, row_index, row_rect, gutter_width)
    else {
        return;
    };
    if let Some(selection) = unified_selection.as_mut() {
        selection.head = diff_selection_point_for_pointer(
            row_index,
            &text,
            content_rect.left(),
            pointer_pos.x,
            char_width,
        );
        ui.ctx().request_repaint();
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_side_by_side_diff_selection_input(
    ui: &egui::Ui,
    rows: &[DiffRenderRow<'_>],
    start: usize,
    end: usize,
    visible_rect: egui::Rect,
    row_height: f32,
    gutter_width: f32,
    side_selection: &mut Option<DiffSideSelection>,
    side_scroll: DiffSideBySideScroll,
    char_width: f32,
    pointer_pos: egui::Pos2,
    start_selection: bool,
    primary_down: bool,
) {
    if start_selection {
        let Some((row_index, row_rect)) =
            diff_row_at_pointer(visible_rect, start, end, row_height, pointer_pos.y, false)
        else {
            return;
        };
        let Some((side, content, content_rect, text_left)) = side_by_side_diff_selection_target(
            rows,
            row_index,
            row_rect,
            gutter_width,
            side_scroll,
            pointer_pos.x,
        ) else {
            return;
        };
        if !content_rect.contains(pointer_pos) {
            return;
        }
        let point = diff_selection_point_for_pointer(
            row_index,
            &content,
            text_left,
            pointer_pos.x,
            char_width,
        );
        *side_selection = Some(DiffSideSelection {
            side,
            anchor: point,
            head: point,
            dragging: true,
        });
        ui.ctx().request_repaint();
        return;
    }

    let Some(selection) = side_selection.as_mut() else {
        return;
    };
    if !primary_down || !selection.dragging {
        return;
    }
    let Some((row_index, row_rect)) =
        diff_row_at_pointer(visible_rect, start, end, row_height, pointer_pos.y, true)
    else {
        return;
    };
    let Some((content, _content_rect, text_left)) = side_by_side_diff_selection_target_for_side(
        rows,
        row_index,
        row_rect,
        gutter_width,
        side_scroll,
        selection.side,
    ) else {
        return;
    };
    selection.head =
        diff_selection_point_for_pointer(row_index, &content, text_left, pointer_pos.x, char_width);
    ui.ctx().request_repaint();
}

fn diff_row_at_pointer(
    visible_rect: egui::Rect,
    start: usize,
    end: usize,
    row_height: f32,
    pointer_y: f32,
    clamp: bool,
) -> Option<(usize, egui::Rect)> {
    let y = if clamp {
        pointer_y.clamp(visible_rect.top(), visible_rect.bottom() - 0.1)
    } else if (visible_rect.top()..=visible_rect.bottom()).contains(&pointer_y) {
        pointer_y
    } else {
        return None;
    };
    let row_offset = ((y - visible_rect.top()) / row_height).floor().max(0.0) as usize;
    if row_offset >= end.saturating_sub(start) {
        return None;
    }
    Some((
        start + row_offset,
        diff_visible_row_rect(visible_rect, row_offset, row_height),
    ))
}

fn unified_diff_selection_target(
    rows: &[DiffRenderRow<'_>],
    row_index: usize,
    row_rect: egui::Rect,
    gutter_width: f32,
) -> Option<(String, egui::Rect)> {
    let DiffRenderRow::UnifiedLine {
        marker, content, ..
    } = rows.get(row_index)?
    else {
        return None;
    };
    let text = unified_diff_visible_text(marker, content);
    let content_rect = egui::Rect::from_min_max(
        egui::pos2(row_rect.left() + gutter_width + 8.0, row_rect.top()),
        row_rect.max,
    );
    Some((text, content_rect))
}

#[allow(clippy::too_many_arguments)]
fn side_by_side_diff_selection_target(
    rows: &[DiffRenderRow<'_>],
    row_index: usize,
    row_rect: egui::Rect,
    gutter_width: f32,
    side_scroll: DiffSideBySideScroll,
    pointer_x: f32,
) -> Option<(DiffSide, String, egui::Rect, f32)> {
    let width = row_rect.width().max(1.0);
    let center_x = row_rect.left() + width * 0.5;
    if pointer_x < center_x - 1.0 {
        let (content, content_rect, text_left) = side_by_side_diff_selection_target_for_side(
            rows,
            row_index,
            row_rect,
            gutter_width,
            side_scroll,
            DiffSide::Old,
        )?;
        Some((DiffSide::Old, content, content_rect, text_left))
    } else if pointer_x > center_x + 1.0 {
        let (content, content_rect, text_left) = side_by_side_diff_selection_target_for_side(
            rows,
            row_index,
            row_rect,
            gutter_width,
            side_scroll,
            DiffSide::New,
        )?;
        Some((DiffSide::New, content, content_rect, text_left))
    } else {
        None
    }
}

fn side_by_side_diff_selection_target_for_side(
    rows: &[DiffRenderRow<'_>],
    row_index: usize,
    row_rect: egui::Rect,
    gutter_width: f32,
    side_scroll: DiffSideBySideScroll,
    side: DiffSide,
) -> Option<(String, egui::Rect, f32)> {
    let DiffRenderRow::SideBySide { old, new } = rows.get(row_index)? else {
        return None;
    };
    let width = row_rect.width().max(1.0);
    let center_x = row_rect.left() + width * 0.5;
    let (cell, rect, scroll_x) = match side {
        DiffSide::Old => (
            old,
            egui::Rect::from_min_max(row_rect.min, egui::pos2(center_x - 1.0, row_rect.max.y)),
            side_scroll.left_x,
        ),
        DiffSide::New => (
            new,
            egui::Rect::from_min_max(egui::pos2(center_x + 1.0, row_rect.min.y), row_rect.max),
            side_scroll.right_x,
        ),
    };
    let content_rect = egui::Rect::from_min_max(
        egui::pos2((rect.left() + gutter_width).min(rect.right()), rect.top()),
        rect.max,
    );
    let text_left = rect.left() + gutter_width + 8.0 - scroll_x;
    Some((cell.content.to_owned(), content_rect, text_left))
}

fn diff_selection_point_for_pointer(
    row_index: usize,
    content: &str,
    text_left: f32,
    pointer_x: f32,
    char_width: f32,
) -> DiffSideSelectionPoint {
    let byte_column =
        byte_column_for_visual_x(None, content, (pointer_x - text_left).max(0.0), char_width)
            as usize;
    DiffSideSelectionPoint {
        row: row_index,
        column: visual_column_for_byte_column(content, byte_column),
    }
}

fn paint_diff_visible_background_runs(
    ui: &egui::Ui,
    rows: &[DiffRenderRow<'_>],
    mode: DiffDisplayMode,
    start: usize,
    end: usize,
    visible_rect: egui::Rect,
    row_height: f32,
) {
    if start >= end {
        return;
    }
    let left = visible_rect.left();
    let top = visible_rect.top();
    let width = visible_rect.width().max(1.0);
    match mode {
        DiffDisplayMode::Unified => {
            paint_unified_diff_background_runs(ui, rows, start, end, left, top, width, row_height)
        }
        DiffDisplayMode::SideBySide => paint_side_by_side_diff_background_runs(
            ui, rows, start, end, left, top, width, row_height,
        ),
    }
}

fn paint_unified_diff_background_runs(
    ui: &egui::Ui,
    rows: &[DiffRenderRow<'_>],
    start: usize,
    end: usize,
    left: f32,
    top: f32,
    width: f32,
    row_height: f32,
) {
    let mut run_kind = None;
    let mut run_start = start;
    for row_index in start..end {
        let kind = diff_unified_background_kind(&rows[row_index]);
        if kind != run_kind {
            paint_diff_background_run(
                ui, run_kind, run_start, row_index, start, left, top, width, row_height,
            );
            run_kind = kind;
            run_start = row_index;
        }
    }
    paint_diff_background_run(
        ui, run_kind, run_start, end, start, left, top, width, row_height,
    );
}

fn paint_side_by_side_diff_background_runs(
    ui: &egui::Ui,
    rows: &[DiffRenderRow<'_>],
    start: usize,
    end: usize,
    left: f32,
    top: f32,
    width: f32,
    row_height: f32,
) {
    let center_x = left + width * 0.5;
    let left_width = (center_x - 1.0 - left).max(0.0);
    let right_left = center_x + 1.0;
    let right_width = (left + width - right_left).max(0.0);
    paint_side_diff_background_runs_for_side(
        ui,
        rows,
        DiffSide::Old,
        start,
        end,
        left,
        top,
        left_width,
        row_height,
    );
    paint_side_diff_background_runs_for_side(
        ui,
        rows,
        DiffSide::New,
        start,
        end,
        right_left,
        top,
        right_width,
        row_height,
    );
}

fn paint_side_diff_background_runs_for_side(
    ui: &egui::Ui,
    rows: &[DiffRenderRow<'_>],
    side: DiffSide,
    start: usize,
    end: usize,
    left: f32,
    top: f32,
    width: f32,
    row_height: f32,
) {
    if width <= 0.0 {
        return;
    }
    let mut run_kind = None;
    let mut run_start = start;
    for row_index in start..end {
        let kind = diff_side_background_kind(&rows[row_index], side);
        if kind != run_kind {
            paint_diff_background_run(
                ui, run_kind, run_start, row_index, start, left, top, width, row_height,
            );
            run_kind = kind;
            run_start = row_index;
        }
    }
    paint_diff_background_run(
        ui, run_kind, run_start, end, start, left, top, width, row_height,
    );
}

fn diff_unified_background_kind(row: &DiffRenderRow<'_>) -> Option<DiffLineKind> {
    match row {
        DiffRenderRow::UnifiedLine { kind, .. } => diff_visible_background_kind(*kind),
        _ => None,
    }
}

fn diff_side_background_kind(row: &DiffRenderRow<'_>, side: DiffSide) -> Option<DiffLineKind> {
    match row {
        DiffRenderRow::SideBySide { old, new } => match side {
            DiffSide::Old => diff_visible_background_kind(old.kind),
            DiffSide::New => diff_visible_background_kind(new.kind),
        },
        _ => None,
    }
}

fn diff_visible_background_kind(kind: DiffLineKind) -> Option<DiffLineKind> {
    match kind {
        DiffLineKind::Hunk | DiffLineKind::Added | DiffLineKind::Removed => Some(kind),
        DiffLineKind::Header | DiffLineKind::Context => None,
    }
}

fn unified_diff_visible_text(marker: &str, content: &str) -> String {
    if marker.is_empty() {
        format!("  {content}")
    } else {
        format!("{marker} {content}")
    }
}

fn paint_diff_background_run(
    ui: &egui::Ui,
    kind: Option<DiffLineKind>,
    run_start: usize,
    run_end: usize,
    visible_start: usize,
    left: f32,
    top: f32,
    width: f32,
    row_height: f32,
) {
    let Some(kind) = kind else {
        return;
    };
    if run_start >= run_end {
        return;
    }
    let y1 = top + (run_start - visible_start) as f32 * row_height;
    let y2 = top + (run_end - visible_start) as f32 * row_height;
    let rect = egui::Rect::from_min_max(egui::pos2(left, y1), egui::pos2(left + width, y2));
    let rect = diff_row_background_rect(rect);
    ui.painter_at(rect)
        .rect_filled(rect, 0.0, diff_line_background(kind));
}

fn diff_render_rows<'a>(
    rows: &'a [DiffDisplayRow],
    mode: DiffDisplayMode,
    collapsed_files: &HashMap<String, bool>,
) -> Vec<DiffRenderRow<'a>> {
    match mode {
        DiffDisplayMode::Unified => unified_diff_render_rows(rows, collapsed_files),
        DiffDisplayMode::SideBySide => side_by_side_diff_render_rows(rows, collapsed_files),
    }
}

fn diff_overview_markers(rows: &[DiffRenderRow<'_>]) -> Vec<DiffOverviewMarker> {
    rows.iter()
        .enumerate()
        .filter_map(|(row_index, row)| {
            let kind = match row {
                DiffRenderRow::UnifiedLine {
                    kind: DiffLineKind::Added,
                    ..
                } => DiffMarkerKind::Added,
                DiffRenderRow::UnifiedLine {
                    kind: DiffLineKind::Removed,
                    ..
                } => DiffMarkerKind::Removed,
                DiffRenderRow::SideBySide { old, new } => {
                    match (
                        old.kind == DiffLineKind::Removed,
                        new.kind == DiffLineKind::Added,
                    ) {
                        (true, true) => DiffMarkerKind::Modified,
                        (true, false) => DiffMarkerKind::Removed,
                        (false, true) => DiffMarkerKind::Added,
                        (false, false) => return None,
                    }
                }
                _ => return None,
            };
            Some(DiffOverviewMarker { row_index, kind })
        })
        .collect()
}

fn diff_side_by_side_content_widths(
    ui: &egui::Ui,
    rows: &[DiffRenderRow<'_>],
    gutter_width: f32,
) -> (f32, f32) {
    let char_width = editor_char_width(ui);
    let base_width = gutter_width + 20.0;
    let mut left_width: f32 = 0.0;
    let mut right_width: f32 = 0.0;
    for row in rows {
        if let DiffRenderRow::SideBySide { old, new } = row {
            left_width =
                left_width.max(base_width + old.content.chars().count() as f32 * char_width);
            right_width =
                right_width.max(base_width + new.content.chars().count() as f32 * char_width);
        }
    }
    (left_width, right_width)
}

fn unified_diff_render_rows<'a>(
    rows: &'a [DiffDisplayRow],
    collapsed_files: &HashMap<String, bool>,
) -> Vec<DiffRenderRow<'a>> {
    let mut render_rows = Vec::new();
    let mut current_file_collapsed = false;
    for row in rows {
        match row {
            DiffDisplayRow::Section(title) => {
                current_file_collapsed = false;
                render_rows.push(DiffRenderRow::Section(title));
            }
            DiffDisplayRow::FileHeader { path, status } => {
                current_file_collapsed = collapsed_files.get(path).copied().unwrap_or(false);
                render_rows.push(DiffRenderRow::FileHeader { path, status });
            }
            DiffDisplayRow::Hunk(_) => {
                if current_file_collapsed {
                    continue;
                }
            }
            DiffDisplayRow::HiddenLines(count) => {
                if current_file_collapsed {
                    continue;
                }
                render_rows.push(DiffRenderRow::HiddenLines(*count));
            }
            DiffDisplayRow::Line {
                line_label,
                content,
                kind,
            } => {
                if current_file_collapsed {
                    continue;
                }
                let marker = match kind {
                    DiffLineKind::Added => "+",
                    DiffLineKind::Removed => "-",
                    _ => "",
                };
                render_rows.push(DiffRenderRow::UnifiedLine {
                    line_label,
                    marker,
                    content,
                    kind: *kind,
                });
            }
            DiffDisplayRow::Message(text) => {
                if current_file_collapsed {
                    continue;
                }
                render_rows.push(DiffRenderRow::Message(text));
            }
        }
    }
    render_rows
}

fn side_by_side_diff_render_rows<'a>(
    rows: &'a [DiffDisplayRow],
    collapsed_files: &HashMap<String, bool>,
) -> Vec<DiffRenderRow<'a>> {
    let mut render_rows = Vec::new();
    let mut current_file_collapsed = false;
    let mut index = 0;

    while index < rows.len() {
        match &rows[index] {
            DiffDisplayRow::Section(title) => {
                current_file_collapsed = false;
                render_rows.push(DiffRenderRow::Section(title));
                index += 1;
            }
            DiffDisplayRow::FileHeader { path, status } => {
                current_file_collapsed = collapsed_files.get(path).copied().unwrap_or(false);
                render_rows.push(DiffRenderRow::FileHeader { path, status });
                index += 1;
            }
            DiffDisplayRow::Hunk(_) => {
                index += 1;
            }
            DiffDisplayRow::HiddenLines(count) => {
                if !current_file_collapsed {
                    render_rows.push(DiffRenderRow::HiddenLines(*count));
                }
                index += 1;
            }
            DiffDisplayRow::Line {
                line_label,
                content,
                kind,
            } => {
                if current_file_collapsed {
                    index += 1;
                    continue;
                }

                if matches!(kind, DiffLineKind::Added | DiffLineKind::Removed) {
                    index = append_side_by_side_change_block(rows, index, &mut render_rows);
                    continue;
                }

                match kind {
                    DiffLineKind::Context => {
                        let cell = DiffCell {
                            line_label,
                            content,
                            kind: DiffLineKind::Context,
                        };
                        render_rows.push(DiffRenderRow::SideBySide {
                            old: cell,
                            new: cell,
                        });
                    }
                    DiffLineKind::Header | DiffLineKind::Hunk => {}
                    DiffLineKind::Added | DiffLineKind::Removed => {}
                }
                index += 1;
            }
            DiffDisplayRow::Message(text) => {
                if !current_file_collapsed {
                    render_rows.push(DiffRenderRow::Message(text));
                }
                index += 1;
            }
        }
    }
    render_rows
}

fn append_side_by_side_change_block<'a>(
    rows: &'a [DiffDisplayRow],
    start_index: usize,
    render_rows: &mut Vec<DiffRenderRow<'a>>,
) -> usize {
    let mut removed = Vec::new();
    let mut added = Vec::new();
    let mut index = start_index;

    while let Some(DiffDisplayRow::Line {
        line_label,
        content,
        kind,
    }) = rows.get(index)
    {
        match kind {
            DiffLineKind::Removed => removed.push((line_label.as_str(), content.as_str())),
            DiffLineKind::Added => added.push((line_label.as_str(), content.as_str())),
            _ => break,
        }
        index += 1;
    }

    let row_count = removed.len().max(added.len());
    for row_index in 0..row_count {
        let old = removed.get(row_index).copied().unwrap_or(("", ""));
        let new = added.get(row_index).copied().unwrap_or(("", ""));
        render_rows.push(DiffRenderRow::SideBySide {
            old: DiffCell {
                line_label: old.0,
                content: old.1,
                kind: if old.0.is_empty() {
                    DiffLineKind::Context
                } else {
                    DiffLineKind::Removed
                },
            },
            new: DiffCell {
                line_label: new.0,
                content: new.1,
                kind: if new.0.is_empty() {
                    DiffLineKind::Context
                } else {
                    DiffLineKind::Added
                },
            },
        });
    }

    index
}

fn render_diff_render_row(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    row_index: usize,
    row: &DiffRenderRow<'_>,
    collapsed_files: &mut HashMap<String, bool>,
    gutter_width: f32,
    side_selection: Option<&DiffSideSelection>,
    unified_selection: Option<&DiffUnifiedSelection>,
    side_scroll: DiffSideBySideScroll,
) {
    match row {
        DiffRenderRow::Section(title) => {
            ui.painter().text(
                egui::pos2(rect.left() + 4.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                *title,
                FontId::monospace(11.0),
                ui.visuals().weak_text_color(),
            );
        }
        DiffRenderRow::FileHeader { path, status } => {
            diff_file_header_row_ui(ui, rect, path, status, collapsed_files);
        }
        DiffRenderRow::HiddenLines(count) => {
            hidden_lines_row_ui(ui, rect, *count);
        }
        DiffRenderRow::UnifiedLine {
            line_label,
            marker,
            content,
            kind,
        } => {
            unified_diff_row_ui(
                ui,
                rect,
                row_index,
                line_label,
                marker,
                content,
                *kind,
                gutter_width,
                unified_selection,
            );
        }
        DiffRenderRow::SideBySide { old, new } => {
            side_by_side_diff_row_ui(
                ui,
                rect,
                row_index,
                old.line_label,
                old.content,
                old.kind,
                new.line_label,
                new.content,
                new.kind,
                gutter_width,
                side_selection,
                side_scroll,
            );
        }
        DiffRenderRow::Message(text) => {
            ui.painter().text(
                egui::pos2(rect.left() + 4.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                *text,
                editor_font_id(),
                ui.visuals().weak_text_color(),
            );
        }
    }
}

fn paint_diff_overview_markers(
    ui: &egui::Ui,
    rect: egui::Rect,
    summary: &DiffRenderSummary,
    row_height: f32,
) {
    let viewport_rect = rect.intersect(ui.clip_rect());
    if summary.total_rows == 0 || summary.markers.is_empty() || !viewport_rect.is_positive() {
        return;
    }

    let lane_width = 4.0;
    let lane_rect = egui::Rect::from_min_max(
        egui::pos2(
            viewport_rect.right() - lane_width - 2.0,
            viewport_rect.top(),
        ),
        egui::pos2(viewport_rect.right() - 2.0, viewport_rect.bottom()),
    );
    let painter = ui.painter_at(lane_rect);
    painter.rect_filled(
        lane_rect,
        0.0,
        ui.visuals()
            .widgets
            .noninteractive
            .bg_stroke
            .color
            .linear_multiply(0.35),
    );

    let total = summary.total_rows.saturating_sub(1).max(1) as f32;
    let marker_height = row_height.max(3.0).min(8.0);
    for marker in &summary.markers {
        let ratio = marker.row_index as f32 / total;
        let y = (lane_rect.top() + ratio * lane_rect.height())
            .clamp(lane_rect.top(), lane_rect.bottom() - marker_height);
        let marker_rect = egui::Rect::from_min_size(
            egui::pos2(lane_rect.left(), y),
            egui::vec2(lane_rect.width(), marker_height),
        )
        .intersect(lane_rect);
        painter.rect_filled(marker_rect, 0.0, diff_marker_color(marker.kind));
    }
}

fn diff_marker_color(kind: DiffMarkerKind) -> Color32 {
    match kind {
        DiffMarkerKind::Added => Color32::from_rgb(72, 160, 92),
        DiffMarkerKind::Removed => Color32::from_rgb(206, 92, 92),
        DiffMarkerKind::Modified => Color32::from_rgb(196, 160, 72),
    }
}

fn unified_diff_row_ui(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    row_index: usize,
    line_label: &str,
    marker: &str,
    content: &str,
    kind: DiffLineKind,
    gutter_width: f32,
    unified_selection: Option<&DiffUnifiedSelection>,
) {
    let painter = ui.painter_at(rect);
    painter.text(
        egui::pos2(rect.left() + gutter_width - 8.0, rect.top()),
        egui::Align2::RIGHT_TOP,
        line_label,
        editor_font_id(),
        ui.visuals().weak_text_color(),
    );
    let selectable_text = unified_diff_visible_text(marker, content);
    let content_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + gutter_width + 8.0, rect.top()),
        rect.max,
    );
    paint_unified_diff_text(
        ui,
        row_index,
        content_rect,
        &selectable_text,
        diff_line_color(ui, kind),
        unified_selection,
    );
}

fn diff_file_header_row_ui(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    path: &str,
    status: &str,
    collapsed_files: &mut HashMap<String, bool>,
) {
    let collapsed = collapsed_files.get(path).copied().unwrap_or(false);
    let icon = if collapsed { "▶" } else { "▼" };
    let response = ui.interact(
        rect,
        ui.auto_id_with(("diff_file_header", path)),
        egui::Sense::click(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, ui.visuals().faint_bg_color);
    painter.text(
        egui::pos2(rect.left() + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        icon,
        FontId::proportional(12.0),
        ui.visuals().text_color(),
    );
    painter.text(
        egui::pos2(rect.left() + 22.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        path,
        FontId::monospace(13.0),
        ui.visuals().text_color(),
    );
    painter.text(
        egui::pos2(rect.right() - 6.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        status,
        FontId::monospace(11.0),
        ui.visuals().weak_text_color(),
    );
    if response.on_hover_text("Toggle file diff").clicked() {
        collapsed_files.insert(path.to_owned(), !collapsed);
        ui.ctx().request_repaint();
    }
}

fn diff_file_header_ui(
    ui: &mut egui::Ui,
    path: &str,
    status: &str,
    collapsed_files: &mut HashMap<String, bool>,
) -> bool {
    let mut collapsed = collapsed_files.get(path).copied().unwrap_or(false);
    let icon = if collapsed { "▶" } else { "▼" };
    ui.add_space(4.0);
    egui::Frame::NONE
        .fill(ui.visuals().faint_bg_color)
        .inner_margin(egui::Margin::symmetric(4, 2))
        .show(ui, |ui| {
            let (rect, response) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), 18.0), egui::Sense::click());
            let painter = ui.painter_at(rect);
            painter.text(
                egui::pos2(rect.left() + 2.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                icon,
                FontId::proportional(12.0),
                ui.visuals().text_color(),
            );
            painter.text(
                egui::pos2(rect.left() + 18.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                path,
                FontId::monospace(13.0),
                ui.visuals().text_color(),
            );
            painter.text(
                egui::pos2(rect.right() - 4.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                status,
                FontId::monospace(11.0),
                ui.visuals().weak_text_color(),
            );
            if response.on_hover_text("Toggle file diff").clicked() {
                collapsed = !collapsed;
                collapsed_files.insert(path.to_owned(), collapsed);
            }
        });
    collapsed
}

fn diff_row_ui(
    ui: &mut egui::Ui,
    line_label: &str,
    marker: &str,
    content: String,
    kind: DiffLineKind,
) {
    let fill = diff_line_background(kind);
    egui::Frame::NONE
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(0, 1))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_sized(
                    [56.0, 16.0],
                    egui::Label::new(
                        RichText::new(line_label)
                            .monospace()
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    ),
                );
                ui.add_sized(
                    [14.0, 16.0],
                    egui::Label::new(
                        RichText::new(marker)
                            .monospace()
                            .small()
                            .color(diff_line_color(ui, kind)),
                    ),
                );
                ui.label(
                    RichText::new(content)
                        .monospace()
                        .color(diff_line_color(ui, kind)),
                );
            });
        });
}

fn hidden_lines_row_ui(ui: &mut egui::Ui, rect: egui::Rect, count: usize) {
    if count == 0 {
        return;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, ui.visuals().faint_bg_color);
    painter.text(
        egui::pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        format!("◇  非表示 {count} 行"),
        FontId::monospace(11.0),
        ui.visuals().weak_text_color(),
    );
}

fn side_by_side_hunk_ui(ui: &mut egui::Ui, text: &str) {
    let width = ui.available_width().max(900.0);
    egui::Frame::NONE
        .fill(diff_line_background(DiffLineKind::Hunk))
        .inner_margin(egui::Margin::symmetric(0, 1))
        .show(ui, |ui| {
            ui.add_sized(
                [width, 18.0],
                egui::Label::new(
                    RichText::new(text)
                        .monospace()
                        .color(diff_line_color(ui, DiffLineKind::Hunk)),
                ),
            );
        });
}

fn side_by_side_diff_row_ui(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    row_index: usize,
    old_label: &str,
    old_content: &str,
    old_kind: DiffLineKind,
    new_label: &str,
    new_content: &str,
    new_kind: DiffLineKind,
    gutter_width: f32,
    side_selection: Option<&DiffSideSelection>,
    side_scroll: DiffSideBySideScroll,
) {
    let width = rect.width().max(1.0);
    let center_x = rect.left() + width * 0.5;
    let left_rect = egui::Rect::from_min_max(rect.min, egui::pos2(center_x - 1.0, rect.max.y));
    let right_rect = egui::Rect::from_min_max(egui::pos2(center_x + 1.0, rect.min.y), rect.max);
    let painter = ui.painter_at(rect);

    painter.line_segment(
        [
            egui::pos2(center_x, rect.top()),
            egui::pos2(center_x, rect.bottom()),
        ],
        egui::Stroke::new(2.0, ui.visuals().widgets.noninteractive.fg_stroke.color),
    );

    let row_y = rect.center().y;
    paint_side_by_side_cell_text(
        ui,
        row_index,
        DiffSide::Old,
        left_rect,
        row_y,
        old_label,
        old_content,
        old_kind,
        gutter_width,
        side_selection,
        side_scroll.left_x,
    );
    paint_side_by_side_cell_text(
        ui,
        row_index,
        DiffSide::New,
        right_rect,
        row_y,
        new_label,
        new_content,
        new_kind,
        gutter_width,
        side_selection,
        side_scroll.right_x,
    );
}

fn paint_side_by_side_cell_text(
    ui: &mut egui::Ui,
    row_index: usize,
    side: DiffSide,
    rect: egui::Rect,
    _y: f32,
    line_label: &str,
    content: &str,
    kind: DiffLineKind,
    gutter_width: f32,
    side_selection: Option<&DiffSideSelection>,
    scroll_x: f32,
) {
    let gutter_rect = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(
            (rect.left() + gutter_width).min(rect.right()),
            rect.bottom(),
        ),
    );
    let content_rect = egui::Rect::from_min_max(
        egui::pos2((rect.left() + gutter_width).min(rect.right()), rect.top()),
        rect.max,
    );
    let gutter_painter = ui.painter_at(gutter_rect);

    gutter_painter.text(
        egui::pos2(rect.left() + gutter_width - 8.0, rect.top()),
        egui::Align2::RIGHT_TOP,
        line_label,
        editor_font_id(),
        ui.visuals().weak_text_color(),
    );
    let text_left = rect.left() + gutter_width + 8.0 - scroll_x;
    let char_width = editor_char_width(ui);
    paint_diff_side_selection_for_cell(
        ui,
        content_rect,
        content,
        row_index,
        side,
        side_selection,
        text_left,
        char_width,
    );
    let content_painter = ui.painter_at(content_rect);
    content_painter.text(
        egui::pos2(text_left, rect.center().y),
        egui::Align2::LEFT_CENTER,
        content,
        editor_font_id(),
        diff_line_color(ui, kind),
    );
}

fn paint_diff_side_selection_for_cell(
    ui: &egui::Ui,
    content_rect: egui::Rect,
    content: &str,
    row_index: usize,
    side: DiffSide,
    selection: Option<&DiffSideSelection>,
    text_left: f32,
    char_width: f32,
) {
    let Some((start_column, end_column)) =
        diff_side_selection_columns_for_cell(selection, side, row_index, content.chars().count())
    else {
        return;
    };

    let start_byte = char_index_to_byte_index(content, start_column).unwrap_or(content.len());
    let end_byte = char_index_to_byte_index(content, end_column).unwrap_or(content.len());
    let x1 = text_left + visual_x_for_byte_column(Some(ui), content, start_byte, char_width);
    let mut x2 = text_left + visual_x_for_byte_column(Some(ui), content, end_byte, char_width);
    if x2 <= x1 {
        x2 = x1 + char_width;
    }
    let rect = egui::Rect::from_min_max(
        egui::pos2(x1, content_rect.top()),
        egui::pos2(x2, content_rect.bottom()),
    )
    .intersect(content_rect);
    if rect.is_positive() {
        ui.painter_at(content_rect)
            .rect_filled(rect, 0.0, ui.visuals().selection.bg_fill);
    }
}

fn diff_side_selection_columns_for_cell(
    selection: Option<&DiffSideSelection>,
    side: DiffSide,
    row_index: usize,
    char_count: usize,
) -> Option<(usize, usize)> {
    let selection = selection?;
    if selection.side != side {
        return None;
    }
    let (start, end) = selection.normalized()?;
    if row_index < start.row || row_index > end.row {
        return None;
    }
    let start_column = if row_index == start.row {
        start.column
    } else {
        0
    }
    .min(char_count);
    let end_column = if row_index == end.row {
        end.column
    } else {
        char_count
    }
    .min(char_count);
    (start_column < end_column).then_some((start_column, end_column))
}

fn diff_side_selection_text_from_render_rows(
    rows: &[DiffRenderRow<'_>],
    selection: DiffSideSelection,
) -> Option<String> {
    let (start, end) = selection.normalized()?;
    let mut lines = Vec::new();
    for row_index in start.row..=end.row {
        let Some(DiffRenderRow::SideBySide { old, new }) = rows.get(row_index) else {
            continue;
        };
        let cell = match selection.side {
            DiffSide::Old => old,
            DiffSide::New => new,
        };
        if cell.line_label.is_empty() {
            continue;
        }
        let content = cell.content;
        let Some((start_column, end_column)) = diff_side_selection_columns_for_cell(
            Some(&selection),
            selection.side,
            row_index,
            content.chars().count(),
        ) else {
            if content.is_empty() {
                lines.push(String::new());
            }
            continue;
        };
        let start_byte = char_index_to_byte_index(content, start_column).unwrap_or(content.len());
        let end_byte = char_index_to_byte_index(content, end_column).unwrap_or(content.len());
        lines.push(content[start_byte..end_byte].to_owned());
    }

    if lines.is_empty() {
        return None;
    }
    let text = lines.join("\n");
    (!text.is_empty()).then_some(text)
}

fn diff_unified_selection_text_from_render_rows(
    rows: &[DiffRenderRow<'_>],
    selection: DiffUnifiedSelection,
) -> Option<String> {
    let (start, end) = selection.normalized()?;
    let mut lines = Vec::new();
    for row_index in start.row..=end.row {
        let Some(DiffRenderRow::UnifiedLine {
            marker, content, ..
        }) = rows.get(row_index)
        else {
            continue;
        };
        let line = if marker.is_empty() {
            format!("  {content}")
        } else {
            format!("{marker} {content}")
        };
        let Some((start_column, end_column)) = diff_unified_selection_columns_for_line(
            Some(&selection),
            row_index,
            line.chars().count(),
        ) else {
            if line.is_empty() {
                lines.push(String::new());
            }
            continue;
        };
        let start_byte = char_index_to_byte_index(&line, start_column).unwrap_or(line.len());
        let end_byte = char_index_to_byte_index(&line, end_column).unwrap_or(line.len());
        lines.push(line[start_byte..end_byte].to_owned());
    }

    if lines.is_empty() {
        return None;
    }
    let text = lines.join("\n");
    (!text.is_empty()).then_some(text)
}

fn paint_unified_diff_text(
    ui: &mut egui::Ui,
    row_index: usize,
    rect: egui::Rect,
    text: &str,
    color: Color32,
    unified_selection: Option<&DiffUnifiedSelection>,
) {
    let clip_rect = rect.intersect(ui.clip_rect());
    if !rect.is_positive() || !clip_rect.is_positive() {
        return;
    }
    let text_left = rect.left();
    let char_width = editor_char_width(ui);
    paint_diff_unified_selection_for_line(
        ui,
        rect,
        text,
        row_index,
        unified_selection,
        text_left,
        char_width,
    );
    ui.painter_at(clip_rect).text(
        egui::pos2(text_left, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        editor_font_id(),
        color,
    );
}

fn paint_diff_unified_selection_for_line(
    ui: &egui::Ui,
    content_rect: egui::Rect,
    content: &str,
    row_index: usize,
    selection: Option<&DiffUnifiedSelection>,
    text_left: f32,
    char_width: f32,
) {
    let Some((start_column, end_column)) =
        diff_unified_selection_columns_for_line(selection, row_index, content.chars().count())
    else {
        return;
    };

    let start_byte = char_index_to_byte_index(content, start_column).unwrap_or(content.len());
    let end_byte = char_index_to_byte_index(content, end_column).unwrap_or(content.len());
    let x1 = text_left + visual_x_for_byte_column(Some(ui), content, start_byte, char_width);
    let mut x2 = text_left + visual_x_for_byte_column(Some(ui), content, end_byte, char_width);
    if x2 <= x1 {
        x2 = x1 + char_width;
    }
    let rect = egui::Rect::from_min_max(
        egui::pos2(x1, content_rect.top()),
        egui::pos2(x2, content_rect.bottom()),
    )
    .intersect(content_rect);
    if rect.is_positive() {
        ui.painter_at(content_rect)
            .rect_filled(rect, 0.0, ui.visuals().selection.bg_fill);
    }
}

fn diff_unified_selection_columns_for_line(
    selection: Option<&DiffUnifiedSelection>,
    row_index: usize,
    char_count: usize,
) -> Option<(usize, usize)> {
    let selection = selection?;
    let (start, end) = selection.normalized()?;
    if row_index < start.row || row_index > end.row {
        return None;
    }
    let start_column = if row_index == start.row {
        start.column
    } else {
        0
    }
    .min(char_count);
    let end_column = if row_index == end.row {
        end.column
    } else {
        char_count
    }
    .min(char_count);
    (start_column < end_column).then_some((start_column, end_column))
}

fn diff_side_by_side_scrollbars_ui(
    ui: &mut egui::Ui,
    left_scroll_x: &mut f32,
    right_scroll_x: &mut f32,
    summary: &DiffRenderSummary,
    viewport_width: f32,
) {
    let pane_width = (viewport_width * 0.5).max(1.0);
    let left_max = (summary.left_content_width - pane_width).max(0.0);
    let right_max = (summary.right_content_width - pane_width).max(0.0);
    *left_scroll_x = left_scroll_x.clamp(0.0, left_max);
    *right_scroll_x = right_scroll_x.clamp(0.0, right_max);

    if left_max <= 0.0 && right_max <= 0.0 {
        ui.add_space(4.0);
        return;
    }

    ui.add_space(2.0);
    let width = ui.available_width().max(1.0);
    let height = 16.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let center_x = rect.left() + rect.width() * 0.5;
    let left_rect = egui::Rect::from_min_max(rect.min, egui::pos2(center_x - 1.0, rect.bottom()));
    let right_rect = egui::Rect::from_min_max(egui::pos2(center_x + 1.0, rect.top()), rect.max);

    let left_changed = diff_side_scrollbar_ui(
        ui,
        left_rect,
        left_scroll_x,
        left_max,
        summary.left_content_width,
        pane_width,
        "left_diff_horizontal_scroll",
    );
    if left_changed {
        *right_scroll_x = left_scroll_x.clamp(0.0, right_max);
    }

    let right_changed = diff_side_scrollbar_ui(
        ui,
        right_rect,
        right_scroll_x,
        right_max,
        summary.right_content_width,
        pane_width,
        "right_diff_horizontal_scroll",
    );
    if right_changed {
        *left_scroll_x = right_scroll_x.clamp(0.0, left_max);
    }

    ui.painter().line_segment(
        [
            egui::pos2(center_x, rect.top()),
            egui::pos2(center_x, rect.bottom()),
        ],
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.fg_stroke.color),
    );
}

fn diff_side_scrollbar_ui(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    value: &mut f32,
    max: f32,
    content_width: f32,
    viewport_width: f32,
    id_salt: &'static str,
) -> bool {
    if !rect.is_positive() {
        return false;
    }

    let track_rect = rect.shrink2(egui::vec2(8.0, 3.0));
    let painter = ui.painter_at(rect);
    let track_color = if ui.visuals().dark_mode {
        Color32::from_gray(64)
    } else {
        Color32::from_gray(210)
    };
    painter.rect_filled(track_rect, 2.0, track_color);

    if max <= 0.0 {
        return false;
    }

    let id = ui.make_persistent_id(id_salt);
    let response = ui.interact(rect, id, egui::Sense::click_and_drag());
    let thumb_width = (track_rect.width() * (viewport_width / content_width.max(viewport_width)))
        .clamp(24.0, track_rect.width());
    let travel_width = (track_rect.width() - thumb_width).max(1.0);
    let ratio = (*value / max).clamp(0.0, 1.0);
    let mut thumb_left = track_rect.left() + ratio * travel_width;
    let mut changed = false;

    if (response.clicked() || response.dragged())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let pointer_ratio =
            ((pointer.x - track_rect.left() - thumb_width * 0.5) / travel_width).clamp(0.0, 1.0);
        *value = pointer_ratio * max;
        thumb_left = track_rect.left() + pointer_ratio * travel_width;
        ui.ctx().request_repaint();
        changed = true;
    }

    let thumb_rect = egui::Rect::from_min_size(
        egui::pos2(thumb_left, track_rect.top()),
        egui::vec2(thumb_width, track_rect.height()),
    );
    let thumb_color = if response.dragged() {
        if ui.visuals().dark_mode {
            Color32::from_gray(180)
        } else {
            Color32::from_gray(100)
        }
    } else if response.hovered() {
        if ui.visuals().dark_mode {
            Color32::from_gray(155)
        } else {
            Color32::from_gray(125)
        }
    } else if ui.visuals().dark_mode {
        Color32::from_gray(130)
    } else {
        Color32::from_gray(150)
    };
    painter.rect_filled(thumb_rect, 2.0, thumb_color);
    changed
}

fn git_history_min_width() -> f32 {
    1120.0
}

fn git_history_header_ui(ui: &mut egui::Ui, width: f32) {
    let row_height = 24.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, row_height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, ui.visuals().faint_bg_color);
    for (label, x) in [
        ("Graph", 8.0),
        ("Description", 220.0),
        ("Date", 760.0),
        ("Author", 910.0),
        ("Commit", 1040.0),
    ] {
        painter.text(
            egui::pos2(rect.left() + x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            FontId::monospace(13.0),
            ui.visuals().strong_text_color(),
        );
    }
}

fn git_history_row_ui(
    ui: &mut egui::Ui,
    row: &GitGraphRow,
    width: f32,
    selected: bool,
) -> egui::Response {
    let row_height = if row.commit.is_some() { 28.0 } else { 6.0 };
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, row_height), egui::Sense::click());
    let painter = ui.painter_at(rect);
    if selected {
        painter.rect_filled(rect, 0.0, ui.visuals().selection.bg_fill);
    }
    if response.hovered() {
        painter.rect_filled(
            rect,
            0.0,
            if selected {
                ui.visuals().selection.bg_fill
            } else {
                ui.visuals().faint_bg_color
            },
        );
    }

    let graph_rect =
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + 204.0, rect.bottom()));
    paint_git_history_graph(ui, &painter, graph_rect, &row.graph);

    let Some(commit) = row.commit.as_ref() else {
        return response;
    };

    let text_color = if selected {
        ui.visuals().selection.stroke.color
    } else {
        ui.visuals().text_color()
    };
    let weak_text_color = if selected {
        ui.visuals().selection.stroke.color
    } else {
        ui.visuals().weak_text_color()
    };
    let description_left = rect.left() + 220.0;
    let date_left = rect.left() + 760.0;
    let author_left = rect.left() + 910.0;
    let commit_left = rect.left() + 1040.0;
    let description_right = date_left - 12.0;
    let description_clip_rect = egui::Rect::from_min_max(
        egui::pos2(description_left, rect.top()),
        egui::pos2(description_right, rect.bottom()),
    );
    let description_painter = painter.with_clip_rect(description_clip_rect);
    let mut description_x = description_left;
    for reference in commit.refs.iter().take(4) {
        let width = (reference.chars().count() as f32 * 7.0 + 14.0).clamp(28.0, 128.0);
        if description_x + width > description_right {
            let ellipsis_font = FontId::monospace(11.0);
            let ellipsis = elide_text_to_width(
                ui,
                "...",
                ellipsis_font.clone(),
                weak_text_color,
                description_right - description_x,
            );
            if !ellipsis.is_empty() {
                description_painter.text(
                    egui::pos2(description_x, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    ellipsis,
                    ellipsis_font,
                    weak_text_color,
                );
            }
            description_x = description_right;
            break;
        }
        let badge_rect = egui::Rect::from_min_size(
            egui::pos2(description_x, rect.center().y - 8.0),
            egui::vec2(width, 16.0),
        );
        description_painter.rect_filled(badge_rect, 3.0, git_ref_badge_color(reference));
        let ref_font = FontId::monospace(11.0);
        let reference_label =
            elide_text_to_width(ui, reference, ref_font.clone(), Color32::WHITE, width - 8.0);
        description_painter.text(
            badge_rect.center(),
            egui::Align2::CENTER_CENTER,
            reference_label,
            ref_font,
            Color32::WHITE,
        );
        description_x += width + 4.0;
    }

    let subject_font = editor_font_id();
    let subject = elide_text_to_width(
        ui,
        &commit.subject,
        subject_font.clone(),
        text_color,
        description_right - description_x,
    );
    description_painter.text(
        egui::pos2(description_x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        subject,
        subject_font,
        text_color,
    );
    paint_git_history_text_cell(
        ui,
        &painter,
        rect,
        date_left,
        author_left - 10.0,
        &commit.date,
        FontId::monospace(13.0),
        text_color,
    );
    paint_git_history_text_cell(
        ui,
        &painter,
        rect,
        author_left,
        commit_left - 10.0,
        &commit.author,
        FontId::monospace(13.0),
        text_color,
    );
    paint_git_history_text_cell(
        ui,
        &painter,
        rect,
        commit_left,
        rect.right(),
        &commit.short_hash,
        FontId::monospace(13.0),
        weak_text_color,
    );
    response
}

fn paint_git_history_text_cell(
    ui: &egui::Ui,
    painter: &egui::Painter,
    row_rect: egui::Rect,
    left: f32,
    right: f32,
    text: &str,
    font_id: FontId,
    color: Color32,
) {
    let cell_rect = egui::Rect::from_min_max(
        egui::pos2(left, row_rect.top()),
        egui::pos2(right.max(left), row_rect.bottom()),
    );
    let label = elide_text_to_width(ui, text, font_id.clone(), color, cell_rect.width());
    painter.with_clip_rect(cell_rect).text(
        cell_rect.left_center(),
        egui::Align2::LEFT_CENTER,
        label,
        font_id,
        color,
    );
}

fn paint_git_history_graph(ui: &egui::Ui, painter: &egui::Painter, rect: egui::Rect, graph: &str) {
    let cell_width = 10.0;
    let origin_x = rect.left() + 18.0;
    for (column, character) in graph.chars().enumerate() {
        let x = origin_x + column as f32 * cell_width;
        let color = git_graph_color(column / 2);
        let stroke = egui::Stroke::new(2.0, color);
        let top = rect.top();
        let middle = rect.center().y;
        let bottom = rect.bottom();
        match character {
            '|' => {
                painter.line_segment([egui::pos2(x, top), egui::pos2(x, bottom)], stroke);
            }
            '*' => {
                painter.line_segment([egui::pos2(x, top), egui::pos2(x, bottom)], stroke);
                painter.circle_filled(egui::pos2(x, middle), 5.0, color);
                painter.circle_stroke(
                    egui::pos2(x, middle),
                    5.0,
                    egui::Stroke::new(1.5, ui.visuals().extreme_bg_color),
                );
            }
            '/' => {
                painter.line_segment(
                    [
                        egui::pos2(x + cell_width * 0.5, top),
                        egui::pos2(x - cell_width * 0.5, bottom),
                    ],
                    stroke,
                );
            }
            '\\' => {
                painter.line_segment(
                    [
                        egui::pos2(x - cell_width * 0.5, top),
                        egui::pos2(x + cell_width * 0.5, bottom),
                    ],
                    stroke,
                );
            }
            '_' | '-' => {
                painter.line_segment(
                    [
                        egui::pos2(x - cell_width * 0.5, middle),
                        egui::pos2(x + cell_width * 0.5, middle),
                    ],
                    stroke,
                );
            }
            _ => {}
        }
    }
}

fn git_ref_badge_color(reference: &str) -> Color32 {
    if reference.starts_with("origin/") {
        Color32::from_rgb(48, 119, 164)
    } else if reference.starts_with("tag:") {
        Color32::from_rgb(138, 118, 34)
    } else {
        Color32::from_rgb(20, 128, 180)
    }
}

fn diff_view_min_width(rows: &[DiffDisplayRow], mode: DiffDisplayMode) -> f32 {
    let max_chars = rows
        .iter()
        .map(diff_row_char_width)
        .max()
        .unwrap_or(80)
        .clamp(80, 260);
    match mode {
        DiffDisplayMode::Unified => 96.0 + max_chars as f32 * 8.0,
        DiffDisplayMode::SideBySide => 980.0 + max_chars.min(120) as f32 * 4.0,
    }
}

fn diff_gutter_width(ui: &egui::Ui, rows: &[DiffDisplayRow], mode: DiffDisplayMode) -> f32 {
    let digits = rows
        .iter()
        .filter_map(|row| match row {
            DiffDisplayRow::Line { line_label, .. } => Some(line_label.chars().count()),
            _ => None,
        })
        .max()
        .unwrap_or(1);
    let row_height = editor_scroll_row_height(ui);
    let base_width = (digits as f32 * row_height * 0.6 + 22.0).max(38.0);
    match mode {
        DiffDisplayMode::Unified => base_width,
        DiffDisplayMode::SideBySide => base_width.max(50.0),
    }
}

fn diff_row_char_width(row: &DiffDisplayRow) -> usize {
    match row {
        DiffDisplayRow::Section(title) => title.chars().count(),
        DiffDisplayRow::FileHeader { path, .. } => path.chars().count(),
        DiffDisplayRow::Hunk(text) | DiffDisplayRow::Message(text) => text.chars().count(),
        DiffDisplayRow::HiddenLines(count) => format!("非表示 {count} 行").chars().count(),
        DiffDisplayRow::Line { content, .. } => content.chars().count(),
    }
}

fn parse_diff_display_rows(source: &str) -> Vec<DiffDisplayRow> {
    if source.trim().is_empty() {
        return vec![DiffDisplayRow::Message("No diffs.".to_owned())];
    }

    let mut rows = Vec::new();
    let mut old_line = 0usize;
    let mut new_line = 0usize;
    let mut in_hunk = false;
    let mut has_seen_hunk_in_file = false;
    let mut current_file_row = None;

    for raw_line in source.lines() {
        if let Some(title) = raw_line.strip_prefix("diff --code-reader ") {
            rows.push(DiffDisplayRow::Section(title.to_owned()));
            in_hunk = false;
            has_seen_hunk_in_file = false;
            current_file_row = None;
            continue;
        }

        if raw_line.starts_with("diff --git ") {
            let path = diff_git_path(raw_line).unwrap_or_else(|| raw_line.to_owned());
            current_file_row = Some(rows.len());
            rows.push(DiffDisplayRow::FileHeader { path, status: "M" });
            in_hunk = false;
            has_seen_hunk_in_file = false;
            continue;
        }

        if raw_line.starts_with("new file mode ") {
            if let Some(index) = current_file_row
                && let Some(DiffDisplayRow::FileHeader { status, .. }) = rows.get_mut(index)
            {
                *status = "A";
            }
            continue;
        }

        if raw_line.starts_with("deleted file mode ") {
            if let Some(index) = current_file_row
                && let Some(DiffDisplayRow::FileHeader { status, .. }) = rows.get_mut(index)
            {
                *status = "D";
            }
            continue;
        }

        if raw_line.starts_with("index ")
            || raw_line.starts_with("--- ")
            || raw_line.starts_with("+++ ")
        {
            continue;
        }

        if raw_line.starts_with("@@") {
            if let Some((next_old_line, next_new_line)) = parse_unified_hunk_header(raw_line) {
                if has_seen_hunk_in_file {
                    let hidden_count = next_old_line
                        .saturating_sub(old_line)
                        .max(next_new_line.saturating_sub(new_line));
                    if hidden_count > 0 {
                        rows.push(DiffDisplayRow::HiddenLines(hidden_count));
                    }
                }
                old_line = next_old_line;
                new_line = next_new_line;
                in_hunk = true;
                has_seen_hunk_in_file = true;
            }
            continue;
        }

        if !in_hunk {
            if raw_line.trim().is_empty() {
                continue;
            }
            rows.push(DiffDisplayRow::Message(raw_line.to_owned()));
            continue;
        }

        if raw_line.starts_with('+') {
            rows.push(DiffDisplayRow::Line {
                line_label: format!("+{new_line}"),
                content: raw_line[1..].to_owned(),
                kind: DiffLineKind::Added,
            });
            new_line += 1;
        } else if raw_line.starts_with('-') {
            rows.push(DiffDisplayRow::Line {
                line_label: format!("-{old_line}"),
                content: raw_line[1..].to_owned(),
                kind: DiffLineKind::Removed,
            });
            old_line += 1;
        } else if raw_line.starts_with(' ') || raw_line.is_empty() {
            let content = raw_line.strip_prefix(' ').unwrap_or(raw_line).to_owned();
            rows.push(DiffDisplayRow::Line {
                line_label: new_line.to_string(),
                content,
                kind: DiffLineKind::Context,
            });
            old_line += 1;
            new_line += 1;
        } else if raw_line.starts_with("\\ No newline") {
            continue;
        }
    }

    if rows.is_empty() {
        rows.push(DiffDisplayRow::Message("No diffs.".to_owned()));
    }
    rows
}

fn diff_git_path(line: &str) -> Option<String> {
    line.split_whitespace()
        .nth(3)
        .map(strip_git_diff_prefix)
        .filter(|path| !path.is_empty())
}

fn strip_git_diff_prefix(path: &str) -> String {
    path.strip_prefix("b/")
        .or_else(|| path.strip_prefix("a/"))
        .unwrap_or(path)
        .to_owned()
}

fn parse_unified_hunk_header(line: &str) -> Option<(usize, usize)> {
    Some((
        parse_hunk_line_number(line, '-')?,
        parse_hunk_line_number(line, '+')?,
    ))
}

fn parse_hunk_line_number(line: &str, marker: char) -> Option<usize> {
    let start = line.find(marker)? + marker.len_utf8();
    let digits = line[start..]
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

fn diff_layout_job(ui: &egui::Ui, source: &str) -> LayoutJob {
    let mut job = LayoutJob::default();
    if source.is_empty() {
        job.append("", 0.0, diff_text_format(ui, DiffLineKind::Context));
        return job;
    }

    for line in source.split_inclusive('\n') {
        let kind = diff_line_kind(line);
        job.append(line, 0.0, diff_text_format(ui, kind));
    }
    job
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffLineKind {
    Header,
    Hunk,
    Added,
    Removed,
    Context,
}

fn diff_line_kind(line: &str) -> DiffLineKind {
    if line.starts_with("@@") {
        DiffLineKind::Hunk
    } else if line.starts_with("diff --git")
        || line.starts_with("index ")
        || line.starts_with("new file mode ")
        || line.starts_with("deleted file mode ")
        || line.starts_with("--- ")
        || line.starts_with("+++ ")
    {
        DiffLineKind::Header
    } else if line.starts_with('+') {
        DiffLineKind::Added
    } else if line.starts_with('-') {
        DiffLineKind::Removed
    } else {
        DiffLineKind::Context
    }
}

fn diff_text_format(ui: &egui::Ui, kind: DiffLineKind) -> TextFormat {
    let (color, background) = match kind {
        DiffLineKind::Header => (ui.visuals().weak_text_color(), Color32::TRANSPARENT),
        DiffLineKind::Hunk => (
            Color32::from_rgb(86, 156, 214),
            Color32::from_rgba_unmultiplied(86, 156, 214, 22),
        ),
        DiffLineKind::Added => (
            Color32::from_rgb(72, 160, 92),
            Color32::from_rgba_unmultiplied(72, 160, 92, 24),
        ),
        DiffLineKind::Removed => (
            Color32::from_rgb(206, 92, 92),
            Color32::from_rgba_unmultiplied(206, 92, 92, 24),
        ),
        DiffLineKind::Context => (ui.visuals().text_color(), Color32::TRANSPARENT),
    };

    TextFormat {
        font_id: editor_font_id(),
        color,
        background,
        ..Default::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkdownSourceSpanKind {
    Heading,
    Strong,
    Emphasis,
    InlineCode,
    CodeBlock,
    Link,
    Blockquote,
    ListMarker,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MarkdownSourceSpan {
    range: std::ops::Range<usize>,
    kind: MarkdownSourceSpanKind,
}

fn markdown_source_layout_job(ui: &egui::Ui, source: &str) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut cursor = 0;
    let mut spans = markdown_source_spans(source);
    let default_format = markdown_source_text_format(ui, None);

    spans.sort_by_key(|span| (span.range.start, span.range.end));

    for span in spans {
        let start = clamp_to_char_boundary(source, span.range.start);
        let end = clamp_to_char_boundary(source, span.range.end);

        if start < cursor || start >= end || end > source.len() {
            continue;
        }

        if cursor < start {
            job.append(&source[cursor..start], 0.0, default_format.clone());
        }
        job.append(
            &source[start..end],
            0.0,
            markdown_source_text_format(ui, Some(span.kind)),
        );
        cursor = end;
    }

    if cursor < source.len() {
        job.append(&source[cursor..], 0.0, default_format);
    }

    job
}

fn markdown_source_spans(source: &str) -> Vec<MarkdownSourceSpan> {
    let mut spans = Vec::new();
    let mut line_start = 0;
    let mut fenced_code_marker: Option<String> = None;

    for line in source.split_inclusive('\n') {
        let line_end = line_start + line.len();
        let line_without_newline = line.trim_end_matches(['\r', '\n']);
        let content_end = line_start + line_without_newline.len();
        let leading_spaces = line_without_newline
            .bytes()
            .take_while(|byte| *byte == b' ' || *byte == b'\t')
            .count();
        let content_start = line_start + leading_spaces;
        let trimmed = &line_without_newline[leading_spaces..];

        if let Some(marker) = &fenced_code_marker {
            spans.push(MarkdownSourceSpan {
                range: line_start..content_end,
                kind: MarkdownSourceSpanKind::CodeBlock,
            });
            if trimmed.starts_with(marker) {
                fenced_code_marker = None;
            }
            line_start = line_end;
            continue;
        }

        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let marker = trimmed.chars().take(3).collect::<String>();
            fenced_code_marker = Some(marker);
            spans.push(MarkdownSourceSpan {
                range: content_start..content_end,
                kind: MarkdownSourceSpanKind::CodeBlock,
            });
            line_start = line_end;
            continue;
        }

        if let Some(hash_count) = markdown_heading_marker_len(trimmed) {
            spans.push(MarkdownSourceSpan {
                range: content_start..content_end,
                kind: MarkdownSourceSpanKind::Heading,
            });
            spans.push(MarkdownSourceSpan {
                range: content_start..content_start + hash_count,
                kind: MarkdownSourceSpanKind::ListMarker,
            });
        } else {
            if trimmed.starts_with('>') {
                spans.push(MarkdownSourceSpan {
                    range: content_start..(content_start + 1),
                    kind: MarkdownSourceSpanKind::Blockquote,
                });
            }

            if let Some(marker_len) = markdown_list_marker_len(trimmed) {
                spans.push(MarkdownSourceSpan {
                    range: content_start..content_start + marker_len,
                    kind: MarkdownSourceSpanKind::ListMarker,
                });
            }

            markdown_inline_source_spans(line_without_newline, line_start, &mut spans);
        }

        line_start = line_end;
    }

    spans
}

fn markdown_heading_marker_len(line: &str) -> Option<usize> {
    let count = line.bytes().take_while(|byte| *byte == b'#').count();
    (1..=6).contains(&count).then_some(count).filter(|count| {
        line.as_bytes()
            .get(*count)
            .is_some_and(u8::is_ascii_whitespace)
    })
}

fn markdown_list_marker_len(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    if bytes.len() >= 2 && matches!(bytes[0], b'-' | b'*' | b'+') && bytes[1].is_ascii_whitespace()
    {
        return Some(1);
    }

    let digit_count = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digit_count > 0
        && bytes.get(digit_count) == Some(&b'.')
        && bytes
            .get(digit_count + 1)
            .is_some_and(u8::is_ascii_whitespace)
    {
        return Some(digit_count + 1);
    }

    None
}

fn markdown_inline_source_spans(
    line: &str,
    line_start: usize,
    spans: &mut Vec<MarkdownSourceSpan>,
) {
    markdown_pair_spans(
        line,
        line_start,
        "`",
        MarkdownSourceSpanKind::InlineCode,
        spans,
    );
    markdown_pair_spans(
        line,
        line_start,
        "**",
        MarkdownSourceSpanKind::Strong,
        spans,
    );
    markdown_pair_spans(
        line,
        line_start,
        "__",
        MarkdownSourceSpanKind::Strong,
        spans,
    );
    markdown_pair_spans(
        line,
        line_start,
        "*",
        MarkdownSourceSpanKind::Emphasis,
        spans,
    );
    markdown_pair_spans(
        line,
        line_start,
        "_",
        MarkdownSourceSpanKind::Emphasis,
        spans,
    );
    markdown_link_spans(line, line_start, spans);
}

fn markdown_pair_spans(
    line: &str,
    line_start: usize,
    marker: &str,
    kind: MarkdownSourceSpanKind,
    spans: &mut Vec<MarkdownSourceSpan>,
) {
    let mut cursor = 0;
    while let Some(open) = line[cursor..].find(marker).map(|index| cursor + index) {
        if marker.len() == 1 && line[open..].starts_with(&marker.repeat(2)) {
            cursor = open + 2;
            continue;
        }
        let content_start = open + marker.len();
        let Some(close) = line[content_start..]
            .find(marker)
            .map(|index| content_start + index)
        else {
            break;
        };
        if close > content_start {
            spans.push(MarkdownSourceSpan {
                range: line_start + open..line_start + close + marker.len(),
                kind,
            });
        }
        cursor = close + marker.len();
    }
}

fn markdown_link_spans(line: &str, line_start: usize, spans: &mut Vec<MarkdownSourceSpan>) {
    let mut cursor = 0;
    while let Some(open) = line[cursor..].find('[').map(|index| cursor + index) {
        let Some(close_label) = line[open + 1..].find(']').map(|index| open + 1 + index) else {
            break;
        };
        if line.as_bytes().get(close_label + 1) != Some(&b'(') {
            cursor = close_label + 1;
            continue;
        }
        let Some(close_url) = line[close_label + 2..]
            .find(')')
            .map(|index| close_label + 2 + index)
        else {
            break;
        };
        spans.push(MarkdownSourceSpan {
            range: line_start + open..line_start + close_url + 1,
            kind: MarkdownSourceSpanKind::Link,
        });
        cursor = close_url + 1;
    }
}

fn markdown_source_text_format(ui: &egui::Ui, kind: Option<MarkdownSourceSpanKind>) -> TextFormat {
    let mut format = TextFormat {
        font_id: editor_font_id(),
        color: ui.visuals().text_color(),
        ..Default::default()
    };

    match kind {
        Some(MarkdownSourceSpanKind::Heading) => {
            format.color = Color32::from_rgb(128, 0, 0);
        }
        Some(MarkdownSourceSpanKind::Strong) => {
            format.color = ui.visuals().strong_text_color();
        }
        Some(MarkdownSourceSpanKind::Emphasis) => {
            format.italics = true;
        }
        Some(MarkdownSourceSpanKind::InlineCode) => {
            format.color = Color32::from_rgb(163, 21, 21);
            format.background = markdown_code_background(ui);
            format.expand_bg = 2.0;
        }
        Some(MarkdownSourceSpanKind::CodeBlock) => {
            format.color = Color32::from_rgb(163, 21, 21);
        }
        Some(MarkdownSourceSpanKind::Link) => {
            format.color = Color32::from_rgb(4, 81, 165);
            format.underline = egui::Stroke::new(1.0, format.color);
        }
        Some(MarkdownSourceSpanKind::Blockquote) => {
            format.color = Color32::from_rgb(0, 128, 0);
        }
        Some(MarkdownSourceSpanKind::ListMarker) => {
            format.color = Color32::from_rgb(4, 81, 165);
        }
        None => {}
    }

    format
}

fn append_text_segment(
    job: &mut LayoutJob,
    source: &str,
    start: usize,
    end: usize,
    format: TextFormat,
    bracket_pair: Option<(usize, usize)>,
    search_matches: &[SourceSearchMatch],
    active_search_match: Option<usize>,
    ui: &egui::Ui,
) {
    let mut cursor = start;
    while cursor < end {
        let mut next = end;
        let mut background = None;

        if let Some((left, right)) = bracket_pair {
            for bracket in [left, right] {
                if bracket < cursor || bracket >= end {
                    continue;
                }
                if cursor < bracket {
                    next = next.min(bracket);
                } else {
                    next = next.min(next_char_boundary(source, bracket));
                    background.get_or_insert_with(|| matching_bracket_background(ui));
                }
            }
        }

        let search_index =
            search_matches.partition_point(|search_match| search_match.end <= cursor);
        if let Some(search_match) = search_matches.get(search_index)
            && search_match.start < end
        {
            if cursor < search_match.start {
                next = next.min(search_match.start);
            } else if cursor < search_match.end {
                next = next.min(search_match.end);
                background = Some(if active_search_match == Some(search_index) {
                    active_source_search_match_background(ui)
                } else {
                    source_search_match_background(ui)
                });
            }
        }

        if next <= cursor {
            next = next_char_boundary(source, cursor);
        }

        let mut next_format = format.clone();
        if let Some(background) = background {
            next_format.background = background;
        }
        job.append(&source[cursor..next], 0.0, next_format);
        cursor = next;
    }
}

fn next_char_boundary(source: &str, index: usize) -> usize {
    source[index..]
        .chars()
        .next()
        .map(|character| index + character.len_utf8())
        .unwrap_or(index)
        .min(source.len())
}

fn text_format(ui: &egui::Ui, kind: Option<HighlightKind>) -> TextFormat {
    let color = match kind {
        Some(HighlightKind::Comment) => Color32::from_rgb(0, 128, 0),
        Some(HighlightKind::String) => Color32::from_rgb(163, 21, 21),
        Some(HighlightKind::Number) => Color32::from_rgb(9, 134, 88),
        Some(HighlightKind::Keyword) => Color32::from_rgb(0, 0, 255),
        Some(HighlightKind::Function) => Color32::from_rgb(121, 94, 38),
        Some(HighlightKind::Type) => Color32::from_rgb(38, 127, 153),
        None => ui.visuals().text_color(),
    };

    TextFormat {
        font_id: editor_font_id(),
        color,
        ..Default::default()
    }
}

fn diff_line_color(ui: &egui::Ui, kind: DiffLineKind) -> Color32 {
    match kind {
        DiffLineKind::Header => ui.visuals().weak_text_color(),
        DiffLineKind::Hunk => Color32::from_rgb(86, 156, 214),
        DiffLineKind::Added => Color32::from_rgb(72, 160, 92),
        DiffLineKind::Removed => Color32::from_rgb(206, 92, 92),
        DiffLineKind::Context => ui.visuals().text_color(),
    }
}

fn diff_line_background(kind: DiffLineKind) -> Color32 {
    match kind {
        DiffLineKind::Hunk => Color32::from_rgba_unmultiplied(86, 156, 214, 22),
        DiffLineKind::Added => Color32::from_rgba_unmultiplied(72, 160, 92, 24),
        DiffLineKind::Removed => Color32::from_rgba_unmultiplied(206, 92, 92, 24),
        DiffLineKind::Header | DiffLineKind::Context => Color32::TRANSPARENT,
    }
}

fn diff_row_background_rect(rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() - 0.5),
        egui::pos2(rect.right(), rect.bottom() + 0.5),
    )
}

fn elapsed_ms(start: Instant) -> f32 {
    start.elapsed().as_secs_f32() * 1000.0
}

fn allocate_line_number_gutter(ui: &mut egui::Ui, line_count: usize) -> egui::Rect {
    let row_height = editor_scroll_row_height(ui);
    let visible_rows = line_count.max(TEXTEDIT_SOURCE_MIN_ROWS);
    let gutter_width = line_number_gutter_width(ui, line_count);
    let height = visible_rows as f32 * row_height;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(gutter_width, height), egui::Sense::hover());

    rect
}

fn line_number_gutter_width(ui: &egui::Ui, line_count: usize) -> f32 {
    let row_height = editor_scroll_row_height(ui);
    let digits = line_count.max(1).to_string().len();
    (digits as f32 * row_height * 0.6 + 22.0 + BOOKMARK_GUTTER_WIDTH).max(56.0)
}

fn paint_line_number_gutter(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    line_count: usize,
    current_line: Option<u32>,
    bookmark_lines: &[u32],
    output: &egui::text_edit::TextEditOutput,
    viewport_rect: egui::Rect,
) -> Option<u32> {
    let painter = ui.painter_at(rect.intersect(viewport_rect));
    let font_id = editor_font_id();
    let inactive_color = ui.visuals().weak_text_color();
    let active_color = ui.visuals().text_color();
    let bookmark_color = ui.visuals().selection.bg_fill;
    let mut clicked_bookmark_line = None;

    for index in visible_galley_row_range(output, line_count, viewport_rect) {
        let row_rect = galley_line_rect(output, index);
        let line_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left(), row_rect.top()),
            egui::vec2(rect.width(), row_rect.height()),
        )
        .intersect(viewport_rect);

        if current_line == Some(index as u32) {
            painter.rect_filled(line_rect, 0.0, current_line_background(ui));
        }

        let bookmark_rect = egui::Rect::from_min_max(
            egui::pos2(rect.left(), line_rect.top()),
            egui::pos2(
                (rect.left() + BOOKMARK_GUTTER_WIDTH).min(rect.right()),
                line_rect.bottom(),
            ),
        );
        let bookmark_response = ui.interact(
            bookmark_rect,
            output.response.id.with(("bookmark_gutter", index)),
            egui::Sense::click(),
        );
        let bookmarked = bookmark_lines.contains(&(index as u32));
        if bookmarked || bookmark_response.hovered() || current_line == Some(index as u32) {
            painter.text(
                bookmark_rect.center(),
                egui::Align2::CENTER_CENTER,
                if bookmarked { "★" } else { "☆" },
                bookmark_gutter_font_id(line_rect.height()),
                if bookmarked {
                    bookmark_color
                } else {
                    inactive_color
                },
            );
        }
        if bookmark_response.clicked() {
            clicked_bookmark_line = Some(index as u32);
        }

        painter.text(
            egui::pos2(rect.right() - 8.0, row_rect.top()),
            egui::Align2::RIGHT_TOP,
            (index + 1).to_string(),
            font_id.clone(),
            if current_line == Some(index as u32) {
                active_color
            } else {
                inactive_color
            },
        );
    }

    painter.line_segment(
        [
            egui::pos2(rect.right(), viewport_rect.top()),
            egui::pos2(rect.right(), viewport_rect.bottom()),
        ],
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );

    clicked_bookmark_line
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VirtualIndentGuideLine {
    line_number: usize,
    guide_count: usize,
    active: bool,
}

fn virtual_indent_guide_lines(
    line_index: &LineIndex,
    text: &EditorTextBuffer,
    visible_range: std::ops::Range<usize>,
    tab_width: usize,
    current_line: Option<u32>,
) -> Vec<VirtualIndentGuideLine> {
    let tab_width = tab_width.max(1);
    visible_range
        .filter_map(|line_number| {
            let line_info = line_index.line(line_number)?;
            if line_info.is_blank {
                return None;
            }

            let line = text.line_text_or_empty(line_info);
            let leading_len = line_info
                .leading_end
                .saturating_sub(line_info.start)
                .min(line.len());
            let indent_columns = leading_indent_columns(&line[..leading_len], tab_width);
            let guide_count = indent_columns / tab_width;
            (guide_count > 0).then_some(VirtualIndentGuideLine {
                line_number,
                guide_count,
                active: current_line == Some(line_number as u32),
            })
        })
        .collect()
}

fn paint_virtual_indent_guides(
    ui: &egui::Ui,
    line_index: &LineIndex,
    text: &EditorTextBuffer,
    visible_range: std::ops::Range<usize>,
    tab_width: usize,
    current_line: Option<u32>,
    text_left: f32,
    row_height: f32,
    scroll_y: f32,
    clip_rect: egui::Rect,
) {
    let guide_lines =
        virtual_indent_guide_lines(line_index, text, visible_range, tab_width, current_line);
    let max_guide_count = guide_lines
        .iter()
        .map(|line| line.guide_count)
        .max()
        .unwrap_or(0);
    if max_guide_count == 0 {
        return;
    }

    let tab_width = tab_width.max(1);
    let char_width = editor_char_width(ui);
    let painter = ui.painter_at(clip_rect);
    let guide_color = indent_guide_color(ui);
    let active_guide_color = active_indent_guide_color(ui);

    for guide_index in 1..=max_guide_count {
        let x = text_left + char_width * (guide_index * tab_width) as f32;
        if !clip_rect.x_range().contains(x) {
            continue;
        }

        let mut run_start = None;
        let mut previous_line = None;
        for guide_line in guide_lines.iter().copied() {
            let line_has_guide = guide_line.guide_count >= guide_index;
            let continues_run =
                previous_line.is_some_and(|previous| guide_line.line_number == previous + 1);

            if !line_has_guide || guide_line.active || !continues_run {
                if let (Some(start), Some(end)) = (run_start.take(), previous_line) {
                    paint_virtual_indent_guide_run(
                        &painter,
                        x,
                        start,
                        end,
                        row_height,
                        scroll_y,
                        clip_rect,
                        guide_color,
                    );
                }
            }

            if line_has_guide && guide_line.active {
                paint_virtual_indent_guide_run(
                    &painter,
                    x,
                    guide_line.line_number,
                    guide_line.line_number,
                    row_height,
                    scroll_y,
                    clip_rect,
                    active_guide_color,
                );
            } else if line_has_guide && run_start.is_none() {
                run_start = Some(guide_line.line_number);
            }

            previous_line = Some(guide_line.line_number);
        }

        if let (Some(start), Some(end)) = (run_start, previous_line) {
            paint_virtual_indent_guide_run(
                &painter,
                x,
                start,
                end,
                row_height,
                scroll_y,
                clip_rect,
                guide_color,
            );
        }
    }
}

fn paint_virtual_indent_guide_run(
    painter: &egui::Painter,
    x: f32,
    start_line: usize,
    end_line: usize,
    row_height: f32,
    scroll_y: f32,
    clip_rect: egui::Rect,
    color: Color32,
) {
    let top = clip_rect.top() + start_line as f32 * row_height - scroll_y;
    let bottom = clip_rect.top() + (end_line + 1) as f32 * row_height - scroll_y;
    painter.line_segment(
        [
            egui::pos2(x, top.max(clip_rect.top())),
            egui::pos2(x, bottom.min(clip_rect.bottom())),
        ],
        egui::Stroke::new(1.0, color),
    );
}

fn paint_virtual_selection(
    ui: &egui::Ui,
    line_index: &LineIndex,
    text: &EditorTextBuffer,
    selection: Option<NormalizedVirtualSelection>,
    visible_range: std::ops::Range<usize>,
    text_left: f32,
    text_clip_rect: egui::Rect,
    row_height: f32,
    scroll_y: f32,
    char_width: f32,
    viewport_rect: egui::Rect,
) {
    let Some(selection) = selection else {
        return;
    };

    let start_line = selection.start.line as usize;
    let end_line = selection.end.line as usize;
    let visible_start = visible_range.start.max(start_line);
    let visible_end = visible_range.end.min(end_line.saturating_add(1));
    if visible_start >= visible_end {
        return;
    }

    let painter = ui.painter_at(text_clip_rect.intersect(viewport_rect));
    let fill = ui.visuals().selection.bg_fill;
    for line_number in visible_start..visible_end {
        let Some(line_info) = line_index.line(line_number) else {
            continue;
        };
        let line = text.line_text_or_empty(line_info);
        let line_len = line.len();
        let start_byte = if line_number == start_line {
            selection.start.character as usize
        } else {
            0
        }
        .min(line_len);
        let end_byte = if line_number == end_line {
            selection.end.character as usize
        } else {
            line_len
        }
        .min(line_len);
        if line_number == start_line && line_number == end_line && start_byte >= end_byte {
            continue;
        }

        let x1 = text_left + visual_x_for_byte_column(Some(ui), line, start_byte, char_width);
        let mut x2 = text_left + visual_x_for_byte_column(Some(ui), line, end_byte, char_width);
        if x2 <= x1 && line_number != end_line {
            x2 = x1 + char_width;
        }

        let top = viewport_rect.top() + line_number as f32 * row_height - scroll_y;
        let rect = egui::Rect::from_min_max(egui::pos2(x1, top), egui::pos2(x2, top + row_height))
            .intersect(text_clip_rect)
            .intersect(viewport_rect);
        if rect.is_positive() {
            painter.rect_filled(rect, 0.0, fill);
        }
    }
}

fn virtual_selection_text(
    text: &EditorTextBuffer,
    selection: NormalizedVirtualSelection,
) -> Option<String> {
    let range = virtual_selection_byte_range(text, selection)?;
    text.as_str().get(range).map(ToOwned::to_owned)
}

fn virtual_selection_byte_range(
    text: &EditorTextBuffer,
    selection: NormalizedVirtualSelection,
) -> Option<std::ops::Range<usize>> {
    let start = virtual_position_to_byte_index(text, selection.start)?;
    let end = virtual_position_to_byte_index(text, selection.end)?;
    if start >= end {
        return None;
    }
    Some(start..end)
}

fn virtual_word_selection_at_position(
    line: &str,
    line_number: u32,
    byte_column: u32,
) -> Option<VirtualTextSelection> {
    let byte_column = clamp_to_char_boundary(line, (byte_column as usize).min(line.len()));
    let target = if line[byte_column..]
        .chars()
        .next()
        .is_some_and(is_editor_word_char)
    {
        byte_column
    } else if byte_column > 0 {
        let previous = previous_char_boundary(line, byte_column);
        if line[previous..]
            .chars()
            .next()
            .is_some_and(is_editor_word_char)
        {
            previous
        } else {
            return None;
        }
    } else {
        return None;
    };

    let mut start = target;
    while start > 0 {
        let previous = previous_char_boundary(line, start);
        let Some(character) = line[previous..].chars().next() else {
            break;
        };
        if !is_editor_word_char(character) {
            break;
        }
        start = previous;
    }

    let mut end = target;
    while end < line.len() {
        let Some(character) = line[end..].chars().next() else {
            break;
        };
        if !is_editor_word_char(character) {
            break;
        }
        end += character.len_utf8();
    }

    (start < end).then_some(VirtualTextSelection {
        anchor: Position::new(line_number, start as u32),
        head: Position::new(line_number, end as u32),
    })
}

fn is_editor_word_char(character: char) -> bool {
    character == '_' || character.is_alphanumeric()
}

fn virtual_position_to_byte_index(text: &EditorTextBuffer, position: Position) -> Option<usize> {
    let line = text.line_index.line(position.line as usize)?;
    let target = (line.start + position.character as usize).min(line.content_end);
    Some(clamp_to_char_boundary(text.as_str(), target))
}

fn previous_char_boundary(source: &str, byte_index: usize) -> usize {
    let mut index = byte_index.min(source.len());
    if index == 0 {
        return 0;
    }
    index -= 1;
    while index > 0 && !source.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn virtual_input_text(text: &str, _source: &EditorTextBuffer, _cursor_index: usize) -> String {
    text.to_owned()
}

fn paint_virtual_cursor(
    ui: &egui::Ui,
    line_index: &LineIndex,
    text: &EditorTextBuffer,
    cursor_position: Option<Position>,
    visible_range: std::ops::Range<usize>,
    text_left: f32,
    text_clip_rect: egui::Rect,
    row_height: f32,
    scroll_y: f32,
    char_width: f32,
    viewport_rect: egui::Rect,
) {
    let Some(rect) = virtual_cursor_rect(
        Some(ui),
        line_index,
        text,
        cursor_position,
        visible_range,
        text_left,
        text_clip_rect,
        row_height,
        scroll_y,
        char_width,
        viewport_rect,
    ) else {
        return;
    };

    let painter = ui.painter_at(text_clip_rect.intersect(viewport_rect));
    painter.line_segment(
        [rect.left_top(), rect.left_bottom()],
        egui::Stroke::new(1.5, ui.visuals().widgets.active.fg_stroke.color),
    );
}

fn virtual_cursor_rect(
    ui: Option<&egui::Ui>,
    line_index: &LineIndex,
    text: &EditorTextBuffer,
    cursor_position: Option<Position>,
    visible_range: std::ops::Range<usize>,
    text_left: f32,
    text_clip_rect: egui::Rect,
    row_height: f32,
    scroll_y: f32,
    char_width: f32,
    viewport_rect: egui::Rect,
) -> Option<egui::Rect> {
    let position = cursor_position?;
    let line_number = position.line as usize;
    if !visible_range.contains(&line_number) {
        return None;
    }
    let line_info = line_index.line(line_number)?;
    let line = text.line_text_or_empty(line_info);
    let x = text_left + visual_x_for_byte_column(ui, line, position.character as usize, char_width);
    if !text_clip_rect.x_range().contains(x) {
        return None;
    }

    let top = viewport_rect.top() + line_number as f32 * row_height - scroll_y + 2.0;
    let bottom = (top + row_height - 4.0).min(viewport_rect.bottom());
    if bottom <= viewport_rect.top() || top >= viewport_rect.bottom() {
        return None;
    }

    Some(egui::Rect::from_min_max(
        egui::pos2(x, top.max(viewport_rect.top())),
        egui::pos2(x + 1.5, bottom),
    ))
}

fn paint_virtual_ime_preedit(
    ui: &egui::Ui,
    preedit: Option<&str>,
    cursor_rect: Option<egui::Rect>,
    text_clip_rect: egui::Rect,
    viewport_rect: egui::Rect,
    row_height: f32,
) {
    let Some(preedit) = preedit.filter(|text| !text.is_empty()) else {
        return;
    };
    let Some(cursor_rect) = cursor_rect else {
        return;
    };
    let painter = ui.painter_at(text_clip_rect.intersect(viewport_rect));
    let position = cursor_rect.left_top();
    let font_id = TextStyle::Monospace.resolve(ui.style());
    painter.text(
        position,
        egui::Align2::LEFT_TOP,
        preedit,
        font_id,
        ui.visuals().text_color(),
    );
    let width = preedit.chars().count().max(1) as f32 * editor_char_width(ui);
    let underline_y = (position.y + row_height - 3.0).min(viewport_rect.bottom());
    painter.line_segment(
        [
            egui::pos2(position.x, underline_y),
            egui::pos2(position.x + width, underline_y),
        ],
        egui::Stroke::new(1.0, ui.visuals().widgets.active.fg_stroke.color),
    );
}

fn highlighted_line_layout_job(
    ui: &egui::Ui,
    source: &str,
    line: LineInfo,
    spans: &[HighlightSpan],
    line_span_range: std::ops::Range<usize>,
    bracket_pair: Option<(usize, usize)>,
    search_matches: &[SourceSearchMatch],
    active_search_match: Option<usize>,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut cursor = line.start;
    let default_format = text_format(ui, None);

    let span_start = line_span_range.start.min(spans.len());
    let span_end = line_span_range.end.min(spans.len());
    for span in &spans[span_start..span_end] {
        let start = clamp_to_char_boundary(source, span.range.start.max(line.start));
        let end = clamp_to_char_boundary(source, span.range.end.min(line.content_end));

        if start < cursor || start >= end || end > line.content_end {
            continue;
        }

        if cursor < start {
            append_text_segment(
                &mut job,
                source,
                cursor,
                start,
                default_format.clone(),
                bracket_pair,
                search_matches,
                active_search_match,
                ui,
            );
        }

        append_text_segment(
            &mut job,
            source,
            start,
            end,
            text_format(ui, Some(span.kind)),
            bracket_pair,
            search_matches,
            active_search_match,
            ui,
        );
        cursor = end;
    }

    if cursor < line.content_end {
        append_text_segment(
            &mut job,
            source,
            cursor,
            line.content_end,
            default_format,
            bracket_pair,
            search_matches,
            active_search_match,
            ui,
        );
    }

    job
}

fn capture_virtual_editor_render_debug(
    line_count: usize,
    row_height: f32,
    scroll_y: f32,
    viewport_rect: egui::Rect,
    visible_rows: std::ops::Range<usize>,
    current_line: Option<u32>,
) -> EditorRenderDebug {
    let first_visible_row = (visible_rows.start < visible_rows.end).then_some(visible_rows.start);
    let first_visible_top =
        first_visible_row.map(|line| viewport_rect.top() + line as f32 * row_height - scroll_y);
    let last_visible_row = visible_rows.end.checked_sub(1);

    EditorRenderDebug {
        line_count,
        text_line_count: line_count,
        galley_rows: line_count,
        scroll_y,
        row_height,
        clip_top: viewport_rect.top(),
        clip_bottom: viewport_rect.bottom(),
        viewport_top: viewport_rect.top(),
        viewport_bottom: viewport_rect.bottom(),
        galley_pos_y: viewport_rect.top() - scroll_y,
        first_visible_row,
        first_visible_top,
        last_visible_row,
        anchor_line: Some(visible_rows.start),
        anchor_top: first_visible_top,
        anchored_range: Some(visible_rows),
        current_line,
    }
}

fn handle_and_paint_virtual_scrollbars(
    ui: &egui::Ui,
    viewport_rect: egui::Rect,
    total_width: f32,
    total_height: f32,
    scroll_x: &mut f32,
    scroll_y: &mut f32,
) {
    let painter = ui.painter_at(viewport_rect);
    let track_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let layout = virtual_scrollbar_layout(viewport_rect, total_width, total_height);

    if let Some(track_rect) = layout.vertical_rect {
        let ratio = (layout.content_rect.height() / total_height).clamp(0.02, 1.0);
        let handle_height = (layout.content_rect.height() * ratio).max(24.0);
        let max_handle_top = (track_rect.height() - handle_height).max(0.0);
        let max_scroll = (total_height - layout.content_rect.height()).max(1.0);
        let id = ui.make_persistent_id("virtual_source_vertical_scrollbar");
        let response = ui.interact(track_rect, id, egui::Sense::click_and_drag());
        let mut handle_top =
            track_rect.top() + max_handle_top * ((*scroll_y / max_scroll).clamp(0.0, 1.0));
        if (response.clicked() || response.dragged())
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let pointer_ratio = ((pointer.y - track_rect.top() - handle_height * 0.5)
                / max_handle_top.max(1.0))
            .clamp(0.0, 1.0);
            *scroll_y = pointer_ratio * max_scroll;
            handle_top = track_rect.top() + max_handle_top * pointer_ratio;
            ui.ctx().request_repaint();
        }
        let handle_color = virtual_scrollbar_handle_color(ui, &response);
        let rect = egui::Rect::from_min_size(
            egui::pos2(track_rect.right() - VIRTUAL_SCROLLBAR_BAR_WIDTH, handle_top),
            egui::vec2(VIRTUAL_SCROLLBAR_BAR_WIDTH, handle_height),
        );
        painter.rect_filled(rect, 3.0, handle_color);
        painter.line_segment(
            [
                egui::pos2(
                    track_rect.right() - VIRTUAL_SCROLLBAR_BAR_WIDTH,
                    track_rect.top(),
                ),
                egui::pos2(
                    track_rect.right() - VIRTUAL_SCROLLBAR_BAR_WIDTH,
                    track_rect.bottom(),
                ),
            ],
            egui::Stroke::new(1.0, track_color),
        );
    }

    if let Some(track_rect) = layout.horizontal_rect {
        let ratio = (layout.content_rect.width() / total_width).clamp(0.02, 1.0);
        let handle_width = (layout.content_rect.width() * ratio).max(24.0);
        let max_handle_left = (track_rect.width() - handle_width).max(0.0);
        let max_scroll = (total_width - layout.content_rect.width()).max(1.0);
        let id = ui.make_persistent_id("virtual_source_horizontal_scrollbar");
        let response = ui.interact(track_rect, id, egui::Sense::click_and_drag());
        let mut handle_left =
            track_rect.left() + max_handle_left * ((*scroll_x / max_scroll).clamp(0.0, 1.0));
        if (response.clicked() || response.dragged())
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let pointer_ratio = ((pointer.x - track_rect.left() - handle_width * 0.5)
                / max_handle_left.max(1.0))
            .clamp(0.0, 1.0);
            *scroll_x = pointer_ratio * max_scroll;
            handle_left = track_rect.left() + max_handle_left * pointer_ratio;
            ui.ctx().request_repaint();
        }
        let handle_color = virtual_scrollbar_handle_color(ui, &response);
        let rect = egui::Rect::from_min_size(
            egui::pos2(
                handle_left,
                track_rect.bottom() - VIRTUAL_SCROLLBAR_BAR_WIDTH,
            ),
            egui::vec2(handle_width, VIRTUAL_SCROLLBAR_BAR_WIDTH),
        );
        painter.rect_filled(rect, 3.0, handle_color);
        painter.line_segment(
            [
                egui::pos2(
                    track_rect.left(),
                    track_rect.bottom() - VIRTUAL_SCROLLBAR_BAR_WIDTH,
                ),
                egui::pos2(
                    track_rect.right(),
                    track_rect.bottom() - VIRTUAL_SCROLLBAR_BAR_WIDTH,
                ),
            ],
            egui::Stroke::new(1.0, track_color),
        );
    }
}

fn virtual_scrollbar_layout(
    viewport_rect: egui::Rect,
    total_width: f32,
    total_height: f32,
) -> VirtualScrollbarLayout {
    let mut content_rect = viewport_rect;
    let mut needs_vertical = total_height > content_rect.height();
    let mut needs_horizontal = total_width > content_rect.width();

    if needs_vertical {
        content_rect.max.x =
            (content_rect.max.x - VIRTUAL_SCROLLBAR_HIT_WIDTH).max(content_rect.min.x + 1.0);
    }
    if needs_horizontal {
        content_rect.max.y =
            (content_rect.max.y - VIRTUAL_SCROLLBAR_HIT_WIDTH).max(content_rect.min.y + 1.0);
    }

    if !needs_vertical && total_height > content_rect.height() {
        needs_vertical = true;
        content_rect.max.x =
            (content_rect.max.x - VIRTUAL_SCROLLBAR_HIT_WIDTH).max(content_rect.min.x + 1.0);
    }
    if !needs_horizontal && total_width > content_rect.width() {
        needs_horizontal = true;
        content_rect.max.y =
            (content_rect.max.y - VIRTUAL_SCROLLBAR_HIT_WIDTH).max(content_rect.min.y + 1.0);
    }

    let vertical_rect = needs_vertical.then(|| {
        egui::Rect::from_min_max(
            egui::pos2(content_rect.right(), content_rect.top()),
            egui::pos2(viewport_rect.right(), content_rect.bottom()),
        )
    });
    let horizontal_rect = needs_horizontal.then(|| {
        egui::Rect::from_min_max(
            egui::pos2(content_rect.left(), content_rect.bottom()),
            egui::pos2(content_rect.right(), viewport_rect.bottom()),
        )
    });

    VirtualScrollbarLayout {
        content_rect,
        vertical_rect,
        horizontal_rect,
    }
}

fn virtual_scrollbar_hit_rects(
    viewport_rect: egui::Rect,
    total_width: f32,
    total_height: f32,
) -> (Option<egui::Rect>, Option<egui::Rect>) {
    let layout = virtual_scrollbar_layout(viewport_rect, total_width, total_height);
    (layout.vertical_rect, layout.horizontal_rect)
}

fn virtual_scrollbar_hit_contains(
    vertical_rect: Option<egui::Rect>,
    horizontal_rect: Option<egui::Rect>,
    position: egui::Pos2,
) -> bool {
    vertical_rect.is_some_and(|rect| rect.contains(position))
        || horizontal_rect.is_some_and(|rect| rect.contains(position))
}

fn virtual_scrollbar_handle_color(ui: &egui::Ui, response: &egui::Response) -> Color32 {
    if response.dragged() {
        ui.visuals().widgets.active.fg_stroke.color
    } else if response.hovered() {
        ui.visuals().widgets.hovered.fg_stroke.color
    } else {
        ui.visuals().widgets.inactive.fg_stroke.color
    }
}

fn paint_current_line_background(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    viewport_rect: egui::Rect,
    current_line: Option<u32>,
) {
    let Some(current_line) = current_line else {
        return;
    };

    let row_rect = galley_line_rect(output, current_line as usize);
    if row_rect.bottom() < viewport_rect.top() || row_rect.top() > viewport_rect.bottom() {
        return;
    }
    let line_rect = egui::Rect::from_min_max(
        egui::pos2(output.text_clip_rect.left(), row_rect.top()),
        egui::pos2(output.text_clip_rect.right(), row_rect.bottom()),
    )
    .intersect(viewport_rect);

    if line_rect.is_positive() {
        ui.painter()
            .rect_filled(line_rect, 0.0, current_line_overlay(ui));
    }
}

fn paint_indent_guides(
    ui: &egui::Ui,
    source: &str,
    tab_width: usize,
    current_line: Option<u32>,
    output: &egui::text_edit::TextEditOutput,
    viewport_rect: egui::Rect,
) {
    let tab_width = tab_width.max(1);
    let char_width = editor_char_width(ui);
    let painter = ui.painter_at(output.text_clip_rect.intersect(viewport_rect));
    let guide_color = indent_guide_color(ui);
    let active_guide_color = active_indent_guide_color(ui);
    let line_count = source.lines().count().max(1);

    let visible_lines = visible_galley_row_range(output, line_count, viewport_rect);
    for (index, line) in source
        .lines()
        .enumerate()
        .skip(visible_lines.start)
        .take(visible_lines.end.saturating_sub(visible_lines.start))
    {
        let indent_columns = leading_indent_columns(line, tab_width);
        let guide_count = indent_columns / tab_width;
        if guide_count == 0 || line.trim().is_empty() {
            continue;
        }

        let row_rect = galley_line_rect(output, index);
        let y_range = row_rect.top()..=row_rect.bottom();
        let color = if current_line == Some(index as u32) {
            active_guide_color
        } else {
            guide_color
        };

        for guide_index in 1..=guide_count {
            let column = guide_index * tab_width;
            let x = output.galley_pos.x + char_width * column as f32;
            if !output.text_clip_rect.x_range().contains(x) {
                continue;
            }

            painter.line_segment(
                [
                    egui::pos2(x, *y_range.start()),
                    egui::pos2(x, *y_range.end()),
                ],
                egui::Stroke::new(1.0, color),
            );
        }
    }
}

fn capture_editor_render_debug(
    output: &egui::text_edit::TextEditOutput,
    line_count: usize,
    text_line_count: usize,
    row_height: f32,
    scroll_y: f32,
    viewport_rect: egui::Rect,
    current_line: Option<u32>,
) -> EditorRenderDebug {
    let visible_rows = visible_galley_row_range(output, line_count, viewport_rect);
    let first_visible_row = (visible_rows.start < visible_rows.end).then_some(visible_rows.start);
    let first_visible_top = first_visible_row.map(|index| galley_line_rect(output, index).top());
    let last_visible_row = visible_rows.end.checked_sub(1);

    EditorRenderDebug {
        line_count,
        text_line_count,
        galley_rows: output.galley.rows.len(),
        scroll_y,
        row_height,
        clip_top: output.text_clip_rect.top(),
        clip_bottom: output.text_clip_rect.bottom(),
        viewport_top: viewport_rect.top(),
        viewport_bottom: viewport_rect.bottom(),
        galley_pos_y: output.galley_pos.y,
        first_visible_row,
        first_visible_top,
        last_visible_row,
        anchor_line: Some(visible_rows.start),
        anchor_top: Some(galley_line_rect(output, visible_rows.start).top()),
        anchored_range: Some(visible_rows),
        current_line,
    }
}

fn visible_galley_row_range(
    output: &egui::text_edit::TextEditOutput,
    line_count: usize,
    viewport_rect: egui::Rect,
) -> std::ops::Range<usize> {
    if line_count == 0 || output.galley.rows.is_empty() {
        return 0..0;
    }

    let row_height = output
        .galley
        .rows
        .first()
        .map_or(16.0, |row| row.rect().height())
        .max(1.0);
    let visible_top = viewport_rect.top() - output.galley_pos.y;
    let visible_bottom = viewport_rect.bottom() - output.galley_pos.y;
    let start = (visible_top / row_height).floor().max(0.0) as usize;
    let end = (visible_bottom / row_height).ceil().max(0.0) as usize + 1;

    start.min(line_count)..end.min(line_count)
}

fn galley_line_rect(output: &egui::text_edit::TextEditOutput, line_index: usize) -> egui::Rect {
    output
        .galley
        .rows
        .get(line_index)
        .map(|row| row.rect().translate(output.galley_pos.to_vec2()))
        .unwrap_or_else(|| {
            let row_height = output
                .galley
                .rows
                .last()
                .map_or(16.0, |row| row.rect().height());
            let top = output.galley_pos.y + line_index as f32 * row_height;
            egui::Rect::from_min_size(
                egui::pos2(output.text_clip_rect.left(), top),
                egui::vec2(output.text_clip_rect.width(), row_height),
            )
        })
}

fn paint_markdown_page_range_background(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    viewport_rect: egui::Rect,
    active_page_range: Option<(u32, u32)>,
) {
    let Some((start_line, end_line)) = active_page_range else {
        return;
    };

    let visible_lines = visible_galley_row_range(output, end_line as usize + 1, viewport_rect);
    let start = (start_line as usize).max(visible_lines.start);
    let end = (end_line as usize + 1).min(visible_lines.end);

    for line in start..end {
        let row_rect = galley_line_rect(output, line);
        let line_rect = egui::Rect::from_min_max(
            egui::pos2(output.text_clip_rect.left(), row_rect.top()),
            egui::pos2(output.text_clip_rect.right(), row_rect.bottom()),
        )
        .intersect(viewport_rect);

        if line_rect.is_positive() {
            ui.painter()
                .rect_filled(line_rect, 0.0, markdown_page_range_overlay(ui));
        }
    }
}

fn current_line_background(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(45, 45, 48)
    } else {
        Color32::from_rgb(232, 242, 255)
    }
}

fn current_line_overlay(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgba_unmultiplied(90, 140, 220, 48)
    } else {
        Color32::from_rgba_unmultiplied(80, 150, 255, 44)
    }
}

fn markdown_page_range_overlay(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgba_unmultiplied(120, 150, 90, 24)
    } else {
        Color32::from_rgba_unmultiplied(180, 210, 120, 28)
    }
}

fn indent_guide_color(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgba_unmultiplied(190, 190, 190, 42)
    } else {
        Color32::from_rgba_unmultiplied(80, 80, 80, 38)
    }
}

fn active_indent_guide_color(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgba_unmultiplied(220, 220, 220, 72)
    } else {
        Color32::from_rgba_unmultiplied(45, 45, 45, 70)
    }
}

fn markdown_search_match_background(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(87, 72, 24)
    } else {
        Color32::from_rgb(255, 238, 160)
    }
}

fn source_search_match_background(ui: &egui::Ui) -> Color32 {
    markdown_search_match_background(ui)
}

fn active_source_search_match_background(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(128, 92, 24)
    } else {
        Color32::from_rgb(255, 190, 80)
    }
}

fn markdown_focus_color(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(116, 190, 255)
    } else {
        Color32::from_rgb(0, 92, 160)
    }
}

fn markdown_focus_background(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(34, 54, 72)
    } else {
        Color32::from_rgb(214, 236, 252)
    }
}

fn matching_bracket_background(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(75, 75, 84)
    } else {
        Color32::from_rgb(210, 226, 255)
    }
}

fn editor_font_id() -> FontId {
    FontId::new(14.0, egui::FontFamily::Monospace)
}

fn bookmark_gutter_font_id(row_height: f32) -> FontId {
    FontId::new(
        (row_height * 1.25).clamp(18.0, 24.0),
        egui::FontFamily::Proportional,
    )
}

fn editor_row_height(ui: &egui::Ui) -> f32 {
    ui.fonts_mut(|fonts| fonts.row_height(&editor_font_id()))
}

fn editor_scroll_row_height(ui: &egui::Ui) -> f32 {
    editor_row_height(ui).ceil()
}

fn estimated_textedit_source_max_scroll_y(
    ui: &egui::Ui,
    line_count: usize,
    viewport_height: f32,
) -> f32 {
    let content_height =
        line_count.max(TEXTEDIT_SOURCE_MIN_ROWS) as f32 * editor_scroll_row_height(ui);
    (content_height - viewport_height).max(0.0)
}

fn scroll_area_vertical_max_scroll_y(content_size: egui::Vec2, inner_rect: egui::Rect) -> f32 {
    (content_size.y - inner_rect.height()).max(0.0)
}

fn editor_char_width(ui: &egui::Ui) -> f32 {
    ui.fonts_mut(|fonts| fonts.glyph_width(&editor_font_id(), ' '))
}

fn should_use_virtual_source_view(source_len: usize, line_count: usize) -> bool {
    source_len >= VIRTUAL_SOURCE_MIN_BYTES || line_count >= VIRTUAL_SOURCE_MIN_LINES
}

/// Removes from `smooth` the part already applied ahead of time (`lead`, per axis and only
/// in the same direction) and returns what is left to scroll.
fn consume_wheel_lead(lead: &mut egui::Vec2, smooth: egui::Vec2) -> egui::Vec2 {
    let mut remaining = smooth;
    for axis in 0..2 {
        if smooth[axis] == 0.0 {
            continue;
        }
        if lead[axis].signum() != smooth[axis].signum() {
            lead[axis] = 0.0;
            continue;
        }
        let skipped = if lead[axis] > 0.0 {
            smooth[axis].min(lead[axis])
        } else {
            smooth[axis].max(lead[axis])
        };
        lead[axis] -= skipped;
        remaining[axis] -= skipped;
    }
    remaining
}

fn immediate_wheel_scroll_delta(
    events: &[egui::Event],
    viewport_height: f32,
) -> Option<egui::Vec2> {
    let mut scroll_delta = egui::Vec2::ZERO;
    for event in events {
        let egui::Event::MouseWheel {
            unit,
            delta,
            phase,
            modifiers,
        } = event
        else {
            continue;
        };
        if matches!(phase, egui::TouchPhase::End | egui::TouchPhase::Cancel) || modifiers.command {
            continue;
        }

        scroll_delta += match unit {
            egui::MouseWheelUnit::Point => *delta,
            egui::MouseWheelUnit::Line => VIRTUAL_WHEEL_LINE_SCROLL_SPEED * *delta,
            egui::MouseWheelUnit::Page => viewport_height * *delta,
        };
    }

    (scroll_delta != egui::Vec2::ZERO).then_some(scroll_delta)
}

struct VirtualEditorClickInput {
    position: Option<egui::Pos2>,
    double_clicked: bool,
    event: Option<VirtualClickEventDebug>,
}

#[allow(clippy::too_many_arguments)]
fn virtual_editor_drag_selection(
    ui: &egui::Ui,
    response: &egui::Response,
    line_index: &LineIndex,
    text: &EditorTextBuffer,
    viewport_rect: egui::Rect,
    text_clip_rect: egui::Rect,
    total_width: f32,
    total_height: f32,
    text_left: f32,
    row_height: f32,
    scroll_y: f32,
    char_width: f32,
) -> Option<(Position, Position)> {
    if !(response.drag_started_by(egui::PointerButton::Primary)
        || response.dragged_by(egui::PointerButton::Primary))
    {
        return None;
    }

    let (vertical_scrollbar_rect, horizontal_scrollbar_rect) =
        virtual_scrollbar_hit_rects(viewport_rect, total_width, total_height);
    let pointer_pos = response
        .interact_pointer_pos()
        .or_else(|| ui.input(|input| input.pointer.interact_pos()))?;
    let press_origin = ui
        .input(|input| input.pointer.press_origin())
        .unwrap_or(pointer_pos);
    if !viewport_rect.contains(press_origin)
        || virtual_scrollbar_hit_contains(
            vertical_scrollbar_rect,
            horizontal_scrollbar_rect,
            press_origin,
        )
    {
        return None;
    }

    let anchor = virtual_pointer_to_text_position(
        Some(ui),
        press_origin,
        line_index,
        text,
        viewport_rect,
        text_clip_rect,
        text_left,
        row_height,
        scroll_y,
        char_width,
    )?;
    let head = virtual_pointer_to_text_position(
        Some(ui),
        pointer_pos,
        line_index,
        text,
        viewport_rect,
        text_clip_rect,
        text_left,
        row_height,
        scroll_y,
        char_width,
    )?;
    Some((anchor, head))
}

/// Some editors park keyboard focus on ids that are not real widgets (the virtual source
/// editor's `text_edit_id`, the Markdown pane focus ids). AccessKit rejects a focused id
/// without a node, and tests always run with AccessKit on because egui_kittest reads the UI
/// through it, so give such an id an empty node in test builds.
/// See DISCREPANCY(D07): with an accessibility client attached the app hits the same issue.
fn register_focused_test_node(context: &egui::Context) {
    #[cfg(test)]
    if let Some(focused) = context.memory(|memory| memory.focused()) {
        context.accesskit_node_builder(focused, |_| {});
    }
    #[cfg(not(test))]
    let _ = context;
}

fn virtual_editor_can_handle_key(ui: &egui::Ui, text_edit_id: egui::Id) -> bool {
    ui.memory(|memory| memory.has_focus(text_edit_id))
}

fn consume_virtual_navigation_command(ui: &mut egui::Ui) -> Option<VirtualNavigationCommand> {
    ui.input_mut(|input| {
        if input.modifiers.ctrl || input.modifiers.command {
            if input.consume_key(egui::Modifiers::CTRL, egui::Key::A)
                || input.consume_key(egui::Modifiers::COMMAND, egui::Key::A)
            {
                return Some(VirtualNavigationCommand {
                    target: VirtualNavigationTarget::SelectAll,
                    extend_selection: false,
                });
            }
            return None;
        }

        let extend_selection = input.modifiers.shift;
        let modifiers = if extend_selection {
            egui::Modifiers::SHIFT
        } else {
            egui::Modifiers::NONE
        };

        let target = if input.consume_key(modifiers, egui::Key::ArrowLeft) {
            Some(VirtualNavigationTarget::Left)
        } else if input.consume_key(modifiers, egui::Key::ArrowRight) {
            Some(VirtualNavigationTarget::Right)
        } else if input.consume_key(modifiers, egui::Key::ArrowUp) {
            Some(VirtualNavigationTarget::Up)
        } else if input.consume_key(modifiers, egui::Key::ArrowDown) {
            Some(VirtualNavigationTarget::Down)
        } else if input.consume_key(modifiers, egui::Key::Home) {
            Some(VirtualNavigationTarget::Home)
        } else if input.consume_key(modifiers, egui::Key::End) {
            Some(VirtualNavigationTarget::End)
        } else if input.consume_key(modifiers, egui::Key::PageUp) {
            Some(VirtualNavigationTarget::PageUp)
        } else if input.consume_key(modifiers, egui::Key::PageDown) {
            Some(VirtualNavigationTarget::PageDown)
        } else {
            None
        }?;

        Some(VirtualNavigationCommand {
            target,
            extend_selection,
        })
    })
}

fn virtual_page_row_delta(row_height: f32, viewport_height: f32) -> i32 {
    ((viewport_height * EDITOR_PAGE_SCROLL_RATIO) / row_height.max(1.0))
        .round()
        .max(1.0) as i32
}

fn clamp_virtual_position(line_index: &LineIndex, position: Position) -> Position {
    let line_count = line_index.line_count().max(1);
    let line_number = (position.line as usize).min(line_count.saturating_sub(1));
    let character = line_index
        .line(line_number)
        .map(|line| (position.character as usize).min(line.content_end - line.start))
        .unwrap_or(0);
    Position::new(line_number as u32, character as u32)
}

fn virtual_document_end(line_index: &LineIndex) -> Position {
    let line_number = line_index.line_count().saturating_sub(1);
    virtual_position_line_end(line_index, Position::new(line_number as u32, 0))
}

fn virtual_position_line_home(line_index: &LineIndex, position: Position) -> Position {
    let line_number = (position.line as usize).min(line_index.line_count().saturating_sub(1));
    Position::new(line_number as u32, 0)
}

fn virtual_position_line_end(line_index: &LineIndex, position: Position) -> Position {
    let line_number = (position.line as usize).min(line_index.line_count().saturating_sub(1));
    let character = line_index
        .line(line_number)
        .map(|line| line.content_end - line.start)
        .unwrap_or(0);
    Position::new(line_number as u32, character as u32)
}

fn virtual_position_vertical(
    line_index: &LineIndex,
    position: Position,
    row_delta: i32,
) -> Position {
    let line_count = line_index.line_count().max(1);
    let next_line = (position.line as i32 + row_delta).clamp(0, line_count as i32 - 1) as usize;
    let character = line_index
        .line(next_line)
        .map(|line| (position.character as usize).min(line.content_end - line.start))
        .unwrap_or(0);
    Position::new(next_line as u32, character as u32)
}

fn virtual_position_left(text: &EditorTextBuffer, position: Position) -> Position {
    let Some(byte_index) = virtual_position_to_byte_index(text, position) else {
        return Position::new(0, 0);
    };
    if byte_index == 0 {
        return Position::new(0, 0);
    }
    text.byte_index_to_position(previous_char_boundary(text.as_str(), byte_index))
        .unwrap_or(Position::new(0, 0))
}

fn virtual_position_right(text: &EditorTextBuffer, position: Position) -> Position {
    let Some(byte_index) = virtual_position_to_byte_index(text, position) else {
        return Position::new(0, 0);
    };
    if byte_index >= text.len() {
        return text
            .byte_index_to_position(text.len())
            .unwrap_or(Position::new(0, 0));
    }
    text.byte_index_to_position(next_char_boundary(text.as_str(), byte_index))
        .unwrap_or(Position::new(0, 0))
}

fn visual_column_for_position(
    ui: Option<&egui::Ui>,
    text: &EditorTextBuffer,
    line_index: &LineIndex,
    position: Position,
    fallback_char_width: f32,
) -> f32 {
    let position = clamp_virtual_position(line_index, position);
    let Some(line) = line_index.line(position.line as usize) else {
        return 0.0;
    };
    visual_x_for_byte_column(
        ui,
        text.line_text_or_empty(line),
        position.character as usize,
        fallback_char_width,
    )
}

#[allow(clippy::too_many_arguments)]
fn virtual_editor_click_input(
    ui: &egui::Ui,
    response: &egui::Response,
    text_edit_id: egui::Id,
    pane_focused: bool,
    viewport_rect: egui::Rect,
    text_clip_rect: egui::Rect,
    total_width: f32,
    total_height: f32,
    scroll_x: f32,
    scroll_y: f32,
    row_height: f32,
    line_count: usize,
    visible_range: std::ops::Range<usize>,
) -> VirtualEditorClickInput {
    let (vertical_scrollbar_rect, horizontal_scrollbar_rect) =
        virtual_scrollbar_hit_rects(viewport_rect, total_width, total_height);
    let response_pointer_pos = response.interact_pointer_pos();
    let response_clicked = response.clicked();
    let response_primary_clicked = response.clicked_by(egui::PointerButton::Primary);
    let response_double_clicked = response.double_clicked_by(egui::PointerButton::Primary);
    let response_hovered = response.hovered();
    let response_contains_pointer = response.contains_pointer();
    let text_edit_focused = ui.memory(|memory| memory.has_focus(text_edit_id));

    let (
        input_pointer_pos,
        primary_pressed,
        primary_released,
        primary_clicked,
        any_pressed,
        any_released,
        any_click,
        dragging,
    ) = ui.input(|input| {
        (
            input.pointer.interact_pos(),
            input.pointer.primary_pressed(),
            input.pointer.primary_released(),
            input.pointer.primary_clicked(),
            input.pointer.any_pressed(),
            input.pointer.any_released(),
            input.pointer.any_click(),
            input.pointer.is_decidedly_dragging(),
        )
    });

    let input_activity = response_clicked
        || response_primary_clicked
        || response_double_clicked
        || primary_pressed
        || primary_released
        || primary_clicked
        || any_pressed
        || any_released
        || any_click;

    let (source, candidate_pos) = if response_double_clicked {
        ("response_double_clicked", response_pointer_pos)
    } else if response_primary_clicked {
        ("response_primary_clicked", response_pointer_pos)
    } else if primary_released {
        ("primary_released", input_pointer_pos)
    } else if primary_pressed {
        ("primary_pressed", input_pointer_pos)
    } else {
        ("none", response_pointer_pos.or(input_pointer_pos))
    };

    let pointer_pos = candidate_pos.or(response_pointer_pos).or(input_pointer_pos);
    let response_hit = pointer_pos.is_some_and(|position| response.rect.contains(position));
    let viewport_hit = pointer_pos.is_some_and(|position| viewport_rect.contains(position));
    let text_hit = pointer_pos.is_some_and(|position| text_clip_rect.contains(position));
    let scrollbar_hit = pointer_pos.is_some_and(|position| {
        virtual_scrollbar_hit_contains(vertical_scrollbar_rect, horizontal_scrollbar_rect, position)
    });

    let reason = if source == "none" {
        "no_primary_press_or_release"
    } else if candidate_pos.is_none() {
        "no_pointer_position"
    } else if source != "response_primary_clicked" && dragging {
        "dragging"
    } else if !viewport_hit {
        "outside_viewport"
    } else if scrollbar_hit {
        "scrollbar"
    } else {
        "accepted"
    };
    let accepted = reason == "accepted";

    let event = input_activity.then_some(VirtualClickEventDebug {
        index: 0,
        source,
        accepted,
        reason,
        pointer_pos,
        response_rect: response.rect,
        viewport_rect,
        text_clip_rect,
        vertical_scrollbar_rect,
        horizontal_scrollbar_rect,
        response_hovered,
        response_contains_pointer,
        response_clicked,
        response_primary_clicked,
        primary_pressed,
        primary_released,
        primary_clicked,
        any_pressed,
        any_released,
        any_click,
        dragging,
        response_hit,
        viewport_hit,
        text_hit,
        scrollbar_hit,
        pane_focused,
        text_edit_focused,
        scroll_x,
        scroll_y,
        row_height,
        line_count,
        visible_range,
    });

    VirtualEditorClickInput {
        position: accepted.then_some(candidate_pos).flatten(),
        double_clicked: accepted && response_double_clicked,
        event,
    }
}

#[allow(clippy::too_many_arguments)]
fn virtual_pointer_to_text_position(
    ui: Option<&egui::Ui>,
    pointer_pos: egui::Pos2,
    line_index: &LineIndex,
    text: &EditorTextBuffer,
    viewport_rect: egui::Rect,
    text_clip_rect: egui::Rect,
    text_left: f32,
    row_height: f32,
    scroll_y: f32,
    char_width: f32,
) -> Option<Position> {
    let line_count = line_index.line_count();
    if line_count == 0 {
        return None;
    }

    let clamped_y = pointer_pos.y.clamp(
        viewport_rect.top(),
        (viewport_rect.bottom() - 1.0).max(viewport_rect.top()),
    );
    let line = ((clamped_y - viewport_rect.top() + scroll_y) / row_height)
        .floor()
        .max(0.0) as usize;
    let line = line.min(line_count.saturating_sub(1));
    let line_info = line_index.line(line)?;
    let line_text = text.line_text_or_empty(line_info);
    let character = virtual_click_byte_column(
        ui,
        line_text,
        pointer_pos.x,
        text_left,
        text_clip_rect.left(),
        char_width,
    );
    Some(Position::new(line as u32, character))
}

fn position_leq(left: Position, right: Position) -> bool {
    (left.line, left.character) <= (right.line, right.character)
}

fn byte_column_for_visual_x(
    ui: Option<&egui::Ui>,
    line: &str,
    x_offset: f32,
    fallback_char_width: f32,
) -> u32 {
    let target_x = x_offset.max(0.0);
    let mut current_x = 0.0;
    for (byte_index, character) in line.char_indices() {
        let character_width = visual_width_for_char(ui, character, fallback_char_width);
        if target_x < current_x + character_width * 0.5 {
            return byte_index as u32;
        }
        current_x += character_width;
    }
    line.len() as u32
}

fn visual_x_for_byte_column(
    ui: Option<&egui::Ui>,
    line: &str,
    byte_column: usize,
    fallback_char_width: f32,
) -> f32 {
    let byte_column = clamp_to_char_boundary(line, byte_column.min(line.len()));
    line[..byte_column]
        .chars()
        .map(|character| visual_width_for_char(ui, character, fallback_char_width))
        .sum()
}

fn visual_column_for_byte_column(line: &str, byte_column: usize) -> usize {
    let byte_column = clamp_to_char_boundary(line, byte_column.min(line.len()));
    line[..byte_column].chars().count()
}

fn visual_width_for_char(ui: Option<&egui::Ui>, character: char, fallback_char_width: f32) -> f32 {
    if character == '\t' {
        return fallback_char_width * DEFAULT_TAB_WIDTH as f32;
    }
    ui.map(|ui| ui.fonts_mut(|fonts| fonts.glyph_width(&editor_font_id(), character)))
        .filter(|width| *width > 0.0)
        .unwrap_or(fallback_char_width)
}

fn virtual_click_x_offset(pointer_x: f32, text_left: f32, text_clip_left: f32) -> f32 {
    if pointer_x < text_clip_left {
        0.0
    } else {
        pointer_x - text_left
    }
}

fn virtual_click_byte_column(
    ui: Option<&egui::Ui>,
    line: &str,
    pointer_x: f32,
    text_left: f32,
    text_clip_left: f32,
    char_width: f32,
) -> u32 {
    let x_offset = virtual_click_x_offset(pointer_x, text_left, text_clip_left);
    byte_column_for_visual_x(ui, line, x_offset, char_width)
}

fn line_contains_bracket_pair(line: LineInfo, bracket_pair: Option<(usize, usize)>) -> bool {
    let Some((left, right)) = bracket_pair else {
        return false;
    };
    [left, right]
        .into_iter()
        .any(|index| (line.start..line.content_end).contains(&index))
}

fn prune_virtual_line_layout_cache(cache: &mut VirtualLineLayoutCache, frame: u64) {
    if cache.entries.len() <= VIRTUAL_LINE_LAYOUT_CACHE_LIMIT {
        return;
    }

    let cutoff = frame.saturating_sub(VIRTUAL_LINE_LAYOUT_CACHE_KEEP_FRAMES);
    cache
        .entries
        .retain(|_, entry| entry.last_used_frame >= cutoff);
    if cache.entries.len() <= VIRTUAL_LINE_LAYOUT_CACHE_LIMIT {
        return;
    }

    let mut entries_by_age = cache
        .entries
        .iter()
        .map(|(line_number, entry)| (*line_number, entry.last_used_frame))
        .collect::<Vec<_>>();
    entries_by_age.sort_by_key(|(_, last_used_frame)| *last_used_frame);
    let remove_count = cache
        .entries
        .len()
        .saturating_sub(VIRTUAL_LINE_LAYOUT_CACHE_LIMIT);
    for (line_number, _) in entries_by_age.into_iter().take(remove_count) {
        cache.entries.remove(&line_number);
    }
}

fn prune_virtual_line_number_layout_cache(cache: &mut VirtualLineNumberLayoutCache, frame: u64) {
    if cache.entries.len() <= VIRTUAL_LINE_LAYOUT_CACHE_LIMIT {
        return;
    }

    let cutoff = frame.saturating_sub(VIRTUAL_LINE_LAYOUT_CACHE_KEEP_FRAMES);
    cache
        .entries
        .retain(|_, entry| entry.last_used_frame >= cutoff);
    if cache.entries.len() <= VIRTUAL_LINE_LAYOUT_CACHE_LIMIT {
        return;
    }

    let mut entries_by_age = cache
        .entries
        .iter()
        .map(|(key, entry)| (*key, entry.last_used_frame))
        .collect::<Vec<_>>();
    entries_by_age.sort_by_key(|(_, last_used_frame)| *last_used_frame);
    let remove_count = cache
        .entries
        .len()
        .saturating_sub(VIRTUAL_LINE_LAYOUT_CACHE_LIMIT);
    for (key, _) in entries_by_age.into_iter().take(remove_count) {
        cache.entries.remove(&key);
    }
}

fn editor_page_scroll_delta(
    ui: &egui::Ui,
    focused: bool,
    text_edit_id: egui::Id,
    viewport_height: f32,
) -> Option<f32> {
    if !focused {
        return None;
    }

    let editor_can_handle_key =
        ui.memory(|memory| memory.focused().is_none() || memory.has_focus(text_edit_id));
    if !editor_can_handle_key {
        return None;
    }

    let amount = (viewport_height * EDITOR_PAGE_SCROLL_RATIO).max(editor_row_height(ui));
    ui.input_mut(|input| {
        if input.consume_key(egui::Modifiers::NONE, egui::Key::PageDown) {
            Some(amount)
        } else if input.consume_key(egui::Modifiers::NONE, egui::Key::PageUp) {
            Some(-amount)
        } else {
            None
        }
    })
}

fn diff_page_scroll_delta(ui: &egui::Ui, focused: bool, viewport_height: f32) -> Option<f32> {
    if !focused {
        return None;
    }

    let amount = (viewport_height * EDITOR_PAGE_SCROLL_RATIO).max(editor_row_height(ui));
    ui.input_mut(|input| {
        if input.consume_key(egui::Modifiers::NONE, egui::Key::PageDown) {
            Some(amount)
        } else if input.consume_key(egui::Modifiers::NONE, egui::Key::PageUp) {
            Some(-amount)
        } else {
            None
        }
    })
}

fn leading_indent_columns(line: &str, tab_width: usize) -> usize {
    let tab_width = tab_width.max(1);
    let mut columns = 0;

    for character in line.chars() {
        match character {
            ' ' => columns += 1,
            '\t' => columns += tab_width - columns % tab_width,
            _ => break,
        }
    }

    columns
}

fn clamp_to_char_boundary(source: &str, mut index: usize) -> usize {
    index = index.min(source.len());
    while index > 0 && !source.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn char_index_to_byte_index(source: &str, char_index: usize) -> Option<usize> {
    if char_index == source.chars().count() {
        return Some(source.len());
    }

    source
        .char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
}

fn char_index_to_position(source: &str, char_index: usize) -> Option<Position> {
    let byte_index = source
        .char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(source.len());
    let before_cursor = &source[..byte_index];
    let line = before_cursor.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let line_start = before_cursor
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let character = before_cursor[line_start..].len() as u32;

    Some(Position::new(line, character))
}

fn byte_index_to_position(source: &str, byte_index: usize) -> Option<Position> {
    let byte_index = clamp_to_char_boundary(source, byte_index);
    let before_cursor = source.get(..byte_index)?;
    let line = before_cursor.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let line_start = before_cursor
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let character = before_cursor[line_start..].len() as u32;

    Some(Position::new(line, character))
}

fn position_to_char_index(source: &str, position: Position) -> Option<usize> {
    let mut line = 0;
    let mut line_start_byte = 0;

    for (index, byte) in source.bytes().enumerate() {
        if line == position.line {
            break;
        }

        if byte == b'\n' {
            line += 1;
            line_start_byte = index + 1;
        }
    }

    if line != position.line {
        return None;
    }

    let target_byte = (line_start_byte + position.character as usize).min(source.len());
    let target_byte = clamp_to_char_boundary(source, target_byte);

    Some(source[..target_byte].chars().count())
}

fn next_char_at(source: &str, char_index: usize) -> Option<char> {
    let byte_index = char_index_to_byte_index(source, char_index)?;
    source[byte_index..].chars().next()
}

fn closing_pair(character: char) -> Option<char> {
    match character {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' => Some('"'),
        _ => None,
    }
}

fn is_closing_bracket(character: char) -> bool {
    matches!(character, ')' | ']' | '}' | '"')
}

fn smart_newline(source: &str, cursor_index: usize, tab_width: usize) -> String {
    let byte_index = char_index_to_byte_index(source, cursor_index).unwrap_or(source.len());
    let before_cursor = &source[..byte_index];
    let line_start = before_cursor
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let current_line = &before_cursor[line_start..];
    let mut indent: String = current_line
        .chars()
        .take_while(|character| matches!(character, ' ' | '\t'))
        .collect();

    if before_cursor
        .trim_end_matches(|character: char| character.is_whitespace())
        .ends_with(['{', '(', '['])
    {
        indent.push_str(&" ".repeat(tab_width));
    }

    format!("\n{indent}")
}

fn strip_trailing_whitespace(source: &str) -> String {
    let mut stripped = String::with_capacity(source.len());

    for segment in source.split_inclusive('\n') {
        if let Some(line) = segment.strip_suffix("\r\n") {
            stripped.push_str(line.trim_end_matches([' ', '\t']));
            stripped.push_str("\r\n");
        } else if let Some(line) = segment.strip_suffix('\n') {
            stripped.push_str(line.trim_end_matches([' ', '\t']));
            stripped.push('\n');
        } else {
            stripped.push_str(segment.trim_end_matches([' ', '\t']));
        }
    }

    stripped
}

fn find_matching_bracket_pair(source: &str, position: Position) -> Option<(usize, usize)> {
    let cursor_index = position_to_char_index(source, position)?;
    let characters: Vec<char> = source.chars().collect();

    let candidates = [
        cursor_index.checked_sub(1),
        (cursor_index < characters.len()).then_some(cursor_index),
    ];

    for candidate in candidates.into_iter().flatten() {
        let character = characters[candidate];
        if let Some(close) = closing_pair(character).filter(|close| *close != character)
            && let Some(match_index) =
                find_matching_forward(&characters, candidate, character, close)
        {
            return Some((
                char_index_to_byte_index(source, candidate)?,
                char_index_to_byte_index(source, match_index)?,
            ));
        }

        if let Some(open) = opening_pair(character)
            && let Some(match_index) =
                find_matching_backward(&characters, candidate, open, character)
        {
            return Some((
                char_index_to_byte_index(source, match_index)?,
                char_index_to_byte_index(source, candidate)?,
            ));
        }
    }

    None
}

fn opening_pair(character: char) -> Option<char> {
    match character {
        ')' => Some('('),
        ']' => Some('['),
        '}' => Some('{'),
        _ => None,
    }
}

fn find_matching_forward(
    characters: &[char],
    start: usize,
    open: char,
    close: char,
) -> Option<usize> {
    let mut depth = 0;

    for (index, character) in characters.iter().enumerate().skip(start) {
        if *character == open {
            depth += 1;
        } else if *character == close {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }

    None
}

fn find_matching_backward(
    characters: &[char],
    start: usize,
    open: char,
    close: char,
) -> Option<usize> {
    let mut depth = 0;

    for (index, character) in characters.iter().enumerate().take(start + 1).rev() {
        if *character == close {
            depth += 1;
        } else if *character == open {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }

    None
}

fn is_csv_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("csv"))
}

fn is_go_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("go"))
}

fn is_rust_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("rs"))
}

fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md")
                || extension.eq_ignore_ascii_case("mdx")
                || extension.eq_ignore_ascii_case("markdown")
        })
}

fn parse_csv(source: &str) -> Result<CsvDocument, String> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = source.chars().peekable();
    let mut in_quotes = false;
    let mut field_started_with_quote = false;
    let mut just_finished_row = false;

    while let Some(character) = chars.next() {
        if in_quotes {
            match character {
                '"' if chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => in_quotes = false,
                _ => field.push(character),
            }
            just_finished_row = false;
            continue;
        }

        match character {
            '"' if field.is_empty() && !field_started_with_quote => {
                in_quotes = true;
                field_started_with_quote = true;
                just_finished_row = false;
            }
            ',' => {
                row.push(std::mem::take(&mut field));
                field_started_with_quote = false;
                just_finished_row = false;
            }
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                field_started_with_quote = false;
                just_finished_row = true;
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                field_started_with_quote = false;
                just_finished_row = true;
            }
            _ => {
                field.push(character);
                just_finished_row = false;
            }
        }
    }

    if in_quotes {
        return Err("unterminated quoted field".to_owned());
    }

    if !just_finished_row || !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }

    Ok(CsvDocument { rows })
}

/// Row separator and final newline of a CSV file, kept when the grid rewrites it so a
/// one-cell edit stays a one-line diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CsvTextStyle {
    line_ending: &'static str,
    trailing_newline: bool,
}

impl CsvTextStyle {
    fn of(text: &str) -> Self {
        Self {
            line_ending: if text.contains("\r\n") { "\r\n" } else { "\n" },
            trailing_newline: text.ends_with('\n'),
        }
    }

    fn join(self, rows: &[Vec<String>]) -> String {
        let mut text = rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|field| serialize_csv_field(field))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect::<Vec<_>>()
            .join(self.line_ending);
        if self.trailing_newline && !text.is_empty() {
            text.push_str(self.line_ending);
        }
        text
    }
}

fn serialize_csv(document: &CsvDocument, style: CsvTextStyle) -> String {
    style.join(&normalized_csv_rows(document, false))
}

fn serialize_csv_for_save(document: &CsvDocument, style: CsvTextStyle) -> String {
    style.join(&normalized_csv_rows(document, true))
}

fn copy_csv_selection_as_csv(document: &CsvDocument, selection: CsvSelection) -> String {
    let rows = normalized_csv_rows(document, false);
    if rows.is_empty() {
        return String::new();
    }

    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    if column_count == 0 {
        return String::new();
    }

    let (row_start, row_end) = selection
        .row_range()
        .unwrap_or((0, rows.len().saturating_sub(1)));
    let (column_start, column_end) = selection
        .column_range()
        .unwrap_or((0, column_count.saturating_sub(1)));
    let row_end = row_end.min(rows.len().saturating_sub(1));
    let column_end = column_end.min(column_count.saturating_sub(1));

    (row_start..=row_end)
        .map(|row| {
            (column_start..=column_end)
                .map(|column| {
                    rows.get(row)
                        .and_then(|fields| fields.get(column))
                        .map(|field| serialize_csv_field(field))
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn paste_csv_selection_from_text(
    document: &mut CsvDocument,
    selection: CsvSelection,
    text: &str,
) -> Result<usize, String> {
    let pasted = parse_csv(text)?;
    let (target_row, target_column) = match selection {
        CsvSelection::Rows { start, end } => (ordered_range(start, end).0, 0),
        CsvSelection::Columns { start, end } => (0, ordered_range(start, end).0),
    };
    let mut cell_count = 0;

    for (row_offset, row) in pasted.rows.iter().enumerate() {
        for (column_offset, field) in row.iter().enumerate() {
            let row = target_row + row_offset;
            let column = target_column + column_offset;
            *document.cell_mut(row, column) = field.clone();
            cell_count += 1;
        }
    }

    Ok(cell_count)
}

fn delete_csv_selection(
    document: &mut CsvDocument,
    selection: CsvSelection,
) -> (CsvSelection, String) {
    match selection {
        CsvSelection::Rows { start, end } => {
            let (start, end) = ordered_range(start, end);
            let row_count = document.rows.len().max(1);
            let start = start.min(row_count.saturating_sub(1));
            let end = end.min(row_count.saturating_sub(1));

            if !document.rows.is_empty() {
                document.rows.drain(start..=end);
            }
            if document.rows.is_empty() {
                document.rows.push(vec![String::new()]);
            }

            let next = start.min(document.rows.len().saturating_sub(1));
            let label = if start == end {
                format!("row {}", start + 1)
            } else {
                format!("rows {}-{}", start + 1, end + 1)
            };

            (
                CsvSelection::Rows {
                    start: next,
                    end: next,
                },
                label,
            )
        }
        CsvSelection::Columns { start, end } => {
            let (start, end) = ordered_range(start, end);
            let column_count = document.column_count().max(1);
            let start = start.min(column_count.saturating_sub(1));
            let end = end.min(column_count.saturating_sub(1));

            if document.rows.is_empty() {
                document.rows.push(vec![String::new()]);
            }
            for row in &mut document.rows {
                row.resize(column_count, String::new());
                row.drain(start..=end);
                if row.is_empty() {
                    row.push(String::new());
                }
            }

            let next_column_count = document.column_count().max(1);
            let next = start.min(next_column_count.saturating_sub(1));
            let label = if start == end {
                format!("column {}", csv_column_label(start))
            } else {
                format!(
                    "columns {}-{}",
                    csv_column_label(start),
                    csv_column_label(end)
                )
            };

            (
                CsvSelection::Columns {
                    start: next,
                    end: next,
                },
                label,
            )
        }
    }
}

fn normalized_csv_rows(
    document: &CsvDocument,
    trim_extra_trailing_empty_rows: bool,
) -> Vec<Vec<String>> {
    let mut rows = document.rows.clone();
    let column_count = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);

    for row in &mut rows {
        row.resize(column_count, String::new());
    }

    if trim_extra_trailing_empty_rows {
        let mut trailing_empty_rows = 0;
        for row in rows.iter().rev() {
            if row.iter().all(String::is_empty) {
                trailing_empty_rows += 1;
            } else {
                break;
            }
        }

        if trailing_empty_rows > 0 {
            rows.truncate(rows.len().saturating_sub(trailing_empty_rows));
        }
    }

    rows
}

fn serialize_csv_field(field: &str) -> String {
    let needs_quotes = field.contains([',', '"', '\n', '\r'])
        || field.starts_with([' ', '\t'])
        || field.ends_with([' ', '\t']);

    if !needs_quotes {
        return field.to_owned();
    }

    format!("\"{}\"", field.replace('"', "\"\""))
}

fn csv_column_widths(ui: &egui::Ui, document: &CsvDocument, column_count: usize) -> Vec<f32> {
    let row_height = editor_row_height(ui);
    let char_width = row_height * 0.58;

    (0..column_count)
        .map(|column| {
            let max_chars = document
                .rows
                .iter()
                .filter_map(|row| row.get(column))
                .flat_map(|cell| cell.lines())
                .map(|line| line.chars().count())
                .max()
                .unwrap_or(0)
                .max(csv_column_label(column).chars().count());

            (max_chars as f32 * char_width + 28.0).clamp(72.0, 360.0)
        })
        .collect()
}

fn ordered_range(start: usize, end: usize) -> (usize, usize) {
    if start <= end {
        (start, end)
    } else {
        (end, start)
    }
}

fn consume_copy_shortcut(input: &mut egui::InputState) -> bool {
    let mut copied = false;
    input.events.retain(|event| {
        let is_copy_event = matches!(
            event,
            egui::Event::Copy
                | egui::Event::Key {
                    key: egui::Key::Copy,
                    pressed: true,
                    ..
                }
        );
        copied |= is_copy_event;
        !is_copy_event
    });

    copied
        || input.consume_key(egui::Modifiers::COMMAND, egui::Key::C)
        || input.consume_key(egui::Modifiers::CTRL, egui::Key::C)
}

fn consume_paste_text(input: &mut egui::InputState) -> Option<String> {
    let mut pasted = None;
    input.events.retain(|event| {
        if let egui::Event::Paste(text) = event {
            pasted = Some(text.clone());
            false
        } else {
            true
        }
    });
    pasted
}

fn consume_paste_shortcut(input: &mut egui::InputState) -> bool {
    let mut pasted = false;
    input.events.retain(|event| {
        let is_paste_key = matches!(
            event,
            egui::Event::Key {
                key: egui::Key::Paste,
                pressed: true,
                ..
            }
        );
        pasted |= is_paste_key;
        !is_paste_key
    });

    pasted
        || input.consume_key(egui::Modifiers::COMMAND, egui::Key::V)
        || input.consume_key(egui::Modifiers::CTRL, egui::Key::V)
}

fn consume_undo_shortcut(input: &mut egui::InputState) -> bool {
    let mut undo = false;
    input.events.retain(|event| {
        let is_undo_key = matches!(
            event,
            egui::Event::Key {
                key: egui::Key::Z,
                pressed: true,
                modifiers,
                ..
            } if (modifiers.ctrl || modifiers.command) && !modifiers.shift
        );
        undo |= is_undo_key;
        !is_undo_key
    });

    // No consume_key fallback: it ignores extra Shift and would swallow Ctrl+Shift+Z (redo).
    undo
}

fn consume_redo_shortcut(input: &mut egui::InputState) -> bool {
    let mut redo = false;
    input.events.retain(|event| {
        let is_redo_key = matches!(
            event,
            egui::Event::Key {
                key: egui::Key::Y,
                pressed: true,
                modifiers,
                ..
            } if modifiers.ctrl || modifiers.command
        ) || matches!(
            event,
            egui::Event::Key {
                key: egui::Key::Z,
                pressed: true,
                modifiers,
                ..
            } if (modifiers.ctrl || modifiers.command) && modifiers.shift
        );
        redo |= is_redo_key;
        !is_redo_key
    });

    redo
}

fn consume_delete_shortcut(input: &mut egui::InputState) -> bool {
    input.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
        || input.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)
}

fn consume_navigation_back_shortcut(input: &mut egui::InputState) -> bool {
    input.consume_key(egui::Modifiers::ALT, egui::Key::ArrowLeft)
        || input.consume_key(egui::Modifiers::NONE, egui::Key::BrowserBack)
}

fn consume_navigation_forward_shortcut(input: &mut egui::InputState) -> bool {
    input.consume_key(egui::Modifiers::ALT, egui::Key::ArrowRight)
}

fn limit_virtual_edit_history(stack: &mut Vec<VirtualEditHistoryEntry>) {
    let overflow = stack.len().saturating_sub(VIRTUAL_EDIT_HISTORY_LIMIT);
    if overflow > 0 {
        stack.drain(0..overflow);
    }
}

fn limit_csv_edit_history(stack: &mut Vec<CsvEditHistoryEntry>) {
    let overflow = stack.len().saturating_sub(CSV_EDIT_HISTORY_LIMIT);
    if overflow > 0 {
        stack.drain(0..overflow);
    }
}

fn consume_ai_command_shortcut(input: &mut egui::InputState) -> bool {
    input.consume_key(
        egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        },
        egui::Key::A,
    )
}

fn raw_enter_pressed(input: &egui::InputState) -> bool {
    input.events.iter().any(|event| {
        matches!(
            event,
            egui::Event::Key {
                key: egui::Key::Enter,
                pressed: true,
                ..
            }
        )
    })
}

fn raw_search_enter_direction(input: &egui::InputState) -> Option<isize> {
    input.events.iter().find_map(|event| match event {
        egui::Event::Key {
            key: egui::Key::Enter,
            pressed: true,
            modifiers,
            ..
        } => Some(if modifiers.shift { -1 } else { 1 }),
        _ => None,
    })
}

fn raw_escape_pressed(input: &egui::InputState) -> bool {
    input.events.iter().any(|event| {
        matches!(
            event,
            egui::Event::Key {
                key: egui::Key::Escape,
                pressed: true,
                ..
            }
        )
    })
}

fn copy_text_to_system_clipboard(text: &str) -> Result<(), String> {
    platform::current().copy_text(text)
}

fn read_text_from_system_clipboard() -> Result<String, String> {
    platform::current().read_text()
}

fn csv_selection_label(selection: CsvSelection) -> String {
    match selection {
        CsvSelection::Rows { start, end } => {
            let (start, end) = ordered_range(start, end);
            if start == end {
                format!("Selected row {}", start + 1)
            } else {
                format!("Selected rows {}-{}", start + 1, end + 1)
            }
        }
        CsvSelection::Columns { start, end } => {
            let (start, end) = ordered_range(start, end);
            if start == end {
                format!("Selected column {}", csv_column_label(start))
            } else {
                format!(
                    "Selected columns {}-{}",
                    csv_column_label(start),
                    csv_column_label(end)
                )
            }
        }
    }
}

fn paint_csv_header(
    ui: &mut egui::Ui,
    row_number_width: f32,
    header_height: f32,
    column_widths: &[f32],
    selection: Option<CsvSelection>,
    drag: Option<CsvSelectionDrag>,
) -> Option<CsvHeaderAction> {
    let mut action = None;

    ui.horizontal(|ui| {
        let (corner_rect, _) = ui.allocate_exact_size(
            egui::vec2(row_number_width, header_height),
            egui::Sense::hover(),
        );
        paint_csv_box(
            ui,
            corner_rect,
            "",
            csv_header_background(ui),
            ui.visuals().weak_text_color(),
            egui::Align2::CENTER_CENTER,
            false,
        );

        for (column, width) in column_widths.iter().enumerate() {
            let selected = selection.is_some_and(|selection| selection.contains(0, column));
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(*width, header_height),
                egui::Sense::click_and_drag(),
            );
            paint_csv_box(
                ui,
                rect,
                &csv_column_label(column),
                if selected {
                    current_line_background(ui)
                } else {
                    csv_header_background(ui)
                },
                if selected {
                    ui.visuals().text_color()
                } else {
                    ui.visuals().weak_text_color()
                },
                egui::Align2::CENTER_CENTER,
                true,
            );

            if let Some(next_action) =
                csv_pointer_action_for_rect(ui, rect, column, CsvSelectionKind::Columns, drag)
            {
                action = Some(next_action);
            }
        }
    });

    action
}

fn csv_pointer_action_for_rect(
    ui: &egui::Ui,
    rect: egui::Rect,
    index: usize,
    kind: CsvSelectionKind,
    drag: Option<CsvSelectionDrag>,
) -> Option<CsvHeaderAction> {
    ui.input(|input| {
        if input.pointer.button_pressed(egui::PointerButton::Primary)
            && input
                .pointer
                .press_origin()
                .or_else(|| input.pointer.latest_pos())
                .is_some_and(|position| rect.contains(position))
        {
            return Some(CsvHeaderAction::Press(index));
        }

        if input.pointer.primary_down()
            && drag.is_some_and(|drag| drag.kind == kind)
            && input
                .pointer
                .latest_pos()
                .is_some_and(|position| rect.contains(position))
        {
            return Some(CsvHeaderAction::ExtendDrag(index));
        }

        None
    })
}

fn paint_csv_row_header(
    ui: &mut egui::Ui,
    row_number: usize,
    width: f32,
    height: f32,
    selected: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click_and_drag());
    paint_csv_box(
        ui,
        rect,
        &row_number.to_string(),
        if selected {
            current_line_background(ui)
        } else {
            csv_header_background(ui)
        },
        if selected {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color()
        },
        egui::Align2::RIGHT_CENTER,
        selected,
    );
    response
}

fn paint_csv_cell(
    ui: &mut egui::Ui,
    text: &str,
    width: f32,
    height: f32,
    selected_row: bool,
    selected_cell: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    paint_csv_box(
        ui,
        rect,
        &csv_cell_display_text(text),
        if selected_row {
            current_line_background(ui)
        } else {
            ui.visuals().extreme_bg_color
        },
        ui.visuals().text_color(),
        egui::Align2::LEFT_CENTER,
        selected_cell,
    );
    response
}

fn paint_csv_box(
    ui: &egui::Ui,
    rect: egui::Rect,
    text: &str,
    fill: Color32,
    text_color: Color32,
    align: egui::Align2,
    strong_border: bool,
) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, fill);

    let stroke_color = if strong_border {
        ui.visuals().widgets.active.bg_stroke.color
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke.color
    };
    let stroke_width = if strong_border { 1.5 } else { 1.0 };
    let stroke = egui::Stroke::new(stroke_width, stroke_color);
    painter.line_segment([rect.left_top(), rect.right_top()], stroke);
    painter.line_segment([rect.left_bottom(), rect.right_bottom()], stroke);
    painter.line_segment([rect.left_top(), rect.left_bottom()], stroke);
    painter.line_segment([rect.right_top(), rect.right_bottom()], stroke);

    let x = match align {
        egui::Align2::RIGHT_CENTER => rect.right() - 6.0,
        egui::Align2::CENTER_CENTER => rect.center().x,
        _ => rect.left() + 6.0,
    };
    painter.text(
        egui::pos2(x, rect.center().y),
        align,
        text,
        editor_font_id(),
        text_color,
    );
}

fn csv_header_background(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(58, 58, 62)
    } else {
        Color32::from_rgb(215, 218, 222)
    }
}

fn csv_cell_display_text(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

fn csv_column_label(mut column: usize) -> String {
    let mut label = String::new();
    column += 1;

    while column > 0 {
        let offset = (column - 1) % 26;
        label.insert(0, (b'A' + offset as u8) as char);
        column = (column - 1) / 26;
    }

    label
}

fn csv_navigation_target(
    ui: &mut egui::Ui,
    text_edit_id: egui::Id,
    row: usize,
    column: usize,
    row_count: usize,
    column_count: usize,
    cell_len: usize,
) -> Option<CsvCellPosition> {
    if !ui.memory(|memory| memory.has_focus(text_edit_id)) {
        return None;
    }

    let cursor_index = TextEditState::load(ui.ctx(), text_edit_id)
        .and_then(|state| state.cursor.char_range().map(|range| range.primary.index))
        .unwrap_or(0);

    ui.input_mut(|input| {
        if input.consume_key(egui::Modifiers::NONE, egui::Key::Home) {
            return Some(CsvCellPosition { row, column: 0 });
        }

        if input.consume_key(egui::Modifiers::NONE, egui::Key::End) {
            return Some(CsvCellPosition {
                row,
                column: column_count.saturating_sub(1),
            });
        }

        // Moving never grows the table; Enter on the last row is the one explicit way to
        // append a row.
        let last_row = row_count.saturating_sub(1);
        let last_column = column_count.saturating_sub(1);
        if input.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
            return Some(CsvCellPosition {
                row: row + 1,
                column,
            });
        }

        if input.consume_key(egui::Modifiers::NONE, egui::Key::Tab) {
            return Some(CsvCellPosition {
                row,
                column: (column + 1).min(last_column),
            });
        }

        if row > 0 && input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
            return Some(CsvCellPosition {
                row: row - 1,
                column,
            });
        }

        if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
            return Some(CsvCellPosition {
                row: (row + 1).min(last_row),
                column,
            });
        }

        if cursor_index == 0
            && column > 0
            && input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)
        {
            return Some(CsvCellPosition {
                row,
                column: column - 1,
            });
        }

        if cursor_index >= cell_len
            && column < last_column
            && input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)
        {
            return Some(CsvCellPosition {
                row,
                column: column + 1,
            });
        }

        None
    })
}

fn debug_or_none(value: &str) -> String {
    if value.is_empty() {
        "none".to_owned()
    } else {
        value.to_owned()
    }
}

fn debug_instant_age_ms(value: Option<Instant>) -> String {
    value
        .map(|instant| format!("{:.0}", instant.elapsed().as_secs_f64() * 1000.0))
        .unwrap_or_else(|| "none".to_owned())
}

fn start_syntax_highlighting(
    path: PathBuf,
    text: String,
    language_registry: LanguageRegistry,
    source_len: usize,
    source_hash: u64,
) -> Receiver<SyntaxHighlightResult> {
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        let mut spans = language_registry
            .language_for_path(&path)
            .map(|language| {
                let document = Document::new(path.clone(), text);
                language.highlight_spans(&document)
            })
            .unwrap_or_default();
        spans.sort_by_key(|span| (span.range.start, span.range.end));

        let _ = sender.send(SyntaxHighlightResult {
            path: Some(path),
            source_len,
            source_hash,
            spans,
        });
    });

    receiver
}

fn draw_workspace_roots(
    ui: &mut egui::Ui,
    roots: &[FileNode],
    root_count: usize,
    git_file_statuses: &HashMap<PathBuf, WorkspaceGitStatus>,
    app: &mut CodeReaderApp,
) {
    for root in roots {
        let mut remove_root = false;
        let has_status = node_contains_git_status(root, git_file_statuses);
        let default_open = app
            .workspace_root_open
            .get(&root.path)
            .copied()
            .unwrap_or(true);
        let state_id = ui.make_persistent_id(("workspace_root", &root.path));
        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            state_id,
            default_open,
        );
        if app
            .pending_workspace_tree_reveal_path
            .as_deref()
            .is_some_and(|path| path.starts_with(&root.path))
        {
            state.set_open(true);
        }
        let mut root_label_clicked = false;
        let selected = app.selected_workspace_path.as_deref() == Some(root.path.as_path());
        let mut header = state.show_header(ui, |ui| {
            root_label_clicked = workspace_tree_folder_label(ui, &root.name, selected).clicked();
            if has_status {
                ui.label(git_folder_status_marker());
            }
            if root_count > 1
                && ui
                    .small_button("×")
                    .on_hover_text("Remove workspace folder")
                    .clicked()
            {
                remove_root = true;
            }
        });
        if root_label_clicked {
            app.workspace_sidebar_active = true;
            app.selected_workspace_path = Some(root.path.clone());
            app.pending_workspace_tree_reveal_path = None;
            header.toggle();
        }
        app.workspace_root_open
            .insert(root.path.clone(), header.is_open());
        header.body(|ui| {
            draw_file_nodes(ui, &root.children, git_file_statuses, app);
        });

        if remove_root {
            app.remove_root_from_current_session(&root.path);
            continue;
        }
        ui.add_space(6.0);
    }
}

fn draw_file_nodes(
    ui: &mut egui::Ui,
    nodes: &[FileNode],
    git_file_statuses: &HashMap<PathBuf, WorkspaceGitStatus>,
    app: &mut CodeReaderApp,
) {
    for node in nodes {
        if node.is_dir() {
            let has_status = node_contains_git_status(node, git_file_statuses);
            let state_id = ui.make_persistent_id(("workspace_dir", &node.path));
            let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                state_id,
                false,
            );
            if app
                .pending_workspace_tree_reveal_path
                .as_deref()
                .is_some_and(|path| path.starts_with(&node.path))
            {
                state.set_open(true);
            }
            let mut folder_label_clicked = false;
            let selected = app.selected_workspace_path.as_deref() == Some(node.path.as_path());
            let mut header = state.show_header(ui, |ui| {
                folder_label_clicked =
                    workspace_tree_folder_label(ui, &node.name, selected).clicked();
                if has_status {
                    ui.label(git_folder_status_marker());
                }
            });
            if folder_label_clicked {
                app.workspace_sidebar_active = true;
                app.selected_workspace_path = Some(node.path.clone());
                app.pending_workspace_tree_reveal_path = None;
                header.toggle();
            }
            header.body(|ui| {
                draw_file_nodes(ui, &node.children, git_file_statuses, app);
            });
        } else {
            let status = git_file_statuses.get(&node.path).copied();
            let mut label = RichText::new(&node.name).monospace();
            if let Some(status) = status {
                label = label.color(status.color());
            }
            ui.horizontal(|ui| {
                let selected = app.selected_workspace_path.as_deref() == Some(node.path.as_path());
                let response = ui.add(
                    egui::Button::new(label)
                        .selected(selected)
                        .sense(egui::Sense::click_and_drag()),
                );
                if response.clicked() {
                    app.workspace_sidebar_active = true;
                    app.selected_workspace_path = Some(node.path.clone());
                    app.pending_workspace_tree_reveal_path = None;
                    app.open_file_in_focused_pane(node.path.clone());
                }
                if app
                    .pending_workspace_tree_reveal_path
                    .as_deref()
                    .is_some_and(|path| path == node.path.as_path())
                {
                    response.scroll_to_me(Some(egui::Align::Center));
                    app.pending_workspace_tree_reveal_path = None;
                }
                if response.drag_started() || response.dragged() {
                    app.dragging_workspace_file = Some(node.path.clone());
                }
                if let Some(status) = status {
                    ui.label(
                        RichText::new(status.label())
                            .monospace()
                            .color(status.color()),
                    );
                }
            });
        }
    }
}

fn workspace_tree_folder_label(ui: &mut egui::Ui, name: &str, selected: bool) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(name).monospace())
            .selected(selected)
            .frame(false)
            .sense(egui::Sense::click()),
    )
}

fn git_folder_status_marker() -> RichText {
    RichText::new("•")
        .monospace()
        .color(WorkspaceGitStatus::Modified.color())
}

fn node_contains_git_status(
    node: &FileNode,
    git_file_statuses: &HashMap<PathBuf, WorkspaceGitStatus>,
) -> bool {
    if git_file_statuses.contains_key(&node.path) {
        return true;
    }
    node.children
        .iter()
        .any(|child| node_contains_git_status(child, git_file_statuses))
}

fn recent_workspace_age_label(last_updated_unix: u64) -> String {
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(last_updated_unix);
    recent_workspace_age_label_at(last_updated_unix, now_unix)
}

fn recent_workspace_age_label_at(last_updated_unix: u64, now_unix: u64) -> String {
    let elapsed = now_unix.saturating_sub(last_updated_unix);

    if elapsed < 60 {
        "just now".to_owned()
    } else if elapsed < 60 * 60 {
        format!("{}m ago", elapsed / 60)
    } else if elapsed < 60 * 60 * 24 {
        format!("{}h ago", elapsed / (60 * 60))
    } else {
        format!("{}d ago", elapsed / (60 * 60 * 24))
    }
}

fn ui_was_primary_clicked(ui: &egui::Ui) -> bool {
    ui.input(|input| {
        input.pointer.button_clicked(egui::PointerButton::Primary)
            && input
                .pointer
                .interact_pos()
                .is_some_and(|position| ui.max_rect().contains(position))
    })
}

fn ui_rect_contains_pointer(ui: &egui::Ui) -> bool {
    ui.input(|input| {
        input
            .pointer
            .interact_pos()
            .is_some_and(|position| ui.max_rect().contains(position))
    })
}

fn workspace_right_pane_drop_target(ui: &mut egui::Ui, path: &Path) -> bool {
    let available = ui.available_size();
    let size = egui::vec2(available.x.max(120.0), available.y.max(160.0));
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let hovered = response.hovered();
    let visuals = ui.visuals();
    let fill = if hovered {
        visuals.selection.bg_fill
    } else {
        visuals.faint_bg_color
    };
    let stroke = if hovered {
        visuals.selection.stroke
    } else {
        visuals.widgets.noninteractive.bg_stroke
    };
    ui.painter().rect_filled(rect, 4.0, fill);
    ui.painter()
        .rect_stroke(rect, 4.0, stroke, egui::StrokeKind::Inside);

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<file>");
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        format!("Drop {file_name} here"),
        FontId::proportional(14.0),
        visuals.text_color(),
    );

    hovered && ui.input(|input| input.pointer.any_released())
}

fn is_external_path(path: &Path) -> bool {
    let display = path.to_string_lossy().replace('\\', "/");
    display.contains("/.cargo/")
        || display.contains("/.rustup/")
        || display.contains("/go/pkg/mod/")
}

fn is_external_rust_path(path: &Path) -> bool {
    if !is_rust_path(path) {
        return false;
    }
    let display = path.to_string_lossy().replace('\\', "/");
    display.contains("/.cargo/") || display.contains("/.rustup/")
}

fn pick_workspace_folder(title: &str) -> Option<PathBuf> {
    platform::current().pick_folder(title)
}

fn file_title(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<invalid>")
        .to_owned()
}

fn validate_workspace_file_name(value: &str) -> Result<String, String> {
    let name = value.trim();
    if name.is_empty() {
        return Err("File name is required.".to_owned());
    }
    if name == "." || name == ".." {
        return Err("File name cannot be . or ..".to_owned());
    }
    if name.chars().any(|character| {
        matches!(
            character,
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
        )
    }) {
        return Err("File name contains an invalid character.".to_owned());
    }
    let path = Path::new(name);
    if path.is_absolute() || path.components().count() != 1 {
        return Err("Enter a file name, not a path.".to_owned());
    }
    Ok(name.to_owned())
}

fn file_name_stem_selection_char_len(file_name: &str) -> usize {
    file_name
        .rfind('.')
        .filter(|index| *index > 0)
        .map(|index| file_name[..index].chars().count())
        .unwrap_or_else(|| file_name.chars().count())
}

fn initial_workspace_file_content(path: &Path) -> String {
    if !is_markdown_path(path) {
        return String::new();
    }

    let title = path
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("New Document");
    format!("# {title}\n")
}

fn recent_files_from_open_files(open_files: &OpenFiles) -> Vec<PathBuf> {
    let mut recent_files = Vec::new();

    if let Some(path) = &open_files.left {
        recent_files.push(path.clone());
    }
    if let Some(path) = &open_files.right
        && !recent_files.iter().any(|recent| recent == path)
    {
        recent_files.push(path.clone());
    }

    recent_files
}

fn load_state(path: &Path) -> Option<AppState> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_state(path: &Path, app_state: &AppState) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut persisted_state = app_state.clone();
    for session in &mut persisted_state.sessions {
        session.bookmarks.retain(|bookmark| !bookmark.temporary);
    }

    let text = serde_json::to_string_pretty(&persisted_state)?;
    fs::write(path, text)?;
    Ok(())
}

#[cfg(test)]
mod event_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn test_navigation_point(file_name: &str, line: u32) -> NavigationPoint {
        NavigationPoint {
            path: PathBuf::from(file_name),
            position: Position::new(line, 0),
        }
    }

    #[test]
    fn smart_newline_keeps_indent_and_adds_one_level_after_open_brace() {
        let source = "fn main() {\n    if ready {";
        let cursor = source.chars().count();

        assert_eq!(smart_newline(source, cursor, 4), "\n        ");
    }

    #[test]
    fn strip_trailing_whitespace_preserves_newlines() {
        let source = "let a = 1;   \r\nlet b = 2;\t\nlet c = 3;  ";

        assert_eq!(
            strip_trailing_whitespace(source),
            "let a = 1;\r\nlet b = 2;\nlet c = 3;"
        );
    }

    #[test]
    fn leading_indent_columns_expands_tabs_to_tab_stops() {
        assert_eq!(leading_indent_columns("    value", 4), 4);
        assert_eq!(leading_indent_columns("\tvalue", 4), 4);
        assert_eq!(leading_indent_columns("  \tvalue", 4), 4);
        assert_eq!(leading_indent_columns("    \tvalue", 4), 8);
        assert_eq!(leading_indent_columns("value", 4), 0);
    }

    #[test]
    fn validate_workspace_file_name_accepts_simple_names() {
        assert_eq!(
            validate_workspace_file_name(" idea.md "),
            Ok("idea.md".to_owned())
        );
        assert_eq!(
            validate_workspace_file_name("Cargo.lock"),
            Ok("Cargo.lock".to_owned())
        );
    }

    #[test]
    fn validate_workspace_file_name_rejects_paths() {
        assert!(validate_workspace_file_name("../idea.md").is_err());
        assert!(validate_workspace_file_name("docs/idea.md").is_err());
        assert!(validate_workspace_file_name("C:\\temp\\idea.md").is_err());
        assert!(validate_workspace_file_name("").is_err());
    }

    #[test]
    fn initial_workspace_file_content_uses_markdown_for_markdown_files() {
        assert_eq!(
            initial_workspace_file_content(Path::new("idea.md")),
            "# idea\n"
        );
        assert_eq!(
            initial_workspace_file_content(Path::new("note.mdx")),
            "# note\n"
        );
    }

    #[test]
    fn initial_workspace_file_content_keeps_non_markdown_files_empty() {
        assert_eq!(initial_workspace_file_content(Path::new("new.csv")), "");
        assert_eq!(initial_workspace_file_content(Path::new("main.rs")), "");
        assert_eq!(initial_workspace_file_content(Path::new("config.toml")), "");
    }

    #[test]
    fn file_name_stem_selection_char_len_selects_before_extension() {
        assert_eq!(file_name_stem_selection_char_len("README.md"), 6);
        assert_eq!(file_name_stem_selection_char_len("archive.tar.gz"), 11);
    }

    #[test]
    fn file_name_stem_selection_char_len_selects_all_without_extension() {
        assert_eq!(file_name_stem_selection_char_len(".gitignore"), 10);
        assert_eq!(file_name_stem_selection_char_len("README"), 6);
    }

    #[test]
    fn virtual_click_byte_column_treats_gutter_click_as_line_start() {
        assert_eq!(
            virtual_click_byte_column(None, "abcdef", 42.0, -120.0, 80.0, 10.0),
            0
        );
    }

    #[test]
    fn virtual_click_byte_column_uses_visible_text_offset() {
        assert_eq!(
            virtual_click_byte_column(None, "abcdef", 110.0, 80.0, 80.0, 10.0),
            3
        );
    }

    #[test]
    fn visual_column_for_byte_column_counts_characters_before_byte_column() {
        assert_eq!(visual_column_for_byte_column("abcdef", 3), 3);
        assert_eq!(visual_column_for_byte_column("aあb", 4), 2);
        assert_eq!(visual_column_for_byte_column("aあb", 2), 1);
    }

    #[test]
    fn virtual_scrollbar_hit_rects_keep_editor_body_clickable() {
        let viewport = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(210.0, 320.0));
        let (vertical, horizontal) = virtual_scrollbar_hit_rects(viewport, 400.0, 800.0);

        assert!(!virtual_scrollbar_hit_contains(
            vertical,
            horizontal,
            egui::pos2(100.0, 100.0)
        ));
        assert!(virtual_scrollbar_hit_contains(
            vertical,
            horizontal,
            egui::pos2(205.0, 100.0)
        ));
        assert!(virtual_scrollbar_hit_contains(
            vertical,
            horizontal,
            egui::pos2(100.0, 315.0)
        ));
    }

    #[test]
    fn virtual_scrollbar_layout_reserves_space_below_content() {
        let viewport = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(210.0, 320.0));
        let layout = virtual_scrollbar_layout(viewport, 400.0, 800.0);
        let horizontal = layout.horizontal_rect.expect("horizontal scrollbar");
        let vertical = layout.vertical_rect.expect("vertical scrollbar");

        assert!(layout.content_rect.bottom() <= horizontal.top());
        assert!(layout.content_rect.right() <= vertical.left());
        assert_eq!(horizontal.bottom(), viewport.bottom());
        assert_eq!(vertical.right(), viewport.right());
    }

    #[test]
    fn virtual_text_selection_normalizes_drag_direction() {
        let selection = VirtualTextSelection {
            anchor: Position::new(3, 8),
            head: Position::new(1, 2),
        };

        assert_eq!(
            selection.normalized(),
            Some(NormalizedVirtualSelection {
                start: Position::new(1, 2),
                end: Position::new(3, 8),
            })
        );
    }

    #[test]
    fn virtual_text_selection_ignores_empty_selection() {
        let selection = VirtualTextSelection {
            anchor: Position::new(3, 8),
            head: Position::new(3, 8),
        };

        assert_eq!(selection.normalized(), None);
    }

    #[test]
    fn virtual_selection_text_copies_single_line_range() {
        let buffer = EditorTextBuffer::new("alpha beta\ngamma".to_owned());
        let selection = NormalizedVirtualSelection {
            start: Position::new(0, 6),
            end: Position::new(0, 10),
        };

        assert_eq!(
            virtual_selection_text(&buffer, selection),
            Some("beta".to_owned())
        );
    }

    #[test]
    fn virtual_selection_text_copies_multi_line_range_with_newline() {
        let buffer = EditorTextBuffer::new("alpha beta\ngamma delta\nomega".to_owned());
        let selection = NormalizedVirtualSelection {
            start: Position::new(0, 6),
            end: Position::new(1, 5),
        };

        assert_eq!(
            virtual_selection_text(&buffer, selection),
            Some("beta\ngamma".to_owned())
        );
    }

    #[test]
    fn virtual_selection_text_clamps_to_utf8_boundary() {
        let buffer = EditorTextBuffer::new("aあb\nnext".to_owned());
        let selection = NormalizedVirtualSelection {
            start: Position::new(0, 1),
            end: Position::new(0, 4),
        };

        assert_eq!(
            virtual_selection_text(&buffer, selection),
            Some("あ".to_owned())
        );
    }

    #[test]
    fn virtual_word_selection_selects_identifier() {
        let selection = virtual_word_selection_at_position("text::{CCursor, LayoutJob}", 4, 9)
            .and_then(|selection| selection.normalized());

        assert_eq!(
            selection,
            Some(NormalizedVirtualSelection {
                start: Position::new(4, 7),
                end: Position::new(4, 14),
            })
        );
    }

    #[test]
    fn virtual_word_selection_uses_previous_word_at_right_edge() {
        let selection = virtual_word_selection_at_position("fmt.Sprint(", 2, 10)
            .and_then(|selection| selection.normalized());

        assert_eq!(
            selection,
            Some(NormalizedVirtualSelection {
                start: Position::new(2, 4),
                end: Position::new(2, 10),
            })
        );
    }

    #[test]
    fn virtual_word_selection_ignores_punctuation_only_position() {
        assert_eq!(virtual_word_selection_at_position("::", 0, 1), None);
    }

    #[test]
    fn virtual_insert_inserts_at_cursor() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.text = EditorTextBuffer::new("abcd".to_owned());
        editor.read_only = false;
        editor.cursor_position = Some(Position::new(0, 2));

        assert!(editor.apply_virtual_insert("X"));
        assert_eq!(editor.text.as_str(), "abXcd");
        assert_eq!(editor.cursor_position, Some(Position::new(0, 3)));
        assert_eq!(editor.virtual_selection, None);
    }

    #[test]
    fn virtual_insert_replaces_selection() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.text = EditorTextBuffer::new("abcd".to_owned());
        editor.read_only = false;
        editor.cursor_position = Some(Position::new(0, 3));
        editor.virtual_selection = Some(VirtualTextSelection {
            anchor: Position::new(0, 1),
            head: Position::new(0, 3),
        });

        assert!(editor.apply_virtual_insert("X"));
        assert_eq!(editor.text.as_str(), "aXd");
        assert_eq!(editor.cursor_position, Some(Position::new(0, 2)));
        assert_eq!(editor.virtual_selection, None);
    }

    #[test]
    fn virtual_backspace_removes_previous_character_across_lines() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.text = EditorTextBuffer::new("a\nb".to_owned());
        editor.read_only = false;
        editor.cursor_position = Some(Position::new(1, 0));

        assert!(editor.apply_virtual_backspace());
        assert_eq!(editor.text.as_str(), "ab");
        assert_eq!(editor.cursor_position, Some(Position::new(0, 1)));
    }

    #[test]
    fn virtual_delete_removes_next_character_across_lines() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.text = EditorTextBuffer::new("a\nb".to_owned());
        editor.read_only = false;
        editor.cursor_position = Some(Position::new(0, 1));

        assert!(editor.apply_virtual_delete());
        assert_eq!(editor.text.as_str(), "ab");
        assert_eq!(editor.cursor_position, Some(Position::new(0, 1)));
    }

    #[test]
    fn virtual_insert_supports_multi_line_paste() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.text = EditorTextBuffer::new("ab".to_owned());
        editor.read_only = false;
        editor.cursor_position = Some(Position::new(0, 1));

        assert!(editor.apply_virtual_insert("x\ny"));
        assert_eq!(editor.text.as_str(), "ax\nyb");
        assert_eq!(editor.cursor_position, Some(Position::new(1, 1)));
    }

    #[test]
    fn virtual_edit_rejects_read_only_file() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.text = EditorTextBuffer::new("ab".to_owned());
        editor.read_only = true;
        editor.cursor_position = Some(Position::new(0, 1));

        assert!(!editor.apply_virtual_insert("X"));
        assert!(!editor.apply_virtual_backspace());
        assert!(!editor.apply_virtual_delete());
        assert_eq!(editor.text.as_str(), "ab");
    }

    #[test]
    fn virtual_undo_redo_restores_insert() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.text = EditorTextBuffer::new("ab".to_owned());
        editor.read_only = false;
        editor.cursor_position = Some(Position::new(0, 1));

        assert!(editor.apply_virtual_insert("X"));
        assert_eq!(editor.text.as_str(), "aXb");
        assert_eq!(editor.virtual_undo_stack.len(), 1);
        assert_eq!(editor.virtual_redo_stack.len(), 0);

        assert!(editor.apply_virtual_undo());
        assert_eq!(editor.text.as_str(), "ab");
        assert_eq!(editor.cursor_position, Some(Position::new(0, 1)));
        assert_eq!(editor.virtual_undo_stack.len(), 0);
        assert_eq!(editor.virtual_redo_stack.len(), 1);

        assert!(editor.apply_virtual_redo());
        assert_eq!(editor.text.as_str(), "aXb");
        assert_eq!(editor.cursor_position, Some(Position::new(0, 2)));
        assert_eq!(editor.virtual_undo_stack.len(), 1);
        assert_eq!(editor.virtual_redo_stack.len(), 0);
    }

    #[test]
    fn virtual_undo_redo_restores_selection_replacement() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.text = EditorTextBuffer::new("abcd".to_owned());
        editor.read_only = false;
        editor.cursor_position = Some(Position::new(0, 3));
        editor.virtual_selection = Some(VirtualTextSelection {
            anchor: Position::new(0, 1),
            head: Position::new(0, 3),
        });

        assert!(editor.apply_virtual_insert("X"));
        assert_eq!(editor.text.as_str(), "aXd");
        assert_eq!(editor.virtual_selection, None);

        assert!(editor.apply_virtual_undo());
        assert_eq!(editor.text.as_str(), "abcd");
        assert_eq!(editor.cursor_position, Some(Position::new(0, 3)));
        assert_eq!(
            editor.virtual_selection,
            Some(VirtualTextSelection {
                anchor: Position::new(0, 1),
                head: Position::new(0, 3),
            })
        );

        assert!(editor.apply_virtual_redo());
        assert_eq!(editor.text.as_str(), "aXd");
        assert_eq!(editor.cursor_position, Some(Position::new(0, 2)));
        assert_eq!(editor.virtual_selection, None);
    }

    #[test]
    fn virtual_undo_restores_delete_and_backspace() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.text = EditorTextBuffer::new("a\nb".to_owned());
        editor.read_only = false;
        editor.cursor_position = Some(Position::new(1, 0));

        assert!(editor.apply_virtual_backspace());
        assert_eq!(editor.text.as_str(), "ab");
        assert!(editor.apply_virtual_undo());
        assert_eq!(editor.text.as_str(), "a\nb");
        assert_eq!(editor.cursor_position, Some(Position::new(1, 0)));

        editor.cursor_position = Some(Position::new(0, 1));
        assert!(editor.apply_virtual_delete());
        assert_eq!(editor.text.as_str(), "ab");
        assert!(editor.apply_virtual_undo());
        assert_eq!(editor.text.as_str(), "a\nb");
        assert_eq!(editor.cursor_position, Some(Position::new(0, 1)));
    }

    #[test]
    fn virtual_new_edit_clears_redo_stack() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.text = EditorTextBuffer::new("ab".to_owned());
        editor.read_only = false;
        editor.cursor_position = Some(Position::new(0, 1));

        assert!(editor.apply_virtual_insert("X"));
        assert!(editor.apply_virtual_undo());
        assert_eq!(editor.virtual_redo_stack.len(), 1);

        assert!(editor.apply_virtual_insert("Y"));
        assert_eq!(editor.text.as_str(), "aYb");
        assert_eq!(editor.virtual_redo_stack.len(), 0);
        assert!(!editor.apply_virtual_redo());
    }

    #[test]
    fn virtual_ime_preedit_does_not_change_text_until_commit() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.text = EditorTextBuffer::new("ab".to_owned());
        editor.read_only = false;
        editor.cursor_position = Some(Position::new(0, 1));

        assert_eq!(
            editor.apply_virtual_ime_event(&egui::ImeEvent::Preedit("に".to_owned())),
            None
        );
        assert_eq!(editor.text.as_str(), "ab");
        assert_eq!(editor.virtual_ime_preedit, Some("に".to_owned()));

        assert_eq!(
            editor.apply_virtual_ime_event(&egui::ImeEvent::Commit("日".to_owned())),
            Some(VirtualEditCommand::Insert("日".to_owned()))
        );
        assert_eq!(editor.virtual_ime_preedit, None);
    }

    #[test]
    fn virtual_ime_commit_uses_edit_history() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.text = EditorTextBuffer::new("ab".to_owned());
        editor.read_only = false;
        editor.cursor_position = Some(Position::new(0, 1));
        let command = editor.apply_virtual_ime_event(&egui::ImeEvent::Commit("日".to_owned()));

        if let Some(VirtualEditCommand::Insert(text)) = command {
            assert!(editor.apply_virtual_insert(&text));
        } else {
            panic!("expected IME commit to produce insert command");
        }

        assert_eq!(editor.text.as_str(), "a日b");
        assert_eq!(editor.virtual_undo_stack.len(), 1);
        assert!(editor.apply_virtual_undo());
        assert_eq!(editor.text.as_str(), "ab");
    }

    #[test]
    fn virtual_ime_disabled_clears_preedit() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.virtual_ime_preedit = Some("変換中".to_owned());

        assert_eq!(
            editor.apply_virtual_ime_event(&egui::ImeEvent::Disabled),
            None
        );
        assert_eq!(editor.virtual_ime_preedit, None);
    }

    #[test]
    fn virtual_navigation_moves_across_line_boundaries() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.text = EditorTextBuffer::new("ab\ncde".to_owned());
        editor.cursor_position = Some(Position::new(0, 2));
        let line_index = editor.text.line_index();

        assert!(editor.apply_virtual_navigation(
            VirtualNavigationCommand {
                target: VirtualNavigationTarget::Right,
                extend_selection: false,
            },
            line_index.as_ref(),
            17.0,
            170.0,
            200.0,
            10.0,
            0.0,
            1000.0,
        ));
        assert_eq!(editor.cursor_position, Some(Position::new(1, 0)));

        assert!(editor.apply_virtual_navigation(
            VirtualNavigationCommand {
                target: VirtualNavigationTarget::Left,
                extend_selection: false,
            },
            line_index.as_ref(),
            17.0,
            170.0,
            200.0,
            10.0,
            0.0,
            1000.0,
        ));
        assert_eq!(editor.cursor_position, Some(Position::new(0, 2)));
    }

    #[test]
    fn virtual_navigation_extends_selection_with_shift() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.text = EditorTextBuffer::new("abcd\nef".to_owned());
        editor.cursor_position = Some(Position::new(0, 1));
        let line_index = editor.text.line_index();

        assert!(editor.apply_virtual_navigation(
            VirtualNavigationCommand {
                target: VirtualNavigationTarget::Right,
                extend_selection: true,
            },
            line_index.as_ref(),
            17.0,
            170.0,
            200.0,
            10.0,
            0.0,
            1000.0,
        ));
        assert_eq!(editor.cursor_position, Some(Position::new(0, 2)));
        assert_eq!(
            editor.virtual_selection,
            Some(VirtualTextSelection {
                anchor: Position::new(0, 1),
                head: Position::new(0, 2),
            })
        );

        assert!(editor.apply_virtual_navigation(
            VirtualNavigationCommand {
                target: VirtualNavigationTarget::Down,
                extend_selection: true,
            },
            line_index.as_ref(),
            17.0,
            170.0,
            200.0,
            10.0,
            0.0,
            1000.0,
        ));
        assert_eq!(
            editor.virtual_selection,
            Some(VirtualTextSelection {
                anchor: Position::new(0, 1),
                head: Position::new(1, 2),
            })
        );
    }

    #[test]
    fn virtual_navigation_home_end_and_select_all() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.text = EditorTextBuffer::new("abcd\nef".to_owned());
        editor.cursor_position = Some(Position::new(0, 2));
        let line_index = editor.text.line_index();

        assert!(editor.apply_virtual_navigation(
            VirtualNavigationCommand {
                target: VirtualNavigationTarget::End,
                extend_selection: false,
            },
            line_index.as_ref(),
            17.0,
            170.0,
            200.0,
            10.0,
            0.0,
            1000.0,
        ));
        assert_eq!(editor.cursor_position, Some(Position::new(0, 4)));

        assert!(editor.apply_virtual_navigation(
            VirtualNavigationCommand {
                target: VirtualNavigationTarget::Home,
                extend_selection: false,
            },
            line_index.as_ref(),
            17.0,
            170.0,
            200.0,
            10.0,
            0.0,
            1000.0,
        ));
        assert_eq!(editor.cursor_position, Some(Position::new(0, 0)));

        assert!(editor.apply_virtual_navigation(
            VirtualNavigationCommand {
                target: VirtualNavigationTarget::SelectAll,
                extend_selection: false,
            },
            line_index.as_ref(),
            17.0,
            170.0,
            200.0,
            10.0,
            0.0,
            1000.0,
        ));
        assert_eq!(
            editor.virtual_selection,
            Some(VirtualTextSelection {
                anchor: Position::new(0, 0),
                head: Position::new(1, 2),
            })
        );
    }

    #[test]
    fn virtual_navigation_page_moves_cursor_and_scroll() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.text = EditorTextBuffer::new((0..100).map(|i| format!("{i}\n")).collect());
        editor.cursor_position = Some(Position::new(0, 0));
        let line_index = editor.text.line_index();

        assert!(editor.apply_virtual_navigation(
            VirtualNavigationCommand {
                target: VirtualNavigationTarget::PageDown,
                extend_selection: false,
            },
            line_index.as_ref(),
            10.0,
            100.0,
            200.0,
            10.0,
            0.0,
            900.0,
        ));
        assert_eq!(editor.cursor_position, Some(Position::new(9, 0)));
        assert_eq!(editor.scroll_y, 90.0);

        assert!(editor.apply_virtual_navigation(
            VirtualNavigationCommand {
                target: VirtualNavigationTarget::PageUp,
                extend_selection: false,
            },
            line_index.as_ref(),
            10.0,
            100.0,
            200.0,
            10.0,
            0.0,
            900.0,
        ));
        assert_eq!(editor.cursor_position, Some(Position::new(0, 0)));
        assert_eq!(editor.scroll_y, 0.0);
    }

    #[test]
    fn virtual_pointer_to_text_position_uses_scroll_and_text_offset() {
        let buffer = EditorTextBuffer::new("zero\none two\nthree".to_owned());
        let line_index = buffer.line_index();
        let viewport = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(210.0, 120.0));
        let text_clip = egui::Rect::from_min_max(egui::pos2(50.0, 20.0), egui::pos2(210.0, 120.0));

        assert_eq!(
            virtual_pointer_to_text_position(
                None,
                egui::pos2(80.0, 37.0),
                &line_index,
                &buffer,
                viewport,
                text_clip,
                50.0,
                17.0,
                17.0,
                10.0,
            ),
            Some(Position::new(2, 3))
        );
    }

    #[test]
    fn virtual_indent_guide_lines_skip_blank_lines_and_track_active_line() {
        let buffer = EditorTextBuffer::new(
            "root\n    child\n        grandchild\n    \n\tchild\n".to_owned(),
        );
        let line_index = buffer.line_index();

        assert_eq!(
            virtual_indent_guide_lines(&line_index, &buffer, 0..5, 4, Some(2)),
            vec![
                VirtualIndentGuideLine {
                    line_number: 1,
                    guide_count: 1,
                    active: false,
                },
                VirtualIndentGuideLine {
                    line_number: 2,
                    guide_count: 2,
                    active: true,
                },
                VirtualIndentGuideLine {
                    line_number: 4,
                    guide_count: 1,
                    active: false,
                },
            ]
        );
    }

    #[test]
    fn navigation_history_supports_back_and_forward() {
        let mut history = NavigationHistory::default();
        let a = test_navigation_point("a.rs", 17);
        let b = test_navigation_point("b.rs", 41);

        history.push_back(a.clone());
        assert_eq!(history.go_back(Some(b.clone())), Some(a.clone()));
        assert_eq!(history.go_forward(Some(a)), Some(b));
    }

    #[test]
    fn navigation_history_clicking_back_entry_preserves_forward_path() {
        let mut history = NavigationHistory::default();
        let a = test_navigation_point("a.rs", 17);
        let b = test_navigation_point("b.rs", 41);
        let c = test_navigation_point("c.rs", 9);

        history.push_back(a.clone());
        history.push_back(b.clone());

        assert_eq!(history.open_back_entry(0, Some(c.clone())), Some(a.clone()));
        assert_eq!(history.back, Vec::<NavigationPoint>::new());
        assert_eq!(history.go_forward(Some(a.clone())), Some(b.clone()));
        assert_eq!(history.go_forward(Some(b)), Some(c));
    }

    #[test]
    fn source_search_matches_returns_byte_ranges() {
        let matches = source_search_matches("alpha beta alpha", "alpha");

        assert_eq!(
            matches,
            vec![
                SourceSearchMatch { start: 0, end: 5 },
                SourceSearchMatch { start: 11, end: 16 },
            ]
        );
    }

    #[test]
    fn workspace_search_file_returns_line_positions_and_previews() {
        let path = PathBuf::from("src/app.rs");
        let result = workspace_search_file(
            path.clone(),
            "alpha\nlet beta = alpha;\nalpha beta",
            "alpha",
        )
        .unwrap();

        assert_eq!(result.path, path);
        assert_eq!(
            result.matches,
            vec![
                WorkspaceSearchMatch {
                    position: Position::new(0, 0),
                    preview: "alpha".to_owned(),
                },
                WorkspaceSearchMatch {
                    position: Position::new(1, 11),
                    preview: "let beta = alpha;".to_owned(),
                },
                WorkspaceSearchMatch {
                    position: Position::new(2, 0),
                    preview: "alpha beta".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn trim_git_command_output_preserves_porcelain_status_prefix() {
        assert_eq!(
            trim_git_command_output(" M idea.md\r\n M src/ui/app_view.rs\r\n"),
            " M idea.md\r\n M src/ui/app_view.rs"
        );
    }

    #[test]
    fn parse_git_status_keeps_unstaged_first_line_path_and_state() {
        let changes = parse_git_status(" M idea.md\0 M src/ui/app_view.rs\0");

        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].path, "idea.md");
        assert!(!changes[0].is_staged());
        assert!(changes[0].has_worktree_change());
        assert_eq!(changes[1].path, "src/ui/app_view.rs");
        assert!(!changes[1].is_staged());
    }

    #[test]
    fn parse_git_status_reads_nul_separated_paths_and_skips_rename_sources() {
        let changes = parse_git_status("R  new name.txt\0old name.txt\0?? 日本語.txt\0");

        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].status, "R");
        assert_eq!(changes[0].path, "new name.txt");
        assert_eq!(changes[1].status, "??");
        assert_eq!(changes[1].path, "日本語.txt");
    }

    #[test]
    fn parse_git_tracking_status_reads_ahead_and_behind_counts() {
        assert_eq!(
            parse_git_tracking_status("0\t3\n"),
            Some(GitTrackingStatus {
                ahead: 0,
                behind: 3,
            })
        );
    }

    #[test]
    fn preferred_git_remote_prefers_origin() {
        assert_eq!(
            preferred_git_remote("upstream\norigin\nfork\n"),
            Some("origin".to_owned())
        );
    }

    #[test]
    fn preferred_git_remote_uses_first_remote_without_origin() {
        assert_eq!(
            preferred_git_remote("upstream\nfork\n"),
            Some("upstream".to_owned())
        );
    }

    #[test]
    fn preferred_git_remote_returns_none_for_empty_output() {
        assert_eq!(preferred_git_remote("\n  \n"), None);
    }

    #[test]
    fn workspace_git_file_statuses_maps_git_paths_to_workspace_paths() {
        let repository_root = PathBuf::from(r"C:\git\code-reader");
        let repository = GitRepositoryState {
            root: repository_root.clone(),
            name: "code-reader".to_owned(),
            branch: "main".to_owned(),
            upstream: Some("origin/main".to_owned()),
            tracking: None,
            branches: vec!["main".to_owned()],
            changes: vec![
                GitChange {
                    status: "M".to_owned(),
                    index_status: ' ',
                    worktree_status: 'M',
                    path: "src/ui/app_view.rs".to_owned(),
                },
                GitChange {
                    status: "?".to_owned(),
                    index_status: '?',
                    worktree_status: '?',
                    path: "notes/todo.md".to_owned(),
                },
            ],
            error: None,
        };

        let statuses = workspace_git_file_statuses(&[repository]);

        assert_eq!(
            statuses.get(&repository_root.join("src").join("ui").join("app_view.rs")),
            Some(&WorkspaceGitStatus::Modified)
        );
        assert_eq!(
            statuses.get(&repository_root.join("notes").join("todo.md")),
            Some(&WorkspaceGitStatus::Untracked)
        );
    }

    #[test]
    fn workspace_snapshot_change_count_counts_added_modified_and_removed_files() {
        let unchanged = PathBuf::from("unchanged.rs");
        let modified = PathBuf::from("modified.rs");
        let added = PathBuf::from("added.rs");
        let removed = PathBuf::from("removed.rs");

        let previous = HashMap::from([
            (
                unchanged.clone(),
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 10,
                    len: 20,
                },
            ),
            (
                modified.clone(),
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 10,
                    len: 20,
                },
            ),
            (
                removed,
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 10,
                    len: 20,
                },
            ),
        ]);
        let next = HashMap::from([
            (
                unchanged,
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 10,
                    len: 20,
                },
            ),
            (
                modified,
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 11,
                    len: 20,
                },
            ),
            (
                added,
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 10,
                    len: 20,
                },
            ),
        ]);

        assert_eq!(workspace_snapshot_change_count(&previous, &next), 3);
    }

    #[test]
    fn workspace_snapshot_diff_classifies_changed_files() {
        let unchanged = PathBuf::from("unchanged.rs");
        let modified = PathBuf::from("modified.rs");
        let added = PathBuf::from("added.rs");
        let removed = PathBuf::from("removed.rs");

        let previous = HashMap::from([
            (
                unchanged.clone(),
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 10,
                    len: 20,
                },
            ),
            (
                modified.clone(),
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 10,
                    len: 20,
                },
            ),
            (
                removed.clone(),
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 10,
                    len: 20,
                },
            ),
        ]);
        let next = HashMap::from([
            (
                unchanged,
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 10,
                    len: 20,
                },
            ),
            (
                modified.clone(),
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 11,
                    len: 20,
                },
            ),
            (
                added.clone(),
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 10,
                    len: 20,
                },
            ),
        ]);

        let diff = workspace_snapshot_diff(&previous, &next);

        assert_eq!(diff.added, vec![added]);
        assert_eq!(diff.modified, vec![modified]);
        assert_eq!(diff.removed, vec![removed]);
    }

    #[test]
    fn workspace_snapshot_diff_requires_full_reindex_for_dependency_files() {
        let diff = WorkspaceSnapshotDiff {
            added: Vec::new(),
            modified: vec![PathBuf::from("Cargo.lock")],
            removed: Vec::new(),
        };

        assert!(diff.requires_full_symbol_reindex());
    }

    #[test]
    fn parse_git_history_graph_log_reads_commit_nodes_refs_and_graph_rows() {
        let output = concat!(
            "* \x1faaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\x1fbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\x1f",
            "Fujita Naoyuki\x1f2026-05-28T20:30:12+09:00\x1fHEAD -> main, origin/main\x1f",
            "Diff graph MVP\x1e\n",
            "|/  \n",
            "* \x1fbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\x1f\x1f",
            "Fujita Naoyuki\x1f2026-05-27T23:59:00+09:00\x1f\x1fInitial graph\x1e\n",
        );

        let rows = parse_git_history_graph_log(output);

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].graph, "*");
        let first_commit = rows[0]
            .commit
            .as_ref()
            .expect("first row should contain a commit");
        assert_eq!(first_commit.short_hash, "aaaaaaaa");
        assert_eq!(
            first_commit.parents,
            vec!["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"]
        );
        assert_eq!(first_commit.refs, vec!["main", "origin/main"]);
        assert_eq!(first_commit.date, "2026-05-28 20:30");
        assert_eq!(rows[1].graph, "|/");
        assert!(rows[1].commit.is_none());
        assert_eq!(rows[2].graph, "*");
        assert_eq!(
            rows[2]
                .commit
                .as_ref()
                .map(|commit| commit.short_hash.as_str()),
            Some("bbbbbbbb")
        );
    }

    #[test]
    fn parse_git_commit_changed_files_reads_status_and_renames() {
        let files = parse_git_commit_changed_files(concat!(
            "M\tREADME.md\n",
            "A\tsrc/app.rs\n",
            "R100\told.rs\tnew.rs\n",
            "D\tremoved.txt\n",
        ));

        assert_eq!(
            files,
            vec![
                GitCommitFileChange {
                    status: "M".to_owned(),
                    path: "README.md".to_owned(),
                    old_path: None,
                },
                GitCommitFileChange {
                    status: "A".to_owned(),
                    path: "src/app.rs".to_owned(),
                    old_path: None,
                },
                GitCommitFileChange {
                    status: "R100".to_owned(),
                    path: "new.rs".to_owned(),
                    old_path: Some("old.rs".to_owned()),
                },
                GitCommitFileChange {
                    status: "D".to_owned(),
                    path: "removed.txt".to_owned(),
                    old_path: None,
                },
            ]
        );
    }

    #[test]
    fn parse_diff_display_rows_uses_real_hunk_line_numbers() {
        let rows = parse_diff_display_rows(concat!(
            "diff --git a/src/main.rs b/src/main.rs\n",
            "index 111..222 100644\n",
            "--- a/src/main.rs\n",
            "+++ b/src/main.rs\n",
            "@@ -10,2 +10,3 @@ fn main() {\n",
            " keep();\n",
            "-old();\n",
            "+new();\n",
            "+more();\n",
        ));

        assert!(matches!(
            rows.first(),
            Some(DiffDisplayRow::FileHeader { path, status })
                if path == "src/main.rs" && *status == "M"
        ));
        assert!(rows.contains(&DiffDisplayRow::Line {
            line_label: "10".to_owned(),
            content: "keep();".to_owned(),
            kind: DiffLineKind::Context,
        }));
        assert!(rows.contains(&DiffDisplayRow::Line {
            line_label: "-11".to_owned(),
            content: "old();".to_owned(),
            kind: DiffLineKind::Removed,
        }));
        assert!(rows.contains(&DiffDisplayRow::Line {
            line_label: "+11".to_owned(),
            content: "new();".to_owned(),
            kind: DiffLineKind::Added,
        }));
        assert!(rows.contains(&DiffDisplayRow::Line {
            line_label: "+12".to_owned(),
            content: "more();".to_owned(),
            kind: DiffLineKind::Added,
        }));
    }

    #[test]
    fn parse_diff_display_rows_splits_files() {
        let rows = parse_diff_display_rows(concat!(
            "diff --code-reader Worktree Changes\n",
            "diff --git a/a.txt b/a.txt\n",
            "--- a/a.txt\n",
            "+++ b/a.txt\n",
            "@@ -1 +1 @@\n",
            "-a\n",
            "+b\n",
            "diff --git a/new.txt b/new.txt\n",
            "new file mode 100644\n",
            "--- /dev/null\n",
            "+++ b/new.txt\n",
            "@@ -0,0 +1 @@\n",
            "+hello\n",
        ));

        assert!(rows.contains(&DiffDisplayRow::Section("Worktree Changes".to_owned())));
        assert!(rows.contains(&DiffDisplayRow::FileHeader {
            path: "a.txt".to_owned(),
            status: "M",
        }));
        assert!(rows.contains(&DiffDisplayRow::FileHeader {
            path: "new.txt".to_owned(),
            status: "A",
        }));
    }

    #[test]
    fn parse_diff_display_rows_hides_hunk_and_no_newline_markers() {
        let rows = parse_diff_display_rows(concat!(
            "diff --git a/test.md b/test.md\n",
            "--- a/test.md\n",
            "+++ b/test.md\n",
            "@@ -30,4 +30,6 @@ abcde\n",
            " line\n",
            "-old\n",
            "\\ No newline at end of file\n",
            "+new\n",
            "\\ No newline at end of file\n",
        ));

        assert!(
            !rows
                .iter()
                .any(|row| matches!(row, DiffDisplayRow::Hunk(_)))
        );
        assert!(!rows.iter().any(|row| matches!(
            row,
            DiffDisplayRow::Line { content, .. } if content.contains("No newline at end of file")
        )));
        assert!(rows.contains(&DiffDisplayRow::Line {
            line_label: "30".to_owned(),
            content: "line".to_owned(),
            kind: DiffLineKind::Context,
        }));
    }

    #[test]
    fn parse_diff_display_rows_adds_hidden_line_markers_between_hunks() {
        let rows = parse_diff_display_rows(concat!(
            "diff --git a/test.md b/test.md\n",
            "--- a/test.md\n",
            "+++ b/test.md\n",
            "@@ -10,1 +10,1 @@\n",
            "-old\n",
            "+new\n",
            "@@ -40,1 +40,1 @@\n",
            "-old2\n",
            "+new2\n",
        ));

        assert!(rows.contains(&DiffDisplayRow::HiddenLines(29)));
    }

    #[test]
    fn diff_side_selection_text_copies_only_selected_side() {
        let rows = vec![
            DiffRenderRow::SideBySide {
                old: DiffCell {
                    line_label: "-1",
                    content: "old alpha",
                    kind: DiffLineKind::Removed,
                },
                new: DiffCell {
                    line_label: "+1",
                    content: "new alpha",
                    kind: DiffLineKind::Added,
                },
            },
            DiffRenderRow::SideBySide {
                old: DiffCell {
                    line_label: "-2",
                    content: "old beta",
                    kind: DiffLineKind::Removed,
                },
                new: DiffCell {
                    line_label: "+2",
                    content: "new beta",
                    kind: DiffLineKind::Added,
                },
            },
        ];
        let selection = DiffSideSelection {
            side: DiffSide::Old,
            anchor: DiffSideSelectionPoint { row: 0, column: 4 },
            head: DiffSideSelectionPoint { row: 1, column: 3 },
            dragging: false,
        };

        assert_eq!(
            diff_side_selection_text_from_render_rows(&rows, selection),
            Some("alpha\nold".to_owned())
        );
    }

    #[test]
    fn diff_side_selection_text_uses_utf8_char_columns() {
        let rows = vec![DiffRenderRow::SideBySide {
            old: DiffCell {
                line_label: "-1",
                content: "abc",
                kind: DiffLineKind::Removed,
            },
            new: DiffCell {
                line_label: "+1",
                content: "あいう",
                kind: DiffLineKind::Added,
            },
        }];
        let selection = DiffSideSelection {
            side: DiffSide::New,
            anchor: DiffSideSelectionPoint { row: 0, column: 1 },
            head: DiffSideSelectionPoint { row: 0, column: 3 },
            dragging: false,
        };

        assert_eq!(
            diff_side_selection_text_from_render_rows(&rows, selection),
            Some("いう".to_owned())
        );
    }

    #[test]
    fn diff_side_selection_text_skips_side_by_side_placeholder_cells() {
        let rows = vec![
            DiffRenderRow::SideBySide {
                old: DiffCell {
                    line_label: "10",
                    content: "before",
                    kind: DiffLineKind::Context,
                },
                new: DiffCell {
                    line_label: "10",
                    content: "before",
                    kind: DiffLineKind::Context,
                },
            },
            DiffRenderRow::SideBySide {
                old: DiffCell {
                    line_label: "",
                    content: "",
                    kind: DiffLineKind::Context,
                },
                new: DiffCell {
                    line_label: "+11",
                    content: "inserted",
                    kind: DiffLineKind::Added,
                },
            },
            DiffRenderRow::SideBySide {
                old: DiffCell {
                    line_label: "11",
                    content: "after",
                    kind: DiffLineKind::Context,
                },
                new: DiffCell {
                    line_label: "12",
                    content: "after",
                    kind: DiffLineKind::Context,
                },
            },
        ];
        let selection = DiffSideSelection {
            side: DiffSide::Old,
            anchor: DiffSideSelectionPoint { row: 0, column: 0 },
            head: DiffSideSelectionPoint { row: 2, column: 5 },
            dragging: false,
        };

        assert_eq!(
            diff_side_selection_text_from_render_rows(&rows, selection),
            Some("before\nafter".to_owned())
        );
    }

    #[test]
    fn diff_unified_selection_text_copies_visible_lines() {
        let rows = vec![
            DiffRenderRow::UnifiedLine {
                line_label: "10",
                marker: "",
                content: "context",
                kind: DiffLineKind::Context,
            },
            DiffRenderRow::UnifiedLine {
                line_label: "+11",
                marker: "+",
                content: "added",
                kind: DiffLineKind::Added,
            },
        ];
        let selection = DiffUnifiedSelection {
            anchor: DiffSideSelectionPoint { row: 0, column: 2 },
            head: DiffSideSelectionPoint { row: 1, column: 4 },
            dragging: false,
        };

        assert_eq!(
            diff_unified_selection_text_from_render_rows(&rows, selection),
            Some("context\n+ ad".to_owned())
        );
    }

    #[test]
    fn git_file_diff_args_request_changes_context() {
        assert_eq!(
            git_file_diff_args(
                "test.md",
                GitDiffScope::Worktree,
                GitDiffContextMode::Changes
            ),
            vec!["diff", "--", "test.md"]
        );
        assert_eq!(
            git_file_diff_args("test.md", GitDiffScope::Staged, GitDiffContextMode::Changes),
            vec!["diff", "--staged", "--", "test.md"]
        );
    }

    #[test]
    fn git_file_diff_args_request_full_file_context() {
        assert_eq!(
            git_file_diff_args(
                "test.md",
                GitDiffScope::Worktree,
                GitDiffContextMode::FullFile
            ),
            vec!["diff", "-U999999", "--", "test.md"]
        );
        assert_eq!(
            git_file_diff_args(
                "test.md",
                GitDiffScope::Staged,
                GitDiffContextMode::FullFile
            ),
            vec!["diff", "--staged", "-U999999", "--", "test.md"]
        );
    }

    #[test]
    fn workspace_search_display_path_prefers_shortest_root_relative_path() {
        let path = PathBuf::from(r"C:\git\code-reader\src\ui\app_view.rs");
        let roots = vec![
            PathBuf::from(r"C:\git"),
            PathBuf::from(r"C:\git\code-reader"),
        ];

        assert_eq!(
            workspace_search_display_path(&path, &roots),
            PathBuf::from(r"src\ui\app_view.rs").display().to_string()
        );
    }

    #[test]
    fn byte_index_to_position_uses_line_and_byte_column() {
        let source = "abc\nあいう\nxyz";
        let byte_index = source.find("う").unwrap();

        assert_eq!(
            byte_index_to_position(source, byte_index),
            Some(Position::new(1, 6))
        );
    }

    #[test]
    fn chunked_text_buffer_preserves_line_index() {
        let source = format!(
            "{}\n{}\n{}",
            "a".repeat(EDITOR_TEXT_CHUNK_TARGET_BYTES / 2),
            "b".repeat(EDITOR_TEXT_CHUNK_TARGET_BYTES / 2),
            "終"
        );
        let buffer = EditorTextBuffer::new(source.clone());
        let line_index = buffer.line_index();

        assert!(buffer.chunks.len() >= 2);
        assert_eq!(line_index.line_count(), 3);
        let second_line = line_index.line(1).unwrap();
        assert_eq!(second_line.leading_end, second_line.start);
        assert!(!second_line.is_blank);
        assert_eq!(
            buffer.line_text(line_index.line(0).unwrap()),
            Some(source.lines().next().unwrap())
        );
        assert_eq!(buffer.line_text(line_index.line(2).unwrap()), Some("終"));
    }

    #[test]
    fn line_index_tracks_leading_indent_and_blank_lines() {
        let source = "fn main() {\n    value\n\t  \n}\n";
        let line_index = build_line_index(source, source.len(), stable_hash(source));

        let indented_line = line_index.line(1).unwrap();
        assert_eq!(indented_line.leading_end - indented_line.start, 4);
        assert!(!indented_line.is_blank);

        let blank_line = line_index.line(2).unwrap();
        assert_eq!(blank_line.leading_end, blank_line.content_end);
        assert!(blank_line.is_blank);
    }

    #[test]
    fn highlight_line_span_ranges_map_spans_to_lines() {
        let source = "fn main() {\n    value\n}\n";
        let line_index = build_line_index(source, source.len(), stable_hash(source));
        let value_start = source.find("value").unwrap();
        let spans = vec![
            HighlightSpan::new(0..2, HighlightKind::Keyword),
            HighlightSpan::new(
                value_start..value_start + "value".len(),
                HighlightKind::Function,
            ),
        ];

        let ranges = build_highlight_line_span_ranges(&spans, &line_index);

        assert_eq!(ranges[0], 0..1);
        assert_eq!(ranges[1], 1..2);
        assert_eq!(ranges[2], 2..2);
    }

    #[test]
    fn immediate_wheel_scroll_delta_converts_line_and_page_units() {
        let events = vec![
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, 2.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Page,
                delta: egui::vec2(1.0, 0.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::default(),
            },
        ];

        assert_eq!(
            immediate_wheel_scroll_delta(&events, 300.0),
            Some(egui::vec2(300.0, 80.0))
        );
    }

    #[test]
    fn immediate_wheel_scroll_delta_ignores_zoom_wheel_events() {
        let events = vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, 120.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers {
                command: true,
                ..Default::default()
            },
        }];

        assert_eq!(immediate_wheel_scroll_delta(&events, 300.0), None);
    }

    #[test]
    fn chunked_text_buffer_converts_positions_across_chunks() {
        let first = "a".repeat(EDITOR_TEXT_CHUNK_TARGET_BYTES / 2);
        let second = "β".repeat(EDITOR_TEXT_CHUNK_TARGET_BYTES / 2);
        let source = format!("{first}\n{second}\nlast");
        let buffer = EditorTextBuffer::new(source);

        let position = Position::new(1, 4);
        let char_index = buffer.position_to_char_index(position).unwrap();
        let byte_index = buffer.char_index_to_byte_index(char_index).unwrap();

        assert_eq!(buffer.char_index_to_position(char_index), Some(position));
        assert_eq!(buffer.byte_index_to_position(byte_index), Some(position));
        assert_eq!(buffer.next_char_at(char_index), Some('β'));
    }

    #[test]
    fn recent_workspace_age_label_uses_compact_units() {
        assert_eq!(recent_workspace_age_label_at(1_000, 1_030), "just now");
        assert_eq!(recent_workspace_age_label_at(1_000, 1_180), "3m ago");
        assert_eq!(recent_workspace_age_label_at(1_000, 8_200), "2h ago");
        assert_eq!(recent_workspace_age_label_at(1_000, 260_200), "3d ago");
    }

    #[test]
    fn find_matching_bracket_pair_finds_pair_next_to_cursor() {
        let source = "call(value[0])";
        let position = char_index_to_position(source, 5).unwrap();
        let pair = find_matching_bracket_pair(source, position).unwrap();

        assert_eq!(&source[pair.0..=pair.0], "(");
        assert_eq!(&source[pair.1..=pair.1], ")");
    }

    #[test]
    fn markdown_source_spans_detect_common_constructs() {
        let source = "# Title\n\n> quote\n\n- item with **bold** and *em*\n\n[link](next.md) and `code`\n\n```rust\nfn main() {}\n```\n";
        let spans = markdown_source_spans(source);

        assert!(
            spans
                .iter()
                .any(|span| span.kind == MarkdownSourceSpanKind::Heading)
        );
        assert!(
            spans
                .iter()
                .any(|span| span.kind == MarkdownSourceSpanKind::Blockquote)
        );
        assert!(
            spans
                .iter()
                .any(|span| span.kind == MarkdownSourceSpanKind::ListMarker)
        );
        assert!(
            spans
                .iter()
                .any(|span| span.kind == MarkdownSourceSpanKind::Strong)
        );
        assert!(
            spans
                .iter()
                .any(|span| span.kind == MarkdownSourceSpanKind::Emphasis)
        );
        assert!(
            spans
                .iter()
                .any(|span| span.kind == MarkdownSourceSpanKind::Link)
        );
        assert!(
            spans
                .iter()
                .any(|span| span.kind == MarkdownSourceSpanKind::InlineCode)
        );
        assert!(
            spans
                .iter()
                .any(|span| span.kind == MarkdownSourceSpanKind::CodeBlock)
        );
    }

    #[test]
    fn markdown_active_source_line_range_uses_active_page_boundaries() {
        let document = MarkdownDocument::parse(
            "guide.md".into(),
            "# First\n\nText\n\n## Second\n\nMore\n\n## Third\n\nEnd\n".to_owned(),
        )
        .unwrap();
        let mut editor = EditorPane::empty(Pane::Left);
        editor.markdown_active_page_id = Some("second".to_owned());

        assert_eq!(
            editor.markdown_active_source_line_range(&document),
            Some((4, 7))
        );

        editor.prepare_markdown_source_entry(&document);
        assert_eq!(
            editor.pending_navigation,
            Some(PendingEditorNavigation {
                position: Position::new(4, 0),
                focus_editor: true,
            })
        );
    }

    #[test]
    fn cached_markdown_document_reuses_until_source_changes() {
        let mut editor = EditorPane::empty(Pane::Left);
        let path = PathBuf::from("guide.md");
        editor.replace_text("# First\n\nText\n".to_owned());

        let first = editor.cached_markdown_document(&path).unwrap();
        let second = editor.cached_markdown_document(&path).unwrap();

        assert!(Rc::ptr_eq(&first, &second));

        editor
            .text
            .insert_str(editor.text.len(), "\n## Second\n\nMore\n");
        editor.mark_text_changed();
        let third = editor.cached_markdown_document(&path).unwrap();

        assert!(!Rc::ptr_eq(&first, &third));
        assert_eq!(third.pages.len(), 2);
    }

    #[test]
    fn markdown_search_matches_rendered_text_across_pages() {
        let document = MarkdownDocument::parse(
            "guide.md".into(),
            "# First\n\nFind **needle** here.\n\n## Second\n\n`needle` again.\n".to_owned(),
        )
        .unwrap();

        let matches = markdown_search_matches(&document, "needle");

        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].page_id, "first");
        assert_eq!(matches[1].page_id, "second");
        assert!(markdown_search_matches(&document, "**needle**").is_empty());
    }

    #[test]
    fn markdown_node_ui_id_stays_stable_across_reparses() {
        let source = "| A | B |\n| - | - |\n| 1 | 2 |\n\n```rust\nfn main() {}\n```\n";
        let ast_a = markdown::to_mdast(source, &ParseOptions::gfm()).unwrap();
        let ast_b = markdown::to_mdast(source, &ParseOptions::gfm()).unwrap();
        let children_a = ast_a.children().unwrap();
        let children_b = ast_b.children().unwrap();

        assert!(matches!(children_a.first(), Some(Node::Table(_))));
        assert!(matches!(children_a.get(1), Some(Node::Code(_))));
        assert_eq!(
            markdown_node_ui_id(120, &children_a[0], "table"),
            markdown_node_ui_id(120, &children_b[0], "table")
        );
        assert_eq!(
            markdown_node_ui_id(120, &children_a[1], "code"),
            markdown_node_ui_id(120, &children_b[1], "code")
        );
    }

    #[test]
    fn markdown_mermaid_renderer_outputs_svg_for_flowchart() {
        let svg = render_mermaid_svg("graph TD\n    A[Start] --> B[End]").unwrap();

        assert!(svg.contains("<svg"));
        assert!(svg.contains("Start"));
    }

    #[test]
    fn markdown_footnotes_are_numbered_and_scoped_to_page() {
        let document = MarkdownDocument::parse(
            "guide.md".into(),
            "# First\n\nText[^a].\n\n[^a]: First note.\n\n## Second\n\nText[^b].\n\n[^b]: Second note.\n"
                .to_owned(),
        )
        .unwrap();

        let first = document.page_by_id("first").unwrap();
        let second = document.page_by_id("second").unwrap();
        let first_definitions = markdown_footnote_definitions_for_page(&document, first);
        let second_definitions = markdown_footnote_definitions_for_page(&document, second);

        assert_eq!(markdown_footnote_number(&document, "a"), Some(1));
        assert_eq!(markdown_footnote_number(&document, "b"), Some(2));
        assert_eq!(first_definitions.len(), 1);
        assert_eq!(first_definitions[0].identifier, "a");
        assert_eq!(second_definitions.len(), 1);
        assert_eq!(second_definitions[0].identifier, "b");
    }

    #[test]
    fn markdown_link_target_splits_path_and_decodes_fragment() {
        let (path, fragment) = split_markdown_link_target("docs/My%20Page.md#Preview%20Mode");

        assert_eq!(path, "docs/My Page.md");
        assert_eq!(fragment.as_deref(), Some("Preview Mode"));
    }

    #[test]
    fn markdown_fragment_target_matches_page_and_heading_slugs() {
        let document = MarkdownDocument::parse(
            "guide.md".into(),
            "# Introduction\n\nText\n\n## Mode Toggle\n\n### Preview Mode\n\nBody\n".to_owned(),
        )
        .unwrap();

        let page = markdown_fragment_target(&document, "mode-toggle").unwrap();
        assert_eq!(page.page_id, "mode-toggle");
        assert_eq!(page.heading_id, None);
        assert_eq!(page.position, Position::new(4, 0));

        let heading = markdown_fragment_target(&document, "preview-mode").unwrap();
        assert_eq!(heading.page_id, "mode-toggle");
        assert_eq!(heading.heading_id.as_deref(), Some("h3-preview-mode"));
        assert_eq!(heading.position, Position::new(6, 0));
    }

    #[test]
    fn markdown_fragment_target_matches_non_ascii_heading() {
        let document =
            MarkdownDocument::parse("guide.md".into(), "# はじめに\n\n### 詳細\n".to_owned())
                .unwrap();

        let target = markdown_fragment_target(&document, "詳細").unwrap();

        assert_eq!(target.page_id, "u306fu3058u3081u306b");
        assert_eq!(target.heading_id.as_deref(), Some("h3-u8a73u7d30"));
    }

    #[test]
    fn parse_csv_supports_quotes_commas_and_newlines() {
        let document =
            parse_csv("name,note\nAlice,\"hello, world\"\nBob,\"line 1\nline 2\"").unwrap();

        assert_eq!(
            document.rows,
            vec![
                vec!["name".to_owned(), "note".to_owned()],
                vec!["Alice".to_owned(), "hello, world".to_owned()],
                vec!["Bob".to_owned(), "line 1\nline 2".to_owned()],
            ]
        );
    }

    #[test]
    fn serialize_csv_quotes_only_when_needed() {
        let document = CsvDocument {
            rows: vec![vec![
                "plain".to_owned(),
                "a,b".to_owned(),
                "with \"quote\"".to_owned(),
                " tail ".to_owned(),
            ]],
        };

        assert_eq!(
            serialize_csv(&document, CsvTextStyle::of("")),
            "plain,\"a,b\",\"with \"\"quote\"\"\",\" tail \""
        );
    }

    #[test]
    fn ensure_cell_expands_rows_and_columns() {
        let mut document = CsvDocument {
            rows: vec![vec!["a".to_owned()]],
        };

        document.ensure_cell(1, 2);

        assert_eq!(
            document.rows,
            vec![
                vec!["a".to_owned(), String::new(), String::new()],
                vec![String::new(), String::new(), String::new()],
            ]
        );
    }

    #[test]
    fn serialize_csv_for_save_removes_trailing_empty_rows() {
        let document = CsvDocument {
            rows: vec![
                vec!["a".to_owned(), "b".to_owned()],
                vec![String::new(), String::new()],
                vec![String::new(), String::new()],
            ],
        };

        assert_eq!(serialize_csv(&document, CsvTextStyle::of("")), "a,b\n,\n,");
        assert_eq!(
            serialize_csv_for_save(&document, CsvTextStyle::of("")),
            "a,b"
        );
    }

    #[test]
    fn consume_wheel_lead_skips_only_what_was_applied_ahead() {
        let mut lead = egui::vec2(0.0, -60.0);
        assert_eq!(
            consume_wheel_lead(&mut lead, egui::vec2(0.0, -40.0)),
            egui::Vec2::ZERO
        );
        assert_eq!(lead, egui::vec2(0.0, -20.0));
        assert_eq!(
            consume_wheel_lead(&mut lead, egui::Vec2::ZERO),
            egui::Vec2::ZERO
        );
        assert_eq!(lead, egui::vec2(0.0, -20.0), "an idle frame keeps the lead");
        assert_eq!(
            consume_wheel_lead(&mut lead, egui::vec2(0.0, -30.0)),
            egui::vec2(0.0, -10.0)
        );
        assert_eq!(lead, egui::Vec2::ZERO);

        // Scrolling the other way drops the lead.
        let mut lead = egui::vec2(0.0, -60.0);
        assert_eq!(
            consume_wheel_lead(&mut lead, egui::vec2(0.0, 10.0)),
            egui::vec2(0.0, 10.0)
        );
        assert_eq!(lead, egui::Vec2::ZERO);
    }

    #[test]
    fn serialize_csv_keeps_line_endings_and_final_newline() {
        let document = parse_csv("a,b\r\n1,2\r\n").expect("parses");
        let style = CsvTextStyle::of("a,b\r\n1,2\r\n");
        assert_eq!(serialize_csv(&document, style), "a,b\r\n1,2\r\n");
        assert_eq!(serialize_csv_for_save(&document, style), "a,b\r\n1,2\r\n");
        let empty = CsvDocument { rows: Vec::new() };
        assert_eq!(serialize_csv(&empty, style), "");
    }

    #[test]
    fn copy_csv_column_selection_keeps_csv_format() {
        let document = CsvDocument {
            rows: vec![
                vec!["A".to_owned(), "B".to_owned(), "C".to_owned()],
                vec!["1".to_owned(), "2,2".to_owned(), "3".to_owned()],
                vec!["4".to_owned(), "5".to_owned(), "6".to_owned()],
            ],
        };

        assert_eq!(
            copy_csv_selection_as_csv(&document, CsvSelection::Columns { start: 1, end: 2 }),
            "B,C\n\"2,2\",3\n5,6"
        );
    }

    #[test]
    fn copy_csv_row_selection_keeps_csv_format() {
        let document = CsvDocument {
            rows: vec![
                vec!["A".to_owned(), "B".to_owned()],
                vec!["hello".to_owned(), "with \"quote\"".to_owned()],
                vec!["tail".to_owned(), "row".to_owned()],
            ],
        };

        assert_eq!(
            copy_csv_selection_as_csv(&document, CsvSelection::Rows { start: 1, end: 2 }),
            "hello,\"with \"\"quote\"\"\"\ntail,row"
        );
    }

    #[test]
    fn paste_csv_column_selection_writes_from_selected_column() {
        let mut document = CsvDocument {
            rows: vec![
                vec![
                    "a".to_owned(),
                    "b".to_owned(),
                    "c".to_owned(),
                    "d1".to_owned(),
                ],
                vec![
                    "aaaa".to_owned(),
                    "bbb".to_owned(),
                    "ccc".to_owned(),
                    "d2".to_owned(),
                ],
            ],
        };

        let cell_count = paste_csv_selection_from_text(
            &mut document,
            CsvSelection::Columns { start: 4, end: 4 },
            "d1\nd2",
        )
        .unwrap();

        assert_eq!(cell_count, 2);
        assert_eq!(document.rows[0][4], "d1");
        assert_eq!(document.rows[1][4], "d2");
    }

    #[test]
    fn paste_csv_row_selection_writes_from_first_column() {
        let mut document = CsvDocument {
            rows: vec![vec!["a".to_owned(), "b".to_owned()]],
        };

        let cell_count = paste_csv_selection_from_text(
            &mut document,
            CsvSelection::Rows { start: 1, end: 1 },
            "x,y",
        )
        .unwrap();

        assert_eq!(cell_count, 2);
        assert_eq!(
            document.rows,
            vec![
                vec!["a".to_owned(), "b".to_owned()],
                vec!["x".to_owned(), "y".to_owned()],
            ]
        );
    }

    #[test]
    fn delete_csv_row_selection_removes_selected_rows() {
        let mut document = CsvDocument {
            rows: vec![
                vec!["h1".to_owned(), "h2".to_owned()],
                vec!["a".to_owned(), "b".to_owned()],
                vec!["c".to_owned(), "d".to_owned()],
                vec!["tail".to_owned(), "row".to_owned()],
            ],
        };

        let (selection, label) =
            delete_csv_selection(&mut document, CsvSelection::Rows { start: 1, end: 2 });

        assert_eq!(label, "rows 2-3");
        assert_eq!(selection, CsvSelection::Rows { start: 1, end: 1 });
        assert_eq!(
            document.rows,
            vec![
                vec!["h1".to_owned(), "h2".to_owned()],
                vec!["tail".to_owned(), "row".to_owned()],
            ]
        );
    }

    #[test]
    fn delete_csv_column_selection_removes_selected_columns() {
        let mut document = CsvDocument {
            rows: vec![
                vec![
                    "A".to_owned(),
                    "B".to_owned(),
                    "C".to_owned(),
                    "D".to_owned(),
                ],
                vec![
                    "1".to_owned(),
                    "2".to_owned(),
                    "3".to_owned(),
                    "4".to_owned(),
                ],
            ],
        };

        let (selection, label) =
            delete_csv_selection(&mut document, CsvSelection::Columns { start: 1, end: 2 });

        assert_eq!(label, "columns B-C");
        assert_eq!(selection, CsvSelection::Columns { start: 1, end: 1 });
        assert_eq!(
            document.rows,
            vec![
                vec!["A".to_owned(), "D".to_owned()],
                vec!["1".to_owned(), "4".to_owned()],
            ]
        );
    }

    #[test]
    fn delete_csv_all_rows_keeps_one_empty_cell() {
        let mut document = CsvDocument {
            rows: vec![vec!["a".to_owned()], vec!["b".to_owned()]],
        };

        let (selection, label) =
            delete_csv_selection(&mut document, CsvSelection::Rows { start: 0, end: 1 });

        assert_eq!(label, "rows 1-2");
        assert_eq!(selection, CsvSelection::Rows { start: 0, end: 0 });
        assert_eq!(document.rows, vec![vec![String::new()]]);
    }

    #[test]
    fn delete_csv_all_columns_keeps_one_empty_column() {
        let mut document = CsvDocument {
            rows: vec![
                vec!["a".to_owned(), "b".to_owned()],
                vec!["c".to_owned(), "d".to_owned()],
            ],
        };

        let (selection, label) =
            delete_csv_selection(&mut document, CsvSelection::Columns { start: 0, end: 1 });

        assert_eq!(label, "columns A-B");
        assert_eq!(selection, CsvSelection::Columns { start: 0, end: 0 });
        assert_eq!(
            document.rows,
            vec![vec![String::new()], vec![String::new()]]
        );
    }

    #[test]
    fn csv_undo_redo_restores_paste() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.path = Some(PathBuf::from("data.csv"));
        editor.read_only = false;
        editor.text = EditorTextBuffer::new("a,b\nc,d".to_owned());
        editor.csv_mode = CsvMode::Grid;
        editor.csv_selection = Some(CsvSelection::Columns { start: 1, end: 1 });

        let cell_count = editor.paste_csv_text("x\ny").unwrap();

        assert_eq!(cell_count, 2);
        assert_eq!(editor.text.as_str(), "a,x\nc,y");
        assert!(editor.apply_csv_undo());
        assert_eq!(editor.text.as_str(), "a,b\nc,d");
        assert!(editor.apply_csv_redo());
        assert_eq!(editor.text.as_str(), "a,x\nc,y");
    }

    #[test]
    fn csv_undo_redo_restores_delete_selection() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.path = Some(PathBuf::from("data.csv"));
        editor.read_only = false;
        editor.text = EditorTextBuffer::new("a,b\nc,d\ne,f".to_owned());
        editor.csv_mode = CsvMode::Grid;
        editor.csv_selection = Some(CsvSelection::Rows { start: 1, end: 1 });

        let label = editor.delete_csv_selection().unwrap();

        assert_eq!(label, "row 2");
        assert_eq!(editor.text.as_str(), "a,b\ne,f");
        assert!(editor.apply_csv_undo());
        assert_eq!(editor.text.as_str(), "a,b\nc,d\ne,f");
        assert!(editor.apply_csv_redo());
        assert_eq!(editor.text.as_str(), "a,b\ne,f");
    }
}
