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
use fonts::*;
use source_editor::*;
use virtual_editor::*;
use virtual_input::*;

const DEFAULT_TAB_WIDTH: usize = 4;
const MARKDOWN_SIDE_PANE_DEFAULT_WIDTH: f32 = 180.0;
const MARKDOWN_SIDE_PANE_MIN_WIDTH: f32 = 120.0;
const MARKDOWN_SIDE_PANE_MAX_WIDTH: f32 = 560.0;
const MARKDOWN_SIDE_PANE_MIN_REMAINING_WIDTH: f32 = 360.0;
const NAVIGATION_BREADCRUMB_LIMIT: usize = 5;
const WORKSPACE_SEARCH_MAX_MATCHES: usize = 2000;
const WORKSPACE_SEARCH_PREVIEW_CHARS: usize = 140;
const WORKSPACE_SECTION_HEADER_HEIGHT: f32 = 24.0;
const WORKSPACE_FOOTER_BODY_ALLOWANCE: f32 = 64.0;
const WORKSPACE_FILE_TREE_MIN_HEIGHT: f32 = 120.0;
const EDITOR_PAGE_SCROLL_RATIO: f32 = 0.85;
const VIRTUAL_SOURCE_MIN_BYTES: usize = 512 * 1024;
const VIRTUAL_SOURCE_MIN_LINES: usize = 10_000;
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

#[derive(Debug, Clone, Default)]
struct MarkdownSearchState {
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

impl WorkspaceGitStatus {
    fn color(self) -> Color32 {
        match self {
            Self::Modified => Color32::from_rgb(196, 160, 72),
            Self::Added => Color32::from_rgb(72, 160, 92),
            Self::Untracked => Color32::from_rgb(86, 156, 214),
        }
    }
}

impl LineIndex {
    fn editor_width(&self, ui: &egui::Ui, available_width: f32) -> f32 {
        let char_width = editor_char_width(ui);
        (self.longest_line_chars as f32 * char_width + 64.0).max(available_width)
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

    fn is_markdown(&self) -> bool {
        self.path.as_deref().is_some_and(is_markdown_path)
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

        if path.as_deref().is_some_and(is_external_rust_path)
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
        self.markdown_side_pane_title_ui(ui, "Pages", MarkdownFocusTarget::Pages);
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
                let body_id = self.markdown_focus_id(MarkdownFocusTarget::Body);
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
            if selected || self.should_scroll_to_markdown_search_match(search_query, &heading.title)
            {
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
        // A fragment is parsed on its own, so add the definitions it does not contain;
        // otherwise its reference links and footnote markers stay literal text.
        let definitions = markdown_definition_sources_outside(
            document,
            fragment_start_offset..fragment_start_offset + source.len(),
        );
        let source_with_definitions;
        let parse_source = if definitions.is_empty() {
            source
        } else {
            source_with_definitions = format!("{source}\n\n{definitions}");
            source_with_definitions.as_str()
        };
        let Ok(ast) = markdown::to_mdast(parse_source, &ParseOptions::gfm()) else {
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
                        match markdown_definition_url(document, &link.identifier) {
                            Some(url) => self.activate_markdown_link(document, &url),
                            None => {
                                self.pending_action = Some(EditorAction::ShowStatus(format!(
                                    "Reference link has no definition: {}",
                                    link.identifier
                                )));
                            }
                        }
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
        self.markdown_side_pane_title_ui(ui, "Outline", MarkdownFocusTarget::Outline);
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
                let mut state = TextEditState::load(ui.ctx(), text_edit_id).unwrap_or_default();
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

        // Shift first: consume_key(NONE, ..) ignores an extra Shift.
        let (previous_pane, next_pane) = ui.input_mut(|input| {
            (
                input.consume_key(egui::Modifiers::SHIFT, egui::Key::F6),
                input.consume_key(egui::Modifiers::NONE, egui::Key::F6),
            )
        });
        if next_pane || previous_pane {
            self.cycle_markdown_focus_target(ui, next_pane);
            ui.ctx().request_repaint();
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

    /// Independent of the calling `ui`, so a request from the page header or a shortcut
    /// handler focuses the same widget the body registers with `ui.interact`.
    fn markdown_focus_id(&self, target: MarkdownFocusTarget) -> egui::Id {
        egui::Id::new(("markdown_focus", self.pane, target))
    }

    fn request_markdown_focus(&mut self, ui: &mut egui::Ui, target: MarkdownFocusTarget) {
        self.markdown_focus_target = target;
        if target != MarkdownFocusTarget::Body {
            // Pages / Outline keys work from `markdown_focus_target` alone. Nothing holds egui
            // focus meanwhile, so egui does not turn their Up/Down into focus moves.
            ui.memory_mut(|memory| {
                if let Some(focused) = memory.focused() {
                    memory.surrender_focus(focused);
                }
            });
            return;
        }
        let focus_id = self.markdown_focus_id(target);
        ui.memory_mut(|memory| memory.request_focus(focus_id));
    }

    /// Title of the Pages / Outline pane; clicking it gives that pane the keyboard.
    fn markdown_side_pane_title_ui(
        &mut self,
        ui: &mut egui::Ui,
        label: &'static str,
        target: MarkdownFocusTarget,
    ) {
        let title = egui::Label::new(self.markdown_focus_label(ui, label, target))
            .sense(egui::Sense::click());
        if ui.add(title).clicked() {
            self.request_markdown_focus(ui, target);
        }
    }

    /// F6 / Shift+F6 move the keyboard between the visible Pages, Body and Outline panes.
    fn cycle_markdown_focus_target(&mut self, ui: &mut egui::Ui, forward: bool) {
        let targets = self.visible_markdown_focus_targets();
        let index = targets
            .iter()
            .position(|target| *target == self.markdown_focus_target)
            .unwrap_or(0);
        let next = if forward {
            (index + 1) % targets.len()
        } else {
            (index + targets.len() - 1) % targets.len()
        };
        self.request_markdown_focus(ui, targets[next]);
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

#[derive(Debug, Clone, Copy, Default)]
struct MarkdownInlineStyle {
    emphasis: bool,
    strong: bool,
    link: bool,
    delete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
        .clamp(MARKDOWN_SIDE_PANE_MIN_WIDTH, MARKDOWN_SIDE_PANE_MAX_WIDTH);
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

fn copy_code_block_to_clipboard(ui: &egui::Ui, code: &str) {
    ui.ctx().copy_text(code.to_owned());
    let _ = copy_text_to_system_clipboard(code);
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

fn editor_char_width(ui: &egui::Ui) -> f32 {
    ui.fonts_mut(|fonts| fonts.glyph_width(&editor_font_id(), ' '))
}

fn should_use_virtual_source_view(source_len: usize, line_count: usize) -> bool {
    source_len >= VIRTUAL_SOURCE_MIN_BYTES || line_count >= VIRTUAL_SOURCE_MIN_LINES
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
#[cfg(test)]
mod event_tests;
mod fonts;
mod git_actions;
mod git_panel;
mod git_windows;
mod lsp;
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
