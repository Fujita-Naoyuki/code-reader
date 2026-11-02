//! `EditorPane`: one side of the editor area, its file, and the mode-independent parts
//! (constructing, saving, unsaved-change tracking, dispatching to the view for its mode).

use super::*;

impl EditorPane {
    pub(super) fn empty(pane: Pane) -> Self {
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

    pub(super) fn from_path(pane: Pane, path: PathBuf) -> Self {
        let mut editor = Self::read_path(pane, path);
        editor.saved_fingerprint = editor.text_fingerprint();
        editor
    }

    pub(super) fn read_path(pane: Pane, path: PathBuf) -> Self {
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

    pub(super) fn from_diff(pane: Pane, diff_view: GitDiffView, text: String) -> Self {
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

    pub(super) fn from_git_history(pane: Pane, history_view: GitHistoryView) -> Self {
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

    pub(super) fn navigation_point(&self) -> Option<NavigationPoint> {
        Some(NavigationPoint {
            path: self.path.clone()?,
            position: self.cursor_position.unwrap_or(Position::new(0, 0)),
        })
    }

    pub(super) fn take_pending_action(&mut self) -> Option<EditorAction> {
        self.pending_action.take()
    }

    pub(super) fn text_fingerprint(&self) -> (usize, u64) {
        self.text.fingerprint()
    }

    /// An editable file whose text differs from what was last read or saved.
    pub(super) fn has_unsaved_changes(&self) -> bool {
        self.path.is_some()
            && !self.read_only
            && self.diff_view.is_none()
            && self.git_history_view.is_none()
            && self.text_fingerprint() != self.saved_fingerprint
    }

    /// Takes over `other`'s unsaved text when `self` is a fresh copy of the same file, so
    /// moving within that file (definition jump, Back/Forward) keeps the edits.
    pub(super) fn adopt_unsaved_text(&mut self, other: &mut EditorPane) {
        self.text = std::mem::replace(&mut other.text, EditorTextBuffer::new(String::new()));
        self.saved_fingerprint = other.saved_fingerprint;
        self.mark_text_changed();
    }

    pub(super) fn mark_text_changed(&mut self) {
        self.text.refresh_metadata();
        self.syntax_highlight_pending = None;
        self.syntax_highlight_receiver = None;
        self.virtual_line_layout_cache = None;
        self.virtual_line_number_layout_cache = None;
    }

    pub(super) fn replace_text(&mut self, text: String) {
        self.text.replace(text);
        self.syntax_highlight_pending = None;
        self.syntax_highlight_receiver = None;
        self.virtual_line_layout_cache = None;
        self.virtual_line_number_layout_cache = None;
        self.virtual_undo_stack.clear();
        self.virtual_redo_stack.clear();
    }

    pub(super) fn save_to_disk(&mut self) -> Result<PathBuf, String> {
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

    pub(super) fn ui(
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

pub(super) const DEFAULT_TAB_WIDTH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PendingEditorNavigation {
    pub(super) position: Position,
    pub(super) focus_editor: bool,
}

pub(super) struct EditorPane {
    pub(super) pane: Pane,
    pub(super) title: String,
    pub(super) path: Option<PathBuf>,
    pub(super) text: EditorTextBuffer,
    pub(super) read_only: bool,
    pub(super) cursor_position: Option<Position>,
    pub(super) target_position: Option<Position>,
    pub(super) pending_navigation: Option<PendingEditorNavigation>,
    pub(super) virtual_selection: Option<VirtualTextSelection>,
    pub(super) virtual_ime_preedit: Option<String>,
    pub(super) virtual_undo_stack: Vec<VirtualEditHistoryEntry>,
    pub(super) virtual_redo_stack: Vec<VirtualEditHistoryEntry>,
    /// Wheel scrolling the large-file editor applied immediately but egui is still going to
    /// deliver as smoothed deltas on later frames; those are skipped up to this amount.
    pub(super) virtual_wheel_lead: egui::Vec2,
    /// Where Up/Down last left the cursor and the column it was aiming for, so passing
    /// through shorter lines does not lose the column (only while the cursor stays there).
    pub(super) virtual_vertical_goal: Option<(Position, u32)>,
    pub(super) scroll_y: f32,
    pub(super) scroll_max_y: f32,
    pub(super) scroll_x: f32,
    pub(super) diff_side_left_scroll_x: f32,
    pub(super) diff_side_right_scroll_x: f32,
    pub(super) selected_csv_cell: Option<CsvCellPosition>,
    pub(super) csv_selection: Option<CsvSelection>,
    pub(super) csv_drag: Option<CsvSelectionDrag>,
    pub(super) csv_debug_message: Option<String>,
    pub(super) csv_mode: CsvMode,
    pub(super) csv_undo_stack: Vec<CsvEditHistoryEntry>,
    pub(super) csv_redo_stack: Vec<CsvEditHistoryEntry>,
    pub(super) csv_edit_history_cell: Option<CsvCellPosition>,
    pub(super) markdown_mode: MarkdownMode,
    pub(super) markdown_active_page_id: Option<String>,
    pub(super) markdown_active_heading_id: Option<String>,
    pub(super) markdown_focus_target: MarkdownFocusTarget,
    pub(super) markdown_show_pages: bool,
    pub(super) markdown_pages_width: f32,
    pub(super) markdown_pages_resize_start_width: Option<f32>,
    pub(super) markdown_show_outline: bool,
    pub(super) markdown_search: MarkdownSearchState,
    pub(super) source_search: SourceSearchState,
    pub(super) markdown_document_cache: Option<MarkdownDocumentCache>,
    pub(super) syntax_highlight_cache: Option<SyntaxHighlightCache>,
    pub(super) syntax_highlight_pending: Option<SyntaxHighlightPending>,
    pub(super) syntax_highlight_receiver: Option<Receiver<SyntaxHighlightResult>>,
    pub(super) editor_metrics_cache: Option<EditorMetricsCache>,
    pub(super) virtual_line_layout_cache: Option<VirtualLineLayoutCache>,
    pub(super) virtual_line_number_layout_cache: Option<VirtualLineNumberLayoutCache>,
    pub(super) editor_render_debug: Option<EditorRenderDebug>,
    pub(super) editor_perf_debug: Option<EditorPerfDebug>,
    pub(super) virtual_click_debug: VirtualClickDebug,
    pub(super) bracket_pair_cache: Option<BracketPairCache>,
    pub(super) source_search_cache: Option<SourceSearchCache>,
    pub(super) markdown_image_cache: HashMap<PathBuf, Result<egui::TextureHandle, String>>,
    pub(super) markdown_mermaid_cache: HashMap<String, Result<String, String>>,
    pub(super) markdown_pending_footnote_scroll: Option<MarkdownFootnoteScrollTarget>,
    pub(super) diff_view: Option<GitDiffView>,
    pub(super) git_history_view: Option<GitHistoryView>,
    pub(super) diff_display_mode: DiffDisplayMode,
    pub(super) diff_collapsed_files: HashMap<String, bool>,
    pub(super) diff_side_selection: Option<DiffSideSelection>,
    pub(super) diff_unified_selection: Option<DiffUnifiedSelection>,
    pub(super) diff_display_cache: Option<DiffDisplayCache>,
    pub(super) pending_action: Option<EditorAction>,
    /// Text fingerprint when the file was last read or saved; see `has_unsaved_changes`.
    pub(super) saved_fingerprint: (usize, u64),
}

pub(super) fn editor_pane_from_session_path(
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum EditorAction {
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
pub(super) enum BookmarkAction {
    Open(SessionBookmark),
    Remove(usize),
}
