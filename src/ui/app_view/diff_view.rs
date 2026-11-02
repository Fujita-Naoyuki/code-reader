//! The diff view: its header, selection, scrolling and pane state. `diff_rows.rs` paints
//! the rows.

use super::*;

impl EditorPane {
    pub(super) fn has_diff_text_selection(&self) -> bool {
        match self.diff.display_mode {
            DiffDisplayMode::Unified => self
                .diff
                .unified_selection
                .and_then(|selection| selection.normalized())
                .is_some(),
            DiffDisplayMode::SideBySide => self
                .diff
                .side_selection
                .and_then(|selection| selection.normalized())
                .is_some(),
        }
    }

    pub(super) fn diff_selection_text(&self) -> Option<String> {
        let rows = parse_diff_display_rows(&self.text);
        match self.diff.display_mode {
            DiffDisplayMode::Unified => {
                let selection = self.diff.unified_selection?;
                let render_rows = unified_diff_render_rows(&rows, &self.diff.collapsed_files);
                diff_unified_selection_text_from_render_rows(&render_rows, selection)
            }
            DiffDisplayMode::SideBySide => {
                let selection = self.diff.side_selection?;
                let render_rows = side_by_side_diff_render_rows(&rows, &self.diff.collapsed_files);
                diff_side_selection_text_from_render_rows(&render_rows, selection)
            }
        }
    }

