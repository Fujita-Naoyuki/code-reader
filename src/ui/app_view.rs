//! The app: `CodeReaderApp`'s state, construction and frame layout. Everything else lives
//! in the child modules (see the map in CLAUDE.md); they read this module's items through
//! `use super::*`.

mod ai_command;
mod clipboard;
mod csv_grid;
mod diagnostics;
mod diff_selection;
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
mod keymap;
mod lsp;
mod markdown_render;
mod markdown_search;
mod markdown_source;
mod markdown_view;
mod modal;
mod navigation;
mod panes;
mod search;
mod sessions;
mod shortcuts;
mod source_editor;
mod state;
mod status;
mod virtual_editor;
mod virtual_input;
#[cfg(test)]
mod virtual_input_tests;
mod virtual_paint;
mod virtual_pointer;
mod workspace;

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
    DiffCell, DiffDisplayMode, DiffDisplayRow, DiffLineKind, DiffMarkerKind, DiffRenderRow,
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
use clipboard::*;
use csv_grid::*;
use diagnostics::*;
use diff_selection::*;
use diff_view::*;
use editor_debug::*;
use editor_pane::*;
use editor_style::*;
use fonts::*;
use git_history_view::*;
use git_windows::*;
use keymap::*;
use lsp::*;
use markdown_render::*;
use markdown_search::*;
use markdown_view::*;
use modal::*;
use navigation::*;
use panes::*;
use search::*;
use source_editor::*;
use state::*;
use status::*;
use virtual_editor::*;
use virtual_input::*;
use virtual_paint::*;
use virtual_pointer::*;
use workspace::*;

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
    markdown_modes: HashMap<PathBuf, MarkdownMode>,
    tab_width: usize,
    status_message: String,
    background_status: BackgroundStatus,
    layout: LayoutState,
    workspace: WorkspaceTreeState,
    search: SearchState,
    lsp: LspState,
    navigation: NavigationState,
    git: GitPanelState,
    session_edit: SessionEditState,
    shift_tap: ShiftTapState,
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
            markdown_modes: HashMap::new(),
            tab_width: DEFAULT_TAB_WIDTH,
            status_message: "Session restored.".to_owned(),
            background_status: BackgroundStatus::default(),
            layout: LayoutState::default(),
            workspace: WorkspaceTreeState::new(workspace_snapshot),
            search: SearchState::new(symbol_index_receiver),
            lsp: LspState::default(),
            navigation: NavigationState {
                recent_files,
                ..NavigationState::default()
            },
            git: GitPanelState::default(),
            session_edit: SessionEditState::default(),
            shift_tap: ShiftTapState::default(),
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
        let scroll_before_y = self.focused_editor().scroll.y;
        let input_sample = FrameInputSample::capture(ui);
        self.dispatch_modal_escape(ui);
        self.poll_workspace_changes(ui.ctx());
        self.poll_symbol_index();
        self.poll_workspace_search(ui.ctx());
        self.poll_lsp(LspLanguage::Go, ui.ctx());
        self.poll_lsp(LspLanguage::Rust, ui.ctx());

        self.handle_global_shortcuts(ui);
        self.top_bar_ui(ui);

        self.track_operation_status();
        let status_message = self.status_bar_message();
        let status_shortcuts = self.status_bar_shortcuts();
        status_bar_ui(ui, &status_message, &status_shortcuts);

        self.sidebar_ui(ui);
        self.editor_area_ui(ui);
        self.dialogs_ui(ui);

        let scroll_after_y = self.editor(focused_pane_at_start).scroll.y;
        self.record_app_perf_sample(
            elapsed_ms(app_frame_start),
            input_sample,
            focused_pane_at_start,
            scroll_before_y,
            scroll_after_y,
        );
    }
}

