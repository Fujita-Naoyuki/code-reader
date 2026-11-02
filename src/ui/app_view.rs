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
use diff_view::*;
use editor_debug::*;
use editor_pane::*;
use editor_style::*;
use fonts::*;
use git_history_view::*;
use lsp::*;
use markdown_render::*;
use markdown_view::*;
use search::*;
use source_editor::*;
use virtual_editor::*;
use virtual_input::*;
use workspace::*;

const NAVIGATION_BREADCRUMB_LIMIT: usize = 5;
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

impl EditorPane {}

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

mod ai_command;
mod clipboard;
mod csv_grid;
mod diagnostics;
mod diff_view;
mod editor_debug;
mod editor_pane;
mod editor_style;
#[cfg(test)]
mod event_tests;
mod fonts;
mod git_actions;
mod git_history_view;
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