    pub(super) fn reload_current_file_diff_context(&mut self, context_mode: GitDiffContextMode) {
        let Some(diff_view) = self.diff_view().cloned() else {
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
                self.scroll.y = 0.0;
                self.scroll.x = 0.0;
                self.diff.display_cache = None;
                self.diff.side_selection = None;
                self.diff.unified_selection = None;
                if let Some(diff_view) = self.diff_view_mut() {
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

    pub(super) fn diff_view_ui(&mut self, ui: &mut egui::Ui, focused: bool) {
        let return_history = self
            .diff_view()
            .and_then(|diff_view| diff_view.return_history.clone());
        let (back_to_history, next_context_mode) =
            self.diff_view_header_ui(ui, return_history.is_some());
        if let Some(context_mode) = next_context_mode {
            self.reload_current_file_diff_context(context_mode);
        }
        if back_to_history && let Some(return_history) = return_history {
            let pane = self.pane;
            let mut next = EditorPane::from_git_history(pane, return_history.view);
            next.scroll.y = return_history.scroll_y;
            next.scroll.x = return_history.scroll_x;
            *self = next;
            return;
        }

        let (source_len, source_hash) = self.text_fingerprint();
        let rows = self.cached_diff_display_rows(source_len, source_hash);
        let display_mode = self.diff.display_mode;
        let side_by_side = display_mode == DiffDisplayMode::SideBySide;
        if side_by_side {
            self.diff.unified_selection = None;
        } else {
            self.diff.side_selection = None;
        }
        let side_scrollbar_height = if side_by_side { 20.0 } else { 0.0 };
        let viewport_height = (ui.available_height() - side_scrollbar_height).max(0.0);
        if let Some(delta) = diff_page_scroll_delta(ui, focused, viewport_height) {
            self.scroll.y = (self.scroll.y + delta).max(0.0);
        }
        self.apply_immediate_wheel_scroll(ui, self.scroll.max_y);

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
        .vertical_scroll_offset(self.scroll.y);

        let side_scroll = DiffSideBySideScroll {
            left_x: self.diff.side_left_scroll_x,
            right_x: self.diff.side_right_scroll_x,
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
                DiffRowsState {
                    collapsed_files: &mut self.diff.collapsed_files,
                    side_selection: &mut self.diff.side_selection,
                    unified_selection: &mut self.diff.unified_selection,
                },
                self.scroll.y,
                side_scroll,
            )
        });
        self.finish_diff_frame(ui, &scroll_output, side_by_side);
    }

    /// Where the diff comes from, the display mode, and the Back / context buttons.
    /// Returns (back to history clicked, context mode picked).
    fn diff_view_header_ui(
        &mut self,
        ui: &mut egui::Ui,
        has_return_history: bool,
    ) -> (bool, Option<GitDiffContextMode>) {
        let mut back_to_history = false;
        let mut next_context_mode = None;
        if let EditorContent::Diff(diff_view) = &self.content {
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
                if has_return_history {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Back to History").clicked() {
                            back_to_history = true;
                        }
                    });
                }
            });
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut self.diff.display_mode,
                    DiffDisplayMode::Unified,
                    "Unified",
                );
                ui.selectable_value(
                    &mut self.diff.display_mode,
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
        (back_to_history, next_context_mode)
    }

    fn finish_diff_frame(
        &mut self,
        ui: &mut egui::Ui,
        scroll_output: &egui::scroll_area::ScrollAreaOutput<DiffRenderSummary>,
        side_by_side: bool,
    ) {
        if let Some(selection) = &mut self.diff.side_selection
            && !ui.input(|input| input.pointer.primary_down())
        {
            selection.dragging = false;
        }
        if let Some(selection) = &mut self.diff.unified_selection
            && !ui.input(|input| input.pointer.primary_down())
        {
            selection.dragging = false;
        }
        self.scroll.max_y =
            scroll_area_vertical_max_scroll_y(scroll_output.content_size, scroll_output.inner_rect);
        self.scroll.y = scroll_output.state.offset.y.clamp(0.0, self.scroll.max_y);
        if side_by_side {
            let viewport_width = scroll_output
                .inner_rect
                .intersect(ui.clip_rect())
                .width()
                .max(ui.available_width());
            diff_side_by_side_scrollbars_ui(
                ui,
                &mut self.diff.side_left_scroll_x,
                &mut self.diff.side_right_scroll_x,
                &scroll_output.inner,
                viewport_width,
            );
        }
        if self.diff_view().is_some_and(|diff_view| {
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

    pub(super) fn cached_diff_display_rows(
        &mut self,
        source_len: usize,
        source_hash: u64,
    ) -> Rc<Vec<DiffDisplayRow>> {
        if let Some(cache) = &self.diff.display_cache
            && cache.source_len == source_len
            && cache.source_hash == source_hash
        {
            return cache.rows.clone();
        }

        let rows = Rc::new(parse_diff_display_rows(&self.text));
        self.diff.display_cache = Some(DiffDisplayCache {
            source_len,
            source_hash,
            rows: rows.clone(),
        });
        rows
    }
}

#[derive(Debug, Clone)]
pub(super) struct DiffDisplayCache {
    pub(super) source_len: usize,
    pub(super) source_hash: u64,
    pub(super) rows: Rc<Vec<DiffDisplayRow>>,
}

/// State only the diff view uses.
pub(super) struct DiffPaneState {
    pub(super) side_left_scroll_x: f32,
    pub(super) side_right_scroll_x: f32,
    pub(super) display_mode: DiffDisplayMode,
    pub(super) collapsed_files: HashMap<String, bool>,
    pub(super) side_selection: Option<DiffSideSelection>,
    pub(super) unified_selection: Option<DiffUnifiedSelection>,
    pub(super) display_cache: Option<DiffDisplayCache>,
}

impl Default for DiffPaneState {
    fn default() -> Self {
        Self {
            side_left_scroll_x: 0.0,
            side_right_scroll_x: 0.0,
            display_mode: DiffDisplayMode::Unified,
            collapsed_files: HashMap::new(),
            side_selection: None,
            unified_selection: None,
            display_cache: None,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct GitDiffView {
    pub(super) title: String,
    pub(super) repository_name: String,
    pub(super) repository_root: Option<PathBuf>,
    pub(super) file_path: Option<String>,
    pub(super) scope: GitDiffScope,
    pub(super) context_mode: GitDiffContextMode,
    pub(super) untracked: bool,
    pub(super) return_history: Option<GitHistoryReturnState>,
}

#[derive(Debug, Clone)]
pub(super) struct GitHistoryReturnState {
    pub(super) view: GitHistoryView,
    pub(super) scroll_y: f32,
    pub(super) scroll_x: f32,
}

pub(super) fn diff_view_can_switch_context(diff_view: &GitDiffView) -> bool {
    diff_view.file_path.is_some()
        && diff_view.repository_root.is_some()
        && !diff_view.untracked
        && matches!(
            diff_view.scope,
            GitDiffScope::Worktree | GitDiffScope::Staged
        )
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct DiffSideBySideScroll {
    pub(super) left_x: f32,
    pub(super) right_x: f32,
}