impl CodeReaderApp {
    /// Keys that work from anywhere. Runs before the panes so they see what is left.
    fn handle_global_shortcuts(&mut self, ui: &mut egui::Ui) {
        if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::F1)) {
            self.layout.shortcuts_help_open = true;
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
        if !self.search.overlay.open && ui.input_mut(consume_ai_command_shortcut) {
            self.copy_cli_agent_command();
        }
        self.handle_workspace_file_shortcuts(ui);
        if !self.search.overlay.open && ui.input_mut(consume_navigation_back_shortcut) {
            self.go_back();
        }
        if !self.search.overlay.open && ui.input_mut(consume_navigation_forward_shortcut) {
            self.go_forward();
        }

        if !self.search.overlay.open && self.focused_editor().is_csv_grid() {
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

        if !self.search.overlay.open
            && self.focused_editor().has_diff_text_selection()
            && ui.input_mut(consume_copy_shortcut)
        {
            self.copy_diff_selection_to_clipboard(ui);
        } else if !self.search.overlay.open
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
            !self.search.overlay.open && ui.input(|input| input.key_pressed(egui::Key::F12));
        if pressed_f12 {
            self.go_to_definition();
        }
        if ui.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::S)) {
            self.save_focused_file();
        }
        if !self.layout.show_right_pane && self.focused_pane == Pane::Right {
            self.focused_pane = Pane::Left;
        }
    }

    fn top_bar_ui(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top_bar").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(self.layout.show_workspace_sidebar, "Sidebar")
                    .on_hover_text("Show or hide the workspace sidebar")
                    .clicked()
                {
                    self.layout.show_workspace_sidebar = !self.layout.show_workspace_sidebar;
                }
                if ui
                    .selectable_label(self.layout.show_right_pane, "Right Pane")
                    .on_hover_text("Show or hide the right editor pane")
                    .clicked()
                {
                    self.layout.show_right_pane = !self.layout.show_right_pane;
                    if !self.layout.show_right_pane && self.focused_pane == Pane::Right {
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
                    self.layout.recent_workspaces_open = true;
                }
                if ui
                    .button("Everywhere")
                    .on_hover_text("Open Search Everywhere")
                    .clicked()
                {
                    self.layout.recent_workspaces_open = false;
                    self.open_search_overlay();
                }
                if ui.button("AI Cmd").clicked() {
                    self.layout.ai_command_open = true;
                }
                if ui.button("Help").on_hover_text("Show shortcuts").clicked() {
                    self.layout.shortcuts_help_open = true;
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
    }

    fn sidebar_ui(&mut self, ui: &mut egui::Ui) {
        if self.layout.show_workspace_sidebar {
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
                                .selectable_label(self.layout.sidebar_view == view, view.label())
                                .clicked()
                            {
                                self.layout.sidebar_view = view;
                                if view == SidebarView::Search {
                                    self.search.workspace.open = true;
                                    self.search.workspace.focus_requested = true;
                                } else if view == SidebarView::Git {
                                    self.refresh_git_view();
                                }
                            }
                        }
                    });
                    ui.separator();

                    match self.layout.sidebar_view {
                        SidebarView::Workspace => {
                            self.workspace_sidebar_ui(ui, &session_name);
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
    }

    fn workspace_sidebar_ui(&mut self, ui: &mut egui::Ui, session_name: &str) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(session_name).strong());
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
                self.workspace.sidebar_active = true;
                self.start_workspace_new_file();
            }
            let has_selected_file = self.selected_workspace_file().is_some();
            if ui
                .add_enabled(has_selected_file, egui::Button::new("Rename"))
                .on_hover_text("Rename selected file")
                .clicked()
            {
                self.workspace.sidebar_active = true;
                self.start_workspace_rename_file();
            }
            if ui
                .add_enabled(has_selected_file, egui::Button::new("Delete"))
                .on_hover_text("Delete selected file")
                .clicked()
            {
                self.workspace.sidebar_active = true;
                self.start_workspace_delete_file();
            }
        });

        let footer_header_count = 1;
        let file_tree_height =
            workspace_file_tree_height(ui.available_height(), footer_header_count);
        self.ensure_git_repositories_loaded();
        let git_file_statuses = workspace_git_file_statuses(&self.git.repositories);
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
                draw_workspace_roots(ui, &file_tree.roots, root_count, &git_file_statuses, self);
            });

        self.bookmarks_ui(ui);
    }

    fn bookmarks_ui(&mut self, ui: &mut egui::Ui) {
        let bookmark_id = ui.make_persistent_id("bookmarks");
        let bookmark_state = egui::collapsing_header::CollapsingState::load_with_default_open(
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
                            action = Some(BookmarkAction::Open(bookmark.clone()));
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("×").on_hover_text("Remove bookmark").clicked() {
                                action = Some(BookmarkAction::Remove(index));
                            }
                        });
                    });
                }

                if let Some(action) = action {
                    match action {
                        BookmarkAction::Open(bookmark) => self.open_bookmark(bookmark),
                        BookmarkAction::Remove(index) => self.remove_bookmark(index),
                    }
                }
            });
    }

    /// One editor pane plus what its frame asked for: focus on click, a pending action.
    fn pane_ui(
        &mut self,
        pane: Pane,
        ui: &mut egui::Ui,
        can_take_focus: bool,
        navigation_history: &[NavigationPoint],
        bookmark_lines: &[u32],
    ) {
        let focused = can_take_focus && self.focused_pane == pane;
        let editor = match pane {
            Pane::Left => &mut self.left,
            Pane::Right => &mut self.right,
        };
        editor.ui(
            ui,
            focused,
            &self.language_registry,
            self.tab_width,
            navigation_history,
            bookmark_lines,
        );
        if can_take_focus && ui_was_primary_clicked(ui) {
            self.workspace.sidebar_active = false;
            self.focused_pane = pane;
        }
        if let Some(action) = self.editor_mut(pane).take_pending_action() {
            self.focused_pane = pane;
            self.handle_editor_action(action);
        }
    }

    fn editor_area_ui(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).fill(ui.visuals().panel_fill))
            .show_inside(ui, |ui| {
                let editor_can_take_focus = !self.session_edit.editing_name;
                let left_navigation_history = self.navigation.left_history.back.clone();
                let left_bookmark_lines = self.bookmark_lines_for_path(self.left.path());
                let right_bookmark_lines = self.bookmark_lines_for_path(self.right.path());
                let dragging_workspace_file = self.workspace.dragging_file.clone();
                let mut right_drop = None;

                if self.layout.show_right_pane || dragging_workspace_file.is_some() {
                    ui.columns(2, |columns| {
                        let right_navigation_history = self.navigation.right_history.back.clone();
                        self.pane_ui(
                            Pane::Left,
                            &mut columns[0],
                            editor_can_take_focus,
                            &left_navigation_history,
                            &left_bookmark_lines,
                        );

                        if self.layout.show_right_pane {
                            self.pane_ui(
                                Pane::Right,
                                &mut columns[1],
                                editor_can_take_focus,
                                &right_navigation_history,
                                &right_bookmark_lines,
                            );
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
                    self.pane_ui(
                        Pane::Left,
                        ui,
                        editor_can_take_focus,
                        &left_navigation_history,
                        &left_bookmark_lines,
                    );
                }

                if let Some(path) = right_drop {
                    self.layout.show_right_pane = true;
                    self.focused_pane = Pane::Right;
                    self.workspace.dragging_file = None;
                    self.open_file_in_pane(Pane::Right, path);
                } else if self.workspace.dragging_file.is_some()
                    && ui.input(|input| input.pointer.any_released())
                {
                    self.workspace.dragging_file = None;
                }
            });
    }

    fn dialogs_ui(&mut self, ui: &mut egui::Ui) {
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
    }
}

/// Paints the status bar: the message on the left, the shortcut hints on the right.
fn status_bar_ui(ui: &mut egui::Ui, status_message: &str, status_shortcuts: &[ShortcutHint]) {
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
            status_message,
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
        ui.painter()
            .with_clip_rect(status_rect)
            .galley(status_pos, status_galley, status_color);
        ui.interact(
            status_rect,
            ui.id().with("status_bar_message"),
            egui::Sense::hover(),
        )
        .on_hover_text(status_message);

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
