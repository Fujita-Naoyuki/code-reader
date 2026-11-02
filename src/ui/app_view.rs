use std::{
    collections::HashMap,
    fs,
    hash::Hash,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

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

use crate::core::csv::{
    CsvCellPosition, CsvDocument, CsvSelection, CsvSelectionKind, CsvTextStyle,
    copy_csv_selection_as_csv, csv_cell_display_text, csv_column_label, csv_selection_label,
    delete_csv_selection, parse_csv, paste_csv_selection_from_text, serialize_csv,
    serialize_csv_for_save,
};
use crate::core::diff::{
    DiffDisplayMode, DiffDisplayRow, DiffLineKind, DiffMarkerKind, DiffRenderRow,
    DiffRenderSummary, DiffSide, DiffSideSelection, DiffSideSelectionPoint, DiffUnifiedSelection,
    diff_overview_markers, diff_render_rows, diff_row_char_width, diff_side_background_kind,
    diff_side_selection_columns_for_cell, diff_side_selection_text_from_render_rows,
    diff_unified_background_kind, diff_unified_selection_columns_for_line,
    diff_unified_selection_text_from_render_rows, hidden_lines_label, parse_diff_display_rows,
    side_by_side_diff_render_rows, unified_diff_render_rows, unified_diff_visible_text,
};
use crate::core::git::{
    GitChange, GitCommitFileChange, GitCommitNode, GitDiffContextMode, GitDiffScope, GitGraphRow,
    GitRepositoryState, WorkspaceGitStatus, discover_git_repositories, git_all_diff_text,
    git_command_output, git_commit_changed_files, git_commit_diff_text, git_file_diff_text,
    git_file_diff_text_for_path, git_history_nodes, push_or_publish_git_branch,
    workspace_git_file_statuses,
};
use crate::core::markdown::definitions::{
    markdown_definition_sources_outside, markdown_definition_url,
    markdown_footnote_definitions_for_page, markdown_footnote_number,
    markdown_page_body_start_offset,
};
use crate::core::markdown::links::{
    is_external_markdown_link, markdown_code_path_for_language, markdown_fragment_target,
    markdown_resolve_local_image_path, resolve_markdown_link_path, split_markdown_link_target,
};
use crate::core::markdown::mermaid::{is_mermaid_language, render_mermaid_svg};
use crate::core::markdown::source_spans::{MarkdownSourceSpanKind, markdown_source_spans};
use crate::core::navigation::{
    NavigationHistory, NavigationPoint, navigation_point_hover_text, navigation_point_label,
};
use crate::core::session::{
    load_state, recent_files_from_open_files, recent_workspace_age_label, save_state,
};
use crate::core::text::{
    EditorTextBuffer, LineIndex, LineInfo, char_index_to_byte_index, clamp_to_char_boundary,
    closing_pair, find_matching_bracket_pair, is_closing_bracket, leading_indent_columns,
    next_char_boundary, previous_char_boundary, smart_newline, stable_hash,
    strip_trailing_whitespace,
};
use crate::core::workspace_files::{
    WorkspaceSnapshot, file_name_stem_selection_char_len, file_title,
    initial_workspace_file_content, is_csv_path, is_external_path, is_external_rust_path,
    is_go_path, is_markdown_path, is_rust_path, validate_workspace_file_name,
    workspace_search_display_path, workspace_snapshot, workspace_snapshot_diff,
};
use crate::core::{
    cli_agent::{CliAgentCommand, build_cli_agent_command},
    document::Document,
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
};
use crate::features::search::{SearchResult, search_everywhere};
use crate::languages::LanguageRegistry;
use crate::platform;
use csv_grid::*;
use editor_debug::*;
use editor_style::*;
use fonts::*;
use markdown_render::*;
use markdown_view::*;
use source_editor::*;
use virtual_editor::*;
use virtual_input::*;

const DEFAULT_TAB_WIDTH: usize = 4;
const NAVIGATION_BREADCRUMB_LIMIT: usize = 5;
const WORKSPACE_SEARCH_MAX_MATCHES: usize = 2000;
const WORKSPACE_SEARCH_PREVIEW_CHARS: usize = 140;
const WORKSPACE_SECTION_HEADER_HEIGHT: f32 = 24.0;
const WORKSPACE_FOOTER_BODY_ALLOWANCE: f32 = 64.0;
const WORKSPACE_FILE_TREE_MIN_HEIGHT: f32 = 120.0;
const WORKSPACE_CHANGE_POLL_INTERVAL: Duration = Duration::from_secs(1);
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
#[allow(clippy::enum_variant_names)] // The actions all act on files.
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
    /// An action that would drop unsaved edits, waiting for Save / Discard / Cancel.
    pending_unsaved_action: Option<UnsavedChangesAction>,
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
        let mut app_state =
            load_state(&state_path).unwrap_or_else(|| AppState::new(fallback_root.clone()));
        if app_state.repair_current_session(fallback_root.clone()) {
            // The state file named a session that does not exist; keep the repair.
            let _ = save_state(&state_path, &app_state);
        }
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
            pending_unsaved_action: None,
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
        self.show_unsaved_changes_window(ui);
        self.show_git_commit_details_window(ui);
        self.show_shortcuts_help_window(ui);

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

#[derive(Debug)]
struct WorkspaceChangeScanResult {
    roots: Vec<PathBuf>,
    files: Vec<PathBuf>,
    snapshot: WorkspaceSnapshot,
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

/// What to do once the user has saved or discarded unsaved edits.
enum UnsavedChangesAction {
    ReplaceEditor {
        pane: Pane,
        next: Box<EditorPane>,
        sync_open_files: bool,
        status: Option<String>,
    },
    SwitchSession(String),
    CreateSession,
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

impl WorkspaceGitStatus {
    fn color(self) -> Color32 {
        match self {
            Self::Modified => Color32::from_rgb(196, 160, 72),
            Self::Added => Color32::from_rgb(72, 160, 92),
            Self::Untracked => Color32::from_rgb(86, 156, 214),
        }
    }
}

#[derive(Debug, Clone)]
struct DiffDisplayCache {
    source_len: usize,
    source_hash: u64,
    rows: Rc<Vec<DiffDisplayRow>>,
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
struct PendingEditorNavigation {
    position: Position,
    focus_editor: bool,
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
    /// Where Up/Down last left the cursor and the column it was aiming for, so passing
    /// through shorter lines does not lose the column (only while the cursor stays there).
    virtual_vertical_goal: Option<(Position, u32)>,
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
    /// Text fingerprint when the file was last read or saved; see `has_unsaved_changes`.
    saved_fingerprint: (usize, u64),
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
            virtual_vertical_goal: None,
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
            saved_fingerprint: (0, 0),
        }
    }

    fn from_path(pane: Pane, path: PathBuf) -> Self {
        let mut editor = Self::read_path(pane, path);
        editor.saved_fingerprint = editor.text_fingerprint();
        editor
    }

    fn read_path(pane: Pane, path: PathBuf) -> Self {
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
                virtual_vertical_goal: None,
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
                saved_fingerprint: (0, 0),
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
                    virtual_vertical_goal: None,
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
                    saved_fingerprint: (0, 0),
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
            virtual_vertical_goal: None,
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
            saved_fingerprint: (0, 0),
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
            virtual_vertical_goal: None,
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
            saved_fingerprint: (0, 0),
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

    /// An editable file whose text differs from what was last read or saved.
    fn has_unsaved_changes(&self) -> bool {
        self.path.is_some()
            && !self.read_only
            && self.diff_view.is_none()
            && self.git_history_view.is_none()
            && self.text_fingerprint() != self.saved_fingerprint
    }

    /// Takes over `other`'s unsaved text when `self` is a fresh copy of the same file, so
    /// moving within that file (definition jump, Back/Forward) keeps the edits.
    fn adopt_unsaved_text(&mut self, other: &mut EditorPane) {
        self.text = std::mem::replace(&mut other.text, EditorTextBuffer::new(String::new()));
        self.saved_fingerprint = other.saved_fingerprint;
        self.mark_text_changed();
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
            .map_err(|error| format!("Failed to save file: {error}"))?;
        self.saved_fingerprint = self.text_fingerprint();
        Ok(path)
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
                if self.has_unsaved_changes() {
                    ui.label(RichText::new(format!("● {}", self.title)).strong())
                        .on_hover_text("Unsaved changes");
                } else {
                    ui.label(RichText::new(&self.title).strong());
                }
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
enum BookmarkAction {
    Open(SessionBookmark),
    Remove(usize),
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
    for (row_index, row) in rows.iter().enumerate().take(end).skip(start) {
        let kind = diff_unified_background_kind(row);
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
    for (row_index, row) in rows.iter().enumerate().take(end).skip(start) {
        let kind = diff_side_background_kind(row, side);
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
    let marker_height = row_height.clamp(3.0, 8.0);
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

fn hidden_lines_row_ui(ui: &mut egui::Ui, rect: egui::Rect, count: usize) {
    if count == 0 {
        return;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, ui.visuals().faint_bg_color);
    painter.text(
        egui::pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        format!("◇  {}", hidden_lines_label(count)),
        FontId::monospace(11.0),
        ui.visuals().weak_text_color(),
    );
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

fn diff_line_color(ui: &egui::Ui, kind: DiffLineKind) -> Color32 {
    match kind {
        DiffLineKind::Added => Color32::from_rgb(72, 160, 92),
        DiffLineKind::Removed => Color32::from_rgb(206, 92, 92),
        DiffLineKind::Context => ui.visuals().text_color(),
    }
}

fn diff_line_background(kind: DiffLineKind) -> Color32 {
    match kind {
        DiffLineKind::Added => Color32::from_rgba_unmultiplied(72, 160, 92, 24),
        DiffLineKind::Removed => Color32::from_rgba_unmultiplied(206, 92, 92, 24),
        DiffLineKind::Context => Color32::TRANSPARENT,
    }
}

fn diff_row_background_rect(rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() - 0.5),
        egui::pos2(rect.right(), rect.bottom() + 0.5),
    )
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

fn pick_workspace_folder(title: &str) -> Option<PathBuf> {
    platform::current().pick_folder(title)
}

mod ai_command;
mod clipboard;
mod csv_grid;
mod diagnostics;
mod editor_debug;
mod editor_style;
#[cfg(test)]
mod event_tests;
mod fonts;
mod git_actions;
mod git_panel;
mod git_windows;
mod lsp;
mod markdown_render;
mod markdown_source;
mod markdown_view;
mod navigation;
mod panes;
mod search;
mod sessions;
mod shortcuts;
mod source_editor;
mod virtual_editor;
mod virtual_input;
mod workspace;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::text::build_line_index;

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
